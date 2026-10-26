//! The reading rules of a terminal (`docs/design/terminal-model.md`,
//! "Reading rules"), on top of the row memory and its alignment
//! ([`crate::terminal_rows`]): what each row is, new output with growing
//! lines, redrawn and volatile rows, spinners, keys and their window, typed
//! characters, the review cursor following the caret, rows away from the
//! caret that keys change, colours, the alternate screen, resizing, focus,
//! padding and floods.
//!
//! [`TerminalReader`] is a state machine with no clock and no thread of its
//! own: every input carries its time, and every input answers with a
//! [`Step`], the speech it gives, the lines it withholds and why, and when
//! the quiet-period timer should next fire. The caller reads the terminal,
//! passes keys as they reach the program, and calls [`TerminalReader::timer`]
//! when the timer it asked for comes due.

use crate::terminal_rows::{Lost, NewRow, OldRow, Outcome, Read, Row, TerminalMemory, Text};
use std::borrow::Cow;

use crate::{Segmenter, WordRules, graphemes, is_space, squeezed};

pub use crate::terminal_rows::LINGERING_MS;

/// A key's window ("Keys and their window"): the reads that begin within this many
/// milliseconds after a key reached the program can carry its effects;
/// a change found later is output.
pub const KEY_WINDOW_MS: u64 = 300;

/// A typed character not shown within this many milliseconds of reaching
/// the program is dropped unspoken ("Keys and their window"), judged at the next read.
pub const TYPED_LIMIT_MS: u64 = 300;

/// A redrawn row that changed twice within this many milliseconds, other
/// than as a key's answer, is volatile ("Volatile rows"), judged at each read.
pub const VOLATILE_MS: u64 = 2000;

/// A row that changed with no key within this many milliseconds before
/// the first of the keys is the program's, not theirs ("Keys and their
/// window").
pub const UNPROMPTED_MS: u64 = 2000;

/// The shortest quiet period ("Every time limit").
pub const QUIET_MS: u64 = 1000;

/// The quiet period after changes `interval_ms` apart ("Every time limit"): 1
/// second, or 1.5 times the interval between the last two changes when that
/// is longer.
#[must_use]
pub fn quiet_period(interval_ms: Option<u64>) -> u64 {
    interval_ms.map_or(QUIET_MS, |interval| {
        QUIET_MS.max(interval.saturating_mul(3) / 2)
    })
}

/// The reader's settings. Floods are Core's to split ("Core's flood
/// policy"), so the lines a flood speaks are not among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Settings {
    /// "Report new output": with it off, no output is spoken, and the whole
    /// document is never asked for ("The read"); keys are still answered.
    pub report_new_output: bool,
    /// "Review follows caret" ("The review anchor and following").
    pub review_follows_caret: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            report_new_output: true,
            review_follows_caret: true,
        }
    }
}

/// The unit a caret key Verbatim knows moves by.
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

/// What a key is, as far as the reading rules care.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyKind {
    /// A typed character, as text.
    Typed(String),
    /// A caret key Verbatim knows, with its unit.
    Caret(Unit),
    /// Enter: it forgets held typing, and its answer is only new output,
    /// never the caret's line ("The caret's line").
    Enter,
    /// A key that forgets held typing: Tab, Escape, Control+C, Control+D
    /// and Control+Break.
    Forget,
    /// Delete, which speaks what it leaves at the caret rather than what
    /// it removed ("The caret's line").
    Delete,
    /// Any other key, answered by what it did (the landing rule, text
    /// gained or lost).
    Other,
}

/// A key as it reached the program.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Key {
    /// What it is.
    pub kind: KeyKind,
    /// When it reached the program, in milliseconds on the caller's clock.
    pub at_ms: u64,
}

/// A stretch of one row's cells in one foreground and background, as the
/// read program's colours give it ("Colours").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ColourRun {
    /// Its first column, in cells.
    pub column: usize,
    /// Its length, in cells.
    pub length: usize,
    /// Its foreground, `0x00BBGGRR`.
    pub foreground: u32,
    /// Its background, `0x00BBGGRR`.
    pub background: u32,
}

/// Why a line is not spoken now, each the silence of a rule ("Reading rules").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A volatile row, held until it settles ("Volatile rows").
    Volatile,
    /// A growing line's unfinished last word, held until the line grows
    /// past it, the cursor leaves it, or the quiet period passes ("Growing lines").
    UnfinishedWord,
    /// A row a key changed away from the caret, held until the keys pause
    /// ("Rows the keys own").
    HeldForKeys,
    /// A spinner's symbol replaced alone ("Changed rows").
    Spinner,
    /// A row that only lost text ("Changed rows").
    LostText,
    /// A row erased and written back within the time a gone row lingers:
    /// text heard before, silent wherever it comes back ("Lingering"; a
    /// limit, "Limits").
    WrittenBack,
}

/// A line withheld, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withheld {
    /// The row the line starts on.
    pub row: usize,
    /// Why.
    pub reason: Reason,
}

/// Something to say. Rows are rows of the memory's last read
/// ([`TerminalMemory::rows`]), counted from its first row, each the first
/// row of the line spoken of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Speech {
    /// A line of new output, whole ("New text").
    Line {
        /// The line's first row.
        row: usize,
        /// Its text, sharing the read's string: a caller that keeps it,
        /// rather than only counting it, makes a string of it.
        text: Text,
    },
    /// What a growing line gained ("Growing lines").
    Growth {
        /// The line's first row.
        row: usize,
        /// The text gained.
        text: String,
    },
    /// A redrawn row's change, by the changed-word rule ("Changed rows").
    Changed {
        /// The line's first row.
        row: usize,
        /// The line from the word that changed.
        text: String,
    },
    /// A held line that the rotation carried above the read before it was
    /// spoken: what it held, as it was last read, spoken before the read's
    /// other output.
    Held {
        /// The held text: a growing line's unfinished word, or a volatile
        /// row from the word that changed.
        text: String,
    },
    /// A lost read ("A lost read"): the lines that follow went by while the
    /// reading lost track, and Core counts those it skips as this says.
    LostRead(Lost),
    /// Typed characters the terminal showed ("Typing echo").
    Echo {
        /// The caret's line's first row.
        row: usize,
        /// The characters shown.
        text: String,
    },
    /// A key's answer on the caret's line: the key's unit for a caret key
    /// Verbatim knows, or the landing rule's for any other ("The caret's line").
    Caret {
        /// The caret's line's first row.
        row: usize,
        /// The unit spoken.
        unit: Unit,
        /// The text at the caret in that unit; empty past the line's end.
        text: String,
    },
    /// Text the caret's line gained that the user did not type ("Keys and their window").
    Gained {
        /// The caret's line's first row.
        row: usize,
        /// The text gained.
        text: String,
    },
    /// Text the caret's line lost next to the caret ("Keys and their window").
    Lost {
        /// The caret's line's first row.
        row: usize,
        /// The text lost.
        text: String,
    },
    /// A row away from the caret that gained text or emphasis within a
    /// key's window: the key's answer, whole ("Rows the keys own").
    Answer {
        /// The line's first row.
        row: usize,
        /// Its text.
        text: String,
    },
    /// A suggestion after the caret in other colours ("Suggestions").
    Suggestion {
        /// The caret's line's first row.
        row: usize,
        /// The suggested text.
        text: String,
    },
}

/// What the reader made of one input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// What to say, in order.
    pub speech: Vec<Speech>,
    /// The new lines not spoken, and why.
    pub withheld: Vec<Withheld>,
    /// When to call [`TerminalReader::timer`], in milliseconds on the
    /// caller's clock; `None` when nothing waits for the quiet period.
    pub timer: Option<u64>,
    /// Read the whole document and give that read instead ("The read");
    /// nothing changed, and only the outcome's shape (the screen's width
    /// and height) is set.
    pub whole_document: bool,
    /// Read again from a screen above the screen, the screen counted up
    /// from the document's end by the outcome's height, with no start, and
    /// give that read instead: a first read with no rows above its screen
    /// ([`crate::terminal_rows::Outcome::history_block`]); nothing changed,
    /// and only the outcome's shape is set.
    pub history_block: bool,
    /// The alignment of the read, for a read.
    pub outcome: Option<Outcome>,
    /// Whether the read moved the review position to the caret because a
    /// key moved the caret within its window with "review follows caret"
    /// on ("The review anchor and following"). The first read after the focus arrived moves it
    /// there too, which the caller's focus does as well, so it is not
    /// told.
    pub followed: bool,
}

