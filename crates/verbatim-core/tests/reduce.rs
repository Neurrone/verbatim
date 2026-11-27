//! Scripted-input tests for the reducer: exact effect sequences for focus
//! speech, foreground changes, announcements, outpost replacement and
//! review, driven through Core's event queue by the public reducer
//! (`desk`), plus flight-recorder replay determinism.

mod desk;
#[path = "support/readers.rs"]
mod readers;

use readers::Found;

use desk::{
    App, Desk, FRAME, Heard, LISTENER, ONE, TWO, focus_heard, focus_now, focus_now_in,
    focus_now_with, focus_read_of, focus_tell, follow, interrupt, listener_read, queued,
    runtime_of,
};
use verbatim_core::{ReducerRecorder, Runtime, SrState, replay};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusRead, ForegroundReport, HeldIds, Instance, KnownAncestor, MsaaIds, Outcome, ProcessFact,
    QueueInput, Reader, Reply,
};
use verbatim_model::tell::{
    AcceptanceFacts, ElementToken, Event, EventObject, Payload, Relation, TellWindow,
};
use verbatim_model::{
    Backend, CellPosition, Effect, FocusNow, FocusValidity, Input, NodeDetails, NodeId,
    NodeSnapshot, NotificationKind, NotificationProcessing, ObjectKey, OutpostId, Phrase,
    ReviewCommand, Role, SegmentContent, SpeechPriority, State, StateSet, TraceId,
    UtteranceSegment, WindowHandle,
};

/// Node `id` of `app`, read through UI Automation.
fn node(
    app: App,
    id: u64,
    role: Role,
    name: Option<&str>,
    value: Option<&str>,
    states: StateSet,
) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(id),
        backend: Backend::Uia,
        role,
        name: name.map(str::to_string),
        value: value.map(str::to_string),
        states,
        details: NodeDetails::default(),
    }
}

/// Node `id` of `app`, of `role`, named `name`, that is window `handle`.
fn window_node(app: App, id: u64, role: Role, name: &str, handle: u64) -> NodeSnapshot {
    let mut window = node(app, id, role, Some(name), None, StateSet::new());
    window.details.window = Some(WindowHandle(handle));
    window
}

/// `app`'s top-level window, node [`FRAME`], named `name`.
fn frame(app: App, name: &str) -> NodeSnapshot {
    window_node(app, FRAME, Role::Window, name, app.top)
}

fn states(list: &[State]) -> StateSet {
    list.iter().copied().collect()
}

fn state(state: State) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::State(state))
}

fn not(state: State) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::NegatedState(state))
}

fn role(role: Role) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Role(role))
}

fn label_role(name: &str, of: Role) -> Vec<UtteranceSegment> {
    vec![UtteranceSegment::label(name), role(of)]
}

/// The one request the last step sent, by number, instance and kind
/// ([`desk::kind`]).
fn one_sent(desk: &Desk) -> Option<(u64, Instance, &'static str)> {
    match &desk.sent[..] {
        [(number, to, request)] => Some((*number, *to, desk::kind(request))),
        _ => None,
    }
}

/// The listener's report that `app`'s top-level window came to the
/// foreground: every effect of it.
fn foreground(desk: &mut Desk, app: App) -> Vec<Heard> {
    desk.app(app);
    desk.step(QueueInput::Foreground(ForegroundReport {
        window: app.top_level(),
        root_owner: app.top_level(),
        process: Some(app.pid),
        thread: 41,
        class: "Frame".to_owned(),
        ghost: None,
        console: false,
        integrity: None,
        own_integrity: None,
    }))
}

/// The listener's first report of a window in front, `app`'s, and the
/// focus Core asks of it at startup, answered as `read`, or not there:
/// every effect of the answer.
fn started(desk: &mut Desk, app: App, read: Option<FocusRead>) -> Vec<Heard> {
    let _ = foreground(desk, app);
    let Some((number, to, "CurrentFocus")) = one_sent(desk) else {
        panic!("the focus at startup asked: {:?}", desk.sent);
    };
    match read {
        Some(read) => desk.found(to, number, &Found::Read(read)),
        None => desk.answer(to, number, Outcome::NotThere),
    }
}

/// The user switching to `app`: its window comes to the foreground and
/// its focus, the window itself, named "App", is spoken.
fn switch_to(desk: &mut Desk, app: App) {
    let _ = foreground(desk, app);
    if let Some((number, to, "CurrentFocus")) = one_sent(desk) {
        let _ = desk.found(
            to,
            number,
            &Found::Read(focus_read_of(app, &frame(app, "App"), &[])),
        );
    } else {
        let _ = desk.focus(app, &frame(app, "App"), &[]);
    }
    assert_eq!(desk.state.attention(), Some(app.pid));
    assert_eq!(
        desk.state
            .focused()
            .map(|(source, focus)| (source, focus.id)),
        Some((app.pid, app.node(FRAME)))
    );
}

/// A UI Automation focus told by the listener as `tell` and read by
/// `app`'s outpost as `read`: every effect of the tell when it asks
/// nothing, else every effect of the read's answer.
fn told_focus(desk: &mut Desk, app: App, tell: Event, read: FocusRead) -> Vec<Heard> {
    desk.app(app);
    let number = read.focus.read.node.expect("a focus node").number();
    let heard = desk.tell(LISTENER, tell);
    let Some((object, LISTENER, "ReadObject")) = one_sent(desk) else {
        assert_eq!(desk.sent, [], "a focus asks only the listener's read");
        return heard;
    };
    let _ = desk.found(LISTENER, object, &listener_read(app, number));
    let Some((completion, to, "Completion")) = one_sent(desk) else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    desk.found(to, completion, &Found::Read(read))
}

/// The listener's UI Automation focus tell of `focus` in `app`, its
/// window's acceptance facts `facts`.
fn tell_with(desk: &Desk, app: App, focus: &NodeSnapshot, facts: AcceptanceFacts) -> Event {
    let mut tell = focus_tell(app, focus.id.number(), desk.at_us + 1_000);
    tell.window = TellWindow::Found(Box::new(facts));
    tell
}

/// A UI Automation focus on `focus` in `app`, under `ancestors`, whose
/// window has the acceptance facts `facts`.
fn focus_with(
    desk: &mut Desk,
    app: App,
    facts: AcceptanceFacts,
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
) -> Vec<Heard> {
    let tell = tell_with(desk, app, focus, facts);
    told_focus(desk, app, tell, focus_read_of(app, focus, ancestors))
}

/// The acceptance facts of `app`'s window while `front`'s top-level
/// window is in the foreground: outside it, its owner chain and its
/// active window.
fn behind(app: App, front: App) -> AcceptanceFacts {
    AcceptanceFacts {
        foreground: front.top_level(),
        foreground_root_owner: front.top_level(),
        window_to_foreground: Relation::Outside,
        root_owner_to_foreground: Relation::Outside,
        root_owner_is_foregrounds: false,
        active_window: front.top_level(),
        window_to_active: Relation::Outside,
        ..app.facts()
    }
}

/// The values a read of `snapshot` carries, as `desk` reads them.
fn read_node(app: App, snapshot: &NodeSnapshot) -> verbatim_model::request::FocusNode {
    desk::focus_node(app, snapshot)
}

/// The effects of an MSAA focus committed at `now` in the application of
/// the focus before it, announced by the utterances `said`, each queued,
/// with no speech cut off: an MSAA focus is followed by no subscription.
fn msaa_heard(now: FocusNow, said: Vec<Vec<UtteranceSegment>>) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now)];
    expected.extend(said.into_iter().map(queued));
    expected
}

/// A focus answer of `focus` in `app` under `ancestors`, read through MSAA
/// by the event of `object` in `app`'s window.
fn msaa_read(app: App, object: i32, focus: &NodeSnapshot, ancestors: &[NodeSnapshot]) -> FocusRead {
    msaa_read_in(app.window(), app, object, focus, ancestors)
}

/// [`msaa_read`] by the event of `object` in window `window`.
fn msaa_read_in(
    window: WindowHandle,
    app: App,
    object: i32,
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
) -> FocusRead {
    FocusRead {
        msaa: Some(HeldIds {
            event: MsaaIds {
                window,
                object,
                child: 0,
            },
            held_child: 0,
        }),
        ..focus_read_of(app, focus, ancestors)
    }
}

/// The listener's MSAA focus tell of object `object` of `app`'s window
/// `window`.
fn msaa_tell(desk: &Desk, app: App, window: WindowHandle, object: i32) -> Event {
    let mut facts = app.facts();
    facts.window.handle = window;
    Event {
        kind: EventKind::MsaaFocus,
        object: EventObject::Msaa {
            window,
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        checked: None,
        window: TellWindow::Found(Box::new(facts)),
        ..focus_tell(app, 1, desk.at_us + 1_000)
    }
}

/// An MSAA focus on object `object` of `app`'s window, told by the
/// listener, its run answered with `read`: every effect of the answer.
fn msaa_focus(desk: &mut Desk, app: App, object: i32, read: FocusRead) -> Vec<Heard> {
    desk.app(app);
    let window = read.msaa.expect("an MSAA read").event.window;
    let tell = msaa_tell(desk, app, window, object);
    let _ = desk.tell(LISTENER, tell);
    let Some((run, to, "Candidate")) = one_sent(desk) else {
        panic!("one run asked: {:?}", desk.sent);
    };
    desk.found(to, run, &Found::Read(read))
}

/// An ancestor's state change spoken just before the focus moves into
/// another top-level window is cut off by that move, as NVDA's foreground
/// change cancels speech (`docs/parity/focus-events.md`, "When speech is
/// cut off, and cancellation of expired focus speech").
#[test]
fn an_ancestors_change_before_a_focus_in_another_window_is_cut_off() {
    let mut desk = Desk::new();
    let dialog = node(
        ONE,
        31,
        Role::Dialog,
        Some("Settings"),
        None,
        StateSet::new(),
    );
    let remove = node(
        ONE,
        32,
        Role::Button,
        Some("Remove"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let _ = desk.focus(ONE, &remove, &[dialog]);
    assert_eq!(
        desk.uia_change(
            ONE,
            31,
            PropertyId::States,
            Some(PropertyValue::States(states(&[State::Disabled])))
        ),
        vec![queued(vec![state(State::Disabled)])]
    );
    let other = ONE.in_window(4_100);
    let no = node(
        other,
        42,
        Role::Button,
        Some("No"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    assert_eq!(
        desk.focus(other, &no, &[]),
        vec![
            Heard::Expire(focus_now(ONE, 42, &[])),
            Heard::Stop,
            queued(label_role("No", Role::Button)),
            follow(ONE),
        ],
        "speech stops before the new window's focus is spoken"
    );
}

/// An MSAA tree item expanded while it is the focus says it is expanded,
/// then how many items it holds, read when its change is judged; a later
/// change while it stays expanded reads and says no count.
#[test]
fn expanding_a_tree_view_item_says_how_many_items_it_holds() {
    let mut desk = Desk::new();
    let focusable = StateSet::new()
        .with(State::Focusable)
        .with(State::Focused)
        .with(State::Selectable);
    let collapsed = focusable.with(State::Selected).with(State::Collapsed);
    let item = node(ONE, 21, Role::TreeItem, Some("Roles"), None, collapsed);
    let _ = msaa_focus(&mut desk, ONE, -4, msaa_read(ONE, -4, &item, &[]));
    let changed = |desk: &mut Desk, states: StateSet| {
        let tell = Event {
            kind: EventKind::StateChange,
            object: EventObject::Msaa {
                window: ONE.window(),
                object: -4,
                child: 0,
            },
            cached: PropertyValues::default(),
            checked: None,
            ..focus_tell(ONE, 21, desk.at_us + 1_000)
        };
        assert_eq!(
            desk.tell(LISTENER, tell),
            [Heard::Sent(Reader::Outpost(ONE.pid), "ReadObject")]
        );
        desk.reply(change_of_focus(listener_object(
            ONE,
            item.id,
            PropertyId::States,
            PropertyValue::States(states),
        )))
    };
    let expanded = focusable.with(State::Selected).with(State::Expanded);
    assert_eq!(
        changed(&mut desk, expanded),
        [Heard::Sent(Reader::Outpost(ONE.pid), "Properties")]
    );
    assert_eq!(
        desk.reply_object(verbatim_model::request::NodeRead {
            node: None,
            window: ONE.node_window(),
            values: PropertyValues(vec![PropertyAnswer {
                id: PropertyId::ChildCount,
                value: Some(PropertyValue::Count(52)),
            }]),
        }),
        vec![
            queued(vec![state(State::Expanded)]),
            queued(vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Items(52)
            ))]),
        ],
        "the state, then the count on its own"
    );

    // A further change while it stays expanded says no count: losing the
    // selection says "not selected" alone.
    let unselected = focusable.with(State::Expanded);
    assert_eq!(
        changed(&mut desk, unselected),
        vec![queued(vec![not(State::Selected)])],
        "only a change that makes the item expanded says the count"
    );
}

/// An outpost's read of node `node` in `app`'s window, answering `id` as
/// `value`.
fn listener_object(
    app: App,
    node: NodeId,
    id: PropertyId,
    value: PropertyValue,
) -> verbatim_model::request::NodeRead {
    verbatim_model::request::NodeRead {
        node: Some(node),
        window: app.node_window(),
        values: PropertyValues(vec![PropertyAnswer {
            id,
            value: Some(value),
        }]),
    }
}

/// An MSAA change's answer: its object is the committed focus, read as
/// `read`.
fn change_of_focus(read: verbatim_model::request::NodeRead) -> Reply {
    Reply::Steps(vec![
        verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::IsOneOf(Some(0))),
        verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Read(
            verbatim_model::request::NodeRead { node: None, ..read },
        )),
    ])
}

/// A short session through Core's event queue: the listener started and
/// ready, a focus told from it, and a tick.
fn sample_script() -> Vec<Input> {
    let queue = |at_us, input| Input::Queue {
        trace_id: TraceId::from_raw(47),
        at_us,
        input,
    };
    vec![
        queue(
            1_000,
            QueueInput::Process(ProcessFact::ListenerStarted(LISTENER.generation)),
        ),
        queue(2_000, QueueInput::ReaderReady(LISTENER)),
        queue(
            3_000,
            QueueInput::Tell {
                from: LISTENER,
                tell: Box::new(focus_tell(ONE, 1, 3_000)),
            },
        ),
        Input::Tick,
    ]
}

#[test]
fn replay_is_deterministic() {
    let initial = SrState::new();
    let script = sample_script();

    let first = replay(&initial, &script);
    let second = replay(&initial, &script);

    assert_eq!(first, second);
    assert_eq!(first.len(), script.len());
}

