//! Tests of Core's queries: each of their endings, and every sequence of
//! up to three steps from two start states, enumerated, against the
//! table's invariants.

use std::collections::BTreeMap;

use verbatim_model::protocol::{QueryOutcome, QueryResult};
use verbatim_model::{
    Backend, CaretChanges, CaretKey, CaretMotion, CaretReply, CaretReport, EarlierElement, Effect,
    FocusText, Input, KeySeq, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid,
    QueryId, ReviewCommand, Role, StateSet, TextAnchor, TextChunk, TextOp, TextReply, TextUnit,
    TraceId,
};

use crate::queries::Then;
use crate::reduce;
use crate::state::SrState;

/// The text box's outpost.
const EDIT_OUTPOST: OutpostId = OutpostId(1);
/// Every held key's trace, so `HeldKeyDue` names the key held last.
pub(super) const HELD: TraceId = TraceId::from_raw(50);

pub(super) fn node(outpost: OutpostId, number: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::in_outpost(outpost, number),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

pub(super) fn line(text: &str, anchor: u64, offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: text.to_owned(),
        start: TextAnchor(anchor),
        offset,
        languages: Vec::new(),
        first: true,
        last: true,
        truncated: false,
        formats: Vec::new(),
    }
}

pub(super) fn caret(anchor: u64, offset: u32) -> CaretReport {
    CaretReport {
        line: line("hello world", anchor, offset),
        selection: None,
        read_at_us: 0,
    }
}

fn event(source: OutpostId, event: NormalizedEvent) -> Input {
    Input::Event {
        trace_id: TraceId::from_raw(1),
        observed_at_us: 0,
        source: Pid(u32::try_from(source.0).expect("small")),
        backend: Backend::Uia,
        window: None,
        event,
        purpose: None,
    }
}

/// A focus on `snapshot`, with its caret when it has text.
fn focus(
    snapshot: NodeSnapshot,
    text: Option<FocusText>,
    earlier: Option<EarlierElement>,
) -> Input {
    let outpost = snapshot.id.outpost();
    event(
        outpost,
        NormalizedEvent::FocusChanged {
            node: snapshot,
            whole_window: false,
            ancestors: vec![node(outpost, 1, Role::Window, "Window")],
            selected_child: None,
            text: text.map(Box::new),
            earlier,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        },
    )
}

/// The text box focused with its caret, on [`EDIT_OUTPOST`].
fn text_box() -> SrState {
    text_box_wired().0
}

/// [`text_box`], with the wire's view of the requests it made.
fn text_box_wired() -> (SrState, Wire) {
    let (state, wire) = started(&[focus(
        node(EDIT_OUTPOST, 5, Role::EditableText, "Notes"),
        Some(FocusText::Caret {
            caret: caret(100, 3),
            selected_text: None,
            changes: CaretChanges::default(),
            facts: None,
        }),
        None,
    )]);
    assert!(
        state
            .focus_caret(NodeId::in_outpost(EDIT_OUTPOST, 5))
            .is_some(),
        "the text box has its caret"
    );
    (state, wire)
}

/// [`text_box_wired`] after a held key's read was answered, so the key is
/// passed on and its watch waited on.
fn held_key_passed_on_wired() -> (SrState, Wire) {
    let (mut state, mut wire) = text_box_wired();
    let held = Input::HeldKey {
        trace_id: TraceId::from_raw(80),
        key_seq: KeySeq::NONE_YET,
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 500,
    };
    let effects = reduce(&mut state, &mut crate::Runtime::default(), &held);
    wire.apply(&held, &effects);
    let asked = requests(&effects);
    let [(read, Some(op @ TextOp::ReadCaret(_)))] = &asked[..] else {
        panic!("one read, not {asked:?}");
    };
    let answer = answered(
        *read,
        EDIT_OUTPOST,
        QueryOutcome::Done(fitting(EDIT_OUTPOST, &Asked::Text(op.clone()))),
    );
    let effects = reduce(&mut state, &mut crate::Runtime::default(), &answer);
    wire.apply(&answer, &effects);
    assert!(state.holds_inputs_back(), "its watch is waited on");
    (state, wire)
}

/// The state `inputs` make from the default, and the wire's view of the
/// requests they made.
fn started(inputs: &[Input]) -> (SrState, Wire) {
    let mut state = SrState::new();
    let mut wire = Wire::default();
    for input in inputs {
        let effects = reduce(&mut state, &mut crate::Runtime::default(), input);
        wire.apply(input, &effects);
    }
    (state, wire)
}

