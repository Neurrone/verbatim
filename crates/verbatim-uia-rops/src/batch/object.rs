//! The call arms of the object operations that read or act on one element
//! (`Read`, `Test`, `IsOneOf`, `Activate`, `Follow`), of a node, of a
//! window's own element, or of a listener's route: each runs alone, by
//! calls, never in a program. A `Read` adds reads a program would drop or
//! cut its segment for (a menu item's legacy state, a selected item's
//! value, and refreshes of their own), and a `Test` reads only what its
//! matcher asks next, so neither has a program arm (Claude, 2026-11-22).

use verbatim_model::batch::{Place, Subject};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::{StateSet, TableId, WindowHandle};
use verbatim_uia::{Cached, ComboBoxRefresh, Node, SuggestionsRefresh, Uia, Ungated};

use super::{Defect, Found, Ran, Resolve, Segment, failed};

/// The result of the object step `op`, at `place` of `segment`, by its
/// call arm.
pub(super) fn found(
    segment: &Segment<'_>,
    place: Place,
    op: &verbatim_model::batch::Op,
) -> Result<Found, Ran> {
    use verbatim_model::batch::Op;
    let (uia, resolver) = (segment.uia, segment.resolver);
    match op {
        Op::Read {
            subject: subject @ Subject::Window { .. },
            ..
        } => window_element(uia, resolver, (subject, place)).map(Found::Window),
        Op::Read {
            subject: subject @ Subject::Route(_),
            ..
        } => resolver
            .route(subject)
            .map(|(read, window)| Found::Route(Box::new(read), window)),
        Op::Test {
            subject: subject @ Subject::Window { .. },
            matcher,
        } => Ok(Found::Test(window_test(
            uia,
            resolver,
            (subject, place),
            matcher,
        )?)),
        Op::Read { subject, set } => read(segment, (subject, place), set).map(Found::Read),
        Op::Test { subject, matcher } => {
            test(uia, resolver, (subject, place), matcher).map(Found::Test)
        }
        Op::IsOneOf {
            subject,
            equal,
            at_address,
        } => is_one_of(resolver, subject, (*equal, at_address)).map(Found::IsOneOf),
        Op::Activate { subject } => activate(uia, resolver, (subject, place)).map(Found::Activate),
        Op::Follow(target) => resolver.follow(target.as_ref()).map(|()| Found::Follow),
        _ => Err(Defect("a step that is no object step").ran()),
    }
}

/// An element read in full, as a `Read` and a `Test` read it: its cache
/// filled, the table of the cell it is, its states completed from its
/// legacy state, and its first selected item's name where that is its
/// value.
#[derive(Debug)]
pub struct ElementRead {
    /// The element, its cache filled by the step's fill.
    pub node: Cached<Node>,
    /// The table of the cell it is, `None` for no cell (one call for a
    /// cell).
    pub table: Option<TableId>,
    /// Its states, with a menu item's legacy checked state read live
    /// (`verbatim_uia::map::with_legacy_checked_states`).
    pub states: StateSet,
    /// Its value when that is its first selected item's name, read live
    /// (`verbatim_uia::map::value_from_selection`): a combo box without
    /// the `Value` pattern, or a looping selector. `None` for any other
    /// node, whose value is its own.
    pub selected: Option<Option<String>>,
}

/// `Test`'s result.
#[derive(Debug)]
pub struct TestFound {
    /// Whether the matcher held.
    pub matched: bool,
    /// Each property read, in the order the matcher asked for it.
    pub values: Vec<PropertyAnswer>,
}

/// Whether a `Read` reads property `id` apart from the element's own
/// cache: by a refresh of its own, from what the outpost holds, or through
/// its text.
fn read_apart(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::Suggestions
            | PropertyId::HasValuePattern
            | PropertyId::WindowClass
            | PropertyId::ComboBoxList
            | PropertyId::ChildCount
            | PropertyId::TextSummary
    )
}

/// Whether a `Test` answers property `id` with no read of the element:
/// those the outpost holds, and those only a focus read or an MSAA object
/// answers, which a node held already answers none of (Claude,
/// 2026-11-21).
fn tested_without_read(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::FocusedWithin
            | PropertyId::MsaaAddress
            | PropertyId::WindowShown
            | PropertyId::IsContent
    )
}

/// The element `subject` names read in full ([`ElementRead`]): the
/// resolver's filled element, then the cell's table, the legacy state and
/// the selected item, each read only where it applies.
fn element_read(
    uia: &Uia,
    resolver: &dyn Resolve,
    (subject, place): (&Subject, Place),
) -> Result<ElementRead, Ran> {
    let node = resolver.filled(subject, place)?;
    let table = node.table();
    let read = node.read();
    let parts = read.parts(node.runtime_id());
    let states = verbatim_uia::map::with_legacy_checked_states(
        node.live().element(),
        parts.role,
        parts.states,
    );
    let selected = if read.value_from_selection() {
        let fill = resolver.fill(place);
        Some(fill.and_then(|fill| {
            uia.selected(node.live(), fill.request())
                .ok()
                .flatten()
                .and_then(|selected| selected.read().parts(selected.runtime_id()).name)
        }))
    } else {
        None
    };
    Ok(ElementRead {
        node,
        table,
        states,
        selected,
    })
}

