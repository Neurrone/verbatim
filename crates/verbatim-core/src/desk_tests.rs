//! A desktop of applications driven through Core's event queue, for the
//! tests of what Core says of focuses and changes: each application's
//! outpost made ready, a focus told by the listener as UI Automation
//! tells one, its two reads answered with the nodes the test gives, and a
//! change told by the outpost following the committed focus. Each step
//! returns every effect it had, as [`Heard`].

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    FocusAncestor, FocusCaret, FocusNode, FocusRead, NodeRead, NodeWindow, ObjectRead, Reply,
    Request as Wire, Route, RouteEnd, RouteStep, WindowVerdict,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Event, EventObject, Payload, Read, Relation,
    TellWindow, WindowFacet,
};
use verbatim_model::{
    Effect as Said, NodeId, NodeSnapshot, ObjectKey, OutpostId, Pid, SegmentContent,
    SpeechPriority, TraceId, UtteranceSegment, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{Held, LISTENER, answer};
use crate::event_queue::{Input, Instance, Reader, Timed};
use verbatim_model::Effect;
use verbatim_model::request::QueueEffect;

/// An application: its process and its outpost's generation, which names
/// its nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct App {
    pub(crate) pid: Pid,
    pub(crate) generation: u64,
    /// Its top-level window's handle; its controls are in the window
    /// after it.
    pub(crate) top: u64,
}

impl App {
    /// Node `number` of this application.
    pub(crate) fn node(self, number: u64) -> NodeId {
        NodeId::in_outpost(OutpostId(self.generation), number)
    }

    pub(crate) fn instance(self) -> Instance {
        Instance {
            reader: Reader::Outpost(self.pid),
            generation: self.generation,
        }
    }

    /// The application with its focus in top-level window `top`.
    pub(crate) const fn in_window(self, top: u64) -> Self {
        Self { top, ..self }
    }

    /// The application's top-level window.
    pub(crate) fn top_level(self) -> WindowHandle {
        WindowHandle(self.top)
    }

    /// The window its controls are in, under its top-level window.
    pub(crate) fn window(self) -> WindowHandle {
        WindowHandle(self.top + 1)
    }

    fn node_window(self) -> NodeWindow {
        NodeWindow {
            handle: self.window(),
            top_level: self.top_level(),
            root_owner: self.top_level(),
            process: self.pid,
            thread: 41,
            desktop: false,
        }
    }

    fn facet(self, handle: WindowHandle, class: &str) -> WindowFacet {
        WindowFacet {
            handle,
            class: class.to_owned(),
            process: self.pid,
            thread: 41,
        }
    }

    /// Acceptance facts of its window, its top-level window in front.
    fn facts(self) -> AcceptanceFacts {
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
pub(crate) const ONE: App = App {
    pid: crate::entries::tests::APP,
    generation: crate::entries::tests::GENERATION,
    top: 4_000,
};

/// A second application.
pub(crate) const TWO: App = App {
    pid: Pid(50),
    generation: 5,
    top: 5_000,
};

/// One effect as these tests compare it: every effect becomes exactly one
/// of these, so comparing the whole list checks every effect a step had.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Heard {
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
pub(crate) fn queued(segments: Vec<UtteranceSegment>) -> Heard {
    Heard::Say(SpeechPriority::Queued, segments)
}

/// The `Follow` a commit sends to `app`'s outpost.
pub(crate) fn follow(app: App) -> Heard {
    Heard::Sent(Reader::Outpost(app.pid), "Follow")
}

/// Where the focus is: node `focus` of `app`, with its `ancestors`
/// (outermost first), and no foreground object yet: the first focus on a
/// desk, whose outermost ancestor stands for the foreground until the
/// read's foreground becomes the foreground object, after it is spoken.
pub(crate) fn focus_now(app: App, focus: u64, ancestors: &[u64]) -> verbatim_model::FocusNow {
    focus_now_with(
        app,
        focus,
        ancestors,
        ancestors.first().map(|outermost| (app, *outermost)),
    )
}

/// [`focus_now`] once a focus of `app` was committed: its top-level window
/// ([`FRAME`]) is the foreground object.
pub(crate) fn focus_now_in(app: App, focus: u64, ancestors: &[u64]) -> verbatim_model::FocusNow {
    focus_now_with(app, focus, ancestors, Some((app, FRAME)))
}

/// [`focus_now`] with the foreground object named, a node of the
/// application named.
pub(crate) fn focus_now_with(
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
    }
}

/// The effects of a focus committed at `now` in `app`, announced by the
/// utterances `said`, each queued, with no speech cut off.
pub(crate) fn focus_heard(
    app: App,
    now: verbatim_model::FocusNow,
    said: Vec<Vec<UtteranceSegment>>,
) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now)];
    expected.extend(said.into_iter().map(queued));
    expected.push(follow(app));
    expected
}

