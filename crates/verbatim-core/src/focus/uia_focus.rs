//! A UI Automation focus at the front (design, "Focus and foreground" and
//! "The focus answers"): judged on its tell's facts, then read in two
//! requests chained under one due time, the listener's read of the
//! element it kept, answering a snapshot and the element's route, and the
//! application's outpost's completion from the route, answering the focus;
//! then executed.

use std::iter;

use verbatim_model::OutpostId;
use verbatim_model::request::{
    CompleteUia, CompletionFor, FocusRead, HiddenRoot, ObjectRead, ObjectRef, Purpose, ReadObject,
    Reply, Request as Wire, RouteEnd,
};

use super::FOCUS_SET;
use super::execute::{Known, Origin, again, execute};
use super::retake::{CurrentFocusRead, Focused, read_by};
use super::shown::Hidden;
use super::validity::{self, How, Subject, Validity};
use super::versions::ReadBy;
use crate::entries::{Entries, UiaFocus, request_window};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Request, RequestWindow};

/// The listener's read of the element it kept for a focus tell.
#[derive(Debug)]
pub(crate) struct ListenerRead {
    /// The element's nearest window.
    pub(crate) window: RequestWindow,
    /// The read.
    pub(crate) read: ReadObject,
}

impl Request for ListenerRead {
    type Reply = Box<ObjectRead>;

    fn reader(&self) -> Reader {
        Reader::Listener
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::ReadObject(self.read.clone())
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Object(read) => Some(read),
            _ => None,
        }
    }
}

/// The application's outpost completing the listener's read from its
/// route.
#[derive(Debug)]
pub(crate) struct Completion {
    /// The element's nearest window.
    pub(crate) window: RequestWindow,
    /// The completion.
    pub(crate) complete: CompleteUia,
}

impl Request for Completion {
    type Reply = Hidden<Box<FocusRead>>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::CompleteUia(Box::new(self.complete.clone()))
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        Hidden::parse(reply, |reply| match reply {
            Reply::Focus(read) => Some(read),
            _ => None,
        })
    }
}

/// The function of a UI Automation focus entry, or of a menu opening: a
/// menu opening on the committed focus executes it again; otherwise the
/// native-element test with the verdict, the overrides, acceptance, the
/// keyboard-focus answer and the duplicate test on its tell's facts; then
/// the two reads; then execution. Any outcome but a result ends it
/// unspoken and clears Core's caret baseline.
pub(crate) async fn run(mut cx: Cx<Entries>, focus: UiaFocus) {
    let executed = read_and_execute(&mut cx, &focus).await;
    if let Some(dialog) = focus.ended_wait {
        super::shown::after_ended_wait(&mut cx, dialog, executed);
    }
}

/// The focus judged, read and executed: whether it executed a focus, or
/// was the readers' last focus.
#[expect(
    clippy::too_many_lines,
    reason = "one chain of two reads, each of their outcomes named"
)]
async fn read_and_execute(cx: &mut Cx<Entries>, focus: &UiaFocus) -> bool {
    match validity::at(cx, Subject::Uia(focus), None, false) {
        Validity::Valid(How::Again) => {
            let said = again(&mut cx.facts_mut(), focus.tell.trace_id);
            for said in said {
                cx.emit(said);
            }
            return true;
        }
        Validity::Valid(How::Read) => {}
        Validity::Duplicate => return true,
        Validity::Refused(_) | Validity::NeedsRead(_) => return false,
    }
    // Valid, the focus's walk found its nearest window.
    let Some(window) = request_window(&focus.tell) else {
        return false;
    };
    let due_us = cx.due_in(FOCUS_DUE_US);
    let walk = Known::of(&cx.facts(), None).sent;
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Kept {
                listener: focus.listener,
                token: focus.token,
            },
            set: crate::details::walk_set(&cx.facts(), FOCUS_SET),
            relation: None,
            descendants: false,
            walk: Some(walk),
            purpose: Purpose::Focus,
        },
    };
    let object = match cx.ask(read, due_us).await {
        Outcome::Result(object) => object,
        outcome => {
            refused_or_not_there(cx, &outcome);
            cx.facts_mut().focus_read_failed();
            return false;
        }
    };
    // A focus read without its route is the listener's fault.
    let Some(route) = object.route else {
        return false;
    };
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(|instance| OutpostId(instance.generation));
    let known = Known::of(&cx.facts(), generation);
    if let RouteEnd::Known(met) = &route.end
        && !known.sent.contains(met)
    {
        // The committed focus's line changed between the two reads (a
        // desktop switch committed the sleeping focus, or a focus Core
        // holds over it took its place), so the ancestor the walk met is
        // no longer in it and the chain cannot be completed against it.
        // The listener let the element go with its read, so the
        // application's focus is read afresh against the line as it now
        // is, as NVDA's walk compares with the focus line at the time.
        return afresh(cx, focus, window, known, due_us).await;
    }
    let complete = CompleteUia {
        hidden: HiddenRoot::Check,
        route,
        read: iter::once(object.object.values)
            .chain(object.ancestors.into_iter().map(|ancestor| ancestor.values))
            .collect(),
        set: crate::details::node_set(&cx.facts(), FOCUS_SET),
        known: known.sent.clone(),
        completes: CompletionFor::Focus,
        details: crate::details::focus_details(&cx.facts()),
    };
    // Completed again once its hidden dialog is shown, from the route the
    // listener read, against the ancestors that route was read against.
    let outcome = super::shown::read_shown(cx, due_us, |_, hidden| {
        let request = Completion {
            window,
            complete: CompleteUia {
                hidden,
                ..complete.clone()
            },
        };
        (request, ())
    })
    .await;
    let read = match outcome {
        Outcome::Result((read, ())) => read,
        // The outpost ended: its registrations went with it (Table 10,
        // column F).
        Outcome::ReaderEnded => {
            cx.facts_mut().focus_read_failed();
            return false;
        }
        outcome @ (Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::NoLongerAwaited(_)) => {
            refused_or_not_there(cx, &outcome);
            cx.facts_mut().focus_read_failed();
            unexecuted(cx, window);
            return false;
        }
    };

    let read_by = ReadBy::Uia {
        runtime_id: focus.runtime_id.clone(),
        token: Some(focus.token),
    };
    super::narrow::judge_ahead(cx, &read);
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    let committed = executed.committed;
    if !committed {
        unexecuted(cx, window);
    }
    executed.deliver(cx);
    committed
}

