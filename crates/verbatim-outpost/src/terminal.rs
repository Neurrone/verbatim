//! A focused terminal's reads (`docs/design/terminal-model.md`, "Where it
//! runs" and "The read"), public so mockapp's tests drive them as the
//! worker does.
//!
//! The reading rules run in Core. The outpost only reads: each read is one
//! snapshot of the terminal's text, from a screen above where the last read
//! said to start to the document's end, taken in one remote program with the
//! screen, the caret and its line, and, when a key reached the terminal
//! since the last read, the screen's colours
//! ([`verbatim_uia_rops::terminal_snapshot`]). It is sent to Core as it is,
//! without its lines' padding ([`TerminalSnapshot`]). Between reads the
//! outpost keeps only where the next read starts, the screen's height and
//! width as Core last told them, whether colours are due, and the on-demand
//! reading's state ([`reading`]).

pub mod reading;

use std::hash::{Hash, Hasher};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};
use windows::core::AgileReference;

use verbatim_model::{TerminalColourRun, TerminalScope, TerminalSnapshot, TerminalText};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, Path, Snapshot, SnapshotQuery,
    terminal_snapshot,
};

use crate::text::TextError;
use reading::Reading;

/// What the outpost keeps of a focused terminal between its reads, while
/// the terminal exists, across focus changes.
#[derive(Default)]
pub struct Terminal {
    /// Where the next read starts ([`Snapshot::next_start`]).
    start: Option<AgileReference<IUIAutomationTextRange>>,
    /// Whether the screen of the last read whose start was kept showed
    /// text ("A read with no text").
    showed_text: bool,
    /// The screen's height in rows and width in cells, as Core last told
    /// them ([`verbatim_model::TextOp::TerminalShape`]); 0 before it did.
    height: u32,
    width: u32,
    /// Whether the next read takes the colours: a key reached the terminal
    /// since the last read ("Colours").
    colours_due: bool,
    /// On-demand reading.
    pub reading: Reading,
    /// When the last read began, in milliseconds on the shared clock, and
    /// in microseconds: a text change observed before it is covered by that
    /// read ([`Terminal::covers`]).
    read_started_ms: u64,
    read_started_us: u64,
    /// Whether the last read took the colours.
    read_colours: bool,
    /// The caret last sent to Core for the terminal: a caret read that
    /// finds it unchanged, at the same place with the same line, is not
    /// sent again as a caret report. Forgotten when the terminal gains the
    /// focus, and with the terminal when Core releases its node.
    pub sent_caret: Option<verbatim_model::CaretReport>,
    /// A hash of the last snapshot sent, its start time and caret left
    /// out ([`Terminal::sends`]).
    sent: Option<u64>,
}

impl Terminal {
    /// A terminal before its first read.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a text change observed at `millis` (milliseconds on
    /// the shared clock), with its microseconds, `micros`, when they are known (0
    /// otherwise), is covered by the last read, having been observed before
    /// that read began.
    #[must_use]
    pub fn covers(&self, (millis, micros): (u64, u64)) -> bool {
        if micros == 0 {
            millis < self.read_started_ms
        } else {
            micros < self.read_started_us
        }
    }

    /// Where the next read starts, for a read of the history above it.
    #[must_use]
    pub fn start_range(&self) -> Option<IUIAutomationTextRange> {
        self.start.as_ref().and_then(|range| range.resolve().ok())
    }

    /// The screen's height as Core last told it, in rows.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Core told the screen's shape as its reading rules found it.
    pub fn shape(&mut self, height: u32, width: u32) {
        self.height = height;
        self.width = width;
    }

    /// A key reached the terminal at `since_ms`
    /// ([`verbatim_model::TextOp::TerminalColours`]): the next read takes
    /// the colours. Returns whether a read is to be taken at once instead:
    /// one began at or after `since_ms` without them, so the key's window
    /// has a read without colours and none with them yet.
    pub fn colours(&mut self, since_ms: u64) -> bool {
        if self.read_started_ms >= since_ms && self.read_started_ms != 0 {
            if self.read_colours {
                return false;
            }
            self.colours_due = true;
            return true;
        }
        self.colours_due = true;
        false
    }

    /// Whether `snapshot` is to be sent to Core as a live read, noting it
    /// as the last sent when it is: a read identical, field for field, to
    /// the last one sent, but for when it began and the caret read with
    /// it, says nothing new and is not sent (Dickson, 2026-10-26), unless
    /// it waited for Core's speech queue: the reading rules count reads in
    /// a row of which the second waited ("Volatile rows"), and a read
    /// dropped between two changes would make them consecutive.
    pub fn sends(&mut self, snapshot: &TerminalSnapshot) -> bool {
        let hash = fingerprint(snapshot);
        if !snapshot.waited && self.sent == Some(hash) {
            return false;
        }
        self.sent = Some(hash);
        true
    }

    /// The terminal gained the focus: its next read is sent whatever the
    /// last one sent, since Core's reading rules take the first read after
    /// the focus as the one that speaks nothing ("The focus"), and a later
    /// read, taken for it, would say nothing of what it showed new.
    pub fn focused(&mut self) {
        self.sent = None;
    }

    /// Notes `snapshot`, sent to Core as an answer, which is always sent,
    /// as the last sent.
    pub fn answered(&mut self, snapshot: &TerminalSnapshot) {
        self.sent = Some(fingerprint(snapshot));
    }
}