#[test]
fn flight_recorder_dump_replays_to_the_same_effects_as_live_reduction() {
    let mut recorder = ReducerRecorder::with_default_bounds(SrState::new());
    let mut state = SrState::new();
    let script = sample_script();

    let mut live_effects = Vec::new();
    for input in &script {
        let effects =
            verbatim_core::reduce(&mut state, &mut verbatim_core::Runtime::default(), input);
        recorder.record_input(input.clone(), effects.len(), &state);
        live_effects.push(effects);
    }

    let dumped = recorder.dump_inputs();
    assert_eq!(dumped, script);

    let replayed = replay(recorder.checkpoint(), &dumped);
    assert_eq!(replayed, live_effects);
}

/// A live session recorded input by input: each queue input fed at the
/// next moment, an outpost Core asks for started and made ready at once.
struct Recorded {
    state: SrState,
    runtime: Runtime,
    recorder: ReducerRecorder,
    at_us: u64,
    /// Every input fed, in order, with its effects.
    fed: Vec<(Input, Vec<Effect>)>,
    /// Every request sent, by number.
    asked: Vec<(u64, verbatim_model::request::Question)>,
}

impl Recorded {
    /// `input`, and the outpost starts it asks for: the requests it sent,
    /// by number and instance.
    fn feed(&mut self, input: QueueInput) -> Vec<(u64, Instance)> {
        let mut pending = vec![input];
        let mut sent = Vec::new();
        while !pending.is_empty() {
            self.at_us += 1_000;
            let input = Input::Queue {
                trace_id: TraceId::from_raw(9),
                at_us: self.at_us,
                input: pending.remove(0),
            };
            let effects = verbatim_core::reduce(&mut self.state, &mut self.runtime, &input);
            self.recorder
                .record_input(input.clone(), effects.len(), &self.state);
            for effect in &effects {
                match effect {
                    Effect::Queue(verbatim_model::request::QueueEffect::StartOutpost(pid)) => {
                        assert_eq!(*pid, ONE.pid);
                        pending.push(QueueInput::Process(ProcessFact::OutpostStarted {
                            process: ONE.pid,
                            generation: ONE.generation,
                        }));
                        pending.push(QueueInput::ReaderReady(ONE.instance()));
                    }
                    Effect::Queue(verbatim_model::request::QueueEffect::Send { to, ask }) => {
                        sent.push((ask.number.0, *to));
                        self.asked.push((ask.number.0, ask.request.clone()));
                    }
                    _ => {}
                }
            }
            self.fed.push((input, effects));
        }
        sent
    }

    /// Answers request `number`, a batch sent to `from`, as its reader
    /// does having found `found`.
    fn found(&mut self, from: Instance, number: u64, found: &Found) -> Vec<(u64, Instance)> {
        let batch = self
            .asked
            .iter()
            .find_map(|(asked, question)| match question {
                verbatim_model::request::Question::Batch(batch) if *asked == number => {
                    Some(batch.clone())
                }
                verbatim_model::request::Question::Batch(_) => None,
            })
            .expect("a batch asked");
        self.feed(QueueInput::Answer {
            from,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(number),
                outcome: readers::outcome(&batch, found),
                probes: Vec::new(),
                detail: String::new(),
                began_us: 0,
                ended_us: 0,
            },
        })
    }
}

/// The segments of every utterance among `effects`.
fn utterances(effects: &[Effect]) -> Vec<Vec<UtteranceSegment>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(utterance.segments.clone()),
            _ => None,
        })
        .collect()
}

/// A window that has dropped its oldest inputs replays from the checkpoint
/// the recorder keeps with it, through a dump written and read back, to
/// exactly the effects the live session produced for the inputs it kept.
/// The focus that makes the kept value changes speak was set by inputs
/// long since dropped, so a replay from an empty state would say nothing.
#[test]
fn a_window_that_dropped_its_start_replays_from_its_checkpoint() {
    let mut live = Recorded {
        state: SrState::new(),
        runtime: Runtime::default(),
        recorder: ReducerRecorder::new(4, 1 << 20, SrState::new()),
        at_us: 1_000,
        fed: Vec::new(),
        asked: Vec::new(),
    };
    let _ = live.feed(QueueInput::Process(ProcessFact::ListenerStarted(
        LISTENER.generation,
    )));
    let _ = live.feed(QueueInput::ReaderReady(LISTENER));
    let slider = node(
        ONE,
        1,
        Role::Slider,
        Some("Rate"),
        Some("0"),
        StateSet::new(),
    );
    let tell = focus_tell(ONE, 1, live.at_us + 1_000);
    let [(object, _)] = live.feed(QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(tell),
    })[..] else {
        panic!("one listener read");
    };
    let [(completion, to)] = live.found(LISTENER, object, &listener_read(ONE, 1))[..] else {
        panic!("one completion");
    };
    let _ = live.found(
        to,
        completion,
        &Found::Read(focus_read_of(ONE, &slider, &[])),
    );
    let start = live.fed.len();
    for value in 1..=20 {
        let tell = Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::UiaPropertyChanged,
            process: ONE.pid,
            application: "app".to_owned(),
            thread: 41,
            at_us: live.at_us + 1_000,
            object: EventObject::Followed {
                runtime_id: runtime_of(1),
            },
            cached: PropertyValues::default(),
            payload: Payload::PropertyChanged {
                property: PropertyId::Value,
                value: Some(PropertyValue::Text(value.to_string())),
            },
            window: TellWindow::NoWindow,
            checked: None,
        };
        assert_eq!(
            live.feed(QueueInput::Tell {
                from: ONE.instance(),
                tell: Box::new(tell),
            }),
            []
        );
    }
    assert_eq!(live.fed.len(), start + 20, "each change one input");

    let kept = live.recorder.dump_inputs();
    assert!(kept.len() < 20, "the window has dropped its start");
    let live_tail: Vec<Vec<Effect>> = live.fed[live.fed.len() - kept.len()..]
        .iter()
        .map(|(_, effects)| effects.clone())
        .collect();
    // The kept inputs set the slider to the last values, each spoken as
    // the new value alone.
    assert_eq!(
        live_tail
            .iter()
            .map(|effects| utterances(effects))
            .collect::<Vec<_>>(),
        (21 - kept.len()..=20)
            .map(|value| vec![vec![UtteranceSegment::value(value.to_string())]])
            .collect::<Vec<_>>()
    );

    let snapshot = live.recorder.snapshot();
    let mut buffer = Vec::new();
    verbatim_core::write_dump(
        &mut buffer,
        "test",
        verbatim_core::UNKNOWN_COMMIT,
        false,
        "now",
        snapshot.checkpoint(),
        snapshot.entries(),
    )
    .expect("writes");
    let contents = verbatim_core::read_dump(&mut buffer.as_slice()).expect("reads");

    assert_eq!(replay(&contents.base, &kept), live_tail);
    assert_ne!(replay(&SrState::new(), &kept), live_tail);
}

/// The speech of every utterance among `heard`, in order.
fn spoken(heard: &[Heard]) -> Vec<Vec<UtteranceSegment>> {
    heard
        .iter()
        .filter_map(|heard| match heard {
            Heard::Say(_, segments) => Some(segments.clone()),
            _ => None,
        })
        .collect()
}

/// A notification of `app`'s, of `application`, with `activity`, its
/// text "Snapped", asking to supersede earlier speech.
fn notification(desk: &Desk, app: App, application: &str, activity: &str) -> Event {
    Event {
        kind: EventKind::UiaNotification,
        application: application.to_owned(),
        payload: Payload::Notification {
            text: "Snapped".to_owned(),
            kind: NotificationKind::Other,
            processing: NotificationProcessing::MostRecent,
            activity: activity.to_owned(),
        },
        checked: None,
        ..focus_tell(app, 1, desk.at_us + 1_000)
    }
}

