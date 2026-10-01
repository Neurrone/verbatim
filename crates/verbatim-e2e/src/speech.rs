//! Speech assertions: collects [`Frame::Speech`] frames from a dedicated,
//! never-shared control-plane connection and checks that the utterances a
//! scenario cares about arrived in order.
//!
//! The dedicated connection matters. [`ControlClient::request`] discards
//! any frame that is not the reply it is waiting for, including speech
//! frames — so a connection also used to send gestures or keys would
//! silently lose utterances that happened to arrive while a request was in
//! flight. [`SpeechCollector`] never calls `request` again after its
//! initial subscribe, so nothing it reads is ever thrown away.
//!
//! Every `expect_*` assertion is about speech being *queued*: it matches the
//! frame Verbatim sends when an utterance enters the speech queue. Whether
//! the utterance then played is a separate question, answered only in a
//! paced run, and only as far as the speech pipeline can report it today:
//! a paced wait records on the timeline either that the matched utterance's
//! own completion arrived (`played`) or that it did not arrive in time
//! (`playback not confirmed`), which covers interruption, failure, and a
//! slow finish alike. Neither outcome fails the run.

use std::collections::{HashSet, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, Request};
use verbatim_model::TraceId;

use crate::timeline::Timeline;

/// How long a paced [`SpeechCollector`] waits for the matched utterance's
/// completion before recording that its playback was not confirmed and
/// letting the caller proceed. Generous enough for any single announcement
/// to play out, capped so a missing completion cannot hang a run.
const PACE_TIMEOUT: Duration = Duration::from_secs(10);

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

/// A queue-time utterance: what the assertions match against.
#[derive(Debug)]
struct Utterance {
    trace_id: TraceId,
    text: String,
}

/// Subscribes to and collects [`Frame::Speech`] frames on its own
/// control-plane connection.
pub struct SpeechCollector {
    source: Box<dyn FrameSource>,
    /// The scenario's shared action-and-speech log: every utterance this
    /// collector reads is pushed here as it arrives, alongside whatever
    /// gestures and keys [`crate::Scenario`] injected on the same clone of
    /// this handle, so a failure can print both interleaved in time order.
    /// See [`crate::timeline`].
    timeline: Timeline,
    /// When set, every successful `expect_*` call additionally waits for the
    /// matched utterance's own [`Frame::SpeechFinished`] before returning, so
    /// a human watching or a recording hears each utterance in full before
    /// the scenario injects the next input. Off for ordinary fast runs.
    paced: bool,
    /// How long a paced wait lasts; [`PACE_TIMEOUT`] outside unit tests.
    pace_timeout: Duration,
    /// Queue-time utterances read while a paced wait was looking for a
    /// completion, not yet offered to an assertion. The next assertion reads
    /// these before anything new, so speech that arrives during pacing is
    /// never lost.
    pending: VecDeque<Utterance>,
    /// Every trace id whose [`Frame::SpeechFinished`] has arrived, whenever
    /// it was read, so a paced wait also sees a completion that arrived
    /// before the wait began.
    finished: HashSet<TraceId>,
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
    pub fn subscribe(
        mut control: ControlClient,
        timeline: Timeline,
        paced: bool,
    ) -> io::Result<Self> {
        ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        Ok(Self::from_source(
            Box::new(control),
            timeline,
            paced,
            PACE_TIMEOUT,
        ))
    }

    fn from_source(
        source: Box<dyn FrameSource>,
        timeline: Timeline,
        paced: bool,
        pace_timeout: Duration,
    ) -> Self {
        Self {
            source,
            timeline,
            paced,
            pace_timeout,
            pending: VecDeque::new(),
            finished: HashSet::new(),
        }
    }

    /// Records one frame on the timeline and in the collector's state, and
    /// returns it as an [`Utterance`] when it is a queue-time speech frame.
    ///
    /// An audio-start follow-up repeats an utterance already seen at queue
    /// time and, under a loaded real synthesizer, arrives seconds late,
    /// interleaved with fresh queue-time frames — matching it would satisfy
    /// a matcher with stale text (the off-by-one that broke the M1 tab walk
    /// on a cold guest). It is recorded for the timeline's audio timing and
    /// never matched. Event, Reply, and Error frames cannot meaningfully
    /// arrive on this speech-only connection after the subscribe and are
    /// ignored.
    fn absorb(&mut self, frame: Frame) -> Option<Utterance> {
        match frame {
            Frame::Speech {
                trace_id,
                text,
                audio_started_at_ms,
                ..
            } => {
                if audio_started_at_ms.is_some() {
                    self.timeline.push_audio_started(&text);
                    None
                } else {
                    self.timeline.push_utterance(&text);
                    Some(Utterance { trace_id, text })
                }
            }
            Frame::SpeechFinished { trace_id } => {
                self.finished.insert(trace_id);
                None
            }
            _ => None,
        }
    }

    /// The next queue-time utterance: one held back by a paced wait if
    /// there is one, otherwise whatever one frame read from the connection
    /// yields (`None` when that frame was not a queue-time utterance).
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

