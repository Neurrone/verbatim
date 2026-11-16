//! The focus entries through the reducer's state: a UI Automation focus's
//! two chained reads, a run tried by application, the judgements that end
//! an entry with no read, and the paused function rebuilt from the state
//! after a clone or a dump.

use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, CompleteUia, FocusAncestor, FocusCaret, FocusNode, FocusRead, HeldIds,
    KnownAncestor, NodeRead, ObjectClass, ObjectRead, Outcome as WireOutcome, Refusal, Reply,
    Request as Wire, RequestNumber, Route, RouteEnd, RouteStep, RunRead, Tried,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Payload, Read, Relation, WindowFacet,
};
use verbatim_model::{NodeId, OutpostId, Pid, Role, SegmentContent};

use super::*;
use crate::event_queue::{Input, Instance, Reader};
use crate::focus::versions::{Committed, LastFocus};

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
        redirected_to: None,
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

pub(crate) fn listener_answer() -> ObjectRead {
    ObjectRead {
        object: NodeRead {
            node: None,
            window: win(5),
            values: values("OK", Role::Button),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: Some(Route {
            window: WindowHandle(5),
            steps: vec![RouteStep {
                runtime_id: RuntimeId(vec![42, 1]),
                window: None,
                content: true,
            }],
            end: RouteEnd::ContinueFrom(WindowHandle(5)),
        }),
    }
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
    feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Send { to, ask } => Some(format!("send {:?} {}", to.reader, kind(&ask))),
            Effect::Said(Said::Speak(utterance, _)) => Some(
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
            Effect::Send { ask, .. } => Some(ask.number.0),
            _ => None,
        })
        .collect();
    let said = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Said(Said::Speak(utterance, _)) => Some(
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
    let (asked, _) = sent_at(
        held,
        at_us + 1,
        answer(LISTENER, object, Reply::Object(Box::new(listener_answer()))),
    );
    let [completion] = asked[..] else {
        panic!("one completion asked: {asked:?}");
    };
    sent_at(
        held,
        at_us + 2,
        answer(OUTPOST, completion, Reply::Focus(Box::new(read))),
    )
    .1
}

pub(crate) fn kind(ask: &Ask) -> &'static str {
    match &ask.request {
        Wire::ReadObject(_) => "ReadObject",
        Wire::CompleteUia(_) => "CompleteUia",
        Wire::MsaaRun(_) => "MsaaRun",
        Wire::CurrentFocus { .. } => "CurrentFocus",
        Wire::Follow(_) => "Follow",
        _ => "other",
    }
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
    Input::Entry(Entry::of_tell(LISTENER, tell).unwrap())
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
        feed_at(
            &mut state,
            2_000,
            answer(LISTENER, 1, Reply::Object(Box::new(listener_answer())))
        ),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    state
}

#[test]
fn a_uia_focus_is_read_in_two_requests_and_executed() {
    let mut state = asked_completion();
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
    let Committed::Read(kept) = &state.versions.committed else {
        panic!("a reader's focus is committed");
    };
    assert_eq!(kept.node, node(2));
    assert_eq!(kept.outpost, OutpostId(GENERATION));
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
        state.queue.table_for_tests(),
        [Wire::Follow(verbatim_model::request::Follow {
            node: Some(verbatim_model::request::Followed {
                node: node(2),
                group: verbatim_model::request::FollowGroup::Base,
                extra: None,
            }),
        })]
    );
}

#[test]
fn a_uia_duplicate_of_the_readers_last_focus_ends_with_no_read() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
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
    assert!(state.focus.object().is_none());
    assert_eq!(state.versions.committed, Committed::Nothing);
    assert!(state.queue.front_for_tests().is_none());
}

/// The completion asked at `at_us` by `input`, as the outpost receives it.
fn sent_completion(held: &mut Held, at_us: u64, input: Input<Entry>) -> Option<Box<CompleteUia>> {
    feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
        .unwrap()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Send { ask, .. } => match ask.request {
                Wire::CompleteUia(complete) => Some(complete),
                _ => None,
            },
            _ => None,
        })
}

