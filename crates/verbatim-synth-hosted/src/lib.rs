//! `HostedSynth`: a synthesizer running in a synthesizer host process
//! (decision D18), seen by the speech manager as an ordinary
//! [`SynthDriver`].
//!
//! Each `HostedSynth` owns one `verbatim-synth-host.exe`, launched contained
//! in a kill-on-close job over two overlapped pipes, and talks to it with
//! the protocol in [`verbatim_speech::hosting`], each message a frame of
//! the transport ([`verbatim_process::transport`]). The host lives exactly
//! as long as the driver: switching synthesizer drops the driver, which
//! ends the process.
//!
//! The synth thread reads the host's pipe itself
//! (`docs/design/communication.md`, sections 7.2 and 7.3). While it waits
//! for the host it waits on the pipe's read and on its utterance's cancel
//! together, so a cancel is written to the host at once. Having written
//! it, the driver returns: it does not wait for the host to end the
//! utterance. It remembers the utterance as draining, and whenever it next
//! reads from the host, for the next utterance or for a setting's answer,
//! it drops every frame of a draining utterance, its `Done` or `Failed`
//! included, unread by anyone. So the next utterance's `Speak` goes right
//! behind the `Cancel`, and the host reads both at its next chunk.
//!
//! Recovery. A host that exits, or answers out of turn, fails the request
//! in flight and is ended; so does a hung one, which has owed the driver a
//! message for [`HANG_TIMEOUT`] and sent nothing. It owes one from the
//! moment a request is written to it while it owes nothing until it has
//! ended that request and every utterance draining. The time counts from
//! the latest of that moment, the last frame read from it, and the sink
//! handing back the last message relayed, so time the sink spends blocked
//! while playback is behind never counts, but the time an utterance
//! drains does: a host frozen while its utterances keep being cancelled,
//! each before the limit, is still found, at the next request or in the
//! next wait, and never by a timer of its own. The next request starts a
//! new host and gives it the settings the old one had. A synthesis error
//! the host reports in turn fails only that utterance, and the host
//! carries on. A host that dies while an utterance drains is found at the
//! next request.
//!
//! Commands are written whole before the driver goes on: the host keeps a
//! read of its command pipe in flight at all times, even while it waits
//! to write audio, so a write waits only on a host that is frozen outright,
//! and then only until the host is judged hung. That write is never
//! cancelled: the host is ended with the write still in flight, and the
//! write ends as the pipe closes.
//!
//! Backpressure crosses the process boundary unchanged: the driver reads a
//! frame only once the sink has taken the one before, and the pipe the
//! host writes to has a small buffer, so the host's synthesizer waits when
//! the audio device is behind.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::warn;
use verbatim_model::UtteranceId;
use verbatim_process::event::{Event, wait_any};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::{EventReader, EventWriter, Taken, put_frame};
use verbatim_process::{ChildEnds, ChildSpec, Contained};
use verbatim_speech::hosting::{
    FromHost, HostDescription, HostTiming, ToHost, decode_from_host, encode_to_host, now_us,
};
use verbatim_speech::{
    SettingDescriptor, SettingId, SettingValue, SpeechSequence, SynthDriver, SynthError,
    SynthFactory, SynthId, SynthSink, SynthTiming,
};

/// How long a host may stay silent while it owes the driver a message (its
/// `Ready`, a request's answer, or a cancelled utterance's end) before it
/// is judged hung and ended. A synthesizer produces audio many times
/// faster than real time, so ten seconds without a message is never
/// normal.
pub const HANG_TIMEOUT: Duration = Duration::from_secs(10);

/// The buffer of the pipe the host writes PCM to, and how much a read of
/// it asks for while a frame's length is unknown: about 90 ms of 22 kHz
/// mono each, so the host cannot run far ahead of playback.
const PIPE_BUFFER: u32 = 4_096;

/// A synthesizer in a host process.
pub struct HostedSynth {
    exe: PathBuf,
    synth: SynthId,
    description: HostDescription,
    host: Option<Host>,
}

