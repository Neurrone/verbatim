//! The outpost's outbound merge against Core (`docs/design/ownership.md`): for every kind of event the outbound queue
//! merges, every sequence of the older report, up to two other messages
//! from a fixed set, and the newer report is reduced in order on one side,
//! and on the other with the others first and the merged report at the
//! newer one's place, as [`merge_into`] places it. Both leave Core in the
//! same state, and the others say the same; the merged report says what
//! Core says for it today. The older report's own speech is left out, since
//! dropping it is what the merge is for.

use serde_json::Value;
use verbatim_core::SrState;
use verbatim_model::protocol::{
    EventTiming, Merge, OutpostToCore, QueryOutcome, QueryResult, event_input, merge_into,
};
use verbatim_model::{
    Backend, CaretReport, Earcon, Effect, FetchResult, Input, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, ProgressReading, PropertyChange, QueryId, QueryKind,
    Rect, Role, SegmentContent, SelectionChange, SelectionText, SpeechPriority, State, StateSet,
    TextAnchor, TextChunk, TextPosition, TextUnit, TraceId, Utterance, UtteranceSegment,
    WindowFacts, WindowHandle,
};

/// The application, and its outpost.
const SOURCE: Pid = Pid(40);
const OUTPOST: OutpostId = OutpostId(4);

fn id(number: u64) -> NodeId {
    NodeId::in_outpost(OUTPOST, number)
}

fn node(number: u64, role: Role, name: &str, states: StateSet) -> NodeSnapshot {
    NodeSnapshot {
        id: id(number),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states,
        details: NodeDetails::default(),
    }
}

/// The application's window, in the foreground.
fn window() -> WindowFacts {
    WindowFacts {
        top_level: WindowHandle(100),
        root_owner: WindowHandle(100),
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        class: String::new(),
        application: String::new(),
        alt_held: false,
    }
}

/// The outpost's report of `event`.
fn report(event: NormalizedEvent) -> OutpostToCore {
    OutpostToCore::Event {
        trace_id: TraceId::mint(),
        observed_at_us: 0,
        backend: Backend::Uia,
        window: Some(window()),
        application: None,
        timing: EventTiming::default(),
        event,
    }
}

fn focused() -> StateSet {
    StateSet::new().with(State::Focused).with(State::Focusable)
}

/// The focus's line: the window, and the pane in it.
fn line_ancestors() -> Vec<NodeSnapshot> {
    vec![
        node(1, Role::Window, "Editor", StateSet::new()),
        node(2, Role::Pane, "Pane", StateSet::new()),
    ]
}

/// A focus on `node`, in the focus's line.
fn focus_on(node: NodeSnapshot) -> NormalizedEvent {
    NormalizedEvent::FocusChanged {
        node,
        foreground: false,
        ancestors: line_ancestors(),
        selected_child: None,
        text: None,
        earlier: None,
        group: None,
    }
}

/// A line of the focus's text with the caret at `offset`.
fn text_line(offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: "alpha beta\n".to_owned(),
        start: TextAnchor(100),
        offset,
        languages: Vec::new(),
        first: true,
        last: true,
        truncated: false,
        formats: Vec::new(),
    }
}

/// A caret report of the focus at `offset`, with `changes` to its
/// selection.
fn caret(offset: u32, changes: &[(bool, &str)]) -> NormalizedEvent {
    NormalizedEvent::CaretMoved {
        node_id: id(3),
        caret: CaretReport {
            line: text_line(offset),
            selection: None,
            selection_line: None,
        },
        selection_changes: changes
            .iter()
            .map(|&(selected, text)| SelectionChange {
                selected,
                text: text.to_owned(),
                characters: u32::try_from(text.len()).expect("short"),
            })
            .collect(),
        content_changed: false,
        new_selection: None,
    }
}

/// Reduces `input` into `state`, returning what it said and played.
fn reduce(state: &mut SrState, input: &Input) -> Vec<Effect> {
    verbatim_core::reduce(state, input)
        .into_iter()
        .filter(|effect| matches!(effect, Effect::Speak(..) | Effect::PlayEarcon(_)))
        .collect()
}

/// The fixed start: Core's focus on the edit field "Notes" in the pane in
/// the window, all in one outpost, with its caret at the start of its
/// line.
fn start() -> SrState {
    let mut state = SrState::new();
    for message in [
        report(focus_on(node(3, Role::EditableText, "Notes", focused()))),
        report(caret(0, &[])),
    ] {
        let input = event_input(SOURCE, message).expect("an event");
        let _ = reduce(&mut state, &input);
    }
    state
}

