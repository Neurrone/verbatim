//! The one seam between the caret watch's generated test (`super`) and
//! production types: Core's reducer (`verbatim_core::reduce`, through its
//! event queue's inputs) and the outpost's reducer
//! ([`verbatim_outpost_logic::Reducer`]) are driven here, through the
//! requests and answers of `verbatim_model`, and everything else speaks the
//! world's words.
//!
//! What stands in for Windows: the outpost's programs, which make every
//! call into the application in `verbatim-outpost`, and the listener's
//! reads. Each program answers from the world's field as it is when the
//! program ends, after the latency the script gives its read, and answers
//! exactly what `verbatim-outpost` answers: a watch's check compares the
//! field with the field as the key's starting read found it, by the
//! effect test `verbatim_model::had_effect` (`text/check.rs`), and with the
//! effect, or at the bound, makes the final read in the same program; a
//! watch's final read reads the caret and the key's unit. Checks and final
//! reads take no time, as the model's reads.
//!
//! The hook stands in too: a key is held, bound to the field, exactly when
//! the model says the committed focus is the field (Core publishes the
//! hook's view only to the shell, `caret_key::binding`).

use std::collections::BTreeMap;
use std::time::Duration;

use verbatim_core::{Runtime, SrState, reduce};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::protocol::{OutpostToCore, UiaSnapshotFact};
use verbatim_model::request::{
    Answer, Ask, CaretBinding, FocusAncestor, FocusCaret, FocusCaretRead, FocusNode, FocusRead,
    ForegroundReport, HeldIds, Instance, MsaaIds, NodeRead, NodeWindow, ObjectClass, ObjectRead,
    Outcome, Press as WirePress, PressAction, Probe, ProcessFact, QueueEffect, QueueInput, Reader,
    Reply, Request as Wire, Route, RouteEnd, RouteStep, RunRead, Tried, WindowVerdict,
};
use verbatim_model::stroke::{HeldStroke, SentTransition, Stroke};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Event, EventObject, Payload, Read, Relation,
    TellWindow, WindowFacet,
};
use verbatim_model::{
    CaretKey, CaretMotion, CaretReport, Compared, Effect, Input as CoreInput, KeySeq, NodeDetails,
    NodeId, OutpostId, Pid, Role, SegmentContent, Started, StateSet, TextAnchor, TextChunk, TextOp,
    TextProvider, TextReply, TextUnit, TraceId, WatchFinalRead, Watched, WatchedRead, WindowHandle,
    had_effect,
};
use verbatim_outpost_logic::{
    Effect as OutpostEffect, Entry, Input as OutpostInput, Item, Now, Reducer, Request, UiaEvent,
    UiaKind, WatchProgram, WatchWork, Work,
};

use super::model::Heard;
use super::world::{App, Key, Kind, Us};

/// The application's process.
const APP: Pid = Pid(40);
/// Its outpost's generation.
const GENERATION: u64 = 3;
const LISTENER: Instance = Instance {
    reader: Reader::Listener,
    generation: 1,
};
const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: GENERATION,
};
/// The field's node, as the outpost numbers it.
const FIELD: u64 = 2;
/// The button's node.
const BUTTON: u64 = 7;

/// What crossing the seam brought back, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Something was said.
    Heard(Heard),
    /// Key `0`'s stroke was sent to the application.
    Sent(usize),
}

/// A program the stand-in runs, ending at a time.
#[derive(Clone, Debug)]
enum Running {
    /// The outpost's program `0`, of Core's request `1`.
    Outpost(Request, Box<Ask>),
    /// A watch's check or final read, program `0`.
    Watch(Request, Box<WatchProgram>),
    /// Another program of the outpost's, which reads nothing Core asked.
    Own(Request),
    /// The listener's read for request `0`.
    Listener(Box<Ask>),
}

