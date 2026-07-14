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

/// Subscribes to and collects [`Frame::Speech`] frames on its own
/// control-plane connection.
pub struct SpeechCollector {
    control: ControlClient,
    /// Every utterance's rendered text, in arrival order — the transcript
    /// [`expect_in_order`](Self::expect_in_order) prints on failure.
    heard: Vec<String>,
}

impl SpeechCollector {
    /// Subscribes to speech on `control`, which becomes exclusively owned
    /// by the collector from this point on.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscribe request fails.
    pub fn subscribe(mut control: ControlClient) -> io::Result<Self> {
        ok_or_error(control.request(Request::SubscribeSpeech)?)?;
        Ok(Self {
            control,
            heard: Vec::new(),
        })
    }

    /// Waits up to `timeout` for utterances containing each of `matchers`,
    /// in order (case-sensitive substring match), tolerating unrelated
    /// utterances in between. Consecutive matchers satisfied by the same
    /// utterance (for example a name fragment and a role word rendered
    /// together) both count without waiting for a second utterance.
    ///
    /// # Panics
    ///
    /// Panics with a transcript of every utterance heard so far — one per
    /// line — if the matchers do not all appear in order before `timeout`
    /// elapses, or if the speech connection fails outright.
    pub fn expect_in_order(&mut self, matchers: &[&str], timeout: Duration) {
        let (next, fatal) = self.advance_through(matchers, timeout);
        if let Some(error) = fatal {
            panic!(
                "speech connection failed while waiting for utterance {} of {} ({:?}); everything heard so far:\n{}\nunderlying error: {error}",
                next + 1,
                matchers.len(),
                matchers[next],
                self.transcript()
            );
        }
        assert!(
            next >= matchers.len(),
            "timed out after {timeout:?} waiting for utterance {} of {} ({:?}); everything heard so far:\n{}",
            next + 1,
            matchers.len(),
            matchers[next],
            self.transcript()
        );
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
    /// for whichever attempt's panic (if the caller gives up) reports it.
    #[must_use]
    pub fn try_expect_in_order(&mut self, matchers: &[&str], timeout: Duration) -> bool {
        let (next, _fatal) = self.advance_through(matchers, timeout);
        next >= matchers.len()
    }

    /// Shared loop behind [`expect_in_order`](Self::expect_in_order) and
    /// [`try_expect_in_order`](Self::try_expect_in_order): reads frames
    /// until every matcher is satisfied, `timeout` elapses, or the
    /// connection fails outright. Returns how many matchers were satisfied
    /// and, on a fatal (non-timeout) connection error, that error.
    fn advance_through(
        &mut self,
        matchers: &[&str],
        timeout: Duration,
    ) -> (usize, Option<io::Error>) {
        let deadline = Instant::now() + timeout;
        let mut next = 0usize;
        while next < matchers.len() && Instant::now() < deadline {
            match self.control.next_frame() {
                Ok(Frame::Speech { text, .. }) => {
                    self.heard.push(text.clone());
                    while next < matchers.len() && text.contains(matchers[next]) {
                        next += 1;
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
                Err(error) => return (next, Some(error)),
            }
        }
        (next, None)
    }

    /// Every utterance heard so far, one per line, oldest first — the
    /// debugging artifact [`expect_in_order`](Self::expect_in_order) prints
    /// on failure.
    #[must_use]
    pub fn transcript(&self) -> String {
        if self.heard.is_empty() {
            return "(no utterances heard)".to_owned();
        }
        self.heard
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