/// The element `subject` names refreshed with shape `S`: one call.
fn refreshed<S: Ungated>(
    uia: &Uia,
    resolver: &dyn Resolve,
    subject: &Subject,
) -> Result<Cached<S>, Ran> {
    let element = resolver.object(subject)?;
    let request = uia
        .request_fixed::<S>()
        .map_err(|error| failed(&error.into()))?;
    verbatim_uia::Live::new(element.element.clone())
        .refresh(&request)
        .map_err(|error| failed(&error.into()))
}

/// `Read`'s call arm: the element read in full when the set names a
/// property its cache holds, each property read apart by its own read,
/// and those the outpost holds from it.
pub(super) fn read(
    segment: &Segment<'_>,
    (subject, place): (&Subject, Place),
    set: &[PropertyId],
) -> Result<PropertyValues, Ran> {
    let (uia, resolver) = (segment.uia, segment.resolver);
    let wants = |id| set.contains(&id);
    let element = if set.iter().any(|&id| !read_apart(id)) {
        Some(element_read(uia, resolver, (subject, place))?)
    } else {
        None
    };
    let suggestions = if wants(PropertyId::Suggestions) {
        Some(refreshed::<SuggestionsRefresh>(uia, resolver, subject)?)
    } else {
        None
    };
    let combo_box = if wants(PropertyId::HasValuePattern) || wants(PropertyId::ComboBoxList) {
        Some(refreshed::<ComboBoxRefresh>(uia, resolver, subject)?)
    } else {
        None
    };
    let summary = if wants(PropertyId::TextSummary) {
        text_summary(segment, subject)
    } else {
        None
    };
    Ok(PropertyValues(
        set.iter()
            .map(|&id| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::TextSummary => summary.clone(),
                    PropertyId::Suggestions => suggestions.as_ref().and_then(|element| {
                        let read = element.read();
                        read.reports_suggestions()
                            .then_some(PropertyValue::Flag(read.controls_any))
                    }),
                    PropertyId::HasValuePattern => combo_box
                        .as_ref()
                        .map(|element| PropertyValue::Flag(element.read().value_available)),
                    PropertyId::ComboBoxList => combo_box.as_ref().and_then(combo_box_list),
                    PropertyId::WindowClass
                    | PropertyId::ChildCount
                    | PropertyId::MsaaAddress
                    | PropertyId::WindowShown => resolver.held(subject, id),
                    id => element
                        .as_ref()
                        .and_then(|element| resolver.value(element, id)),
                },
            })
            .collect(),
    ))
}

/// What the subject's text says of it, as NVDA's object speech reads it
/// for report focus: the caret's line, or the selected text when the
/// selection holds some, read as one segment of the text operations
/// (`Caret`, `ReadText` of the line at the caret, and `ReadRange` of the
/// selection only when text is selected), in the segment's mode, and made
/// the property's value by the resolver. `None` when the subject has no
/// text or a read failed, as NVDA then speaks no text.
fn text_summary(segment: &Segment<'_>, subject: &Subject) -> Option<PropertyValue> {
    use verbatim_model::batch::{Check, Condition, Op, Point, PointOf, Step};
    use verbatim_model::{TextDetails, TextUnit};
    let of = |which| Point::Of(Place(0), which);
    let steps = [
        Step {
            op: Op::Caret {
                subject: subject.clone(),
                adopt: None,
            },
            when: None,
        },
        Step {
            op: Op::ReadText {
                trim: false,
                whole: false,
                subject: subject.clone(),
                at: of(PointOf::Caret),
                movement: None,
                unit: TextUnit::Line,
                count: 1,
                details: TextDetails::NONE,
            },
            when: None,
        },
        Step {
            op: Op::ReadRange {
                trim: false,
                subject: subject.clone(),
                start: of(PointOf::SelectionStart),
                end: of(PointOf::SelectionEnd),
                through: None,
            },
            when: Some(Condition {
                on: Place(0),
                check: Check::Found,
                negated: false,
            }),
        },
    ];
    let answer = super::run_batch(&Segment {
        steps: &steps,
        first: 0,
        earlier: &[],
        ..*segment
    });
    let mut ran = answer.steps.into_iter();
    let line = match (ran.next(), ran.next()) {
        (Some(Ran::Done(Found::Caret(_))), Some(Ran::Done(Found::ReadText(mut line)))) => {
            line.units.swap_remove(0).text
        }
        _ => return None,
    };
    let selected = match ran.next() {
        Some(Ran::Done(Found::ReadRange(selected))) => Some(selected),
        Some(Ran::Unrun(super::Unrun::Skipped)) => None,
        _ => return None,
    };
    segment
        .resolver
        .text_summary((&line, selected.as_deref()), answer.read_classically)
}

