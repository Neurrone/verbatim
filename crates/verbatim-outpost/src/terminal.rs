//! A focused terminal's reads (`docs/design/terminal-model.md`, "Where it
//! runs" and "The read"), public so mockapp's tests drive them as the
//! worker does.
//!
//! The reading rules run in Core. The outpost only reads: each read is one
//! snapshot of the terminal's text, from a screen above where the last read
//! said to start to the document's end, taken in one remote program with the
//! screen, the caret and its line, and, when it begins within a key's
//! window Core asked colours for, the screen's colours
//! ([`verbatim_uia_rops::terminal_snapshot`]). It is sent to Core as it is,
//! without its lines' padding ([`TerminalSnapshot`]). Between reads the
//! outpost keeps only where the next read starts, the screen's height and
//! width as Core last told them, the keys' windows that take colours, and
//! the on-demand reading's state ([`reading`]).

pub mod reading;

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, PoisonError};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};
use windows::core::AgileReference;

use verbatim_model::{TerminalColourRun, TerminalScope, TerminalSnapshot, TerminalText};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, FindQuery, Path, Snapshot, SnapshotQuery,
    terminal_find, terminal_snapshot,
};

use crate::text::TextError;
use reading::Reading;

/// What the outpost keeps of a focused terminal between its reads, while
/// the terminal exists, across focus changes.
#[derive(Default)]
pub struct Terminal {
    /// Where the next read starts ([`Snapshot::next_start`]).
    start: Option<AgileReference<IUIAutomationTextRange>>,
    /// The reads taken, which number each ([`TerminalSnapshot::read`]).
    reads: u64,
    /// The read whose start is kept, as Core knows it: the number of the
    /// last read sent when the read that set it was not sent, being
    /// identical to it, so its start is the same; 0 before any.
    start_read: u64,
    /// The number of the last read sent.
    sent_read: u64,
    /// Whether the screen of the last read whose start was kept showed
    /// text ("A read with no text").
    showed_text: bool,
    /// The screen's height in rows and width in cells, as Core last told
    /// them ([`verbatim_model::TextOp::TerminalShape`]); 0 before it did.
    height: u32,
    width: u32,
    /// The latest end of a key's window Core asked colours for, in
    /// milliseconds on the shared clock; 0 before any ("Colours"). Every
    /// read that begins at or before it takes the colours.
    colours_until: u64,
    /// Whether the next read takes the colours whenever it begins: the read
    /// taken at once for a colours request, which counts as inside its
    /// key's window even when the request came after the window's end.
    colours_now: bool,
    /// On-demand reading.
    pub reading: Reading,
    /// When the last read began, in milliseconds on the shared clock, and
    /// in microseconds: a text change observed before it is covered by that
    /// read ([`Terminal::covers`]).
    read_started_ms: u64,
    read_started_us: u64,
    /// When the last read ended, in milliseconds on the shared clock.
    read_ended_ms: u64,
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

    /// A key reached the terminal at `from_ms`, its window lasting until
    /// `until_ms` ([`verbatim_model::TextOp::TerminalColours`]): every read
    /// that begins by the window's end takes the colours. Returns whether a
    /// read is to be taken at once with them, which the next read then
    /// does: the last read ended at or after the key without them, because
    /// this request had not yet come, so what the key did may show only in
    /// a read without colours.
    pub fn colours(&mut self, (from_ms, until_ms): (u64, u64)) -> bool {
        self.colours_until = self.colours_until.max(until_ms);
        self.colours_now =
            self.read_ended_ms != 0 && self.read_ended_ms >= from_ms && !self.read_colours;
        self.colours_now
    }

