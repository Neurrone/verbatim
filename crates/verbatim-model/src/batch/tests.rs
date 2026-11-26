use super::*;
use crate::property::PropertyValue;
use crate::request::{
    Answer, FocusAncestor, FocusCaret, FocusCaretRead, FocusNode, HeldIds, NodeWindow, Outcome,
    Reply, RequestNumber, RouteEnd, RouteStep,
};
use crate::{
    CaretReport, CaretSelection, OutpostId, Pid, Role, Selection, State, TextPosition, TextUnit,
};

fn node(number: u64) -> NodeId {
    NodeId::new(number)
}

fn at(anchor: u64) -> Point {
    Point::At(TextPoint::At(TextPosition::at(TextAnchor(anchor))))
}

fn step(op: Op) -> Step {
    Step { op, when: None }
}

fn when(op: Op, on: u8, check: Check, negated: bool) -> Step {
    Step {
        op,
        when: Some(Condition::step(Place(on), check, negated)),
    }
}

fn batch(steps: Vec<Step>) -> Batch {
    Batch {
        steps,
        placement: Placement::Queue,
        spare: None,
    }
}

fn read(subject: Subject) -> Op {
    Op::Read {
        subject,
        set: vec![PropertyId::Name, PropertyId::Role],
    }
}

fn kept(token: u64, keep: bool) -> Subject {
    Subject::Kept {
        listener: 1,
        token: ElementToken(token),
        keep,
    }
}

fn msaa() -> Subject {
    Subject::Msaa {
        ids: MsaaIds {
            window: WindowHandle(5),
            object: -4,
            child: 0,
        },
        redirect: true,
    }
}

fn window(which: WindowOf, visible: bool) -> Subject {
    Subject::Window {
        which,
        backend: Backend::Msaa,
        visible,
    }
}

fn text(subject: Subject, at: Point, movement: Option<i32>, unit: TextUnit) -> Op {
    Op::ReadText {
        trim: false,
        whole: false,
        subject,
        at,
        movement: movement.map(|count| TextMovement { unit, count }),
        unit,
        count: 1,
        details: TextDetails::NONE,
    }
}

fn caret() -> Op {
    Op::Caret {
        subject: Subject::Node(node(12)),
        adopt: None,
    }
}

fn focus_read(subject: Subject) -> Op {
    Op::FocusRead {
        subject,
        known: vec![KnownAncestor::Node(node(2))],
        set: vec![PropertyId::Name],
        details: TextDetails::NONE,
    }
}

fn descend(subject: Subject, depth: u32, limit: u32, nodes: u32) -> Op {
    Op::Descendants {
        subject,
        walk: Box::new(Walk {
            depth,
            view: View::Control,
            matcher: None,
            prune: None,
            include_subject: false,
            limit,
            nodes,
            set: vec![PropertyId::Name],
        }),
    }
}

fn wait(poll_us: u32, bound_us: u32) -> Op {
    Op::WaitForCaret {
        subject: Subject::Node(node(12)),
        wait: Box::new(CaretWait {
            from: TextAnchor(40),
            line: "plain italic text".into(),
            offset: 12,
            word: None,
            effect: EffectTest {
                unit: false,
                line: true,
            },
            passed_at_us: 0,
            poll_us,
            bound_us,
            events: true,
        }),
    }
}

/// The hidden-dialog check of a focus read: the top-level window's own
/// MSAA object, hidden and a dialog's.
fn hidden_test(of: WindowOf) -> Op {
    Op::Test {
        subject: window(of, false),
        matcher: Matcher::All(vec![
            Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
            Matcher::IsAnyOf(
                PropertyId::Role,
                vec![
                    PropertyValue::Role(Role::Dialog),
                    PropertyValue::Role(Role::Window),
                ],
            ),
        ]),
    }
}

