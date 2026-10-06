//! Speech assertions: collects speech frames from a dedicated, never-shared
//! control-plane connection and checks that the utterances a scenario cares
//! about were spoken, in order, and heard in full.
//!
//! The dedicated connection matters. [`ControlClient::request`] discards
//! any frame that is not the reply it is waiting for, including speech
//! frames — so a connection also used to send gestures or keys would
//! silently lose utterances that happened to arrive while a request was in
//! flight. [`SpeechCollector`] never calls `request` again after its
//! initial subscribe, so nothing it reads is ever thrown away.
//!
//! Every utterance Verbatim queues is announced by a [`Frame::Speech`] and
//! later ends with exactly one [`Frame::SpeechEnded`] (decision D17): it
//! completed when the audio device played its last frame, or it was
//! cancelled, or it failed. An `expect_*` assertion matches queued text and
//! then waits for the matched utterance's ending, and fails unless it
//! completed, so a passing assertion means the speech was heard in full and
//! the next input the scenario injects cannot cut it off.
//! [`SpeechCollector::wait_until_quiet`] waits for every utterance to end,
//! which every scenario does before its teardown.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, Request};
use verbatim_model::{UtteranceEnding, UtteranceId};

use crate::timeline::Timeline;

/// How long an assertion waits for a matched utterance to end once it has
/// been queued. Generous enough for any announcement to play out behind
/// whatever was queued before it.
const ENDING_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a [`SpeechCollector`] reads frames from: the live, subscribed
/// control connection, or a scripted sequence in this module's unit tests.
trait FrameSource {
    fn next_frame(&mut self) -> io::Result<Frame>;
}

impl FrameSource for ControlClient {
    fn next_frame(&mut self) -> io::Result<Frame> {
        ControlClient::next_frame(self)
    }
}

/// A queued utterance: what the assertions match against.
#[derive(Debug)]
struct Utterance {
    utterance: UtteranceId,
    text: String,
}

/// Subscribes to and collects speech frames on its own control-plane
/// connection.
pub struct SpeechCollector {
    source: Box<dyn FrameSource>,
    /// The scenario's shared action-and-speech log: every utterance this
    /// collector reads is pushed here as it arrives, alongside whatever
    /// gestures and keys [`crate::Scenario`] injected on the same clone of
    /// this handle, so a failure can print both interleaved in time order.
    /// See [`crate::timeline`].
    timeline: Timeline,
    /// How long an assertion waits for its utterance's ending;
    /// [`ENDING_TIMEOUT`] outside unit tests.
    ending_timeout: Duration,
    /// Queued utterances read while waiting for an ending, not yet offered
    /// to an assertion. The next assertion reads these before anything new,
    /// so speech that arrives during a wait is never lost.
    pending: VecDeque<Utterance>,
    /// The text of every utterance seen, for timeline entries about it.
    texts: HashMap<UtteranceId, String>,
    /// Every ending read so far, whenever it was read.
    endings: HashMap<UtteranceId, UtteranceEnding>,
    /// Utterances queued and not yet ended.
    unended: BTreeSet<UtteranceId>,
    /// When the last utterance was queued or ended.
    last_activity: Instant,
}

impl SpeechCollector {
    /// Subscribes to speech on `control`, which becomes exclusively owned
    /// by the collector from this point on, pushing every utterance it
    /// reads to `timeline` — normally a clone of the same handle the owning
    /// [`crate::Scenario`] records its injected gestures and keys on.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscribe request fails.
    pub fn subscribe(mut control: ControlClient, timeline: Timeline) -> io::Result<Self> {
        ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        Ok(Self::from_source(
            Box::new(control),
            timeline,
            ENDING_TIMEOUT,
        ))
    }

    fn from_source(
        source: Box<dyn FrameSource>,
        timeline: Timeline,
        ending_timeout: Duration,
    ) -> Self {
        Self {
            source,
            timeline,
            ending_timeout,
            pending: VecDeque::new(),
            texts: HashMap::new(),
            endings: HashMap::new(),
            unended: BTreeSet::new(),
            last_activity: Instant::now(),
        }
    }

