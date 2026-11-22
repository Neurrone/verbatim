//! A terminal's window read as steps of the request list (the
//! verbatim-core crate guide, "The terminal"): the outpost answers each
//! generic step, and Core makes the window its watch judges of their
//! outcomes ([`window_of`]), so every fact the read holds is read by an
//! operation of the list and every rule over them is Core's.
//!
//! The steps, in order ([`add_steps`]):
//!
//! - `Visible`: the screen's text, padding off, and whether it starts at
//!   the text's start;
//! - the line at the screen's top, trimmed: its padded length is the
//!   screen's width;
//! - a move from the screen's top to the text's end: how far above the end
//!   the screen's top lies;
//! - the window: from twice the screen's last height above the text's
//!   end, a move up from the end and the text from where it landed, or,
//!   before the height is known, the text from the screen's top, trimmed;
//! - a move from the text's end to its start: the document's rows, the
//!   point it started from held as the next end mark;
//! - the first line, trimmed;
//! - the caret, its line, and a move from it to the end: the caret's rows
//!   to the end;
//! - for each end mark Core names, a move from it to the end, the rows
//!   written past it, refused past the text's end, and, when it was not,
//!   "Lines spoken in full" lines read whole from it;
//! - when Core asks, the search for the current mark's line back from the
//!   text's end, by calls;
//! - when Core asks, the colours of the screen's rows.
//!
//! The search and the colours come last, each only when asked, so the
//! outcomes say which ran by their kinds.

use verbatim_model::batch::{Done, FoundText, StepOutcome};
use verbatim_model::{
    CurrentMark, EndMark, MarkPlace, MarkSearch, NodeId, TerminalColourRun, TerminalText,
    TerminalWindow, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit, WindowAsk,
    WindowCaret,
};

use crate::batch::{Builder, Search};

/// More rows than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// How many `FindText` calls the search for a mark's line makes at most,
/// as the outpost's search always made.
const MARK_SEARCH_CALLS: u8 = 100;

/// Why a window read made no window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// A step failed, or answered a result of another kind.
    Failed,
    /// The terminal, or an anchor Core named, is not there.
    NotThere,
}

/// Adds the window read of terminal `node` that `ask` names to `batch`,
/// with the colours of `colours`' rows and width when given, as the
/// module's documentation lists its steps.
pub fn add_steps<B>(
    batch: &mut Builder<B>,
    node: NodeId,
    ask: &WindowAsk,
    colours: Option<(u32, u32)>,
) {
    let line = |count: i32| TextMovement {
        unit: TextUnit::Line,
        count,
    };
    let visible = batch.visible(node);
    let _top = batch.read_trimmed(node, visible.visible_start(), TextUnit::Line, (1, false));
    let _screen_top = batch.move_point(node, visible.visible_start(), line(FAR));
    if ask.height > 0 {
        let up = i32::try_from(ask.height)
            .ok()
            .and_then(|height| height.checked_mul(-2))
            .unwrap_or(-FAR);
        let window = batch.move_point(node, TextPoint::End.into(), line(up));
        let _rows = batch.read_range_trimmed(node, (window.landed(), TextPoint::End.into()));
    } else {
        let _rows =
            batch.read_range_trimmed(node, (visible.visible_start(), TextPoint::End.into()));
    }
    let _document = batch.move_point(node, TextPoint::End.into(), line(-FAR));
    let _first = batch.read_trimmed(node, TextPoint::Start.into(), TextUnit::Line, (1, false));
    let caret = batch.caret(node, None);
    let _line = batch.read_text(
        node,
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let _caret_rows = batch.move_point(node, caret.caret(), line(FAR));
    let opening = u16::try_from(ask.opening.max(1)).unwrap_or(u16::MAX);
    let marks = ask.end.as_ref().map(|end| end.mark.anchor);
    for mark in marks
        .into_iter()
        .chain(ask.aside.as_ref().map(|aside| aside.anchor))
    {
        let at = || TextPoint::At(TextPosition::at(mark)).into();
        let written = batch.move_point(node, at(), line(FAR));
        batch.when(written.found());
        let _opening = batch.read_trimmed(node, at(), TextUnit::Line, (opening, true));
    }
    if let Some(CurrentMark { mark, find: true }) = &ask.end {
        let _found = batch.find_text(
            node,
            TextPoint::End.into(),
            (mark.line.clone(), true),
            Search {
                unit_start: Some(TextUnit::Line),
                read: 1,
                limit: 1,
                calls: MARK_SEARCH_CALLS,
            },
        );
    }
    if let Some((rows, width)) = colours {
        let _colours = batch.read_colours(node, visible.visible_start(), (rows, width));
    }
}

/// The outcomes of a window read's steps, taken in order.
struct Outcomes<'a>(std::slice::Iter<'a, StepOutcome>);

