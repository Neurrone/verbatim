//! The ordinary-calls executor: each fusable operation through the UI
//! Automation adapter, one call at a time, as the classic twins made the
//! calls. It answers a segment the setting or the window keeps from
//! programs, the steps no program can take, and the fallback's reruns.

use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};

use verbatim_model::batch::{Edges, Missing, PointOf, Relation, View};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};
use verbatim_uia::{Cached, Direction, ElementExt, Fill, Live, Node};

use super::walk::{Reads, Walk};
use super::{
    Answer, CaretFound, Compared, Ends, Found, Input, Object, Point, Prior, Ran, Related, Segment,
    State, UnitsFound, Unrun, failed,
};
use crate::caret::{Attributes, MAX_COMPARED_TEXT, RangeEnd, classic_changes, classic_runs};
use crate::error::Error;
use crate::text::{
    FoundPoint, TextTarget, classic_caret, classic_location, classic_point, classic_range,
    classic_units, collapsed_copy,
};

/// Whether a test armed the step at `index` to fail its calls
/// (`verbatim_faults::Fault::StepCallsFail`).
#[cfg(any(test, feature = "faults"))]
fn injected_failure(state: &State<'_, '_>, index: usize) -> bool {
    verbatim_faults::hit(
        verbatim_faults::Fault::StepCallsFail,
        &state.segment.steps[index].place.0,
    )
}

/// No test arms a failure in a build without fault points.
#[cfg(not(any(test, feature = "faults")))]
fn injected_failure(_: &State<'_, '_>, _: usize) -> bool {
    false
}

/// Runs every step of `segment` by calls, in order: the developer setting
/// with remote operations off, and a window marked for classic reading.
#[must_use]
pub fn run_calls(segment: &Segment<'_>) -> Answer {
    let mut state = State::new(segment);
    state.calls(0..segment.steps.len());
    state.answer()
}

/// Runs `input`, the step at `index` of `state`'s segment, by calls, its
/// gate already passed.
pub(super) fn run(state: &State<'_, '_>, index: usize, input: &Input<'_>) -> Ran {
    if injected_failure(state, index) {
        return failed(&Error::Uia(windows::Win32::Foundation::E_FAIL.into()));
    }
    let uia = state.segment.uia;
    let found = match input {
        Input::Answered(unrun) => return Ran::Unrun(*unrun),
        Input::Related {
            subject,
            relation,
            fill,
        } => match one(state, index, subject) {
            Ok((element, window)) => related(uia, &element, window, *relation, fill),
            Err(ran) => return ran,
        },
        Input::Descendants { subject, walk } => match starts(state, index, subject) {
            Ok(starts) => {
                let mut walk = Walk::new(walk, starts);
                walk.run(uia, Reads::Calls)
                    .map(|()| Found::Descendants(walk.finish()))
            }
            Err(ran) => return ran,
        },
        Input::Caret { text } => caret(*text).map(Found::Caret),
        Input::ReadText { text, at, read } => point(state, index, *text, at).and_then(|from| {
            let answer = classic_units(
                *text,
                from,
                (read.movement, read.unit, read.count),
                (read.max_text, read.max_total, read.culture),
            )?;
            let (runs, unsupported) = formats(
                &answer.units[0],
                read.formats,
                read.learning,
                read.unit,
                read.max_text,
            )?;
            Ok(Found::ReadText(UnitsFound {
                moved: answer.moved,
                units: answer.units,
                ended: answer.ended,
                runs,
                unsupported,
            }))
        }),
        Input::ReadRange {
            text,
            ends,
            max_text,
        } => range(state, index, *text, ends)
            .and_then(|range| Ok(Found::ReadRange(range.text(*max_text)?))),
        Input::Select { text, ends } => match range(state, index, *text, ends) {
            // A refused selection is the step's refusal, as the program's
            // catch answers it.
            Ok(range) => match range.select() {
                Ok(()) => Ok(Found::Select),
                Err(_) => {
                    return Ran::Unrun(Unrun::Refused(
                        verbatim_model::batch::StepRefusal::Unsupported,
                    ));
                }
            },
            Err(error) => Err(error),
        },
        Input::CompareSelection {
            before,
            after,
            max_text,
            ..
        } => compare(state, index, before, *after, *max_text).map(Found::CompareSelection),
        Input::Locate { text, at } => point(state, index, *text, at)
            .and_then(|at| classic_location(&at))
            .map(Found::Locate),
        Input::WaitCheck(check) => wait_check(check),
    };
    match found {
        Ok(found) => Ran::Done(found),
        Err(error) => failed(&error),
    }
}

