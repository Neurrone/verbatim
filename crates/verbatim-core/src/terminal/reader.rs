//! Core's terminal reader: a terminal's reads written as batches of the
//! request list's generic text steps, and their answers read back as the
//! read the reading rules take ([`TerminalSnapshot`]). It is the only code
//! that knows which steps a terminal's read is made of: the rules see the
//! snapshot, and whoever must tell a terminal's batch from another asks
//! [`asked`], the builders' inverse.
//!
//! Every batch holds the text's end once, by a move from it of more rows
//! than any terminal holds, and every later step counts from where that
//! move landed, so that output written while the batch runs stays out of
//! every part of it alike.
//!
//! The window read ([`window_batch`]), in order:
//!
//! - for a caret key's starting read, the caret and its line, so the key's
//!   starting point is read before the text;
//! - the screen (`Visible`), and, when colours are asked, the line at its
//!   top, whose length is a row's width;
//! - the held end, and the next read's start: the end moved up by the
//!   screen's height, or the screen's top when no height is known;
//! - the text, from one of two starts to the held end. A read that names a
//!   start reads from it when it lies at or before the next start, and
//!   from the next start when it does not or cannot be compared: each
//!   start's steps run under the comparison or its negation, so only one
//!   set runs. Each set is the move up by the rows asked above the start,
//!   the trimmed text from where it landed, and a probe, a move one row
//!   further up, which goes no row when the text starts there. A whole
//!   read is one trimmed range from the text's start instead;
//! - the text's first line, trimmed;
//! - the caret and its line, unless they were read first, and the caret's
//!   rows to the text's end;
//! - when asked, the screen's colours.
//!
//! The history read ([`history_batch`]) is the held end, the next start
//! and the two sets without their probes; a whole history read is one
//! trimmed range from the text's start to its end.
//!
//! The command search ([`search_batch`]) is a move from the start an
//! earlier read named to the text's end, which its matches are counted
//! against, a move from that start to the row searched from, taken
//! wherever it lands, and from there a `FindText` toward the text's end
//! and one toward its start, each taking the rows that start with the
//! text sought (Dickson, 2026-11-26).

use std::num::{NonZeroU8, NonZeroU32};

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{Batch, Chunks, Moved, Op, Placement, Point, StepRefusal, Subject};
use verbatim_model::{
    NodeId, TerminalMatch, TerminalSnapshot, TerminalText, TextAnchor, TextDetails, TextFailure,
    TextMovement, TextPoint, TextPosition, TextUnit,
};

use crate::batch::{Answers, At, Builder, Cond, Got, Ref, Sent, kind};

#[cfg(test)]
#[path = "reader_answers.rs"]
pub(crate) mod answers;

/// More rows than any terminal holds.
const FAR: i32 = 1_000_000;

/// The brand of a window read's batch.
pub enum Window {}

/// The brand of a history read's batch.
pub enum History {}

/// The brand of a command search's batch.
pub enum Search {}

/// How many matches a command search takes at most, and each of its two
/// ways too: the bound the console host's search needed
/// (`books/dev/src/reference/performance.md`, "A terminal's upward
/// search").
pub const SEARCH_MATCHES: u8 = 20;

/// How many `FindText` calls each way of a command search makes at most,
/// the hits within a row it passes over among them, so that a needle found
/// mostly within rows cannot walk the whole history (Dickson, 2026-11-26).
pub const SEARCH_CALLS: u8 = 100;

/// Which read of the window: one value per read, each with only the facts
/// that read has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowRead {
    /// The screen alone, before its height is known.
    First,
    /// From a screen above `start`, the start an earlier read named, to
    /// the text's end; from a screen above the next read's start when
    /// `start` lies below it or outside the text.
    From {
        /// The start an earlier read named.
        start: TextAnchor,
        /// The screen's height in rows.
        height: u32,
    },
    /// A first read again with its history block: from a screen above the
    /// next read's start.
    HistoryBlock {
        /// The screen's height in rows.
        height: NonZeroU32,
    },
    /// The whole text.
    Whole {
        /// The screen's height in rows, 0 when not known.
        height: u32,
    },
}

/// What Core asks of a window read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowAsk {
    /// Which read.
    pub read: WindowRead,
    /// Whether it takes the screen's colours.
    pub colours: bool,
    /// Whether it is a caret key's starting read, which reads the caret
    /// first and answers the key's starting point.
    pub starting: bool,
    /// Whether it is an output read, asked outside the event queue.
    pub output: bool,
}

impl WindowAsk {
    /// The start the read names, when it names one.
    #[must_use]
    pub const fn start(&self) -> Option<TextAnchor> {
        match self.read {
            WindowRead::From { start, .. } => Some(start),
            WindowRead::First | WindowRead::HistoryBlock { .. } | WindowRead::Whole { .. } => None,
        }
    }
}

/// What Core asks of a history read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryAsk {
    /// The whole text.
    Whole,
    /// From `above` rows above `start` to the text's end; from as many
    /// rows above the next read's start when `start` lies below it or
    /// outside the text.
    Above {
        /// The start an earlier read named.
        start: TextAnchor,
        /// How many rows above the start.
        above: u32,
        /// The screen's height in rows, from which the next read's start
        /// is counted.
        height: NonZeroU32,
    },
}