/// A second focus, of runtime id `[42, 2]`, asked of the listener after
/// the focus "OK" was committed with its frame "Editor" known by the
/// runtime id `[42, 4]`.
fn second_focus_asked() -> Held {
    let mut state = asked_completion();
    let mut first = focus_read(None);
    first.ancestors[0].node.runtime_id = Some(RuntimeId(vec![42, 4]));
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(first))),
    );
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
    let mut object = listener_answer();
    if let Some(route) = object.route.as_mut() {
        route.end = RouteEnd::Known(met.clone());
    }
    let complete = sent_completion(
        &mut state,
        5_000,
        answer(LISTENER, 4, Reply::Object(Box::new(object))),
    )
    .unwrap();
    assert_eq!(complete.route.end, RouteEnd::Known(met.clone()));
    assert!(complete.known.contains(&met));
}

#[test]
fn a_focus_inside_the_committed_focus_meets_it_and_says_only_itself() {
    // NVDA's walk meets its focus object as it meets the focus's
    // ancestors (`docs/parity/focus-context.md`), so the committed focus
    // is known first, and a focus inside it says no container again.
    let mut state = second_focus_asked();
    let met = KnownAncestor::Uia(RuntimeId(vec![42, 1]));
    let mut object = listener_answer();
    if let Some(route) = object.route.as_mut() {
        route.end = RouteEnd::Known(met.clone());
    }
    let complete = sent_completion(
        &mut state,
        5_000,
        answer(LISTENER, 4, Reply::Object(Box::new(object))),
    )
    .unwrap();
    let place = complete.known.iter().position(|known| *known == met);
    let mut inner = focus_read(None);
    inner.focus.read.node = Some(node(3));
    inner.focus.read.values = values("Inner", Role::Button);
    inner.focus.runtime_id = Some(RuntimeId(vec![42, 2]));
    inner.ancestors = Vec::new();
    inner.met = Some(u32::try_from(place.unwrap()).unwrap());
    assert_eq!(
        feed_at(
            &mut state,
            6_000,
            answer(OUTPOST, 5, Reply::Focus(Box::new(inner)))
        ),
        [
            "Label(\"Inner\") Role(Button)",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    let committed = state.versions.committed_read().unwrap();
    assert_eq!(
        committed
            .ancestors
            .iter()
            .map(|ancestor| ancestor.node)
            .collect::<Vec<_>>(),
        [node(2), node(1)]
    );
}

#[test]
fn a_walk_that_met_an_ancestor_core_no_longer_knows_ends_the_focus_unread() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(uia_tell(1, Vec::new()))),
        ["send Listener ReadObject"]
    );
    let mut object = listener_answer();
    if let Some(route) = object.route.as_mut() {
        route.end = RouteEnd::Known(KnownAncestor::Uia(RuntimeId(vec![42, 4])));
    }
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(LISTENER, 1, Reply::Object(Box::new(object)))
        ),
        NOTHING
    );
    assert!(state.focus.object().is_none());
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_clone_of_the_state_rebuilds_the_paused_function_and_goes_on_alike() {
    let mut state = asked_completion();
    let mut cloned = state.clone();
    let reply = answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))));
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
    let reply = answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))));
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
    assert!(
        feed(
            &mut cloned.state,
            &mut cloned.runtime,
            Timed {
                at_us: 3_000,
                input: answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
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

#[test]
fn a_run_asks_its_application_newest_first_and_executes_the_valid_focus() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    // The first candidate is asked alone; two more join a run behind it.
    feed_at(&mut state, 1_100, entry(msaa_tell(-5)));
    feed_at(&mut state, 1_200, entry(msaa_tell(-6)));
    let tried = RunRead {
        tried: vec![Tried::NotThere],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    let asked = state
        .queue
        .table_for_tests()
        .into_iter()
        .find_map(|request| match request {
            Wire::MsaaRun(run) => Some(
                run.candidates
                    .iter()
                    .map(|candidate| candidate.ids.object)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
    assert_eq!(asked, Some(vec![-6, -5]));
    let tried = RunRead {
        tried: vec![
            Tried::Refused(Refusal::NotFocused),
            Tried::Focus(Box::new(focus_read(Some(held(-5))))),
        ],
    };
    assert_eq!(
        feed_at(&mut state, 3_000, answer(OUTPOST, 2, Reply::Run(tried))),
        SPOKEN
    );
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
        class: ObjectClass::Plain,
        role: verbatim_model::Role::Button,
    });
    assert_eq!(feed_at(&mut state, 1_000, entry(msaa_tell(-4))), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// File Explorer raises a second focus on the desktop's item it focused,
/// child 3 of its list, as it does when a program starts from the desktop
/// (`text_box_caret_keys`' artifacts): the first is read and spoken, and
/// the second, with the same ids, is refused with no read, though a
/// child above 0 is never a duplicate.
#[test]
fn a_second_focus_on_an_item_file_explorer_raises_twice_is_refused_with_no_read() {
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
    assert_eq!(item(&mut state, 1_000), ["send Outpost(Pid(40)) MsaaRun"]);
    let mut read = focus_read(Some(HeldIds {
        event: MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 3,
        },
        held_child: 3,
    }));
    read.class = ObjectClass::FocusRaisedTwice;
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
        SPOKEN
    );
    assert_eq!(item(&mut state, 3_000), NOTHING);
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

#[test]
fn a_foreground_marker_empties_the_last_focus_for_an_unrelated_window() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    assert!(state.versions.last.is_some());
    let foreground = Foreground {
        window: WindowHandle(70),
        root_owner: WindowHandle(70),
        process: Pid(41),
        thread: 0,
        by_class: None,
        shell: false,
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

#[test]
fn a_focus_on_the_child_a_redirect_reached_ends_the_run_with_no_read() {
    let mut state = ready();
    feed_at(&mut state, 1_000, entry(msaa_tell(-4)));
    let mut read = focus_read(Some(held(-4)));
    read.redirected_to = Some(held(-5).event);
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
        SPOKEN
    );
    assert_eq!(feed_at(&mut state, 3_000, entry(msaa_tell(-5))), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// Verbatim's settings dialog: Core reads the list's focused child as the
/// dialog's current focus and speaks it, then the list's own focus comes
/// to the front and its read is redirected to that child, which is not
/// spoken again (`settings_dialog_keys`' artifacts).
#[test]
fn a_redirect_that_reaches_the_committed_focus_ends_the_run_unspoken() {
    let mut state = ready();
    feed_at(&mut state, 1_000, entry(msaa_tell(-5)));
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(focus_read(Some(held(-5)))))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
        SPOKEN
    );
    assert_eq!(
        feed_at(&mut state, 3_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    let mut read = focus_read(Some(held(-4)));
    read.redirected_to = Some(held(-5).event);
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(&mut state, 4_000, answer(OUTPOST, 2, Reply::Run(tried))),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
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
        ["send Outpost(Pid(40)) MsaaRun"]
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
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
        ["Label(\"Verbatim\") Role(Window)", "Label(\"Settings...\")",]
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
        ["send Outpost(Pid(40)) MsaaRun"]
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
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 1, Reply::Run(tried))),
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
        shell: false,
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
    let tried = |read| RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(OUTPOST, 2, Reply::Run(tried(menu)))
        ),
        ["Label(\"Verbatim\")", "Label(\"Context\") Role(Menu)"]
    );
    feed_at(&mut state, 3_000, entry(frame_menu_tell(1)));
    let mut settings = frame_menu_read(5, "Settings...", Role::MenuItem, 1, false);
    settings.met = Some(0);
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(OUTPOST, 3, Reply::Run(tried(settings)))
        ),
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
        dialog_text: None,
    });
    assert_eq!(
        feed_at(
            &mut state,
            6_000,
            answer(OUTPOST, 4, Reply::Run(tried(exit)))
        ),
        ["Label(\"Exit\")"]
    );
}

