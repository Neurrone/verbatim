//! A caret tell as an event-queue entry (`books/dev/src/reference/crates/verbatim-core.md`,
//! "Caret tells and Core's text facts"): the system
//! caret's location change or show from the listener, or a UI Automation
//! text-selection change from the outpost following the focus.
//!
//! At its turn on the committed focus, with no focus pending, it asks one
//! caret read naming the selection baseline; the outpost compares the
//! selection now with the baseline's range as it now is and mints the next
//! baseline. Core speaks the difference in general terms when the
//! text-changed flag is set, or, in an edit control, when a value change of
//! the focus waits in the queue at the answer, which it takes as counted;
//! otherwise in specific terms. NVDA: `NVDAObjects/behaviors.py` 346 to
//! 349; `editableText.py` 429 to 451; `speech/speech.py` 1340 to 1418;
//! `NVDAObjects/window/edit.py` 1068 to 1080.
//!
//! The system caret's tell is for the readers' last focus, whatever its
//! window, as NVDA handles the MSAA caret's events as the last queued
//! focus's (`IAccessibleHandler/__init__.py` 634 to 666), and is said only
//! when that is the committed focus; a UI Automation tell is for the
//! element its runtime id names, said only when that is the committed
//! focus (`UIAHandler/__init__.py`).

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{Missing, StepRefusal};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::MsaaIds;
use verbatim_model::tell::{ElementToken, Event, EventObject};
use verbatim_model::{NodeId, TextProvider};

use crate::batch::Got;
use crate::changes::MsaaChange;
use crate::entries::{Entries, Entry, request_window};
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, RequestWindow};
use crate::focus::acceptance;
use crate::focus::versions::{Focus, Kept, ReadBy};
use crate::keys::entry::Fence;

/// The object a caret tell is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Target {
    /// The system caret's: the readers' last focus.
    System(MsaaIds),
    /// A UI Automation element, by its runtime id.
    Uia(RuntimeId),
}

/// A caret tell, parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaretTell {
    /// The tell.
    pub(crate) tell: Event,
    /// The object it is for.
    pub(crate) target: Target,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
}

impl CaretTell {
    /// The caret tell `tell` is, `None` for a tell of another kind, or one
    /// whose object does not fit its kind, a reader's fault.
    pub(crate) fn of_tell(tell: Event) -> Option<Self> {
        let target = match (tell.kind, &tell.object) {
            (
                EventKind::SystemCaret,
                EventObject::Msaa {
                    window,
                    object,
                    child,
                },
            ) => Target::System(MsaaIds {
                window: *window,
                object: *object,
                child: *child,
            }),
            (
                EventKind::UiaTextSelectionChanged,
                EventObject::Uia { runtime_id, .. } | EventObject::Followed { runtime_id },
            ) => Target::Uia(runtime_id.clone()),
            _ => return None,
        };
        Some(Self {
            tell,
            target,
            fence: Fence::default(),
        })
    }

    /// The element the outpost adopts as the node's when the node's own
    /// has no text pattern
    /// (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts"): a UI Automation tell's, from the outpost following the
    /// focus, when the outpost kept it; the system caret's names none.
    pub(crate) fn adopted(&self) -> Option<ElementToken> {
        match (&self.target, &self.tell.object) {
            (Target::Uia(_), EventObject::Uia { token, .. }) => Some(*token),
            _ => None,
        }
    }

    /// The window its request names, for the foreground test.
    pub(crate) fn window(&self) -> Option<RequestWindow> {
        request_window(&self.tell)
    }
}

/// Whether two caret tells waiting for one object merge into one entry:
/// UI Automation tells of one element from one thread. The system caret's
/// merge by the MSAA limiter's place (NVDA:
/// `IAccessibleHandler/orderedWinEventLimiter.py` 79).
pub(crate) fn merges(waiting: &CaretTell, arriving: &CaretTell) -> bool {
    matches!((&waiting.target, &arriving.target), (Target::Uia(a), Target::Uia(b)) if a == b)
        && waiting.tell.thread == arriving.tell.thread
}

/// What a caret tell's batch read on the committed focus's node.
#[derive(Debug)]
struct Told {
    /// The caret as it is now: the line at it, with the caret at its
    /// offset, and the selection.
    caret: verbatim_model::CaretReport,
    /// The next selection baseline, the selection as the `Caret` held it.
    baseline: verbatim_model::TextAnchor,
    /// The difference from the baseline Core named; `None` when it named
    /// none, or one the outpost no longer holds ("no baseline").
    difference: Option<verbatim_model::SelectionDifference>,
    /// The selection's offsets, where the text counts by offsets (an edit
    /// control's).
    edit_selection: Option<verbatim_model::EditSelection>,
}

