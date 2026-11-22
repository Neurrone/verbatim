//! A focused terminal's reads (`books/dev/src/explanation/design/terminal-model.md`, "Where it
//! runs" and "The read"), public so mockapp's tests drive them as the
//! worker does.
//!
//! The reading rules run in Core, and Core asks every read
//! (`docs/design/readers-tell-core-asks-build3.md`). The outpost only
//! reads: each read is one snapshot of the terminal's text, from a screen
//! above the start Core names to the document's end, taken in one remote
//! program with the screen, the caret and its line, and, when Core asks,
//! the screen's colours ([`verbatim_uia_rops::terminal_snapshot`]). It is
//! sent to Core as it is, without its lines' padding ([`TerminalSnapshot`]).
//! The outpost keeps nothing of a terminal between reads but the start
//! anchors Core names, held as text anchors.

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};

use verbatim_model::{
    MarkPlace, MarkSearch, TerminalColourRun, TerminalScope, TerminalSnapshot, TerminalText,
};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, FindQuery, MarkRows, Path, ReadQuery,
    Snapshot, SnapshotQuery, row_width, screen_height, terminal_find, terminal_read,
    terminal_read_find, terminal_row_colours, terminal_snapshot,
};

use crate::text::TextError;
pub use crate::text::trim::unpadded_utf16;

/// The wire form of a snapshot taken at `started_ms`: its texts as UTF-8
/// without their lines' padding, and the caret's row and its column in it in
/// characters, from the caret read with it. `waited` says whether the read
/// waited for Core's speech queue ("Volatile rows").
#[must_use]
pub fn snapshot_of(snapshot: &Snapshot, started_ms: u64, waited: bool) -> TerminalSnapshot {
    let (caret_line, caret_column) = snapshot.caret.as_ref().map_or_else(
        || (String::new(), 0),
        |caret| caret_row(&caret.line.text, caret.line.offset),
    );
    let cells = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    TerminalSnapshot {
        text: unpadded_utf16(&snapshot.text),
        screen: unpadded_utf16(&snapshot.screen),
        from_start: snapshot.from_start,
        shrank: snapshot.shrank,
        at_document_start: snapshot.at_document_start,
        alternate: snapshot.alternate,
        first_row: TerminalText::unpadded(&snapshot.first_row),
        caret_rows_to_end: snapshot.caret_rows_to_end,
        caret_line: TerminalText::unpadded(&caret_line),
        caret_column: cells(caret_column),
        above: snapshot.above,
        started_ms,
        ended_ms: started_ms,
        waited,
        colours: snapshot.colours.as_ref().map(|rows| {
            rows.iter()
                .map(|runs| {
                    runs.iter()
                        .map(|run| TerminalColourRun {
                            column: cells(run.column),
                            length: cells(run.length),
                            foreground: run.foreground,
                            background: run.background,
                        })
                        .collect()
                })
                .collect()
        }),
        caret: None,
    }
}

/// The caret's row as UTF-8, and the caret's column in it in characters,
/// from the row's UTF-16 text and the code units before the caret.
fn caret_row(row: &[u16], offset: usize) -> (String, usize) {
    let before = String::from_utf16_lossy(&row[..offset.min(row.len())]);
    (String::from_utf16_lossy(row), before.chars().count())
}

/// Whether a failed remote operation or classic read means the terminal
/// is gone.
fn is_gone(error: &RopsError) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}

/// What [`read`] found.
pub struct ReadAnswer {
    /// The read, as Core takes it, without its caret report.
    pub snapshot: TerminalSnapshot,
    /// The caret the read found.
    pub caret: Option<CaretAnswer>,
    /// Where a later read may start: the top of this read's screen, which
    /// the caller holds as the read's start anchor.
    pub next_start: Option<IUIAutomationTextRange>,
    /// The path the snapshot took, for the caller to log a fallback.
    pub path: Path,
    /// When the read began, in microseconds on the shared clock, from the
    /// clock read the snapshot's `started_ms` was taken from.
    pub started_us: u64,
}

/// What a full read asks, every fact of it Core's
/// ([`verbatim_model::TerminalRead`]): the range of the start anchor Core
/// named, `None` for a first read; which read; the screen's height and
/// width from Core's model, 0 when it has none; and whether to take the
/// colours.
pub struct ReadAsk<'a> {
    /// The start Core named, a screen above which the read begins.
    pub start: Option<&'a IUIAutomationTextRange>,
    /// Which read.
    pub scope: TerminalScope,
    /// The screen's height in rows.
    pub height: u32,
    /// The screen's width in cells.
    pub width: u32,
    /// Whether the read takes the screen's colours.
    pub colours: bool,
}

