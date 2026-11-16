//! Core's merge in its event queue (`docs/design/readers-tell-core-asks-build1.md`,
//! "The event queue"): a newer tell of the same object and kind, from the
//! same thread, replaces one still waiting, the newest keeping its place.
//! For every kind Core merges, each sequence of up to two other inputs
//! from a fixed set between the older tell and the newer, both told while
//! the front entry's request is out, does exactly what the same sequence
//! does without the older tell: every effect and the focus Core holds
//! are the same, so the older tell is gone and the newer is judged at its
//! own place. What the newer tell says is asserted on its own.

mod desk;

use desk::{
    Desk, Heard, LISTENER, ONE, focus_heard, focus_now_in, focus_read_of, focus_tell,
    listener_read, queued, runtime_of,
};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    Instance, NodeRead, NodeWindow, ObjectRead, ObjectRef, Outcome, Probe, Reader, RelationFound,
    Reply, Request as Wire,
};
use verbatim_model::tell::{Checked, Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, Earcon, NodeDetails, NodeSnapshot, Role, SegmentContent, SpeechPriority, State,
    StateSet, TraceId, UtteranceSegment, WindowHandle,
};

/// The focus's window, the pane in it, and the edit field "Notes" Core
/// holds as its focus.
const WINDOW: u64 = 1;
const PANE: u64 = 2;
const NOTES: u64 = 3;

/// The MSAA object id of the alert another input may raise.
const ALERT: i32 = 9;

fn node(number: u64, role: Role, name: &str, states: StateSet) -> NodeSnapshot {
    NodeSnapshot {
        id: ONE.node(number),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states,
        details: NodeDetails::default(),
    }
}

fn focused() -> StateSet {
    StateSet::new().with(State::Focused).with(State::Focusable)
}

/// The focus's line: the window, and the pane in it.
fn line_ancestors() -> Vec<NodeSnapshot> {
    vec![
        node(WINDOW, Role::Window, "Editor", StateSet::new()),
        node(PANE, Role::Pane, "Pane", StateSet::new()),
    ]
}

/// The fixed start: Core's focus on the edit field "Notes" in the pane in
/// the window.
fn start() -> Desk {
    let mut desk = Desk::new();
    let _ = desk.focus(
        ONE,
        &node(NOTES, Role::EditableText, "Notes", focused()),
        &line_ancestors(),
    );
    desk
}

/// The window of the application's alerts, under its top-level window,
/// which has no UI Automation provider.
fn alert_window() -> WindowHandle {
    WindowHandle(ONE.top + 2)
}

/// The listener's MSAA alert tell on [`alert_window`], object `object`,
/// its check's probe finding no UI Automation provider there.
fn alert(object: i32) -> Event {
    let mut facts = ONE.facts();
    facts.window = ONE.facet(alert_window(), "AlertPane");
    Event {
        trace_id: TraceId::from_raw(20),
        kind: EventKind::Alert,
        process: ONE.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us: 0,
        object: EventObject::Msaa {
            window: alert_window(),
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts)),
        checked: Some(Checked {
            probe: Some(Probe {
                window: alert_window(),
                thread: 41,
                provider: false,
                began_us: 0,
            }),
            keyboard_focus: None,
            kept: Vec::new(),
            previous_sibling: None,
        }),
    }
}

/// A tell, and the reader that told it.
#[derive(Clone)]
struct Told {
    from: Instance,
    tell: Event,
}

/// The outpost's tell of a UI Automation change of `property` on the
/// focus, which it follows, carrying `value`.
fn followed(property: PropertyId, value: Option<PropertyValue>) -> Told {
    Told {
        from: ONE.instance(),
        tell: Event {
            trace_id: TraceId::from_raw(21),
            kind: EventKind::UiaPropertyChanged,
            process: ONE.pid,
            application: "app".to_owned(),
            thread: 41,
            at_us: 0,
            object: EventObject::Followed {
                runtime_id: runtime_of(NOTES),
            },
            cached: PropertyValues::default(),
            payload: Payload::PropertyChanged { property, value },
            window: TellWindow::NoWindow,
            checked: None,
        },
    }
}

/// One of the inputs that may come between the older tell and the newer,
/// none of which concerns either tell's object.
#[derive(Clone, Copy, Debug)]
enum Other {
    /// An alert of an object outside the focus's line, which Core reads
    /// and speaks from anywhere.
    Alert,
    /// An answer to a request Core is not waiting for, which it drops.
    Late,
}