/// One running host process.
struct Host {
    commands: EventWriter,
    replies: EventReader,
    /// Messages read and not yet handled, in order.
    read: VecDeque<FromHost>,
    /// Why the reading of the host's pipe ended, once it has.
    ended: Option<String>,
    /// Utterances cancelled whose `Done` or `Failed` has not been read:
    /// every frame of theirs is dropped as it is read.
    draining: VecDeque<UtteranceId>,
    /// Since when the host has owed a message and sent none: the latest of
    /// the last frame read from it, the sink handing back after the last
    /// message relayed, and the request written while it owed nothing. It
    /// is judged hung [`HANG_TIMEOUT`] after this.
    silent_since: Instant,
    /// Ends the process when dropped.
    contained: Contained,
}

/// What a wait for the host brought.
enum Next {
    /// A message, not of a draining utterance.
    Message(FromHost),
    /// The utterance's cancel woke the wait.
    Cancelled,
}

/// Why a host can no longer be used.
enum Lost {
    /// Its pipe ended or failed: the process ended, or was ending.
    Ended(String),
    /// It owed a message for [`HANG_TIMEOUT`] and sent nothing.
    Hung,
    /// It sent something out of turn.
    OutOfTurn(String),
}

impl Lost {
    /// The host hung or misbehaved, rather than ended.
    fn misbehaved(&self) -> bool {
        !matches!(self, Self::Ended(_))
    }

    fn error(&self) -> SynthError {
        SynthError::Synthesis(match self {
            Self::Ended(why) => format!("the synthesizer host ended: {why}"),
            Self::Hung => format!("the synthesizer host sent nothing for {HANG_TIMEOUT:?}"),
            Self::OutOfTurn(what) => format!("the synthesizer host sent {what} out of turn"),
        })
    }
}

impl HostedSynth {
    /// Starts a host for synthesizer `synth` from the host executable at
    /// `exe`, and waits for it to describe the synthesizer.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when the host cannot be started
    /// or its synthesizer cannot.
    pub fn start(exe: PathBuf, synth: SynthId) -> Result<Self, SynthError> {
        let (host, description) = Host::start(&exe, &synth)?;
        Ok(Self {
            exe,
            synth,
            description,
            host: Some(host),
        })
    }

    /// The host process's id, or `None` while no host is running (after one
    /// ended, until the next request starts another).
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.host.as_ref().map(|host| host.contained.pid)
    }

    /// The running host, starting a new one with the current settings if
    /// the last one ended. A new host that fails while its settings are
    /// restored is ended again, so no reply of its can reach a later
    /// request; a setting it refuses (a voice since uninstalled) is only
    /// logged.
    fn host(&mut self) -> Result<&mut Host, SynthError> {
        // Between requests a host sends only the last frames of utterances
        // that drain, so anything else already read from it is its end or
        // a message out of turn: either way it is replaced now, before the
        // next request is lost to it. A host whose process has exited is
        // replaced too, whether or not its pipe's end has been read yet.
        // One whose pipe ends only as the request is made fails the
        // request before anything is relayed, which `speak` sends again.
        if let Some(host) = &mut self.host
            && let Err(lost) = host.between_requests()
        {
            self.lose_host(&lost.error());
        } else if self
            .host
            .as_ref()
            .is_some_and(|host| host.contained.has_exited())
        {
            self.lose_host(&SynthError::Synthesis(
                "the synthesizer host ended while idle".to_owned(),
            ));
        }
        if self.host.is_none() {
            warn!(target: "verbatim::speech", synth = %self.synth, "starting the synthesizer host again");
            let (mut host, _) = Host::start(&self.exe, &self.synth)?;
            for (id, value) in self.description.values.clone() {
                match host.set_setting(&id, value) {
                    Ok(()) => {}
                    Err(SynthError::Setting(refusal)) => {
                        warn!(target: "verbatim::speech", %id, %refusal, "the new synthesizer host refused a setting");
                    }
                    Err(error) => return Err(error),
                }
            }
            self.host = Some(host);
        }
        Ok(self.host.as_mut().expect("a host was just started"))
    }

    /// Ends the host after `error`, so the next request starts a new one.
    fn lose_host(&mut self, error: &SynthError) {
        warn!(target: "verbatim::speech", synth = %self.synth, %error, "ending the synthesizer host");
        self.host = None;
    }
}