/// Core and the outpost, with the stand-ins.
pub struct Seam {
    kind: Kind,
    core: SrState,
    runtime: Runtime,
    outpost: Reducer,
    now: Us,
    /// Core's wake.
    wake: Option<Us>,
    /// Programs running, by the time they end, in order.
    running: Vec<(Us, Running)>,
    /// Each held key's stroke not yet sent, by its place in the hook's line.
    unsent: BTreeMap<u32, usize>,
    /// The latency of each key's read before it, and of the focus's reads.
    read_us: Vec<Us>,
    focus_us: (Us, Us),
    /// The anchors the stand-in minted.
    next_anchor: u64,
    /// The field as each starting read found it, by its starting point.
    points: BTreeMap<u64, App>,
    /// What crossed the seam, for a failure's report.
    pub log: Vec<String>,
}

fn now_of(at: Us) -> Now {
    Now {
        at: Duration::from_micros(at),
        observed_us: at,
        wall_us: at,
    }
}

impl Seam {
    /// Core started with the listener and the application's outpost, the
    /// field committed with `app`'s text and caret; what startup said is
    /// returned for the caller to assert.
    pub fn new(
        kind: Kind,
        app: &App,
        read_us: Vec<Us>,
        focus_us: (Us, Us),
        at: Us,
    ) -> (Self, Vec<Heard>) {
        let mut seam = Self {
            kind,
            core: SrState::default(),
            runtime: Runtime::default(),
            outpost: Reducer::new(),
            now: at,
            wake: None,
            running: Vec::new(),
            unsent: BTreeMap::new(),
            read_us,
            focus_us,
            next_anchor: 100,
            points: BTreeMap::new(),
            log: Vec::new(),
        };
        let mut seen = Vec::new();
        seen.extend(seam.core(QueueInput::Process(ProcessFact::ListenerStarted(1)), app));
        seen.extend(seam.core(QueueInput::ReaderReady(LISTENER), app));
        seen.extend(seam.core(
            QueueInput::Foreground(ForegroundReport {
                window: WindowHandle(4),
                root_owner: WindowHandle(4),
                process: APP,
                thread: 41,
                class: "Frame".to_owned(),
            }),
            app,
        ));
        seen.extend(seam.core(
            QueueInput::Process(ProcessFact::OutpostStarted {
                process: APP,
                generation: GENERATION,
            }),
            app,
        ));
        seen.extend(seam.core(QueueInput::ReaderReady(OUTPOST), app));
        // The startup read, and the field's `Follow`, end at once.
        seen.extend(seam.advance(at, app));
        let heard = seen
            .into_iter()
            .filter_map(|seen| match seen {
                Seen::Heard(heard) => Some(heard),
                Seen::Sent(_) => None,
            })
            .collect();
        (seam, heard)
    }

    /// The next time something of Core's or the stand-ins' is due.
    pub fn next_due(&self) -> Option<Us> {
        let program = self.running.iter().map(|(at, _)| *at).min();
        [self.wake, program, self.outpost_timer()]
            .into_iter()
            .flatten()
            .min()
    }

    /// When the outpost's shell next ticks its reducer: the waiting
    /// table's next check.
    fn outpost_timer(&self) -> Option<Us> {
        self.outpost
            .next_timer(now_of(self.now))
            .map(|at| u64::try_from(at.as_micros()).expect("a run's times fit"))
    }

