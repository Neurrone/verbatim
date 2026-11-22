//! The navigator object's commands as event-queue entries
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Object navigation and review"): object
//! navigation, activate, report the current object and
//! move to the focus. Each resolves the navigator object at its turn and
//! makes its reads as the queue's requests, naming the object's window, so
//! each holds the queue until its last answer, as NVDA runs its scripts one
//! at a time on its queue; a command whose request is out is left to
//! finish.
//!
//! The navigator object's window is the committed focus's when the object
//! is that focus's node, else the one the read that put the navigator
//! there found. An object with neither, the focus executed and no longer
//! the committed focus's node, is read by no command.

use verbatim_model::batch::Relation;
use verbatim_model::property::{PropertyId, PropertyValues};
use verbatim_model::request::{FocusNode, NodeRead};
use verbatim_model::{
    Effect, Message, NodeId, NodeSnapshot, Pid, QueryKind, SegmentContent, TextDetails, TextPoint,
    TextUnit, TraceId, UtteranceSegment,
};

use crate::batch::{Single, flat};
use crate::editing::{self, NavigatorText};
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, RequestWindow};
use crate::keys::entry::ReviewPress;
use crate::reduce::{self, Reason};
use crate::state::{Navigator, NavigatorRead, SrState};
use crate::text_request::Answerer;

/// The navigator object and its window, as a command finds them at its
/// turn.
pub(crate) struct On {
    pub(crate) object: NodeSnapshot,
    pub(crate) window: RequestWindow,
}

/// What a command finds of the navigator at its turn.
pub(crate) enum Found {
    /// No navigator object: "No navigator object".
    Nothing,
    /// An object with no window to name: nothing is asked or said.
    Unread,
    /// The object and its window.
    On(Box<On>),
}

pub(crate) fn found(state: &SrState) -> Found {
    let Some(navigator) = state.navigator.as_ref() else {
        return Found::Nothing;
    };
    let window = match state.versions.live() {
        Some(focus) if focus.context.snapshot.id == navigator.object.id => Some(focus.kept.asked),
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

/// The values of `set` of node `node`, read live by the outpost of
/// `process`, naming `window`, under `due_us`.
pub(crate) async fn read_values(
    cx: &mut Cx<Entries>,
    (process, window, node): (Pid, RequestWindow, NodeId),
    set: &[PropertyId],
    due_us: u64,
) -> Outcome<PropertyValues> {
    let mut batch = cx.batch(|| ());
    let read = batch.read(node, set);
    let request = batch
        .finish()
        .of(Reader::Outpost(process), window)
        .one(read);
    match flat(cx.ask(request, due_us).await).split() {
        Ok(read) => Outcome::Result(read.values),
        Err(other) => other,
    }
}

/// `read` as the snapshot the announcement code speaks, read through the
/// backend of `like`, the object it was reached from.
pub(crate) fn snapshot(read: NodeRead, like: &NodeSnapshot) -> Option<NodeSnapshot> {
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
pub(crate) fn no_navigator(cx: &mut Cx<Entries>, trace_id: TraceId) {
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
    let (start, end) = match start {
        Ok(said) => return emit_all(cx, said),
        Err(ends) => ends,
    };
    let by = Answerer::Outpost(window);
    let due_us = cx.due_in(OTHER_DUE_US);
    let mut sent = crate::batch::batch(|| ());
    let range = sent.read_range(object.id, (start.into(), end.into()), None);
    let Some(answers) = crate::text_request::ask(cx, by, sent.finish(), due_us).await else {
        return;
    };
    let range = crate::text_request::got(&answers, range);
    if let NavigatorText::Said(said) =
        editing::navigator_selection_answered(trace_id, object, read, range)
    {
        return emit_all(cx, said);
    }
    let due_us = cx.due_in(OTHER_DUE_US);
    let mut sent = crate::batch::batch(|| ());
    let line = sent.read_text(
        object.id,
        TextPoint::Caret.into(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let Some(answers) = crate::text_request::ask(cx, by, sent.finish(), due_us).await else {
        return;
    };
    let line = crate::text_request::got(&answers, line);
    let said = editing::navigator_line_answered(trace_id, object, read, line);
    emit_all(cx, said);
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
    let relation = match kind {
        QueryKind::Parent => Relation::Parent,
        QueryKind::FirstChild => Relation::FirstChild,
        QueryKind::NextSibling => Relation::NextSibling,
        QueryKind::PreviousSibling => Relation::PreviousSibling,
    };
    let set = crate::details::node_set(&cx.facts(), &crate::report_focus::ANNOUNCED_SET);
    let mut batch = cx.batch(|| ());
    let reached = batch.related(on.object.id, relation, &set);
    let request = batch
        .finish()
        .of(Reader::Outpost(on.window.process), on.window)
        .one(reached);
    let due_us = cx.due_in(OTHER_DUE_US);
    match cx.ask(request, due_us).await {
        Outcome::Result(Single::Done(reached)) if !reached.is_empty() => {
            let Some(read) = reached.into_iter().next() else {
                return;
            };
            let window = RequestWindow::of_node(&read.window);
            let Some(reached) = snapshot(read, &on.object) else {
                return;
            };
            cx.facts_mut().navigator = Some(Navigator::on(reached.clone(), Some(window)));
            announce(&mut cx, trace_id, (&reached, window), Reason::Focus).await;
        }
        // A tree edge or a node not there: the navigator stays put and the
        // edge is said.
        Outcome::Result(Single::Done(_) | Single::NotThere) | Outcome::NotThere => {
            emit_all(&mut cx, reduce::navigation_edge(trace_id, kind));
        }
        // No answer: the navigator stays where it is, and nothing is known
        // to speak.
        _ => {}
    }
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
    let mut batch = cx.batch(|| ());
    let activated = batch.activate(on.object.id);
    let request = batch
        .finish()
        .of(Reader::Outpost(on.window.process), on.window)
        .one(activated);
    let due_us = cx.due_in(OTHER_DUE_US);
    let said = match flat(cx.ask(request, due_us).await) {
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
    let set = crate::details::node_set(&cx.facts(), &crate::report_focus::ANNOUNCED_SET);
    let due_us = cx.due_in(OTHER_DUE_US);
    let Outcome::Result(values) = read_values(
        &mut cx,
        (on.window.process, on.window, on.object.id),
        &set,
        due_us,
    )
    .await
    else {
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
    let (object, of_focus) = {
        let mut facts = cx.facts_mut();
        // The navigator is the focus's own object when it is the focus's
        // reading, not a copy a navigation fetched of the same node.
        let of_focus = match (
            facts.versions.focus().map(|focus| &focus.context),
            facts.navigator.as_ref(),
        ) {
            (Some(focus), Some(navigator)) => focus.snapshot == navigator.object,
            _ => false,
        };
        let Some(navigator) = facts.navigator.as_mut() else {
            return;
        };
        // The desktop window keeps the name Core gives it, which a live
        // read of the window does not carry.
        if !navigator.object.details.desktop_window {
            navigator.object = live;
        }
        (navigator.object.clone(), of_focus)
    };
    let has_text = editing::may_have_text(object.role);
    match press.repeat {
        0 => {
            if of_focus {
                reduce::focus_read_spoken(&mut cx.facts_mut(), &object);
            }
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
        let Some(focus) = facts.versions.live() else {
            return;
        };
        (focus.context.snapshot.clone(), focus.kept.asked)
    };
    let said = {
        let mut facts = cx.facts_mut();
        facts.navigator = Some(Navigator::on(object.clone(), Some(window)));
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
