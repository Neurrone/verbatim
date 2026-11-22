use verbatim_model::batch::{CaretWait, Done, EffectTest, Tested};
use verbatim_model::request::{MsaaIds, NodeRead, NodeWindow};
use verbatim_model::{Pid, WindowHandle};

use super::*;
use crate::wants::Wants;

fn node() -> NodeId {
    NodeId::new(12)
}

fn step(op: Op, when: Option<Condition>) -> Step {
    Step { op, when }
}

fn read_text(at: Point, unit: TextUnit) -> Op {
    Op::ReadText {
        subject: Subject::Node(node()),
        at,
        movement: None,
        unit,
        count: 1,
        details: TextDetails::NONE,
    }
}

/// The builder writes each step with its inputs as places on the wire,
/// a condition on the step after it, and the batch's placement and spare.
#[test]
fn a_built_batch_names_earlier_steps_by_their_places() {
    let mut watch = batch(|| ());
    let waited = watch.wait_for_caret(
        node(),
        CaretWait {
            from: TextAnchor(40),
            line: "plain italic text".into(),
            offset: 12,
            word: None,
            effect: EffectTest {
                unit: false,
                line: true,
            },
            passed_at_us: 5,
            poll_us: 10_000,
            bound_us: 400_000,
            events: true,
        },
    );
    let caret = watch.when(waited.found()).caret(node(), None);
    let _line = watch.when(!waited.found()).read_text(
        node(),
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    watch.outside().spare(node(), TextAnchor(40));
    let sent = watch.finish();
    let found = |negated| {
        Some(Condition {
            on: Place(0),
            check: Check::Found,
            negated,
        })
    };
    assert_eq!(sent.batch().placement, Placement::Outside);
    assert_eq!(sent.batch().spare, Some((node(), TextAnchor(40))));
    assert_eq!(
        sent.batch().steps[1..],
        [
            step(
                Op::Caret {
                    subject: Subject::Node(node()),
                    adopt: None,
                },
                found(false)
            ),
            step(
                read_text(Point::Of(Place(1), PointOf::Caret), TextUnit::Line),
                found(true)
            ),
        ]
    );
}

/// A numbered batch leaves off a step given another batch's handle, and
/// every step after it that names it, each answered failed.
#[test]
fn a_step_naming_a_step_left_off_is_left_off_too() {
    let ledger = Ledger::default();
    let mut first = Builder::<fn()>::numbered(&ledger);
    let theirs = first.caret(node(), None);
    let mut second = Builder::<fn()>::numbered(&ledger);
    let own = second.caret(node(), None);
    let foreign = second.read_text(
        node(),
        theirs.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let after = second.read_text(
        node(),
        foreign.start(),
        None,
        TextUnit::Word,
        1,
        TextDetails::NONE,
    );
    let sent = second.finish();
    assert_eq!(
        sent.batch().steps,
        [step(
            Op::Caret {
                subject: Subject::Node(node()),
                adopt: None,
            },
            None
        )]
    );
    assert_eq!(
        ledger.take(),
        [QueueEffect::ForeignHandle {
            batch: Some(2),
            handle: Some(1),
        }]
    );
    let answers = Answers::parse(&sent, Reply::Steps(vec![StepOutcome::Skipped])).unwrap();
    assert_eq!(answers.get(own), Got::Skipped);
    assert_eq!(answers.get(foreign), Got::Failed);
    assert_eq!(answers.get(after), Got::Failed);
    assert_eq!(ledger.take(), []);
}

/// A reply that is no batch's answer does not parse.
#[test]
fn a_reply_of_an_old_kind_is_no_batchs_answer() {
    let mut read = batch(|| ());
    let _ = read.caret(node(), None);
    let sent = read.finish();
    assert!(Answers::parse(&sent, crate::entries::tests::followed()).is_none());
}

/// The brand a bar's read on Core's want list takes, named for its one
/// call site.
enum Bar {}

/// The bar read's one step.
const BAR_READ: Ref<Bar, kind::Read> = Ref::named(0);

/// The bar's read, as its site builds it each time: for the want, and for
/// parsing the answer at a later step.
fn bar_read() -> Sent<Bar> {
    let mut read = Builder::<Bar>::named();
    let _ = read.read(
        On::msaa(
            MsaaIds {
                window: WindowHandle(5),
                object: -4,
                child: 0,
            },
            false,
        ),
        &[PropertyId::Role, PropertyId::Value],
    );
    read.finish()
}

/// A batch held on Core's want list goes out as wanted, and its answer,
/// arriving at a later step, is parsed against the batch held, read by
/// the site's constant handle.
#[test]
fn a_wanted_batch_is_answered_by_its_named_handles() {
    let mut wants = Wants::default();
    let token = wants.token();
    let window = RequestWindow::of_node(&NodeWindow {
        handle: WindowHandle(5),
        top_level: WindowHandle(5),
        root_owner: WindowHandle(5),
        process: Pid(9),
        thread: 1,
        desktop: false,
    });
    wants.want(
        Some(token),
        (Reader::Listener, window),
        bar_read().question(),
    );
    let sent = wants.take();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].request, bar_read().question());

    let read = NodeRead {
        node: None,
        window: window.node_window(),
        values: verbatim_model::property::PropertyValues::default(),
    };
    let reply = Reply::Steps(vec![StepOutcome::Done(Done::Read(read.clone()))]);
    let answers = Answers::parse(&bar_read(), reply).unwrap();
    assert_eq!(answers.get(BAR_READ), Got::Done(&read));
    let tested = Reply::Steps(vec![StepOutcome::Done(Done::Test(Tested {
        matched: true,
        node: read,
    }))]);
    assert!(Answers::parse(&bar_read(), tested).is_none());
}

/// A terminal's window read goes out as one step naming its marks, and its
/// answer is read by its handle.
#[test]
fn a_terminal_read_is_one_step_read_by_its_handle() {
    let ask = verbatim_model::WindowAsk {
        end: Some(verbatim_model::CurrentMark {
            mark: verbatim_model::EndMark {
                anchor: TextAnchor(7),
                line: "ready>".into(),
                line_above: 0,
            },
            find: true,
        }),
        aside: None,
        height: 30,
        opening: 30,
        colours: Some(vec![verbatim_model::row_hash("ready>")]),
    };
    let mut read = Builder::<Bar>::named();
    let window = read.terminal_read(node(), ask.clone());
    let sent = read.finish();
    assert_eq!(
        sent.batch().steps,
        [step(
            Op::TerminalRead {
                subject: Subject::Node(node()),
                ask: Box::new(ask),
            },
            None
        )]
    );
    let answer = Box::new(verbatim_model::TerminalWindow {
        rows: verbatim_model::TerminalText::unpadded("ready>"),
        above: 1,
        screen_top: 1,
        screen_rows: 30,
        screen_at_start: true,
        caret: None,
        end: TextAnchor(8),
        first_row: verbatim_model::TerminalText::unpadded("ready>"),
        document_rows: 1,
        end_place: Some(verbatim_model::MarkPlace::Rows {
            written: 0,
            opening: None,
        }),
        aside_place: None,
        found: verbatim_model::MarkSearch::InWindow,
        colours: Some(vec![None]),
        began_us: 1,
        ended_us: 2,
    });
    let reply = Reply::Steps(vec![StepOutcome::Done(Done::TerminalRead(answer.clone()))]);
    let answers = Answers::parse(&sent, reply).unwrap();
    assert_eq!(answers.get(window), Got::Done(&answer));
}
