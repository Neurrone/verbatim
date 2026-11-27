//! A desktop of applications driven through Core's event queue by the
//! public reducer, as the shell drives it: the listener started and ready,
//! each application's outpost started when Core asks for it and made
//! ready at once, a focus told by the listener as UI Automation tells one,
//! its two reads answered with the nodes the test gives, and a change told
//! by the outpost following the committed focus or by the listener. Each
//! step returns every effect it had, as [`Heard`], and the requests it
//! sent, by number, so the test answers them.

#![allow(dead_code, reason = "each test file uses part of the support")]

use crate::readers::{self, Found};
use verbatim_model::batch::{AncestorsEnd, Op, Subject};
use verbatim_model::request::Question;

use verbatim_core::{Runtime, SrState, reduce};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, FocusAncestor, FocusCaret, FocusNode, FocusRead, Instance, KnownAncestor, NodeRead,
    NodeWindow, Outcome, ProcessFact, QueueEffect, QueueInput, Reader, Reply, RequestNumber, Route,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Event, EventObject, Payload, Read, Relation,
    TellWindow, WindowFacet,
};
use verbatim_model::{
    Effect, Input, NodeId, NodeSnapshot, ObjectKey, OutpostId, Pid, SegmentContent, SpeechPriority,
    TraceId, UtteranceSegment, WindowHandle,
};

/// The listener's instance.
pub const LISTENER: Instance = Instance {
    reader: Reader::Listener,
    generation: 1,
};

/// An application: its process and its outpost's generation, which names
/// its nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    pub pid: Pid,
    pub generation: u64,
    /// Its top-level window's handle; its controls are in the window after
    /// it.
    pub top: u64,
}

impl App {
    /// Node `number` of this application.
    pub fn node(self, number: u64) -> NodeId {
        NodeId::in_outpost(OutpostId(self.generation), number)
    }

    /// Its outpost's instance.
    pub fn instance(self) -> Instance {
        Instance {
            reader: Reader::Outpost(self.pid),
            generation: self.generation,
        }
    }

    /// The application with its focus in top-level window `top`.
    pub const fn in_window(self, top: u64) -> Self {
        Self { top, ..self }
    }

    /// The application's next outpost generation, as its replacement.
    pub const fn replaced(self) -> Self {
        Self {
            generation: self.generation + 1,
            ..self
        }
    }

    /// The application's top-level window.
    pub fn top_level(self) -> WindowHandle {
        WindowHandle(self.top)
    }

    /// The window its controls are in, under its top-level window.
    pub fn window(self) -> WindowHandle {
        WindowHandle(self.top + 1)
    }

    /// Its controls' window, as a node's read names it.
    pub fn node_window(self) -> NodeWindow {
        NodeWindow {
            handle: self.window(),
            top_level: self.top_level(),
            root_owner: self.top_level(),
            process: Some(self.pid),
            thread: 41,
            desktop: false,
        }
    }

    /// Window `handle` of the application, of `class`.
    pub fn facet(self, handle: WindowHandle, class: &str) -> WindowFacet {
        WindowFacet {
            handle,
            class: class.to_owned(),
            process: Some(self.pid),
            thread: 41,
        }
    }

    /// Acceptance facts of its window, its top-level window in front.
    pub fn facts(self) -> AcceptanceFacts {
        AcceptanceFacts {
            window: self.facet(self.window(), "AppPane"),
            parent_class: String::new(),
            root: self.facet(self.top_level(), "Frame"),
            root_owner: self.facet(self.top_level(), "Frame"),
            window_topmost: false,
            root_topmost: false,
            foreground: self.top_level(),
            foreground_root_owner: self.top_level(),
            window_to_foreground: Relation::Under,
            root_owner_to_foreground: Relation::Equal,
            root_owner_is_foregrounds: true,
            active_window: self.top_level(),
            window_to_active: Relation::Under,
            desktop: false,
            hidden_frame: false,
            hidden_frame_owned: false,
        }
    }
}

/// The application of most tests.
pub const ONE: App = App {
    pid: Pid(40),
    generation: 3,
    top: 4_000,
};

/// A second application.
pub const TWO: App = App {
    pid: Pid(50),
    generation: 5,
    top: 5_000,
};