/// The origin of `focus` read on `window`: its tell's, with Alt as Core's
/// key state had it at the tell's arrival.
fn origin(
    focus: &UiaFocus,
    window: RequestWindow,
    foreground: Option<crate::event_queue::Foreground>,
) -> Origin {
    let mut origin = Origin::of_tell(&focus.tell, window, foreground, focus.fence);
    if let Some(facts) = origin.window.as_mut() {
        facts.alt_held = focus.alt_down;
    }
    origin
}

/// The read of `focus` afresh, when the ancestor its listener read met
/// left the committed focus's line before the completion: the current
/// focus of `window`'s application, read by its outpost against the
/// ancestors Core now knows, `known`, then executed as the tell's focus.
/// Any outcome but a result ends it unspoken and clears Core's caret
/// baseline, as a failed read of the tell's element does.
async fn afresh(
    cx: &mut Cx<Entries>,
    focus: &UiaFocus,
    window: RequestWindow,
    known: Known,
    due_us: u64,
) -> bool {
    let outcome = super::shown::read_shown(cx, due_us, |cx, hidden| {
        let request = CurrentFocusRead::<super::msaa_run::Full> {
            window,
            known: known.sent.clone(),
            details: crate::details::focus_details(&cx.facts()),
            set: crate::details::focus_set(&cx.facts(), FOCUS_SET),
            depth: std::marker::PhantomData,
            hidden,
        };
        (request, ())
    })
    .await;
    let read = match outcome {
        Outcome::Result((Focused::Read(read), ())) => read,
        Outcome::Result((Focused::InFront(front), ())) => {
            let trace = (focus.tell.trace_id, focus.fence);
            super::plain::decide(cx, super::plain::HungRead::Afresh, window, &front, trace);
            return false;
        }
        outcome => {
            refused_or_not_there(cx, &outcome);
            if matches!(outcome, Outcome::ReaderEnded) {
                cx.facts_mut().versions.tried_and_refused();
            }
            cx.facts_mut().focus_read_failed();
            return false;
        }
    };
    let Some(read_by) = read_by(&read) else {
        return false;
    };
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    let committed = executed.committed;
    executed.deliver(cx);
    committed
}

/// A UI Automation focus read tried at its own turn and answered refused or
/// not there ends the readers' last focus's fence (Dickson, 2026-11-18).
fn refused_or_not_there<T>(cx: &mut Cx<Entries>, outcome: &Outcome<T>) {
    // A malformed answer is no try (`Refused::Undecodable`).
    if matches!(
        outcome,
        Outcome::Refused(crate::event_queue::Refused::Reader(_)) | Outcome::NotThere
    ) {
        cx.facts_mut().versions.tried_and_refused();
    }
}

/// A completion the outpost of `window`'s application answered, or Core
/// stopped waiting for, ended without its focus executed: a `Follow`
/// naming what Core names there removes the completion's registration
/// (Table 10, column C).
fn unexecuted(cx: &mut Cx<Entries>, window: RequestWindow) {
    let request = cx.facts().follows.unexecuted(window.process, window);
    super::follow::send(cx, vec![request]);
}