/// A read's colours by row, each row's when the read took them.
type RowColours<'a> = Option<&'a [Option<&'a Vec<ColourRun>>]>;

/// What a line waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
enum Hold {
    /// A volatile row's settling.
    Volatile,
    /// A growing line's unfinished word.
    Word,
    /// The keys' pause.
    Keys,
    /// A read with colours in the keys' window: the caret's line's text
    /// after the caret that was not typed, which a read without colours
    /// cannot tell from a suggestion ("Colours"); spoken as gained when
    /// the window ends without one.
    Colours(String),
}

/// A line's history ("The memory"), kept on its first row, or on the
/// first row kept of it once the rows above are trimmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct LineState {
    /// Its text as last read, shared with the memory's row when it is one
    /// row read whole.
    text: Text,
    /// What the user was last told of it, or knew of it when it was read
    /// silently: a growing line's spoken start while its word is held.
    /// Usually the same text as `text`, shared.
    said: Text,
    /// Whether it was rewritten in place other than by growth.
    redrawn: bool,
    /// Whether it is volatile.
    volatile: bool,
    /// When it last changed in a way that is spoken, other than as a key's
    /// answer, and the interval before that change.
    changed_ms: Option<u64>,
    interval_ms: Option<u64>,
    /// The read that change was found at.
    changed_read: Option<u64>,
    /// When it last changed outside a key's window.
    unprompted_ms: Option<u64>,
    /// When it last grew, and the interval before that growth.
    grew_ms: Option<u64>,
    grew_interval_ms: Option<u64>,
    /// What it waits for.
    hold: Option<Hold>,
    /// Whether a key changed it since it appeared: it holds input.
    input: bool,
    /// How much of `text`, in bytes, is above the read, or above the
    /// rows kept once they are trimmed: the start of a line the first row
    /// continues. Speech quotes only the rest.
    cut: usize,
}

impl LineState {
    fn read(text: &Text) -> Self {
        Self {
            text: text.clone(),
            said: text.clone(),
            ..Self::default()
        }
    }

    /// When what it waits for is due, given the keys' last two times.
    fn due(&self, keys: Option<(u64, Option<u64>)>) -> Option<u64> {
        match self.hold.as_ref()? {
            Hold::Volatile => self
                .changed_ms
                .map(|at| at + quiet_period(self.interval_ms)),
            Hold::Word => self
                .grew_ms
                .map(|at| at + quiet_period(self.grew_interval_ms)),
            Hold::Keys => keys.map(|(at, interval)| at + quiet_period(interval)),
            Hold::Colours(_) => keys.map(|(at, _)| at + KEY_WINDOW_MS),
        }
    }
}

/// A row's history beyond the memory's: its colours, and its line's when it
/// starts one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct RowState {
    hash: u64,
    /// The memory's row's text, shared.
    text: Text,
    colours: Option<Vec<ColourRun>>,
    line: Option<Box<LineState>>,
}

/// The caret as a read left it, its rows counted from the read's first.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Caret {
    /// Its row.
    row: usize,
    /// The first row of its line, or the first row kept of it.
    line: usize,
    /// Its line's text.
    text: String,
    /// Its place in the line, in characters.
    column: usize,
    /// Its line's text as the typing shows it: a suggestion after the
    /// caret blanked ("Suggestions").
    shown: String,
    /// Where the next character typed goes, in characters: the caret, but
    /// for a caret held on the last cell of a row the typing filled, its
    /// wrap pending, which the next character follows ("Typing echo").
    typing: usize,
}

/// The reader's view of one read: its rows from `skipped` on, the rows
/// the memory keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Screen {
    rows: Vec<RowState>,
    /// The first row of each row's line, an index of `rows`.
    starts: Vec<usize>,
    caret: Option<Caret>,
    size: (usize, usize),
    /// The screen's first row, an index of `rows`: rows above it are in
    /// the history, where no program redraws them.
    top: usize,
    /// How many of the read's rows lie above `rows`, trimmed with the
    /// memory's.
    skipped: usize,
}

impl Screen {
    /// The read's row `row`, when it is kept.
    fn row(&self, row: usize) -> Option<&RowState> {
        self.rows.get(row.checked_sub(self.skipped)?)
    }

    /// The first row of the line holding the read's row `row`, counted from
    /// the read's first row, when it is kept.
    fn start(&self, row: usize) -> Option<usize> {
        let start = self.starts.get(row.checked_sub(self.skipped)?)?;
        Some(start + self.skipped)
    }

    /// Drops the first `dropped` rows, as the memory trimmed them: a line
    /// that starts above the first row kept keeps its history on that row,
    /// the text above it as its cut.
    fn trim(&mut self, dropped: usize) {
        if dropped == 0 || dropped >= self.rows.len() {
            return;
        }
        let start = self.starts[dropped];
        if start < dropped {
            let above: usize = self.rows[start..dropped]
                .iter()
                .map(|row| row.text.len())
                .sum();
            if let Some(mut line) = self.rows[start].line.take() {
                line.cut = (line.cut + above).min(line.text.len());
                self.rows[dropped].line = Some(line);
            }
        }
        self.rows = self.rows.split_off(dropped);
        self.starts = self.starts[dropped..]
            .iter()
            .map(|start| start.saturating_sub(dropped))
            .collect();
        self.top = self.top.saturating_sub(dropped);
        self.skipped += dropped;
        if let Some(caret) = &mut self.caret {
            caret.line = caret.line.max(self.skipped);
        }
    }
}

/// The keys whose window is open.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Keys {
    /// When the first and the latest reached the program.
    first_ms: u64,
    last_ms: u64,
    /// The keys, oldest first.
    kinds: Vec<KeyKind>,
    /// The caret's line before them, mapped through each read: a caret back
    /// there, on its line unchanged, is where the keys left it, whatever
    /// the program did with its cursor as it drew in between ("Keys and their window").
    origin: Option<Caret>,
    /// Whether a read in their window moved the review position to the
    /// caret.
    followed: bool,
    /// Typing not yet shown, with when each reached the program.
    typed: Vec<(String, u64)>,
}

/// Who owns a change a read found ("Keys and their window").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    /// The program's, judged by the rules of output.
    Program,
    /// The keys': the caret's line by the caret step, any other line by
    /// "Rows the keys own".
    Keys,
}

/// How the caret moved from the last read's caret, mapped through the
/// alignment, to the read's: found once for each read ("The caret's
/// line").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    /// No caret, or the last read's caret not found: nothing to compare.
    Unknown,
    /// On the same row, at the same column.
    Still,
    /// Along the same line.
    Along,
    /// Onto another line that was already there.
    Onto,
    /// Onto a new line.
    New,
}

impl Move {
    /// The move from `before` to `caret`, the caret's line new when every
    /// row of it with text is new.
    fn of(caret: Option<&Caret>, before: Option<&Caret>, outcome: &Outcome, rows: &[Row]) -> Self {
        let Some(caret) = caret else {
            return Self::Unknown;
        };
        if let Some(before) = before
            && before.line == caret.line
        {
            return if before.row == caret.row && before.typing == caret.column {
                Self::Still
            } else {
                Self::Along
            };
        }
        let line_rows = caret.line
            ..rows.len().min(
                outcome
                    .lines
                    .iter()
                    .find(|line| line.rows.start == caret.line)
                    .map_or(caret.line + 1, |line| line.rows.end),
            );
        let fresh = outcome.rows[line_rows.clone()]
            .iter()
            .zip(&rows[line_rows])
            .all(|(kind, row)| row.is_blank() || *kind == NewRow::New);
        if fresh { Self::New } else { Self::Onto }
    }
}

/// What the caret step answered.
#[derive(Default)]
struct CaretAnswer {
    /// The caret's line, when the step spoke of it or found it the keys';
    /// its change is then not output.
    line: Option<usize>,
    /// From a read without colours, the text after the caret that was not
    /// typed, which waits for a read with them ("Colours").
    held: Option<String>,
    /// Whether the caret is back where the keys started, on its line
    /// unchanged: the keys moved nothing.
    back: bool,
}

/// A line found gone, kept while it lingers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Gone {
    hash: u64,
    line: Box<LineState>,
    since_ms: u64,
}

/// The reading rules for one terminal ("Reading rules").
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TerminalReader {
    memory: TerminalMemory,
    settings: Settings,
    /// Stateless, so made afresh when a reader is restored.
    #[cfg_attr(feature = "serde", serde(skip))]
    segmenter: Segmenter,
    screen: Screen,
    lingering: Vec<Gone>,
    keys: Option<Keys>,
    /// The last key's time and the interval before it.
    last_key: Option<(u64, Option<u64>)>,
    focused: bool,
    /// The next read speaks nothing: the first, or the first after the
    /// focus arrived.
    fresh: bool,
    reads: u64,
    suggestion: Option<String>,
}

