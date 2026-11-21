//! Core's requests on the outpost's one worker
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14): queued in
//! the order Core sent them, the event queue's requests (`Follow` and a
//! terminal's baseline among them) ahead of every request from outside
//! the queue still waiting ([`Question::outside_the_queue`]: a tree dump,
//! the taskbar's reads, a terminal's reads for its output), so that keys
//! never wait behind them; run one at a time ahead of the outpost's own
//! work, each answered once with one of the five outcomes.
//!
//! A request queued is dispatched when no program runs; the reducer's
//! record of the program in flight says which request runs, if one does.
//! Cancel drops one still queued, answered cancelled. One running is abandoned with its
//! worker replaced only when the reason is the foreground moving away,
//! since a thread in a hung or slow window would stick again and a
//! withdrawal must not cost a thread; for any other reason it is left to
//! run, and its answer is sent when it ends, which Core drops by number. A
//! request that ends without an answer, or panics, is answered failed.
//! At shutdown nothing more is dispatched and the requests queued are
//! dropped unanswered.
//!
//! A `Follow` replaces one still queued, which is answered "ended or
//! stopped" (Table 10): the newest `Follow` wins, and since it names the
//! whole registration, skipping the older one changes nothing that runs
//! between them. A window destroyed answers "not there" every queued read
//! whose MSAA ids name it (Table 14).

use std::collections::VecDeque;

use verbatim_model::WindowHandle;
use verbatim_model::batch::{Op, Subject};
use verbatim_model::request::{
    Answer, Ask, Cancel, ObjectRef, Outcome, Question, Request, RequestNumber,
};

/// The window of the MSAA ids `request`'s subject is named by, for the
/// kinds that name one: a read of an object by its ids and a run's
/// candidate, and a batch with a step on an MSAA object. Every kind is
/// classed, so a new one is classed where it is added.
fn ids_window(request: &Question) -> Option<WindowHandle> {
    let request = match request {
        Question::Legacy(request) => request,
        Question::Batch(batch) => {
            return batch.steps.iter().find_map(|step| match step.op.subject() {
                Some(Subject::Msaa { ids, .. }) => Some(ids.window),
                _ => None,
            });
        }
    };
    match request {
        Request::ReadObject(read) => match read.object {
            ObjectRef::Msaa { window, .. } => Some(window),
            ObjectRef::Kept { .. } | ObjectRef::Node(_) => None,
        },
        Request::Properties { .. }
        | Request::Navigate { .. }
        | Request::Find { .. }
        | Request::Children { .. }
        | Request::Text { .. }
        | Request::Activate { .. }
        | Request::Follow(_)
        | Request::DumpTree { .. }
        | Request::ShellItems { .. }
        | Request::ShellItemPosition { .. } => None,
    }
}

/// Whether `request` only moves the outpost's registration: a `Follow`,
/// or a batch of `Follow` steps alone, which names the whole registration.
fn is_follow(request: &Question) -> bool {
    match request {
        Question::Legacy(request) => matches!(request, Request::Follow(_)),
        Question::Batch(batch) => batch
            .steps
            .iter()
            .all(|step| matches!(step.op, Op::Follow(_))),
    }
}

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

/// Core's requests queued for the worker.
#[derive(Clone, Debug, Default)]
pub struct Asks {
    queued: VecDeque<Box<Ask>>,
}

impl Asks {
    /// Queues `ask` behind those already queued of its order class, an
    /// event queue's request ahead of every request from outside the queue
    /// still queued; a `Follow` removes any `Follow` still queued, whose
    /// answers are returned.
    pub fn push(&mut self, ask: Box<Ask>) -> Vec<Answer> {
        let mut replaced = Vec::new();
        if is_follow(&ask.request) {
            self.queued.retain(|queued| {
                let follow = is_follow(&queued.request);
                if follow {
                    replaced.push(ended(queued.number, "replaced by a newer Follow"));
                }
                !follow
            });
        }
        let place = if ask.request.outside_the_queue() {
            self.queued.len()
        } else {
            self.queued
                .iter()
                .position(|queued| queued.request.outside_the_queue())
                .unwrap_or(self.queued.len())
        };
        self.queued.insert(place, ask);
        replaced
    }

    /// Window `window` was destroyed: every queued request whose subject's
    /// own MSAA ids name it, a read of an object by its ids or a run's
    /// candidate, is removed, and their answers, "not there", are
    /// returned. A request is never matched by the window it was asked on
    /// ([`Ask::window`]): a node's request, a current-focus read, which
    /// reads the application's focus window, and every other stay queued
    /// and run after the window's forget. The running one answers what it
    /// found.
    pub fn destroyed(&mut self, window: WindowHandle) -> Vec<Answer> {
        let mut gone = Vec::new();
        self.queued.retain(|queued| {
            let on = ids_window(&queued.request) == Some(window);
            if on {
                gone.push(Answer {
                    number: queued.number,
                    outcome: Outcome::NotThere,
                    probes: Vec::new(),
                    detail: "its window was destroyed".to_owned(),
                });
            }
            !on
        });
        gone
    }

    /// The next request to dispatch, for the reducer to run when no
    /// program runs.
    pub fn take_next(&mut self) -> Option<Box<Ask>> {
        self.queued.pop_front()
    }

