//! The text reads of Core's requests that hold a read behind an id
//! (`docs/design/readers-tell-core-asks-build1.md`, "A caret key at its
//! turn", "Selection and the caret's text facts" and Table 16): a caret
//! key's starting read, which holds its starting point; a caret tell's
//! read and a focus's caret read, which compare the selection with the
//! baseline Core names and hold the next; and Verbatim's own caret moves
//! and selections, whose answers hold the next baseline.
//!
//! A held read is the node's read as it was made: the caret, the
//! selection's ends and the line. Through UI Automation its ends are
//! ranges, which follow the edits, so a comparison with a baseline is made
//! against the baseline's range as it now is, as NVDA's
//! `speakSelectionChange` compares its kept old range
//! (`speech/speech.py` 1340 to 1418); an edit control's ends are offsets.

#![forbid(unsafe_code)]

use verbatim_model::{
    CaretReport, SelectionChange, SelectionDifference, Started, TextAnchor, TextChunk, TextOp,
    TextReply, TextUnit,
};

use super::{
    CaretRead, CaretRequest, CaretState, NodeText, TextError, TextResult, TextSource, change,
    change_of, keep_unit, perform, report_for, selection_changes, unit_at_caret,
};

/// Reads the caret, the selection and the caret's line, and reports them,
/// replacing the node's last read: in one go where the source can
/// ([`TextSource::caret_read`], UI Automation's remote operations), which
/// also finds where a selection whose caret is its end starts, in the
/// caret's line or the line before, so Core knows what comes before the
/// start, as a Backspace over the selection says it; else by its parts,
/// where a selection's start lies on the caret's line only when it is the
/// caret. With `selection`, a read in one go also reads the selection's
/// text, and with `previous`, the old selection's ends and whether it was
/// collapsed, its changes from it.
fn read_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read_at_us: u64,
    (selection, previous): (bool, Option<OldSelection<'_, S::Pos>>),
) -> TextResult<CaretFound<S::Pos>> {
    let request = CaretRequest {
        since: None,
        previous: previous.map(|(ends, _)| ends),
        previous_collapsed: previous.is_some_and(|(_, collapsed)| collapsed),
        unit: None,
        formats: None,
        recheck: false,
        read_selection_text: selection,
        read_previous_text: false,
    };
    if let Some(read) = source.caret_read(&request)? {
        let (line, offset) = read.line;
        let report = report_for(
            source,
            anchors,
            (&read.state, read_at_us),
            (
                Some((line, offset, read.formats)),
                Some(read.selection_start),
            ),
            None,
        )?;
        return Ok(CaretFound {
            state: read.state,
            report,
            changes: read.changes,
            selection_text: read.selection_text,
        });
    }
    let state = source.caret()?;
    let report = report_for(source, anchors, (&state, read_at_us), (None, None), None)?;
    Ok(CaretFound {
        state,
        report,
        changes: None,
        selection_text: None,
    })
}

/// An old selection a read compares with: its ends, and whether it was
/// collapsed.
type OldSelection<'a, P> = (&'a (P, P), bool);

/// What [`read_caret`] found: the caret and the selection, the report, and
/// what a read in one go gave of the selection, which a read by parts
/// leaves to its caller.
struct CaretFound<P> {
    state: CaretState<P>,
    report: CaretReport,
    /// The changes from the old selection, as [`super::CaretRead::changes`].
    changes: Option<Vec<(bool, Vec<u16>)>>,
    /// The selection's text, as [`super::CaretRead::selection_text`].
    selection_text: Option<Vec<u16>>,
}

/// The most UTF-16 code units of a selection's text a read in one go
/// gives (`verbatim_uia_rops::MAX_COMPARED_TEXT`, the only source that
/// gives one): a text as long is cut, and is read whole by its range.
const SELECTION_TEXT_CAP: usize = 513;