    /// In paced mode, waits until `utterance`'s own [`Frame::SpeechFinished`]
    /// has arrived, so a watcher or recording hears it in full before the
    /// caller injects the next input; a no-op when not paced. Queue-time
    /// utterances read while waiting are kept for the next assertion, and a
    /// completion for any other utterance does not end the wait.
    ///
    /// Records the outcome on the timeline: `played` when the completion
    /// arrived, `playback not confirmed` when the wait timed out or the
    /// connection failed. Neither fails the run: the matching assertion
    /// already passed, and a failed connection is reported by the next one.
    fn wait_for_playback(&mut self, utterance: &Utterance) {
        if !self.paced {
            return;
        }
        let deadline = Instant::now() + self.pace_timeout;
        while !self.finished.contains(&utterance.trace_id) {
            if Instant::now() >= deadline {
                self.timeline.push_playback_unconfirmed(&utterance.text);
                return;
            }
            match self.source.next_frame() {
                Ok(frame) => {
                    if let Some(later) = self.absorb(frame) {
                        self.pending.push_back(later);
                    }
                }
                Err(error) if is_read_timeout(&error) => {}
                Err(_) => {
                    self.timeline.push_playback_unconfirmed(&utterance.text);
                    return;
                }
            }
        }
        self.timeline.push_played(&utterance.text);
    }

    /// Waits up to `timeout` for utterances containing each of `matchers`,
    /// in order (case-sensitive substring match), tolerating unrelated
    /// utterances in between. Consecutive matchers satisfied by the same
    /// utterance (for example a name fragment and a role word rendered
    /// together) both count without waiting for a second utterance. Asserts
    /// that the speech was queued; see this module's doc comment.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far — one entry per line, in time order — if the
    /// matchers do not all appear in order before `timeout` elapses, or if
    /// the speech connection fails outright.
    pub fn expect_in_order(&mut self, matchers: &[&str], timeout: Duration) {
        let _ = self.expect_in_order_capturing(matchers, timeout);
    }