    /// Key `key` pressed at `at`: held and bound to the field when `held`.
    pub fn press(&mut self, key: usize, which: Key, held: bool, at: Us, app: &App) -> Vec<Seen> {
        self.now = at;
        let sequence = u32::try_from(key + 1).expect("few keys");
        let caret_key = caret_key(which);
        let (held_stroke, bound) = if held {
            self.unsent.insert(sequence, key);
            (
                Some(HeldStroke {
                    stroke: stroke(which, sequence),
                    modifiers: modifiers(which),
                }),
                Some(Box::new(CaretBinding {
                    node: node(FIELD),
                    window: window(5),
                    role: Role::EditableText,
                    provider: match self.kind {
                        Kind::Notepad => TextProvider::Uia,
                        Kind::TextBox => TextProvider::EditMessages,
                    },
                    record: None,
                    formatting: verbatim_model::TextAttributes::default(),
                })),
            )
        } else {
            (None, None)
        };
        let press = WirePress {
            key_seq: KeySeq::new(u64::from(sequence)),
            pressed_at_us: at,
            action: PressAction::CaretKey {
                key: caret_key,
                held: held_stroke,
                bound,
            },
        };
        let mut seen = Vec::new();
        if !held {
            seen.push(Seen::Sent(key));
        }
        seen.extend(self.core_traced(
            QueueInput::Press(Box::new(press)),
            TraceId::from_raw(1_000 + u64::from(sequence)),
            app,
        ));
        seen
    }

    /// The listener tells Core of the button's focus at `at`.
    pub fn focus_told(&mut self, at: Us, app: &App) -> Vec<Seen> {
        self.now = at;
        let tell = match self.kind {
            Kind::Notepad => uia_focus(at),
            Kind::TextBox => msaa_focus(at),
        };
        self.core(
            QueueInput::Tell {
                from: LISTENER,
                tell: Box::new(tell),
            },
            app,
        )
    }

    /// The application raised its events for the field at `at`: a
    /// `TextSelectionChanged`, and a `TextChanged` after it when
    /// `text_changed`, heard by the outpost following the field.
    pub fn raised(&mut self, at: Us, text_changed: bool, app: &App) -> Vec<Seen> {
        self.now = at;
        let mut seen = Vec::new();
        let kinds = if text_changed {
            vec![UiaKind::TextSelection, UiaKind::TextChanged]
        } else {
            vec![UiaKind::TextSelection]
        };
        for kind in kinds {
            let entry = Entry::own(
                Item::Uia(UiaEvent {
                    kind,
                    parts: UiaSnapshotFact {
                        runtime_id: vec![42, 1],
                        role: Role::EditableText,
                        name: Some("Body".to_owned()),
                        value: None,
                        states: StateSet::default(),
                        details: NodeDetails::default(),
                    },
                    hwnd: 0,
                    focus_window: 0,
                    element: None,
                }),
                TraceId::from_raw(5_000),
                at,
            );
            let effects = self.outpost.input(OutpostInput::Entry(entry), now_of(at));
            seen.extend(self.outpost_effects(effects, app));
        }
        seen
    }

    /// Runs whatever of Core's and the stand-ins' is due at `at`.
    pub fn advance(&mut self, at: Us, app: &App) -> Vec<Seen> {
        self.now = at;
        let mut seen = Vec::new();
        loop {
            if let Some(place) = self.running.iter().position(|(end, _)| *end <= at) {
                let (_, program) = self.running.remove(place);
                seen.extend(self.end(program, app));
                continue;
            }
            if self.wake.is_some_and(|wake| wake <= at) {
                self.wake = None;
                seen.extend(self.core(QueueInput::Due, app));
                continue;
            }
            if self.outpost_timer().is_some_and(|due| due <= at) {
                let effects = self.outpost.tick(now_of(at));
                if effects.is_empty() {
                    break;
                }
                seen.extend(self.outpost_effects(effects, app));
                continue;
            }
            break;
        }
        seen
    }

    fn core(&mut self, input: QueueInput, app: &App) -> Vec<Seen> {
        self.core_traced(input, TraceId::from_raw(1), app)
    }