impl<'a> Outcomes<'a> {
    /// The next step's result, or why the read made no window.
    fn done(&mut self) -> Result<&'a Done, Unread> {
        match self.0.next() {
            Some(StepOutcome::Done(done)) => Ok(done),
            Some(StepOutcome::NotThere(_)) => Err(Unread::NotThere),
            Some(StepOutcome::Refused(_) | StepOutcome::Failed | StepOutcome::Skipped) | None => {
                Err(Unread::Failed)
            }
        }
    }

    /// The next step's outcome.
    fn next(&mut self) -> Option<&'a StepOutcome> {
        self.0.next()
    }
}

/// How many rows a move went.
fn rows(moved: i32) -> u32 {
    moved.unsigned_abs()
}

/// A trimmed text and its padded lengths as a terminal's text.
fn terminal_text(text: &str, padded: Option<&Vec<(u32, u32)>>) -> TerminalText {
    TerminalText {
        text: text.to_owned(),
        lengths: padded.cloned().unwrap_or_default(),
    }
}

/// The one chunk of a trimmed read as a terminal's text.
fn read_text(done: &Done) -> Result<TerminalText, Unread> {
    match done {
        Done::ReadText(read) => read
            .chunks
            .first()
            .map(|chunk| terminal_text(&chunk.text, read.padded.as_ref()))
            .ok_or(Unread::Failed),
        _ => Err(Unread::Failed),
    }
}

/// How many rows the screen's text fills at `width` cells a row: each
/// line as many as its padded length takes, at least one, the empty line
/// after a last line break not a line.
fn screen_rows(text: &str, padded: &[(u32, u32)], width: u32) -> u32 {
    if width == 0 {
        return 0;
    }
    let mut lines: Vec<u32> = padded
        .iter()
        .flat_map(|&(count, length)| {
            std::iter::repeat_n(length, usize::try_from(count).unwrap_or(0))
        })
        .collect();
    if text.ends_with("\r\n") {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|length| length.div_ceil(width).max(1))
        .fold(0, u32::saturating_add)
}

/// The rows of `text` as the provider ends them, without the empty line
/// after a last line break.
fn lines_of(text: &str) -> Vec<&str> {
    text.strip_suffix("\r\n")
        .unwrap_or(text)
        .split("\r\n")
        .collect()
}

/// What a mark's move and its opening read showed, the window's rows
/// being `above` and "Lines spoken in full" `opening`.
fn mark_place(
    (moved, read): (Option<&StepOutcome>, Option<&StepOutcome>),
    (above, opening): (u32, u32),
) -> Result<MarkPlace, Unread> {
    let written = match moved {
        Some(StepOutcome::Done(Done::Move(moved))) => rows(moved.moved),
        Some(StepOutcome::Refused(_)) => return Ok(MarkPlace::Refused),
        Some(StepOutcome::NotThere(_)) => return Err(Unread::NotThere),
        _ => return Err(Unread::Failed),
    };
    let long = u64::from(written) > u64::from(opening).saturating_add(u64::from(above));
    let opening = match read {
        Some(StepOutcome::Done(done)) => long.then(|| read_text(done)).transpose()?,
        // A read that failed refuses the mark, as a mark whose rows could
        // not be read always was.
        Some(StepOutcome::Failed | StepOutcome::Refused(_)) => return Ok(MarkPlace::Refused),
        Some(StepOutcome::NotThere(_)) => return Err(Unread::NotThere),
        Some(StepOutcome::Skipped) | None => return Err(Unread::Failed),
    };
    Ok(MarkPlace::Rows { written, opening })
}