    /// Like [`expect_in_order`](Self::expect_in_order), but returns the full
    /// text of the utterance that satisfied the final matcher, for callers
    /// that need to read a runtime value out of it — for example the numeric
    /// value a slider announces after its name and role ("Rate slider 80")
    /// — rather than asserting a literal they already know. Same wait,
    /// tolerance for unrelated intervening utterances, and
    /// timeline-on-failure discipline as [`expect_in_order`](Self::expect_in_order).
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far if the matchers do not all appear in order before
    /// `timeout` elapses, or if the speech connection fails outright.
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
        self.wait_for_playback(&utterance);
        utterance.text
    }

    /// Waits for an utterance whose full text differs from `unchanged` (a
    /// value already returned by this method itself, or by
    /// [`expect_change_capturing`](Self::expect_change_capturing)'s own
    /// previous call) and returns it, tolerating both unrelated utterances
    /// and repeats of
    /// `unchanged` itself in between — a focus-driven announcement can
    /// legitimately re-fire before the value it is reporting actually
    /// changes, and a caller with no substring to match on ahead of time
    /// (the whole point of capturing rather than asserting a literal) needs
    /// exactly the same tolerance for that as
    /// [`expect_in_order`](Self::expect_in_order) has for unrelated
    /// utterances.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far if no utterance differing from `unchanged` arrives
    /// before `timeout` elapses, or if the speech connection fails outright.
    pub fn expect_change_capturing(&mut self, unchanged: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            match self.next_utterance() {
                Ok(Some(utterance)) if utterance.text != unchanged => {
                    self.wait_for_playback(&utterance);
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
    /// is spoken again verbatim, as a substring of some later utterance.
    /// Same wait, tolerance for unrelated intervening utterances, and
    /// timeline-on-failure discipline as
    /// [`expect_in_order`](Self::expect_in_order) with a single matcher; a
    /// dedicated method rather than callers reaching for `expect_in_order`
    /// themselves so the panic message can name the captured value as what
    /// it is, not a matcher literal the caller wrote by hand.
    ///
    /// Fits a captured value that reappears unchanged. A value captured
    /// alongside surrounding role or state wording that a later utterance
    /// speaks bare will never satisfy this — use
    /// [`expect_change_capturing`](Self::expect_change_capturing) directly
    /// for that case instead.
    ///
    /// # Panics
    ///
    /// Panics with the full timeline of gestures, keys, and utterances
    /// recorded so far if no utterance containing `captured` arrives before
    /// `timeout` elapses, or if the speech connection fails outright.
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
        self.wait_for_playback(&utterance);
    }

    /// The non-panicking form of [`expect_in_order`](Self::expect_in_order):
    /// same wait and matching behavior, but reports success as a plain
    /// `bool` instead of panicking, for callers that want to retry a
    /// flaky first interaction (a real one: `verbatim-app`'s gesture
    /// router silently drops a gesture that arrives before its `GuiHandle`
    /// exists, and a live desktop's own unrelated foreground activity can
    /// occasionally steal the popup before it is ever heard — both
    /// documented on `crates/verbatim-e2e/tests/m1_exit_regression.rs`).
    /// A failed attempt's utterances remain in [`transcript`](Self::transcript)
    /// and the shared [`timeline`](crate::timeline::Timeline) for whichever
    /// attempt's panic (if the caller gives up) reports them.
    #[must_use]
    pub fn try_expect_in_order(&mut self, matchers: &[&str], timeout: Duration) -> bool {
        let (_, _fatal, completing) = self.advance_through(matchers, timeout);
        match completing {
            Some(utterance) => {
                self.wait_for_playback(&utterance);
                true
            }
            None => false,
        }
    }

    /// Shared loop behind [`expect_in_order`](Self::expect_in_order) and
    /// [`try_expect_in_order`](Self::try_expect_in_order): reads utterances
    /// until every matcher is satisfied, `timeout` elapses, or the
    /// connection fails outright. Returns how many matchers were satisfied,
    /// on a fatal (non-timeout) connection error that error, and — when
    /// every matcher was satisfied — the utterance whose match completed the
    /// final one.
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

/// Unit coverage for the collector's matching and pacing, driven from a
/// scripted frame sequence instead of a live connection, and for
/// [`SpeechCollector::transcript`]'s utterance-only formatting. The
/// interleaved-timeline rendering the panic messages print is covered in
/// `timeline.rs`.
#[cfg(test)]
mod tests {
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

    fn queued(trace_id: TraceId, text: &str) -> Frame {
        Frame::Speech {
            trace_id,
            text: text.to_owned(),
            event_observed_at_ms: None,
            queued_at_ms: 0,
            audio_started_at_ms: None,
        }
    }

    fn audio_started(trace_id: TraceId, text: &str) -> Frame {
        Frame::Speech {
            trace_id,
            text: text.to_owned(),
            event_observed_at_ms: None,
            queued_at_ms: 0,
            audio_started_at_ms: Some(1),
        }
    }

    fn collector(frames: Vec<Frame>, paced: bool) -> (SpeechCollector, Timeline) {
        let timeline = Timeline::new();
        let collector = SpeechCollector::from_source(
            Box::new(Scripted(frames.into())),
            timeline.clone(),
            paced,
            SHORT,
        );
        (collector, timeline)
    }

    #[test]
    fn speech_read_while_pacing_reaches_the_next_assertion() {
        let (first, second) = (TraceId::mint(), TraceId::mint());
        let (mut speech, timeline) = collector(
            vec![
                queued(first, "first"),
                queued(second, "second"),
                Frame::SpeechFinished { trace_id: first },
            ],
            true,
        );
        speech.expect_in_order(&["first"], SHORT);
        assert!(
            speech.try_expect_in_order(&["second"], SHORT),
            "an utterance read while pacing must still satisfy the next assertion"
        );
        assert!(timeline.render().contains(r#"played "first""#));
    }

    #[test]
    fn an_unrelated_completion_does_not_end_the_wait() {
        let (matched, other) = (TraceId::mint(), TraceId::mint());
        let (mut speech, timeline) = collector(
            vec![
                queued(matched, "matched"),
                Frame::SpeechFinished { trace_id: other },
            ],
            true,
        );
        speech.expect_in_order(&["matched"], SHORT);
        let rendered = timeline.render();
        assert!(rendered.contains(r#"playback not confirmed "matched""#));
        assert!(!rendered.contains("played"));
    }

    #[test]
    fn a_completion_that_arrived_before_its_wait_still_counts() {
        let (first, second) = (TraceId::mint(), TraceId::mint());
        let (mut speech, timeline) = collector(
            vec![
                queued(first, "first"),
                queued(second, "second"),
                Frame::SpeechFinished { trace_id: second },
                Frame::SpeechFinished { trace_id: first },
            ],
            true,
        );
        speech.expect_in_order(&["first"], SHORT);
        speech.expect_in_order(&["second"], SHORT);
        let rendered = timeline.render();
        assert!(rendered.contains(r#"played "first""#));
        assert!(rendered.contains(r#"played "second""#));
        assert!(!rendered.contains("not confirmed"));
    }

    #[test]
    fn audio_start_followups_read_while_pacing_are_never_utterances() {
        let first = TraceId::mint();
        let (mut speech, timeline) = collector(
            vec![
                queued(first, "first"),
                audio_started(first, "first"),
                Frame::SpeechFinished { trace_id: first },
            ],
            true,
        );
        speech.expect_in_order(&["first"], SHORT);
        assert_eq!(timeline.utterances(), vec!["first".to_owned()]);
        assert!(
            !speech.try_expect_in_order(&["first"], SHORT),
            "the audio-start follow-up must not satisfy a later assertion"
        );
    }

    #[test]
    fn an_unpaced_run_records_no_playback_outcome() {
        let first = TraceId::mint();
        let (mut speech, timeline) = collector(vec![queued(first, "first")], false);
        speech.expect_in_order(&["first"], SHORT);
        let rendered = timeline.render();
        assert!(!rendered.contains("played"));
        assert!(!rendered.contains("not confirmed"));
    }

    #[test]
    fn transcript_lists_only_utterances_with_indices() {
        let (speech, timeline) = collector(Vec::new(), false);
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
