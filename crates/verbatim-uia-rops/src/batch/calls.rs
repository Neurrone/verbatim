//! The ordinary-calls executor: each fusable operation through the UI
//! Automation adapter, one call at a time, as the classic twins made the
//! calls. It answers a segment the setting or the window keeps from
//! programs, the steps no program can take, and the fallback's reruns.

use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};

use verbatim_model::batch::{
    Before, Edges, Missing, Op, Place, Point, PointOf, Relation, Subject, Through, View,
};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};
use verbatim_uia::{Cached, Direction, ElementExt, Fill, Live, Node};

use super::walk::{Reads, Walk};
use super::{
    Answer, CaretFound, Compared, Defect, Descend, Found, MAX_CHANGE_TEXT, MAX_RANGE_TEXT,
    MAX_UNIT_TEXT, Ran, Related, Segment, State, UnitsFound, Unrun, failed, named,
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
    verbatim_faults::hit(verbatim_faults::Fault::StepCallsFail, &state.place(index).0)
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

/// A call's failure as its step's outcome.
#[expect(
    clippy::needless_pass_by_value,
    reason = "an adapter for `map_err`, which hands the error over by value"
)]
fn call_failed(error: Error) -> Ran {
    failed(&error)
}

/// Runs the step at `index` of `state`'s segment by calls, its gate
/// already passed.
pub(super) fn run(state: &State<'_, '_>, index: usize) -> Ran {
    if injected_failure(state, index) {
        return failed(&Error::Uia(windows::Win32::Foundation::E_FAIL.into()));
    }
    match found(state, index) {
        Ok(found) => Ran::Done(found),
        Err(ran) => ran,
    }
}

/// The step at `index`'s result by calls, or its outcome.
fn found(state: &State<'_, '_>, index: usize) -> Result<Found, Ran> {
    let uia = state.segment.uia;
    let resolver = state.segment.resolver;
    let place = state.place(index);
    let no_fill = || Defect("an object step the resolver gives no fill").ran();
    match &state.step(index).op {
        Op::Related {
            subject, relation, ..
        } => {
            let fill = resolver.fill(place).ok_or_else(no_fill)?;
            let (element, window) = one(state, index, subject)?;
            related(uia, &element, window, *relation, fill).map_err(call_failed)
        }
        Op::Descendants { subject, walk } => {
            let walk = Descend::of(walk, resolver, place).ok_or_else(no_fill)?;
            let mut walking = Walk::new(&walk, starts(state, index, subject)?);
            walking.run(uia, Reads::Calls).map_err(call_failed)?;
            Ok(Found::Descendants(walking.finish()))
        }
        Op::Caret { subject, .. } => caret(resolver.text(subject)?)
            .map(Found::Caret)
            .map_err(call_failed),
        Op::ReadText {
            subject,
            at,
            movement,
            unit,
            count,
            details,
        } => {
            let text = resolver.text(subject)?;
            let read = resolver.units((*movement, *unit, *count), *details)?;
            let from = point(state, index, text, at)?;
            let answer = classic_units(
                text,
                from,
                (read.movement, read.unit, read.count),
                (MAX_UNIT_TEXT, read.max_total, read.culture),
            )
            .map_err(call_failed)?;
            let (runs, unsupported) =
                formats(&answer.units[0], read.formats, read.learning, read.unit)
                    .map_err(call_failed)?;
            Ok(Found::ReadText(UnitsFound {
                moved: answer.moved,
                units: answer.units,
                ended: answer.ended,
                runs,
                unsupported,
            }))
        }
        Op::ReadRange {
            subject,
            start,
            end,
            through,
        } => {
            let range = range(state, index, resolver.text(subject)?, (start, end, through))?;
            Ok(Found::ReadRange(
                range
                    .text(MAX_RANGE_TEXT)
                    .map_err(|error| failed(&error.into()))?,
            ))
        }
        Op::Select {
            subject,
            start,
            end,
            through,
        } => {
            let range = range(state, index, resolver.text(subject)?, (start, end, through))?;
            // A refused selection is the step's refusal, as the program's
            // catch answers it.
            match range.select() {
                Ok(()) => Ok(Found::Select),
                Err(_) => Err(Ran::Unrun(Unrun::Refused(
                    verbatim_model::batch::StepRefusal::Unsupported,
                ))),
            }
        }
        Op::CompareSelection { before, after, .. } => {
            compare(state, index, before, *after).map(Found::CompareSelection)
        }
        Op::Locate { subject, at } => {
            let at = point(state, index, resolver.text(subject)?, at)?;
            classic_location(&at)
                .map(Found::Locate)
                .map_err(call_failed)
        }
        Op::WaitForCaret { .. } => wait_check(&resolver.wait_check()?).map_err(call_failed),
        // UI Automation has no sentence unit: once the subject's text is
        // found, the move is refused, as NVDA's UIA text has none.
        Op::MoveBySentence { subject, .. } => {
            resolver.text(subject)?;
            Err(Ran::Unrun(Unrun::Refused(
                verbatim_model::batch::StepRefusal::NoSuchUnit,
            )))
        }
        op @ (Op::Read { .. }
        | Op::Test { .. }
        | Op::IsOneOf { .. }
        | Op::Activate { .. }
        | Op::Follow(_)) => super::object::found(state.segment, place, op),
        _ => Err(Defect("a step no executor of fusable steps runs").ran()),
    }
}

