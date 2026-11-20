//! A terminal Core reads, for the tests and measurements of Core's terminal
//! model: the simulated terminal (`verbatim-term-sim`) read as the outpost
//! reads a real one (`docs/design/readers-tell-core-asks-build3.md`), and a
//! reducer driven through its event queue: the terminal committed as the
//! focus at startup, every write told to Core as the outpost tells a
//! change, Core's requests for full reads, history reads and searches
//! answered from the simulated terminal from the start anchors Core names,
//! in the order Core sends them, and the speech Core queues played by
//! reaching each utterance's marks.

#![allow(dead_code, reason = "each test file uses part of the support")]

use std::collections::{BTreeMap, VecDeque};

use verbatim_core::{SrState, reduce, snapshot_of};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, CaretBinding, FocusCaret, FocusNode, FocusRead, ForegroundReport, Instance,
    NodeRead, NodeWindow, Outcome, Press, PressAction, QueueEffect, QueueInput, Reader, Refusal,
    Reply, Request as Wire,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    CaretKey, Earcon, Effect, Input, KeySeq, NodeId, OutpostId, Pid, Role, SegmentContent,
    SpeechMark, TerminalRead, TerminalReadReply, TerminalScope, TextAnchor, TextOp, TextReply,
    TraceId, WindowHandle,
};
use verbatim_term_sim::{Flavor, Query, Range, Terminal};
use verbatim_text::terminal_rows::{Read, ReadText};

pub const TERMINAL: u64 = 12;

/// The terminal's application.
pub const APP: Pid = Pid(1);

/// The terminal's outpost.
pub const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: 1,
};

/// The terminal's node.
pub fn node() -> NodeId {
    NodeId::in_outpost(OutpostId(OUTPOST.generation), TERMINAL)
}

/// The terminal's window, as a request names it.
pub fn window() -> NodeWindow {
    NodeWindow {
        handle: WindowHandle(5),
        top_level: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: APP,
        thread: 41,
        desktop: false,
    }
}

/// The record a caret key pressed in the terminal is bound to.
pub fn binding() -> CaretBinding {
    CaretBinding {
        node: node(),
        window: window(),
        role: Role::Terminal,
        provider: verbatim_model::TextProvider::Uia,
        record: None,
        formatting: verbatim_model::TextAttributes::default(),
    }
}

/// A key a test presses: its trace, when it was pressed, and what the
/// hook reports it does.
#[derive(Clone, Debug)]
pub struct Key {
    pub trace_id: TraceId,
    pub at_ms: u64,
    pub action: PressAction,
}

/// Caret key `key` pressed at `at_ms` in the terminal, passed at once, or
/// held back when `held`.
pub fn caret(trace_id: TraceId, key: CaretKey, at_ms: u64, held: bool) -> Key {
    let held = held.then(|| verbatim_model::stroke::HeldStroke {
        stroke: verbatim_model::stroke::Stroke {
            vk: 0x26,
            scan_code: 0x48,
            extended: true,
            sequence: 0,
        },
        modifiers: Vec::new(),
    });
    Key {
        trace_id,
        at_ms,
        action: PressAction::CaretKey {
            key,
            held,
            bound: Some(Box::new(binding())),
        },
    }
}

/// What Core asked of the terminal beside its output reads: a full read
/// inside the event queue (a review command's fresh read or a caret key's
/// final read), a caret key's starting read, a read the rules owe (the
/// whole document or a first read again with its history block), a
/// history read, a search, or a caret key's watch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    Fresh,
    Starting,
    Whole,
    HistoryBlock(u32),
    History {
        above: u32,
        whole: bool,
    },
    Find {
        needle: String,
        from: i32,
        rows: u32,
    },
    Watch,
}

/// The simulated terminal read as the outpost reads it for Core: each read
/// starts a screen above the start anchor Core names, at the height Core
/// names, and mints the anchor at the top of its own screen; a first read
/// takes the screen alone; a read Core asks the colours of takes them.
pub struct Outpost {
    pub sim: Terminal,
    /// The screen's width in cells.
    width: usize,
    /// The start anchors minted, by number.
    anchors: BTreeMap<u64, Range>,
    /// The last anchor minted.
    minted: u64,
}