    /// Whether a read beginning at `started_ms` takes the colours: one
    /// taken at once for a colours request, or one that begins by the end
    /// of a key's window, and so may show what the key did.
    fn takes_colours(&self, started_ms: u64) -> bool {
        self.colours_now || started_ms <= self.colours_until
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
            if self.start_read == snapshot.read {
                self.start_read = self.sent_read;
            }
            return false;
        }
        self.sent = Some(hash);
        self.sent_read = snapshot.read;
        true
    }

    /// Numbers `wire` as the terminal's next read, and notes it as the
    /// read whose start is kept when it is: a read whose screen shows text,
    /// or any read while the last kept showed none ("A read with no text").
    /// Returns whether its start is kept.
    fn number(&mut self, wire: &mut TerminalSnapshot) -> bool {
        self.reads += 1;
        wire.read = self.reads;
        let shows = verbatim_text::terminal_rows::shows_text(&wire.screen.text);
        let kept = shows || !self.showed_text;
        if kept {
            self.start_read = self.reads;
            self.showed_text = shows;
        }
        kept
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
        self.sent_read = snapshot.read;
    }
}

/// A hash of every field of `snapshot` but when it began and ended, its
/// number, and the caret read with it.
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
        ended_ms: _,
        waited,
        read: _,
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

/// The most bytes a spare buffer keeps once a read has been written to
/// Core ([`recycle`]): a few times an ordinary read's text, about 1 KB
/// unpadded for a screen of a shell and 7 KB for two screens of output, so
/// a whole document's read, about 1 MB with a full history, gives the rest
/// back rather than holding it until the outpost ends.
pub const SPARE_KEPT_BYTES: usize = 64 * 1024;

/// How many pairs of spare buffers the outpost keeps: it reads one focused
/// terminal at a time, and a read is written before the next but one is
/// taken.
const SPARES: usize = 2;

/// Buffers a terminal's text and screen are converted into, given back
/// once the read that used them has been written to Core, so an ordinary
/// read allocates nothing for them.
static SPARE: Mutex<VecDeque<(TerminalText, TerminalText)>> = Mutex::new(VecDeque::new());

/// Gives the buffers of `snapshot`, written to Core, back for the next
/// read, emptied and kept to at most [`SPARE_KEPT_BYTES`] each.
pub fn recycle(snapshot: TerminalSnapshot) {
    let mut spare = SPARE.lock().unwrap_or_else(PoisonError::into_inner);
    if spare.len() >= SPARES {
        return;
    }
    let emptied = |mut text: TerminalText| {
        text.text.clear();
        text.text.shrink_to(SPARE_KEPT_BYTES);
        text.lengths.clear();
        text.lengths
            .shrink_to(SPARE_KEPT_BYTES / std::mem::size_of::<(u32, u32)>());
        text
    };
    spare.push_back((emptied(snapshot.text), emptied(snapshot.screen)));
}

/// A pair of buffers for a read's text and screen: spare ones when there
/// are, new ones otherwise.
fn spare() -> (TerminalText, TerminalText) {
    SPARE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pop_front()
        .unwrap_or_default()
}