/// The one element `subject` names, with its own window, 0 when not known;
/// the step's outcome when it names none.
fn one(
    state: &State<'_, '_>,
    index: usize,
    subject: &Object<'_>,
) -> Result<(IUIAutomationElement, isize), Ran> {
    match *subject {
        Object::Held { element, window } => Ok((element.clone(), window)),
        Object::Result(_) => Err(super::Defect("a step names a list as its one subject").ran()),
        Object::Item(place, item) => match list(state, place, index)?.get(item as usize) {
            Some(node) => Ok(node_of(node)),
            None => Err(Ran::Unrun(Unrun::NotThere(Missing::Object))),
        },
    }
}

/// A node's element and its own window.
fn node_of(node: &Cached<Node>) -> (IUIAutomationElement, isize) {
    (node.live().element().clone(), node.read().role.hwnd)
}

/// The nodes of the list an earlier step of the segment answered.
fn list<'r>(
    state: &'r State<'_, '_>,
    place: verbatim_model::batch::Place,
    index: usize,
) -> Result<Vec<&'r Cached<Node>>, Ran> {
    match state.named(place, index) {
        Some(Ran::Done(Found::Related(related))) => Ok(related.nodes.iter().collect()),
        Some(Ran::Done(Found::Descendants(descendants))) => Ok(descendants
            .nodes
            .iter()
            .map(|descendant| &descendant.node)
            .collect()),
        _ => Err(super::Defect("a step names no list of elements").ran()),
    }
}

/// Where a walk down the tree starts: its subject, or every node of an
/// earlier list.
fn starts(
    state: &State<'_, '_>,
    index: usize,
    subject: &Object<'_>,
) -> Result<Vec<IUIAutomationElement>, Ran> {
    match *subject {
        Object::Result(place) => Ok(list(state, place, index)?
            .into_iter()
            .map(|node| node.live().element().clone())
            .collect()),
        _ => one(state, index, subject).map(|(element, _)| vec![element]),
    }
}

/// `element` with its cache filled by `fill`: one call.
fn refresh(element: &IUIAutomationElement, fill: &Fill<Node>) -> Result<Cached<Node>, Error> {
    Ok(Live::new(element.clone()).refresh(fill.request())?)
}

/// The objects one step away from `element`, whose own window is
/// `window`: a step through the tree as the navigation step takes it, the
/// nearest window (one call unless it has its own) and one tree walker
/// step; the controller-for relation's elements, each filled (one call
/// each and one for the relation); or the first selected item
/// ([`verbatim_uia::Uia::selected`]).
fn related(
    uia: &verbatim_uia::Uia,
    element: &IUIAutomationElement,
    window: isize,
    relation: Relation,
    fill: &Fill<Node>,
) -> Result<Found, Error> {
    let direction = match relation {
        Relation::Parent => Direction::Parent,
        Relation::FirstChild => Direction::FirstChild,
        Relation::LastChild => Direction::LastChild,
        Relation::NextSibling => Direction::NextSibling,
        Relation::PreviousSibling => Direction::PreviousSibling,
        Relation::ControllerFor => {
            let nodes = element
                .controller_for()?
                .iter()
                .map(|controlled| refresh(controlled, fill))
                .collect::<Result<_, _>>()?;
            return Ok(Found::Related(Related {
                nodes,
                window: None,
                table: None,
            }));
        }
        Relation::FirstSelectedItem => {
            let selected = uia.selected(&Live::new(element.clone()), fill.request())?;
            return Ok(Found::Related(Related {
                nodes: selected.into_iter().collect(),
                window: None,
                table: None,
            }));
        }
    };
    let window = if window == 0 {
        verbatim_uia::nearest_window_handle(element)
    } else {
        Some(window)
    };
    let walker = uia.raw_view_walker()?;
    // A step that fails because the element is gone is an error; any other
    // failure reads as no neighbour, as NVDA's tree-walker failures do.
    let neighbour = match Live::new(element.clone()).walk(&walker, direction, fill.request()) {
        Ok(neighbour) => Some(neighbour),
        Err(error) if verbatim_uia::element_is_gone(&error) => return Err(Error::Uia(error)),
        Err(_) => None,
    };
    let table = neighbour.as_ref().and_then(Cached::table);
    Ok(Found::Related(Related {
        nodes: neighbour.into_iter().collect(),
        window,
        table,
    }))
}