/// The batch of each conversion of the request list's build, as Core
/// writes it.
#[expect(clippy::too_many_lines, reason = "one entry per conversion")]
fn conversions() -> Vec<(&'static str, ReaderKind, Vec<Step>)> {
    let node12 = || Subject::Node(node(12));
    let route = Subject::Route(Box::new(Route {
        window: WindowHandle(5),
        steps: vec![RouteStep {
            runtime_id: crate::property::RuntimeId(vec![42]),
            window: None,
            content: true,
        }],
        end: RouteEnd::Root,
        read: vec![crate::property::PropertyValues::default()],
    }));
    let same = Matcher::Any(vec![
        Matcher::Is(PropertyId::FocusedWithin, PropertyValue::Flag(false)),
        Matcher::All(vec![
            Matcher::Is(
                PropertyId::MsaaAddress,
                PropertyValue::MsaaAddress(HeldIds {
                    event: MsaaIds {
                        window: WindowHandle(5),
                        object: -4,
                        child: 0,
                    },
                    held_child: 0,
                }),
            ),
            Matcher::HasState(State::Focused),
            Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::ListItem)),
            Matcher::Is(PropertyId::Name, PropertyValue::Text("a".into())),
            Matcher::Absent(PropertyId::Value),
        ]),
    ]);
    let line = |at| text(node12(), at, None, TextUnit::Line);
    vec![
        (
            "the listener's focus read",
            ReaderKind::Listener,
            vec![
                step(read(kept(7, true))),
                step(Op::Ancestors {
                    subject: kept(7, true),
                    until: vec![KnownAncestor::Uia(crate::property::RuntimeId(vec![1]))],
                    depth: 60,
                    set: vec![PropertyId::RuntimeId, PropertyId::IsContent],
                }),
            ],
        ),
        (
            "a toast",
            ReaderKind::Listener,
            vec![step(read(kept(8, false)))],
        ),
        (
            "a classic progress bar",
            ReaderKind::Listener,
            vec![step(Op::Read {
                subject: msaa(),
                set: LISTENER_MSAA_SET.to_vec(),
            })],
        ),
        (
            "an alert",
            ReaderKind::Outpost,
            vec![
                step(read(node12())),
                step(descend(Subject::Result(Place(0)), 64, 4_096, 4_096)),
                step(Op::IsOneOf {
                    subject: node12(),
                    equal: Some(node(1)),
                    at_address: vec![node(2)],
                }),
            ],
        ),
        (
            "an MSAA change",
            ReaderKind::Outpost,
            vec![
                step(Op::IsOneOf {
                    subject: msaa(),
                    equal: Some(node(1)),
                    at_address: vec![node(2), node(3)],
                }),
                when(read(msaa()), 0, Check::Found, false),
            ],
        ),
        (
            "navigation",
            ReaderKind::Plain,
            vec![step(Op::Related {
                subject: node12(),
                relation: Relation::Parent,
                set: vec![PropertyId::Name],
            })],
        ),
        (
            "activate",
            ReaderKind::Outpost,
            vec![step(Op::Activate { subject: node12() })],
        ),
        (
            "follow",
            ReaderKind::Outpost,
            vec![step(Op::Follow(Some(FollowTarget {
                subject: node12(),
                group: FollowGroup::Base,
                extra: None,
            })))],
        ),
        (
            "a UI Automation focus's completion",
            ReaderKind::Outpost,
            vec![
                step(hidden_test(WindowOf::TopLevelOf(WindowHandle(5)))),
                when(focus_read(route.clone()), 0, Check::Matched, true),
            ],
        ),
        (
            "a selection item's relation",
            ReaderKind::Outpost,
            vec![
                step(read(route)),
                step(Op::Related {
                    subject: Subject::Node(node(1)),
                    relation: Relation::ControllerFor,
                    set: Vec::new(),
                }),
                step(Op::Descendants {
                    subject: Subject::Result(Place(1)),
                    walk: Box::new(Walk {
                        depth: 64,
                        view: View::Raw,
                        matcher: Some(Matcher::Is(
                            PropertyId::RuntimeId,
                            PropertyValue::RuntimeId(crate::property::RuntimeId(vec![42])),
                        )),
                        prune: None,
                        include_subject: true,
                        limit: 1,
                        nodes: 4_096,
                        set: Vec::new(),
                    }),
                }),
            ],
        ),
        (
            "an MSAA candidate unless it is the last focus",
            ReaderKind::Outpost,
            vec![
                step(hidden_test(WindowOf::TopLevelOf(WindowHandle(5)))),
                when(
                    Op::Test {
                        subject: msaa(),
                        matcher: same,
                    },
                    0,
                    Check::Matched,
                    true,
                ),
                when(focus_read(msaa()), 1, Check::Matched, true),
            ],
        ),
        (
            "the current focus",
            ReaderKind::Outpost,
            vec![
                step(hidden_test(WindowOf::Focused)),
                when(focus_read(Subject::Focus), 0, Check::Matched, true),
            ],
        ),
        (
            "a tree dump",
            ReaderKind::Outpost,
            vec![step(descend(
                window(WindowOf::Foreground, false),
                64,
                4_096,
                4_096,
            ))],
        ),
        (
            "the tray list",
            ReaderKind::Outpost,
            vec![
                step(descend(
                    window(
                        WindowOf::ClassPath(vec!["Shell_TrayWnd".into(), "TrayNotifyWnd".into()]),
                        false,
                    ),
                    16,
                    1_024,
                    1_024,
                )),
                step(descend(
                    window(
                        WindowOf::ClassPath(vec!["NotifyIconOverflowWindow".into()]),
                        true,
                    ),
                    16,
                    1_024,
                    1_024,
                )),
            ],
        ),
        (
            "Delete's starting read",
            ReaderKind::Outpost,
            vec![
                step(caret()),
                step(line(Point::Of(Place(0), PointOf::Caret))),
                step(text(
                    node12(),
                    Point::Of(Place(0), PointOf::Caret),
                    None,
                    TextUnit::Character,
                )),
                step(text(
                    node12(),
                    Point::Of(Place(0), PointOf::Caret),
                    None,
                    TextUnit::Word,
                )),
            ],
        ),
        (
            "Control+Backspace's starting read",
            ReaderKind::Outpost,
            vec![
                step(caret()),
                step(line(Point::Of(Place(0), PointOf::Caret))),
                step(Op::ReadText {
                    trim: false,
                    whole: false,
                    subject: node12(),
                    at: Point::Of(Place(0), PointOf::Caret),
                    movement: Some(TextMovement {
                        unit: TextUnit::Character,
                        count: -1,
                    }),
                    unit: TextUnit::Word,
                    count: 1,
                    details: TextDetails::NONE,
                }),
            ],
        ),
        (
            "a caret key's watch",
            ReaderKind::Outpost,
            vec![
                step(wait(10_000, 400_000)),
                when(caret(), 0, Check::Found, false),
                when(
                    line(Point::Of(Place(1), PointOf::Caret)),
                    0,
                    Check::Found,
                    false,
                ),
                when(
                    text(
                        node12(),
                        Point::Of(Place(1), PointOf::Caret),
                        None,
                        TextUnit::Word,
                    ),
                    0,
                    Check::Found,
                    false,
                ),
            ],
        ),
        (
            "a sentence key",
            ReaderKind::Outpost,
            vec![step(Op::MoveBySentence {
                subject: node12(),
                from: TextAnchor(40),
                forward: true,
                details: TextDetails::NONE,
            })],
        ),
        (
            "a caret tell",
            ReaderKind::Outpost,
            vec![
                step(Op::Caret {
                    subject: node12(),
                    adopt: Some(ElementToken(3)),
                }),
                step(line(Point::Of(Place(0), PointOf::Caret))),
                step(Op::CompareSelection {
                    subject: node12(),
                    before: Before::Baseline(TextAnchor(30)),
                    after: Place(0),
                }),
            ],
        ),
        (
            "say-all's read ahead",
            ReaderKind::Outpost,
            vec![step(Op::ReadText {
                trim: false,
                whole: false,
                subject: node12(),
                at: at(50),
                movement: None,
                unit: TextUnit::Line,
                count: MAX_UNITS,
                details: TextDetails::NONE,
            })],
        ),
        (
            "review's previous word",
            ReaderKind::Outpost,
            vec![
                step(text(node12(), at(50), None, TextUnit::Word)),
                step(text(
                    node12(),
                    Point::Of(Place(0), PointOf::ChunkStart),
                    Some(-1),
                    TextUnit::Word,
                )),
                step(line(Point::Of(Place(1), PointOf::ChunkStart))),
            ],
        ),
        (
            "review's next word",
            ReaderKind::Outpost,
            vec![
                step(text(node12(), at(50), Some(1), TextUnit::Word)),
                step(Op::ReadRange {
                    trim: false,
                    subject: node12(),
                    start: at(50),
                    end: Point::Of(Place(0), PointOf::ChunkStart),
                    through: None,
                }),
                step(line(Point::Of(Place(0), PointOf::ChunkStart))),
            ],
        ),
        (
            "review's select",
            ReaderKind::Outpost,
            vec![
                step(caret()),
                step(Op::Select {
                    subject: node12(),
                    start: at(50),
                    end: at(51),
                    through: Some(Through {
                        start: at(50),
                        end: at(51),
                    }),
                }),
                step(caret()),
                step(Op::CompareSelection {
                    subject: node12(),
                    before: Before::SelectionOf(Place(0)),
                    after: Place(2),
                }),
            ],
        ),
        (
            "say-all's caret move",
            ReaderKind::Outpost,
            vec![
                step(Op::Select {
                    subject: node12(),
                    start: at(50),
                    end: at(50),
                    through: None,
                }),
                step(caret()),
            ],
        ),
        (
            "report caret location",
            ReaderKind::Outpost,
            vec![step(Op::Locate {
                subject: node12(),
                at: Point::At(TextPoint::Caret),
            })],
        ),
    ]
}