/// One step of the enumeration: an input, or several applied as one.
#[derive(Clone, Debug)]
enum Step {
    CaretKey,
    HeldKey,
    HeldKeyDue,
    ReadCurrentLine,
    Navigate,
    ToFocus,
    SayAll,
    SpeechCut,
    Typed,
    FocusElsewhere,
    Refocus,
    OutpostEnded,
    /// An answer to the oldest open number, the newest, the last ended or
    /// one never asked, with an outcome of each group.
    Answer(Which, Outcome),
    /// An answer to the newest open number from the other outpost.
    FromElsewhere,
}

#[derive(Clone, Copy, Debug)]
enum Which {
    Oldest,
    Newest,
    LastEnded,
    NeverAsked,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Outcome {
    Done,
    Gone,
    Failed,
}

impl Step {
    /// The step as the Rust expression that makes it, as a found case
    /// lists it.
    fn code(&self) -> String {
        match self {
            Step::Answer(which, outcome) => {
                format!("Step::Answer(Which::{which:?}, Outcome::{outcome:?})")
            }
            other => format!("Step::{other:?}"),
        }
    }
}

fn steps() -> Vec<Step> {
    let mut steps = vec![
        Step::CaretKey,
        Step::HeldKey,
        Step::HeldKeyDue,
        Step::ReadCurrentLine,
        Step::Navigate,
        Step::ToFocus,
        Step::SayAll,
        Step::SpeechCut,
        Step::Typed,
        Step::FocusElsewhere,
        Step::Refocus,
        Step::OutpostEnded,
        Step::FromElsewhere,
    ];
    for which in [
        Which::Oldest,
        Which::Newest,
        Which::LastEnded,
        Which::NeverAsked,
    ] {
        for outcome in [Outcome::Done, Outcome::Gone, Outcome::Failed] {
            steps.push(Step::Answer(which, outcome));
        }
    }
    steps
}

/// The wire's view of Core's queries: the numbers in the request effects,
/// each with its operation, less those an accepted answer, an outpost's
/// end or a withdrawal ended. It knows no continuation and no
/// supersession.
#[derive(Clone, Debug, Default)]
pub(super) struct Wire {
    pub(super) open: BTreeMap<QueryId, (OutpostId, Asked)>,
    pub(super) last_ended: Option<(QueryId, OutpostId, Asked)>,
}

/// What a request asked, for an answer that fits it.
#[derive(Clone, Debug)]
pub(super) enum Asked {
    Navigate,
    Activate,
    ReadNode,
    Text(TextOp),
}

impl Wire {
    fn take(&mut self, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::ForKey(_, inner) => self.take(std::slice::from_ref(&**inner)),
                Effect::Fetch(query) => {
                    self.open
                        .insert(query.query_id, (query.node_id.outpost(), Asked::Navigate));
                }
                Effect::Activate { query_id, node_id } => {
                    self.open
                        .insert(*query_id, (node_id.outpost(), Asked::Activate));
                }
                Effect::ReadNode {
                    query_id, node_id, ..
                } => {
                    self.open
                        .insert(*query_id, (node_id.outpost(), Asked::ReadNode));
                }
                Effect::Text(request) => {
                    self.open.insert(
                        request.query_id,
                        (request.node_id.outpost(), Asked::Text(request.op.clone())),
                    );
                }
                _ => {}
            }
        }
    }

    fn end(&mut self, query_id: QueryId) {
        if let Some((outpost, asked)) = self.open.remove(&query_id) {
            self.last_ended = Some((query_id, outpost, asked));
        }
    }

    fn outpost_ended(&mut self, outpost: OutpostId) {
        let ended: Vec<QueryId> = self
            .open
            .iter()
            .filter(|(_, (asked, _))| *asked == outpost)
            .map(|(query_id, _)| *query_id)
            .collect();
        for query_id in ended {
            self.end(query_id);
        }
    }

    /// The inputs of `step`, given the wire's view now; none when it does
    /// not apply.
    fn inputs(&self, start: OutpostId, step: &Step) -> Vec<Input> {
        let trace = TraceId::from_raw(60);
        let key = |motion| CaretKey {
            motion,
            select: false,
        };
        let command = |command| Input::Command {
            trace_id: TraceId::from_raw(61),
            key_seq: KeySeq::NONE_YET,
            command,
            pressed_at_ms: 0,
            repeat: 0,
        };
        let start_node = node(start, 5, start_role(start), "Start");
        match step {
            Step::CaretKey => vec![Input::CaretKey {
                trace_id: trace,
                key_seq: KeySeq::NONE_YET,
                key: key(CaretMotion::NextCharacter),
                pressed_at_ms: 1_000,
            }],
            Step::HeldKey => vec![Input::HeldKey {
                trace_id: HELD,
                key_seq: KeySeq::NONE_YET,
                key: key(CaretMotion::NextCharacter),
                pressed_at_ms: 1_000,
            }],
            Step::HeldKeyDue => vec![Input::HeldKeyDue {
                trace_id: HELD,
                at_us: 1_500_000,
            }],
            Step::ReadCurrentLine => vec![command(ReviewCommand::ReviewCurrentLine)],
            Step::Navigate => vec![command(ReviewCommand::Parent)],
            Step::ToFocus => vec![command(ReviewCommand::ToFocus)],
            Step::SayAll => vec![command(ReviewCommand::SayAllFromCaret)],
            Step::SpeechCut => vec![Input::SpeechCancelled { at_us: 2_000_000 }],
            Step::Typed => vec![Input::CharacterTyped {
                trace_id: trace,
                key_seq: KeySeq::NONE_YET,
                text: "a".to_owned(),
                at_ms: 1_000,
            }],
            Step::FocusElsewhere => vec![focus(node(start, 6, Role::Button, "OK"), None, None)],
            Step::Refocus => vec![focus(
                start_node,
                (start == EDIT_OUTPOST).then(|| FocusText::Caret {
                    caret: caret(200, 4),
                    selected_text: None,
                    changes: CaretChanges::default(),
                    facts: None,
                }),
                Some(EarlierElement::NotFocused),
            )],
            Step::OutpostEnded => vec![Input::OutpostEnded {
                trace_id: TraceId::from_raw(62),
                outpost: start,
            }],
            Step::Answer(which, outcome) => {
                let chosen = match which {
                    Which::Oldest => {
                        self.open.iter().next().map(|(query_id, (outpost, asked))| {
                            (*query_id, *outpost, asked.clone())
                        })
                    }
                    Which::Newest => self
                        .open
                        .iter()
                        .next_back()
                        .map(|(query_id, (outpost, asked))| (*query_id, *outpost, asked.clone())),
                    Which::LastEnded => self.last_ended.clone(),
                    Which::NeverAsked => Some((QueryId(10_000), start, Asked::Text(key_op()))),
                };
                chosen
                    .map(|(query_id, outpost, asked)| answer(query_id, outpost, &asked, *outcome))
                    .into_iter()
                    .collect()
            }
            Step::FromElsewhere => self
                .open
                .iter()
                .next_back()
                .map(|(query_id, (outpost, asked))| {
                    let elsewhere = OutpostId(outpost.0 + 10);
                    answer(*query_id, elsewhere, asked, Outcome::Done)
                })
                .into_iter()
                .collect(),
        }
    }

    /// Takes in what `input` ended, and the requests `effects` made.
    pub(super) fn apply(&mut self, input: &Input, effects: &[Effect]) {
        match input {
            Input::Answered {
                at_us: 0,
                query_id,
                outpost,
                ..
            } if self
                .open
                .get(query_id)
                .is_some_and(|(asked, _)| asked == outpost) =>
            {
                self.end(*query_id);
            }
            Input::OutpostEnded { outpost, .. } => self.outpost_ended(*outpost),
            _ => {}
        }
        self.take(effects);
    }
}

