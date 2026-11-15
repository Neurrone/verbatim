//! A UI Automation focus at the front (design, "Focus and foreground" and
//! "The focus answers"): judged on its tell's facts, then read in two
//! requests chained under one due time, the listener's read of the
//! element it kept, answering a snapshot and the element's route, and the
//! application's outpost's completion from the route, answering the focus;
//! then executed.

use std::iter;

use verbatim_model::OutpostId;
use verbatim_model::request::{
    CompleteUia, CompletionFor, FocusRead, ObjectRead, ObjectRef, Purpose, ReadObject, Reply,
    Request as Wire,
};
use verbatim_model::tell::ElementToken;

use super::FOCUS_SET;
use super::execute::{Known, execute};
use super::pending::counts_as_pending;
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
    type Reply = Box<FocusRead>;

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
        match reply {
            Reply::Focus(read) => Some(read),
            _ => None,
        }
    }
}

/// The readers' last focus's token, which the listener keeps while it is
/// a UI Automation focus it holds.
fn kept_token(cx: &Cx<Entries>) -> Option<ElementToken> {
    match cx.facts().versions.last.as_ref().map(|last| &last.read_by) {
        Some(ReadBy::Uia { token, .. }) => *token,
        Some(ReadBy::Msaa { .. }) | None => None,
    }
}

/// The function of a UI Automation focus entry: the native-element test
/// with the verdict, the overrides, acceptance, the keyboard-focus answer
/// and the duplicate test on its tell's facts; then the two reads; then
/// execution. Any outcome but a result ends it unspoken.
pub(crate) async fn run(mut cx: Cx<Entries>, focus: UiaFocus) {
    let passes = {
        let facts = cx.facts();
        let backend = cx.backend();
        counts_as_pending(
            &focus.tell,
            facts.versions.last.as_ref(),
            &backend,
            cx.now_us(),
        )
    };
    // Passing, the focus's walk found its nearest window.
    let (true, Some(window)) = (passes, request_window(&focus.tell)) else {
        return;
    };
    let due_us = cx.due_in(FOCUS_DUE_US);
    let walk = Known::of(&cx.facts(), None).sent;
    let keep = kept_token(&cx);
    let read = ListenerRead {
        window,
        read: ReadObject {
            object: ObjectRef::Kept(focus.token),
            set: FOCUS_SET.to_vec(),
            relation: None,
            descendants: false,
            walk: Some(walk),
            purpose: Purpose::Focus { keep },
        },
    };
    let Outcome::Result(object) = cx.ask(read, due_us).await else {
        return;
    };
    // A focus read without its route is the listener's fault.
    let Some(route) = object.route else {
        return;
    };
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(|instance| OutpostId(instance.generation));
    let known = Known::of(&cx.facts(), generation);
    let complete = Completion {
        window,
        complete: CompleteUia {
            route,
            read: iter::once(object.object.values)
                .chain(object.ancestors.into_iter().map(|ancestor| ancestor.values))
                .collect(),
            set: FOCUS_SET.to_vec(),
            known: known.sent.clone(),
            completes: CompletionFor::Focus,
        },
    };
    let Outcome::Result(read) = cx.ask(complete, due_us).await else {
        return;
    };
    let read_by = ReadBy::Uia {
        runtime_id: focus.runtime_id.clone(),
        token: Some(focus.token),
    };
    let said = execute(
        &mut cx.facts_mut(),
        (&focus.tell, &window),
        &read,
        &known,
        read_by,
    );
    for said in said {
        cx.emit(said);
    }
}
