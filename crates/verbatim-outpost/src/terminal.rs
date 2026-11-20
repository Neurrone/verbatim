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

use verbatim_model::{TerminalColourRun, TerminalScope, TerminalSnapshot, TerminalText};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, FindQuery, Path, Snapshot, SnapshotQuery,
    terminal_find, terminal_snapshot,
};

use crate::text::TextError;

/// `units`, UTF-16 as a terminal's provider gives its text, as UTF-8
/// without the spaces that pad its lines, with each line's padded length,
/// exactly as `TerminalText::unpadded(&String::from_utf16_lossy(units))`
/// makes it, an unpaired surrogate becoming U+FFFD, but in one pass and
/// with no copy of the padded text between. The text starts with room for
/// one byte a code unit, which ASCII text never outgrows, since padding is
/// only removed.
#[must_use]
pub fn unpadded_utf16(units: &[u16]) -> TerminalText {
    let mut into = TerminalText {
        text: String::with_capacity(units.len()),
        lengths: Vec::new(),
    };
    let TerminalText { text, lengths } = &mut into;
    // The line's length so far in characters, padding included; the spaces
    // after its last other character, written only once another follows;
    // and a carriage return that may begin a line break.
    let mut length: u32 = 0;
    let mut spaces: u32 = 0;
    let mut carriage_return = false;
    let push = |text: &mut String, spaces: &mut u32, length: &mut u32, character: char| {
        for _ in 0..*spaces {
            text.push(' ');
        }
        *spaces = 0;
        text.push(character);
        *length += 1;
    };
    let end_line = |lengths: &mut Vec<(u32, u32)>, length: u32| match lengths.last_mut() {
        Some((count, last)) if *last == length => *count += 1,
        _ => lengths.push((1, length)),
    };
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
        if carriage_return {
            carriage_return = false;
            if character == '\n' {
                end_line(lengths, length);
                text.push_str("\r\n");
                length = 0;
                spaces = 0;
                continue;
            }
            push(text, &mut spaces, &mut length, '\r');
        }
        match character {
            '\r' => carriage_return = true,
            ' ' => {
                spaces += 1;
                length += 1;
            }
            character => push(text, &mut spaces, &mut length, character),
        }
    }
    if carriage_return {
        push(text, &mut spaces, &mut length, '\r');
    }
    end_line(lengths, length);
    into
}

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
    let calls_before = verbatim_uia::calls::peek().uia;
    let result = terminal_snapshot(uia, &query, remote);
    let elapsed_us = u64::try_from(timer.elapsed().as_micros()).unwrap_or(u64::MAX);
    let calls = verbatim_uia::calls::peek().uia.saturating_sub(calls_before);
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

#[cfg(test)]
mod tests;