impl Outpost {
    /// A terminal of `flavor`, `width` by `height`, with `history` rows.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self {
            sim: Terminal::new(flavor, width, height, history),
            width,
            anchors: BTreeMap::new(),
            minted: 0,
        }
    }

    /// The range Core names by `anchor`.
    fn range(&self, anchor: Option<TextAnchor>) -> Option<Range> {
        anchor.and_then(|anchor| self.anchors.get(&anchor.0).copied())
    }

    /// Mints an anchor at `range`.
    fn mint(&mut self, range: Range) -> TextAnchor {
        self.minted += 1;
        self.anchors.insert(self.minted, range);
        TextAnchor(self.minted)
    }

    /// The read `request` asks, begun at `at_ms`: the rules' read and the
    /// answer the outpost sends.
    pub fn read(&mut self, request: &TerminalRead, at_ms: u64) -> (Read, TerminalReadReply) {
        let query = match request.scope {
            TerminalScope::Whole => Query {
                start: None,
                screen_rows: request.height,
                whole: true,
                above: 0,
            },
            TerminalScope::HistoryBlock { height } => Query {
                start: None,
                screen_rows: height,
                whole: false,
                above: height,
            },
            TerminalScope::Fresh => Query {
                start: self.range(request.start),
                screen_rows: request.height,
                whole: false,
                above: request.height,
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
        let colours = request.colours.then(|| self.colours());
        let wire = snapshot_of(&read, false, colours.as_deref());
        let start = self.mint(snapshot.next_start);
        let point = request.starting.map(|_| self.mint(snapshot.next_start));
        let reply = TerminalReadReply {
            snapshot: wire,
            start,
            started_us: at_ms * 1_000,
            point,
        };
        (read, reply)
    }

    /// The history read from `above` rows above the start `start`, or the
    /// whole document.
    fn history(&self, start: TextAnchor, (above, whole, height): (u32, bool, u32)) -> TextReply {
        let query = Query {
            start: if whole { None } else { self.range(Some(start)) },
            screen_rows: height,
            whole,
            above: if whole { 0 } else { above },
        };
        let snapshot = self.sim.snapshot(&query).expect("a simulated history read");
        TextReply::TerminalHistory {
            text: snapshot.text,
            above: snapshot.above,
        }
    }

    /// The search the outpost's find program makes
    /// (`verbatim_uia_rops::terminal_find`): the rows starting with
    /// `needle`, outward from the row `from` rows above the start `start`,
    /// alternately the next at or below it and the next above it, at most
    /// 20, each with `rows` rows of text read from it.
    fn find(&self, start: TextAnchor, needle: &str, (from, rows): (i32, u32)) -> TextReply {
        // The document from its start, and how many rows lie above the
        // start, read by moving that far up from it.
        let query = Query {
            start: self.range(Some(start)),
            screen_rows: 0,
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

/// The terminal's focus read, as the outpost answers Core's read of the
/// application's focus at startup, inside `depth` groups, and inside a
/// console host's window, outermost, when `console`.
fn focus_read(depth: u64, console: bool) -> FocusRead {
    let value = |name: String, role: Role| {
        PropertyValues(vec![
            PropertyAnswer {
                id: PropertyId::Name,
                value: Some(PropertyValue::Text(name)),
            },
            PropertyAnswer {
                id: PropertyId::Role,
                value: Some(PropertyValue::Role(role)),
            },
        ])
    };
    let ancestors = (0..depth)
        .map(|index| verbatim_model::request::FocusAncestor {
            node: FocusNode {
                read: NodeRead {
                    node: Some(NodeId::in_outpost(
                        OutpostId(OUTPOST.generation),
                        100_000 + index,
                    )),
                    window: window(),
                    values: value(format!("Group {index}"), Role::Group),
                },
                runtime_id: Some(RuntimeId(vec![
                    42,
                    100_000 + i32::try_from(index).expect("depth"),
                ])),
            },
            simple_child: false,
        })
        .chain(console.then(console_window))
        .collect();
    FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: Some(node()),
                window: window(),
                values: PropertyValues(vec![
                    PropertyAnswer {
                        id: PropertyId::Name,
                        value: Some(PropertyValue::Text("Terminal".to_owned())),
                    },
                    PropertyAnswer {
                        id: PropertyId::Role,
                        value: Some(PropertyValue::Role(Role::Terminal)),
                    },
                ]),
            },
            runtime_id: Some(RuntimeId(vec![42, 12])),
        },
        msaa: None,
        ancestors,
        met: None,
        foreground: None,
        value_from_selection: false,
        runtime_id_reused: false,
        caret: FocusCaret::NotRead,
    }
}

/// A console host's window, as a focus read's ancestor: its own window, of
/// the console's class.
fn console_window() -> verbatim_model::request::FocusAncestor {
    verbatim_model::request::FocusAncestor {
        node: FocusNode {
            read: NodeRead {
                node: Some(NodeId::in_outpost(OutpostId(OUTPOST.generation), 99_999)),
                window: window(),
                values: PropertyValues(vec![
                    PropertyAnswer {
                        id: PropertyId::Name,
                        value: Some(PropertyValue::Text("Console".to_owned())),
                    },
                    PropertyAnswer {
                        id: PropertyId::Role,
                        value: Some(PropertyValue::Role(Role::Window)),
                    },
                    PropertyAnswer {
                        id: PropertyId::NativeWindow,
                        value: Some(PropertyValue::Window(WindowHandle(4))),
                    },
                    PropertyAnswer {
                        id: PropertyId::ClassName,
                        value: Some(PropertyValue::Text("ConsoleWindowClass".to_owned())),
                    },
                ]),
            },
            runtime_id: Some(RuntimeId(vec![42, 99_999])),
        },
        simple_child: false,
    }
}

/// A reducer reading the simulated terminal through Core's event queue,
/// with playback that reaches each utterance's marks as it plays.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches a test sets for how the outpost answers and when speech plays"
)]
pub struct Session {
    pub state: SrState,
    runtime: verbatim_core::Runtime,
    pub outpost: Outpost,
    /// What was said, in order.
    pub heard: Vec<Said>,
    /// Every request Core made of the terminal but its output reads, in
    /// order.
    pub requests: Vec<Asked>,
    /// How many output reads Core asked.
    pub output_reads: usize,
    /// Every caret key's watch Core sent, in order.
    pub watches: Vec<verbatim_model::Watch>,
    /// Every full read the outpost answered, in order.
    pub answered_reads: Vec<verbatim_model::TerminalSnapshot>,
    /// Every event Core gave the control plane's stream, in order.
    pub observed: Vec<verbatim_model::Observed>,
    /// Every due time of the quiet-period timer, as the shell reads it
    /// after each step that changed it, in order.
    pub timers: Vec<Option<u64>>,
    /// The due time the shell read last.
    armed: Option<u64>,
    /// The text Core copied to the clipboard, in order.
    pub copied: Vec<String>,
    /// Every input given to the reducer since the session began, with its
    /// effects.
    pub log: Vec<(Input, Vec<Effect>)>,
    /// Whether the outpost holds back a whole read Core asks for, until
    /// [`Session::answer_held`].
    pub hold_whole: bool,
    held: Option<Ask>,
    /// Whether the outpost answers Core's searches refused, as its worker
    /// does when the terminal lost the focus before searching.
    pub unanswered_finds: bool,
    /// Output the terminal prints before each search Core asks for, the
    /// next taken for each: written, and told to Core, at its time, before
    /// the outpost searches, as when a program keeps printing while a
    /// search is on its way.
    pub output_before_finds: VecDeque<(String, u64)>,
    /// Output the terminal prints before each history read Core asks for,
    /// the next taken for each, as [`Session::output_before_finds`] is
    /// before a search.
    pub output_before_histories: VecDeque<(String, u64)>,
    /// Whether what Core queues is played only when the test calls
    /// [`Session::play`] or [`Session::play_one`], so that output can be
    /// written while a group plays.
    pub manual_play: bool,
    /// Requests sent and not yet answered, in the order Core sent them:
    /// the outpost answers each once Core's step is done, in order.
    pending: VecDeque<Ask>,
    /// Utterances queued and their marks, not yet played.
    queued: VecDeque<(Vec<SpeechMark>, Said)>,
    /// The time reads Core asks for begin at.
    pub at_ms: u64,
    /// The last key's number.
    keys: u64,
    /// How many groups the terminal is inside.
    depth: u64,
    /// Whether the terminal is inside a console host's window.
    console: bool,
    /// When the next full read the outpost answers began, when earlier
    /// than when it ended, the session's time.
    pub next_read_began: Option<u64>,
    /// Whether the outpost holds back Core's output reads, until
    /// [`Session::release_output`].
    pub hold_output: bool,
    held_output: VecDeque<Ask>,
    /// Whether the terminal's window is hung: the shell's live hang check
    /// refuses every request to it unsent, and answers Core's checks of it
    /// hung.
    pub hung: bool,
    /// The earcons played since the last [`Session::earcons`], in order.
    earcons: Vec<Earcon>,
    /// The wake Core last asked for, in microseconds.
    wake: Option<u64>,
    /// The windows Core asked the shell to check for a hang, not yet
    /// answered.
    checks: Vec<WindowHandle>,
}

