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

pub(crate) fn node(number: u64) -> NodeId {
    NodeId::in_outpost(OutpostId(GENERATION), number)
}

/// A focus answer: the button "OK" in the frame "Editor".
pub(crate) fn focus_read(msaa: Option<HeldIds>) -> FocusRead {
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

pub(crate) fn listener_answer() -> ObjectRead {
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

/// A state with the runtime its test keeps beside it. A clone has no
/// paused function, as a clone of the state has none.
#[derive(Debug, Default)]
pub(crate) struct Held {
    state: SrState,
    runtime: Runtime,
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

pub(crate) fn ready() -> Held {
    let mut state = Held::default();
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
            ancestor_windows: Vec::new(),
        },
        class: ObjectClass::Plain,
        role: verbatim_model::Role::Button,
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
        thread: 0,
        by_class: None,
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
    read.focus.read.window = WindowHandle(6);
    read.ancestors.clear();
    let mut frame = values("Verbatim", Role::Window);
    frame.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(8))),
    });
    read.foreground = Some(FocusNode {
        read: NodeRead {
            node: Some(node(1)),
            window: WindowHandle(8),
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
    let effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &queued(QueueInput::ReaderReady(OUTPOST)),
    );
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
