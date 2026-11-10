//! The one seam between the caret generated test (`super`) and production
//! types: everything the test knows of [`Reducer`] and Core's reducer goes
//! through [`Seam`], which speaks the world's words in and out. The caret
//! rework's builder changes this file alone, in its own commit.
//!
//! [`Seam::read_done`] stands in for the outpost's programs
//! (`crates/verbatim-outpost`), which make every read and comparison as
//! `TextChange`s, and report them as `CaretMoved` or a reply carrying
//! `ReadFacts`, as `docs/design/caret-selection-rework.md` lays out. A read
//! reads the note in its program's copy, and compares with the node's last
//! read as it commits.

use std::collections::{BTreeMap, BTreeSet};

use verbatim_core::{SrState, reduce};
use verbatim_model::protocol::{EventTiming, OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    Backend, CaretChanges, CaretKey, CaretMotion, CaretRead, CaretReply, CaretReport,
    CaretSelection, Change as FactsChange, Compared, Effect as CoreEffect, FirstCachedAncestor,
    FocusReaderKind, FocusText, ForegroundWindow, Input as CoreInput, KeySeq, Message, NodeDetails,
    NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, QueryId, ReadFacts, ReadPurpose,
    Role, SegmentContent, Selection, SelectionChange, SelectionText, StateSet, TextAnchor,
    TextChunk, TextOp, TextPosition, TextReply, TextUnit, TraceId, WindowFacts, WindowHandle,
    had_effect,
};

use verbatim_model::{NoteRequest, TerminalSnapshot, TerminalText};

use super::super::*;
use super::model::{CONSOLE, Heard, ReadRole, TERMINAL};
use super::world::{Key, Text, Us, selection_changes};

/// What crossing the seam brought back, in order.
#[derive(Clone, Debug)]
pub enum Seen {
    /// Something was said.
    Heard(Heard),
    /// Key `0` reached the application.
    Passed(usize),
    /// A read committed, of the field named, with what it saw, and whether
    /// it is a held key's before read.
    Committed(usize, Text, bool),
    /// Core asked the read before a held key, at this time.
    BeforeReadAsked(Us),
    /// A read of the field named was dispatched, at this time.
    Dispatched(usize, Us),
    /// Core let go of the field named.
    Released(usize),
    /// The field named had its element replaced, at this time.
    Replaced(usize, Us),
    /// A terminal's before read of the field named ended, at this time.
    TerminalRead(usize, Us),
    /// A console poll read the field named, at this time.
    PollRead(usize, Us),
    /// A report reached Core, of the read committed just before.
    Report {
        /// What it saw.
        text: Text,
        /// When the read began.
        began: Us,
        /// When it ended.
        ended: Us,
        /// Whether it is Core's final read of a key.
        final_read: bool,
    },
    /// The read in flight ended without a result.
    Abandoned(ReadRole),
    /// Core asked its final read.
    FinalReadAsked,
}

/// What a read in flight is, as a program of the outpost.
#[derive(Clone, Debug)]
enum ReadKind {
    /// The read before a held key.
    Before(RequestId, TraceId, CaretRead),
    /// A key's final read.
    Final(RequestId, TraceId, CaretRead),
    /// The application's caret event.
    Event,
    /// The read before a held key in a terminal.
    TerminalBefore(RequestId, TraceId),
}

/// The program in flight, and its copy of the facts, as dispatched.
#[derive(Clone, Debug)]
struct Flight {
    request: Request,
    node: NodeId,
    field: usize,
    kind: ReadKind,
    facts: Facts,
    began: Us,
    /// The note its copy of the facts held as it was dispatched.
    note: Option<KeyNote>,
    /// A final read's: the end of the note that lived as it arrived.
    final_note_until: Option<u64>,
}

/// Core's reducer and the outpost's together, with a stand-in for the
/// outpost's programs.
pub struct Seam {
    core: SrState,
    outpost: Reducer,
    answers: Answers,
    generation: u64,
    flight: Option<Flight>,
    /// Each node's note as the reducer held it after every input, by when
    /// it became so: what a read begun earlier can have seen of it.
    note_log: BTreeMap<u64, Vec<(Us, Option<KeyNote>)>>,
    /// The commit of a focus report, for the program dispatched to make it.
    focusing: Option<Box<Change>>,
    /// The field whose release was asked for, until the shell's answer.
    releasing: Option<usize>,
    /// The field whose element is replaced by the next name change.
    replacing: Option<usize>,
    /// Whether the foreground window is the console host's.
    console_front: bool,
    /// Core's held-key deadline: the key and when it falls due.
    due: Option<(TraceId, Us)>,
    keys: BTreeMap<TraceId, usize>,
    /// The final reads Core has asked that have not been answered.
    final_outstanding: u32,
    /// This outpost generation has reported a focus: until it has, it has
    /// no focus whose caret events it reads.
    focus_reported: bool,
    /// Core gave up the before read with this query id: it answers as an
    /// ordinary report.
    given_up: Option<u64>,
    /// The live re-read asked of the report waiting, by its request.
    reread: Option<u64>,
    /// When this outpost generation began: none of its reads began earlier.
    generation_began: Us,
    now: Us,
    next_trace: u64,
    /// What this seam could not express.
    pub unexpressed: BTreeSet<&'static str>,
}

/// The terminal node that is the console host's; node 10 is the other
/// terminal.
const CONSOLE_NODE: u64 = 11;

/// The node of field `field` in outpost generation `generation`.
fn node_of(generation: u64, field: usize) -> NodeId {
    NodeId::in_outpost(OutpostId(generation), 5 + field as u64)
}

fn window_of(field: usize) -> isize {
    if field == 0 { 0x100 } else { 0x101 }
}