/// What Core asks of a command search: the rows that start with a
/// command mark's line's first row, each way from the mark's row, for the
/// caller to check by its line's hash, since the console host's search
/// also matches a longer row with the same start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchAsk {
    start: TextAnchor,
    needle: String,
    from: i32,
    rows: NonZeroU8,
}

impl SearchAsk {
    /// The search for `needle`, a row's text with no line break, each way
    /// from the row `from` rows above `start`, the start an earlier read
    /// named (negative below it), reading `rows` rows of text from each
    /// match: at least one and at most 255, a step's most, so a longer
    /// line's match is read short.
    #[must_use]
    pub fn new(start: TextAnchor, needle: String, from: i32, rows: u32) -> Self {
        let rows = u8::try_from(rows).unwrap_or(u8::MAX);
        Self {
            start,
            needle,
            // A count the move can negate.
            from: from.max(-i32::MAX),
            rows: NonZeroU8::new(rows).unwrap_or(NonZeroU8::MIN),
        }
    }

    /// The start the search names.
    #[must_use]
    pub const fn start(&self) -> TextAnchor {
        self.start
    }

    /// The text sought.
    #[must_use]
    pub fn needle(&self) -> &str {
        &self.needle
    }
}

/// What a batch asks of a terminal, when one of this module's builders
/// wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// A window read.
    Window(WindowAsk),
    /// A history read.
    History(HistoryAsk),
    /// A command search.
    Search(SearchAsk),
}

/// A window read's answer: the read the reading rules take, and the
/// anchors it minted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowReply {
    /// The read.
    pub snapshot: TerminalSnapshot,
    /// The anchor held at the next read's start, which a later read or
    /// history read may name.
    pub start: TextAnchor,
    /// When the read began, in microseconds on the run's shared clock.
    pub started_us: u64,
    /// A caret key's starting point, for a starting read: the selection as
    /// the caret read held it.
    pub point: Option<TextAnchor>,
}

/// A move by `count` lines.
const fn lines(count: i32) -> TextMovement {
    TextMovement {
        unit: TextUnit::Line,
        count,
    }
}

/// A move up by `rows` lines.
fn up(rows: u32) -> TextMovement {
    lines(
        i32::try_from(rows)
            .ok()
            .and_then(i32::checked_neg)
            .unwrap_or(-FAR),
    )
}

/// One start's steps: the move up by the rows asked above it, when any
/// are, the text from where it landed to the held end, and, for a window
/// read, the probe.
struct Set<B> {
    up: Option<Ref<B, kind::Move>>,
    text: Ref<B, kind::ReadRange>,
    probe: Option<Ref<B, kind::Move>>,
}

/// What a set is built from: the rows asked above its start, the held
/// end, and whether it has a probe.
struct Shape<B> {
    above: u32,
    end: At<B>,
    probe: bool,
}

impl<B> Clone for Shape<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for Shape<B> {}

/// Adds the steps of the start `from` to `batch`, each of them that names
/// no earlier step of the set under `condition`, when one is given.
fn set<B>(
    batch: &mut Builder<B>,
    node: NodeId,
    from: At<B>,
    shape: Shape<B>,
    condition: Option<&dyn Fn() -> Cond<B>>,
) -> Set<B> {
    let gated = |batch: &mut Builder<B>| {
        if let Some(condition) = condition {
            batch.when(condition());
        }
    };
    // With rows asked above the start, the move carries the condition and
    // the text and the probe name the move; with none, each starts at the
    // start itself and carries the condition.
    let up = (shape.above > 0).then(|| {
        gated(batch);
        batch.move_point(node, from, up(shape.above))
    });
    let from = if let Some(up) = up {
        up.landed()
    } else {
        gated(batch);
        from
    };
    let text = batch.read_range_trimmed(node, (from, shape.end));
    let probe = shape.probe.then(|| {
        if up.is_none() {
            gated(batch);
        }
        batch.move_point(node, from, lines(-1))
    });
    Set { up, text, probe }
}

/// The sets of a read that names `start`: the start's own, when it lies at
/// or before `next`, and the next start's when it does not.
fn sets<B>(
    batch: &mut Builder<B>,
    node: NodeId,
    (start, next): (TextAnchor, At<B>),
    shape: Shape<B>,
) -> (Set<B>, Set<B>) {
    let start: At<B> = TextPoint::At(TextPosition::at(start)).into();
    let named = set(
        batch,
        node,
        start,
        shape,
        Some(&|| Cond::not_after(start, next)),
    );
    let fallback = set(
        batch,
        node,
        next,
        shape,
        Some(&|| !Cond::not_after(start, next)),
    );
    (named, fallback)
}

/// The text a window read takes.
enum Texts<B> {
    /// The whole text.
    Whole(Ref<B, kind::ReadRange>),
    /// From the next read's start.
    Next(Set<B>),
    /// From the start named, or else from the next read's.
    Named(Set<B>, Set<B>),
}

/// The steps of a window read's batch ([`window_batch`]).
pub struct WindowSteps {
    visible: Ref<Window, kind::Visible>,
    next: Option<Ref<Window, kind::Move>>,
    texts: Texts<Window>,
    first: Ref<Window, kind::ReadText>,
    caret: Ref<Window, kind::Caret>,
    caret_line: Ref<Window, kind::ReadText>,
    caret_rows: Ref<Window, kind::Move>,
    colours: Option<Ref<Window, kind::ReadColours>>,
}