    /// Core's `cancel`, with `running` the request the program in flight
    /// runs and has not answered, if one.
    pub fn cancel(&mut self, cancel: Cancel, running: Option<RequestNumber>) -> Cancelled {
        if let Some(place) = self
            .queued
            .iter()
            .position(|ask| ask.number == cancel.number)
        {
            self.queued.remove(place);
            return Cancelled::Dropped(ended(cancel.number, "cancelled"));
        }
        if running == Some(cancel.number) && cancel.reason.abandons_call() {
            return Cancelled::Abandon(ended(cancel.number, "abandoned"));
        }
        Cancelled::Nothing
    }

    /// The outpost shuts down: the requests queued are dropped unanswered.
    pub fn close(&mut self) {
        self.queued.clear();
    }
}

/// The answer to request `number` whose program ended without answering.
pub(crate) fn failed(number: RequestNumber) -> Answer {
    ended(number, "the outpost failed handling the request")
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
    use verbatim_model::request::{CancelReason, Follow, WindowVerdict};

    use super::*;

    fn ask(number: u64) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window: WindowHandle(1),
            verdict: WindowVerdict::Msaa,
            request: Request::Properties {
                node: verbatim_model::NodeId::new(1),
                set: Vec::new(),
            }
            .into(),
        })
    }

    fn follow(number: u64, window: u64) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window: WindowHandle(window),
            verdict: WindowVerdict::Uia,
            request: Request::Follow(Follow { node: None }).into(),
        })
    }

    fn numbers(asks: &Asks) -> Vec<u64> {
        asks.queued.iter().map(|ask| ask.number.0).collect()
    }

    fn outside(number: u64) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window: WindowHandle(1),
            verdict: WindowVerdict::Uia,
            request: Request::DumpTree { locations: false }.into(),
        })
    }

    /// The event queue's requests are made ahead of every request from
    /// outside the queue still queued, each class in the order Core sent
    /// it; one already taken to run is not among them (Table 14).
    #[test]
    fn the_queues_requests_go_ahead_of_requests_from_outside_it() {
        let mut asks = Asks::default();
        let _ = asks.push(outside(1));
        let _ = asks.take_next();
        let _ = asks.push(outside(2));
        let _ = asks.push(ask(3));
        let _ = asks.push(outside(4));
        let _ = asks.push(follow(5, 1));
        let _ = asks.push(ask(6));
        assert_eq!(numbers(&asks), [3, 5, 6, 2, 4]);
    }

    fn cancel(number: u64, reason: CancelReason) -> Cancel {
        Cancel {
            number: RequestNumber(number),
            reason,
        }
    }

    /// A cancel of a request the worker never had, or already answered,
    /// does nothing.
    #[test]
    fn a_cancel_of_a_request_not_here_does_nothing() {
        let mut asks = Asks::default();
        let _ = asks.push(ask(1));
        let _ = asks.take_next();
        assert_eq!(
            asks.cancel(cancel(1, CancelReason::Foreground), None),
            Cancelled::Nothing
        );
        assert_eq!(
            asks.cancel(cancel(9, CancelReason::Withdrawn), None),
            Cancelled::Nothing
        );
    }

    /// A batch of `Follow` steps alone replaces a `Follow` still queued,
    /// answered ended; a batch on an MSAA object is answered not there
    /// when that object's window is destroyed, and any other batch stays.
    #[test]
    fn a_follow_batch_replaces_a_queued_follow_and_an_msaa_batch_goes_with_its_window() {
        use verbatim_model::batch::{Batch, Op, Placement, Step, Subject};
        let batch = |number, op| {
            Box::new(Ask {
                number: RequestNumber(number),
                window: WindowHandle(1),
                verdict: WindowVerdict::Msaa,
                request: Batch {
                    steps: vec![Step { op, when: None }],
                    placement: Placement::Queue,
                    spare: None,
                }
                .into(),
            })
        };
        let read = |subject| Op::Read {
            subject,
            set: Vec::new(),
        };
        let mut asks = Asks::default();
        let _ = asks.push(follow(1, 1));
        let replaced = asks.push(batch(2, Op::Follow(None)));
        assert_eq!(
            replaced,
            [ended(RequestNumber(1), "replaced by a newer Follow")]
        );
        let msaa = Subject::Msaa {
            ids: verbatim_model::request::MsaaIds {
                window: WindowHandle(7),
                object: 0,
                child: 0,
            },
            redirect: true,
        };
        let _ = asks.push(batch(3, read(msaa)));
        let _ = asks.push(batch(
            4,
            read(Subject::Node(verbatim_model::NodeId::new(5))),
        ));
        let gone: Vec<u64> = asks
            .destroyed(WindowHandle(7))
            .iter()
            .map(|answer| answer.number.0)
            .collect();
        assert_eq!(gone, [3]);
        assert_eq!(numbers(&asks), [2, 4]);
    }

    /// At shutdown the queued requests are dropped unanswered.
    #[test]
    fn shutdown_drops_the_queued_requests_unanswered() {
        let mut asks = Asks::default();
        let _ = asks.push(ask(1));
        let _ = asks.push(ask(2));
        let _ = asks.take_next();
        asks.close();
        assert_eq!(asks.take_next(), None);
    }
}