/// One of the messages that may come between the older report and the
/// newer, none of which concerns either report's node or line.
#[derive(Clone, Copy, Debug)]
enum Other {
    /// An alert of a node outside both lines, which Core speaks from
    /// anywhere.
    Alert,
    /// A reply to a query Core is not waiting for.
    Reply,
    /// A fault, which the app logs.
    Fault,
}

impl Other {
    /// The message, and the input the app makes of it.
    fn message(self) -> (OutpostToCore, Option<Input>) {
        match self {
            Other::Alert => {
                let message = report(NormalizedEvent::Alert {
                    node: node(9, Role::Alert, "Saved", StateSet::new()),
                });
                let input = event_input(SOURCE, message.clone());
                (message, input)
            }
            Other::Reply => {
                let trace_id = TraceId::mint();
                let message = OutpostToCore::Reply {
                    trace_id,
                    request_id: 77,
                    outcome: QueryOutcome::Done(QueryResult::Navigated(None)),
                    timing: EventTiming::default(),
                };
                let input = Input::FetchCompleted {
                    trace_id,
                    query_id: QueryId(77),
                    kind: QueryKind::NextSibling,
                    result: FetchResult::NoNeighbor,
                };
                (message, Some(input))
            }
            Other::Fault => (
                OutpostToCore::Fault {
                    detail: "a fault".to_owned(),
                },
                None,
            ),
        }
    }
}

/// Every sequence of zero to two others, each from the fixed set: 13.
fn sequences() -> Vec<Vec<Other>> {
    let set = [Other::Alert, Other::Reply, Other::Fault];
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

/// What the equivalence compares of Core's state: its focus and line,
/// with each node's states, name and value, the nodes it holds, the last
/// selection it announced, and its caret's node and position.
fn projection(state: &SrState) -> Value {
    let whole = serde_json::to_value(state).expect("the state serializes");
    let focus = &whole["focus"];
    let caret = &whole["caret"];
    serde_json::json!({
        "focus": focus["snapshot"],
        "line": focus["ancestors"],
        "last_selection": focus["last_selection"],
        "held": format!("{:?}", state.held_nodes()),
        "caret_node": caret["node"],
        "caret_at": [caret["line"]["start"], caret["line"]["offset"]],
    })
}

/// What one sequence did on each side.
struct Outcome {
    /// Whether the newer report was merged into the older.
    merged: bool,
    /// What the merged report said, when it was merged.
    merged_said: Vec<Effect>,
}

/// Runs `others` between `older` and `newer` on both sides and asserts
/// what both sides share.
fn run(older: &OutpostToCore, others: &[Other], newer: &OutpostToCore) -> Outcome {
    let others: Vec<(OutpostToCore, Option<Input>)> =
        others.iter().map(|other| other.message()).collect();
    let input = |message: &OutpostToCore| event_input(SOURCE, message.clone()).expect("an event");

    // In order, as Core reads them unmerged.
    let mut unmerged = start();
    let _ = reduce(&mut unmerged, &input(older));
    let unmerged_said: Vec<Vec<Effect>> = others
        .iter()
        .map(|(_, other)| {
            other
                .as_ref()
                .map(|other| reduce(&mut unmerged, other))
                .unwrap_or_default()
        })
        .collect();
    let _ = reduce(&mut unmerged, &input(newer));

    // As the outbound queue places the newer report.
    let mut waiting: Vec<OutpostToCore> = vec![older.clone()];
    waiting.extend(others.iter().map(|(message, _)| message.clone()));
    let place = merge_into(waiting.iter().enumerate(), newer);
    let mut merged_state = start();
    if place.is_none() {
        let _ = reduce(&mut merged_state, &input(older));
    } else {
        assert_eq!(place, Some(0), "the newer report merges into the older");
    }
    let merged_said_others: Vec<Vec<Effect>> = others
        .iter()
        .map(|(_, other)| {
            other
                .as_ref()
                .map(|other| reduce(&mut merged_state, other))
                .unwrap_or_default()
        })
        .collect();
    let last = match place {
        Some(_) => OutpostToCore::merged(older.clone(), newer.clone()),
        None => newer.clone(),
    };
    let merged_said = reduce(&mut merged_state, &input(&last));

    assert_eq!(
        projection(&unmerged),
        projection(&merged_state),
        "Core's state, with {others:?}"
    );
    assert_eq!(
        unmerged_said, merged_said_others,
        "what the others said, with {others:?}"
    );
    Outcome {
        merged: place.is_some(),
        merged_said,
    }
}

/// The words and sounds of `effects`, without their traces.
fn heard(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Speak(
                Utterance {
                    priority, segments, ..
                },
                _,
            ) => format!("{priority:?}: {segments:?}"),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Runs every sequence between `older` and `newer`, asserting that the
/// newer report merges into the older in each, unless `refused` says the
/// sequence's others bar it, and that the merged report says `said`.
fn equivalent(
    older: &OutpostToCore,
    newer: &OutpostToCore,
    refused: impl Fn(&[Other]) -> bool,
    said: &[String],
) {
    for others in sequences() {
        let outcome = run(older, &others, newer);
        assert_eq!(outcome.merged, !refused(&others), "merged with {others:?}");
        if outcome.merged {
            assert_eq!(heard(&outcome.merged_said), said, "with {others:?}");
        }
    }
}

/// The merge is never refused.
fn never(_: &[Other]) -> bool {
    false
}

fn segments(segments: &[UtteranceSegment]) -> String {
    format!("{segments:?}")
}

fn queued(spoken: &[UtteranceSegment]) -> String {
    format!("{:?}: {}", SpeechPriority::Queued, segments(spoken))
}

fn interrupting(spoken: &[UtteranceSegment]) -> String {
    format!("{:?}: {}", SpeechPriority::Interrupt, segments(spoken))
}

fn played(earcon: Earcon) -> String {
    format!("{:?}", Effect::PlayEarcon(earcon))
}

fn role(role: Role) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Role(role))
}

