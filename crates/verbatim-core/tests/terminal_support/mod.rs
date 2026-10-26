//! A terminal Core reads, for the tests and measurements of Core's terminal
//! model: the simulated terminal (`verbatim-term-sim`) read as the outpost
//! reads a real one once it reads for Core (`docs/design/terminal-model.md`,
//! "Where it runs"), and a reducer driven with its snapshots, answering
//! Core's requests for reads, history, colours and the screen's shape, and
//! playing the speech Core queues by reaching each utterance's marks.

#![allow(dead_code, reason = "each test file uses part of the support")]

use std::collections::VecDeque;

use verbatim_core::{SrState, reduce, snapshot_of};
use verbatim_model::{
    Backend, Effect, Input, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid,
    QueryId, Role, SegmentContent, SpeechMark, StateSet, TerminalScope, TerminalSnapshot, TextOp,
    TextReply, TextRequest, TraceId,
};
use verbatim_term_sim::{Flavor, Query, Range, Terminal};
use verbatim_text::terminal_rows::{Read, ReadText, shows_text};

pub const OUTPOST: OutpostId = OutpostId(1);
pub const TERMINAL: u64 = 12;

/// The terminal's node.
pub fn node() -> NodeId {
    NodeId::in_outpost(OUTPOST, TERMINAL)
}

/// An event from the terminal's outpost.
pub fn event(event: NormalizedEvent) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
    }
}

