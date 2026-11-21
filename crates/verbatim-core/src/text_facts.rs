//! Core's text facts for the committed focus, [`SelectionFacts`]: the
//! selection baseline and the text-changed flag, kept on the committed
//! focus's record, and the functions that mint, take and clear them
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text
//! facts").

use serde::{Deserialize, Serialize};
use verbatim_model::request::{FocusCaret, Question, Request as Wire};
use verbatim_model::{EditSelection, NodeId, OutpostId, TextAnchor, TextOp, TextProvider};

use crate::focus::versions::{Focus, Kept, ReadBy};
use crate::state::SrState;

/// The committed focus's text facts: the baseline and the flag, each
/// moved by its own rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SelectionFacts {
    /// The selection baseline the focus's outpost generation holds, `None`
    /// when Core holds none.
    pub(crate) baseline: Option<TextAnchor>,
    /// The text-changed flag.
    pub(crate) text_changed: bool,
}

/// How the reader reads the text of `kept`, the provider Core names in
/// each of its text requests: a plain window's reader, Core's own, reads
/// no text and takes none, so it is named as an MSAA window's is.
pub(crate) fn provider(kept: &Kept) -> TextProvider {
    match kept.read_by {
        ReadBy::Uia { .. } => TextProvider::Uia,
        ReadBy::Msaa { .. } | ReadBy::Plain => TextProvider::EditMessages,
    }
}

/// Whether `focus` is editable text with automatic selection detection,
/// whose value changes set the text-changed flag: every text control
/// Verbatim reads detects selection changes automatically, and a terminal
/// has no selection baseline (`books/dev/src/reference/crates/verbatim-core.md`,
/// "Caret tells and Core's text facts" and "Terminals").
pub(crate) fn detects_selection(focus: &Focus) -> bool {
    let role = focus.context.snapshot.role;
    crate::editing::may_have_text(role) && !crate::editing::is_grid(role)
}

/// The committed focus's record, when its reader lives and its node is
/// `node`.
fn committed_on(state: &mut SrState, node: NodeId) -> Option<&mut Kept> {
    state
        .versions
        .live_mut()
        .filter(|focus| focus.context.snapshot.id == node)
        .map(|focus| &mut focus.kept)
}

/// Core's own move or selection on `node` minted `baseline`: when `node`
/// is the committed focus, it becomes the baseline, nothing said, the flag
/// kept. Verbatim-only, today's
/// `keep_made`: Verbatim's own moves are not spoken as selection changes.
pub(crate) fn minted(state: &mut SrState, node: NodeId, baseline: TextAnchor) {
    if let Some(kept) = committed_on(state, node) {
        kept.selection.baseline = Some(baseline);
    }
}

/// A caret tell's read found that `node`, the committed focus, has no
/// text: Core holds no caret and no selection for it, as a focus answer
/// with no text holds none ([`focus_answered`]), nothing said. NVDA has no
/// selection to compare for an object with no text interface, which is no
/// `EditableText`, so its caret event says nothing
/// (`NVDAObjects/__init__.py` 1424 to 1429).
pub(crate) fn no_text(state: &mut SrState, node: NodeId) {
    if state
        .versions
        .live()
        .is_some_and(|focus| focus.context.snapshot.id == node)
    {
        state.forget_caret();
    }
    if let Some(kept) = committed_on(state, node) {
        kept.selection = SelectionFacts::default();
    }
}

/// A value change of the committed focus, editable text with automatic
/// selection detection, reached its turn: the flag is set, with
/// no request and nothing said. NVDA: `NVDAObjects/IAccessible/__init__.py`
/// 2017 to 2021.
pub(crate) fn value_changed(kept: &mut Kept) {
    kept.selection.text_changed = true;
}

/// The committed focus's outpost generation `outpost` ended:
/// its baseline's anchor ended with it; the flag is the focus's and stays.
pub(crate) fn outpost_ended(focus: &mut Focus, outpost: OutpostId) {
    if focus.context.snapshot.id.outpost() == Some(outpost) {
        focus.kept.selection.baseline = None;
    }
}