/// The search's answer for the current mark's line: in the window when its
/// rows hold it, else the nearest match's rows above the text's end.
fn searched(rows: &TerminalText, line: &str, found: &[FoundText]) -> MarkSearch {
    if lines_of(&rows.text).contains(&line) {
        return MarkSearch::InWindow;
    }
    found
        .first()
        .map_or(MarkSearch::Missed, |found| MarkSearch::Found(found.to_end))
}

/// The window a read's `outcomes` make, its steps as [`add_steps`] added
/// them for `ask`, answered at `at_us`.
///
/// # Errors
///
/// [`Unread`] when a step the window needs failed or was not there.
pub fn window_of(
    outcomes: &[StepOutcome],
    ask: &WindowAsk,
    at_us: u64,
) -> Result<TerminalWindow, Unread> {
    let mut steps = Outcomes(outcomes.iter());
    let Done::Visible(visible) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let width = read_text(steps.done()?)?
        .lengths
        .first()
        .map_or(0, |&(_, length)| length);
    let Done::Move(screen_top) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let screen_top = rows(screen_top.moved);
    let above = if ask.height > 0 {
        let Done::Move(window) = steps.done()? else {
            return Err(Unread::Failed);
        };
        rows(window.moved)
    } else {
        screen_top
    };
    let Done::ReadRange(window) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let rows_read = terminal_text(&window.text, window.padded.as_ref());
    let Done::Move(document) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let first_row = read_text(steps.done()?)?;
    let Done::Caret(caret) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let Done::ReadText(line) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let Done::Move(caret_rows) = steps.done()? else {
        return Err(Unread::Failed);
    };
    let line = line.chunks.first().cloned().ok_or(Unread::Failed)?;
    let mut mark = |named: bool| -> Result<Option<MarkPlace>, Unread> {
        if !named {
            return Ok(None);
        }
        let moved = steps.next();
        let read = steps.next();
        mark_place((moved, read), (above, ask.opening)).map(Some)
    };
    let end_place = mark(ask.end.is_some())?;
    let aside_place = mark(ask.aside.is_some())?;
    let (mut found, mut colours) = (MarkSearch::NotAsked, None);
    for outcome in steps.0 {
        match (outcome, &ask.end) {
            (
                StepOutcome::Done(Done::FindText(matches)),
                Some(CurrentMark {
                    mark: EndMark { line, .. },
                    find: true,
                }),
            ) => found = searched(&rows_read, line, matches),
            (StepOutcome::Done(Done::ReadColours(rows)), _) => colours = Some(rows.clone()),
            (StepOutcome::NotThere(_), _) => return Err(Unread::NotThere),
            _ => return Err(Unread::Failed),
        }
    }
    let colours: Option<Vec<Vec<TerminalColourRun>>> = colours;
    Ok(TerminalWindow {
        rows: rows_read,
        above,
        screen_top,
        screen_rows: screen_rows(&visible.text, &visible.padded, width),
        width,
        screen_at_start: visible.at_text_start,
        caret: Some(WindowCaret {
            report: crate::text_facts::caret_report(caret, line),
            point: caret.held,
        }),
        caret_rows_to_end: Some(rows(caret_rows.moved)),
        end: document.from,
        first_row,
        document_rows: rows(document.moved),
        end_place,
        aside_place,
        found,
        colours,
        began_us: at_us,
        ended_us: at_us,
    })
}