/// The children of `parent` in `view`, each filled by `fill`, and whether
/// they are all of them: one call for the raw view, one call a child for
/// the control view. A child gone ends the children there, those before
/// it kept, as a program's run ends them; in the raw view, where the one
/// call fails for it, they are read again one call a child to find them
/// (Claude, 2026-11-22).
pub(super) fn children(
    uia: &verbatim_uia::Uia,
    parent: &IUIAutomationElement,
    view: View,
    fill: &Fill<Node>,
) -> Result<(Vec<Cached<Node>>, bool), Error> {
    let parent = Live::new(parent.clone());
    let walker = if view == View::Raw {
        match uia.children(&parent, fill.request()) {
            Ok(children) => return Ok((children, true)),
            Err(error) if !verbatim_uia::element_is_gone(&error) => return Err(Error::Uia(error)),
            Err(_) => uia.raw_view_walker()?,
        }
    } else {
        uia.control_view_walker()?
    };
    let mut children = Vec::new();
    let mut next = parent.walk(&walker, Direction::FirstChild, fill.request());
    // The null element `windows` reports as an error with no failure code
    // ends the children.
    while let Ok(child) = next {
        next = child
            .live()
            .walk(&walker, Direction::NextSibling, fill.request());
        children.push(child);
    }
    match next {
        // The parent gone, or a child as the walk steps to it.
        Err(error) if verbatim_uia::element_is_gone(&error) => Ok((children, false)),
        Err(error) if error.code().is_err() => Err(Error::Uia(error)),
        _ => Ok((children, true)),
    }
}

/// `element` filled by `fill`, for a walk's start judged with its subtree.
pub(super) fn filled(
    element: &IUIAutomationElement,
    fill: &Fill<Node>,
) -> Result<Cached<Node>, Error> {
    refresh(element, fill)
}

/// The caret, the selection, and whether each is at the text's start or
/// end, as the program reads them: the selection (one call), its order
/// (one), a collapsed copy of the caret (two, or none when it is one), the
/// document range (one) and two comparisons for each point.
fn caret(text: TextTarget<'_>) -> Result<CaretFound, Error> {
    let state = classic_caret(text)?;
    let caret = if state.caret.collapsed {
        state.caret.range.clone()
    } else {
        collapsed_copy(&state.caret)?
    };
    let document = text.pattern.document_range()?;
    let edges = |range: &IUIAutomationTextRange, end: Endpoint| -> Result<Edges, Error> {
        Ok(Edges {
            start: range.compare_endpoints(end, &document, Endpoint::Start)? == 0,
            end: range.compare_endpoints(end, &document, Endpoint::End)? == 0,
        })
    };
    let selection = match state.selection {
        Some(selection) => {
            let start = edges(&selection, Endpoint::Start)?;
            let end = edges(&selection, Endpoint::End)?;
            Some((selection, start, end))
        }
        None => None,
    };
    Ok(CaretFound {
        edges: edges(&caret, Endpoint::Start)?,
        caret,
        selection,
    })
}

/// The point `at` names in `text`.
fn point(
    state: &State<'_, '_>,
    index: usize,
    text: TextTarget<'_>,
    at: &Point<'_>,
) -> Result<FoundPoint, Error> {
    match *at {
        Point::From(from) => classic_point(text, from, &mut None),
        Point::Of(place, which) => match (state.named(place, index), which) {
            (Some(Ran::Done(Found::Caret(caret))), _) => Ok(caret_point(caret, which)),
            (Some(Ran::Done(Found::ReadText(read))), PointOf::ChunkStart) => Ok(FoundPoint {
                range: read.units[0].range.clone(),
                endpoint: Endpoint::Start,
                collapsed: false,
            }),
            _ => Err(defect("a point names a step that holds none")),
        },
    }
}

/// The point `which` of a caret read.
pub(super) fn caret_point(caret: &CaretFound, which: PointOf) -> FoundPoint {
    match (&caret.selection, which) {
        (Some((selection, ..)), PointOf::SelectionStart | PointOf::SelectionEnd) => FoundPoint {
            range: selection.clone(),
            endpoint: if which == PointOf::SelectionEnd {
                Endpoint::End
            } else {
                Endpoint::Start
            },
            collapsed: false,
        },
        _ => FoundPoint::collapsed(caret.caret.clone()),
    }
}

/// A defect of the segment's, as an error that fails its step.
fn defect(what: &'static str) -> Error {
    let _ = super::Defect(what).ran();
    Error::Uia(windows::core::Error::from(
        windows::Win32::Foundation::E_INVALIDARG,
    ))
}