fn start_role(start: OutpostId) -> Role {
    if start == EDIT_OUTPOST {
        Role::EditableText
    } else {
        Role::Terminal
    }
}

/// An answer to `query_id`, which asked `asked`, from `outpost`.
pub(super) fn answer(
    query_id: QueryId,
    outpost: OutpostId,
    asked: &Asked,
    outcome: Outcome,
) -> Input {
    let outcome = match outcome {
        Outcome::Gone => QueryOutcome::Gone,
        Outcome::Failed => QueryOutcome::Failed("failed".to_owned()),
        Outcome::Done => QueryOutcome::Done(fitting(outpost, asked)),
    };
    Input::Answered {
        at_us: 0,
        trace_id: TraceId::from_raw(70),
        query_id,
        outpost,
        outcome,
    }
}

/// A key's request, as the answers to one are made for.
pub(super) fn key_op() -> TextOp {
    TextOp::ReadCaret(verbatim_model::CaretRead {
        fetch: verbatim_model::KeyFetch {
            unit: TextUnit::Character,
            motion: CaretMotion::NextCharacter,
            details: verbatim_model::TextDetails::ALL,
        },
        purpose: verbatim_model::ReadPurpose::BeforeFieldKey,
    })
}

/// A result that fits what was asked.
fn fitting(outpost: OutpostId, asked: &Asked) -> QueryResult {
    let caret_reply = || {
        TextReply::Caret(Box::new(CaretReply {
            caret: caret(300, 5),
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
            facts: verbatim_model::ReadFacts {
                unit: Some(line("hello world", 300, 5)),
                change: verbatim_model::Change::NoPrevious,
                read_ended_at_us: 1,
            },
        }))
    };
    match asked {
        Asked::Navigate => QueryResult::Navigated(Some(node(outpost, 1, Role::Window, "Window"))),
        Asked::Activate => QueryResult::Activated(None),
        Asked::ReadNode => QueryResult::Node(node(outpost, 1, Role::Window, "Window")),
        Asked::Text(op) => QueryResult::Text(match op {
            TextOp::ReadCaret(_) => caret_reply(),
            TextOp::Read(_) => TextReply::Read {
                moved: 0,
                chunk: line("hello world", 300, 0),
            },
            TextOp::ReadAhead(_) => TextReply::Chunks {
                moved: 0,
                chunks: vec![line("hello world", 300, 0)],
            },
            TextOp::TerminalHistory { .. } => TextReply::TerminalHistory {
                text: String::new(),
                above: 0,
                read: 2,
            },
            TextOp::TerminalFind { .. } => TextReply::TerminalFound {
                matches: Vec::new(),
                read: 2,
            },
            TextOp::Location(_) => TextReply::NoText,
            _ => TextReply::Unsupported,
        }),
    }
}