#[test]
fn every_conversion_parses_at_its_reader() {
    for (name, reader, steps) in conversions() {
        let sent = batch(steps);
        let parsed =
            parse(sent.clone(), reader).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(parsed.batch(), &sent, "{name}");
    }
}

/// `steps` refused at an outpost with `refusal`.
fn refused(steps: Vec<Step>, reader: ReaderKind, refusal: Malformed) {
    assert_eq!(parse(batch(steps), reader), Err(refusal));
}

#[test]
#[expect(clippy::too_many_lines, reason = "one case per bound")]
fn a_batch_breaking_a_bound_is_refused_whole() {
    let outpost = ReaderKind::Outpost;
    let bound = |step, bound| Malformed::Bound {
        step: Place(step),
        bound,
    };
    refused(Vec::new(), outpost, Malformed::Empty);
    refused(vec![step(caret()); 17], outpost, Malformed::TooLong(17));
    let ancestors = |depth| Op::Ancestors {
        subject: Subject::Node(node(1)),
        until: Vec::new(),
        depth,
        set: Vec::new(),
    };
    refused(
        vec![step(ancestors(65))],
        outpost,
        bound(0, Bound::Depth(65)),
    );
    let node1 = || Subject::Node(node(1));
    refused(
        vec![step(descend(node1(), 65, 1, 1))],
        outpost,
        bound(0, Bound::Depth(65)),
    );
    refused(
        vec![step(descend(node1(), 1, 1, 4_097))],
        outpost,
        bound(0, Bound::Nodes(4_097)),
    );
    refused(
        vec![step(descend(node1(), 1, 11, 10))],
        outpost,
        bound(0, Bound::Limit(11)),
    );
    let count = |count| Op::ReadText {
        trim: false,
        whole: false,
        subject: node1(),
        at: Point::At(TextPoint::Caret),
        movement: None,
        unit: TextUnit::Line,
        count,
        details: TextDetails::NONE,
    };
    refused(vec![step(count(0))], outpost, bound(0, Bound::Count(0)));
    refused(vec![step(count(33))], outpost, bound(0, Bound::Count(33)));
    // A whole read counts to the terminal's most lines; a trimmed one reads
    // one chunk, and neither reads details or, whole, moves first.
    let shaped = |count, (whole, trim), details, movement| Op::ReadText {
        trim,
        whole,
        subject: node1(),
        at: Point::At(TextPoint::Caret),
        movement,
        unit: TextUnit::Line,
        count,
        details,
    };
    let none = TextDetails::NONE;
    let lines = Some(TextMovement {
        unit: TextUnit::Line,
        count: -1,
    });
    let max = crate::MAX_TERMINAL_LINES;
    assert!(
        parse(
            batch(vec![step(shaped(max, (true, true), none, None))]),
            outpost
        )
        .is_ok()
    );
    refused(
        vec![step(shaped(max + 1, (true, true), none, None))],
        outpost,
        bound(0, Bound::Count(max + 1)),
    );
    refused(
        vec![step(shaped(2, (false, true), none, None))],
        outpost,
        bound(0, Bound::Shape),
    );
    refused(
        vec![step(shaped(1, (false, true), TextDetails::ALL, None))],
        outpost,
        bound(0, Bound::Shape),
    );
    refused(
        vec![step(shaped(2, (true, false), none, lines))],
        outpost,
        bound(0, Bound::Shape),
    );
    let find = |read, limit, calls| Op::FindText {
        subject: node1(),
        at: Point::At(TextPoint::End),
        needle: "cmd".to_owned(),
        backward: true,
        unit_start: Some(TextUnit::Line),
        read,
        limit,
        calls,
    };
    assert!(parse(batch(vec![step(find(1, 20, 200))]), outpost).is_ok());
    refused(vec![step(find(0, 1, 1))], outpost, bound(0, Bound::Search));
    refused(vec![step(find(1, 21, 1))], outpost, bound(0, Bound::Search));
    refused(
        vec![step(find(1, 1, 201))],
        outpost,
        bound(0, Bound::Search),
    );
    refused(
        vec![step(wait(999, 1_000))],
        outpost,
        bound(0, Bound::Poll(999)),
    );
    refused(
        vec![step(wait(1_000, 1_000_001))],
        outpost,
        bound(0, Bound::Wait(1_000_001)),
    );
    refused(
        vec![step(caret()), step(wait(1_000, 1_000))],
        outpost,
        Malformed::WaitNotFirst(Place(1)),
    );
    refused(
        vec![step(wait(1_000, 1_000)), step(wait(1_000, 1_000))],
        outpost,
        Malformed::WaitNotFirst(Place(1)),
    );
}