fn snapshot(generation: u64, number: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::in_outpost(OutpostId(generation), number),
        backend: Backend::Msaa,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// `text`'s caret as a report of field `field`, its read begun at `began`.
fn caret_report(text: &Text, field: usize, began: Us) -> CaretReport {
    let anchor = TextAnchor(100 + field as u64);
    let position = |offset: usize| TextPosition {
        anchor,
        offset: u32::try_from(offset).expect("a short line"),
    };
    CaretReport {
        line: TextChunk {
            unit: TextUnit::Line,
            text: text.line(),
            start: anchor,
            offset: position(text.caret).offset,
            languages: Vec::new(),
            first: true,
            last: true,
            truncated: false,
            formats: Vec::new(),
        },
        selection: text.selection.map(|(start, end)| CaretSelection {
            range: Selection {
                start: position(start),
                end: position(end),
            },
            start_line: None,
        }),
        read_at_us: began,
    }
}

/// `changes`, in the words of a report.
fn report_changes(changes: Vec<(bool, String)>) -> Vec<SelectionChange> {
    changes
        .into_iter()
        .map(|(selected, text)| SelectionChange {
            selected,
            characters: u32::try_from(text.chars().count()).expect("a short line"),
            text,
        })
        .collect()
}

fn motion(key: Key) -> CaretKey {
    let (motion, select) = match key {
        Key::Right => (CaretMotion::NextCharacter, false),
        Key::Left => (CaretMotion::PreviousCharacter, false),
        Key::Down => (CaretMotion::NextLine, false),
        Key::ShiftRight => (CaretMotion::NextCharacter, true),
        Key::Delete => (CaretMotion::Delete, false),
        Key::Backspace => (CaretMotion::Backspace, false),
        Key::WordBackspace => (CaretMotion::BackspaceWord, false),
    };
    CaretKey { motion, select }
}

/// An utterance's segments as the test's words: a focus's own speech by its
/// validity, words joined with spaces.
fn heard(utterance: &verbatim_model::Utterance) -> Heard {
    if utterance.validity.is_some() {
        return Heard::Focus;
    }
    let words: Vec<String> = utterance
        .segments
        .iter()
        .map(|segment| match &segment.content {
            SegmentContent::Text(text) | SegmentContent::Character(text) => text.clone(),
            SegmentContent::Message(Message::Blank) => "blank".to_owned(),
            SegmentContent::Message(Message::SelectionRemoved) => "selection removed".to_owned(),
            SegmentContent::Phrase(Phrase::SelectedInstead(text)) => {
                format!("{} selected instead", selection_words(text))
            }
            SegmentContent::Phrase(Phrase::Selected(text)) => {
                format!("{} selected", selection_words(text))
            }
            SegmentContent::Phrase(Phrase::Unselected(text)) => {
                format!("{} unselected", selection_words(text))
            }
            other => format!("{other:?}"),
        })
        .collect();
    Heard::Words(words.join(" "))
}

fn selection_words(text: &SelectionText) -> String {
    match text {
        SelectionText::Text(text) | SelectionText::Character(text) => text.clone(),
        SelectionText::Characters(count) => count.to_string(),
    }
}

impl Seam {
    /// Both reducers, started, with no focus.
    #[must_use]
    pub fn new() -> Self {
        let mut seam = Self {
            core: SrState::new(),
            outpost: Reducer::new(),
            answers: Answers::default(),
            generation: 1,
            flight: None,
            note_log: BTreeMap::new(),
            focusing: None,
            releasing: None,
            replacing: None,
            console_front: false,
            due: None,
            keys: BTreeMap::new(),
            final_outstanding: 0,
            focus_reported: false,
            given_up: None,
            reread: None,
            generation_began: 0,
            now: 1_000_000,
            next_trace: 10_000,
            unexpressed: BTreeSet::new(),
        };
        let mut seen = Vec::new();
        seam.enter_core(&CoreInput::Started, &mut seen);
        seam
    }

    fn trace(&mut self) -> TraceId {
        self.next_trace += 1;
        TraceId::from_raw(self.next_trace)
    }

    fn outpost_now(&self) -> Now {
        Now {
            at: Duration::from_micros(self.now),
            observed_us: self.now,
            wall_us: self.now,
        }
    }

    fn outpost_in(&mut self, input: Input, seen: &mut Vec<Seen>) {
        let now = self.outpost_now();
        let effects = self.outpost.input(input, now);
        let effects = with_answers(&mut self.outpost, &mut self.answers, effects, now);
        self.record_notes();
        self.after_outpost(effects, seen);
    }

    /// Notes the reducer's note of each field node, when it changed.
    fn record_notes(&mut self) {
        for field in [0, 1] {
            let node = node_of(self.generation, field).number();
            let held = self.outpost.facts.text.live_note(node, self.now);
            let log = self.note_log.entry(node).or_default();
            if log.last().map(|(_, was)| was) != Some(&held) {
                log.push((self.now, held));
            }
        }
    }

    /// The note of `node` a read begun at `at` saw: the one the reducer
    /// held then, if it had not ended.
    fn note_seen(&self, node: u64, at: Us) -> Option<KeyNote> {
        self.note_log
            .get(&node)?
            .iter()
            .rev()
            .find(|(when, _)| *when <= at)?
            .1
            .filter(|note| note.until_us > at)
    }

    /// Follows the outpost's effects: its messages reach Core, and its
    /// programs are taken, a caret read held for the world to end.
    fn after_outpost(&mut self, effects: Vec<Effect>, seen: &mut Vec<Seen>) {
        for effect in effects {
            match effect {
                Effect::Send(message) => self.enter_core_message(*message, seen),
                Effect::Dispatch(program) => self.take_program(&program, seen),
                Effect::Release { .. } => {
                    let released: BTreeSet<u64> = self
                        .releasing
                        .take()
                        .map(|field| node_of(self.generation, field).number())
                        .into_iter()
                        .collect();
                    if let Some(&node) = released.iter().next() {
                        seen.push(Seen::Released(usize::from(node != 5)));
                    }
                    self.outpost_in(
                        Input::Released {
                            nodes: released,
                            anchors: BTreeSet::new(),
                            objects: false,
                        },
                        seen,
                    );
                }
                _ => {}
            }
        }
    }

    fn take_program(&mut self, program: &Program, seen: &mut Vec<Seen>) {
        if let Work::NoteRead(node) = &program.work {
            seen.push(Seen::PollRead(field_of_node(*node), self.now));
        }
        let Work::Entry {
            entry, handling, ..
        } = &program.work
        else {
            self.finish(program.request, seen);
            return;
        };
        let read = match &entry.item {
            Item::Query {
                request_id,
                query:
                    Query::Text {
                        node_id,
                        op: TextOp::ReadCaret(read),
                    },
            } => Some((
                *node_id,
                match read.purpose {
                    ReadPurpose::BeforeFieldKey => {
                        ReadKind::Before(*request_id, entry.trace, *read)
                    }
                    ReadPurpose::BeforeTerminalKey => {
                        ReadKind::TerminalBefore(*request_id, entry.trace)
                    }
                    ReadPurpose::Final => ReadKind::Final(*request_id, entry.trace, *read),
                },
            )),
            Item::Msaa {
                kind: WinKind::NameChange,
                ..
            } if self.focusing.is_some() => {
                if let Some(change) = self.focusing.take() {
                    self.commit(program.request, *change, seen);
                }
                None
            }
            Item::Msaa {
                kind: WinKind::NameChange,
                ..
            } => {
                if let Some(field) = self.replacing.take() {
                    let node = node_of(self.generation, field);
                    seen.push(Seen::Replaced(field, self.now));
                    self.commit(
                        program.request,
                        Change {
                            replaced: Some(node),
                            ..Change::default()
                        },
                        seen,
                    );
                }
                None
            }
            Item::Msaa {
                kind: WinKind::ValueChange,
                hwnd,
                ..
            } => {
                // The worker reads a text change only under a note that
                // has seen no effect.
                let node = node_of(self.generation, field_of(*hwnd));
                program
                    .facts
                    .text
                    .live_note(node.number(), self.now)
                    .is_some_and(|note| !note.reads.effect_seen())
                    .then_some((node, ReadKind::Event))
            }
            Item::Msaa { hwnd, .. } => {
                Some((node_of(self.generation, field_of(*hwnd)), ReadKind::Event))
            }
            _ => None,
        };
        let Some((node, kind)) = read else {
            self.finish(program.request, seen);
            return;
        };
        seen.push(Seen::Dispatched(field_of_node(node), self.now));
        self.flight = Some(Flight {
            request: program.request,
            node,
            field: field_of_node(node),
            kind,
            note: program.facts.text.live_note(node.number(), self.now),
            final_note_until: match handling {
                crate::intake::Handling::FinalRead { note_until } => *note_until,
                _ => None,
            },
            facts: program.facts.clone(),
            began: self.now,
        });
    }

    fn finish(&mut self, request: Request, seen: &mut Vec<Seen>) {
        self.outpost_in(
            Input::Finished {
                request,
                reported: false,
                panicked: false,
                cut: None,
            },
            seen,
        );
    }

    fn commit(&mut self, request: Request, change: Change, seen: &mut Vec<Seen>) {
        self.outpost_in(
            Input::Commit {
                request,
                change: Box::new(change),
            },
            seen,
        );
    }

    fn enter_core_message(&mut self, message: OutpostToCore, seen: &mut Vec<Seen>) {
        let outpost = OutpostId(self.generation);
        let input = match message {
            OutpostToCore::Reply {
                trace_id,
                request_id,
                outcome,
                ..
            } => CoreInput::Answered {
                trace_id,
                query_id: QueryId(request_id.number),
                outpost,
                outcome,
                at_us: self.now,
            },
            OutpostToCore::Event {
                trace_id,
                observed_at_us,
                backend,
                window,
                event,
                ..
            } => CoreInput::Event {
                trace_id,
                observed_at_us,
                source: Pid(1),
                backend,
                window,
                event,
                purpose: None,
            },
            _ => return,
        };
        self.enter_core(&input, seen);
    }

    /// `input` into Core, its effects followed: speech heard, keys passed
    /// on, requests given to the outpost.
    fn enter_core(&mut self, input: &CoreInput, seen: &mut Vec<Seen>) {
        let effects = reduce(&mut self.core, input);
        for effect in effects {
            self.core_effect(effect, seen);
        }
    }

    fn core_effect(&mut self, effect: CoreEffect, seen: &mut Vec<Seen>) {
        match effect {
            CoreEffect::ForKey(_, inner) => self.core_effect(*inner, seen),
            CoreEffect::Speak(utterance, _) => seen.push(Seen::Heard(heard(&utterance))),
            CoreEffect::StopSpeech => seen.push(Seen::Heard(Heard::Cut)),
            CoreEffect::LiveReread(reread) => self.reread = Some(reread.request),
            CoreEffect::PassHeldKey(trace) => {
                if let Some(&id) = self.keys.get(&trace) {
                    seen.push(Seen::Passed(id));
                }
            }
            CoreEffect::BoundHeldKey(bound) => {
                self.due = bound.map(|(trace, ms)| (trace, self.now + ms * 1_000));
            }
            CoreEffect::KeyNote(note) => {
                self.outpost_in(
                    Input::KeyNote {
                        node: note.node(),
                        fetch: note.fetch(),
                        on: match note {
                            NoteRequest::Field { .. } => NoteOn::Field,
                            NoteRequest::Terminal { .. }
                                if note.node().number() == CONSOLE_NODE =>
                            {
                                NoteOn::ConsoleHost
                            }
                            NoteRequest::Terminal { .. } => NoteOn::Terminal,
                        },
                    },
                    seen,
                );
            }
            CoreEffect::Withdraw { query_id, .. } => {
                self.given_up = Some(query_id.0);
                self.outpost_in(Input::Cancel(RequestId::shell(query_id.0)), seen);
            }
            CoreEffect::Text(request) => {
                match request.op {
                    TextOp::ReadCaret(CaretRead {
                        purpose: ReadPurpose::BeforeFieldKey | ReadPurpose::BeforeTerminalKey,
                        ..
                    }) => seen.push(Seen::BeforeReadAsked(self.now)),
                    TextOp::ReadCaret(CaretRead {
                        purpose: ReadPurpose::Final,
                        ..
                    }) => {
                        self.final_outstanding += 1;
                        seen.push(Seen::FinalReadAsked);
                    }
                    _ => {}
                }
                let item = Item::Query {
                    request_id: RequestId::shell(request.query_id.0),
                    query: Query::Text {
                        node_id: request.node_id,
                        op: request.op,
                    },
                };
                let entry = Entry {
                    item,
                    trace: self.trace(),
                    observed_at_us: self.now,
                    timing: EventTiming::default(),
                    front: crate::Front::Untested,
                };
                self.outpost_in(
                    Input::Entry {
                        entry,
                        source: Source::Other,
                    },
                    seen,
                );
            }
            _ => {}
        }
    }

    /// A key is pressed at `now`, its number `id`; a key the hook does not
    /// hold reaches the application as pressed.
    pub fn press(&mut self, id: usize, key: Key, held: bool, now: Us) -> Vec<Seen> {
        self.now = now;
        let trace = TraceId::from_raw(1_000 + id as u64);
        self.keys.insert(trace, id);
        let mut seen = Vec::new();
        // The hook cuts speech for a key before it reports the key.
        self.enter_core(&CoreInput::SpeechCancelled { at_us: now }, &mut seen);
        let pressed_at_ms = now / 1_000;
        if held {
            let input = CoreInput::HeldKey {
                trace_id: trace,
                key_seq: KeySeq::NONE_YET,
                key: motion(key),
                pressed_at_ms,
            };
            self.enter_core(&input, &mut seen);
        } else {
            let input = CoreInput::CaretKey {
                trace_id: trace,
                key_seq: KeySeq::NONE_YET,
                key: motion(key),
                pressed_at_ms,
            };
            self.enter_core(&input, &mut seen);
            seen.push(Seen::Passed(id));
        }
        seen
    }

    /// The application raised a caret event for `field` at `now`.
    pub fn caret_event(&mut self, field: usize, now: Us) -> Vec<Seen> {
        self.now = now;
        let mut seen = Vec::new();
        if !self.focus_reported {
            return seen;
        }
        let entry = Entry {
            item: Item::Msaa {
                kind: WinKind::Caret,
                hwnd: window_of(field),
                id_object: -8,
                id_child: 0,
                raiser: 3,
            },
            trace: self.trace(),
            observed_at_us: now,
            timing: EventTiming::default(),
            front: crate::Front::Untested,
        };
        self.outpost_in(
            Input::Entry {
                entry,
                source: Source::Other,
            },
            &mut seen,
        );
        seen
    }

    /// Whether a final read Core asked has not been answered.
    #[must_use]
    pub fn final_read_outstanding(&self) -> bool {
        self.final_outstanding > 0
    }

    /// The application raised a text change event for `field`.
    pub fn text_change(&mut self, field: usize, now: Us) -> Vec<Seen> {
        self.now = now;
        let mut seen = Vec::new();
        if !self.focus_reported {
            return seen;
        }
        self.push_msaa(WinKind::ValueChange, field, &mut seen);
        seen
    }

    /// Core no longer holds `field`.
    pub fn release(&mut self, field: usize, now: Us) -> Vec<Seen> {
        self.now = now;
        self.releasing = Some(field);
        let mut seen = Vec::new();
        let entry = Entry {
            item: Item::NodesHeld {
                nodes: Vec::new(),
                anchors: Vec::new(),
                acknowledged: 0,
            },
            trace: self.trace(),
            observed_at_us: now,
            timing: EventTiming::default(),
            front: crate::Front::Untested,
        };
        self.outpost_in(
            Input::Entry {
                entry,
                source: Source::Other,
            },
            &mut seen,
        );
        seen
    }

    /// `field`'s element is replaced under its node.
    pub fn replace(&mut self, field: usize, now: Us) -> Vec<Seen> {
        self.now = now;
        self.replacing = Some(field);
        let mut seen = Vec::new();
        self.push_msaa(WinKind::NameChange, field, &mut seen);
        seen
    }

    /// An MSAA event of `kind` for `field` reaches the outpost.
    fn push_msaa(&mut self, kind: WinKind, field: usize, seen: &mut Vec<Seen>) {
        let entry = Entry {
            item: Item::Msaa {
                kind,
                hwnd: window_of(field),
                id_object: -8,
                id_child: 0,
                raiser: 3,
            },
            trace: self.trace(),
            observed_at_us: self.now,
            timing: EventTiming::default(),
            front: crate::Front::Untested,
        };
        self.outpost_in(
            Input::Entry {
                entry,
                source: Source::Other,
            },
            seen,
        );
    }

    /// The terminal before read in flight ended at `now`.
    pub fn terminal_read_done(&mut self, now: Us) -> Vec<Seen> {
        self.now = now;
        let flight = self.flight.take().expect("a read in flight");
        let mut seen = Vec::new();
        let ReadKind::TerminalBefore(request_id, trace_id) = flight.kind else {
            return seen;
        };
        if !self.outpost.accepts(flight.request) {
            return seen;
        }
        seen.push(Seen::TerminalRead(flight.field, now));
        let text = flight
            .note
            .filter(|_| flight.node.number() == CONSOLE_NODE)
            .into_iter()
            .map(|note| TextChange::NotePoll {
                node: flight.node.number(),
                until_us: note.until_us,
                read_ended_at_us: now,
            });
        let change = Change {
            text: text.collect(),
            result: Some(OutpostToCore::Reply {
                trace_id,
                request_id,
                outcome: QueryOutcome::Done(QueryResult::Text(TextReply::TerminalSnapshot(
                    Box::new(terminal_snapshot(now)),
                ))),
                timing: EventTiming::default(),
            }),
            ..Change::default()
        };
        self.commit(flight.request, change, &mut seen);
        self.finish(flight.request, &mut seen);
        seen
    }

    /// A focus on a terminal, the console host's when `console`.
    pub fn focus_terminal(&mut self, console: bool, now: Us) -> Vec<Seen> {
        self.now = now;
        self.focus_reported = true;
        let mut seen = Vec::new();
        self.set_console(console, &mut seen);
        let input = CoreInput::Event {
            trace_id: self.trace(),
            observed_at_us: now,
            source: Pid(1),
            backend: Backend::Msaa,
            window: None,
            event: NormalizedEvent::FocusChanged {
                ancestors: vec![snapshot(self.generation, 1, Role::Window, "Window")],
                node: snapshot(
                    self.generation,
                    if console { CONSOLE_NODE } else { 10 },
                    Role::Terminal,
                    "Terminal",
                ),
                whole_window: false,
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                answers: None,
                first_cached_ancestor: None,
            },
            purpose: None,
        };
        self.enter_core(&input, &mut seen);
        drop_cut(&mut seen);
        seen.dedup_by(same_focus);
        seen
    }

    /// The foreground window becomes the console host's or stops being it.
    fn set_console(&mut self, console: bool, seen: &mut Vec<Seen>) {
        if self.console_front != console {
            self.console_front = console;
            self.foreground(WINDOWS[0], seen);
        }
    }

    /// Field `field`'s last read as the reducer holds it.
    #[must_use]
    pub fn last_read(&self, field: usize) -> Option<Text> {
        self.last_read_of(node_of(self.generation, field))
            .map(|(text, _)| text)
    }

    /// Whether a read is in flight.
    #[must_use]
    pub fn read_in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// The field the read in flight reads.
    #[must_use]
    pub fn reading_field(&self) -> usize {
        self.flight.as_ref().map_or(0, |flight| flight.field)
    }

    /// The node's last read, as the outpost's reducer holds it now, in the
    /// world's words.
    fn last_read_of(&self, node: NodeId) -> Option<(Text, LastRead<u32>)> {
        let last = self.outpost.facts.text.edit(node.number())?.last.clone()?;
        let text = Text {
            letters: last.line.text.chars().collect(),
            caret: last.caret as usize,
            selection: last
                .selection
                .as_ref()
                .map(|selection| (selection.start as usize, selection.end as usize)),
        };
        Some((text, last))
    }

    /// The read in flight ended at `now`, seeing `text`.
    ///
    /// # Panics
    ///
    /// With no read in flight.
    pub fn read_done(&mut self, text: &Text, now: Us) -> Vec<Seen> {
        self.now = now;
        let flight = self.flight.take().expect("a read in flight");
        let mut seen = Vec::new();
        if !self.outpost.accepts(flight.request) {
            return seen;
        }
        let field = flight.field;
        let note = flight.note.map(|note| (note.fetch, Some(note.until_us)));
        let fetch = match &flight.kind {
            ReadKind::Before(_, _, read) | ReadKind::Final(_, _, read) => Some((
                NoteFetch {
                    unit: read.fetch.unit,
                    motion: read.fetch.motion,
                },
                None,
            )),
            ReadKind::Event => note,
            ReadKind::TerminalBefore(..) => unreachable!("answered by terminal_read_done"),
        };
        let found = self.found(flight.node, text, flight.began, now, fetch.map(|(f, _)| f));
        let mut changes = vec![committed_read(
            flight
                .facts
                .text
                .edit(flight.node.number())
                .map(|anchors| anchors.part(&[])),
            flight.node.number(),
            &found,
        )];
        if let Some(until_us) = flight.final_note_until {
            changes.push(TextChange::NoteCleared {
                node: flight.node.number(),
                until_us,
            });
        }
        if let Some((fetch, Some(until_us))) = fetch
            && found.shows_effect(fetch.motion)
        {
            changes.push(TextChange::NoteEffect {
                node: flight.node.number(),
                until_us,
            });
        }
        let before =
            matches!(flight.kind, ReadKind::Before(id, ..) if Some(id.number) != self.given_up);
        seen.push(Seen::Committed(field, text.clone(), before));
        let final_read = matches!(flight.kind, ReadKind::Final(..));
        // The report reaches Core before anything it causes.
        seen.push(Seen::Report {
            text: text.clone(),
            began: flight.began,
            ended: now,
            final_read,
        });
        let result = match flight.kind {
            ReadKind::Before(request_id, trace, _) | ReadKind::Final(request_id, trace, _) => {
                OutpostToCore::Reply {
                    trace_id: trace,
                    request_id,
                    outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Caret(Box::new(
                        found.reply(),
                    )))),
                    timing: EventTiming::default(),
                }
            }
            ReadKind::TerminalBefore(..) => unreachable!("answered by terminal_read_done"),
            ReadKind::Event => OutpostToCore::Event {
                trace_id: self.trace(),
                observed_at_us: flight.began,
                backend: Backend::Msaa,
                window: None,
                application: None,
                timing: EventTiming::default(),
                event: found.event(flight.node),
            },
        };
        if final_read {
            self.final_outstanding -= 1;
        }
        self.commit(
            flight.request,
            Change {
                text: changes,
                caret_read: Some((flight.node, flight.began)),
                result: Some(result),
                ..Change::default()
            },
            &mut seen,
        );
        self.finish(flight.request, &mut seen);
        seen
    }

    /// What a read of `node`, seeing `text`, begun at `began` and ended at
    /// `now`, finds compared with the node's last read, its facts when it
    /// was made for `fetch`.
    fn found(
        &self,
        node: NodeId,
        text: &Text,
        began: Us,
        now: Us,
        fetch: Option<NoteFetch>,
    ) -> Found {
        let field = field_of_node(node);
        let report = caret_report(text, field, began);
        let last = self.last_read_of(node);
        let was = last.as_ref().map(|(was, _)| was);
        let changes = was.map_or_else(Vec::new, |was| report_changes(selection_changes(was, text)));
        let content_changed = was.is_some_and(|was| was.letters != text.letters);
        let new_selection = text
            .selection
            .filter(|_| content_changed)
            .map(|(start, end)| SelectionChange {
                selected: true,
                characters: u32::try_from(end - start).expect("a short line"),
                text: text.letters[start..end].iter().collect(),
            });
        let facts = fetch.map(|fetch| ReadFacts {
            unit: Some(unit_chunk(text, &report, fetch.unit)),
            change: last.map_or(FactsChange::NoPrevious, |(was, last)| {
                FactsChange::Compared {
                    compared: compared(&was, text),
                    previous_selection: last.selection.map(|selection| selection.report),
                    previous_line: last.line,
                    on_previous_line: true,
                }
            }),
            read_ended_at_us: now,
        });
        Found {
            text: text.clone(),
            began,
            report,
            changes,
            content_changed,
            new_selection,
            facts,
        }
    }

    /// The read in flight is abandoned: its deadline passes.
    pub fn abandon(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        let Some(flight) = self.flight.take() else {
            return seen;
        };
        let Some(deadline) = self.outpost.flight.deadline() else {
            return seen;
        };
        if matches!(flight.kind, ReadKind::Final(..)) {
            self.final_outstanding -= 1;
        }
        seen.push(Seen::Abandoned(match flight.kind {
            ReadKind::Before(id, ..) | ReadKind::TerminalBefore(id, _)
                if Some(id.number) == self.given_up =>
            {
                ReadRole::Other
            }
            ReadKind::TerminalBefore(..) | ReadKind::Before(..) => ReadRole::Before,
            ReadKind::Final(..) => ReadRole::Final,
            ReadKind::Event => ReadRole::Other,
        }));
        let now = Now {
            at: Duration::from_micros(deadline),
            observed_us: self.now,
            wall_us: self.now,
        };
        let effects = self.outpost.tick(now);
        let effects = with_answers(&mut self.outpost, &mut self.answers, effects, now);
        self.after_outpost(effects, &mut seen);
        seen
    }

    /// A focus on `field`, reported at `now` from a read begun at `began`,
    /// back into the first window from another when `back`.
    pub fn focus(
        &mut self,
        field: usize,
        text: &Text,
        began: Us,
        now: Us,
        back: bool,
    ) -> Vec<Seen> {
        let shape = Shape {
            top: back.then_some(WINDOWS[0]),
            ..Shape::default()
        };
        self.focus_report(field, text, began, now, shape)
    }

    /// A focus on `field`, entering a cached container, set aside for its
    /// live re-read.
    pub fn focus_waits(&mut self, field: usize, text: &Text, began: Us, now: Us) -> Vec<Seen> {
        let shape = Shape {
            cached: Some(1),
            ..Shape::default()
        };
        let seen = self.focus_report(field, text, began, now, shape);
        if self.reread.is_none() {
            self.unexpressed.insert("a focus report that waits");
        }
        seen
    }

    /// The live re-read's answer: the report that waited is the focus.
    pub fn focus_answer(&mut self, field: usize, text: &Text, began: Us, now: Us) -> Vec<Seen> {
        let Some(request) = self.reread.take() else {
            self.unexpressed.insert("a focus report that waits");
            return Vec::new();
        };
        let shape = Shape {
            answers: Some(request),
            ..Shape::default()
        };
        self.focus_report(field, text, began, now, shape)
    }

    /// A focus on a button, reported at `now`, in another top-level window
    /// when `new_window`.
    pub fn focus_button(&mut self, new_window: bool, now: Us) -> Vec<Seen> {
        self.now = now;
        let mut seen = Vec::new();
        if new_window {
            self.foreground(WINDOWS[1], &mut seen);
        }
        let input = CoreInput::Event {
            trace_id: self.trace(),
            observed_at_us: now,
            source: Pid(1),
            backend: Backend::Msaa,
            window: new_window.then(|| facts_in(WINDOWS[1])),
            event: NormalizedEvent::FocusChanged {
                ancestors: vec![if new_window {
                    snapshot(self.generation, 2, Role::Window, "Second")
                } else {
                    snapshot(self.generation, 1, Role::Window, "Window")
                }],
                node: snapshot(self.generation, 7, Role::Button, "Button"),
                whole_window: false,
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                answers: None,
                first_cached_ancestor: None,
            },
            purpose: None,
        };
        self.enter_core(&input, &mut seen);
        if !new_window {
            drop_cut(&mut seen);
        }
        seen.dedup_by(same_focus);
        seen
    }

    fn foreground(&mut self, window: WindowHandle, seen: &mut Vec<Seen>) {
        let change = ForegroundWindow {
            window,
            root_owner: window,
            process: Pid(1),
            observed_at_us: self.now.saturating_sub(1),
            own: false,
            thread: 1,
            console: self.console_front,
            integrity: None,
            own_integrity: None,
        };
        self.enter_core(&CoreInput::Foreground(change), seen);
    }

    /// A focus on `field`, reported at `now` from a read begun at `began`,
    /// shaped as `shape` says.
    fn focus_report(
        &mut self,
        field: usize,
        text: &Text,
        began: Us,
        now: Us,
        shape: Shape,
    ) -> Vec<Seen> {
        self.now = now;
        self.focus_reported = true;
        let mut ancestors = vec![snapshot(self.generation, 1, Role::Window, "Window")];
        // The second field is in a container the first is not.
        if field == 1 {
            ancestors.push(snapshot(self.generation, 9, Role::Pane, "Tools"));
        }
        let node = snapshot(
            self.generation,
            5 + field as u64,
            Role::EditableText,
            if field == 0 { "First" } else { "Second" },
        );
        // A focus read is a read of the node: under a live note it carries
        // the key's facts, and it replaces the node's last read.
        let node_id = node.id;
        let live = self
            .note_seen(node_id.number(), began)
            .map(|note| (note.fetch, note.until_us));
        let fetch = live.map(|(fetch, _)| fetch);
        let found = self.found(node_id, text, began, now, fetch);
        let mut changes = vec![committed_read(
            self.outpost
                .facts
                .text
                .edit(node_id.number())
                .map(|anchors| anchors.part(&[])),
            node_id.number(),
            &found,
        )];
        if let Some((fetch, until_us)) = live
            && found.shows_effect(fetch.motion)
        {
            changes.push(TextChange::NoteEffect {
                node: node_id.number(),
                until_us,
            });
        }
        let mut seen = Vec::new();
        if let Some(top) = shape.top {
            self.foreground(top, &mut seen);
        }
        let message = OutpostToCore::Event {
            trace_id: self.trace(),
            observed_at_us: began.max(self.generation_began),
            backend: Backend::Msaa,
            window: shape.top.map(facts_in),
            application: None,
            timing: EventTiming::default(),
            event: NormalizedEvent::FocusChanged {
                ancestors,
                node,
                whole_window: false,
                selected_child: None,
                text: Some(Box::new(FocusText::Caret {
                    caret: found.report,
                    selected_text: None,
                    changes: CaretChanges {
                        selection_changes: found.changes,
                        content_changed: found.content_changed,
                        new_selection: found.new_selection,
                    },
                    facts: found.facts.map(Box::new),
                })),
                earlier: None,
                group: None,
                answers: shape.answers,
                first_cached_ancestor: shape.cached.map(|index| FirstCachedAncestor {
                    index,
                    reader: FocusReaderKind::Listener,
                }),
            },
        };
        let change = Change {
            text: changes,
            caret_read: Some((node_id, began)),
            result: Some(message),
            ..Change::default()
        };
        self.commit_focus(field, change, &mut seen);
        if shape.top.is_none() {
            drop_cut(&mut seen);
        }
        seen.dedup_by(same_focus);
        seen
    }

    /// `change`, a focus report, reaches the reducer as a program's commit:
    /// the program in flight's, or one dispatched for it.
    fn commit_focus(&mut self, field: usize, change: Change, seen: &mut Vec<Seen>) {
        let running = |seam: &Self| {
            seam.flight
                .as_ref()
                .map(|flight| flight.request)
                .filter(|&request| seam.outpost.accepts(request))
        };
        if let Some(request) = running(self) {
            self.commit_beside(request, change, seen);
            return;
        }
        self.focusing = Some(Box::new(change));
        self.push_msaa(WinKind::NameChange, field, seen);
        if let (Some(change), Some(request)) = (self.focusing.take(), running(self)) {
            self.commit_beside(request, *change, seen);
        }
    }

    /// `change` commits under `request`, a read still in flight: the world
    /// raises a focus while a read runs, which one program at a time cannot
    /// do, so the commit must not count as that program's own answer.
    fn commit_beside(&mut self, request: Request, change: Change, seen: &mut Vec<Seen>) {
        let running = self
            .outpost
            .current_of(request)
            .and_then(|current| current.running.take());
        self.commit(request, change, seen);
        if let Some(current) = self.outpost.current_of(request) {
            current.running = running;
        }
    }

    /// The clock reaches `now`, Core's held-key deadline firing when due.
    pub fn tick(&mut self, now: Us) -> Vec<Seen> {
        self.now = now;
        let mut seen = Vec::new();
        // A deadline fires when it falls due, so one it sets runs from then.
        while let Some((trace, at)) = self.due
            && at <= now
        {
            self.due = None;
            self.now = at;
            self.enter_core(
                &CoreInput::HeldKeyDue {
                    trace_id: trace,
                    at_us: at,
                },
                &mut seen,
            );
        }
        self.now = now;
        let outpost_now = self.outpost_now();
        let effects = self.outpost.tick(outpost_now);
        let effects = with_answers(&mut self.outpost, &mut self.answers, effects, outpost_now);
        self.after_outpost(effects, &mut seen);
        seen
    }

    /// The outpost ends, and a new generation stands ready.
    pub fn outpost_ended(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        let input = CoreInput::OutpostEnded {
            trace_id: self.trace(),
            outpost: OutpostId(self.generation),
        };
        self.flight = None;
        self.note_log.clear();
        self.generation += 1;
        self.outpost = Reducer::new();
        self.final_outstanding = 0;
        self.focus_reported = false;
        self.reread = None;
        self.generation_began = self.now;
        self.enter_core(&input, &mut seen);
        seen
    }

    /// Verbatim exits.
    pub fn exit(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        self.enter_core(&CoreInput::Exiting { replaced: false }, &mut seen);
        drop_cut(&mut seen);
        seen
    }

    /// Core's caret.
    pub fn core_caret(&mut self) -> Option<usize> {
        self.core.focus_caret_offset().map(|offset| offset as usize)
    }
}

