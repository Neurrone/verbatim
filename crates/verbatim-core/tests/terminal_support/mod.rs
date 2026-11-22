//! A terminal Core reads, for the tests of Core's terminal reading: the
//! simulated terminal (`verbatim-term-sim`), whose provider answers each
//! call as the console host's or Windows Terminal's does (its conformance
//! expectations check it against them), with each step of every batch Core
//! sends its outpost answered from it as the UI Automation provider answers
//! the step's calls; and the public reducer driven through its event queue
//! as the shell drives it (`desk`): the terminal focused through the
//! listener and the outpost, every write told to Core as the outpost tells
//! a change, the requests answered in the order Core sent them, and the
//! speech Core queues played by reaching each utterance's marks.

#![allow(dead_code, reason = "each test file uses part of the support")]
#![expect(
    clippy::result_large_err,
    reason = "a step's outcome is what ends it, as the outpost answers it"
)]

use std::collections::VecDeque;

use crate::desk::{Desk, ONE, focus_read_of};
use verbatim_model::batch::{
    Batch, CaretAt, Chunks, Done, FoundText, Gate, Located, Missing, Moved, Op, Point, PointOf,
    RangeText, StepOutcome, StepRefusal, VisibleText, Waited,
};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{
    Answer, CaretBinding, Outcome, Press, PressAction, Question, QueueEffect, QueueInput, Reply,
    RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, CaretKey, Effect, Input, KeySeq, NodeDetails, NodeId, NodeSnapshot, ReaderSettings,
    Role, SegmentContent, SpeechMark, StateSet, TerminalColourRun, TerminalText, TextAnchor,
    TextChunk, TextPoint, TextUnit, TraceId,
};
use verbatim_term_sim::{Flavor, Position, Range, Terminal};

/// The terminal's node number.
pub const TERMINAL: u64 = 31;

/// The terminal's node.
pub fn node() -> NodeId {
    ONE.node(TERMINAL)
}

/// More rows than any terminal holds.
const FAR: isize = 1_000_000;

/// The simulated terminal answering each step as its provider does, with
/// the ranges it holds for Core, by anchor.
pub struct Sim {
    /// The terminal.
    pub terminal: Terminal,
    /// The ranges held, anchor `n` the `n - 1`th.
    held: Vec<Position>,
    /// How a caret wait ends.
    pub waited: Waited,
}

/// A step's point found, or the outcome that ends the step.
type Found<T> = Result<T, StepOutcome>;

