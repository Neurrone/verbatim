//! The focus entries through the reducer's state: a UI Automation focus's
//! two chained reads, a run tried by application, the judgements that end
//! an entry with no read, and the paused function rebuilt from the state
//! after a clone or a dump.

use verbatim_model::batch::{AncestorsEnd, Batch, Op, Subject};
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, FocusAncestor, FocusCaret, FocusNode, FocusRead, HeldIds, KnownAncestor, NodeRead,
    Outcome as WireOutcome, Question, Refusal, Reply, RequestNumber, Route, RouteEnd,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Payload, Read, Relation, WindowFacet,
};
use verbatim_model::{NodeId, OutpostId, Pid, Role, SegmentContent};

use super::*;
use crate::event_queue::{Input, Instance, Reader};
use crate::focus::versions::{Committed, LastFocus};
use crate::readers::Found;
use verbatim_model::Effect;
use verbatim_model::WindowHandle;

pub(crate) const APP: Pid = Pid(40);
pub(crate) const GENERATION: u64 = 3;
pub(crate) const LISTENER: Instance = Instance {
    reader: Reader::Listener,
    generation: 1,
};
pub(crate) const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: GENERATION,
};

pub(crate) fn facet(handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: APP,
        thread: 41,
    }
}

/// Window 5, of `class`, under the foreground window 4, so acceptance
/// passes by the foreground relation.
pub(crate) fn facts(class: &str) -> AcceptanceFacts {
    AcceptanceFacts {
        window: facet(5, class),
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

pub(crate) fn uia_tell(runtime: i32, kept: Vec<(ElementToken, Read<bool>)>) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(7),
        kind: EventKind::UiaFocus,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Uia {
            token: ElementToken(9),
            runtime_id: RuntimeId(vec![42, runtime]),
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        // A class on neither list: no verdict is in force.
        window: TellWindow::Found(Box::new(facts("AppPane"))),
        checked: Some(Checked {
            // The check thread's probe found a provider.
            probe: Some(Probe {
                window: WindowHandle(5),
                thread: 41,
                provider: true,
                began_us: 900,
            }),
            keyboard_focus: Some(Read::Answered(true)),
            kept,
            previous_sibling: None,
        }),
    }
}

pub(crate) fn msaa_tell(object: i32) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(7),
        kind: EventKind::MsaaFocus,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: 1_000,
        object: EventObject::Msaa {
            window: WindowHandle(5),
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts("Button"))),
        checked: None,
    }
}

pub(crate) fn values(name: &str, role: Role) -> PropertyValues {
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

/// Window `handle` of the application, inside its top-level window 4, as
/// an answer reports a node's nearest window.
pub(crate) fn win(handle: u64) -> verbatim_model::request::NodeWindow {
    verbatim_model::request::NodeWindow {
        handle: WindowHandle(handle),
        top_level: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: APP,
        thread: 41,
        desktop: false,
    }
}

pub(crate) fn node(number: u64) -> NodeId {
    NodeId::in_outpost(OutpostId(GENERATION), number)
}

/// A focus answer: the button "OK" in the frame "Editor".
pub(crate) fn focus_read(msaa: Option<HeldIds>) -> FocusRead {
    FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: Some(node(2)),
                window: win(5),
                values: values("OK", Role::Button),
            },
            runtime_id: msaa.is_none().then(|| RuntimeId(vec![42, 1])),
        },
        msaa,
        ancestors: vec![FocusAncestor {
            node: FocusNode {
                read: NodeRead {
                    node: Some(node(1)),
                    window: win(4),
                    values: values("Editor", Role::Window),
                },
                runtime_id: None,
            },
            simple_child: false,
            uia_layout: false,
            described: false,
        }],
        met: None,
        foreground: None,
        value_from_selection: false,
        runtime_id_reused: false,
        caret: FocusCaret::NotRead,
    }
}

/// A kept element as the listener reads it for a route: `name`, a
/// `role`, of runtime id `runtime`, in window 5, both a control and
/// content.
pub(crate) fn walked_element(name: &str, role: Role, runtime: Vec<i32>) -> NodeRead {
    let mut values = values(name, role);
    values.0.extend([
        PropertyAnswer {
            id: PropertyId::RuntimeId,
            value: Some(PropertyValue::RuntimeId(RuntimeId(runtime))),
        },
        PropertyAnswer {
            id: PropertyId::IsContent,
            value: Some(PropertyValue::Flag(true)),
        },
    ]);
    NodeRead {
        node: None,
        window: win(5),
        values,
    }
}

/// The listener's read of the focus "OK": the element alone, its walk
/// going on from its window.
pub(crate) fn listener_answer() -> Found {
    Found::Walked(
        walked_element("OK", Role::Button, vec![42, 1]),
        Vec::new(),
        AncestorsEnd::ContinueFrom(WindowHandle(5)),
    )
}

/// The listener's read of the focus "OK" of runtime id `runtime`, the
/// element alone, its walk going on from its window.
pub(crate) fn listener_answer_of(runtime: Vec<i32>) -> Found {
    Found::Walked(
        walked_element("OK", Role::Button, runtime),
        Vec::new(),
        AncestorsEnd::ContinueFrom(WindowHandle(5)),
    )
}

/// The committed focus given its own window `window`, which a walk then
/// knows it by, so a listener's walk sent after can meet it.
pub(crate) fn committed_window_known(held: &mut Held, window: u64) {
    let Committed::Read(focus) = &mut held.state.versions.committed else {
        panic!("a reader's focus committed");
    };
    focus.context.snapshot.details.window = Some(WindowHandle(window));
}

/// The listener's answer to its walk `number` of the focus "OK", meeting
/// the first ancestor it was asked to stop at, the window
/// [`committed_window_known`] gave the committed focus; then the committed
/// focus's line changes before the answer comes, that window no longer
/// its, so the ancestor is no longer known and the focus is read afresh.
pub(crate) fn walk_meeting_a_changed_line(held: &mut Held, number: u64) -> Input<Entry> {
    let Found::Walked(element, ancestors, _) = listener_answer() else {
        unreachable!("the listener's answer is a walk");
    };
    let input = found(
        held,
        LISTENER,
        number,
        Found::Walked(element, ancestors, AncestorsEnd::Met(0)),
    );
    let Committed::Read(focus) = &mut held.state.versions.committed else {
        panic!("a reader's focus committed");
    };
    focus.context.snapshot.details.window = None;
    input
}

/// The listener's answer to its walk `number`: `element` alone, its walk
/// meeting `met`, by its place among the ancestors the walk was asked to
/// stop at, or, with none, going on from its window.
pub(crate) fn walk_answer(
    held: &Held,
    number: u64,
    element: NodeRead,
    met: Option<&KnownAncestor>,
) -> Input<Entry> {
    let end = match met {
        None => AncestorsEnd::ContinueFrom(WindowHandle(5)),
        Some(met) => {
            let batch = held.queue.batch_for_tests(number).unwrap();
            let place = batch
                .steps
                .iter()
                .find_map(|step| match &step.op {
                    Op::Ancestors { until, .. } => until.iter().position(|known| known == met),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{met:?} is among the walk's stops"));
            AncestorsEnd::Met(u32::try_from(place).unwrap())
        }
    };
    found(
        held,
        LISTENER,
        number,
        Found::Walked(element, Vec::new(), end),
    )
}

/// A state with the runtime its test keeps beside it. A clone has no
/// paused function, as a clone of the state has none.
#[derive(Debug, Default)]
pub(crate) struct Held {
    pub(crate) state: SrState,
    pub(crate) runtime: Runtime,
}

impl Clone for Held {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            runtime: Runtime::default(),
        }
    }
}

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

impl std::ops::Deref for Held {
    type Target = SrState;

    fn deref(&self) -> &SrState {
        &self.state
    }
}

impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut SrState {
        &mut self.state
    }
}