/// One effect as these tests compare it: every effect becomes exactly one
/// of these, so comparing the whole list checks every effect a step had.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// Expired speech dropped, with where the focus now is, each key by
    /// its node alone.
    Expire(verbatim_model::FocusNow),
    /// Speech stopped.
    Stop,
    /// An utterance: its priority and segments, its index marks left out.
    Say(SpeechPriority, Vec<UtteranceSegment>),
    /// An earcon played.
    Earcon(verbatim_model::Earcon),
    /// A request sent to a reader, by its kind.
    Sent(Reader, &'static str),
    /// Text copied to the clipboard.
    Copied(String),
}

/// A queued utterance of `segments`.
pub fn queued(segments: Vec<UtteranceSegment>) -> Heard {
    Heard::Say(SpeechPriority::Queued, segments)
}

/// An utterance that cuts off speech, of `segments`.
pub fn interrupt(segments: Vec<UtteranceSegment>) -> Heard {
    Heard::Say(SpeechPriority::Interrupt, segments)
}

/// The `Follow` a commit sends to `app`'s outpost.
pub fn follow(app: App) -> Heard {
    Heard::Sent(Reader::Outpost(app.pid), "Follow")
}

/// Where the focus is: node `focus` of `app`, with its `ancestors`
/// (outermost first), and no foreground object yet: the first focus on a
/// desk, whose outermost ancestor stands for the foreground until the
/// read's foreground becomes the foreground object, after it is spoken.
pub fn focus_now(app: App, focus: u64, ancestors: &[u64]) -> verbatim_model::FocusNow {
    focus_now_with(
        app,
        focus,
        ancestors,
        ancestors.first().map(|outermost| (app, *outermost)),
    )
}

/// [`focus_now`] once a focus of `app` was committed: its top-level window
/// ([`FRAME`]) is the foreground object.
pub fn focus_now_in(app: App, focus: u64, ancestors: &[u64]) -> verbatim_model::FocusNow {
    focus_now_with(app, focus, ancestors, Some((app, FRAME)))
}

/// [`focus_now`] with the foreground object named, a node of the
/// application named.
pub fn focus_now_with(
    app: App,
    focus: u64,
    ancestors: &[u64],
    foreground: Option<(App, u64)>,
) -> verbatim_model::FocusNow {
    let key = |app: App, node| ObjectKey::of_id(app.node(node));
    verbatim_model::FocusNow {
        focus: key(app, focus),
        ancestors: ancestors.iter().map(|&node| key(app, node)).collect(),
        foreground: foreground.map(|(app, node)| key(app, node)),
        menu_item_of_focus: None,
        replaced: Vec::new(),
    }
}

/// The effects of a focus committed at `now` in `app`, announced by the
/// utterances `said`, each queued, with no speech cut off.
pub fn focus_heard(
    app: App,
    now: verbatim_model::FocusNow,
    said: Vec<Vec<UtteranceSegment>>,
) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now)];
    expected.extend(said.into_iter().map(queued));
    expected.push(follow(app));
    expected
}

/// A request's kind, as [`Heard::Sent`] names it: a batch by what it
/// reads, a candidate's read, a read of the current focus, the listener's
/// read of a kept element, the outpost's read of a route.
pub fn kind(request: &Question) -> &'static str {
    let Question::Batch(batch) = request;
    // A caret key's or a caret tell's batch: its first step a text
    // operation.
    if matches!(
        batch.steps.first().map(|step| &step.op),
        Some(Op::Caret { .. } | Op::WaitForCaret { .. } | Op::MoveBySentence { .. })
    ) {
        return "Text";
    }
    let names = |subject: fn(&Subject) -> bool| {
        batch
            .steps
            .iter()
            .filter_map(|step| step.op.subject())
            .any(subject)
    };
    if let [
        verbatim_model::batch::Step {
            op:
                Op::Read {
                    subject: Subject::Node(_),
                    ..
                },
            ..
        },
    ] = batch.steps.as_slice()
    {
        return "Properties";
    }
    if batch.reads_focus_first() {
        "CurrentFocus"
    } else if batch.steps.iter().any(|step| {
        matches!(
            step.op,
            Op::IsOneOf { .. }
                | Op::Descendants {
                    subject: Subject::Msaa { .. },
                    ..
                }
        )
    }) {
        "ReadObject"
    } else if let [
        verbatim_model::batch::Step {
            op:
                Op::Read {
                    subject: Subject::Msaa { .. },
                    set,
                },
            ..
        },
    ] = batch.steps.as_slice()
        && !(set.is_empty() || set.as_slice() == [verbatim_model::property::PropertyId::Role])
    {
        // A show's or the desktop name's read of its MSAA object, where a
        // menu start's reads its role and a foreground's nothing.
        "ReadObject"
    } else if names(|subject| matches!(subject, Subject::Msaa { .. })) {
        "Candidate"
    } else if names(|subject| matches!(subject, Subject::Kept { .. })) {
        "ReadObject"
    } else if names(|subject| matches!(subject, Subject::Route(_))) {
        "Completion"
    } else if batch
        .steps
        .iter()
        .all(|step| matches!(step.op, Op::Follow(_)))
    {
        "Follow"
    } else {
        "other"
    }
}