fn kind(request: &Wire) -> &'static str {
    match request {
        Wire::ReadObject(_) => "ReadObject",
        Wire::CompleteUia(_) => "CompleteUia",
        Wire::MsaaRun(_) => "MsaaRun",
        Wire::CurrentFocus { .. } => "CurrentFocus",
        Wire::Follow(_) => "Follow",
        Wire::Properties { .. } => "Properties",
        Wire::Text { .. } => "Text",
        _ => "other",
    }
}

/// The requests a step sent, by number, reader and request.
type Sent = Vec<(u64, Reader, Wire)>;

/// `effects` as [`Heard`], with the requests sent, by number, and the
/// verdict each carried, by number.
fn heard_of(effects: Vec<Effect>) -> (Vec<Heard>, Sent, Vec<(u64, WindowVerdict)>) {
    let mut heard = Vec::new();
    let mut sent = Vec::new();
    let mut verdicts = Vec::new();
    for effect in effects {
        match effect {
            Effect::Queue(QueueEffect::Send { to, ask }) => {
                heard.push(Heard::Sent(to.reader, kind(&ask.request)));
                verdicts.push((ask.number.0, ask.verdict));
                sent.push((ask.number.0, to.reader, ask.request));
            }
            said => heard.extend(said_heard(said)),
        }
    }
    (heard, sent, verdicts)
}

fn said_heard(said: Said) -> Option<Heard> {
    match said {
        Said::DropExpiredSpeech(now) => {
            let node = |key: &ObjectKey| ObjectKey::of_id(key.id);
            Some(Heard::Expire(verbatim_model::FocusNow {
                focus: node(&now.focus),
                ancestors: now.ancestors.iter().map(node).collect(),
                foreground: now.foreground.as_ref().map(node),
                menu_item_of_focus: now.menu_item_of_focus,
            }))
        }
        Said::StopSpeech => Some(Heard::Stop),
        Said::Speak(utterance, _) => Some(Heard::Say(
            utterance.priority,
            utterance
                .segments
                .into_iter()
                .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                .collect(),
        )),
        Said::PlayEarcon(earcon) => Some(Heard::Earcon(earcon)),
        Said::CopyToClipboard(text) => Some(Heard::Copied(text)),
        Said::ForKey(_, said) => said_heard(*said),
        // The control plane's stream of the events Core takes.
        Said::Observed(_) => None,
        other => panic!("an effect these tests do not expect: {other:?}"),
    }
}