impl Sim {
    /// A terminal of `flavor`, `width` by `height`, with `history` rows.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self {
            terminal: Terminal::new(flavor, width, height, history),
            held: Vec::new(),
            waited: Waited::Effect,
        }
    }

    /// Holds `at`, minting its anchor.
    fn hold(&mut self, at: Position) -> TextAnchor {
        self.held.push(at);
        TextAnchor(u64::try_from(self.held.len()).expect("anchors"))
    }

    /// The position anchor `anchor` holds.
    fn held(&self, anchor: TextAnchor) -> Found<Position> {
        usize::try_from(anchor.0)
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| self.held.get(index).copied())
            .ok_or(StepOutcome::NotThere(Missing::Anchor(anchor)))
    }

    /// The text of `range`, as `GetText` gives it; a refused call fails
    /// the step.
    fn text(&self, range: Range) -> Found<String> {
        self.terminal.text(range).map_err(|_| StepOutcome::Failed)
    }

    /// `at` moved on by the characters of `offset` bytes of the text from
    /// it, as the outpost finds a position by its offset.
    fn advanced(&self, at: Position, offset: u32) -> Found<Position> {
        if offset == 0 {
            return Ok(at);
        }
        let row = self.terminal.row_of(at).map_err(|_| StepOutcome::Failed)?;
        let text = self.text(Range {
            start: at,
            end: row.end,
        })?;
        let offset = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(text.len());
        let columns = text
            .get(..offset)
            .map_or(0, |before| before.chars().count());
        Ok(Position {
            row: at.row,
            column: at.column + columns,
        })
    }

    /// The position `point` names, the outcomes `earlier` answering a point
    /// of an earlier step's result.
    fn point(&self, point: &Point, earlier: &[StepOutcome]) -> Found<Position> {
        match point {
            Point::At(TextPoint::Start) => Ok(self.terminal.document_range().start),
            Point::At(TextPoint::End) => Ok(self.terminal.document_range().end),
            Point::At(TextPoint::Caret | TextPoint::SelectionStart | TextPoint::SelectionEnd) => {
                Ok(self.terminal.caret().start)
            }
            Point::At(TextPoint::At(position)) => {
                self.advanced(self.held(position.anchor)?, position.offset)
            }
            Point::Of(place, which) => {
                let anchor = match (earlier.get(place.index()), which) {
                    (Some(StepOutcome::Done(Done::Visible(visible))), PointOf::VisibleStart) => {
                        visible.start
                    }
                    (Some(StepOutcome::Done(Done::Visible(visible))), PointOf::VisibleEnd) => {
                        visible.end
                    }
                    (Some(StepOutcome::Done(Done::Move(moved))), PointOf::Moved) => moved.to,
                    (
                        Some(StepOutcome::Done(Done::Caret(caret))),
                        PointOf::Caret | PointOf::SelectionStart | PointOf::SelectionEnd,
                    ) => caret.caret,
                    (Some(StepOutcome::Done(Done::ReadText(read))), PointOf::ChunkStart) => read
                        .chunks
                        .first()
                        .map(|chunk| chunk.start)
                        .ok_or(StepOutcome::Failed)?,
                    other => panic!("a point of an earlier result the support answers: {other:?}"),
                };
                self.held(anchor)
            }
        }
    }

    /// The trimmed text of `text`, with its padded lengths when `trim`.
    fn trimmed(text: String, trim: bool) -> (String, Option<Vec<(u32, u32)>>) {
        if trim {
            let unpadded = TerminalText::unpadded(&text);
            (unpadded.text, Some(unpadded.lengths))
        } else {
            (text, None)
        }
    }

    /// A chunk of a line, `text`, held at `start`, the point `offset` bytes
    /// into it.
    fn chunk(&mut self, start: Position, text: String, offset: usize) -> TextChunk {
        let last = self.terminal.document_range().end.row;
        TextChunk {
            unit: TextUnit::Line,
            start: self.hold(start),
            offset: u32::try_from(offset).unwrap_or(u32::MAX),
            first: start.row == 0,
            last: start.row == last,
            text,
            languages: Vec::new(),
            truncated: false,
            formats: Vec::new(),
        }
    }

    /// `count` lines from `at`, after a move of `movement` lines from the
    /// start of `at`'s line, as the outpost's classic read takes them.
    fn read_lines(
        &mut self,
        at: Position,
        movement: Option<i32>,
        (count, trim): (u16, bool),
    ) -> Found<Done> {
        let failed = |_| StepOutcome::Failed;
        let (point, moved, on_start) = match movement {
            None => (at, 0, false),
            Some(by) => {
                let start = self.terminal.row_of(at).map_err(failed)?.start;
                let (landed, moved) = self
                    .terminal
                    .move_rows(start, isize::try_from(by).unwrap_or(0))
                    .map_err(failed)?;
                (landed, i32::try_from(moved).unwrap_or(0), true)
            }
        };
        let mut chunks = Vec::new();
        let mut padded;
        let mut row = self.terminal.row_of(point).map_err(failed)?;
        loop {
            let text = self.text(row)?;
            let offset = if chunks.is_empty() && !on_start {
                let before = self.text(Range {
                    start: row.start,
                    end: point,
                })?;
                before.len().min(text.len())
            } else {
                0
            };
            let (text, lengths) = Self::trimmed(text, trim);
            padded = lengths;
            let offset = offset.min(text.len());
            chunks.push(self.chunk(row.start, text, offset));
            if chunks.len() >= usize::from(count) {
                break;
            }
            let (next, went) = self.terminal.move_rows(row.start, 1).map_err(failed)?;
            let next = self.terminal.row_of(next).map_err(failed)?;
            if went == 0 || next.start.row <= row.start.row {
                break;
            }
            row = next;
        }
        Ok(Done::ReadText(Chunks {
            moved,
            chunks,
            padded,
        }))
    }

    /// The matches of `needle` from `at` to the text's end, or back to its
    /// start, the nearest first, as the outpost's classic search finds them:
    /// each occurrence one call, a match not at its row's start passed over
    /// when `starts`.
    fn find(
        &self,
        at: Position,
        (needle, backward): (&str, bool),
        (starts, read): (bool, u8),
        (limit, most): (u8, u8),
    ) -> Found<Vec<FoundText>> {
        let failed = |_| StepOutcome::Failed;
        let last = self.terminal.document_range().end.row;
        let mut hits = Vec::new();
        for row in 0..=last {
            let start = Position { row, column: 0 };
            let text = self.text(self.terminal.row_of(start).map_err(failed)?)?;
            let cells: Vec<char> = text.trim_end_matches("\r\n").chars().collect();
            let wanted: Vec<char> = needle.chars().collect();
            if wanted.is_empty() || wanted.len() > cells.len() {
                continue;
            }
            for column in 0..=cells.len() - wanted.len() {
                if cells[column..column + wanted.len()] == wanted[..] {
                    hits.push((
                        Position { row, column },
                        Position {
                            row,
                            column: column + wanted.len(),
                        },
                    ));
                }
            }
        }
        let ordered: Vec<(Position, Position)> = if backward {
            hits.into_iter()
                .rev()
                .filter(|(_, end)| *end <= at)
                .collect()
        } else {
            hits.into_iter().filter(|(start, _)| *start >= at).collect()
        };
        let mut found = Vec::new();
        for (start, _) in ordered.into_iter().take(usize::from(most)) {
            if found.len() >= usize::from(limit) {
                break;
            }
            if starts && start.column != 0 {
                continue;
            }
            let row_start = Position {
                row: start.row,
                column: 0,
            };
            let to_end = self.terminal.move_rows(row_start, FAR).map_err(failed)?.1;
            let (end, _) = self
                .terminal
                .move_rows(row_start, isize::from(read.max(1)))
                .map_err(failed)?;
            found.push(FoundText {
                to_end: u32::try_from(to_end.unsigned_abs()).unwrap_or(u32::MAX),
                text: self.text(Range {
                    start: row_start,
                    end,
                })?,
            });
        }
        Ok(found)
    }

    /// One step, `op`, after the outcomes `earlier`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per operation, in the list's order"
    )]
    fn step(&mut self, op: &Op, earlier: &[StepOutcome]) -> Found<Done> {
        let failed = |_| StepOutcome::Failed;
        match op {
            Op::Follow(_) => Ok(Done::Follow),
            Op::Visible { .. } => {
                let screen = self.terminal.visible();
                let document = self.terminal.document_range();
                let (text, padded) = Self::trimmed(self.text(screen)?, true);
                let at_text_start = self
                    .terminal
                    .compare(document.start, screen.start)
                    .map_err(failed)?
                    == 0;
                Ok(Done::Visible(VisibleText {
                    start: self.hold(screen.start),
                    end: self.hold(screen.end),
                    at_text_start,
                    text,
                    padded: padded.unwrap_or_default(),
                }))
            }
            Op::ReadText {
                at,
                movement,
                unit,
                count,
                whole,
                trim,
                ..
            } => {
                if *unit != TextUnit::Line
                    || movement.is_some_and(|movement| movement.unit != TextUnit::Line)
                {
                    return Err(StepOutcome::Refused(StepRefusal::NoSuchUnit));
                }
                let from = self.point(at, earlier)?;
                if *whole {
                    let (end, _) = self
                        .terminal
                        .move_rows(from, isize::try_from(*count).unwrap_or(isize::MAX))
                        .map_err(failed)?;
                    let (text, padded) =
                        Self::trimmed(self.text(Range { start: from, end })?, *trim);
                    let chunk = self.chunk(from, text, 0);
                    return Ok(Done::ReadText(Chunks {
                        moved: 0,
                        chunks: vec![chunk],
                        padded,
                    }));
                }
                self.read_lines(
                    from,
                    movement.map(|movement| movement.count),
                    (*count, *trim),
                )
            }
            Op::ReadRange {
                start, end, trim, ..
            } => {
                let (a, b) = (self.point(start, earlier)?, self.point(end, earlier)?);
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                let (text, padded) = Self::trimmed(self.text(Range { start, end })?, *trim);
                Ok(Done::ReadRange(RangeText {
                    text,
                    truncated: false,
                    padded,
                }))
            }
            Op::Move { at, movement, .. } => {
                if movement.unit != TextUnit::Line {
                    return Err(StepOutcome::Refused(StepRefusal::NoSuchUnit));
                }
                let held = matches!(at, Point::At(TextPoint::At(_)));
                let outside = StepOutcome::Refused(StepRefusal::OutsideText);
                let from = match self.point(at, earlier) {
                    Err(StepOutcome::Failed) if held => return Err(outside),
                    other => other?,
                };
                if held {
                    let end = self.terminal.document_range().end;
                    match self.terminal.compare(from, end) {
                        Ok(order) if order <= 0 => {}
                        Ok(_) | Err(_) => return Err(outside),
                    }
                }
                let (to, moved) = self
                    .terminal
                    .move_rows(from, isize::try_from(movement.count).unwrap_or(0))
                    .map_err(failed)?;
                Ok(Done::Move(Moved {
                    moved: i32::try_from(moved).unwrap_or(0),
                    from: self.hold(from),
                    to: self.hold(to),
                }))
            }
            Op::Caret { .. } => {
                let caret = self.hold(self.terminal.caret().start);
                Ok(Done::Caret(CaretAt {
                    caret,
                    selection: None,
                    held: caret,
                    offsets: None,
                }))
            }
            Op::FindText {
                at,
                needle,
                backward,
                unit_start,
                read,
                limit,
                calls,
                ..
            } => {
                let from = self.point(at, earlier)?;
                self.find(
                    from,
                    (needle, *backward),
                    (unit_start.is_some(), *read),
                    (*limit, *calls),
                )
                .map(Done::FindText)
            }
            Op::ReadColours { .. } => {
                let rows = self.terminal.program_colours().map_err(failed)?;
                Ok(Done::ReadColours(
                    rows.into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|run| TerminalColourRun {
                                    column: u32::try_from(run.column).unwrap_or(u32::MAX),
                                    length: u32::try_from(run.length).unwrap_or(u32::MAX),
                                    foreground: run.foreground,
                                    background: run.background,
                                })
                                .collect()
                        })
                        .collect(),
                ))
            }
            Op::WaitForCaret { .. } => Ok(Done::WaitForCaret(self.waited)),
            Op::Locate { .. } => Ok(Done::Locate(Located {
                screen: None,
                offset: None,
            })),
            // The terminals' text cannot be selected.
            Op::Select { .. } => Err(StepOutcome::Refused(StepRefusal::Unsupported)),
            other => panic!("a step the simulated terminal does not answer: {other:?}"),
        }
    }

    /// Every step of `batch`, in order, each run when its gate lets it.
    pub fn run(&mut self, batch: &Batch) -> Vec<StepOutcome> {
        let mut outcomes = Vec::new();
        for step in &batch.steps {
            let outcome = match step.gate(&outcomes).expect("a gate the support can judge") {
                Gate::Skip => StepOutcome::Skipped,
                Gate::Run => match self.step(&step.op, &outcomes) {
                    Ok(done) => StepOutcome::Done(done),
                    Err(outcome) => outcome,
                },
            };
            outcomes.push(outcome);
        }
        outcomes
    }
}

