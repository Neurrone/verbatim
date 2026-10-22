//! Speech assertions: collects speech frames from a dedicated, never-shared
//! control-plane connection and checks them against exact expectations.
//!
//! There is one assertion: the next utterances are exactly this list. Each
//! has its exact text, comes in the order given, ends as expected (heard
//! in full unless the step interrupts it, when it must be cancelled), and
//! nothing else comes in between. An utterance is anything Verbatim queued,
//! an empty one included, and a sound played at once for an event, which
//! reads as `sound:` and its indication. A step that interrupts speech
//! still playing first asserts, with [`SpeechCollector::expect_started`],
//! that the utterance it interrupts has started to play. A pause or resume
//! of speech, which Verbatim reports apart from utterances, is asserted
//! with [`SpeechCollector::expect_paused`] and
//! [`SpeechCollector::expect_resumed`] after the input that caused it.
//! Every Shift pressed with another key, such as Shift+Tab, pauses speech
//! and the key after it resumes it, as in NVDA, so a pause or resume no
//! step asserted is left behind, on the timeline, at the next input.
//!
//! A scenario ends with [`SpeechCollector::expect_nothing_more`]: Verbatim
//! is asked, on this same connection, to answer once it has handled the
//! scenario's last injected key and is idle (`Request::AwaitIdle`), so
//! every utterance it queued before then reaches the collector before the
//! answer does, and none may be left that no assertion matched. That is
//! evidence, never a period of silence.
//!
//! Nothing is ever skipped or discarded: an utterance an assertion did not
//! expect fails it, and an input injected while an utterance is waiting
//! that no assertion has matched is a harness error
//! ([`SpeechCollector::require_all_asserted`]).
//!
//! The dedicated connection matters. [`ControlClient::request`] discards
//! any frame that is not the reply it is waiting for, so the collector
//! never calls it after its subscribe: the connection is split
//! ([`ControlClient::split`]), its one request, `AwaitIdle`, is sent on
//! the writing half, and its reply is read among the frames.
//!
//! The reading half is read on a thread of its own, which records each
//! frame on the timeline the moment it reaches the harness and passes it
//! on to the assertions. The scenario's own thread waits on its requests
//! to the agent, such as bringing a window forward, which takes a few
//! hundred milliseconds and once took 1.4 seconds; read on that thread,
//! a frame that arrived meanwhile was recorded only once the request was
//! answered, so the timeline put it late, after the step it preceded.
//!
//! In compare mode ([`crate::compare`]), the collector reads NVDA's speech
//! through the same kind of source and sink ([`crate::nvda`]) and asserts
//! nothing: each assertion records its expected texts against the step
//! under way and waits until `AwaitIdle`, which NVDA's sink answers when
//! NVDA has been quiet for the quiet period, has been answered, so the
//! scenario does not run ahead of NVDA, except an assertion on speech the
//! next input interrupts, which that input follows once NVDA has begun to
//! speak; the check
//! before each input or action, and the one at the end, close the step,
//! waiting again only when anything has happened since.
//! [`SpeechCollector::compare_mode`] says more.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;
use std::io;
use std::ops::Range;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use verbatim_control::client::{self as control, Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, Request};
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use crate::compare::CompareStep;
use crate::timeline::Timeline;

/// How long an assertion waits for each expected utterance to be queued,
/// and then for each to end: a bound on a hang, not a latency expectation.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long [`SpeechCollector::expect_nothing_more`] lets Verbatim take to
/// become idle: a bound on a hang.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// How much longer than [`IDLE_TIMEOUT`] Verbatim's answer to `AwaitIdle`
/// may take to arrive: a bound on a hang.
const IDLE_ANSWER_MARGIN: Duration = Duration::from_secs(4);

/// Where a [`SpeechCollector`]'s reader thread reads frames from: the
/// reading half of the live, subscribed control connection, or a scripted
/// sequence in this module's unit tests. A read blocks until a frame
/// arrives or the connection ends, which is an error. In compare mode it
/// is NVDA's transcript ([`crate::nvda`]).
pub(crate) trait FrameSource: Send {
    fn next_frame(&mut self) -> io::Result<Frame>;
}

/// Where a [`SpeechCollector`] sends its one request, `AwaitIdle`: the
/// writing half of the same connection, the scripted sequence, or, in
/// compare mode, NVDA's transcript.
pub(crate) trait RequestSink: Send {
    fn send(&mut self, request: Request) -> io::Result<u64>;
    /// Ends the connection, so a read pending on the reader thread
    /// returns at once with an error.
    fn shutdown(&mut self) -> io::Result<()>;
}

impl FrameSource for control::Receiver {
    fn next_frame(&mut self) -> io::Result<Frame> {
        control::Receiver::next_frame(self)
    }
}

impl RequestSink for control::Sender {
    fn send(&mut self, request: Request) -> io::Result<u64> {
        control::Sender::send(self, request)
    }

    fn shutdown(&mut self) -> io::Result<()> {
        control::Sender::shutdown(self)
    }
}

/// How an expected utterance must end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Heard in full.
    Completed,
    /// Cut off, by the step that interrupts it or by speech that replaces
    /// it.
    Cancelled,
}

/// How many times an expected utterance is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    /// Exactly once, ending as [`Expected::ending`] says.
    Once,
    /// An optional utterance ([`optional`]): not at all, or up to this
    /// many times in a row, each heard in full or cut off.
    UpTo(usize),
}

/// One expected utterance: its exact text, how it must end, and how many
/// times it is said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The exact text.
    pub text: String,
    /// How it must end; not checked for an optional utterance, which may
    /// end either way.
    pub ending: Ending,
    /// Once, or, for an optional utterance, up to how many times.
    pub said: Said,
}

/// An utterance heard in full, for [`SpeechCollector::expect_sequence`].
#[must_use]
pub fn heard(text: &str) -> Expected {
    Expected {
        text: text.to_owned(),
        ending: Ending::Completed,
        said: Said::Once,
    }
}

/// An utterance cut off, for [`SpeechCollector::expect_sequence`].
#[must_use]
pub fn cut_off(text: &str) -> Expected {
    Expected {
        text: text.to_owned(),
        ending: Ending::Cancelled,
        said: Said::Once,
    }
}

/// An optional utterance, for [`SpeechCollector::expect_sequence`]: speech
/// whose presence the application decides by its own timing, accepted at
/// this place in the sequence heard in full, cut off, whatever cut it
/// off, or not said at all, and nothing else in its place. Each use is approved
/// and listed in `docs/testing.md` ("Optional utterances"), and what
/// became of it is reported with the run ([`OptionalOutcome`]). It must be
/// followed in the same sequence by an utterance of other text, which
/// shows that it was not said.
#[must_use]
pub fn optional(text: &str) -> Expected {
    optional_up_to(text, 1)
}

/// [`optional`] said up to `times` times in a row, for speech the
/// application may repeat.
#[must_use]
pub fn optional_up_to(text: &str, times: usize) -> Expected {
    Expected {
        text: text.to_owned(),
        ending: Ending::Completed,
        said: Said::UpTo(times),
    }
}

/// What became of one optional utterance ([`optional`]) in a run: how
/// each time it was said ended, none when it was not said, reported in the
/// run's output, timeline and summary so a change in how often it comes
/// stays visible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionalOutcome {
    /// Its text.
    pub text: String,
    /// The input its step answers, as the timeline describes it; `None`
    /// before any input.
    pub step: Option<String>,
    /// How each time it was said ended, in order; empty when it was not
    /// said.
    pub endings: Vec<Ending>,
}

impl std::fmt::Display for OptionalOutcome {
    /// `"text" after keys enter: absent`, `in full`, `cut off`, or, said
    /// more than once, `2 times: cut off, in full`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.text)?;
        if let Some(step) = &self.step {
            write!(f, " after {step}")?;
        }
        let endings: Vec<&str> = self
            .endings
            .iter()
            .map(|ending| match ending {
                Ending::Completed => "in full",
                Ending::Cancelled => "cut off",
            })
            .collect();
        match endings.as_slice() {
            [] => f.write_str(": absent"),
            [one] => write!(f, ": {one}"),
            all => write!(f, ": {} times: {}", all.len(), all.join(", ")),
        }
    }
}

/// A queued utterance, in the order queued.
#[derive(Clone, Debug)]
struct Utterance {
    utterance: UtteranceId,
    text: String,
}

/// An utterance an assertion matched, for a later assertion about how it
/// ended ([`SpeechCollector::expect_started`] returns one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard {
    /// The utterance's full text at queue time.
    pub text: String,
    utterance: UtteranceId,
}

/// What the collector knows of one utterance's timing, for the latency
/// report.
#[derive(Clone, Debug)]
struct Timing {
    text: String,
    event_observed_at_ms: Option<u64>,
    queued_at_ms: u64,
    audio_started_at_ms: Option<u64>,
}

/// One row of the latency report: an utterance, the step it answered, and
/// how long it took from the event behind it to the queue and to audio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatencyRow {
    /// The step injected last before the event: keys, typed text, or a
    /// gesture, as the timeline describes it; `None` when nothing had been
    /// injected, such as for the speech during setup.
    pub step: Option<String>,
    /// The utterance's text.
    pub text: String,
    /// From the event to the utterance being queued, in milliseconds.
    pub event_to_queue_ms: u64,
    /// From the event to its audio starting, in milliseconds; `None` when
    /// it never played.
    pub event_to_audio_ms: Option<u64>,
}

/// A failed speech assertion: the payload a [`SpeechCollector`]
/// assertion unwinds with, which [`crate::registry`] reports with
/// [`SpeechFailure::report`] once the run's logs are collected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechFailure {
    /// What failed: the expected and heard sequences, and where they first
    /// differ, for a mismatch.
    pub message: String,
    /// The failing step's trace ID: the trace of the utterance the
    /// assertion failed on, or, when it failed on none (an utterance never
    /// queued), of the newest utterance heard in the step; `None` when the
    /// step heard nothing with one.
    pub trace: Option<TraceId>,
    /// The failing step's entries on the timeline
    /// ([`Timeline::current_step`]).
    pub step: Range<usize>,
}

impl SpeechFailure {
    /// The failure as printed: the message; `trace_lines`, every log line
    /// carrying the failing step's trace in time order
    /// ([`crate::artifacts::trace_lines`]), or why they could not be read;
    /// and `timeline` with each earlier step on one line and the failing
    /// step in full.
    #[must_use]
    pub fn report(&self, timeline: &Timeline, trace_lines: Result<&[String], &str>) -> String {
        let mut out = format!("{}\n\n", self.message);
        match (self.trace, trace_lines) {
            (None, _) => out.push_str(
                "the failing step's trace: none, since the step heard nothing that carries a trace ID\n",
            ),
            (Some(trace), Err(error)) => {
                let _ = writeln!(
                    out,
                    "the failing step's trace, {trace}: the logs could not be read: {error}"
                );
            }
            (Some(trace), Ok([])) => {
                let _ = writeln!(
                    out,
                    "the failing step's trace, {trace}: no log line carries it"
                );
            }
            (Some(trace), Ok(lines)) => {
                let _ = writeln!(
                    out,
                    "the failing step's trace, {trace}, from Verbatim's log, Core's flight recorder and the outposts' logs, in time order:"
                );
                for line in lines {
                    let _ = writeln!(out, "{line}");
                }
            }
        }
        let _ = write!(
            out,
            "\ntimeline, each earlier step on one line and the failing step in full:\n{}",
            timeline.render_failed_step(self.step.clone())
        );
        out
    }
}