/// Feeds `input` at `at_us`, the step's effects as a test reads them:
/// each request sent by its reader and kind, each utterance by its
/// segments, every other effect left out.
pub(crate) fn feed_at(held: &mut Held, at_us: u64, input: Input<Entry>) -> Vec<String> {
    read_as_a_test(feed(&mut held.state, &mut held.runtime, Timed { at_us, input }).unwrap())
}

/// Core's wake at `at_us` as the shell feeds it, with `in_front` what it
/// read of Windows in front for the checks after a menu, popup or switcher
/// end, `None` when none waited: the step's effects as [`feed_at`] gives
/// them.
pub(crate) fn due_at(
    held: &mut Held,
    at_us: u64,
    in_front: Option<verbatim_model::request::LiveFront>,
) -> Vec<String> {
    read_as_a_test(
        crate::entries::route(
            &mut held.state,
            &mut held.runtime,
            (verbatim_model::TraceId::from_raw(1), at_us),
            &verbatim_model::request::QueueInput::Due { in_front },
        )
        .said(),
    )
}

/// The checks after a menu, popup or switcher end due by `at_us` take what
/// the shell's wake carries for them, Windows naming `front`'s window in
/// front (`crate::focus::menus::due`), for a test that then feeds the
/// wake's tick itself.
pub(crate) fn in_front_at(held: &mut Held, at_us: u64, front: &Foreground) {
    let in_front =
        verbatim_model::request::LiveFront::Window(verbatim_model::request::FrontWindow {
            window: front.window,
            root_owner: front.root_owner,
            process: front.process,
            thread: front.thread,
            class: String::new(),
        });
    crate::focus::menus::due(&mut held.state, at_us, &in_front);
}

/// `effects` as a test reads them ([`feed_at`]).
fn read_as_a_test(effects: Vec<Said>) -> Vec<String> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { to, ask }) => {
                Some(format!("send {:?} {}", to.reader, kind(&ask)))
            }
            Said::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect()
}

/// The requests sent at `at_us` by `input`, by number, and what it said.
pub(crate) fn sent_at(held: &mut Held, at_us: u64, input: Input<Entry>) -> (Vec<u64>, Vec<String>) {
    let effects = feed(&mut held.state, &mut held.runtime, Timed { at_us, input }).unwrap();
    let numbers = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. }) => Some(ask.number.0),
            _ => None,
        })
        .collect();
    let said = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Said::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                    .map(|segment| format!("{:?}", segment.content))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect();
    (numbers, said)
}

/// A UI Automation focus told by the listener at `at_us` as `tell`, its
/// listener read answered with [`listener_answer`] and its completion
/// answered with `read`: what the completion's answer said.
pub(crate) fn uia_focus_read(
    held: &mut Held,
    at_us: u64,
    tell: Event,
    read: FocusRead,
) -> Vec<String> {
    let (asked, _) = sent_at(held, at_us, entry(tell));
    let [object] = asked[..] else {
        panic!("one listener read asked: {asked:?}");
    };
    let (asked, _) = found_sent(held, at_us + 1, (LISTENER, object), listener_answer());
    let [completion] = asked[..] else {
        panic!("one completion asked: {asked:?}");
    };
    found_sent(held, at_us + 2, (OUTPOST, completion), Found::Read(read)).1
}

pub(crate) fn kind(ask: &Ask) -> &'static str {
    match &ask.request {
        Question::Batch(batch) => batch_kind(batch),
        Question::Terminal(_) => "other",
    }
}

/// What a batch reads, as the tests name it: a candidate's read, a read of
/// the current focus (with its focused state as a fact for the check after
/// a menu end), an MSAA object's read for a change or an alert, the
/// listener's read of a kept element, and the outpost's read of the route
/// that walk made.
pub(crate) fn batch_kind(batch: &Batch) -> &'static str {
    let subjects: Vec<&Subject> = batch
        .steps
        .iter()
        .filter_map(|step| step.op.subject())
        .collect();
    if batch.reads_focus_if_focused() {
        "CurrentFocus with the focused state as a fact"
    } else if batch.reads_focus_first() {
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
        && !(set.is_empty() || set.as_slice() == [PropertyId::Role])
    {
        // A show's or the desktop name's read of its MSAA object, where a
        // menu start's reads its role and a foreground's nothing.
        "ReadObject"
    } else if subjects
        .iter()
        .any(|subject| matches!(subject, Subject::Msaa { .. }))
    {
        "Candidate"
    } else if subjects
        .iter()
        .any(|subject| matches!(subject, Subject::Kept { .. }))
    {
        "ReadObject"
    } else if subjects
        .iter()
        .any(|subject| matches!(subject, Subject::Route(_)))
    {
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

/// The answer of `from` to its request `number`, the batch Core sent,
/// made by the stand-in reader from what it found.
#[expect(
    clippy::needless_pass_by_value,
    reason = "its callers build what was found in place"
)]
pub(crate) fn found(held: &Held, from: Instance, number: u64, found: Found) -> Input<Entry> {
    let batch = held
        .queue
        .batch_for_tests(number)
        .unwrap_or_else(|| panic!("request {number} is a batch out"));
    answer_outcome(from, number, crate::readers::outcome(&batch, &found))
}

/// [`found`] fed at `at_us`: what the step sent and said.
pub(crate) fn found_at(
    held: &mut Held,
    at_us: u64,
    (from, number): (Instance, u64),
    reader_found: Found,
) -> Vec<String> {
    let input = found(held, from, number, reader_found);
    feed_at(held, at_us, input)
}

/// [`found`] fed at `at_us`: the requests it sent, by number, and what it
/// said.
pub(crate) fn found_sent(
    held: &mut Held,
    at_us: u64,
    (from, number): (Instance, u64),
    reader_found: Found,
) -> (Vec<u64>, Vec<String>) {
    let input = found(held, from, number, reader_found);
    sent_at(held, at_us, input)
}

/// A batch's answer of one step done with `done`.
pub(crate) fn one(done: verbatim_model::batch::Done) -> Reply {
    Reply::Steps(vec![verbatim_model::batch::StepOutcome::Done(done)])
}

/// A one-step batch's answer: `values` read.
pub(crate) fn properties(values: PropertyValues) -> Reply {
    one(verbatim_model::batch::Done::Read(NodeRead {
        node: None,
        window: crate::readers::no_window(),
        values,
    }))
}

/// A one-step batch's answer: `reached` one step away, or nothing at an
/// edge.
pub(crate) fn related(reached: Option<NodeRead>) -> Reply {
    one(verbatim_model::batch::Done::Related(
        reached.into_iter().collect(),
    ))
}

/// A one-step batch's answer: activated with `action`.
pub(crate) fn activated(action: Option<verbatim_model::ActionName>) -> Reply {
    one(verbatim_model::batch::Done::Activate(action))
}

/// A one-step batch's answer: followed.
pub(crate) fn followed() -> Reply {
    one(verbatim_model::batch::Done::Follow)
}

/// The one operation of the newest batch waiting or out.
pub(crate) fn last_op(held: &Held) -> Option<Op> {
    let (_, batch) = held.queue.batches_for_tests().pop()?;
    let [step] = batch.steps.as_slice() else {
        panic!("a one-step batch: {batch:?}");
    };
    Some(step.op.clone())
}

pub(crate) fn answer(from: Instance, number: u64, reply: Reply) -> Input<Entry> {
    answer_outcome(from, number, WireOutcome::Result(reply))
}

