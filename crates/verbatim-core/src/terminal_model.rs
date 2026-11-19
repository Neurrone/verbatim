//! Core's model of each terminal it reads (`books/dev/src/explanation/design/terminal-model.md`,
//! "Where it runs"): the reading rules, with their memory of the terminal's
//! rows, run here in the reducer, and the outpost only reads what Core asks
//! for and sends each read back ([`TerminalReadReply`]).
//!
//! For each terminal node it reads, Core keeps the rules
//! ([`TerminalReader`], behind an `Arc` so that a checkpoint's clone of the
//! state shares it), the keys it took in and has not yet given the rules,
//! the start anchor its reads count from, the scope the rules owe, what the
//! last read showed of the caret and the screen's shape, and when the
//! quiet-period timer is due. Which reads Core asks, and when, is the
//! focused terminal's reading's ([`crate::terminal::reading`]).
//!
//! - Keys. Every key Core takes in while it reads the focused terminal is
//!   kept, in the order Core took it in, with when it reached the program
//!   and its trace. Before a read is given to the rules, they are given
//!   every kept key whose time is at or before the read's end, in order,
//!   stopping at the first later one ([`read_at`]): the terminal answered
//!   the read at some moment before then, so a key whose effect it shows
//!   reached the program no later. Before the timer at T, likewise up to
//!   T. The arrival of a message decides nothing.
//! - A read's speech key: once the keys up to its time are given, the
//!   newest key given, when its window (`KEY_WINDOW_MS` after it) holds
//!   the read's time. The read's speech is spoken under that key's trace
//!   ([`Effect::ForKey`]).
//! - The start (`docs/design/readers-tell-core-asks-build3.md`, Table 7):
//!   every read names the start anchor of the last read the memory took
//!   whose screen showed text, which Core holds; a read whose start is no
//!   longer that one when it answers is given to the rules as reading from
//!   another start, so its rows are found by their text.
//! - What is owed (Table 4): when the rules ask for the whole document, or
//!   a first read again with its history block, the next full read sent
//!   takes that scope.
//! - Identical reads: a read identical to the last one given, but for its
//!   times, number, start and caret, is not given to the rules, unless it
//!   waited for Core's speech queue or is the first after the focus
//!   arrived (Dickson, 2026-10-26).
//!
//! A terminal's model is made with its first read.

pub(crate) mod review;

use std::collections::VecDeque;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    Effect, NodeId, OutpostId, TerminalColourRun, TerminalReadReply, TerminalScope,
    TerminalSnapshot, TerminalText, TextAnchor, TraceId,
};
use verbatim_text::terminal_reading::{
    ColourRun, KEY_WINDOW_MS, Key, KeyKind, QUIET_MS, Settings, Speech, Step, TerminalReader, Unit,
};
use verbatim_text::terminal_rows::{Anchor, Lost, MARKS, NewRow, Read, ReadText};

use crate::state::SrState;
use crate::terminal::{Said, SaidItem, Skipped, TerminalItem, TerminalKeyKind, TerminalUnit};

pub(crate) use review::CaretAt;

/// How many terminals Core keeps a model of, the most recently read: a
/// terminal's memory lives while its node does.
const MODELS: usize = 16;

/// How many keys wait for a read at most; past it the oldest is given to
/// the rules at once, so the state stays bounded however long no read
/// comes.
const PENDING_KEYS: usize = 256;

/// The text anchors Core mints for a terminal's review answers have this
/// bit set; an outpost's never do.
pub(crate) const TERMINAL_ANCHORS: u64 = 1 << 63;

/// A key Core took in and has not yet given the reading rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingKey {
    kind: TerminalKeyKind,
    /// When it reached the program, on the shared clock.
    at_ms: u64,
    trace: TraceId,
}

/// A command mark's line as it was when the mark was set: the text of its
/// first row, which a search above the memory looks for, and how many rows
/// it held ("Features and settings").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MarkLine {
    pub(crate) hash: u64,
    pub(crate) text: String,
    pub(crate) rows: u32,
}

/// Rows of the history above the memory, read for a review answer and
/// placed among the rows of the memory's last read when they came
/// ([`review::placed_history`]): the row where the first of them is, the
/// rows, and whether they start at the document's start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HistoryRead {
    pub(crate) first: isize,
    pub(crate) rows: Vec<verbatim_text::terminal_rows::Row>,
    pub(crate) reached: bool,
}

