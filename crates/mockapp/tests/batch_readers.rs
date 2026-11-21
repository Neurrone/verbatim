//! A real outpost running Core's batches on mockapp's objects (the request
//! list's design, sections 3 and 7): a batch that does not parse is refused
//! whole with none of it run; an MSAA candidate is acquired once for the
//! batch, its `Test` reading lazily in the matcher's order and stopping at
//! the first value that decides, as the identity check does
//! (`docs/parity/focus-events.md`, "Duplicate focus suppression"), and its
//! `FocusRead` run only when the test's condition holds; a window's hidden
//! check reads nothing past a shown window.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::WindowHandle;
use verbatim_model::batch::{
    Batch, Check, Condition, Done, Malformed, Op, Place, Placement, Step, StepOutcome, Subject,
    Tested, WindowOf,
};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{FocusRead, MsaaIds, Outcome, Refusal, Reply, WindowVerdict};
use windows::Win32::Foundation::HWND;

use common::outpost::{OutpostUnderTest, focus_set};

/// A batch of `steps` in the event queue's order.
fn batch(steps: Vec<Step>) -> Batch {
    Batch {
        steps,
        placement: Placement::Queue,
        spare: None,
    }
}

/// The outcome of each step `answer` gives, which must be a batch's.
fn steps(outcome: Outcome) -> Vec<StepOutcome> {
    match outcome {
        Outcome::Result(Reply::Steps(steps)) => steps,
        other => panic!("a batch's steps, not {other:?}"),
    }
}

/// The properties a test read, in the order it read them.
fn read_order(outcome: &StepOutcome) -> Vec<PropertyId> {
    match outcome {
        StepOutcome::Done(Done::Test(Tested { node, .. })) => {
            node.values.0.iter().map(|answer| answer.id).collect()
        }
        other => panic!("a test's result, not {other:?}"),
    }
}

/// The identity matcher Core writes for an MSAA candidate it committed as
/// `read`, in ADR 0004's order: read in full unless it lacks the focused
/// state over it and its ancestors, or it is at the ids and reads as the
/// focus named.
fn unless_same(read: &FocusRead) -> Matcher {
    let value = |id| {
        read.focus
            .read
            .values
            .answer(id)
            .and_then(|answer| answer.value.clone())
    };
    let is = |id| match value(id) {
        Some(value) => Matcher::Is(id, value),
        None => Matcher::Absent(id),
    };
    Matcher::Any(vec![
        Matcher::Is(PropertyId::FocusedWithin, PropertyValue::Flag(false)),
        Matcher::All(vec![
            Matcher::Is(
                PropertyId::MsaaAddress,
                PropertyValue::MsaaAddress(read.msaa.expect("an MSAA focus's ids")),
            ),
            is(PropertyId::States),
            is(PropertyId::Role),
            is(PropertyId::Name),
            is(PropertyId::Value),
        ]),
    ])
}

/// The candidate's test and its full read unless the test matched.
fn run_unless(
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    ids: MsaaIds,
    matcher: Matcher,
) -> Vec<StepOutcome> {
    let subject = Subject::Msaa {
        ids,
        redirect: true,
    };
    let (answer, _) = outpost.batch(
        hwnd,
        WindowVerdict::Msaa,
        batch(vec![
            Step {
                op: Op::Test {
                    subject: subject.clone(),
                    matcher,
                },
                when: None,
            },
            Step {
                op: Op::FocusRead {
                    subject,
                    known: outpost.known(),
                    set: focus_set(),
                    details: verbatim_model::TextDetails::NONE,
                },
                when: Some(Condition {
                    on: Place(0),
                    check: Check::Matched,
                    negated: true,
                }),
            },
        ]),
    );
    steps(answer.outcome)
}

