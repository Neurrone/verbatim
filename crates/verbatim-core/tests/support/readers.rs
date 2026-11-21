//! A reader standing in for the outpost and the listener in Core's tests:
//! what the test says a reader found of the objects a batch names
//! ([`Found`]), turned into the outcomes of the batch Core sent, each step
//! under its condition as a reader gates it. It models what a reader
//! reports, not how Core builds its batches: the hidden-dialog check, the
//! focused state and the identity check are answered by what was found,
//! whatever matcher the test carries.

#![allow(dead_code, reason = "each test file uses part of the support")]

use verbatim_model::batch::{
    AncestorsEnd, Ancestry, Batch, Descendant, Descended, Done, Gate, Missing, Op, StepOutcome,
    Subject, Tested,
};
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusRead, NodeRead, NodeWindow, Outcome, Refusal, Reply, RequestNumber,
};
use verbatim_model::{Pid, Role, WindowHandle};

/// What a reader found of the objects a batch names.
#[derive(Clone, Debug)]
pub enum Found {
    /// The object, or the application's focus, read in full: with the
    /// focused state when it is an MSAA focus.
    Read(FocusRead),
    /// The object found without the focused state.
    Unfocused,
    /// The object found with the focused state, its facts alone read.
    Focused,
    /// The object found at the ids and reading as the identity named.
    Same,
    /// A foreground or menu start found, its role `role`, and read in full
    /// as `read` when a full read is asked.
    Role(Option<Role>, Option<FocusRead>),
    /// The object is not there.
    NotThere,
    /// The window's top-level window is a hidden dialog.
    HiddenDialog(NodeWindow),
    /// A hidden top-level window whose role could not be read.
    HiddenUnread,
    /// The object's window is read through UI Automation: the batch is
    /// refused whole.
    ThroughUia,
    /// The listener's read of a kept element: the element, its ancestors
    /// nearest first, and where the walk ended.
    Walked(NodeRead, Vec<NodeRead>, AncestorsEnd),
    /// A selection's item read by its route, and whether the committed
    /// focus controls it: `None` for a controller-for read that failed.
    Item(NodeRead, Option<bool>),
    /// The object an event names, or a node, read as this: none of the
    /// nodes Core names, and with no descendants.
    Object(NodeRead),
}

/// No window, for a read whose window nothing judges.
pub fn no_window() -> NodeWindow {
    NodeWindow {
        handle: WindowHandle(0),
        top_level: WindowHandle(0),
        root_owner: WindowHandle(0),
        process: Pid(0),
        thread: 0,
        desktop: false,
    }
}

fn values(answers: &[(PropertyId, Option<PropertyValue>)]) -> PropertyValues {
    PropertyValues(
        answers
            .iter()
            .map(|(id, value)| PropertyAnswer {
                id: *id,
                value: value.clone(),
            })
            .collect(),
    )
}

fn only(values: &PropertyValues, set: &[PropertyId]) -> PropertyValues {
    PropertyValues(
        set.iter()
            .map(|&id| PropertyAnswer {
                id,
                value: values.answer(id).and_then(|answer| answer.value.clone()),
            })
            .collect(),
    )
}

fn tested(matched: bool, window: NodeWindow, read: PropertyValues) -> StepOutcome {
    StepOutcome::Done(Done::Test(Tested {
        matched,
        node: NodeRead {
            node: None,
            window,
            values: read,
        },
    }))
}