/// One effect as [`Heard`], `None` for the event queue's own business and
/// the control plane's stream.
fn heard_of(effect: Effect) -> Option<Heard> {
    match effect {
        Effect::DropExpiredSpeech(now) => {
            let node = |key: &ObjectKey| ObjectKey::of_id(key.id);
            Some(Heard::Expire(verbatim_model::FocusNow {
                focus: node(&now.focus),
                ancestors: now.ancestors.iter().map(node).collect(),
                foreground: now.foreground.as_ref().map(node),
                menu_item_of_focus: now.menu_item_of_focus,
                replaced: now.replaced.iter().map(node).collect(),
            }))
        }
        Effect::StopSpeech => Some(Heard::Stop),
        Effect::Speak(utterance, _) => Some(Heard::Say(
            utterance.priority,
            utterance
                .segments
                .into_iter()
                .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                .collect(),
        )),
        Effect::PlayEarcon(earcon) => Some(Heard::Earcon(earcon)),
        Effect::CopyToClipboard(text) => Some(Heard::Copied(text)),
        Effect::ForKey(_, effect) => heard_of(*effect),
        Effect::Queue(QueueEffect::Send { to, ask }) => {
            Some(Heard::Sent(to.reader, kind(&ask.request)))
        }
        Effect::Observed(_) | Effect::Queue(_) => None,
        other => panic!("an effect these tests do not expect: {other:?}"),
    }
}

/// The values a node's read carries of `snapshot`: every property a
/// snapshot holds.
pub fn values_of(snapshot: &NodeSnapshot) -> PropertyValues {
    let mut values = Vec::new();
    let mut push = |id, value: Option<PropertyValue>| {
        if let Some(value) = value {
            values.push(PropertyAnswer {
                id,
                value: Some(value),
            });
        }
    };
    let details = &snapshot.details;
    push(
        PropertyId::Name,
        snapshot.name.clone().map(PropertyValue::Text),
    );
    push(PropertyId::Role, Some(PropertyValue::Role(snapshot.role)));
    push(
        PropertyId::Value,
        snapshot.value.clone().map(PropertyValue::Text),
    );
    push(
        PropertyId::States,
        Some(PropertyValue::States(snapshot.states)),
    );
    push(
        PropertyId::Description,
        details.description.clone().map(PropertyValue::Text),
    );
    push(
        PropertyId::KeyboardShortcut,
        details.keyboard_shortcut.clone().map(PropertyValue::Text),
    );
    push(
        PropertyId::PositionInSet,
        details.position_in_set.map(PropertyValue::Count),
    );
    push(
        PropertyId::SetSize,
        details.set_size.map(PropertyValue::Count),
    );
    push(PropertyId::Level, details.level.map(PropertyValue::Count));
    push(PropertyId::Rect, details.rect.map(PropertyValue::Rect));
    push(
        PropertyId::ClassName,
        details.class_name.clone().map(PropertyValue::Text),
    );
    push(PropertyId::Cell, details.cell.map(PropertyValue::Cell));
    push(
        PropertyId::TableCounts,
        details.table_counts.map(PropertyValue::TableCounts),
    );
    push(
        PropertyId::NativeWindow,
        details.window.map(PropertyValue::Window),
    );
    PropertyValues(values)
}