/// How a caret tell's batch ended.
enum Read {
    /// It read the caret.
    Told(Box<Told>),
    /// The node has no text.
    NoText,
    /// It failed, or was not answered.
    Failed,
}

/// Asks a caret tell's batch on `node` (the request list's design, section
/// 6, `ReadSelection`): the caret, adopting the tell's element; the
/// caret's line; and the selection compared with `baseline` when Core
/// holds one.
async fn read(
    cx: &mut Cx<Entries>,
    (window, node): (RequestWindow, NodeId),
    (baseline, adopt): (Option<verbatim_model::TextAnchor>, Option<ElementToken>),
    due_us: u64,
) -> Read {
    let mut batch = cx.batch(|| ());
    let caret = batch.caret(node, adopt);
    let line = batch.read_text(
        node,
        caret.caret(),
        None,
        verbatim_model::TextUnit::Line,
        1,
        verbatim_model::TextDetails::NONE,
    );
    let compared = baseline.map(|baseline| {
        batch.compare_selection(node, crate::batch::Baseline::Held(baseline), caret)
    });
    let request = batch.finish().of(Reader::Outpost(window.process), window);
    let Outcome::Result(answers) = cx.ask(request, due_us).await else {
        return Read::Failed;
    };
    let at = match answers.get(caret) {
        Got::Done(at) => at,
        Got::Refused(StepRefusal::NoText) => return Read::NoText,
        _ => return Read::Failed,
    };
    let Got::Done(line) = answers.get(line) else {
        return Read::Failed;
    };
    let Some(line) = line.chunks.first() else {
        return Read::Failed;
    };
    let difference = match compared.map(|compared| answers.get(compared)) {
        None | Some(Got::NotThere(Missing::Anchor(_))) => None,
        Some(Got::Done(compared)) => Some(compared.difference.clone()),
        Some(_) => return Read::Failed,
    };
    Read::Told(Box::new(Told {
        caret: crate::text_facts::caret_report(at, line.clone()),
        baseline: at.held,
        difference,
        edit_selection: at.offsets,
    }))
}

/// The committed focus `tell` is for, when its reader lives: the system
/// caret's, when the readers' last focus is the committed focus (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", the committed focus); a UI Automation tell's, when its runtime id is the
/// committed focus's.
fn target(cx: &Cx<Entries>, tell: &CaretTell) -> Option<Focus> {
    let focus = {
        let facts = cx.facts();
        match &tell.target {
            Target::System(_) => facts.versions.caret_target().cloned(),
            Target::Uia(runtime_id) => facts
                .versions
                .live()
                .filter(|focus| matches!(&focus.kept.read_by, ReadBy::Uia { runtime_id: id, .. } if id == runtime_id))
                .cloned(),
        }
    }?;
    let live = cx
        .instance(Reader::Outpost(focus.kept.asked.process))
        .is_some_and(|instance| Some(instance.outpost_id()) == focus.context.snapshot.id.outpost());
    live.then_some(focus)
}

/// Whether `change` is an MSAA value change of the committed focus
/// `kept`'s own window object, whose value is its text.
fn value_change_of(kept: &Kept, change: &MsaaChange) -> bool {
    /// The object id of a window's client area.
    const OBJID_CLIENT: i32 = -4;
    change.tell.kind == EventKind::ValueChange
        && change.ids.window == kept.place.window
        && change.ids.object == OBJID_CLIENT
        && change.ids.child == 0
}

/// Whether `change` is a value change of the committed focus `kept`, read
/// through an edit control's messages: a change of its text.
pub(crate) fn edit_text_change(kept: &Kept, change: &MsaaChange) -> bool {
    crate::text_facts::provider(kept) == TextProvider::EditMessages && value_change_of(kept, change)
}

/// Whether `change`, an MSAA change at its turn, is a value change of the
/// committed focus `focus` that sets its text-changed flag
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts"): `focus` is editable text with automatic selection detection
/// read through an edit control's messages.
pub(crate) fn sets_text_changed(focus: &Focus, change: &MsaaChange) -> bool {
    edit_text_change(&focus.kept, change) && crate::text_facts::detects_selection(focus)
}

/// Whether the committed focus is a console host's terminal, whose caret
/// event is its caret.
fn console_terminal_committed(state: &crate::state::SrState) -> bool {
    state
        .versions
        .live()
        .is_some_and(|focus| focus.kept.console_terminal)
}

/// Whether `entry` is a system caret tell, dropped at its arrival while
/// the committed focus is a console host's terminal
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Terminals").
/// Verbatim-only: the terminal model.
pub(crate) fn dropped_in_console(state: &crate::state::SrState, entry: &Entry) -> bool {
    matches!(entry, Entry::Caret(caret) if matches!(caret.target, Target::System(_)))
        && console_terminal_committed(state)
}