impl TerminalReader {
    /// A reader with `settings`, before the terminal's first read. It
    /// starts with the focus.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self {
            memory: TerminalMemory::new(),
            settings,
            segmenter: Segmenter::new(),
            screen: Screen::default(),
            lingering: Vec::new(),
            keys: None,
            last_key: None,
            focused: true,
            fresh: true,
            reads: 0,
            suggestion: None,
        }
    }

    /// The row memory, for the review commands.
    #[must_use]
    pub fn memory(&self) -> &TerminalMemory {
        &self.memory
    }

    /// The row memory, for the review commands to move the review
    /// position and set marks.
    pub fn memory_mut(&mut self) -> &mut TerminalMemory {
        &mut self.memory
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// Changes the settings. Turning "report new output" off drops what
    /// output held.
    pub fn set_settings(&mut self, settings: Settings) -> Step {
        self.settings = settings;
        if !settings.report_new_output {
            self.drop_holds();
        }
        self.idle()
    }

    /// The focus arrived at the terminal at `at_ms` ("The focus"): the
    /// next read speaks nothing and puts the review cursor at the caret.
    pub fn focus(&mut self, at_ms: u64) -> Step {
        let _ = at_ms;
        self.focused = true;
        self.fresh = true;
        self.keys = None;
        self.drop_holds();
        self.idle()
    }

    /// The focus left the terminal: output written meanwhile is not heard
    /// on return ("The focus").
    pub fn blur(&mut self) -> Step {
        self.focused = false;
        self.keys = None;
        self.drop_holds();
        self.idle()
    }

    /// A key reached the program.
    pub fn key(&mut self, key: Key) -> Step {
        let interval = self.last_key.map(|(at, _)| key.at_ms.saturating_sub(at));
        self.last_key = Some((key.at_ms, interval));
        let open = self
            .keys
            .as_ref()
            .is_some_and(|keys| key.at_ms <= keys.last_ms + KEY_WINDOW_MS);
        if !open {
            self.keys = Some(Keys {
                first_ms: key.at_ms,
                last_ms: key.at_ms,
                kinds: Vec::new(),
                origin: self.screen.caret.clone(),
                followed: false,
                typed: Vec::new(),
            });
        }
        if let Some(keys) = &mut self.keys {
            keys.last_ms = key.at_ms;
            match &key.kind {
                KeyKind::Typed(text) => keys.typed.push((text.clone(), key.at_ms)),
                KeyKind::Enter | KeyKind::Forget => keys.typed.clear(),
                KeyKind::Caret(_) | KeyKind::Delete | KeyKind::Other => {}
            }
            keys.kinds.push(key.kind);
        }
        self.idle()
    }

    /// The timer came due at `now_ms`: what waited for the quiet period
    /// and has had it is spoken.
    pub fn timer(&mut self, now_ms: u64) -> Step {
        let mut step = Step::default();
        self.release(now_ms, &mut step.speech);
        step.timer = self.next_due();
        step
    }

    /// A step with nothing to say, and the timer as it now stands.
    fn idle(&self) -> Step {
        Step {
            timer: self.next_due(),
            ..Step::default()
        }
    }

    fn drop_holds(&mut self) {
        for row in &mut self.screen.rows {
            if let Some(line) = &mut row.line {
                line.hold = None;
                line.volatile = false;
                line.said.clone_from(&line.text);
            }
        }
    }

    /// Who owns a change a read taken at `now` found in a line with
    /// `state` ("Keys and their window"): the keys, when the read lies
    /// within 300 milliseconds after the latest of them and the line did
    /// not change with no key in the 2 seconds before the first of them;
    /// the program otherwise. With no line, whose the read's time makes it.
    fn owner(&self, now: u64, state: Option<&LineState>) -> Owner {
        let Some(keys) = &self.keys else {
            return Owner::Program;
        };
        let window = now >= keys.last_ms && now <= keys.last_ms + KEY_WINDOW_MS;
        let unprompted = state
            .and_then(|state| state.unprompted_ms)
            .is_some_and(|at| at >= keys.first_ms.saturating_sub(UNPROMPTED_MS));
        if window && !unprompted {
            Owner::Keys
        } else {
            Owner::Program
        }
    }

    /// When the earliest hold is due.
    fn next_due(&self) -> Option<u64> {
        self.screen
            .rows
            .iter()
            .filter_map(|row| row.line.as_ref()?.due(self.last_key))
            .min()
    }

    /// Speaks every hold due at `now`, and every hold on a line that has
    /// scrolled above the screen, where no program redraws it and the next
    /// read does not reach: a volatile row or a row the keys changed by the
    /// changed-word rule against what it last said, a growing line's held
    /// word as growth.
    fn release(&mut self, now: u64, speech: &mut Vec<Speech>) {
        let keys = self.last_key;
        let (top, skipped) = (self.screen.top, self.screen.skipped);
        for index in 0..self.screen.rows.len() {
            let Some(line) = &self.screen.rows[index].line else {
                continue;
            };
            let Some(due) = line.due(keys) else {
                continue;
            };
            let end = (index + 1..self.screen.rows.len())
                .find(|&row| self.screen.starts[row] != index)
                .unwrap_or(self.screen.rows.len());
            if due > now && end > top {
                continue;
            }
            let (text, said, hold, cut) = (
                line.text.clone(),
                line.said.clone(),
                line.hold.clone(),
                line.cut,
            );
            match hold {
                Some(Hold::Word) => {
                    if let Some(rest) = text.strip_prefix(&*said)
                        && !rest.is_empty()
                    {
                        speech.push(Speech::Growth {
                            row: index + skipped,
                            text: text[(text.len() - rest.len()).max(cut)..].to_owned(),
                        });
                    }
                }
                Some(Hold::Volatile | Hold::Keys) => {
                    if let Some(start) = self.changed_start(&[&*said], &text) {
                        speech.push(Speech::Changed {
                            row: index + skipped,
                            text: text[start.max(cut)..].to_owned(),
                        });
                    }
                }
                Some(Hold::Colours(after)) => speech.push(Speech::Gained {
                    row: index + skipped,
                    text: after,
                }),
                None => {}
            }
            if let Some(line) = &mut self.screen.rows[index].line {
                line.hold = None;
                line.volatile = false;
                line.said = text;
            }
        }
    }

    /// What `line` holds, as [`TerminalReader::release`] would speak it: a
    /// growing line's unfinished word, or a volatile row, or a row the keys
    /// changed, from the word that changed; `None` when it holds nothing.
    fn held_text(&self, line: &LineState) -> Option<String> {
        let text = &line.text;
        let start = match line.hold.as_ref()? {
            Hold::Colours(held) => return Some(held.clone()),
            Hold::Word => text
                .strip_prefix(&*line.said)
                .filter(|rest| !rest.is_empty())
                .map(|rest| text.len() - rest.len())?,
            Hold::Volatile | Hold::Keys => self.changed_start(&[&*line.said], text)?,
        };
        Some(text[start.max(line.cut)..].to_owned())
    }

    /// Where the changed-word rule speaks `new` from ("Changed rows"): the
    /// start of the word where it first differs, by whole graphemes, from
    /// the earliest of `before`; `None` when it differs from none of them
    /// or only lost text at its end.
    fn changed_start(&self, before: &[&str], new: &str) -> Option<usize> {
        let first = before
            .iter()
            .filter(|old| **old != new)
            .map(|old| Difference::new(old, new).start)
            .min()?;
        self.word_start(new, first)
    }

    /// The start of the word holding byte `first` of `new`, where it first
    /// differs from what it is compared with ("Changed rows"); `None` when
    /// that is its end.
    fn word_start(&self, new: &str, first: usize) -> Option<usize> {
        if first >= new.len() {
            return None;
        }
        let words = self.segmenter.words(new, WordRules::for_text(new, None));
        let word = words
            .iter()
            .find(|word| word.contains(&first))
            .cloned()
            .unwrap_or(first..new.len());
        Some(word.start)
    }

    /// One read of the terminal, with whether it waited for the speech
    /// queue, and the screen's colour runs, row by row from the screen's
    /// top to the document's end, when the read took them (every read in a
    /// key's window, but one taken before Core's request for them came).
    pub fn read(&mut self, read: &Read, waited: bool, colours: Option<&[Vec<ColourRun>]>) -> Step {
        // The memory is taken out while the read is handled, so its rows are
        // read in place rather than copied.
        let mut memory = std::mem::take(&mut self.memory);
        let step = self.handle(&mut memory, read, waited, colours);
        self.memory = memory;
        step
    }

    /// One read, against `memory`, as [`TerminalReader::read`].
    #[expect(
        clippy::too_many_lines,
        reason = "one read's rules in the design's order, each a short step"
    )]
    fn handle(
        &mut self,
        memory: &mut TerminalMemory,
        read: &Read,
        waited: bool,
        colours: Option<&[Vec<ColourRun>]>,
    ) -> Step {
        let now = read.at_ms;
        let mut outcome = memory.read(read);
        if outcome.history_block {
            return Step {
                history_block: true,
                timer: self.next_due(),
                outcome: Some(outcome),
                ..Step::default()
            };
        }
        if outcome.blank {
            // A read whose screen shows no text changes nothing ("A read with
            // no text").
            return Step {
                timer: self.next_due(),
                outcome: Some(outcome),
                ..Step::default()
            };
        }
        if outcome.whole_document {
            if self.settings.report_new_output {
                return Step {
                    whole_document: true,
                    timer: self.next_due(),
                    outcome: Some(outcome),
                    ..Step::default()
                };
            }
            // With nothing reported nothing waits, so the whole document is
            // not read: the memory starts again from this read ("The read").
            self.screen = Screen::default();
            self.lingering.clear();
            outcome = memory.restart(read);
        }
        self.reads += 1;
        let previous = std::mem::take(&mut self.screen);
        self.lingering
            .retain(|gone| now.saturating_sub(gone.since_ms) <= LINGERING_MS);
        for (from, fate) in outcome.old.iter().enumerate() {
            if *fate == (OldRow::Gone { lingering: true })
                && let Some(row) = previous.row(from)
                && let Some(line) = row.line.clone()
            {
                self.lingering.push(Gone {
                    hash: row.hash,
                    line,
                    since_ms: now,
                });
            }
        }
        // The read's rows, all of them until the read is handled.
        let rows: &[Row] = memory.rows().kept();
        // A line above the rows kept once the read is handled, holding
        // nothing, needs no history beyond this read.
        let keep_from = memory.first_kept();
        let mut screen = Screen {
            rows: outcome
                .rows
                .iter()
                .enumerate()
                .map(|(to, kind)| RowState {
                    hash: rows[to].hash,
                    text: rows[to].text.clone(),
                    colours: match kind {
                        NewRow::Kept { from, .. }
                        | NewRow::Grown { from }
                        | NewRow::Rewritten { from, .. } => {
                            previous.row(*from).and_then(|row| row.colours.clone())
                        }
                        _ => None,
                    },
                    line: None,
                })
                .collect(),
            starts: vec![0; rows.len()],
            caret: None,
            size: (outcome.width, outcome.height),
            top: rows.len().saturating_sub(outcome.height),
            skipped: 0,
        };
        for line in &outcome.lines {
            for row in line.rows.clone() {
                screen.starts[row] = line.rows.start;
            }
        }
        let mut caret = outcome.caret.and_then(|row| {
            let line = outcome.lines.iter().find(|line| line.rows.contains(&row))?;
            let before: usize = rows[line.rows.start..row]
                .iter()
                .map(|row| row.text.chars().count())
                .sum();
            let text = line.text.to_string();
            // Without a suggestion after the caret, which is not the line's
            // text ("Suggestions").
            let shown = self
                .suggestion
                .as_ref()
                .and_then(|suggested| text.strip_suffix(suggested.as_str()))
                .map_or_else(|| text.clone(), |shown| shown.trim_end().to_owned());
            let column = before + read.caret_column;
            Some(Caret {
                row,
                line: line.rows.start,
                text,
                column,
                shown,
                typing: column,
            })
        });
        let new_colours: Option<Vec<Option<&Vec<ColourRun>>>> = colours.map(|colours| {
            let skip = rows.len().saturating_sub(colours.len());
            (0..rows.len())
                .map(|row| row.checked_sub(skip).and_then(|at| colours.get(at)))
                .collect()
        });
        let resized = previous.size != (0, 0) && previous.size != screen.size;
        let silent = self.fresh || !self.focused || outcome.first || resized;
        let report = self.settings.report_new_output && !silent;
        let mut step = Step::default();
        // Whether the read lies in the keys' window: its changes are theirs
        // unless a line's own history says otherwise.
        let window = self.owner(now, None) == Owner::Keys;
        // The last read's caret, mapped through the alignment: when the
        // caret's row went but its line's first row stayed, as when a
        // wrapped line is erased, the caret was on that line, at its column.
        let mapped = |caret: &Caret| match (outcome.old.get(caret.row), outcome.old.get(caret.line))
        {
            (Some(OldRow::Kept(to) | OldRow::Rewritten(to)), _)
            | (_, Some(OldRow::Kept(to) | OldRow::Rewritten(to))) => Some(Caret {
                row: *to,
                line: screen.starts[*to],
                ..caret.clone()
            }),
            _ => None,
        };
        let before = previous.caret.as_ref().and_then(mapped);
        if let Some(keys) = &mut self.keys {
            // Typing not shown within its limit is dropped unspoken.
            keys.typed
                .retain(|(_, at)| now <= at.saturating_add(TYPED_LIMIT_MS));
            keys.origin = keys.origin.as_ref().and_then(mapped);
        }
        let movement = Move::of(caret.as_ref(), before.as_ref(), &outcome, rows);
        // Every read in the keys' window has the colours, but one the
        // outpost took before Core's request for them came ("Colours"):
        // that read leaves the emphasis, the suggestion and text after the
        // caret that was not typed to the next read, which has them.
        let window_colours = window && colours.is_some();
        let mut answered: Option<usize> = None;
        // The caret's line's text after the caret held for a read with
        // colours.
        let mut colours_held: Option<String> = None;
        let mut key_speech: Vec<Speech> = Vec::new();
        // Where the review position goes, set once the read is handled.
        let mut reviews: Vec<(usize, usize)> = Vec::new();
        if window && !silent {
            let answer = self.caret_step(
                (caret.as_mut(), before.as_ref(), movement),
                (rows, new_colours.as_deref()),
                &mut key_speech,
            );
            (answered, colours_held) = (answer.line, answer.held);
            if let Some(caret) = &caret {
                if answer.back {
                    // A review position that followed the program's cursor
                    // while it drew comes back with the caret.
                    if self.keys.as_ref().is_some_and(|keys| keys.followed) {
                        step.followed = true;
                        reviews.push((caret.row, read.caret_column));
                    }
                } else if self.settings.review_follows_caret && movement != Move::Still {
                    step.followed = true;
                    reviews.push((caret.row, read.caret_column));
                    if let Some(keys) = &mut self.keys {
                        keys.followed = true;
                    }
                }
            }
        }
        screen.caret.clone_from(&caret);
        if self.fresh
            && let Some(caret) = &caret
        {
            // The focus moves the review cursor to the caret ("The review anchor and following").
            reviews.push((caret.row, read.caret_column));
        }
        // The lines, top to bottom.
        let mut items: Vec<(usize, Speech)> = Vec::new();
        // New lines, each with its text, or none when it is blank.
        let mut fresh_lines: Vec<(usize, Option<Text>)> = Vec::new();
        let mut emphasis_candidates: Vec<usize> = Vec::new();
        let old_caret = previous.caret.clone();
        // The old lines whose history a line of the read carries.
        let mut carried: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for line in &outcome.lines {
            let start = line.rows.start;
            let kinds = &outcome.rows[line.rows.clone()];
            // A line the read's first row continues keeps its start, above
            // the read, from the line its first row was in.
            // A read from the document's start has nothing above it: its
            // first row was split off its line, or the line's start left a
            // full history, and either way it is a line of its own.
            let continued = outcome.continued && start == 0 && !read.at_document_start;
            let cut_text = match (continued, kinds.first()) {
                (
                    true,
                    Some(
                        NewRow::Kept { from, .. }
                        | NewRow::Grown { from }
                        | NewRow::Rewritten { from, .. },
                    ),
                ) => above_read(&previous, *from),
                _ => String::new(),
            };
            let cut = cut_text.len();
            let joined: Text;
            let text: &Text = if cut_text.is_empty() {
                &line.text
            } else {
                joined = Text::from(format!("{cut_text}{}", line.text));
                &joined
            };
            // Whether the line's history is kept once the read is handled
            // ([`put`]), so a line above the rows kept that holds nothing
            // needs none built.
            let kept = line.rows.end > keep_from;
            let part = |at: usize| text[at.max(cut)..].to_owned();
            let first = line
                .rows
                .clone()
                .find(|&row| !rows[row].is_blank())
                .unwrap_or(start);
            // A line's history carries from the old line its first row with
            // text started, or that the read's first row continues; a row
            // split off a line starts a line of its own.
            let base = match outcome.rows[first] {
                NewRow::Kept { from, .. }
                | NewRow::Grown { from }
                | NewRow::Rewritten { from, .. } => previous
                    .start(from)
                    .filter(|&old| old == from || (continued && first == start))
                    .and_then(|old| {
                        carried.insert(old);
                        previous.row(old)
                    })
                    .and_then(|row| row.line.as_deref().cloned()),
                NewRow::Returned { .. } => self
                    .lingering
                    .iter()
                    .position(|gone| gone.hash == rows[first].hash)
                    .map(|at| *self.lingering.remove(at).line),
                _ => None,
            };
            // A row rewritten alone, away from its neighbours, keeps what it
            // last said and starts its change history afresh ("The
            // alignment").
            let base = match outcome.rows[first] {
                NewRow::Rewritten { alone: true, .. } => base.map(|old| LineState {
                    text: old.text,
                    said: old.said,
                    input: old.input,
                    cut: old.cut,
                    ..LineState::default()
                }),
                _ => base,
            };
            // The text of the old line a row split off a line was in.
            let split = match outcome.rows[first] {
                NewRow::Kept { from, .. } if base.is_none() => previous
                    .start(from)
                    .and_then(|old| previous.row(old))
                    .and_then(|row| row.line.as_ref())
                    .map(|line| line.text.clone()),
                _ => None,
            };
            let changed = line.rows.clone().zip(kinds).any(|(row, kind)| {
                !rows[row].is_blank()
                    && matches!(
                        kind,
                        NewRow::New | NewRow::Grown { .. } | NewRow::Rewritten { .. }
                    )
            });
            // A line with a lingering row taken back is text heard before
            // ("Lingering"): its silence, when nothing new is in it or it
            // only lost text, is that of text written back.
            let written_back = kinds
                .iter()
                .any(|kind| matches!(kind, NewRow::Returned { .. }));
            let lost_reason = if written_back {
                Reason::WrittenBack
            } else {
                Reason::LostText
            };
            if answered == Some(start) {
                put(
                    &mut screen,
                    &line.rows,
                    keep_from,
                    LineState {
                        input: true,
                        cut,
                        hold: colours_held.take().map(Hold::Colours),
                        ..base.map_or_else(
                            || LineState::read(text),
                            |mut base| {
                                base.text.clone_from(text);
                                base.said.clone_from(text);
                                base
                            },
                        )
                    },
                );
                continue;
            }
            if line.text.is_empty() {
                if kinds.iter().all(|kind| *kind == NewRow::New) {
                    fresh_lines.push((start, None));
                }
                if kept {
                    put(&mut screen, &line.rows, keep_from, LineState::default());
                }
                continue;
            }
            if silent || !report {
                // Nothing waits, so only a kept line needs its history.
                if kept {
                    let mut state = base.unwrap_or_default();
                    state.cut = cut;
                    state.text.clone_from(text);
                    state.said.clone_from(text);
                    state.hold = None;
                    state.volatile = false;
                    put(&mut screen, &line.rows, keep_from, state);
                }
                continue;
            }
            let Some(mut state) = base.map(|mut base| {
                base.cut = cut;
                base
            }) else {
                // New output, unless it is the input the shell reprinted
                // ("New text").
                let reprint = caret.as_ref().is_some_and(|caret| caret.line == start)
                    && self.reprinted(text, old_caret.as_ref(), &previous, &outcome, start);
                if reprint {
                    // The input the shell reprinted says nothing; its history
                    // records it as input below.
                } else if changed {
                    fresh_lines.push((start, Some(text.clone())));
                } else if written_back {
                    step.withheld.push(Withheld {
                        row: start,
                        reason: lost_reason,
                    });
                } else if let Some(old) = split.filter(|old| old != text) {
                    // Kept rows split off a line: silent when that line held
                    // their text, spoken as a change otherwise.
                    if Difference::new(&old, text).lost {
                        step.withheld.push(Withheld {
                            row: start,
                            reason: Reason::LostText,
                        });
                    } else {
                        items.push((
                            start,
                            Speech::Changed {
                                row: start,
                                text: part(0),
                            },
                        ));
                    }
                }
                // A new line waits for nothing, so only a kept one needs its
                // history: its text, and whether it is the input reprinted.
                if kept {
                    let mut state = LineState::read(text);
                    state.input = reprint;
                    put(&mut screen, &line.rows, keep_from, state);
                }
                continue;
            };
            let old_text = state.text.clone();
            if !changed {
                // Kept rows are silent, wherever they moved; a line that
                // only lost text is silent too, and rows joined into a
                // line no read held speak it.
                if old_text != *text {
                    if Difference::new(&old_text, text).lost {
                        step.withheld.push(Withheld {
                            row: start,
                            reason: lost_reason,
                        });
                        drop_word(&mut state);
                    } else {
                        items.push((
                            start,
                            Speech::Changed {
                                row: start,
                                text: part(0),
                            },
                        ));
                        state.said.clone_from(text);
                    }
                    state.text.clone_from(text);
                } else if window_colours && caret.as_ref().is_none_or(|caret| caret.line != start) {
                    emphasis_candidates.push(start);
                }
                // The cursor leaving a growing line speaks its held word.
                if state.hold == Some(Hold::Word)
                    && caret.as_ref().is_none_or(|caret| caret.line != start)
                {
                    if let Some(rest) = text.strip_prefix(&*state.said)
                        && !rest.is_empty()
                    {
                        items.push((
                            start,
                            Speech::Growth {
                                row: start,
                                text: part(text.len() - rest.len()),
                            },
                        ));
                    }
                    state.said.clone_from(text);
                    state.hold = None;
                }
                put(&mut screen, &line.rows, keep_from, state);
                continue;
            }
            state.text.clone_from(text);
            if !window {
                state.unprompted_ms = Some(now);
            }
            // Growth is spoken from what was said when the line as last read
            // was all heard, or held back only by its unfinished word; a line
            // that lost text or was held for another reason since is spoken
            // by the changed-word rule below.
            let heard = state.said == old_text || state.hold == Some(Hold::Word);
            if line.grew_from.is_some() && heard && text.starts_with(&*state.said) {
                // A growing line speaks what it gained, holding back an
                // unfinished last word ("Growing lines").
                state.grew_interval_ms = state.grew_ms.map(|at| now.saturating_sub(at));
                state.grew_ms = Some(now);
                if state.hold == Some(Hold::Volatile) || state.hold == Some(Hold::Keys) {
                    state.hold = None;
                }
                let unfinished = caret.as_ref().is_some_and(|caret| {
                    caret.line == start
                        && caret.column == text.chars().count()
                        && !text.ends_with(is_space)
                });
                let end = if unfinished {
                    text.rfind(is_space).map_or(0, |at| {
                        at + text[at..].chars().next().map_or(1, char::len_utf8)
                    })
                } else {
                    text.len()
                };
                let said = state.said.len();
                // Spaces alone wait with the word after them.
                let end = if text[said.min(end)..end].trim().is_empty() {
                    said.min(end)
                } else {
                    end
                };
                if end > said {
                    items.push((
                        start,
                        Speech::Growth {
                            row: start,
                            text: text[said.max(cut).min(end)..end].to_owned(),
                        },
                    ));
                    state.said = if end == text.len() {
                        text.clone()
                    } else {
                        text.prefix(end)
                    };
                }
                if end < text.len() {
                    step.withheld.push(Withheld {
                        row: start,
                        reason: Reason::UnfinishedWord,
                    });
                    state.hold = Some(Hold::Word);
                } else {
                    state.hold = None;
                }
                put(&mut screen, &line.rows, keep_from, state);
                continue;
            }
            // A redrawn row ("Changed rows"), or a row a key changed away from
            // the caret ("Rows the keys own"), compared once.
            let difference = Difference::new(&old_text, text);
            state.redrawn = true;
            let caret_line = caret.as_ref().is_some_and(|caret| caret.line == start);
            // "Rows the keys own".
            if !caret_line && self.owner(now, Some(&state)) == Owner::Keys {
                if difference.gained {
                    key_speech.push(Speech::Answer {
                        row: start,
                        text: part(0),
                    });
                    state.said.clone_from(text);
                    state.hold = None;
                } else if difference.lost {
                    step.withheld.push(Withheld {
                        row: start,
                        reason: lost_reason,
                    });
                    drop_word(&mut state);
                } else {
                    step.withheld.push(Withheld {
                        row: start,
                        reason: Reason::HeldForKeys,
                    });
                    state.hold = Some(Hold::Keys);
                }
                put(&mut screen, &line.rows, keep_from, state);
                continue;
            }
            if difference.lost {
                step.withheld.push(Withheld {
                    row: start,
                    reason: lost_reason,
                });
                drop_word(&mut state);
                put(&mut screen, &line.rows, keep_from, state);
                continue;
            }
            if difference.spinner() {
                step.withheld.push(Withheld {
                    row: start,
                    reason: Reason::Spinner,
                });
                if state.hold.is_none() {
                    state.said.clone_from(text);
                }
                put(&mut screen, &line.rows, keep_from, state);
                continue;
            }
            let interval = state.changed_ms.map(|at| now.saturating_sub(at));
            let volatile = state.volatile
                || interval.is_some_and(|interval| interval <= VOLATILE_MS)
                || (waited && state.changed_read == Some(self.reads - 1));
            state.interval_ms = interval;
            state.changed_ms = Some(now);
            state.changed_read = Some(self.reads);
            if volatile {
                state.volatile = true;
                state.hold = Some(Hold::Volatile);
                step.withheld.push(Withheld {
                    row: start,
                    reason: Reason::Volatile,
                });
            } else {
                // From where it first differs from what it last said or
                // from its last read, whichever is earlier.
                let from_read = (old_text != *text).then_some(difference.start);
                let from_said = (*state.said != **text).then(|| {
                    if state.said == old_text {
                        difference.start
                    } else {
                        Difference::new(&state.said, text).start
                    }
                });
                let first = from_read.into_iter().chain(from_said).min();
                if let Some(at) = first.and_then(|first| self.word_start(text, first)) {
                    items.push((
                        start,
                        Speech::Changed {
                            row: start,
                            text: part(at),
                        },
                    ));
                }
                state.said.clone_from(text);
                state.hold = None;
            }
            put(&mut screen, &line.rows, keep_from, state);
        }
        // Gained emphasis: a row whose colours changed while its text did
        // not ("Rows the keys own").
        if window_colours
            && !silent
            && let Some(new_colours) = &new_colours
            && let Some(row) = Self::emphasis(&emphasis_candidates, &screen, new_colours)
            && let Some(line) = &mut screen.rows[row].line
        {
            key_speech.push(Speech::Answer {
                row,
                text: line.text.to_string(),
            });
            line.said.clone_from(&line.text);
        }
        if let Some(new_colours) = &new_colours {
            for (row, colours) in new_colours.iter().enumerate() {
                if let Some(colours) = colours {
                    screen.rows[row].colours = Some((*colours).clone());
                }
            }
        }
        // New output, every line in order, blank ones included, for Core's
        // flood policy, which counts them ("New text"); a blank row is a
        // line only once a row with text is below it. After a lost read,
        // the lines follow its mark, which a read that reports nothing, as
        // the focus's, does not give ("The focus").
        while fresh_lines.last().is_some_and(|(_, text)| text.is_none()) {
            fresh_lines.pop();
        }
        if report && let Some(lost) = outcome.lost {
            items.push((0, Speech::LostRead(lost)));
        }
        items.reserve(fresh_lines.len());
        // A blank line's text, the same empty string for each.
        let blank = Text::default();
        for (row, text) in fresh_lines {
            let text = text.unwrap_or_else(|| blank.clone());
            items.push((row, Speech::Line { row, text }));
        }
        // Mostly in order already: sorted only when they are not, which a
        // stable sort would leave as they are.
        if !items.is_sorted_by_key(|(row, _)| *row) {
            items.sort_by_key(|(row, _)| *row);
        }
        if resized && !outcome.first {
            // Resizing speaks nothing but the caret's line if it changed;
            // the memory finds the review position again ("Starting again").
            if let Some(caret) = &caret
                && old_caret.as_ref().is_none_or(|old| old.text != caret.text)
                && self.focused
            {
                key_speech.push(Speech::Caret {
                    row: caret.line,
                    unit: Unit::Line,
                    text: caret.text.clone(),
                });
            }
        }
        // A held line the rotation carried above the read's start, with no
        // line of the read carrying it, is spoken as it was last read,
        // before the read's output. What an overflow pushed out while held
        // is not heard ("Limits").
        let held: Vec<Speech> = if report {
            previous
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| (index + previous.skipped, row))
                .filter(|&(old, _)| previous.start(old) == Some(old) && !carried.contains(&old))
                .filter(|&(old, _)| {
                    matches!(outcome.old.get(old), Some(OldRow::Above | OldRow::Left))
                })
                .filter_map(|(_, row)| row.line.as_ref())
                .filter_map(|line| self.held_text(line))
                .map(|text| Speech::Held { text })
                .collect()
        } else {
            Vec::new()
        };
        self.fresh = false;
        self.screen = screen;
        // The read's speech in the items' own space: the answers to keys
        // and the held lines go before them.
        step.speech = items.into_iter().map(|(_, speech)| speech).collect();
        step.speech.splice(0..0, key_speech.into_iter().chain(held));
        if report {
            self.release(now, &mut step.speech);
        }
        if !window
            && let Some(keys) = &self.keys
            && now > keys.last_ms + KEY_WINDOW_MS
        {
            self.keys = None;
        }
        for (row, column) in reviews {
            memory.set_review(row, column);
        }
        // Of the read, only what "The memory" needs is kept.
        let dropped = memory.trim();
        self.screen.trim(dropped);
        step.timer = self.next_due();
        step.outcome = Some(outcome);
        step
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the cases of the caret's line rule in order, each short"
    )]
    /// The caret step ("The caret's line"), the one place the keys' kinds
    /// are read: the caret's line when the keys own the read, compared with
    /// the last read's caret `before` by the read's `movement`. The caret
    /// back where the keys started, on its line unchanged, says nothing;
    /// after Enter nothing either, its answer being only new output; on a
    /// new line, the line is output; on another line already there, that
    /// line is spoken, or the unit of a character or word key; along its
    /// line, typing echoed, the key's unit or the landing rule, text gained
    /// or lost, or the line changed some other way. Sets the caret's view
    /// of its line as the typing shows it.
    fn caret_step(
        &mut self,
        (caret, before, movement): (Option<&mut Caret>, Option<&Caret>, Move),
        (rows, colours): (&[Row], RowColours<'_>),
        speech: &mut Vec<Speech>,
    ) -> CaretAnswer {
        let (Some(caret), Some(keys)) = (caret, self.keys.as_ref()) else {
            return CaretAnswer::default();
        };
        // The caret back where the keys found it, on its line unchanged,
        // from another line the last read left it on: the keys moved
        // nothing, and the program's cursor moves while it drew are not
        // spoken. A line the last read showed with other text under the
        // caret changed, and is answered against that read.
        if let Some(origin) = &keys.origin
            && caret.row == origin.row
            && caret.column == origin.typing
            && caret.text == origin.shown
            && before.is_none_or(|before| before.line != caret.line || before.shown == caret.text)
        {
            return CaretAnswer {
                back: true,
                ..CaretAnswer::default()
            };
        }
        // In a terminal Enter's answer is only new output, never the caret's
        // line, as NVDA turns off its new-line announcement in terminals
        // (`docs/parity/terminals.md`, "Typing"): what Enter changed is read
        // as output, and a line the caret moved onto, such as the screen's
        // first row a clear homes it to before it erases, says nothing.
        if keys.kinds.last() == Some(&KeyKind::Enter) {
            return CaretAnswer::default();
        }
        let unit = keys.kinds.iter().rev().find_map(|kind| match kind {
            KeyKind::Caret(unit) => Some(*unit),
            _ => None,
        });
        let line_key = keys.kinds.contains(&KeyKind::Caret(Unit::Line));
        let deleted = keys.kinds.last() == Some(&KeyKind::Delete);
        // A line key with a character typed at once after it: the recalled
        // line's change is spoken once with the character in it, "echo
        // onex", not the whole line ("Keys and their window").
        let typed_after = keys
            .kinds
            .iter()
            .rposition(|kind| *kind == KeyKind::Caret(Unit::Line))
            .is_some_and(|at| {
                keys.kinds[at + 1..]
                    .iter()
                    .any(|kind| matches!(kind, KeyKind::Typed(_)))
            });
        // Everything typed since the keys began, which a suggestion follows.
        let typed_since: String = keys
            .kinds
            .iter()
            .filter_map(|kind| match kind {
                KeyKind::Typed(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // Typing not yet shown.
        let typed: String = keys.typed.iter().map(|(text, _)| text.as_str()).collect();
        // A suggestion after the caret in other colours than the typing; in
        // a read without colours, the last suggestion where it still shows,
        // less what was typed along it ("Colours").
        let suggestion = match colours {
            Some(colours) => suggestion(caret, rows, colours),
            None => self.carried_suggestion(caret, &typed_since),
        };
        // The line as the typing shows it: the suggestion blanked where it
        // is, from the caret, whatever follows it.
        let mut shown = blanked(&caret.text, caret.column, suggestion.as_deref());
        let mut answer = None;
        let mut held = None;
        // Where the next character typed goes: the caret, but for a caret
        // held on the last cell of a row the typing filled, its wrap
        // pending, which the next character follows.
        let mut column = caret.column;
        if movement == Move::New {
            // A new line at the caret is output (Enter, a new prompt).
            return CaretAnswer::default();
        }
        if !matches!(movement, Move::Still | Move::Along) {
            // The caret ended on another line that was already there: that
            // line is spoken whatever changed on it ("The caret's line").
            if before.is_some() || unit.is_some() {
                let unit = unit.filter(|unit| *unit != Unit::Line && !line_key);
                speech.push(self.unit_at(caret, unit.unwrap_or(Unit::Line)));
                answer = Some(caret.line);
            }
        } else if let Some(before) = before {
            let (mut at, mut compared) = compare(before, &shown, caret.column, &typed);
            // Without colours, text after the caret on its row that was not
            // typed may be a suggestion: when the line with it blanked is
            // the typing or a move, it waits for a read with colours
            // ("Colours").
            if colours.is_none() && !compared.clean() {
                let row_end = rows[caret.line..=caret.row]
                    .iter()
                    .map(|row| row.text.chars().count())
                    .sum();
                if let Some((cut, after)) = cut_after(&shown, caret.column, row_end) {
                    let (cut_at, cut_compared) = compare(before, &cut, caret.column, &typed);
                    if cut_compared.clean() {
                        (shown, at, compared, held) = (cut, cut_at, cut_compared, Some(after));
                        answer = Some(caret.line);
                    }
                }
            }
            column = at;
            match compared {
                Compared::Unchanged => {}
                Compared::Moved => {
                    let text = match unit {
                        Some(unit) => self.unit_at(caret, unit),
                        None => self.landing(caret, before),
                    };
                    speech.push(text);
                    answer = Some(caret.line);
                }
                Compared::Echo { echoed, rest } => {
                    if !echoed.is_empty() {
                        speech.push(Speech::Echo {
                            row: caret.line,
                            text: echoed.clone(),
                        });
                    }
                    if !rest.trim().is_empty() {
                        speech.push(Speech::Gained {
                            row: caret.line,
                            text: rest.trim().to_owned(),
                        });
                    }
                    if let Some(keys) = &mut self.keys {
                        let mut count = echoed.chars().count();
                        keys.typed.retain(|(text, _)| {
                            let take = count >= text.chars().count();
                            if take {
                                count -= text.chars().count();
                            }
                            !take
                        });
                    }
                    answer = Some(caret.line);
                }
                Compared::Other(difference) => {
                    if let Some(keys) = &mut self.keys {
                        keys.typed.clear();
                    }
                    let (prefix, removed, inserted) = (
                        difference.start_chars(),
                        difference.removed(),
                        difference.inserted(),
                    );
                    if line_key && !typed_after {
                        speech.push(self.unit_at(caret, Unit::Line));
                    } else if removed.is_empty() && !inserted.trim().is_empty() {
                        speech.push(Speech::Gained {
                            row: caret.line,
                            text: inserted.trim().to_owned(),
                        });
                    } else if inserted.is_empty()
                        && prefix == caret.column
                        && !removed.trim().is_empty()
                    {
                        // Delete speaks what it leaves at the caret.
                        if deleted {
                            speech.push(self.unit_at(caret, Unit::Character));
                        } else {
                            speech.push(Speech::Lost {
                                row: caret.line,
                                text: removed.trim().to_owned(),
                            });
                        }
                    } else if let Some(start) = self.changed_start(&[&before.shown], &shown) {
                        speech.push(Speech::Changed {
                            row: caret.line,
                            text: shown[start..].to_owned(),
                        });
                    }
                    answer = Some(caret.line);
                }
            }
        }
        caret.shown = shown;
        caret.typing = column;
        if let Some(suggested) = suggestion {
            // A suggestion carried through a read without colours is the
            // last one still showing: nothing to say, nothing to keep.
            if colours.is_some() {
                let repeated = self.suggestion.as_ref().is_some_and(|last| {
                    *last == suggested
                        || last.strip_prefix(typed_since.as_str()) == Some(suggested.as_str())
                });
                if !repeated && !suggested.is_empty() {
                    speech.push(Speech::Suggestion {
                        row: caret.line,
                        text: suggested.clone(),
                    });
                }
                self.suggestion = Some(suggested);
            }
            answer = Some(caret.line);
        } else if colours.is_some() {
            self.suggestion = None;
        }
        CaretAnswer {
            line: answer,
            held,
            back: false,
        }
    }

    /// The last suggestion as it still shows after the caret in a read
    /// without colours, less what was typed along it: its end that follows
    /// the caret, the rest of it the end of `typed` ("Colours").
    fn carried_suggestion(&self, caret: &Caret, typed: &str) -> Option<String> {
        let last = self.suggestion.as_deref()?;
        let after: String = caret.text.chars().skip(caret.column).collect();
        last.char_indices()
            .map(|(at, _)| at)
            .find(|&at| typed.ends_with(&last[..at]) && after.starts_with(&last[at..]))
            .map(|at| last[at..].to_owned())
    }

    /// The caret's line spoken in `unit` at the caret.
    fn unit_at(&self, caret: &Caret, unit: Unit) -> Speech {
        let text = match unit {
            Unit::Line => caret.text.clone(),
            Unit::Character => character_at(&caret.text, caret.column),
            Unit::Word => self.word_at(&caret.text, caret.column),
        };
        Speech::Caret {
            row: caret.line,
            unit,
            text,
        }
    }

    /// The word at character `column` of `text`, or empty past its end.
    fn word_at(&self, text: &str, column: usize) -> String {
        let Some(offset) = byte_offset(text, column) else {
            return String::new();
        };
        self.segmenter
            .word_at(text, offset, WordRules::for_text(text, None))
            .map(|word| text[word].to_owned())
            .unwrap_or_default()
    }

    /// The landing rule ("The caret's line"): one character moved, that
    /// character; the start or end of the line, the character there; the
    /// start or end of a word, the word there; otherwise the character.
    fn landing(&self, caret: &Caret, before: &Caret) -> Speech {
        let length = caret.text.chars().count();
        let column = caret.column;
        let moved = column.abs_diff(before.typing);
        let unit = if moved == 1 || column == 0 || column >= length {
            Unit::Character
        } else {
            let chars: Vec<char> = caret.text.chars().collect();
            let word = |c: char| !is_space(c);
            let at_start = word(chars[column]) && !word(chars[column - 1]);
            let at_end = word(chars[column - 1]) && !word(chars[column]);
            if at_start || at_end {
                Unit::Word
            } else {
                Unit::Character
            }
        };
        let text = match unit {
            Unit::Word => {
                let chars: Vec<char> = caret.text.chars().collect();
                if column < length && !is_space(chars[column]) {
                    self.word_at(&caret.text, column)
                } else {
                    self.word_at(&caret.text, column - 1)
                }
            }
            _ => character_at(&caret.text, column),
        };
        Speech::Caret {
            row: caret.line,
            unit,
            text,
        }
    }

    /// Whether the new line at the caret, `text` starting at row `start`,
    /// is the shell reprinting the input below new rows ("Reprinted input"): it
    /// equals the caret's line as last read, with any typing since, that
    /// line held input and is still above, and new rows lie between.
    fn reprinted(
        &self,
        text: &str,
        old: Option<&Caret>,
        previous: &Screen,
        outcome: &Outcome,
        start: usize,
    ) -> bool {
        let Some(old) = old else {
            return false;
        };
        let held = previous
            .row(old.line)
            .and_then(|row| row.line.as_ref())
            .is_some_and(|line| line.input);
        let above = match outcome.old.get(old.line) {
            Some(OldRow::Kept(to)) if *to < start => *to,
            _ => return false,
        };
        let between = outcome.rows[above + 1..start].contains(&NewRow::New);
        let typed: String = self
            .keys
            .as_ref()
            .map(|keys| keys.typed.iter().map(|(text, _)| text.as_str()).collect())
            .unwrap_or_default();
        let with_typing = {
            let mut line = old.text.clone();
            line.extend(std::iter::repeat_n(
                ' ',
                old.column.saturating_sub(old.text.chars().count()),
            ));
            let at = byte_offset(&line, old.column).unwrap_or(line.len());
            line.insert_str(at, &typed);
            line.trim_end().to_owned()
        };
        held && between && (text == old.text || text == with_typing)
    }

    /// The row that gained emphasis among `candidates`, lines whose text
    /// did not change ("Rows the keys own"): of those whose colours changed,
    /// or were read for the first time, the one whose colours now differ
    /// from both its neighbours'.
    fn emphasis(
        candidates: &[usize],
        screen: &Screen,
        colours: &[Option<&Vec<ColourRun>>],
    ) -> Option<usize> {
        let now = |row: usize| colours.get(row).copied().flatten();
        let stands_out = |row: usize| {
            let Some(own) = now(row) else {
                return false;
            };
            row > 0
                && now(row - 1).is_some_and(|above| above != own)
                && now(row + 1).is_some_and(|below| below != own)
        };
        let mut picked = candidates.iter().copied().filter(|&row| {
            now(row).is_some_and(|new| {
                screen.rows[row]
                    .colours
                    .as_ref()
                    .is_none_or(|old| old != new)
            }) && stands_out(row)
        });
        match (picked.next(), picked.next()) {
            (Some(row), None) => Some(row),
            _ => None,
        }
    }
}

/// The start of the line holding row `from` of `previous`, counted from
/// its read's first row, up to that row: the part of a line above a read
/// whose first row continues it.
fn above_read(previous: &Screen, from: usize) -> String {
    let Some(start) = previous.start(from) else {
        return String::new();
    };
    let own: String = (start..from)
        .filter_map(|row| previous.row(row))
        .map(|row| &*row.text)
        .collect();
    let earlier = previous
        .row(start)
        .and_then(|row| row.line.as_ref())
        .and_then(|line| line.text.get(..line.cut))
        .unwrap_or_default();
    format!("{earlier}{own}")
}

/// Puts `state` on the first of `rows`, unless the line is above the rows
/// kept once the read is handled, `keep_from` on, and holds nothing: its
/// history would be dropped unused.
fn put(screen: &mut Screen, rows: &std::ops::Range<usize>, keep_from: usize, state: LineState) {
    if rows.end > keep_from || state.hold.is_some() {
        screen.rows[rows.start].line = Some(Box::new(state));
    }
}

/// A line that lost text no longer holds its unfinished word.
fn drop_word(state: &mut LineState) {
    if state.hold == Some(Hold::Word) {
        state.hold = None;
    }
}

/// How a line's text changed from `old` to `new`, computed once for a
/// rewritten line ("Changed rows", "The caret's line"): the start they
/// share, by whole graphemes; the end they share after it; what `old` had
/// between and what `new` has there; and, compared without spaces, whether
/// `new` gained text other than at its end and whether it only lost text.
struct Difference<'a> {
    old: Cow<'a, str>,
    new: Cow<'a, str>,
    /// The bytes the two share at their start, by whole graphemes.
    start: usize,
    /// The bytes they share at their end, after `start`, by whole
    /// graphemes.
    end: usize,
    /// Compared without spaces, `old` is a part of `new` in order and
    /// `new` adds more, not only after it: "[x]" for "[ ]", a marker before
    /// an entry ("Rows the keys own").
    gained: bool,
    /// Compared without spaces, `old` contains `new` and is longer: `new`
    /// only lost text ("New text").
    lost: bool,
}

impl<'a> Difference<'a> {
    /// The difference from `old` to `new`.
    fn new(old: &'a str, new: &'a str) -> Self {
        Self::of(Cow::Borrowed(old), Cow::Borrowed(new))
    }

    /// The difference between the caret's line before and after keys, each
    /// with spaces after it up to its caret's column, so a caret past its
    /// end has its place.
    fn at_caret((old, old_column): (&str, usize), (new, new_column): (&str, usize)) -> Self {
        let padded = |text: &str, column: usize| {
            let length = text.chars().count();
            let mut padded = text.to_owned();
            padded.extend(std::iter::repeat_n(' ', column.saturating_sub(length)));
            padded
        };
        Self::of(
            Cow::Owned(padded(old, old_column)),
            Cow::Owned(padded(new, new_column)),
        )
    }

    fn of(old: Cow<'a, str>, new: Cow<'a, str>) -> Self {
        let (old_graphemes, new_graphemes) = (graphemes(&old), graphemes(&new));
        let shared = old_graphemes
            .iter()
            .zip(&new_graphemes)
            .take_while(|(a, b)| old[(*a).clone()] == new[(*b).clone()])
            .count();
        let start = new_graphemes
            .get(..shared)
            .and_then(<[_]>::last)
            .map_or(0, |last| last.end);
        let longest = old_graphemes.len().min(new_graphemes.len()) - shared;
        let end_graphemes = old_graphemes
            .iter()
            .rev()
            .zip(new_graphemes.iter().rev())
            .take(longest)
            .take_while(|(a, b)| old[(*a).clone()] == new[(*b).clone()])
            .count();
        let end = new_graphemes[new_graphemes.len() - end_graphemes..]
            .first()
            .map_or(0, |first| new.len() - first.start);
        let (old_squeezed, new_squeezed) = (squeezed(&old), squeezed(&new));
        let gained = new_squeezed.len() > old_squeezed.len()
            && !new_squeezed.starts_with(&old_squeezed)
            && {
                let mut rest = new_squeezed.chars();
                old_squeezed.chars().all(|c| rest.any(|n| n == c))
            };
        let lost = old_squeezed.len() > new_squeezed.len() && old_squeezed.contains(&new_squeezed);
        Self {
            old,
            new,
            start,
            end,
            gained,
            lost,
        }
    }

    /// What `old` had between the shared start and end.
    fn removed(&self) -> &str {
        &self.old[self.start..self.old.len() - self.end]
    }

    /// What `new` has there.
    fn inserted(&self) -> &str {
        &self.new[self.start..self.new.len() - self.end]
    }

    /// The characters the two share at their start.
    fn start_chars(&self) -> usize {
        self.new[..self.start].chars().count()
    }

    /// Whether exactly one character changed, neither a letter nor a
    /// digit: a spinner's step ("Changed rows").
    fn spinner(&self) -> bool {
        let mut removed = self.removed().chars();
        let mut inserted = self.inserted().chars();
        matches!(
            (removed.next(), removed.next(), inserted.next(), inserted.next()),
            (Some(a), None, Some(b), None) if !a.is_alphanumeric() && !b.is_alphanumeric()
        )
    }

    /// The typing shown ("Typing echo"): `new` is `old` with the start of
    /// `typed` written at character `from`, inserted or over text that
    /// showed it already, and the caret moved to `to` past it. Returns the
    /// typing shown and any text gained after it.
    fn typed(&self, from: usize, to: usize, typed: &str) -> Option<(String, String)> {
        let (old, new) = (&*self.old, &*self.new);
        if typed.is_empty() || to <= from {
            return None;
        }
        let count = to - from;
        let typed: Vec<char> = typed.chars().collect();
        let old_chars: Vec<char> = old.chars().collect();
        let new_chars: Vec<char> = new.chars().collect();
        let shown: String = typed.iter().take(count).collect();
        if shown.chars().count() != count {
            // The caret moved past more than the typing: what follows it is
            // text gained.
            let all: String = typed.iter().collect();
            let mut expected: Vec<char> = old_chars[..from.min(old_chars.len())].to_vec();
            expected.extend(all.chars());
            if new_chars.len() >= expected.len() && new_chars[..expected.len()] == expected[..] {
                let rest: String = new_chars[expected.len()..to.min(new_chars.len())]
                    .iter()
                    .collect();
                let after: String = new_chars[to.min(new_chars.len())..].iter().collect();
                let old_after: String = old_chars[from.min(old_chars.len())..].iter().collect();
                if after.trim_end() == old_after.trim_end() {
                    return Some((all, rest));
                }
            }
            return None;
        }
        let head: String = old_chars[..from.min(old_chars.len())].iter().collect();
        let tail: String = old_chars[from.min(old_chars.len())..].iter().collect();
        let inserted = format!("{head}{shown}{tail}");
        let over: String = {
            let skip: String = tail.chars().skip(count).collect();
            format!("{head}{shown}{skip}")
        };
        (new.trim_end() == inserted.trim_end() || new.trim_end() == over.trim_end())
            .then(|| (shown, String::new()))
    }
}

/// How the caret's line, as the typing shows it, compares with the line
/// before the keys ("The caret's line").
enum Compared {
    /// No text changed and the caret did not move.
    Unchanged,
    /// No text changed and the caret moved along the line.
    Moved,
    /// The typing shown, and any text gained after it.
    Echo { echoed: String, rest: String },
    /// The line changed some other way, and how.
    Other(Difference<'static>),
}

impl Compared {
    /// Whether the line is the typing or a move alone.
    fn clean(&self) -> bool {
        match self {
            Self::Unchanged | Self::Moved => true,
            Self::Echo { rest, .. } => rest.trim().is_empty(),
            Self::Other(_) => false,
        }
    }
}

/// `shown`, the caret's line as the typing shows it with the caret at
/// `column`, against `before`, with `typed` not yet shown; and where the
/// next character typed goes: the caret, but for a caret held on the last
/// cell of a row the typing filled, its wrap pending, which the next
/// character follows ("Typing echo", typing past the margin).
fn compare(before: &Caret, shown: &str, column: usize, typed: &str) -> (usize, Compared) {
    let difference = Difference::at_caret((&before.shown, before.typing), (shown, column));
    let (old, new) = (&*difference.old, &*difference.new);
    let mut at = column;
    // At the right margin the terminal writes the character at the caret
    // and keeps the caret there, its wrap pending: the typing went in at
    // the caret.
    if column == before.typing
        && !typed.is_empty()
        && shown.chars().count() == column + 1
        && before.shown.chars().count() <= column
    {
        at = column + 1;
    }
    // Typing that moved the caret over text already shown is matched as
    // typing, below.
    let typing_over = at > before.typing && !typed.is_empty();
    let compared = if old.trim_end() == new.trim_end() && !typing_over {
        if column == before.typing {
            Compared::Unchanged
        } else {
            Compared::Moved
        }
    } else if let Some((echoed, rest)) = difference.typed(before.typing, at, typed) {
        Compared::Echo { echoed, rest }
    } else {
        Compared::Other(difference)
    };
    (at, compared)
}

/// `text` with `suggestion`, when there is one, blanked from character
/// `column` on, whatever follows it, and no spaces at its end.
fn blanked(text: &str, column: usize, suggestion: Option<&str>) -> String {
    let Some(suggested) = suggestion else {
        return text.to_owned();
    };
    let chars: Vec<char> = text.chars().collect();
    let end = (column + suggested.chars().count()).min(chars.len());
    let mut shown: String = chars[..column.min(chars.len())].iter().collect();
    shown.extend(std::iter::repeat_n(' ', end.saturating_sub(column)));
    shown.extend(&chars[end..]);
    shown.trim_end().to_owned()
}

/// `text` with its characters from `column` to `end` blanked, and those
/// characters without the spaces around them; `None` when they are all
/// spaces.
fn cut_after(text: &str, column: usize, end: usize) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let end = end.min(chars.len());
    let after: String = chars.get(column..end)?.iter().collect();
    let after = after.trim();
    if after.is_empty() {
        return None;
    }
    let mut cut: String = chars[..column].iter().collect();
    cut.extend(std::iter::repeat_n(' ', end - column));
    cut.extend(&chars[end..]);
    Some((cut.trim_end().to_owned(), after.to_owned()))
}

/// The byte offset of character `column` of `text`, or `None` past its
/// end.
fn byte_offset(text: &str, column: usize) -> Option<usize> {
    text.char_indices().nth(column).map(|(at, _)| at)
}

/// The character at `column` of `text`, or empty past its end.
fn character_at(text: &str, column: usize) -> String {
    byte_offset(text, column)
        .and_then(|at| crate::grapheme_at(text, at))
        .map(|range| text[range].to_owned())
        .unwrap_or_default()
}

/// The suggestion on the caret's row: its text after the caret in colours
/// other than the cell before the caret's, the typing's ("Suggestions").
fn suggestion(caret: &Caret, rows: &[Row], colours: &[Option<&Vec<ColourRun>>]) -> Option<String> {
    let runs = colours.get(caret.row).copied().flatten()?;
    let row = &rows[caret.row];
    let column = caret.column
        - (caret.column.min(
            rows[caret.line..caret.row]
                .iter()
                .map(|row| row.text.chars().count())
                .sum::<usize>(),
        ));
    if column == 0 {
        return None;
    }
    let at = |cell: usize| {
        runs.iter()
            .find(|run| cell >= run.column && cell < run.column + run.length)
            .map(|run| (run.foreground, run.background))
    };
    let typing = at(column - 1)?;
    let chars: Vec<char> = row.text.chars().collect();
    let end = (column..chars.len())
        .take_while(|&cell| at(cell).is_some_and(|colour| colour != typing))
        .last()?;
    let text: String = chars[column..=end].iter().collect();
    let text = text.trim_end().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests;