fn selected(text: &str) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Phrase(Phrase::Selected(
        SelectionText::Text(text.to_owned()),
    )))
}

#[test]
fn a_focus_merged_is_the_newer_focus() {
    let older = report(focus_on(node(4, Role::Button, "First", focused())));
    let newer = report(focus_on(node(4, Role::Button, "Second", focused())));
    equivalent(
        &older,
        &newer,
        never,
        &[queued(&[
            UtteranceSegment::label("Second"),
            role(Role::Button),
        ])],
    );
}

/// A property change of the focus, from `older` to `newer`, merged.
fn property(older: PropertyChange, newer: PropertyChange, said: &[String]) {
    let change = |change| {
        report(NormalizedEvent::PropertyChanged {
            node_id: id(3),
            change,
            child_count: None,
        })
    };
    equivalent(&change(older), &change(newer), never, said);
}

#[test]
fn a_name_change_merged_is_the_newer_name() {
    property(
        PropertyChange::Name(Some("Draft".to_owned())),
        PropertyChange::Name(Some("Final".to_owned())),
        &[queued(&[UtteranceSegment::label("Final")])],
    );
}

#[test]
fn a_value_property_change_merged_is_the_newer_value() {
    property(
        PropertyChange::Value(Some("one".to_owned())),
        PropertyChange::Value(Some("two".to_owned())),
        &[],
    );
}

#[test]
fn a_states_change_merged_is_the_newer_states() {
    property(
        PropertyChange::States(focused().with(State::ReadOnly)),
        PropertyChange::States(focused().with(State::Required)),
        &[queued(&[UtteranceSegment::new(SegmentContent::State(
            State::Required,
        ))])],
    );
}

#[test]
fn a_description_change_merged_is_the_newer_description() {
    property(
        PropertyChange::Description(Some("Older".to_owned())),
        PropertyChange::Description(Some("Newer".to_owned())),
        &[queued(&[UtteranceSegment::new(
            SegmentContent::Description("Newer".to_owned()),
        )])],
    );
}

#[test]
fn a_suggestions_change_merged_is_the_newer_one() {
    property(
        PropertyChange::Suggestions(false),
        PropertyChange::Suggestions(true),
        &[played(Earcon::SuggestionsOpened)],
    );
}

#[test]
fn a_selection_merged_is_the_newer_selection() {
    let selected = |name: &str| {
        report(NormalizedEvent::SelectionChanged {
            node: node(
                5,
                Role::ListItem,
                name,
                StateSet::new().with(State::Selected),
            ),
        })
    };
    equivalent(&selected("One"), &selected("One, renamed"), never, &[]);
}

#[test]
fn a_controlled_selection_merged_is_the_newer_one() {
    let selected = |name: &str| {
        report(NormalizedEvent::ControlledSelection {
            controller: id(3),
            node: node(
                6,
                Role::ListItem,
                name,
                StateSet::new().with(State::Selected),
            ),
        })
    };
    equivalent(
        &selected("Suggestion one"),
        &selected("Suggestion two"),
        never,
        &[interrupting(&[
            UtteranceSegment::label("Suggestion two"),
            UtteranceSegment::new(SegmentContent::State(State::Selected)),
        ])],
    );
}