/// `units`, UTF-16 as a terminal's provider gives its text, in `into`'s
/// buffers, emptied first: as UTF-8 without the spaces that pad its lines,
/// with each line's padded length, exactly as
/// `TerminalText::unpadded(&String::from_utf16_lossy(units))` makes it, an
/// unpaired surrogate becoming U+FFFD, but in one pass and with no copy
/// of the padded text between.
#[must_use]
pub fn unpadded_utf16(units: &[u16], mut into: TerminalText) -> TerminalText {
    let TerminalText { text, lengths } = &mut into;
    text.clear();
    lengths.clear();
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
/// waited for Core's speech queue ("Volatile rows"). The text and the
/// screen are converted into spare buffers when there are ([`recycle`]).
#[must_use]
pub fn snapshot_of(snapshot: &Snapshot, started_ms: u64, waited: bool) -> TerminalSnapshot {
    let (caret_line, caret_column) = snapshot.caret.as_ref().map_or_else(
        || (String::new(), 0),
        |caret| caret_row(&caret.line.text, caret.line.offset),
    );
    let cells = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    let (text, screen) = spare();
    TerminalSnapshot {
        text: unpadded_utf16(&snapshot.text, text),
        screen: unpadded_utf16(&snapshot.screen, screen),
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
        read: 0,
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
    let colours = terminal
        .takes_colours(started_ms)
        .then_some(terminal.width as usize);
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
    let mut wire = snapshot_of(&snapshot, started_ms, waited);
    wire.ended_ms = (crate::protocol::now_us() / 1_000).max(started_ms);
    if terminal.number(&mut wire) {
        terminal.start = snapshot
            .next_start
            .take()
            .and_then(|range| AgileReference::new(&range).ok());
    }
    terminal.read_started_ms = started_ms;
    terminal.read_started_us = clock_us;
    terminal.read_ended_ms = wire.ended_ms;
    terminal.read_colours = wire.colours.is_some();
    terminal.colours_now = false;
    Ok(ReadAnswer {
        snapshot: wire,
        caret: snapshot.caret.take(),
        path,
    })
}

/// What [`history`] read: the text, padding kept, how many rows it holds
/// above where the next read starts, the read whose start that is
/// ([`verbatim_model::TerminalSnapshot::read`]), and which path answered.
pub struct History {
    /// The text.
    pub text: String,
    /// Rows above the start.
    pub above: u32,
    /// The read whose start the rows are counted from; 0 before any.
    pub read: u64,
    /// Which implementation answered.
    pub path: Path,
}

/// Reads the history of `terminal` by place, for a review command above
/// the rows Core's memory keeps ([`verbatim_model::TextOp::TerminalHistory`]):
/// from `above` rows above where its next read starts to the document's
/// end, or the whole document when `whole`, in one remote operation. The
/// answer names the read whose start that is, so Core places the rows only
/// when its memory last took that read.
///
/// # Errors
///
/// [`TextError::Failed`] when it could not be read.
pub fn history(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    terminal: &Terminal,
    (above, whole, remote): (u32, bool, bool),
) -> Result<History, TextError> {
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
    let (snapshot, path) = terminal_snapshot(uia, &query, remote).map_err(|error| {
        if is_gone(&error) {
            TextError::Gone
        } else {
            TextError::Failed(error.to_string())
        }
    })?;
    Ok(History {
        text: String::from_utf16_lossy(&snapshot.text),
        above: snapshot.above,
        read: terminal.start_read,
        path,
    })
}

/// What [`find`] found: the matches, the read whose start their rows are
/// counted from ([`verbatim_model::TerminalSnapshot::read`]), and which
/// path answered.
pub struct Found {
    /// The matches.
    pub matches: Vec<verbatim_model::TerminalMatch>,
    /// The read whose start the rows are counted from; 0 before any.
    pub read: u64,
    /// Which implementation answered.
    pub path: Path,
}

/// Searches `terminal`'s text for a command mark's line, as Core asks
/// ([`verbatim_model::TextOp::TerminalFind`]): `needle` outward from the
/// row `from` rows above where its next read starts, reading `rows` rows
/// from each match, in one remote operation. No matches before the
/// terminal's first read, which places that start. Core decides which
/// match, if any, is the mark's line.
///
/// # Errors
///
/// [`TextError::Failed`] when a call before the search failed.
pub fn find(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    terminal: &Terminal,
    (needle, from, rows): (&str, i32, u32),
    remote: bool,
) -> Result<Found, TextError> {
    let Some(start) = terminal.start_range() else {
        return Ok(Found {
            matches: Vec::new(),
            read: terminal.start_read,
            path: if remote { Path::Remote } else { Path::Classic },
        });
    };
    let query = FindQuery {
        element,
        pattern,
        start: &start,
        needle,
        from,
        rows,
    };
    let (found, path) = terminal_find(uia, &query, remote).map_err(|error| {
        if is_gone(&error) {
            TextError::Gone
        } else {
            TextError::Failed(error.to_string())
        }
    })?;
    Ok(Found {
        matches: found
            .into_iter()
            .map(|found| verbatim_model::TerminalMatch {
                above: found.above,
                text: found.text,
            })
            .collect(),
        read: terminal.start_read,
        path,
    })
}

#[cfg(test)]
mod tests;
