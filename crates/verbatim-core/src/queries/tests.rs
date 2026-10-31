//! Tests of Core's queries: each of their endings, and every sequence of
//! three inputs from two start states, enumerated.

use std::collections::BTreeMap;

use verbatim_model::protocol::{QueryOutcome, QueryResult};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReply, CaretReport, EarlierElement, Effect, FocusText,
    Input, KeySeq, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid, QueryId,
    ReviewCommand, Role, StateSet, TerminalSnapshot, TerminalText, TextAnchor, TextChunk, TextOp,
    TextReply, TextUnit, TraceId,
};

use crate::reduce;
use crate::state::SrState;

/// The text box's outpost.
const EDIT_OUTPOST: OutpostId = OutpostId(1);
/// The console's outpost.
const CONSOLE_OUTPOST: OutpostId = OutpostId(2);
/// Every held key's trace, so `HeldKeyDue` names the key held last.
const HELD: TraceId = TraceId::from_raw(50);

fn node(outpost: OutpostId, number: u64, role: Role, name: &str) -> NodeSnapshot {
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

fn line(text: &str, anchor: u64, offset: u32) -> TextChunk {
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

fn caret(anchor: u64, offset: u32) -> CaretReport {
    CaretReport {
        line: line("hello world", anchor, offset),
        selection: None,
        selection_line: None,
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
            foreground: false,
            ancestors: vec![node(outpost, 1, Role::Window, "Window")],
            selected_child: None,
            text: text.map(Box::new),
            earlier,
            group: None,
        },
    )
}

/// A read of a console showing a prompt, its caret after it.
fn console_read(read: u64, at_ms: u64) -> TerminalSnapshot {
    TerminalSnapshot {
        text: TerminalText::unpadded("PS> "),
        screen: TerminalText::unpadded("PS> "),
        from_start: false,
        shrank: false,
        at_document_start: true,
        alternate: false,
        first_row: TerminalText::unpadded("PS> "),
        caret_rows_to_end: 1,
        caret_line: TerminalText::unpadded("PS> "),
        caret_column: 4,
        above: 0,
        started_ms: at_ms,
        ended_ms: at_ms,
        waited: false,
        read,
        colours: None,
        caret: Some(CaretReport {
            line: line("PS> ", 900, 4),
            selection: None,
            selection_line: None,
        }),
    }
}

fn snapshot_event(node_id: NodeId, read: u64, at_ms: u64) -> Input {
    event(
        node_id.outpost(),
        NormalizedEvent::TerminalSnapshot {
            node_id,
            snapshot: Box::new(console_read(read, at_ms)),
        },
    )
}

/// The text box focused with its caret, on [`EDIT_OUTPOST`].
fn text_box() -> SrState {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &focus(
            node(EDIT_OUTPOST, 5, Role::EditableText, "Notes"),
            Some(FocusText::Caret {
                caret: caret(100, 3),
                selected_text: None,
            }),
            None,
        ),
    );
    assert!(state.caret.is_some(), "the text box has its caret");
    state
}

