//! A terminal's new output (milestone M4 item 9; `phase6-design.md`,
//! "Terminal reading by diffing the screen"), public so mockapp's tests
//! drive it as the worker does.
//!
//! Every read fetches the screen, in one remote program
//! ([`verbatim_uia_rops::terminal_screen`]), and diffs it by line with the
//! screen as last seen ([`screen::diff`]): what was inserted is spoken,
//! never what was only deleted, and a changed line speaks from the start of
//! the word that changed. The screen's top two rows as last read are the
//! anchor, found again by a range kept at the top row while the terminal
//! has discarded nothing, and otherwise by their text; how far up they now
//! lie is how far the text scrolled, which lines the two screens up, and the rows that
//! scrolled by beyond the old screen's lines went by unread: the first of
//! them are read too, so a flood's start is heard, and the rest are
//! counted as skipped. When the anchor has left a history it is not found;
//! with history above both screens that means the history overflowed, and
//! the skipped lines are "more than" the history's rows left unspoken.
//! Otherwise (the screen cleared, a full-screen program's alternate screen
//! opened or closed) the screens are lined up by their common lines.
//!
//! When to read is decided by on-demand reading ([`reading`]).

pub mod keys;
pub mod reading;
pub mod screen;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};
use windows::core::AgileReference;

use verbatim_model::{LineChange, Skipped, TerminalOutput};
use verbatim_uia::Uia;
use verbatim_uia_rops::{
    CaretAnswer, CaretLineQuery, Error as RopsError, Path, Screen, ScreenAnchor, ScreenQuery,
    terminal_screen,
};

use crate::text::TextError;
use reading::Reading;
use screen::{Shift, diff, screen_lines};

/// What the outpost remembers of a terminal's screen between reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// The screen's lines as last read ([`screen_lines`]).
    pub screen: Vec<String>,
    /// The same lines as they were last said: a line that has only got
    /// shorter since keeps what it said before it was cut, because it is
    /// being rewritten (a progress line cleared and written again), and
    /// the read that finds it written again says what changed from what it
    /// said ("51%"), not from a fragment ("oading 51%").
    pub said: Vec<String>,
    /// Its top row, as the provider gave it: the anchor.
    pub top_row: String,
    /// The row after it.
    pub next_row: String,
    /// Whether that screen had no history above it.
    pub alternate: bool,
    /// The text's first row as it was read, as the provider gave it: while
    /// it reads the same, the terminal has discarded nothing.
    pub first_row: String,
    /// The lines of the screen this one replaced whole, sharing no line
    /// with it: the main screen a full-screen program's alternate screen
    /// covers, which the terminal shows again as it was when the program
    /// closes the alternate screen. `None` once a screen shares lines with
    /// the one before it is replaced whole again.
    pub main: Option<Vec<String>>,
    /// The line of `screen` the caret was on, when it was read with it and
    /// found there: the last line that reads as the caret's row does, or,
    /// for a line that wrapped onto more rows, the last that holds it.
    pub caret: Option<usize>,
    /// How far up the screen before had scrolled when this one was read,
    /// when that was found: 0 when the two line up row for row.
    pub scrolled: Option<u32>,
}

/// A read of a terminal's screen, as text: [`Screen`] without its caret,
/// so the diff can be tested on simulated text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenText {
    /// The screen's text.
    pub text: String,
    /// Its top row.
    pub top_row: String,
    /// The row after it.
    pub next_row: String,
    /// Whether it has no history above it.
    pub alternate: bool,
    /// How far up the anchor's top row now lies, when it was found.
    pub shift: Option<u32>,
    /// The first rows that went by unread.
    pub head: String,
    /// How many rows `head` holds.
    pub head_rows: u32,
    /// The row the old screen's last line was on, as it is now.
    pub old_last_row: String,
    /// The rows of the whole text, when the anchor was sought and not
    /// found.
    pub document_rows: Option<u32>,
    /// Whether the text held still while it was read.
    pub settled: bool,
    /// Whether the terminal's view moved while it was read.
    pub view_moved: bool,
    /// The text's first row.
    pub first_row: String,
    /// The caret's line, when the caret was read with the screen, as the
    /// provider gave it.
    pub caret_line: Option<String>,
}

