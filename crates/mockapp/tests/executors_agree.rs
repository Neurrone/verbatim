//! The request list's executors agree (the verbatim-outpost crate guide,
//! "Core's requests"): every batch shape runs through a real outpost's own
//! `batch::answer`, its resolvers and its executors, once with remote
//! operations on and once with them off, against the same mockapp, and
//! each batch's outcome is the same both ways. Built with the `faults`
//! feature, as `cargo xtask faults` builds it, it runs instead the caret's
//! reads with each of the program faults the fallback rule covers armed on
//! the remote run, which still answer as the calls alone do.
//!
//! Each run starts with a read of the focus and a batch that names the
//! nodes after it ([`naming`]). The two runs share one mockapp, one after the other, so the windows and
//! the node ids the two outposts mint, in the same order, are the same;
//! the one activation selects an item, which a second selection leaves as
//! it is.

#![cfg_attr(
    feature = "faults",
    expect(dead_code, reason = "the fault build runs only the faulted cases")
)]

mod common;
#[path = "common/harness.rs"]
mod harness;

#[cfg(feature = "faults")]
use verbatim_faults::Fault;
use verbatim_model::batch::{
    Batch, Done, FollowTarget, Op, Place, Placement, Point, PointOf, Relation, Step, StepOutcome,
    Subject, View, Walk,
};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{FollowGroup, Outcome, Reply, WindowVerdict};
use verbatim_model::{NodeId, Role, State, TextDetails, TextPoint, TextUnit};
use verbatim_process::arguments::OutpostOptions;
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// Every property a node read can answer that the outpost reads of a UI
/// Automation node, those read apart included.
const EVERY: &[PropertyId] = &[
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Description,
    PropertyId::KeyboardShortcut,
    PropertyId::PositionInSet,
    PropertyId::SetSize,
    PropertyId::Level,
    PropertyId::Rect,
    PropertyId::ClassName,
    PropertyId::NativeWindow,
    PropertyId::Process,
    PropertyId::RuntimeId,
    PropertyId::HasKeyboardFocus,
    PropertyId::Suggestions,
    PropertyId::HasValuePattern,
    PropertyId::WindowClass,
    PropertyId::ComboBoxList,
    PropertyId::MsaaAddress,
    PropertyId::WindowShown,
];

/// A batch of `ops`, each run unconditionally, in the event queue's order.
fn batch(ops: Vec<Op>) -> Batch {
    Batch {
        steps: ops.into_iter().map(|op| Step { op, when: None }).collect(),
        placement: Placement::Queue,
        spare: None,
    }
}

/// The batch that names a fixture's nodes: the focus's next three
/// siblings and every node below the second. It goes no further back or
/// up than the focus: before a window's first child, and at the window
/// itself, UI Automation's raw view holds the title bar's elements, read
/// in this process through its proxies, which wait on the Desktop Window
/// Manager (`books/dev/src/how-to/troubleshooting.md`).
fn naming(focus: NodeId) -> Batch {
    let next = |subject| Op::Related {
        subject,
        relation: Relation::NextSibling,
        set: vec![PropertyId::Name],
    };
    let first = |of| Subject::Item {
        of: Place(of),
        index: 0,
    };
    batch(vec![
        next(Subject::Node(focus)),
        next(first(0)),
        next(first(1)),
        Op::Descendants {
            subject: first(1),
            walk: Box::new(Walk {
                depth: 4,
                view: View::Raw,
                matcher: None,
                prune: None,
                include_subject: false,
                limit: 64,
                nodes: 64,
                set: vec![PropertyId::Name],
            }),
        },
    ])
}

/// The focus a read of it minted.
fn focus_of(outcome: &Outcome) -> NodeId {
    let Outcome::Result(Reply::Steps(steps)) = outcome else {
        panic!("the focus read's steps, not {outcome:?}")
    };
    let Some(StepOutcome::Done(Done::FocusRead(read))) = steps.first() else {
        panic!("the focus read, not {steps:?}")
    };
    read.focus.read.node.expect("the focus's node")
}

/// The nodes of a fixture by name, as the walk of its window minted them.
struct Named(Vec<(String, NodeId)>);