impl Host {
    fn start(
        exe: &std::path::Path,
        synth: &SynthId,
    ) -> Result<(Self, HostDescription), SynthError> {
        let unavailable = |error: io::Error| {
            SynthError::Unavailable(format!(
                "start the synthesizer host {}: {error}",
                exe.display()
            ))
        };
        let (to_host, commands_end) =
            pipe(Flow::Outbound, 0, End::Overlapped).map_err(unavailable)?;
        let (from_host, messages_end) =
            pipe(Flow::Inbound, PIPE_BUFFER, End::Overlapped).map_err(unavailable)?;
        let synth_name = synth.0.clone();
        let arguments = move |pipe_in: usize, pipe_out: usize| {
            format!("--pipe-in {pipe_in} --pipe-out {pipe_out} --synth {synth_name}")
        };
        let contained = verbatim_process::launch_with_ends(
            &ChildSpec {
                exe,
                arguments: &arguments,
                log_stem: &format!("synth-{}", synth.0),
                memory_cap: None,
            },
            ChildEnds {
                commands: commands_end,
                messages: messages_end,
            },
        )
        .map_err(unavailable)?;
        let mut host = Self {
            commands: EventWriter::new(to_host).map_err(unavailable)?,
            replies: EventReader::new(from_host, PIPE_BUFFER as usize).map_err(unavailable)?,
            read: VecDeque::new(),
            ended: None,
            draining: VecDeque::new(),
            silent_since: Instant::now(),
            contained,
        };
        match host.next(None) {
            Ok(Next::Message(FromHost::Ready(description))) => Ok((host, description)),
            Ok(Next::Message(FromHost::Unavailable(reason))) => {
                Err(SynthError::Unavailable(reason))
            }
            Ok(Next::Message(message)) => Err(SynthError::Unavailable(format!(
                "the synthesizer host began with {message:?}"
            ))),
            Ok(Next::Cancelled) => unreachable!("no cancel is watched"),
            Err(lost) => Err(SynthError::Unavailable(lost.error().to_string())),
        }
    }