/// The caret of `node` and its line.
fn caret_and_line(
    batch: &mut Builder<Window>,
    node: NodeId,
) -> (Ref<Window, kind::Caret>, Ref<Window, kind::ReadText>) {
    let caret = batch.caret(node, None);
    let line = batch.read_text(
        node,
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    (caret, line)
}

/// The window read `ask` names of terminal `node`, as a batch, with its
/// steps: a pure function of the two.
#[must_use]
pub fn window_batch(node: NodeId, ask: &WindowAsk) -> (Sent<Window>, WindowSteps) {
    let mut batch = Builder::<Window>::named();
    if ask.output {
        batch.outside();
    }
    let read_first = ask.starting.then(|| caret_and_line(&mut batch, node));
    let visible = batch.visible(node);
    let top = ask
        .colours
        .then(|| batch.read_trimmed(node, visible.visible_start(), TextUnit::Line, (1, false)));
    let end = batch
        .move_point(node, TextPoint::End.into(), lines(FAR))
        .landed();
    let height = match ask.read {
        WindowRead::First => 0,
        WindowRead::From { height, .. } | WindowRead::Whole { height } => height,
        WindowRead::HistoryBlock { height } => height.get(),
    };
    let next = (height > 0).then(|| batch.move_point(node, end, up(height)));
    let next_start = next.map_or(visible.visible_start(), Ref::landed);
    let shape = |above| Shape {
        above,
        end,
        probe: true,
    };
    let texts = match ask.read {
        WindowRead::Whole { .. } => {
            Texts::Whole(batch.read_range_trimmed(node, (TextPoint::Start.into(), end)))
        }
        WindowRead::First => Texts::Next(set(&mut batch, node, next_start, shape(0), None)),
        WindowRead::HistoryBlock { .. } => {
            Texts::Next(set(&mut batch, node, next_start, shape(height), None))
        }
        WindowRead::From { start, .. } => {
            let (named, fallback) = sets(&mut batch, node, (start, next_start), shape(height));
            Texts::Named(named, fallback)
        }
    };
    let first = batch.read_trimmed(node, TextPoint::Start.into(), TextUnit::Line, (1, false));
    let (caret, caret_line) = read_first.unwrap_or_else(|| caret_and_line(&mut batch, node));
    let caret_rows = batch.move_point(node, caret.caret(), lines(FAR));
    let colours = top.map(|top| batch.read_colours(node, visible, top));
    let steps = WindowSteps {
        visible,
        next,
        texts,
        first,
        caret,
        caret_line,
        caret_rows,
        colours,
    };
    (batch.finish(), steps)
}

/// The steps of a history read's batch ([`history_batch`]).
pub struct HistorySteps(Histories);

enum Histories {
    Whole(Ref<History, kind::ReadRange>),
    Above(Set<History>, Set<History>),
}

/// The history read `ask` names of terminal `node`, as a batch, with its
/// steps: a pure function of the two.
#[must_use]
pub fn history_batch(node: NodeId, ask: &HistoryAsk) -> (Sent<History>, HistorySteps) {
    let mut batch = Builder::<History>::named();
    let steps = match *ask {
        HistoryAsk::Whole => Histories::Whole(
            batch.read_range_trimmed(node, (TextPoint::Start.into(), TextPoint::End.into())),
        ),
        HistoryAsk::Above {
            start,
            above,
            height,
        } => {
            let end = batch
                .move_point(node, TextPoint::End.into(), lines(FAR))
                .landed();
            let next = batch.move_point(node, end, up(height.get())).landed();
            let shape = Shape {
                above,
                end,
                probe: false,
            };
            let (named, fallback) = sets(&mut batch, node, (start, next), shape);
            Histories::Above(named, fallback)
        }
    };
    (batch.finish(), HistorySteps(steps))
}

/// A step's result, or the failure its outcome is for the read: what it
/// names not there is the terminal gone, as a start no longer held is; a
/// text with no text interface is no text; and a step refused otherwise,
/// failed or skipped leaves the read unanswered.
fn need<'a, T>(got: &Got<'a, T>) -> Result<&'a T, TextFailure> {
    match *got {
        Got::Done(done) => Ok(done),
        Got::NotThere(_) => Err(TextFailure::Gone),
        Got::Refused(StepRefusal::NoText) => Err(TextFailure::NoText),
        Got::Refused(_) | Got::Failed | Got::Skipped => Err(TextFailure::Unanswered),
    }
}

/// How many rows a move went.
const fn rows(moved: &Moved) -> u32 {
    moved.moved.unsigned_abs()
}

/// A trimmed range's text.
fn range_text(range: &verbatim_model::batch::RangeText) -> Result<TerminalText, TextFailure> {
    Ok(TerminalText {
        text: range.text.clone(),
        lengths: range.padded.clone().ok_or(TextFailure::Unanswered)?,
    })
}

/// The one line a trimmed read of one line gave.
fn line(read: &Chunks) -> Result<TerminalText, TextFailure> {
    let chunk = read.chunks.first().ok_or(TextFailure::Unanswered)?;
    Ok(TerminalText {
        text: chunk.text.clone(),
        lengths: read.padded.clone().ok_or(TextFailure::Unanswered)?,
    })
}