impl Named {
    /// The nodes `named`'s outcomes read with a name, and the focus,
    /// `focus`, named `focus_name`.
    fn of((focus, focus_name): (NodeId, &str), named: &Outcome) -> Self {
        let Outcome::Result(Reply::Steps(steps)) = named else {
            panic!("the naming batch's steps, not {named:?}")
        };
        let mut reads = Vec::new();
        for step in steps {
            match step {
                StepOutcome::Done(Done::Related(related)) => reads.extend(related),
                StepOutcome::Done(Done::Descendants(walked)) => {
                    reads.extend(walked.nodes.iter().map(|descendant| &descendant.node));
                }
                _ => {}
            }
        }
        let mut nodes = vec![(focus_name.to_owned(), focus)];
        nodes.extend(reads.into_iter().filter_map(|read| {
            match read.values.answer(PropertyId::Name)?.value.as_ref()? {
                PropertyValue::Text(name) => Some((name.clone(), read.node?)),
                _ => None,
            }
        }));
        Self(nodes)
    }

    /// The node named `name`.
    fn node(&self, name: &str) -> Subject {
        Subject::Node(self.id(name))
    }

    /// The one node named `name`.
    fn id(&self, name: &str) -> NodeId {
        let mut found = self.0.iter().filter(|(named, _)| named == name);
        let (Some((_, node)), None) = (found.next(), found.next()) else {
            panic!("not one node named {name:?} in {:?}", self.0)
        };
        *node
    }
}

/// A batch with its label and the program fault armed on its remote run,
/// if any.
type Labelled = (String, Batch, Option<Armed>);

/// A program fault and the place it fires at.
#[cfg(feature = "faults")]
type Armed = (Fault, u8);

/// No fault is armed in a build without fault points.
#[cfg(not(feature = "faults"))]
type Armed = std::convert::Infallible;

/// The batches of one fixture, made from the nodes its walk named.
type Script = fn(HWND, &Named) -> Vec<Labelled>;