    /// Feeds Core one input and carries out its effects.
    fn core_traced(&mut self, input: QueueInput, trace_id: TraceId, app: &App) -> Vec<Seen> {
        self.note(format!("Core takes {input:?}"));
        let effects = reduce(
            &mut self.core,
            &mut self.runtime,
            &CoreInput::Queue {
                trace_id,
                at_us: self.now,
                input,
            },
        );
        let mut seen = Vec::new();
        for effect in effects {
            if !matches!(effect, Effect::Queue(QueueEffect::WakeAt(_))) {
                self.note(format!("Core gives {effect:?}"));
            }
            match effect {
                Effect::Speak(utterance, _) => {
                    let parts: Vec<SegmentContent> = utterance
                        .segments
                        .into_iter()
                        .map(|segment| segment.content)
                        .filter(|content| !matches!(content, SegmentContent::Mark(_)))
                        .collect();
                    seen.push(Seen::Heard(heard(&parts)));
                }
                Effect::Queue(queue) => seen.extend(self.queue_effect(queue, app)),
                _ => {}
            }
        }
        seen
    }

    fn queue_effect(&mut self, effect: QueueEffect, app: &App) -> Vec<Seen> {
        match effect {
            QueueEffect::Send { to, mut ask } => match to.reader {
                Reader::Listener => {
                    let end = self.now + self.focus_us.0;
                    self.running.push((end, Running::Listener(Box::new(ask))));
                    Vec::new()
                }
                Reader::Outpost(_) => {
                    ask.unstamp();
                    let effects = self
                        .outpost
                        .input(OutpostInput::Ask(Box::new(ask)), now_of(self.now));
                    self.outpost_effects(effects, app)
                }
            },
            QueueEffect::Cancel { to, cancel } => match to.reader {
                Reader::Listener => {
                    self.running.retain(|(_, program)| {
                        !matches!(program, Running::Listener(ask) if ask.number == cancel.number)
                    });
                    Vec::new()
                }
                Reader::Outpost(_) => {
                    let effects = self
                        .outpost
                        .input(OutpostInput::CancelAsk(cancel), now_of(self.now));
                    self.outpost_effects(effects, app)
                }
            },
            QueueEffect::SendStrokes(sent) => sent
                .into_iter()
                .filter_map(|transition| match transition {
                    SentTransition::Held(held) => self.unsent.remove(&held.stroke.sequence),
                    SentTransition::Deferred { .. } => None,
                })
                .map(Seen::Sent)
                .collect(),
            QueueEffect::WakeAt(wake) => {
                self.wake = wake;
                Vec::new()
            }
            QueueEffect::CheckHung(windows) => {
                let mut seen = Vec::new();
                for window in windows {
                    seen.extend(self.core(
                        QueueInput::HangCheck {
                            window,
                            hung: false,
                        },
                        app,
                    ));
                }
                seen
            }
            QueueEffect::Answered { .. }
            | QueueEffect::ReleaseAtExit(_)
            | QueueEffect::WindowDestroyed { .. }
            | QueueEffect::AnswerDropped { .. }
            | QueueEffect::Undecodable(_)
            | QueueEffect::Mismatch { .. }
            | QueueEffect::StartOutpost(_)
            | QueueEffect::EndOutpost(_)
            | QueueEffect::StaleFrame(_) => Vec::new(),
        }
    }

    /// Notes what crossed the seam at this time.
    fn note(&mut self, what: String) {
        let at = self.now.saturating_sub(super::world::FIRST);
        let mut what = what;
        what.truncate(400);
        self.log
            .push(format!("{}.{:03} ms: {what}", at / 1_000, at % 1_000));
    }