/// Reads a terminal through UIA, with the caret and its line read in the
/// same round trip (`caret`), as Core asks (`ask`): [`TerminalScope::Fresh`]
/// from a screen above the start Core named, [`TerminalScope::Whole`] the
/// whole document, and [`TerminalScope::HistoryBlock`] counted up from the
/// document's end by the screen's height with as many rows above. `remote`
/// tries the remote program first.
///
/// # Errors
///
/// [`TextError::Gone`] when the terminal is gone, [`TextError::Failed`]
/// when it could not be read.
pub fn read<'a>(
    uia: &'a Uia,
    (element, pattern): (&'a IUIAutomationElement, &'a IUIAutomationTextPattern),
    caret: CaretLineQuery<'a>,
    ask: &ReadAsk<'a>,
    remote: bool,
) -> Result<ReadAnswer, TextError> {
    let clock_us = verbatim_process::clock::now_us();
    let started_ms = clock_us / 1_000;
    let colours = ask.colours.then_some(ask.width as usize);
    let query = match ask.scope {
        TerminalScope::Whole => SnapshotQuery {
            element,
            pattern,
            start: None,
            screen_rows: ask.height,
            whole: true,
            above: 0,
            caret: Some(caret),
            colours,
        },
        TerminalScope::HistoryBlock { height } => SnapshotQuery {
            element,
            pattern,
            start: None,
            screen_rows: height,
            whole: false,
            above: height,
            caret: Some(caret),
            colours,
        },
        TerminalScope::Fresh => SnapshotQuery {
            element,
            pattern,
            start: ask.start,
            screen_rows: ask.height,
            whole: false,
            above: ask.height,
            caret: Some(caret),
            colours,
        },
    };
    let timer = std::time::Instant::now();
    let calls_before = verbatim_win32::calls::peek().uia;
    let result = terminal_snapshot(uia, &query, remote);
    let elapsed_us = u64::try_from(timer.elapsed().as_micros()).unwrap_or(u64::MAX);
    let calls = verbatim_win32::calls::peek()
        .uia
        .saturating_sub(calls_before);
    let (mut snapshot, path) = match result {
        Ok(answer) => answer,
        Err(error) => {
            tracing::debug!(elapsed_us, calls, %error, "terminal snapshot failed");
            return Err(gone_or_failed(&error));
        }
    };
    // What each read costs (`books/dev/src/reference/performance.md`, "A terminal
    // flood").
    tracing::debug!(
        path = path.name(),
        elapsed_us,
        calls,
        whole = query.whole,
        above = query.above,
        text_units = snapshot.text.len(),
        from_start = snapshot.from_start,
        shrank = snapshot.shrank,
        alternate = snapshot.alternate,
        colours = snapshot.colours.is_some(),
        "terminal snapshot timing"
    );
    let mut wire = snapshot_of(&snapshot, started_ms, false);
    wire.ended_ms = (verbatim_process::clock::now_us() / 1_000).max(started_ms);
    Ok(ReadAnswer {
        snapshot: wire,
        caret: snapshot.caret.take(),
        next_start: snapshot.next_start.take(),
        path,
        started_us: clock_us,
    })
}

/// The error a failed remote operation or classic read is.
fn gone_or_failed(error: &RopsError) -> TextError {
    if is_gone(error) {
        TextError::Gone
    } else {
        TextError::Failed(error.to_string())
    }
}

/// What [`history`] read: the text, padding kept, how many rows it holds
/// above the start it was counted from, and which path answered.
pub struct History {
    /// The text.
    pub text: String,
    /// Rows above the start.
    pub above: u32,
    /// Which implementation answered.
    pub path: Path,
}

/// Reads the history of a terminal by place, for a review command above
/// the rows Core's memory keeps ([`verbatim_model::TextOp::TerminalHistory`]):
/// from `above` rows above `start`, the start anchor Core named, to the
/// document's end, or the whole document when `whole`, in one remote
/// operation, the screen `height` rows high.
///
/// # Errors
///
/// [`TextError::Failed`] when it could not be read.
pub fn history(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    start: &IUIAutomationTextRange,
    (above, whole, height, remote): (u32, bool, u32, bool),
) -> Result<History, TextError> {
    let query = SnapshotQuery {
        element,
        pattern,
        start: if whole { None } else { Some(start) },
        screen_rows: height,
        whole,
        above: if whole { 0 } else { above },
        caret: None,
        colours: None,
    };
    let (snapshot, path) =
        terminal_snapshot(uia, &query, remote).map_err(|error| gone_or_failed(&error))?;
    Ok(History {
        text: String::from_utf16_lossy(&snapshot.text),
        above: snapshot.above,
        path,
    })
}