/// What one input of a sequence did.
pub(super) struct Visit<'a> {
    pub(super) before: &'a SrState,
    pub(super) input: &'a Input,
    pub(super) state: &'a SrState,
    pub(super) effects: &'a [Effect],
    pub(super) wire: &'a Wire,
}

/// Runs every sequence of up to `depth` steps from `start`, calling
/// `visit` after each input with its sequence; the state is cloned at each
/// step, so sequences sharing a prefix share its steps.
fn enumerate(
    (start, wire): &(SrState, Wire),
    outpost: OutpostId,
    depth: usize,
    visit: &mut dyn FnMut(&[usize], Visit<'_>),
) {
    fn go(
        state: &SrState,
        wire: &Wire,
        outpost: OutpostId,
        path: &mut Vec<usize>,
        depth: usize,
        steps: &[Step],
        visit: &mut dyn FnMut(&[usize], Visit<'_>),
    ) {
        if path.len() == depth {
            return;
        }
        for (index, step) in steps.iter().enumerate() {
            let inputs = wire.inputs(outpost, step);
            if inputs.is_empty() {
                continue;
            }
            path.push(index);
            let mut state = state.clone();
            let mut wire = wire.clone();
            for input in &inputs {
                let before = state.clone();
                let effects = reduce(&mut state, &mut crate::Runtime::default(), input);
                wire.apply(input, &effects);
                visit(
                    path,
                    Visit {
                        before: &before,
                        input,
                        state: &state,
                        effects: &effects,
                        wire: &wire,
                    },
                );
            }
            go(&state, &wire, outpost, path, depth, steps, visit);
            path.pop();
        }
    }
    let steps = steps();
    go(start, wire, outpost, &mut Vec::new(), depth, &steps, visit);
}

/// The nodes `state` holds as the table says it should: the focus, its
/// ancestors and last selection, the navigator, the caret, the start
/// marker and say-all, with the node an open navigation starts from and
/// the nodes of the open caret keys', text commands' and held keys'
/// queries.
fn nodes_held(state: &SrState) -> BTreeMap<OutpostId, std::collections::BTreeSet<NodeId>> {
    let mut held: BTreeMap<OutpostId, std::collections::BTreeSet<NodeId>> = BTreeMap::new();
    let mut insert = |id: NodeId| {
        held.entry(id.outpost()).or_default().insert(id);
    };
    if let Some(focus) = state.focus.object().filter(|focus| focus.alive) {
        insert(focus.snapshot.id);
        focus
            .ancestors
            .iter()
            .for_each(|ancestor| insert(ancestor.id));
        focus.last_selection.into_iter().for_each(&mut insert);
    }
    if let Some(navigator) = &state.navigator {
        insert(navigator.object.id);
    }
    insert_key_line(state, &mut insert);
    if let Some(marker) = state.start_marker {
        insert(marker.node());
    }
    if let Some(say_all) = &state.say_all {
        insert(say_all.node);
    }
    for (_, open) in state.queries.entries() {
        match &open.then {
            Then::Key { node } => insert(*node),
            Then::Text(pending) => insert(pending.node),
            _ => {}
        }
    }
    held
}

/// Inserts the node of the key in `state`'s line, which Core holds.
fn insert_key_line(state: &SrState, insert: &mut impl FnMut(NodeId)) {
    state.key_line_node().into_iter().for_each(insert);
}

/// Which kind of continuation `then` is, for the rule that at most one of
/// each is open: `None` for those of which several may be.
fn one_of(then: &Then) -> Option<String> {
    match then {
        Then::Text(_) => Some("text".to_owned()),
        Then::SayAll => Some("say-all".to_owned()),
        Then::Key { .. } | Then::MovedCaret { .. } | Then::Review(_) | Then::Nothing => None,
    }
}

/// Checks the table's invariants after `visit`, against the wire's view.
pub(super) fn check(path: &[usize], visit: &Visit<'_>) {
    let Visit {
        before,
        input,
        state,
        effects,
        wire,
    } = visit;
    let at = format!("after {path:?}, {input:?}");
    // The idle barrier waits for exactly the reads the wire has open.
    let reads = wire.open.len();
    assert_eq!(state.outstanding_queries(), reads, "{at}");
    for (query_id, _) in state.queries.entries() {
        // Every query is open on the wire.
        assert!(wire.open.contains_key(&query_id), "{query_id:?} {at}");
    }
    // An outpost's end leaves none of its queries open.
    if let Input::OutpostEnded { outpost, .. } = input {
        assert!(
            state
                .queries
                .entries()
                .all(|(_, open)| open.outpost != *outpost),
            "{at}"
        );
    }
    // An answer the table does not take, to no open query or from another
    // outpost, changes nothing; one to a query nothing waits for changes
    // nothing but the table.
    if let Input::Answered {
        query_id, outpost, ..
    } = input
    {
        let open = before
            .queries
            .entries()
            .find(|(open, _)| open == query_id)
            .map(|(_, open)| open);
        let taken = open.is_some_and(|open| open.outpost == *outpost);
        let waited_for = open.is_some_and(|open| open.then != Then::Nothing);
        if !taken || !waited_for {
            let mut after = (*state).clone();
            if taken {
                after.queries = before.queries.clone();
            }
            assert_eq!(&after, *before, "{at}");
            assert_eq!(*effects, [], "{at}");
        }
    }
    // At most one open query of each kind.
    let mut kinds = std::collections::BTreeSet::new();
    for (_, open) in state.queries.entries() {
        if let Some(kind) = one_of(&open.then) {
            assert!(kinds.insert(kind.clone()), "two open {kind} {at}");
        }
    }
    assert_eq!(state.held_nodes(), nodes_held(state), "{at}");
}

/// A start of the enumeration: the function that makes it, its name, and
/// the outpost it reads, with its constant's name.
type Start = (Wired, &'static str, OutpostId, &'static str);

/// A function making a start of the enumeration.
type Wired = fn() -> (SrState, Wire);

/// The enumeration's starts.
const STARTS: [Start; 2] = [
    (
        text_box_wired,
        "text_box_wired",
        EDIT_OUTPOST,
        "EDIT_OUTPOST",
    ),
    (
        held_key_passed_on_wired,
        "held_key_passed_on_wired",
        EDIT_OUTPOST,
        "EDIT_OUTPOST",
    ),
];

/// Every sequence of up to three steps from a text box with its caret,
/// and from the same with a held key's watch waited on, keeps the table's
/// invariants after every input, against the
/// wire's view of the requests. A failing sequence is printed as a line
/// for [`found`] before the failure.
#[test]
fn every_sequence_keeps_the_query_tables_invariants() {
    let steps = steps();
    // Each depth in turn, so the sequence a failure prints is one of the
    // shortest that fail.
    for (depth, (start, start_name, outpost, outpost_name)) in
        (1..=3).flat_map(|depth| STARTS.map(|start| (depth, start)))
    {
        enumerate(&start(), outpost, depth, &mut |path, visit| {
            let line = || {
                let path: Vec<String> = path.iter().map(|&index| steps[index].code()).collect();
                found_report(&format!(
                    "({start_name}, {outpost_name}, vec![{}]),",
                    path.join(", ")
                ))
            };
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(path, &visit))) {
                Ok(()) => {}
                Err(payload) => {
                    eprintln!("{}", line());
                    std::panic::resume_unwind(payload);
                }
            }
        });
    }
}