    /// Carries out the outpost's effects.
    fn outpost_effects(&mut self, effects: Vec<OutpostEffect>, app: &App) -> Vec<Seen> {
        let mut seen = Vec::new();
        for effect in effects {
            self.note(format!("the outpost gives {effect:?}"));
            match effect {
                OutpostEffect::Dispatch(program) => {
                    let ends = match &program.work {
                        Work::Ask(ask) => self.latency(ask),
                        _ => 0,
                    };
                    let running = match program.work {
                        Work::Ask(ask) => Running::Outpost(program.request, ask),
                        Work::Watch(watch) => Running::Watch(program.request, watch),
                        _ => Running::Own(program.request),
                    };
                    self.running.push((self.now + ends, running));
                }
                OutpostEffect::Send(message) => {
                    if let OutpostToCore::Answer(mut answer) = *message {
                        answer.assign_outpost(OutpostId(GENERATION));
                        seen.extend(self.core(
                            QueueInput::Answer {
                                from: OUTPOST,
                                answer,
                            },
                            app,
                        ));
                    }
                }
                OutpostEffect::Release { nodes, anchors, .. } => {
                    let effects = self.outpost.input(
                        OutpostInput::Released {
                            nodes: nodes.into_iter().collect(),
                            anchors: anchors.into_iter().collect(),
                            objects: false,
                        },
                        now_of(self.now),
                    );
                    seen.extend(self.outpost_effects(effects, app));
                }
                OutpostEffect::LogicError(error) => panic!("the outpost's logic error: {error:?}"),
                _ => {}
            }
        }
        seen
    }

    /// How long the stand-in takes to answer `ask`: a key's read before it
    /// the script's latency for the oldest held key not yet sent, which is
    /// the key at the front; the focus's completion the script's; the rest
    /// at once.
    fn latency(&self, ask: &Ask) -> Us {
        match &ask.request {
            Wire::Text {
                op: TextOp::StartingRead(_),
                ..
            } => self
                .unsent
                .values()
                .next()
                .map_or(0, |&key| self.read_us[key]),
            Wire::CompleteUia(_) | Wire::MsaaRun(_) => self.focus_us.1,
            _ => 0,
        }
    }

    /// A program ends: its answer, from the field as it is now.
    fn end(&mut self, program: Running, app: &App) -> Vec<Seen> {
        match program {
            Running::Listener(ask) => {
                let answer = Answer {
                    number: ask.number,
                    outcome: Outcome::Result(Reply::Object(Box::new(listener_read()))),
                    probes: Vec::new(),
                    detail: String::new(),
                };
                self.core(
                    QueueInput::Answer {
                        from: LISTENER,
                        answer,
                    },
                    app,
                )
            }
            Running::Outpost(request, ask) => {
                let reply = self.reply(&ask, app);
                // A window with no verdict in force is probed first, and the
                // probe answered: Notepad's has a UI Automation provider,
                // the text box's window none.
                let probes = (ask.verdict == WindowVerdict::ProbeFirst)
                    .then(|| Probe {
                        window: ask.window,
                        thread: 41,
                        provider: self.kind == Kind::Notepad,
                        began_us: self.now,
                    })
                    .into_iter()
                    .collect();
                let answer = Answer {
                    number: ask.number,
                    outcome: Outcome::Result(reply),
                    probes,
                    detail: String::new(),
                };
                let mut effects = self.outpost.input(
                    OutpostInput::Answered {
                        request,
                        answer: Box::new(answer),
                    },
                    now_of(self.now),
                );
                effects.extend(self.outpost.input(
                    OutpostInput::Finished {
                        request,
                        reported: false,
                        panicked: false,
                        cut: None,
                    },
                    now_of(self.now),
                ));
                self.outpost_effects(effects, app)
            }
            Running::Watch(request, program) => {
                let mut effects = Vec::new();
                if let Some(watched) = self.watched(&program, app) {
                    let answer = Answer {
                        number: program.number,
                        outcome: Outcome::Result(Reply::Text(TextReply::Watched(Box::new(
                            watched,
                        )))),
                        probes: Vec::new(),
                        detail: String::new(),
                    };
                    effects.extend(self.outpost.input(
                        OutpostInput::Answered {
                            request,
                            answer: Box::new(answer),
                        },
                        now_of(self.now),
                    ));
                }
                effects.extend(self.outpost.input(
                    OutpostInput::Finished {
                        request,
                        reported: false,
                        panicked: false,
                        cut: None,
                    },
                    now_of(self.now),
                ));
                self.outpost_effects(effects, app)
            }
            Running::Own(request) => {
                let effects = self.outpost.input(
                    OutpostInput::Finished {
                        request,
                        reported: false,
                        panicked: false,
                        cut: None,
                    },
                    now_of(self.now),
                );
                self.outpost_effects(effects, app)
            }
        }
    }