impl From<&Screen> for ScreenText {
    fn from(screen: &Screen) -> Self {
        Self {
            text: screen.text.clone(),
            top_row: screen.top_row.clone(),
            next_row: screen.next_row.clone(),
            alternate: screen.alternate,
            shift: screen.shift,
            head: screen.head.clone(),
            head_rows: screen.head_rows,
            old_last_row: screen.old_last_row.clone(),
            document_rows: screen.document_rows,
            settled: screen.settled,
            view_moved: screen.view_moved,
            first_row: screen.first_row.clone(),
            caret_line: screen
                .caret
                .as_ref()
                .map(|caret| String::from_utf16_lossy(&caret.line.text)),
        }
    }
}

/// Where a terminal's screen is read from: the provider, or simulated text
/// in the unit tests.
pub trait ScreenSource {
    /// What a read fails with.
    type Error;

    /// Reads the screen, finding `anchor`'s rows when given, and reading up
    /// to `head_wanted` of the rows past the old screen's `seen_rows` that
    /// went by unread.
    ///
    /// # Errors
    ///
    /// The source's error, when the terminal is gone or cannot be read.
    fn read(
        &mut self,
        anchor: Option<&Memory>,
        seen_rows: u32,
        head_wanted: u32,
    ) -> Result<ScreenText, Self::Error>;
}

/// What a read found.
#[derive(Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "one per read, handed straight to the caller, never stored"
)]
pub enum Found {
    /// What is new (empty when nothing is), and what to remember.
    Output(TerminalOutput, Memory),
    /// The terminal wrote to the screen while it was read, so nothing is
    /// trusted and nothing is remembered: the change that disturbed it
    /// causes another read.
    Unsettled,
}

/// The lines of a block of rows read in one call, each without its line
/// break and padding.
fn block_lines(text: &str, rows: u32) -> Vec<String> {
    if rows == 0 {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n')
        .map(|line| verbatim_text::trim_padding(line.strip_suffix('\r').unwrap_or(line)).to_owned())
        .collect()
}

/// Why a terminal is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMode {
    /// A focus arriving: only note where the text is now; what the screen
    /// holds is not new to the user.
    Baseline,
    /// Its text changed, or Core asked: what is new.
    Change,
    /// Speech was cut off: what is new, for typing echo only. A read the
    /// terminal wrote to meanwhile is still remembered, finding nothing, so
    /// what came before the cut is never spoken.
    Cancel,
}