/// Which of two sets read the text, and whether it is the named start's:
/// the named start's when its text was read, the next start's when the
/// named one's did not run. A named start no longer held answers both
/// sets' first steps not there, whichever way their comparison is negated,
/// so the next start's set then reads as the terminal gone.
fn chosen<'a, B>(
    answers: &Answers<B>,
    (named, fallback): (&'a Set<B>, &'a Set<B>),
) -> (&'a Set<B>, bool) {
    match answers.get(named.text) {
        Got::Done(_) => (named, true),
        _ => (fallback, false),
    }
}

/// The text `set` read and the rows it holds above its start.
fn set_text<B>(answers: &Answers<B>, set: &Set<B>) -> Result<(TerminalText, u32), TextFailure> {
    let above = match set.up {
        Some(up) => rows(need(&answers.get(up))?),
        None => 0,
    };
    Ok((range_text(need(&answers.get(set.text))?)?, above))
}

/// The window read the answers to `steps` make, the batch `ask` named,
/// begun and ended at `times`, the answer's own, in microseconds.
///
/// # Errors
///
/// The failure a step the read needs ended with: the terminal gone, a
/// start named that is no longer held included, no text, or unanswered.
pub fn window_of(
    answers: &Answers<Window>,
    steps: &WindowSteps,
    ask: &WindowAsk,
    (began_us, ended_us): (u64, u64),
) -> Result<WindowReply, TextFailure> {
    let visible = need(&answers.get(steps.visible))?;
    let start = match steps.next {
        Some(next) => need(&answers.get(next))?.to,
        None => visible.start,
    };
    let (text, above, at_document_start, from_start) = match &steps.texts {
        Texts::Whole(text) => (range_text(need(&answers.get(*text))?)?, 0, true, false),
        Texts::Next(set) => {
            let (text, above) = set_text(answers, set)?;
            (text, above, at_text_start(answers, set)?, false)
        }
        Texts::Named(named, fallback) => {
            let (set, from_start) = chosen(answers, (named, fallback));
            let (text, above) = set_text(answers, set)?;
            (text, above, at_text_start(answers, set)?, from_start)
        }
    };
    let caret = need(&answers.get(steps.caret))?;
    let caret_line = need(&answers.get(steps.caret_line))?;
    let chunk = caret_line.chunks.first().ok_or(TextFailure::Unanswered)?;
    let column = usize::try_from(chunk.offset)
        .ok()
        .and_then(|offset| chunk.text.get(..offset))
        .and_then(|before| u32::try_from(before.chars().count()).ok())
        .ok_or(TextFailure::Unanswered)?;
    let caret_rows = rows(need(&answers.get(steps.caret_rows))?);
    let colours = match steps.colours {
        Some(colours) => Some(need(&answers.get(colours))?.clone()),
        None => None,
    };
    let started_ms = began_us / 1_000;
    Ok(WindowReply {
        snapshot: TerminalSnapshot {
            text,
            screen: TerminalText {
                text: visible.text.clone(),
                lengths: visible.padded.clone(),
            },
            from_start,
            shrank: matches!(steps.texts, Texts::Named(..)) && !from_start,
            at_document_start,
            alternate: visible.at_text_start,
            first_row: line(need(&answers.get(steps.first))?)?,
            // A provider that reported no selection has no caret: the
            // text's start stands in for it, and its rows are not counted.
            caret_rows_to_end: if caret.reported { caret_rows } else { 0 },
            caret_line: TerminalText::unpadded(&chunk.text),
            caret_column: column,
            above,
            started_ms,
            ended_ms: (ended_us / 1_000).max(started_ms),
            waited: false,
            colours,
            caret: Some(crate::text_facts::caret_report(caret, chunk.clone())),
        },
        start,
        started_us: began_us,
        point: ask.starting.then_some(caret.held),
    })
}

/// Whether `set`'s text starts at the text's start: its probe, one row
/// further up, went no row.
fn at_text_start(answers: &Answers<Window>, set: &Set<Window>) -> Result<bool, TextFailure> {
    let probe = set.probe.ok_or(TextFailure::Unanswered)?;
    Ok(rows(need(&answers.get(probe))?) == 0)
}

/// The history read the answers to `steps` make: its text, each line
/// padded again as the terminal gave it, and the rows it holds above the
/// start it was counted from, 0 for a whole read.
///
/// # Errors
///
/// As [`window_of`].
pub fn history_of(
    answers: &Answers<History>,
    steps: &HistorySteps,
) -> Result<(String, u32), TextFailure> {
    let (text, above) = match &steps.0 {
        Histories::Whole(text) => (range_text(need(&answers.get(*text))?)?, 0),
        Histories::Above(named, fallback) => {
            let (set, _) = chosen(answers, (named, fallback));
            set_text(answers, set)?
        }
    };
    Ok((text.padded(), above))
}

/// The steps of a command search's batch ([`search_batch`]).
pub struct SearchSteps {
    to_end: Ref<Search, kind::Move>,
    row: Ref<Search, kind::Move>,
    down: Ref<Search, kind::FindText>,
    up: Ref<Search, kind::FindText>,
}