    /// What `verbatim-outpost`'s program answers to `ask`, read from `app`.
    fn reply(&mut self, ask: &Ask, app: &App) -> Reply {
        match &ask.request {
            Wire::CurrentFocus { .. } => Reply::Focus(Box::new(self.field_read(app))),
            Wire::Follow(_) => Reply::Followed,
            Wire::CompleteUia(_) => Reply::Focus(Box::new(button_read(self.kind))),
            Wire::MsaaRun(_) => Reply::Run(RunRead {
                tried: vec![Tried::Focus(Box::new(button_read(self.kind)))],
            }),
            Wire::Text {
                op: TextOp::StartingRead(read),
                ..
            } => {
                let point = self.anchor();
                self.points.insert(point.0, app.clone());
                Reply::Text(TextReply::Started(Box::new(Started {
                    point,
                    caret: self.caret(app),
                    unit: self.unit(read.unit, app),
                    word: read.word.then(|| self.unit(TextUnit::Word, app)),
                })))
            }
            other => panic!("the stand-in answers no request {other:?}"),
        }
    }

    /// What watch `program`'s check or final read answers, read from
    /// `app`: `None` for a check that shows no effect before the bound.
    fn watched(&mut self, program: &WatchProgram, app: &App) -> Option<Watched> {
        let (unit, at_effect) = match program.watch.final_read {
            WatchFinalRead::Unit { unit, .. } => (unit, false),
            WatchFinalRead::UnitAtEffect { unit, .. } => (unit, true),
            WatchFinalRead::Terminal => panic!("the stand-in is asked no terminal's watch"),
        };
        let effect = match program.work {
            WatchWork::Final => true,
            WatchWork::Check { word, .. } => {
                let before = self
                    .points
                    .get(&program.watch.point.0)
                    .expect("a watch names a starting point the stand-in minted");
                had_effect(compared(before, app, word), program.watch.motion)
            }
        };
        let last = matches!(program.work, WatchWork::Check { last: true, .. });
        if !effect && !last {
            return None;
        }
        if !effect && at_effect {
            return Some(Watched::BoundUnread);
        }
        let read = WatchedRead {
            caret: self.caret(app),
            unit: self.unit(unit, app),
        };
        Some(if effect {
            Watched::Effect(read)
        } else {
            Watched::Bound(read)
        })
    }

    fn anchor(&mut self) -> TextAnchor {
        self.next_anchor += 1;
        TextAnchor(self.next_anchor)
    }

    /// The line, with the caret at its offset.
    fn caret(&mut self, app: &App) -> CaretReport {
        let start = self.anchor();
        CaretReport {
            line: chunk(TextUnit::Line, &app.text, start, app.caret),
            selection: None,
            read_at_us: self.now,
        }
    }

    /// The unit at the caret, as the application's text provider gives it.
    fn unit(&mut self, unit: TextUnit, app: &App) -> TextChunk {
        let text = match unit {
            TextUnit::Character => app.character().map(String::from).unwrap_or_default(),
            TextUnit::Word => app.word(),
            _ => app.text.clone(),
        };
        let start = self.anchor();
        chunk(unit, &text, start, 0)
    }

    /// The field's focus read, its caret with it.
    fn field_read(&mut self, app: &App) -> FocusRead {
        let caret = self.caret(app);
        let baseline = self.anchor();
        let mut read = focus_read(node(FIELD), window(5), values("Body", Role::EditableText));
        match self.kind {
            Kind::Notepad => read.focus.runtime_id = Some(RuntimeId(vec![42, 1])),
            Kind::TextBox => {
                read.focus.runtime_id = None;
                read.msaa = Some(HeldIds {
                    event: MsaaIds {
                        window: WindowHandle(5),
                        object: -4,
                        child: 0,
                    },
                    held_child: 0,
                });
            }
        }
        read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
            caret,
            selected_text: None,
            baseline,
            difference: None,
        }));
        read
    }
}