/// What is new on a terminal's screen since `memory` was read, and what to
/// remember. With no memory, or for a baseline, nothing is new.
/// `head_wanted` is how many of the lines that went by unread are read
/// from their first.
///
/// # Errors
///
/// The source's error.
pub fn read_new<S: ScreenSource>(
    source: &mut S,
    memory: Option<&Memory>,
    mode: ReadMode,
    head_wanted: u32,
) -> Result<Found, S::Error> {
    let earlier = memory.filter(|_| mode != ReadMode::Baseline);
    let seen = earlier.map_or(0, |memory| {
        u32::try_from(memory.screen.len()).unwrap_or(u32::MAX)
    });
    let screen = source.read(earlier, seen, head_wanted)?;
    let unsettled = earlier.is_some_and(|old| !trusted(old, &screen));
    if unsettled && mode != ReadMode::Cancel {
        return Ok(Found::Unsettled);
    }
    let new = screen_lines(&screen.text);
    let mut remembered = Memory {
        screen: new.clone(),
        said: new.clone(),
        top_row: screen.top_row.clone(),
        next_row: screen.next_row.clone(),
        alternate: screen.alternate,
        first_row: screen.first_row.clone(),
        main: None,
        caret: None,
        scrolled: screen.shift,
    };
    let Some(old) = earlier.filter(|_| !unsettled) else {
        return Ok(Found::Output(TerminalOutput::default(), remembered));
    };
    let (main, restored) = main_screen(old, &new);
    remembered.main = main;
    if let Some(output) = restored {
        return Ok(Found::Output(output, remembered));
    }
    // The caret's line on the new screen, the last that reads as it does.
    let caret_row = screen
        .caret_line
        .as_deref()
        .map(|line| verbatim_text::trim_padding(line.trim_end_matches(['\r', '\n'])));
    let cursor = caret_row.and_then(|line| new.iter().rposition(|row| row == line));
    // The caret's row is one row of a line that wrapped onto more, which
    // the screen's text gives whole: the last line holding that row.
    remembered.caret = cursor.or_else(|| {
        caret_row
            .filter(|row| !row.is_empty())
            .and_then(|row| new.iter().rposition(|line| line.contains(row)))
    });
    let output = match screen.shift {
        Some(shift) => {
            let unread = shift.saturating_sub(seen);
            let head = block_lines(&screen.head, screen.head_rows);
            let counted = unread.saturating_sub(screen.head_rows);
            let shift_rows = Shift::Known(shift as usize);
            let mut found = diff(&old.said, &new, shift_rows, cursor);
            let since_read = diff(&old.screen, &new, shift_rows, cursor).changed;
            found.changed = with_since_read(found.changed, since_read);
            // The old screen scrolled away whole: its last line, which
            // output may have been written to, is read where it is now.
            let old_last_row =
                verbatim_text::trim_padding(screen.old_last_row.trim_end_matches(['\r', '\n']));
            let changed = if shift >= seen {
                old.screen
                    .last()
                    .and_then(|last| screen::line_change(last, old_last_row))
            } else {
                found.changed
            };
            let mut lines = found.below;
            if shift >= seen {
                keep_footer(&mut lines, &old.screen, old_last_row);
            }
            TerminalOutput {
                above: found.above,
                changed,
                head,
                skipped: (counted > 0).then_some(Skipped::Count(counted)),
                lines,
            }
        }
        None => match screen.document_rows {
            // The anchor left a history above both screens: it overflowed.
            Some(rows) if !screen.alternate && !old.alternate => TerminalOutput {
                // With no history beyond the screen, no count is possible.
                skipped: Some(
                    match rows.saturating_sub(u32::try_from(new.len()).unwrap_or(u32::MAX)) {
                        0 => Skipped::Uncounted,
                        beyond => Skipped::MoreThan(beyond),
                    },
                ),
                lines: new,
                ..TerminalOutput::default()
            },
            _ => {
                let found = diff(&old.screen, &new, Shift::Unknown, cursor);
                TerminalOutput {
                    above: found.above,
                    changed: found.changed,
                    lines: found.below,
                    ..TerminalOutput::default()
                }
            }
        },
    };
    if let Some(shift) = screen.shift {
        let kept = old.said.get(shift as usize..).unwrap_or_default();
        for (row, was) in remembered.said.iter_mut().zip(kept) {
            // A row erased whole is drawn again, not rewritten: what is
            // written to it next is new.
            if !row.trim().is_empty() && was.starts_with(row.as_str()) && was != row {
                row.clone_from(was);
            }
        }
    }
    Ok(Found::Output(output, remembered))
}

/// Whether a read after `old` can be trusted: its text held still, and
/// either its view did not move or the old screen's last line is still on
/// the row it was on, or only grew there (output was being written to it).
/// Output that scrolls the view leaves every row it scrolled where it was,
/// so the rows read are rows the terminal showed, and those below them are
/// read next time. The console host moving its view down a row for each
/// line written into a scroll region above a footer redraws the footer
/// lower and writes over the row it left, so a read whose range was taken
/// before the move has lost the footer, which the next read would find as
/// new. During a flood in the console host the view moves under nearly
/// every read, so distrusting them all left the flood unread until it
/// ended, losing its first lines and the anchor.
fn trusted(old: &Memory, screen: &ScreenText) -> bool {
    if !screen.settled {
        return false;
    }
    let Some(last) = old.screen.last().filter(|_| screen.view_moved) else {
        return true;
    };
    screen.shift.is_some()
        && verbatim_text::trim_padding(screen.old_last_row.trim_end_matches(['\r', '\n']))
            .starts_with(last.as_str())
}