    /// Records one frame on the timeline and in the collector's state, and
    /// returns it as an [`Utterance`] when it announces a queued utterance.
    /// Event, Reply, and Error frames cannot meaningfully arrive on this
    /// speech-only connection after the subscribe and are ignored.
    fn absorb(&mut self, frame: Frame) -> Option<Utterance> {
        match frame {
            Frame::Speech {
                utterance, text, ..
            } => {
                self.timeline.push_utterance(&text);
                self.texts.insert(utterance, text.clone());
                if !self.endings.contains_key(&utterance) {
                    self.unended.insert(utterance);
                }
                self.last_activity = Instant::now();
                Some(Utterance { utterance, text })
            }
            Frame::SpeechStarted { utterance, .. } => {
                self.timeline.push_audio_started(self.text_of(utterance));
                None
            }
            Frame::SpeechEnded { utterance, ending } => {
                self.timeline.push_ended(self.text_of(utterance), &ending);
                self.unended.remove(&utterance);
                self.endings.insert(utterance, ending);
                self.last_activity = Instant::now();
                None
            }
            _ => None,
        }
    }

    fn text_of(&self, utterance: UtteranceId) -> &str {
        self.texts
            .get(&utterance)
            .map_or("(unknown utterance)", String::as_str)
    }

    /// The next queued utterance: one held back by an earlier wait if there
    /// is one, otherwise whatever one frame read from the connection yields
    /// (`None` when that frame did not announce an utterance).
    ///
    /// # Errors
    ///
    /// Returns the connection's error, including a read timeout, which
    /// callers treat as "nothing yet" and recheck their own deadline.
    fn next_utterance(&mut self) -> io::Result<Option<Utterance>> {
        if let Some(utterance) = self.pending.pop_front() {
            return Ok(Some(utterance));
        }
        let frame = self.source.next_frame()?;
        Ok(self.absorb(frame))
    }

    /// Reads one frame, keeping any utterance it announces for the next
    /// assertion.
    ///
    /// # Errors
    ///
    /// Returns the connection's error, including a read timeout.
    fn read_aside(&mut self) -> io::Result<()> {
        let frame = self.source.next_frame()?;
        if let Some(later) = self.absorb(frame) {
            self.pending.push_back(later);
        }
        Ok(())
    }