/// The console Core reads, focused, with its model, on
/// [`CONSOLE_OUTPOST`].
fn console() -> SrState {
    let mut state = SrState::new();
    let terminal = node(CONSOLE_OUTPOST, 5, Role::Terminal, "Console");
    let id = terminal.id;
    let _ = reduce(&mut state, &focus(terminal, None, None));
    let _ = reduce(&mut state, &snapshot_event(id, 1, 10));
    assert_eq!(state.terminals.len(), 1, "the console has its model");
    state
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
    /// The first snapshots of 16 other terminals, which evict the
    /// console's model, then the focus back on the console.
    OtherTerminals,
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
enum Outcome {
    Done,
    Gone,
    Failed,
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
        Step::OtherTerminals,
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
struct Wire {
    open: BTreeMap<QueryId, (OutpostId, Asked)>,
    last_ended: Option<(QueryId, OutpostId, Asked)>,
}

/// What a request asked, for an answer that fits it.
#[derive(Clone, Debug)]
enum Asked {
    Navigate,
    Activate,
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
                Effect::Text(request) => {
                    self.open.insert(
                        request.query_id,
                        (request.node_id.outpost(), Asked::Text(request.op.clone())),
                    );
                }
                Effect::Withdraw { query_id, .. } => self.end(*query_id),
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
            Step::HeldKeyDue => vec![Input::HeldKeyDue { trace_id: HELD }],
            Step::ReadCurrentLine => vec![command(ReviewCommand::ReviewCurrentLine)],
            Step::Navigate => vec![command(ReviewCommand::Parent)],
            Step::ToFocus => vec![command(ReviewCommand::ToFocus)],
            Step::SayAll => vec![command(ReviewCommand::SayAllFromCaret)],
            Step::SpeechCut => vec![Input::SpeechCancelled { at_ms: 2_000 }],
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
                }),
                Some(EarlierElement::NotFocused),
            )],
            Step::OtherTerminals if start == CONSOLE_OUTPOST => {
                let mut inputs = Vec::new();
                for number in 10..26 {
                    let other = node(CONSOLE_OUTPOST, number, Role::Terminal, "Other");
                    let id = other.id;
                    inputs.push(focus(other, None, None));
                    inputs.push(snapshot_event(id, 1, 20));
                }
                inputs.push(focus(start_node, None, None));
                inputs
            }
            Step::OtherTerminals => Vec::new(),
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
                    Which::NeverAsked => {
                        Some((QueryId(10_000), start, Asked::Text(TextOp::ReadCaret)))
                    }
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
    fn apply(&mut self, input: &Input, effects: &[Effect]) {
        match input {
            Input::Answered {
                query_id, outpost, ..
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
fn answer(query_id: QueryId, outpost: OutpostId, asked: &Asked, outcome: Outcome) -> Input {
    let outcome = match outcome {
        Outcome::Gone => QueryOutcome::Gone,
        Outcome::Failed => QueryOutcome::Failed("failed".to_owned()),
        Outcome::Done => QueryOutcome::Done(fitting(outpost, asked)),
    };
    Input::Answered {
        trace_id: TraceId::from_raw(70),
        query_id,
        outpost,
        outcome,
    }
}

/// A result that fits what was asked.
fn fitting(outpost: OutpostId, asked: &Asked) -> QueryResult {
    let caret_reply = || {
        TextReply::Caret(Box::new(CaretReply {
            moved: true,
            caret: caret(300, 5),
            read_at_ms: 1_500,
            unit: None,
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
            same_line: None,
            removed: None,
        }))
    };
    match asked {
        Asked::Navigate => QueryResult::Navigated(Some(node(outpost, 1, Role::Window, "Window"))),
        Asked::Activate => QueryResult::Activated(None),
        Asked::Text(op) => QueryResult::Text(match op {
            TextOp::AwaitCaret(_) | TextOp::ReadCaret => caret_reply(),
            TextOp::Read(_) => TextReply::Read {
                moved: 0,
                chunk: line("hello world", 300, 0),
            },
            TextOp::ReadAhead(_) => TextReply::Chunks {
                moved: 0,
                chunks: vec![line("hello world", 300, 0)],
            },
            TextOp::TerminalRead { .. } | TextOp::TerminalCancel | TextOp::TerminalReadNow(_) => {
                TextReply::TerminalSnapshot(Box::new(console_read(2, 1_600)))
            }
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
            _ => TextReply::Done,
        }),
    }
}

/// FNV-1a over `bytes`, folded into `hash`.
fn fold(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What one step of a sequence did, as the enumeration records it.
struct Visit<'a> {
    state: &'a SrState,
    effects: &'a [Effect],
}

/// Runs every sequence of up to `depth` steps from `start`, calling
/// `visit` after each step with its sequence; the state is cloned at each
/// step, so sequences sharing a prefix share its steps.
fn enumerate(
    start: &SrState,
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
            let mut state = state.clone();
            let mut wire = wire.clone();
            let mut effects = Vec::new();
            for input in &inputs {
                let caused = reduce(&mut state, input);
                wire.apply(input, &caused);
                effects.extend(caused);
            }
            path.push(index);
            visit(
                path,
                Visit {
                    state: &state,
                    effects: &effects,
                },
            );
            go(&state, &wire, outpost, path, depth, steps, visit);
            path.pop();
        }
    }
    let steps = steps();
    let mut wire = Wire::default();
    wire.take(&[]);
    go(start, &wire, outpost, &mut Vec::new(), depth, &steps, visit);
}

/// The differential check's digest of every sequence of three steps from
/// both start states: each sequence's effects and held nodes after each
/// step, written one sequence a line to the file `QUERIES_DIGEST` names,
/// or compared with that file when `QUERIES_DIGEST_COMPARE` is set.
#[test]
#[ignore = "the differential check of the query table build, run by hand"]
#[expect(
    clippy::disallowed_methods,
    reason = "the differential check reads and writes its digests, once, by hand"
)]
fn digest_every_sequence() {
    let Ok(path) = std::env::var("QUERIES_DIGEST") else {
        return;
    };
    let mut lines = Vec::new();
    for (name, start, outpost) in [
        ("text box", text_box(), EDIT_OUTPOST),
        ("console", console(), CONSOLE_OUTPOST),
    ] {
        enumerate(&start, outpost, 3, &mut |path, visit| {
            let mut hash = 0xcbf2_9ce4_8422_2325;
            hash = fold(hash, format!("{:?}", visit.effects).as_bytes());
            hash = fold(hash, format!("{:?}", visit.state.held_nodes()).as_bytes());
            lines.push(format!("{name} {path:?} {hash:016x}"));
        });
    }
    let written = lines.join("\n");
    if std::env::var("QUERIES_DIGEST_COMPARE").is_ok() {
        let expected = std::fs::read_to_string(&path).expect("the digest file");
        let differing: Vec<(&str, &str)> = expected
            .lines()
            .zip(written.lines())
            .filter(|(expected, now)| expected != now)
            .take(10)
            .collect();
        assert_eq!(
            (expected.lines().count(), differing),
            (written.lines().count(), Vec::new())
        );
    } else {
        std::fs::write(&path, written).expect("the digest file is written");
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

fn speaks(effects: &[Effect]) -> bool {
    effects.iter().any(|effect| match effect {
        Effect::Speak(..) => true,
        Effect::ForKey(_, inner) => matches!(**inner, Effect::Speak(..)),
        _ => false,
    })
}

/// A navigation from the text box, asked: its number.
fn navigate(state: &mut SrState) -> QueryId {
    let asked = requests(&reduce(state, &command(ReviewCommand::Parent)));
    let [(query_id, None)] = asked[..] else {
        panic!("one navigation, not {asked:?}");
    };
    query_id
}

fn window_found() -> QueryOutcome {
    QueryOutcome::Done(QueryResult::Navigated(Some(node(
        EDIT_OUTPOST,
        1,
        Role::Window,
        "Window",
    ))))
}

/// An answer runs what its query continues and ends it: the navigator
/// moves, and the same answer again finds nothing and does nothing.
#[test]
fn an_answer_runs_its_continuation_once_and_ends_its_query() {
    let mut state = text_box();
    let query = navigate(&mut state);
    assert_eq!(state.outstanding_queries(), 1);
    let effects = reduce(&mut state, &answered(query, EDIT_OUTPOST, window_found()));
    assert!(speaks(&effects), "the window is announced: {effects:?}");
    assert_eq!(
        state
            .navigator
            .as_ref()
            .map(|navigator| navigator.object.id),
        Some(NodeId::in_outpost(EDIT_OUTPOST, 1))
    );
    assert_eq!(state.outstanding_queries(), 0);
    assert_eq!(
        reduce(&mut state, &answered(query, EDIT_OUTPOST, window_found())),
        []
    );
}

/// A navigation a newer one superseded is waited for until its answer
/// comes, which runs nothing; the newer one's answer moves the navigator.
#[test]
fn a_superseded_navigations_answer_runs_nothing_and_is_waited_for_until_it_comes() {
    let mut state = text_box();
    let first = navigate(&mut state);
    let second = navigate(&mut state);
    assert_eq!(state.outstanding_queries(), 2);
    assert_eq!(
        reduce(&mut state, &answered(first, EDIT_OUTPOST, window_found())),
        []
    );
    assert_eq!(state.outstanding_queries(), 1);
    let effects = reduce(&mut state, &answered(second, EDIT_OUTPOST, window_found()));
    assert!(speaks(&effects), "the newer navigation moves: {effects:?}");
    assert_eq!(state.outstanding_queries(), 0);
}

/// An answer from another outpost than the one asked is dropped, and the
/// query still waits for its own.
#[test]
fn an_answer_from_another_outpost_leaves_its_query_open() {
    let mut state = text_box();
    let query = navigate(&mut state);
    assert_eq!(
        reduce(&mut state, &answered(query, OutpostId(9), window_found())),
        []
    );
    assert_eq!(state.outstanding_queries(), 1);
    let effects = reduce(&mut state, &answered(query, EDIT_OUTPOST, window_found()));
    assert!(speaks(&effects));
}

/// A navigation's outcome other than an answer reads as unanswered, which
/// leaves the navigator where it is, and only a gone node as gone, which
/// returns it to the focus.
#[test]
fn an_unanswered_navigation_stays_and_only_a_gone_node_returns_to_the_focus() {
    for outcome in [
        QueryOutcome::Abandoned,
        QueryOutcome::NotStarted,
        QueryOutcome::Failed("the read failed".to_owned()),
    ] {
        let mut state = text_box();
        let query = navigate(&mut state);
        assert_eq!(
            reduce(&mut state, &answered(query, EDIT_OUTPOST, outcome)),
            []
        );
    }
    let mut state = text_box();
    let query = navigate(&mut state);
    let effects = reduce(
        &mut state,
        &answered(query, EDIT_OUTPOST, QueryOutcome::Gone),
    );
    assert!(speaks(&effects), "the focus is announced: {effects:?}");
}

/// A text request's outcome other than an answer reads as unanswered, and
/// a gone node as gone: a review command's read of the caret's line says
/// nothing for either.
#[test]
fn a_text_requests_failure_reads_as_unanswered() {
    for outcome in [
        QueryOutcome::Abandoned,
        QueryOutcome::NotStarted,
        QueryOutcome::Failed("the read failed".to_owned()),
        QueryOutcome::Gone,
    ] {
        let mut state = text_box();
        let asked = requests(&reduce(
            &mut state,
            &command(ReviewCommand::ReviewCurrentLine),
        ));
        let [(query, Some(TextOp::Read(_)))] = asked[..] else {
            panic!("one read, not {asked:?}");
        };
        let effects = reduce(&mut state, &answered(query, EDIT_OUTPOST, outcome));
        assert!(!speaks(&effects), "{effects:?}");
        assert_eq!(state.outstanding_queries(), 0);
    }
}

/// A review command in a console Core reads is answered from the console's
/// memory after a fresh read: only the fresh read is waited for, and the
/// command's query ends at Core's answer.
#[test]
fn a_console_review_answered_from_memory_is_never_waited_for() {
    let mut state = console();
    // The first read told the outpost the screen's shape.
    let before = state.outstanding_queries();
    let asked = requests(&reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentLine),
    ));
    let [(fresh, Some(TextOp::TerminalReadNow(_)))] = asked[..] else {
        panic!("one fresh read, not {asked:?}");
    };
    assert_eq!(
        state.outstanding_queries(),
        before + 1,
        "the fresh read alone"
    );
    let effects = reduce(
        &mut state,
        &answered(
            fresh,
            CONSOLE_OUTPOST,
            QueryOutcome::Done(QueryResult::Text(TextReply::TerminalSnapshot(Box::new(
                console_read(2, 20),
            )))),
        ),
    );
    assert!(speaks(&effects), "the line is read: {effects:?}");
    assert_eq!(state.outstanding_queries(), before);
    assert_eq!(
        state.queries.numbers().count(),
        before,
        "the command's query has ended"
    );
}