/// Every sequence of zero to two others, each from the fixed set: 7.
fn sequences() -> Vec<Vec<Other>> {
    let set = [Other::Alert, Other::Late];
    let mut sequences = vec![Vec::new()];
    for first in set {
        sequences.push(vec![first]);
    }
    for first in set {
        for second in set {
            sequences.push(vec![first, second]);
        }
    }
    sequences
}

/// The outpost's read of the alert, "Saved".
fn alert_read() -> Outcome {
    let values = PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::Name,
            value: Some(PropertyValue::Text("Saved".to_owned())),
        },
        PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(Role::Alert)),
        },
    ]);
    Outcome::Result(Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: Some(ONE.node(50)),
            window: NodeWindow {
                handle: alert_window(),
                ..ONE.node_window()
            },
            values,
        },
        relation: Some(RelationFound::Unrelated),
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: None,
    })))
}

/// A focus on another element, node 30, told by the listener, whose read
/// is the front entry's request out while the tells under test arrive:
/// its number.
fn busy(desk: &mut Desk) -> u64 {
    assert_eq!(
        desk.tell(LISTENER, focus_tell(ONE, 30, 0)),
        [Heard::Sent(Reader::Listener, "ReadObject")]
    );
    let [(front, _, _)] = desk.sent[..] else {
        panic!("one read asked: {:?}", desk.sent);
    };
    front
}

/// What one sequence did: every effect after the start, and the focus
/// Core holds at the end.
#[derive(Debug, PartialEq)]
struct Run {
    heard: Vec<Heard>,
    focus: Option<NodeSnapshot>,
}

/// From the start, with the front entry's request out ([`busy`]), `older`
/// when given, `others`, and `newer` are told; the front's read then
/// answers that the element is not there, and each request the queue sends next is
/// answered, the alert's from [`alert_read`] and the rest by `answer`.
fn run(
    older: Option<&Told>,
    others: &[Other],
    newer: &Told,
    answer: &dyn Fn(&Wire) -> Outcome,
) -> Run {
    let mut desk = start();
    let front = busy(&mut desk);
    let mut heard = Vec::new();
    if let Some(older) = older {
        heard.extend(desk.tell(older.from, older.tell.clone()));
    }
    for other in others {
        heard.extend(match other {
            Other::Alert => desk.tell(LISTENER, alert(ALERT)),
            Other::Late => desk.answer(ONE.instance(), 999, Outcome::NotThere),
        });
    }
    heard.extend(desk.tell(newer.from, newer.tell.clone()));
    heard.extend(desk.answer(LISTENER, front, Outcome::NotThere));
    while let Some((number, to, request)) = desk
        .sent
        .iter()
        .find(|(_, _, request)| !matches!(request, Wire::Follow(_)))
        .cloned()
    {
        let outcome = match &request {
            Wire::ReadObject(read)
                if matches!(read.object, ObjectRef::Msaa { object: ALERT, .. }) =>
            {
                alert_read()
            }
            other => answer(other),
        };
        heard.extend(desk.answer(to, number, outcome));
    }
    Run {
        heard,
        focus: desk.state.focused().map(|(_, node)| node.clone()),
    }
}

/// For every sequence of others, `older` then `newer` does exactly what
/// `newer` alone does; with no others, `newer` alone does `heard`.
fn equivalent(older: &Told, newer: &Told, answer: &dyn Fn(&Wire) -> Outcome, heard: &[Heard]) {
    for others in sequences() {
        assert_eq!(
            run(Some(older), &others, newer, answer),
            run(None, &others, newer, answer),
            "with {others:?}"
        );
    }
    assert_eq!(run(None, &[], newer, answer).heard, heard);
}

/// No request is expected beside the front's and the alert's.
fn unasked(request: &Wire) -> Outcome {
    panic!("an unexpected request: {request:?}")
}

fn role(role: Role) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Role(role))
}

/// The listener's UI Automation focus tell of node 4.
fn focus_on_four() -> Told {
    Told {
        from: LISTENER,
        tell: focus_tell(ONE, 4, 0),
    }
}

