//! Core's requests on the outpost's one worker
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14): queued in
//! the order Core sent them, run one at a time ahead of the outpost's own
//! work, each answered once with one of the five outcomes.
//!
//! A request queued is dispatched when no program runs. Cancel drops one
//! still queued, answered cancelled. One running is abandoned with its
//! worker replaced only when the reason is the foreground moving away,
//! since a thread in a hung or slow window would stick again and a
//! withdrawal must not cost a thread; for any other reason it is left to
//! run, and its answer is sent when it ends, which Core drops by number. A
//! request that ends without an answer, or panics, is answered failed.
//! At shutdown nothing more is dispatched and the requests queued are
//! dropped unanswered.

use std::collections::VecDeque;

use verbatim_model::request::{Answer, Ask, Cancel, Outcome, RequestNumber};

/// What a cancel does.
#[derive(Debug, PartialEq, Eq)]
pub enum Cancelled {
    /// The request was queued: it is removed and answered, with this
    /// answer.
    Dropped(Answer),
    /// The request runs and the foreground moved away: its worker is
    /// abandoned and replaced, and it is answered, with this answer.
    Abandon(Answer),
    /// Nothing: the request runs and is left alone, or is not here.
    Nothing,
}

/// Core's requests queued for the worker, and the one it runs.
#[derive(Clone, Debug, Default)]
pub struct Asks {
    queued: VecDeque<Box<Ask>>,
    /// The request the program in flight answers, until it is answered.
    running: Option<RequestNumber>,
}

impl Asks {
    /// Queues `ask` behind those already queued.
    pub fn push(&mut self, ask: Box<Ask>) {
        self.queued.push_back(ask);
    }

    /// The next request to dispatch, which runs from now on, when none
    /// runs.
    pub fn take_next(&mut self) -> Option<Box<Ask>> {
        if self.running.is_some() {
            return None;
        }
        let ask = self.queued.pop_front()?;
        self.running = Some(ask.number);
        Some(ask)
    }

    /// The request running, until it is answered.
    #[must_use]
    pub fn running(&self) -> Option<RequestNumber> {
        self.running
    }

    /// Whether any request is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// The requests queued, in order.
    pub fn queued(&self) -> impl Iterator<Item = &Ask> {
        self.queued.iter().map(AsRef::as_ref)
    }

    /// The running request's answer, `answer`, is sent: it no longer runs.
    /// `None` when `answer` is not the running request's.
    pub fn answered(&mut self, answer: Answer) -> Option<Answer> {
        self.running.take_if(|running| *running == answer.number)?;
        Some(answer)
    }

    /// The program that ran the request ended: a request left unanswered
    /// is answered failed.
    pub fn ended(&mut self) -> Option<Answer> {
        self.running
            .take()
            .map(|number| ended(number, "the outpost failed handling the request"))
    }

    /// Core's `cancel`.
    pub fn cancel(&mut self, cancel: Cancel) -> Cancelled {
        if let Some(place) = self
            .queued
            .iter()
            .position(|ask| ask.number == cancel.number)
        {
            self.queued.remove(place);
            return Cancelled::Dropped(ended(cancel.number, "cancelled"));
        }
        if self.running == Some(cancel.number) && cancel.reason.abandons_call() {
            self.running = None;
            return Cancelled::Abandon(ended(cancel.number, "abandoned"));
        }
        Cancelled::Nothing
    }

    /// The outpost shuts down: the requests queued are dropped unanswered,
    /// and the running one's answer is dropped.
    pub fn close(&mut self) {
        self.queued.clear();
        self.running = None;
    }
}