/// A hash of every field of `snapshot` but when it began and the caret
/// read with it.
fn fingerprint(snapshot: &TerminalSnapshot) -> u64 {
    let TerminalSnapshot {
        text,
        screen,
        from_start,
        shrank,
        at_document_start,
        alternate,
        first_row,
        caret_rows_to_end,
        caret_line,
        caret_column,
        above,
        started_ms: _,
        waited,
        colours,
        caret: _,
    } = snapshot;
    let mut hasher = std::hash::DefaultHasher::new();
    for text in [text, screen, first_row, caret_line] {
        text.text.hash(&mut hasher);
        text.lengths.hash(&mut hasher);
    }
    (
        from_start,
        shrank,
        at_document_start,
        alternate,
        caret_rows_to_end,
        caret_column,
        above,
        waited,
    )
        .hash(&mut hasher);
    match colours {
        None => 0u8.hash(&mut hasher),
        Some(rows) => {
            1u8.hash(&mut hasher);
            rows.len().hash(&mut hasher);
            for row in rows {
                row.len().hash(&mut hasher);
                for run in row {
                    (run.column, run.length, run.foreground, run.background).hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
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
        text: TerminalText::unpadded(&String::from_utf16_lossy(&snapshot.text)),
        screen: TerminalText::unpadded(&String::from_utf16_lossy(&snapshot.screen)),
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
    /// The path the snapshot took, for the caller to log a fallback.
    pub path: Path,
}

/// Reads `terminal` through UIA, with the caret and its line read in the
/// same round trip (`caret`), and the colours when they are due. `scope`
/// says which read: `None` or [`TerminalScope::Fresh`] from a screen above
/// where the last read said to start, [`TerminalScope::Whole`] the whole
/// document, and [`TerminalScope::HistoryBlock`] counted up from the
/// document's end by the screen's height with as many rows above. Where the
/// next read starts is kept from the read unless its screen shows no text
/// where the screen of the last read kept did ("A read with no text").
/// `waited` says whether the read waited for Core's speech queue. `remote`
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
    terminal: &mut Terminal,
    (scope, remote, waited): (Option<TerminalScope>, bool, bool),
) -> Result<ReadAnswer, TextError> {
    let clock_us = crate::protocol::now_us();
    let started_ms = clock_us / 1_000;
    let start = terminal.start_range();
    let colours = terminal.colours_due.then_some(terminal.width as usize);
    let height = terminal.height;
    let query = match scope {
        Some(TerminalScope::Whole) => SnapshotQuery {
            element,
            pattern,
            start: None,
            screen_rows: height,
            whole: true,
            above: 0,
            caret: Some(caret),
            colours,
        },
        Some(TerminalScope::HistoryBlock { height }) => {
            terminal.height = height;
            SnapshotQuery {
                element,
                pattern,
                start: None,
                screen_rows: height,
                whole: false,
                above: height,
                caret: Some(caret),
                colours,
            }
        }
        None | Some(TerminalScope::Fresh) => SnapshotQuery {
            element,
            pattern,
            start: start.as_ref(),
            screen_rows: height,
            whole: false,
            above: height,
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
            return Err(if is_gone(&error) {
                TextError::Gone
            } else {
                TextError::Failed(error.to_string())
            });
        }
    };
    // What each read costs (`docs/performance.md`, "A terminal
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
    let wire = snapshot_of(&snapshot, started_ms, waited);
    let shows = verbatim_text::terminal_rows::shows_text(&wire.screen.text);
    if shows || !terminal.showed_text {
        terminal.start = snapshot
            .next_start
            .take()
            .and_then(|range| AgileReference::new(&range).ok());
        terminal.showed_text = shows;
    }
    terminal.read_started_ms = started_ms;
    terminal.read_started_us = clock_us;
    terminal.read_colours = wire.colours.is_some();
    if wire.colours.is_some() {
        terminal.colours_due = false;
    }
    Ok(ReadAnswer {
        snapshot: wire,
        caret: snapshot.caret.take(),
        path,
    })
}

/// Reads the history of `terminal` by place, for a review command above
/// the rows Core's memory keeps ([`verbatim_model::TextOp::TerminalHistory`]):
/// from `above` rows above where its next read starts to the document's
/// end, or the whole document when `whole`, in one remote operation. The
/// text is returned with its padding, with how many rows it holds above
/// that start.
///
/// # Errors
///
/// [`TextError::Failed`] when it could not be read.
pub fn history(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    terminal: &Terminal,
    (above, whole, remote): (u32, bool, bool),
) -> Result<(String, u32), TextError> {
    let start = terminal.start_range();
    let query = SnapshotQuery {
        element,
        pattern,
        start: if whole { None } else { start.as_ref() },
        screen_rows: terminal.height,
        whole,
        above: if whole { 0 } else { above },
        caret: None,
        colours: None,
    };
    let (snapshot, _) = terminal_snapshot(uia, &query, remote).map_err(|error| {
        if is_gone(&error) {
            TextError::Gone
        } else {
            TextError::Failed(error.to_string())
        }
    })?;
    Ok((String::from_utf16_lossy(&snapshot.text), snapshot.above))
}

#[cfg(test)]
mod tests;