    /// Writes `message` to the host, whole, returning the shared clock's
    /// stamp taken as the write began. A write the host has not taken by
    /// the time it is judged hung is left in flight, never cancelled, and
    /// the host is lost.
    fn send(&mut self, message: &ToHost) -> Result<u64, Lost> {
        // A request written to a host that owes nothing starts its time; a
        // cancel is written only while its utterance is owed.
        if self.draining.is_empty() && !matches!(message, ToHost::Cancel(_)) {
            self.silent_since = Instant::now();
        }
        let deadline = self.hung_at();
        let commands = &mut self.commands;
        stamped(now_us, || {
            write_by(commands, deadline, |buffer| {
                put_frame(buffer, |body| encode_to_host(message, body))
            })
        })
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                Lost::Hung
            } else {
                Lost::Ended(error.to_string())
            }
        })
    }

    /// When the host is judged hung if it sends nothing before then.
    fn hung_at(&self) -> Instant {
        self.silent_since + HANG_TIMEOUT
    }

    /// The sink has handed back after taking a message: the time it took,
    /// blocked while playback is behind, is never counted against the
    /// host.
    fn handed_back(&mut self) {
        self.silent_since = Instant::now();
    }

    /// Cancels `utterance`, whose `Speak` was written, and notes it as
    /// draining.
    fn cancel(&mut self, utterance: UtteranceId) -> Result<(), Lost> {
        self.send(&ToHost::Cancel(utterance))?;
        self.draining.push_back(utterance);
        Ok(())
    }

    /// Reads once without waiting, if the reading has not ended: whether a
    /// read is now in flight with nothing more to hand out.
    fn fill(&mut self) -> bool {
        if self.ended.is_some() {
            return false;
        }
        let Self { replies, read, .. } = self;
        let taken = replies.take(|body| {
            read.push_back(decode_from_host(body)?);
            Ok(())
        });
        match taken {
            Taken::Frames => {
                self.silent_since = Instant::now();
                false
            }
            Taken::Waiting => true,
            Taken::Ended(Ok(())) => {
                self.ended = Some("its pipe closed".to_owned());
                false
            }
            Taken::Ended(Err(error)) => {
                self.ended = Some(error.to_string());
                false
            }
        }
    }

    /// The next message read that is not of a draining utterance, dropping
    /// those, waiting for one until the host is judged hung; or, when
    /// `cancel` is given, its being set, the utterance's cancel.
    fn next(&mut self, cancel: Option<&Event>) -> Result<Next, Lost> {
        loop {
            while let Some(message) = self.read.pop_front() {
                if let Some(utterance) = message.utterance()
                    && let Some(at) = self.draining.iter().position(|d| *d == utterance)
                {
                    if message.ends_utterance() {
                        self.draining.remove(at);
                    }
                    continue;
                }
                return Ok(Next::Message(message));
            }
            if let Some(why) = &self.ended {
                return Err(Lost::Ended(why.clone()));
            }
            if !self.fill() {
                continue;
            }
            let left = self.hung_at().saturating_duration_since(Instant::now());
            let signalled = match cancel {
                Some(cancel) => wait_any(&[self.replies.event(), cancel], Some(left)),
                None => wait_any(&[self.replies.event()], Some(left)),
            };
            match signalled {
                Ok(Some(0)) => {}
                Ok(Some(_)) => return Ok(Next::Cancelled),
                Ok(None) => return Err(Lost::Hung),
                Err(error) => return Err(Lost::Ended(error.to_string())),
            }
        }
    }

    /// Looks over what the host has sent and not yet been handled, as a
    /// request is about to be made, reading what has come without waiting:
    /// only draining utterances' frames may be among it, and those are
    /// dropped. Nothing is read between requests, which leaves a host
    /// blocked in a paced write of a cancelled utterance waiting, at no
    /// cost, until the next one. A host still owing a draining utterance's
    /// end that has sent nothing for [`HANG_TIMEOUT`] is hung, though no
    /// wait for it ran that long: its utterances were cancelled, each
    /// before the limit.
    fn between_requests(&mut self) -> Result<(), Lost> {
        loop {
            if let Some(message) = self.read.front() {
                let draining = message
                    .utterance()
                    .and_then(|utterance| self.draining.iter().position(|d| *d == utterance));
                let Some(at) = draining else {
                    return Err(Lost::OutOfTurn(format!("{message:?}")));
                };
                if message.ends_utterance() {
                    self.draining.remove(at);
                }
                self.read.pop_front();
                continue;
            }
            if let Some(why) = &self.ended {
                return Err(Lost::Ended(why.clone()));
            }
            if self.fill() {
                break;
            }
        }
        if !self.draining.is_empty() && Instant::now() >= self.hung_at() {
            return Err(Lost::Hung);
        }
        Ok(())
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        let answer = self
            .send(&ToHost::SetSetting {
                id: id.clone(),
                value,
            })
            .and_then(|_| self.next(None));
        match answer {
            Ok(Next::Message(FromHost::SettingApplied(None))) => Ok(()),
            Ok(Next::Message(FromHost::SettingApplied(Some(refusal)))) => {
                Err(SynthError::Setting(refusal))
            }
            Ok(Next::Message(message)) => Err(SynthError::Synthesis(format!(
                "the synthesizer host answered a setting with {message:?}"
            ))),
            Ok(Next::Cancelled) => unreachable!("no cancel is watched"),
            Err(lost) => Err(lost.error()),
        }
    }
}