/// An MSAA candidate the outpost acquires once for a batch: its test reads
/// lazily in the identity check's order, stopping at the first value that
/// differs, and its full read runs only when the test did not match: the
/// focus named reads the same and is read no further; one renamed, or not
/// focused, is decided at its first difference.
fn an_msaa_candidates_test_reads_lazily_and_gates_its_full_read() {
    let title = common::unique_title("mockapp-batch-candidate");
    let mut app = common::spawn_visible("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "set-focus first");
    let outpost = OutpostUnderTest::new(&app);
    let ids = MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: 3,
        child: 0,
    };
    let first = run_unless(&outpost, hwnd, ids, Matcher::Any(Vec::new()));
    let Some(StepOutcome::Done(Done::FocusRead(read))) = first.get(1) else {
        panic!("a full read when nothing matched: {first:?}");
    };
    let read = *read.clone();
    let same = run_unless(&outpost, hwnd, ids, unless_same(&read));
    assert_eq!(
        read_order(&same[0]),
        [
            PropertyId::FocusedWithin,
            PropertyId::MsaaAddress,
            PropertyId::States,
            PropertyId::Role,
            PropertyId::Name,
            PropertyId::Value,
        ]
    );
    assert!(matches!(
        &same[0],
        StepOutcome::Done(Done::Test(Tested { matched: true, .. }))
    ));
    assert_eq!(same[1], StepOutcome::Skipped);
    let mut renamed = read.clone();
    for answer in &mut renamed.focus.read.values.0 {
        if answer.id == PropertyId::Name {
            answer.value = Some(PropertyValue::Text("Second".to_owned()));
        }
    }
    let other = run_unless(&outpost, hwnd, ids, unless_same(&renamed));
    assert_eq!(
        read_order(&other[0]),
        [
            PropertyId::FocusedWithin,
            PropertyId::MsaaAddress,
            PropertyId::States,
            PropertyId::Role,
            PropertyId::Name,
        ],
        "stops at the name"
    );
    assert!(matches!(other[1], StepOutcome::Done(Done::FocusRead(_))));
    common::apply(&mut app, hwnd, "set-focus second");
    let unfocused = run_unless(&outpost, hwnd, ids, unless_same(&read));
    assert_eq!(read_order(&unfocused[0]), [PropertyId::FocusedWithin]);
    assert_eq!(unfocused[1], StepOutcome::Skipped);
    drop(outpost);
    app.quit();
}

/// The hidden check of a shown window reads whether it is shown and
/// nothing more; a batch that does not parse is refused whole.
fn a_shown_windows_hidden_check_reads_nothing_more_and_a_malformed_batch_is_refused() {
    let title = common::unique_title("mockapp-batch-window");
    let app = common::spawn_visible("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let window = Subject::Window {
        which: WindowOf::TopLevelOf(WindowHandle(hwnd.0 as u64)),
        backend: verbatim_model::Backend::Msaa,
        visible: false,
    };
    let hidden = Matcher::All(vec![
        Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
        Matcher::IsAnyOf(
            PropertyId::Role,
            vec![PropertyValue::Role(verbatim_model::Role::Dialog)],
        ),
    ]);
    let (answer, _) = outpost.batch(
        hwnd,
        WindowVerdict::Msaa,
        batch(vec![Step {
            op: Op::Test {
                subject: window,
                matcher: hidden,
            },
            when: None,
        }]),
    );
    let outcomes = steps(answer.outcome);
    assert_eq!(read_order(&outcomes[0]), [PropertyId::WindowShown]);
    assert!(matches!(
        &outcomes[0],
        StepOutcome::Done(Done::Test(Tested { matched: false, .. }))
    ));
    let read = |subject| Step {
        op: Op::Read {
            subject,
            set: vec![PropertyId::Name],
        },
        when: None,
    };
    let node = Subject::Node(verbatim_model::NodeId::new(1));
    let (refused, calls) = outpost.batch(
        hwnd,
        WindowVerdict::Msaa,
        batch(vec![read(node.clone()); 5]),
    );
    assert_eq!(
        refused.outcome,
        Outcome::Refused(Refusal::Malformed(Malformed::TooLong(5)))
    );
    assert_eq!(calls.total(), 0, "none of it run");
    drop(outpost);
    app.quit();
}

/// An MSAA candidate in a window whose verdict is UI Automation is refused
/// whole, `ReadThroughUia`, before any step runs: the window's hidden
/// check, which would match this hidden window and skip the candidate's
/// steps, does not run first.
fn an_msaa_candidate_in_a_uia_window_is_refused_before_its_hidden_check() {
    let title = common::unique_title("mockapp-batch-uia-candidate");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let candidate = Subject::Msaa {
        ids: MsaaIds {
            window: WindowHandle(hwnd.0 as u64),
            object: -4,
            child: 0,
        },
        redirect: true,
    };
    let unless_hidden = Some(Condition {
        on: Place(0),
        check: Check::Matched,
        negated: true,
    });
    let (answer, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![
            Step {
                op: Op::Test {
                    subject: Subject::Window {
                        which: WindowOf::TopLevelOf(WindowHandle(hwnd.0 as u64)),
                        backend: verbatim_model::Backend::Msaa,
                        visible: false,
                    },
                    matcher: Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
                },
                when: None,
            },
            Step {
                op: Op::Test {
                    subject: candidate.clone(),
                    matcher: Matcher::Any(Vec::new()),
                },
                when: unless_hidden,
            },
            Step {
                op: Op::FocusRead {
                    subject: candidate,
                    known: outpost.known(),
                    set: focus_set(),
                    details: verbatim_model::TextDetails::NONE,
                },
                when: unless_hidden,
            },
        ]),
    );
    assert_eq!(answer.outcome, Outcome::Refused(Refusal::ReadThroughUia));
    drop(outpost);
    app.quit();
}