#[test]
fn an_alert_merged_is_the_newer_alert() {
    let alert = |name: &str| {
        report(NormalizedEvent::Alert {
            node: node(7, Role::Alert, name, StateSet::new()),
        })
    };
    equivalent(
        &alert("Copied"),
        &alert("Copied twice"),
        never,
        &[queued(&[
            UtteranceSegment::label("Copied twice"),
            role(Role::Alert),
        ])],
    );
}

#[test]
fn a_progress_bar_merged_is_the_newer_reading() {
    let progress = |value: &str| {
        report(NormalizedEvent::ProgressChanged {
            bar: ProgressReading {
                id: id(8),
                value: Some(value.to_owned()),
                offscreen: false,
                rect: Some(Rect {
                    left: 0,
                    top: 0,
                    width: 100,
                    height: 10,
                }),
            },
        })
    };
    equivalent(
        &progress("50"),
        &progress("60"),
        never,
        &[played(Earcon::Progress(60))],
    );
}

#[test]
fn a_value_change_merged_is_the_newer_value() {
    let value = |value: &str| {
        report(NormalizedEvent::ValueChanged {
            node_id: id(3),
            value: Some(value.to_owned()),
        })
    };
    equivalent(&value("50"), &value("51"), never, &[]);
}

/// A caret report that carries selection changes merges with a newer one
/// except across a reply, which the reply's placements among the others
/// test; the merged report carries the older's changes before its own.
#[test]
fn a_caret_report_merged_carries_both_selections_and_never_crosses_a_reply() {
    let older = report(caret(5, &[(true, "alpha")]));
    let newer = report(caret(10, &[(true, " beta")]));
    equivalent(
        &older,
        &newer,
        |others| others.iter().any(|other| matches!(other, Other::Reply)),
        &[queued(&[selected("alpha")]), queued(&[selected(" beta")])],
    );
}

#[test]
fn a_text_change_merged_is_the_newer_one() {
    let changed = || report(NormalizedEvent::TextChanged { node_id: id(3) });
    equivalent(&changed(), &changed(), never, &[]);
}

#[test]
fn an_active_text_position_change_merged_is_the_newer_one() {
    let moved = |offset| {
        report(NormalizedEvent::ActiveTextPositionChanged {
            node_id: id(3),
            position: TextPosition {
                anchor: TextAnchor(100),
                offset,
            },
        })
    };
    equivalent(&moved(2), &moved(6), never, &[]);
}

/// What the merged side said, in order, for `between`, a report of the
/// merged reports' node or line queued between them, which is not in the
/// equivalence's set: the merge moves the newer report behind it, the
/// merge's reordering.
fn reordered(older: &OutpostToCore, between: &OutpostToCore, newer: &OutpostToCore) -> Vec<String> {
    let waiting = [older.clone(), between.clone()];
    assert_eq!(merge_into(waiting.iter().enumerate(), newer), Some(0));
    let mut state = start();
    [
        between.clone(),
        OutpostToCore::merged(older.clone(), newer.clone()),
    ]
    .into_iter()
    .flat_map(|message| {
        heard(&reduce(
            &mut state,
            &event_input(SOURCE, message).expect("an event"),
        ))
    })
    .collect()
}

/// A name change of the focus the older report leaves, queued between two
/// reports of the new focus, reaches Core before the merged focus, so Core
/// judges it against the focus before: it is spoken as that focus's
/// change, and the new focus announced after it.
#[test]
fn a_focus_merged_across_a_change_of_its_line_moves_behind_it() {
    let older = report(focus_on(node(4, Role::Button, "First", focused())));
    let newer = report(focus_on(node(4, Role::Button, "Second", focused())));
    let between = report(NormalizedEvent::PropertyChanged {
        node_id: id(3),
        change: PropertyChange::Name(Some("Renamed".to_owned())),
        child_count: None,
    });
    assert_eq!(
        reordered(&older, &between, &newer),
        [
            queued(&[UtteranceSegment::label("Renamed")]),
            queued(&[UtteranceSegment::label("Second"), role(Role::Button)]),
        ]
    );
}

/// A value change of the caret's node queued between two caret reports
/// reaches Core before the merged report, unspoken, as an edit field's
/// value change is; the merged report then speaks both selections.
#[test]
fn a_caret_report_merged_across_a_change_of_its_node_moves_behind_it() {
    let older = report(caret(5, &[(true, "alpha")]));
    let newer = report(caret(10, &[(true, " beta")]));
    let between = report(NormalizedEvent::ValueChanged {
        node_id: id(3),
        value: Some("alpha beta".to_owned()),
    });
    assert_eq!(
        reordered(&older, &between, &newer),
        [queued(&[selected("alpha")]), queued(&[selected(" beta")])]
    );
}