/// The last full read the model took: when it ended, whether it took the
/// screen's colours, and how many full reads had been sent when it was
/// taken, so that colours are owed only while none has been sent since
/// (`docs/design/readers-tell-core-asks-build3.md`, "Colours").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LastRead {
    pub(crate) ended_ms: u64,
    pub(crate) coloured: bool,
    pub(crate) sends: u64,
}

/// Core's model of one terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalModel {
    pub(crate) node: NodeId,
    /// The reading rules and their memory, shared with checkpoints.
    pub(crate) reader: Arc<TerminalReader>,
    /// The keys not yet given to the rules, in the order Core took them in.
    pending: VecDeque<PendingKey>,
    /// The newest key given to the rules: its trace and when it reached the
    /// program.
    last_key: Option<(TraceId, u64)>,
    /// Which model made for its node this is: a model forgotten or
    /// evicted and made again for the same node has a newer one, so the
    /// speech of the older one's reads is said by neither.
    pub(crate) generation: u64,
    /// The start anchor the next read names: the start of the last read
    /// the memory took whose screen showed text, or of the first
    /// (Table 7).
    pub(crate) start: Option<TextAnchor>,
    /// The scope the rules asked for, which the next full read sent takes
    /// (Table 4); `None` for a fresh read.
    pub(crate) owed: Option<TerminalScope>,
    /// The last read taken, for the colours owed.
    pub(crate) last_read: Option<LastRead>,
    /// How many full reads of the terminal Core has sent.
    pub(crate) sends: u64,
    /// When the newest read taken began, in microseconds on the shared
    /// clock: a change observed before it is covered by it.
    pub(crate) started_us: u64,
    /// A hash of the last read given to the rules, its times, number,
    /// start and caret left out.
    given: Option<u64>,
    /// Whether the next read is the first after the focus arrived, which
    /// is given to the rules even when identical to the last.
    pub(crate) focus_arrived: bool,
    /// The lines of the command marks set, newest last, at most twice
    /// [`MARKS`], for the marks the memory and the aside screen keep.
    pub(crate) mark_lines: VecDeque<MarkLine>,
    /// The mark set at a command's Enter and when the quiet period after
    /// the key ends: until a read shows that mark kept, or the period ends,
    /// a new row starting a line with its line's hash is marked, as where
    /// Pi and Claude Code print the input they cleared.
    pending_mark: Option<(Anchor, u64)>,
    /// The places the review answers minted, each a text anchor's number
    /// and the memory's place for it, oldest first.
    pub(crate) places: VecDeque<(u64, u64)>,
    /// The caret as the last read found it.
    pub(crate) caret: Option<CaretAt>,
    /// Whether the last read started at the document's start.
    pub(crate) at_document_start: bool,
    /// The screen's height in rows and width in cells as the last read
    /// found them.
    pub(crate) height: u32,
    pub(crate) width: usize,
    /// When the rules' quiet-period timer is due.
    due_ms: Option<u64>,
}

/// Core's models of the terminals it reads, the most recently read last,
/// at most [`MODELS`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Terminals {
    models: Vec<TerminalModel>,
    /// The generation the next model made is given.
    next_generation: u64,
}

impl std::ops::Deref for Terminals {
    type Target = [TerminalModel];

    fn deref(&self) -> &Self::Target {
        &self.models
    }
}

impl std::ops::DerefMut for Terminals {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.models
    }
}

impl Terminals {
    /// Makes a model of terminal `node`, the most recently read, with the
    /// next generation, and returns it.
    pub(crate) fn add(&mut self, node: NodeId, settings: Settings) -> &mut TerminalModel {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.models
            .push_mut(TerminalModel::new(node, settings, generation))
    }

    /// Makes the model at `at`, which is one of them, the most recently
    /// read, and returns it.
    fn move_to_back(&mut self, at: usize) -> &mut TerminalModel {
        let model = self.models.remove(at);
        self.models.push_mut(model)
    }

    /// Removes every model `which` picks.
    pub(crate) fn remove_where(&mut self, which: impl Fn(&TerminalModel) -> bool) {
        self.models.retain(|model| !which(model));
    }
}

