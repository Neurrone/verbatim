//! What a terminal's read says, judged against the read before it: the
//! text half of Core's watch of the focused terminal
//! (`books/dev/src/explanation/design/terminal-model.md`, "TM" below with
//! its rule numbers, and `docs/parity/terminals.md`). It is written from
//! those documents and `docs/nvda/editable-text-and-terminals.md`, not from
//! NVDA's source. No clock, thread or map: every input carries its time.
//!
//! # The API, for Core
//!
//! - A read's answer is parsed once into a [`Window`] ([`Window::new`],
//!   from a [`Read`]: the unpadded text with each line's padded length,
//!   the width, where the screen lies, the caret's row counted from the
//!   end and its column, the colours when read). The watch keeps the last
//!   one as its `old`; [`Window::empty`] is NVDA's empty baseline for a
//!   first read that failed.
//! - [`Reading`] is the rest of the terminal's reading state, one per
//!   watch: typed characters waiting to be shown (F9), the held items with
//!   their one due time (F5, F7, F8), the rows' last changes (F7), the
//!   lines that linger (TM 9) and the keys' window (F8).
//! - At each key Core passes on, [`Reading::key`], with the watch's `old`
//!   as the window before the keys; a typed character with "speak
//!   passwords" off is [`Reading::hold_typed`] as well.
//! - At each read's answer, [`Reading::answer`] with `old` and the new
//!   window gives an [`Answer`]: the typing echoed, the keys' answers, the
//!   new output for the pacing in order (blank lines passed on and
//!   counted, Q1), whether the read kept any line of the last (F14's
//!   judgement is Core's, with the place count), and whether the read is to
//!   be dropped, `old` kept (TM 10). Core then makes the new window `old`
//!   unless it was dropped.
//! - [`Reading::due`] is the one time anything held waits for, for
//!   `Logic::due`; at it, [`Reading::timer`] gives what comes due.
//! - For places and marks: [`Window::screen_lines`] (F14's speech),
//!   [`Window::last_text_line`] (a new end mark's line) and
//!   [`Window::line_place`] (whether the window holds a mark's line).
//!
//! Places are rows above the document's last row, 0 for the last row.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::proven::text_in;
use crate::{Segmenter, WordRules, graphemes, is_word_grapheme, lost_text, squeezed};

mod diff;
mod keys;
#[cfg(test)]
mod tests;

use diff::{Fate, common_ends};

/// A key's window (TM 18): a read ending within this long after the latest
/// key is the keys'.
pub const KEY_WINDOW_US: u64 = 300_000;

/// A typed character not shown within this long of reaching the program is
/// dropped unspoken (TM 20), judged at the next read.
pub const TYPED_LIMIT_US: u64 = 300_000;

/// A rewritten row that changed twice within this long, each a change the
/// program owns, is volatile (TM 17).
pub const VOLATILE_US: u64 = 2_000_000;

/// A row that changed with no key within this long before the first of the
/// keys is the program's, not theirs (TM 18).
pub const UNPROMPTED_US: u64 = 2_000_000;

/// How long a line gone at or below the caret, or rewritten before, is
/// remembered, so that its text coming back is not new (TM 9).
pub const LINGERING_US: u64 = 2_000_000;

/// The shortest quiet period (TM, "Every time limit").
pub const QUIET_US: u64 = 1_000_000;

/// The quiet period after changes `interval_us` apart: 1 second, or 1.5
/// times the interval between the last two changes when that is longer.
#[must_use]
pub fn quiet_period(interval_us: Option<u64>) -> u64 {
    interval_us.map_or(QUIET_US, |interval| {
        QUIET_US.max(interval.saturating_mul(3) / 2)
    })
}

/// A stretch of one row's cells in one foreground and background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ColourRun {
    /// Its first column, in cells.
    pub column: u32,
    /// Its length, in cells.
    pub length: u32,
    /// Its foreground, `0x00BBGGRR`.
    pub foreground: u32,
    /// Its background, `0x00BBGGRR`.
    pub background: u32,
}

/// Where a read found the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    /// How many rows the caret's row and those below it hold, to the
    /// document's end (1 for the last row).
    pub rows_to_end: u32,
    /// Its column in its row, in characters.
    pub column: usize,
}

/// A window read's answer as Core gives it, to be parsed into a [`Window`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read<'a> {
    /// The rows' text from the window's start to the document's end, lines
    /// parted by a carriage return and line feed, each without its padding.
    pub text: &'a str,
    /// Each line's length with its padding, in characters, as runs of
    /// lines of one length: how many lines, and their length.
    pub lengths: &'a [(u32, u32)],
    /// The screen's width in cells.
    pub width: u32,
    /// How many rows the screen's top lies above the document's end.
    pub screen_top: u32,
    /// The screen's height in rows.
    pub screen_rows: u32,
    /// The caret, when the provider reports one.
    pub caret: Option<Caret>,
    /// Each screen row's colours from its top, when read.
    pub colours: Option<Vec<Vec<ColourRun>>>,
    /// When the read's calls ended, in microseconds on the shared clock.
    pub ended_us: u64,
}