/// How a focus report is shaped, past its node and its read.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    /// The top-level window it is in, with the foreground change that
    /// reached Core before it; `None` for no window facts.
    top: Option<WindowHandle>,
    /// The index of the innermost cached ancestor among its ancestors.
    cached: Option<u32>,
    /// The live re-read it answers.
    answers: Option<u64>,
}

/// The two top-level windows focuses are reported in.
const WINDOWS: [WindowHandle; 2] = [WindowHandle(0x100), WindowHandle(0x200)];

/// The window facts of a window in the top-level window `top`, in front.
fn facts_in(top: WindowHandle) -> WindowFacts {
    WindowFacts {
        top_level: top,
        root_owner: top,
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        foreground_window: top,
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
    }
}

/// Forgets speech cut where the model has no rule for it: on a step that
/// is not a focus in another window, such as exit or a new outpost
/// generation's first focus, whose nodes all have new ids.
fn drop_cut(seen: &mut Vec<Seen>) {
    seen.retain(|seen| !matches!(seen, Seen::Heard(Heard::Cut)));
}

/// Whether `later` and `earlier` are one focus's speech: one focus is one
/// thing heard, whatever utterances say it.
fn same_focus(later: &mut Seen, earlier: &mut Seen) -> bool {
    matches!(
        (later, earlier),
        (Seen::Heard(Heard::Focus), Seen::Heard(Heard::Focus))
    )
}