/// What Core asked of the terminal, by the batch's kind: a window read, a
/// read of lines (a review command's or a command mark's), a command
/// mark's search, a caret key's wait with its final window read, or the
/// commit's `Follow`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    Window,
    Lines,
    Search,
    Wait,
    Follow,
}

/// What `batch` asks, by its kind.
fn asked_of(batch: &Batch) -> Asked {
    let ops: Vec<&Op> = batch.steps.iter().map(|step| &step.op).collect();
    match ops.first() {
        Some(Op::Visible { .. }) => Asked::Window,
        Some(Op::WaitForCaret { .. }) => Asked::Wait,
        Some(Op::Follow(_)) => Asked::Follow,
        _ if ops.iter().any(|op| matches!(op, Op::FindText { .. })) => Asked::Search,
        _ if ops
            .iter()
            .all(|op| matches!(op, Op::ReadText { .. } | Op::Move { .. })) =>
        {
            Asked::Lines
        }
        _ => panic!("a batch the support does not name: {batch:?}"),
    }
}

/// One thing Core said: its words, and the key it was spoken for, when it
/// went out on behalf of one ([`Effect::ForKey`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub key: Option<TraceId>,
    pub words: String,
}

/// Something said for `key`.
pub fn said(key: Option<TraceId>, words: &str) -> Said {
    Said {
        key,
        words: words.to_owned(),
    }
}