/// A console's model dropped while a review command waits for its fresh
/// read, by a refocus that forgets its text or by its eviction past 16
/// models: the command's query ends with it, the fresh read's answer is
/// taken by no model, and the model made again for the console asks its
/// own fresh read rather than wait for that one.
#[test]
fn a_dropped_models_review_ends_and_its_fresh_read_is_taken_by_no_model() {
    let refocus = || {
        vec![focus(
            node(CONSOLE_OUTPOST, 5, Role::Terminal, "Console"),
            None,
            Some(EarlierElement::NotFocused),
        )]
    };
    let evict = || {
        let mut inputs = Vec::new();
        for number in 10..26 {
            let other = node(CONSOLE_OUTPOST, number, Role::Terminal, "Other");
            let id = other.id;
            inputs.push(focus(other, None, None));
            inputs.push(snapshot_event(id, 1, 20));
        }
        inputs.push(focus(
            node(CONSOLE_OUTPOST, 5, Role::Terminal, "Console"),
            None,
            None,
        ));
        inputs
    };
    for drop_model in [refocus(), evict()] {
        let mut state = console();
        let asked = requests(&reduce(
            &mut state,
            &command(ReviewCommand::ReviewCurrentLine),
        ));
        let [(fresh, Some(_))] = asked[..] else {
            panic!("one fresh read, not {asked:?}");
        };
        for input in &drop_model {
            let _ = reduce(&mut state, input);
        }
        let _ = reduce(
            &mut state,
            &snapshot_event(NodeId::in_outpost(CONSOLE_OUTPOST, 5), 3, 30),
        );
        let asked = requests(&reduce(
            &mut state,
            &command(ReviewCommand::ReviewCurrentLine),
        ));
        let [(own, Some(TextOp::TerminalReadNow(_)))] = asked[..] else {
            panic!("the new model's own fresh read, not {asked:?}");
        };
        assert_ne!(own, fresh);
        let before = state.outstanding_queries();
        let effects = reduce(
            &mut state,
            &answered(
                fresh,
                CONSOLE_OUTPOST,
                QueryOutcome::Done(QueryResult::Text(TextReply::TerminalSnapshot(Box::new(
                    console_read(4, 40),
                )))),
            ),
        );
        assert!(!speaks(&effects), "{effects:?}");
        assert_eq!(state.outstanding_queries(), before - 1);
    }
}