/// A focused terminal's memory and reading state, kept by the worker
/// between reads.
#[derive(Default)]
pub struct Terminal {
    memory: Option<Memory>,
    /// On-demand reading.
    pub reading: Reading,
    /// Core's request, by its id and trace, whose read the terminal
    /// disturbed: answered by the next read that settles.
    pub owed: Option<(u64, verbatim_model::TraceId)>,
    /// Whether the terminal's `FindText` matches a row's padding, as the
    /// console host's does and Windows Terminal's does not
    /// ([`ScreenQuery::matches_padding`]), set by the worker from the
    /// terminal's window.
    pub matches_padding: bool,
    /// When the last read began, in milliseconds since the Unix epoch: a
    /// text change observed before it is covered by that read.
    pub read_started_ms: u64,
    /// A range at the start of the screen's top row as last read, tried
    /// before a search ([`ScreenAnchor::range`]).
    top: Option<AgileReference<IUIAutomationTextRange>>,
    /// When the read that `memory` holds ended: a key pressed after that
    /// cannot show in it, where one pressed while it was under way can.
    memory_read_ms: u64,
    /// What was remembered before the last read, from up to [`KEPT_READS`]
    /// reads before it, oldest first, each with when its read ended, for a
    /// key pressed before the last read ([`Terminal::screen_before`]): a
    /// program's answer to a key can be read several times before the
    /// key's request reaches the outpost.
    previous: std::collections::VecDeque<Kept>,
    /// While a line key's watch is open (`keys`), reads do not advance the
    /// memory: each compares with the screen before the key, so a redraw
    /// read half done is never taken as new output, and what the key
    /// redrew is said once, by its answer.
    pub frozen: bool,
}

/// How many earlier reads [`Terminal`] keeps.
const KEPT_READS: usize = 8;

/// A memory kept from an earlier read.
struct Kept {
    read_ms: u64,
    memory: Memory,
    top: Option<AgileReference<IUIAutomationTextRange>>,
}

impl Terminal {
    /// Makes the memory the screen as it was before a key pressed at
    /// `pressed_at_ms` (Unix milliseconds): the memory itself when its read
    /// ended before then, else the newest kept from a read that did, the
    /// reads after it forgotten. Returns that screen, or `None` when no
    /// read kept ended before the key.
    pub fn screen_before(&mut self, pressed_at_ms: u64) -> Option<Memory> {
        let memory = self.memory.as_ref()?;
        if self.memory_read_ms < pressed_at_ms {
            return Some(memory.clone());
        }
        let at = self
            .previous
            .iter()
            .rposition(|kept| kept.read_ms < pressed_at_ms)?;
        tracing::debug!(
            later = self.previous.len() - at,
            "a line key's screen before it is from a read before the last"
        );
        self.previous.truncate(at + 1);
        let kept = self.previous.pop_back()?;
        self.memory = Some(kept.memory.clone());
        self.memory_read_ms = kept.read_ms;
        self.top = kept.top;
        Some(kept.memory)
    }

    /// Keeps the memory as an earlier read's, with `top` as its range,
    /// before `memory` takes its place.
    fn keep(&mut self, memory: Memory, top: Option<AgileReference<IUIAutomationTextRange>>) {
        if let Some(kept) = self.memory.replace(memory) {
            if self.previous.len() == KEPT_READS {
                self.previous.pop_front();
            }
            self.previous.push_back(Kept {
                read_ms: self.memory_read_ms,
                memory: kept,
                top,
            });
        }
        self.memory_read_ms = crate::protocol::now_us() / 1_000;
    }

    /// The screen as it was before a key pressed at `pressed_at_ms`, as
    /// [`Terminal::screen_before`] finds it, without changing what is
    /// remembered.
    #[must_use]
    pub fn screen_at(&self, pressed_at_ms: u64) -> Option<&Memory> {
        let memory = self.memory.as_ref()?;
        if self.memory_read_ms < pressed_at_ms {
            return Some(memory);
        }
        self.previous
            .iter()
            .rev()
            .find(|kept| kept.read_ms < pressed_at_ms)
            .map(|kept| &kept.memory)
    }

    /// Ends a line key's watch (`frozen`) with `memory`, the screen read
    /// that answered it, as what is remembered from now on; or, with none,
    /// keeps the screen before the key, so the next read finds what the
    /// key did as new.
    pub fn thaw(&mut self, memory: Option<Memory>) {
        self.frozen = false;
        let Some(memory) = memory else {
            return;
        };
        let top = self.top.take();
        self.keep(memory, top);
    }
}

impl Terminal {
    /// What is remembered of the screen, `None` before the first read.
    #[must_use]
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
    }
}

/// Whether a failed remote operation or classic read means the terminal
/// is gone.
fn is_gone(error: &RopsError) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}