#[test]
fn a_focus_merged_is_the_newer_focus() {
    let second = node(4, Role::Button, "Second", focused());
    let answer = move |request: &Wire| match request {
        Wire::ReadObject(_) => Outcome::Result(Reply::Object(Box::new(listener_read(ONE, 4)))),
        Wire::CompleteUia(_) => Outcome::Result(Reply::Focus(Box::new(focus_read_of(
            ONE,
            &second,
            &line_ancestors(),
        )))),
        other => unasked(other),
    };
    let mut heard = vec![Heard::Sent(Reader::Listener, "ReadObject")];
    heard.push(Heard::Sent(Reader::Outpost(ONE.pid), "CompleteUia"));
    heard.extend(focus_heard(
        ONE,
        focus_now_in(ONE, 4, &[WINDOW, PANE]),
        vec![vec![UtteranceSegment::label("Second"), role(Role::Button)]],
    ));
    equivalent(&focus_on_four(), &focus_on_four(), &answer, &heard);
}

/// A property change of the focus, from `older` to `newer`, carried by the
/// tells, merged: what `newer` alone says is `said`.
fn property(property: PropertyId, (older, newer): (PropertyValue, PropertyValue), said: &[Heard]) {
    equivalent(
        &followed(property, Some(older)),
        &followed(property, Some(newer)),
        &unasked,
        said,
    );
}

/// A property change of the focus that its tells do not carry, merged:
/// read once at its turn, its read answering `value`, after which it says
/// `said`.
fn read_property(property: PropertyId, value: PropertyValue, said: &[Heard]) {
    let answer = move |request: &Wire| match request {
        Wire::Properties { .. } => {
            Outcome::Result(Reply::Properties(PropertyValues(vec![PropertyAnswer {
                id: property,
                value: Some(value.clone()),
            }])))
        }
        other => unasked(other),
    };
    let mut heard = vec![Heard::Sent(Reader::Outpost(ONE.pid), "Properties")];
    heard.extend_from_slice(said);
    equivalent(
        &followed(property, None),
        &followed(property, None),
        &answer,
        &heard,
    );
}

#[test]
fn a_name_change_merged_is_the_newer_name() {
    property(
        PropertyId::Name,
        (
            PropertyValue::Text("Draft".to_owned()),
            PropertyValue::Text("Final".to_owned()),
        ),
        &[queued(vec![UtteranceSegment::label("Final")])],
    );
}

#[test]
fn a_value_property_change_merged_is_the_newer_value() {
    property(
        PropertyId::Value,
        (
            PropertyValue::Text("one".to_owned()),
            PropertyValue::Text("two".to_owned()),
        ),
        &[],
    );
}

#[test]
fn a_states_change_merged_is_the_newer_states() {
    property(
        PropertyId::States,
        (
            PropertyValue::States(focused().with(State::ReadOnly)),
            PropertyValue::States(focused().with(State::Required)),
        ),
        &[queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Required,
        ))])],
    );
}

#[test]
fn a_description_change_merged_is_the_newer_description() {
    read_property(
        PropertyId::Description,
        PropertyValue::Text("Newer".to_owned()),
        &[queued(vec![UtteranceSegment::new(
            SegmentContent::Description("Newer".to_owned()),
        )])],
    );
}

#[test]
fn a_suggestions_change_merged_is_the_newer_one() {
    read_property(
        PropertyId::Suggestions,
        PropertyValue::Flag(true),
        &[Heard::Earcon(Earcon::SuggestionsOpened)],
    );
}

/// The listener's tell that node `number`'s element was selected.
fn selected(number: u64) -> Told {
    let mut tell = focus_tell(ONE, number, 0);
    tell.kind = EventKind::UiaElementSelected;
    tell.checked = None;
    tell.cached = PropertyValues(vec![PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(StateSet::new().with(State::Selected))),
    }]);
    Told {
        from: LISTENER,
        tell,
    }
}