/// The values a node's read carries of `snapshot`: every property a
/// snapshot holds.
pub(crate) fn values_of(snapshot: &NodeSnapshot) -> PropertyValues {
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
fn runtime_of(number: u64) -> RuntimeId {
    RuntimeId(vec![
        42,
        i32::try_from(number).expect("a small node number"),
    ])
}

fn focus_node(app: App, snapshot: &NodeSnapshot) -> FocusNode {
    FocusNode {
        read: NodeRead {
            node: Some(snapshot.id),
            window: app.node_window(),
            values: values_of(snapshot),
        },
        runtime_id: Some(runtime_of(snapshot.id.number())),
    }
}

/// The outpost's focus read of `focus` in `app`, under `ancestors`,
/// outermost first, its caret not read.
pub(crate) fn focus_read_of(
    app: App,
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
) -> FocusRead {
    FocusRead {
        focus: focus_node(app, focus),
        msaa: None,
        ancestors: ancestors
            .iter()
            .rev()
            .map(|ancestor| FocusAncestor {
                node: focus_node(app, ancestor),
                simple_child: false,
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
pub(crate) const FRAME: u64 = 9_000;

/// `app`'s top-level window, node [`FRAME`], as a focus read's foreground.
fn frame_of(app: App) -> FocusNode {
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
pub(crate) fn focus_tell(app: App, number: u64, at_us: u64) -> Event {
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

/// The listener's read of the focus element in `app`: its route to the
/// application's window.
pub(crate) fn listener_read(app: App, number: u64) -> ObjectRead {
    ObjectRead {
        object: NodeRead {
            node: None,
            window: app.node_window(),
            values: PropertyValues::default(),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: Some(Route {
            window: app.window(),
            steps: vec![RouteStep {
                runtime_id: runtime_of(number),
                window: None,
                content: true,
            }],
            end: RouteEnd::ContinueFrom(app.window()),
        }),
    }
}

/// A desktop whose listener is ready, and the outposts of the applications
/// named so far.
pub(crate) struct Desk {
    pub(crate) held: Held,
    /// The queue's time, in microseconds, moved on by each step.
    pub(crate) at_us: u64,
    ready: Vec<App>,
    /// The last key's number.
    keys: u64,
    /// The requests the last step sent, by number, reader and request.
    pub(crate) sent: Sent,
    /// The verdict each request the last step sent carried, by number.
    pub(crate) verdicts: Vec<(u64, WindowVerdict)>,
}

impl Desk {
    /// The listener and [`ONE`]'s outpost ready, nothing focused.
    pub(crate) fn new() -> Self {
        Self {
            held: crate::entries::tests::ready(),
            at_us: 1_000,
            ready: vec![ONE],
            keys: 0,
            sent: Vec::new(),
            verdicts: Vec::new(),
        }
    }

    /// One step: `input` fed at the next moment, its effects heard and its
    /// requests sent.
    pub(crate) fn step(&mut self, input: Input<Entry>) -> (Vec<Heard>, Sent) {
        self.at_us += 1_000;
        let effects = crate::entries::feed(
            &mut self.held.state,
            &mut self.held.runtime,
            Timed {
                at_us: self.at_us,
                input,
            },
        )
        .unwrap();
        let (heard, sent, verdicts) = heard_of(effects);
        self.sent.clone_from(&sent);
        self.verdicts = verdicts;
        (heard, sent)
    }

    /// Answers the one request the last step sent with `outcome`, from
    /// the reader it went to: every effect of the answer.
    pub(crate) fn reply_outcome(
        &mut self,
        outcome: verbatim_model::request::Outcome,
    ) -> Vec<Heard> {
        let [(number, reader, _)] = self.sent[..] else {
            panic!("one request out: {:?}", self.sent);
        };
        let from = match reader {
            Reader::Listener => LISTENER,
            Reader::Outpost(pid) => self
                .ready
                .iter()
                .find(|app| app.pid == pid)
                .expect("a ready outpost")
                .instance(),
        };
        self.step(crate::entries::tests::answer_outcome(from, number, outcome))
            .0
    }

    /// Answers the one request the last step sent with `reply`.
    pub(crate) fn reply(&mut self, reply: Reply) -> Vec<Heard> {
        self.reply_outcome(verbatim_model::request::Outcome::Result(reply))
    }

    /// `app`'s outpost made ready, if it is not.
    pub(crate) fn app(&mut self, app: App) {
        if self.ready.iter().any(|ready| ready.pid == app.pid) {
            return;
        }
        self.ready.push(app);
        let _ = crate::entries::tests::ready_at(&mut self.held, self.at_us, app.instance());
    }

    /// The focus on `focus` in `app`, under `ancestors`, outermost first,
    /// as the listener tells it and the outpost reads it: every effect of
    /// the read's answer.
    pub(crate) fn focus(
        &mut self,
        app: App,
        focus: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
    ) -> Vec<Heard> {
        self.focus_read(app, focus_read_of(app, focus, ancestors))
    }

    /// The focus the outpost reads as `read`, told by the listener.
    pub(crate) fn focus_read(&mut self, app: App, read: FocusRead) -> Vec<Heard> {
        self.app(app);
        let number = read.focus.read.node.expect("a focus node").number();
        let tell = focus_tell(app, number, self.at_us + 1_000);
        let (_, sent) = self.step(Input::Entry(Entry::of_tell(LISTENER, tell).unwrap()));
        let [(object, Reader::Listener, Wire::ReadObject(_))] = &sent[..] else {
            panic!("one listener read asked: {sent:?}");
        };
        let (_, sent) = self.step(answer(
            LISTENER,
            *object,
            Reply::Object(Box::new(listener_read(app, number))),
        ));
        let [(completion, _, Wire::CompleteUia(_))] = &sent[..] else {
            panic!("one completion asked: {sent:?}");
        };
        self.step(answer(
            app.instance(),
            *completion,
            Reply::Focus(Box::new(read)),
        ))
        .0
    }

    /// A UI Automation property change of `property` to `value` on the
    /// committed focus of `app`, told by its outpost, which follows it.
    pub(crate) fn followed_change(
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
        self.step(Input::Entry(Entry::of_tell(app.instance(), tell).unwrap()))
            .0
    }

    /// A UI Automation property change of `property` to `value` on node
    /// `number` of `app`, told by the listener, which kept the element.
    pub(crate) fn uia_change(
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
        self.step(Input::Entry(Entry::of_tell(LISTENER, tell).unwrap()))
            .0
    }

    /// A repeat of the reader's last focus, node `number` of `app`, which
    /// the check thread found still kept and focused: a duplicate, which
    /// ends with no read. Every effect of it.
    pub(crate) fn duplicate(&mut self, app: App, number: u64) -> Vec<Heard> {
        let mut tell = focus_tell(app, number, self.at_us + 1_000);
        if let Some(checked) = tell.checked.as_mut() {
            checked.kept = vec![(ElementToken(number), Read::Answered(true))];
        }
        self.step(Input::Entry(Entry::of_tell(LISTENER, tell).unwrap()))
            .0
    }

    /// Review command `command` pressed, `repeat` extra times, each read
    /// it asks of the navigator answered as the object `navigator` still
    /// is: its properties as they are, and no text. Every effect after the
    /// press's requests.
    pub(crate) fn review(
        &mut self,
        command: verbatim_model::ReviewCommand,
        repeat: u8,
        navigator: &NodeSnapshot,
    ) -> Vec<Heard> {
        self.keys += 1;
        let press = crate::keys::entry::PressEntry::new(
            TraceId::from_raw(100 + self.keys),
            verbatim_model::request::Press {
                key_seq: verbatim_model::KeySeq::new(self.keys),
                pressed_at_us: self.at_us + 1_000,
                action: verbatim_model::request::PressAction::Command { command, repeat },
            },
        );
        let mut heard = self.step(Input::Entry(Entry::Press(Box::new(press)))).0;
        loop {
            let asked = self.sent.clone();
            let mut sent = heard
                .iter()
                .filter(|heard| matches!(heard, Heard::Sent(..)));
            if sent.next().is_none() {
                return heard;
            }
            assert!(
                heard.iter().all(|heard| matches!(heard, Heard::Sent(..))),
                "a press's requests with nothing else: {heard:?}"
            );
            let [(_, _, request)] = &asked[..] else {
                panic!("one request out: {asked:?}");
            };
            let reply = match request {
                Wire::Properties { .. } => Reply::Properties(values_of(navigator)),
                Wire::Text { .. } => Reply::Text(verbatim_model::TextReply::NoText),
                other => panic!("a review read these tests do not answer: {other:?}"),
            };
            heard = self.reply(reply);
        }
    }
}