/// How `now` compares with `before`, as a check compares the caret with a
/// key's starting point: the caret's place, the characters beside it, the
/// line, and with `word` the word at the caret.
fn compared(before: &App, now: &App, word: bool) -> Compared {
    Compared {
        caret_moved: now.caret != before.caret,
        selection_moved: false,
        beside_changed: now.before() != before.before() || now.character() != before.character(),
        unit_changed: word && now.word() != before.word(),
        line_changed: now.text != before.text,
    }
}

/// The application's button "OK" read as the focus.
fn button_read(kind: Kind) -> FocusRead {
    let handle = match kind {
        Kind::Notepad => 5,
        Kind::TextBox => 6,
    };
    let mut read = focus_read(node(BUTTON), window(handle), values("OK", Role::Button));
    match kind {
        Kind::Notepad => read.focus.runtime_id = Some(RuntimeId(vec![42, 7])),
        Kind::TextBox => {
            read.focus.runtime_id = None;
            read.msaa = Some(HeldIds {
                event: MsaaIds {
                    window: WindowHandle(6),
                    object: -4,
                    child: 0,
                },
                held_child: 0,
            });
        }
    }
    read.caret = FocusCaret::NoText;
    read
}

fn focus_read(node: NodeId, window: NodeWindow, values: PropertyValues) -> FocusRead {
    FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: Some(node),
                window,
                values,
            },
            runtime_id: None,
        },
        msaa: None,
        redirected_to: None,
        ancestors: vec![FocusAncestor {
            node: FocusNode {
                read: NodeRead {
                    node: Some(NodeId::new(1)),
                    window: self::window(4),
                    values: values_of("Editor", Role::Window),
                },
                runtime_id: None,
            },
            simple_child: false,
            dialog_text: None,
        }],
        met: None,
        foreground: None,
        value_from_selection: false,
        class: ObjectClass::Plain,
        runtime_id_reused: false,
        caret: FocusCaret::NotRead,
    }
}

/// The listener's read of the button's element, with its route.
fn listener_read() -> ObjectRead {
    ObjectRead {
        object: NodeRead {
            node: None,
            window: window(5),
            values: values("OK", Role::Button),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: Some(Route {
            window: WindowHandle(5),
            steps: vec![RouteStep {
                runtime_id: RuntimeId(vec![42, 7]),
                window: None,
                content: true,
            }],
            end: RouteEnd::ContinueFrom(WindowHandle(5)),
        }),
    }
}

fn values(name: &str, role: Role) -> PropertyValues {
    values_of(name, role)
}

fn values_of(name: &str, role: Role) -> PropertyValues {
    PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::Name,
            value: Some(PropertyValue::Text(name.to_owned())),
        },
        PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(role)),
        },
    ])
}

/// The outpost's node `number`, as the outpost numbers it; Core stamps
/// its generation on every answer.
fn node(number: u64) -> NodeId {
    NodeId::in_outpost(OutpostId(GENERATION), number)
}

/// Window `handle` of the application, inside its top-level window 4.
fn window(handle: u64) -> NodeWindow {
    NodeWindow {
        handle: WindowHandle(handle),
        top_level: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: APP,
        thread: 41,
        desktop: false,
    }
}

fn chunk(unit: TextUnit, text: &str, start: TextAnchor, offset: usize) -> TextChunk {
    TextChunk {
        unit,
        text: text.to_owned(),
        start,
        offset: u32::try_from(offset).expect("a short line"),
        languages: Vec::new(),
        first: true,
        last: true,
        truncated: false,
        formats: Vec::new(),
    }
}

fn facet(handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: APP,
        thread: 41,
    }
}