/// A caret key's watch whose text a refocus forgot: its answer is taken by
/// nothing, and says nothing.
#[test]
fn a_refocus_drops_an_earlier_caret_keys_answer() {
    let mut state = text_box();
    let key = Input::CaretKey {
        trace_id: TraceId::from_raw(60),
        key_seq: KeySeq::NONE_YET,
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 1_000,
    };
    let asked = requests(&reduce(&mut state, &key));
    let [(watch, Some(TextOp::AwaitCaret(_)))] = asked[..] else {
        panic!("one watch, not {asked:?}");
    };
    assert_eq!(state.outstanding_queries(), 0, "a watch is not waited for");
    let _ = reduce(
        &mut state,
        &focus(
            node(EDIT_OUTPOST, 5, Role::EditableText, "Notes"),
            Some(FocusText::Caret {
                caret: caret(200, 4),
                selected_text: None,
            }),
            Some(EarlierElement::NotFocused),
        ),
    );
    let effects = reduce(
        &mut state,
        &answered(
            watch,
            EDIT_OUTPOST,
            QueryOutcome::Done(fitting(EDIT_OUTPOST, &Asked::Text(TextOp::ReadCaret))),
        ),
    );
    assert_eq!(effects, []);
}

/// At an outpost's end the watch the held keys wait on ends "gone", which
/// lets the key behind it be read, and an activation asked of it says
/// "No action".
#[test]
fn an_outposts_end_resumes_the_held_keys_and_says_no_action() {
    let mut state = text_box();
    let _ = reduce(&mut state, &command(ReviewCommand::Activate));
    let held = |trace, at| Input::HeldKey {
        trace_id: TraceId::from_raw(trace),
        key_seq: KeySeq::NONE_YET,
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: at,
    };
    // The first key's read is answered, so it is passed on and its watch
    // waited on; the second waits behind it.
    let asked = requests(&reduce(&mut state, &held(80, 1_000)));
    let [(read, Some(TextOp::ReadCaret))] = asked[..] else {
        panic!("one read, not {asked:?}");
    };
    let _ = reduce(
        &mut state,
        &answered(
            read,
            EDIT_OUTPOST,
            QueryOutcome::Done(fitting(EDIT_OUTPOST, &Asked::Text(TextOp::ReadCaret))),
        ),
    );
    assert!(state.held_keys.awaiting_watch.is_some());
    let _ = reduce(&mut state, &held(81, 1_010));
    let effects = reduce(
        &mut state,
        &Input::OutpostEnded {
            trace_id: TraceId::from_raw(62),
            outpost: EDIT_OUTPOST,
        },
    );
    assert_eq!(state.held_keys.awaiting_watch, None);
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::ForKey(_, inner) if **inner == Effect::PassHeldKey(TraceId::from_raw(81))
        )),
        "the key behind the watch is passed on: {effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Speak(utterance, _) if format!("{:?}", utterance.segments).contains("NoAction")
        )),
        "{effects:?}"
    );
    assert_eq!(state.outstanding_queries(), 0);
}