/// The report of a failing sequence, given as `line`: where to paste it,
/// and the command that runs the found sequences.
fn found_report(line: &str) -> String {
    format!(
        "the failing sequence, as a line for `found()` in crates/verbatim-core/src/queries/tests.rs:\n        {line}\n\
         the found sequences alone: cargo test -p verbatim-core --lib -- --exact queries::tests::found_sequences_keep_the_query_tables_invariants"
    )
}

/// The sequences the enumeration failed on, each kept as its start, its
/// outpost and its steps, as the failure's report gives them.
fn found() -> Vec<(Wired, OutpostId, Vec<Step>)> {
    Vec::new()
}

/// Every found sequence keeps the table's invariants after every input.
#[test]
fn found_sequences_keep_the_query_tables_invariants() {
    for (start, outpost, sequence) in found() {
        let (mut state, mut wire) = start();
        for step in &sequence {
            for input in wire.inputs(outpost, step) {
                let before = state.clone();
                let effects = reduce(&mut state, &mut crate::Runtime::default(), &input);
                wire.apply(&input, &effects);
                check(
                    &[],
                    &Visit {
                        before: &before,
                        input: &input,
                        state: &state,
                        effects: &effects,
                        wire: &wire,
                    },
                );
            }
        }
    }
}

/// `command` pressed.
fn command(command: ReviewCommand) -> Input {
    Input::Command {
        trace_id: TraceId::from_raw(61),
        key_seq: KeySeq::NONE_YET,
        command,
        pressed_at_ms: 0,
        repeat: 0,
    }
}

