//! Core's text facts for the committed focus (`docs/design/readers-tell-core-asks-build1.md`,
//! "Selection and the caret's text facts", "Held anchors", Table 17): the
//! selection baseline, the held anchor of the selection Core last spoke
//! from, and the text-changed flag, the focus's text having changed since
//! the selection was last compared (NVDA's
//! `hasContentChangedSinceLastSelection`), each kept on the committed
//! focus's record, so a new focus starts with neither.
//!
//! Only a caret tell's read compares selections, as only NVDA's caret
//! event and focus do (`crate::caret_tell`); a caret key's starting and
//! final reads name no baseline and change none. Verbatim's own moves
//! (`MoveCaret`, a sentence move, `Select` on the focused text) mint the
//! next baseline and say nothing of it.

use serde::{Deserialize, Serialize};
use verbatim_model::request::{FocusCaret, Request as Wire};
use verbatim_model::{
    EditSelection, NodeId, OutpostId, TextAnchor, TextOp, TextPoint, TextProvider, TextThrough,
};

use crate::focus::versions::{Committed, Kept, ReadBy};
use crate::state::SrState;

/// Table 17's state for the committed focus: T1 to T4 are the four
/// combinations of the two facts, each moved by its own rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SelectionFacts {
    /// The selection baseline the focus's outpost generation holds, `None`
    /// when Core holds none.
    pub(crate) baseline: Option<TextAnchor>,
    /// The text-changed flag.
    pub(crate) text_changed: bool,
}

/// How the outpost reads the text of `kept`, the provider Core names in
/// each of its text requests.
pub(crate) fn provider(kept: &Kept) -> TextProvider {
    match kept.read_by {
        ReadBy::Uia { .. } => TextProvider::Uia,
        ReadBy::Msaa { .. } => TextProvider::EditMessages,
    }
}

/// Whether `kept` is editable text with automatic selection detection,
/// whose value changes set the text-changed flag: every text control
/// Verbatim reads detects selection changes automatically, and a terminal
/// has no selection baseline (design, "The hook and Core's stroke line";
/// "Terminals").
pub(crate) fn detects_selection(kept: &Kept) -> bool {
    crate::editing::may_have_text(kept.role) && !crate::editing::is_grid(kept.role)
}

/// The committed focus's record, when its reader lives and its node is
/// `node`.
fn committed_on(state: &mut SrState, node: NodeId) -> Option<&mut Kept> {
    match &mut state.versions.committed {
        Committed::Read(kept) if kept.node == node => Some(kept),
        _ => None,
    }
}

/// Core's own move or selection on `node` minted `baseline` (Table 17,
/// column F): when `node` is the committed focus, it becomes the
/// baseline, nothing said, the flag kept. Verbatim-only, today's
/// `keep_made`: Verbatim's own moves are not spoken as selection changes.
pub(crate) fn minted(state: &mut SrState, node: NodeId, baseline: TextAnchor) {
    if let Some(kept) = committed_on(state, node) {
        kept.selection.baseline = Some(baseline);
    }
}

/// A value change of the committed focus, editable text with automatic
/// selection detection, reached its turn (column G): the flag is set, with
/// no request and nothing said. NVDA: `NVDAObjects/IAccessible/__init__.py`
/// 2017 to 2021.
pub(crate) fn value_changed(kept: &mut Kept) {
    kept.selection.text_changed = true;
}

/// The committed focus's outpost generation `outpost` ended (column H):
/// its baseline's anchor ended with it; the flag is the focus's and stays.
pub(crate) fn outpost_ended(kept: &mut Kept, outpost: OutpostId) {
    if kept.outpost == outpost {
        kept.selection.baseline = None;
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
/// text facts are `facts` (Table 17, columns A to C): a caret read mints
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

/// The selection baseline the committed focus's text facts hold, with
/// the node whose text it is in.
pub(crate) fn held(state: &SrState) -> Option<(NodeId, TextAnchor)> {
    let Committed::Read(kept) = &state.versions.committed else {
        return None;
    };
    kept.selection
        .baseline
        .map(|baseline| (kept.node, baseline))
}

/// The node `request` is about and every text anchor it names, which stay
/// held while it waits or is out (design, "Held anchors").
pub(crate) fn named_by(request: &Wire) -> Option<(NodeId, Vec<TextAnchor>)> {
    let Wire::Text { node, op } = request else {
        return None;
    };
    let point = |point: &TextPoint| match point {
        TextPoint::At(position) => Some(position.anchor),
        TextPoint::Caret
        | TextPoint::SelectionStart
        | TextPoint::SelectionEnd
        | TextPoint::Start
        | TextPoint::End => None,
    };
    let through = |through: &Option<TextThrough>| {
        through
            .iter()
            .flat_map(|through| [point(&through.start), point(&through.end)])
            .flatten()
            .collect::<Vec<_>>()
    };
    let anchors = match op {
        TextOp::Watch(watch) => vec![watch.point],
        TextOp::MoveBySentence(moving) => vec![moving.point],
        TextOp::ReadSelection(read) => read.baseline.into_iter().collect(),
        TextOp::Read(read) => point(&read.at).into_iter().collect(),
        TextOp::ReadAhead(read) => point(&read.at).into_iter().collect(),
        TextOp::ReadRange {
            start,
            end,
            through: past,
        }
        | TextOp::Select {
            start,
            end,
            through: past,
        } => [point(start), point(end)]
            .into_iter()
            .flatten()
            .chain(through(past))
            .collect(),
        TextOp::MoveCaret(at) | TextOp::Location(at) => point(at).into_iter().collect(),
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