/// The runtime id of node `number`.
pub fn runtime_of(number: u64) -> RuntimeId {
    RuntimeId(vec![
        42,
        i32::try_from(number).expect("a small node number"),
    ])
}

/// `snapshot` as a focus read's node in `app`: a runtime id for a UI
/// Automation node, none for an MSAA one, as the outpost answers.
pub fn focus_node(app: App, snapshot: &NodeSnapshot) -> FocusNode {
    FocusNode {
        read: NodeRead {
            node: Some(snapshot.id),
            window: app.node_window(),
            values: values_of(snapshot),
        },
        runtime_id: (snapshot.backend == verbatim_model::Backend::Uia)
            .then(|| runtime_of(snapshot.id.number())),
    }
}

/// The outpost's focus read of `focus` in `app`, under `ancestors`,
/// outermost first, its caret not read.
pub fn focus_read_of(app: App, focus: &NodeSnapshot, ancestors: &[NodeSnapshot]) -> FocusRead {
    FocusRead {
        focus: focus_node(app, focus),
        msaa: None,
        ancestors: ancestors
            .iter()
            .rev()
            .map(|ancestor| FocusAncestor {
                node: focus_node(app, ancestor),
                simple_child: false,
                uia_layout: ancestor.details.uia_layout,
                described: ancestor.details.described,
            })
            .collect(),
        met: None,
        foreground: Some(frame_of(app)),
        value_from_selection: false,
        runtime_id_reused: false,
        caret: FocusCaret::NoText,
    }
}

/// The node of each application's top-level window, which its outpost
/// reads as a focus's foreground object.
pub const FRAME: u64 = 9_000;

/// `app`'s top-level window, node [`FRAME`], as a focus read's foreground.
pub fn frame_of(app: App) -> FocusNode {
    FocusNode {
        read: NodeRead {
            node: Some(app.node(FRAME)),
            window: NodeWindow {
                handle: app.top_level(),
                ..app.node_window()
            },
            values: PropertyValues(vec![
                PropertyAnswer {
                    id: PropertyId::Role,
                    value: Some(PropertyValue::Role(verbatim_model::Role::Window)),
                },
                PropertyAnswer {
                    id: PropertyId::NativeWindow,
                    value: Some(PropertyValue::Window(app.top_level())),
                },
            ]),
        },
        runtime_id: Some(runtime_of(FRAME)),
    }
}

/// The listener's UI Automation focus tell of node `number` in `app`.
pub fn focus_tell(app: App, number: u64, at_us: u64) -> Event {
    Event {
        trace_id: TraceId::from_raw(7),
        kind: EventKind::UiaFocus,
        process: app.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us,
        object: EventObject::Uia {
            token: ElementToken(number),
            runtime_id: runtime_of(number),
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(app.facts())),
        checked: Some(Checked {
            probe: Some(verbatim_model::request::Probe {
                window: app.window(),
                thread: 41,
                provider: true,
                began_us: at_us,
            }),
            keyboard_focus: Some(Read::Answered(true)),
            kept: Vec::new(),
            previous_sibling: None,
        }),
    }
}

/// The listener's read of the focus element `number` in `app`: the element
/// alone, its walk going on from the application's window.
pub fn listener_read(app: App, number: u64) -> Found {
    Found::Walked(
        NodeRead {
            node: None,
            window: app.node_window(),
            values: PropertyValues(vec![
                PropertyAnswer {
                    id: PropertyId::RuntimeId,
                    value: Some(PropertyValue::RuntimeId(runtime_of(number))),
                },
                PropertyAnswer {
                    id: PropertyId::IsContent,
                    value: Some(PropertyValue::Flag(true)),
                },
            ]),
        },
        Vec::new(),
        AncestorsEnd::ContinueFrom(app.window()),
    )
}