/// A terminal's screen read through UIA: the remote program when `remote`,
/// the classic implementation otherwise or when the program fails.
struct UiaScreen<'a> {
    uia: &'a Uia,
    element: &'a IUIAutomationElement,
    pattern: &'a IUIAutomationTextPattern,
    /// The caret to read with the screen.
    caret: Option<CaretLineQuery<'a>>,
    /// The caret the read found.
    caret_found: Option<CaretAnswer>,
    remote: bool,
    matches_padding: bool,
    /// The range at the start of the top row the last read remembered.
    top: Option<IUIAutomationTextRange>,
    /// The range at the start of the top row this read found.
    top_found: Option<IUIAutomationTextRange>,
    /// The path the read took.
    path: Option<Path>,
}

impl ScreenSource for UiaScreen<'_> {
    type Error = TextError;

    fn read(
        &mut self,
        anchor: Option<&Memory>,
        seen_rows: u32,
        head_wanted: u32,
    ) -> Result<ScreenText, TextError> {
        let query = ScreenQuery {
            element: self.element,
            pattern: self.pattern,
            anchor: anchor.map(|memory| ScreenAnchor {
                top: &memory.top_row,
                next: &memory.next_row,
                range: self.top.as_ref(),
                first: &memory.first_row,
                no_history: memory.alternate,
            }),
            matches_padding: self.matches_padding,
            seen_rows,
            head_wanted,
            caret: self.caret,
        };
        let started = std::time::Instant::now();
        let calls_before = verbatim_uia::calls::peek().uia;
        let result = terminal_screen(self.uia, &query, self.remote);
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let calls = verbatim_uia::calls::peek().uia.saturating_sub(calls_before);
        let (mut screen, path) = match result {
            Ok(answer) => answer,
            Err(error) => {
                tracing::debug!(elapsed_us, calls, %error, "terminal screen read failed");
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
            shift = ?screen.shift,
            head_rows = screen.head_rows,
            document_rows = ?screen.document_rows,
            alternate = screen.alternate,
            settled = screen.settled,
            view_moved = screen.view_moved,
            top = screen.top_row.trim_end(),
            "terminal screen timing"
        );
        self.path = Some(path);
        let text = ScreenText::from(&screen);
        self.caret_found = screen.caret.take();
        self.top_found = screen.top.take();
        Ok(text)
    }
}

/// What [`read`] found.
pub struct ReadAnswer {
    /// What is new, or that the read was disturbed.
    pub found: Found,
    /// The caret the read found.
    pub caret: Option<CaretAnswer>,
    /// The path the read took, for the caller to log a fallback.
    pub path: Option<Path>,
}

/// Reads what is new on a focused terminal's screen since the last read
/// ([`read_new`]) through UIA, keeping the memory in `terminal` (unless the
/// read was disturbed). `remote` tries the remote program first. With
/// `caret`, the caret and its line are read in the same round trip.
///
/// # Errors
///
/// [`TextError::Gone`] when the terminal is gone, [`TextError::Failed`]
/// when it could not be read.
pub fn read<'a>(
    uia: &'a Uia,
    (element, pattern): (&'a IUIAutomationElement, &'a IUIAutomationTextPattern),
    caret: Option<CaretLineQuery<'a>>,
    terminal: &mut Terminal,
    (head_wanted, remote, mode): (u32, bool, ReadMode),
) -> Result<ReadAnswer, TextError> {
    let mut source = UiaScreen {
        uia,
        element,
        pattern,
        caret,
        caret_found: None,
        remote,
        matches_padding: terminal.matches_padding,
        top: terminal.top.as_ref().and_then(|range| range.resolve().ok()),
        top_found: None,
        path: None,
    };
    let found = read_new(&mut source, terminal.memory.as_ref(), mode, head_wanted)?;
    if let Found::Output(_, memory) = &found
        && !terminal.frozen
    {
        let top = source
            .top_found
            .as_ref()
            .and_then(|range| AgileReference::new(range).ok());
        let kept_top = std::mem::replace(&mut terminal.top, top);
        terminal.keep(memory.clone(), kept_top);
    }
    Ok(ReadAnswer {
        found,
        caret: source.caret_found,
        path: source.path,
    })
}

/// The text one message's terminal output may take, well inside
/// [`crate::protocol::MAX_MESSAGE_BYTES`], since JSON's escaping can make a
/// line several times longer ([`split`]).
pub const MESSAGE_TEXT_BUDGET: usize = crate::protocol::MAX_MESSAGE_BYTES / 8;

