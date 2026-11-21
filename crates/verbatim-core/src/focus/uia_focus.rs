//! A UI Automation focus at the front (design, "Focus and foreground" and
//! "The focus answers"): judged on its tell's facts, then read in two
//! requests chained under one due time, the listener's read of the
//! element it kept, answering a snapshot and the element's route, and the
//! application's outpost's completion from the route, answering the focus;
//! then executed.

use std::iter;

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
    let cx = &mut cx;
    let focus = &focus;
    match validity::at(cx, Subject::Uia(focus), None, false) {
        Validity::Valid(How::Again) => {
            let said = again(&mut cx.facts_mut(), focus.tell.trace_id);
            for said in said {
                cx.emit(said);
            }
            return;
        }
        Validity::Valid(How::Read) => {}
        Validity::Duplicate | Validity::Refused(_) | Validity::NeedsRead => return,
    }
    // Valid, the focus's walk found its nearest window.
    let Some(window) = request_window(&focus.tell) else {
        return;
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
            set: crate::details::description_kept(&cx.facts(), FOCUS_SET),
            relation: None,
            descendants: false,
            walk: Some(walk),
            purpose: Purpose::Focus,
        },
    };
    let object = match cx.ask(read, due_us).await {
        Outcome::Result(object) => object,
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            return;
        }
    };
    // A focus read without its route is the listener's fault.
    let Some(route) = object.route else {
        return;
    };
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(verbatim_model::request::Instance::outpost_id);
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
        afresh(cx, focus, window, known, due_us).await;
        return;
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
    let request = Completion { window, complete };
    // A hidden dialog postponed, answered before any registration: its
    // show reads the current focus.
    let Some(outcome) = super::shown::ask(cx, request, due_us).await else {
        return;
    };
    let read = match outcome {
        Outcome::Result(read) => read,
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            // An outpost that ended took its registrations with it (Table
            // 10, column F).
            if !matches!(outcome, Outcome::ReaderEnded) {
                unexecuted(cx, window);
            }
            return;
        }
    };

    let read_by = ReadBy::Uia {
        runtime_id: focus.runtime_id.clone(),
        token: Some(focus.token),
    };
    super::narrow::judge_ahead(cx, &read);
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    if !executed.committed {
        unexecuted(cx, window);
    }
    let fence = crate::entries::front_fence(cx);
    executed.deliver(cx, fence);
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
) {
    let request = CurrentFocusRead::<super::msaa_run::Full> {
        window,
        known: known.sent.clone(),
        details: crate::details::focus_details(&cx.facts()),
        set: crate::details::focus_set(&cx.facts(), FOCUS_SET),
        depth: std::marker::PhantomData,
    };
    let Some(outcome) = super::shown::ask(cx, request, due_us).await else {
        return;
    };
    let read = match outcome {
        Outcome::Result(Focused::Read(read)) => read,
        Outcome::Result(Focused::InFront(front)) => {
            let trace = (focus.tell.trace_id, focus.fence);
            super::plain::decide(cx, super::plain::HungRead::Afresh, window, &front, trace);
            return;
        }
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            return;
        }
    };
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    let fence = crate::entries::front_fence(cx);
    executed.deliver(cx, fence);
}

/// A completion the outpost of `window`'s application answered, or Core
/// stopped waiting for, ended without its focus executed: a `Follow`
/// naming what Core names there removes the completion's registration
/// (Table 10, column C).
fn unexecuted(cx: &mut Cx<Entries>, window: RequestWindow) {
    let request = cx.facts().follows.unexecuted(window.process, window);
    super::follow::send(cx, vec![request]);
}