#[test]
#[expect(clippy::too_many_lines, reason = "one case per kind of reference")]
fn a_reference_names_an_earlier_step_of_a_kind_it_can_take() {
    let outpost = ReaderKind::Outpost;
    let node12 = || Subject::Node(node(12));
    let from = |place, which| {
        text(
            node12(),
            Point::Of(Place(place), which),
            None,
            TextUnit::Line,
        )
    };
    // Forward, its own place, and past the end.
    refused(
        vec![step(from(1, PointOf::Caret)), step(caret())],
        outpost,
        Malformed::Reference {
            step: Place(0),
            to: Place(1),
        },
    );
    refused(
        vec![step(read(Subject::Result(Place(0))))],
        outpost,
        Malformed::Reference {
            step: Place(0),
            to: Place(0),
        },
    );
    let wrong = |step, to, kind| Malformed::WrongKind {
        step: Place(step),
        to: Place(to),
        kind,
    };
    refused(
        vec![step(caret()), step(from(0, PointOf::ChunkStart))],
        outpost,
        wrong(1, 0, OpKind::Caret),
    );
    refused(
        vec![step(from_caret_read()), step(from(0, PointOf::Caret))],
        outpost,
        wrong(1, 0, OpKind::ReadText),
    );
    // A visible range's points are a `Visible`'s, and a landing a `Move`'s.
    refused(
        vec![step(caret()), step(from(0, PointOf::VisibleStart))],
        outpost,
        wrong(1, 0, OpKind::Caret),
    );
    refused(
        vec![step(caret()), step(from(0, PointOf::Moved))],
        outpost,
        wrong(1, 0, OpKind::Caret),
    );
    let visible = Op::Visible { subject: node12() };
    let moved = Op::Move {
        subject: node12(),
        at: Point::Of(Place(0), PointOf::VisibleStart),
        movement: TextMovement {
            unit: TextUnit::Line,
            count: 2,
        },
    };
    assert!(
        parse(
            batch(vec![
                step(visible),
                step(moved),
                step(from(1, PointOf::Moved))
            ]),
            outpost
        )
        .is_ok()
    );
    refused(
        vec![
            step(read(node12())),
            step(read(Subject::Item {
                of: Place(0),
                index: 0,
            })),
        ],
        outpost,
        wrong(1, 0, OpKind::Read),
    );
    let related = Op::Related {
        subject: node12(),
        relation: Relation::FirstChild,
        set: Vec::new(),
    };
    refused(
        vec![step(related.clone()), step(read(Subject::Result(Place(0))))],
        outpost,
        wrong(1, 0, OpKind::Related),
    );
    assert!(
        parse(
            batch(vec![
                step(related),
                step(descend(Subject::Result(Place(0)), 1, 1, 1))
            ]),
            outpost
        )
        .is_ok()
    );
    refused(
        vec![
            step(read(node12())),
            step(Op::CompareSelection {
                subject: node12(),
                before: Before::Baseline(TextAnchor(1)),
                after: Place(0),
            }),
        ],
        outpost,
        wrong(1, 0, OpKind::Read),
    );
    refused(
        vec![
            step(read(node12())),
            when(caret(), 0, Check::Matched, false),
        ],
        outpost,
        wrong(1, 0, OpKind::Read),
    );
    refused(
        vec![
            step(from_caret_read()),
            when(caret(), 0, Check::Found, true),
        ],
        outpost,
        wrong(1, 0, OpKind::ReadText),
    );
}

/// A caret finds a selection, so a read of it can wait on one.
#[test]
fn a_read_can_wait_on_a_carets_selection() {
    let node12 = || Subject::Node(node(12));
    assert!(
        parse(
            batch(vec![
                step(caret()),
                when(
                    text(
                        node12(),
                        Point::Of(Place(0), PointOf::SelectionStart),
                        None,
                        TextUnit::Line
                    ),
                    0,
                    Check::Found,
                    false
                ),
            ]),
            ReaderKind::Outpost
        )
        .is_ok()
    );
}