/// The answers to a selected item's two reads: the listener's, and the
/// outpost's completion, reading item `number` named `name` with
/// `relation` to the focus.
fn item_answer(
    number: u64,
    name: &'static str,
    relation: RelationFound,
) -> impl Fn(&Wire) -> Outcome {
    move |request: &Wire| match request {
        Wire::ReadObject(_) => Outcome::Result(Reply::Object(Box::new(listener_read(ONE, number)))),
        Wire::CompleteUia(_) => Outcome::Result(Reply::Object(Box::new(ObjectRead {
            object: NodeRead {
                node: Some(ONE.node(number)),
                window: ONE.node_window(),
                values: desk::values_of(&node(
                    number,
                    Role::ListItem,
                    name,
                    StateSet::new().with(State::Selected),
                )),
            },
            relation: Some(relation),
            descendants: Vec::new(),
            ancestors: Vec::new(),
            met: None,
            route: None,
        }))),
        other => unasked(other),
    }
}

/// The two requests a selected item's reads send.
fn item_reads() -> Vec<Heard> {
    vec![
        Heard::Sent(Reader::Listener, "ReadObject"),
        Heard::Sent(Reader::Outpost(ONE.pid), "CompleteUia"),
    ]
}

#[test]
fn a_selection_merged_is_the_newer_selection() {
    equivalent(
        &selected(5),
        &selected(5),
        &item_answer(5, "One, renamed", RelationFound::Unrelated),
        &item_reads(),
    );
}

#[test]
fn a_controlled_selection_merged_is_the_newer_one() {
    let mut heard = item_reads();
    heard.push(Heard::Say(
        SpeechPriority::Interrupt,
        vec![
            UtteranceSegment::label("Suggestion two"),
            UtteranceSegment::new(SegmentContent::State(State::Selected)),
        ],
    ));
    equivalent(
        &selected(6),
        &selected(6),
        &item_answer(6, "Suggestion two", RelationFound::Controlled),
        &heard,
    );
}

#[test]
fn an_alert_merged_is_the_newer_alert() {
    let alert = || Told {
        from: LISTENER,
        tell: alert(7),
    };
    let answer = |request: &Wire| match request {
        Wire::ReadObject(_) => {
            let Outcome::Result(Reply::Object(mut read)) = alert_read() else {
                unreachable!("an object read");
            };
            read.object.node = Some(ONE.node(7));
            read.object.values =
                desk::values_of(&node(7, Role::Alert, "Copied twice", StateSet::new()));
            Outcome::Result(Reply::Object(read))
        }
        other => unasked(other),
    };
    equivalent(
        &alert(),
        &alert(),
        &answer,
        &[
            Heard::Sent(Reader::Outpost(ONE.pid), "ReadObject"),
            queued(vec![
                UtteranceSegment::label("Copied twice"),
                role(Role::Alert),
            ]),
        ],
    );
}

#[test]
fn a_value_change_merged_is_the_newer_value() {
    property(
        PropertyId::Value,
        (
            PropertyValue::Text("50".to_owned()),
            PropertyValue::Text("51".to_owned()),
        ),
        &[],
    );
}

/// A name change of the focus the older focus tell leaves, told between
/// two focus tells of the new focus: the merged focus takes the newer
/// tell's place behind it, so Core judges the change against the focus
/// before, speaking it as that focus's change, and announces the new
/// focus after it.
#[test]
fn a_focus_merged_across_a_change_of_its_line_moves_behind_it() {
    let mut desk = start();
    let front = busy(&mut desk);
    let older = focus_on_four();
    let between = followed(
        PropertyId::Name,
        Some(PropertyValue::Text("Renamed".to_owned())),
    );
    let newer = focus_on_four();
    assert_eq!(desk.tell(older.from, older.tell), []);
    assert_eq!(desk.tell(between.from, between.tell), []);
    assert_eq!(desk.tell(newer.from, newer.tell), []);
    assert_eq!(
        desk.answer(LISTENER, front, Outcome::NotThere),
        [
            queued(vec![UtteranceSegment::label("Renamed")]),
            Heard::Sent(Reader::Listener, "ReadObject"),
        ]
    );
    assert_eq!(
        desk.reply(Reply::Object(Box::new(listener_read(ONE, 4)))),
        [Heard::Sent(Reader::Outpost(ONE.pid), "CompleteUia")]
    );
    let said = focus_heard(
        ONE,
        focus_now_in(ONE, 4, &[WINDOW, PANE]),
        vec![vec![UtteranceSegment::label("Second"), role(Role::Button)]],
    );
    assert_eq!(
        desk.reply(Reply::Focus(Box::new(focus_read_of(
            ONE,
            &node(4, Role::Button, "Second", focused()),
            &line_ancestors(),
        )))),
        said
    );
}