#[test]
fn notification_without_text_is_silent() {
    let mut desk = Desk::new();
    let button = node(ONE, 1, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(ONE, &button, &[]);
    let mut silent = notification(&desk, ONE, "app", "");
    silent.payload = Payload::Notification {
        text: String::new(),
        kind: NotificationKind::Other,
        processing: NotificationProcessing::All,
        activity: String::new(),
    };
    assert_eq!(
        desk.tell(LISTENER, silent),
        [],
        "a notification with no display text says nothing"
    );
}

// ---- Windows, menus, and name changes ----

/// The Settings frame's client area, node `id` of the frame host
/// (`ONE`): a pane named by the window's title, carrying its handle, read
/// through MSAA.
fn settings_client(id: u64) -> NodeSnapshot {
    window_node(ONE, id, Role::Pane, "Settings", ONE.top)
}

/// The Settings page's application, whose content is in the frame host's
/// top-level window.
const PAGE: App = TWO.in_window(ONE.top);

/// The frame host's own focus, its client area, as its outpost reads it on
/// a return to the window: the window itself is its foreground object.
fn frame_host_focus(desk: &mut Desk) -> Vec<Heard> {
    let client = settings_client(1);
    let mut read = msaa_read_in(ONE.top_level(), ONE, -4, &client, &[]);
    read.foreground = Some(read_node(ONE, &client));
    msaa_focus(desk, ONE, -4, read)
}

/// The page's focus on its search box, the frame host's window above its
/// process boundary read as a pane, `frame`, and the content window,
/// `content`, between.
fn page_search(desk: &mut Desk, name: &str) -> Vec<Heard> {
    let frame = window_node(PAGE, 4, Role::Pane, "Settings", ONE.top);
    let content = window_node(PAGE, 2, Role::Window, "Settings", ONE.top + 11);
    let search = node(
        PAGE,
        3,
        Role::EditableText,
        Some(name),
        None,
        StateSet::new(),
    );
    desk.focus(PAGE, &search, &[frame, content])
}

/// The frame host's window in front, told as the listener's foreground
/// report and its foreground event, whose run reads the window's client
/// area: what the run's answer said, or the event when it asked nothing.
fn frame_host_in_front(desk: &mut Desk) -> Vec<Heard> {
    in_front(desk, ONE, &settings_client(1))
}

/// The foreground event of `app`'s top-level window, whose run reads the
/// window, `window`, as the focus and the foreground object: what the
/// run's answer said, or the event when it asked nothing.
fn in_front(desk: &mut Desk, app: App, window: &NodeSnapshot) -> Vec<Heard> {
    let mut read = msaa_read_in(app.top_level(), app, 0, window, &[]);
    read.foreground = Some(read_node(app, window));
    let tell = Event {
        kind: EventKind::Foreground,
        ..msaa_tell(desk, app, app.top_level(), 0)
    };
    let heard = desk.tell(LISTENER, tell);
    let Some((run, to, "Candidate")) = one_sent(desk) else {
        return heard;
    };
    desk.found(to, run, &Found::Read(read))
}

#[test]
fn a_settings_page_is_announced_before_its_focus_whichever_outpost_reports_first() {
    // The Settings app: the frame window is `ApplicationFrameHost`'s, read
    // through MSAA as its client area, a pane carrying the frame's handle,
    // and the page and its focus are `SystemSettings`'s, whose focus
    // reaches the frame through its ancestry, as NVDA's walk does. Either
    // can reach Core first: the frame host's foreground event, or the
    // page's focus.
    let expected = vec![
        vec![UtteranceSegment::label("Settings")],
        label_role("Settings", Role::Window),
        label_role("Search box", Role::EditableText),
    ];
    let mut desk = Desk::new();
    let _ = started(&mut desk, ONE, None);
    let mut heard = frame_host_in_front(&mut desk);
    heard.extend(page_search(&mut desk, "Search box"));
    assert_eq!(
        spoken(&heard),
        expected,
        "the frame host's report first: the page's focus does not repeat the frame"
    );
    let mut desk = Desk::new();
    let _ = started(&mut desk, ONE, None);
    let mut heard = page_search(&mut desk, "Search box");
    heard.extend(frame_host_in_front(&mut desk));
    assert_eq!(
        spoken(&heard),
        expected,
        "the page's focus first: it announces the frame, whose report then says nothing"
    );
}

/// The Settings app as its readers report it: the page's outpost reads the
/// frame, above its process boundary, as NVDA's MSAA client for it, a pane
/// named by its title, carrying the frame's handle, and the frame host's
/// own focus is the frame's client area, read through MSAA. The two are
/// one object, so the page's focus says the content window and the focus,
/// as NVDA says "Settings" and then "Settings window"
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_settings_frame_read_by_two_outposts_through_msaa_is_entered_once() {
    let mut desk = Desk::new();
    let mut heard = frame_host_focus(&mut desk);
    heard.extend(page_search(&mut desk, "Search box"));
    assert_eq!(
        spoken(&heard),
        vec![
            vec![UtteranceSegment::label("Settings")],
            label_role("Settings", Role::Window),
            label_role("Search box", Role::EditableText),
        ]
    );
}

/// The owner window "Owner" of `ONE` and its button "Open".
fn owner_open() -> (NodeSnapshot, NodeSnapshot) {
    (
        window_node(ONE, 1, Role::Window, "Owner", ONE.top),
        node(ONE, 2, Role::Button, Some("Open"), None, StateSet::new()),
    )
}

/// The focus on "OK" in `TWO`'s window `dialog` of `role`, owned by
/// `ONE`'s window, which `TWO`'s outpost describes above its process
/// boundary as a pane, a stand-in, which its read takes as the foreground
/// object.
fn owned_ok(desk: &mut Desk, role_of_dialog: Role) -> Vec<Heard> {
    let stand_in = window_node(TWO, 3, Role::Pane, "Owner", ONE.top);
    let dialog = window_node(TWO, 4, role_of_dialog, "Owned", TWO.top);
    let ok = node(TWO, 5, Role::Button, Some("OK"), None, StateSet::new());
    let mut facts = TWO.facts();
    facts.root_owner = ONE.facet(ONE.top_level(), "Frame");
    let mut read = focus_read_of(TWO, &ok, &[stand_in.clone(), dialog]);
    read.foreground = Some(read_node(TWO, &stand_in));
    let tell = tell_with(desk, TWO, &ok, facts);
    told_focus(desk, TWO, tell, read)
}

/// A dialog of one application owned by a window another application's
/// outpost reads through UI Automation: the dialog's outpost describes the
/// owner, above its process boundary, as a pane, a stand-in, which is the
/// owner's element, as NVDA's two lines hold one object there. So the
/// dialog is entered with no owner title said, and the focus line keeps
/// the owner's own entry.
#[test]
fn a_uia_owner_window_above_another_process_meets_its_stand_in() {
    let mut desk = Desk::new();
    let (owner, open) = owner_open();
    let _ = desk.focus(ONE, &open, &[owner]);
    assert_eq!(
        owned_ok(&mut desk, Role::Dialog),
        vec![
            Heard::Expire(FocusNow {
                focus: ObjectKey::of_id(TWO.node(5)),
                ancestors: vec![ObjectKey::of_id(ONE.node(1)), ObjectKey::of_id(TWO.node(4)),],
                // The foreground record keeps the stand-in that entered the
                // dialog's top-level window.
                foreground: Some(ObjectKey::of_id(TWO.node(3))),
                menu_item_of_focus: None,
            }),
            queued(label_role("Owned", Role::Dialog)),
            queued(label_role("OK", Role::Button)),
            follow(ONE),
            follow(TWO),
        ],
        "the dialog entered with no owner said"
    );
}

/// [`a_uia_owner_window_above_another_process_meets_its_stand_in`] as it
/// comes live, both ways: each window's foreground change reaches Core
/// before its focus, and the dialog, owned but not a pop-up window, is its
/// own root owner. The lines are compared as NVDA compares them: the
/// dialog is entered with no owner said, and the return to the owner says
/// only its focus (capture of 2026-11-08, `owned_dialog_across_processes`).
#[test]
fn a_window_owned_across_processes_meets_its_owner_both_ways() {
    let mut desk = Desk::new();
    let (owner, open) = owner_open();
    let mut heard = started(
        &mut desk,
        ONE,
        Some(focus_read_of(ONE, &open, std::slice::from_ref(&owner))),
    );
    heard.extend(foreground(&mut desk, TWO));
    heard.extend(owned_ok(&mut desk, Role::Window));
    heard.extend(foreground(&mut desk, ONE));
    heard.extend(desk.focus(ONE, &open, &[owner]));
    assert_eq!(
        spoken(&heard),
        vec![
            label_role("Owner", Role::Window),
            label_role("Open", Role::Button),
            label_role("Owned", Role::Window),
            label_role("OK", Role::Button),
            label_role("Open", Role::Button),
        ]
    );
}

/// The return to the owner as `owned_dialog_across_processes` recorded it
/// (2026-11-17): the owner's focus is told, the dialog's application exits
/// while the listener reads it, ending the committed focus's outpost, and
/// the listener's walk then answers, having met the owner's window. The
/// owner's window, which the owner's outpost minted, is still in the
/// committed focus's line, as NVDA's old focus line keeps it, so the read
/// is completed against it and the return says only its focus.
#[test]
fn a_return_to_the_owner_as_the_dialogs_application_exits_says_its_focus() {
    let mut desk = Desk::new();
    let (owner, open) = owner_open();
    let _ = started(
        &mut desk,
        ONE,
        Some(focus_read_of(ONE, &open, std::slice::from_ref(&owner))),
    );
    // The dialog, a pop-up window the owner's window owns, comes to the
    // foreground.
    desk.app(TWO);
    let _ = desk.step(QueueInput::Foreground(ForegroundReport {
        window: TWO.top_level(),
        root_owner: ONE.top_level(),
        process: Some(TWO.pid),
        thread: 41,
        class: "Frame".to_owned(),
        ghost: None,
        console: false,
        integrity: None,
        own_integrity: None,
    }));
    // The dialog's focus, its walk meeting the owner's window, which the
    // owner's outpost minted, as the recording's did.
    let dialog = window_node(TWO, 4, Role::Window, "Owned", TWO.top);
    let ok = node(TWO, 5, Role::Button, Some("OK"), None, StateSet::new());
    desk.app(TWO);
    let mut facts = TWO.facts();
    facts.root_owner = ONE.facet(ONE.top_level(), "Frame");
    let tell = tell_with(&desk, TWO, &ok, facts);
    let _ = desk.tell(LISTENER, tell);
    let Some((object, LISTENER, "ReadObject")) = one_sent(&desk) else {
        panic!("the dialog's read asked: {:?}", desk.sent);
    };
    let _ = desk.found(LISTENER, object, &listener_read(TWO, ok.id.number()));
    let Some((completion, to, "Completion")) = one_sent(&desk) else {
        panic!("the dialog's completion asked: {:?}", desk.sent);
    };
    let (_, known) = desk::completion_of(&desk.sent[0].2).expect("a completion");
    let owner_window = KnownAncestor::Window(ONE.top_level());
    let place = known
        .iter()
        .position(|known| *known == owner_window)
        .expect("the owner's window is known");
    let mut read = focus_read_of(TWO, &ok, std::slice::from_ref(&dialog));
    read.met = Some(u32::try_from(place).expect("a short list"));
    read.foreground = None;
    let heard = desk.found(to, completion, &Found::Read(read));
    assert_eq!(
        spoken(&heard),
        vec![
            label_role("Owned", Role::Window),
            label_role("OK", Role::Button)
        ]
    );
    let _ = foreground(&mut desk, ONE);

    let tell = focus_tell(ONE, open.id.number(), desk.at_us + 1_000);
    let _ = desk.tell(LISTENER, tell);
    let Some((object, LISTENER, "ReadObject")) = one_sent(&desk) else {
        panic!("the listener's read asked: {:?}", desk.sent);
    };
    let _ = desk.step(QueueInput::Process(ProcessFact::ApplicationExited(TWO.pid)));
    let met = KnownAncestor::Uia(runtime_of(owner.id.number()));
    let read = desk::listener_read_meeting(&desk, object, (ONE, open.id.number()), &met);
    let _ = desk.found(LISTENER, object, &read);
    let Some((completion, to, "Completion")) = one_sent(&desk) else {
        panic!("the completion asked: {:?}", desk.sent);
    };
    let (_, known) = desk::completion_of(&desk.sent[0].2).expect("a completion");
    assert_eq!(to, ONE.instance());
    assert!(
        known.contains(&KnownAncestor::Node(owner.id)),
        "the owner's window is known to the outpost that minted it: {known:?}"
    );
    let place = known
        .iter()
        .position(|known| *known == met)
        .expect("the ancestor the walk met is known");
    let mut focus = focus_read_of(ONE, &open, &[]);
    focus.met = Some(u32::try_from(place).expect("a short list"));
    focus.foreground = None;
    let heard = desk.found(to, completion, &Found::Read(focus));
    assert_eq!(spoken(&heard), vec![label_role("Open", Role::Button)]);
}

/// The last step of a focus: the batch `number` asked of `to`, answered
/// as its reader answers it having found `found`, fed with every effect
/// it had, whole.
fn raw_found(desk: &mut Desk, to: Instance, number: u64, found: &Found) -> Vec<Effect> {
    let batch = desk
        .asked
        .iter()
        .find_map(|(asked, _, question)| match question {
            verbatim_model::request::Question::Batch(batch) if *asked == number => {
                Some(batch.clone())
            }
            verbatim_model::request::Question::Batch(_) => None,
        })
        .expect("a batch asked");
    desk.at_us += 1_000;
    desk.feed(QueueInput::Answer {
        from: to,
        answer: verbatim_model::request::Answer {
            number: verbatim_model::request::RequestNumber(number),
            outcome: readers::outcome(&batch, found),
            probes: Vec::new(),
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
        },
    })
}

/// [`msaa_focus`], its answer's effects whole.
fn raw_msaa_focus(desk: &mut Desk, app: App, object: i32, read: FocusRead) -> Vec<Effect> {
    desk.app(app);
    let window = read.msaa.expect("an MSAA read").event.window;
    let tell = msaa_tell(desk, app, window, object);
    let _ = desk.tell(LISTENER, tell);
    let Some((run, to, "Candidate")) = one_sent(desk) else {
        panic!("one run asked: {:?}", desk.sent);
    };
    raw_found(desk, to, run, &Found::Read(read))
}

/// [`Desk::focus`], with `read` as the outpost's answer, its effects
/// whole.
fn raw_focus(desk: &mut Desk, app: App, read: FocusRead) -> Vec<Effect> {
    desk.app(app);
    let number = read.focus.read.node.expect("a focus node").number();
    let tell = focus_tell(app, number, desk.at_us + 1_000);
    let _ = desk.tell(LISTENER, tell);
    let Some((object, LISTENER, "ReadObject")) = one_sent(desk) else {
        panic!("one listener read asked: {:?}", desk.sent);
    };
    let _ = desk.found(LISTENER, object, &listener_read(app, number));
    let Some((completion, to, "Completion")) = one_sent(desk) else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    raw_found(desk, to, completion, &Found::Read(read))
}

/// Where the focus is, as `effects` told the speech manager, before any
/// utterance.
fn told_where(effects: &[Effect]) -> &FocusNow {
    let told = effects
        .iter()
        .position(|effect| matches!(effect, Effect::DropExpiredSpeech(_)));
    let first_said = effects
        .iter()
        .position(|effect| matches!(effect, Effect::Speak(..)));
    match (told, first_said) {
        (Some(told), first_said) if first_said.is_none_or(|said| told < said) => {
            let Effect::DropExpiredSpeech(now) = &effects[told] else {
                unreachable!("found as one")
            };
            now
        }
        _ => panic!("the speech manager is told where the focus is first: {effects:?}"),
    }
}

/// Whether each utterance among `said` still holds where the focus is
/// once `effects` told the speech manager.
fn still_held(said: &[Effect], effects: &[Effect]) -> Vec<Option<bool>> {
    let now = told_where(effects);
    said.iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => {
                Some(utterance.validity.map(|validity| validity.holds(now)))
            }
            _ => None,
        })
        .collect()
}

/// The Settings frame's own focus, its client area as the frame host's
/// outpost reports it on a return by Alt+Tab, keeps its speech when the
/// page's outpost then reports the content's focus with its own node for
/// that window at the top of its chain: the two nodes are one object by
/// their window, as NVDA's `==` finds its one object for the frame
/// (`docs/parity/focus-events.md`, "When speech is cut off, and
/// cancellation of expired focus speech").
#[test]
fn a_frame_s_focus_speech_survives_the_content_s_focus() {
    let mut desk = Desk::new();
    let client = settings_client(1);
    let mut read = msaa_read_in(ONE.top_level(), ONE, -4, &client, &[]);
    read.foreground = Some(read_node(ONE, &client));
    let said = raw_msaa_focus(&mut desk, ONE, -4, read);
    let frame = window_node(PAGE, 4, Role::Pane, "Settings", ONE.top);
    let content = window_node(PAGE, 2, Role::Window, "Settings", ONE.top + 11);
    let search = node(
        PAGE,
        3,
        Role::EditableText,
        Some("Search box"),
        None,
        StateSet::new(),
    );
    let effects = raw_focus(
        &mut desk,
        PAGE,
        focus_read_of(PAGE, &search, &[frame, content]),
    );
    assert_eq!(
        still_held(&said, &effects),
        [Some(true)],
        "the frame's \"Settings\" is still worth hearing"
    );
}

/// A window's focus speech, reported by one outpost, holds while another
/// outpost reports the same window as the focus under a node of its own:
/// the new focus meets the old one by its window, so the old node is not
/// among the focus's ancestors, and only the identity rule keeps the speech
/// (`docs/parity/focus-events.md`, "When speech is cut off, and
/// cancellation of expired focus speech").
#[test]
fn a_window_s_focus_speech_holds_while_another_outpost_reports_it_as_the_focus() {
    let mut desk = Desk::new();
    let client = settings_client(1);
    let mut read = msaa_read_in(ONE.top_level(), ONE, -4, &client, &[]);
    read.foreground = Some(read_node(ONE, &client));
    let said = raw_msaa_focus(&mut desk, ONE, -4, read);
    let frame = window_node(PAGE, 4, Role::Pane, "Settings", ONE.top);
    let mut read = focus_read_of(PAGE, &frame, &[]);
    read.foreground = Some(read_node(PAGE, &frame));
    let effects = raw_focus(&mut desk, PAGE, read);
    assert_eq!(
        still_held(&said, &effects),
        [Some(true)],
        "the first outpost's \"Settings\" is still worth hearing"
    );
}

/// A window above the process boundary with no system menu, a host's
/// child window between its top-level window and the focus's own window,
/// is read as NVDA's MSAA client for it, a pane, which is structure and
/// never spoken, named or not, while the top-level window's client above
/// it is spoken by its title (`docs/parity/focus-context.md`, "Top-level
/// windows in the ancestry").
#[test]
fn a_child_window_above_the_process_boundary_is_not_spoken() {
    let mut desk = Desk::new();
    let host = window_node(TWO, 1, Role::Pane, "Host", TWO.top);
    let between = window_node(TWO, 2, Role::Pane, "Island site", TWO.top + 2);
    let ok = node(TWO, 3, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        spoken(&desk.focus(TWO, &ok, &[host, between])),
        vec![
            vec![UtteranceSegment::label("Host")],
            label_role("OK", Role::Button),
        ]
    );
}

/// Two windows are two containers whatever their role and name: a focus in
/// a second "Remove" dialog, another window in the same top-level window,
/// enters it (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn two_windows_with_one_role_and_name_are_two_containers() {
    let mut desk = Desk::new();
    let first = window_node(ONE, 1, Role::Dialog, "Remove", ONE.top + 20);
    let second = window_node(ONE, 3, Role::Dialog, "Remove", ONE.top + 21);
    let button = |id, name| node(ONE, id, Role::Button, Some(name), None, StateSet::new());
    let mut heard = msaa_focus(
        &mut desk,
        ONE,
        -4,
        msaa_read(ONE, -4, &button(2, "OK"), &[first]),
    );
    heard.extend(msaa_focus(
        &mut desk,
        ONE,
        -5,
        msaa_read(ONE, -5, &button(4, "Yes"), &[second]),
    ));
    let dialog = label_role("Remove", Role::Dialog);
    assert_eq!(
        spoken(&heard),
        vec![
            dialog.clone(),
            label_role("OK", Role::Button),
            dialog,
            label_role("Yes", Role::Button),
        ]
    );
}