/// The text operations on a UI Automation document the outpost holds, its
/// text "alpha beta", a line break, "gamma" and a line break,
/// each step through the node's text: the caret held as anchors, the line
/// read from it, the text from it to the end, where it is; then a select
/// of the whole text, the caret read again, and the two selections
/// compared.
fn text_operations_read_and_select_through_the_nodes_text() {
    common::init_com();
    let title = common::unique_title("mockapp-batch-text");
    let mut app = common::spawn_visible("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("set-focus doc");
    let outpost = OutpostUnderTest::new(&app);
    let uia = verbatim_uia::Uia::new().expect("a UIA client");
    outpost.read_focus_as(
        common::listener_focus(&uia, hwnd.0 as isize)
            .live()
            .element(),
    );
    let node = doc_node(&outpost, hwnd);
    text_reads(&outpost, hwnd, node);
    drop(outpost);
    app.quit();
}

/// A run of text steps on a UI Automation node is one remote program, its
/// unit read with the formatting its details name: on mockapp's text, its
/// bold "alpha" then its misspelt "beta", the caret at the start, the
/// caret, the word at it with its font attributes and spelling errors,
/// and the line with no details are read by one call, the word bold and
/// not misspelt, the line with no formatting; then the word after it, read
/// from the earlier batch's caret in a batch of its own, is one call too,
/// and misspelt.
#[expect(clippy::too_many_lines, reason = "two batches, each with its assertions")]
fn a_run_of_text_steps_is_one_program_with_its_formatting() {
    use verbatim_model::batch::{Point, PointOf};
    use verbatim_model::{FormatAttribute, FormatSet, TextDetails, TextMovement, TextUnit};
    common::init_com();
    let title = common::unique_title("mockapp-batch-text-formats");
    let mut app = common::spawn_visible("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("set-focus doc");
    let outpost = OutpostUnderTest::new(&app);
    let uia = verbatim_uia::Uia::new().expect("a UIA client");
    outpost.read_focus_as(
        common::listener_focus(&uia, hwnd.0 as isize)
            .live()
            .element(),
    );
    let node = doc_node(&outpost, hwnd);
    let details = TextDetails {
        formats: FormatSet::of(&[FormatAttribute::FontAttributes]),
        spelling_errors: true,
        ..TextDetails::NONE
    };
    let step = |op| Step { op, when: None };
    let doc = Subject::Node(node);
    let read = |at, movement, unit, details| Op::ReadText {
        subject: doc.clone(),
        at,
        movement,
        unit,
        count: 1,
        details,
    };
    let caret = Point::Of(Place(0), PointOf::Caret);
    let (answer, calls) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![
            step(Op::Caret {
                subject: doc.clone(),
                adopt: None,
            }),
            step(read(caret, None, TextUnit::Word, details)),
            step(read(caret, None, TextUnit::Line, TextDetails::NONE)),
        ]),
    );
    assert_eq!(calls.total(), 1, "one program: {calls:?}");
    let read_steps = steps(answer.outcome);
    let chunk = |outcome: &StepOutcome| match outcome {
        StepOutcome::Done(Done::ReadText(read)) => read.chunks[0].clone(),
        other => panic!("a read's chunks, not {other:?}"),
    };
    let word = chunk(&read_steps[1]);
    assert_eq!(word.text, "alpha ");
    let attributes: Vec<_> = word
        .formats
        .iter()
        .map(|run| {
            (
                &word.text[run.start as usize..run.end as usize],
                run.attributes.bold,
                run.attributes.spelling_error,
            )
        })
        .collect();
    assert_eq!(
        attributes,
        [("alpha", Some(true), false), (" ", Some(false), false)]
    );
    let line = chunk(&read_steps[2]);
    assert_eq!(
        (line.text.as_str(), line.formats.len()),
        ("alpha beta\n", 0)
    );
    let Some(StepOutcome::Done(Done::Caret(at))) = read_steps.first() else {
        panic!("the caret: {read_steps:?}");
    };
    // The next word, from the caret the first batch held.
    let (next, calls) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![step(read(
            Point::At(verbatim_model::TextPoint::At(
                verbatim_model::TextPosition::at(at.caret.anchor),
            )),
            Some(TextMovement {
                unit: TextUnit::Word,
                count: 1,
            }),
            TextUnit::Word,
            details,
        ))]),
    );
    assert_eq!(calls.total(), 1, "one program: {calls:?}");
    let next = steps(next.outcome);
    let word = chunk(&next[0]);
    assert_eq!(word.text, "beta");
    assert!(
        word.formats
            .iter()
            .all(|run| run.attributes.spelling_error && run.attributes.bold == Some(false)),
        "{word:?}"
    );
    drop(outpost);
    app.quit();
}

