//! The one seam between the caret generated test (`super`) and production
//! types: everything the test knows of [`Reducer`] and Core's reducer goes
//! through [`Seam`], which speaks the world's words in and out. The caret
//! rework's builder changes this file alone, in its own commit.
//!
//! Written for main's types at 28ee6a5a, which have no Core-owned bound, no
//! final read and no last read in the rework's sense: [`Seam::read_done`]
//! stands in for the outpost's programs (`crates/verbatim-outpost`), which
//! make every read and comparison as `Change`s, and answers them with main's
//! `AwaitCaret` watch, whose judgement moves to Core in the rework.
//! [`cannot_express`] marks what main's types cannot show, so the test fails
//! rather than passing vacuously; the builder replaces each.

use std::collections::{BTreeMap, BTreeSet};

use verbatim_core::{SrState, reduce};
use verbatim_model::protocol::{EventTiming, OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    Backend, CallCounts, CaretKey, CaretMotion, CaretReply, CaretReport, Effect as CoreEffect,
    FocusText, Input as CoreInput, KeySeq, Message, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, QueryId, Role, SegmentContent, Selection,
    SelectionChange, SelectionText, StateSet, TextAnchor, TextChunk, TextOp, TextPosition,
    TextReply, TextUnit, TraceId,
};

use super::super::*;
use super::model::Heard;
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
    /// Core asked the read before a held key.
    BeforeReadAsked,
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
    /// Core asked its final read.
    #[expect(
        dead_code,
        reason = "main's Core asks no final read; the builder's seam makes this"
    )]
    FinalReadAsked,
}

/// What a read in flight is, as a program of main's outpost.
#[derive(Clone, Debug)]
enum ReadKind {
    /// The read before a held key.
    Before(RequestId, TraceId),
    /// A key's watch being opened.
    Key(RequestId, TraceId, verbatim_model::CaretWatch),
    /// The application's caret event.
    Event,
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
}

/// Core's reducer and the outpost's together, with a stand-in for the
/// outpost's programs.
pub struct Seam {
    core: SrState,
    outpost: Reducer,
    answers: Answers,
    generation: u64,
    flight: Option<Flight>,
    /// Core's held-key deadline: the key and when it falls due.
    due: Option<(TraceId, Us)>,
    keys: BTreeMap<TraceId, usize>,
    /// The fields' caret reports' numbers.
    reports: u64,
    /// The letters of each node's last committed read, which main's programs
    /// compare as the node's value change's note.
    letters: BTreeMap<u64, Vec<char>>,
    now: Us,
    next_trace: u64,
    /// What this seam could not express.
    pub unexpressed: BTreeSet<&'static str>,
}

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