/// The outpost's answer `outcome` to `query_id`, from `outpost`.
fn answered(query_id: QueryId, outpost: OutpostId, outcome: QueryOutcome) -> Input {
    Input::Answered {
        at_us: 0,
        trace_id: TraceId::from_raw(70),
        query_id,
        outpost,
        outcome,
    }
}

/// The one request `effects` make, inside a key's effect included: its
/// number and operation, `None` for a navigation or activation.
fn requests(effects: &[Effect]) -> Vec<(QueryId, Option<TextOp>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ForKey(_, inner) => requests(std::slice::from_ref(&**inner)).pop(),
            Effect::Fetch(query) => Some((query.query_id, None)),
            Effect::Activate { query_id, .. } => Some((*query_id, None)),
            Effect::Text(request) => Some((request.query_id, Some(request.op.clone()))),
            _ => None,
        })
        .collect()
}

pub(super) fn speaks(effects: &[Effect]) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::Speak(..) => true,
        Effect::ForKey(_, inner) => matches!(**inner, Effect::Speak(..)),
        _ => false,
    })
}

/// A text request's outcome other than an answer or a gone node reads as
/// unanswered: a review command's read of the caret's line says nothing
/// for it.
#[test]
fn a_text_requests_failure_reads_as_unanswered() {
    for outcome in [
        QueryOutcome::Abandoned,
        QueryOutcome::NotStarted,
        QueryOutcome::Failed("the read failed".to_owned()),
    ] {
        let mut state = text_box();
        let asked = requests(&reduce(
            &mut state,
            &mut crate::Runtime::default(),
            &command(ReviewCommand::ReviewCurrentLine),
        ));
        let [(query, Some(TextOp::Read(_)))] = asked[..] else {
            panic!("one read, not {asked:?}");
        };
        let effects = reduce(
            &mut state,
            &mut crate::Runtime::default(),
            &answered(query, EDIT_OUTPOST, outcome),
        );
        assert!(!speaks(&effects), "{effects:?}");
        assert_eq!(state.outstanding_queries(), 0);
    }
}