/// Things said with no key.
pub fn plain(words: &[&str]) -> Vec<Said> {
    words.iter().map(|words| said(None, words)).collect()
}

/// The words of an utterance: its text, its characters, and the debug name
/// of any message or phrase, the index marks left out.
pub fn words(segments: &[verbatim_model::UtteranceSegment]) -> String {
    segments
        .iter()
        .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
        .map(|segment| match &segment.content {
            SegmentContent::Text(text) | SegmentContent::Character(text) => text.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The record a caret key pressed in the terminal is bound to.
pub fn binding() -> CaretBinding {
    CaretBinding {
        node: node(),
        window: ONE.node_window(),
        role: Role::Terminal,
        provider: verbatim_model::TextProvider::Uia,
        record: None,
        formatting: verbatim_model::TextAttributes::default(),
    }
}

/// Caret key `key` pressed in the terminal, passed at once.
pub fn caret_key(key: CaretKey) -> PressAction {
    PressAction::CaretKey {
        key,
        held: None,
        bound: Some(Box::new(binding())),
    }
}

/// The focused terminal read through Core's event queue, with playback that
/// reaches each utterance's marks as it plays.
pub struct Session {
    pub desk: Desk,
    pub sim: Sim,
    /// What was said, in order.
    pub heard: Vec<Said>,
    /// Every request Core made of the terminal, in order.
    pub requests: Vec<Asked>,
    /// The text Core copied to the clipboard, in order.
    pub copied: Vec<String>,
    /// Whether what Core queues is played only when the test calls
    /// [`Session::play`] or [`Session::play_one`], so that output can be
    /// written while a group plays.
    pub manual_play: bool,
    /// Output the terminal prints before the outpost answers the next
    /// request of a kind, the next taken for each: written, and told to
    /// Core, at its time, as when a program keeps printing while a request
    /// is on its way.
    pub output_before: VecDeque<(Asked, String, u64)>,
    /// Requests sent and not yet answered, in the order Core sent them.
    pending: VecDeque<(u64, Batch)>,
    /// Utterances queued and their marks, not yet played.
    queued: VecDeque<(Vec<SpeechMark>, Said)>,
    /// The wake Core last asked for, in microseconds.
    pub wake: Option<u64>,
}

impl Session {
    /// A terminal of `flavor`, `width` by `height` with `history` rows,
    /// showing `text`, focused and read twice, all of which is let pass
    /// unasserted.
    pub fn new(
        flavor: Flavor,
        (width, height, history): (usize, usize, usize),
        text: &str,
    ) -> Self {
        let mut sim = Sim::new(flavor, width, height, history);
        sim.terminal.write(text.as_bytes());
        let mut desk = Desk::new();
        let terminal = NodeSnapshot {
            id: node(),
            backend: Backend::Uia,
            role: Role::Terminal,
            name: Some("Terminal".to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let _ = desk.focus_read(ONE, focus_read_of(ONE, &terminal, &[]));
        let pending = desk
            .sent
            .iter()
            .map(|(number, _, question)| {
                let Question::Batch(batch) = question;
                (*number, batch.clone())
            })
            .collect();
        let mut session = Self {
            desk,
            sim,
            heard: Vec::new(),
            requests: Vec::new(),
            copied: Vec::new(),
            manual_play: false,
            output_before: VecDeque::new(),
            pending,
            queued: VecDeque::new(),
            wake: None,
        };
        session.run();
        // A read at the screen's height, once the baseline found it, so
        // that every later read spans the same two screens: the baseline,
        // taken before the height is known, reads the screen alone.
        session.told(2);
        session.play();
        session.heard.clear();
        session.requests.clear();
        session
    }

    /// The session's time, in milliseconds.
    pub fn at_ms(&self) -> u64 {
        self.desk.at_us / 1_000
    }

    /// Feeds `input` at the session's time and takes in its effects.
    fn feed(&mut self, input: QueueInput) {
        let effects = self.desk.feed(input);
        self.take(effects, None);
    }

    /// Feeds `input`, answers what it asked, and plays what was queued.
    pub fn queue(&mut self, input: QueueInput) {
        self.feed(input);
        self.run();
        self.settle();
    }

    /// Applies `settings`, as the shell gives Core the settings.
    pub fn settings(&mut self, settings: ReaderSettings) {
        let input = Input::Settings(settings);
        let effects = verbatim_core::reduce(&mut self.desk.state, &mut self.desk.runtime, &input);
        self.take(effects, None);
        self.run();
        self.settle();
    }

    /// Both flood limits at `limit`.
    pub fn limits(&mut self, limit: u16) {
        self.settings(ReaderSettings {
            terminal_full_lines: verbatim_model::TerminalLines::new(limit),
            terminal_last_lines: verbatim_model::TerminalLines::new(limit),
            ..ReaderSettings::default()
        });
    }

    /// A key press doing `action` at `at_ms`, under `trace_id`.
    pub fn press_at(&mut self, at_ms: u64, trace_id: TraceId, action: PressAction) {
        self.desk.at_us = at_ms * 1_000;
        self.desk.keys += 1;
        let input = Input::Queue {
            trace_id,
            at_us: self.desk.at_us,
            input: QueueInput::Press(Box::new(Press {
                key_seq: KeySeq::new(self.desk.keys),
                pressed_at_us: self.desk.at_us,
                action,
            })),
        };
        let effects = verbatim_core::reduce(&mut self.desk.state, &mut self.desk.runtime, &input);
        self.take(effects, None);
        self.run();
        self.settle();
    }

    /// Review command `command`, its `repeat`th quick press, a millisecond
    /// on.
    pub fn command(&mut self, command: verbatim_model::ReviewCommand, repeat: u8) {
        let at = self.at_ms() + 1;
        self.press_at(
            at,
            TraceId::from_raw(1),
            PressAction::Command { command, repeat },
        );
    }

    /// Writes `bytes` to the terminal, its change told to Core at `at_ms`.
    pub fn write(&mut self, bytes: &str, at_ms: u64) {
        self.sim.terminal.write(bytes.as_bytes());
        self.told(at_ms);
    }

    /// The terminal's change told to Core at `at_ms`, as its outpost tells
    /// one.
    pub fn told(&mut self, at_ms: u64) {
        self.desk.at_us = at_ms * 1_000;
        let tell = Event {
            trace_id: TraceId::from_raw(3),
            kind: EventKind::TerminalChanged,
            process: ONE.pid,
            application: "terminal".to_owned(),
            thread: 41,
            at_us: self.desk.at_us,
            object: EventObject::Terminal {
                node: node(),
                window: ONE.window(),
            },
            cached: PropertyValues::default(),
            payload: Payload::None,
            window: TellWindow::NoWindow,
            checked: None,
        };
        self.queue(QueueInput::Tell {
            from: ONE.instance(),
            tell: Box::new(tell),
        });
    }

    /// The queue's wake fed, at the time Core asked for.
    pub fn due(&mut self) {
        let wake = self.wake.take().expect("Core asked to be woken");
        self.desk.at_us = self.desk.at_us.max(wake);
        self.queue(QueueInput::Due);
    }

    /// Answers every request out, in the order Core sent them.
    fn run(&mut self) {
        while let Some((number, batch)) = self.pending.pop_front() {
            let asked = asked_of(&batch);
            if let Some(at) = self
                .output_before
                .iter()
                .position(|(kind, _, _)| *kind == asked)
                && let Some((_, output, at_ms)) = self.output_before.remove(at)
            {
                self.sim.terminal.write(output.as_bytes());
                self.desk.at_us = at_ms * 1_000;
                let tell = Event {
                    trace_id: TraceId::from_raw(3),
                    kind: EventKind::TerminalChanged,
                    process: ONE.pid,
                    application: "terminal".to_owned(),
                    thread: 41,
                    at_us: self.desk.at_us,
                    object: EventObject::Terminal {
                        node: node(),
                        window: ONE.window(),
                    },
                    cached: PropertyValues::default(),
                    payload: Payload::None,
                    window: TellWindow::NoWindow,
                    checked: None,
                };
                self.feed(QueueInput::Tell {
                    from: ONE.instance(),
                    tell: Box::new(tell),
                });
            }
            let outcomes = self.sim.run(&batch);
            self.feed(QueueInput::Answer {
                from: ONE.instance(),
                answer: Answer {
                    number: RequestNumber(number),
                    outcome: Outcome::Result(Reply::Steps(outcomes)),
                    probes: Vec::new(),
                    detail: String::new(),
                },
            });
        }
    }

    /// Takes in effects: speech is queued, or heard at once when it has no
    /// marks and nothing is queued ahead of it; requests are kept for the
    /// outpost; copies are recorded. Any other effect fails the test.
    fn take(&mut self, effects: Vec<Effect>, key: Option<TraceId>) {
        for effect in effects {
            match effect {
                Effect::ForKey(key, effect) => self.take(vec![*effect], Some(key)),
                Effect::Speak(utterance, _) => {
                    let marks: Vec<SpeechMark> = utterance
                        .segments
                        .iter()
                        .filter_map(|segment| match segment.content {
                            SegmentContent::Mark(mark) => Some(mark),
                            _ => None,
                        })
                        .collect();
                    let said = Said {
                        key,
                        words: words(&utterance.segments),
                    };
                    if marks.is_empty() && self.queued.is_empty() {
                        self.heard.push(said);
                    } else {
                        self.queued.push_back((marks, said));
                    }
                }
                Effect::StopSpeech => self.queued.clear(),
                Effect::CopyToClipboard(text) => self.copied.push(text),
                Effect::Queue(QueueEffect::Send { to, ask }) => {
                    assert_eq!(to, ONE.instance(), "a request to the terminal's outpost");
                    let Question::Batch(batch) = ask.request;
                    let asked = asked_of(&batch);
                    if asked != Asked::Follow {
                        self.requests.push(asked);
                    }
                    self.pending.push_back((ask.number.0, batch));
                }
                Effect::Queue(QueueEffect::WakeAt(wake)) => self.wake = wake,
                Effect::Queue(_)
                | Effect::DropExpiredSpeech(_)
                | Effect::Observed(_)
                | Effect::PlayEarcon(_)
                | Effect::SettingsChanged(_) => {}
                other => panic!("unexpected effect: {other:?}"),
            }
        }
    }

    /// Plays what is queued, unless the test plays it.
    fn settle(&mut self) {
        if !self.manual_play {
            self.play();
        }
    }

    /// Plays every utterance queued, in order, reaching its marks.
    pub fn play(&mut self) {
        while self.play_one() {}
    }

    /// Plays the next utterance queued, reaching its marks; `false` when
    /// nothing is queued.
    pub fn play_one(&mut self) -> bool {
        let Some((marks, said)) = self.queued.pop_front() else {
            return false;
        };
        self.heard.push(said);
        for mark in marks {
            self.feed(QueueInput::MarkReached(mark));
            self.run();
        }
        true
    }

    /// What was heard since the last [`Session::heard`], taken.
    pub fn heard(&mut self) -> Vec<Said> {
        std::mem::take(&mut self.heard)
    }

    /// The requests made since the last [`Session::requests`], taken.
    pub fn requests(&mut self) -> Vec<Asked> {
        std::mem::take(&mut self.requests)
    }
}
