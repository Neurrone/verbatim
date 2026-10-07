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
//! that the utterance it interrupts has started to play.
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
//! never calls it after its subscribe: its one request, `AwaitIdle`, is
//! sent with [`ControlClient::send`] and its reply read among the frames.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;
use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, Request};
use verbatim_model::{UtteranceEnding, UtteranceId};

use crate::timeline::Timeline;

/// How long an assertion waits for each expected utterance to be queued,
/// and then for each to end: a bound on a hang, not a latency expectation.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long [`SpeechCollector::expect_nothing_more`] lets Verbatim take to
/// become idle: a bound on a hang.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a read for frames already on their way waits before deciding
/// there are none: the collector drains what has arrived before an input
/// is injected. Not a wait for anything to happen.
const DRAIN_READ: Duration = Duration::from_millis(1);

/// Where a [`SpeechCollector`] reads frames from: the live, subscribed
/// control connection, or a scripted sequence in this module's unit tests.
trait FrameSource {
    fn next_frame(&mut self) -> io::Result<Frame>;
    fn send(&mut self, request: Request) -> io::Result<u64>;
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()>;
}

impl FrameSource for ControlClient {
    fn next_frame(&mut self) -> io::Result<Frame> {
        ControlClient::next_frame(self)
    }

    fn send(&mut self, request: Request) -> io::Result<u64> {
        ControlClient::send(self, request)
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        ControlClient::set_read_timeout(self, Some(timeout))
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

/// One expected utterance: its exact text and how it must end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The exact text.
    pub text: String,
    /// How it must end.
    pub ending: Ending,
}

/// An utterance heard in full, for [`SpeechCollector::expect_sequence`].
#[must_use]
pub fn heard(text: &str) -> Expected {
    Expected {
        text: text.to_owned(),
        ending: Ending::Completed,
    }
}

/// An utterance cut off, for [`SpeechCollector::expect_sequence`].
#[must_use]
pub fn cut_off(text: &str) -> Expected {
    Expected {
        text: text.to_owned(),
        ending: Ending::Cancelled,
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
    /// injected, such as for startup speech.
    pub step: Option<String>,
    /// The utterance's text.
    pub text: String,
    /// From the event to the utterance being queued, in milliseconds.
    pub event_to_queue_ms: u64,
    /// From the event to its audio starting, in milliseconds; `None` when
    /// it never played.
    pub event_to_audio_ms: Option<u64>,
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
    source: Box<dyn FrameSource>,
    /// The scenario's shared action-and-speech log (see [`crate::timeline`]).
    timeline: Timeline,
    /// The read timeout the connection normally has.
    read_timeout: Duration,
    /// How long an assertion waits; [`STEP_TIMEOUT`] outside unit tests.
    step_timeout: Duration,
    /// Utterances read and not yet matched by an assertion, oldest first.
    pending: VecDeque<Utterance>,
    /// Every utterance matched so far, in order, for failure messages.
    matched: Vec<String>,
    texts: HashMap<UtteranceId, String>,
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
}

impl SpeechCollector {
    /// Subscribes to speech on `control`, which becomes exclusively owned
    /// by the collector, pushing every utterance it reads to `timeline`.
    /// The first speech subscription to a Verbatim also receives the
    /// speech it queued before anyone subscribed, its startup speech.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscribe request fails.
    pub fn subscribe(mut control: ControlClient, timeline: Timeline) -> io::Result<Self> {
        ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        Ok(Self::from_source(
            Box::new(control),
            timeline,
            crate::agent_client::CONTROL_READ_TIMEOUT,
            STEP_TIMEOUT,
        ))
    }

    fn from_source(
        source: Box<dyn FrameSource>,
        timeline: Timeline,
        read_timeout: Duration,
        step_timeout: Duration,
    ) -> Self {
        Self {
            source,
            timeline,
            read_timeout,
            step_timeout,
            pending: VecDeque::new(),
            matched: Vec::new(),
            texts: HashMap::new(),
            endings: HashMap::new(),
            unended: BTreeSet::new(),
            started: BTreeSet::new(),
            timings: HashMap::new(),
            order: Vec::new(),
            next_sound: u64::MAX,
        }
    }

    /// Records one frame on the timeline and in the collector's state.
    fn absorb(&mut self, frame: Frame) -> Absorbed {
        match frame {
            Frame::Speech {
                utterance,
                text,
                event_observed_at_ms,
                queued_at_ms,
                ..
            } => {
                self.timeline.push_utterance(&text);
                self.texts.insert(utterance, text.clone());
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
                self.timeline.push_audio_started(self.text_of(utterance));
                self.started.insert(utterance);
                if let Some(timing) = self.timings.get_mut(&utterance) {
                    timing.audio_started_at_ms = Some(at_ms);
                }
                Absorbed::Other
            }
            Frame::SpeechEnded { utterance, ending } => {
                self.timeline.push_ended(self.text_of(utterance), &ending);
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
                self.timeline.push_utterance(&text);
                self.texts.insert(utterance, text.clone());
                self.started.insert(utterance);
                self.endings.insert(utterance, UtteranceEnding::Completed);
                Absorbed::Utterance(Utterance { utterance, text })
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

    /// Reads one frame, keeping any utterance it announces in
    /// [`Self::pending`]. A read timeout is `Ok(None)`.
    fn read_one(&mut self) -> io::Result<Option<Absorbed>> {
        match self.source.next_frame() {
            Ok(frame) => {
                let absorbed = self.absorb(frame);
                if let Absorbed::Utterance(utterance) = &absorbed {
                    self.pending.push_back(utterance.clone());
                }
                Ok(Some(absorbed))
            }
            Err(error) if is_read_timeout(&error) => Ok(None),
            Err(error) => Err(error),
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
            if let Err(error) = self.read_one() {
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
            if let Err(error) = self.read_one() {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
    }

    /// Panics with `message`, then the utterances matched so far and the
    /// timeline.
    fn fail(&self, message: &str) -> ! {
        panic!(
            "{message}\nutterances matched so far, in order: {:?}\ntimeline:\n{}",
            self.matched,
            self.timeline.render()
        );
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
        let mut actual: Vec<Utterance> = Vec::new();
        for (index, want) in expected.iter().enumerate() {
            let deadline = Instant::now() + timeout;
            let Some(got) = self.next_utterance(deadline) else {
                self.fail(&format!(
                    "{}\nutterance {index} ({:?}) was not queued within {timeout:?}",
                    describe_mismatch(expected, &actual),
                    want.text
                ));
            };
            actual.push(got.clone());
            if got.text != want.text {
                self.fail(&describe_mismatch(expected, &actual));
            }
        }
        for (want, got) in expected.iter().zip(&actual) {
            let deadline = Instant::now() + timeout;
            let ending = self.ending_by(got.utterance, deadline);
            let ok = matches!(
                (want.ending, &ending),
                (Ending::Completed, Some(UtteranceEnding::Completed))
                    | (Ending::Cancelled, Some(UtteranceEnding::Cancelled))
            );
            if !ok {
                self.fail(&format!(
                    "the utterance {:?} was expected to end {:?}, but {}",
                    want.text,
                    want.ending,
                    ending.map_or_else(
                        || format!("had not ended after {timeout:?}"),
                        |ending| format!("ended {ending:?}")
                    )
                ));
            }
        }
        self.matched
            .extend(actual.into_iter().map(|utterance| utterance.text));
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
        let deadline = Instant::now() + self.step_timeout;
        let expected = [heard(text)];
        let Some(got) = self.next_utterance(deadline) else {
            self.fail(&format!(
                "{}\nthe utterance {text:?} was not queued within {:?}",
                describe_mismatch(&expected, &[]),
                self.step_timeout
            ));
        };
        if got.text != text {
            self.fail(&describe_mismatch(&expected, &[got]));
        }
        while !self.started.contains(&got.utterance) {
            if let Some(ending) = self.endings.get(&got.utterance) {
                self.fail(&format!("{text:?} ended {ending:?} without playing"));
            }
            if Instant::now() >= deadline {
                self.fail(&format!(
                    "{text:?} did not start playing within {:?}",
                    self.step_timeout
                ));
            }
            if let Err(error) = self.read_one() {
                self.fail(&format!("the speech connection failed: {error}"));
            }
        }
        self.matched.push(got.text.clone());
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
        let expected: Vec<Expected> = texts.iter().map(|text| heard(text)).collect();
        let mut actual: Vec<Utterance> = Vec::new();
        for (index, want) in expected.iter().enumerate() {
            let deadline = Instant::now() + self.step_timeout;
            let Some(got) = self.next_utterance(deadline) else {
                self.fail(&format!(
                    "{}\nutterance {index} ({:?}) was not queued within {:?}",
                    describe_mismatch(&expected, &actual),
                    want.text,
                    self.step_timeout
                ));
            };
            actual.push(got.clone());
            if got.text != want.text {
                self.fail(&describe_mismatch(&expected, &actual));
            }
        }
        self.matched
            .extend(actual.iter().map(|utterance| utterance.text.clone()));
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
        let deadline = Instant::now() + self.step_timeout;
        let ended = self.ending_by(heard.utterance, deadline);
        let ok = matches!(
            (ending, &ended),
            (Ending::Completed, Some(UtteranceEnding::Completed))
                | (Ending::Cancelled, Some(UtteranceEnding::Cancelled))
        );
        if !ok {
            self.fail(&format!(
                "the utterance {:?} was expected to end {ending:?}, but {}",
                heard.text,
                ended.map_or_else(
                    || "had not ended".to_owned(),
                    |ended| format!("ended {ended:?}")
                )
            ));
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
        let mut taken = Vec::new();
        loop {
            let deadline = Instant::now() + timeout;
            let Some(got) = self.next_utterance(deadline) else {
                self.fail(&format!(
                    "{until:?} was not queued; nothing was queued for {timeout:?} after {} utterances",
                    taken.len()
                ));
            };
            self.matched.push(got.text.clone());
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

    /// How `heard` ended, waiting up to `timeout` for it to end; `None`
    /// when it had not.
    pub fn ending_of(&mut self, heard: &Heard, timeout: Duration) -> Option<UtteranceEnding> {
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
        self.await_idle(after_input);
        self.require_all_asserted("after the scenario's last assertion");
        let unended: Vec<UtteranceId> = self.unended.iter().copied().collect();
        for utterance in unended {
            let deadline = Instant::now() + self.step_timeout;
            if self.ending_by(utterance, deadline).is_none() {
                self.fail(&format!(
                    "{:?} had not ended {:?} after the scenario's last assertion",
                    self.text_of(utterance),
                    self.step_timeout
                ));
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
        self.await_idle(after_input);
        let taken: Vec<Heard> = self
            .pending
            .drain(..)
            .map(|utterance| Heard {
                text: utterance.text,
                utterance: utterance.utterance,
            })
            .collect();
        self.matched
            .extend(taken.iter().map(|heard| heard.text.clone()));
        taken
    }

    /// Asks Verbatim, on this connection, to answer once it has handled
    /// the input numbered `after_input` and everything before it, and is
    /// idle, and reads every frame until the answer: every utterance
    /// queued before it is then read.
    fn await_idle(&mut self, after_input: Option<u64>) {
        let id = match self.source.send(Request::AwaitIdle {
            after_input,
            timeout_ms: u64::try_from(IDLE_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
        }) {
            Ok(id) => id,
            Err(error) => self.fail(&format!(
                "could not ask Verbatim whether it is idle: {error}"
            )),
        };
        let deadline = Instant::now() + IDLE_TIMEOUT + self.read_timeout * 2;
        loop {
            match self.read_one() {
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
    /// scenario never moves on past speech it has not asserted. Called
    /// before every input the harness injects.
    ///
    /// # Panics
    ///
    /// Panics, naming the unmatched utterances, if there are any.
    pub fn require_all_asserted(&mut self, when: &str) {
        if let Err(error) = self.source.set_read_timeout(DRAIN_READ) {
            self.fail(&format!("could not read the speech connection: {error}"));
        }
        let drained = loop {
            match self.read_one() {
                Ok(Some(_)) => {}
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        let restored = self.source.set_read_timeout(self.read_timeout);
        if let Err(error) = drained.and(restored) {
            self.fail(&format!("the speech connection failed: {error}"));
        }
        if !self.pending.is_empty() {
            let unmatched: Vec<&str> = self.pending.iter().map(|u| u.text.as_str()).collect();
            self.fail(&format!(
                "harness error: utterances no assertion matched were waiting {when}: {unmatched:?}"
            ));
        }
    }

    /// One row per utterance heard so far that carries the time of the
    /// event behind it, in the order they were queued: the latency report
    /// a run's artifacts keep.
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

/// The expected and actual sequences, `{:?}` escaped, with the index of
/// the first utterance that differs and, when both have one there, the
/// first character that differs in it.
fn describe_mismatch(expected: &[Expected], actual: &[Utterance]) -> String {
    let expected_texts: Vec<&str> = expected.iter().map(|item| item.text.as_str()).collect();
    let actual_texts: Vec<&str> = actual.iter().map(|item| item.text.as_str()).collect();
    let mut out = String::new();
    let _ = writeln!(out, "speech did not match");
    let _ = writeln!(out, "expected: {expected_texts:?}");
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

fn is_read_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use verbatim_model::TraceId;

    use super::*;

    /// Hands out scripted frames in order, then reports a read timeout on
    /// every later read, as an idle connection would. `AwaitIdle` is
    /// answered at once, after the frames scripted before it.
    struct Scripted {
        frames: VecDeque<Frame>,
        next_id: u64,
    }

    impl FrameSource for Scripted {
        fn next_frame(&mut self) -> io::Result<Frame> {
            self.frames
                .pop_front()
                .ok_or_else(|| io::ErrorKind::TimedOut.into())
        }

        fn send(&mut self, _request: Request) -> io::Result<u64> {
            self.next_id += 1;
            self.frames.push_back(Frame::Reply {
                to: self.next_id,
                payload: verbatim_control::protocol::ReplyPayload::Idle,
            });
            Ok(self.next_id)
        }

        fn set_read_timeout(&mut self, _timeout: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    const SHORT: Duration = Duration::from_millis(50);

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
        SpeechCollector::from_source(
            Box::new(Scripted {
                frames: frames.into(),
                next_id: 0,
            }),
            Timeline::new(),
            SHORT,
            SHORT,
        )
    }

    fn fails(run: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(run)).expect_err("the assertion fails");
        payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default()
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
            message.contains("first difference: utterance 1, character 0"),
            "{message}"
        );
    }

    #[test]
    fn a_text_that_only_contains_the_expected_text_fails() {
        let mut speech = collector(vec![queued(1, "Rate slider 119")]);
        let message = fails(move || speech.expect(&["Rate slider 19"]));
        assert!(message.contains("character 12"), "{message}");
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
    fn an_input_injected_past_unmatched_speech_is_a_harness_error() {
        let mut speech = collector(vec![queued(1, "unread")]);
        let message = fails(move || speech.require_all_asserted("before keys tab"));
        assert!(message.contains("harness error"), "{message}");
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
}