/// The command search `ask` names of terminal `node`, as a batch, with its
/// steps: a pure function of the two.
#[must_use]
pub fn search_batch(node: NodeId, ask: &SearchAsk) -> (Sent<Search>, SearchSteps) {
    let mut batch = Builder::<Search>::named();
    let start = || TextPoint::At(TextPosition::at(ask.start)).into();
    // The start's rows to the end are counted before the searches, so
    // that output written while they run does not skew a match's row.
    let to_end = batch.move_point(node, start(), lines(FAR));
    let row = batch.move_point(node, start(), lines(ask.from.saturating_neg()));
    let way = crate::batch::Search {
        unit_start: Some(TextUnit::Line),
        read: ask.rows.get(),
        limit: SEARCH_MATCHES,
        calls: SEARCH_CALLS,
    };
    let mut find =
        |backward| batch.find_text(node, row.landed(), (ask.needle.clone(), backward), way);
    let (down, up) = (find(false), find(true));
    let steps = SearchSteps {
        to_end,
        row,
        down,
        up,
    };
    (batch.finish(), steps)
}

/// The matches the answers to `steps` make: alternately the next toward
/// the text's end, the row searched from first when it starts with the
/// text sought, and the next toward its start, one way's when the other
/// has no more, at most [`SEARCH_MATCHES`], each counted in rows above the
/// start. A way that found nothing has none; it is the caller that drops
/// a mark, when no match of either way is its line.
///
/// # Errors
///
/// A search that did not run whole, so that its caller drops nothing:
/// the terminal gone when any step's node or start is not there, as
/// [`history_of`] reads it, and otherwise the failure of the first step
/// not done, a move refused because the start lies past the text's end
/// among them.
pub fn search_of(
    answers: &Answers<Search>,
    steps: &SearchSteps,
) -> Result<Vec<TerminalMatch>, TextFailure> {
    let (to_end, row) = (
        need(&answers.get(steps.to_end)),
        need(&answers.get(steps.row)),
    );
    let (down, up) = (need(&answers.get(steps.down)), need(&answers.get(steps.up)));
    let outcomes = [
        to_end.map(|_| ()),
        row.map(|_| ()),
        down.map(|_| ()),
        up.map(|_| ()),
    ];
    if outcomes.contains(&Err(TextFailure::Gone)) {
        return Err(TextFailure::Gone);
    }
    let (from_end, down, up) = (i64::from(rows(to_end?)), down?, up?);
    row?;
    let (mut down, mut up) = (down.iter(), up.iter());
    let mut taken = Vec::new();
    let mut going_down = true;
    while taken.len() < usize::from(SEARCH_MATCHES) {
        let next = if going_down {
            down.next().or_else(|| up.next())
        } else {
            up.next().or_else(|| down.next())
        };
        let Some(found) = next else {
            break;
        };
        taken.push(TerminalMatch {
            // A row count no terminal holds leaves the search unanswered.
            above: i32::try_from(i64::from(found.to_end).saturating_sub(from_end))
                .map_err(|_| TextFailure::Unanswered)?,
            text: found.text.clone(),
        });
        going_down = !going_down;
    }
    Ok(taken)
}

/// The search `batch` could be: its start and row from its first two
/// moves, and its text and rows from its first search.
fn search_hint(batch: &Batch) -> Option<SearchAsk> {
    let [first, second, third, ..] = batch.steps.as_slice() else {
        return None;
    };
    let (Op::Move { at, .. }, Op::Move { movement, .. }, Op::FindText { needle, read, .. }) =
        (&first.op, &second.op, &third.op)
    else {
        return None;
    };
    Some(SearchAsk::new(
        held(at)?,
        needle.clone(),
        movement.count.checked_neg()?,
        u32::from(*read),
    ))
}

/// The anchor `point` names, when it is one Core holds, at no offset.
fn held(point: &Point) -> Option<TextAnchor> {
    match point {
        Point::At(TextPoint::At(position)) if position.offset == 0 => Some(position.anchor),
        Point::At(_) | Point::Of(..) => None,
    }
}

/// What a batch's moves say of the read it could be: the start it names,
/// the rows the next start lies above the held end, and the rows asked
/// above the start named.
#[derive(Default)]
struct Hints {
    start: Option<TextAnchor>,
    height: u32,
    above: u32,
}

/// The hints of `batch`: from its first move at a held anchor, and from
/// the first move that starts where another landed, the next start's.
fn hints(batch: &Batch) -> Hints {
    let mut hints = Hints::default();
    let mut next_seen = false;
    for step in &batch.steps {
        match &step.op {
            Op::Move { at, movement, .. } => {
                if let Some(anchor) = held(at) {
                    if hints.start.is_none() {
                        hints.above = movement.count.unsigned_abs();
                    }
                    hints.start.get_or_insert(anchor);
                } else if matches!(at, Point::Of(..)) && !next_seen {
                    next_seen = true;
                    hints.height = movement.count.unsigned_abs();
                }
            }
            Op::ReadRange { start, .. } => {
                if let Some(anchor) = held(start) {
                    hints.start.get_or_insert(anchor);
                }
            }
            _ => {}
        }
    }
    hints
}

