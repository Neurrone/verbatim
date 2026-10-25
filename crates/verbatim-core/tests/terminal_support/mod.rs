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
use verbatim_text::terminal_rows::{Read, shows_text};

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
/// the screen alone; and the first read after a key takes the colours.
pub struct Outpost {
    pub sim: Terminal,
    /// Where the next read starts.
    start: Option<Range>,
    /// The screen's height Core told, 0 before it did.
    height: u32,
    /// Whether the last read kept for its start showed text.
    showed_text: bool,
    /// The earliest time a key reached the program since the last read
    /// with colours: the first read that begins at or after it takes them.
    colours_since: Option<u64>,
}

impl Outpost {
    /// A terminal of `flavor`, `width` by `height`, with `history` rows.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self {
            sim: Terminal::new(flavor, width, height, history),
            start: None,
            height: 0,
            showed_text: false,
            colours_since: None,
        }
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
            text: snapshot.text,
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
        if shows || !self.showed_text {
            self.start = Some(snapshot.next_start);
            self.showed_text = shows;
        }
        let colours = self
            .colours_since
            .take_if(|since| *since <= at_ms)
            .map(|_| self.colours());
        let wire = snapshot_of(&read, false, colours.as_deref());
        (read, wire)
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
                TextReply::TerminalSnapshot(Box::new(self.read(Some(*scope), at_ms).1))
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
            TextOp::TerminalShape { height, .. } => {
                self.height = *height;
                TextReply::Done
            }
            TextOp::TerminalColours { since_ms } => {
                self.colours_since = Some(
                    self.colours_since
                        .map_or(*since_ms, |since| since.min(*since_ms)),
                );
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
            if let Some(reply) = self.outpost.answer(&op, self.at_ms) {
                self.reply(query_id, reply);
            }
        }
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
        self.input(&event(NormalizedEvent::TerminalSnapshot {
            node_id: node(),
            snapshot: Box::new(snapshot),
        }));
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
