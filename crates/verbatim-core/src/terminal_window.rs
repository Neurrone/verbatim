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
//! - when Core asks, the colours of the screen's rows, as many and as
//!   wide as the screen and its top line read by this batch.
//!
//! The search and the colours come last, each only when asked, so the
//! outcomes say which ran by their kinds.

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{Done, FoundText, StepOutcome};
use verbatim_model::{
    CaretReport, NodeId, TerminalColourRun, TerminalText, TextAnchor, TextDetails, TextMovement,
    TextPoint, TextPosition, TextUnit,
};

use crate::batch::{Answers, Builder, Got, Ref, Search, kind};

/// More rows than any terminal holds, for moving to the end of the text.
const FAR: i32 = 1_000_000;

/// How many `FindText` calls the search for a mark's line makes at most,
/// as the outpost's search always made.
const MARK_SEARCH_CALLS: u8 = 100;

/// An end mark Core names: a held range at an earlier read's document
/// end, with the text of the last line with text at or above it and that
/// line's distance in rows above the mark, by which a read finds the line
/// again once the history rotated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndMark {
    /// The held range at the earlier read's document end.
    pub anchor: TextAnchor,
    /// The last line with text at or above the mark, without its padding.
    pub line: String,
    /// How many rows that line lies above the mark.
    pub line_above: u32,
}

/// The current end mark, and whether the read searches for its line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentMark {
    /// The mark.
    pub mark: EndMark,
    /// Whether the read searches for the mark's line when the window it
    /// read lacks it ([`MarkSearch`]): Core asks only on a read its pacing
    /// held once an earlier read showed the history rotated.
    pub find: bool,
}

/// What Core names to a window read ([`add_steps`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowAsk {
    /// The current end mark, none for a first read.
    pub end: Option<CurrentMark>,
    /// The end mark kept aside while a full-screen program's screen is up.
    pub aside: Option<EndMark>,
    /// The screen's height in rows as the last read found it, 0 at first:
    /// the window starts twice that many rows above the document's end,
    /// or at the screen's top when it is 0.
    pub height: u32,
    /// "Lines spoken in full": past a mark with more rows written after it
    /// than this and the window together, the read gives this many rows
    /// after the mark ([`MarkPlace::Rows::opening`]).
    pub opening: u32,
    /// Whether to read the colours of every screen row, as Core does for
    /// each read in a key's window.
    pub colours: bool,
}

/// What a mark showed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkPlace {
    /// The mark lies past the document's end or the provider would not
    /// compare it: the text shrank under a clear, or the mark is on the
    /// other screen.
    Refused,
    /// The rows written past the mark.
    Rows {
        /// How many rows lie from the mark to the document's end.
        written: u32,
        /// The first "Lines spoken in full" rows after the mark, when more
        /// rows follow it than those and the window together.
        opening: Option<TerminalText>,
    },
}

/// The search for the current mark's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkSearch {
    /// Core did not ask for it.
    NotAsked,
    /// The window read holds the line, so nothing was searched.
    InWindow,
    /// The nearest row starting with the line's text, this many rows above
    /// the document's end.
    Found(u32),
    /// No row starting with it was found.
    Missed,
}

/// The caret a window read found, as a caret report, with the point held
/// at it: a caret key's starting point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCaret {
    /// The caret and its line.
    pub report: CaretReport,
    /// The point held at the caret.
    pub point: TextAnchor,
}

/// The window Core makes of a window read's outcomes ([`window_of`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWindow {
    /// The rows from two screens above the document's end to its end, read
    /// in one call, without their padding.
    pub rows: TerminalText,
    /// How many rows the window starts above the document's end.
    pub above: u32,
    /// How many rows the screen's top lies above the document's end; the
    /// screen's last rows may lie past the end, blank.
    pub screen_top: u32,
    /// The screen's height in rows.
    pub screen_rows: u32,
    /// The screen's width in cells: a line longer than it wraps over
    /// several rows, which `above`, `screen_top` and the marks count.
    pub width: u32,
    /// Whether the screen starts at the document's start.
    pub screen_at_start: bool,
    /// The caret, when the provider reports one.
    pub caret: Option<WindowCaret>,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end, when the provider reports a caret: a blank caret
    /// row is told from the blank rows around it only by its place.
    pub caret_rows_to_end: Option<u32>,
    /// The new end mark, held at the document's end.
    pub end: TextAnchor,
    /// The document's first row, without its padding.
    pub first_row: TerminalText,
    /// How many rows lie from the document's end to its start.
    pub document_rows: u32,
    /// What the current mark showed, when Core named one.
    pub end_place: Option<MarkPlace>,
    /// What the aside mark showed, when Core named one.
    pub aside_place: Option<MarkPlace>,
    /// The search for the current mark's line.
    pub found: MarkSearch,
    /// When Core asked for colours, each screen row's runs from the
    /// screen's top, a wrapped line's rows each its own; the rows stop at
    /// the document's end.
    pub colours: Option<Vec<Vec<TerminalColourRun>>>,
    /// When the read began, in microseconds on the run's shared clock.
    pub began_us: u64,
    /// When the read's calls ended, before its text was converted.
    pub ended_us: u64,
}