#[test]
fn a_name_change_on_a_focus_ancestor_is_silent() {
    let mut desk = Desk::new();
    let window = node(ONE, 1, Role::Window, None, None, StateSet::new());
    let edit = node(
        ONE,
        2,
        Role::EditableText,
        Some("Text"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &edit, &[window]);
    assert_eq!(
        desk.uia_change(
            ONE,
            1,
            PropertyId::Name,
            Some(PropertyValue::Text("Untitled - Notepad".to_owned()))
        ),
        [],
        "a window nameless when focus entered it is not announced later"
    );
}

#[test]
fn entering_menus_is_silent_and_only_the_item_is_announced() {
    let mut desk = Desk::new();
    let menu_bar = node(
        ONE,
        1,
        Role::MenuBar,
        Some("Application"),
        None,
        StateSet::new(),
    );
    let menu = node(ONE, 2, Role::Menu, Some("File"), None, StateSet::new());
    let item = node(ONE, 3, Role::MenuItem, Some("Open"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &item, &[menu_bar, menu]),
        vec![
            Heard::Expire(focus_now(ONE, 3, &[1, 2])),
            Heard::Stop,
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Open")]),
            follow(ONE),
        ],
        "entering each menu cancels speech, and only the item is announced"
    );
}

/// A container entered outside a menu in the same focus change is cut off
/// as the menu is entered, as NVDA cancels in the menu's place among the
/// entered containers: Windows 11 Notepad's File menu queues its popup
/// window and cancels it (`docs/parity/changes-and-selection.md`, "Entered menus").
#[test]
fn a_container_entered_before_a_menu_is_cut_off_by_it() {
    let mut desk = Desk::new();
    let popup = node(ONE, 1, Role::Window, Some("Popup"), None, StateSet::new());
    let menu = node(ONE, 2, Role::Menu, None, None, StateSet::new());
    let item = node(
        ONE,
        3,
        Role::MenuItem,
        Some("New tab"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &item, &[popup, menu]),
        vec![
            Heard::Expire(focus_now(ONE, 3, &[1, 2])),
            queued(label_role("Popup", Role::Window)),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("New tab")]),
            follow(ONE),
        ]
    );
}

// ---- Attention (decision D14) ----

/// The effects of a focus on node `focus` of `TWO`, with no ancestors,
/// while `ONE`'s window is in the foreground: it cuts speech and is
/// spoken as `said`, and `ONE`'s followed node is let go.
fn taken_behind_one(focus: u64, ancestors: &[u64], said: Vec<Vec<UtteranceSegment>>) -> Vec<Heard> {
    let foreground = match ancestors.first() {
        Some(outermost) => (TWO, *outermost),
        None => (ONE, FRAME),
    };
    entered_behind_one(
        focus_now_with(TWO, focus, ancestors, Some(foreground)),
        said,
        &[ONE, TWO],
    )
}

/// The effects of a focus entering another top-level window, told to the
/// speech manager as `now`: it cuts speech, is spoken as `said`, and a
/// `Follow` goes to each of `followed`.
fn entered_behind_one(
    now: FocusNow,
    said: Vec<Vec<UtteranceSegment>>,
    followed: &[App],
) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now), Heard::Stop];
    expected.extend(said.into_iter().map(queued));
    expected.extend(followed.iter().map(|app| follow(*app)));
    expected
}

#[test]
fn topmost_shared_owner_and_active_uwp_windows_are_attended() {
    let topmost = AcceptanceFacts {
        window_topmost: true,
        ..behind(TWO, ONE)
    };
    // A window sharing the foreground window's root owner.
    let shared_owner = AcceptanceFacts {
        root_owner: ONE.facet(ONE.top_level(), "Frame"),
        root_owner_is_foregrounds: true,
        ..behind(TWO, ONE)
    };
    let mut active_uwp = behind(TWO, ONE);
    "Windows.UI.Core.CoreWindow".clone_into(&mut active_uwp.window.class);
    active_uwp.window_to_active = Relation::Under;
    for facts in [topmost, shared_owner, active_uwp] {
        let mut desk = Desk::new();
        switch_to(&mut desk, ONE);
        let button = node(TWO, 2, Role::Button, Some("OK"), None, StateSet::new());
        assert_eq!(
            focus_with(&mut desk, TWO, facts.clone(), &button, &[]),
            taken_behind_one(2, &[], vec![label_role("OK", Role::Button)]),
            "attended: {facts:?}"
        );
    }
}

#[test]
fn a_foreground_change_moves_attention_and_says_nothing() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    // A focus in the previous application, observed before the switch:
    // the listener tells it before the foreground report, and its read is
    // out when the switch comes.
    assert_eq!(
        desk.tell(LISTENER, focus_tell(ONE, 3, desk.at_us)),
        [Heard::Sent(Reader::Listener, "ReadObject")]
    );
    let Some((late, to, "ReadObject")) = one_sent(&desk) else {
        panic!("one read asked: {:?}", desk.sent);
    };
    assert_eq!(foreground(&mut desk, TWO), [], "nothing is said");
    assert_eq!(desk.state.attention(), Some(TWO.pid));
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(ONE.node(FRAME)),
        "the focus is unchanged"
    );

    // The window's own focus is spoken.
    let other = window_node(TWO, 2, Role::Window, "Calculator", TWO.top);
    let mut read = focus_read_of(TWO, &other, &[]);
    read.foreground = Some(read_node(TWO, &other));
    assert_eq!(
        desk.focus_read(TWO, read),
        vec![
            Heard::Expire(focus_now_with(TWO, 2, &[], None)),
            Heard::Stop,
            queued(label_role("Calculator", Role::Window)),
            follow(ONE),
            follow(TWO),
        ]
    );

    // The previous application's focus observed before the switch is
    // dropped: Core stopped waiting for its read at the switch, so its
    // late answer says nothing and leaves the focus where it is.
    assert_eq!(desk.found(to, late, &listener_read(ONE, 3)), []);
    assert_eq!(
        desk.state.focused().map(|(_, node)| node.id),
        Some(TWO.node(2))
    );
}

#[test]
fn notifications_are_spoken_only_from_the_focus_application() {
    // As in Settings: the frame's application holds attention, and the
    // focus is in another application's content inside that window.
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let toggle = node(
        PAGE,
        2001,
        Role::Button,
        Some("Wi-Fi"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(PAGE, &toggle, &[]);
    let tell = notification(&desk, PAGE, "app", "");
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![interrupt(vec![UtteranceSegment::text("Snapped")])],
        "the focus's application"
    );
    let tell = notification(&desk, ONE, "app", "");
    assert_eq!(
        desk.tell(LISTENER, tell),
        [],
        "the attended application without the focus"
    );
}

#[test]
fn with_no_focus_notifications_are_spoken_from_the_attention_application() {
    let mut desk = Desk::new();
    // The focus at startup, which the application does not give.
    assert_eq!(started(&mut desk, ONE, None), []);
    assert_eq!(desk.state.focused(), None);
    let tell = notification(&desk, TWO, "app", "");
    assert_eq!(
        desk.tell(LISTENER, tell),
        [],
        "a background application's notification"
    );
    let tell = notification(&desk, ONE, "app", "");
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![interrupt(vec![UtteranceSegment::text("Snapped")])]
    );
}

/// The shell's window-snap results, which File Explorer gives.
const SNAP_RESULTS: &str = "Windows.Shell.SnapComponent.SnapHotKeyResults";

#[test]
fn the_snap_results_notification_is_spoken_from_anywhere_queued() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let tell = notification(&desk, TWO, "explorer", SNAP_RESULTS);
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![queued(vec![UtteranceSegment::text("Snapped")])]
    );
    assert_eq!(
        desk.state.attention(),
        Some(ONE.pid),
        "background never moves attention"
    );

    // Queued even from the attended application, which asks for the
    // notification to supersede earlier speech.
    let tell = notification(&desk, ONE, "explorer", SNAP_RESULTS);
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![queued(vec![UtteranceSegment::text("Snapped")])]
    );
}

// ---- Outpost replacement ----

/// `app`'s outpost ended, and its replacement started and asked for the
/// focus: what the replacement's answer, `read`, said.
fn replaced(desk: &mut Desk, app: App, read: FocusRead) -> Vec<Heard> {
    desk.app(app.replaced());
    let _ = desk.step(QueueInput::ReaderEnded(app.instance()));
    let Some((number, to, "CurrentFocus")) = one_sent(desk) else {
        panic!("the replacement asked for the focus: {:?}", desk.sent);
    };
    assert_eq!(to, app.replaced().instance());
    desk.found(to, number, &Found::Read(read))
}

/// A replacement outpost's focus that is not a silent re-take meets the
/// dead outpost's line at its first window, by its handle: the dialog that
/// is its window's client is not said again, and the windowless group
/// below it is, under the replacement's new ids (Dickson accepted
/// re-announcing; NVDA has no counterpart).
#[test]
fn a_replacement_outpost_reporting_a_different_focus_announces_it() {
    let mut desk = Desk::new();
    let general = |app, id| node(app, id, Role::Group, Some("General"), None, StateSet::new());
    let button = node(ONE, 5, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(
        ONE,
        &button,
        &[
            window_node(ONE, 2, Role::Dialog, "Options", ONE.top),
            general(ONE, 3),
        ],
    );
    let next = ONE.replaced();
    let other = node(next, 9, Role::Button, Some("Cancel"), None, StateSet::new());
    let read = focus_read_of(
        next,
        &other,
        &[
            window_node(next, 7, Role::Dialog, "Options", ONE.top),
            general(next, 8),
        ],
    );
    assert_eq!(
        replaced(&mut desk, ONE, read),
        vec![
            Heard::Expire(FocusNow {
                focus: ObjectKey::of_id(next.node(9)),
                ancestors: vec![
                    ObjectKey::of_id(ONE.node(2)),
                    ObjectKey::of_id(next.node(8)),
                ],
                foreground: Some(ObjectKey::of_id(ONE.node(FRAME))),
                menu_item_of_focus: None,
            }),
            queued(label_role("General", Role::Group)),
            queued(label_role("Cancel", Role::Button)),
            follow(next),
        ]
    );
}

#[test]
fn a_replacement_numbering_afresh_after_many_ids_is_heard_at_once() {
    // A replaced outpost started its counters again while Core remembered
    // the old ones. Node ids name their generation, so a replacement's
    // small numbers are accepted at once, and the same numbers of the old
    // generation are a different node.
    let mut desk = Desk::new();
    for number in 1..=10_000 {
        let control = node(
            ONE,
            number,
            Role::Button,
            Some("Old"),
            None,
            StateSet::new(),
        );
        let _ = desk.focus(ONE, &control, &[]);
        // The outpost answers each commit's `Follow`, as a live one does.
        let [(follow, to, ref request)] = desk.sent[..] else {
            panic!("one Follow sent: {:?}", desk.sent);
        };
        assert_eq!(desk::kind(request), "Follow");
        let _ = desk.found(to, follow, &Found::Unfocused);
    }
    let next = ONE.replaced();
    let slider = node(
        next,
        1,
        Role::Slider,
        Some("Volume"),
        Some("50"),
        StateSet::new(),
    );
    assert_eq!(
        replaced(&mut desk, ONE, focus_read_of(next, &slider, &[])),
        focus_heard(
            next,
            focus_now_with(next, 1, &[], Some((ONE, FRAME))),
            vec![vec![
                UtteranceSegment::label("Volume"),
                role(Role::Slider),
                UtteranceSegment::value("50"),
            ]]
        ),
        "the replacement's first focus is taken"
    );
    let value = |desk: &mut Desk, app: App, number, value: &str| {
        desk.followed_change(
            app,
            number,
            PropertyId::Value,
            Some(PropertyValue::Text(value.to_owned())),
        )
    };
    assert_eq!(
        value(&mut desk, ONE, 10_000, "90"),
        [],
        "a late message from the replaced outpost"
    );
    assert_eq!(
        value(&mut desk, ONE, 1, "60"),
        [],
        "the replaced outpost's own node 1 is not the focus"
    );
    assert_eq!(
        value(&mut desk, next, 1, "60"),
        vec![queued(vec![UtteranceSegment::value("60")])],
        "the replacement's node 1 is the focus"
    );
}

#[test]
fn a_focus_in_the_window_in_front_does_not_move_attention() {
    // Windows raised the new window's foreground event while refusing it
    // the foreground, and raised none when it got the foreground later, so
    // the listener sent no foreground report. Its focus is spoken, but
    // only a foreground report moves attention (D14): a stated limit.
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let item = node(
        TWO,
        2,
        Role::TreeItem,
        Some("System Summary"),
        None,
        StateSet::new(),
    );
    // The window, unnamed, which both focuses are in.
    let window = window_node(TWO, 4, Role::Window, "", TWO.top);
    assert_eq!(
        desk.focus(TWO, &item, std::slice::from_ref(&window)),
        taken_behind_one(
            2,
            &[4],
            vec![vec![UtteranceSegment::label("System Summary")]]
        ),
        "the focus is spoken, cutting off speech for the window left"
    );
    assert_eq!(desk.state.attention(), Some(ONE.pid), "attention stays");
    let button = node(TWO, 3, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(TWO, &button, &[window]),
        focus_heard(
            TWO,
            focus_now_in(TWO, 3, &[4]),
            vec![label_role("OK", Role::Button)]
        ),
        "the window's later events are attended"
    );
}

#[test]
fn a_focus_in_a_new_foreground_window_without_a_foreground_fact_announces_the_window() {
    // The same missing foreground report, in a File Explorer folder
    // window: the focus's outermost ancestor is the window's client area,
    // read through MSAA as a pane, which, being a top-level window's
    // client with a system menu, is presented by its title before the
    // containers and the focus, as NVDA presents a window it meets in the
    // focus's ancestry.
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let title = window_node(TWO, 5, Role::Pane, "Folder - File Explorer", TWO.top);
    let list = node(
        TWO,
        6,
        Role::List,
        Some("Items View"),
        None,
        StateSet::new(),
    );
    let item = node(
        TWO,
        7,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        StateSet::new(),
    );
    let mut read = focus_read_of(TWO, &item, &[title.clone(), list]);
    read.foreground = Some(read_node(TWO, &title));
    assert_eq!(
        desk.focus_read(TWO, read),
        taken_behind_one(
            7,
            &[5, 6],
            vec![
                vec![UtteranceSegment::label("Folder - File Explorer")],
                label_role("Items View", Role::List),
                vec![UtteranceSegment::label("alpha.txt")],
            ]
        ),
        "window, list, item"
    );
}

#[test]
fn a_pane_ancestor_that_is_no_top_level_windows_client_is_not_announced() {
    // The same window entered, but its outermost ancestor is a pane that is
    // not a top-level window's client with a system menu: it is structure,
    // as any other pane, and the containers and the focus are spoken
    // without it.
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let pane = node(
        TWO,
        5,
        Role::Pane,
        Some("Folder - File Explorer"),
        None,
        StateSet::new(),
    );
    let list = node(
        TWO,
        6,
        Role::List,
        Some("Items View"),
        None,
        StateSet::new(),
    );
    let item = node(
        TWO,
        7,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(TWO, &item, &[pane, list]),
        taken_behind_one(
            7,
            &[5, 6],
            vec![
                label_role("Items View", Role::List),
                vec![UtteranceSegment::label("alpha.txt")],
            ]
        ),
        "list, item"
    );
}

#[test]
fn a_focus_entering_another_top_level_window_speaks_its_entered_ancestors_outermost_first() {
    // No foreground report came for the Settings window, as none comes for
    // a window read through UI Automation: the focus entering it cuts off
    // the speech of the window left, then speaks its entered ancestors, the
    // window first, then the focus (`books/dev/src/reference/adr/0001-focus.md`,
    // "The foreground and the window title").
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let window = node(
        TWO,
        1,
        Role::Window,
        Some("Settings"),
        None,
        StateSet::new(),
    );
    let group = node(TWO, 2, Role::Group, Some("Display"), None, StateSet::new());
    let button = node(TWO, 3, Role::Button, Some("Apply"), None, StateSet::new());
    assert_eq!(
        desk.focus(TWO, &button, &[window, group]),
        taken_behind_one(
            3,
            &[1, 2],
            vec![
                label_role("Settings", Role::Window),
                label_role("Display", Role::Group),
                label_role("Apply", Role::Button),
            ]
        ),
        "window, group, button"
    );
    assert_eq!(
        desk.state.attention(),
        Some(ONE.pid),
        "attention moves only on a foreground report"
    );
}

#[test]
fn a_window_already_announced_by_its_foreground_fact_is_not_announced_again() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let title = window_node(TWO, 5, Role::Pane, "Folder - File Explorer", TWO.top);
    let mut read = focus_read_of(TWO, &title, &[]);
    read.foreground = Some(read_node(TWO, &title));
    assert_eq!(
        desk.focus_read(TWO, read),
        entered_behind_one(
            focus_now_with(TWO, 5, &[], None),
            vec![vec![UtteranceSegment::label("Folder - File Explorer")]],
            &[ONE, TWO]
        ),
        "the window's own focus"
    );
    let item = node(
        TWO,
        7,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        StateSet::new(),
    );
    let mut read = focus_read_of(TWO, &item, std::slice::from_ref(&title));
    read.foreground = Some(read_node(TWO, &title));
    assert_eq!(
        desk.focus_read(TWO, read),
        focus_heard(
            TWO,
            focus_now_with(TWO, 7, &[5], Some((TWO, 5))),
            vec![vec![UtteranceSegment::label("alpha.txt")]]
        ),
        "only the item"
    );
}