fn field_of(hwnd: isize) -> usize {
    usize::from(hwnd == window_of(1))
}

fn field_of_node(node: NodeId) -> usize {
    match node.number() {
        5 => 0,
        10 => TERMINAL,
        11 => CONSOLE,
        _ => 1,
    }
}

/// A terminal's snapshot, unchanged from any other, with no caret.
fn terminal_snapshot(now: Us) -> TerminalSnapshot {
    TerminalSnapshot {
        text: TerminalText::unpadded("PS> "),
        screen: TerminalText::unpadded("PS> "),
        from_start: false,
        shrank: false,
        at_document_start: true,
        alternate: false,
        first_row: TerminalText::unpadded("PS> "),
        caret_rows_to_end: 1,
        caret_line: TerminalText::unpadded("PS> "),
        caret_column: 4,
        above: 0,
        started_ms: now / 1_000,
        ended_ms: now / 1_000,
        waited: false,
        read: 1,
        colours: None,
        caret: None,
    }
}

/// What a read found, compared with the node's last read.
struct Found {
    text: Text,
    began: Us,
    report: CaretReport,
    changes: Vec<SelectionChange>,
    content_changed: bool,
    new_selection: Option<SelectionChange>,
    facts: Option<ReadFacts>,
}

impl Found {
    /// Whether it shows the effect of a key of `motion`, as the outpost
    /// narrows a note by.
    fn shows_effect(&self, motion: CaretMotion) -> bool {
        matches!(
            self.facts.as_ref().map(|facts| &facts.change),
            Some(FactsChange::Compared { compared, .. }) if had_effect(*compared, motion)
        )
    }