/// The one element `subject` names, with its own window, 0 when not known;
/// the step's outcome when it names none.
fn one(
    state: &State<'_, '_>,
    index: usize,
    subject: &Subject,
) -> Result<(IUIAutomationElement, isize), Ran> {
    match *subject {
        Subject::Result(place) if !state.before(place) => {
            Err(Defect("a step names a list as its one subject").ran())
        }
        Subject::Item { of, index: item } if !state.before(of) => {
            match list(state, of, index)?.get(item as usize) {
                Some(node) => Ok(node_of(node)),
                None => Err(Ran::Unrun(Unrun::NotThere(Missing::Object))),
            }
        }
        ref subject => {
            let held = state.segment.resolver.object(subject)?;
            Ok((held.element.clone(), held.window))
        }
    }
}

/// A node's element and its own window.
fn node_of(node: &Cached<Node>) -> (IUIAutomationElement, isize) {
    (node.live().element().clone(), node.read().role.hwnd)
}

/// The nodes of the list an earlier step of the segment answered.
fn list<'r>(
    state: &'r State<'_, '_>,
    place: Place,
    index: usize,
) -> Result<Vec<&'r Cached<Node>>, Ran> {
    match state.named(place, index) {
        Some(Ran::Done(Found::Related(related))) => Ok(related.nodes.iter().collect()),
        Some(Ran::Done(Found::Descendants(descendants))) => Ok(descendants
            .nodes
            .iter()
            .map(|descendant| &descendant.node)
            .collect()),
        _ => Err(Defect("a step names no list of elements").ran()),
    }
}

/// Where a walk down the tree starts: its subject, or every node of an
/// earlier list.
fn starts(
    state: &State<'_, '_>,
    index: usize,
    subject: &Subject,
) -> Result<Vec<IUIAutomationElement>, Ran> {
    match *subject {
        Subject::Result(place) if !state.before(place) => Ok(list(state, place, index)?
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
    at: &Point,
) -> Result<FoundPoint, Ran> {
    if let Some(from) = named(at) {
        return classic_point(text, from, &mut None).map_err(call_failed);
    }
    match *at {
        Point::Of(place, which) if !state.before(place) => {
            match (state.named(place, index), which) {
                (Some(Ran::Done(Found::Caret(caret))), _) => Ok(caret_point(caret, which)),
                (Some(Ran::Done(Found::ReadText(read))), PointOf::ChunkStart) => Ok(FoundPoint {
                    range: read.units[0].range.clone(),
                    endpoint: Endpoint::Start,
                    collapsed: false,
                }),
                _ => Err(Defect("a point names a step that holds none").ran()),
            }
        }
        _ => {
            let held = state.segment.resolver.point(at)?;
            classic_point(text, held.from(), &mut None).map_err(call_failed)
        }
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

/// The range `ends` names in `text`.
fn range(
    state: &State<'_, '_>,
    index: usize,
    text: TextTarget<'_>,
    (start, end, through): (&Point, &Point, &Option<Through>),
) -> Result<IUIAutomationTextRange, Ran> {
    let start = point(state, index, text, start)?;
    let end = point(state, index, text, end)?;
    let through = match through {
        Some(through) => Some((
            point(state, index, text, &through.start)?,
            point(state, index, text, &through.end)?,
        )),
        None => None,
    };
    classic_range(
        &start,
        &end,
        through.as_ref().map(|(start, end)| (start, end)),
    )
    .map_err(call_failed)
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
        max_text: MAX_UNIT_TEXT,
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
        MAX_UNIT_TEXT,
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
    before: &Before,
    after: Place,
) -> Result<Compared, Ran> {
    let caret_of = |place| match state.named(place, index) {
        Some(Ran::Done(Found::Caret(caret))) => Ok(caret),
        _ => Err(Defect("a comparison names no caret read").ran()),
    };
    let (old, old_end, old_collapsed) = match *before {
        Before::SelectionOf(place) if !state.before(place) => ends_of(caret_of(place)?),
        ref before => {
            let held = state.segment.resolver.baseline(before)?;
            (held.range, held.endpoint, held.collapsed)
        }
    };
    let after = caret_of(after)?;
    let (new, new_end, new_collapsed) = ends_of(after);
    compared(
        (old, old_end, old_collapsed),
        (after, new, new_end, new_collapsed),
    )
    .map_err(call_failed)
}

/// The selection `after` read, whose ends are `new` to `new_end`, compared
/// with the one from `old` to `old_end`, each collapsed or not.
fn compared(
    (old, old_end, old_collapsed): (IUIAutomationTextRange, Endpoint, bool),
    (after, new, new_end, new_collapsed): (&CaretFound, IUIAutomationTextRange, Endpoint, bool),
) -> Result<Compared, Error> {
    let at = |range, endpoint| RangeEnd { range, endpoint };
    let changes = classic_changes(
        (
            (at(&old, Endpoint::Start), at(&old, old_end)),
            (at(&new, Endpoint::Start), at(&new, new_end)),
        ),
        (old_collapsed, new_collapsed),
        MAX_CHANGE_TEXT,
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