#[test]
fn focus_returning_from_a_topmost_popup_is_still_attended() {
    // A context menu is topmost and takes focus without becoming the
    // foreground window, so it must not take attention with it.
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let popup = ONE.in_window(7_700);
    let facts = AcceptanceFacts {
        root_topmost: true,
        ..behind(popup, ONE)
    };
    let item = node(
        popup,
        2,
        Role::MenuItem,
        Some("Copy"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        focus_with(&mut desk, popup, facts, &item, &[]),
        vec![
            Heard::Expire(focus_now_with(ONE, 2, &[], Some((ONE, FRAME)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Copy")]),
            follow(ONE),
        ],
        "the topmost menu is attended"
    );
    assert_eq!(desk.state.attention(), Some(ONE.pid));

    let edit = node(
        ONE,
        3,
        Role::EditableText,
        Some("Text"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &edit, &[]),
        vec![
            Heard::Expire(focus_now_with(ONE, 3, &[], None)),
            Heard::Stop,
            queued(label_role("Text", Role::EditableText)),
            follow(ONE),
        ],
        "focus back in the foreground window"
    );
}

#[test]
fn a_nameless_whole_window_focus_becomes_the_focus_silently() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let _ = foreground(&mut desk, TWO);
    let mut nameless = node(TWO, 5, Role::Window, None, None, StateSet::new());
    nameless.details.window = Some(TWO.top_level());
    assert_eq!(
        in_front(&mut desk, TWO, &nameless),
        vec![
            Heard::Expire(focus_now_with(TWO, 5, &[], Some((TWO, 5)))),
            Heard::Stop,
            follow(ONE),
        ],
        "a bare window says nothing, but a new foreground window cancels speech, nameless or not"
    );
    assert_eq!(desk.state.attention(), Some(TWO.pid));

    // The window has its name by the time its control takes focus, but the
    // focus line already holds it, so it is not announced later.
    let named = window_node(TWO, 6, Role::Window, "Calculator", TWO.top);
    let button = node(TWO, 7, Role::Button, Some("Seven"), None, StateSet::new());
    assert_eq!(
        msaa_focus(&mut desk, TWO, -4, msaa_read(TWO, -4, &button, &[named])),
        msaa_heard(
            focus_now_with(TWO, 7, &[5], Some((TWO, 5))),
            vec![label_role("Seven", Role::Button)]
        )
    );
}

#[test]
fn held_nodes_group_focus_ancestors_selection_and_navigator_by_outpost() {
    let mut desk = Desk::new();
    let list = node(ONE, 1, Role::List, Some("Files"), None, StateSet::new());
    let pane = node(ONE, 2, Role::Pane, None, None, StateSet::new());
    let _ = desk.focus(ONE, &list, &[pane]);
    let outpost = OutpostId(ONE.generation);
    let held = desk.state.held_nodes();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[&outpost].iter().copied().collect::<Vec<_>>(),
        vec![ONE.node(1), ONE.node(2)]
    );
}

// ---- Toasts ----

/// A Windows toast's window opened, the element `[42, id]`, in
/// `TWO`'s Core window, heard at `at_us`, and its read answered: every
/// effect of it.
fn windows_toast(desk: &mut Desk, id: u64, at_us: u64) -> Vec<Heard> {
    let mut facts = TWO.facts();
    "Windows.UI.Core.CoreWindow".clone_into(&mut facts.window.class);
    let toast = node(
        TWO,
        9,
        Role::Window,
        Some("Download complete"),
        None,
        StateSet::new(),
    );
    let tell = Event {
        kind: EventKind::UiaWindowOpened,
        at_us,
        object: EventObject::Uia {
            token: ElementToken(id),
            runtime_id: runtime_of(id),
        },
        // Its event's cache names it, which it is spoken by.
        cached: PropertyValues(vec![PropertyAnswer {
            id: PropertyId::Name,
            value: Some(PropertyValue::Text("Download complete".to_owned())),
        }]),
        window: TellWindow::Found(Box::new(facts)),
        checked: None,
        ..focus_tell(TWO, id, at_us)
    };
    desk.at_us = desk.at_us.max(at_us);
    let heard = desk.tell(LISTENER, tell);
    if heard.is_empty() {
        return heard;
    }
    assert_eq!(heard, [Heard::Sent(Reader::Listener, "ReadObject")]);
    desk.reply_object(verbatim_model::request::NodeRead {
        node: None,
        window: TWO.node_window(),
        values: desk::values_of(&toast),
    })
}

/// The toast's speech, queued, as a toast is spoken.
fn toast_speech() -> Vec<Heard> {
    vec![queued(label_role("Download complete", Role::Window))]
}

/// A toast is spoken and recorded. The same toast within a second is
/// dropped, leaving the record, so a third is measured from the spoken one:
/// at exactly one second from it the toast is spoken, even with a dropped
/// one half a second before.
#[test]
fn a_toast_within_a_second_of_the_one_spoken_is_dropped() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    assert_eq!(windows_toast(&mut desk, 1, 1_000_000), toast_speech());
    assert_eq!(windows_toast(&mut desk, 1, 1_500_000), []);
    assert_eq!(windows_toast(&mut desk, 1, 2_000_000), toast_speech());
}

/// Another toast within a second is spoken and replaces the record: the
/// first toast again is not a repeat of the one spoken last.
#[test]
fn another_toast_within_a_second_is_spoken_and_replaces_the_record() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let _ = windows_toast(&mut desk, 1, 1_000_000);
    assert_eq!(windows_toast(&mut desk, 2, 1_100_000), toast_speech());
    assert_eq!(windows_toast(&mut desk, 1, 1_200_000), toast_speech());
}

#[test]
fn a_toast_is_spoken_from_anywhere_queued() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    assert_eq!(windows_toast(&mut desk, 1, 1_000_000), toast_speech());
    assert_eq!(
        desk.state.attention(),
        Some(ONE.pid),
        "a toast never moves attention"
    );
}

// ---- Selection in a list the focus controls ----

#[test]
fn a_controlled_selection_is_silent_once_the_controller_is_not_the_focus() {
    let mut desk = Desk::new();
    let other = node(ONE, 60, Role::Button, Some("Close"), None, StateSet::new());
    let _ = desk.focus(ONE, &other, &[]);
    let tell = Event {
        kind: EventKind::UiaElementSelected,
        cached: PropertyValues(vec![PropertyAnswer {
            id: PropertyId::States,
            value: Some(PropertyValue::States(states(&[
                State::Selectable,
                State::Selected,
            ]))),
        }]),
        checked: None,
        ..focus_tell(ONE, 51, desk.at_us + 1_000)
    };
    let heard = desk.tell(LISTENER, tell);
    let Some((number, LISTENER, "ReadObject")) = one_sent(&desk) else {
        panic!("the item is read: {heard:?}");
    };
    let _ = desk.found(LISTENER, number, &listener_read(ONE, 51));
    let Some((completion, to, _)) = one_sent(&desk) else {
        panic!("the item is completed: {:?}", desk.sent);
    };
    let result = node(
        ONE,
        51,
        Role::ListItem,
        Some("Notepad, App"),
        None,
        states(&[State::Selectable, State::Selected]),
    );
    assert_eq!(
        desk.found(
            to,
            completion,
            &Found::Item(
                listener_object(
                    ONE,
                    result.id,
                    PropertyId::Name,
                    PropertyValue::Text("Notepad, App".to_owned())
                ),
                Some(false)
            ),
        ),
        []
    );
}

// ---- Where a new chain meets the previous one ----

/// The Settings frame, focused in its own outpost as its client area and
/// read by the page's outpost as a pane above the page, is one object: the
/// page's chain meets the frame host's focus at the frame by its handle,
/// the frame host's object stays the foreground object, only the content
/// window below it is entered, and speech is not cut (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn a_new_chain_meets_the_previous_one_at_a_window_reported_by_two_outposts() {
    let mut desk = Desk::new();
    let _ = frame_host_focus(&mut desk);
    assert_eq!(
        page_search(&mut desk, "Search"),
        vec![
            Heard::Expire(FocusNow {
                focus: ObjectKey::of_id(PAGE.node(3)),
                ancestors: vec![
                    ObjectKey::of_id(PAGE.node(4)),
                    ObjectKey::of_id(PAGE.node(2)),
                ],
                foreground: Some(ObjectKey::of_id(ONE.node(1))),
                menu_item_of_focus: None,
            }),
            queued(label_role("Settings", Role::Window)),
            queued(label_role("Search", Role::EditableText)),
            follow(PAGE),
        ]
    );
}

/// A focus whose walk was cut at its depth limit, its chain completed by
/// plain windows from its nearest window, read as MSAA clients, shares
/// nothing with a line holding that window's UIA element, another object
/// by NVDA's equality, and says the window again; the focus line then
/// holds no more than that window, so the next focus, read in full, enters
/// again the window and the dialog the cut walk did not reach.
#[test]
fn a_depth_limited_chain_in_a_uia_window_says_again_what_it_did_not_reach() {
    let mut desk = Desk::new();
    let app = window_node(ONE, 1, Role::Window, "App", ONE.top);
    let options = node(ONE, 3, Role::Dialog, Some("Options"), None, StateSet::new());
    let button = |id, name| node(ONE, id, Role::Button, Some(name), None, StateSet::new());
    let plain = NodeSnapshot {
        backend: Backend::Msaa,
        ..window_node(ONE, 4, Role::Window, "App", ONE.top)
    };
    let mut heard = desk.focus(ONE, &button(2, "OK"), &[app.clone(), options.clone()]);
    heard.extend(desk.focus(ONE, &button(5, "Cancel"), &[plain]));
    heard.extend(desk.focus(ONE, &button(6, "Apply"), &[app, options]));
    assert_eq!(
        spoken(&heard),
        vec![
            label_role("App", Role::Window),
            label_role("Options", Role::Dialog),
            label_role("OK", Role::Button),
            label_role("App", Role::Window),
            label_role("Cancel", Role::Button),
            label_role("App", Role::Window),
            label_role("Options", Role::Dialog),
            label_role("Apply", Role::Button),
        ]
    );
}

/// Speech is cut when the new focus line shares no entry with the previous
/// one below the desktop, NVDA's focus difference level of one or less,
/// and not when they meet: a popup in a top-level window of its own whose
/// chain goes through the window the focus was in meets the previous line
/// there and is not cut, while a focus in another top-level window, whose
/// chain shares nothing, is (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn speech_is_cut_only_when_the_chains_differ_below_the_desktop() {
    let mut desk = Desk::new();
    let first = window_node(ONE, 1, Role::Window, "First", ONE.top);
    let ok = node(ONE, 2, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(ONE, &ok, std::slice::from_ref(&first));
    let popup_app = ONE.in_window(4_200);
    let popup = window_node(popup_app, 3, Role::List, "Choices", popup_app.top);
    let red = node(
        popup_app,
        4,
        Role::ListItem,
        Some("Red"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(popup_app, &red, &[first, popup]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 4, &[1, 3], Some((ONE, 1))),
            vec![
                label_role("Choices", Role::List),
                vec![UtteranceSegment::label("Red")],
            ]
        ),
        "the popup's chain meets the line at the first window"
    );
    let other = ONE.in_window(4_400);
    let second = window_node(other, 5, Role::Window, "Second", other.top);
    let cancel = node(
        other,
        6,
        Role::Button,
        Some("Cancel"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(other, &cancel, &[second]),
        vec![
            Heard::Expire(focus_now_with(ONE, 6, &[5], Some((ONE, 5)))),
            Heard::Stop,
            queued(label_role("Second", Role::Window)),
            queued(label_role("Cancel", Role::Button)),
            follow(ONE),
        ],
        "the chains share nothing below the desktop"
    );
}

/// A reader leaves an unpresentable top-level window, such as an unnamed
/// UI Automation one, out of a chain, which NVDA's line holds: two focuses
/// in one top-level window whose chains share no entry meet at that
/// window, so speech is not cut and their containers are entered
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_top_level_window_left_out_of_both_chains_is_where_they_meet() {
    let mut desk = Desk::new();
    let group = |id, name: &str| node(ONE, id, Role::Group, Some(name), None, StateSet::new());
    let ok = node(ONE, 2, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(ONE, &ok, &[group(1, "Left")]);
    let apply = node(ONE, 4, Role::Button, Some("Apply"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &apply, &[group(3, "Right")]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 4, &[3]),
            vec![
                label_role("Right", Role::Group),
                label_role("Apply", Role::Button),
            ]
        )
    );
}

/// A reader leaves layout ancestors out of a chain, so a focus can come
/// with no ancestors at all: focuses in one top-level window meet at that
/// window, empty chains or not, and speech is not cut as the focus moves
/// among them (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn focuses_with_empty_chains_in_one_top_level_window_meet_there() {
    let mut desk = Desk::new();
    let button = |id, name: &str| node(ONE, id, Role::Button, Some(name), None, StateSet::new());
    let _ = desk.focus(ONE, &button(1, "One"), &[]);
    assert_eq!(
        desk.focus(ONE, &button(2, "Two"), &[]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 2, &[]),
            vec![label_role("Two", Role::Button)]
        ),
        "the second button"
    );
    assert_eq!(
        desk.focus(ONE, &button(3, "Three"), &[]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 3, &[]),
            vec![label_role("Three", Role::Button)]
        ),
        "the third button"
    );
}