fn from_caret_read() -> Op {
    text(
        Subject::Node(node(12)),
        Point::At(TextPoint::Caret),
        None,
        TextUnit::Line,
    )
}

#[test]
fn a_batch_a_reader_cannot_run_is_refused_whole() {
    let not_run = |op| Malformed::NotRun { step: Place(0), op };
    refused(
        vec![step(caret())],
        ReaderKind::Listener,
        not_run(OpKind::Caret),
    );
    refused(
        vec![step(read(Subject::Node(node(1))))],
        ReaderKind::Listener,
        not_run(OpKind::Read),
    );
    refused(
        vec![step(Op::Read {
            subject: msaa(),
            set: vec![PropertyId::Description],
        })],
        ReaderKind::Listener,
        not_run(OpKind::Read),
    );
    // The listener reads only elements it kept, never an earlier step's
    // node: it would answer the kept element's own values.
    refused(
        vec![
            step(Op::Ancestors {
                subject: kept(7, true),
                until: Vec::new(),
                depth: 60,
                set: vec![PropertyId::Name],
            }),
            step(read(Subject::Item {
                of: Place(0),
                index: 1,
            })),
        ],
        ReaderKind::Listener,
        Malformed::NotRun {
            step: Place(1),
            op: OpKind::Read,
        },
    );
    refused(
        vec![
            step(read(kept(7, true))),
            step(Op::Ancestors {
                subject: Subject::Result(Place(0)),
                until: Vec::new(),
                depth: 60,
                set: vec![PropertyId::Name],
            }),
        ],
        ReaderKind::Listener,
        Malformed::NotRun {
            step: Place(1),
            op: OpKind::Ancestors,
        },
    );
    refused(
        vec![step(focus_read(msaa()))],
        ReaderKind::Plain,
        not_run(OpKind::FocusRead),
    );
    refused(
        vec![step(Op::Ancestors {
            subject: Subject::Node(node(1)),
            until: Vec::new(),
            depth: 1,
            set: Vec::new(),
        })],
        ReaderKind::Plain,
        not_run(OpKind::Ancestors),
    );
}

#[test]
fn a_subject_an_operation_cannot_take_is_refused() {
    let outpost = ReaderKind::Outpost;
    refused(
        vec![step(focus_read(Subject::Node(node(1))))],
        outpost,
        Malformed::Subject(Place(0)),
    );
    refused(
        vec![step(Op::Caret {
            subject: msaa(),
            adopt: None,
        })],
        outpost,
        Malformed::Subject(Place(0)),
    );
    refused(
        vec![
            step(read(Subject::Node(node(1)))),
            step(text(Subject::Result(Place(0)), at(9), None, TextUnit::Line)),
        ],
        outpost,
        Malformed::AnchorWithoutNode(Place(1)),
    );
    refused(
        vec![step(read(kept(7, true))), step(read(kept(7, false)))],
        ReaderKind::Listener,
        Malformed::KeepMarks(ElementToken(7)),
    );
    refused(
        vec![step(Op::Test {
            subject: msaa(),
            matcher: Matcher::Is(PropertyId::Name, PropertyValue::Flag(true)),
        })],
        outpost,
        Malformed::MatcherValue {
            step: Place(0),
            property: PropertyId::Name,
        },
    );
}

#[test]
fn a_condition_runs_its_step_as_the_named_outcome_says() {
    let tested = |matched| {
        StepOutcome::Done(Done::Test(Tested {
            matched,
            node: node_read(1),
        }))
    };
    let condition = |check, negated| Condition::step(Place(0), check, negated);
    let point = TextAnchor;
    let caret = |selected: bool| {
        StepOutcome::Done(Done::Caret(CaretAt {
            caret: point(1),
            selection: selected.then(|| HeldSelection {
                start: point(1),
                end: point(2),
                at_text_start: false,
            }),
            held: TextAnchor(3),
            offsets: None,
            reported: true,
        }))
    };
    let cases = [
        (Check::Found, caret(true), Gate::Run, Gate::Skip),
        (Check::Found, caret(false), Gate::Skip, Gate::Run),
        (Check::Matched, tested(true), Gate::Run, Gate::Skip),
        (Check::Matched, tested(false), Gate::Skip, Gate::Run),
        (
            Check::Found,
            StepOutcome::Done(Done::IsOneOf(Some(0))),
            Gate::Run,
            Gate::Skip,
        ),
        (
            Check::Found,
            StepOutcome::Done(Done::IsOneOf(None)),
            Gate::Skip,
            Gate::Run,
        ),
        (
            Check::Found,
            StepOutcome::Done(Done::WaitForCaret(Waited::Bound)),
            Gate::Skip,
            Gate::Run,
        ),
        (Check::Matched, StepOutcome::Skipped, Gate::Skip, Gate::Skip),
        (
            Check::Matched,
            StepOutcome::Refused(StepRefusal::Unsupported),
            Gate::Skip,
            Gate::Run,
        ),
        (
            Check::Found,
            StepOutcome::NotThere(Missing::Object),
            Gate::Skip,
            Gate::Run,
        ),
        (Check::Found, StepOutcome::Failed, Gate::Skip, Gate::Run),
    ];
    for (check, named, plain, negated) in cases {
        assert_eq!(
            condition(check, false).gate(std::slice::from_ref(&named)),
            Ok(plain),
            "{check:?} {named:?}"
        );
        assert_eq!(
            condition(check, true).gate(std::slice::from_ref(&named)),
            Ok(negated),
            "not {check:?} {named:?}"
        );
    }
    assert_eq!(
        condition(Check::Matched, false).gate(&[StepOutcome::Done(Done::Follow)]),
        Err(KindMismatch)
    );
}