/// The answer "ended or stopped" to request `number`, for the log
/// `detail`.
fn ended(number: RequestNumber, detail: &str) -> Answer {
    Answer {
        number,
        outcome: Outcome::Ended,
        probes: Vec::new(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::WindowHandle;
    use verbatim_model::request::{CancelReason, Reply, Request, WindowVerdict};

    use super::*;

    fn ask(number: u64) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window: WindowHandle(1),
            verdict: WindowVerdict::Msaa,
            request: Request::CurrentFocus {
                set: Vec::new(),
                known: Vec::new(),
                baseline: None,
            },
        })
    }

    fn cancel(number: u64, reason: CancelReason) -> Cancel {
        Cancel {
            number: RequestNumber(number),
            reason,
        }
    }

    fn result(number: u64) -> Answer {
        Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(Reply::Followed),
            probes: Vec::new(),
            detail: String::new(),
        }
    }

    /// Requests are dispatched one at a time, in the order Core sent them.
    #[test]
    fn requests_run_one_at_a_time_in_sending_order() {
        let mut asks = Asks::default();
        asks.push(ask(1));
        asks.push(ask(2));
        assert_eq!(
            asks.take_next().map(|ask| ask.number),
            Some(RequestNumber(1))
        );
        assert_eq!(asks.take_next(), None, "one runs");
        assert_eq!(asks.answered(result(1)), Some(result(1)));
        assert_eq!(
            asks.take_next().map(|ask| ask.number),
            Some(RequestNumber(2))
        );
    }

    /// A queued request cancelled for any reason is dropped and answered
    /// cancelled, and never dispatched.
    #[test]
    fn a_queued_request_cancelled_is_dropped_and_answered_cancelled() {
        for reason in [
            CancelReason::Foreground,
            CancelReason::Hung,
            CancelReason::DueTime,
            CancelReason::Withdrawn,
        ] {
            let mut asks = Asks::default();
            asks.push(ask(1));
            asks.push(ask(2));
            let _ = asks.take_next();
            assert_eq!(
                asks.cancel(cancel(2, reason)),
                Cancelled::Dropped(ended(RequestNumber(2), "cancelled")),
                "{reason:?}"
            );
            let _ = asks.answered(result(1));
            assert_eq!(asks.take_next(), None, "{reason:?}");
        }
    }

    /// A running request cancelled because the foreground moved away is
    /// abandoned and answered abandoned; the next is dispatched.
    #[test]
    fn a_running_request_cancelled_for_the_foreground_is_abandoned() {
        let mut asks = Asks::default();
        asks.push(ask(1));
        asks.push(ask(2));
        let _ = asks.take_next();
        assert_eq!(
            asks.cancel(cancel(1, CancelReason::Foreground)),
            Cancelled::Abandon(ended(RequestNumber(1), "abandoned"))
        );
        assert_eq!(asks.running(), None);
        assert_eq!(
            asks.answered(result(1)),
            None,
            "its late answer is not sent"
        );
        assert_eq!(
            asks.take_next().map(|ask| ask.number),
            Some(RequestNumber(2))
        );
    }

    /// A running request cancelled for a hang, its due time or a
    /// withdrawal is left to run, and its answer is sent when it ends.
    #[test]
    fn a_running_request_cancelled_for_another_reason_is_left_alone() {
        for reason in [
            CancelReason::Hung,
            CancelReason::DueTime,
            CancelReason::Withdrawn,
        ] {
            let mut asks = Asks::default();
            asks.push(ask(1));
            let _ = asks.take_next();
            assert_eq!(asks.cancel(cancel(1, reason)), Cancelled::Nothing);
            assert_eq!(asks.running(), Some(RequestNumber(1)));
            assert_eq!(asks.answered(result(1)), Some(result(1)), "{reason:?}");
        }
    }

    /// A request whose program ends without answering is answered failed,
    /// once.
    #[test]
    fn a_request_ended_unanswered_is_answered_failed_once() {
        let mut asks = Asks::default();
        asks.push(ask(1));
        let _ = asks.take_next();
        assert_eq!(
            asks.ended(),
            Some(ended(
                RequestNumber(1),
                "the outpost failed handling the request"
            ))
        );
        assert_eq!(asks.ended(), None);
        let mut answered = Asks::default();
        answered.push(ask(2));
        let _ = answered.take_next();
        let _ = answered.answered(result(2));
        assert_eq!(answered.ended(), None, "an answered request is not again");
    }

    /// A cancel of a request the worker never had, or already answered,
    /// does nothing.
    #[test]
    fn a_cancel_of_a_request_not_here_does_nothing() {
        let mut asks = Asks::default();
        asks.push(ask(1));
        let _ = asks.take_next();
        let _ = asks.answered(result(1));
        assert_eq!(
            asks.cancel(cancel(1, CancelReason::Foreground)),
            Cancelled::Nothing
        );
        assert_eq!(
            asks.cancel(cancel(9, CancelReason::Withdrawn)),
            Cancelled::Nothing
        );
    }

    /// At shutdown the queued requests are dropped unanswered and the
    /// running one's answer is not sent.
    #[test]
    fn shutdown_drops_the_queued_requests_unanswered() {
        let mut asks = Asks::default();
        asks.push(ask(1));
        asks.push(ask(2));
        let _ = asks.take_next();
        asks.close();
        assert_eq!(asks.take_next(), None);
        assert_eq!(asks.answered(result(1)), None);
        assert_eq!(asks.ended(), None);
    }
}