impl TerminalModel {
    fn new(node: NodeId, settings: Settings, generation: u64) -> Self {
        Self {
            node,
            reader: Arc::new(TerminalReader::new(settings)),
            pending: VecDeque::new(),
            last_key: None,
            generation,
            start: None,
            owed: None,
            last_read: None,
            sends: 0,
            started_us: 0,
            given: None,
            focus_arrived: true,
            mark_lines: VecDeque::new(),
            pending_mark: None,
            places: VecDeque::new(),
            caret: None,
            at_document_start: false,
            height: 0,
            width: 0,
            due_ms: None,
        }
    }

    /// The rules, to change: copied first when a checkpoint shares them.
    pub(crate) fn rules(&mut self) -> &mut TerminalReader {
        Arc::make_mut(&mut self.reader)
    }

    /// Gives the rules every kept key that reached the program at or before
    /// `up_to_ms`, in order.
    fn give_keys(&mut self, up_to_ms: u64) {
        while self
            .pending
            .front()
            .is_some_and(|key| key.at_ms <= up_to_ms)
        {
            if let Some(key) = self.pending.pop_front() {
                self.give(&key);
            }
        }
    }

    fn give(&mut self, key: &PendingKey) {
        if key.kind == TerminalKeyKind::Enter && self.reader.caret_line_input() {
            self.mark_caret_line(key.at_ms);
        }
        let step = self.rules().key(rules_key(&key.kind, key.at_ms));
        self.due_ms = step.timer;
        self.last_key = Some((key.trace, key.at_ms));
    }

    /// When the newest key Core took in reached the program, given to the
    /// rules or not.
    pub(crate) fn newest_key_ms(&self) -> Option<u64> {
        self.pending
            .back()
            .map(|key| key.at_ms)
            .or_else(|| self.last_key.map(|(_, at_ms)| at_ms))
    }

    /// Whether the colours are owed: the last read taken ended at or after
    /// the newest key's time, within its window, without the colours, and
    /// no full read has been sent since it was taken
    /// (`docs/design/readers-tell-core-asks-build3.md`, "Colours").
    pub(crate) fn colours_owed(&self) -> bool {
        let (Some(last), Some(key_ms)) = (self.last_read, self.newest_key_ms()) else {
            return false;
        };
        !last.coloured
            && last.sends == self.sends
            && last.ended_ms >= key_ms
            && last.ended_ms <= key_ms.saturating_add(KEY_WINDOW_MS)
    }

    /// Whether a full read sent at `now_ms` takes the screen's colours:
    /// within a key's window of the newest key's time, or when they are
    /// owed.
    pub(crate) fn takes_colours(&self, now_ms: u64) -> bool {
        self.colours_owed()
            || self
                .newest_key_ms()
                .is_some_and(|key_ms| now_ms <= key_ms.saturating_add(KEY_WINDOW_MS))
    }

    /// Enter at `at_ms` on the caret's line, which holds input: the line is
    /// marked at its first row, unless it is already, and noted until the
    /// quiet period after the key ends, so that the read that shows the
    /// input printed elsewhere marks it there ("Features and settings").
    fn mark_caret_line(&mut self, at_ms: u64) {
        let Some(caret) = self.caret else {
            return;
        };
        let Some(mark) = self.rules().memory_mut().add_mark(caret.row) else {
            return;
        };
        self.keep_mark_line(mark);
        self.pending_mark = Some((mark, at_ms.saturating_add(QUIET_MS)));
    }

    /// Keeps the line of `mark`, just set on a row the memory keeps.
    fn keep_mark_line(&mut self, mark: Anchor) {
        let rows = self.reader.memory().rows();
        let Some(first) = usize::try_from(mark.row).ok().and_then(|row| rows.get(row)) else {
            return;
        };
        let mut count: u32 = 1;
        let mut row = usize::try_from(mark.row).unwrap_or(0);
        while rows.get(row).is_some_and(|found| found.wraps) && row.saturating_add(1) < rows.len() {
            row = row.saturating_add(1);
            count = count.saturating_add(1);
        }
        let line = MarkLine {
            hash: mark.line_hash,
            text: first.text.to_string(),
            rows: count,
        };
        self.mark_lines.retain(|kept| kept.hash != line.hash);
        if self.mark_lines.len() >= 2 * MARKS {
            self.mark_lines.pop_front();
        }
        self.mark_lines.push_back(line);
    }