impl SynthDriver for HostedSynth {
    fn id(&self) -> SynthId {
        self.synth.clone()
    }

    fn display_name(&self) -> String {
        self.description.display_name.clone()
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        self.description.settings.clone()
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        self.description
            .values
            .iter()
            .find(|(setting, _)| setting == id)
            .map(|(_, value)| value.clone())
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        let result = self.host()?.set_setting(id, value.clone());
        match &result {
            Ok(()) => {
                match self
                    .description
                    .values
                    .iter_mut()
                    .find(|(setting, _)| setting == id)
                {
                    Some(entry) => entry.1 = value,
                    None => self.description.values.push((id.clone(), value)),
                }
            }
            Err(SynthError::Setting(_)) => {}
            Err(error) => self.lose_host(&error.clone()),
        }
        result
    }

    fn places_marks(&self) -> bool {
        self.description.places_marks
    }

    fn changes_pitch(&self) -> bool {
        self.description.changes_pitch
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        // A host can die at any moment, and ending a process is not
        // immediate, so a request can reach a host that is already dying.
        // If its pipe ended before any of the utterance was relayed,
        // nothing was heard, so it is sent once more to a fresh host. An
        // utterance cut off part-way fails, and so does one whose host hung
        // or answered out of turn, which a second host would likely repeat.
        let mut relayed = false;
        for round in 0..2 {
            let host = self.host()?;
            match speak_with(host, sequence, sink, &mut relayed) {
                Ok(outcome) => return outcome,
                Err(lost) => {
                    let error = lost.error();
                    self.lose_host(&error);
                    if relayed || lost.misbehaved() || round == 1 || sink.is_cancelled() {
                        return Err(error);
                    }
                }
            }
        }
        unreachable!("the second attempt always returns")
    }
}

/// Runs one utterance on `host`, relaying its audio and marks to `sink`,
/// until the host ends it, or until it is cancelled, which is written to
/// the host and returns at once. The outer error means the host is no
/// longer usable; the inner result is the utterance's own outcome, which
/// the host reported in turn. `relayed` records whether anything was
/// passed to the sink.
fn speak_with(
    host: &mut Host,
    sequence: &SpeechSequence,
    sink: &mut dyn SynthSink,
    relayed: &mut bool,
) -> Result<Result<(), SynthError>, Lost> {
    let utterance = sequence.utterance;
    let asked_at_us = now_us();
    let sent_at_us = host.send(&ToHost::Speak(sequence.clone()))?;
    // The cancel sets this utterance's own event, which the wait below
    // watches with the pipe, so the host is told at once, even before it
    // has sent any audio, and nothing wakes at intervals to look. A waker
    // of an earlier utterance, called late, sets that utterance's event.
    // It is registered once the request is written.
    let cancelled =
        Arc::new(Event::new().map_err(|error| Lost::Ended(format!("create an event: {error}")))?);
    let wake = Arc::clone(&cancelled);
    sink.on_cancel(Box::new(move || wake.set()));
    loop {
        // Checked at each wake, which is the cancel's own or a reply's, so a
        // sink that keeps no waker is still heard at its next reply.
        if sink.is_cancelled() {
            host.cancel(utterance)?;
            return Ok(Ok(()));
        }
        let message = match host.next(Some(&cancelled))? {
            Next::Message(message) => message,
            Next::Cancelled => continue,
        };
        if message.utterance() != Some(utterance) {
            return Err(Lost::OutOfTurn(format!("{message:?}")));
        }
        match message {
            FromHost::FirstAudio(_, timing) => {
                let taken_at_us = now_us();
                sink.synth_timing(split_synthesis(
                    &DriverStamps {
                        asked: asked_at_us,
                        sent: sent_at_us,
                        taken: taken_at_us,
                    },
                    timing,
                ));
            }
            FromHost::Pcm(_, format, samples) => {
                *relayed = true;
                if sink.push_pcm(format, &samples).is_break() {
                    host.cancel(utterance)?;
                    return Ok(Ok(()));
                }
            }
            FromHost::Mark(_, mark) => {
                *relayed = true;
                sink.index_reached(mark);
            }
            FromHost::Done(_) => return Ok(Ok(())),
            FromHost::Failed(_, reason) => return Ok(Err(SynthError::Synthesis(reason))),
            other => return Err(Lost::OutOfTurn(format!("{other:?}"))),
        }
        host.handed_back();
    }
}