/// One step's outcome, `op` run on what was `found`.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per operation a reader answers"
)]
fn answer_step(op: &Op, found: &Found) -> StepOutcome {
    let gone = StepOutcome::NotThere(Missing::Object);
    let focused = |read: &FocusRead| read.msaa.map(|_| PropertyValue::Flag(true));
    match op {
        Op::Test {
            subject: Subject::Window { .. },
            ..
        } => match found {
            Found::HiddenDialog(window) => tested(
                true,
                *window,
                values(&[
                    (PropertyId::WindowShown, Some(PropertyValue::Flag(false))),
                    (PropertyId::Role, Some(PropertyValue::Role(Role::Dialog))),
                ]),
            ),
            Found::HiddenUnread => tested(
                false,
                no_window(),
                values(&[
                    (PropertyId::WindowShown, Some(PropertyValue::Flag(false))),
                    (PropertyId::Role, None),
                ]),
            ),
            _ => tested(
                false,
                no_window(),
                values(&[(PropertyId::WindowShown, Some(PropertyValue::Flag(true)))]),
            ),
        },
        Op::Test { .. } => {
            let within = |value| values(&[(PropertyId::FocusedWithin, value)]);
            match found {
                Found::Unfocused => {
                    tested(true, no_window(), within(Some(PropertyValue::Flag(false))))
                }
                Found::Same => tested(true, no_window(), within(Some(PropertyValue::Flag(true)))),
                Found::Focused => {
                    tested(false, no_window(), within(Some(PropertyValue::Flag(true))))
                }
                Found::Read(read) => tested(false, read.focus.read.window, within(focused(read))),
                Found::Role(..) => tested(false, no_window(), within(None)),
                _ => gone,
            }
        }
        Op::Read {
            subject: Subject::Msaa { .. },
            set,
        } => match found {
            Found::Role(role, _) => StepOutcome::Done(Done::Read(NodeRead {
                node: None,
                window: no_window(),
                values: only(
                    &values(&[(PropertyId::Role, role.map(PropertyValue::Role))]),
                    set,
                ),
            })),
            Found::Read(read) => StepOutcome::Done(Done::Read(NodeRead {
                values: only(&read.focus.read.values, set),
                ..read.focus.read.clone()
            })),
            Found::Object(read) => StepOutcome::Done(Done::Read(NodeRead {
                node: None,
                values: only(&read.values, set),
                ..read.clone()
            })),
            Found::Unfocused | Found::Same | Found::Focused => {
                StepOutcome::Done(Done::Read(NodeRead {
                    node: None,
                    window: no_window(),
                    values: only(&PropertyValues::default(), set),
                }))
            }
            _ => gone,
        },
        Op::Read { set, .. } => match found {
            Found::Walked(element, ..) => StepOutcome::Done(Done::Read(NodeRead {
                values: only(&element.values, set),
                ..element.clone()
            })),
            Found::Item(item, _) => StepOutcome::Done(Done::Read(item.clone())),
            Found::Object(read) => StepOutcome::Done(Done::Read(NodeRead {
                values: only(&read.values, set),
                ..read.clone()
            })),
            _ => gone,
        },
        Op::IsOneOf { .. } => match found {
            Found::Object(_) => StepOutcome::Done(Done::IsOneOf(None)),
            _ => gone,
        },
        Op::Ancestors { set, .. } => match found {
            Found::Walked(_, ancestors, end) => StepOutcome::Done(Done::Ancestors(Ancestry {
                ancestors: ancestors
                    .iter()
                    .map(|ancestor| NodeRead {
                        values: only(&ancestor.values, set),
                        ..ancestor.clone()
                    })
                    .collect(),
                end: *end,
            })),
            _ => gone,
        },
        Op::Related { .. } => match found {
            Found::Item(_, None) => StepOutcome::Failed,
            Found::Item(item, Some(_)) => StepOutcome::Done(Done::Related(vec![item.clone()])),
            _ => gone,
        },
        Op::Descendants { .. } => match found {
            Found::Object(read) => StepOutcome::Done(Done::Descendants(Descended {
                nodes: vec![Descendant {
                    node: read.clone(),
                    parent: None,
                    root: 0,
                }],
                complete: true,
            })),
            Found::Item(item, Some(controlled)) => {
                StepOutcome::Done(Done::Descendants(Descended {
                    nodes: controlled
                        .then(|| Descendant {
                            node: item.clone(),
                            parent: None,
                            root: 0,
                        })
                        .into_iter()
                        .collect(),
                    complete: true,
                }))
            }
            _ => gone,
        },
        Op::FocusRead { .. } => match found {
            Found::Read(read) | Found::Role(_, Some(read)) => {
                StepOutcome::Done(Done::FocusRead(Box::new(read.clone())))
            }
            _ => gone,
        },
        // A registration moved, whatever was found.
        Op::Follow(_) => StepOutcome::Done(Done::Follow),
        _ => StepOutcome::Refused(verbatim_model::batch::StepRefusal::Unsupported),
    }
}

/// The outcome a reader answers `batch` with, having found `found`.
pub fn outcome(batch: &Batch, found: &Found) -> Outcome {
    if matches!(found, Found::ThroughUia) {
        return Outcome::Refused(Refusal::ReadThroughUia);
    }
    let mut outcomes = Vec::new();
    for step in &batch.steps {
        let outcome = match step.gate(&outcomes) {
            Ok(Gate::Run) => answer_step(&step.op, found),
            Ok(Gate::Skip) => StepOutcome::Skipped,
            Err(mismatch) => panic!("a condition on a result it cannot test: {mismatch:?}"),
        };
        outcomes.push(outcome);
    }
    Outcome::Result(Reply::Steps(outcomes))
}

/// A request's number, as a test names it.
pub fn number(number: u64) -> RequestNumber {
    RequestNumber(number)
}