/// The outcome of each batch of `script` against mockapp's window `hwnd`,
/// its element `focused` read as the focus, read by an outpost with remote operations `remote`, after the walk that
/// names the nodes; on the remote run each batch's fault is armed to fire
/// once.
fn outcomes(
    (app, hwnd, (focused, focus_name)): (
        &common::MockApp,
        HWND,
        (
            &windows::Win32::UI::Accessibility::IUIAutomationElement,
            &str,
        ),
    ),
    remote: bool,
    script: Script,
) -> Vec<(String, String)> {
    let outpost = OutpostUnderTest::with_options(
        app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost.read_focus_as(focused);
    let (focus, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        batch(vec![Op::FocusRead {
            subject: Subject::Focus,
            known: Vec::new(),
            set: common::outpost::focus_set(),
            details: TextDetails::NONE,
        }]),
    );
    let focus_node = focus_of(&focus.outcome);
    let (walked, _) = outpost.batch(hwnd, WindowVerdict::Uia, naming(focus_node));
    let named = Named::of((focus_node, focus_name), &walked.outcome);
    let mut outcomes = vec![
        ("the focus".to_owned(), format!("{:#?}", focus.outcome)),
        ("the naming".to_owned(), format!("{:#?}", walked.outcome)),
    ];
    for (label, sent, fault) in script(hwnd, &named) {
        #[cfg(feature = "faults")]
        let armed = fault
            .filter(|_| remote)
            .map(|(fault, place)| verbatim_faults::arm(fault, move |at: &u8| *at == place, 1));
        #[cfg(not(feature = "faults"))]
        let _: Option<Armed> = fault;
        let (answer, _) = outpost.batch(hwnd, WindowVerdict::Uia, sent);
        #[cfg(feature = "faults")]
        drop(armed);
        outcomes.push((label, format!("{:#?}", answer.outcome)));
    }
    drop(outpost);
    outcomes
}

/// Runs `script` against `fixture`, its element `focus`, named
/// `focus_name`, focused, with remote operations on, then off, and asserts
/// each batch answered the same both ways.
fn agree(fixture: &str, (focus, focus_name): (&str, &str), script: Script) {
    common::init_com();
    let title = common::unique_title("mockapp-executors-agree");
    let mut app = common::spawn_visible(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    app.send(&format!("set-focus {focus}"));
    // The focused element is found once, before either outpost runs.
    let uia = verbatim_uia::Uia::new().expect("a UIA client");
    let focused = common::listener_focus(&uia, hwnd.0 as isize)
        .live()
        .element()
        .clone();
    let remote = outcomes((&app, hwnd, (&focused, focus_name)), true, script);
    let classic = outcomes((&app, hwnd, (&focused, focus_name)), false, script);
    assert_eq!(remote.len(), classic.len());
    for ((label, remote), (_, classic)) in remote.iter().zip(&classic) {
        assert_eq!(
            remote, classic,
            "{fixture}, {label}: remote operations on and off answer differently"
        );
    }
    app.quit();
}

/// The object operations on `controller.json`'s nodes: a read of every
/// property, a test that holds and one that does not, each relation, the
/// ancestors, a walk, the identity check and a follow. No step reaches
/// the window's own element or the title bar before its first child, as
/// [`naming`] says why.
#[expect(clippy::too_many_lines, reason = "one batch per operation")]
fn object_operations(_: HWND, named: &Named) -> Vec<Labelled> {
    let mut batches = Vec::new();
    for name in ["Other box", "Suggestions", "Recent", "Display settings"] {
        let node = named.node(name);
        batches.push((
            format!("a read of {name}"),
            batch(vec![Op::Read {
                subject: node.clone(),
                set: EVERY.to_vec(),
            }]),
            None,
        ));
        batches.push((
            format!("tests of {name}"),
            batch(vec![
                Op::Test {
                    subject: node.clone(),
                    matcher: Matcher::All(vec![
                        Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::ListItem)),
                        Matcher::HasState(State::Selectable),
                    ]),
                },
                Op::Test {
                    subject: node.clone(),
                    matcher: Matcher::Any(vec![
                        Matcher::Absent(PropertyId::Description),
                        Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(true)),
                    ]),
                },
            ]),
            None,
        ));
        for relations in [
            [
                Relation::FirstChild,
                Relation::LastChild,
                Relation::FirstChild,
            ],
            [
                Relation::NextSibling,
                Relation::PreviousSibling,
                Relation::ControllerFor,
            ],
        ] {
            batches.push((
                format!("{relations:?} of {name}"),
                batch(
                    relations
                        .into_iter()
                        .map(|relation| Op::Related {
                            subject: node.clone(),
                            relation,
                            set: vec![PropertyId::Name, PropertyId::Role, PropertyId::States],
                        })
                        .collect(),
                ),
                None,
            ));
        }
        batches.push((
            format!("the descendants of {name}"),
            batch(vec![Op::Descendants {
                subject: node.clone(),
                walk: Box::new(Walk {
                    depth: 4,
                    view: View::Control,
                    matcher: Some(Matcher::HasState(State::Selectable)),
                    prune: None,
                    include_subject: true,
                    limit: 8,
                    nodes: 64,
                    set: vec![PropertyId::Name],
                }),
            }]),
            None,
        ));
        batches.push((
            format!("identity checks of {name}"),
            batch(vec![
                Op::IsOneOf {
                    subject: node.clone(),
                    equal: Some(named.id("Search box")),
                    at_address: vec![named.id("Suggestions"), named.id(name)],
                },
                Op::IsOneOf {
                    subject: node.clone(),
                    equal: None,
                    at_address: vec![named.id("Other box")],
                },
            ]),
            None,
        ));
    }
    batches.push((
        "the parent and ancestors of Display settings".to_owned(),
        batch(vec![
            Op::Related {
                subject: named.node("Display settings"),
                relation: Relation::Parent,
                set: vec![PropertyId::Name, PropertyId::Role],
            },
            Op::Ancestors {
                subject: named.node("Display settings"),
                until: Vec::new(),
                depth: 2,
                set: vec![PropertyId::Name, PropertyId::Role],
            },
        ]),
        None,
    ));
    batches.push((
        "the first selected item and an item of a result".to_owned(),
        batch(vec![
            Op::Related {
                subject: named.node("Suggestions"),
                relation: Relation::FirstSelectedItem,
                set: vec![PropertyId::Name],
            },
            Op::Related {
                subject: named.node("Suggestions"),
                relation: Relation::FirstChild,
                set: vec![PropertyId::Name],
            },
            Op::Read {
                subject: Subject::Item {
                    of: Place(1),
                    index: 0,
                },
                set: vec![PropertyId::Name, PropertyId::Role],
            },
        ]),
        None,
    ));
    batches.push((
        "a follow and its end".to_owned(),
        batch(vec![
            Op::Follow(Some(FollowTarget {
                subject: named.node("Search box"),
                group: FollowGroup::Base,
                extra: None,
            })),
            Op::Follow(None),
        ]),
        None,
    ));
    batches
}