pub(crate) fn answer_outcome(from: Instance, number: u64, outcome: WireOutcome) -> Input<Entry> {
    Input::Answer {
        from,
        answer: Answer {
            number: RequestNumber(number),
            outcome,
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// `instance` made ready at `at_us`: its start set up as the owner thread
/// reports it, then its `Ready`; what the step said.
pub(crate) fn ready_at(held: &mut Held, at_us: u64, instance: Instance) -> Vec<String> {
    held.state.queue.start_for_tests(instance);
    feed_at(held, at_us, Input::ReaderReady(instance))
}

pub(crate) fn ready() -> Held {
    let mut state = Held::default();
    state.queue.start_for_tests(LISTENER);
    state.queue.start_for_tests(OUTPOST);
    assert_eq!(
        feed_at(&mut state, 0, Input::ReaderReady(LISTENER)),
        NOTHING
    );
    assert_eq!(feed_at(&mut state, 0, Input::ReaderReady(OUTPOST)), NOTHING);
    state
}

pub(crate) fn entry(tell: Event) -> Input<Entry> {
    Input::Entry(Entry::of_tell(LISTENER, tell, false).unwrap())
}

/// Nothing sent or said.
pub(crate) const NOTHING: [&str; 0] = [];

/// The window's title, then the focus.
pub(crate) const SPOKEN: [&str; 2] = [
    "Label(\"Editor\") Role(Window)",
    "Label(\"OK\") Role(Button)",
];

/// [`SPOKEN`], then the `Follow` naming the UI Automation focus.
pub(crate) const SPOKEN_FOLLOWED: [&str; 3] = [
    "Label(\"Editor\") Role(Window)",
    "Label(\"OK\") Role(Button)",
    "send Outpost(Pid(40)) Follow",
];

/// A UI Automation focus asked of the listener, then completed by the
/// outpost, up to the completion's answer.
pub(crate) fn asked_completion() -> Held {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(uia_tell(1, Vec::new()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        found_at(&mut state, 2_000, (LISTENER, 1), listener_answer()),
        ["send Outpost(Pid(40)) Completion"]
    );
    state
}

#[test]
fn a_uia_focus_is_read_in_two_requests_and_executed() {
    let mut state = asked_completion();
    assert_eq!(
        found_at(
            &mut state,
            3_000,
            (OUTPOST, 2),
            Found::Read(focus_read(None))
        ),
        SPOKEN_FOLLOWED
    );
    assert_eq!(state.versions.focus().unwrap().context.snapshot.id, node(2));
    let Committed::Read(focus) = &state.versions.committed else {
        panic!("a reader's focus is committed");
    };
    assert_eq!(focus.context.snapshot.id, node(2));
    assert_eq!(
        focus.context.snapshot.id.outpost(),
        Some(OutpostId(GENERATION))
    );
    assert_eq!(
        state.versions.last.as_ref().map(|last| &last.read_by),
        Some(&ReadBy::Uia {
            runtime_id: RuntimeId(vec![42, 1]),
            token: Some(ElementToken(9)),
        })
    );
    assert_eq!(state.queue.front_for_tests(), None);
    // Only the commit's `Follow` is out, outside the queue.
    assert_eq!(
        state
            .queue
            .batches_for_tests()
            .into_iter()
            .map(|(_, batch)| batch.steps[0].op.clone())
            .collect::<Vec<_>>(),
        [Op::Follow(Some(verbatim_model::batch::FollowTarget {
            subject: Subject::Node(node(2)),
            group: verbatim_model::request::FollowGroup::Base,
            extra: None,
        }))]
    );
}

#[test]
fn a_uia_duplicate_of_the_readers_last_focus_ends_with_no_read() {
    let mut state = asked_completion();
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    let repeat = uia_tell(1, vec![(ElementToken(9), Read::Answered(true))]);
    assert_eq!(feed_at(&mut state, 4_000, entry(repeat)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_uia_focus_without_the_keyboard_focus_ends_with_no_read() {
    let mut state = ready();
    let mut tell = uia_tell(1, Vec::new());
    if let Some(checked) = tell.checked.as_mut() {
        checked.keyboard_focus = Some(Read::Answered(false));
    }
    assert_eq!(feed_at(&mut state, 1_000, entry(tell)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_refused_completion_ends_the_focus_unspoken() {
    let mut state = asked_completion();
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer_outcome(OUTPOST, 2, WireOutcome::Refused(Refusal::ReadThroughMsaa))
        ),
        // Naming nothing, which removes the completion's registration.
        ["send Outpost(Pid(40)) Follow"]
    );
    assert!(state.versions.focus().is_none());
    assert_eq!(state.versions.committed, Committed::Nothing);
    assert!(state.queue.front_for_tests().is_none());
}

/// The route and the known ancestors of the completion asked at `at_us`
/// by `input`, as the outpost receives them.
fn sent_completion(
    held: &mut Held,
    at_us: u64,
    input: Input<Entry>,
) -> Option<(Route, Vec<KnownAncestor>)> {
    feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. }) => match ask.request {
                Question::Batch(batch) => batch.steps.into_iter().find_map(|step| match step.op {
                    Op::FocusRead {
                        subject: Subject::Route(route),
                        known,
                        ..
                    } => Some((*route, known)),
                    _ => None,
                }),
                Question::Terminal(_) => None,
            },
            _ => None,
        })
}

/// The listener's answer to its walk `number`, [`listener_answer`] with
/// its walk meeting `met`.
fn walk_meeting(held: &Held, number: u64, met: &KnownAncestor) -> Input<Entry> {
    let element = walked_element("OK", Role::Button, vec![42, 1]);
    walk_answer(held, number, element, Some(met))
}

/// A second focus, of runtime id `[42, 2]`, asked of the listener after
/// the focus "OK" was committed with its frame "Editor" known by the
/// runtime id `[42, 4]`.
fn second_focus_asked() -> Held {
    let mut state = asked_completion();
    let mut first = focus_read(None);
    first.ancestors[0].node.runtime_id = Some(RuntimeId(vec![42, 4]));
    found_at(&mut state, 3_000, (OUTPOST, 2), Found::Read(first));
    assert_eq!(
        feed_at(&mut state, 4_000, entry(uia_tell(2, Vec::new()))),
        ["send Listener ReadObject"]
    );
    state
}

#[test]
fn the_known_ancestor_a_listeners_walk_met_is_named_in_the_completion() {
    let mut state = second_focus_asked();
    let met = KnownAncestor::Uia(RuntimeId(vec![42, 4]));
    let input = walk_meeting(&state, 4, &met);
    let (route, known) = sent_completion(&mut state, 5_000, input).unwrap();
    assert_eq!(route.end, RouteEnd::Known(met.clone()));
    assert!(known.contains(&met));
}

#[test]
fn a_focus_inside_the_committed_focus_meets_it_and_says_only_itself() {
    // NVDA's walk meets its focus object as it meets the focus's
    // ancestors (`docs/parity/focus-context.md`), so the committed focus
    // is known first, and a focus inside it says no container again.
    let mut state = second_focus_asked();
    let met = KnownAncestor::Uia(RuntimeId(vec![42, 1]));
    let input = walk_meeting(&state, 4, &met);
    let (_, known) = sent_completion(&mut state, 5_000, input).unwrap();
    let place = known.iter().position(|known| *known == met);
    let mut inner = focus_read(None);
    inner.focus.read.node = Some(node(3));
    inner.focus.read.values = values("Inner", Role::Button);
    inner.focus.runtime_id = Some(RuntimeId(vec![42, 2]));
    inner.ancestors = Vec::new();
    inner.met = Some(u32::try_from(place.unwrap()).unwrap());
    assert_eq!(
        found_at(&mut state, 6_000, (OUTPOST, 5), Found::Read(inner)),
        [
            "Label(\"Inner\") Role(Button)",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    let committed = state.versions.live().unwrap();
    assert_eq!(
        committed
            .line
            .iter()
            .rev()
            .map(|ancestor| ancestor.snapshot.id)
            .collect::<Vec<_>>(),
        [node(2), node(1)]
    );
}

/// A walk that met an ancestor no longer in the committed focus's line
/// cannot be completed against it, and the listener let its element go,
/// so the application's focus is read afresh and spoken: nothing is lost.
#[test]
fn a_walk_that_met_an_ancestor_core_no_longer_knows_reads_the_focus_afresh() {
    let mut state = second_focus_asked();
    let met = KnownAncestor::Uia(RuntimeId(vec![42, 4]));
    let input = walk_meeting(&state, 4, &met);
    // The committed focus's line changes between the two reads, so the
    // frame the walk met is no longer known.
    state.versions.committed = Committed::Nothing;
    assert_eq!(
        feed_at(&mut state, 5_000, input),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    let said = found_at(
        &mut state,
        6_000,
        (OUTPOST, 5),
        Found::Read(focus_read(None)),
    );
    assert!(
        said.contains(&"Label(\"OK\") Role(Button)".to_owned()),
        "{said:?}"
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_clone_of_the_state_rebuilds_the_paused_function_and_goes_on_alike() {
    let mut state = asked_completion();
    let mut cloned = state.clone();
    let reply = found(&state, OUTPOST, 2, Found::Read(focus_read(None)));
    let straight = feed_at(&mut state, 3_000, reply.clone());
    assert_eq!(feed_at(&mut cloned, 3_000, reply), straight);
    assert_eq!(cloned, state);
    assert_eq!(format!("{:?}", cloned.state), format!("{:?}", state.state));
}

#[test]
fn a_dump_of_the_state_rebuilds_the_paused_function_and_goes_on_alike() {
    let mut state = asked_completion();
    let json = serde_json::to_string(&state.state).unwrap();
    let mut restored = Held {
        state: serde_json::from_str(&json).unwrap(),
        runtime: Runtime::default(),
    };
    assert_eq!(restored, state);
    let reply = found(&state, OUTPOST, 2, Found::Read(focus_read(None)));
    let straight = feed_at(&mut state, 3_000, reply.clone());
    assert_eq!(feed_at(&mut restored, 3_000, reply), SPOKEN_FOLLOWED);
    assert_eq!(straight, SPOKEN_FOLLOWED);
    assert_eq!(
        format!("{:?}", restored.state),
        format!("{:?}", state.state)
    );
}

#[test]
fn a_checkpoint_that_does_not_replay_is_an_error_and_leaves_the_state() {
    let mut cloned = asked_completion().clone();
    if let Some(checkpoint) = cloned.queue.checkpoint_for_tests() {
        checkpoint.asked_for_tests().clear();
    }
    let before = cloned.clone();
    let input = found(&cloned, OUTPOST, 2, Found::Read(focus_read(None)));
    assert!(
        feed(
            &mut cloned.state,
            &mut cloned.runtime,
            Timed {
                at_us: 3_000,
                input,
            }
        )
        .is_err()
    );
    assert_eq!(cloned, before);
}

/// The run's held ids for the candidate of object `object`.
pub(crate) fn held(object: i32) -> HeldIds {
    HeldIds {
        event: MsaaIds {
            window: WindowHandle(5),
            object,
            child: 0,
        },
        held_child: 0,
    }
}

/// The object of the one candidate the MSAA batch out names, and whether
/// the batch reads it in full only when it is focused.
fn asked_candidate(state: &Held) -> Option<(i32, bool)> {
    state
        .queue
        .batches_for_tests()
        .into_iter()
        .find_map(|(_, batch)| {
            let object = batch
                .steps
                .iter()
                .find_map(|step| match step.op.subject() {
                    Some(Subject::Msaa { ids, .. }) => Some(ids.object),
                    _ => None,
                })?;
            let if_focused = batch.steps.iter().any(|step| {
                matches!(&step.op, Op::Test { subject: Subject::Msaa { .. }, matcher }
                    if matcher.properties().contains(&PropertyId::FocusedWithin))
            });
            Some((object, if_focused))
        })
}

#[test]
fn a_run_asks_one_candidate_at_a_time_newest_first_and_executes_the_valid_focus() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(asked_candidate(&state), Some((-4, true)));
    // The first candidate is asked alone; two more join a run behind it.
    feed_at(&mut state, 1_100, entry(msaa_tell(-5)));
    feed_at(&mut state, 1_200, entry(msaa_tell(-6)));
    assert_eq!(
        found_at(&mut state, 2_000, (OUTPOST, 1), Found::NotThere),
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(asked_candidate(&state), Some((-6, true)));
    assert_eq!(
        found_at(&mut state, 2_500, (OUTPOST, 2), Found::Unfocused),
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(asked_candidate(&state), Some((-5, true)));
    let tried = Found::Read(focus_read(Some(held(-5))));
    assert_eq!(found_at(&mut state, 3_000, (OUTPOST, 3), tried), SPOKEN);
    assert_eq!(
        state.versions.last.as_ref().map(|last| &last.read_by),
        Some(&ReadBy::Msaa {
            event: Some(held(-5).event),
            held_child: 0,
        })
    );
}

#[test]
fn an_msaa_duplicate_of_the_readers_last_focus_ends_the_run_with_no_read() {
    let mut state = ready();
    state.versions.last = Some(LastFocus {
        read_by: ReadBy::Msaa {
            event: Some(held(-4).event),
            held_child: 0,
        },
        place: crate::focus::versions::Place {
            window: WindowHandle(5),
            top_level: WindowHandle(4),
            root_owner: WindowHandle(4),
            ancestor_windows: Vec::new().into(),
        },
        role: verbatim_model::Role::Button,
        since: None,
    });
    assert_eq!(feed_at(&mut state, 1_000, entry(msaa_tell(-4))), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// File Explorer raises a second focus on the desktop's item it focused,
/// child 3 of its list, as it does when the desktop comes back from
/// another application (NVDA's #2988) or a program starts from it
/// (`text_box_caret_keys`' artifacts): the first is read and spoken, and
/// the second, with the same ids and no key between, is asked unless it
/// is that item, which its outpost answers it is: said once, by the
/// identity check, with no rule of File Explorer's own (Dickson,
/// 2026-11-18).
#[test]
fn a_second_focus_on_the_desktops_item_with_no_key_between_is_said_once() {
    let mut state = ready();
    let item = |state: &mut Held, at_us| {
        let mut tell = msaa_tell(-4);
        tell.object = EventObject::Msaa {
            window: WindowHandle(5),
            object: -4,
            child: 3,
        };
        feed_at(state, at_us, entry(tell))
    };
    assert_eq!(item(&mut state, 1_000), ["send Outpost(Pid(40)) Candidate"]);
    let read = focus_read(Some(HeldIds {
        event: MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 3,
        },
        held_child: 3,
    }));
    let tried = Found::Read(read);
    assert_eq!(found_at(&mut state, 2_000, (OUTPOST, 1), tried), SPOKEN);
    assert_eq!(item(&mut state, 3_000), ["send Outpost(Pid(40)) Candidate"]);
    assert_eq!(
        found_at(&mut state, 4_000, (OUTPOST, 2), Found::Same),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// A foreground tell of window 4, whose window facts found `front` in
/// front as the listener placed it.
fn foreground_tell(front: u64) -> Event {
    let mut tell = msaa_tell(0);
    tell.kind = EventKind::Foreground;
    tell.object = EventObject::Msaa {
        window: WindowHandle(4),
        object: 0,
        child: 0,
    };
    if let TellWindow::Found(facts) = &mut tell.window {
        facts.window = facet(4, "Frame");
        facts.window_to_foreground = Relation::Equal;
        facts.foreground = WindowHandle(front);
        facts.foreground_root_owner = WindowHandle(front);
    }
    tell
}

/// The listener's report that window 4 came to the foreground.
fn window_4_in_front() -> Input<Entry> {
    Input::Foreground(Foreground {
        window: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: APP,
        thread: 41,
        by_class: None,
        ..Foreground::BLANK
    })
}

/// A foreground tell is judged against the foreground record, as NVDA
/// reads the window in front as it processes the event
/// (`docs/parity/focus-events.md`): in the listener's order,
/// the report that window 4 came to the foreground, then its tell, the
/// tell is asked (`msaa_flood_then_focus`, `background_progress_bar`).
#[test]
fn a_foreground_tell_after_the_report_of_its_window_is_asked() {
    let mut state = ready();
    feed_at(&mut state, 500, window_4_in_front());
    feed_at(
        &mut state,
        600,
        answer_outcome(OUTPOST, 1, WireOutcome::NotThere),
    );
    assert_eq!(
        feed_at(&mut state, 1_000, entry(foreground_tell(4))),
        ["send Outpost(Pid(40)) Candidate"]
    );
}

/// A foreground tell that reaches Core ahead of the report of its window
/// is refused with no read, which is why the listener sends the report
/// first.
#[test]
fn a_foreground_tell_ahead_of_the_report_of_its_window_is_refused() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(foreground_tell(4))),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_candidate_refused_by_acceptance_is_not_asked() {
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    if let TellWindow::Found(facts) = &mut tell.window {
        facts.window_to_foreground = Relation::Outside;
        facts.root_owner_to_foreground = Relation::Outside;
        facts.root_owner_is_foregrounds = false;
        facts.window_to_active = Relation::Outside;
    }
    assert_eq!(feed_at(&mut state, 1_000, entry(tell)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// A value change of the focus from a background window, waiting behind
/// the focus's completion, is judged with "report background progress
/// bars" as it stood at its arrival (`docs/parity/focus-events.md`, "Event
/// acceptance"): refused when off then and turned on before its turn,
/// spoken when on then and turned off since. Derived from NVDA's pinned
/// source, which judges acceptance as an event is queued: capture C5
/// cannot be made, its window being about a millisecond (Dickson's ruling
/// of 2026-11-20 for captures that cannot be made).
#[test]
fn a_background_value_change_is_judged_with_the_setting_at_its_arrival() {
    for (on_at_arrival, said) in [(false, None), (true, Some("Value(\"1:23\")"))] {
        let mut state = asked_completion();
        state.settings.report_background_progress_bars = on_at_arrival;
        let mut tell = msaa_tell(-4);
        tell.kind = EventKind::UiaPropertyChanged;
        tell.object = EventObject::Followed {
            runtime_id: RuntimeId(vec![42, 1]),
        };
        let value = Some(PropertyValue::Text("1:23".to_owned()));
        tell.payload = Payload::PropertyChanged {
            property: PropertyId::Value,
            value,
        };
        if let TellWindow::Found(facts) = &mut tell.window {
            (facts.window_to_foreground, facts.root_owner_to_foreground) =
                (Relation::Outside, Relation::Outside);
            facts.root_owner_is_foregrounds = false;
        }
        let input = QueueInput::Tell {
            from: OUTPOST,
            tell: Box::new(tell),
        };
        let queued = verbatim_model::Input::Queue {
            trace_id: TraceId::from_raw(5),
            at_us: 2_500,
            input,
        };
        let _ = crate::reduce(&mut state.state, &mut state.runtime, &queued);
        state.settings.report_background_progress_bars = !on_at_arrival;
        let read = found(&state, OUTPOST, 2, Found::Read(focus_read(None)));
        let spoken: Vec<_> = SPOKEN_FOLLOWED.into_iter().chain(said).collect();
        assert_eq!(feed_at(&mut state, 3_000, read), spoken, "{on_at_arrival}");
    }
}

#[test]
fn a_foreground_marker_empties_the_last_focus_for_an_unrelated_window() {
    let mut state = asked_completion();
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    assert!(state.versions.last.is_some());
    let foreground = Foreground {
        window: WindowHandle(70),
        root_owner: WindowHandle(70),
        process: Pid(41),
        thread: 0,
        by_class: None,
        ..Foreground::BLANK
    };
    // The first foreground report appends the startup read, of an
    // application whose outpost has not started; the marker waits behind
    // it until Core stops waiting for it.
    assert_eq!(
        feed_at(&mut state, 4_000, Input::Foreground(foreground)),
        NOTHING
    );
    assert!(state.versions.last.is_some());
    assert_eq!(feed_at(&mut state, 13_004_000, Input::Tick), NOTHING);
    assert_eq!(state.versions.last, None);
}

// The rules judged at a run's turn and at execution.

/// The item a list's focus was redirected to raises its own focus with no
/// key between: read unless it is that item by its ids and its reading,
/// which its outpost answers it is, so it says nothing (the identity check
/// in the duplicate test, `docs/parity/focus-events.md`, "Duplicate focus
/// suppression").
#[test]
fn the_item_a_redirect_reached_says_nothing_when_its_outpost_finds_it_the_same() {
    let mut state = ready();
    feed_at(&mut state, 1_000, entry(msaa_tell(-4)));
    // The list's focus, child 0, redirected to its item, child 2, whose
    // ids the answer names as its event's, as NVDA's redirected object.
    let child = MsaaIds {
        window: WindowHandle(5),
        object: -4,
        child: 2,
    };
    let read = focus_read(Some(HeldIds {
        event: child,
        held_child: 2,
    }));
    let tried = Found::Read(read);
    assert_eq!(found_at(&mut state, 2_000, (OUTPOST, 1), tried), SPOKEN);
    let mut tell = msaa_tell(-4);
    tell.object = EventObject::Msaa {
        window: child.window,
        object: child.object,
        child: child.child,
    };
    assert_eq!(
        feed_at(&mut state, 3_000, entry(tell)),
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(
        found_at(&mut state, 4_000, (OUTPOST, 2), Found::Same),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
    // The limiter never drops the item's events, by the ids NVDA's
    // redirected object carries, and caps the list's own.
    let key = |id| crate::event_queue::MsaaKey {
        kind: EventKind::MsaaFocus,
        window: child.window,
        object: child.object,
        child: id,
        thread: 41,
    };
    assert!(<Entries as crate::event_queue::Logic>::exempt_from_cap(
        &state,
        &key(2)
    ));
    assert!(!<Entries as crate::event_queue::Logic>::exempt_from_cap(
        &state,
        &key(0)
    ));
}

/// A list's focus that a redirect took to the child Core committed, whose
/// outpost reads it in full because it no longer reads as that child, is
/// spoken: the child's own focus event was read and spoken, then the list
/// raises focus on its own object with no key between, read unless it is
/// that child by its ids and its reading (`docs/parity/focus-events.md`,
/// "Duplicate focus suppression"; `docs/parity/windows-and-processes.md`,
/// "A control's own focus with a focused child").
#[test]
fn a_redirect_to_the_committed_focus_that_reads_differently_is_spoken() {
    let child = MsaaIds {
        window: WindowHandle(5),
        object: -4,
        child: 2,
    };
    let at_child = HeldIds {
        event: child,
        held_child: 2,
    };
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    tell.object = EventObject::Msaa {
        window: child.window,
        object: child.object,
        child: child.child,
    };
    feed_at(&mut state, 1_000, entry(tell));
    let tried = Found::Read(focus_read(Some(at_child)));
    assert_eq!(found_at(&mut state, 2_000, (OUTPOST, 1), tried), SPOKEN);
    assert_eq!(
        feed_at(&mut state, 3_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) Candidate"]
    );
    let tried = Found::Read(focus_read(Some(at_child)));
    assert_eq!(
        found_at(&mut state, 4_000, (OUTPOST, 2), tried),
        ["Label(\"OK\") Role(Button)"]
    );
}

#[test]
fn a_focus_on_the_hidden_frames_own_object_is_dropped() {
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    if let TellWindow::Found(facts) = &mut tell.window {
        facts.hidden_frame = true;
    }
    assert_eq!(feed_at(&mut state, 1_000, entry(tell)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_focus_in_a_window_the_hidden_frame_owns_takes_the_frame_as_its_foreground() {
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    // Verbatim's menu, window 6, owned by the frame, window 8, which is in
    // front.
    tell.window = TellWindow::Found(Box::new(AcceptanceFacts {
        window: facet(6, "#32768"),
        root: facet(6, "#32768"),
        root_owner: facet(8, "wxWindowNR"),
        foreground: WindowHandle(8),
        foreground_root_owner: WindowHandle(8),
        window_to_foreground: Relation::Outside,
        root_owner_to_foreground: Relation::Equal,
        hidden_frame_owned: true,
        ..facts("#32768")
    }));
    assert_eq!(
        feed_at(&mut state, 1_000, entry(tell)),
        ["send Outpost(Pid(40)) Candidate"]
    );
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = values("Settings...", Role::MenuItem);
    read.focus.read.window = win(6);
    read.ancestors.clear();
    let mut frame = values("Verbatim", Role::Window);
    frame.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(8))),
    });
    read.foreground = Some(FocusNode {
        read: NodeRead {
            node: Some(node(1)),
            window: win(8),
            values: frame,
        },
        runtime_id: None,
    });
    let tried = Found::Read(read);
    assert_eq!(
        found_at(&mut state, 2_000, (OUTPOST, 1), tried),
        ["Label(\"Verbatim\") Role(Window)", "Label(\"Settings...\")",]
    );
}

/// A dialog the hidden frame owns, in front and focused itself: the
/// answer reads it as its own foreground object, the same node, which is
/// not taken as its ancestor, so it is said once. NVDA: the focus is never
/// among its own ancestors (`docs/parity/focus-context.md`).
#[test]
fn a_focused_dialog_the_hidden_frame_owns_is_not_its_own_ancestor() {
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    // Verbatim's settings dialog, window 5, owned by the frame, window 8,
    // and in front.
    tell.window = TellWindow::Found(Box::new(AcceptanceFacts {
        window: facet(5, "#32770"),
        root: facet(5, "#32770"),
        root_owner: facet(8, "wxWindowNR"),
        foreground: WindowHandle(5),
        foreground_root_owner: WindowHandle(8),
        window_to_foreground: Relation::Equal,
        root_owner_to_foreground: Relation::Outside,
        root_owner_is_foregrounds: true,
        active_window: WindowHandle(5),
        window_to_active: Relation::Equal,
        hidden_frame_owned: true,
        ..facts("#32770")
    }));
    assert_eq!(
        feed_at(&mut state, 1_000, entry(tell)),
        ["send Outpost(Pid(40)) Candidate"]
    );
    let mut dialog = values("Settings", Role::Dialog);
    dialog.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(5))),
    });
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = dialog.clone();
    read.ancestors.clear();
    read.foreground = Some(FocusNode {
        read: NodeRead {
            node: Some(node(2)),
            window: win(5),
            values: dialog,
        },
        runtime_id: None,
    });
    let tried = Found::Read(read);
    assert_eq!(
        found_at(&mut state, 2_000, (OUTPOST, 1), tried),
        ["Label(\"Settings\") Role(Dialog)"]
    );
}

/// Verbatim's popup menu as Windows makes it: a window of its own with no
/// owner, outside the frame in front, marked by the listener as the
/// frame's (`verbatim_win32::window_facts`); the answer reads the frame as
/// its foreground, which the focus enters, saying its title.
#[test]
fn a_focus_in_the_hidden_frames_popup_menu_with_no_owner_says_the_frames_title() {
    let mut state = ready();
    let mut tell = msaa_tell(-4);
    tell.window = TellWindow::Found(Box::new(AcceptanceFacts {
        window: facet(6, "#32768"),
        root: facet(6, "#32768"),
        root_owner: facet(6, "#32768"),
        foreground: WindowHandle(8),
        foreground_root_owner: WindowHandle(8),
        window_to_foreground: Relation::Outside,
        root_owner_to_foreground: Relation::Outside,
        root_owner_is_foregrounds: false,
        window_to_active: Relation::Outside,
        root_topmost: true,
        window_topmost: true,
        hidden_frame_owned: true,
        ..facts("#32768")
    }));
    assert_eq!(
        feed_at(&mut state, 1_000, entry(tell)),
        ["send Outpost(Pid(40)) Candidate"]
    );
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = values("Context", Role::Menu);
    read.focus.read.window = win(6);
    read.ancestors.clear();
    let mut frame = values("Verbatim", Role::Pane);
    frame.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(8))),
    });
    read.foreground = Some(FocusNode {
        read: NodeRead {
            node: Some(node(1)),
            window: win(8),
            values: frame,
        },
        runtime_id: None,
    });
    let tried = Found::Read(read);
    assert_eq!(
        found_at(&mut state, 2_000, (OUTPOST, 1), tried),
        ["Label(\"Verbatim\")", "Label(\"Context\") Role(Menu)"]
    );
}

/// A focus tell of child `child` of Verbatim's popup menu, as
/// [`a_focus_in_the_hidden_frames_popup_menu_with_no_owner_says_the_frames_title`]
/// makes it.
fn frame_menu_tell(child: i32) -> Event {
    let mut tell = msaa_tell(-4);
    tell.object = EventObject::Msaa {
        window: WindowHandle(6),
        object: -4,
        child,
    };
    tell.window = TellWindow::Found(Box::new(AcceptanceFacts {
        window: facet(6, "#32768"),
        root: facet(6, "#32768"),
        root_owner: facet(6, "#32768"),
        foreground: WindowHandle(8),
        foreground_root_owner: WindowHandle(8),
        window_to_foreground: Relation::Outside,
        root_owner_to_foreground: Relation::Outside,
        root_owner_is_foregrounds: false,
        window_to_active: Relation::Outside,
        root_topmost: true,
        window_topmost: true,
        hidden_frame_owned: true,
        ..facts("#32768")
    }));
    tell
}

/// A focus answer in Verbatim's popup menu: node `number` named `name`
/// with role `role`, read by child `child`, and the hidden frame, titled
/// "Verbatim", as its foreground object when `frame`.
fn frame_menu_read(number: u64, name: &str, role: Role, child: i32, frame: bool) -> FocusRead {
    let mut read = focus_read(Some(HeldIds {
        event: MsaaIds {
            window: WindowHandle(6),
            object: -4,
            child,
        },
        held_child: child,
    }));
    read.focus.read.node = Some(node(number));
    read.focus.read.values = values(name, role);
    read.focus.read.window = win(6);
    read.ancestors.clear();
    let mut values = values("Verbatim", Role::Pane);
    values.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(8))),
    });
    read.foreground = frame.then(|| FocusNode {
        read: NodeRead {
            node: Some(node(1)),
            window: win(8),
            values,
        },
        runtime_id: None,
    });
    read
}

/// Down Arrow twice in Verbatim's popup menu (`menu_and_settings_dialog`'s
/// artifacts): the first item meets the menu and reads no foreground; the
/// frame's foreground report then arrives, which leaves the focus in the
/// menu, the frame being its outermost ancestor; the second item, its
/// walk meeting nothing Core knows, reads the menu and the frame again.
/// The frame's title is said only on entering the menu.
#[test]
fn a_focus_moving_in_the_hidden_frames_popup_menu_says_the_frames_title_once() {
    let mut state = ready();
    let frame = Foreground {
        window: WindowHandle(8),
        root_owner: WindowHandle(8),
        process: APP,
        thread: 41,
        by_class: None,
        ..Foreground::BLANK
    };
    // The first foreground report's startup read, which finds nothing.
    assert_eq!(
        feed_at(&mut state, 500, Input::Foreground(frame)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    feed_at(
        &mut state,
        600,
        answer_outcome(OUTPOST, 1, WireOutcome::NotThere),
    );
    feed_at(&mut state, 1_000, entry(frame_menu_tell(0)));
    let menu = frame_menu_read(3, "Context", Role::Menu, 0, true);
    let tried = |read| Found::Read(read);
    assert_eq!(
        found_at(&mut state, 2_000, (OUTPOST, 2), tried(menu)),
        ["Label(\"Verbatim\")", "Label(\"Context\") Role(Menu)"]
    );
    feed_at(&mut state, 3_000, entry(frame_menu_tell(1)));
    let mut settings = frame_menu_read(5, "Settings...", Role::MenuItem, 1, false);
    settings.met = Some(0);
    assert_eq!(
        found_at(&mut state, 4_000, (OUTPOST, 3), tried(settings)),
        ["Label(\"Settings...\")"]
    );
    // The frame's foreground report comes again, after the item's focus.
    assert_eq!(
        feed_at(&mut state, 4_500, Input::Foreground(frame)),
        NOTHING
    );
    assert!(state.versions.last.is_some());
    feed_at(&mut state, 5_000, entry(frame_menu_tell(3)));
    let mut exit = frame_menu_read(6, "Exit", Role::MenuItem, 3, true);
    exit.ancestors.push(FocusAncestor {
        node: frame_menu_read(3, "Context", Role::Menu, 0, false).focus,
        simple_child: false,
        uia_layout: false,
        described: false,
    });
    assert_eq!(
        found_at(&mut state, 6_000, (OUTPOST, 4), tried(exit)),
        ["Label(\"Exit\")"]
    );
}

#[test]
fn a_focus_whose_top_level_window_is_an_ignored_processs_is_dropped_at_its_answer() {
    let mut state = asked_completion();
    state.ignored = vec![APP];
    assert_eq!(
        found_at(
            &mut state,
            3_000,
            (OUTPOST, 2),
            Found::Read(focus_read(None))
        ),
        // Not executed: naming nothing removes the completion's
        // registration.
        ["send Outpost(Pid(40)) Follow"]
    );
    assert_eq!(state.versions.last, None);
    assert!(state.versions.focus().is_none());
}

#[test]
fn an_executed_focus_runs_under_its_tells_trace_and_takes_its_top_ancestor_as_foreground() {
    let mut state = asked_completion();
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    let focus = state.versions.focus().unwrap();
    assert_eq!(
        focus.context.trace_id,
        Some(verbatim_model::TraceId::from_raw(7))
    );
    // The walk met no ancestor Core knew and the answer read no
    // foreground object: the top ancestor, the frame, stands for it.
    let frame = &focus.line.first().unwrap().snapshot;
    assert_eq!(frame.id, node(1));
    assert_eq!(
        state.foreground.object(),
        Some(verbatim_model::ObjectKey::of(frame))
    );
}

#[test]
fn the_reducer_routes_a_tell_into_the_queue_and_asks_to_be_woken_at_its_due_time() {
    use verbatim_model::request::{QueueEffect, QueueInput};
    let mut held = Held::default();
    let queued = |input| verbatim_model::Input::Queue {
        trace_id: verbatim_model::TraceId::from_raw(5),
        at_us: 1_000,
        input,
    };
    held.state.queue.start_for_tests(LISTENER);
    held.state.queue.start_for_tests(OUTPOST);
    let effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &queued(QueueInput::ReaderReady(OUTPOST)),
    );
    // A ready outpost times no wake of its own: it is never ended for
    // being idle, and this one's liveness is due at the clock's end.
    assert_eq!(effects, [Said::Queue(QueueEffect::WakeAt(None))]);
    let effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &queued(QueueInput::Tell {
            from: LISTENER,
            tell: Box::new(msaa_tell(-4)),
        }),
    );
    let [
        Said::Queue(QueueEffect::Send { to, ask }),
        Said::Queue(QueueEffect::WakeAt(wake)),
    ] = effects.as_slice()
    else {
        panic!("one request and the wake: {effects:?}");
    };
    assert_eq!((*to, kind(ask)), (OUTPOST, "Candidate"));
    assert_eq!(*wake, Some(1_000 + crate::event_queue::HANG_ROUND_US));
}

#[test]
fn a_failed_focus_read_clears_cores_caret_baseline() {
    let mut state = asked_completion();
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    state.versions.focus_mut().unwrap().context.caret = Some(crate::state::CaretContext {
        line: std::sync::Arc::new(verbatim_model::TextChunk {
            unit: verbatim_model::TextUnit::Line,
            text: "text".to_owned(),
            start: verbatim_model::TextAnchor(1),
            offset: 0,
            languages: Vec::new(),
            first: true,
            last: true,
            truncated: false,
            formats: Vec::new(),
        }),
        selection: None,
    });
    feed_at(&mut state, 4_000, entry(uia_tell(2, Vec::new())));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer_outcome(LISTENER, 4, WireOutcome::NotThere)
        ),
        NOTHING
    );
    assert_eq!(state.versions.focus().unwrap().context.caret, None);
}

/// A focus on one of the shell's staging windows is taken like any other,
/// so the focus and the navigator move to it, but nothing is said for it
/// (`docs/parity/windows-and-processes.md`, "The shell's staging
/// windows").
#[test]
fn a_focus_on_a_shell_staging_window_is_taken_and_says_nothing() {
    for class in [
        "ForegroundStaging",
        "LauncherTipWnd",
        "ApplicationManager_DesktopShellWindow",
    ] {
        let mut state = ready();
        let mut tell = uia_tell(1, Vec::new());
        tell.window = TellWindow::Found(Box::new(facts(class)));
        assert_eq!(
            feed_at(&mut state, 1_000, entry(tell)),
            ["send Listener ReadObject"]
        );
        found_at(&mut state, 2_000, (LISTENER, 1), listener_answer());
        let mut read = focus_read(None);
        read.focus.read.values = values("Staging", Role::Pane);
        assert_eq!(
            found_at(&mut state, 3_000, (OUTPOST, 2), Found::Read(read)),
            ["send Outpost(Pid(40)) Follow"],
            "{class}"
        );
        assert_eq!(state.versions.focus().unwrap().context.snapshot.id, node(2));
        assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
    }
}

/// The reducer fed `input` from the shell at `at_us`, its effects but the
/// wake.
fn shell_feeds(
    held: &mut Held,
    at_us: u64,
    input: verbatim_model::request::QueueInput,
) -> Vec<Said> {
    let mut effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &verbatim_model::Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(5),
            at_us,
            input,
        },
    );
    effects.retain(|effect| {
        !matches!(
            effect,
            Said::Queue(verbatim_model::request::QueueEffect::WakeAt(_))
        )
    });
    effects
}