/// [`listener_read`] of `number` in `app`, its walk meeting `met`, by its
/// place among the stops of the walk `asked`, the listener's batch.
pub fn listener_read_meeting(
    desk: &Desk,
    asked: u64,
    (app, number): (App, u64),
    met: &KnownAncestor,
) -> Found {
    let place = desk
        .asked
        .iter()
        .find_map(|(each, _, question)| match question {
            Question::Batch(batch) if *each == asked => {
                batch.steps.iter().find_map(|step| match &step.op {
                    Op::Ancestors { until, .. } => until.iter().position(|known| known == met),
                    _ => None,
                })
            }
            Question::Batch(_) => None,
        })
        .unwrap_or_else(|| panic!("{met:?} among the stops of walk {asked}"));
    let Found::Walked(element, ancestors, _) = listener_read(app, number) else {
        unreachable!("the listener's read is a walk");
    };
    let place = u32::try_from(place).expect("a short list");
    Found::Walked(element, ancestors, AncestorsEnd::Met(place))
}

/// The route and the ancestors Core knows of the outpost's read of a route
/// that `question` asks, if it asks one.
pub fn completion_of(question: &Question) -> Option<(Route, Vec<KnownAncestor>)> {
    let Question::Batch(batch) = question;
    batch.steps.iter().find_map(|step| match &step.op {
        Op::FocusRead {
            subject: Subject::Route(route),
            known,
            ..
        } => Some(((**route).clone(), known.clone())),
        _ => None,
    })
}

/// A desktop whose listener is ready, and the outposts Core asked for,
/// each started and ready as soon as it is asked for.
pub struct Desk {
    pub state: SrState,
    pub runtime: Runtime,
    /// The queue's time, in microseconds, moved on by each step.
    pub at_us: u64,
    /// The generation each application's outpost starts as, by process.
    apps: Vec<App>,
    /// The last key's number.
    pub keys: u64,
    /// The requests the last step sent, by number, instance and request.
    pub sent: Vec<(u64, Instance, Question)>,
    /// The wake Core last asked for.
    pub wake: Option<u64>,
    /// Every input fed, with how many effects it had, as the flight
    /// recorder keeps them: a replay fixture's inputs.
    pub recorded: Vec<verbatim_core::RecordedInput>,
    /// Every request sent since the desk was made, in order.
    pub asked: Vec<(u64, Instance, Question)>,
    /// Every request Core cancelled, by number.
    pub cancelled: Vec<u64>,
}

impl Default for Desk {
    fn default() -> Self {
        Self::new()
    }
}

impl Desk {
    /// The listener ready, nothing focused; [`ONE`] and [`TWO`] start as
    /// their generations when Core asks for their outposts.
    pub fn new() -> Self {
        let mut desk = Self {
            state: SrState::new(),
            runtime: Runtime::default(),
            at_us: 1_000,
            apps: vec![ONE, TWO],
            keys: 0,
            sent: Vec::new(),
            wake: None,
            recorded: Vec::new(),
            asked: Vec::new(),
            cancelled: Vec::new(),
        };
        let _ = desk.step(QueueInput::Process(ProcessFact::ListenerStarted(
            LISTENER.generation,
        )));
        let _ = desk.step(QueueInput::ReaderReady(LISTENER));
        desk
    }

    /// `app`'s outpost, when Core next asks for one for its process,
    /// starts as `app`'s generation.
    pub fn app(&mut self, app: App) {
        self.apps.retain(|known| known.pid != app.pid);
        self.apps.push(app);
    }

    /// The application whose outpost serves `pid`.
    fn app_of(&self, pid: Pid) -> App {
        *self
            .apps
            .iter()
            .find(|app| app.pid == pid)
            .unwrap_or_else(|| panic!("no application for {pid:?}"))
    }

    /// Feeds `input` at the queue's time, its effects as they came.
    pub fn feed(&mut self, input: QueueInput) -> Vec<Effect> {
        let input = Input::Queue {
            trace_id: TraceId::from_raw(9),
            at_us: self.at_us,
            input,
        };
        let effects = reduce(&mut self.state, &mut self.runtime, &input);
        self.recorded.push(verbatim_core::RecordedInput {
            input,
            effect_count: effects.len(),
        });
        effects
    }

    /// One step, as [`Desk::step`], with `input` fed at `at_us`.
    pub fn step_at(&mut self, at_us: u64, input: QueueInput) -> Vec<Heard> {
        self.at_us = at_us.saturating_sub(1_000);
        self.step(input)
    }