/// One line of a window.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Line {
    /// Its text, without padding.
    text: String,
    /// How many rows it takes.
    rows: u32,
    /// The place of its last row.
    below: u32,
}

impl Line {
    /// The place of its first row.
    fn top(&self) -> u32 {
        self.below.saturating_add(self.rows.saturating_sub(1))
    }
}

/// The caret in a window: its line, its row in that line, and its column
/// in that row, in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct At {
    line: usize,
    row: u32,
    column: usize,
}

/// The byte ranges of the rows of `text` at `width` cells a row: each row
/// as many characters as fill its cells, a wide character taking two.
fn row_ranges(text: &str, width: u32) -> Vec<std::ops::Range<usize>> {
    let width = usize::try_from(width).unwrap_or(usize::MAX).max(1);
    let mut rows = Vec::new();
    let (mut start, mut cells) = (0usize, 0usize);
    for (at, c) in text.char_indices() {
        let wide = crate::cell_width(text_in(text, at..at.saturating_add(c.len_utf8())));
        if cells.saturating_add(wide) > width && cells > 0 {
            rows.push(start..at);
            (start, cells) = (at, 0);
        }
        cells = cells.saturating_add(wide);
    }
    rows.push(start..text.len());
    rows
}

/// A read, parsed: the window's lines with where each lies, the screen,
/// the caret, and the screen rows' colours when read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Window {
    lines: Vec<Line>,
    width: u32,
    screen_top: u32,
    screen_rows: u32,
    caret: Option<At>,
    colours: Option<Vec<Vec<ColourRun>>>,
    ended_us: u64,
}

impl Window {
    /// `read` parsed: its lines cut into rows by the width, each line's
    /// rows as many as its padded length takes and at least one, the
    /// empty line after a last line break not a line.
    #[must_use]
    pub fn new(read: Read<'_>) -> Self {
        let width = read.width.max(1);
        let mut texts: Vec<&str> = read.text.split("\r\n").collect();
        if texts.len() > 1 && texts.last() == Some(&"") {
            texts.pop();
        }
        let padded = read.lengths.iter().flat_map(|&(count, length)| {
            std::iter::repeat_n(length, usize::try_from(count).unwrap_or(0))
        });
        let mut padded = padded.chain(std::iter::repeat(0));
        let mut lines: Vec<Line> = texts
            .into_iter()
            .map(|text| {
                let characters = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
                let padding = padded.next().unwrap_or(0).saturating_sub(characters);
                let cells = u32::try_from(crate::cell_width(text))
                    .unwrap_or(u32::MAX)
                    .saturating_add(padding);
                Line {
                    text: text.to_owned(),
                    rows: cells.div_ceil(width).max(1),
                    below: 0,
                }
            })
            .collect();
        let mut below = 0u32;
        for line in lines.iter_mut().rev() {
            line.below = below;
            below = below.saturating_add(line.rows);
        }
        let caret = read.caret.and_then(|caret| {
            let place = caret.rows_to_end.saturating_sub(1);
            let index = lines
                .iter()
                .position(|line| line.below <= place && place <= line.top())?;
            let line = lines.get(index)?;
            let row = line.top().saturating_sub(place);
            Some(At {
                line: index,
                row,
                column: caret.column,
            })
        });
        Self {
            lines,
            width,
            screen_top: read.screen_top,
            screen_rows: read.screen_rows,
            caret,
            colours: read.colours,
            ended_us: read.ended_us,
        }
    }