    /// The reply to a key's read: made for a key, so it has facts.
    fn reply(self) -> CaretReply {
        CaretReply {
            caret: self.report,
            selection_changes: self.changes,
            content_changed: self.content_changed,
            new_selection: self.new_selection,
            facts: self.facts.expect("a key's read has facts"),
        }
    }

    /// The report of a caret event.
    fn event(self, node_id: NodeId) -> NormalizedEvent {
        NormalizedEvent::CaretMoved {
            node_id,
            caret: self.report,
            selection_changes: self.changes,
            content_changed: self.content_changed,
            new_selection: self.new_selection,
            facts: self.facts.map(Box::new),
        }
    }
}

/// The comparisons the outpost makes of `text` with `was`.
fn compared(was: &Text, text: &Text) -> Compared {
    let beside = |text: &Text| {
        (
            text.caret
                .checked_sub(1)
                .and_then(|at| text.letters.get(at).copied()),
            text.at_caret(),
        )
    };
    Compared {
        caret_moved: was.caret != text.caret,
        selection_moved: was.selection != text.selection,
        beside_changed: beside(was) != beside(text),
        unit_changed: was.at_caret() != text.at_caret(),
        line_changed: was.letters != text.letters,
    }
}

/// The key's unit at the caret: the character there, or the line.
fn unit_chunk(text: &Text, report: &CaretReport, unit: TextUnit) -> TextChunk {
    if unit != TextUnit::Character {
        return report.line.clone();
    }
    TextChunk {
        unit: TextUnit::Character,
        text: text.at_caret().map(String::from).unwrap_or_default(),
        offset: 0,
        ..report.line.clone()
    }
}

/// The read's commit of the node's text facts, on `part`, what its program
/// copied: its report the node's last read.
fn committed_read(
    part: Option<(NodeAnchors<u32>, BTreeSet<u64>)>,
    node: u64,
    found: &Found,
) -> TextChange {
    let (mut part, copied) = part.unwrap_or_default();
    let selection = found
        .text
        .selection
        .zip(found.report.selection.clone())
        .map(|((start, end), report)| ReadSelection {
            start: u32::try_from(start).expect("a short line"),
            end: u32::try_from(end).expect("a short line"),
            text: None,
            report,
        });
    part.last = Some(LastRead {
        caret: u32::try_from(found.text.caret).expect("a short line"),
        selection,
        line: found.report.line.clone(),
        text_changed: false,
        read_at_us: found.began,
        unit: found.facts.as_ref().and_then(|facts| facts.unit.clone()),
    });
    TextChange::Edit { node, part, copied }
}