/// `output` split into outputs that each take at most `budget` bytes of
/// text, in order, so that a message carrying one stays within
/// [`crate::protocol::MAX_MESSAGE_BYTES`] (`phase6-design.md`, the limit
/// on outpost messages). What is said of the last line read before
/// (`above` and `changed`) goes in the first; the rest keeps its order,
/// the lines before a skipped count as a head and those after it as the
/// newest lines. A single line larger than the budget is an output of its
/// own. One output that fits is returned as it is.
#[must_use]
pub fn split(output: TerminalOutput, budget: usize) -> Vec<TerminalOutput> {
    let size = |line: &String| line.len() + 8;
    let total: usize = output
        .above
        .iter()
        .chain(&output.head)
        .chain(&output.lines)
        .map(size)
        .sum::<usize>()
        + output
            .changed
            .as_ref()
            .map_or(0, |change| change.text.len() + change.line.len());
    if total <= budget {
        return vec![output];
    }
    let mut first = TerminalOutput {
        changed: output.changed.clone(),
        ..TerminalOutput::default()
    };
    let mut parts: Vec<TerminalOutput> = Vec::new();
    let mut used = first
        .changed
        .as_ref()
        .map_or(0, |change| change.text.len() + change.line.len());
    let mut above = output.above;
    let mut stream = pieces(TerminalOutput {
        above: Vec::new(),
        ..output
    });
    for line in above.drain(..) {
        if used + size(&line) > budget && !first.above.is_empty() {
            break;
        }
        used += size(&line);
        first.above.push(line);
    }
    // Lines of `above` that did not fit go first among the rest.
    let mut rest: Vec<Piece> = above.into_iter().map(Piece::Line).collect();
    rest.append(&mut stream);
    let mut current = first;
    for piece in rest {
        let cost = match &piece {
            Piece::Line(line) => size(line),
            Piece::Skipped(_) => 16,
        };
        if used + cost > budget && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            used = 0;
        }
        used += cost;
        match piece {
            Piece::Skipped(skipped) => {
                current.head.append(&mut current.lines);
                current.skipped = Some(current.skipped.map_or(skipped, |was| was.plus(skipped)));
            }
            Piece::Line(line) => current.lines.push(line),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// One piece of new output, in the order it is spoken.
enum Piece {
    Line(String),
    Skipped(Skipped),
}

/// The new lines of `output`, in the order they are spoken, without its
/// changed line.
fn pieces(output: TerminalOutput) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = output.head.into_iter().map(Piece::Line).collect();
    pieces.extend(output.skipped.map(Piece::Skipped));
    pieces.extend(output.lines.into_iter().map(Piece::Line));
    pieces
}

/// Whether `line` holds nothing but white space, which Core does not speak.
fn is_blank(line: &str) -> bool {
    line.chars().all(verbatim_text::is_space)
}

/// One output that says what `older` and then `newer` say, for the same
/// terminal, when `newer` was found before `older` could be sent: the
/// outpost's messages to Core are merged while they wait
/// (`docs/crates/verbatim-outpost.md`, "Messages to Core"). It keeps the
/// flood policy's limits: at most `wanted` lines in its head and as many
/// in its lines, the rest counted as skipped, which Core then treats as it
/// treats any output.
///
/// `newer`'s changed line is the last line `older` read. When `older` has
/// new lines, that line is among them and not yet spoken, so the whole line
/// as it is now takes its place, as Core puts a changed line in place of an
/// earlier version still waiting; when the last of them is blank, Core
/// would speak the change after it, and so does the combined output. When
/// `older` has none, the two changes of the same line become one
/// ([`merged_change`]).
#[must_use]
pub fn combine(older: TerminalOutput, newer: TerminalOutput, wanted: usize) -> TerminalOutput {
    let wanted = wanted.max(1);
    let mut older = older;
    // What `older` found above its last line comes first; its change of
    // that line is then a line like the others.
    let mut stream: Vec<Piece> = std::mem::take(&mut older.above)
        .into_iter()
        .map(Piece::Line)
        .collect();
    let mut older_changed = older.changed.take();
    if !stream.is_empty()
        && let Some(change) = older_changed.take()
    {
        stream.push(Piece::Line(change.text));
    }
    stream.extend(pieces(older));
    let TerminalOutput {
        above: newer_above,
        changed: newer_changed,
        head,
        skipped,
        lines,
    } = newer;
    let changed = if stream.is_empty() && newer_above.is_empty() {
        merged_change(older_changed, newer_changed)
    } else {
        if stream.is_empty()
            && let Some(change) = older_changed.take()
        {
            stream.push(Piece::Line(change.text));
        }
        let in_place = matches!(stream.last(), Some(Piece::Line(line)) if !is_blank(line));
        match newer_changed {
            Some(change) if in_place => {
                if let Some(Piece::Line(line)) = stream.last_mut() {
                    *line = change.line;
                }
                stream.extend(newer_above.into_iter().map(Piece::Line));
            }
            Some(change) => {
                stream.extend(newer_above.into_iter().map(Piece::Line));
                stream.push(Piece::Line(change.text));
            }
            None => stream.extend(newer_above.into_iter().map(Piece::Line)),
        }
        older_changed
    };
    stream.extend(pieces(TerminalOutput {
        head,
        skipped,
        lines,
        ..TerminalOutput::default()
    }));
    let (head, skipped, lines) = fit(stream, wanted);
    TerminalOutput {
        above: Vec::new(),
        changed,
        head,
        skipped,
        lines,
    }
}

/// Fits new output into a head, a skipped count, and the newest lines, at
/// most `wanted` lines each: everything when it is no more than `wanted`
/// lines with nothing skipped; otherwise the lines before the first skipped
/// count (the start of the flood) as the head, the lines after the last
/// one as the newest lines, and everything between them counted.
fn fit(stream: Vec<Piece>, wanted: usize) -> (Vec<String>, Option<Skipped>, Vec<String>) {
    let any_skipped = stream
        .iter()
        .any(|piece| matches!(piece, Piece::Skipped(_)));
    if !any_skipped && stream.len() <= wanted {
        let lines = stream
            .into_iter()
            .filter_map(|piece| match piece {
                Piece::Line(line) => Some(line),
                Piece::Skipped(_) => None,
            })
            .collect();
        return (Vec::new(), None, lines);
    }
    let mut rest = std::collections::VecDeque::from(stream);
    let mut head = Vec::new();
    while head.len() < wanted
        && let Some(Piece::Line(_)) = rest.front()
    {
        if let Some(Piece::Line(line)) = rest.pop_front() {
            head.push(line);
        }
    }
    let mut lines = Vec::new();
    while lines.len() < wanted
        && let Some(Piece::Line(_)) = rest.back()
    {
        if let Some(Piece::Line(line)) = rest.pop_back() {
            lines.push(line);
        }
    }
    lines.reverse();
    let skipped = rest
        .into_iter()
        .map(|piece| match piece {
            Piece::Line(_) => Skipped::Count(1),
            Piece::Skipped(count) => count,
        })
        .reduce(Skipped::plus);
    (head, skipped, lines)
}

/// Two changes of the same line, `older` and then `newer`, as one: what
/// changed from the line as it was before `older` to the line as it is
/// after `newer`. Each change's text is the end of its line, from where it
/// differs, so the line is unchanged up to the earlier of the two starts.
/// It only grew when both only grew.
fn merged_change(older: Option<LineChange>, newer: Option<LineChange>) -> Option<LineChange> {
    let (older, newer) = match (older, newer) {
        (older, None) => return older,
        (None, newer) => return newer,
        (Some(older), Some(newer)) => (older, newer),
    };
    let start = |change: &LineChange| {
        change
            .line
            .len()
            .checked_sub(change.text.len())
            .filter(|&start| change.line.get(start..) == Some(change.text.as_str()))
    };
    let (Some(older_start), Some(newer_start)) = (start(&older), start(&newer)) else {
        // Not a change this outpost made; say the whole line.
        return Some(LineChange {
            text: newer.line.clone(),
            line: newer.line,
            appended: false,
            uncertain: 0,
            inserted: String::new(),
            since_read: None,
        });
    };
    let from = older_start.min(newer_start);
    let appended = older.appended && newer.appended;
    let inserted = if appended {
        newer.line[from..].to_owned()
    } else {
        older.inserted + &newer.inserted
    };
    Some(LineChange {
        text: newer.line[from..].to_owned(),
        line: newer.line,
        appended,
        uncertain: if appended { older.uncertain } else { 0 },
        inserted,
        since_read: merged_change(
            older.since_read.map(|change| *change),
            newer.since_read.map(|change| *change),
        )
        .map(Box::new),
    })
}

/// For a screen whose rows all scrolled away since `old` was read, its
/// lines `lines`, all new: when the old screen's last line was a footer
/// that stayed on the screen's last row while the rows above it scrolled
/// (a scroll region above it), it is said only as it changed. That is so
/// when the row it was on now holds something else (`old_last_row`, not
/// that line nor that line grown, as output written to a last line would
/// be), and the new screen's last line shares a start with it.
fn keep_footer(lines: &mut Vec<String>, old: &[String], old_last_row: &str) {
    let Some(footer) = old.last().filter(|line| !line.trim().is_empty()) else {
        return;
    };
    let Some(now) = lines.last() else {
        return;
    };
    let moved_on = !old_last_row.starts_with(footer.as_str());
    let shares_start = footer
        .chars()
        .zip(now.chars())
        .take_while(|(a, b)| a == b)
        .next()
        .is_some();
    if moved_on && shares_start {
        let change = screen::line_change(footer, now);
        lines.pop();
        lines.extend(change.map(|change| change.text.trim_start().to_owned()));
    }
}

/// What a read whose new screen is `new` makes of the main screen
/// (`Memory::main`), after the screen `old`: the main screen to remember
/// from now on, and, when `new` is the main screen shown again as it was
/// after a full-screen program closed its alternate screen, what is new on
/// it since, which is then the read's output. A screen replaced whole is
/// remembered as the main screen: a full-screen program opened its
/// alternate screen over it.
fn main_screen(old: &Memory, new: &[String]) -> (Option<Vec<String>>, Option<TerminalOutput>) {
    if !replaces_whole(&old.screen, new) {
        return (old.main.clone(), None);
    }
    let restored = old
        .main
        .as_ref()
        .and_then(|main| main_shift(main, new).map(|shift| (main, shift)));
    match restored {
        Some((main, shift)) => {
            let found = diff(main, new, Shift::Known(shift), None);
            let output = TerminalOutput {
                above: found.above,
                changed: found.changed,
                lines: found.below,
                ..TerminalOutput::default()
            };
            (None, Some(output))
        }
        None => (Some(old.screen.clone()), None),
    }
}

/// Whether `new` replaced `old` whole: `old` has lines that are not blank,
/// and none of them is on `new`, nor grown there or cut short, as a line
/// being typed on is. A screen cleared on its way to being drawn again
/// (the alternate screen cleared before a program draws it) counts.
fn replaces_whole(old: &[String], new: &[String]) -> bool {
    let written = |line: &&String| !line.trim().is_empty();
    let related = |old: &String| {
        new.iter()
            .filter(written)
            .any(|new| new.starts_with(old.as_str()) || old.starts_with(new.as_str()))
    };
    old.iter().any(|line| written(&line)) && !old.iter().filter(written).any(related)
}

/// How many of the main screen's first lines, `main`, have scrolled away
/// on the screen shown again after an alternate screen, `new`: the fewest
/// whose removal leaves lines that start `new` as they were, the last
/// allowed to have grown (output written to it). `None` when no part of
/// the main screen starts `new`, as after the screen was cleared.
fn main_shift(main: &[String], new: &[String]) -> Option<usize> {
    (0..main.len()).find(|&first| {
        let kept = &main[first..];
        let (last, earlier) = kept.split_last().expect("kept is not empty");
        new.len() >= kept.len()
            && new[..earlier.len()] == *earlier
            && new[earlier.len()].starts_with(last.as_str())
    })
}

/// The changed line's change from what it said, `said`, with its change
/// since it was read, `since_read`, kept beside it where the two differ
/// ([`LineChange::since_read`]); a line whose change since it was read
/// brought it back to what it said is a change with nothing to speak.
fn with_since_read(said: Option<LineChange>, since_read: Option<LineChange>) -> Option<LineChange> {
    match (said, since_read) {
        (said, None) => said,
        (Some(said), Some(since_read)) if said == since_read => Some(said),
        (Some(said), Some(since_read)) => Some(LineChange {
            since_read: Some(Box::new(since_read)),
            ..said
        }),
        (None, Some(since_read)) => Some(LineChange {
            text: String::new(),
            line: since_read.line.clone(),
            appended: false,
            uncertain: 0,
            inserted: String::new(),
            since_read: Some(Box::new(since_read)),
        }),
    }
}

#[cfg(test)]
mod tests;