    /// The window of no text, as a first read that failed leaves it: the
    /// next read's lines are all new.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            lines: Vec::new(),
            width: 1,
            screen_top: 0,
            screen_rows: 0,
            caret: None,
            colours: None,
            ended_us: 0,
        }
    }

    /// When the read's calls ended.
    #[must_use]
    pub fn ended_us(&self) -> u64 {
        self.ended_us
    }

    /// Whether line `index` has a row on the screen.
    fn on_screen(&self, index: usize) -> bool {
        let Some(line) = self.lines.get(index) else {
            return false;
        };
        let bottom = self.screen_top.saturating_sub(self.screen_rows);
        self.screen_top > 0 && line.below < self.screen_top && line.top() >= bottom
    }

    /// The indices of the lines with a row on the screen.
    fn screen(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.lines.len()).filter(|&index| self.on_screen(index))
    }

    /// The text of the screen's lines that have text, top to bottom: what a
    /// new screen says (F14).
    #[must_use]
    pub fn screen_lines(&self) -> Vec<String> {
        self.screen()
            .filter_map(|index| self.lines.get(index))
            .filter(|line| !line.text.is_empty())
            .map(|line| line.text.clone())
            .collect()
    }

    /// Whether the screen shows any text.
    fn screen_has_text(&self) -> bool {
        self.screen()
            .filter_map(|index| self.lines.get(index))
            .any(|line| !line.text.is_empty())
    }

    /// The last line with text and the place of its first row: an end
    /// mark's line and its distance.
    #[must_use]
    pub fn last_text_line(&self) -> Option<(String, u32)> {
        self.lines
            .iter()
            .rev()
            .find(|line| !line.text.is_empty())
            .map(|line| (line.text.clone(), line.top()))
    }

    /// The text of the window's last row, without padding, empty when it
    /// is blank: the row an end mark set at this read sits after.
    #[must_use]
    pub fn last_row_text(&self) -> String {
        self.lines
            .len()
            .checked_sub(1)
            .and_then(|last| {
                let line = self.lines.get(last)?;
                Some(self.row_text(last, line.rows.saturating_sub(1)))
            })
            .unwrap_or_default()
    }

    /// The place of the first row of every line reading `text`, lowest
    /// first.
    #[must_use]
    pub fn places_of(&self, text: &str) -> Vec<u32> {
        self.lines
            .iter()
            .rev()
            .filter(|line| line.text == text)
            .map(Line::top)
            .collect()
    }

    /// The place of the first row of the lowest line reading `text`, when
    /// the window holds one: whether a mark's line is in the window.
    #[must_use]
    pub fn line_place(&self, text: &str) -> Option<u32> {
        self.lines
            .iter()
            .rev()
            .find(|line| line.text == text)
            .map(Line::top)
    }

    /// Every line's text, top to bottom, a blank line empty: what a read
    /// says when more was written than its window holds (F1).
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|line| line.text.as_str())
    }

    /// The caret's line: its text and the place of its first row.
    #[must_use]
    pub fn caret_line(&self) -> Option<(&str, u32)> {
        let line = self.lines.get(self.caret?.line)?;
        Some((line.text.as_str(), line.top()))
    }

    /// The lines with text below the caret's line, top to bottom: a
    /// program's footer (F13).
    #[must_use]
    pub fn below_caret(&self) -> Vec<String> {
        let Some(caret) = self.caret else {
            return Vec::new();
        };
        self.lines
            .iter()
            .skip(caret.line.saturating_add(1))
            .filter(|line| !line.text.is_empty())
            .map(|line| line.text.clone())
            .collect()
    }

    /// The text of line `index`, empty when there is none.
    fn text(&self, index: usize) -> &str {
        self.lines.get(index).map_or("", |line| line.text.as_str())
    }

    /// The screen row of line `index`'s `row`th row, counted from the
    /// screen's top, when it lies on the screen.
    fn screen_row(&self, index: usize, row: u32) -> Option<usize> {
        let line = self.lines.get(index)?;
        let place = line.top().checked_sub(row)?;
        let from_top = self.screen_top.checked_sub(1)?.checked_sub(place)?;
        (from_top < self.screen_rows).then(|| usize::try_from(from_top).ok())?
    }

    /// The colours of line `index`'s first row on the screen, when read.
    fn row_colours(&self, index: usize, row: u32) -> Option<&[ColourRun]> {
        let row = self.screen_row(index, row)?;
        self.colours.as_ref()?.get(row).map(Vec::as_slice)
    }

    /// The line and its row at screen row `at`, when a line lies there.
    fn at_screen_row(&self, at: usize) -> Option<(usize, u32)> {
        let place = self
            .screen_top
            .checked_sub(1)?
            .checked_sub(u32::try_from(at).ok()?)?;
        let index = self
            .lines
            .iter()
            .position(|line| line.below <= place && place <= line.top())?;
        let line = self.lines.get(index)?;
        Some((index, line.top().saturating_sub(place)))
    }

    /// Screen row `at`'s emphasis, when its colours were read: the colours
    /// of its first letter or digit, or of its first cell when it has none.
    /// A list's highlight covers its selected entry's text (fzf, tig,
    /// Microsoft Edit), while its marker, border or scroll bar may not.
    fn emphasis(&self, at: usize) -> Option<(u32, u32)> {
        let runs = self.colours.as_ref()?.get(at)?;
        let column = self
            .at_screen_row(at)
            .and_then(|(index, row)| {
                let text = self.row_text(index, row);
                let found = text.char_indices().find(|(_, c)| c.is_alphanumeric())?;
                Some(crate::cell_width(text_in(&text, ..found.0)))
            })
            .unwrap_or(0);
        let column = u32::try_from(column).ok()?;
        runs.iter()
            .find(|run| run.column <= column && column < run.column.saturating_add(run.length))
            .map(|run| (run.foreground, run.background))
    }

    /// The byte range of line `index`'s `row`th row.
    fn row_range(&self, index: usize, row: u32) -> std::ops::Range<usize> {
        let text = self.text(index);
        let rows = row_ranges(text, self.width);
        let last = rows.len().saturating_sub(1);
        let row = usize::try_from(row).unwrap_or(usize::MAX).min(last);
        rows.get(row).cloned().unwrap_or(text.len()..text.len())
    }

    /// The byte offset of the caret in its line.
    fn caret_at(&self) -> Option<(usize, usize)> {
        let caret = self.caret?;
        let text = self.text(caret.line);
        let range = self.row_range(caret.line, caret.row);
        let at = text_in(text, range.clone())
            .char_indices()
            .nth(caret.column)
            .map_or(range.end, |(at, _)| range.start.saturating_add(at));
        Some((caret.line, at))
    }

    /// The caret's line's text split at the caret: before it and after it.
    fn at_caret(&self) -> Option<(usize, &str, &str)> {
        let (line, at) = self.caret_at()?;
        let text = self.text(line);
        Some((line, text_in(text, ..at), text_in(text, at..)))
    }

    /// The text of line `index`'s `row`th row, without padding: what a
    /// caret key's line unit reads, a row, as the provider's line unit is.
    fn row_text(&self, index: usize, row: u32) -> String {
        text_in(self.text(index), self.row_range(index, row))
            .trim_end()
            .to_owned()
    }
}