/// Explorer's taskbar, `Shell_TrayWnd`, window 90 of process 41's outpost.
fn taskbar() -> verbatim_model::request::NodeWindow {
    verbatim_model::request::NodeWindow {
        handle: WindowHandle(90),
        top_level: WindowHandle(90),
        root_owner: WindowHandle(90),
        process: Pid(41),
        thread: 7,
        desktop: false,
    }
}

/// Explorer's outpost.
const EXPLORER: Instance = Instance {
    reader: Reader::Outpost(Pid(41)),
    generation: 9,
};

/// A read from outside the queue of the taskbar's own object, as the
/// notification area list's reads are batches outside it, naming
/// `Shell_TrayWnd`.
fn taskbar_batch() -> Batch {
    Batch {
        steps: vec![verbatim_model::batch::Step {
            op: Op::Read {
                subject: Subject::Window {
                    which: verbatim_model::batch::WindowOf::Handle(WindowHandle(90)),
                    backend: verbatim_model::Backend::Uia,
                    visible: false,
                },
                set: vec![PropertyId::Name],
            },
            when: None,
        }],
        placement: verbatim_model::batch::Placement::Outside,
        spare: None,
    }
}

/// [`taskbar_batch`] asked under `asker`.
fn taskbar_read(asker: u64) -> verbatim_model::request::QueueInput {
    verbatim_model::request::QueueInput::AskOutside {
        asker,
        window: taskbar(),
        request: taskbar_batch().into(),
    }
}