    /// One step: `input` fed at the next moment, an outpost Core asks for
    /// started and made ready at once, as the owner thread starts one;
    /// every effect heard and the requests sent.
    pub fn step(&mut self, input: QueueInput) -> Vec<Heard> {
        self.at_us += 1_000;
        let mut pending = vec![input];
        let mut heard = Vec::new();
        self.sent.clear();
        while !pending.is_empty() {
            let input = pending.remove(0);
            for effect in self.feed(input) {
                match &effect {
                    Effect::Queue(QueueEffect::StartOutpost(pid)) => {
                        let app = self.app_of(*pid);
                        pending.push(QueueInput::Process(ProcessFact::OutpostStarted {
                            process: *pid,
                            generation: app.generation,
                        }));
                        pending.push(QueueInput::ReaderReady(app.instance()));
                    }
                    Effect::Queue(QueueEffect::Send { to, ask }) => {
                        self.sent.push((ask.number.0, *to, ask.request.clone()));
                        self.asked.push((ask.number.0, *to, ask.request.clone()));
                    }
                    Effect::Queue(QueueEffect::WakeAt(wake)) => self.wake = *wake,
                    Effect::Queue(QueueEffect::Cancel { cancel, .. }) => {
                        self.cancelled.push(cancel.number.0);
                    }
                    _ => {}
                }
                heard.extend(heard_of(effect));
            }
        }
        heard
    }

    /// A tell from `from`.
    pub fn tell(&mut self, from: Instance, tell: Event) -> Vec<Heard> {
        self.step(QueueInput::Tell {
            from,
            tell: Box::new(tell),
        })
    }

    /// Answers request `number`, sent to `from`, with `outcome`.
    pub fn answer(&mut self, from: Instance, number: u64, outcome: Outcome) -> Vec<Heard> {
        self.step(QueueInput::Answer {
            from,
            answer: Answer {
                number: RequestNumber(number),
                outcome,
                probes: Vec::new(),
                detail: String::new(),
                began_us: 0,
                ended_us: 0,
            },
        })
    }

    /// Answers the one request the last step sent with `outcome`, from
    /// the instance it went to: every effect of the answer.
    pub fn reply_outcome(&mut self, outcome: Outcome) -> Vec<Heard> {
        let [(number, to, _)] = self.sent[..] else {
            panic!("one request out: {:?}", self.sent);
        };
        self.answer(to, number, outcome)
    }

    /// Answers the one request the last step sent with `reply`.
    pub fn reply(&mut self, reply: Reply) -> Vec<Heard> {
        self.reply_outcome(Outcome::Result(reply))
    }

    /// Answers the one request out, a batch, as its reader does having
    /// found `found`.
    pub fn reply_object(&mut self, read: NodeRead) -> Vec<Heard> {
        self.reply_found(&Found::Object(read))
    }

    /// Answers request `number`, a batch sent to `from`, as its reader does
    /// having found `found`.
    pub fn found(&mut self, from: Instance, number: u64, found: &Found) -> Vec<Heard> {
        let batch = self
            .asked
            .iter()
            .find_map(|(asked, _, question)| match question {
                Question::Batch(batch) if *asked == number => Some(batch.clone()),
                Question::Batch(_) => None,
            })
            .unwrap_or_else(|| panic!("request {number} is a batch asked"));
        self.answer(from, number, readers::outcome(&batch, found))
    }

    /// Answers the one batch the last step sent as its reader does, having
    /// found `found`.
    pub fn reply_found(&mut self, found: &Found) -> Vec<Heard> {
        let [(number, to, _)] = self.sent[..] else {
            panic!("one request out: {:?}", self.sent);
        };
        self.found(to, number, found)
    }

    /// The queue's wake fed, at the time Core asked for, with no check
    /// after a menu, popup or switcher end waiting, whose wake carries the
    /// window in front ([`Desk::due_over`]).
    pub fn due(&mut self) -> Vec<Heard> {
        assert!(
            !self.state.menu_check_waits(),
            "a check waits: its wake carries the window in front"
        );
        let wake = self.wake.expect("a wake asked for");
        self.at_us = self.at_us.max(wake) - 1_000;
        self.step(QueueInput::Due { in_front: None })
    }