/// Two chains read through MSAA in one web page, whose document carries
/// its window: the new chain meets the previous line only at the
/// document, by its window handle, and an unnamed list below it with the
/// role and name of the list the previous focus was in is a new container,
/// since the group above it is new, so NVDA's "Search grouping", "list",
/// "b link" is said (`docs/parity/focus-context.md`, "Focus-ancestry
/// context").
#[test]
fn a_container_matched_by_role_and_name_does_not_end_the_walk_to_the_meeting() {
    let mut desk = Desk::new();
    let plain = |id, role, name: Option<&str>| node(ONE, id, role, name, None, StateSet::new());
    let document = window_node(ONE, 1, Role::Document, "Page", ONE.window().0);
    let a = plain(4, Role::Link, Some("a"));
    let _ = msaa_focus(
        &mut desk,
        ONE,
        -4,
        msaa_read(
            ONE,
            -4,
            &a,
            &[
                document.clone(),
                plain(2, Role::List, None),
                plain(3, Role::ListItem, None),
            ],
        ),
    );
    let b = plain(8, Role::Link, Some("b"));
    assert_eq!(
        msaa_focus(
            &mut desk,
            ONE,
            -5,
            msaa_read(
                ONE,
                -5,
                &b,
                &[
                    document,
                    plain(5, Role::Group, Some("Search")),
                    plain(6, Role::List, None),
                    plain(7, Role::ListItem, None),
                ],
            )
        ),
        msaa_heard(
            focus_now_in(ONE, 8, &[1, 5, 6, 7]),
            vec![
                label_role("Search", Role::Group),
                vec![role(Role::List)],
                label_role("b", Role::Link),
            ]
        )
    );
}

/// A UI Automation group and an MSAA group of one window, alike in role
/// and name, are two entries, as NVDA's classes never compare equal: the
/// MSAA group is entered and said, while the window, met by its handle, is
/// not said again (`docs/parity/focus-context.md`, "Focus-ancestry
/// context").
#[test]
fn a_uia_group_and_an_msaa_group_of_one_window_are_two_entries() {
    let mut desk = Desk::new();
    let group = |id| node(ONE, id, Role::Group, Some("Tools"), None, StateSet::new());
    let cut = node(ONE, 3, Role::Button, Some("Cut"), None, StateSet::new());
    let _ = desk.focus(
        ONE,
        &cut,
        &[window_node(ONE, 1, Role::Window, "App", ONE.top), group(2)],
    );
    // The window's verdict lapses, so its MSAA focus is read; the
    // outpost's walk stops at the window Core names by its handle.
    desk.at_us += 1_000_000;
    let paste = node(ONE, 6, Role::Button, Some("Paste"), None, StateSet::new());
    desk.app(ONE);
    let tell = msaa_tell(&desk, ONE, ONE.window(), -4);
    let _ = desk.tell(LISTENER, tell);
    let Some((run, to, "Candidate")) = one_sent(&desk) else {
        panic!("one run asked: {:?}", desk.sent);
    };
    let known = match &desk.sent[0].2 {
        verbatim_model::request::Question::Batch(batch) => batch
            .steps
            .iter()
            .find_map(|step| match &step.op {
                verbatim_model::batch::Op::FocusRead { known, .. } => Some(known.clone()),
                _ => None,
            })
            .expect("a full read"),
    };
    let met = known
        .iter()
        .position(|known| *known == KnownAncestor::Window(WindowHandle(ONE.top)))
        .expect("the window named by its handle");
    let read = FocusRead {
        met: Some(u32::try_from(met).expect("a short list")),
        ..msaa_read(ONE, -4, &paste, &[group(5)])
    };
    assert_eq!(
        desk.found(to, run, &Found::Read(read)),
        focus_heard(
            ONE,
            focus_now_in(ONE, 6, &[1, 5]),
            vec![
                label_role("Tools", Role::Group),
                label_role("Paste", Role::Button),
            ]
        )
    );
}

/// A duplicate of the focus does not replace the focus line, as NVDA keeps
/// its line when the focus matches itself: a duplicate ends with no read,
/// leaving the dialog in the line, so the next focus in the dialog says
/// only itself (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_duplicate_focus_does_not_replace_the_focus_line() {
    let mut desk = Desk::new();
    let app = window_node(ONE, 1, Role::Window, "App", ONE.top);
    let dialog = node(ONE, 2, Role::Dialog, Some("Options"), None, StateSet::new());
    let ok = node(ONE, 3, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(ONE, &ok, &[app.clone(), dialog.clone()]);
    assert_eq!(desk.duplicate(ONE, 3), [], "the duplicate is silent");
    let apply = node(ONE, 4, Role::Button, Some("Apply"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &apply, &[app, dialog]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 4, &[1, 2], Some((ONE, FRAME))),
            vec![label_role("Apply", Role::Button)]
        )
    );
}

/// An invisible ancestor and a border are not presented as entered
/// containers, as NVDA counts an invisible object unavailable and a border
/// layout, while a visible dialog between them is
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn an_invisible_or_border_ancestor_is_not_announced() {
    let mut desk = Desk::new();
    let of = |id, role, name: &str, states| node(ONE, id, role, Some(name), None, states);
    let hidden = of(
        1,
        Role::Group,
        "Hidden",
        StateSet::new().with(State::Invisible),
    );
    let frame = of(2, Role::Border, "Frame", StateSet::new());
    let dialog = of(3, Role::Dialog, "Visible", StateSet::new());
    let ok = of(4, Role::Button, "OK", StateSet::new());
    assert_eq!(
        spoken(&msaa_focus(
            &mut desk,
            ONE,
            -4,
            msaa_read(ONE, -4, &ok, &[hidden, frame, dialog])
        )),
        vec![
            label_role("Visible", Role::Dialog),
            label_role("OK", Role::Button),
        ]
    );
}

/// A UI Automation slider of class `SensitiveSlider`, which gives the focus
/// to its thumb.
fn sensitive_slider(id: u64, value: &str) -> NodeSnapshot {
    let mut slider = node(
        ONE,
        id,
        Role::Slider,
        Some("Volume"),
        Some(value),
        StateSet::new(),
    );
    slider.details.class_name = Some("SensitiveSlider".to_owned());
    slider
}

/// An entered `SensitiveSlider` is spoken as a focus is, its value
/// included, where any other entered container leaves its value out
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
#[test]
fn a_sensitive_slider_ancestor_is_spoken_with_its_value() {
    let mut desk = Desk::new();
    let thumb = node(ONE, 2, Role::Thumb, Some("Position"), None, StateSet::new());
    assert_eq!(
        spoken(&desk.focus(ONE, &thumb, &[sensitive_slider(1, "50")])),
        vec![
            vec![
                UtteranceSegment::label("Volume"),
                role(Role::Slider),
                UtteranceSegment::value("50"),
            ],
            label_role("Position", Role::Thumb),
        ]
    );
}

/// A `SensitiveSlider`'s change of value is spoken while it is the focus's
/// parent, its thumb being the focus, and not when it repeats the value it
/// was last read with (`docs/parity/focus-context.md`, "A slider that
/// focuses its thumb").
#[test]
fn a_sensitive_slider_s_value_change_is_spoken_while_its_thumb_is_the_focus() {
    let mut desk = Desk::new();
    let thumb = node(ONE, 2, Role::Thumb, Some("Position"), None, StateSet::new());
    let _ = desk.focus(ONE, &thumb, &[sensitive_slider(1, "50")]);
    let changed = |desk: &mut Desk, value: &str| {
        desk.followed_change(
            ONE,
            1,
            PropertyId::Value,
            Some(PropertyValue::Text(value.to_owned())),
        )
    };
    assert_eq!(
        changed(&mut desk, "60"),
        vec![queued(vec![UtteranceSegment::value("60")])]
    );
    assert_eq!(
        changed(&mut desk, "60"),
        [],
        "the value it was last read with"
    );
}

// ---- The navigator stays current, and slow answers leave it put ----

/// A UI Automation selection of node `number` of `app`, whose states are
/// now `states`.
fn selected(desk: &Desk, app: App, number: u64, now: StateSet) -> Event {
    Event {
        kind: EventKind::UiaElementSelected,
        cached: PropertyValues(vec![PropertyAnswer {
            id: PropertyId::States,
            value: Some(PropertyValue::States(now)),
        }]),
        checked: None,
        ..focus_tell(app, number, desk.at_us + 1_000)
    }
}

#[test]
fn selecting_the_focused_item_itself_says_selected() {
    let mut desk = Desk::new();
    let selectable = states(&[State::Focusable, State::Selectable]);
    let item = node(
        ONE,
        122,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        selectable,
    );
    let _ = desk.focus(ONE, &item, &[]);
    let tell = selected(&desk, ONE, 122, selectable.with(State::Selected));
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![queued(vec![state(State::Selected)])]
    );
}

#[test]
fn an_entered_container_speaks_its_states_and_position_but_not_its_value() {
    let mut desk = Desk::new();
    let mut tab = node(
        ONE,
        130,
        Role::Tab,
        Some("General"),
        Some("tab value"),
        states(&[State::Selected, State::Selectable]),
    );
    tab.details.position_in_set = Some(1);
    tab.details.set_size = Some(3);
    tab.details.keyboard_shortcut = Some("Alt+G".to_string());
    tab.details.description = Some("General".to_string());
    let field = node(
        ONE,
        131,
        Role::EditableText,
        Some("Name"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &field, &[tab]),
        focus_heard(
            ONE,
            focus_now(ONE, 131, &[130]),
            vec![
                vec![
                    UtteranceSegment::label("General"),
                    role(Role::Tab),
                    state(State::Selected),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(3),
                    }),
                ],
                label_role("Name", Role::EditableText),
            ]
        ),
        "no value, no shortcut, and no description repeating the name"
    );
}

// ---- Review messages and repeated presses, as NVDA's review commands ----

/// When speech is cut off (`docs/nvda/speech.md`, "Cancellation"): a
/// window coming to the front cancels speech and is announced, and its
/// control taking the focus a moment later queues behind the window's
/// title rather than cutting it off, since the title stays valid while its
/// window is in front.
#[test]
fn a_window_title_is_queued_before_its_control_not_cut_off() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let _ = foreground(&mut desk, TWO);
    let notepad = window_node(TWO, 10, Role::Window, "Notepad", TWO.top);
    let mut read = focus_read_of(TWO, &notepad, &[]);
    read.foreground = Some(read_node(TWO, &notepad));
    let said = raw_focus(&mut desk, TWO, read);
    let titles: Vec<_> = said
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(utterance.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        titles
            .iter()
            .map(|utterance| (utterance.priority, utterance.segments.clone()))
            .collect::<Vec<_>>(),
        [(SpeechPriority::Queued, label_role("Notepad", Role::Window))],
        "a new foreground cancels speech and is announced"
    );
    assert!(said.contains(&Effect::StopSpeech));
    let validity = titles[0]
        .validity
        .expect("focus speech carries its validity");
    assert_eq!(validity.node, TWO.node(10));

    let editor = node(
        TWO,
        11,
        Role::Document,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let mut read = focus_read_of(TWO, &editor, std::slice::from_ref(&notepad));
    read.foreground = Some(read_node(TWO, &notepad));
    let effects = raw_focus(&mut desk, TWO, read);
    let now = told_where(&effects);
    assert_eq!(now.foreground.map(|key| key.id), Some(TWO.node(10)));
    assert_eq!(
        now.ancestors.iter().map(|key| key.id).collect::<Vec<_>>(),
        vec![TWO.node(10)]
    );
    assert!(
        !effects.contains(&Effect::StopSpeech),
        "the same window: nothing is cancelled"
    );
    assert_eq!(
        utterances(&effects),
        vec![label_role("Text editor", Role::Document)],
        "the control queues behind the title"
    );
    assert!(
        validity.holds(now),
        "the window's title is still worth hearing"
    );
}

/// Focus speech for a control the user has left no longer holds, but an
/// entered container's does while the focus is inside it.
#[test]
fn focus_speech_holds_while_its_node_is_the_focus_or_contains_it() {
    let now = FocusNow {
        focus: ObjectKey::of_id(NodeId::new(3)),
        ancestors: vec![ObjectKey::of_id(NodeId::new(1))],
        foreground: Some(ObjectKey::of_id(NodeId::new(9))),
        menu_item_of_focus: None,
    };
    let left = FocusValidity {
        node: NodeId::new(2),
        window: None,
    };
    assert!(!left.holds(&now), "a control the user has left");
    for node in [1, 3, 9] {
        let validity = FocusValidity {
            node: NodeId::new(node),
            window: None,
        };
        assert!(validity.holds(&now), "node {node}");
    }
}

/// An MSAA submenu item's speech stays valid when the application then
/// focuses the submenu that holds it, and only then: not for a menu that
/// does not hold it, nor for UIA (`docs/nvda/speech.md`, "Expired focus
/// speech").
#[test]
fn a_submenu_items_speech_holds_when_its_submenu_takes_the_focus() {
    let of =
        |id: u64, role: Role| node(ONE, id, role, Some("Node"), None, states(&[State::Focused]));
    let menu_item_of_focus = |heard: &[Heard]| match heard.first() {
        Some(Heard::Expire(now)) => now.menu_item_of_focus.map(NodeId::number),
        other => panic!("the speech manager is told where the focus is first: {other:?}"),
    };
    let item_in_submenu = |desk: &mut Desk| {
        let _ = msaa_focus(
            desk,
            ONE,
            -6,
            msaa_read(ONE, -6, &of(6, Role::MenuItem), &[of(5, Role::Menu)]),
        );
    };
    let mut desk = Desk::new();
    item_in_submenu(&mut desk);
    let menu = msaa_focus(
        &mut desk,
        ONE,
        -5,
        msaa_read(ONE, -5, &of(5, Role::Menu), &[]),
    );
    assert_eq!(menu_item_of_focus(&menu), Some(6), "its submenu");
    let mut desk = Desk::new();
    item_in_submenu(&mut desk);
    let other = msaa_focus(
        &mut desk,
        ONE,
        -7,
        msaa_read(ONE, -7, &of(7, Role::Menu), &[]),
    );
    assert_eq!(menu_item_of_focus(&other), None, "another menu");

    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &of(6, Role::MenuItem), &[of(5, Role::Menu)]);
    // The window's verdict lapses, so its MSAA focus is read.
    desk.at_us += 1_000_000;
    let menu = msaa_focus(
        &mut desk,
        ONE,
        -5,
        msaa_read(ONE, -5, &of(5, Role::Menu), &[]),
    );
    assert_eq!(menu_item_of_focus(&menu), None, "a UIA item");
}

