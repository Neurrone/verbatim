//! What `verbatim-synth-host.exe` runs once it has its synthesizer: one
//! thread serving Core over its two pipes (decision D18;
//! `docs/design/communication.md`, sections 6.5 and 7). A library, so a
//! test can serve Core with a scripted driver through the same code.
//!
//! The thread waits only on events. Idle, it waits on the command pipe's
//! read, which is always in flight while nothing else is read. Woken by a
//! command, it reads every whole command waiting, then speaks, applies a
//! setting, or answers an utterance cancelled before it began. While its
//! driver synthesizes, it learns of a command from the same event without
//! another thread: the sink reads the commands waiting each time the
//! driver pushes audio or asks whether its utterance was cancelled, a
//! read that returns at once when nothing has come; and a driver that
//! waits for its synthesizer, as `OneCore` does, is given the command
//! event to wait on too ([`serve`]'s `build`). A `Cancel` of the utterance
//! being spoken stops it at the driver's next push; one of an utterance
//! still waiting answers it with `Done` when its turn comes, unspoken.
//!
//! Audio goes to Core by paced writes: a write the pipe cannot take at
//! once, since Core's synth thread has not yet read what is ahead of it,
//! is waited for together with the command event, so commands are read
//! while it waits. The write is never cancelled, whatever comes: the frame
//! is finished whole, and only then is a cancelled utterance stopped, so
//! the pipe never holds part of a frame (section 4.3).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::ops::ControlFlow;
use std::os::windows::io::OwnedHandle;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_process::clock::now_us;
use verbatim_process::event::{Event, wait_any};
use verbatim_process::transport::{EventReader, EventWriter, Taken, put_frame, put_frame_with_raw};
use verbatim_speech::hosting::{
    Framing, FromHost, HostDescription, HostTiming, ToHost, decode_to_host, put_from_host, put_pcm,
};
use verbatim_speech::{
    CancelWake, IndexMark, SettingId, SettingValue, SpeechSequence, SynthDriver, SynthError,
    SynthSink,
};

/// How much a read of Core's commands asks for while the next command's
/// length is unknown: the pipe's buffer.
const COMMAND_READ: usize = 4096;

/// `--pipe-in <handle> --pipe-out <handle> --synth <id>`, in any order:
/// the raw values of the inherited pipe ends the host reads commands from
/// and writes to, and the synthesizer's id.
#[must_use]
pub fn parse_args(args: &[String]) -> Option<(usize, usize, String)> {
    let mut pipe_in = None;
    let mut pipe_out = None;
    let mut synth = None;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--pipe-in" => pipe_in = iter.next()?.parse().ok(),
            "--pipe-out" => pipe_out = iter.next()?.parse().ok(),
            "--synth" => synth = iter.next().cloned(),
            _ => return None,
        }
    }
    Some((pipe_in?, pipe_out?, synth?))
}