/// [`taskbar_read`] asked under asker 5 with explorer's outpost ready,
/// sent as request 1.
fn taskbar_read_out() -> Held {
    use verbatim_model::request::QueueEffect;
    let mut held = Held::default();
    held.state.queue.start_for_tests(EXPLORER);
    let _ = shell_feeds(
        &mut held,
        1_000,
        verbatim_model::request::QueueInput::ReaderReady(EXPLORER),
    );
    let effects = shell_feeds(&mut held, 2_000, taskbar_read(5));
    let [Said::Queue(QueueEffect::Send { to, ask })] = effects.as_slice() else {
        panic!("the request sent: {effects:?}");
    };
    assert_eq!(*to, EXPLORER);
    assert_eq!(ask.window, WindowHandle(90));
    assert_eq!(ask.number, RequestNumber(1));
    held
}

/// A request from outside the queue goes to the outpost of its window's
/// process, and its answer goes to its asker by its token.
#[test]
fn a_request_from_outside_the_queue_is_sent_to_its_windows_outpost_and_answered_to_its_asker() {
    use verbatim_model::request::{QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let reply = Reply::Steps(vec![verbatim_model::batch::StepOutcome::Failed]);
    assert_eq!(
        shell_feeds(
            &mut held,
            3_000,
            QueueInput::Answer {
                from: EXPLORER,
                answer: Answer {
                    number: RequestNumber(1),
                    outcome: WireOutcome::Result(reply.clone()),
                    probes: Vec::new(),
                    detail: String::new(),
                },
            }
        ),
        [Said::Queue(QueueEffect::Answered {
            asker: 5,
            outcome: WireOutcome::Result(reply),
        })]
    );
}

/// A foreground change, as the list dialog closing makes, does not stop
/// a request from outside the queue.
#[test]
fn a_request_from_outside_the_queue_is_not_stopped_by_a_foreground_change() {
    use verbatim_model::request::{ForegroundReport, QueueInput};
    let mut held = taskbar_read_out();
    let effects = shell_feeds(
        &mut held,
        3_000,
        QueueInput::Foreground(ForegroundReport {
            window: WindowHandle(70),
            root_owner: WindowHandle(70),
            process: Pid(50),
            thread: 3,
            class: "Notepad".to_owned(),
            ghost: None,
            console: false,
            integrity: None,
            own_integrity: None,
        }),
    );
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Said::Queue(verbatim_model::request::QueueEffect::Cancel { .. })
        )),
        "{effects:?}"
    );
    assert_eq!(held.queue.batch_for_tests(1), Some(taskbar_batch()));
}