#[test]
fn a_focus_whose_top_level_window_is_an_ignored_processs_is_dropped_at_its_answer() {
    let mut state = asked_completion();
    state.ignored = vec![APP];
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))))
        ),
        // Not executed: naming nothing removes the completion's
        // registration.
        ["send Outpost(Pid(40)) Follow"]
    );
    assert_eq!(state.versions.last, None);
    assert!(state.focus.object().is_none());
}

#[test]
fn an_executed_focus_runs_under_its_tells_trace_and_takes_its_top_ancestor_as_foreground() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    let focus = state.focus.object().unwrap();
    assert_eq!(focus.trace_id, Some(verbatim_model::TraceId::from_raw(7)));
    // The walk met no ancestor Core knew and the answer read no
    // foreground object: the top ancestor, the frame, stands for it.
    let frame = focus.ancestors.first().unwrap();
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
    assert_eq!((*to, kind(ask)), (OUTPOST, "MsaaRun"));
    assert_eq!(*wake, Some(1_000 + crate::event_queue::HANG_ROUND_US));
}

#[test]
fn a_failed_focus_read_clears_cores_caret_baseline() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    state.focus.object_mut().unwrap().caret = Some(crate::state::CaretContext {
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
        line_break: None,
        read_at_us: 0,
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
    assert_eq!(state.focus.object().unwrap().caret, None);
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
        feed_at(
            &mut state,
            2_000,
            answer(LISTENER, 1, Reply::Object(Box::new(listener_answer()))),
        );
        let mut read = focus_read(None);
        read.focus.read.values = values("Staging", Role::Pane);
        assert_eq!(
            feed_at(
                &mut state,
                3_000,
                answer(OUTPOST, 2, Reply::Focus(Box::new(read)))
            ),
            ["send Outpost(Pid(40)) Follow"],
            "{class}"
        );
        assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
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

/// The notification area list's read of the tray's items, asked under
/// `asker`, naming `Shell_TrayWnd`.
fn taskbar_read(asker: u64) -> verbatim_model::request::QueueInput {
    verbatim_model::request::QueueInput::AskOutside {
        asker,
        window: taskbar(),
        request: Wire::ShellItems {
            kind: verbatim_model::ShellItemKind::SystemTray,
        },
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
    use verbatim_model::request::{OutsideOutcome, QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let reply = Reply::ShellItems(vec![verbatim_model::ShellItem {
        name: "Clock".to_owned(),
        runtime_id: vec![42, 1],
    }]);
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
            outcome: OutsideOutcome::Result(reply),
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
        }),
    );
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Said::Queue(verbatim_model::request::QueueEffect::Cancel { .. })
        )),
        "{effects:?}"
    );
    assert!(held.queue.table_for_tests().contains(&Wire::ShellItems {
        kind: verbatim_model::ShellItemKind::SystemTray,
    }));
}