/// The focus on the terminal.
pub fn focus() -> Input {
    event(NormalizedEvent::FocusChanged {
        node: NodeSnapshot {
            id: node(),
            backend: Backend::Uia,
            role: Role::Terminal,
            name: Some("Terminal".to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        },
        foreground: false,
        ancestors: Vec::new(),
        ancestors_unknown: false,
        selected_child: None,
        text: None,
    })
}

/// The simulated terminal read as the outpost reads it for Core: each read
/// starts where the last read whose screen showed text said to start, a
/// screen of rows above it, at the height Core told; the first read takes
/// the screen alone; every read that begins by the latest end of a key's
/// window Core asked colours for takes them, and a request that comes after
/// a read ended at or after its key without them has a read taken at once
/// with them, sent as a live read; and a live read identical to the last
/// read sent, but for its times and caret, is not sent.
pub struct Outpost {
    pub sim: Terminal,
    /// The screen's width in cells.
    width: usize,
    /// Where the next read starts.
    start: Option<Range>,
    /// The reads taken, which number each.
    reads: u64,
    /// The read whose start is kept, as Core knows it: the last read sent
    /// when the read that set it was not sent, being identical to it.
    start_read: u64,
    /// The number of the last read sent.
    sent_read: u64,
    /// The screen's height Core told, 0 before it did.
    height: u32,
    /// Whether the last read kept for its start showed text.
    showed_text: bool,
    /// The latest end of a key's window Core asked colours for.
    colours_until: u64,
    /// Whether the next read takes the colours whenever it begins.
    colours_now: bool,
    /// When the last read ended, and whether it took the colours.
    read_ended: Option<(u64, bool)>,
    /// The last read sent, its times and caret cleared.
    sent: Option<TerminalSnapshot>,
    /// A read taken at once for a colours request, for Core as a live read.
    pub reread: Option<TerminalSnapshot>,
}

impl Outpost {
    /// A terminal of `flavor`, `width` by `height`, with `history` rows.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self {
            sim: Terminal::new(flavor, width, height, history),
            width,
            start: None,
            reads: 0,
            start_read: 0,
            sent_read: 0,
            height: 0,
            showed_text: false,
            colours_until: 0,
            colours_now: false,
            read_ended: None,
            sent: None,
            reread: None,
        }
    }

    /// A snapshot as compared with the last sent: without its times, its
    /// number and its caret.
    fn compared(snapshot: &TerminalSnapshot) -> TerminalSnapshot {
        TerminalSnapshot {
            started_ms: 0,
            ended_ms: 0,
            read: 0,
            caret: None,
            ..snapshot.clone()
        }
    }

    /// Whether a live read is sent, noting it as the last sent when it is.
    pub fn sends(&mut self, snapshot: &TerminalSnapshot) -> bool {
        let compared = Self::compared(snapshot);
        if !snapshot.waited && self.sent.as_ref() == Some(&compared) {
            if self.start_read == snapshot.read {
                self.start_read = self.sent_read;
            }
            return false;
        }
        self.sent = Some(compared);
        self.sent_read = snapshot.read;
        true
    }

    /// Notes an answer, which is always sent, as the last sent.
    pub fn answered(&mut self, snapshot: &TerminalSnapshot) {
        self.sent = Some(Self::compared(snapshot));
        self.sent_read = snapshot.read;
    }

    /// The read the rules take of a read of `scope` begun at `at_ms`, and
    /// the snapshot it is sent as; `None` for a live read.
    pub fn read(&mut self, scope: Option<TerminalScope>, at_ms: u64) -> (Read, TerminalSnapshot) {
        let query = match scope {
            Some(TerminalScope::Whole) => Query {
                start: None,
                screen_rows: self.height,
                whole: true,
                above: 0,
            },
            Some(TerminalScope::HistoryBlock { height }) => {
                self.height = height;
                Query {
                    start: None,
                    screen_rows: height,
                    whole: false,
                    above: height,
                }
            }
            None | Some(TerminalScope::Fresh) => Query {
                start: self.start,
                screen_rows: self.height,
                whole: false,
                above: self.height,
            },
        };
        let snapshot = self.sim.snapshot(&query).expect("a simulated read");
        let read = Read {
            text: ReadText::unpadded(&snapshot.text),
            screen: snapshot.screen,
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: snapshot.first_row,
            caret_rows_to_end: snapshot.caret_rows_to_end,
            caret_line: snapshot.caret_line,
            caret_column: snapshot.caret_column,
            at_ms,
            above: usize::try_from(snapshot.above).expect("rows"),
        };
        // A read whose screen shows no text where the last kept read's did
        // leaves where the next read starts ("A read with no text").
        let shows = shows_text(&read.screen);
        self.reads += 1;
        if shows || !self.showed_text {
            self.start = Some(snapshot.next_start);
            self.start_read = self.reads;
            self.showed_text = shows;
        }
        let colours = (self.colours_now || at_ms <= self.colours_until).then(|| self.colours());
        self.colours_now = false;
        self.read_ended = Some((at_ms, colours.is_some()));
        let mut wire = snapshot_of(&read, false, colours.as_deref());
        wire.read = self.reads;
        (read, wire)
    }

    /// The search the outpost's find program makes
    /// (`verbatim_uia_rops::terminal_find`): the rows starting with
    /// `needle`, outward from the row `from` rows above where the next read
    /// starts, alternately the next at or below it and the next above it,
    /// at most 20, each with `rows` rows of text read from it, and the read
    /// whose start they are counted from.
    fn find(&self, needle: &str, from: i32, rows: u32) -> TextReply {
        // The document from its start, and how many rows lie above the
        // start, read by moving that far up from it.
        let query = Query {
            start: self.start,
            screen_rows: self.height,
            whole: false,
            above: u32::MAX / 2,
        };
        let snapshot = self.sim.snapshot(&query).expect("a simulated read");
        let all = verbatim_text::terminal_rows::rows_of(&snapshot.text, self.width);
        let start = i64::from(snapshot.above);
        let boundary = start - i64::from(from);
        let text_of = |row: usize| {
            let mut text = String::new();
            for found in all.iter().skip(row).take(rows as usize) {
                text.push_str(&found.text);
                if !found.wraps {
                    text.push_str("\r\n");
                }
            }
            text
        };
        let at = |row: &usize| i64::try_from(*row).expect("rows");
        let starts = |row: &usize| all[*row].text.starts_with(needle);
        let mut below = (0..all.len())
            .filter(|row| at(row) >= boundary)
            .filter(starts);
        let mut above = (0..all.len())
            .rev()
            .filter(|row| at(row) < boundary)
            .filter(starts);
        let mut taken = Vec::new();
        let mut going_down = true;
        while taken.len() < 20 {
            let next = if going_down {
                below.next().or_else(|| above.next())
            } else {
                above.next().or_else(|| below.next())
            };
            let Some(row) = next else {
                break;
            };
            taken.push(row);
            going_down = !going_down;
        }
        TextReply::TerminalFound {
            matches: taken
                .into_iter()
                .map(|row| verbatim_model::TerminalMatch {
                    above: i32::try_from(start - at(&row)).expect("rows"),
                    text: text_of(row),
                })
                .collect(),
            read: self.start_read,
        }
    }

    /// The screen's colours as the read program gives them.
    fn colours(&self) -> Vec<Vec<verbatim_text::terminal_reading::ColourRun>> {
        self.sim
            .colours()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|run| verbatim_text::terminal_reading::ColourRun {
                        column: run.column,
                        length: run.length,
                        foreground: run.foreground,
                        background: run.background,
                    })
                    .collect()
            })
            .collect()
    }

    /// The outpost's answer to a request Core makes of a terminal it reads,
    /// a read begun at `at_ms`; `None` for any other request.
    pub fn answer(&mut self, op: &TextOp, at_ms: u64) -> Option<TextReply> {
        Some(match op {
            TextOp::TerminalReadNow(scope) => {
                let (_, snapshot) = self.read(Some(*scope), at_ms);
                self.answered(&snapshot);
                TextReply::TerminalSnapshot(Box::new(snapshot))
            }
            TextOp::TerminalHistory { above, whole } => {
                let query = Query {
                    start: if *whole { None } else { self.start },
                    screen_rows: self.height,
                    whole: *whole,
                    above: if *whole { 0 } else { *above },
                };
                let snapshot = self.sim.snapshot(&query).expect("a simulated history read");
                TextReply::TerminalHistory {
                    text: snapshot.text,
                    above: snapshot.above,
                }
            }
            TextOp::TerminalFind { needle, from, rows } => self.find(needle, *from, *rows),
            TextOp::TerminalShape { height, .. } => {
                self.height = *height;
                TextReply::Done
            }
            TextOp::TerminalColours { from_ms, until_ms } => {
                self.colours_until = self.colours_until.max(*until_ms);
                if self
                    .read_ended
                    .is_some_and(|(ended, colours)| ended >= *from_ms && !colours)
                {
                    self.colours_now = true;
                    self.reread = Some(self.read(None, at_ms).1);
                }
                TextReply::Done
            }
            _ => return None,
        })
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

/// A reducer reading the simulated terminal through Core's terminal model,
/// with playback that reaches each utterance's marks as it plays.
pub struct Session {
    pub state: SrState,
    pub outpost: Outpost,
    /// What was said, in order.
    pub heard: Vec<Said>,
    /// Every request Core made, in order.
    pub requests: Vec<TextOp>,
    /// Every timer Core armed, in order.
    pub timers: Vec<Option<u64>>,
    /// The text Core copied to the clipboard, in order.
    pub copied: Vec<String>,
    /// Every input given to the reducer since the session began, with its
    /// effects.
    pub log: Vec<(Input, Vec<Effect>)>,
    /// Whether the outpost holds back a whole read Core asks for, until
    /// [`Session::answer_held`].
    pub hold_whole: bool,
    held: Option<(QueryId, TerminalScope)>,
    /// The caret read before a held key, held back until
    /// [`Session::release_held`].
    caret_read: Option<QueryId>,
    /// Whether the outpost holds back Core's colours requests, as they
    /// travel to it, until [`Session::deliver_colours`].
    pub hold_colours: bool,
    /// Whether the outpost answers Core's searches unanswered, as its
    /// worker does when the terminal lost the focus before searching.
    pub unanswered_finds: bool,
    /// Output the terminal prints before each search Core asks for, the
    /// next taken for each: written, and read by a live read sent to Core,
    /// at its time, before the outpost searches, as when a program keeps
    /// printing while a search is on its way.
    pub output_before_finds: VecDeque<(String, u64)>,
    held_colours: VecDeque<(QueryId, TextOp)>,
    /// Requests not yet answered, in the order Core made them: the outpost
    /// answers each once Core's step is done, in order.
    pending: VecDeque<(QueryId, TextOp)>,
    /// Utterances queued and their marks, not yet played.
    queued: VecDeque<(Vec<SpeechMark>, Said)>,
    /// The time reads Core asks for begin at.
    pub at_ms: u64,
}

impl Session {
    /// A terminal of `flavor`, `width` by `height` with `history` rows,
    /// focused and read once at time 0.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self::with(Outpost::new(flavor, width, height, history))
    }

    /// `outpost`'s terminal, focused and read once at time 0, all of which
    /// is let pass unasserted: the focus and the first read say nothing of
    /// the terminal's output.
    pub fn with(outpost: Outpost) -> Self {
        let mut session = Self {
            state: SrState::new(),
            outpost,
            heard: Vec::new(),
            requests: Vec::new(),
            timers: Vec::new(),
            copied: Vec::new(),
            log: Vec::new(),
            hold_whole: false,
            held: None,
            caret_read: None,
            hold_colours: false,
            unanswered_finds: false,
            output_before_finds: VecDeque::new(),
            held_colours: VecDeque::new(),
            pending: VecDeque::new(),
            queued: VecDeque::new(),
            at_ms: 0,
        };
        session.input(&focus());
        session.live(0);
        session.heard.clear();
        session.requests.clear();
        session.timers.clear();
        session
    }

    /// One step of the reducer, logged.
    fn step(&mut self, input: Input) -> Vec<Effect> {
        let effects = reduce(&mut self.state, &input);
        self.log.push((input, effects.clone()));
        effects
    }

    /// Feeds `input`, takes in its effects, and plays what it queued.
    pub fn input(&mut self, input: &Input) {
        let effects = self.step(input.clone());
        self.run(effects);
        self.play();
    }

    /// Takes in `effects`, then answers the requests they and their answers
    /// made, in order.
    fn run(&mut self, effects: Vec<Effect>) {
        self.take(effects, None);
        while let Some((query_id, op)) = self.pending.pop_front() {
            self.answer(query_id, &op);
        }
    }

    /// The outpost's answer to `op`, and the read it took at once for it,
    /// if any, sent as a live read.
    fn answer(&mut self, query_id: QueryId, op: &TextOp) {
        if matches!(op, TextOp::TerminalFind { .. })
            && let Some((output, at_ms)) = self.output_before_finds.pop_front()
        {
            self.write(&output, at_ms);
        }
        if self.unanswered_finds && matches!(op, TextOp::TerminalFind { .. }) {
            self.reply(query_id, TextReply::Unanswered);
        } else if let Some(reply) = self.outpost.answer(op, self.at_ms) {
            self.reply(query_id, reply);
        }
        if let Some(snapshot) = self.outpost.reread.take() {
            self.send(snapshot);
        }
    }

    /// Delivers the colours requests held back, arriving at `at_ms`.
    pub fn deliver_colours(&mut self, at_ms: u64) {
        self.at_ms = at_ms;
        while let Some((query_id, op)) = self.held_colours.pop_front() {
            self.answer(query_id, &op);
        }
        self.play();
    }

    /// Writes `bytes` to the terminal, then a live read begun at `at_ms`.
    pub fn write(&mut self, bytes: &str, at_ms: u64) {
        self.outpost.sim.write(bytes.as_bytes());
        self.live(at_ms);
    }

    /// A live read begun at `at_ms`, sent to Core as the outpost sends it.
    pub fn live(&mut self, at_ms: u64) {
        self.at_ms = at_ms;
        let (_, snapshot) = self.outpost.read(None, at_ms);
        self.send(snapshot);
    }

    /// Sends a live read to Core, unless it is the one last sent.
    fn send(&mut self, snapshot: TerminalSnapshot) {
        if self.outpost.sends(&snapshot) {
            self.input(&event(NormalizedEvent::TerminalSnapshot {
                node_id: node(),
                snapshot: Box::new(snapshot),
            }));
        }
    }

    /// Answers the whole read held back.
    pub fn answer_held(&mut self) {
        let (query_id, scope) = self.held.take().expect("a whole read held back");
        let reply = self
            .outpost
            .answer(&TextOp::TerminalReadNow(scope), self.at_ms)
            .expect("the outpost answers its reads");
        let effects = self.completed(query_id, reply);
        self.run(effects);
        self.play();
    }

    /// Answers the caret read before a held key as failed, so the key is
    /// passed on with the hook's time.
    pub fn release_held(&mut self) {
        let query_id = self.caret_read.take().expect("a caret read held back");
        let effects = self.completed(query_id, TextReply::Unanswered);
        self.run(effects);
        self.play();
    }

    /// Answers the caret read before a held key as a terminal's outpost
    /// does, with a read begun at `at_ms` that carries `caret`.
    pub fn answer_caret_read(&mut self, at_ms: u64, caret: verbatim_model::CaretReport) {
        let query_id = self.caret_read.take().expect("a caret read held back");
        self.at_ms = at_ms;
        let (_, mut snapshot) = self.outpost.read(None, at_ms);
        snapshot.caret = Some(caret);
        self.outpost.answered(&snapshot);
        let effects = self.completed(query_id, TextReply::TerminalSnapshot(Box::new(snapshot)));
        self.run(effects);
        self.play();
    }

    fn completed(&mut self, query_id: QueryId, reply: TextReply) -> Vec<Effect> {
        self.step(Input::TextCompleted {
            trace_id: TraceId::mint(),
            query_id,
            reply,
        })
    }

    fn reply(&mut self, query_id: QueryId, reply: TextReply) {
        let effects = self.completed(query_id, reply);
        self.take(effects, None);
    }

    /// Takes in effects: speech is queued, or heard at once when it has no
    /// marks; requests are kept for the outpost, which answers those of the
    /// terminal and leaves others, such as a caret key's watch, unanswered;
    /// timers and copies are recorded. Any other effect fails the test.
    fn take(&mut self, effects: Vec<Effect>, key: Option<TraceId>) {
        for effect in effects {
            match effect {
                Effect::ForKey(key, effect) => self.take(vec![*effect], Some(key)),
                Effect::Speak(utterance) => {
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
                    if marks.is_empty() {
                        self.heard.push(said);
                    } else {
                        self.queued.push_back((marks, said));
                    }
                }
                Effect::StopSpeech => self.queued.clear(),
                Effect::TerminalTimer { due_ms } => self.timers.push(due_ms),
                Effect::CopyToClipboard(text) => self.copied.push(text),
                Effect::Text(TextRequest { query_id, op, .. }) => {
                    self.requests.push(op.clone());
                    match op {
                        TextOp::TerminalReadNow(scope @ TerminalScope::Whole)
                            if self.hold_whole =>
                        {
                            self.held = Some((query_id, scope));
                        }
                        TextOp::ReadCaret => self.caret_read = Some(query_id),
                        op @ TextOp::TerminalColours { .. } if self.hold_colours => {
                            self.held_colours.push_back((query_id, op));
                        }
                        op => self.pending.push_back((query_id, op)),
                    }
                }
                // Passing a held key on is the shell's; the focus's own
                // effects are not these tests'.
                Effect::PassHeldKey(_)
                | Effect::DropExpiredSpeech(_)
                | Effect::Fetch(_)
                | Effect::PlayEarcon(_) => {}
                other => panic!("unexpected effect: {other:?}"),
            }
        }
    }

    /// Plays every utterance queued, in order, reaching its marks.
    pub fn play(&mut self) {
        while let Some((marks, said)) = self.queued.pop_front() {
            self.heard.push(said);
            for mark in marks {
                let effects = self.step(Input::MarkReached { mark });
                self.run(effects);
            }
        }
    }

    /// What was heard since the last [`Session::heard`], taken.
    pub fn heard(&mut self) -> Vec<Said> {
        std::mem::take(&mut self.heard)
    }

    /// The requests made since the last [`Session::requests`], taken.
    pub fn requests(&mut self) -> Vec<TextOp> {
        std::mem::take(&mut self.requests)
    }
}
