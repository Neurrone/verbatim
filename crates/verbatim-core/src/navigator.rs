//! The navigator object's commands as event-queue entries
//! (`docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
//! this build"): object navigation, activate, report the current object and
//! move to the focus. Each resolves the navigator object at its turn and
//! makes its reads as the queue's requests, naming the object's window, so
//! each holds the queue until its last answer, as NVDA runs its scripts one
//! at a time on its queue; a command whose request is out is left to
//! finish.
//!
//! The navigator object's window is the committed focus's when the object
//! is that focus's node, else the one the navigation answer that reached
//! it read. An object with neither, put there by the old focus path, is
//! read by no command until that path is deleted.

use verbatim_model::property::PropertyValues;
use verbatim_model::request::{FocusNode, NodeRead, Reply, Request as Wire};
use verbatim_model::{
    ActionName, Effect, FetchResult, Message, NodeId, NodeSnapshot, QueryKind, SegmentContent,
    TraceId, UtteranceSegment,
};

use crate::editing::{self, NavigatorText};
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::focus::versions::Committed;
use crate::key_line::ReviewPress;
use crate::reduce::{self, Reason};
use crate::state::{Navigator, NavigatorRead, SrState};
use crate::text_request::TextRequest;

/// The navigator object and its window, as a command finds them at its
/// turn.
struct On {
    object: NodeSnapshot,
    window: RequestWindow,
}

/// What a command finds of the navigator at its turn.
enum Found {
    /// No navigator object: "No navigator object".
    Nothing,
    /// An object with no window to name: nothing is asked or said.
    Unread,
    /// The object and its window.
    On(Box<On>),
}

fn found(state: &SrState) -> Found {
    let Some(navigator) = state.navigator.as_ref() else {
        return Found::Nothing;
    };
    let window = match &state.versions.committed {
        Committed::Read(kept) if kept.node == navigator.object.id => Some(kept.window),
        _ => navigator.window,
    };
    match window {
        Some(window) => Found::On(Box::new(On {
            object: navigator.object.clone(),
            window,
        })),
        None => Found::Unread,
    }
}

/// One navigation step from a node, with the properties of the node
/// reached.
#[derive(Debug)]
struct Navigate {
    window: RequestWindow,
    node: NodeId,
    direction: QueryKind,
}

impl Request for Navigate {
    type Reply = Option<NodeRead>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Navigate {
            node: self.node,
            direction: self.direction,
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Navigated(read) => Some(read),
            _ => None,
        }
    }
}

/// A node's default action.
#[derive(Debug)]
struct Activate {
    window: RequestWindow,
    node: NodeId,
}

impl Request for Activate {
    type Reply = Option<ActionName>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Activate { node: self.node }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Activated(action) => Some(action),
            _ => None,
        }
    }
}

/// A node's properties, read live.
#[derive(Debug)]
struct Properties {
    window: RequestWindow,
    node: NodeId,
}

impl Request for Properties {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Properties {
            node: self.node,
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Properties(values) => Some(values),
            _ => None,
        }
    }
}

/// `read` as the snapshot the announcement code speaks, read through the
/// backend of `like`, the object it was reached from.
fn snapshot(read: NodeRead, like: &NodeSnapshot) -> Option<NodeSnapshot> {
    crate::focus::execute::snapshot(
        &FocusNode {
            read,
            runtime_id: None,
        },
        like.backend,
    )
}

/// Emits `effects`.
fn emit_all(cx: &mut Cx<Entries>, effects: Vec<Effect>) {
    for effect in effects {
        cx.emit(effect);
    }
}

/// Says "No navigator object".
fn no_navigator(cx: &mut Cx<Entries>, trace_id: TraceId) {
    cx.emit(editing::speak(
        trace_id,
        vec![UtteranceSegment::new(SegmentContent::Message(
            Message::NoNavigatorObject,
        ))],
    ));
}

/// The text of `object`, in `window`, for `read`: the focus's caret Core
/// holds without asking, else the selected text, else the line at the
/// caret, each asked in turn ([`editing::navigator_text_start`]).
async fn text(
    cx: &mut Cx<Entries>,
    trace_id: TraceId,
    (object, window): (&NodeSnapshot, RequestWindow),
    read: NavigatorRead,
) {
    let start = editing::navigator_text_start(&mut cx.facts_mut(), trace_id, object, read);
    let mut op = match start {
        Ok(said) => return emit_all(cx, said),
        Err(op) => op,
    };
    let mut selection = true;
    loop {
        let request = TextRequest {
            process: window.process,
            window,
            node: object.id,
            op,
        };
        let due_us = cx.due_in(OTHER_DUE_US);
        let Outcome::Result(reply) = cx.ask(request, due_us).await else {
            return;
        };
        match editing::navigator_text_answered(trace_id, object, read, selection, reply) {
            NavigatorText::Said(said) => return emit_all(cx, said),
            NavigatorText::ReadLine if selection => {
                selection = false;
                op = editing::navigator_line_op();
            }
            NavigatorText::ReadLine => return,
        }
    }
}

/// `object` announced as the navigator object for `reason`, its text read
/// after the rest.
async fn announce(
    cx: &mut Cx<Entries>,
    trace_id: TraceId,
    (object, window): (&NodeSnapshot, RequestWindow),
    reason: Reason,
) {
    let said = reduce::navigator_utterance(&mut cx.facts_mut(), trace_id, object, reason);
    cx.emit(said);
    text(cx, trace_id, (object, window), NavigatorRead::Announce).await;
}