/// A caret key's unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Unit {
    /// A character: Left Arrow, Right Arrow, Home, End.
    Character,
    /// A word: Control+Left Arrow, Control+Right Arrow.
    Word,
    /// A line: Up Arrow, Down Arrow.
    Line,
}

/// What a key is, as far as reading cares.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyKind {
    /// A typed character.
    Typed,
    /// A caret key with its unit.
    Caret(Unit),
    /// Enter: it forgets held typing, and its answer is only new output.
    Enter,
    /// Tab, Escape, Control+C, Control+D or Control+Break, which forget
    /// held typing.
    Forget,
    /// Delete, which speaks what it leaves at the caret.
    Delete,
    /// Any other key.
    Other,
}

/// What a key's answer says: the caret's line, or its unit at the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    /// The unit: [`Unit::Line`] for a whole line, which is empty for a
    /// blank one.
    pub unit: Unit,
    /// The text.
    pub text: String,
}

/// What a read or the timer says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// Typed characters the terminal showed, one grapheme each (F9).
    pub echo: Vec<String>,
    /// The keys' answers (F8), spoken whole, never paced.
    pub keys: Vec<Said>,
    /// New output for the pacing, in order; a blank line is empty, passed
    /// on to be counted, never spoken (Q1).
    pub output: Vec<String>,
    /// Whether the read kept or changed any line with text of the last
    /// (F14's input).
    pub shares_line: bool,
    /// Whether the read showed no text where the last had text: it is
    /// dropped and the last kept, but for its caret (TM 10).
    pub dropped: bool,
}

/// Something held for the quiet period.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
enum Hold {
    /// A growing line's unfinished last word (F5): `said` is what was
    /// spoken of the line.
    Word { said: String },
    /// A volatile row (F7), spoken whole once it settles.
    Volatile,
    /// A row the keys changed (TM 21), spoken by its changed words against
    /// `said` once no key has come for the quiet period.
    Keys { said: String },
}

/// A held line, by its index in the last window and its text there.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Held {
    line: usize,
    text: String,
    hold: Hold,
    due_us: u64,
}

/// A line's last two changes the program made (F7).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Change {
    line: usize,
    text: String,
    last_us: u64,
    volatile: bool,
}

/// The keys' window (TM 18): from the first key to the latest, with the
/// window before the first.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct KeyWindow {
    first_us: u64,
    last_us: u64,
    before: Arc<Window>,
    /// The keys passed in it, in order.
    keys: Vec<KeyKind>,
}

/// A terminal's reading state beside the watch's last window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reading {
    typed: VecDeque<(String, u64)>,
    held: Vec<Held>,
    changes: Vec<Change>,
    lingering: Vec<(String, u64)>,
    keys: Option<KeyWindow>,
    suggestion: Option<String>,
}

impl Reading {
    /// A terminal's reading with nothing held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A typed character, `text`, passed on at `at_us` with "speak
    /// passwords" off: held until a read shows it (TM 20).
    pub fn hold_typed(&mut self, text: &str, at_us: u64) {
        self.typed.push_back((text.to_owned(), at_us));
    }