/// The list window of a classic .NET Framework Windows Forms combo box,
/// by `GetComboBoxInfo`, from Windows.
fn combo_box_list(element: &Cached<ComboBoxRefresh>) -> Option<PropertyValue> {
    let read = element.read();
    let class =
        verbatim_model::class::normalize(&verbatim_win32::window::window_class_name(read.hwnd))
            .to_owned();
    if !read.is_net_framework_win_forms(&class) {
        return None;
    }
    verbatim_win32::window::combo_box_list(read.hwnd)
        .map(|list| PropertyValue::Window(WindowHandle(u64::from_ne_bytes(list.to_ne_bytes()))))
}

/// `Test`'s call arm: the matcher evaluated lazily, each property read at
/// most once when the matcher first needs it, in its order, the element
/// read in full at most once, for the first property the outpost does not
/// hold.
pub(super) fn test(
    uia: &Uia,
    resolver: &dyn Resolve,
    (subject, place): (&Subject, Place),
    matcher: &Matcher,
) -> Result<TestFound, Ran> {
    let mut element: Option<ElementRead> = None;
    let mut values = Vec::new();
    let holds = matcher.evaluate(&mut |id| {
        let value = if tested_without_read(id) {
            match id {
                PropertyId::MsaaAddress | PropertyId::WindowShown => resolver.held(subject, id),
                _ => None,
            }
        } else {
            if element.is_none() {
                element = Some(element_read(uia, resolver, (subject, place))?);
            }
            element
                .as_ref()
                .and_then(|element| resolver.value(element, id))
        };
        values.push(PropertyAnswer {
            id,
            value: value.clone(),
        });
        Ok(value)
    })?;
    Ok(TestFound {
        matched: holds,
        values,
    })
}

/// The own element of the window `subject` names, filled by the fill of
/// the step at `place`: one call, whatever serves the window, since Core
/// named its backend. Not there when it cannot be read.
fn window_element(
    uia: &Uia,
    resolver: &dyn Resolve,
    (subject, place): (&Subject, Place),
) -> Result<Cached<Node>, Ran> {
    let hwnd = resolver
        .window(subject)
        .ok_or_else(|| Defect("a window subject its resolver names no window of").ran())?;
    let fill = resolver
        .fill(place)
        .ok_or_else(|| Defect("a window read with no fill").ran())?;
    uia.any_window_element(hwnd, fill.request())
        .map_err(|error| {
            tracing::debug!(%error, "a window's own element could not be read");
            Ran::Unrun(super::Unrun::NotThere(
                verbatim_model::batch::Missing::Object,
            ))
        })
}

/// `Test`'s call arm on a window's own element: the matcher evaluated
/// lazily, whether the window is shown read from Windows with no call into
/// the application, and the element read at most once, for the first
/// other property; a property of an element that cannot be read answers
/// none.
fn window_test(
    uia: &Uia,
    resolver: &dyn Resolve,
    (subject, place): (&Subject, Place),
    matcher: &Matcher,
) -> Result<TestFound, Ran> {
    let hwnd = resolver
        .window(subject)
        .ok_or_else(|| Defect("a window subject its resolver names no window of").ran())?;
    let mut element: Option<Option<Cached<Node>>> = None;
    let mut values = Vec::new();
    let Ok(holds) = matcher.evaluate::<std::convert::Infallible>(&mut |id| {
        let value = if id == PropertyId::WindowShown {
            Some(PropertyValue::Flag(
                verbatim_win32::window::window_is_visible(hwnd),
            ))
        } else {
            element
                .get_or_insert_with(|| window_element(uia, resolver, (subject, place)).ok())
                .as_ref()
                .and_then(|element| resolver.node_value(element, id))
        };
        values.push(PropertyAnswer {
            id,
            value: value.clone(),
        });
        Ok(value)
    });
    Ok(TestFound {
        matched: holds,
        values,
    })
}

/// `IsOneOf`'s call arm: the subject's node compared by its id with the
/// equal node and the nodes at an address, with no call.
pub(super) fn is_one_of(
    resolver: &dyn Resolve,
    subject: &Subject,
    (equal, at_address): (Option<verbatim_model::NodeId>, &[verbatim_model::NodeId]),
) -> Result<Option<u32>, Ran> {
    let node = resolver
        .node_id(subject)
        .ok_or(Ran::Unrun(super::Unrun::NotThere(
            verbatim_model::batch::Missing::Object,
        )))?;
    Ok(equal
        .into_iter()
        .chain(at_address.iter().copied())
        .position(|each| each == node)
        .and_then(|place| u32::try_from(place).ok()))
}

/// `Activate`'s call arm: exactly the subject activated, its element
/// filled first, answering the action's name. A failed activation is a
/// failure, whatever its cause, which the dispatcher's walk up the parents
/// goes on from.
pub(super) fn activate(
    uia: &Uia,
    resolver: &dyn Resolve,
    (subject, place): (&Subject, Place),
) -> Result<Option<verbatim_model::ActionName>, Ran> {
    let node = resolver.filled(subject, place)?;
    uia.activate(node.live().element()).map_err(|error| {
        tracing::debug!(%error, "UIA activation failed");
        Ran::Failed
    })
}