/// What a selection read found ([`read_selection`]).
pub struct SelectionFound<P> {
    /// The caret as it is now.
    pub report: CaretReport,
    /// The caret and the selection as the source read them.
    pub state: CaretState<P>,
    /// The whole selection's text, `None` when nothing is selected.
    pub selected: Option<SelectionChange>,
    /// The next selection baseline, held from this read.
    pub baseline: TextAnchor,
    /// The difference from the named baseline; `None` when none was named,
    /// or the outpost no longer holds it ("no baseline").
    pub difference: Option<SelectionDifference>,
}

impl<P: Clone> NodeText<'_, P> {
    /// Holds the node's last read, just made, under a new id, recorded as
    /// touched for the message that carries it.
    ///
    /// # Errors
    ///
    /// [`TextError::Failed`] when there is no last read, which only a read
    /// that made none could leave.
    fn hold_last(&mut self) -> TextResult<TextAnchor> {
        let read = self
            .anchors
            .last
            .clone()
            .ok_or_else(|| TextError::Failed("no read to hold".to_owned()))?;
        let number = self.next_number();
        self.anchors.held.insert(number, read);
        self.touch(number);
        Ok(TextAnchor(number))
    }

    /// The read held under `baseline`, its positions as the source's.
    pub(super) fn held_read(&self, baseline: TextAnchor) -> Option<super::LastRead<P>> {
        self.anchors
            .held
            .get(&baseline.0)
            .cloned()
            .and_then(|read| read.filter_map(|slot| Some(slot.at)))
    }
}

/// Reads the caret and the selection, compares the selection with the
/// read held under `baseline` when it is held, and holds this read as the
/// next baseline. The difference is given in both terms Core may speak it
/// in: what became selected and unselected, each part's text what its
/// range now holds, nothing when the old range and the new selection are
/// both empty; and the whole new selection.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn read_selection<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read_at_us: u64,
    baseline: Option<TextAnchor>,
) -> TextResult<SelectionFound<S::Pos>> {
    let old = baseline.and_then(|baseline| anchors.held_read(baseline));
    let kept = old.as_ref().map(super::LastRead::kept_selection);
    // The selection's text and its changes from the baseline are read with
    // the caret where the source reads in one go, as the caret report
    // reads them.
    let CaretFound {
        state,
        report,
        changes: read_changes,
        selection_text,
    } = read_caret(
        source,
        anchors,
        read_at_us,
        (true, kept.as_ref().map(|kept| (&kept.ends, kept.collapsed))),
    )?;
    // A selection whose text reads as empty holds nothing to say.
    let selected = match (&state.selection, selection_text) {
        (Some(_), Some(units)) if units.len() < SELECTION_TEXT_CAP => {
            Some(change_of(true, &units))
        }
        (Some((start, end)), _) => Some(change(source, true, start, end)?),
        (None, _) => None,
    }
    .filter(|selected| !selected.text.is_empty());
    let difference = match kept {
        None => None,
        Some(kept) => {
            let changes = match read_changes {
                _ if kept.collapsed && state.selection.is_none() => Vec::new(),
                Some(read) => read
                    .into_iter()
                    .map(|(selected, text)| change_of(selected, &text))
                    .collect(),
                None => selection_changes(source, (&kept.ends, kept.collapsed), &state)?,
            };
            Some(SelectionDifference {
                changes,
                new_selection: selected.clone(),
            })
        }
    };
    let baseline = anchors.hold_last()?;
    Ok(SelectionFound {
        report,
        state,
        selected,
        baseline,
        difference,
    })
}

/// A held caret key's starting read: the caret and the selection, held
/// as the starting point, with the key's `unit` at the caret (the line
/// where the text has no such unit) and, with `word`, the word at the
/// caret. It compares no selection and holds no baseline.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn starting_read<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read_at_us: u64,
    (unit, word): (TextUnit, bool),
) -> TextResult<Started> {
    let CaretFound { state, report, .. } = read_caret(source, anchors, read_at_us, (false, None))?;
    started_at(source, anchors, (state, report), (unit, word))
}