/// Why a window read made no window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// A step failed, or answered a result of another kind.
    Failed,
    /// The terminal, or an anchor Core named, is not there.
    NotThere,
}

/// Adds the window read of terminal `node` that `ask` names to `batch`,
/// as the module's documentation lists its steps.
pub fn add_steps<B>(batch: &mut Builder<B>, node: NodeId, ask: &WindowAsk) {
    let line = |count: i32| TextMovement {
        unit: TextUnit::Line,
        count,
    };
    let visible = batch.visible(node);
    let top = batch.read_trimmed(node, visible.visible_start(), TextUnit::Line, (1, false));
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
    if ask.colours {
        let _colours = batch.read_colours(node, visible, top);
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

/// How many rows the screen's trimmed text fills at `width` cells a row
/// ([`verbatim_model::screen_rows`]), from its lines' padded lengths.
fn screen_rows(text: &str, padded: &[(u32, u32)], width: u32) -> u32 {
    let lines = padded.iter().flat_map(|&(count, length)| {
        std::iter::repeat_n(length, usize::try_from(count).unwrap_or(0))
    });
    verbatim_model::screen_rows(lines, text.ends_with("\r\n"), width)
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
                    ..
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

/// How each way of a command mark's search bounds it: the matches that
/// start a row, each read for one row, at most 20 over at most 100
/// `FindText` calls, the bound the console host's search needed
/// (`books/dev/src/reference/performance.md`, "A terminal's upward
/// search").
const WAY: Search = Search {
    unit_start: Some(TextUnit::Line),
    read: 1,
    limit: 20,
    calls: 100,
};

/// A command mark's search's steps.
pub struct SearchSteps<B> {
    down: Ref<B, kind::FindText>,
    up: Ref<B, kind::FindText>,
    to_end: Ref<B, kind::Move>,
}

/// A match of a command mark's search, counted in rows above the end mark
/// it was counted from (negative below it), with its row's text as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Rows above the mark.
    pub above: i32,
    /// The row's text, padding and line break as the provider gave them.
    pub text: String,
}

/// Adds to `batch` the search for `needle` in terminal `node` outward from
/// the row `from` rows above `start`, its end mark: a move to that row, a
/// search down from it and one up, each to its own bound, and a move from
/// the mark to the text's end, which the matches are counted against.
/// `None` when `from` cannot be negated.
pub fn add_search<B>(
    batch: &mut Builder<B>,
    node: NodeId,
    (start, from): (verbatim_model::TextAnchor, i32),
    needle: &str,
) -> Option<SearchSteps<B>> {
    let mark = || TextPoint::At(TextPosition::at(start)).into();
    let line = |count| TextMovement {
        unit: TextUnit::Line,
        count,
    };
    let row = batch.move_point(node, mark(), line(from.checked_neg()?));
    let down = batch.find_text(node, row.landed(), (needle.to_owned(), false), WAY);
    let up = batch.find_text(node, row.landed(), (needle.to_owned(), true), WAY);
    let to_end = batch.move_point(node, mark(), line(FAR));
    Some(SearchSteps { down, up, to_end })
}

/// The matches a command mark's search answered, below its row first and
/// then above, each way nearest first; `None` when a step did not run.
#[must_use]
pub fn matches_of<B>(answers: &Answers<B>, steps: &SearchSteps<B>) -> Option<Vec<Match>> {
    let (Got::Done(down), Got::Done(up), Got::Done(to_end)) = (
        answers.get(steps.down),
        answers.get(steps.up),
        answers.get(steps.to_end),
    ) else {
        return None;
    };
    let from_end = i64::from(to_end.moved.unsigned_abs());
    Some(
        down.iter()
            .chain(up)
            .filter_map(|found| {
                Some(Match {
                    above: i32::try_from(i64::from(found.to_end).checked_sub(from_end)?).ok()?,
                    text: found.text.clone(),
                })
            })
            .collect(),
    )
}

/// The outcomes a window read's steps, added for `ask`, answer to make
/// `window` ([`window_of`]): what a test or a measurement of Core's reading
/// answers Core's read with, in place of an outpost.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one outcome per step, in the steps' order"
)]
pub fn outcomes_making(window: &TerminalWindow, ask: &WindowAsk) -> Vec<StepOutcome> {
    use verbatim_model::batch::{CaretAt, Chunks, Moved, RangeText, StepRefusal, VisibleText};
    let held = TextAnchor(0);
    let moved = |moved: i32, from: TextAnchor| {
        StepOutcome::Done(Done::Move(Moved {
            moved,
            from,
            to: held,
        }))
    };
    let negative = |rows: u32| i32::try_from(rows).map_or(i32::MIN, i32::wrapping_neg);
    let chunk = |text: &str| verbatim_model::TextChunk {
        unit: TextUnit::Line,
        text: text.to_owned(),
        start: held,
        offset: 0,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    };
    let trimmed = |text: &TerminalText| {
        StepOutcome::Done(Done::ReadText(Chunks {
            moved: 0,
            chunks: vec![chunk(&text.text)],
            padded: Some(text.lengths.clone()),
        }))
    };
    let mut outcomes = vec![
        StepOutcome::Done(Done::Visible(VisibleText {
            start: held,
            end: held,
            at_text_start: window.screen_at_start,
            text: String::new(),
            padded: vec![(window.screen_rows, window.width)],
        })),
        trimmed(&TerminalText {
            text: String::new(),
            lengths: vec![(1, window.width)],
        }),
        moved(i32::try_from(window.screen_top).unwrap_or(i32::MAX), held),
    ];
    if ask.height > 0 {
        outcomes.push(moved(negative(window.above), held));
    }
    outcomes.extend([
        StepOutcome::Done(Done::ReadRange(RangeText {
            text: window.rows.text.clone(),
            truncated: false,
            padded: Some(window.rows.lengths.clone()),
        })),
        moved(negative(window.document_rows), window.end),
        trimmed(&window.first_row),
    ]);
    let caret = window.caret.as_ref();
    outcomes.extend([
        StepOutcome::Done(Done::Caret(CaretAt {
            caret: held,
            selection: None,
            held: caret.map_or(held, |caret| caret.point),
            offsets: None,
        })),
        StepOutcome::Done(Done::ReadText(Chunks {
            moved: 0,
            chunks: vec![caret.map_or_else(|| chunk(""), |caret| caret.report.line.clone())],
            padded: None,
        })),
        moved(
            i32::try_from(window.caret_rows_to_end.unwrap_or(0)).unwrap_or(i32::MAX),
            held,
        ),
    ]);
    for (named, place) in [
        (ask.end.is_some(), &window.end_place),
        (ask.aside.is_some(), &window.aside_place),
    ] {
        if !named {
            continue;
        }
        match place {
            Some(MarkPlace::Rows { written, opening }) => outcomes.extend([
                moved(i32::try_from(*written).unwrap_or(i32::MAX), held),
                trimmed(&opening.clone().unwrap_or_default()),
            ]),
            Some(MarkPlace::Refused) | None => outcomes.extend([
                StepOutcome::Refused(StepRefusal::OutsideText),
                StepOutcome::Skipped,
            ]),
        }
    }
    match window.found {
        MarkSearch::NotAsked => {}
        MarkSearch::InWindow | MarkSearch::Missed => {
            outcomes.push(StepOutcome::Done(Done::FindText(Vec::new())));
        }
        MarkSearch::Found(to_end) => {
            outcomes.push(StepOutcome::Done(Done::FindText(vec![FoundText {
                to_end,
                text: String::new(),
            }])));
        }
    }
    if let Some(rows) = &window.colours {
        outcomes.push(StepOutcome::Done(Done::ReadColours(rows.clone())));
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen's width is its top line's padded length, its line break
    /// left out, and the screen fills each line's rows at that width, at
    /// least one, the empty line after a last line break no line.
    #[test]
    fn the_screen_fills_each_lines_rows_at_its_top_lines_width() {
        let width = |text: &str| {
            let read = TerminalText::unpadded(text);
            read.lengths.first().map_or(0, |&(_, length)| length)
        };
        assert_eq!(width("abcd\r\n"), 4);
        assert_eq!(width("abcd"), 4);
        let rows = |text: &str, width| {
            let read = TerminalText::unpadded(text);
            screen_rows(&read.text, &read.lengths, width)
        };
        assert_eq!(rows("abcd\r\nefghijkl\r\nij", 4), 4);
        assert_eq!(rows("abcdefgh", 4), 2);
        assert_eq!(rows("ab  \r\n", 4), 1);
        assert_eq!(rows("ab  \r\n    ", 4), 2);
        assert_eq!(rows("abcd", 0), 0);
    }
}
