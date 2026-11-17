//! A UI Automation focus at the front (design, "Focus and foreground" and
//! "The focus answers"): judged on its tell's facts, then read in two
//! requests chained under one due time, the listener's read of the
//! element it kept, answering a snapshot and the element's route, and the
//! application's outpost's completion from the route, answering the focus;
//! then executed.

use std::iter;

use verbatim_model::OutpostId;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{
    CompleteUia, CompletionFor, FocusRead, ObjectRead, ObjectRef, Purpose, ReadObject, Reply,
    Request as Wire, RouteEnd,
};
use verbatim_model::tell::{ElementToken, Read, TellWindow};

use super::FOCUS_SET;
use super::execute::{Known, Origin, again, execute};
use super::pending::counts_as_pending;
use super::retake::{CurrentFocusRead, read_by};
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

/// Whether `focus` is a menu opening on the committed focus's runtime id.
fn opens_on_committed(cx: &Cx<Entries>, focus: &UiaFocus) -> bool {
    focus.tell.kind == EventKind::UiaMenuOpened
        && cx
            .facts()
            .versions
            .committed_read()
            .is_some_and(|kept| {
                matches!(&kept.read_by, ReadBy::Uia { runtime_id, .. } if *runtime_id == focus.runtime_id)
            })
}

/// Whether the check thread found `focus`'s element without the keyboard
/// focus at delivery. A menu opening on the committed focus is a focus on
/// it, subject to that answer as any focus is
/// (`docs/parity/focus-events.md`), so one whose element lost the keyboard
/// focus is dropped.
fn lost_keyboard_focus(focus: &UiaFocus) -> bool {
    focus
        .tell
        .checked
        .as_ref()
        .and_then(|checked| checked.keyboard_focus)
        == Some(Read::Answered(false))
}

/// Whether `focus` names Verbatim's hidden frame's own element, which is
/// dropped (Verbatim-only, design "Focus and foreground").
fn on_hidden_frame(focus: &UiaFocus) -> bool {
    let TellWindow::Found(facts) = &focus.tell.window else {
        return false;
    };
    facts.hidden_frame
        && matches!(
            focus
                .tell
                .cached
                .answer(PropertyId::NativeWindow)
                .and_then(|answer| answer.value.as_ref()),
            Some(PropertyValue::Window(window)) if *window == facts.window.handle
        )
}

/// The function of a UI Automation focus entry, or of a menu opening: a
/// menu opening on the committed focus executes it again; otherwise the
/// native-element test with the verdict, the overrides, acceptance, the
/// keyboard-focus answer and the duplicate test on its tell's facts; then
/// the two reads; then execution. Any outcome but a result ends it
/// unspoken and clears Core's caret baseline.
pub(crate) async fn run(mut cx: Cx<Entries>, focus: UiaFocus) {
    if opens_on_committed(&cx, &focus) {
        if lost_keyboard_focus(&focus) {
            return;
        }
        let said = again(&mut cx.facts_mut(), focus.tell.trace_id);
        for said in said {
            cx.emit(said);
        }
        return;
    }
    if on_hidden_frame(&focus) {
        return;
    }
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
            set: crate::details::walk_set(&cx.facts(), FOCUS_SET),
            relation: None,
            descendants: false,
            walk: Some(walk),
            purpose: Purpose::Focus { keep },
        },
    };
    let Outcome::Result(object) = cx.ask(read, due_us).await else {
        cx.facts_mut().focus_read_failed();
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
        afresh(cx, &focus, window, known, due_us).await;
        return;
    }
    let complete = Completion {
        window,
        complete: CompleteUia {
            route,
            read: iter::once(object.object.values)
                .chain(object.ancestors.into_iter().map(|ancestor| ancestor.values))
                .collect(),
            set: crate::details::node_set(&cx.facts(), FOCUS_SET),
            known: known.sent.clone(),
            completes: CompletionFor::Focus,
            details: crate::details::focus_details(&cx.facts()),
        },
    };
    let read = match cx.ask(complete, due_us).await {
        Outcome::Result(read) => read,
        // The outpost ended: its registrations went with it (Table 10,
        // column F).
        Outcome::ReaderEnded => {
            cx.facts_mut().focus_read_failed();
            return;
        }
        Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::NoLongerAwaited(_) => {
            cx.facts_mut().focus_read_failed();
            unexecuted(&mut cx, window);
            return;
        }
    };

    let read_by = ReadBy::Uia {
        runtime_id: focus.runtime_id.clone(),
        token: Some(focus.token),
    };
    super::narrow::judge_ahead(&mut cx, &read);
    let origin = origin(&focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    if !executed.committed {
        unexecuted(&mut cx, window);
    }
    executed.deliver(&mut cx);
}

/// The origin of `focus` read on `window`: its tell's, with Alt as Core's
/// key state had it at the tell's arrival.
fn origin(
    focus: &UiaFocus,
    window: RequestWindow,
    foreground: Option<crate::event_queue::Foreground>,
) -> Origin {
    let mut origin = Origin::of_tell(&focus.tell, window, foreground);
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
    mut cx: Cx<Entries>,
    focus: &UiaFocus,
    window: RequestWindow,
    known: Known,
    due_us: u64,
) {
    let request = CurrentFocusRead {
        window,
        known: known.sent.clone(),
        details: crate::details::focus_details(&cx.facts()),
        set: crate::details::node_set(&cx.facts(), FOCUS_SET),
        focused_state_test: false,
    };
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        cx.facts_mut().focus_read_failed();
        return;
    };
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    executed.deliver(&mut cx);
}

/// A completion the outpost of `window`'s application answered, or Core
/// stopped waiting for, ended without its focus executed: a `Follow`
/// naming what Core names there removes the completion's registration
/// (Table 10, column C).
fn unexecuted(cx: &mut Cx<Entries>, window: RequestWindow) {
    let request = cx.facts().follows.unexecuted(window.process, window);
    super::follow::send(cx, vec![request]);
}