/// [`starting_read`] from the caret, selection and line `read` already
/// found, read at `read_at_us`: a terminal's full read reads them in its
/// own round trip, so a caret key's starting read mints its starting point
/// with no call of its own for the caret
/// (`docs/design/readers-tell-core-asks-build3.md`, "The full read request
/// and its starts").
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn starting_from<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: CaretRead<S::Pos>,
    read_at_us: u64,
    (unit, word): (TextUnit, bool),
) -> TextResult<Started> {
    let (line, offset) = read.line;
    let report = report_for(
        source,
        anchors,
        (&read.state, read_at_us),
        (
            Some((line, offset, read.formats)),
            Some(read.selection_start),
        ),
        None,
    )?;
    started_at(source, anchors, (read.state, report), (unit, word))
}

/// The starting point held from the caret `state` and its `report`, with
/// the key's `unit` and, with `word`, the word at the caret.
fn started_at<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (state, report): (CaretState<S::Pos>, CaretReport),
    (unit, word): (TextUnit, bool),
) -> TextResult<Started> {
    let mut at_caret = |anchors: &mut NodeText<'_, S::Pos>, unit| -> TextResult<TextChunk> {
        Ok(unit_at_caret(
            source,
            anchors,
            (&state, &report.line),
            unit,
            (None, Vec::new()),
        )?
        .unwrap_or_else(|| report.line.clone()))
    };
    let unit_read = at_caret(anchors, unit)?;
    let word = if word {
        Some(at_caret(anchors, TextUnit::Word)?)
    } else {
        None
    };
    // Delete's check compares the word at the caret with the word read
    // here; every other check compares the character, cut from the line.
    keep_unit(anchors, Some(word.as_ref().unwrap_or(&unit_read)));
    let point = anchors.hold_last()?;
    Ok(Started {
        point,
        caret: report,
        unit: unit_read,
        word,
    })
}

/// Carries out `op` for an event-queue entry: as [`perform`] does, but a
/// caret moved or a selection made by Verbatim is answered with the next
/// selection baseline held from it, so Core does not speak its own move as
/// a selection change. The baseline is the read the move kept; when it
/// kept none, the caret is read again.
pub fn perform_placed<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    op: &TextOp,
) -> TextReply {
    let kept_before = anchors.anchors.last.as_ref().map(|last| last.read_at_us);
    let reply = perform(source, anchors, op);
    let placed = match reply {
        TextReply::Moved { read_at_us } => {
            held_after_move(source, anchors, kept_before, read_at_us).map(|baseline| {
                TextReply::CaretPlaced {
                    read_at_us,
                    baseline,
                }
            })
        }
        TextReply::Selected(changes) => {
            let read_at_us = verbatim_process::clock::now_us();
            held_after_move(source, anchors, kept_before, read_at_us)
                .map(|baseline| TextReply::SelectionPlaced { changes, baseline })
        }
        other => Ok(other),
    };
    placed.unwrap_or_else(|error| match error {
        TextError::Gone => TextReply::Gone,
        TextError::Failed(reason) => {
            tracing::debug!(reason, "a move's baseline was not read");
            TextReply::Unanswered
        }
    })
}

/// The baseline held after a move: the read the move kept, when it kept
/// one (its time changed from `kept_before`), else a fresh read of the
/// caret begun at `read_at_us`.
pub(super) fn held_after_move<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    kept_before: Option<u64>,
    read_at_us: u64,
) -> TextResult<TextAnchor> {
    let kept_now = anchors.anchors.last.as_ref().map(|last| last.read_at_us);
    if kept_now.is_none() || kept_now == kept_before {
        let state = source.caret()?;
        report_for(source, anchors, (&state, read_at_us), (None, None), None)?;
    }
    anchors.hold_last()
}