    /// The queue's wake fed, at the time Core asked for, as the shell makes
    /// it while a check after a menu, popup or switcher end waits: with
    /// `app`'s top-level window the one Windows names in front.
    pub fn due_over(&mut self, app: App) -> Vec<Heard> {
        let wake = self.wake.expect("a wake asked for");
        self.at_us = self.at_us.max(wake) - 1_000;
        self.step(QueueInput::Due {
            in_front: Some(verbatim_model::request::LiveFront::Window(
                verbatim_model::request::FrontWindow {
                    window: app.top_level(),
                    root_owner: app.top_level(),
                    process: app.pid,
                    thread: 41,
                    class: "Frame".to_owned(),
                },
            )),
        })
    }

    /// The focus on `focus` in `app`, under `ancestors`, outermost first,
    /// as the listener tells it and the outpost reads it: every effect of
    /// the read's answer.
    pub fn focus(
        &mut self,
        app: App,
        focus: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
    ) -> Vec<Heard> {
        self.focus_read(app, focus_read_of(app, focus, ancestors))
    }

    /// The focus the outpost reads as `read`, told by the listener.
    pub fn focus_read(&mut self, app: App, read: FocusRead) -> Vec<Heard> {
        self.app(app);
        let number = read.focus.read.node.expect("a focus node").number();
        let tell = focus_tell(app, number, self.at_us + 1_000);
        let _ = self.tell(LISTENER, tell);
        let [(_, LISTENER, Question::Batch(_))] = &self.sent[..] else {
            panic!("one listener read asked: {:?}", self.sent);
        };
        let _ = self.reply_found(&listener_read(app, number));
        let [(_, _, Question::Batch(_))] = &self.sent[..] else {
            panic!("one completion asked: {:?}", self.sent);
        };
        self.reply_found(&Found::Read(read))
    }

    /// A UI Automation property change of `property` to `value` on the
    /// committed focus of `app`, node `number`, told by its outpost, which
    /// follows it.
    pub fn followed_change(
        &mut self,
        app: App,
        number: u64,
        property: PropertyId,
        value: Option<PropertyValue>,
    ) -> Vec<Heard> {
        let tell = Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::UiaPropertyChanged,
            process: app.pid,
            application: "app".to_owned(),
            thread: 41,
            at_us: self.at_us + 1_000,
            object: EventObject::Followed {
                runtime_id: runtime_of(number),
            },
            cached: PropertyValues::default(),
            payload: Payload::PropertyChanged { property, value },
            window: TellWindow::NoWindow,
            checked: None,
        };
        self.tell(app.instance(), tell)
    }

    /// A UI Automation property change of `property` to `value` on node
    /// `number` of `app`, told by the listener, which kept the element.
    pub fn uia_change(
        &mut self,
        app: App,
        number: u64,
        property: PropertyId,
        value: Option<PropertyValue>,
    ) -> Vec<Heard> {
        let tell = Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::UiaPropertyChanged,
            payload: Payload::PropertyChanged { property, value },
            checked: None,
            ..focus_tell(app, number, self.at_us + 1_000)
        };
        self.tell(LISTENER, tell)
    }

    /// A repeat of the reader's last focus, node `number` of `app`, which
    /// the check thread found still kept and focused: a duplicate, which
    /// ends with no read. Every effect of it.
    pub fn duplicate(&mut self, app: App, number: u64) -> Vec<Heard> {
        let mut tell = focus_tell(app, number, self.at_us + 1_000);
        if let Some(checked) = tell.checked.as_mut() {
            checked.kept = vec![(ElementToken(number), Read::Answered(true))];
        }
        self.tell(LISTENER, tell)
    }

    /// A key press doing `action`: every effect of it.
    pub fn press(&mut self, action: verbatim_model::request::PressAction) -> Vec<Heard> {
        self.keys += 1;
        self.step(QueueInput::Press(Box::new(
            verbatim_model::request::Press {
                key_seq: verbatim_model::KeySeq::new(self.keys),
                pressed_at_us: self.at_us + 1_000,
                action,
            },
        )))
    }

    /// Review command `command` pressed, `repeat` extra times.
    pub fn command(&mut self, command: verbatim_model::ReviewCommand, repeat: u8) -> Vec<Heard> {
        self.press(verbatim_model::request::PressAction::Command { command, repeat })
    }

    /// Playback reached index mark `mark`.
    pub fn mark(&mut self, mark: verbatim_model::SpeechMark) -> Vec<Heard> {
        self.step(QueueInput::MarkReached(mark))
    }
}