/// Writes the frames `put` appends with `writer`, waiting for the write to
/// finish until `deadline` at the latest. A write still in flight then is
/// left so, never cancelled, and [`io::ErrorKind::TimedOut`] returned: the
/// caller ends the host, whose pipe closes and ends the write with it.
fn write_by(
    writer: &mut EventWriter,
    deadline: Instant,
    put: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    if writer.start(put)? {
        return Ok(());
    }
    loop {
        if let Some(done) = writer.poll() {
            return done;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if wait_any(&[writer.event()], Some(left))?.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the synthesizer host took no command",
            ));
        }
    }
}

/// Runs `write`, returning `now`'s stamp taken just before it begins, so
/// the write's own time falls in the hop to the host
/// (`docs/design/communication.md`, section 10.1).
fn stamped(now: impl FnOnce() -> u64, write: impl FnOnce() -> io::Result<()>) -> io::Result<u64> {
    let sent_at_us = now();
    write()?;
    Ok(sent_at_us)
}

/// The driver's stamps on an utterance's start, in microseconds on the
/// clock every process shares.
struct DriverStamps {
    /// The synth thread asked to speak.
    asked: u64,
    /// The `Speak` request's write began.
    sent: u64,
    /// The synth thread handled the host's first audio.
    taken: u64,
}

/// Splits a hosted synthesizer's start into its stages from the driver's
/// stamps and the host's (`docs/design/communication.md`, section 10.1).
fn split_synthesis(driver: &DriverStamps, host: HostTiming) -> SynthTiming {
    SynthTiming {
        request_us: driver.sent.saturating_sub(driver.asked),
        to_host_us: host.received_at_us.saturating_sub(driver.sent),
        first_chunk_us: host.first_chunk_us,
        return_us: driver.taken.saturating_sub(host.sent_at_us),
    }
}

/// A [`SynthFactory`] that starts a host for `synth` from `exe`.
pub fn factory(exe: PathBuf, synth: SynthId) -> SynthFactory {
    Box::new(move || {
        Ok(Box::new(HostedSynth::start(exe.clone(), synth.clone())?) as Box<dyn SynthDriver>)
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// The split of a start asked for at 900, whose request's write began
    /// at 1,000 and took 300 microseconds, was read by the host at 1,450,
    /// and whose first audio the host wrote at 5,500 and the synth thread
    /// handled at 6,050.
    fn split() -> SynthTiming {
        let clock = Cell::new(1_000);
        let sent = stamped(
            || clock.get(),
            || {
                clock.set(clock.get() + 300);
                Ok(())
            },
        )
        .expect("the pipe takes every write");
        split_synthesis(
            &DriverStamps {
                asked: 900,
                sent,
                taken: 6_050,
            },
            HostTiming {
                received_at_us: 1_450,
                first_chunk_us: 4_000,
                sent_at_us: 5_500,
            },
        )
    }

    #[test]
    fn the_request_runs_from_the_ask_to_the_write_beginning() {
        assert_eq!(split().request_us, 100);
    }

    #[test]
    fn the_trip_to_the_host_runs_from_the_write_beginning_to_the_hosts_read() {
        assert_eq!(split().to_host_us, 450);
    }

    #[test]
    fn the_trip_back_runs_from_the_hosts_write_to_the_synth_threads_handling() {
        assert_eq!(split().return_us, 550);
    }
}