fn node_read(number: u64) -> NodeRead {
    NodeRead {
        node: Some(node(number)),
        window: NodeWindow {
            handle: WindowHandle(5),
            top_level: WindowHandle(5),
            root_owner: WindowHandle(5),
            process: Pid(9),
            thread: 1,
            desktop: false,
        },
        values: crate::property::PropertyValues::default(),
    }
}

fn chunk(anchor: u64) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: "line".into(),
        start: TextAnchor(anchor),
        offset: 0,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

fn focus_node(number: u64) -> FocusNode {
    FocusNode {
        read: node_read(number),
        runtime_id: None,
    }
}

/// One outcome of every kind that carries a node id or a text anchor,
/// numbering them from 1 in the order the walk meets them.
fn outcomes_naming_every_node_and_anchor() -> Vec<StepOutcome> {
    let done = StepOutcome::Done;
    let point = TextAnchor;
    vec![
        done(Done::Read(node_read(1))),
        done(Done::Test(Tested {
            matched: true,
            node: node_read(2),
        })),
        done(Done::Related(vec![node_read(3)])),
        done(Done::Ancestors(Ancestry {
            ancestors: vec![node_read(4)],
            end: AncestorsEnd::Top,
        })),
        done(Done::Descendants(Descended {
            nodes: vec![Descendant {
                node: node_read(5),
                parent: None,
                root: 0,
            }],
            complete: true,
        })),
        done(Done::FocusRead(Box::new(FocusRead {
            focus: focus_node(6),
            msaa: None,
            ancestors: vec![FocusAncestor {
                node: focus_node(7),
                simple_child: false,
                uia_layout: false,
                described: false,
            }],
            met: None,
            foreground: Some(focus_node(8)),
            value_from_selection: false,
            runtime_id_reused: false,
            caret: FocusCaret::Read(Box::new(FocusCaretRead {
                caret: CaretReport {
                    line: chunk(1),
                    selection: Some(CaretSelection {
                        range: Selection {
                            start: TextPosition::at(TextAnchor(2)),
                            end: TextPosition::at(TextAnchor(3)),
                        },
                    }),
                },
                selected_text: None,
                baseline: TextAnchor(4),
            })),
        }))),
        done(Done::Caret(CaretAt {
            caret: point(5),
            selection: Some(HeldSelection {
                start: point(6),
                end: point(7),
                at_text_start: false,
            }),
            held: TextAnchor(8),
            offsets: None,
            reported: true,
        })),
        done(Done::ReadText(Chunks {
            padded: None,
            moved: 0,
            chunks: vec![chunk(9), chunk(10)],
        })),
        done(Done::MoveBySentence(SentenceMoved::End {
            line: chunk(11),
            baseline: TextAnchor(12),
        })),
        StepOutcome::NotThere(Missing::Node(node(9))),
        StepOutcome::NotThere(Missing::Anchor(TextAnchor(13))),
        done(Done::Visible(VisibleText {
            start: TextAnchor(14),
            end: TextAnchor(15),
            at_text_start: true,
            text: String::new(),
            padded: Vec::new(),
        })),
        done(Done::Move(Moved {
            moved: 1,
            from: TextAnchor(16),
            to: TextAnchor(17),
        })),
    ]
}

/// A move of `subject`'s text from the anchor `from` by a line.
fn moved_from(subject: Subject, from: u64) -> Op {
    Op::Move {
        subject,
        at: Point::At(TextPoint::At(crate::TextPosition::at(TextAnchor(from)))),
        movement: TextMovement {
            unit: TextUnit::Line,
            count: 1,
        },
    }
}

#[test]
fn the_outcome_walk_meets_every_node_and_anchor_and_an_answer_stamps_them() {
    let mut outcomes = outcomes_naming_every_node_and_anchor();
    let (mut nodes, mut anchors) = (Vec::new(), Vec::new());
    visit_outcomes(
        &mut outcomes,
        &mut |node| nodes.push(node.number()),
        &mut |anchor| {
            anchors.push(anchor.0);
        },
    );
    assert_eq!(nodes, (1..=9).collect::<Vec<_>>());
    assert_eq!(anchors, (1..=17).collect::<Vec<_>>());

    let mut answer = Answer {
        number: RequestNumber(1),
        outcome: Outcome::Result(Reply::Steps(outcomes)),
        probes: Vec::new(),
        detail: String::new(),
        began_us: 0,
        ended_us: 0,
    };
    assert_eq!(
        answer.named().1,
        (1..=17).map(TextAnchor).collect::<Vec<_>>()
    );
    answer.assign_outpost(OutpostId(3));
    let (stamped, _) = answer.named();
    assert_eq!(stamped.len(), 9);
    assert!(
        stamped
            .iter()
            .all(|node| node.outpost() == Some(OutpostId(3)))
    );
}

