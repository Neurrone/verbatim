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

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, Request};

use crate::timeline::Timeline;

/// How long a paced [`SpeechCollector`] waits for one utterance's audio to
/// finish before giving up and letting the caller proceed anyway. Generous
/// enough for any single announcement to play out, capped so a missing
/// completion signal cannot hang a run.
const PACE_TIMEOUT: Duration = Duration::from_secs(10);

/// Subscribes to and collects [`Frame::Speech`] frames on its own
/// control-plane connection.
pub struct SpeechCollector {
    control: ControlClient,
    /// The scenario's shared action-and-speech log: every utterance this
    /// collector reads is pushed here as it arrives, alongside whatever
    /// gestures and keys [`crate::Scenario`] injected on the same clone of
    /// this handle, so a failure can print both interleaved in time order.
    /// See [`crate::timeline`].
    timeline: Timeline,
    /// When set, every successful `expect_*` call additionally waits for the
    /// matched utterance's audio to finish playing (a [`Frame::SpeechFinished`])
    /// before returning, so a human watching or a recording hears each
    /// utterance in full before the scenario injects the next input. Off for
    /// ordinary fast, deterministic test runs.
    paced: bool,
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
        Ok(Self {
            control,
            timeline,
            paced,
        })
    }

    /// In paced mode, blocks until the current utterance's audio has finished
    /// playing (a [`Frame::SpeechFinished`]), so a watcher or recording hears
    /// it in full before the caller injects the next input. A no-op when not
    /// paced. Best-effort: on timeout or a connection error it simply returns
    /// rather than failing — pacing is a presentation aid, never a correctness
    /// assertion, and a real speech problem is caught by the `expect_*` match
    /// that already succeeded, not here. Utterances read while waiting are
    /// still pushed to the timeline.
    fn wait_for_audio_finished(&mut self) {
        if !self.paced {
            return;
        }
        let deadline = Instant::now() + PACE_TIMEOUT;
        loop {
            match self.control.next_frame() {
                Ok(Frame::SpeechFinished { .. }) => return,
                Ok(Frame::Speech { text, .. }) => self.timeline.push_utterance(&text),
                Ok(_) => {}
                Err(error) if is_read_timeout(&error) => {}
                Err(_) => return,
            }
            if Instant::now() >= deadline {
                return;
            }
        }
    }

    /// Waits up to `timeout` for utterances containing each of `matchers`,
    /// in order (case-sensitive substring match), tolerating unrelated
    /// utterances in between. Consecutive matchers satisfied by the same
    /// utterance (for example a name fragment and a role word rendered
    /// together) both count without waiting for a second utterance.
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
        let (next, fatal, captured) = self.advance_through(matchers, timeout);
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
        let text = captured.expect(
            "advance_through returns the completing utterance once every matcher is satisfied",
        );
        self.wait_for_audio_finished();
        text
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
            match self.control.next_frame() {
                Ok(Frame::Speech {
                    text,
                    audio_started_at_ms,
                    ..
                }) => {
                    // Never a change candidate: an audio-start follow-up is
                    // stale text by definition (see `advance_through`).
                    if audio_started_at_ms.is_some() {
                        self.timeline.push_audio_started(&text);
                    } else {
                        self.timeline.push_utterance(&text);
                        if text != unchanged {
                            self.wait_for_audio_finished();
                            return text;
                        }
                    }
                }
                Ok(_) => {
                    // Not speech: ignore it, same as advance_through.
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
        let (next, fatal, _) = self.advance_through(&[captured], timeout);
        if let Some(error) = fatal {
            panic!(
                "speech connection failed while waiting for the previously captured value {captured:?} to be spoken again; timeline so far:\n{}\nunderlying error: {error}",
                self.timeline.render()
            );
        }
        assert!(
            next >= 1,
            "timed out after {timeout:?} waiting for the previously captured value {captured:?} to be spoken again; timeline so far:\n{}",
            self.timeline.render()
        );
        self.wait_for_audio_finished();
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
        let (next, _fatal, _captured) = self.advance_through(matchers, timeout);
        let matched = next >= matchers.len();
        if matched {
            self.wait_for_audio_finished();
        }
        matched
    }

    /// Shared loop behind [`expect_in_order`](Self::expect_in_order) and
    /// [`try_expect_in_order`](Self::try_expect_in_order): reads frames
    /// until every matcher is satisfied, `timeout` elapses, or the
    /// connection fails outright. Returns how many matchers were satisfied,
    /// on a fatal (non-timeout) connection error that error, and — when
    /// every matcher was satisfied — the full text of the utterance whose
    /// match completed the final one.
    fn advance_through(
        &mut self,
        matchers: &[&str],
        timeout: Duration,
    ) -> (usize, Option<io::Error>, Option<String>) {
        let deadline = Instant::now() + timeout;
        let mut next = 0usize;
        let mut completing_text = None;
        while next < matchers.len() && Instant::now() < deadline {
            match self.control.next_frame() {
                Ok(Frame::Speech {
                    text,
                    audio_started_at_ms,
                    ..
                }) => {
                    // An audio-start follow-up repeats an utterance already
                    // matched at queue time, and under a loaded real
                    // synthesizer arrives seconds late, interleaved with
                    // fresh queue-time frames — matching it would satisfy a
                    // matcher with stale text (the off-by-one that broke the
                    // M1 tab walk on a cold guest). Record it for the
                    // timeline's audio timing and never match it.
                    if audio_started_at_ms.is_some() {
                        self.timeline.push_audio_started(&text);
                        continue;
                    }
                    self.timeline.push_utterance(&text);
                    while next < matchers.len() && text.contains(matchers[next]) {
                        next += 1;
                        if next == matchers.len() {
                            completing_text = Some(text.clone());
                        }
                    }
                }
                Ok(_) => {
                    // An Event, Reply, or Error frame arrived on this
                    // speech-only connection (a Reply/Error can only be the
                    // subscribe's own, already consumed by `subscribe`, but
                    // ignoring the rest of the frame kinds by design keeps
                    // this loop dumb and reliable). Not speech: ignore it.
                }
                Err(error) if is_read_timeout(&error) => {
                    // The fixed per-read socket timeout expired with no
                    // frame; loop back around to recheck the overall
                    // deadline rather than treating this as a failure.
                }
                Err(error) => return (next, Some(error), None),
            }
        }
        (next, None, completing_text)
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

/// Unit coverage for [`SpeechCollector::transcript`]'s utterance-only
/// formatting, driven directly off a [`Timeline`] since building a real
/// `SpeechCollector` needs a live `ControlClient` connection. The
/// interleaved-timeline rendering that the panic messages above actually
/// print is covered in `timeline.rs`, against
/// [`Timeline::render`](crate::timeline::Timeline::render) itself.
#[cfg(test)]
mod tests {
    use super::Timeline;

    #[test]
    fn transcript_reports_no_utterances_heard_when_timeline_is_empty() {
        let timeline = Timeline::new();
        assert_eq!(timeline.utterances(), Vec::<String>::new());
    }

    #[test]
    fn transcript_formatting_matches_indexed_utterance_lines_and_excludes_gestures() {
        let timeline = Timeline::new();
        timeline.push_gesture("kb:verbatim+v");
        timeline.push_utterance("Settings... menu item");
        timeline.push_utterance("Exit menu item");

        let heard = timeline.utterances();
        let rendered = heard
            .iter()
            .enumerate()
            .map(|(index, text)| format!("{index}: {text}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            rendered,
            "0: Settings... menu item\n1: Exit menu item".to_owned()
        );
        assert_eq!(
            heard.len(),
            2,
            "the gesture must not leak into the utterance-only view"
        );
    }
}