    /// After a read at `at_ms` that `rows` describes, the command entered
    /// last: forgotten once the read is past the quiet period after its
    /// Enter; else, unless the mark set at the Enter survived the read, as
    /// when the line it was entered on did, marked on the new row starting
    /// a line with its hash nearest the caret. An older mark on a line with
    /// the same text does not count.
    fn settle_mark(&mut self, rows: &[NewRow], at_ms: u64) {
        let Some((entered, until_ms)) = self.pending_mark else {
            return;
        };
        if at_ms > until_ms {
            self.pending_mark = None;
            return;
        }
        let hash = entered.line_hash;
        let memory = self.reader.memory();
        // The memory follows the mark set last, the one set at the Enter
        // unless Core set another since, which ends the pending mark.
        if memory
            .newest_mark()
            .is_some_and(|newest| newest.line_hash == hash)
        {
            self.pending_mark = None;
            return;
        }
        let kept = memory.rows();
        let caret = self.caret.map_or(kept.len(), |caret| caret.row);
        let found = (0..rows.len())
            .filter(|&row| {
                rows.get(row) == Some(&NewRow::New)
                    && kept.get(row).is_some_and(|found| found.line_hash == hash)
                    && (row == 0
                        || kept
                            .get(row.saturating_sub(1))
                            .is_some_and(|above| !above.wraps))
            })
            .min_by_key(|&row| row.abs_diff(caret));
        if let Some(row) = found {
            if let Some(mark) = self.rules().memory_mut().add_mark(row) {
                self.keep_mark_line(mark);
            }
            self.pending_mark = None;
        }
    }

    /// The key a read taken at `at_ms` ([`read_at`]) is spoken under: the
    /// newest key given, when its window holds the read.
    fn speech_key(&self, at_ms: u64) -> Option<TraceId> {
        self.last_key
            .filter(|(_, key_ms)| key_ms.saturating_add(KEY_WINDOW_MS) >= at_ms)
            .map(|(trace, _)| trace)
    }

    /// The focus arrived at the terminal at `at_ms`: the rules are told,
    /// and its next read is given to them whatever it shows.
    pub(crate) fn focused(&mut self, at_ms: u64) {
        let step = self.rules().focus(at_ms);
        self.due_ms = step.timer;
        self.focus_arrived = true;
    }

    /// The focus left the terminal: the rules are told, and its waiting
    /// keys dropped.
    pub(crate) fn blurred(&mut self) {
        let step = self.rules().blur();
        self.due_ms = step.timer;
        self.pending.clear();
        self.last_key = None;
    }
}

/// The reading rules' settings from the reader settings.
fn settings_of(state: &SrState) -> Settings {
    Settings {
        report_new_output: state.settings.report_terminal_output,
        review_follows_caret: state.settings.follow_caret,
    }
}

/// The model of terminal `node`.
pub(crate) fn model_mut(state: &mut SrState, node: NodeId) -> Option<&mut TerminalModel> {
    state.terminals.iter_mut().find(|model| model.node == node)
}

/// The model of terminal `node`.
pub(crate) fn model_of(state: &SrState, node: NodeId) -> Option<&TerminalModel> {
    state.terminals.iter().find(|model| model.node == node)
}

/// The model of terminal `node`, made with its first read; the least
/// recently read model is forgotten past [`MODELS`].
fn model_or_new(state: &mut SrState, node: NodeId) -> &mut TerminalModel {
    if let Some(at) = state.terminals.iter().position(|model| model.node == node) {
        return state.terminals.move_to_back(at);
    }
    if state.terminals.len() >= MODELS
        && let Some(oldest) = state.terminals.first().map(|model| model.node)
    {
        state.terminals.remove_where(|model| model.node == oldest);
    }
    let settings = settings_of(state);
    state.terminals.add(node, settings)
}

/// The focused terminal, when Core models it: the one whose quiet-period
/// timer counts.
pub(crate) fn focused(state: &SrState) -> Option<NodeId> {
    crate::terminal::reading::modelled(state)
}