/// Window `handle`, of `class`, under the foreground window 4.
fn facts(handle: u64, class: &str) -> AcceptanceFacts {
    AcceptanceFacts {
        window: facet(handle, class),
        parent_class: String::new(),
        root: facet(4, "Frame"),
        root_owner: facet(4, "Frame"),
        window_topmost: false,
        root_topmost: false,
        foreground: WindowHandle(4),
        foreground_root_owner: WindowHandle(4),
        window_to_foreground: Relation::Under,
        root_owner_to_foreground: Relation::Equal,
        root_owner_is_foregrounds: true,
        active_window: WindowHandle(4),
        window_to_active: Relation::Under,
        desktop: false,
        hidden_frame: false,
        hidden_frame_owned: false,
    }
}

/// The listener's tell of the button's UI Automation focus in Notepad's
/// window 5.
fn uia_focus(at: Us) -> Event {
    Event {
        trace_id: TraceId::from_raw(9_000),
        kind: EventKind::UiaFocus,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: at,
        object: EventObject::Uia {
            token: ElementToken(9),
            runtime_id: RuntimeId(vec![42, 7]),
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts(5, "AppPane"))),
        checked: Some(Checked {
            probe: Some(Probe {
                window: WindowHandle(5),
                thread: 41,
                provider: true,
                began_us: at,
            }),
            keyboard_focus: Some(Read::Answered(true)),
            kept: Vec::new(),
            previous_sibling: None,
        }),
    }
}

/// The listener's tell of the button's MSAA focus, its own window 6.
fn msaa_focus(at: Us) -> Event {
    Event {
        trace_id: TraceId::from_raw(9_000),
        kind: EventKind::MsaaFocus,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: at,
        object: EventObject::Msaa {
            window: WindowHandle(6),
            object: -4,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts(6, "Button"))),
        checked: None,
    }
}

fn caret_key(key: Key) -> CaretKey {
    let motion = match key {
        Key::Left => CaretMotion::PreviousCharacter,
        Key::Right => CaretMotion::NextCharacter,
        Key::Home => CaretMotion::StartOfLine,
        Key::End => CaretMotion::EndOfLine,
        Key::Up => CaretMotion::PreviousLine,
        Key::Down => CaretMotion::NextLine,
        Key::WordRight => CaretMotion::NextWord,
        Key::Backspace => CaretMotion::Backspace,
        Key::Delete => CaretMotion::Delete,
    };
    CaretKey {
        motion,
        select: false,
    }
}

fn stroke(key: Key, sequence: u32) -> Stroke {
    let (vk, scan_code, extended) = match key {
        Key::Left => (0x25, 0x4B, true),
        Key::Right | Key::WordRight => (0x27, 0x4D, true),
        Key::Home => (0x24, 0x47, true),
        Key::End => (0x23, 0x4F, true),
        Key::Up => (0x26, 0x48, true),
        Key::Down => (0x28, 0x50, true),
        Key::Backspace => (0x08, 0x0E, false),
        Key::Delete => (0x2E, 0x53, true),
    };
    Stroke {
        vk,
        scan_code,
        extended,
        sequence,
    }
}

fn modifiers(key: Key) -> Vec<verbatim_model::stroke::Modifier> {
    match key {
        Key::WordRight => vec![verbatim_model::stroke::Modifier::Control],
        _ => Vec::new(),
    }
}

/// What an utterance is, in the model's words: a character, text, "blank",
/// a button's focus, or anything else by its parts.
fn heard(parts: &[SegmentContent]) -> Heard {
    match parts {
        [SegmentContent::Character(text)] if text.chars().count() == 1 => {
            Heard::Char(text.chars().next().expect("one character"))
        }
        // Spaces around text are not heard.
        [SegmentContent::Text(text)] => Heard::Text(text.trim().to_owned()),
        [SegmentContent::Message(verbatim_model::Message::Blank)] => Heard::Blank,
        [
            SegmentContent::Label(name),
            SegmentContent::Role(Role::Button),
        ] => Heard::Focus(name.clone()),
        other => Heard::Other(format!("{other:?}")),
    }
}