/// The activations on `activation.json`: an item that selects, selected
/// again on the second run, and a check box that invokes. An object with
/// nothing to do is left out: its walk up reaches the window and the
/// desktop.
fn activations(_: HWND, named: &Named) -> Vec<Labelled> {
    vec![(
        "activations".to_owned(),
        batch(vec![
            Op::Activate {
                subject: named.node("Select only"),
            },
            Op::Activate {
                subject: named.node("Invoke, toggle or select"),
            },
            Op::Read {
                subject: named.node("Select only"),
                set: vec![PropertyId::States],
            },
        ]),
        None,
    )]
}

/// The caret with the line at it, the whole text and where the caret is,
/// on `doc`.
fn caret_reads(doc: &Subject) -> Batch {
    batch(vec![
        Op::Caret {
            subject: doc.clone(),
            adopt: None,
        },
        Op::ReadText {
            subject: doc.clone(),
            at: Point::Of(Place(0), PointOf::Caret),
            movement: None,
            unit: TextUnit::Line,
            count: 1,
            details: TextDetails::ALL,
        },
        Op::ReadRange {
            subject: doc.clone(),
            start: Point::At(TextPoint::Start),
            end: Point::At(TextPoint::End),
            through: None,
        },
        Op::Locate {
            subject: doc.clone(),
            at: Point::Of(Place(0), PointOf::Caret),
        },
    ])
}

/// The text operations on `text.json`'s document: the caret's reads, and
/// its summary.
fn text_operations(_: HWND, named: &Named) -> Vec<Labelled> {
    let doc = named.node("Notes");
    vec![
        ("the caret's reads".to_owned(), caret_reads(&doc), None),
        (
            "the summary".to_owned(),
            batch(vec![Op::Read {
                subject: doc,
                set: vec![PropertyId::Name, PropertyId::TextSummary],
            }]),
            None,
        ),
    ]
}

/// The caret's reads on `text.json`'s document, with each program fault
/// armed in turn on the remote run.
#[cfg(feature = "faults")]
fn faulted_text_operations(_: HWND, named: &Named) -> Vec<Labelled> {
    let doc = named.node("Notes");
    [
        (Fault::ProgramStepFails, 1),
        (Fault::ProgramStops, 2),
        (Fault::ProgramAnswerLost, 0),
    ]
    .into_iter()
    .map(|fault| {
        (
            format!("the caret's reads, {fault:?}"),
            caret_reads(&doc),
            Some(fault),
        )
    })
    .collect()
}

fn object_operations_agree() {
    agree(
        "controller.json",
        ("search", "Search box"),
        object_operations,
    );
}

fn activations_agree() {
    agree(
        "activation.json",
        ("all_three", "Invoke, toggle or select"),
        activations,
    );
}

fn text_operations_agree() {
    agree("text.json", ("doc", "Notes"), text_operations);
}

#[cfg(feature = "faults")]
fn faulted_text_operations_agree() {
    agree("text.json", ("doc", "Notes"), faulted_text_operations);
}

/// The ordinary cases run with the suite; the fault step, which builds the
/// fault points in, runs the faulted ones alone.
#[cfg(not(feature = "faults"))]
fn main() {
    harness::run_isolated(&[
        ("object_operations_agree", object_operations_agree),
        ("activations_agree", activations_agree),
        ("text_operations_agree", text_operations_agree),
    ]);
}

#[cfg(feature = "faults")]
fn main() {
    harness::run_isolated(&[(
        "faulted_text_operations_agree",
        faulted_text_operations_agree,
    )]);
}