/// When the quiet-period timer is due: the focused terminal's model's due
/// time, and no other's, so a model dropped or left has no timer. The
/// shell reads it after each step ([`SrState::terminal_timer_due`]).
pub(crate) fn due(state: &SrState) -> Option<u64> {
    model_of(state, focused(state)?)?.due_ms
}

/// A key of `kind` that reached the focused terminal at `at_ms`, with its
/// trace: kept until a read taken at or after it. Nothing when Core does
/// not model the focus.
pub(crate) fn key(state: &mut SrState, kind: TerminalKeyKind, at_ms: u64, trace: TraceId) {
    let Some(node) = focused(state) else {
        return;
    };
    let Some(model) = model_mut(state, node) else {
        return;
    };
    if model.pending.len() >= PENDING_KEYS
        && let Some(oldest) = model.pending.pop_front()
    {
        model.give(&oldest);
    }
    model.pending.push_back(PendingKey { kind, at_ms, trace });
}

/// The kinds of the keys `node`'s model holds pending, oldest first, for
/// tests of the keys it takes.
#[cfg(test)]
pub(crate) fn pending_kinds_for_tests(state: &SrState, node: NodeId) -> Vec<TerminalKeyKind> {
    model_of(state, node)
        .map(|model| model.pending.iter().map(|key| key.kind.clone()).collect())
        .unwrap_or_default()
}

/// The reading rules' key for `kind`, reaching the program at `at_ms`.
fn rules_key(kind: &TerminalKeyKind, at_ms: u64) -> Key {
    Key {
        kind: match kind {
            TerminalKeyKind::Typed(text) => KeyKind::Typed(text.clone()),
            TerminalKeyKind::Caret(unit) => KeyKind::Caret(match unit {
                TerminalUnit::Character => Unit::Character,
                TerminalUnit::Word => Unit::Word,
                TerminalUnit::Line => Unit::Line,
            }),
            TerminalKeyKind::Enter => KeyKind::Enter,
            TerminalKeyKind::Forget => KeyKind::Forget,
            TerminalKeyKind::Delete => KeyKind::Delete,
            TerminalKeyKind::Other => KeyKind::Other,
        },
        at_ms,
    }
}

/// When `snapshot`'s text and caret were taken, as far as keys are
/// concerned: when the read ended, since the terminal answered it at some
/// moment before then, so a key whose effect it shows reached the program
/// no later; when it began, for a snapshot that does not say when it
/// ended. A key that reached the program after the read began but before
/// the terminal answered it was taken as coming after the read, its effect
/// in the read then judged as no key's.
#[must_use]
pub fn read_at(snapshot: &TerminalSnapshot) -> u64 {
    snapshot.ended_ms.max(snapshot.started_ms)
}

/// The reading rules' read of `snapshot`: its text as it came, without
/// its lines' padding, which the rules take as each line's length; the
/// screen and the rows, a screen at most, padded again.
#[must_use]
pub fn read_of(snapshot: &TerminalSnapshot) -> Read {
    Read {
        text: ReadText {
            text: snapshot.text.text.clone(),
            lengths: snapshot.text.lengths.clone(),
        },
        screen: snapshot.screen.padded(),
        from_start: snapshot.from_start,
        shrank: snapshot.shrank,
        at_document_start: snapshot.at_document_start,
        alternate: snapshot.alternate,
        first_row: snapshot.first_row.padded(),
        caret_rows_to_end: snapshot.caret_rows_to_end,
        caret_line: snapshot.caret_line.padded(),
        caret_column: usize::try_from(snapshot.caret_column).unwrap_or(usize::MAX),
        at_ms: read_at(snapshot),
        above: usize::try_from(snapshot.above).unwrap_or(usize::MAX),
    }
}