/// Review command `command` pressed on `desk`, `repeat` extra times, each
/// read it asks of the navigator answered as `navigator` still is: every
/// effect after its requests.
fn review(
    desk: &mut Desk,
    (command, repeat): (ReviewCommand, u8),
    navigator: &NodeSnapshot,
) -> Vec<Heard> {
    let mut heard = desk.command(command, repeat);
    loop {
        let Some((_, _, request)) = one_sent(desk) else {
            return heard;
        };
        assert_eq!(
            request, "Properties",
            "a review read these tests do not answer"
        );
        heard = desk.reply_object(verbatim_model::request::NodeRead {
            node: None,
            window: ONE.node_window(),
            values: desk::values_of(navigator),
        });
    }
}

/// Spelling marks each capital so the theme raises its pitch, as NVDA
/// does; other characters stay plain text.
#[test]
fn spelling_marks_capitals_for_a_raised_pitch() {
    let mut desk = Desk::new();
    let body = node(
        ONE,
        140,
        Role::StaticText,
        None,
        Some("Hi"),
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &body, &[]);
    assert_eq!(
        review(&mut desk, (ReviewCommand::ReviewCurrentLine, 1), &body),
        vec![queued(vec![
            UtteranceSegment::new(SegmentContent::SpelledCapital("H".to_owned())),
            UtteranceSegment::text("i"),
        ])]
    );
}

// ---- A runtime id reused for a new element ----

/// A File Explorer item, `position` of `set_size` in its folder's list.
fn explorer_item(
    id: u64,
    name: &str,
    item_states: &[State],
    position: u32,
    set_size: u32,
) -> NodeSnapshot {
    let mut item = node(
        ONE,
        id,
        Role::ListItem,
        Some(name),
        None,
        states(item_states),
    );
    item.details.position_in_set = Some(position);
    item.details.set_size = Some(set_size);
    item
}

/// The list "Items View" of node `id`.
fn items_view(id: u64) -> NodeSnapshot {
    node(
        ONE,
        id,
        Role::List,
        Some("Items View"),
        None,
        StateSet::new(),
    )
}

/// The folder window both lists are in, unnamed here.
fn folder() -> NodeSnapshot {
    window_node(ONE, 23, Role::Window, "", ONE.top)
}

fn position(position: u32, set_size: u32) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Position {
        position,
        set_size: Some(set_size),
    })
}

/// File Explorer, going back from a subfolder (found 2026-10-07), in the
/// order its events arrived: the focus moves to the parent folder's item
/// "Inner" in a new list, its states read before Explorer selected it, then
/// Explorer selects it, then reports the same focus again. Inner keeps the
/// node of delta.txt, whose runtime id Explorer gave it, and its read says
/// the runtime id named another element before
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"), so
/// it is no duplicate: Inner is announced after its list, a new element, as
/// NVDA says it; the selection that follows says only "selected"; the
/// repeated focus, Inner still having the focus, is silent.
#[test]
fn a_reused_runtime_id_whose_element_is_gone_is_announced_and_its_selection_follows() {
    let mut desk = Desk::new();
    let unselected = [
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Offscreen,
    ];
    let delta = explorer_item(28, "delta.txt", &unselected[..3], 1, 1);
    assert_eq!(
        desk.focus(ONE, &delta, &[folder(), items_view(24)]),
        focus_heard(
            ONE,
            focus_now(ONE, 28, &[23, 24]),
            vec![
                label_role("Items View", Role::List),
                vec![
                    UtteranceSegment::label("delta.txt"),
                    not(State::Selected),
                    position(1, 1),
                ],
            ]
        ),
        "the first focus Core takes cuts nothing off"
    );

    let inner = explorer_item(28, "Inner", &unselected, 1, 4);
    let mut read = focus_read_of(ONE, &inner, &[folder(), items_view(29)]);
    read.runtime_id_reused = true;
    assert_eq!(
        desk.focus_read(ONE, read),
        focus_heard(
            ONE,
            focus_now_in(ONE, 28, &[23, 29]),
            vec![
                label_role("Items View", Role::List),
                vec![
                    UtteranceSegment::label("Inner"),
                    not(State::Selected),
                    position(1, 4),
                ],
            ]
        ),
        "a new list with the same name and role is entered anew"
    );

    let selected = states(&[
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Selected,
    ]);
    let tell = selected_tell(&desk, 28, selected);
    assert_eq!(
        desk.tell(LISTENER, tell),
        vec![queued(vec![state(State::Selected)])]
    );
    assert_eq!(desk.duplicate(ONE, 28), [], "the repeated focus is silent");
}

/// A UI Automation selection of `ONE`'s node `number`, whose states are now
/// `now`.
fn selected_tell(desk: &Desk, number: u64, now: StateSet) -> Event {
    selected(desk, ONE, number, now)
}

/// The same return to the parent folder with the states the outpost reads
/// when it handles the focus, after Explorer selected Inner: "Items View
/// list", "Inner 1 of 4", as NVDA says it, and the selection and repeated
/// focus that follow are silent, since Inner was announced selected.
#[test]
fn a_focus_read_after_its_item_was_selected_is_announced_without_the_selection() {
    let mut desk = Desk::new();
    let selected = [
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Selected,
    ];
    let delta = explorer_item(28, "delta.txt", &selected, 1, 1);
    let _ = desk.focus(ONE, &delta, &[folder(), items_view(24)]);

    let inner = explorer_item(30, "Inner", &selected, 1, 4);
    assert_eq!(
        desk.focus(ONE, &inner, &[folder(), items_view(29)]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 30, &[23, 29]),
            vec![
                label_role("Items View", Role::List),
                vec![UtteranceSegment::label("Inner"), position(1, 4)],
            ]
        )
    );
    let tell = selected_tell(&desk, 30, states(&selected));
    assert_eq!(
        desk.tell(LISTENER, tell),
        [],
        "the selection was already heard"
    );
    assert_eq!(desk.duplicate(ONE, 30), [], "the repeated focus is silent");
}

// ---- The task switcher ----

/// The Windows 11 task switcher's `InputSite` window, in File Explorer
/// (`TWO`), topmost while `ONE`'s window is in the foreground.
fn task_switcher_window() -> AcceptanceFacts {
    let mut facts = AcceptanceFacts {
        window_topmost: true,
        root_topmost: true,
        ..behind(TWO, ONE)
    };
    "Windows.UI.Input.InputSite.WindowClass".clone_into(&mut facts.window.class);
    facts
}

/// A UI Automation focus on `focus` of `TWO`, in `application`, its window
/// `facts`, under `ancestors`, its tell caching the focus's class and role,
/// and its parent's automation id when it has one.
fn switcher_focus(
    desk: &mut Desk,
    (application, facts): (&str, AcceptanceFacts),
    focus: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
) -> Vec<Heard> {
    let mut tell = tell_with(desk, TWO, focus, facts);
    application.clone_into(&mut tell.application);
    let mut cached = vec![PropertyAnswer {
        id: PropertyId::Role,
        value: Some(PropertyValue::Role(focus.role)),
    }];
    if let Some(class) = &focus.details.class_name {
        cached.push(PropertyAnswer {
            id: PropertyId::ClassName,
            value: Some(PropertyValue::Text(class.clone())),
        });
    }
    if let Some(parent) = &focus.details.parent_automation_id {
        cached.push(PropertyAnswer {
            id: PropertyId::ParentAutomationId,
            value: Some(PropertyValue::Text(parent.clone())),
        });
    }
    tell.cached = PropertyValues(cached);
    told_focus(desk, TWO, tell, focus_read_of(TWO, focus, ancestors))
}

/// Node `id` of `TWO`, of `role` and named `name`, whose own UI Automation
/// class is `class`.
fn node_of_class(id: u64, role: Role, name: Option<&str>, class: &str) -> NodeSnapshot {
    let mut snapshot = node(TWO, id, role, name, None, StateSet::new());
    snapshot.details.class_name = Some(class.to_owned());
    snapshot
}

const INPUT_SITE: &str = "Windows.UI.Input.InputSite.WindowClass";

/// A UI Automation focus in File Explorer on a pane whose own class is
/// `Windows.UI.Input.InputSite.WindowClass`, the Windows 11 Alt+Tab
/// switcher's pane, is refused whole, as NVDA's File Explorer module
/// refuses it: no speech is cut, nothing is said, and the focus stays
/// where it was (`docs/parity/windows-and-processes.md`, "The Alt+Tab
/// switcher's pane"). The switcher's list item in the same window is
/// announced.
#[test]
fn a_uia_focus_on_the_task_switching_pane_is_refused_whole() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let pane = node_of_class(3, Role::Pane, None, INPUT_SITE);
    assert_eq!(
        switcher_focus(&mut desk, ("explorer", task_switcher_window()), &pane, &[]),
        []
    );
    assert_eq!(
        desk.state
            .focused()
            .map(|(source, focus)| (source, focus.id)),
        Some((ONE.pid, ONE.node(FRAME)))
    );

    let item = node(TWO, 4, Role::ListItem, Some("Notes"), None, StateSet::new());
    assert_eq!(
        switcher_focus(&mut desk, ("explorer", task_switcher_window()), &item, &[]),
        taken_behind_one(4, &[], vec![vec![UtteranceSegment::label("Notes")]])
    );
}

/// On Windows 10, the task switcher's frame is a window whose own UI
/// Automation class is `MultitaskingViewFrame`, in File Explorer: its UI
/// Automation focus is refused whole too, by the same rule of NVDA's File
/// Explorer module (`docs/parity/windows-and-processes.md`, "The Alt+Tab
/// switcher's pane").
#[test]
fn a_uia_focus_on_the_windows_10_task_switching_frame_is_refused_whole() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let mut facts = task_switcher_window();
    "MultitaskingViewFrame".clone_into(&mut facts.window.class);
    let frame = node_of_class(
        3,
        Role::Window,
        Some("Task Switching"),
        "MultitaskingViewFrame",
    );
    assert_eq!(
        switcher_focus(&mut desk, ("explorer", facts), &frame, &[]),
        []
    );
    assert_eq!(
        desk.state
            .focused()
            .map(|(source, focus)| (source, focus.id)),
        Some((ONE.pid, ONE.node(FRAME)))
    );
}

/// The rule matches the focused element's own class and its application,
/// not the class of the window it is in: a pane of another class in File
/// Explorer's `InputSite` window, and a pane of the `InputSite` class in
/// another application's `InputSite` window, as a XAML island in Windows
/// Terminal has, are taken as any focus (`docs/parity/windows-and-processes.md`,
/// "The Alt+Tab switcher's pane").
#[test]
fn a_pane_is_refused_only_by_its_own_class_in_file_explorer() {
    let scroll_viewer = node_of_class(3, Role::Pane, Some("Output"), "ScrollViewer");
    let terminal_pane = node_of_class(3, Role::Pane, Some("Output"), INPUT_SITE);
    for (application, pane) in [
        ("explorer", scroll_viewer),
        ("windowsterminal", terminal_pane),
    ] {
        let mut desk = Desk::new();
        switch_to(&mut desk, ONE);
        assert_eq!(
            switcher_focus(&mut desk, (application, task_switcher_window()), &pane, &[]),
            taken_behind_one(3, &[], vec![vec![UtteranceSegment::label("Output")]])
        );
    }
}

/// The task switcher's item for the window `name`, the second of ten, in
/// the first row and second column of its list, as Windows 11 gives it: a
/// list item whose raw-view parent, the list, has the automation id
/// `parent_automation_id`.
fn task_switching_item(id: u64, name: &str, parent_automation_id: Option<&str>) -> NodeSnapshot {
    let mut item = node_of_class(id, Role::ListItem, Some(name), "ListViewItem");
    item.details.position_in_set = Some(2);
    item.details.set_size = Some(10);
    item.details.cell = Some(CellPosition {
        row: 1,
        column: 2,
        table: None,
    });
    item.details.parent_automation_id = parent_automation_id.map(str::to_owned);
    item
}

/// The task switcher's containers: its window, "Task Switching", and its
/// list, "Running applications", outermost first.
fn task_switching_containers() -> Vec<NodeSnapshot> {
    vec![
        node(
            TWO,
            10,
            Role::Window,
            Some("Task Switching"),
            None,
            StateSet::new(),
        ),
        node(
            TWO,
            11,
            Role::List,
            Some("Running applications"),
            None,
            StateSet::new(),
        ),
    ]
}

/// What the task switcher's item `name` says itself: its title, row,
/// column and position.
fn task_switching_item_said(name: &str) -> Vec<UtteranceSegment> {
    vec![
        UtteranceSegment::label(name),
        UtteranceSegment::new(SegmentContent::Row(1)),
        UtteranceSegment::new(SegmentContent::Column(2)),
        position(2, 10),
    ]
}

/// What the task switcher's containers and its item `name` say as the
/// containers are entered.
fn with_containers_said(name: &str) -> Vec<Vec<UtteranceSegment>> {
    vec![
        label_role("Task Switching", Role::Window),
        label_role("Running applications", Role::List),
        task_switching_item_said(name),
    ]
}

/// Alt down or up, as the hook reports it.
fn alt(desk: &mut Desk, down: bool) {
    assert_eq!(desk.step(QueueInput::Alt { down }), []);
}