#[test]
fn the_batch_walk_meets_every_node_it_names_and_unstamps_them() {
    let stamped = |number| NodeId::in_outpost(OutpostId(3), number);
    let mut sent = Batch {
        steps: vec![
            step(Op::Ancestors {
                subject: Subject::Node(stamped(1)),
                until: vec![
                    KnownAncestor::Node(stamped(2)),
                    KnownAncestor::Window(WindowHandle(1)),
                ],
                depth: 1,
                set: Vec::new(),
            }),
            step(Op::IsOneOf {
                subject: Subject::Node(stamped(3)),
                equal: Some(stamped(4)),
                at_address: vec![stamped(5)],
            }),
            step(Op::Follow(Some(FollowTarget {
                subject: Subject::Node(stamped(6)),
                group: FollowGroup::Base,
                extra: Some(stamped(7)),
            }))),
            step(Op::FocusRead {
                subject: Subject::Focus,
                known: vec![KnownAncestor::Node(stamped(8))],
                set: Vec::new(),
                details: TextDetails::NONE,
            }),
        ],
        placement: Placement::Queue,
        spare: Some((stamped(9), TextAnchor(1))),
    };
    let mut ask = crate::request::Ask {
        number: RequestNumber(1),
        window: WindowHandle(1),
        verdict: crate::request::WindowVerdict::Uia,
        request: sent.clone().into(),
    };
    let mut met = Vec::new();
    sent.visit_nodes(&mut |node| met.push(node.number()));
    assert_eq!(met, (1..=9).collect::<Vec<_>>());
    ask.unstamp();
    let crate::request::Question::Batch(mut unstamped) = ask.request;
    unstamped.visit_nodes(&mut |node| assert_eq!(node.outpost(), Some(OutpostId::UNASSIGNED)));
}

#[test]
fn a_batch_names_every_anchor_of_its_points_baselines_and_starting_points() {
    let node12 = || Subject::Node(node(12));
    let sent = Batch {
        steps: vec![
            step(Op::Select {
                subject: node12(),
                start: at(1),
                end: at(2),
                through: Some(Through {
                    start: at(3),
                    end: Point::At(TextPoint::Caret),
                }),
            }),
            step(text(node12(), at(4), None, TextUnit::Word)),
            step(Op::CompareSelection {
                subject: node12(),
                before: Before::Baseline(TextAnchor(5)),
                after: Place(0),
            }),
            step(Op::Locate {
                subject: node12(),
                at: at(6),
            }),
        ],
        placement: Placement::Queue,
        spare: Some((node(1), TextAnchor(9))),
    };
    let anchors = |list: &[u64]| list.iter().copied().map(TextAnchor).collect::<Vec<_>>();
    assert_eq!(
        sent.anchors(),
        [
            (node(12), anchors(&[1, 2, 3])),
            (node(12), anchors(&[4])),
            (node(12), anchors(&[5])),
            (node(12), anchors(&[6])),
            (node(1), anchors(&[9])),
        ]
    );
    let caret_only = batch(vec![step(caret()), step(read(Subject::Node(node(3))))]);
    assert_eq!(caret_only.anchors(), [(node(12), Vec::new())]);
    let starting = batch(vec![step(wait(1_000, 1_000))]);
    assert_eq!(starting.anchors(), [(node(12), vec![TextAnchor(40)])]);
    let sentence = batch(vec![step(Op::MoveBySentence {
        subject: node12(),
        from: TextAnchor(41),
        forward: false,
        details: TextDetails::NONE,
    })]);
    assert_eq!(sentence.anchors(), [(node(12), vec![TextAnchor(41)])]);
    let terminal = batch(vec![
        step(moved_from(Subject::Node(node(12)), 42)),
        step(moved_from(Subject::Node(node(12)), 43)),
    ]);
    assert_eq!(
        terminal.anchors(),
        [
            (node(12), vec![TextAnchor(42)]),
            (node(12), vec![TextAnchor(43)])
        ]
    );
}

/// A terminal's operations run only in an outpost, and a move names its
/// held point only on a node.
#[test]
fn a_terminals_operations_run_in_an_outpost_and_name_anchors_on_a_node() {
    let node12 = || Subject::Node(node(12));
    assert!(
        parse(
            batch(vec![step(moved_from(node12(), 1))]),
            ReaderKind::Outpost
        )
        .is_ok()
    );
    for (op, kind) in [
        (Op::Visible { subject: node12() }, OpKind::Visible),
        (moved_from(node12(), 1), OpKind::Move),
    ] {
        refused(
            vec![step(op)],
            ReaderKind::Plain,
            Malformed::NotRun {
                step: Place(0),
                op: kind,
            },
        );
    }
    refused(
        vec![
            step(self::read(node12())),
            step(moved_from(Subject::Result(Place(0)), 1)),
        ],
        ReaderKind::Outpost,
        Malformed::AnchorWithoutNode(Place(1)),
    );
}

#[test]
fn a_batch_and_its_outcomes_travel_as_json_and_back() {
    let (_, _, steps) = conversions().swap_remove(10);
    let sent = batch(steps);
    let json = serde_json::to_string(&sent).unwrap();
    assert_eq!(serde_json::from_str::<Batch>(&json).unwrap(), sent);
    let outcomes = outcomes_naming_every_node_and_anchor();
    let json = serde_json::to_string(&outcomes).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<StepOutcome>>(&json).unwrap(),
        outcomes
    );
}

/// A step runs only when every step whose result it takes is done and
/// its condition holds; an item past its list's end names no node.
#[test]
fn a_step_taking_a_result_not_done_is_skipped_and_an_item_past_the_end_names_none() {
    let read_node = NodeRead {
        node: Some(node(4)),
        window: NodeWindow {
            handle: WindowHandle(5),
            top_level: WindowHandle(5),
            root_owner: WindowHandle(5),
            process: Pid(9),
            thread: 1,
            desktop: false,
        },
        values: crate::property::PropertyValues::default(),
    };
    let earlier = [
        StepOutcome::Done(Done::Related(vec![read_node.clone()])),
        StepOutcome::Failed,
    ];
    let on = |subject| step(read(subject));
    assert_eq!(
        on(Subject::Item {
            of: Place(0),
            index: 0
        })
        .gate(&earlier),
        Ok(Gate::Run)
    );
    assert_eq!(on(Subject::Result(Place(1))).gate(&earlier), Ok(Gate::Skip));
    let unless_found = when(read(Subject::Node(node(9))), 1, Check::Found, true);
    assert_eq!(unless_found.gate(&earlier), Ok(Gate::Run));
    assert_eq!(
        Subject::Item {
            of: Place(0),
            index: 0
        }
        .earlier_nodes(&earlier),
        Some(vec![&read_node])
    );
    assert_eq!(
        Subject::Item {
            of: Place(0),
            index: 1
        }
        .earlier_nodes(&earlier),
        Some(Vec::new())
    );
    assert_eq!(Subject::Node(node(4)).earlier_nodes(&earlier), None);
}