/// `text`'s caret as a report of field `field` in generation `generation`.
fn caret_report(text: &Text, field: usize) -> CaretReport {
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
        selection: text.selection.map(|(start, end)| Selection {
            start: position(start),
            end: position(end),
        }),
        selection_line: None,
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
            due: None,
            keys: BTreeMap::new(),
            reports: 0,
            letters: BTreeMap::new(),
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
        self.after_outpost(effects, seen);
    }

    /// Follows the outpost's effects: its messages reach Core, and its
    /// programs are taken, a caret read held for the world to end.
    fn after_outpost(&mut self, effects: Vec<Effect>, seen: &mut Vec<Seen>) {
        for effect in effects {
            match effect {
                Effect::Send(message) => self.enter_core_message(*message, seen),
                Effect::Dispatch(program) => self.take_program(&program, seen),
                _ => {}
            }
        }
    }

    fn take_program(&mut self, program: &Program, seen: &mut Vec<Seen>) {
        let Work::Entry { entry, .. } = &program.work else {
            self.finish(program.request, seen);
            return;
        };
        let read = match &entry.item {
            Item::Query {
                request_id,
                query:
                    Query::Text {
                        node_id,
                        op: TextOp::ReadCaret,
                    },
            } => Some((*node_id, ReadKind::Before(*request_id, entry.trace))),
            Item::Query {
                request_id,
                query:
                    Query::Text {
                        node_id,
                        op: TextOp::AwaitCaret(watch),
                    },
            } => Some((
                *node_id,
                ReadKind::Key(*request_id, entry.trace, watch.clone()),
            )),
            Item::Msaa { hwnd, .. } => {
                Some((node_of(self.generation, field_of(*hwnd)), ReadKind::Event))
            }
            _ => None,
        };
        let Some((node, kind)) = read else {
            self.finish(program.request, seen);
            return;
        };
        self.flight = Some(Flight {
            request: program.request,
            node,
            field: field_of_node(node),
            kind,
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
            },
            OutpostToCore::Event {
                trace_id,
                observed_at_us,
                backend,
                event,
                ..
            } => CoreInput::Event {
                trace_id,
                observed_at_us,
                source: Pid(1),
                backend,
                window: None,
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
            CoreEffect::PassHeldKey(trace) => {
                if let Some(&id) = self.keys.get(&trace) {
                    seen.push(Seen::Passed(id));
                }
            }
            CoreEffect::BoundHeldKey(bound) => {
                self.due = bound.map(|(trace, ms)| (trace, self.now + ms * 1_000));
            }
            CoreEffect::Text(request) => {
                if request.op == TextOp::ReadCaret {
                    seen.push(Seen::BeforeReadAsked);
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

    /// The read in flight ended at `now`, seeing `text`.
    ///
    /// # Panics
    ///
    /// With no read in flight.
    #[expect(
        clippy::too_many_lines,
        clippy::single_match_else,
        reason = "one arm for each kind of read, as main's programs take them"
    )]
    pub fn read_done(&mut self, text: &Text, now: Us) -> Vec<Seen> {
        self.now = now;
        let flight = self.flight.take().expect("a read in flight");
        let mut seen = Vec::new();
        if !self.outpost.accepts(flight.request) {
            return seen;
        }
        let field = flight.field;
        let last = last_selection(&flight, text, self.letters.get(&flight.node.number()));
        self.letters
            .insert(flight.node.number(), text.letters.clone());
        let content_changed = last.letters != text.letters;
        let new_selection = text
            .selection
            .filter(|_| content_changed)
            .map(|(start, end)| {
                let letters: String = text.letters[start..end].iter().collect();
                SelectionChange {
                    selected: true,
                    characters: u32::try_from(end - start).expect("a short line"),
                    text: letters,
                }
            });
        let change = committed_read(&flight, text, self.reports + 1);
        self.reports += 1;
        let report = caret_report(text, field);
        let changes = report_changes(selection_changes(&last, text));
        let reported = Seen::Report {
            text: text.clone(),
            began: flight.began,
            ended: now,
            final_read: false,
        };
        let reply = |moved: bool, changes: Vec<SelectionChange>| {
            TextReply::Caret(Box::new(CaretReply {
                moved,
                caret: report.clone(),
                read_at_ms: flight.began / 1_000,
                unit: None,
                selection_changes: changes,
                content_changed,
                new_selection: new_selection.clone(),
                same_line: None,
                removed: None,
            }))
        };
        match &flight.kind {
            ReadKind::Before(request_id, trace) => {
                seen.push(Seen::Committed(field, text.clone(), true));
                // The report reaches Core before the pass it causes.
                seen.push(reported);
                self.commit(
                    flight.request,
                    Change {
                        text: vec![change],
                        caret_read: Some((flight.node, flight.began)),
                        result: Some(OutpostToCore::Reply {
                            trace_id: *trace,
                            request_id: *request_id,
                            outcome: QueryOutcome::Done(QueryResult::Text(reply(
                                false,
                                Vec::new(),
                            ))),
                            timing: EventTiming::default(),
                        }),
                        ..Change::default()
                    },
                    &mut seen,
                );
            }
            ReadKind::Key(request_id, trace, watch) => {
                seen.push(Seen::Committed(field, text.clone(), false));
                if has_evidence(watch, &last, text, &report) {
                    self.commit(
                        flight.request,
                        Change {
                            text: vec![change],
                            caret_read: Some((flight.node, flight.began)),
                            result: Some(OutpostToCore::Reply {
                                trace_id: *trace,
                                request_id: *request_id,
                                outcome: QueryOutcome::Done(QueryResult::Text(reply(
                                    true, changes,
                                ))),
                                timing: EventTiming::default(),
                            }),
                            ..Change::default()
                        },
                        &mut seen,
                    );
                    seen.push(reported);
                } else {
                    self.commit(
                        flight.request,
                        Change {
                            text: vec![change],
                            caret_read: Some((flight.node, flight.began)),
                            watch: Some(WatchChange::Open {
                                open: OpenWatch {
                                    request_id: *request_id,
                                    trace: *trace,
                                    node_id: flight.node,
                                    watch: watch.clone(),
                                    timing: EventTiming::default(),
                                    calls: CallCounts::default(),
                                },
                                console: false,
                            }),
                            ..Change::default()
                        },
                        &mut seen,
                    );
                }
            }
            ReadKind::Event => {
                seen.push(Seen::Committed(field, text.clone(), false));
                let open = self.outpost.facts.follow.watch.kept().cloned();
                match open.filter(|open| open.node_id == flight.node) {
                    Some(open) => {
                        self.commit(
                            flight.request,
                            Change {
                                watch: Some(WatchChange::Take(flight.node)),
                                caret_read: Some((flight.node, flight.began)),
                                ..Change::default()
                            },
                            &mut seen,
                        );
                        let checked = if has_evidence(&open.watch, &last, text, &report) {
                            Checked::Answered {
                                reply: reply(true, changes),
                                calls: CallCounts::default(),
                                awaited_at_us: now,
                            }
                        } else {
                            Checked::Watching {
                                calls: CallCounts::default(),
                            }
                        };
                        let answered = matches!(checked, Checked::Answered { .. });
                        self.commit(
                            flight.request,
                            Change {
                                text: vec![change],
                                watch: Some(WatchChange::Checked(checked)),
                                ..Change::default()
                            },
                            &mut seen,
                        );
                        if answered {
                            seen.push(reported);
                        }
                    }
                    None => {
                        let trace_id = self.trace();
                        self.commit(
                            flight.request,
                            Change {
                                text: vec![change],
                                caret_read: Some((flight.node, flight.began)),
                                result: Some(OutpostToCore::Event {
                                    trace_id,
                                    observed_at_us: flight.began,
                                    backend: Backend::Msaa,
                                    window: None,
                                    application: None,
                                    timing: EventTiming::default(),
                                    event: NormalizedEvent::CaretMoved {
                                        node_id: flight.node,
                                        caret: report,
                                        selection_changes: changes,
                                        content_changed,
                                        new_selection: new_selection.clone(),
                                    },
                                }),
                                ..Change::default()
                            },
                            &mut seen,
                        );
                        seen.push(reported);
                    }
                }
            }
        }
        self.finish(flight.request, &mut seen);
        seen
    }

    /// The read in flight is abandoned: its deadline passes.
    pub fn abandon(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        if self.flight.take().is_none() {
            return seen;
        }
        let Some(deadline) = self.outpost.flight.deadline() else {
            return seen;
        };
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

    /// A focus on `field`, reported at `now` from a read begun at `began`.
    pub fn focus(&mut self, field: usize, text: &Text, began: Us, now: Us) -> Vec<Seen> {
        self.now = now;
        let node = snapshot(
            self.generation,
            5 + field as u64,
            Role::EditableText,
            "Field",
        );
        let input = CoreInput::Event {
            trace_id: self.trace(),
            observed_at_us: began,
            source: Pid(1),
            backend: Backend::Msaa,
            window: None,
            event: NormalizedEvent::FocusChanged {
                ancestors: vec![snapshot(self.generation, 1, Role::Window, "Window")],
                node,
                whole_window: false,
                selected_child: None,
                text: Some(Box::new(FocusText::Caret {
                    caret: caret_report(text, field),
                    selected_text: None,
                })),
                earlier: None,
                group: None,
                answers: None,
                first_cached_ancestor: None,
            },
            purpose: None,
        };
        let mut seen = Vec::new();
        self.enter_core(&input, &mut seen);
        // One focus is one thing heard, whatever utterances say it.
        seen.dedup_by(|later, earlier| {
            matches!(
                (later, earlier),
                (Seen::Heard(Heard::Focus), Seen::Heard(Heard::Focus))
            )
        });
        seen
    }

    /// The clock reaches `now`, Core's held-key deadline firing when due.
    pub fn tick(&mut self, now: Us) -> Vec<Seen> {
        self.now = now;
        let mut seen = Vec::new();
        if let Some((trace, at)) = self.due
            && at <= now
        {
            self.due = None;
            self.enter_core(&CoreInput::HeldKeyDue { trace_id: trace }, &mut seen);
        }
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
        self.generation += 1;
        self.outpost = Reducer::new();
        self.letters.clear();
        self.enter_core(&input, &mut seen);
        seen
    }

    /// Verbatim exits.
    pub fn exit(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        self.enter_core(&CoreInput::Exiting { replaced: false }, &mut seen);
        seen
    }

    /// Core's caret, or `None` with what could not be expressed recorded.
    pub fn core_caret(&mut self) -> Option<usize> {
        self.unexpressed.insert(cannot_express(
            "Core's caret: SrState does not expose it outside its crate",
        ));
        None
    }
}

/// Marks what main's types cannot show the test: the test fails on it at
/// its end, so it never passes vacuously. The builder replaces each.
#[must_use]
pub fn cannot_express(what: &'static str) -> &'static str {
    what
}

fn field_of(hwnd: isize) -> usize {
    usize::from(hwnd == window_of(1))
}

fn field_of_node(node: NodeId) -> usize {
    usize::from(node.number() != 5)
}

/// What the read in flight compares its selection with: the node's kept
/// selection in its copy of the facts, or none.
fn last_selection(flight: &Flight, text: &Text, letters: Option<&Vec<char>>) -> Text {
    let kept = flight
        .facts
        .text
        .edit(flight.node.number())
        .and_then(|anchors| anchors.selection.clone());
    let mut last = text.clone();
    if let Some(letters) = letters {
        last.letters.clone_from(letters);
    }
    match kept {
        Some(kept) => {
            let (start, end) = kept.ends;
            last.selection = (!kept.collapsed).then_some((start as usize, end as usize));
            if kept.collapsed {
                last.caret = start as usize;
            }
        }
        None => last.selection = None,
    }
    last
}

/// The read's commit of the node's text facts: its selection the one the
/// next report compares with.
fn committed_read(flight: &Flight, text: &Text, report: u64) -> TextChange {
    let node = flight.node.number();
    let (mut part, copied) = flight
        .facts
        .text
        .edit(node)
        .map(|anchors| anchors.part(&[]))
        .unwrap_or_default();
    let (start, end) = text.ends();
    part.selection = Some(KeptSelection {
        ends: (
            u32::try_from(start).expect("a short line"),
            u32::try_from(end).expect("a short line"),
        ),
        collapsed: text.selection.is_none(),
        text: None,
        report,
    });
    part.reports = report;
    TextChange::Edit { node, part, copied }
}

/// Main's evidence, in the watch's terms: the caret left where Core knew it,
/// the selection changed, or the text the key compares changed.
fn has_evidence(
    watch: &verbatim_model::CaretWatch,
    last: &Text,
    text: &Text,
    report: &CaretReport,
) -> bool {
    let moved = watch
        .since
        .is_none_or(|since| since.offset != report.line.offset);
    let compared_changed = watch.compare.as_deref().is_some_and(|compare| {
        text.at_caret().map(|letter| letter.to_string()).as_deref() != Some(compare)
    });
    moved || compared_changed || last.ends() != text.ends()
}
