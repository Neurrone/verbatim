//! The text reads of Core's requests that hold a read behind an id
//! (`docs/design/readers-tell-core-asks-build1.md`, "A caret key at its
//! turn", "Selection and the caret's text facts" and Table 16): a caret
//! key's starting read, which holds its starting point; a focus's caret
//! read, which holds the next selection baseline; and Verbatim's own caret
//! moves and selections, whose answers hold the next baseline.
//!
//! A held read is the node's read as it was made: the caret, the
//! selection's ends and the line. Through UI Automation its ends are
//! ranges, which follow the edits, so a comparison with a baseline is made
//! against the baseline's range as it now is, as NVDA's
//! `speakSelectionChange` compares its kept old range
//! (`speech/speech.py` 1340 to 1418); an edit control's ends are offsets.

#![forbid(unsafe_code)]

use std::sync::Arc;

use verbatim_model::{CaretReport, SelectionChange, TextAnchor};

use super::{
    CaretRead, CaretState, NodeText, TextError, TextResult, TextSource, change, change_of,
    report_for,
};

/// Reports the caret, the selection and the caret's line, replacing the
/// node's last read: from `one_go`, the read UI Automation's program made
/// of them in one go, which also found where the selection starts in the
/// caret's line, so Core knows what comes before the start, as a Backspace
/// over the selection says it, and read the selection's text and the
/// line's formatting; else read by the source's parts, where a selection's
/// start lies on the caret's line only when it is the caret.
fn read_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (one_go, read_at_us): (Option<CaretRead<S::Pos>>, u64),
) -> TextResult<CaretFound<S::Pos>> {
    if let Some(read) = one_go {
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
            selection_text: read.selection_text,
        });
    }
    let state = source.caret()?;
    let report = report_for(source, anchors, (&state, read_at_us), (None, None), None)?;
    Ok(CaretFound {
        state,
        report,
        selection_text: None,
    })
}

/// What [`read_caret`] found: the caret and the selection, the report, and
/// what a read in one go gave of the selection, which a read by parts
/// leaves to its caller.
struct CaretFound<P> {
    state: CaretState<P>,
    report: CaretReport,
    /// The selection's text, as [`super::CaretRead::selection_text`].
    selection_text: Option<Vec<u16>>,
}

/// A selection a batch's caret read holds: its two ends, how they were
/// reported, and its text when read.
pub type HeldEnds<P> = ((P, P), verbatim_model::CaretSelection, Option<Vec<u16>>);

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
    #[must_use]
    pub fn held_read(&self, baseline: TextAnchor) -> Option<super::LastRead<P>> {
        self.anchors
            .held
            .get(&baseline.0)
            .cloned()
            .and_then(|read| read.filter_map(|slot| Some(slot.at)))
    }

    /// Holds a batch's caret read, begun at `read_at_us`, as the node's
    /// last read and under a new id, the next selection baseline: the
    /// caret, and the selection's two ends with how they were reported and
    /// its text when a step of the batch read it. It holds no line, which a
    /// batch reads by a step of its own.
    pub fn hold_caret(
        &mut self,
        caret: P,
        selection: Option<HeldEnds<P>>,
        read_at_us: u64,
    ) -> TextAnchor {
        let read = super::LastRead {
            caret: super::Slot::new(caret),
            selection: selection.map(|((start, end), report, text)| super::ReadSelection {
                start: super::Slot::new(start),
                end: super::Slot::new(end),
                text: text.map(Arc::from),
                report,
            }),
            line: None,
            read_at_us,
            unit: None,
        };
        let number = self.next_number();
        self.anchors.held.insert(number, read.clone());
        self.anchors.last = Some(read);
        self.touch(number);
        TextAnchor(number)
    }
}

/// A focus's caret read: the caret, its line with its formatting, which
/// NVDA speaks with the focus (`speech/speech.py` 904 to 922), and the
/// selection with its text, held as the next selection baseline, compared
/// with nothing: a focus read becomes the baseline silently, as NVDA's
/// `initAutoSelectDetection` sets it (`editableText.py` 417 to 427).
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn read_focus<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: (Option<CaretRead<S::Pos>>, u64),
) -> TextResult<(CaretReport, Option<SelectionChange>, TextAnchor)> {
    let (report, selected) = caret_and_selected(source, anchors, read)?;
    let baseline = anchors.hold_last()?;
    Ok((report, selected, baseline))
}

/// What a node's text says of it, as NVDA's object speech reads it for
/// report focus: the selected text when the selection is not empty, else
/// the line at the caret. It holds no baseline.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn summary<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: (Option<CaretRead<S::Pos>>, u64),
) -> TextResult<verbatim_model::property::TextSummary> {
    let (report, selected) = caret_and_selected(source, anchors, read)?;
    Ok(match selected {
        Some(selected) => verbatim_model::property::TextSummary::Selected(selected.text),
        None => verbatim_model::property::TextSummary::Line(report.line.text),
    })
}

/// The caret's report with its line's formatting, and the selection as a
/// change from nothing when it holds text.
fn caret_and_selected<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: (Option<CaretRead<S::Pos>>, u64),
) -> TextResult<(CaretReport, Option<SelectionChange>)> {
    let CaretFound {
        state,
        report,
        selection_text,
        ..
    } = read_caret(source, anchors, read)?;
    // A selection whose text reads as empty holds nothing to say.
    let selected = match (&state.selection, selection_text) {
        (Some(_), Some(units)) => Some(change_of(true, &units)),
        (Some((start, end)), _) => Some(change(source, true, start, end)?),
        (None, _) => None,
    }
    .filter(|selected| !selected.text.is_empty());
    Ok((report, selected))
}

/// The baseline held after a move: the read the move kept, when it kept
/// one (its time changed from `kept_before`), else a fresh read of the
/// caret begun at `read_at_us`.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn held_after_move<S: TextSource>(
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