/// The caret tell's function at its turn.
pub(crate) async fn run(mut cx: Cx<Entries>, tell: CaretTell) {
    if acceptance::accepted(&tell.tell).is_err() {
        return;
    }
    // Dropped again at its turn while a console host's terminal is the
    // committed focus.
    if matches!(tell.target, Target::System(_)) && console_terminal_committed(&cx.facts()) {
        return;
    }
    // An unconfirmed MSAA focus does not count and is not read, as NVDA's
    // events not yet pumped are not pending.
    if crate::focus::validity::pending_behind_front(&cx, crate::focus::validity::Within::ALL)
        == crate::focus::validity::Pending::Counts
    {
        return;
    }
    let Some(focus) = target(&cx, &tell) else {
        return;
    };
    let (kept, node) = (&focus.kept, focus.context.snapshot.id);
    let provider = crate::text_facts::provider(kept);
    let due_us = cx.due_in(OTHER_DUE_US);
    let read = match read(
        &mut cx,
        (kept.asked, node),
        (kept.selection.baseline, tell.adopted()),
        due_us,
    )
    .await
    {
        Read::Told(read) => *read,
        // The node has no text: no caret and no selection, nothing said.
        Read::NoText => {
            crate::text_facts::no_text(&mut cx.facts_mut(), node);
            return;
        }
        // A read that fails says nothing and keeps the baseline and the
        // flag. NVDA: `editableText.py` 433 to 436.
        Read::Failed => return,
    };
    answered(&mut cx, &tell, (&focus, provider), read);
}

/// Judges the caret tell's answer `read` on the committed focus `focus`
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts").
fn answered(
    cx: &mut Cx<Entries>,
    tell: &CaretTell,
    (focus, provider): (&Focus, TextProvider),
    read: Told,
) {
    let (kept, node) = (&focus.kept, focus.context.snapshot.id);
    if let Some(offsets) = read.edit_selection
        && crate::text_facts::edit_selection_repeated(&mut cx.facts_mut(), offsets)
    {
        return;
    }
    cx.emit(verbatim_model::Observed::effect(
        tell.tell.trace_id,
        (tell.tell.process, kept.read_by.backend()),
        None,
        verbatim_model::NormalizedEvent::CaretMoved {
            node_id: node,
            caret: read.caret.clone(),
            selection_changes: read
                .difference
                .as_ref()
                .map(|difference| difference.changes.clone())
                .unwrap_or_default(),
        },
    ));
    let collapsed = read.caret.selection.is_none();
    crate::editing::take_caret(&mut cx.facts_mut(), node, read.caret);
    let Some(difference) = read.difference else {
        // No baseline to compare with: nothing said, the next one minted,
        // the flag kept (column E).
        crate::text_facts::minted(&mut cx.facts_mut(), node, read.baseline);
        return;
    };
    // In an edit control, a value change of the focus waiting now makes
    // the terms general, and is taken as counted, as NVDA runs it inside
    // its caret event (`NVDAObjects/window/edit.py` 1075 to 1080). Taken,
    // it is still the control plane's text change.
    let counted = if provider == TextProvider::EditMessages {
        cx.take_waiting(
            |entry| matches!(entry, Entry::MsaaChange(change) if value_change_of(kept, change)),
        )
    } else {
        Vec::new()
    };
    for entry in &counted {
        if let Entry::MsaaChange(change) = entry {
            crate::changes::observe_text_change(cx, change);
        }
    }
    let value_waiting = !counted.is_empty();
    let general = {
        let mut facts = cx.facts_mut();
        let Some(committed) = facts
            .versions
            .live_mut()
            .filter(|committed| committed.context.snapshot.id == node)
            .map(|committed| &mut committed.kept)
        else {
            return;
        };
        let general = committed.selection.text_changed || value_waiting;
        committed.selection.text_changed = false;
        committed.selection.baseline = Some(read.baseline);
        general
    };
    let segments = crate::editing::selection_segments(
        &difference.changes,
        general.then_some(crate::editing::General {
            selection: difference.new_selection.as_ref(),
            collapsed,
        }),
    );
    // The read saw the selection every key that joined the queue before
    // its answer made, so its words are the newest such key's and carry
    // its number: a modifier's later cut silences them, while the cut of
    // a key that joined while the read was out, whose selection the read
    // saw, does not (`docs/parity/text.md`, "Speech cut off by a
    // modifier").
    let fence = tell.fence.max(cx.facts().arrivals.key_fence());
    // Each part is its own utterance, as NVDA speaks each selection
    // message on its own.
    for mut said in crate::editing::speak_each(tell.tell.trace_id, segments) {
        crate::keys::entry::stamp(fence, &mut said);
        cx.emit_stamped(said);
    }
}

#[cfg(test)]
#[path = "caret_tell_tests.rs"]
mod tests;