/// A held key's read past Core's deadline is withdrawn: the key is passed
/// on once, the read is no longer waited for, and its late answer and a
/// "not started" are dropped.
#[test]
fn a_withdrawn_held_read_passes_its_key_once_and_drops_its_late_answers() {
    let mut state = text_box();
    let asked = requests(&reduce(
        &mut state,
        &Input::HeldKey {
            trace_id: HELD,
            key_seq: KeySeq::NONE_YET,
            key: CaretKey {
                motion: CaretMotion::NextCharacter,
                select: false,
            },
            pressed_at_ms: 1_000,
        },
    ));
    let [(read, Some(TextOp::ReadCaret))] = asked[..] else {
        panic!("one read, not {asked:?}");
    };
    assert_eq!(state.outstanding_queries(), 1);
    let effects = reduce(&mut state, &Input::HeldKeyDue { trace_id: HELD });
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::PassHeldKey(_)))
            .count(),
        1
    );
    assert!(effects.contains(&Effect::Withdraw {
        outpost: EDIT_OUTPOST,
        query_id: read,
    }));
    assert_eq!(state.outstanding_queries(), 0);
    for outcome in [
        QueryOutcome::NotStarted,
        QueryOutcome::Done(fitting(EDIT_OUTPOST, &Asked::Text(TextOp::ReadCaret))),
    ] {
        assert_eq!(
            reduce(&mut state, &answered(read, EDIT_OUTPOST, outcome)),
            []
        );
    }
}