/// What [`find`] found: the matches, and which path answered.
pub struct Found {
    /// The matches.
    pub matches: Vec<verbatim_model::TerminalMatch>,
    /// Which implementation answered.
    pub path: Path,
}

/// Searches a terminal's text for a command mark's line, as Core asks
/// ([`verbatim_model::TextOp::TerminalFind`]): `needle` outward from the
/// row `from` rows above `start`, the start anchor Core named, reading
/// `rows` rows from each match, in one remote operation. Core decides
/// which match, if any, is the mark's line.
///
/// # Errors
///
/// [`TextError::Failed`] when a call before the search failed.
pub fn find(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    start: &IUIAutomationTextRange,
    (needle, from, rows): (&str, i32, u32),
    remote: bool,
) -> Result<Found, TextError> {
    let query = FindQuery {
        element,
        pattern,
        start,
        needle,
        from,
        rows,
    };
    let (found, path) =
        terminal_find(uia, &query, remote).map_err(|error| gone_or_failed(&error))?;
    Ok(Found {
        matches: found
            .into_iter()
            .map(|found| verbatim_model::TerminalMatch {
                above: found.above,
                text: found.text,
            })
            .collect(),
        path,
    })
}

/// What Core names to a window read, its marks as the ranges the outpost
/// holds for them ([`verbatim_model::WindowAsk`]).
pub struct WindowRead<'a> {
    /// The current end mark's range, its line's text, and whether to
    /// search for that line when the window lacks it.
    pub end: Option<(&'a IUIAutomationTextRange, &'a str, bool)>,
    /// The aside end mark's range.
    pub aside: Option<&'a IUIAutomationTextRange>,
    /// The screen's height as the last read found it, 0 at first.
    pub height: u32,
    /// "Lines spoken in full".
    pub opening: u32,
    /// Whether to read the colours of every screen row.
    pub colours: bool,
}

/// What [`window`] read: everything of the operation's answer but its
/// anchors, which the caller holds, with the ranges it holds them at.
pub struct WindowAnswer {
    /// The rows from two screens above the document's end to its end.
    pub rows: TerminalText,
    /// How many rows the window starts above the document's end.
    pub above: u32,
    /// How many rows the screen's top lies above the document's end.
    pub screen_top: u32,
    /// The screen's height in rows.
    pub screen_rows: u32,
    /// The screen's width in cells.
    pub width: u32,
    /// Whether the screen starts at the document's start.
    pub screen_at_start: bool,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end, when the provider reports a caret.
    pub caret_rows_to_end: Option<u32>,
    /// The document's first row.
    pub first_row: TerminalText,
    /// How many rows lie from the document's end to its start.
    pub document_rows: u32,
    /// What the current mark showed.
    pub end_place: Option<MarkPlace>,
    /// What the aside mark showed.
    pub aside_place: Option<MarkPlace>,
    /// The search for the current mark's line.
    pub found: MarkSearch,
    /// Every screen row's colours, when asked, from the screen's top.
    pub colours: Option<Vec<Vec<TerminalColourRun>>>,
    /// When the read began, in microseconds on the shared clock.
    pub began_us: u64,
    /// When its calls ended, before the text was converted.
    pub ended_us: u64,
    /// The document's end, to hold as the next end mark.
    pub end: IUIAutomationTextRange,
    /// The caret the read found.
    pub caret: Option<CaretAnswer>,
    /// Whether the program could not be imported, so the window is to be
    /// read classically from now on.
    pub import_failed: bool,
}

/// The rows of `text` as the provider ends them, without the empty line
/// after a last line break.
fn lines_of(text: &str) -> Vec<&str> {
    text.strip_suffix("\r\n")
        .unwrap_or(text)
        .split("\r\n")
        .collect()
}

/// A mark's place as Core takes it.
fn place_of(rows: MarkRows) -> MarkPlace {
    match rows {
        MarkRows::Refused => MarkPlace::Refused,
        MarkRows::Rows { written, opening } => MarkPlace::Rows {
            written,
            opening: opening.map(|text| unpadded_utf16(&text)),
        },
    }
}