fn lines(count: i32) -> TextMovement {
    TextMovement {
        unit: TextUnit::Line,
        count,
    }
}

/// A step under a comparison of two points: a `Move` of the subject's
/// text from `at(7)`, compared with where the step at 0 landed.
fn compared(negated: bool) -> Step {
    Step {
        op: Op::Move {
            subject: Subject::Node(node(1)),
            at: at(7),
            movement: lines(-1),
        },
        when: Some(Condition {
            test: Test::NotAfter {
                point: at(7),
                than: Point::Of(Place(0), PointOf::Moved),
            },
            negated,
        }),
    }
}

#[test]
fn a_comparison_is_left_to_the_executor_once_the_steps_it_names_are_done() {
    let moved = StepOutcome::Done(Done::Move(Moved {
        moved: 0,
        from: TextAnchor(1),
        to: TextAnchor(2),
    }));
    let compare = |negated| Gate::Compare {
        point: at(7),
        than: Point::Of(Place(0), PointOf::Moved),
        negated,
    };
    // The outcome of the step the comparison names, then what the step
    // under it does plain and negated: the condition table's rows.
    let cases = [
        (moved, compare(false), compare(true)),
        (StepOutcome::Skipped, Gate::Skip, Gate::Skip),
        (
            StepOutcome::Refused(StepRefusal::OutsideText),
            Gate::Skip,
            Gate::Run,
        ),
        (
            StepOutcome::NotThere(Missing::Anchor(TextAnchor(7))),
            Gate::Skip,
            Gate::Run,
        ),
        (StepOutcome::Failed, Gate::Skip, Gate::Run),
    ];
    for (named, plain, negated) in cases {
        let earlier = [named];
        assert_eq!(compared(false).gate(&earlier), Ok(plain), "{earlier:?}");
        assert_eq!(
            compared(true).gate(&earlier),
            Ok(negated),
            "not {earlier:?}"
        );
    }
}

#[test]
fn a_comparison_parses_only_on_an_outposts_text_step_naming_points() {
    let end = Step {
        op: Op::Move {
            subject: Subject::Node(node(1)),
            at: Point::At(TextPoint::End),
            movement: lines(1),
        },
        when: None,
    };
    let good = batch(vec![end.clone(), compared(false)]);
    assert!(parse(good.clone(), ReaderKind::Outpost).is_ok());
    // Its anchors are kept held with the step's own.
    assert_eq!(
        good.anchors(),
        vec![
            (node(1), vec![]),
            (node(1), vec![TextAnchor(7), TextAnchor(7)])
        ]
    );
    // Only an outpost compares: the plain reader runs a caret read, but
    // not one under a comparison.
    let caret_under = Step {
        op: caret(),
        when: Some(Condition {
            test: Test::NotAfter {
                point: Point::At(TextPoint::Start),
                than: Point::At(TextPoint::End),
            },
            negated: false,
        }),
    };
    assert!(parse(batch(vec![step(caret())]), ReaderKind::Plain).is_ok());
    assert!(parse(batch(vec![caret_under.clone()]), ReaderKind::Outpost).is_ok());
    assert_eq!(
        parse(batch(vec![caret_under]), ReaderKind::Plain),
        Err(Malformed::NotRun {
            step: Place(0),
            op: OpKind::Caret
        })
    );
    // A point of a step that answers no such point.
    let caret_first = batch(vec![step(caret()), compared(false)]);
    assert_eq!(
        parse(caret_first, ReaderKind::Outpost),
        Err(Malformed::WrongKind {
            step: Place(1),
            to: Place(0),
            kind: OpKind::Caret
        })
    );
    // A point of a step not before it.
    assert_eq!(
        parse(batch(vec![compared(false)]), ReaderKind::Outpost),
        Err(Malformed::Reference {
            step: Place(0),
            to: Place(0)
        })
    );
    // A step that reads no text.
    let mut object = compared(false);
    object.op = read(Subject::Node(node(1)));
    assert_eq!(
        parse(batch(vec![end.clone(), object]), ReaderKind::Outpost),
        Err(Malformed::Comparison(Place(1)))
    );
    // An anchor on a subject that is no node.
    let mut on_result = compared(false);
    on_result.op = Op::Visible {
        subject: Subject::Result(Place(0)),
    };
    let first = step(read(Subject::Node(node(1))));
    let moved = Step {
        op: Op::Move {
            subject: Subject::Node(node(1)),
            at: Point::At(TextPoint::End),
            movement: lines(1),
        },
        when: None,
    };
    let mut names_second = on_result;
    names_second.when = Some(Condition {
        test: Test::NotAfter {
            point: at(7),
            than: Point::Of(Place(1), PointOf::Moved),
        },
        negated: false,
    });
    assert_eq!(
        parse(batch(vec![first, moved, names_second]), ReaderKind::Outpost),
        Err(Malformed::AnchorWithoutNode(Place(2)))
    );
}