/// A request from outside the queue keeps the queue's due time: 11
/// seconds after it was asked Core stops waiting and answers its asker.
#[test]
fn a_request_from_outside_the_queue_past_its_due_time_is_cancelled_and_answered() {
    use verbatim_model::request::{Cancel, CancelReason, QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let due_us = 2_000 + crate::event_queue::OTHER_DUE_US;
    let effects = shell_feeds(&mut held, due_us, QueueInput::Due { in_front: None });
    assert!(
        effects.contains(&Said::Queue(QueueEffect::Cancel {
            to: EXPLORER,
            cancel: Cancel {
                number: RequestNumber(1),
                reason: CancelReason::DueTime,
            },
        })),
        "{effects:?}"
    );
    assert!(
        effects.contains(&Said::Queue(QueueEffect::Answered {
            asker: 5,
            outcome: WireOutcome::Ended,
        })),
        "{effects:?}"
    );
}

/// A request from outside the queue keeps the live hang check while it
/// is out: its window is checked each half second.
#[test]
fn a_request_from_outside_the_queue_has_its_window_checked_for_a_hang() {
    use verbatim_model::request::{QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let effects = shell_feeds(
        &mut held,
        2_000 + crate::event_queue::HANG_ROUND_US,
        QueueInput::Due { in_front: None },
    );
    assert_eq!(
        effects,
        [Said::Queue(QueueEffect::CheckHung(vec![WindowHandle(90)]))]
    );
}

/// Its asker gone, the request is cancelled as withdrawn.
#[test]
fn a_request_from_outside_the_queue_withdrawn_by_its_asker_is_cancelled() {
    use verbatim_model::request::{Cancel, CancelReason, QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let effects = shell_feeds(&mut held, 3_000, QueueInput::WithdrawOutside(5));
    assert!(
        effects.contains(&Said::Queue(QueueEffect::Cancel {
            to: EXPLORER,
            cancel: Cancel {
                number: RequestNumber(1),
                reason: CancelReason::Withdrawn,
            },
        })),
        "{effects:?}"
    );
}

/// A focus on the task switcher's item, in File Explorer, its tell
/// carrying its parent's automation id, told after the hook reported Alt
/// `down` (or up), read and answered: what Core says.
fn switcher_item_said(down: bool) -> Vec<String> {
    use verbatim_model::request::QueueInput;
    let mut held = ready();
    shell_feeds(&mut held, 500, QueueInput::Alt { down });
    let mut tell = uia_tell(1, Vec::new());
    tell.application = "explorer".to_owned();
    tell.cached.0.push(PropertyAnswer {
        id: PropertyId::ParentAutomationId,
        value: Some(PropertyValue::Text("SwitchItemListControl".to_owned())),
    });
    let sent = shell_feeds(
        &mut held,
        1_000,
        QueueInput::Tell {
            from: LISTENER,
            tell: Box::new(tell),
        },
    );
    assert_eq!(sent.len(), 1, "the listener's read: {sent:?}");
    found_at(&mut held, 2_000, (LISTENER, 1), listener_answer());
    let mut read = focus_read(None);
    read.focus.read.values = values("Window B", Role::ListItem);
    found_at(&mut held, 3_000, (OUTPOST, 2), Found::Read(read))
}

/// The task switcher's item reached with Alt down says only itself, its
/// containers dropped, by Core's own Alt state at its tell's arrival; with
/// Alt up its window is spoken as any focus's is
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
#[test]
fn the_switchers_item_with_alt_down_at_its_arrival_says_only_itself() {
    assert_eq!(
        switcher_item_said(true),
        ["Label(\"Window B\")", "send Outpost(Pid(40)) Follow"]
    );
    assert_eq!(
        switcher_item_said(false),
        [
            "Label(\"Editor\") Role(Window)",
            "Label(\"Window B\")",
            "send Outpost(Pid(40)) Follow"
        ]
    );
}

/// A node of the application read through MSAA, `name` of `role`, as a
/// focus's report carries it.
fn msaa_node(number: u64, name: &str, role: Role) -> verbatim_model::NodeSnapshot {
    verbatim_model::NodeSnapshot {
        id: node(number),
        backend: verbatim_model::Backend::Msaa,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: verbatim_model::StateSet::new(),
        details: verbatim_model::NodeDetails::default(),
    }
}

/// A focus executed is the control plane's focus change, as its reader
/// reported it before Core read its focus itself: the node, its ancestors
/// outermost first, its text as read, under its tell's trace with its
/// window's facts, which the harness waits on by the node's name
/// (`rapid_tabbing_in_settings.rs`, `wait_for_focus_on`).
#[test]
fn an_executed_focus_is_the_control_planes_focus_change() {
    let mut state = ready();
    let _ = feed_at(&mut state, 1_000, entry(msaa_tell(-4)));
    let input = found(&state, OUTPOST, 1, Found::Read(focus_read(Some(held(-4)))));
    let observed: Vec<verbatim_model::Observed> = feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 2_000,
            input,
        },
    )
    .unwrap()
    .into_iter()
    .filter_map(|effect| match effect {
        verbatim_model::Effect::Observed(observed) => Some(*observed),
        _ => None,
    })
    .collect();
    assert_eq!(
        observed,
        [verbatim_model::Observed {
            trace_id: verbatim_model::TraceId::from_raw(7),
            source: APP,
            backend: verbatim_model::Backend::Msaa,
            window: Some(verbatim_model::WindowFacts {
                top_level: WindowHandle(4),
                root_owner: WindowHandle(4),
                topmost: false,
                under_active_window: None,
                in_foreground: true,
                foreground_window: WindowHandle(4),
                class: "Button".to_owned(),
                application: "app".to_owned(),
                alt_held: false,
                foreground_is_shell: false,
            }),
            event: verbatim_model::NormalizedEvent::FocusChanged {
                node: msaa_node(2, "OK", Role::Button),
                whole_window: false,
                ancestors: vec![msaa_node(1, "Editor", Role::Window)],
                text: Some(Box::new(verbatim_model::FocusText::NotRead)),
                earlier: None,
            },
        }]
    );
}