    /// A key passed on to the program at `at_us`, `old` the watch's last
    /// window: it opens the keys' window, or extends the one open, and
    /// Enter and the keys that forget typing forget it.
    pub fn key(&mut self, kind: KeyKind, at_us: u64, old: &Arc<Window>) {
        if matches!(kind, KeyKind::Enter | KeyKind::Forget) {
            self.typed.clear();
        }
        // A window holding Enter is Enter's answer, only new output (TM
        // 19): a key after it opens a window of its own.
        match &mut self.keys {
            Some(window)
                if at_us <= window.last_us.saturating_add(KEY_WINDOW_US)
                    && !window.keys.contains(&KeyKind::Enter) =>
            {
                window.last_us = at_us;
                window.keys.push(kind);
            }
            _ => {
                self.keys = Some(KeyWindow {
                    first_us: at_us,
                    last_us: at_us,
                    before: Arc::clone(old),
                    keys: vec![kind],
                });
            }
        }
        // Rows the keys hold wait for no key for the quiet period.
        let due = at_us.saturating_add(QUIET_US);
        for held in &mut self.held {
            if matches!(held.hold, Hold::Keys { .. }) {
                held.due_us = due;
            }
        }
    }

    /// The footer of `window`, the watch's last window, judged by this
    /// reading's record of the rows the program rewrote (F13): the lines
    /// with text below the caret's line; otherwise the run of lines at the
    /// screen's bottom the program has rewritten, blank lines left out and
    /// the trailing blank ones skipped; otherwise the last line with text.
    /// Empty when the window has no text.
    #[must_use]
    pub fn footer(&self, window: &Window) -> Vec<String> {
        let below = window.below_caret();
        if !below.is_empty() {
            return below;
        }
        let rewritten = |index: usize| self.changes.iter().any(|change| change.line == index);
        let screen: Vec<usize> = window.screen().collect();
        let mut run: Vec<String> = screen
            .into_iter()
            .rev()
            .skip_while(|&index| window.text(index).is_empty())
            .take_while(|&index| window.text(index).is_empty() || rewritten(index))
            .filter(|&index| !window.text(index).is_empty())
            .map(|index| window.text(index).to_owned())
            .collect();
        run.reverse();
        if run.is_empty() {
            window
                .last_text_line()
                .map(|(line, _)| vec![line])
                .unwrap_or_default()
        } else {
            run
        }
    }