/// One frame, as the collector reads it.
enum Absorbed {
    Utterance(Utterance),
    Reply { to: u64, error: Option<String> },
    Other,
}

/// Subscribes to and collects speech frames on its own control-plane
/// connection.
pub struct SpeechCollector {
    /// The connection's writing half.
    sink: Box<dyn RequestSink>,
    /// The frames the reader thread has read, already on the timeline, in
    /// the order they came, or the error that ended the connection.
    frames: mpsc::Receiver<io::Result<Frame>>,
    /// The reader thread, joined when the collector is dropped, once the
    /// connection is shut down.
    reader: Option<JoinHandle<()>>,
    /// The scenario's shared action-and-speech log (see [`crate::timeline`]).
    timeline: Timeline,
    /// How long an assertion waits; [`STEP_TIMEOUT`] outside unit tests.
    step_timeout: Duration,
    /// Utterances read and not yet matched by an assertion, oldest first.
    pending: VecDeque<Utterance>,
    texts: HashMap<UtteranceId, String>,
    /// The trace ID behind each utterance that has one.
    traces: HashMap<UtteranceId, TraceId>,
    endings: HashMap<UtteranceId, UtteranceEnding>,
    unended: BTreeSet<UtteranceId>,
    started: BTreeSet<UtteranceId>,
    /// Every utterance's timing, by id, and the order they came in.
    timings: HashMap<UtteranceId, Timing>,
    order: Vec<UtteranceId>,
    /// The id the next sound played at once is matched under: they count
    /// down from the top of the id space, which the pipeline's own ids,
    /// counting up from 1, never reach.
    next_sound: u64,
    /// Pauses, `true`, and resumes, `false`, read and not yet matched by
    /// an assertion, oldest first.
    pauses: VecDeque<bool>,
    /// Whether the scenario asserts no speech, as the measurement
    /// workloads do ([`Self::collect_only`]): unmatched utterances are
    /// collected in [`Self::collected`] instead of failing it.
    collect_only: bool,
    /// Utterances collected in collect-only mode and not yet taken, oldest
    /// first.
    collected: Vec<Heard>,
    /// Compare mode's record, when the scenario runs against NVDA
    /// ([`Self::compare_mode`]).
    compare: Option<Comparison>,
    /// What became of each optional utterance asserted so far, in order.
    optional_outcomes: Vec<OptionalOutcome>,
}

/// Compare mode's record: the steps closed so far, the texts expected in
/// the step under way, the timeline entry that step begins at, how many
/// entries the timeline held when NVDA was last found quiet, and whether
/// the step under way recorded speech the next input interrupts without
/// waiting for NVDA.
struct Comparison {
    steps: Vec<CompareStep>,
    expected: Vec<String>,
    from: usize,
    settled_at: Option<usize>,
    unsettled: bool,
}

impl SpeechCollector {
    /// Subscribes to speech on `control`, which becomes exclusively owned
    /// by the collector, pushing every utterance it reads to `timeline`.
    /// The first speech subscription to a Verbatim also receives the
    /// speech it queued before anyone subscribed, its startup speech. The
    /// connection is then split, with no read timeout, and its reading
    /// half read on a thread of its own, whose read returns only when a
    /// frame arrives or the connection ends; dropping the collector shuts
    /// the connection down, which ends that read at once.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscribe request fails, the read timeout
    /// cannot be cleared, the connection cannot be split, or the reader
    /// thread cannot be started.
    pub fn subscribe(mut control: ControlClient, timeline: Timeline) -> io::Result<Self> {
        ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        control.set_read_timeout(None)?;
        let (sender, receiver) = control.split()?;
        Self::from_source(Box::new(receiver), Box::new(sender), timeline, STEP_TIMEOUT)
    }

    /// A collector in compare mode ([`Self::compare_mode`]) reading NVDA's
    /// speech from `source` and sending on `sink`, which [`crate::nvda`]
    /// makes from the transcript, pushing every utterance to `timeline`.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader thread cannot be started.
    pub(crate) fn comparing(
        source: Box<dyn FrameSource>,
        sink: Box<dyn RequestSink>,
        timeline: Timeline,
    ) -> io::Result<Self> {
        let mut collector = Self::from_source(source, sink, timeline, STEP_TIMEOUT)?;
        collector.compare_mode();
        Ok(collector)
    }

    /// A collector reading `source` on a thread it starts, and sending on
    /// `sink`.
    fn from_source(
        source: Box<dyn FrameSource>,
        sink: Box<dyn RequestSink>,
        timeline: Timeline,
        step_timeout: Duration,
    ) -> io::Result<Self> {
        let (passed, frames) = mpsc::channel();
        let reader = {
            let timeline = timeline.clone();
            thread::Builder::new()
                .name("speech reader".to_owned())
                .spawn(move || read_frames(source, &timeline, &passed))?
        };
        Ok(Self {
            sink,
            frames,
            reader: Some(reader),
            timeline,
            step_timeout,
            pending: VecDeque::new(),
            texts: HashMap::new(),
            traces: HashMap::new(),
            endings: HashMap::new(),
            unended: BTreeSet::new(),
            started: BTreeSet::new(),
            timings: HashMap::new(),
            order: Vec::new(),
            next_sound: u64::MAX,
            pauses: VecDeque::new(),
            collect_only: false,
            collected: Vec::new(),
            compare: None,
            optional_outcomes: Vec::new(),
        })
    }

    /// Records one frame, which the reader thread has put on the timeline,
    /// in the collector's state.
    fn absorb(&mut self, frame: Frame) -> Absorbed {
        match frame {
            Frame::Speech {
                utterance,
                trace_id,
                text,
                event_observed_at_ms,
                queued_at_ms,
            } => {
                self.texts.insert(utterance, text.clone());
                self.traces.insert(utterance, trace_id);
                if !self.endings.contains_key(&utterance) {
                    self.unended.insert(utterance);
                }
                self.timings.insert(
                    utterance,
                    Timing {
                        text: text.clone(),
                        event_observed_at_ms,
                        queued_at_ms,
                        audio_started_at_ms: None,
                    },
                );
                self.order.push(utterance);
                Absorbed::Utterance(Utterance { utterance, text })
            }
            Frame::SpeechStarted { utterance, at_ms } => {
                self.started.insert(utterance);
                if let Some(timing) = self.timings.get_mut(&utterance) {
                    timing.audio_started_at_ms = Some(at_ms);
                }
                Absorbed::Other
            }
            Frame::SpeechEnded { utterance, ending } => {
                self.unended.remove(&utterance);
                self.endings.insert(utterance, ending);
                Absorbed::Other
            }
            // A sound played at once for an event is an utterance of its
            // own, `sound:` and its indication, the way a sound in an
            // utterance's text reads. It has no ending of its own, so it
            // counts as heard in full at once.
            Frame::Sound { indication, .. } => {
                let text = format!("sound: {indication}");
                let utterance = UtteranceId(self.next_sound);
                self.next_sound -= 1;
                self.texts.insert(utterance, text.clone());
                self.started.insert(utterance);
                self.endings.insert(utterance, UtteranceEnding::Completed);
                Absorbed::Utterance(Utterance { utterance, text })
            }
            // A pause or resume is not an utterance: it is asserted on its
            // own, after the input that caused it.
            Frame::SpeechPaused { paused, .. } => {
                self.pauses.push_back(paused);
                Absorbed::Other
            }
            Frame::Reply { to, .. } => Absorbed::Reply { to, error: None },
            Frame::Error { to, message } => Absorbed::Reply {
                to,
                error: Some(message),
            },
            _ => Absorbed::Other,
        }
    }

    fn text_of(&self, utterance: UtteranceId) -> &str {
        self.texts
            .get(&utterance)
            .map_or("(unknown utterance)", String::as_str)
    }

    /// Takes the next frame from the reader thread, waiting until
    /// `deadline` for one, and keeps any utterance it announces in
    /// [`Self::pending`]. No frame by then is `Ok(None)`.
    fn read_by(&mut self, deadline: Instant) -> io::Result<Option<Absorbed>> {
        let wait = deadline.saturating_duration_since(Instant::now());
        match self.frames.recv_timeout(wait) {
            Ok(received) => self.take(received).map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(reader_stopped()),
        }
    }

    /// Absorbs one frame the reader thread passed on, keeping any
    /// utterance it announces in [`Self::pending`].
    fn take(&mut self, received: io::Result<Frame>) -> io::Result<Absorbed> {
        let absorbed = self.absorb(received?);
        if let Absorbed::Utterance(utterance) = &absorbed {
            self.pending.push_back(utterance.clone());
        }
        Ok(absorbed)
    }