/// The node of the document in focus, as a batch's focus read mints it.
fn doc_node(outpost: &OutpostUnderTest, hwnd: HWND) -> verbatim_model::NodeId {
    use verbatim_model::TextDetails;
    let (focus, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![Step {
            op: Op::FocusRead {
                subject: Subject::Focus,
                known: Vec::new(),
                set: focus_set(),
                details: TextDetails::NONE,
            },
            when: None,
        }]),
    );
    match steps(focus.outcome).first() {
        Some(StepOutcome::Done(Done::FocusRead(read))) => read.focus.read.node.expect("a node"),
        other => panic!("the focus read: {other:?}"),
    }
}

/// The document `node`'s text read, selected and compared, as
/// [`text_operations_read_and_select_through_the_nodes_text`] says.
#[expect(clippy::too_many_lines, reason = "one assertion per text operation")]
fn text_reads(outpost: &OutpostUnderTest, hwnd: HWND, node: verbatim_model::NodeId) {
    use verbatim_model::batch::{Before, Point, PointOf};
    use verbatim_model::{TextDetails, TextPoint, TextUnit};
    let step = |op| Step { op, when: None };
    let doc = Subject::Node(node);
    let caret = Point::Of(Place(0), PointOf::Caret);
    let (read, calls) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![
            step(Op::Caret {
                subject: doc.clone(),
                adopt: None,
            }),
            step(Op::ReadText {
                subject: doc.clone(),
                at: caret,
                movement: None,
                unit: TextUnit::Line,
                count: 1,
                details: TextDetails::NONE,
            }),
            step(Op::ReadRange {
                subject: doc.clone(),
                start: caret,
                end: Point::At(TextPoint::End),
                through: None,
            }),
            step(Op::Locate {
                subject: doc.clone(),
                at: caret,
            }),
        ]),
    );
    assert_eq!(calls.total(), 1, "one program: {calls:?}");
    let read = steps(read.outcome);
    let Some(StepOutcome::Done(Done::Caret(at))) = read.first() else {
        panic!("the caret: {read:?}");
    };
    assert!(at.caret.edges.start && at.selection.is_none(), "{at:?}");
    let Some(StepOutcome::Done(Done::ReadText(line))) = read.get(1) else {
        panic!("the line: {read:?}");
    };
    assert_eq!(
        (line.chunks[0].text.as_str(), line.chunks[0].offset),
        (
            "alpha beta
",
            0
        )
    );
    assert!(
        matches!(&read[2], StepOutcome::Done(Done::ReadRange(range)) if range.text == "alpha beta
gamma
"),
        "{:?}",
        read[2]
    );
    assert!(
        matches!(read[3], StepOutcome::Done(Done::Locate(_))),
        "{:?}",
        read[3]
    );
    let (selected, calls) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![
            step(Op::Select {
                subject: doc.clone(),
                start: Point::At(TextPoint::Start),
                end: Point::At(TextPoint::End),
                through: None,
            }),
            step(Op::Caret {
                subject: doc.clone(),
                adopt: None,
            }),
            step(Op::CompareSelection {
                subject: doc,
                before: Before::Baseline(at.held),
                after: Place(1),
            }),
        ]),
    );
    // A program ends after its act: the select, then the reads after it.
    assert_eq!(calls.total(), 2, "two programs: {calls:?}");
    let selected = steps(selected.outcome);
    assert!(
        matches!(selected[0], StepOutcome::Done(Done::Select(_))),
        "{selected:?}"
    );
    let Some(StepOutcome::Done(Done::CompareSelection(compared))) = selected.get(2) else {
        panic!("the comparison: {selected:?}");
    };
    assert_eq!(
        compared
            .difference
            .new_selection
            .as_ref()
            .map(|selection| selection.text.as_str()),
        Some(
            "alpha beta
gamma
"
        )
    );
}

fn main() {
    harness::run_isolated(&[
        (
            "an_msaa_candidates_test_reads_lazily_and_gates_its_full_read",
            an_msaa_candidates_test_reads_lazily_and_gates_its_full_read,
        ),
        (
            "a_shown_windows_hidden_check_reads_nothing_more_and_a_malformed_batch_is_refused",
            a_shown_windows_hidden_check_reads_nothing_more_and_a_malformed_batch_is_refused,
        ),
        (
            "an_msaa_candidate_in_a_uia_window_is_refused_before_its_hidden_check",
            an_msaa_candidate_in_a_uia_window_is_refused_before_its_hidden_check,
        ),
        (
            "text_operations_read_and_select_through_the_nodes_text",
            text_operations_read_and_select_through_the_nodes_text,
        ),
        (
            "a_run_of_text_steps_is_one_program_with_its_formatting",
            a_run_of_text_steps_is_one_program_with_its_formatting,
        ),
    ]);
}