/// A navigation press, `kind` from the navigator object, at its turn: the
/// navigator moves to the node reached and announces it, or the edge is
/// said.
pub(crate) async fn navigate(mut cx: Cx<Entries>, press: ReviewPress, kind: QueryKind) {
    let trace_id = press.trace_id;
    let found = found(&cx.facts());
    let on = match found {
        Found::Nothing => return no_navigator(&mut cx, trace_id),
        Found::Unread => return,
        Found::On(on) => on,
    };
    let request = Navigate {
        window: on.window,
        node: on.object.id,
        direction: kind,
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let result = match cx.ask(request, due_us).await {
        Outcome::Result(Some(read)) => {
            let window = RequestWindow::of_node(&read.window);
            let Some(reached) = snapshot(read, &on.object) else {
                return;
            };
            cx.facts_mut().navigator = Some(Navigator {
                window: Some(window),
                ..Navigator::on(reached.clone())
            });
            announce(&mut cx, trace_id, (&reached, window), Reason::Focus).await;
            return;
        }
        Outcome::Result(None) => FetchResult::NoNeighbor,
        Outcome::NotThere => FetchResult::Gone,
        _ => return,
    };
    let said = reduce::navigate_completed(&mut cx.facts_mut(), trace_id, press, kind, &result);
    emit_all(&mut cx, said);
}

/// Activate at its turn: the navigator object's default action, its
/// outcome said. An activation stopped or refused because its window is
/// hung, or no longer awaited, says nothing either way, since whether it
/// happened is unknown; whatever it caused announces itself.
pub(crate) async fn activate(mut cx: Cx<Entries>, trace_id: TraceId) {
    let found = found(&cx.facts());
    let on = match found {
        Found::Nothing => return no_navigator(&mut cx, trace_id),
        Found::Unread => return,
        Found::On(on) => on,
    };
    let request = Activate {
        window: on.window,
        node: on.object.id,
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let said = match cx.ask(request, due_us).await {
        Outcome::Result(action) => reduce::activation_completed(trace_id, true, action.as_ref()),
        Outcome::NotResponding | Outcome::NoLongerAwaited(_) => return,
        Outcome::Refused(_) | Outcome::NotThere | Outcome::ReaderEnded => {
            reduce::activation_completed(trace_id, false, None)
        }
    };
    emit_all(&mut cx, said);
}

/// Report the current object at its turn: the navigator object read live,
/// then on the first press its announcement, on the second its name and
/// text spelled, on the third copied (NVDA's multi-press semantics).
pub(crate) async fn report_object(mut cx: Cx<Entries>, press: ReviewPress) {
    let trace_id = press.trace_id;
    let found = found(&cx.facts());
    let on = match found {
        Found::Nothing => return no_navigator(&mut cx, trace_id),
        Found::Unread => return,
        Found::On(on) => on,
    };
    let request = Properties {
        window: on.window,
        node: on.object.id,
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(values) = cx.ask(request, due_us).await else {
        return;
    };
    let read = NodeRead {
        node: Some(on.object.id),
        window: on.window.node_window(),
        values,
    };
    let Some(live) = snapshot(read, &on.object) else {
        return;
    };
    let object = {
        let mut facts = cx.facts_mut();
        let Some(navigator) = facts.navigator.as_mut() else {
            return;
        };
        // The desktop window keeps the name Core gives it, which a live
        // read of the window does not carry.
        if !navigator.object.details.desktop_window {
            navigator.object = live;
        }
        let flat = crate::review::text_of(&navigator.object);
        if !flat.is_char_boundary(navigator.review_offset) {
            navigator.review_offset = 0;
        }
        navigator.object.clone()
    };
    let has_text = editing::may_have_text(object.role);
    match press.repeat {
        0 => {
            reduce::focus_spoken_now(&mut cx.facts_mut(), &object);
            announce(&mut cx, trace_id, (&object, on.window), Reason::Query).await;
        }
        1 | 2 if has_text => {
            let read = if press.repeat == 1 {
                NavigatorRead::Spell
            } else {
                NavigatorRead::Copy
            };
            text(&mut cx, trace_id, (&object, on.window), read).await;
        }
        1 => {
            // NVDA spells the name and value joined by a space.
            let segments = reduce::spelled(&reduce::clipboard_text(&object));
            if !segments.is_empty() {
                cx.emit(editing::speak_about(trace_id, &object, segments));
            }
        }
        _ => {
            let text = reduce::clipboard_text(&object);
            if !text.is_empty() {
                cx.emit(Effect::CopyToClipboard(text));
            }
        }
    }
}

/// Move to focus at its turn: the navigator and review cursor go to the
/// focus, announced after "Move to focus", its text read after the rest.
/// With nothing focused it does nothing.
pub(crate) async fn to_focus(mut cx: Cx<Entries>, trace_id: TraceId) {
    let (object, window) = {
        let facts = cx.facts();
        let Some(focus) = facts.focus.object().filter(|focus| focus.alive) else {
            return;
        };
        let Committed::Read(kept) = &facts.versions.committed else {
            return;
        };
        if kept.node != focus.snapshot.id {
            return;
        }
        (focus.snapshot.clone(), kept.window)
    };
    let said = {
        let mut facts = cx.facts_mut();
        facts.navigator = Some(Navigator::on(object.clone()));
        reduce::focus_spoken_now(&mut facts, &object);
        reduce::navigator_utterance(&mut facts, trace_id, &object, Reason::Focus)
    };
    let mut said = said;
    if let Effect::Speak(utterance, _) = &mut said {
        utterance.segments.insert(
            0,
            UtteranceSegment::new(SegmentContent::Message(Message::MoveToFocus)),
        );
    }
    cx.emit(said);
    text(
        &mut cx,
        trace_id,
        (&object, window),
        NavigatorRead::Announce,
    )
    .await;
}

#[cfg(test)]
#[path = "navigator_tests.rs"]
mod tests;