    /// Waits for `utterance` to end and returns how it ended, or `None` if
    /// it had not ended within the ending timeout. Utterances queued
    /// meanwhile are kept for the next assertion.
    ///
    /// # Errors
    ///
    /// Returns the connection's error when it fails outright.
    fn await_ending(&mut self, utterance: UtteranceId) -> io::Result<Option<UtteranceEnding>> {
        let deadline = Instant::now() + self.ending_timeout;
        loop {
            if let Some(ending) = self.endings.get(&utterance) {
                return Ok(Some(ending.clone()));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            match self.read_aside() {
                Ok(()) => {}
                Err(error) if is_read_timeout(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Waits for the matched `utterance` to end, and panics, with the
    /// timeline, unless it completed: an utterance an assertion is about
    /// must be heard in full.
    fn expect_heard(&mut self, utterance: &Utterance) {
        match self.await_ending(utterance.utterance) {
            Ok(Some(UtteranceEnding::Completed)) => {}
            Ok(Some(ending)) => panic!(
                "the utterance {:?} was queued but not heard in full: it ended {ending:?}; timeline so far:\n{}",
                utterance.text,
                self.timeline.render()
            ),
            Ok(None) => panic!(
                "the utterance {:?} was queued but did not end within {:?}; timeline so far:\n{}",
                utterance.text,
                self.ending_timeout,
                self.timeline.render()
            ),
            Err(error) => panic!(
                "speech connection failed while waiting for {:?} to end; timeline so far:\n{}\nunderlying error: {error}",
                utterance.text,
                self.timeline.render()
            ),
        }
    }

    /// Waits up to `timeout` for utterances containing each of `matchers`,
    /// in order (case-sensitive substring match), tolerating unrelated
    /// utterances in between, then waits for the utterance that matched the
    /// last of them to be heard in full. Consecutive matchers satisfied by
    /// the same utterance (for example a name fragment and a role word
    /// rendered together) both count without waiting for a second
    /// utterance.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far — one entry per line, in time order — if the
    /// matchers do not all appear in order before `timeout` elapses, if the
    /// last matched utterance does not complete, or if the speech
    /// connection fails outright.
    pub fn expect_in_order(&mut self, matchers: &[&str], timeout: Duration) {
        let _ = self.expect_in_order_capturing(matchers, timeout);
    }

    /// Like [`expect_in_order`](Self::expect_in_order), but returns the full
    /// text of the utterance that satisfied the final matcher, for callers
    /// that need to read a runtime value out of it — for example the numeric
    /// value a slider announces after its name and role ("Rate slider 80")
    /// — rather than asserting a literal they already know.
    ///
    /// # Panics
    ///
    /// As [`expect_in_order`](Self::expect_in_order).
    pub fn expect_in_order_capturing(&mut self, matchers: &[&str], timeout: Duration) -> String {
        let (next, fatal, completing) = self.advance_through(matchers, timeout);
        if let Some(error) = fatal {
            panic!(
                "speech connection failed while waiting for utterance {} of {} ({:?}); timeline so far:\n{}\nunderlying error: {error}",
                next + 1,
                matchers.len(),
                matchers[next],
                self.timeline.render()
            );
        }
        assert!(
            next >= matchers.len(),
            "timed out after {timeout:?} waiting for utterance {} of {} ({:?}); timeline so far:\n{}",
            next + 1,
            matchers.len(),
            matchers[next],
            self.timeline.render()
        );
        let utterance = completing.expect(
            "advance_through returns the completing utterance once every matcher is satisfied",
        );
        self.expect_heard(&utterance);
        utterance.text
    }

    /// Waits for an utterance whose full text differs from `unchanged` (a
    /// value already returned by this method itself, or by
    /// [`expect_in_order_capturing`](Self::expect_in_order_capturing)) and
    /// returns it once heard in full, tolerating both unrelated utterances
    /// and repeats of `unchanged` itself in between — a focus-driven
    /// announcement can legitimately re-fire before the value it is
    /// reporting actually changes.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far if no utterance differing from `unchanged` arrives
    /// before `timeout` elapses, if it does not complete, or if the speech
    /// connection fails outright.
    pub fn expect_change_capturing(&mut self, unchanged: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            match self.next_utterance() {
                Ok(Some(utterance)) if utterance.text != unchanged => {
                    self.expect_heard(&utterance);
                    return utterance.text;
                }
                Ok(_) => {
                    // A repeat of the unchanged value, or not an utterance.
                }
                Err(error) if is_read_timeout(&error) => {
                    // Recheck the deadline below rather than treating a bare
                    // read timeout as failure.
                }
                Err(error) => panic!(
                    "speech connection failed while waiting for a value to change from the previously captured {unchanged:?}; timeline so far:\n{}\nunderlying error: {error}",
                    self.timeline.render()
                ),
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {timeout:?} waiting for a value to change from the previously captured {unchanged:?}; timeline so far:\n{}",
                self.timeline.render()
            );
        }
    }

    /// Asserts that a previously captured runtime value (for example one
    /// returned by [`expect_change_capturing`](Self::expect_change_capturing))
    /// is spoken again verbatim, as a substring of some later utterance, and
    /// heard in full. A dedicated method rather than callers reaching for
    /// `expect_in_order` themselves so the panic message can name the
    /// captured value as what it is.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far if no utterance containing `captured` arrives before
    /// `timeout` elapses, if it does not complete, or if the speech
    /// connection fails outright.
    pub fn expect_captured(&mut self, captured: &str, timeout: Duration) {
        let (_, fatal, completing) = self.advance_through(&[captured], timeout);
        if let Some(error) = fatal {
            panic!(
                "speech connection failed while waiting for the previously captured value {captured:?} to be spoken again; timeline so far:\n{}\nunderlying error: {error}",
                self.timeline.render()
            );
        }
        let Some(utterance) = completing else {
            panic!(
                "timed out after {timeout:?} waiting for the previously captured value {captured:?} to be spoken again; timeline so far:\n{}",
                self.timeline.render()
            );
        };
        self.expect_heard(&utterance);
    }

    /// Waits until every utterance queued so far has ended and nothing new
    /// has been queued or ended for `quiet_for`. Frames already waiting on
    /// the connection are read before deciding, so speech queued just
    /// before the call is not overlooked. Everything spoken up to then is
    /// consumed: no later assertion can be satisfied by it.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline if speech has not gone quiet within
    /// `timeout`, or if the speech connection fails outright.
    pub fn wait_until_quiet(&mut self, quiet_for: Duration, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_aside() {
                Ok(()) if Instant::now() < deadline => continue,
                Ok(()) => {}
                Err(error) if is_read_timeout(&error) => {}
                Err(error) => panic!(
                    "speech connection failed while waiting for speech to go quiet; timeline so far:
{}
underlying error: {error}",
                    self.timeline.render()
                ),
            }
            // Nothing more is waiting on the connection.
            if self.unended.is_empty() && self.last_activity.elapsed() >= quiet_for {
                self.pending.clear();
                return;
            }
            assert!(
                Instant::now() < deadline,
                "speech did not go quiet within {timeout:?}; still unended: {:?}; timeline so far:
{}",
                self.unended
                    .iter()
                    .map(|utterance| self.text_of(*utterance))
                    .collect::<Vec<_>>(),
                self.timeline.render()
            );
        }
    }

    /// The text of the first utterance containing `matcher` heard within
    /// `timeout`, or `None` when none was, for a scenario that searches by
    /// pressing keys until it hears what it is looking for. Utterances that
    /// do not match are consumed.
    ///
    /// # Panics
    ///
    /// Panics with the timeline so far if the speech connection fails
    /// outright.
    pub fn heard_within(&mut self, matcher: &str, timeout: Duration) -> Option<String> {
        match self.advance_through(&[matcher], timeout) {
            (_, Some(error), _) => panic!(
                "speech connection failed while listening for {matcher:?}; timeline so far:
{}
underlying error: {error}",
                self.timeline.render()
            ),
            (_, None, utterance) => utterance.map(|utterance| utterance.text),
        }
    }

    /// Shared loop behind the `expect_*` methods: reads utterances until
    /// every matcher is satisfied, `timeout` elapses, or the connection
    /// fails outright. Returns how many matchers were satisfied, on a fatal
    /// (non-timeout) connection error that error, and — when every matcher
    /// was satisfied — the utterance whose match completed the final one.
    fn advance_through(
        &mut self,
        matchers: &[&str],
        timeout: Duration,
    ) -> (usize, Option<io::Error>, Option<Utterance>) {
        let deadline = Instant::now() + timeout;
        let mut next = 0usize;
        while next < matchers.len() && Instant::now() < deadline {
            match self.next_utterance() {
                Ok(Some(utterance)) => {
                    while next < matchers.len() && utterance.text.contains(matchers[next]) {
                        next += 1;
                    }
                    if next == matchers.len() {
                        return (next, None, Some(utterance));
                    }
                }
                Ok(None) => {}
                Err(error) if is_read_timeout(&error) => {
                    // The fixed per-read socket timeout expired with no
                    // frame; loop back around to recheck the overall
                    // deadline rather than treating this as a failure.
                }
                Err(error) => return (next, Some(error), None),
            }
        }
        (next, None, None)
    }

    /// Every utterance heard so far, one per line, oldest first, prefixed
    /// with its index — derived from the shared timeline's utterance
    /// entries alone, with no gestures or keys interleaved. Failure
    /// messages in this module use the full interleaved
    /// [`Timeline::render`] instead; this narrower view remains for callers
    /// that want speech only.
    #[must_use]
    pub fn transcript(&self) -> String {
        let heard = self.timeline.utterances();
        if heard.is_empty() {
            return "(no utterances heard)".to_owned();
        }
        heard
            .iter()
            .enumerate()
            .map(|(index, text)| format!("{index}: {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn is_read_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Unit coverage for the collector's matching and ending waits, driven from
/// a scripted frame sequence instead of a live connection, and for
/// [`SpeechCollector::transcript`]'s utterance-only formatting. The
/// interleaved-timeline rendering the panic messages print is covered in
/// `timeline.rs`.
#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use verbatim_model::TraceId;

    use super::*;

    /// Hands out scripted frames in order, then reports a read timeout on
    /// every later read, as an idle connection would.
    struct Scripted(VecDeque<Frame>);

    impl FrameSource for Scripted {
        fn next_frame(&mut self) -> io::Result<Frame> {
            self.0
                .pop_front()
                .ok_or_else(|| io::ErrorKind::TimedOut.into())
        }
    }

    const SHORT: Duration = Duration::from_millis(50);

    fn queued(id: u64, text: &str) -> Frame {
        Frame::Speech {
            utterance: UtteranceId(id),
            trace_id: TraceId::mint(),
            text: text.to_owned(),
            event_observed_at_ms: None,
            queued_at_ms: 0,
        }
    }

    fn ended(id: u64, ending: UtteranceEnding) -> Frame {
        Frame::SpeechEnded {
            utterance: UtteranceId(id),
            ending,
        }
    }

    fn collector(frames: Vec<Frame>) -> (SpeechCollector, Timeline) {
        let timeline = Timeline::new();
        let collector = SpeechCollector::from_source(
            Box::new(Scripted(frames.into())),
            timeline.clone(),
            SHORT,
        );
        (collector, timeline)
    }

    #[test]
    fn speech_read_while_waiting_for_an_ending_reaches_the_next_assertion() {
        let (mut speech, timeline) = collector(vec![
            queued(1, "first"),
            queued(2, "second"),
            ended(1, UtteranceEnding::Completed),
            ended(2, UtteranceEnding::Completed),
        ]);
        speech.expect_in_order(&["first"], SHORT);
        speech.expect_in_order(&["second"], SHORT);
        assert!(timeline.render().contains(r#"completed "first""#));
    }

    #[test]
    fn an_assertion_fails_when_its_utterance_is_cut_off() {
        let (mut speech, _) = collector(vec![
            queued(1, "matched"),
            ended(1, UtteranceEnding::Cancelled),
        ]);
        let result = catch_unwind(AssertUnwindSafe(|| {
            speech.expect_in_order(&["matched"], SHORT);
        }));
        assert!(
            result.is_err(),
            "a cancelled utterance was not heard in full"
        );
    }

    #[test]
    fn only_the_matched_utterances_own_ending_counts() {
        for matched_ending in [
            None,
            Some(UtteranceEnding::Cancelled),
            Some(UtteranceEnding::Failed("no audio device".to_owned())),
        ] {
            let mut frames = vec![
                queued(1, "matched"),
                queued(2, "other"),
                ended(2, UtteranceEnding::Completed),
            ];
            frames.extend(matched_ending.clone().map(|ending| ended(1, ending)));
            let (mut speech, _) = collector(frames);
            let result = catch_unwind(AssertUnwindSafe(|| {
                speech.expect_in_order(&["matched"], SHORT);
            }));
            assert!(
                result.is_err(),
                "another utterance completing does not make {matched_ending:?} heard"
            );
        }
    }

    #[test]
    fn an_ending_read_before_its_assertion_still_counts() {
        let (mut speech, _) = collector(vec![
            queued(1, "first"),
            queued(2, "second"),
            ended(2, UtteranceEnding::Completed),
            ended(1, UtteranceEnding::Completed),
        ]);
        speech.expect_in_order(&["first"], SHORT);
        speech.expect_in_order(&["second"], SHORT);
    }

    #[test]
    fn quiet_means_every_queued_utterance_has_ended() {
        let (mut speech, _) = collector(vec![queued(1, "first"), queued(2, "second")]);
        let result = catch_unwind(AssertUnwindSafe(|| {
            speech.wait_until_quiet(Duration::ZERO, SHORT);
        }));
        assert!(result.is_err(), "two utterances never ended");

        let (mut speech, _) = collector(vec![
            queued(1, "first"),
            ended(1, UtteranceEnding::Cancelled),
        ]);
        speech.wait_until_quiet(Duration::ZERO, SHORT);
    }

    #[test]
    fn transcript_lists_only_utterances_with_indices() {
        let (speech, timeline) = collector(Vec::new());
        assert_eq!(speech.transcript(), "(no utterances heard)");
        timeline.push_gesture("kb:verbatim+v");
        timeline.push_utterance("Settings... menu item");
        timeline.push_utterance("Exit menu item");
        assert_eq!(
            speech.transcript(),
            "0: Settings... menu item\n1: Exit menu item"
        );
    }
}