/// The snapshot of `read`, as an outpost sends it: its texts without their
/// padding, with whether it waited for Core's speech queue and the colours
/// it took. For the tests and the measurements, which read the simulated
/// terminal as the outpost reads a real one.
#[must_use]
pub fn snapshot_of(
    read: &Read,
    waited: bool,
    colours: Option<&[Vec<ColourRun>]>,
) -> TerminalSnapshot {
    let cells = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    TerminalSnapshot {
        text: TerminalText {
            text: read.text.text.clone(),
            lengths: read.text.lengths.clone(),
        },
        screen: TerminalText::unpadded(&read.screen),
        from_start: read.from_start,
        shrank: read.shrank,
        at_document_start: read.at_document_start,
        alternate: read.alternate,
        first_row: TerminalText::unpadded(&read.first_row),
        caret_rows_to_end: read.caret_rows_to_end,
        caret_line: TerminalText::unpadded(&read.caret_line),
        caret_column: cells(read.caret_column),
        above: cells(read.above),
        started_ms: read.at_ms,
        ended_ms: read.at_ms,
        waited,
        read: 0,
        colours: colours.map(|rows| {
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

/// The reading rules' colours of a snapshot.
fn colours_of(colours: &[Vec<TerminalColourRun>]) -> Vec<Vec<ColourRun>> {
    colours
        .iter()
        .map(|row| {
            row.iter()
                .map(|run| ColourRun {
                    column: usize::try_from(run.column).unwrap_or(usize::MAX),
                    length: usize::try_from(run.length).unwrap_or(usize::MAX),
                    foreground: run.foreground,
                    background: run.background,
                })
                .collect()
        })
        .collect()
}

/// What the reading rules said, as Core's flood policy takes it: the
/// speech's text moved, not copied, and a new line's left as the rules
/// share it with the read.
fn speech_of(step: Step) -> Said {
    let items = step
        .speech
        .into_iter()
        .map(|speech| {
            SaidItem::Item(match speech {
                Speech::Line { text, .. } => return SaidItem::Line(text),
                Speech::Changed { text, .. } | Speech::Held { text } => TerminalItem::Output(text),
                // What a line gained, without the space that parts it from the
                // text it follows, which says nothing.
                Speech::Growth { text, .. } => TerminalItem::Growth(text.trim_start().to_owned()),
                // After a lost read, its lines follow a count of none of how
                // they are counted, to which each adds when the flood policy
                // skips it ("Core's flood policy").
                Speech::LostRead(Lost::About) => TerminalItem::Skipped(Skipped::About(0)),
                Speech::LostRead(Lost::MoreThan) => TerminalItem::Skipped(Skipped::MoreThan(0)),
                Speech::LostRead(Lost::Uncounted) => TerminalItem::Skipped(Skipped::Uncounted),
                Speech::Echo { text, .. } => TerminalItem::Echo(text),
                Speech::Caret { unit, text, .. } => TerminalItem::Caret {
                    unit: match unit {
                        Unit::Character => TerminalUnit::Character,
                        Unit::Word => TerminalUnit::Word,
                        Unit::Line => TerminalUnit::Line,
                    },
                    text,
                },
                Speech::Gained { text, .. } => TerminalItem::Gained(text),
                Speech::Lost { text, .. } => TerminalItem::Lost(text),
                Speech::Answer { text, .. } => TerminalItem::Answer(text),
                Speech::Suggestion { text, .. } => TerminalItem::Suggestion(text),
            })
        })
        .collect();
    Said {
        items,
        followed: step.followed,
    }
}

/// A hash of every field of `snapshot` but when it began and ended, its
/// number, and the caret read with it, by which an identical read is
/// known.
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

/// What taking a read gave.
#[derive(Debug, Default)]
pub(crate) struct Taken {
    /// What the rules said; `None` when the read was not given to them,
    /// being identical to the last, or when they asked for another scope
    /// instead ([`TerminalModel::owed`]).
    pub(crate) speech: Option<Said>,
    /// The read's speech key.
    pub(crate) key: Option<TraceId>,
    /// The model's generation.
    pub(crate) generation: u64,
}

/// Takes `reply`, a full read of terminal `node` that named the start
/// `named` and `waited` for Core's speech queue: covers the changes told
/// before it began, gives the rules the keys that reached the program by
/// its end and the read, keeps the scope they ask for, the start, the
/// caret and the shape it found, and takes the read's caret as Core's. A
/// read identical to the last given is not given, unless it waited or is
/// the first after the focus arrived.
pub(crate) fn take(
    state: &mut SrState,
    node: NodeId,
    reply: &TerminalReadReply,
    (named, waited): (Option<TextAnchor>, bool),
) -> Taken {
    let snapshot = &reply.snapshot;
    let at_ms = read_at(snapshot);
    let model = model_or_new(state, node);
    model.started_us = model.started_us.max(reply.started_us);
    model.last_read = Some(LastRead {
        ended_ms: at_ms,
        coloured: snapshot.colours.is_some(),
        sends: model.sends,
    });
    let generation = model.generation;
    let mut fingerprinted = snapshot.clone();
    fingerprinted.waited = waited;
    let print = fingerprint(&fingerprinted);
    if !waited && !model.focus_arrived && model.given == Some(print) {
        return Taken {
            speech: None,
            key: None,
            generation,
        };
    }
    model.given = Some(print);
    model.focus_arrived = false;
    model.give_keys(at_ms);
    let mut read = read_of(snapshot);
    // A read named a start other than the memory's: its rows are found by
    // their text, as after a refused start (Table 7; rules 2 and 4).
    if named != model.start {
        read.from_start = false;
    }
    let colours = snapshot.colours.as_deref().map(colours_of);
    let step = model.rules().read(&read, waited, colours.as_deref());
    model.due_ms = step.timer;
    let key = model.speech_key(at_ms);
    let height = step.outcome.as_ref().map_or(0, |outcome| {
        u32::try_from(outcome.height).unwrap_or(u32::MAX)
    });
    if step.whole_document || step.history_block {
        model.owed = Some(if step.whole_document {
            TerminalScope::Whole
        } else {
            TerminalScope::HistoryBlock { height }
        });
        return Taken {
            speech: None,
            key,
            generation,
        };
    }
    if let Some(outcome) = &step.outcome {
        model.height = height;
        model.width = outcome.width;
        model.caret = outcome.caret.map(|row| CaretAt {
            row,
            column: read.caret_column,
        });
        if !outcome.blank {
            model.settle_mark(&outcome.rows, at_ms);
        }
        // The start of a read whose screen shows text, or of the first,
        // is the one the next read names (Table 7).
        if !outcome.blank || model.start.is_none() {
            model.start = Some(reply.start);
        }
    } else if model.start.is_none() {
        model.start = Some(reply.start);
    }
    model.at_document_start = read.at_document_start;
    if let Some(caret) = &snapshot.caret {
        crate::editing::update_caret(state, node, caret.clone());
    }
    Taken {
        speech: Some(speech_of(step)),
        key,
        generation,
    }
}

/// `effects`, each on behalf of `key` when there is one.
pub(crate) fn under(key: Option<TraceId>, effects: Vec<Effect>) -> Vec<Effect> {
    match key {
        Some(key) => effects
            .into_iter()
            .map(|effect| Effect::ForKey(key, Box::new(effect)))
            .collect(),
        None => effects,
    }
}

/// The quiet-period timer came due at `at_ms`: the focused terminal's rules
/// are given the keys that reached the program by then, then the timer.
/// Its next due time is after `at_ms`, since the shell fires the timer
/// whenever the due time it reads is past.
pub(crate) fn timer(state: &mut SrState, trace_id: TraceId, at_ms: u64) -> Vec<Effect> {
    let Some(node) = focused(state) else {
        return Vec::new();
    };
    let Some(model) = model_mut(state, node) else {
        return Vec::new();
    };
    model.give_keys(at_ms);
    let step = model.rules().timer(at_ms);
    model.due_ms = step.timer;
    debug_assert!(
        model.due_ms.is_none_or(|due_ms| due_ms > at_ms),
        "the quiet-period timer fired at {at_ms} is due again at {:?}",
        model.due_ms
    );
    crate::terminal::speech(state, trace_id, at_ms, node, speech_of(step))
}

/// Gives every model's rules the reader settings when they changed:
/// "report new output" and "review follows caret".
pub(crate) fn sync_settings(state: &mut SrState) {
    let settings = settings_of(state);
    for model in state.terminals.iter_mut() {
        if model.reader.settings() != settings {
            let step = model.rules().set_settings(settings);
            model.due_ms = step.timer;
        }
    }
}

/// An outpost ended: the terminals it read are forgotten.
pub(crate) fn outpost_ended(state: &mut SrState, outpost: OutpostId) {
    state.drop_models(|model| model.node.outpost() == outpost);
}

#[cfg(test)]
mod tests;