    /// Takes every frame that has reached the harness, which the reader
    /// thread passes on as soon as it arrives, without waiting for any:
    /// what has arrived is the evidence the caller checks. A frame still
    /// on the wire is not yet evidence of anything, and whatever it
    /// carries is checked by a later assertion, at the latest by
    /// [`Self::expect_nothing_more`].
    fn read_arrived(&mut self) {
        loop {
            let received = match self.frames.try_recv() {
                Ok(received) => received,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.fail(&format!(
                    "the speech connection failed: {}",
                    reader_stopped()
                )),
            };
            if let Err(error) = self.take(received) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
    }

    /// The next unmatched utterance, waiting until `deadline` for one.
    fn next_utterance(&mut self, deadline: Instant) -> Option<Utterance> {
        loop {
            if let Some(utterance) = self.pending.pop_front() {
                return Some(utterance);
            }
            if Instant::now() >= deadline {
                return None;
            }
            if let Err(error) = self.read_by(deadline) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
    }

    /// Waits until `utterance` has ended, reading frames meanwhile, and
    /// returns how, or `None` at `deadline`.
    fn ending_by(&mut self, utterance: UtteranceId, deadline: Instant) -> Option<UtteranceEnding> {
        loop {
            if let Some(ending) = self.endings.get(&utterance) {
                return Some(ending.clone());
            }
            if Instant::now() >= deadline {
                return None;
            }
            if let Err(error) = self.read_by(deadline) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
    }

    /// Fails the assertion with `message`; the failing step's trace is
    /// that of the newest utterance heard in the step under way.
    fn fail(&self, message: &str) -> ! {
        self.fail_traced(message, None)
    }

    /// Fails the assertion with `message`; the failing step's trace is
    /// that of `utterance`, the one the assertion failed on.
    fn fail_at(&self, message: &str, utterance: UtteranceId) -> ! {
        self.fail_traced(message, self.traces.get(&utterance).copied())
    }

    /// Unwinds with a [`SpeechFailure`]: `message`, `trace` or else the
    /// newest trace heard in the step under way, and that step.
    ///
    /// It unwinds without the panic hook (`resume_unwind`), which would
    /// print the message at once: [`crate::registry`] catches the failure
    /// and reports it in full once the run's logs are collected, which
    /// only then hold the step's trace.
    fn fail_traced(&self, message: &str, trace: Option<TraceId>) -> ! {
        let step = self.timeline.current_step();
        let trace = trace.or_else(|| self.timeline.last_trace_in(step.clone()));
        std::panic::resume_unwind(Box::new(SpeechFailure {
            message: message.to_owned(),
            trace,
            step,
        }))
    }

    /// Asserts that the next utterances are exactly `texts`, in order, each
    /// heard in full, with nothing else in between, each queued and ended
    /// within [`STEP_TIMEOUT`].
    ///
    /// # Panics
    ///
    /// Panics, printing the expected and actual sequences and the timeline,
    /// on any difference.
    pub fn expect(&mut self, texts: &[&str]) {
        let expected: Vec<Expected> = texts.iter().map(|text| heard(text)).collect();
        self.expect_sequence_within(&expected, self.step_timeout);
    }

    /// [`expect`](Self::expect) with each utterance's ending given, for a
    /// step that interrupts speech or whose speech replaces earlier speech.
    ///
    /// # Panics
    ///
    /// As [`expect`](Self::expect).
    pub fn expect_sequence(&mut self, expected: &[Expected]) {
        self.expect_sequence_within(expected, self.step_timeout);
    }

    /// [`expect`](Self::expect) waiting up to `timeout` for each utterance
    /// to be queued and to end, for a step that speaks for a long time,
    /// such as a flood of terminal output.
    ///
    /// # Panics
    ///
    /// As [`expect`](Self::expect).
    pub fn expect_within(&mut self, texts: &[&str], timeout: Duration) {
        let expected: Vec<Expected> = texts.iter().map(|text| heard(text)).collect();
        self.expect_sequence_within(&expected, timeout);
    }

    fn expect_sequence_within(&mut self, expected: &[Expected], timeout: Duration) {
        // In compare mode an optional utterance is left out: NVDA's speech
        // is compared with what is always expected.
        let required = expected.iter().filter(|want| want.said == Said::Once);
        if self.record_expected(required.map(|want| want.text.as_str())) {
            return;
        }
        Self::require_optional_followed(expected);
        // The sequence as matched so far, each optional utterance as many
        // times as it was said, for the mismatch report and the endings.
        let mut resolved: Vec<Expected> = Vec::new();
        let mut actual: Vec<Utterance> = Vec::new();
        let mut optionals: Vec<(usize, Range<usize>)> = Vec::new();
        for (index, want) in expected.iter().enumerate() {
            let Said::UpTo(times) = want.said else {
                let deadline = Instant::now() + timeout;
                resolved.push(want.clone());
                let Some(got) = self.next_utterance(deadline) else {
                    self.fail(&format!(
                        "{}\nutterance {} ({:?}) was not queued within {timeout:?}",
                        describe_mismatch(&resolved, &actual, expected),
                        actual.len(),
                        want.text
                    ));
                };
                actual.push(got.clone());
                if got.text != want.text {
                    self.fail_at(
                        &describe_mismatch(&resolved, &actual, expected),
                        got.utterance,
                    );
                }
                continue;
            };
            // The same text expected once right after belongs to the end of
            // the run of it, the optional utterance to its start: a repeat
            // cuts off the one before it.
            let same_after = expected[index + 1..]
                .iter()
                .take_while(|next| next.said == Said::Once && next.text == want.text)
                .count();
            let run = self.take_optional(&want.text, times, same_after, timeout);
            let said = run.len();
            optionals.push((index, actual.len()..actual.len() + said));
            resolved.extend(run.iter().map(|_| want.clone()));
            actual.extend(run);
        }
        for (want, got) in resolved.iter().zip(&actual) {
            if matches!(want.said, Said::UpTo(_)) {
                continue;
            }
            let deadline = Instant::now() + timeout;
            let ending = self.ending_by(got.utterance, deadline);
            let ok = matches!(
                (want.ending, &ending),
                (Ending::Completed, Some(UtteranceEnding::Completed))
                    | (Ending::Cancelled, Some(UtteranceEnding::Cancelled))
            );
            if !ok {
                self.fail_at(
                    &format!(
                        "the utterance {:?} was expected to end {:?}, but {}",
                        want.text,
                        want.ending,
                        ending.map_or_else(
                            || format!("had not ended after {timeout:?}"),
                            |ending| format!("ended {ending:?}")
                        )
                    ),
                    got.utterance,
                );
            }
        }
        for (index, said) in optionals {
            self.record_optional(&expected[index].text, &actual[said], timeout);
        }
    }

    /// Takes the utterances an optional utterance of `text` said up to
    /// `times` times matches: the run of `text` that comes next, up to
    /// `times` plus `same_after` long, less the last `same_after`, which
    /// are left for the utterances of the same text expected once right
    /// after it. The first utterance of other text is left unmatched.
    fn take_optional(
        &mut self,
        text: &str,
        times: usize,
        same_after: usize,
        timeout: Duration,
    ) -> Vec<Utterance> {
        let mut run = Vec::new();
        while run.len() < times + same_after {
            let Some(got) = self.next_utterance(Instant::now() + timeout) else {
                break;
            };
            if got.text != text {
                self.pending.push_front(got);
                break;
            }
            run.push(got);
        }
        let said = run.len().saturating_sub(same_after);
        for got in run.drain(said..).rev() {
            self.pending.push_front(got);
        }
        run
    }

    /// Waits for each of `said`, an optional utterance of `text` as many
    /// times as it was said, to end, heard in full or cut off, and records
    /// what became of it, on the timeline and in
    /// [`Self::optional_outcomes`].
    fn record_optional(&mut self, text: &str, said: &[Utterance], timeout: Duration) {
        let mut endings = Vec::new();
        for got in said {
            let deadline = Instant::now() + timeout;
            match self.ending_by(got.utterance, deadline) {
                Some(UtteranceEnding::Completed) => endings.push(Ending::Completed),
                Some(UtteranceEnding::Cancelled) => endings.push(Ending::Cancelled),
                ending => self.fail_at(
                    &format!(
                        "the optional utterance {:?} was expected to be heard in full or cut off, but {}",
                        got.text,
                        ending.map_or_else(
                            || format!("had not ended after {timeout:?}"),
                            |ending| format!("ended {ending:?}")
                        )
                    ),
                    got.utterance,
                ),
            }
        }
        let outcome = OptionalOutcome {
            text: text.to_owned(),
            step: self.timeline.step_at(crate::timeline::unix_ms()),
            endings,
        };
        self.timeline
            .push_note(&format!("optional utterance {outcome}"));
        self.optional_outcomes.push(outcome);
    }

    /// Fails as a harness error unless every optional utterance in
    /// `expected` is followed by an utterance said once with other text,
    /// whose arrival shows that the optional one was not said.
    fn require_optional_followed(expected: &[Expected]) {
        for (index, want) in expected.iter().enumerate() {
            if want.said != Said::Once
                && !expected[index + 1..]
                    .iter()
                    .any(|next| next.said == Said::Once && next.text != want.text)
            {
                panic!(
                    "harness error: the optional utterance {:?} is not followed by an utterance of other text that is always said",
                    want.text
                );
            }
        }
    }

    /// What became of each optional utterance asserted so far, in order.
    #[must_use]
    pub fn optional_outcomes(&self) -> &[OptionalOutcome] {
        &self.optional_outcomes
    }

    /// Asserts that the next utterance is exactly `text` and that its audio
    /// has started to play, without waiting for it to end: for a step that
    /// then interrupts it. Its ending is asserted later with
    /// [`expect_ended`](Self::expect_ended).
    ///
    /// # Panics
    ///
    /// Panics if the next utterance is anything else, if it ends without
    /// having played, or if it does not start within [`STEP_TIMEOUT`].
    pub fn expect_started(&mut self, text: &str) -> Heard {
        if self.record_interrupted([text]) {
            return Heard {
                text: text.to_owned(),
                utterance: COMPARED,
            };
        }
        let deadline = Instant::now() + self.step_timeout;
        let expected = [heard(text)];
        let Some(got) = self.next_utterance(deadline) else {
            self.fail(&format!(
                "{}\nthe utterance {text:?} was not queued within {:?}",
                describe_mismatch(&expected, &[], &expected),
                self.step_timeout
            ));
        };
        if got.text != text {
            self.fail_at(
                &describe_mismatch(&expected, std::slice::from_ref(&got), &expected),
                got.utterance,
            );
        }
        while !self.started.contains(&got.utterance) {
            if let Some(ending) = self.endings.get(&got.utterance) {
                self.fail_at(
                    &format!("{text:?} ended {ending:?} without playing"),
                    got.utterance,
                );
            }
            if Instant::now() >= deadline {
                self.fail_at(
                    &format!(
                        "{text:?} did not start playing within {:?}",
                        self.step_timeout
                    ),
                    got.utterance,
                );
            }
            if let Err(error) = self.read_by(deadline) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
        Heard {
            text: got.text,
            utterance: got.utterance,
        }
    }

    /// Asserts that the next utterances are exactly `texts`, in order,
    /// queued, without waiting for them to end: for a step that then
    /// interrupts them, whose endings are asserted afterwards with
    /// [`expect_ended`](Self::expect_ended).
    ///
    /// # Panics
    ///
    /// As [`expect`](Self::expect), apart from the endings.
    pub fn expect_queued(&mut self, texts: &[&str]) -> Vec<Heard> {
        if self.record_interrupted(texts.iter().copied()) {
            return texts
                .iter()
                .map(|text| Heard {
                    text: (*text).to_owned(),
                    utterance: COMPARED,
                })
                .collect();
        }
        let expected: Vec<Expected> = texts.iter().map(|text| heard(text)).collect();
        let mut actual: Vec<Utterance> = Vec::new();
        for (index, want) in expected.iter().enumerate() {
            let deadline = Instant::now() + self.step_timeout;
            let Some(got) = self.next_utterance(deadline) else {
                self.fail(&format!(
                    "{}\nutterance {index} ({:?}) was not queued within {:?}",
                    describe_mismatch(&expected, &actual, &expected),
                    want.text,
                    self.step_timeout
                ));
            };
            actual.push(got.clone());
            if got.text != want.text {
                self.fail_at(
                    &describe_mismatch(&expected, &actual, &expected),
                    got.utterance,
                );
            }
        }
        actual
            .into_iter()
            .map(|utterance| Heard {
                text: utterance.text,
                utterance: utterance.utterance,
            })
            .collect()
    }

    /// Asserts that `heard`, matched by
    /// [`expect_started`](Self::expect_started), ended as `ending` says,
    /// waiting up to [`STEP_TIMEOUT`] for it to end.
    ///
    /// # Panics
    ///
    /// Panics if it ended otherwise, or had not ended in time.
    pub fn expect_ended(&mut self, heard: &Heard, ending: Ending) {
        if self.compare.is_some() {
            return;
        }
        let deadline = Instant::now() + self.step_timeout;
        let ended = self.ending_by(heard.utterance, deadline);
        let ok = matches!(
            (ending, &ended),
            (Ending::Completed, Some(UtteranceEnding::Completed))
                | (Ending::Cancelled, Some(UtteranceEnding::Cancelled))
        );
        if !ok {
            self.fail_at(
                &format!(
                    "the utterance {:?} was expected to end {ending:?}, but {}",
                    heard.text,
                    ended.map_or_else(
                        || "had not ended".to_owned(),
                        |ended| format!("ended {ended:?}")
                    )
                ),
                heard.utterance,
            );
        }
    }

    /// Requires, for a step meant to cut `heard` off while it plays, that
    /// `heard` had not been heard in full when the step's action had
    /// completed, which is now: every frame already on its way is read
    /// first. Its having been cut off already passes, as Verbatim may act
    /// on the change before the harness sees the action complete. An
    /// action that completes only after the utterance has played to its
    /// end did not test what the step was meant to, so that fails as a
    /// harness error, not as Verbatim's: the action was too slow, which
    /// `when` describes.
    ///
    /// # Panics
    ///
    /// Panics if `heard` had been heard in full.
    pub fn require_not_completed(&mut self, heard: &Heard, when: &str) {
        if self.compare.is_some() {
            return;
        }
        self.read_arrived();
        if matches!(
            self.endings.get(&heard.utterance),
            Some(UtteranceEnding::Completed)
        ) {
            self.fail_at(
                &format!(
                    "harness error: {:?} had been heard in full {when}, so the step meant to cut it off came too late",
                    heard.text
                ),
                heard.utterance,
            );
        }
    }

    /// Asserts that speech was paused next, waiting up to [`STEP_TIMEOUT`]
    /// for Verbatim to report it: the evidence that Shift's pause has been
    /// applied.
    ///
    /// # Panics
    ///
    /// Panics if speech was resumed instead, or no pause is reported in
    /// time.
    pub fn expect_paused(&mut self) {
        self.expect_pause(true);
    }

    /// Asserts that speech was resumed next, waiting up to
    /// [`STEP_TIMEOUT`] for Verbatim to report it.
    ///
    /// # Panics
    ///
    /// Panics if speech was paused instead, or no resume is reported in
    /// time.
    pub fn expect_resumed(&mut self) {
        self.expect_pause(false);
    }

    fn expect_pause(&mut self, paused: bool) {
        if self.compare.is_some() {
            return;
        }
        let what = |paused: bool| if paused { "paused" } else { "resumed" };
        let deadline = Instant::now() + self.step_timeout;
        loop {
            if let Some(got) = self.pauses.pop_front() {
                if got != paused {
                    self.fail(&format!(
                        "speech was expected to be {}, but was {}",
                        what(paused),
                        what(got)
                    ));
                }
                return;
            }
            if Instant::now() >= deadline {
                self.fail(&format!(
                    "speech was not {} within {:?}",
                    what(paused),
                    self.step_timeout
                ));
            }
            if let Err(error) = self.read_by(deadline) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
    }

    /// The utterances queued from now until one whose text is exactly
    /// `until`, inclusive, each waited for up to `timeout`, for a step whose
    /// speech cannot be known before it runs. The caller asserts on every
    /// one returned; none of them is matched otherwise.
    ///
    /// # Panics
    ///
    /// Panics if `until` is not queued, or a gap between utterances is
    /// longer than `timeout`.
    pub fn take_until(&mut self, until: &str, timeout: Duration) -> Vec<Heard> {
        self.refuse_in_compare_mode("take_until");
        let mut taken = Vec::new();
        loop {
            let deadline = Instant::now() + timeout;
            let Some(got) = self.next_utterance(deadline) else {
                self.fail(&format!(
                    "{until:?} was not queued; nothing was queued for {timeout:?} after {} utterances",
                    taken.len()
                ));
            };
            let last = got.text == until;
            taken.push(Heard {
                text: got.text,
                utterance: got.utterance,
            });
            if last {
                return taken;
            }
        }
    }

    /// The next utterance queued, whatever its text, waited for up to
    /// `timeout`, for a measurement step whose speech cannot be known
    /// before it runs. The caller asserts on it; it is matched otherwise
    /// by nothing.
    ///
    /// # Panics
    ///
    /// Panics if nothing is queued within `timeout`.
    pub fn take_next(&mut self, timeout: Duration) -> Heard {
        self.refuse_in_compare_mode("take_next");
        let Some(got) = self.next_utterance(Instant::now() + timeout) else {
            self.fail(&format!("nothing was queued for {timeout:?}"));
        };
        Heard {
            text: got.text,
            utterance: got.utterance,
        }
    }

    /// How `heard` ended, waiting up to `timeout` for it to end; `None`
    /// when it had not.
    pub fn ending_of(&mut self, heard: &Heard, timeout: Duration) -> Option<UtteranceEnding> {
        if self.compare.is_some() {
            return None;
        }
        self.ending_by(heard.utterance, Instant::now() + timeout)
    }

    /// Asserts that nothing more was said: Verbatim is asked, on this
    /// connection, to answer once it has handled the input numbered
    /// `after_input` (the last key or character the scenario injected, if
    /// any) and everything before it, and is idle; every utterance queued
    /// before the answer arrives first, and every one must already have
    /// been matched. Every matched utterance must also have ended.
    ///
    /// # Panics
    ///
    /// Panics, naming the unexpected utterances, if anything was queued
    /// that no assertion matched, or if Verbatim does not become idle.
    pub fn expect_nothing_more(&mut self, after_input: Option<u64>) {
        if self.compare.is_some() {
            self.close_step();
            return;
        }
        self.await_idle(after_input, IDLE_TIMEOUT);
        if self.collect_only {
            // Collected, never failed on: the timeline keeps every
            // utterance with its time, and Verbatim's log its latency.
            self.require_all_asserted("after the scenario's last step");
            return;
        }
        self.require_all_asserted("after the scenario's last assertion");
        let unended: Vec<UtteranceId> = self.unended.iter().copied().collect();
        for utterance in unended {
            let deadline = Instant::now() + self.step_timeout;
            if self.ending_by(utterance, deadline).is_none() {
                self.fail_at(
                    &format!(
                        "{:?} had not ended {:?} after the scenario's last assertion",
                        self.text_of(utterance),
                        self.step_timeout
                    ),
                    utterance,
                );
            }
        }
        self.require_all_asserted("while the last utterances ended");
    }

    /// Every utterance not yet matched that was queued before Verbatim
    /// reports it has handled the input numbered `after_input` and
    /// everything before it, and is idle, in order: for the one step whose
    /// speech cannot be known before it runs (`docs/testing.md`, "Exact
    /// assertions"). The caller asserts on every one returned; each counts
    /// as matched.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle.
    pub fn take_until_idle(&mut self, after_input: Option<u64>) -> Vec<Heard> {
        self.refuse_in_compare_mode("take_until_idle");
        self.await_idle(after_input, IDLE_TIMEOUT);
        let taken: Vec<Heard> = self
            .pending
            .drain(..)
            .map(|utterance| Heard {
                text: utterance.text,
                utterance: utterance.utterance,
            })
            .collect();
        taken
    }

    /// For the measurement workloads, which assert no speech: every
    /// utterance collected or not yet matched once Verbatim is idle after
    /// the input numbered `after_input`, and every frame that has reached
    /// the harness since, each counted as matched without its content
    /// being asserted. Taken just before an input, it leaves nothing for
    /// that input's own check, including speech that comes after Verbatim
    /// reports itself idle, such as an application's event after a key.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle or the connection fails.
    pub fn take_all_queued(&mut self, after_input: Option<u64>) -> Vec<Heard> {
        self.take_all_queued_within(after_input, IDLE_TIMEOUT)
    }

    /// [`Self::take_all_queued`], letting Verbatim take up to `bound` to
    /// become idle: for a measurement whose keys Verbatim is known to keep
    /// waiting longer than [`IDLE_TIMEOUT`]. A bound on a hang.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle within `bound` or the
    /// connection fails.
    pub fn take_all_queued_within(
        &mut self,
        after_input: Option<u64>,
        bound: Duration,
    ) -> Vec<Heard> {
        self.refuse_in_compare_mode("take_all_queued");
        let mut taken = std::mem::take(&mut self.collected);
        self.await_idle(after_input, bound);
        self.read_arrived();
        self.pauses.clear();
        taken.extend(self.pending.drain(..).map(|utterance| Heard {
            text: utterance.text,
            utterance: utterance.utterance,
        }));
        taken
    }

    /// For the measurement workloads: the utterances collect-only mode has
    /// already collected (moved aside by [`Self::require_all_asserted`]
    /// before an input), in order, without reading or waiting for anything,
    /// each counted as matched. Everything they hold arrived before
    /// anything still pending, so taking them before [`Self::take_until`]
    /// keeps a step's speech in arrival order.
    pub fn take_collected(&mut self) -> Vec<Heard> {
        std::mem::take(&mut self.collected)
    }

    /// From now on this scenario asserts no speech, as the measurement
    /// workloads do. Every utterance is still read, timed and kept on the
    /// timeline, and unmatched ones are collected for
    /// [`Self::take_all_queued`] rather than failing the scenario, before
    /// an input or at its end. Only the registry's `measure` group sets it.
    pub fn collect_only(&mut self) {
        self.collect_only = true;
    }

    /// From now on this scenario runs against NVDA and asserts nothing
    /// ([`crate::compare`]). [`expect`](Self::expect),
    /// [`expect_sequence`](Self::expect_sequence) and
    /// [`expect_within`](Self::expect_within) record their texts against
    /// the step under way and wait for `AwaitIdle`'s answer, which NVDA's
    /// sink gives once NVDA has queued nothing for the quiet period, so
    /// what the scenario does next, input or not, does not run ahead of
    /// NVDA. [`expect_started`](Self::expect_started) and
    /// [`expect_queued`](Self::expect_queued) record their texts and return
    /// placeholders without waiting for the quiet period, only until NVDA
    /// has queued speech in the step, since they assert speech the next
    /// input interrupts, which follows at once, as the second press of a
    /// key pressed twice does. The assertions on endings, pauses and
    /// resumes return at once, and [`ending_of`](Self::ending_of) answers
    /// `None`. [`require_all_asserted`](Self::require_all_asserted), before
    /// every input and every action that changes what is in front, and
    /// [`expect_nothing_more`](Self::expect_nothing_more) close the step:
    /// they wait for `AwaitIdle`'s answer in the same way, unless NVDA was
    /// found quiet and the timeline has had nothing since, and record the
    /// step as a [`CompareStep`] from what the timeline holds since the
    /// step before and the texts expected. A step whose speech the next
    /// input interrupts is closed without waiting and without being
    /// recorded: it stays open, and its expectations and what NVDA queued
    /// are recorded with the next step's, as one unit with both inputs,
    /// since NVDA's speech for a rapid double press cannot be told apart
    /// press by press. [`take_until`](Self::take_until),
    /// [`take_until_idle`](Self::take_until_idle) and
    /// [`take_all_queued`](Self::take_all_queued) cannot keep their
    /// promises without Verbatim, and fail as a harness error.
    pub fn compare_mode(&mut self) {
        self.compare = Some(Comparison {
            steps: Vec::new(),
            expected: Vec::new(),
            from: 0,
            settled_at: None,
            unsettled: false,
        });
    }

    /// Whether the collector is in compare mode.
    #[must_use]
    pub fn is_comparing(&self) -> bool {
        self.compare.is_some()
    }

    /// Every step compare mode has recorded, the one under way included
    /// when anything is recorded for it, closed without waiting: for the
    /// report, once the scenario has ended, passed or failed. Empty
    /// outside compare mode.
    pub fn take_comparison(&mut self) -> Vec<CompareStep> {
        self.take_remaining();
        self.record_step();
        self.compare
            .as_mut()
            .map(|comparison| std::mem::take(&mut comparison.steps))
            .unwrap_or_default()
    }

    /// In compare mode, records `texts` as expected in the step under way,
    /// waits for NVDA to be quiet ([`Self::settle`]), and returns `true`;
    /// otherwise returns `false`.
    fn record_expected<'a>(&mut self, texts: impl IntoIterator<Item = &'a str>) -> bool {
        let Some(comparison) = &mut self.compare else {
            return false;
        };
        comparison
            .expected
            .extend(texts.into_iter().map(str::to_owned));
        self.settle();
        true
    }

    /// In compare mode, records `texts`, speech the next input interrupts,
    /// as expected in the step under way without waiting for NVDA to be
    /// quiet, which leaves the step open ([`Self::close_step`]), and
    /// returns `true`; otherwise returns `false`. It waits only until NVDA
    /// has queued speech in the step, if it has not yet: the evidence that
    /// the speech to interrupt has started, as
    /// [`expect_started`](Self::expect_started) waits for Verbatim's
    /// utterance to start playing, so the interrupting input comes when it
    /// does in a run against Verbatim, and never before NVDA has spoken.
    /// When NVDA queues nothing within the step timeout, a note says so.
    fn record_interrupted<'a>(&mut self, texts: impl IntoIterator<Item = &'a str>) -> bool {
        let Some(comparison) = &mut self.compare else {
            return false;
        };
        comparison
            .expected
            .extend(texts.into_iter().map(str::to_owned));
        comparison.unsettled = true;
        let deadline = Instant::now() + self.step_timeout;
        while self.pending.is_empty() {
            if Instant::now() >= deadline {
                self.timeline.push_note(&format!(
                    "NVDA queued no speech within {:?} for the next input to interrupt",
                    self.step_timeout
                ));
                break;
            }
            if let Err(error) = self.read_by(deadline) {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
        true
    }

    /// In compare mode, waits for NVDA to be quiet (`AwaitIdle`), reads
    /// what has arrived, and marks how many entries the timeline then
    /// held, as they were when the answer came.
    fn settle(&mut self) {
        self.await_idle(None, IDLE_TIMEOUT);
        let count = self.timeline.entry_count();
        self.read_arrived();
        if let Some(comparison) = &mut self.compare {
            comparison.settled_at = Some(count);
            comparison.unsettled = false;
        }
    }

    /// Closes compare mode's step under way, before an input or an action
    /// or at the end. A step whose speech the next input interrupts
    /// ([`Self::record_interrupted`]) is left open, without waiting: what
    /// it expected and what NVDA queued are recorded with the next step's,
    /// as one unit. Any other step waits for NVDA to be quiet, unless NVDA
    /// was found quiet and the timeline has had no entry since, which
    /// rules out an input or an action since, as each puts an entry on
    /// the timeline as it starts its step; then the step is recorded.
    fn close_step(&mut self) {
        let Some(comparison) = &mut self.compare else {
            return;
        };
        if std::mem::take(&mut comparison.unsettled) {
            self.timeline.push_note(
                "the next input or action follows at once and cuts this speech off, so the two are compared as one",
            );
        } else {
            let settled = comparison.settled_at;
            if settled != Some(self.timeline.entry_count()) {
                self.settle();
            }
            self.record_step();
        }
        self.pending.clear();
        self.pauses.clear();
    }

    /// Records compare mode's step under way from the timeline's entries
    /// since the step before and the texts expected, unless it has nothing
    /// at all to report.
    fn record_step(&mut self) {
        let Some(comparison) = &mut self.compare else {
            return;
        };
        let (entries, next) = self.timeline.step_since(comparison.from);
        let step = CompareStep::new(
            entries,
            std::mem::take(&mut comparison.expected),
            comparison.from == 0,
        );
        comparison.from = next;
        comparison.steps.extend(step);
    }

    /// Fails as a harness error in compare mode, naming `method`, which
    /// needs Verbatim's own speech to keep its promise.
    fn refuse_in_compare_mode(&self, method: &str) {
        if self.compare.is_some() {
            self.fail(&format!(
                "harness error: SpeechCollector::{method} needs Verbatim's speech and has no NVDA equivalent, so this scenario cannot be compared"
            ));
        }
    }

    /// Asks Verbatim, on this connection, to answer once it has handled
    /// the input numbered `after_input` and everything before it, and is
    /// idle, and reads every frame until the answer: every utterance
    /// queued before it is then read. `bound` bounds the wait for idleness.
    fn await_idle(&mut self, after_input: Option<u64>, bound: Duration) {
        let id = match self.sink.send(Request::AwaitIdle {
            after_input,
            timeout_ms: u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
        }) {
            Ok(id) => id,
            Err(error) => self.fail(&format!(
                "could not ask Verbatim whether it is idle: {error}"
            )),
        };
        let deadline = Instant::now() + bound + IDLE_ANSWER_MARGIN;
        loop {
            match self.read_by(deadline) {
                Ok(Some(Absorbed::Reply { to, error })) if to == id => {
                    if let Some(error) = error {
                        self.fail(&format!("Verbatim did not become idle: {error}"));
                    }
                    break;
                }
                Ok(_) => {}
                Err(error) => self.fail(&format!("the speech connection failed: {error}")),
            }
            if Instant::now() >= deadline {
                self.fail("Verbatim did not answer whether it is idle");
            }
        }
    }

    /// Reads every frame already on its way, without waiting for any to
    /// happen, and fails if any utterance read so far is unmatched: a
    /// scenario never moves on past speech it has not asserted. Pauses and
    /// resumes no step asserted are left behind. Called before every input
    /// the harness injects.
    ///
    /// # Panics
    ///
    /// Panics, naming the unmatched utterances, if there are any.
    pub fn require_all_asserted(&mut self, when: &str) {
        self.require_asserted_before(when, &[]);
    }

    /// [`require_all_asserted`](Self::require_all_asserted) before the input
    /// of a step whose speech is `expected`, whose leading optional
    /// utterances ([`optional`]) may have arrived before the input: the
    /// unmatched utterances may be exactly those, in order, each up to the
    /// times it declares, and are then left for the step's assertion of
    /// `expected` to match (`docs/testing.md`, "Optional utterances").
    /// Any other unmatched utterance fails as it does there.
    ///
    /// # Panics
    ///
    /// As [`require_all_asserted`](Self::require_all_asserted).
    pub fn require_asserted_before(&mut self, when: &str, expected: &[Expected]) {
        if self.compare.is_some() {
            self.close_step();
            return;
        }
        self.read_arrived();
        // A pause or resume no step asserted is left behind here, on the
        // timeline: every Shift pressed with another key pauses speech,
        // which the key after it resumes, as in NVDA, and only a step
        // about pausing asserts it. So a pause asserted after an input
        // is one that input caused.
        self.pauses.clear();
        if self.collect_only {
            let pending = self.pending.drain(..).map(|utterance| Heard {
                text: utterance.text,
                utterance: utterance.utterance,
            });
            self.collected.extend(pending);
            return;
        }
        let mut admitted = 0;
        for want in expected {
            let Said::UpTo(times) = want.said else {
                break;
            };
            admitted += self
                .pending
                .iter()
                .skip(admitted)
                .take(times)
                .take_while(|got| got.text == want.text)
                .count();
        }
        if admitted < self.pending.len()
            && let Some(first) = self.pending.front()
        {
            let unmatched: Vec<&str> = self.pending.iter().map(|u| u.text.as_str()).collect();
            self.fail_at(
                &format!(
                    "harness error: utterances no assertion matched were waiting {when}: {unmatched:?}"
                ),
                first.utterance,
            );
        }
    }

    /// Absorbs every frame that has already reached the harness, without
    /// waiting for any and without failing on a connection that has gone,
    /// since the run may have failed for that: for compare mode's report
    /// ([`Self::take_comparison`]), and once a run whose quit of Verbatim
    /// failed has ended, when the connection's end may never come.
    pub fn take_remaining(&mut self) {
        while let Ok(Ok(frame)) = self.frames.try_recv() {
            self.absorb(frame);
        }
    }

    /// Absorbs every frame Verbatim sent, waiting for the speech
    /// connection's end, for at most the step timeout: called once Verbatim
    /// has quit cleanly. When Verbatim's pipe closes, the agent shuts the
    /// connection down, and the reader thread passes that end on after
    /// every frame it read, so the end, or the reader's channel closing
    /// after it, is the evidence that every frame has been absorbed. The
    /// latency rows then cover every utterance that started to play, the
    /// last one included, which a run that asserts no endings (the measure
    /// set) or that failed has not waited for. The reader records each
    /// frame on the timeline and then passes it on, so the timeline can
    /// hold a frame the collector has not yet absorbed; the end covers
    /// both.
    ///
    /// # Errors
    ///
    /// Returns the problem, for the run's report, when the connection has
    /// not ended within the step timeout.
    pub fn take_until_the_end(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + self.step_timeout;
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(wait) {
                Ok(Ok(frame)) => {
                    self.absorb(frame);
                }
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "the speech connection did not end within {:?} after Verbatim quit, so its last frames may be missing from the latency report",
                        self.step_timeout
                    ));
                }
            }
        }
    }

    /// One row per utterance heard so far that carries the time of the
    /// event behind it, in the order they were queued: the latency report
    /// a run's artifacts keep. An utterance's time to audio includes any
    /// time it waited while the speech queued before it played.
    #[must_use]
    pub fn latency_rows(&self) -> Vec<LatencyRow> {
        self.order
            .iter()
            .filter_map(|utterance| self.timings.get(utterance))
            .filter_map(|timing| {
                let observed = timing.event_observed_at_ms?;
                Some(LatencyRow {
                    step: self.timeline.step_at(observed),
                    text: timing.text.clone(),
                    event_to_queue_ms: timing.queued_at_ms.saturating_sub(observed),
                    event_to_audio_ms: timing
                        .audio_started_at_ms
                        .map(|audio| audio.saturating_sub(observed)),
                })
            })
            .collect()
    }
}

impl Drop for SpeechCollector {
    /// Shuts the connection down, which ends the reader thread's pending
    /// read at once, and joins the thread, so none outlives the collector.
    fn drop(&mut self) {
        if let Err(error) = self.sink.shutdown() {
            // A connection that cannot be shut down has already ended, and
            // the reader's read with it.
            tracing::debug!(%error, "the speech connection was already closed");
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The utterance a compare-mode assertion's placeholder [`Heard`] names,
/// which no frame carries: Verbatim's pipeline and NVDA's transcript count
/// their ids up from 1, and sounds count down from the top.
const COMPARED: UtteranceId = UtteranceId(0);

/// The error a read gets once the reader thread has stopped.
fn reader_stopped() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the speech connection's reader has stopped",
    )
}

/// The reader thread: reads every frame from `source` as it arrives,
/// records it on `timeline` at once, and passes it on through `passed`,
/// until the connection ends, as dropping the collector ends it, whose
/// error is passed on last.
fn read_frames(
    mut source: Box<dyn FrameSource>,
    timeline: &Timeline,
    passed: &mpsc::Sender<io::Result<Frame>>,
) {
    let mut texts: HashMap<UtteranceId, String> = HashMap::new();
    loop {
        match source.next_frame() {
            Ok(frame) => {
                record(timeline, &mut texts, &frame);
                if passed.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = passed.send(Err(error));
                return;
            }
        }
    }
}

/// Records `frame` on `timeline` as it arrives; `texts` holds every
/// utterance's text so far, by id, which an audio start or an ending is
/// recorded with.
fn record(timeline: &Timeline, texts: &mut HashMap<UtteranceId, String>, frame: &Frame) {
    let text_of = |texts: &HashMap<UtteranceId, String>, utterance: &UtteranceId| {
        texts
            .get(utterance)
            .map_or("(unknown utterance)", String::as_str)
            .to_owned()
    };
    match frame {
        Frame::Speech {
            utterance,
            trace_id,
            text,
            ..
        } => {
            timeline.push_utterance(text, Some(*trace_id));
            texts.insert(*utterance, text.clone());
        }
        Frame::SpeechStarted { utterance, .. } => {
            timeline.push_audio_started(&text_of(texts, utterance));
        }
        Frame::SpeechEnded { utterance, ending } => {
            timeline.push_ended(&text_of(texts, utterance), ending);
        }
        Frame::Sound { indication, .. } => {
            timeline.push_utterance(&format!("sound: {indication}"), None);
        }
        Frame::SpeechPaused { paused, .. } => timeline.push_paused(*paused),
        _ => {}
    }
}

/// The expected sequence as `declared`, and the actual one, `{:?}`
/// escaped, with the index of the first utterance that differs between
/// `resolved`, the expected sequence as matched so far, each optional
/// utterance as many times as it was said, and `actual`, and, when both
/// have one there, the first character that differs in it.
fn describe_mismatch(resolved: &[Expected], actual: &[Utterance], declared: &[Expected]) -> String {
    let declared_texts: Vec<String> = declared
        .iter()
        .map(|item| match item.said {
            Said::Once => format!("{:?}", item.text),
            Said::UpTo(1) => format!("{:?} (optional)", item.text),
            Said::UpTo(times) => format!("{:?} (optional, up to {times} times)", item.text),
        })
        .collect();
    let expected_texts: Vec<&str> = resolved.iter().map(|item| item.text.as_str()).collect();
    let actual_texts: Vec<&str> = actual.iter().map(|item| item.text.as_str()).collect();
    let mut out = String::new();
    let _ = writeln!(out, "speech did not match");
    let _ = writeln!(out, "expected: [{}]", declared_texts.join(", "));
    let _ = writeln!(out, "actual:   {actual_texts:?}");
    let first = expected_texts
        .iter()
        .zip(&actual_texts)
        .position(|(want, got)| want != got);
    match first {
        Some(index) => {
            let want = expected_texts[index];
            let got = actual_texts[index];
            let character = want
                .chars()
                .zip(got.chars())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| want.chars().count().min(got.chars().count()));
            let _ = write!(
                out,
                "first difference: utterance {index}, character {character}: expected {want:?}, got {got:?}"
            );
        }
        None => {
            let _ = write!(
                out,
                "first difference: utterance {} is missing",
                actual_texts.len()
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use verbatim_model::TraceId;

    use super::*;

    /// Hands the reader thread the frames a test feeds it, in order,
    /// waiting for each, until the sink shuts it down (`None`), which ends
    /// the connection. Each time it is asked for a frame while it has
    /// none, it reports how many fed frames it has handed out, every one
    /// of which the reader has then passed on.
    struct Scripted {
        frames: mpsc::Receiver<Option<Frame>>,
        handed: usize,
        idle: mpsc::Sender<usize>,
    }

    impl FrameSource for Scripted {
        fn next_frame(&mut self) -> io::Result<Frame> {
            let next = if let Ok(next) = self.frames.try_recv() {
                next
            } else {
                let _ = self.idle.send(self.handed);
                self.frames
                    .recv()
                    .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?
            };
            let frame = next.ok_or_else(|| io::Error::from(io::ErrorKind::ConnectionAborted))?;
            if !matches!(frame, Frame::Reply { .. }) {
                self.handed += 1;
            }
            Ok(frame)
        }
    }

    /// Answers `AwaitIdle` at once, after the frames fed before it,
    /// counting the requests in `sent`.
    struct ScriptedSink {
        frames: mpsc::Sender<Option<Frame>>,
        next_id: u64,
        sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    impl RequestSink for ScriptedSink {
        fn send(&mut self, _request: Request) -> io::Result<u64> {
            self.next_id += 1;
            self.sent
                .store(self.next_id, std::sync::atomic::Ordering::SeqCst);
            self.frames
                .send(Some(Frame::Reply {
                    to: self.next_id,
                    payload: verbatim_control::protocol::ReplyPayload::Idle,
                }))
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            Ok(self.next_id)
        }

        fn shutdown(&mut self) -> io::Result<()> {
            self.frames
                .send(None)
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    /// Feeds a collector's reader thread.
    struct Script {
        feed: mpsc::Sender<Option<Frame>>,
        idle: mpsc::Receiver<usize>,
        fed: usize,
        sent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    impl Script {
        /// How many requests the collector has sent its sink.
        fn requests_sent(&self) -> u64 {
            self.sent.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Feeds `frames` and waits until the reader thread has passed
        /// every one on, so each is on the timeline and waiting for the
        /// collector.
        fn feed(&mut self, frames: Vec<Frame>) {
            self.fed += frames.len();
            for frame in frames {
                self.feed.send(Some(frame)).expect("the reader is running");
            }
            while self.idle.recv().expect("the reader is running") < self.fed {}
        }
    }

    const SHORT: Duration = Duration::from_millis(50);

    /// A collector on `timeline` with nothing fed yet, and what feeds it.
    fn scripted(timeline: Timeline) -> (SpeechCollector, Script) {
        let (feed, frames) = mpsc::channel();
        let (idle, handed) = mpsc::channel();
        let sent = std::sync::Arc::default();
        let speech = SpeechCollector::from_source(
            Box::new(Scripted {
                frames,
                handed: 0,
                idle,
            }),
            Box::new(ScriptedSink {
                frames: feed.clone(),
                next_id: 0,
                sent: std::sync::Arc::clone(&sent),
            }),
            timeline,
            SHORT,
        )
        .expect("starts the reader thread");
        let script = Script {
            feed,
            idle: handed,
            fed: 0,
            sent,
        };
        (speech, script)
    }

    fn queued(id: u64, text: &str) -> Frame {
        Frame::Speech {
            utterance: UtteranceId(id),
            trace_id: TraceId::mint(),
            text: text.to_owned(),
            event_observed_at_ms: Some(1_000),
            queued_at_ms: 1_005,
        }
    }

    fn started(id: u64) -> Frame {
        Frame::SpeechStarted {
            utterance: UtteranceId(id),
            at_ms: 1_050,
        }
    }

    fn ended(id: u64, ending: UtteranceEnding) -> Frame {
        Frame::SpeechEnded {
            utterance: UtteranceId(id),
            ending,
        }
    }

    fn collector(frames: Vec<Frame>) -> SpeechCollector {
        collector_on(Timeline::new(), frames)
    }

    /// A collector on `timeline` whose reader has passed `frames` on.
    fn collector_on(timeline: Timeline, frames: Vec<Frame>) -> SpeechCollector {
        let (speech, mut script) = scripted(timeline);
        script.feed(frames);
        speech
    }

    fn traced(id: u64, text: &str, trace: TraceId) -> Frame {
        Frame::Speech {
            utterance: UtteranceId(id),
            trace_id: trace,
            text: text.to_owned(),
            event_observed_at_ms: Some(1_000),
            queued_at_ms: 1_005,
        }
    }

    /// `report` with each timeline line's elapsed time removed, which
    /// varies from run to run.
    fn without_times(report: &str) -> String {
        report
            .lines()
            .map(|line| match line.strip_prefix('+') {
                Some(rest) => rest.split_once("ms ").map_or(line, |(_, text)| text),
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn failure(run: impl FnOnce()) -> SpeechFailure {
        let payload = catch_unwind(AssertUnwindSafe(run)).expect_err("the assertion fails");
        *payload
            .downcast::<SpeechFailure>()
            .expect("the assertion fails with a SpeechFailure")
    }

    fn fails(run: impl FnOnce()) -> String {
        failure(run).message
    }

    #[test]
    fn an_exact_sequence_heard_in_full_passes() {
        let mut speech = collector(vec![
            queued(1, "first"),
            queued(2, ""),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect(&["first", ""]);
        speech.expect_nothing_more(None);
    }

    #[test]
    fn an_utterance_in_between_fails_naming_where() {
        let mut speech = collector(vec![
            queued(1, "first"),
            queued(2, "stray"),
            queued(3, "second"),
        ]);
        let message = fails(move || speech.expect(&["first", "second"]));
        assert!(
            message.contains("first difference: utterance 1, character 1"),
            "{message}"
        );
    }

    #[test]
    fn a_text_that_only_contains_the_expected_text_fails() {
        let mut speech = collector(vec![queued(1, "Rate slider 119")]);
        let message = fails(move || speech.expect(&["Rate slider 19"]));
        assert!(message.contains("character 13"), "{message}");
    }

    #[test]
    fn every_utterance_must_end_as_expected_not_only_the_last() {
        let mut speech = collector(vec![
            queued(1, "first"),
            queued(2, "second"),
            ended(1, UtteranceEnding::Cancelled),
            ended(2, UtteranceEnding::Completed),
        ]);
        fails(move || speech.expect(&["first", "second"]));

        let mut speech = collector(vec![
            queued(1, "first"),
            queued(2, "second"),
            ended(1, UtteranceEnding::Cancelled),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&[cut_off("first"), heard("second")]);
    }

    #[test]
    fn an_optional_utterance_not_said_passes_and_is_reported_absent() {
        let mut speech = collector(vec![
            queued(1, "list"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&[optional("Working on it..."), heard("list")]);
        speech.expect_nothing_more(None);
        let outcomes: Vec<String> = speech
            .optional_outcomes()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(outcomes, ["\"Working on it...\": absent"]);
    }

    #[test]
    fn an_optional_utterance_heard_in_full_passes_and_is_reported_in_full() {
        let mut speech = collector(vec![
            queued(1, "Working on it..."),
            ended(1, UtteranceEnding::Completed),
            queued(2, "list"),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&[optional("Working on it..."), heard("list")]);
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"Working on it...\": in full"
        );
    }

    #[test]
    fn an_optional_repeat_cut_off_by_the_utterance_it_repeats_passes_and_is_reported_cut_off() {
        let mut speech = collector(vec![
            queued(1, "title window"),
            queued(2, "title terminal"),
            queued(3, "title terminal"),
            queued(4, "blank"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Cancelled),
            ended(3, UtteranceEnding::Completed),
            ended(4, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&[
            heard("title window"),
            optional("title terminal"),
            heard("title terminal"),
            heard("blank"),
        ]);
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"title terminal\": cut off"
        );
    }

    /// A step whose speech may begin with a second suggestions-closed
    /// sound, which may arrive before its keys are sent.
    fn tab_after_escape() -> [Expected; 2] {
        [
            optional("sound: suggestions-closed"),
            heard("account button"),
        ]
    }

    #[test]
    fn a_leading_optional_utterance_arrived_before_the_keys_is_matched_by_the_step() {
        let (mut speech, mut script) = scripted(Timeline::new());
        script.feed(vec![
            queued(1, "sound: suggestions-closed"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.require_asserted_before("before the keys [\"tab\"]", &tab_after_escape());
        script.feed(vec![
            queued(2, "account button"),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&tab_after_escape());
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"sound: suggestions-closed\": in full"
        );
    }

    #[test]
    fn other_speech_arrived_before_the_keys_fails_the_check_despite_a_leading_optional() {
        let mut speech = collector(vec![
            queued(1, "sound: suggestions-closed"),
            queued(2, "Search box"),
        ]);
        let message = fails(move || {
            speech.require_asserted_before("before the keys [\"tab\"]", &tab_after_escape());
        });
        assert_eq!(
            message,
            "harness error: utterances no assertion matched were waiting before the keys [\"tab\"]: [\"sound: suggestions-closed\", \"Search box\"]"
        );
    }

    #[test]
    fn a_leading_optional_utterance_arrived_after_the_keys_is_matched_in_sequence() {
        let (mut speech, mut script) = scripted(Timeline::new());
        speech.require_asserted_before("before the keys [\"tab\"]", &tab_after_escape());
        script.feed(vec![
            queued(1, "sound: suggestions-closed"),
            queued(2, "account button"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&tab_after_escape());
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"sound: suggestions-closed\": in full"
        );
    }

    #[test]
    fn a_leading_optional_utterance_never_said_passes_the_check_and_the_step() {
        let (mut speech, mut script) = scripted(Timeline::new());
        speech.require_asserted_before("before the keys [\"tab\"]", &tab_after_escape());
        script.feed(vec![
            queued(1, "account button"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&tab_after_escape());
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"sound: suggestions-closed\": absent"
        );
    }

    #[test]
    fn more_repeats_than_an_optional_utterance_declares_fail() {
        let mut speech = collector(vec![
            queued(1, "title window"),
            queued(2, "title terminal"),
            queued(3, "title terminal"),
            queued(4, "title terminal"),
            queued(5, "blank"),
        ]);
        let message = fails(move || {
            speech.expect_sequence(&[
                heard("title window"),
                optional("title terminal"),
                heard("title terminal"),
                heard("blank"),
            ]);
        });
        assert!(
            message.contains("first difference: utterance 3, character 0"),
            "{message}"
        );
    }

    #[test]
    fn two_repeats_of_an_optional_utterance_declared_twice_pass_and_are_reported() {
        let mut speech = collector(vec![
            queued(1, "title window"),
            queued(2, "title terminal"),
            queued(3, "title terminal"),
            queued(4, "title terminal"),
            queued(5, "blank"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Cancelled),
            ended(3, UtteranceEnding::Cancelled),
            ended(4, UtteranceEnding::Completed),
            ended(5, UtteranceEnding::Completed),
        ]);
        speech.expect_sequence(&[
            heard("title window"),
            optional_up_to("title terminal", 2),
            heard("title terminal"),
            heard("blank"),
        ]);
        speech.expect_nothing_more(None);
        assert_eq!(
            speech.optional_outcomes()[0].to_string(),
            "\"title terminal\": 2 times: cut off, cut off"
        );
    }

    #[test]
    fn an_optional_utterance_followed_only_by_its_own_text_is_a_harness_error() {
        let mut speech = collector(Vec::new());
        let payload = catch_unwind(AssertUnwindSafe(move || {
            speech.expect_sequence(&[optional("title terminal"), heard("title terminal")]);
        }))
        .expect_err("the assertion fails");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some(
                "harness error: the optional utterance \"title terminal\" is not followed by an utterance of other text that is always said"
            )
        );
    }

    #[test]
    fn an_absent_optional_repeat_leaves_its_one_required_copy_to_be_heard_in_full() {
        let mut speech = collector(vec![
            queued(1, "title window"),
            queued(2, "title terminal"),
            queued(3, "blank"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Cancelled),
            ended(3, UtteranceEnding::Completed),
        ]);
        let message = fails(move || {
            speech.expect_sequence(&[
                heard("title window"),
                optional("title terminal"),
                heard("title terminal"),
                heard("blank"),
            ]);
        });
        assert_eq!(
            message,
            "the utterance \"title terminal\" was expected to end Completed, but ended Cancelled"
        );
    }

    #[test]
    fn other_text_in_an_optional_utterance_s_place_fails() {
        let mut speech = collector(vec![
            queued(1, "Working on it"),
            queued(2, "list"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Completed),
        ]);
        let message =
            fails(move || speech.expect_sequence(&[optional("Working on it..."), heard("list")]));
        assert!(
            message.contains("first difference: utterance 0, character 0"),
            "{message}"
        );
    }

    #[test]
    fn an_utterance_interrupted_once_it_has_started_is_asserted_in_two_steps() {
        let mut speech = collector(vec![
            queued(1, "long line"),
            started(1),
            ended(1, UtteranceEnding::Cancelled),
        ]);
        let playing = speech.expect_started("long line");
        speech.expect_ended(&playing, Ending::Cancelled);
        speech.expect_nothing_more(None);
    }

    #[test]
    fn anything_said_after_the_last_assertion_fails_the_scenario() {
        let mut speech = collector(vec![
            queued(1, "expected"),
            ended(1, UtteranceEnding::Completed),
            queued(2, "extra"),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect(&["expected"]);
        let message = fails(move || speech.expect_nothing_more(Some(7)));
        assert!(message.contains("\"extra\""), "{message}");
    }

    #[test]
    fn collect_only_collects_unasserted_speech_for_the_measure_set_instead_of_failing() {
        let mut speech = collector(vec![queued(1, "first"), queued(2, "second")]);
        speech.collect_only();
        speech.require_all_asserted("before keys tab");
        let taken: Vec<String> = speech
            .take_all_queued(Some(3))
            .into_iter()
            .map(|heard| heard.text)
            .collect();
        assert_eq!(taken, vec!["first", "second"]);
        speech.expect_nothing_more(Some(3));
    }

    #[test]
    fn an_input_injected_past_unmatched_speech_is_a_harness_error() {
        let mut speech = collector(vec![queued(1, "unread")]);
        let message = fails(move || speech.require_all_asserted("before keys tab"));
        assert!(message.contains("harness error"), "{message}");
    }

    #[test]
    fn an_utterance_heard_in_full_before_the_step_meant_to_cut_it_off_is_a_harness_error() {
        let mut speech = collector(vec![
            queued(1, "long line"),
            started(1),
            ended(1, UtteranceEnding::Cancelled),
        ]);
        let playing = speech.expect_started("long line");
        speech.require_not_completed(&playing, "when the window was in front");
        speech.expect_ended(&playing, Ending::Cancelled);

        let mut speech = collector(vec![
            queued(1, "long line"),
            started(1),
            ended(1, UtteranceEnding::Completed),
        ]);
        let playing = speech.expect_started("long line");
        let message = fails(move || {
            speech.require_not_completed(&playing, "when the window was in front");
        });
        assert!(
            message.starts_with(
                "harness error: \"long line\" had been heard in full when the window was in front"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_pause_and_a_resume_are_asserted_in_order_apart_from_speech() {
        let paused = |paused| Frame::SpeechPaused { paused, at_ms: 0 };
        let mut speech = collector(vec![
            queued(1, "held"),
            paused(true),
            paused(false),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect_paused();
        speech.expect_resumed();
        speech.expect(&["held"]);
        speech.require_all_asserted("after the resume");
        let mut speech = collector(vec![paused(false)]);
        let message = fails(move || speech.expect_paused());
        assert!(message.contains("expected to be paused"), "{message}");
    }

    #[test]
    fn a_pause_no_step_asserted_is_left_behind_at_the_next_input() {
        let mut speech = collector(vec![Frame::SpeechPaused {
            paused: true,
            at_ms: 0,
        }]);
        speech.require_all_asserted("before keys tab");
        let message = fails(move || speech.expect_paused());
        assert!(message.contains("was not paused"), "{message}");
    }

    #[test]
    fn a_sound_played_at_once_is_an_utterance_of_its_own() {
        let mut speech = collector(vec![Frame::Sound {
            indication: "exit".to_owned(),
            at_ms: 0,
        }]);
        speech.expect(&["sound: exit"]);
    }

    #[test]
    fn latency_rows_measure_from_the_event() {
        let mut speech = collector(vec![
            queued(1, "first"),
            started(1),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect(&["first"]);
        assert_eq!(
            speech.latency_rows(),
            vec![LatencyRow {
                step: None,
                text: "first".to_owned(),
                event_to_queue_ms: 5,
                event_to_audio_ms: Some(50),
            }]
        );
    }

    /// Every frame sent before the connection's end is taken, though the
    /// reader has not passed it on when the wait begins: the last
    /// utterance's audio start counts in the latency rows.
    #[test]
    fn the_speech_up_to_the_connections_end_is_taken() {
        let (mut speech, script) = scripted(Timeline::new());
        for frame in [queued(1, "last"), started(1)] {
            script
                .feed
                .send(Some(frame))
                .expect("the reader is running");
        }
        script.feed.send(None).expect("the reader is running");
        assert_eq!(speech.take_until_the_end(), Ok(()));
        assert_eq!(
            speech.latency_rows(),
            vec![LatencyRow {
                step: None,
                text: "last".to_owned(),
                event_to_queue_ms: 5,
                event_to_audio_ms: Some(50),
            }]
        );
    }

    /// A connection that does not end within the step timeout is reported
    /// as a problem of the run, not waited on further.
    #[test]
    fn a_connection_that_does_not_end_is_reported() {
        let (mut speech, _script) = scripted(Timeline::new());
        assert_eq!(
            speech.take_until_the_end(),
            Err(format!(
                "the speech connection did not end within {SHORT:?} after Verbatim quit, so its last frames may be missing from the latency report"
            ))
        );
    }

    #[test]
    fn a_failure_prints_the_sequences_the_steps_trace_and_the_failed_step_in_full() {
        let timeline = Timeline::new();
        let stray = TraceId::mint();
        let (mut speech, mut script) = scripted(timeline.clone());
        script.feed(vec![
            queued(1, "first"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect(&["first"]);
        timeline.push_keys(&["downarrow"]);
        script.feed(vec![traced(2, "stray", stray)]);
        let failed = failure(move || speech.expect(&["second"]));
        assert_eq!(failed.trace, Some(stray));

        let lines = [format!(
            "stderr.log: 2026-10-09T10:00:00.300000Z  INFO verbatim: gesture trace_id={stray}"
        )];
        assert_eq!(
            without_times(&failed.report(&timeline, Ok(&lines))),
            format!(
                "speech did not match\n\
                 expected: [\"second\"]\n\
                 actual:   [\"stray\"]\n\
                 first difference: utterance 0, character 1: expected \"second\", got \"stray\"\n\
                 \n\
                 the failing step's trace, {stray}, from Verbatim's log, Core's flight recorder and the outposts' logs, in time order:\n\
                 stderr.log: 2026-10-09T10:00:00.300000Z  INFO verbatim: gesture trace_id={stray}\n\
                 \n\
                 timeline, each earlier step on one line and the failing step in full:\n\
                 start, said \"first\"\n\
                 keys [downarrow]\n\
                 speech \"stray\""
            )
        );
    }

    #[test]
    fn an_utterance_never_queued_is_traced_by_the_steps_newest_utterance_or_by_none() {
        let timeline = Timeline::new();
        let heard_first = TraceId::mint();
        let (mut speech, mut script) = scripted(timeline.clone());
        timeline.push_keys(&["downarrow"]);
        script.feed(vec![
            traced(1, "first", heard_first),
            ended(1, UtteranceEnding::Completed),
        ]);
        let failed = failure(move || speech.expect(&["first", "second"]));
        assert_eq!(failed.trace, Some(heard_first));
        assert_eq!(
            trace_lines_in(&failed.report(&timeline, Ok(&[]))),
            [format!(
                "the failing step's trace, {heard_first}: no log line carries it"
            )]
        );

        let timeline = Timeline::new();
        let mut speech = collector_on(
            timeline.clone(),
            vec![queued(1, "before"), ended(1, UtteranceEnding::Completed)],
        );
        speech.expect(&["before"]);
        timeline.push_keys(&["downarrow"]);
        let failed = failure(move || speech.expect(&["after"]));
        assert_eq!(failed.trace, None);
        assert_eq!(
            trace_lines_in(&failed.report(&timeline, Ok(&[]))),
            [
                "the failing step's trace: none, since the step heard nothing that carries a trace ID"
            ]
        );
    }

    /// The lines of a failure's report that say the failing step's trace.
    fn trace_lines_in(report: &str) -> Vec<String> {
        report
            .lines()
            .filter(|line| line.starts_with("the failing step's trace"))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn compare_mode_records_each_step_s_expectations_beside_what_was_said() {
        use crate::compare::Verdict;
        use crate::timeline::Said;

        let timeline = Timeline::new();
        let (mut speech, mut script) = scripted(timeline.clone());
        speech.compare_mode();
        // Before any input: the desktop, not compared.
        script.feed(vec![
            queued(1, "Desktop list"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect(&["Desktop list"]);
        speech.require_all_asserted("before the keys");
        // A step declared different, whose assertion on a pause returns at
        // once.
        timeline.push_keys(&["numpad2"]);
        timeline.push_deviation("docs/parity/review.md", "Review cursor columns");
        script.feed(vec![queued(2, "A"), ended(2, UtteranceEnding::Completed)]);
        speech.expect(&["3"]);
        speech.expect_paused();
        speech.require_all_asserted("before the gesture");
        // A gesture with the Verbatim modifier, pressed for NVDA and
        // compared, then the end, where NVDA said nothing.
        timeline.push_gesture("kb:verbatim+v");
        speech.expect(&["Verbatim", "Context menu"]);
        speech.expect_nothing_more(None);
        let steps = speech.take_comparison();

        let verdicts: Vec<(&str, Verdict)> = steps
            .iter()
            .map(|step| (step.input.as_str(), step.verdict()))
            .collect();
        assert_eq!(
            verdicts,
            [
                ("start", Verdict::NotCompared),
                (
                    "keys [numpad2]",
                    Verdict::Declared("Review cursor columns".to_owned())
                ),
                ("gesture kb:verbatim+v", Verdict::Differs),
            ]
        );
        assert_eq!(steps[1].expected, ["3"]);
        assert_eq!(steps[1].heard, [Said::Speech("A".to_owned())]);
        assert_eq!(steps[2].expected, ["Verbatim", "Context menu"]);
        assert_eq!(steps[2].heard, []);
    }

    #[test]
    fn a_double_press_in_compare_mode_is_one_unit_its_first_press_not_waited_for() {
        use crate::compare::{QuietPeriod, ReportHeader, Verdict, render_report};

        let timeline = Timeline::new();
        let (mut speech, mut script) = scripted(timeline.clone());
        speech.compare_mode();
        // The first press's speech is asserted to start, for the second
        // press to cut off: neither it nor the check before the second
        // press waits for NVDA to be quiet, since NVDA counts the second
        // press as a repeat only within its multi-press timeout. The
        // assertion waits only for NVDA's first speech of the step, here
        // already queued.
        timeline.push_keys(&["insert+f10"]);
        script.feed(vec![queued(1, "Name selected")]);
        let selected = speech.expect_started("Name selected");
        speech.require_all_asserted("before the second press");
        assert_eq!(script.requests_sent(), 0, "nothing waited for NVDA");
        timeline.push_keys(&["insert+f10"]);
        script.feed(vec![queued(2, "Copied to clipboard: Name")]);
        speech.expect_ended(&selected, Ending::Cancelled);
        speech.expect(&["Copied to clipboard: Name"]);
        assert_eq!(script.requests_sent(), 1, "the second press's speech waits");
        speech.expect_nothing_more(None);
        let steps = speech.take_comparison();

        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(steps[0].input, "keys [insert+f10]; keys [insert+f10]");
        assert_eq!(steps[0].verdict(), Verdict::Same);
        let header = ReportHeader {
            scenario: "text_box_review_cursor".to_owned(),
            date: "2026-10-15".to_owned(),
            nvda_version: "alpha-58022,732d1ce2".to_owned(),
            pin: "732d1ce2f".to_owned(),
            pin_matches: Some(true),
            quiet: QuietPeriod::from_asked(Duration::from_secs(1)),
        };
        assert_eq!(
            render_report(&header, &steps),
            "Comparing text_box_review_cursor with NVDA\n\
             Date: 2026-10-15\n\
             NVDA: alpha-58022,732d1ce2\n\
             Submodule pin: 732d1ce2f\n\
             The pin is the installed build.\n\
             Quiet period: 1000 ms\n\
             \n\
             Step 1: keys [insert+f10]; keys [insert+f10]\n\
             Verbatim:\n\
             \x20 Name selected\n\
             \x20 Copied to clipboard: Name\n\
             NVDA:\n\
             \x20 Name selected\n\
             \x20 Copied to clipboard: Name\n\
             Note: the next input or action follows at once and cuts this speech off, so the two are compared as one\n\
             Verdict: same\n\
             \n\
             Summary of 1 steps:\n\
             \x20 same: 1\n\
             \x20 differs: 0\n\
             \x20 declared deviation: 0\n\
             \x20 declared, but NVDA agrees: 0\n\
             \x20 not compared: 0\n"
        );
    }

    #[test]
    fn speech_to_interrupt_in_compare_mode_waits_for_nvda_s_first_speech_or_notes_none() {
        let timeline = Timeline::new();
        let (mut speech, mut script) = scripted(timeline.clone());
        speech.compare_mode();
        timeline.push_keys(&["insert+f10"]);
        // Nothing queued: the assertion gives up after the step timeout,
        // with a note, and asks NVDA nothing.
        let _ = speech.expect_started("Name selected");
        assert_eq!(script.requests_sent(), 0);
        speech.require_all_asserted("before the second press");
        timeline.push_keys(&["insert+f10"]);
        script.feed(vec![queued(1, "Copied to clipboard: Name")]);
        speech.expect(&["Copied to clipboard: Name"]);
        let steps = speech.take_comparison();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(
            steps[0].notes,
            [
                "NVDA queued no speech within 50ms for the next input to interrupt",
                "the next input or action follows at once and cuts this speech off, so the two are compared as one",
            ]
        );
    }

    #[test]
    fn an_action_in_compare_mode_starts_a_step_and_the_check_before_it_waits() {
        let timeline = Timeline::new();
        let (mut speech, mut script) = scripted(timeline.clone());
        speech.compare_mode();
        speech.expect(&[]);
        assert_eq!(script.requests_sent(), 1);
        speech.require_all_asserted("before the action");
        assert_eq!(script.requests_sent(), 1, "NVDA was already quiet");
        timeline.push_action("bring forward the window titled \"second\"");
        script.feed(vec![queued(1, "second window")]);
        speech.require_all_asserted("before the keys");
        assert_eq!(
            script.requests_sent(),
            2,
            "the action came since, so the check waits"
        );
        let steps = speech.take_comparison();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(steps[0].input, "bring forward the window titled \"second\"");
        assert_eq!(
            steps[0].heard,
            [crate::timeline::Said::Speech("second window".to_owned())]
        );
    }

    #[test]
    fn a_compare_mode_expectation_waits_for_nvda_and_the_input_after_it_does_not_wait_again() {
        let timeline = Timeline::new();
        let (mut speech, mut script) = scripted(timeline.clone());
        speech.compare_mode();
        script.feed(vec![
            queued(1, "Desktop list"),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect(&["Desktop list"]);
        assert_eq!(script.requests_sent(), 1, "the expectation waits for NVDA");
        speech.require_all_asserted("before the keys");
        assert_eq!(script.requests_sent(), 1, "NVDA was already quiet");
        timeline.push_keys(&["tab"]);
        speech.require_all_asserted("before the keys");
        assert_eq!(script.requests_sent(), 2, "the keys came since");
    }

    #[test]
    fn compare_mode_refuses_what_needs_verbatim_s_own_speech() {
        let (mut speech, _script) = scripted(Timeline::new());
        speech.compare_mode();
        let message = fails(move || {
            speech.take_until("flood line 100", SHORT);
        });
        assert_eq!(
            message,
            "harness error: SpeechCollector::take_until needs Verbatim's speech and has no NVDA equivalent, so this scenario cannot be compared"
        );
    }
}