/// How a caret tell's answer with an edit control's selection `now` stands
/// to the previous caret tell's: equal, it says nothing and changes nothing,
/// as NVDA's `Edit.event_caret` returns before comparing
/// (`NVDAObjects/window/edit.py` 1068 to 1074). The previous offsets are
/// replaced either way, as NVDA's are, whatever object they were read in.
pub(crate) fn edit_selection_repeated(state: &mut SrState, now: EditSelection) -> bool {
    state.edit_selection.replace(now) == Some(now)
}

/// A focus answer's caret, `caret`, for a focus being committed whose
/// text facts are `facts` by a focus answer: a caret read mints
/// the baseline and clears the flag, silently, a repeated focus's too, as
/// NVDA's `initAutoSelectDetection` sets the baseline of the object gaining
/// focus (`editableText.py` 417 to 427; Dickson, 2026-11-17); no caret
/// clears both.
pub(crate) fn focus_answered(facts: &mut SelectionFacts, caret: &FocusCaret) {
    *facts = match caret {
        FocusCaret::Read(read) => SelectionFacts {
            baseline: Some(read.baseline),
            text_changed: false,
        },
        FocusCaret::NoText | FocusCaret::NotRead => SelectionFacts::default(),
    };
}

/// The caret report a batch's `Caret` result `caret` and the caret's
/// `line`, read from its caret, make. The selection, when something is
/// selected, starts at the caret, as NVDA's caret is the selection's start
/// (the readers' caret is), so its start is the caret's place in the line,
/// unless the line was cut before the caret, when the start is the
/// position the `Caret` holds, in no line reported; its end is the end the
/// `Caret` holds. A selection starting at the text's start marks its line
/// the text's first, as the caret reads made it; a caret alone there does
/// not, so Backspace there after a focus says nothing, as it did
/// (Claude, 2026-11-22).
pub(crate) fn caret_report(
    caret: &verbatim_model::batch::CaretAt,
    mut line: verbatim_model::TextChunk,
) -> verbatim_model::CaretReport {
    let cut_before = line.truncated
        && usize::try_from(line.offset).is_ok_and(|offset| offset >= line.text.len());
    let selection = caret.selection.map(|selection| {
        let start = if cut_before {
            verbatim_model::TextPosition::at(selection.start.anchor)
        } else {
            line.first |= selection.start.edges.start;
            verbatim_model::TextPosition {
                anchor: line.start,
                offset: line.offset,
            }
        };
        verbatim_model::CaretSelection {
            range: verbatim_model::Selection {
                start,
                end: verbatim_model::TextPosition::at(selection.end.anchor),
            },
        }
    });
    verbatim_model::CaretReport { line, selection }
}

/// The selection baseline the committed focus's text facts hold, with
/// the node whose text it is in.
pub(crate) fn held(state: &SrState) -> Option<(NodeId, TextAnchor)> {
    let focus = state.versions.live()?;
    focus
        .kept
        .selection
        .baseline
        .map(|baseline| (focus.context.snapshot.id, baseline))
}

/// The nodes whose text `request` is about, each with the text anchors it
/// names there, which stay held while it waits or is out: a batch's by its
/// one list ([`verbatim_model::batch::Batch::anchors`]).
pub(crate) fn named_by(request: &Question) -> Vec<(NodeId, Vec<TextAnchor>)> {
    match request {
        Question::Batch(batch) => batch.anchors(),
        Question::Legacy(request) => legacy_named_by(request).into_iter().collect(),
    }
}

/// The node a kind of request not yet converted to a batch is about, and
/// every text anchor it names.
fn legacy_named_by(request: &Wire) -> Option<(NodeId, Vec<TextAnchor>)> {
    let Wire::Text { node, op } = request else {
        return None;
    };
    let anchors = match op {
        TextOp::ReadTerminal(read) => read.start.into_iter().collect(),
        TextOp::TerminalHistory { start, .. } | TextOp::TerminalFind { start, .. } => {
            vec![*start]
        }
        _ => Vec::new(),
    };
    Some((*node, anchors))
}

#[cfg(test)]
#[path = "text_facts_tests.rs"]
mod tests;