/// What `batch` asks of a terminal: the ask for which [`window_batch`],
/// [`history_batch`] or [`search_batch`] writes exactly this batch, the
/// builders' inverse;
/// `None` for every other batch. A candidate ask is read from the batch's
/// shape and kept only when its builder writes the batch back, so no
/// batch another function wrote passes.
#[must_use]
pub fn asked(batch: &Batch) -> Option<Asked> {
    let Some(Subject::Node(node)) = batch.steps.first().and_then(|step| step.op.subject()) else {
        return None;
    };
    if let Some(ask) = search_hint(batch).filter(|ask| search_batch(*node, ask).0.batch() == batch)
    {
        return Some(Asked::Search(ask));
    }
    let hints = hints(batch);
    let histories = [
        Some(HistoryAsk::Whole),
        hints
            .start
            .zip(NonZeroU32::new(hints.height))
            .map(|(start, height)| HistoryAsk::Above {
                start,
                above: hints.above,
                height,
            }),
    ];
    if let Some(ask) = histories
        .into_iter()
        .flatten()
        .find(|ask| history_batch(*node, ask).0.batch() == batch)
    {
        return Some(Asked::History(ask));
    }
    // In a window read the first move from where another landed is the
    // next start's, from the held end, or with no height known a set's
    // probe, one row up.
    let reads = match hints.start {
        Some(start) => vec![
            WindowRead::From {
                start,
                height: hints.height,
            },
            WindowRead::From { start, height: 0 },
        ],
        None => vec![
            Some(WindowRead::First),
            Some(WindowRead::Whole {
                height: hints.height,
            }),
            Some(WindowRead::Whole { height: 0 }),
            NonZeroU32::new(hints.height).map(|height| WindowRead::HistoryBlock { height }),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    let starting = matches!(
        batch.steps.first().map(|step| &step.op),
        Some(Op::Caret { .. })
    );
    let colours = matches!(
        batch.steps.last().map(|step| &step.op),
        Some(Op::ReadColours { .. })
    );
    reads
        .into_iter()
        .map(|read| WindowAsk {
            read,
            colours,
            starting,
            output: batch.placement == Placement::Outside,
        })
        .find(|ask| window_batch(*node, ask).0.batch() == batch)
        .map(Asked::Window)
}

#[cfg(test)]
mod tests {
    use verbatim_model::batch::{Done, FoundText, StepOutcome};

    use super::*;

    fn node() -> NodeId {
        NodeId::new(12)
    }

    fn height(rows: u32) -> NonZeroU32 {
        NonZeroU32::new(rows).expect("a height")
    }

    /// Every window ask of these reads, with and without colours, a
    /// starting read and an output read.
    fn window_asks() -> Vec<WindowAsk> {
        let reads = [
            WindowRead::First,
            WindowRead::From {
                start: TextAnchor(7),
                height: 30,
            },
            WindowRead::From {
                start: TextAnchor(7),
                height: 1,
            },
            WindowRead::From {
                start: TextAnchor(8),
                height: 0,
            },
            WindowRead::HistoryBlock { height: height(30) },
            WindowRead::HistoryBlock { height: height(1) },
            WindowRead::Whole { height: 30 },
            WindowRead::Whole { height: 1 },
            WindowRead::Whole { height: 0 },
        ];
        let mut asks = Vec::new();
        for read in reads {
            for colours in [false, true] {
                for starting in [false, true] {
                    for output in [false, true] {
                        asks.push(WindowAsk {
                            read,
                            colours,
                            starting,
                            output,
                        });
                    }
                }
            }
        }
        asks
    }

    fn history_asks() -> Vec<HistoryAsk> {
        let above = |above, rows| HistoryAsk::Above {
            start: TextAnchor(7),
            above,
            height: height(rows),
        };
        vec![
            HistoryAsk::Whole,
            above(0, 30),
            above(13, 30),
            above(1, 1),
            above(500, 4),
        ]
    }

    /// Searches from rows above and below the start, of one row and of
    /// more, and of more rows than a step reads.
    fn search_asks() -> Vec<SearchAsk> {
        let ask = |from, rows| SearchAsk::new(TextAnchor(7), "$ echo hi".to_owned(), from, rows);
        vec![
            ask(0, 1),
            ask(40, 1),
            ask(-3, 2),
            ask(9_000, 255),
            ask(12, 300),
            ask(i32::MIN, 0),
        ]
    }

    /// The outcomes of a search's four steps: the start's `to_end` rows to
    /// the text's end, a move to the row that went `went`, and each way's
    /// matches, by their rows to the text's end.
    fn search_outcomes(to_end: i32, went: i32, ways: [&[u32]; 2]) -> Vec<StepOutcome> {
        let moved = |moved| {
            StepOutcome::Done(Done::Move(Moved {
                moved,
                from: TextAnchor(20),
                to: TextAnchor(21),
            }))
        };
        let found = |way: &[u32]| {
            StepOutcome::Done(Done::FindText(
                way.iter()
                    .map(|&to_end| FoundText {
                        to_end,
                        text: format!("row {to_end}"),
                    })
                    .collect(),
            ))
        };
        vec![moved(to_end), moved(went), found(ways[0]), found(ways[1])]
    }

    /// What `search_of` reads of `outcomes`, the answers to a search five
    /// rows above its start: each match's rows above the start, or the
    /// failure.
    fn searched(outcomes: Vec<StepOutcome>) -> Result<Vec<i32>, TextFailure> {
        let ask = SearchAsk::new(TextAnchor(7), "row".to_owned(), 5, 1);
        let (sent, steps) = search_batch(node(), &ask);
        let answers = Answers::parse(&sent, verbatim_model::request::Reply::Steps(outcomes))
            .expect("an outcome of each step's kind");
        search_of(&answers, &steps).map(|matches| {
            matches
                .into_iter()
                .map(|found| {
                    assert_eq!(found.text, format!("row {}", found.above + 10));
                    found.above
                })
                .collect()
        })
    }

    /// A search whose steps are all done answers its two ways' matches
    /// alternately, the way toward the text's end first, one way's when
    /// the other has no more, each as rows above the start, whose own rows
    /// to the end are 10 here; a way with no match has none, and a second
    /// move that went fewer rows than asked is taken as it landed.
    #[test]
    fn a_search_done_answers_its_ways_matches_alternately() {
        assert_eq!(
            searched(search_outcomes(10, -5, [&[15, 12, 3], &[16, 20]])),
            Ok(vec![5, 6, 2, 10, -7])
        );
        assert_eq!(
            searched(search_outcomes(10, -5, [&[15], &[16, 20, 22]])),
            Ok(vec![5, 6, 10, 12])
        );
        assert_eq!(
            searched(search_outcomes(10, -5, [&[], &[16, 20]])),
            Ok(vec![6, 10])
        );
        assert_eq!(
            searched(search_outcomes(10, -5, [&[15, 12], &[]])),
            Ok(vec![5, 2])
        );
        assert_eq!(searched(search_outcomes(10, -5, [&[], &[]])), Ok(vec![]));
        assert_eq!(
            searched(search_outcomes(10, 0, [&[10], &[]])),
            Ok(vec![0]),
            "a move that went no row"
        );
    }

    /// A search answers at most twenty matches, ten of each way when each
    /// found twenty.
    #[test]
    fn a_search_answers_at_most_twenty_matches() {
        let down: Vec<u32> = (0..20).map(|row| 115 - row).collect();
        let up: Vec<u32> = (0..20).map(|row| 116 + row).collect();
        let expected: Vec<i32> = (0..10).flat_map(|row| [105 - row, 106 + row]).collect();
        assert_eq!(
            searched(search_outcomes(10, -5, [&down, &up])),
            Ok(expected)
        );
    }

    /// A search with a step not done answers no matches at all, so that
    /// its caller drops no mark for it: the terminal gone when any step
    /// answers that its node or its start is not there, whatever the
    /// others answer, and otherwise unanswered, for a step that failed,
    /// was refused, as a move from a start past the text's end is, or was
    /// skipped.
    #[test]
    fn a_search_with_a_step_not_done_answers_its_failure() {
        use verbatim_model::batch::Missing;
        let done = || search_outcomes(10, -5, [&[15], &[16]]);
        let with = |at: usize, outcome: StepOutcome| {
            let mut outcomes = done();
            outcomes[at] = outcome;
            searched(outcomes)
        };
        let lost = StepOutcome::NotThere(Missing::Anchor(TextAnchor(7)));
        let gone = StepOutcome::NotThere(Missing::Node(node()));
        let outside = StepOutcome::Refused(StepRefusal::OutsideText);
        for at in 0..4 {
            assert_eq!(with(at, gone.clone()), Err(TextFailure::Gone), "{at}");
            for outcome in [StepOutcome::Failed, StepOutcome::Skipped, outside.clone()] {
                assert_eq!(
                    with(at, outcome.clone()),
                    Err(TextFailure::Unanswered),
                    "{at} {outcome:?}"
                );
            }
        }
        // A start no longer held: both moves not there, the searches
        // skipped.
        assert_eq!(
            searched(vec![
                lost.clone(),
                lost.clone(),
                StepOutcome::Skipped,
                StepOutcome::Skipped
            ]),
            Err(TextFailure::Gone)
        );
        // Not there after a failure is still the terminal gone.
        assert_eq!(
            searched(vec![
                StepOutcome::Failed,
                outside,
                StepOutcome::Skipped,
                gone
            ]),
            Err(TextFailure::Gone)
        );
        assert_eq!(
            with(2, StepOutcome::Refused(StepRefusal::NoText)),
            Err(TextFailure::NoText)
        );
    }

    /// A search reads at least one row and at most 255 from each match,
    /// and moves to its row by the rows asked, negated.
    #[test]
    fn a_search_reads_at_most_255_rows_from_a_match() {
        let steps = |from, rows| {
            let ask = SearchAsk::new(TextAnchor(7), "$".to_owned(), from, rows);
            let (sent, _) = search_batch(node(), &ask);
            sent.batch()
                .steps
                .iter()
                .map(|step| match &step.op {
                    Op::Move { movement, .. } => movement.count,
                    Op::FindText {
                        read,
                        limit,
                        calls,
                        backward,
                        ..
                    } => {
                        assert_eq!((*limit, *calls), (20, 100));
                        i32::from(*read) * if *backward { -1 } else { 1 }
                    }
                    other => panic!("a search's step, not {other:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(steps(40, 1), [FAR, -40, 1, -1]);
        assert_eq!(steps(-3, 255), [FAR, 3, 255, -255]);
        assert_eq!(steps(0, 256), [FAR, 0, 255, -255]);
        assert_eq!(steps(i32::MIN, 0), [FAR, i32::MAX, 1, -1]);
    }

    /// `asked` is the builders' inverse: every ask's batch is asked back
    /// as that ask.
    #[test]
    fn every_asks_batch_is_asked_back_as_the_ask() {
        for ask in search_asks() {
            let (sent, _) = search_batch(node(), &ask);
            assert_eq!(
                asked(sent.batch()),
                Some(Asked::Search(ask.clone())),
                "{ask:?}"
            );
        }
        for ask in window_asks() {
            let (sent, _) = window_batch(node(), &ask);
            assert_eq!(asked(sent.batch()), Some(Asked::Window(ask)), "{ask:?}");
        }
        for ask in history_asks() {
            let (sent, _) = history_batch(node(), &ask);
            assert_eq!(asked(sent.batch()), Some(Asked::History(ask)), "{ask:?}");
        }
    }

    /// The batches' sizes, the largest one step short of a batch's most.
    #[test]
    fn each_reads_batch_has_its_count_of_steps() {
        let steps = |read, colours| {
            let ask = WindowAsk {
                read,
                colours,
                starting: false,
                output: false,
            };
            window_batch(node(), &ask).0.batch().steps.len()
        };
        let from = WindowRead::From {
            start: TextAnchor(7),
            height: 30,
        };
        let block = WindowRead::HistoryBlock { height: height(30) };
        let whole = WindowRead::Whole { height: 30 };
        assert_eq!(
            [
                steps(from, false),
                steps(from, true),
                steps(block, false),
                steps(block, true),
                steps(WindowRead::First, false),
                steps(WindowRead::First, true),
                steps(whole, false),
                steps(whole, true),
            ],
            [13, 15, 10, 12, 8, 10, 8, 10]
        );
        let history = |ask| history_batch(node(), &ask).0.batch().steps.len();
        assert_eq!(history(HistoryAsk::Whole), 1);
        assert_eq!(
            history(HistoryAsk::Above {
                start: TextAnchor(7),
                above: 13,
                height: height(30),
            }),
            6
        );
        for ask in search_asks() {
            let (sent, _) = search_batch(node(), &ask);
            assert_eq!(sent.batch().steps.len(), 4, "{ask:?}");
            verbatim_model::batch::parse(
                sent.batch().clone(),
                verbatim_model::batch::ReaderKind::Outpost,
            )
            .unwrap_or_else(|error| panic!("{ask:?}: {error:?}"));
        }
        // Every batch parses as one an outpost runs.
        for ask in window_asks() {
            let (sent, _) = window_batch(node(), &ask);
            verbatim_model::batch::parse(
                sent.batch().clone(),
                verbatim_model::batch::ReaderKind::Outpost,
            )
            .unwrap_or_else(|error| panic!("{ask:?}: {error:?}"));
        }
        for ask in history_asks() {
            let (sent, _) = history_batch(node(), &ask);
            verbatim_model::batch::parse(
                sent.batch().clone(),
                verbatim_model::batch::ReaderKind::Outpost,
            )
            .unwrap_or_else(|error| panic!("{ask:?}: {error:?}"));
        }
    }

    /// A batch one step away from a read's is no read: another node's
    /// step in it, a step more, a step fewer, or another placement of a
    /// history read.
    #[test]
    fn a_batch_no_builder_wrote_is_not_asked() {
        let ask = WindowAsk {
            read: WindowRead::From {
                start: TextAnchor(7),
                height: 30,
            },
            colours: true,
            starting: true,
            output: false,
        };
        let (sent, _) = window_batch(node(), &ask);
        let written = sent.batch().clone();
        assert_eq!(asked(&written), Some(Asked::Window(ask)));
        let mut fewer = written.clone();
        fewer.steps.pop();
        // Without its last step, the colours, it is no read: a read
        // without colours has no line at the screen's top either.
        assert_eq!(asked(&fewer), None);
        let mut more = written.clone();
        more.steps.push(more.steps[0].clone());
        assert_eq!(asked(&more), None);
        let mut other = written.clone();
        other.steps[3].op = Op::Visible {
            subject: Subject::Node(NodeId::new(13)),
        };
        assert_eq!(asked(&other), None);
        let mut spare = written;
        spare.spare = Some((node(), TextAnchor(9)));
        assert_eq!(asked(&spare), None);
        let (history, _) = history_batch(node(), &HistoryAsk::Whole);
        let mut outside = history.batch().clone();
        assert_eq!(asked(&outside), Some(Asked::History(HistoryAsk::Whole)));
        outside.placement = Placement::Outside;
        assert_eq!(asked(&outside), None);
        // A search with a step more, and with one way's bound changed.
        let search = SearchAsk::new(TextAnchor(7), "$".to_owned(), 3, 1);
        let (sent, _) = search_batch(node(), &search);
        let written = sent.batch().clone();
        assert_eq!(asked(&written), Some(Asked::Search(search)));
        let mut more = written.clone();
        more.steps.push(more.steps[3].clone());
        assert_eq!(asked(&more), None);
        let mut bounded = written;
        if let Op::FindText { calls, .. } = &mut bounded.steps[3].op {
            *calls = 8;
        }
        assert_eq!(asked(&bounded), None);
        // A caret read alone, as a caret key's own batches open.
        let mut caret = Builder::<Window>::named();
        let _ = caret_and_line(&mut caret, node());
        assert_eq!(asked(caret.finish().batch()), None);
    }
}
