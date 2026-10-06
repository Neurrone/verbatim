//! `HostedSynth`: a synthesizer running in a synthesizer host process
//! (decision D18), seen by the speech manager as an ordinary
//! [`SynthDriver`].
//!
//! Each `HostedSynth` owns one `verbatim-synth-host.exe`, launched contained
//! in a kill-on-close job, and talks to it with the protocol in
//! [`verbatim_speech::hosting`]. The host lives exactly as long as the
//! driver: switching synthesizer drops the driver, which ends the process.
//!
//! Recovery. A host that exits, sends nothing for [`HANG_TIMEOUT`] while
//! speaking, or answers out of turn, fails the request in flight and is
//! ended; the next request starts a new host and gives it the settings the
//! old one had. A synthesis error the host reports in turn fails only that
//! utterance, and the host carries on.
//!
//! Commands are written without a timeout: the host's reader thread drains
//! its command pipe at all times, so a write waits only on a host that is
//! frozen outright.
//!
//! Backpressure crosses the process boundary unchanged: PCM is read from
//! the host only as fast as the sink accepts it, through a two-message
//! channel and a small pipe buffer, so the host's synthesizer waits when
//! the audio device is behind.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, bounded};
use tracing::warn;
use verbatim_process::{ChildSpec, Contained};
use verbatim_speech::hosting::{FromHost, HostDescription, ToHost, read_from_host, write_to_host};
use verbatim_speech::{
    SettingDescriptor, SettingId, SettingValue, SpeechSequence, SynthDriver, SynthError,
    SynthFactory, SynthId, SynthSink,
};

/// How long a host may stay silent while starting or speaking before it is
/// judged hung and ended. A synthesizer produces audio many times faster
/// than real time, so ten seconds without a message is never normal.
pub const HANG_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a driver waiting for its host checks whether the utterance
/// was cancelled, so a cancel reaches a synthesizer that has not produced
/// audio yet.
const CANCEL_CHECK: Duration = Duration::from_millis(10);