/// A watch the held keys no longer wait on, its key superseded, does not
/// resume them when its late answer comes.
#[test]
fn a_superseded_watchs_late_answer_does_not_resume_the_held_keys() {
    let mut state = text_box();
    let held = |trace, at| Input::HeldKey {
        trace_id: TraceId::from_raw(trace),
        key_seq: KeySeq::NONE_YET,
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: at,
    };
    let read_answer = || QueryOutcome::Done(fitting(EDIT_OUTPOST, &Asked::Text(TextOp::ReadCaret)));
    // K1 is passed on; its watch W1 is waited on.
    let asked = requests(&reduce(&mut state, &held(80, 1_000)));
    let [(read, _)] = asked[..] else {
        panic!("one read, not {asked:?}");
    };
    let asked = requests(&reduce(
        &mut state,
        &answered(read, EDIT_OUTPOST, read_answer()),
    ));
    let first_watch = state.held_keys.awaiting_watch.expect("W1 is waited on");
    assert!(asked.iter().any(|(query, _)| *query == first_watch));
    // K2 waits for W1 until its bound, then is read and passed on, its
    // watch W2 waited on.
    let _ = reduce(&mut state, &held(81, 1_020));
    let _ = reduce(
        &mut state,
        &Input::HeldKeyDue {
            trace_id: TraceId::from_raw(81),
        },
    );
    let read = state.held_keys.reading.expect("K2 is read").query_id;
    let _ = reduce(&mut state, &answered(read, EDIT_OUTPOST, read_answer()));
    let second_watch = state.held_keys.awaiting_watch.expect("W2 is waited on");
    assert_ne!(second_watch, first_watch);
    // K3 waits behind W2; W1's late answer resumes nothing.
    let _ = reduce(&mut state, &held(82, 1_040));
    let _ = reduce(
        &mut state,
        &answered(first_watch, EDIT_OUTPOST, read_answer()),
    );
    assert_eq!(state.held_keys.awaiting_watch, Some(second_watch));
    assert!(!state.held_keys.waiting.is_empty(), "K3 still waits");
}

/// Core's open queries are kept with the rest of its state, so a dump
/// replays them.
#[test]
fn the_open_queries_round_trip_with_the_state() {
    let mut state = text_box();
    let _ = navigate(&mut state);
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewCurrentLine));
    let json = serde_json::to_string(&state).expect("the state serializes");
    let read: SrState = serde_json::from_str(&json).expect("the state reads");
    assert_eq!(read, state);
    assert_eq!(read.outstanding_queries(), 2);
}