/// Serves Core on this thread until it closes the command pipe: builds the
/// driver with `build`, which is given the command event for a driver that
/// waits for its synthesizer to wait on as well, sends `Ready` with its
/// description (or `Unavailable`, and returns), then serves commands in
/// order. `commands` and `replies` are the host's ends of its two pipes,
/// both opened for overlapped I/O.
pub fn serve(
    commands: OwnedHandle,
    replies: OwnedHandle,
    build: impl FnOnce(&Arc<Event>) -> Result<Box<dyn SynthDriver>, SynthError>,
) -> ExitCode {
    let host = match Host::new(commands, replies) {
        Ok(host) => host,
        Err(error) => {
            tracing::error!(%error, "the pipes to Core could not be opened");
            return ExitCode::FAILURE;
        }
    };
    let mut driver = match build(&host.command_event) {
        Ok(driver) => driver,
        Err(error) => {
            tracing::error!(%error, "the synthesizer could not start");
            host.send(&[&FromHost::Unavailable(error.to_string())]);
            return ExitCode::FAILURE;
        }
    };
    if !host.send(&[&FromHost::Ready(describe(driver.as_ref()))]) {
        return ExitCode::FAILURE;
    }
    loop {
        let Some(command) = host.commands.borrow_mut().next() else {
            break;
        };
        let reply = match command {
            Command::Skip(utterance) => FromHost::Done(utterance),
            Command::Speak(sequence, received) => {
                let utterance = sequence.utterance;
                let result = speak(&host, driver.as_mut(), &sequence, received);
                if host.broken.get() {
                    return ExitCode::FAILURE;
                }
                match result {
                    Ok(()) => FromHost::Done(utterance),
                    Err(error) => FromHost::Failed(utterance, error.to_string()),
                }
            }
            Command::SetSetting(id, value) => FromHost::SettingApplied(
                driver
                    .set_setting(&id, value)
                    .err()
                    .map(|error| error.to_string()),
            ),
        };
        if !host.send(&[&reply]) {
            return ExitCode::FAILURE;
        }
    }
    // Core closed the command pipe: it is done with this host.
    if host.commands.borrow().clean_end {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Speaks `sequence`, taken in at `received`, with `driver`, its audio and
/// marks going to Core.
fn speak(
    host: &Host,
    driver: &mut dyn SynthDriver,
    sequence: &SpeechSequence,
    received: Received,
) -> Result<(), SynthError> {
    tracing::trace!(target: "verbatim::stage", trace = %sequence.trace_id, stage = "host request");
    host.commands.borrow_mut().begin(sequence.utterance);
    let mut sink = PipeSink {
        host,
        utterance: sequence.utterance,
        trace_id: sequence.trace_id,
        audio: false,
        received,
    };
    let result = driver.speak(sequence, &mut sink);
    host.commands.borrow_mut().end();
    result
}

/// What the host's synthesizer is, for Core.
fn describe(driver: &dyn SynthDriver) -> HostDescription {
    HostDescription {
        display_name: driver.display_name(),
        places_marks: driver.places_marks(),
        changes_pitch: driver.changes_pitch(),
        settings: driver.supported_settings(),
        values: driver
            .supported_settings()
            .iter()
            .filter_map(|descriptor| {
                let id = descriptor.id().clone();
                driver.setting(&id).map(|value| (id, value))
            })
            .collect(),
    }
}

/// The transport's framing, appending each frame to one buffer.
struct Frames<'b>(&'b mut Vec<u8>);

impl Framing for Frames<'_> {
    fn body(&mut self, put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<()> {
        put_frame(self.0, put_body)
    }

    fn body_and_raw(
        &mut self,
        put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
        put_raw: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> io::Result<()> {
        put_frame_with_raw(self.0, put_body, put_raw)
    }
}

/// The host's two pipes, used from its one thread.
struct Host {
    commands: RefCell<Commands>,
    /// The event the command pipe's read sets when it finishes.
    command_event: Arc<Event>,
    replies: RefCell<EventWriter>,
    /// Core is gone: the pipe to it could not be written.
    broken: Cell<bool>,
}

impl Host {
    fn new(commands: OwnedHandle, replies: OwnedHandle) -> io::Result<Self> {
        let reader = EventReader::new(commands, COMMAND_READ)?;
        let command_event = Arc::clone(reader.event());
        let mut commands = Commands {
            reader,
            waiting: VecDeque::new(),
            current: None,
            cancelled: false,
            wake: None,
            ended: false,
            clean_end: false,
        };
        // The first read goes in flight now, so a command sent while the
        // synthesizer starts sets the event.
        commands.read();
        Ok(Self {
            commands: RefCell::new(commands),
            command_event,
            replies: RefCell::new(EventWriter::new(replies)?),
            broken: Cell::new(false),
        })
    }

    /// Writes `messages` to Core in one write, and waits until the pipe has
    /// taken all of it, reading Core's commands meanwhile. Returns whether
    /// it was written; once a write fails, Core is gone, and nothing more
    /// is written.
    fn send(&self, messages: &[&FromHost]) -> bool {
        if self.broken.get() {
            return false;
        }
        let mut writer = self.replies.borrow_mut();
        let written = writer
            .start(|buffer| {
                for message in messages {
                    put_from_host(message, &mut Frames(buffer))?;
                }
                Ok(())
            })
            .and_then(|done| {
                if done {
                    Ok(())
                } else {
                    self.paced(&mut writer)
                }
            });
        self.written(written)
    }

    /// Writes one PCM frame of `utterance`, after `first` when it is the
    /// utterance's first audio, as [`send`](Self::send) does.
    fn send_pcm(
        &self,
        first: Option<&FromHost>,
        utterance: UtteranceId,
        format: PcmFormat,
        samples: &[i16],
    ) -> bool {
        if self.broken.get() {
            return false;
        }
        let mut writer = self.replies.borrow_mut();
        let written = writer
            .start(|buffer| {
                if let Some(first) = first {
                    put_from_host(first, &mut Frames(buffer))?;
                }
                put_pcm(utterance, format, samples, &mut Frames(buffer))
            })
            .and_then(|done| {
                if done {
                    Ok(())
                } else {
                    self.paced(&mut writer)
                }
            });
        self.written(written)
    }

    fn written(&self, written: io::Result<()>) -> bool {
        if let Err(error) = written {
            tracing::error!(%error, "Core could not be written to");
            self.broken.set(true);
            return false;
        }
        true
    }

    /// Waits for the write in flight to finish, reading every command that
    /// arrives meanwhile: the paced send of section 4.3. The write is
    /// never cancelled; a `Cancel` read here takes effect once it has
    /// finished.
    fn paced(&self, writer: &mut EventWriter) -> io::Result<()> {
        loop {
            let ended = self.commands.borrow().ended;
            let signalled = if ended {
                wait_any(&[writer.event()], None)?
            } else {
                wait_any(&[writer.event(), &self.command_event], None)?
            };
            if signalled == Some(0) {
                if let Some(done) = writer.poll() {
                    return done;
                }
            } else {
                self.commands.borrow_mut().read();
            }
        }
    }
}

/// When a `Speak` was taken in: on the shared clock, for Core's trip to
/// the host, and on this process's own clock, for the first chunk's time,
/// which runs from here (`docs/design/communication.md`, section 10.1), so
/// the time the request waits its turn behind an utterance still ending
/// is counted in it.
#[derive(Clone, Copy)]
struct Received {
    at_us: u64,
    at: Instant,
}

impl Received {
    fn now() -> Self {
        Self {
            at: Instant::now(),
            at_us: now_us(),
        }
    }
}

/// A command waiting its turn.
enum Command {
    /// Speak this, taken in at this time.
    Speak(SpeechSequence, Received),
    /// This utterance was cancelled before its turn: it is answered with
    /// `Done`, unspoken.
    Skip(UtteranceId),
    /// Change this setting.
    SetSetting(SettingId, SettingValue),
}

/// Core's commands: read as they come, waiting their turn in order, a
/// `Cancel` taking effect as soon as it is read.
struct Commands {
    reader: EventReader,
    waiting: VecDeque<Command>,
    /// The utterance being spoken.
    current: Option<UtteranceId>,
    /// Its cancel has been read.
    cancelled: bool,
    /// The waker its driver registered ([`SynthSink::on_cancel`]).
    wake: Option<CancelWake>,
    /// The command pipe has ended, and how: Core closed it between
    /// commands, or not.
    ended: bool,
    clean_end: bool,
}

impl Commands {
    /// Reads every whole command waiting, without waiting, leaving a read
    /// in flight unless the pipe has ended.
    fn read(&mut self) {
        while !self.ended {
            let Self {
                reader,
                waiting,
                current,
                cancelled,
                wake,
                ..
            } = self;
            let taken = reader.take(|parts| {
                match decode_to_host(parts.without_raw()?)? {
                    ToHost::Cancel(utterance) if *current == Some(utterance) => {
                        *cancelled = true;
                        if let Some(wake) = wake.take() {
                            wake();
                        }
                    }
                    ToHost::Cancel(utterance) => {
                        // One still waiting is skipped; one already ended
                        // needs nothing.
                        if let Some(command) = waiting.iter_mut().find(|command| {
                            matches!(command, Command::Speak(sequence, _) if sequence.utterance == utterance)
                        }) {
                            *command = Command::Skip(utterance);
                        }
                    }
                    ToHost::Speak(sequence) => {
                        waiting.push_back(Command::Speak(sequence, Received::now()));
                    }
                    ToHost::SetSetting { id, value } => {
                        waiting.push_back(Command::SetSetting(id, value));
                    }
                }
                Ok(())
            });
            match taken {
                Taken::Frames => {}
                Taken::Waiting => return,
                Taken::Ended(ended) => {
                    if let Err(error) = &ended {
                        tracing::error!(%error, "Core's commands could not be read");
                    }
                    self.ended = true;
                    self.clean_end = ended.is_ok();
                    // Core is gone: the utterance being spoken is over.
                    if let Some(wake) = self.wake.take() {
                        wake();
                    }
                }
            }
        }
    }

    /// The next command in turn, waiting for one; `None` once the command
    /// pipe has ended and every command before its end has been served.
    fn next(&mut self) -> Option<Command> {
        loop {
            if let Some(command) = self.waiting.pop_front() {
                return Some(command);
            }
            if self.ended {
                return None;
            }
            self.read();
            if self.waiting.is_empty()
                && !self.ended
                && let Err(error) = wait_any(&[self.reader.event()], None)
            {
                tracing::error!(%error, "waiting for Core's commands failed");
                self.ended = true;
            }
        }
    }

    fn begin(&mut self, utterance: UtteranceId) {
        self.current = Some(utterance);
        self.cancelled = false;
        self.wake = None;
    }

    fn end(&mut self) {
        self.current = None;
        self.wake = None;
    }

    /// Whether the utterance being spoken is cancelled, reading the
    /// commands that have come first: Core cancelled it, or Core is gone.
    fn is_cancelled(&mut self) -> bool {
        self.read();
        self.cancelled || self.ended
    }
}

/// Sends the driver's output to Core.
struct PipeSink<'a> {
    host: &'a Host,
    /// The utterance being spoken.
    utterance: UtteranceId,
    /// The trace the utterance belongs to, and whether the driver has given
    /// audio yet, for the stage log.
    trace_id: TraceId,
    audio: bool,
    /// When the request was read, for the first audio's timing.
    received: Received,
}

impl SynthSink for PipeSink<'_> {
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        if self.is_cancelled() {
            return ControlFlow::Break(());
        }
        let first = (!self.audio).then(|| {
            self.audio = true;
            tracing::trace!(target: "verbatim::stage", trace = %self.trace_id, stage = "host audio");
            FromHost::FirstAudio(
                self.utterance,
                HostTiming {
                    received_at_us: self.received.at_us,
                    first_chunk_us: u64::try_from(self.received.at.elapsed().as_micros())
                        .unwrap_or(u64::MAX),
                    sent_at_us: now_us(),
                },
            )
        });
        // Waits while Core is behind: the pipe's small buffer is the
        // backpressure that paces the synthesizer.
        if !self
            .host
            .send_pcm(first.as_ref(), self.utterance, format, samples)
        {
            return ControlFlow::Break(());
        }
        // A cancel read while the frame was pended stops the utterance now
        // that the frame is whole.
        if self.host.commands.borrow().cancelled {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn index_reached(&mut self, mark: IndexMark) {
        self.host.send(&[&FromHost::Mark(self.utterance, mark)]);
    }

    fn is_cancelled(&self) -> bool {
        self.host.broken.get() || self.host.commands.borrow_mut().is_cancelled()
    }

    fn on_cancel(&mut self, wake: CancelWake) {
        let mut commands = self.host.commands.borrow_mut();
        if commands.cancelled {
            drop(commands);
            wake();
        } else {
            commands.wake = Some(wake);
        }
    }
}