    /// The one time something held waits for, while anything is held.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        self.held.iter().map(|held| held.due_us).min()
    }

    /// What comes due at `now_us`: each held item whose quiet period has
    /// passed, spoken once, top to bottom.
    pub fn timer(&mut self, now_us: u64) -> Answer {
        let mut answer = Answer::default();
        let (mut due, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|held| held.due_us <= now_us);
        self.held = kept;
        due.sort_by_key(|held| held.line);
        for held in due {
            match held.hold {
                Hold::Word { said } => {
                    let rest = held.text.strip_prefix(&*said).unwrap_or(&held.text);
                    push_output(&mut answer, rest.trim());
                }
                Hold::Volatile => {
                    for change in &mut self.changes {
                        if change.line == held.line {
                            change.volatile = false;
                        }
                    }
                    push_output(&mut answer, &held.text);
                }
                Hold::Keys { said } => {
                    if let Some(text) = changed_words(&said, &held.text) {
                        answer.keys.push(Said {
                            unit: Unit::Line,
                            text,
                        });
                    }
                }
            }
        }
        answer
    }

    /// What the read `new` says, against `old`, the watch's last window.
    pub fn answer(&mut self, old: &Window, new: &Window) -> Answer {
        self.judge(old, new, (false, false))
    }

    /// What the read `new` says, against `old`, when it waited for speech:
    /// a row its program rewrote at `old` too is volatile however long
    /// apart the two reads were (TM 17), since the reads between were held.
    pub fn answer_waited(&mut self, old: &Window, new: &Window) -> Answer {
        self.judge(old, new, (true, false))
    }

    /// What the read `new` says, against `old`, when a key sent it: the
    /// key's own read, in the keys' window however long it waited for the
    /// caret (TM 18).
    pub fn answer_key(&mut self, old: &Window, new: &Window) -> Answer {
        self.judge(old, new, (false, true))
    }

    /// [`Reading::answer`], `waited` when the read waited for speech, `key`
    /// when a key sent it.
    fn judge(&mut self, old: &Window, new: &Window, (waited, key): (bool, bool)) -> Answer {
        let now = new.ended_us;
        self.lingering.retain(|(_, until)| *until >= now);
        if old.screen_has_text() && !new.screen_has_text() {
            return Answer {
                dropped: true,
                ..Answer::default()
            };
        }
        // After Enter, the answer is only new output (TM 19): the rows a
        // read changes in its window are the program's.
        let keyed = match &self.keys {
            Some(window) if key || now <= window.last_us.saturating_add(KEY_WINDOW_US) => {
                Some(window.clone()).filter(|window| !window.keys.contains(&KeyKind::Enter))
            }
            Some(_) | None => {
                self.keys = None;
                None
            }
        };
        let (mut fates, mut used) = diff::align(old, new);
        if keyed.is_some() {
            diff::pair_by_place(old, new, (&mut fates, &mut used));
        }
        let mut answer = Answer {
            // TM 7: a read that keeps or pairs a line with text of the last
            // is no new screen.
            shares_line: fates.iter().any(
                |fate| matches!(fate, Fate::Kept(i) | Fate::Paired(i) if !old.text(*i).is_empty()),
            ),
            ..Answer::default()
        };
        let mut read = Judged {
            old,
            new,
            now,
            fates: &fates,
            waited,
            answer: &mut answer,
            held: std::mem::take(&mut self.held),
            changes: std::mem::take(&mut self.changes),
            caret_change: None,
        };
        for (j, fate) in fates.iter().enumerate() {
            match *fate {
                Fate::Inserted => self.inserted(&mut read, j),
                Fate::Kept(i) => self.kept(&mut read, i, j, keyed.as_ref()),
                Fate::Paired(i) => self.paired(&mut read, i, j, keyed.as_ref()),
            }
        }
        if let Some(window) = &keyed {
            keys::caret(self, &mut read, window);
            keys::emphasis(&mut read);
        }
        let (held, changes) = (
            std::mem::take(&mut read.held),
            std::mem::take(&mut read.changes),
        );
        self.remember((old, new), (&fates, &used), (held, changes));
        let limit = now.saturating_sub(TYPED_LIMIT_US);
        self.typed.retain(|(_, at)| *at >= limit);
        answer
    }

    /// An inserted line: output, unless it is the typing on the caret's
    /// line or a lingering line back (TM 9); a blank line counts only once
    /// a line with text is below it (TM 12).
    fn inserted(&mut self, read: &mut Judged<'_>, j: usize) {
        let text = read.new.text(j);
        if text.is_empty() {
            let below = read.new.lines.iter().skip(j.saturating_add(1));
            if below.into_iter().any(|line| !line.text.is_empty()) {
                read.answer.output.push(String::new());
            }
            return;
        }
        if let Some(at) = self.lingering.iter().position(|(line, _)| line == text) {
            self.lingering.remove(at);
            return;
        }
        // Only padding, the spaces after a row's last character, is not
        // its text (TM 3): a list's unmarked entries keep their indent.
        let rest = self.echo_typing(read, j, "");
        push_output(read.answer, rest.trim_end_matches(' '));
    }

    /// A kept line: a held word whose line the caret left is spoken.
    fn kept(&mut self, read: &mut Judged<'_>, i: usize, j: usize, keyed: Option<&KeyWindow>) {
        let caret_here = read.new.caret.is_some_and(|caret| caret.line == j);
        if keyed.is_none() && caret_here {
            // Typing the terminal cannot show (a space at the line's end)
            // is echoed as the caret passes over it (TM 20).
            self.echo_hidden(read, i, j);
        }
        if caret_here {
            return;
        }
        let Some(at) = read
            .held
            .iter()
            .position(|held| held.line == i && matches!(held.hold, Hold::Word { .. }))
        else {
            return;
        };
        let held = read.held.remove(at);
        if let Hold::Word { said } = held.hold {
            let rest = read.new.text(j).strip_prefix(&*said).unwrap_or("");
            push_output(read.answer, rest.trim());
        }
    }

    /// A changed line, owned by the program or, in the keys' window, by
    /// the keys.
    fn paired(&mut self, read: &mut Judged<'_>, i: usize, j: usize, keyed: Option<&KeyWindow>) {
        let word = read
            .held
            .iter()
            .position(|held| held.line == i && matches!(held.hold, Hold::Word { .. }));
        let base = match word.map(|at| read.held.remove(at)) {
            Some(Held {
                hold: Hold::Word { said },
                ..
            }) => said,
            _ => read.old.text(i).to_owned(),
        };
        let caret_here = read.new.caret.is_some_and(|caret| caret.line == j);
        let new = read.new.text(j).to_owned();
        let changed = if caret_here {
            self.caret_line_change(read, j, &base)
        } else {
            Some(new.clone())
        };
        let Some(changed) = changed else {
            return;
        };
        let program = keyed.is_none_or(|window| program_owns(&read.changes, i, window));
        if !program {
            if caret_here {
                read.caret_change = Some((base, changed));
            } else {
                keys::away(self, read, (i, j), (&base, &changed));
            }
            return;
        }
        self.program_change(read, (i, j), (&base, &changed), caret_here);
    }

    /// The caret's line changed from `base`: the typing it shows echoed
    /// (TM 20), and the line with the echoed typing taken out, when
    /// anything else changed; `None` when nothing else did.
    fn caret_line_change(&mut self, read: &mut Judged<'_>, j: usize, base: &str) -> Option<String> {
        let (_, head, tail) = read.new.at_caret()?;
        let (prefix, _) = common_ends(base, head);
        let inserted = text_in(head, prefix..);
        let shown = self.typing_shown(inserted);
        if shown.is_empty() {
            if !inserted.trim().is_empty() && !self.typed.is_empty() {
                // A line changed some other way drops the typing.
                self.typed.clear();
            }
            return (read.new.text(j) != base).then(|| read.new.text(j).to_owned());
        }
        let cut = head.len().saturating_sub(shown.len());
        read.answer.echo.extend(
            graphemes(&shown)
                .into_iter()
                .map(|range| text_in(&shown, range).to_owned()),
        );
        let without = format!("{}{tail}", text_in(head, ..cut));
        (without != base).then_some(without)
    }

    /// The longest run of the typed characters, from the first, that
    /// `inserted` ends with, taken from the typing; empty when none.
    fn typing_shown(&mut self, inserted: &str) -> String {
        let mut shown = String::new();
        let mut count = 0usize;
        let mut best = (String::new(), 0usize);
        for (text, _) in &self.typed {
            shown.push_str(text);
            count = count.saturating_add(1);
            if inserted.ends_with(&*shown) {
                best = (shown.clone(), count);
            }
        }
        self.typed.drain(..best.1);
        best.0
    }

    /// Typed spaces the caret passed over on an unchanged caret line.
    fn echo_hidden(&mut self, read: &mut Judged<'_>, i: usize, j: usize) {
        let (Some(was), Some(now)) = (read.old.caret, read.new.caret) else {
            return;
        };
        if was.line != i || now.line != j {
            return;
        }
        let mut moved = if was.row == now.row {
            now.column.saturating_sub(was.column)
        } else {
            0
        };
        while moved > 0 && self.typed.front().is_some_and(|(text, _)| text == " ") {
            self.typed.pop_front();
            read.answer.echo.push(" ".to_owned());
            moved = moved.saturating_sub(1);
        }
    }

    /// The typing an inserted caret line shows, echoed; the rest of the
    /// line returned, or `fallback` when it is not the caret's line.
    fn echo_typing<'t>(&mut self, read: &mut Judged<'t>, j: usize, base: &str) -> &'t str {
        let new: &'t str = read.new.text(j);
        if read.new.caret.is_none_or(|caret| caret.line != j) {
            return new;
        }
        let Some((_, head, _)) = read.new.at_caret() else {
            return new;
        };
        let (prefix, _) = common_ends(base, head);
        let shown = self.typing_shown(text_in(head, prefix..));
        if shown.is_empty() {
            return new;
        }
        read.answer.echo.extend(
            graphemes(&shown)
                .into_iter()
                .map(|range| text_in(&shown, range).to_owned()),
        );
        let cut = head.len().saturating_sub(shown.len());
        text_in(new, ..cut)
    }

    /// A change the program made to line `i`, now `j`: growth spoken past
    /// what was said, its unfinished word held (TM 13, F5); a rewrite from
    /// its first changed word, a spinner silent (TM 16, F6), a volatile row
    /// held (TM 17, F7).
    fn program_change(
        &mut self,
        read: &mut Judged<'_>,
        (i, j): (usize, usize),
        (base, new): (&str, &str),
        caret_here: bool,
    ) {
        if new == base || lost_text(base, new) || squeezed(new) == squeezed(base) {
            return;
        }
        if let Some(gained) = new.strip_prefix(base) {
            let gained = gained.trim_start();
            let at_end = caret_here
                && read
                    .new
                    .at_caret()
                    .is_some_and(|(_, _, tail)| tail.trim().is_empty());
            if at_end && !gained.ends_with(char::is_whitespace) {
                let last = last_word(gained);
                push_output(read.answer, text_in(gained, ..last).trim_end());
                let said = text_in(
                    new,
                    ..new.len().saturating_sub(gained.len().saturating_sub(last)),
                );
                self.held.push(Held {
                    line: j,
                    text: new.to_owned(),
                    hold: Hold::Word {
                        said: said.to_owned(),
                    },
                    due_us: read.now.saturating_add(QUIET_US),
                });
            } else {
                push_output(read.answer, gained.trim_end());
            }
            return;
        }
        if spinner(base, new) {
            return;
        }
        let now = read.now;
        let earlier = read
            .changes
            .iter()
            .position(|change| change.line == i)
            .map(|at| read.changes.remove(at));
        // Changed at the read before a read that waited for speech counts
        // as changed twice within the limit (TM 17).
        let interval = earlier
            .as_ref()
            .map(|change| now.saturating_sub(change.last_us))
            .filter(|&interval| {
                interval <= VOLATILE_US
                    || (read.waited
                        && earlier
                            .as_ref()
                            .is_some_and(|change| change.last_us == read.old.ended_us))
            });
        let volatile = earlier.as_ref().is_some_and(|change| change.volatile);
        self.changes.push(Change {
            line: j,
            text: new.to_owned(),
            last_us: now,
            volatile: volatile || interval.is_some(),
        });
        if volatile || interval.is_some() {
            read.held.retain(|held| held.line != i);
            self.held.push(Held {
                line: j,
                text: new.to_owned(),
                hold: Hold::Volatile,
                due_us: now.saturating_add(quiet_period(interval)),
            });
            return;
        }
        if let Some(text) = changed_words(base, new) {
            push_output(read.answer, &text);
        }
    }

    /// Carries what is held and remembered of `old`'s lines, `held` and
    /// `changes`, to `new`'s by the alignment, beside what this read made;
    /// a line gone drops them, and lingers when it was at or below the
    /// caret or the program rewrote it before (TM 9).
    fn remember(
        &mut self,
        (old, new): (&Window, &Window),
        (fates, used): (&[Fate], &[bool]),
        (held, changes): (Vec<Held>, Vec<Change>),
    ) {
        let mut moved = vec![None; old.lines.len()];
        for (j, fate) in fates.iter().enumerate() {
            if let Fate::Kept(i) | Fate::Paired(i) = *fate
                && let Some(slot) = moved.get_mut(i)
            {
                *slot = Some(j);
            }
        }
        let to = |line: usize, text: &str| {
            moved
                .get(line)
                .copied()
                .flatten()
                .filter(|_| old.text(line) == text)
        };
        let caret = old.caret.map_or(usize::MAX, |caret| caret.line);
        let until = new.ended_us.saturating_add(LINGERING_US);
        for (i, kept) in used.iter().enumerate() {
            let text = old.text(i);
            if !kept && !text.is_empty() && (i >= caret || changes.iter().any(|c| c.line == i)) {
                self.lingering.push((text.to_owned(), until));
            }
        }
        self.held.extend(held.into_iter().filter_map(|mut held| {
            let j = to(held.line, &held.text)?;
            if !matches!(held.hold, Hold::Word { .. }) {
                new.text(j).clone_into(&mut held.text);
            }
            held.line = j;
            Some(held)
        }));
        self.changes
            .extend(changes.into_iter().filter_map(|mut change| {
                let j = to(change.line, &change.text)?;
                change.line = j;
                new.text(j).clone_into(&mut change.text);
                Some(change)
            }));
    }
}