/// The colour runs as Core takes them.
fn runs_of(runs: Vec<verbatim_uia_rops::ColourRun>) -> Vec<TerminalColourRun> {
    let cells = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    runs.into_iter()
        .map(|run| TerminalColourRun {
            column: cells(run.column),
            length: cells(run.length),
            foreground: run.foreground,
            background: run.background,
        })
        .collect()
}

/// The colours of the screen's rows, `height` of them from `top`, each
/// `width` cells: a wrapped line's rows each its own, the rows stopping at
/// the document's end.
fn screen_colours(
    top: &IUIAutomationTextRange,
    (height, width): (u32, usize),
    (remote, window_handle): (bool, Option<isize>),
) -> Result<Vec<Vec<TerminalColourRun>>, TextError> {
    let (rows, path) = terminal_row_colours(top, (height, width), remote)
        .map_err(|error| gone_or_failed(&error))?;
    if let Path::Fallback(error) = &path {
        verbatim_uia_rops::batch::remote_fell_back("terminal colours", window_handle, error);
    }
    Ok(rows.into_iter().map(runs_of).collect())
}

/// A terminal's window read ([`verbatim_model::batch::Op::TerminalRead`]):
/// one program, or its classic calls when `remote` is false or the
/// program fails, the fallback logged for `window_handle`; then, when
/// Core asked, the search for the current mark's line, by classic calls
/// alone and only when the window lacks the line, and, when Core asked,
/// the colours of every screen row. Every text is
/// converted once, from UTF-16 ([`unpadded_utf16`]).
///
/// # Errors
///
/// [`TextError::Gone`] when the terminal is gone, [`TextError::Failed`]
/// when it could not be read.
pub fn window<'a>(
    uia: &'a Uia,
    (element, pattern): (&'a IUIAutomationElement, &'a IUIAutomationTextPattern),
    caret: CaretLineQuery<'a>,
    ask: &WindowRead<'a>,
    (remote, window_handle): (bool, Option<isize>),
) -> Result<WindowAnswer, TextError> {
    let began_us = verbatim_process::clock::now_us();
    let query = ReadQuery {
        element,
        pattern,
        height: ask.height,
        marks: [ask.end.map(|(range, _, _)| range), ask.aside],
        opening: ask.opening,
        caret,
    };
    let (mut read, path) =
        terminal_read(uia, &query, remote).map_err(|error| gone_or_failed(&error))?;
    let mut import_failed = false;
    if let Path::Fallback(error) = &path {
        verbatim_uia_rops::batch::remote_fell_back("terminal read", window_handle, error);
        import_failed = matches!(error, RopsError::Import(_));
    }
    // The rows are converted first: the search and the colour read
    // judge them. The read's end is taken once its calls are done, before
    // the rest is converted.
    let rows = unpadded_utf16(&read.rows);
    let found = match ask.end {
        Some((_, line, true)) => {
            if lines_of(&rows.text).contains(&line) {
                MarkSearch::InWindow
            } else {
                match terminal_read_find(uia, (element, pattern), line)
                    .map_err(|error| gone_or_failed(&error))?
                {
                    Some(above) => MarkSearch::Found(above),
                    None => MarkSearch::Missed,
                }
            }
        }
        Some((_, _, false)) | None => MarkSearch::NotAsked,
    };
    let width = row_width(&read.top_row);
    let screen_rows = u32::try_from(screen_height(&read.screen, width)).unwrap_or(u32::MAX);
    let colours = match (ask.colours, read.top.as_ref()) {
        (true, Some(top)) => Some(screen_colours(
            top,
            (screen_rows, width),
            (remote, window_handle),
        )?),
        (true, None) | (false, _) => None,
    };
    let ended_us = verbatim_process::clock::now_us().max(began_us);
    let end = read
        .end
        .take()
        .ok_or_else(|| TextError::Failed("the read found no document end".to_owned()))?;
    let [end_place, aside_place] = std::mem::take(&mut read.marks).map(|rows| rows.map(place_of));
    Ok(WindowAnswer {
        rows,
        above: read.above,
        screen_top: read.screen_top,
        screen_rows,
        width: u32::try_from(width).unwrap_or(u32::MAX),
        screen_at_start: read.screen_at_start,
        caret_rows_to_end: read.caret_rows_to_end,
        first_row: unpadded_utf16(&read.first_row),
        document_rows: read.document_rows,
        end_place,
        aside_place,
        found,
        colours,
        began_us,
        ended_us,
        end,
        caret: read.caret.take(),
        import_failed,
    })
}

#[cfg(test)]
mod tests;