/// The range `ends` names in `text`.
fn range(
    state: &State<'_, '_>,
    index: usize,
    text: TextTarget<'_>,
    ends: &Ends<'_>,
) -> Result<IUIAutomationTextRange, Error> {
    let start = point(state, index, text, &ends.start)?;
    let end = point(state, index, text, &ends.end)?;
    let through = match &ends.through {
        Some((past_start, past_end)) => Some((
            point(state, index, text, past_start)?,
            point(state, index, text, past_end)?,
        )),
        None => None,
    };
    classic_range(
        &start,
        &end,
        through.as_ref().map(|(start, end)| (start, end)),
    )
}

/// A caret wait's check the classic way ([`crate::watch_check_classic`]).
fn wait_check(check: &super::WaitCheck<'_>) -> Result<Found, Error> {
    fn end<'p>(position: &crate::text::Position<'p>) -> RangeEnd<'p> {
        RangeEnd {
            range: position.range,
            endpoint: position.endpoint,
        }
    }
    let answer = crate::watch_check_classic(&crate::WatchQuery {
        element: check.text.element,
        pattern: check.text.pattern,
        start: end(&check.start),
        start_selection: (end(&check.start_selection.0), end(&check.start_selection.1)),
        word: check.word,
        max_text: check.max_text,
    })?;
    let reads_after = check.last || answer.caret_moved || answer.selection_moved;
    Ok(Found::WaitCheck(super::Checked {
        answer,
        reads_after,
    }))
}

/// The formatting of `unit` read the classic way, with what was learned of
/// the attributes' support: a character's one stretch, or the unit's walk.
fn formats(
    unit: &crate::text::UnitText,
    attributes: Attributes,
    learning: Attributes,
    kind: windows::Win32::UI::Accessibility::TextUnit,
    max_text: i32,
) -> Result<(Vec<crate::caret::Run>, Option<Attributes>), Error> {
    let ids: Vec<_> = attributes.iter().collect();
    if ids.is_empty() {
        return Ok((Vec::new(), None));
    }
    let character = kind == windows::Win32::UI::Accessibility::TextUnit_Character;
    let learns = !character && !unit.text.is_empty();
    let learning = if learns { learning } else { Attributes::NONE };
    let (runs, unsupported) = classic_runs(
        (&unit.range, unit.text.len()),
        character,
        &ids,
        learning,
        max_text,
    )?;
    Ok((runs, learns.then_some(unsupported)))
}

/// The selection a caret read found as its two ends, and whether it holds
/// nothing: the selection, or the caret twice.
fn ends_of(caret: &CaretFound) -> (IUIAutomationTextRange, Endpoint, bool) {
    match &caret.selection {
        Some((selection, ..)) => (selection.clone(), Endpoint::End, false),
        None => (caret.caret.clone(), Endpoint::Start, true),
    }
}

/// How the selection `after` read differs from `before`, by the text
/// protocol's rule, with the two texts compared to tell whether the text
/// changed.
fn compare(
    state: &State<'_, '_>,
    index: usize,
    before: &Prior<'_>,
    after: verbatim_model::batch::Place,
    max_text: i32,
) -> Result<Compared, Error> {
    let caret_of = |place| match state.named(place, index) {
        Some(Ran::Done(Found::Caret(caret))) => Ok(caret),
        _ => Err(defect("a comparison names no caret read")),
    };
    let (old, old_end, old_collapsed) = match *before {
        Prior::Baseline(position) => (
            position.range.clone(),
            position.endpoint,
            position.collapsed,
        ),
        Prior::SelectionOf(place) => ends_of(caret_of(place)?),
    };
    let after = caret_of(after)?;
    let (new, new_end, new_collapsed) = ends_of(after);
    let at = |range, endpoint| RangeEnd { range, endpoint };
    let changes = classic_changes(
        (
            (at(&old, Endpoint::Start), at(&old, old_end)),
            (at(&new, Endpoint::Start), at(&new, new_end)),
        ),
        (old_collapsed, new_collapsed),
        max_text,
    )?;
    let previous_text = if old_collapsed {
        None
    } else {
        let between = old.clone_range()?;
        between.move_endpoint_to(Endpoint::End, &old, old_end)?;
        Some(between.text(MAX_COMPARED_TEXT)?)
    };
    let selection_text = match &after.selection {
        Some((selection, ..)) => Some(selection.text(MAX_COMPARED_TEXT)?),
        None => None,
    };
    Ok(Compared {
        changes,
        selection_text,
        previous_text,
    })
}
