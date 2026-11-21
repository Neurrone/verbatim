//! The request list's two text operations that read a held selection,
//! `Caret` and `CompareSelection` (the request list's design, section 3),
//! on any [`TextSource`] read call by call (an edit control's messages; UI
//! Automation's text runs through the program builder, `super::fused`):
//! the caret and the selection, held as anchors, the read held as the next
//! selection baseline, and a
//! comparison of two held reads' selections by the contract's rule
//! ([`super::selection_changes`]).

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use verbatim_model::batch::{CaretAt, Edges, HeldPoint, HeldSelection, SelectionCompared};
use verbatim_model::{CaretSelection, Selection, TextAnchor, TextPosition};

use super::{
    CaretState, MAX_RANGE_UNITS, NodeText, TextResult, TextSource, change, change_of,
    selection_changes,
};

/// The caret and the selection, each end held as an anchor with whether it
/// is at the text's start or end, and the read held as the next selection
/// baseline, compared with nothing. It reads no line and no selected text,
/// which a batch reads by steps of its own.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read_at_us: u64,
) -> TextResult<CaretAt> {
    let state = source.caret()?;
    let (start, end) = (source.start()?, source.end()?);
    let mut held = |anchors: &mut NodeText<'_, S::Pos>, at: &S::Pos| -> TextResult<HeldPoint> {
        Ok(HeldPoint {
            anchor: anchors.hold_point(at.clone()),
            edges: Edges {
                start: source.compare(at, &start)? != Ordering::Greater,
                end: source.compare(at, &end)? != Ordering::Less,
            },
        })
    };
    let caret = held(anchors, &state.caret)?;
    let selection = match &state.selection {
        Some((start, end)) => Some(HeldSelection {
            start: held(anchors, start)?,
            end: held(anchors, end)?,
        }),
        None => None,
    };
    let kept = state.selection.clone().zip(selection).map(|(ends, held)| {
        let report = CaretSelection {
            range: Selection {
                start: TextPosition::at(held.start.anchor),
                end: TextPosition::at(held.end.anchor),
            },
        };
        (ends, report, None)
    });
    let baseline = anchors.hold_caret(state.caret.clone(), kept, read_at_us);
    Ok(CaretAt {
        caret,
        selection,
        held: baseline,
        offsets: source.edit_selection(&state),
    })
}

/// How the selection held under `after`, a `Caret`'s read, differs from the
/// one held under `before`, a baseline or another `Caret`'s read: what
/// became selected and unselected, the whole new selection, and whether
/// the text between the old selection's ends changed since it was read.
/// `Ok(Err(anchor))` names a held read no longer held.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn compare_selection<S: TextSource>(
    source: &mut S,
    anchors: &NodeText<'_, S::Pos>,
    (before, after): (TextAnchor, TextAnchor),
) -> TextResult<Result<SelectionCompared, TextAnchor>> {
    let Some(old) = anchors.held_read(before) else {
        return Ok(Err(before));
    };
    let Some(new) = anchors.held_read(after) else {
        return Ok(Err(after));
    };
    let kept = old.kept_selection();
    let state = CaretState {
        caret: new.caret,
        selection: new
            .selection
            .map(|selection| (selection.start, selection.end)),
    };
    let changes = if kept.collapsed && state.selection.is_none() {
        Vec::new()
    } else {
        selection_changes(source, (&kept.ends, kept.collapsed), &state)?
    };
    let new_selection = match &state.selection {
        Some((start, end)) => Some(change(source, true, start, end)?),
        None => None,
    }
    .filter(|selected| !selected.text.is_empty());
    let text_changed = match (&kept.text, kept.collapsed) {
        (Some(text), false) => {
            let (now, _) = source.text(&kept.ends.0, &kept.ends.1, MAX_RANGE_UNITS)?;
            change_of(false, &now).text != change_of(false, text).text
        }
        _ => false,
    };
    Ok(Ok(SelectionCompared {
        difference: verbatim_model::SelectionDifference {
            changes,
            new_selection,
        },
        text_changed,
    }))
}
