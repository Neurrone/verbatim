//! The focus entries through the reducer's state: a UI Automation focus's
//! two chained reads, a run tried by application, the judgements that end
//! an entry with no read, and the paused function rebuilt from the state
//! after a clone or a dump.

use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, FocusAncestor, FocusNode, FocusRead, HeldIds, NodeRead, ObjectClass, ObjectRead,
    Outcome as WireOutcome, Refusal, Reply, Request as Wire, RequestNumber, Route, RouteEnd,
    RouteStep, RunRead, Tried,
};
use verbatim_model::tell::{
    AcceptanceFacts, Checked, ElementToken, Payload, Read, Relation, WindowFacet,
};
use verbatim_model::{NodeId, OutpostId, Pid, Role, SegmentContent};

use super::*;
use crate::event_queue::{Input, Instance, Reader};
use crate::focus::versions::{Committed, LastFocus};

const APP: Pid = Pid(40);
const GENERATION: u64 = 3;
const LISTENER: Instance = Instance {
    reader: Reader::Listener,
    generation: 1,
};
const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: GENERATION,
};

fn facet(handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: APP,
        thread: 41,
    }
}

/// Window 5, of `class`, under the foreground window 4, so acceptance
/// passes by the foreground relation.
fn facts(class: &str) -> AcceptanceFacts {
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
    }
}

fn uia_tell(runtime: i32, kept: Vec<(ElementToken, Read<bool>)>) -> Event {
    Event {
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

fn msaa_tell(object: i32) -> Event {
    Event {
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

fn values(name: &str, role: Role) -> PropertyValues {
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

fn node(number: u64) -> NodeId {
    NodeId::in_outpost(OutpostId(GENERATION), number)
}

/// A focus answer: the button "OK" in the frame "Editor".
fn focus_read(msaa: Option<HeldIds>) -> FocusRead {
    FocusRead {
        focus: FocusNode {
            read: NodeRead {
                node: Some(node(2)),
                window: WindowHandle(5),
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
                    window: WindowHandle(4),
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
    }
}

fn listener_answer() -> ObjectRead {
    ObjectRead {
        object: NodeRead {
            node: None,
            window: WindowHandle(5),
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

/// Feeds `input` at `at_us`, the step's effects as a test reads them:
/// each request sent by its reader and kind, each utterance by its
/// segments, every other effect left out.
fn feed_at(state: &mut SrState, at_us: u64, input: Input<Entry>) -> Vec<String> {
    feed(state, Timed { at_us, input })
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

fn kind(ask: &Ask) -> &'static str {
    match &ask.request {
        Wire::ReadObject(_) => "ReadObject",
        Wire::CompleteUia(_) => "CompleteUia",
        Wire::MsaaRun(_) => "MsaaRun",
        _ => "other",
    }
}

fn answer(from: Instance, number: u64, reply: Reply) -> Input<Entry> {
    answer_outcome(from, number, WireOutcome::Result(reply))
}

fn answer_outcome(from: Instance, number: u64, outcome: WireOutcome) -> Input<Entry> {
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

fn ready() -> SrState {
    let mut state = SrState::new();
    assert_eq!(
        feed_at(&mut state, 0, Input::ReaderReady(LISTENER)),
        NOTHING
    );
    assert_eq!(feed_at(&mut state, 0, Input::ReaderReady(OUTPOST)), NOTHING);
    state
}

fn entry(tell: Event) -> Input<Entry> {
    Input::Entry(Entry::of_tell(tell).unwrap())
}

/// Nothing sent or said.
const NOTHING: [&str; 0] = [];

/// The window's title, then the focus.
const SPOKEN: [&str; 2] = [
    "Label(\"Editor\") Role(Window)",
    "Label(\"OK\") Role(Button)",
];

/// A UI Automation focus asked of the listener, then completed by the
/// outpost, up to the completion's answer.
fn asked_completion() -> SrState {
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
        SPOKEN
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
    assert_eq!(state.queue.table_for_tests(), Vec::new());
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
        NOTHING
    );
    assert!(state.focus.object().is_none());
    assert_eq!(state.versions.committed, Committed::Nothing);
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
    assert_eq!(format!("{cloned:?}"), format!("{state:?}"));
}

#[test]
fn a_dump_of_the_state_rebuilds_the_paused_function_and_goes_on_alike() {
    let mut state = asked_completion();
    let json = serde_json::to_string(&state).unwrap();
    let mut restored: SrState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, state);
    let reply = answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))));
    let straight = feed_at(&mut state, 3_000, reply.clone());
    assert_eq!(feed_at(&mut restored, 3_000, reply), SPOKEN);
    assert_eq!(straight, SPOKEN);
    assert_eq!(format!("{restored:?}"), format!("{state:?}"));
}

#[test]
fn a_state_whose_function_is_not_in_its_threads_table_rebuilds_it() {
    let mut state = asked_completion();
    let mut copy = state.clone();
    let reply = answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))));
    let straight = feed_at(&mut copy, 3_000, reply.clone());
    // As for a state moved to another thread: its key finds nothing.
    forget_paused_for_tests();
    assert_eq!(feed_at(&mut state, 3_000, reply), straight);
    assert_eq!(state, copy);
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
            &mut cloned,
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
fn held(object: i32) -> HeldIds {
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
            ancestor_windows: Vec::new(),
        },
        class: ObjectClass::Plain,
    });
    assert_eq!(feed_at(&mut state, 1_000, entry(msaa_tell(-4))), NOTHING);
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
    };
    assert_eq!(
        feed_at(&mut state, 4_000, Input::Foreground(foreground)),
        NOTHING
    );
    assert_eq!(state.versions.last, None);
}