/// A request from outside the queue keeps the queue's due time: 11
/// seconds after it was asked Core stops waiting and answers its asker.
#[test]
fn a_request_from_outside_the_queue_past_its_due_time_is_cancelled_and_answered() {
    use verbatim_model::request::{Cancel, CancelReason, OutsideOutcome, QueueEffect, QueueInput};
    let mut held = taskbar_read_out();
    let due_us = 2_000 + crate::event_queue::OTHER_DUE_US;
    let effects = shell_feeds(&mut held, due_us, QueueInput::Due);
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
            outcome: OutsideOutcome::NoLongerAwaited(CancelReason::DueTime),
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
        QueueInput::Due,
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
    feed_at(
        &mut held,
        2_000,
        answer(LISTENER, 1, Reply::Object(Box::new(listener_answer()))),
    );
    let mut read = focus_read(None);
    read.focus.read.values = values("Window B", Role::ListItem);
    feed_at(
        &mut held,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(read))),
    )
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
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(focus_read(Some(held(-4)))))],
    };
    let observed: Vec<verbatim_model::Observed> = feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 2_000,
            input: answer(OUTPOST, 1, Reply::Run(tried)),
        },
    )
    .unwrap()
    .into_iter()
    .filter_map(|effect| match effect {
        Effect::Said(verbatim_model::Effect::Observed(observed)) => Some(*observed),
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
                selected_child: None,
                text: Some(Box::new(verbatim_model::FocusText::NotRead)),
                earlier: None,
                group: None,
            },
        }]
    );
}