impl Session {
    /// A terminal of `flavor`, `width` by `height` with `history` rows,
    /// focused and read once at time 0.
    pub fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        Self::with(Outpost::new(flavor, width, height, history))
    }

    /// `outpost`'s terminal, focused at startup and read once at time 0,
    /// all of which is let pass unasserted: the focus and the first read
    /// say nothing of the terminal's output.
    pub fn with(outpost: Outpost) -> Self {
        Self::inside(outpost, 0)
    }

    /// [`Session::with`], the terminal inside `depth` groups.
    pub fn inside(outpost: Outpost, depth: u64) -> Self {
        Self::built(outpost, depth, false)
    }

    /// [`Session::with`], the terminal inside a console host's window,
    /// which Core takes from the focus read's ancestor of the console's
    /// class.
    pub fn in_console(outpost: Outpost) -> Self {
        Self::built(outpost, 0, true)
    }

    fn built(outpost: Outpost, depth: u64, console: bool) -> Self {
        let mut session = Self {
            state: SrState::new(),
            runtime: verbatim_core::Runtime::default(),
            outpost,
            heard: Vec::new(),
            requests: Vec::new(),
            output_reads: 0,
            watches: Vec::new(),
            answered_reads: Vec::new(),
            observed: Vec::new(),
            timers: Vec::new(),
            armed: None,
            copied: Vec::new(),
            log: Vec::new(),
            hold_whole: false,
            held: None,
            unanswered_finds: false,
            output_before_finds: VecDeque::new(),
            output_before_histories: VecDeque::new(),
            manual_play: false,
            pending: VecDeque::new(),
            queued: VecDeque::new(),
            at_ms: 0,
            keys: 0,
            depth,
            console,
            next_read_began: None,
            hold_output: false,
            held_output: VecDeque::new(),
            hung: false,
            earcons: Vec::new(),
            wake: None,
            checks: Vec::new(),
        };
        // The terminal's window in front: Core starts the application's
        // outpost to read its focus, which commits the terminal.
        session.queue(QueueInput::Foreground(ForegroundReport {
            window: WindowHandle(4),
            root_owner: WindowHandle(4),
            process: APP,
            thread: 41,
            class: if console {
                "ConsoleWindowClass"
            } else {
                "CASCADIA_HOSTING_WINDOW_CLASS"
            }
            .to_owned(),
            ghost: None,
            console,
            integrity: None,
            own_integrity: None,
        }));
        session.queue(QueueInput::Process(
            verbatim_model::request::ProcessFact::OutpostStarted {
                process: APP,
                generation: OUTPOST.generation,
            },
        ));
        session.queue(QueueInput::ReaderReady(OUTPOST));
        session.heard.clear();
        session.requests.clear();
        session.output_reads = 0;
        session.answered_reads.clear();
        session.observed.clear();
        session.timers.clear();
        session
    }

    /// One step of the reducer, logged, with the timer's due time read
    /// after it, as the shell reads it.
    fn step(&mut self, input: Input) -> Vec<Effect> {
        let effects = reduce(&mut self.state, &mut self.runtime, &input);
        self.log.push((input, effects.clone()));
        let due = self.state.terminal_timer_due();
        if due != self.armed {
            self.armed = due;
            self.timers.push(due);
        }
        effects
    }

    /// `input`, an input of Core's event queue, at the session's time.
    fn queued_input(&self, trace_id: TraceId, input: QueueInput) -> Input {
        Input::Queue {
            trace_id,
            at_us: self.at_ms * 1_000,
            input,
        }
    }

    /// Feeds `input` to the event queue, takes in its effects, answers what
    /// they asked, and plays what was queued.
    pub fn queue(&mut self, input: QueueInput) {
        let input = self.queued_input(TraceId::from_raw(1), input);
        let effects = self.step(input);
        self.run(effects);
        self.settle();
    }

    /// `key` as the hook reports it to the event queue, numbered in turn,
    /// a held key's stroke numbered as the key is.
    fn pressed(&mut self, key: &Key) -> Input {
        self.keys += 1;
        let mut action = key.action.clone();
        if let PressAction::CaretKey {
            held: Some(held), ..
        } = &mut action
        {
            held.stroke.sequence = u32::try_from(self.keys).expect("keys");
        }
        Input::Queue {
            trace_id: key.trace_id,
            at_us: key.at_ms * 1_000,
            input: QueueInput::Press(Box::new(Press {
                key_seq: KeySeq::new(self.keys),
                pressed_at_us: key.at_ms * 1_000,
                action,
            })),
        }
    }

    /// Feeds `input`, takes in its effects, and plays what it queued.
    pub fn input(&mut self, input: &Input) {
        let effects = self.step(input.clone());
        self.run(effects);
        self.settle();
    }

    /// Presses `key`, takes in its effects, and plays what it queued.
    pub fn press(&mut self, key: &Key) {
        let input = self.pressed(key);
        self.input(&input);
    }

    /// Presses `keys` one after another before the outpost answers any
    /// request they make, as keys pressed faster than it answers, then
    /// answers those requests, in order, and plays what they queued.
    pub fn presses(&mut self, keys: &[Key]) {
        for key in keys {
            let input = self.pressed(key);
            let effects = self.step(input);
            self.take(effects, None);
        }
        self.run(Vec::new());
        self.settle();
    }

    /// Takes in `effects`, then answers the requests they and their answers
    /// made, in order.
    fn run(&mut self, effects: Vec<Effect>) {
        self.take(effects, None);
        while let Some(ask) = self.pending.pop_front() {
            self.answer(ask);
        }
    }

    /// The outpost's answer to `ask`, or, for a hung window, the shell's
    /// refusal of it unsent: a read of the current focus with the hung
    /// terminal's own window in front, no ghost before it, as the shell
    /// answers one; any other "not responding".
    fn answer(&mut self, ask: Ask) {
        if self.hung {
            let refused = match &ask.request {
                Wire::CurrentFocus { .. } => QueueInput::InFront {
                    number: ask.number,
                    front: verbatim_model::request::Front::Hung,
                },
                _ => QueueInput::RefusedHung(ask.number),
            };
            let input = self.queued_input(TraceId::from_raw(2), refused);
            let effects = self.step(input);
            self.take(effects, None);
            return;
        }
        let outcome = match &ask.request {
            Wire::Follow(_) => Outcome::Result(Reply::Followed),
            Wire::CurrentFocus { .. } => Outcome::Result(Reply::Candidate(
                verbatim_model::request::CandidateAnswer::focused(focus_read(
                    self.depth,
                    self.console,
                )),
            )),
            Wire::Text { op, .. } => match op {
                TextOp::ReadTerminal(read) => {
                    if self.hold_whole && read.scope == TerminalScope::Whole {
                        self.held = Some(ask);
                        return;
                    }
                    if self.hold_output && read.output {
                        self.held_output.push_back(ask);
                        return;
                    }
                    Outcome::Result(Reply::Text(TextReply::TerminalRead(Box::new(
                        self.full_read(read),
                    ))))
                }
                TextOp::TerminalHistory {
                    above,
                    whole,
                    start,
                    height,
                } => {
                    if let Some((output, at_ms)) = self.output_before_histories.pop_front() {
                        self.write(&output, at_ms);
                    }
                    Outcome::Result(Reply::Text(
                        self.outpost.history(*start, (*above, *whole, *height)),
                    ))
                }
                TextOp::TerminalFind {
                    needle,
                    from,
                    rows,
                    start,
                } => {
                    if let Some((output, at_ms)) = self.output_before_finds.pop_front() {
                        self.write(&output, at_ms);
                    }
                    if self.unanswered_finds {
                        Outcome::Refused(Refusal::Unsupported)
                    } else {
                        Outcome::Result(Reply::Text(self.outpost.find(
                            *start,
                            needle,
                            (*from, *rows),
                        )))
                    }
                }
                TextOp::Watch(_) => Outcome::Result(Reply::Text(TextReply::Watched(Box::new(
                    verbatim_model::Watched::EffectUnread,
                )))),
                other => panic!("an unexpected text request: {other:?}"),
            },
            other => panic!("an unexpected request: {other:?}"),
        };
        self.reply(&ask, outcome);
    }

    /// The full read `read`, taken at the session's time, begun then or at
    /// [`Session::next_read_began`].
    fn full_read(&mut self, read: &TerminalRead) -> TerminalReadReply {
        let (_, mut reply) = self.outpost.read(read, self.at_ms);
        if let Some(began) = self.next_read_began.take() {
            reply.snapshot.started_ms = began;
            reply.started_us = began * 1_000;
        }
        self.answered_reads.push(reply.snapshot.clone());
        reply
    }

    /// Answers the output reads held back, taken at `at_ms`.
    pub fn release_output(&mut self, at_ms: u64) {
        self.at_ms = at_ms;
        while let Some(ask) = self.held_output.pop_front() {
            self.answer(ask);
        }
        self.run(Vec::new());
        self.settle();
    }

    /// Sends the answer to `ask`, `outcome`, and takes in its effects.
    fn reply(&mut self, ask: &Ask, outcome: Outcome) {
        let input = self.queued_input(
            TraceId::from_raw(2),
            QueueInput::Answer {
                from: OUTPOST,
                answer: Answer {
                    number: ask.number,
                    outcome,
                    probes: Vec::new(),
                    detail: String::new(),
                },
            },
        );
        let effects = self.step(input);
        self.take(effects, None);
    }

    /// Writes `bytes` to the terminal, its change told to Core at `at_ms`.
    pub fn write(&mut self, bytes: &str, at_ms: u64) {
        self.outpost.sim.write(bytes.as_bytes());
        self.live(at_ms);
    }

    /// The terminal's change told to Core at `at_ms`, as its outpost tells
    /// one: Core reads it when it chooses.
    pub fn live(&mut self, at_ms: u64) {
        self.at_ms = at_ms;
        let tell = Event {
            trace_id: TraceId::from_raw(3),
            kind: EventKind::TerminalChanged,
            process: APP,
            application: "terminal".to_owned(),
            thread: 41,
            at_us: at_ms * 1_000,
            object: EventObject::Terminal {
                node: node(),
                window: WindowHandle(5),
            },
            cached: PropertyValues::default(),
            payload: Payload::None,
            window: TellWindow::NoWindow,
            checked: None,
        };
        self.queue(QueueInput::Tell {
            from: OUTPOST,
            tell: Box::new(tell),
        });
    }

    /// Answers the whole read held back.
    pub fn answer_held(&mut self) {
        let ask = self.held.take().expect("a whole read held back");
        let Wire::Text {
            op: TextOp::ReadTerminal(read),
            ..
        } = &ask.request
        else {
            panic!("a read held back");
        };
        let read = *read;
        let reply = self.full_read(&read);
        let outcome = Outcome::Result(Reply::Text(TextReply::TerminalRead(Box::new(reply))));
        self.reply(&ask, outcome);
        self.run(Vec::new());
        self.settle();
    }

    /// Records what `ask` asks of the terminal.
    fn record(&mut self, ask: &Ask) {
        let Wire::Text { op, .. } = &ask.request else {
            return;
        };
        let asked = match op {
            TextOp::ReadTerminal(read) => match (read.scope, read.output, read.starting) {
                (TerminalScope::Whole, ..) => Asked::Whole,
                (TerminalScope::HistoryBlock { height }, ..) => Asked::HistoryBlock(height),
                (TerminalScope::Fresh, true, _) => {
                    self.output_reads += 1;
                    return;
                }
                (TerminalScope::Fresh, false, Some(_)) => Asked::Starting,
                (TerminalScope::Fresh, false, None) => Asked::Fresh,
            },
            TextOp::TerminalHistory { above, whole, .. } => Asked::History {
                above: *above,
                whole: *whole,
            },
            TextOp::TerminalFind {
                needle, from, rows, ..
            } => Asked::Find {
                needle: needle.clone(),
                from: *from,
                rows: *rows,
            },
            TextOp::Watch(watch) => {
                self.watches.push(*watch);
                Asked::Watch
            }
            _ => return,
        };
        self.requests.push(asked);
    }

    /// Takes in effects: speech is queued, or heard at once when it has no
    /// marks and nothing is queued ahead of it; requests are kept for the
    /// outpost, which answers them in order; copies are recorded. Any
    /// other effect fails the test.
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
                    // Speech plays in the order it was queued: an utterance
                    // with no marks is heard at once only when nothing is
                    // queued ahead of it.
                    if marks.is_empty() && self.queued.is_empty() {
                        self.heard.push(said);
                    } else {
                        self.queued.push_back((marks, said));
                    }
                }
                Effect::StopSpeech => self.queued.clear(),
                Effect::CopyToClipboard(text) => self.copied.push(text),
                Effect::Observed(observed) => self.observed.push(*observed),
                Effect::Queue(QueueEffect::Send { to, ask }) => {
                    assert_eq!(to, OUTPOST, "a request to the terminal's outpost");
                    self.record(&ask);
                    self.pending.push_back(ask);
                }
                // The event queue's own business: cancels, wakes, strokes
                // sent, answers dropped by number.
                Effect::Queue(
                    QueueEffect::Cancel { .. }
                    | QueueEffect::SendStrokes(_)
                    | QueueEffect::AnswerDropped { .. }
                    | QueueEffect::StartOutpost(_),
                )
                | Effect::DropExpiredSpeech(_)
                // The listener's configuration, at every settings input.
                | Effect::ListenerConfiguration { .. } => {}
                Effect::PlayEarcon(earcon) => self.earcons.push(earcon),
                Effect::Queue(QueueEffect::WakeAt(wake)) => self.wake = wake,
                Effect::Queue(QueueEffect::CheckHung(windows)) => self.checks.extend(windows),
                other => panic!("unexpected effect: {other:?}"),
            }
        }
    }

    /// Plays what is queued, unless the test plays it
    /// ([`Session::manual_play`]).
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
            let input = self.queued_input(TraceId::from_raw(1), QueueInput::MarkReached(mark));
            let effects = self.step(input);
            self.run(effects);
        }
        true
    }

    /// When the shell's quiet-period timer is due, if it is armed.
    pub fn armed(&self) -> Option<u64> {
        self.armed
    }

    /// The earcons played since the last [`Session::earcons`], taken.
    pub fn earcons(&mut self) -> Vec<Earcon> {
        std::mem::take(&mut self.earcons)
    }

    /// Wakes Core at the time it last asked to be woken at, the session's
    /// time moved on to it, and answers the hang checks it asks then, the
    /// window hung as [`Session::hung`] says, as the shell's live check
    /// answers them.
    pub fn due(&mut self) {
        let wake = self.wake.take().expect("Core asked to be woken");
        self.at_ms = self.at_ms.max(wake.div_ceil(1_000));
        self.queue(QueueInput::Due);
        while !self.checks.is_empty() {
            let checks = std::mem::take(&mut self.checks);
            for window in checks {
                let hung = self.hung;
                self.queue(QueueInput::HangCheck { window, hung });
            }
        }
    }

    /// The terminal's focus read again under the same node, as the read
    /// after a return to the user's desktop reads it: a switch to an input
    /// desktop that opens is told by the listener, Core is woken when its read comes due,
    /// and the outpost answers the read with `runtime_id_reused`, whether
    /// the runtime id its reader held for the node now names another
    /// element.
    pub fn focus_after(&mut self, runtime_id_reused: bool) {
        let tell = Event {
            trace_id: TraceId::from_raw(4),
            kind: EventKind::DesktopSwitch,
            process: APP,
            application: "terminal".to_owned(),
            thread: 41,
            at_us: self.at_ms * 1_000,
            object: EventObject::Msaa {
                window: WindowHandle(0),
                object: 0,
                child: 0,
            },
            cached: PropertyValues::default(),
            payload: Payload::DesktopSwitch {
                input_desktop_open: true,
            },
            window: TellWindow::NoWindow,
            checked: None,
        };
        self.queue(QueueInput::Tell {
            from: Instance {
                reader: Reader::Listener,
                generation: 1,
            },
            tell: Box::new(tell),
        });
        let wake = self.wake.take().expect("the read after the return timed");
        self.at_ms = self.at_ms.max(wake.div_ceil(1_000));
        let input = self.queued_input(TraceId::from_raw(1), QueueInput::Due);
        let effects = self.step(input);
        self.take(effects, None);
        let ask = self.pending.pop_front().expect("the read after the return");
        assert!(
            matches!(ask.request, Wire::CurrentFocus { .. }),
            "the read after the return: {ask:?}"
        );
        let mut read = focus_read(self.depth, self.console);
        read.runtime_id_reused = runtime_id_reused;
        self.reply(
            &ask,
            Outcome::Result(Reply::Candidate(
                verbatim_model::request::CandidateAnswer::focused(read),
            )),
        );
        while let Some(ask) = self.pending.pop_front() {
            self.answer(ask);
        }
        self.settle();
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