/// Whether the program owns a change to old line `i` in the keys' window,
/// by the lines' last changes, `changes`: it changed with no key in the 2
/// seconds before the first key (TM 18).
fn program_owns(changes: &[Change], i: usize, window: &KeyWindow) -> bool {
    changes.iter().any(|change| {
        change.line == i
            && change.last_us < window.first_us
            && window.first_us.saturating_sub(change.last_us) <= UNPROMPTED_US
    })
}

/// A read being judged: the two windows, the alignment and the answer.
struct Judged<'a> {
    old: &'a Window,
    new: &'a Window,
    now: u64,
    fates: &'a [Fate],
    /// Whether the read waited for speech (TM 17).
    waited: bool,
    answer: &'a mut Answer,
    /// What was held of `old`'s lines, by their indices there, each taken
    /// out as the read consumes it.
    held: Vec<Held>,
    /// The program's last changes to `old`'s lines, likewise.
    changes: Vec<Change>,
    /// The caret's line in the keys' window, from what it was to what it
    /// is with the echoed typing taken out, when anything else changed.
    caret_change: Option<(String, String)>,
}

/// Pushes `text` as a line of output unless it is empty.
fn push_output(answer: &mut Answer, text: &str) {
    if !text.is_empty() {
        answer.output.push(text.to_owned());
    }
}