/// The buffer asked for on the pipe the host writes PCM to: about 90 ms of
/// 22 kHz mono, so the host cannot run far ahead of playback.
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
    /// Ends the process when dropped.
    contained: Contained,
    commands: BufWriter<File>,
    replies: Receiver<io::Result<FromHost>>,
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
        // Between requests a host sends nothing, so anything waiting from it
        // is its end (the reader queues the pipe's end of stream when the
        // process dies) or a message out of turn: either way it is replaced
        // now, before the next request is lost to it.
        if self
            .host
            .as_ref()
            .is_some_and(|host| !host.replies.is_empty())
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
        let synth_name = synth.0.clone();
        let arguments = move |pipe_in: usize, pipe_out: usize| {
            format!("--pipe-in {pipe_in} --pipe-out {pipe_out} --synth {synth_name}")
        };
        let (contained, pipes) = verbatim_process::launch(&ChildSpec {
            exe,
            arguments: &arguments,
            log_stem: &format!("synth-{}", synth.0),
            memory_cap: None,
            from_child_buffer: PIPE_BUFFER,
        })
        .map_err(|error| {
            SynthError::Unavailable(format!(
                "start the synthesizer host {}: {error}",
                exe.display()
            ))
        })?;
        // Two messages of slack: the reader thread blocks once they are
        // waiting, so the pipe fills and the host's writes wait.
        let (reply_tx, replies) = bounded(2);
        let mut from_host = BufReader::new(pipes.from_child);
        std::thread::Builder::new()
            .name(format!("verbatim-synth-host-{}", synth.0))
            .spawn(move || {
                loop {
                    let reply = match read_from_host(&mut from_host) {
                        Ok(Some(reply)) => Ok(reply),
                        Ok(None) => Err(io::ErrorKind::UnexpectedEof.into()),
                        Err(error) => Err(error),
                    };
                    let ended = reply.is_err();
                    if reply_tx.send(reply).is_err() || ended {
                        return;
                    }
                }
            })
            .map_err(|error| SynthError::Unavailable(format!("start the host reader: {error}")))?;
        let host = Self {
            contained,
            commands: BufWriter::new(pipes.to_child),
            replies,
        };
        match host.receive() {
            Ok(FromHost::Ready(description)) => Ok((host, description)),
            Ok(FromHost::Unavailable(reason)) => Err(SynthError::Unavailable(reason)),
            Ok(other) => Err(SynthError::Unavailable(format!(
                "the synthesizer host began with {other:?}"
            ))),
            Err(error) => Err(SynthError::Unavailable(error.to_string())),
        }
    }

    fn send(&mut self, message: &ToHost) -> Result<(), SynthError> {
        write_to_host(&mut self.commands, message).map_err(|error| {
            SynthError::Synthesis(format!("the synthesizer host is gone: {error}"))
        })
    }

    /// The host's next message, or an error if it ended or hung.
    fn receive(&self) -> Result<FromHost, SynthError> {
        match self.replies.recv_timeout(HANG_TIMEOUT) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error)) => Err(SynthError::Synthesis(format!(
                "the synthesizer host ended: {error}"
            ))),
            Err(RecvTimeoutError::Timeout) => Err(SynthError::Synthesis(format!(
                "the synthesizer host sent nothing for {HANG_TIMEOUT:?}"
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(SynthError::Synthesis(
                "the synthesizer host ended".to_owned(),
            )),
        }
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        self.send(&ToHost::SetSetting {
            id: id.clone(),
            value,
        })?;
        match self.receive()? {
            FromHost::SettingApplied(None) => Ok(()),
            FromHost::SettingApplied(Some(refusal)) => Err(SynthError::Setting(refusal)),
            other => Err(SynthError::Synthesis(format!(
                "the synthesizer host answered a setting with {other:?}"
            ))),
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
        let mut attempt = Attempt::default();
        for round in 0..2 {
            let host = self.host()?;
            match speak_with(host, sequence, sink, &mut attempt) {
                Ok(outcome) => return outcome,
                Err(error) => {
                    self.lose_host(&error);
                    if attempt.relayed || attempt.misbehaved || round == 1 || sink.is_cancelled() {
                        return Err(error);
                    }
                }
            }
        }
        unreachable!("the second attempt always returns")
    }
}

/// What happened while a host was asked to speak.
#[derive(Default)]
struct Attempt {
    /// Some audio or a mark was passed to the sink.
    relayed: bool,
    /// The host hung or sent something out of turn, rather than ending.
    misbehaved: bool,
}

/// Runs one utterance on `host`, relaying its audio and marks to `sink`.
/// The outer error means the host is no longer usable; the inner result is
/// the utterance's own outcome, which the host reported in turn.
/// `attempt` records whether anything was relayed, and whether the host
/// misbehaved rather than ended.
fn speak_with(
    host: &mut Host,
    sequence: &SpeechSequence,
    sink: &mut dyn SynthSink,
    attempt: &mut Attempt,
) -> Result<Result<(), SynthError>, SynthError> {
    host.send(&ToHost::Speak(sequence.clone()))?;
    let mut cancelled = false;
    let mut last_heard = Instant::now();
    loop {
        // Wait in short slices, so a cancel that arrives before the host
        // has sent any audio still reaches it promptly.
        let reply = match host.replies.recv_timeout(CANCEL_CHECK) {
            Ok(reply) => {
                last_heard = Instant::now();
                reply.map_err(|error| {
                    SynthError::Synthesis(format!("the synthesizer host ended: {error}"))
                })?
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_heard.elapsed() >= HANG_TIMEOUT {
                    attempt.misbehaved = true;
                    return Err(SynthError::Synthesis(format!(
                        "the synthesizer host sent nothing for {HANG_TIMEOUT:?}"
                    )));
                }
                if !cancelled && sink.is_cancelled() {
                    cancelled = true;
                    host.send(&ToHost::Cancel(sequence.utterance))?;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(SynthError::Synthesis(
                    "the synthesizer host ended".to_owned(),
                ));
            }
        };
        match reply {
            FromHost::Pcm(format, samples) => {
                attempt.relayed |= !cancelled;
                if !cancelled && sink.push_pcm(format, &samples).is_break() {
                    cancelled = true;
                    host.send(&ToHost::Cancel(sequence.utterance))?;
                }
            }
            FromHost::Mark(mark) => {
                if !cancelled {
                    attempt.relayed = true;
                    sink.index_reached(mark);
                }
            }
            FromHost::Done => return Ok(Ok(())),
            FromHost::Failed(reason) => return Ok(Err(SynthError::Synthesis(reason))),
            other => {
                attempt.misbehaved = true;
                return Err(SynthError::Synthesis(format!(
                    "the synthesizer host sent {other:?} while speaking"
                )));
            }
        }
    }
}

/// A [`SynthFactory`] that starts a host for `synth` from `exe`.
pub fn factory(exe: PathBuf, synth: SynthId) -> SynthFactory {
    Box::new(move || {
        Ok(Box::new(HostedSynth::start(exe.clone(), synth.clone())?) as Box<dyn SynthDriver>)
    })
}