/// With Alt held in the Windows 11 Alt+Tab switcher, its item is said
/// alone, by its window's title with its row, column and position, as
/// NVDA's capture says it ("Nvda Sw B  row 1  column 2  2 of 10",
/// `target/e2e-evidence/switcher-pane-2026-10-24/NvdaSw/`): NVDA's File
/// Explorer module drops the item's containers while Alt is held
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
/// None of them is kept either: a later focus with Alt released, as in the
/// switcher Control+Alt+Tab keeps open, says them both.
#[test]
fn a_task_switching_item_reached_with_alt_held_says_only_itself() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    alt(&mut desk, true);
    let item = task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"));
    assert_eq!(
        switcher_focus(
            &mut desk,
            ("explorer", task_switcher_window()),
            &item,
            &task_switching_containers()
        ),
        taken_behind_one(4, &[], vec![task_switching_item_said("Nvda Sw B")])
    );
    // The same focus again, a duplicate, ends with no read.
    assert_eq!(desk.duplicate(TWO, 4), []);

    alt(&mut desk, false);
    let released = task_switching_item(5, "Nvda Sw A", Some("SwitchItemListControl"));
    let mut expected = vec![
        Heard::Expire(focus_now_with(TWO, 5, &[10, 11], Some((TWO, FRAME)))),
        // The line of the item said alone held only the item, and so
        // shares nothing with this one below the desktop, as NVDA's,
        // whose item had the desktop for its container.
        Heard::Stop,
    ];
    expected.extend(with_containers_said("Nvda Sw A").into_iter().map(queued));
    expected.push(follow(TWO));
    assert_eq!(
        switcher_focus(
            &mut desk,
            ("explorer", task_switcher_window()),
            &released,
            &task_switching_containers()
        ),
        expected
    );
}

/// Tab again with Alt held, as `alt_tab_switcher` does: the first item was
/// said alone and Core kept none of its containers, in what it spoke and
/// in what it keeps alike, as NVDA's item has the desktop for its
/// container. So the second item's completion names only the first item as
/// an ancestor Core knows, and the second item is said alone: Core once
/// kept the containers it had dropped, the outpost's walk met the list it
/// was sent, and the focus was dropped unspoken.
#[test]
fn a_second_task_switching_item_with_alt_held_knows_no_containers_and_is_said_alone() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    alt(&mut desk, true);
    let first = task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"));
    switcher_focus(
        &mut desk,
        ("explorer", task_switcher_window()),
        &first,
        &task_switching_containers(),
    );
    let second = task_switching_item(5, "Nvda Sw A", Some("SwitchItemListControl"));
    let mut tell = tell_with(&desk, TWO, &second, task_switcher_window());
    "explorer".clone_into(&mut tell.application);
    tell.cached = PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(Role::ListItem)),
        },
        PropertyAnswer {
            id: PropertyId::ClassName,
            value: Some(PropertyValue::Text("ListViewItem".to_owned())),
        },
        PropertyAnswer {
            id: PropertyId::ParentAutomationId,
            value: Some(PropertyValue::Text("SwitchItemListControl".to_owned())),
        },
    ]);
    desk.tell(LISTENER, tell);
    let Some((object, LISTENER, "ReadObject")) = one_sent(&desk) else {
        panic!("the listener's read asked: {:?}", desk.sent);
    };
    desk.found(LISTENER, object, &listener_read(TWO, 5));
    let Some((completion, to, "Completion")) = one_sent(&desk) else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    let (_, known) = desk::completion_of(&desk.sent[0].2).expect("a completion");
    assert_eq!(
        known,
        [
            KnownAncestor::Node(TWO.node(4)),
            KnownAncestor::Uia(runtime_of(4))
        ]
    );
    let read = focus_read_of(TWO, &second, &task_switching_containers());
    assert_eq!(
        desk.found(to, completion, &Found::Read(read)),
        [
            Heard::Expire(focus_now_with(TWO, 5, &[], Some((TWO, FRAME)))),
            Heard::Stop,
            queued(task_switching_item_said("Nvda Sw A")),
            follow(TWO),
        ]
    );
}

/// The rule drops an item's containers only for a UI Automation list item
/// in File Explorer, read with Alt held, whose parent is the task
/// switcher's list, or, in Windows 10 before version 1809, whose window is
/// the switcher's `MultitaskingViewFrame` window. A list item in File
/// Explorer under another list, one in another application under a list of
/// that id, one read through MSAA, and an element of another role under the
/// switcher's list say their containers with Alt held
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
#[test]
fn only_the_task_switchers_items_drop_their_containers() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    alt(&mut desk, true);
    let mut windows_10 = task_switcher_window();
    "MultitaskingViewFrame".clone_into(&mut windows_10.window.class);
    assert_eq!(
        switcher_focus(
            &mut desk,
            ("explorer", windows_10),
            &task_switching_item(4, "Nvda Sw B", None),
            &task_switching_containers()
        ),
        taken_behind_one(4, &[], vec![task_switching_item_said("Nvda Sw B")])
    );

    let containers_said = || {
        entered_behind_one(
            focus_now_with(TWO, 4, &[10, 11], Some((ONE, FRAME))),
            with_containers_said("Nvda Sw B"),
            &[ONE, TWO],
        )
    };
    let cell = NodeSnapshot {
        role: Role::Cell,
        ..task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"))
    };
    for (application, item) in [
        (
            "explorer",
            task_switching_item(4, "Nvda Sw B", Some("ItemsView")),
        ),
        (
            "windowsterminal",
            task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl")),
        ),
        ("explorer", cell),
    ] {
        let mut desk = Desk::new();
        switch_to(&mut desk, ONE);
        alt(&mut desk, true);
        assert_eq!(
            switcher_focus(
                &mut desk,
                (application, task_switcher_window()),
                &item,
                &task_switching_containers()
            ),
            containers_said(),
            "{application}: {item:?}"
        );
    }

    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    alt(&mut desk, true);
    let item = task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"));
    assert_eq!(
        msaa_focus(
            &mut desk,
            TWO,
            -4,
            msaa_read(TWO, -4, &item, &task_switching_containers())
        ),
        entered_behind_one(
            focus_now_with(TWO, 4, &[10, 11], Some((TWO, 10))),
            with_containers_said("Nvda Sw B"),
            &[ONE],
        ),
        "read through MSAA"
    );
}

/// The rule matches the class with its role: an element of the Windows 11
/// switcher pane's class in File Explorer that is a window, not a pane, is
/// taken as any focus (`docs/parity/windows-and-processes.md`, "The Alt+Tab
/// switcher's pane").
#[test]
fn a_window_of_the_task_switching_panes_class_is_taken() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let window = node_of_class(3, Role::Window, None, INPUT_SITE);
    let _ = switcher_focus(
        &mut desk,
        ("explorer", task_switcher_window()),
        &window,
        &[],
    );
    assert_eq!(
        desk.state
            .focused()
            .map(|(source, focus)| (source, focus.id)),
        Some((TWO.pid, TWO.node(3)))
    );
}

/// The rule refuses a UI Automation focus event only: the same pane as a
/// foreground change, which NVDA never refuses so, or read through MSAA,
/// which NVDA's rule does not match, is taken as any focus or foreground
/// change (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's
/// pane").
#[test]
fn the_task_switching_panes_foreground_change_or_msaa_focus_is_taken() {
    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let mut pane = node_of_class(3, Role::Pane, Some("Output"), INPUT_SITE);
    let _ = foreground(&mut desk, TWO);
    pane.details.window = Some(TWO.top_level());
    let _ = in_front(&mut desk, TWO, &pane);
    assert_eq!(
        desk.state
            .focused()
            .map(|(source, focus)| (source, focus.id)),
        Some((TWO.pid, TWO.node(3)))
    );

    let mut desk = Desk::new();
    switch_to(&mut desk, ONE);
    let pane = node_of_class(3, Role::Pane, Some("Output"), INPUT_SITE);
    assert_eq!(
        msaa_focus(&mut desk, TWO, -4, msaa_read(TWO, -4, &pane, &[])),
        entered_behind_one(
            focus_now_with(TWO, 3, &[], None),
            vec![vec![UtteranceSegment::label("Output")]],
            &[ONE]
        )
    );
}

// ---- Changes and the focus line (`docs/parity/changes-and-selection.md`) ----

/// File Explorer's Properties sheet, read through MSAA: Space on Apply,
/// in the order the listener tells what Explorer raised (captured
/// 2026-11-19, five runs alike): Apply's state change, disabled and
/// without the focused state, read while a focus with no window, which
/// the listener tells on the desktop window, and the focus on Read-only
/// wait behind it. NVDA says "unavailable", then the page, then
/// Read-only: the change is spoken, Apply being the focus when it is
/// handled, whatever its own focused state.
#[test]
fn an_msaa_button_disabled_as_the_focus_leaves_it_says_unavailable_then_the_new_focus() {
    let mut desk = Desk::new();
    let sheet = node(
        ONE,
        50,
        Role::Dialog,
        Some("apply.txt Properties"),
        None,
        StateSet::new(),
    );
    let page = node(
        ONE,
        51,
        Role::PropertyPage,
        Some("General"),
        None,
        StateSet::new(),
    );
    let apply_window = WindowHandle(ONE.top + 2);
    let read_only_window = WindowHandle(ONE.top + 3);
    let focused = states(&[State::Focusable, State::Focused]);
    // Each control is a window of its own, which its read names.
    let in_own_window = |window: WindowHandle, focus: &NodeSnapshot, ancestors: &[NodeSnapshot]| {
        let mut read = msaa_read_in(window, ONE, -4, focus, ancestors);
        read.focus.read.window.handle = window;
        read
    };
    let apply = node(ONE, 52, Role::Button, Some("Apply"), None, focused);
    let _ = msaa_focus(
        &mut desk,
        ONE,
        -4,
        in_own_window(apply_window, &apply, std::slice::from_ref(&sheet)),
    );
    let state_change = Event {
        kind: EventKind::StateChange,
        ..msaa_tell(&desk, ONE, apply_window, -4)
    };
    assert_eq!(
        desk.tell(LISTENER, state_change),
        [Heard::Sent(Reader::Outpost(ONE.pid), "ReadObject")]
    );
    let Some((change_read, _, _)) = one_sent(&desk) else {
        panic!("the change's read asked: {:?}", desk.sent);
    };
    // The focus with no window, which the listener tells on the desktop
    // window, served by the desktop's process (`TWO` stands for it).
    let mut desktop = msaa_tell(&desk, TWO, WindowHandle(10), -4);
    if let TellWindow::Found(facts) = &mut desktop.window {
        facts.desktop = true;
    }
    assert_eq!(desk.tell(LISTENER, desktop), []);
    assert_eq!(
        desk.tell(LISTENER, msaa_tell(&desk, ONE, read_only_window, -4)),
        []
    );
    let read = desk.answer(
        ONE.instance(),
        change_read,
        Outcome::Result(change_of_focus(listener_object(
            ONE,
            apply.id,
            PropertyId::States,
            PropertyValue::States(states(&[State::Focusable, State::Disabled])),
        ))),
    );
    let Some((run, to, "Candidate")) = one_sent(&desk) else {
        panic!("the run of focuses asked: {:?}", desk.sent);
    };
    assert_eq!(
        read,
        [
            queued(vec![state(State::Disabled)]),
            Heard::Sent(Reader::Outpost(ONE.pid), "Candidate")
        ]
    );
    let read_only = node(ONE, 53, Role::CheckBox, Some("Read-only"), None, focused);
    assert_eq!(
        desk.found(
            to,
            run,
            &Found::Read(in_own_window(read_only_window, &read_only, &[sheet, page]))
        ),
        msaa_heard(
            focus_now_in(ONE, 53, &[50, 51]),
            vec![
                label_role("General", Role::PropertyPage),
                vec![
                    UtteranceSegment::label("Read-only"),
                    role(Role::CheckBox),
                    not(State::Checked)
                ],
            ]
        )
    );
}

/// A WPF window's Apply button, read through UI Automation, disabling
/// itself and focusing OK: WPF raises OK's focus first and Apply's
/// change after it, with states lacking the focused state (captured
/// 2026-11-19, five runs alike). The outpost's tell of the change can
/// reach Core before the listener's tell of the focus; its states show
/// the focus had left, so it is not spoken, and Verbatim says "OK button"
/// alone, as NVDA does.
#[test]
fn a_uia_button_disabled_after_the_focus_left_it_says_only_the_new_focus() {
    let mut desk = Desk::new();
    let focused = states(&[State::Focusable, State::Focused]);
    let apply = node(ONE, 62, Role::Button, Some("Apply"), None, focused);
    let _ = desk.focus(ONE, &apply, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            62,
            PropertyId::States,
            Some(PropertyValue::States(states(&[
                State::Focusable,
                State::Disabled
            ])))
        ),
        [],
        "the change's states lack the focused state"
    );
    let ok = node(ONE, 61, Role::Button, Some("OK"), None, focused);
    assert_eq!(
        desk.focus(ONE, &ok, &[]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 61, &[]),
            vec![label_role("OK", Role::Button)]
        )
    );
}

/// The list item X is the focus; the application focuses the button Y
/// inside it, then selects X. In the listener's order: X's focus, Y's
/// focus tell, X's selection tell while Y is read, and Y's read answered
/// with X already selected. NVDA says Y, then "selected": X, its focus
/// when the selection was delivered, is Y's ancestor when it is handled,
/// and is compared with the states spoken of it as the focus.
#[test]
fn an_item_selected_as_the_focus_moves_inside_it_says_the_new_focus_then_selected() {
    let mut desk = Desk::new();
    let list = node(ONE, 70, Role::List, Some("Files"), None, StateSet::new());
    let selectable = states(&[State::Focusable, State::Focused, State::Selectable]);
    let x = node(ONE, 71, Role::ListItem, Some("X"), None, selectable);
    let _ = desk.focus(ONE, &x, std::slice::from_ref(&list));
    let y = node(
        ONE,
        72,
        Role::Button,
        Some("Y"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let _ = desk.tell(LISTENER, focus_tell(ONE, 72, desk.at_us + 1_000));
    let Some((object, LISTENER, "ReadObject")) = one_sent(&desk) else {
        panic!("the listener's read asked: {:?}", desk.sent);
    };
    let x_selected = states(&[State::Focusable, State::Selectable, State::Selected]);
    assert_eq!(
        desk.tell(LISTENER, selected(&desk, ONE, 71, x_selected)),
        []
    );
    let _ = desk.found(LISTENER, object, &listener_read(ONE, 72));
    let Some((completion, to, "Completion")) = one_sent(&desk) else {
        panic!("the completion asked: {:?}", desk.sent);
    };
    let mut x_now = x.clone();
    x_now.states = x_selected;
    let mut heard = focus_heard(
        ONE,
        focus_now_in(ONE, 72, &[70, 71]),
        vec![label_role("Y", Role::Button)],
    );
    heard.push(queued(vec![state(State::Selected)]));
    assert_eq!(
        desk.found(
            to,
            completion,
            &Found::Read(focus_read_of(ONE, &y, &[list, x_now]))
        ),
        heard
    );
}