/// Where the last word of `text` starts: after its last space (F5).
fn last_word(text: &str) -> usize {
    text.char_indices()
        .rev()
        .find(|(_, c)| crate::is_space(*c))
        .map_or(0, |(at, c)| at.saturating_add(c.len_utf8()))
}

/// Whether a rewrite from `old` to `new` changed one character, neither a
/// letter nor a digit, in place: a spinner (TM 16, Q6).
fn spinner(old: &str, new: &str) -> bool {
    let (prefix, suffix) = common_ends(old, new);
    let (was, now) = (
        text_in(old, prefix..old.len().saturating_sub(suffix)),
        text_in(new, prefix..new.len().saturating_sub(suffix)),
    );
    let one = |text: &str| graphemes(text).len() == 1 && !is_word_grapheme(text);
    one(was) && one(now)
}

/// `new` from the start of the first word that differs from `old` (TM 16):
/// `None` when nothing but white space differs.
fn changed_words(old: &str, new: &str) -> Option<String> {
    let (prefix, _) = common_ends(old, new);
    let words = Segmenter::new().words(new, WordRules::for_text(new, None));
    let start = words
        .iter()
        .find(|range| range.end > prefix && !text_in(new, (*range).clone()).trim().is_empty())
        .map(|range| range.start)?;
    let text = text_in(new, start..).trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}
