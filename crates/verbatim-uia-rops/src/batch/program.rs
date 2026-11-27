//! The program builder: a run of a segment's steps lowered to a shape
//! that holds no object of UI Automation's ([`Low`]), each object an import
//! by its place, so that a run of the same shape binds a kept program to
//! its own imports ([`ProgramCache`]); the shape emitted, each step in a
//! try block with a status of its own; and its answer read back.

use std::ops::Range;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextRange, TextUnit_Character, UIA_ControllerForPropertyId,
};

use verbatim_model::TextPoint;
use verbatim_model::batch::{
    Before as Prior, Check, Gate as Decided, Known, Op, Outcomes, Pending, Place, Point, PointOf,
    Relation, StepRefusal, Subject, Test, View,
};
use verbatim_uia::text::Endpoint;
use verbatim_uia::{Cached, Fill, Node};

use super::calls::{FAR, caret_point};
use super::walk::{Reads, RunRegisters, Walk, emit_run};
use super::{
    CaretFound, Compared, Cut, Defect, Descend, Found, MAX_CHANGE_TEXT, MAX_RANGE_TEXT,
    MAX_UNIT_TEXT, MoveFound, Ran, Related, State, TextMatch, UnitsFound, UnitsRead, Unrun,
    VisibleFound, failed, named,
};
use crate::builder::{Builder, Reg, kind};
use crate::caret::{
    CaretRegisters, Constants, MAX_COMPARED_TEXT, ProgramEnd, RunRegisters as FormatRegisters,
    emit_caret, emit_changes, emit_runs, endpoint_number, gone_or_timed_out, read_changes,
    string_of,
};
use crate::error::Error;
use crate::focus::{
    RemoteCache, absent_record, emit_first_selected, read_table, reads_cell, table_answered,
};
use crate::instruction::TypeTest;
use crate::navigate::emit_nearest_window;
use crate::opcode::{Comparison, NavigationDirection, Status as RunStatus};
use crate::operation::{Import, Outcome, ProgramCache, Value};
use crate::text::{
    FoundPoint, Movement, TextFrom, UnitsRegisters, emit_after, emit_between, emit_units,
};
use crate::watch::emit_check;

/// The most instructions one run may execute.
pub(super) const LIMIT: u32 = 10_000;

/// A step's status when it is done.
const DONE: i32 = 0;
/// A step's status when it was refused because a point Core holds lies
/// past the text's end, or the provider would not compare it with the
/// text ([`StepRefusal::OutsideText`]): a move's.
const REFUSED_OUTSIDE: i32 = -1;
/// A step's status when the provider refused it
/// ([`StepRefusal::Unsupported`]): a selection's.
const REFUSED_UNSUPPORTED: i32 = -2;
/// A step's status when its gate did not hold. The statuses from it to
/// [`DONE`] are the endings whose outcome the program knows: done,
/// refused or skipped.
const SKIPPED: i32 = -3;
/// A step's status before it runs: a run that ends early leaves it so.
const NOT_REACHED: i32 = -4;
/// A step's status when its list was longer than a program reads: it runs
/// by calls, a size-limit path and no fallback.
const CUT: i32 = -5;

/// The most controller-for elements a program reads; a longer relation is
/// read by calls.
const CONTROLLED_PER_RUN: u32 = 16;

/// The instructions every program spends outside its steps.
pub(super) const PROLOGUE: u32 = 60;

/// Text no terminal's buffer holds: control characters, which a terminal
/// acts on rather than writes, so that a search for it answers what the
/// provider answers for text that is nowhere ([`Emit::find_text`]).
const ABSENT: &str = "\u{1}\u{2}\u{3}";

/// What a `FindText` may execute at most outside its calls: its span, the
/// search of the whole text and the comparison with the answer for
/// [`ABSENT`], beside finding its point.
const FIND_SETUP: u32 = 60;
/// What one `FindText` call whose hit is passed over may execute at most.
const FIND_CALL: u32 = 20;
/// What taking a match may execute at most beyond its call.
const FIND_MATCH: u32 = 8;

/// What the step at `index` may execute at most, from its caps, after
/// `prior` instructions of its program, pinned by
/// `crates/mockapp/tests/instruction_limit.rs`; nothing for a step that
/// cannot be lowered, which the program does not emit.
fn worst(state: &State<'_, '_>, index: usize, prior: u32) -> u32 {
    let step = &state.step(index);
    let compared = match step.when.map(|condition| condition.test) {
        Some(Test::NotAfter { point, than }) => {
            COMPARISON + point_cost(state, &point) + point_cost(state, &than)
        }
        _ => 0,
    };
    match worst_of_op(state, index, prior) {
        // A step the program does not emit has no gate either.
        0 => 0,
        op => op + compared,
    }
}

/// What a condition's comparison of two points may execute at most beside
/// finding them: the tests of the steps they name, the comparison and its
/// try block.
const COMPARISON: u32 = 40;

/// What the operation of the step at `index` may execute at most.
fn worst_of_op(state: &State<'_, '_>, index: usize, prior: u32) -> u32 {
    let resolver = state.segment.resolver;
    let place = state.place(index);
    match &state.step(index).op {
        Op::Related { relation, .. } => {
            let Some(fill) = resolver.fill(place) else {
                return 0;
            };
            let (setup, each) = RemoteCache::costs(fill);
            match relation {
                Relation::ControllerFor => 30 + setup + CONTROLLED_PER_RUN * (8 + each),
                // The nearest window's walk, at most the tree's depth, and a
                // cell's table.
                _ => 80 + 8 * verbatim_model::batch::MAX_DEPTH + setup + each,
            }
        }
        Op::Descendants { walk, .. } => match Descend::of(walk, resolver, place) {
            Some(walk) => {
                let starts = starts_cap(state, index);
                super::walk::run_cost(&walk, starts, room(&walk, starts, prior))
            }
            None => 0,
        },
        Op::Caret { .. } | Op::Visible { .. } => 60,
        Op::ReadText {
            at, whole: true, ..
        } => 40 + point_cost(state, at),
        Op::Move { at, .. } => 60 + point_cost(state, at),
        Op::ReadText {
            at,
            movement,
            unit,
            count,
            details,
            ..
        } => match resolver.units((*movement, *unit, *count), *details) {
            Ok(read) => {
                let formats = if read.formats.is_empty() { 0 } else { 7_700 };
                80 + point_cost(state, at) + 24 * read.count + formats
            }
            Err(_) => 0,
        },
        Op::ReadRange {
            start,
            end,
            through,
            ..
        }
        | Op::Select {
            start,
            end,
            through,
            ..
        } => {
            40 + point_cost(state, start)
                + point_cost(state, end)
                + through.map_or(0, |through| {
                    point_cost(state, &through.start) + point_cost(state, &through.end)
                })
        }
        Op::CompareSelection { .. } => 120,
        Op::Locate { at, .. } => 20 + point_cost(state, at),
        Op::FindText {
            at, limit, calls, ..
        } => {
            FIND_SETUP
                + point_cost(state, at)
                + FIND_CALL * u32::from(*calls)
                + FIND_MATCH * u32::from(*limit.min(calls))
        }
        Op::WaitForCaret { .. } => WAIT_CHECK,
        Op::ReadColours { subject, .. } if fused_size(state, subject).is_some() => {
            super::colours::FUSED_COST
        }
        _ => 0,
    }
}

/// The rows and the width a program reads the colours of `subject`'s text
/// at: the size its last colour read found, kept with the text, when a
/// program may read colours at it.
fn fused_size(state: &State<'_, '_>, subject: &Subject) -> Option<(u32, u32)> {
    state
        .segment
        .resolver
        .colour_size(subject)
        .filter(|&size| super::colours::fusable(size))
}

/// Whether a program of the steps from `start` reads the colours of the
/// step at `index` itself: a size is kept to read them at, and the screen
/// they are read from is a step of the program. Any other colour read
/// runs the colour programs alone.
fn fuses_colours(state: &State<'_, '_>, start: usize, index: usize) -> bool {
    match &state.step(index).op {
        Op::ReadColours {
            subject, screen, ..
        } => {
            fused_size(state, subject).is_some()
                && state.index(*screen, index).is_some_and(|at| {
                    at >= start && matches!(state.step(at).op, Op::Visible { .. })
                })
        }
        _ => false,
    }
}

/// What a caret wait's check may execute at most: its caret read twice,
/// three comparisons with the starting point, its line and word, and its
/// gate's test.
const WAIT_CHECK: u32 = 120;

/// What finding `point` may execute at most: a held position's text after
/// it is found by trying each count.
fn point_cost(state: &State<'_, '_>, point: &Point) -> u32 {
    let held =
        named(point).is_none() && !matches!(point, Point::Of(place, _) if !state.before(*place));
    match held.then(|| state.segment.resolver.point(point)) {
        Some(Ok(held)) if !held.prefix.is_empty() => {
            20 + 12 * u32::try_from(held.counts.len()).unwrap_or(u32::MAX / 24)
        }
        _ => 40,
    }
}

/// The most starts the walk at `index` may have: one node, an earlier
/// list's nodes, or a relation of the program's, at most what a program
/// reads of one.
fn starts_cap(state: &State<'_, '_>, index: usize) -> u32 {
    match state.step(index).op.subject() {
        Some(Subject::Result(place)) if !state.before(*place) => match state.named(*place, index) {
            Some(Ran::Done(found)) => {
                nodes_of(found).map_or(1, |nodes| u32::try_from(nodes.len()).unwrap_or(u32::MAX))
            }
            _ => CONTROLLED_PER_RUN,
        },
        _ => 1,
    }
}

/// How many children a walk's first run reads after `prior` instructions
/// of its program: as many as the rest of the limit holds, at most
/// [`CHILDREN_PER_RUN`] and the walk's node cap.
fn room(walk: &Descend<'_>, starts: u32, prior: u32) -> u32 {
    super::walk::run_room(walk, starts, LIMIT.saturating_sub(prior))
}

/// Whether the step at `index` runs by calls whatever the segment's mode:
/// its subject is one no program takes, or it walks the control view.
fn by_calls(state: &State<'_, '_>, index: usize) -> bool {
    let op = &state.step(index).op;
    let calls_only = |subject: &Subject| match subject {
        Subject::Result(place) | Subject::Item { of: place, .. } if !state.before(*place) => {
            match state.named(*place, index) {
                Some(Ran::Done(Found::Related(related))) => related
                    .nodes
                    .iter()
                    .any(|node| node.served_only_by_uia_proxies() == Some(true)),
                _ => false,
            }
        }
        subject => state
            .segment
            .resolver
            .object(subject)
            .is_ok_and(|element| element.calls_only),
    };
    match op {
        Op::Descendants { walk, subject } => walk.view == View::Control || calls_only(subject),
        Op::Related { subject, .. } => calls_only(subject),
        // The operations with call arms only.
        Op::Read { .. }
        | Op::Test { .. }
        | Op::IsOneOf { .. }
        | Op::Activate { .. }
        | Op::Follow(_)
        | Op::Ancestors { .. }
        | Op::MoveBySentence { .. }
        // The colours run programs of their own unless a program that
        // read their screen reads them (`fuses_colours`).
        | Op::ReadColours { .. } => true,
        _ => false,
    }
}

/// Whether a program holding the steps from `start` cannot take the step
/// at `index`: it names a node of an earlier list of the program by its
/// place, which a program would have to check against the list's length.
fn names_item_inside(state: &State<'_, '_>, start: usize, index: usize) -> bool {
    match &state.step(index).op {
        Op::Related { subject, .. } | Op::Descendants { subject, .. } => matches!(
            subject,
            Subject::Item { of, .. } if state.index(*of, index).is_some_and(|at| at >= start)
        ),
        _ => false,
    }
}

/// What runs next from step `at`: a program of the steps from it, or one
/// step by calls.
pub(super) fn cut(state: &State<'_, '_>, at: usize) -> Cut {
    if by_calls(state, at) {
        return Cut::Calls(at);
    }
    let steps = state.segment.steps;
    let mut end = at;
    let mut cost = PROLOGUE;
    while end < steps.len() {
        let op = &steps[end].op;
        let step = worst(state, end, cost);
        let walk_fits = match op {
            Op::Descendants { walk, .. } => {
                Descend::of(walk, state.segment.resolver, state.place(end))
                    .is_none_or(|walk| room(&walk, starts_cap(state, end), cost) > 0)
            }
            _ => true,
        };
        if end > at
            && ((by_calls(state, end) && !fuses_colours(state, at, end))
                || names_item_inside(state, at, end)
                || !walk_fits
                || cost + step > LIMIT)
        {
            break;
        }
        cost += step;
        end += 1;
        if matches!(op, Op::Select { .. } | Op::Descendants { .. }) {
            break;
        }
    }
    Cut::Program(at..end)
}

/// A node's source in a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Src {
    /// An import.
    Import(usize),
    /// Several imports, a walk's starts.
    Imports(Vec<usize>),
    /// The result of the program's step at this place: its node, or a
    /// walk's starts, its list.
    Here(usize),
}

/// A text point in a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Pt {
    Caret,
    SelectionStart,
    SelectionEnd,
    Start,
    End,
    /// An imported range's end, and whether the range is collapsed.
    At(usize, bool, bool),
    /// [`TextFrom::After`]'s, from an imported range's end.
    After {
        from: (usize, bool, bool),
        prefix: Vec<u16>,
        counts: Vec<i32>,
    },
    /// A point of the program's step at this place.
    Here(usize, PointOf),
}

/// The selection a comparison starts from, in a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Before {
    /// An imported range's start to its end or start, and whether it held
    /// nothing.
    Held(usize, bool, bool),
    /// The program's caret read at this place.
    Here(usize),
}

/// The caret read a comparison judges, in a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum After {
    /// An earlier program's: its imported caret, and its selection.
    Held(usize, Option<usize>),
    /// The program's caret read at this place.
    Here(usize),
}

/// A range's points in a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LowEnds {
    start: Pt,
    end: Pt,
    through: Option<(Pt, Pt)>,
}

/// A fill's shape: its properties and patterns.
type FillKey = (Vec<i32>, Vec<i32>);

fn fill_key(fill: &Fill<Node>) -> FillKey {
    (
        fill.properties().iter().map(|id| id.0).collect(),
        fill.patterns().iter().map(|id| id.0).collect(),
    )
}

/// One step's shape: everything its emitter emits from but its imports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Low {
    /// Answered before the program, not emitted.
    Unemitted,
    Related(Src, isize, Relation, FillKey),
    Descendants(Src, (u32, bool, bool, u32), FillKey),
    Caret(usize),
    ReadText(usize, Pt, [i32; 8]),
    ReadRange(usize, LowEnds, i32),
    Compare(usize, Before, After, i32),
    Select(usize, LowEnds),
    Locate(usize, Pt),
    /// The element, the starting caret's and selection's ends as imports
    /// and whether each is its range's end, whether the word is read,
    /// whether it is the check at the bound, and the text read's limit.
    WaitCheck(usize, [(usize, bool); 3], bool, bool, i32),
    Visible(usize),
    /// The element, the point, the unit and count, and whether the point
    /// is one Core holds, checked against the text's end.
    Move(usize, Pt, (i32, i32), bool),
    /// The element, the point, the unit and count.
    Whole(usize, Pt, (i32, u16)),
    /// The program's place of the screen's `Visible`, and the rows and the
    /// width the colours are read at.
    Colours(usize, (u32, u32)),
    /// The element and the point; the text sought and whether toward the
    /// text's start; the unit, whether only a match that starts one is
    /// taken, and the units read from each; the most matches and calls.
    FindText((usize, Pt), (String, bool), (i32, bool, u8), (u8, u8)),
}

impl Low {
    /// The import of the element whose text the step reads through its
    /// pattern or its document range.
    const fn text(&self) -> Option<usize> {
        match self {
            Self::Caret(text)
            | Self::ReadText(text, ..)
            | Self::ReadRange(text, ..)
            | Self::Compare(text, ..)
            | Self::Select(text, ..)
            | Self::Locate(text, ..)
            | Self::Visible(text)
            | Self::Move(text, ..)
            | Self::Whole(text, ..)
            | Self::FindText((text, _), ..) => Some(*text),
            Self::Unemitted
            | Self::Related(..)
            | Self::Descendants(..)
            | Self::WaitCheck(..)
            | Self::Colours(..) => None,
        }
    }
}

/// What a lowered step's emitter and its answer take beyond its shape:
/// the fill, the walk or the unit read the resolver gave it.
#[derive(Clone, Copy)]
enum Given<'a> {
    Nothing,
    Fill(&'a Fill<Node>),
    Walk(Descend<'a>),
    Read(UnitsRead),
    /// The rows and the width a colour read's rows are read at.
    Colours((u32, u32)),
}

/// What is left of a step's gate to test in its program: the model's
/// rule's tests of the program's steps it names
/// ([`verbatim_model::batch::Pending`], by place in the program), and the
/// comparison of two points it makes, lowered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Gate {
    /// The tests of the program's steps.
    pending: Pending<usize>,
    /// Its comparison of two points.
    compare: Option<Compare>,
}

impl Gate {
    /// The gate of a step under no condition, naming no step.
    const OPEN: Self = Self {
        pending: Pending::none(),
        compare: None,
    };
}

/// A condition's comparison of two points in a program
/// (`verbatim_model::batch::Gate::Compare`), made once the program's steps
/// it names ([`Pending::compared`]) are done.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Compare {
    /// The text's element, imported, with the point and the point it is
    /// compared with.
    points: (usize, Pt, Pt),
    /// Whether the step runs when the test is false.
    negated: bool,
}

/// A program's shape, its cache's key.
type Key = Vec<(Low, Gate)>;

/// A program planned: its shape, its imports in order, what each step was
/// given, and each step's outcome decided before it ran.
struct Plan<'a> {
    key: Key,
    imports: Vec<Import>,
    given: Vec<Given<'a>>,
    pre: Vec<Option<Ran>>,
}

thread_local! {
    /// The programs this thread built, by their shape.
    static PROGRAMS: ProgramCache<Key, Vec<Option<Emitted>>> = ProgramCache::default();
}

struct Planner<'p, 's, 'a> {
    state: &'p State<'s, 'a>,
    range: Range<usize>,
    imports: Vec<Import>,
    /// The outcome of each step of the program planned so far that was
    /// decided here, before the program runs; `None` for a step lowered.
    pre: Vec<Option<Ran>>,
    /// The text elements imported so far, by the subject that names each:
    /// every step on a subject takes the one import, since UI Automation
    /// finds each imported element again in the application, five calls
    /// to its provider.
    texts: Vec<(Subject, usize)>,
    /// The positions the outpost holds imported so far, by the point that
    /// names each.
    held_points: Vec<(Point, Pt)>,
}

impl<'p, 'a> Planner<'p, '_, 'a> {
    fn import(&mut self, import: Import) -> usize {
        self.imports.push(import);
        self.imports.len() - 1
    }

    /// The program's place of the batch's `place`, named by the step at
    /// `by`, when it is in the program.
    fn here(&self, place: Place, by: usize) -> Option<usize> {
        self.state
            .index(place, by)
            .filter(|index| self.range.contains(index))
            .map(|index| index - self.range.start)
    }

    fn done(&self, place: Place, by: usize) -> Result<&'p Found, Ran> {
        let state: &'p State<'_, '_> = self.state;
        match state.named(place, by) {
            Some(Ran::Done(found)) => Ok(found),
            Some(_) => Err(Ran::Unrun(Unrun::Skipped)),
            None => Err(Defect("a step names one not before it").ran()),
        }
    }

    fn src(&mut self, subject: &Subject, by: usize, many: bool) -> Result<(Src, isize), Ran> {
        match *subject {
            Subject::Result(place) if !self.state.before(place) => {
                if let Some(here) = self.here(place, by) {
                    return Ok((Src::Here(here), 0));
                }
                let found = self.done(place, by)?;
                if !many {
                    return Err(Defect("a step names a list as its one subject").ran());
                }
                let nodes =
                    nodes_of(found).ok_or_else(|| Defect("a walk starts from no list").ran())?;
                let imports = nodes
                    .into_iter()
                    .map(|node| self.import(Import::Element(node.live().element().clone())))
                    .collect();
                Ok((Src::Imports(imports), 0))
            }
            Subject::Item { of, index } if !self.state.before(of) => {
                let found = self.done(of, by)?;
                let node = nodes_of(found)
                    .ok_or_else(|| Defect("an item of no list").ran())?
                    .get(index as usize)
                    .copied()
                    .ok_or(Ran::Unrun(Unrun::NotThere(
                        verbatim_model::batch::Missing::Object,
                    )))?;
                let window = node.read().role.hwnd;
                let element = node.live().element().clone();
                Ok((Src::Import(self.import(Import::Element(element))), window))
            }
            ref subject => {
                let held = self.state.segment.resolver.object(subject)?;
                Ok((
                    Src::Import(self.import(Import::Element(held.element.clone()))),
                    held.window,
                ))
            }
        }
    }

    fn held(
        &mut self,
        range: &IUIAutomationTextRange,
        end: bool,
        collapsed: bool,
    ) -> (usize, bool, bool) {
        (
            self.import(Import::TextRange(range.clone())),
            end,
            collapsed,
        )
    }

    fn found_point(&mut self, point: &FoundPoint) -> (usize, bool, bool) {
        self.held(
            &point.range,
            point.endpoint == Endpoint::End,
            point.collapsed,
        )
    }

    fn point(&mut self, point: &Point, by: usize) -> Result<Pt, Ran> {
        match named(point) {
            Some(TextFrom::Caret) => return Ok(Pt::Caret),
            Some(TextFrom::SelectionStart) => return Ok(Pt::SelectionStart),
            Some(TextFrom::SelectionEnd) => return Ok(Pt::SelectionEnd),
            Some(TextFrom::Start) => return Ok(Pt::Start),
            Some(TextFrom::End) => return Ok(Pt::End),
            Some(TextFrom::At(_) | TextFrom::After { .. }) | None => {}
        }
        if let Point::Of(place, which) = *point
            && !self.state.before(place)
        {
            if let Some(here) = self.here(place, by) {
                return Ok(Pt::Here(here, which));
            }
            return match (self.done(place, by)?, which) {
                (Found::Caret(caret), _) => {
                    let (range, end, collapsed) = self.found_point(&caret_point(caret, which));
                    Ok(Pt::At(range, end, collapsed))
                }
                (Found::ReadText(read), PointOf::ChunkStart) => {
                    let range = read.units[0].range.clone();
                    Ok(Pt::At(self.import(Import::TextRange(range)), false, false))
                }
                (Found::Visible(visible), PointOf::VisibleStart | PointOf::VisibleEnd) => {
                    let range = self.import(Import::TextRange(visible.range.clone()));
                    Ok(Pt::At(range, which == PointOf::VisibleEnd, false))
                }
                (Found::Move(moved), PointOf::Moved) => {
                    let range = self.import(Import::TextRange(moved.to.clone()));
                    Ok(Pt::At(range, false, true))
                }
                _ => Err(Defect("a point of no text read").ran()),
            };
        }
        if let Some((_, imported)) = self.held_points.iter().find(|(named, _)| named == point) {
            return Ok(imported.clone());
        }
        let held = self.state.segment.resolver.point(point)?;
        let from = self.found_point(&held.at);
        let imported = if held.prefix.is_empty() {
            Pt::At(from.0, from.1, from.2)
        } else {
            Pt::After {
                from,
                prefix: held.prefix,
                counts: held.counts,
            }
        };
        self.held_points.push((*point, imported.clone()));
        Ok(imported)
    }

    fn ends(
        &mut self,
        (start, end, through): (&Point, &Point, Option<&verbatim_model::batch::Through>),
        by: usize,
    ) -> Result<LowEnds, Ran> {
        Ok(LowEnds {
            start: self.point(start, by)?,
            end: self.point(end, by)?,
            through: match through {
                Some(through) => Some((
                    self.point(&through.start, by)?,
                    self.point(&through.end, by)?,
                )),
                None => None,
            },
        })
    }

    fn caret_of(&self, place: Place, by: usize) -> Result<&'p CaretFound, Ran> {
        match self.done(place, by)? {
            Found::Caret(caret) => Ok(caret),
            _ => Err(Defect("a comparison names no caret read").ran()),
        }
    }

    /// A comparison's two selections, by the program's registers or as
    /// imports.
    fn compared(
        &mut self,
        before: &Prior,
        after: Place,
        index: usize,
    ) -> Result<(Before, After), Ran> {
        let before = match *before {
            Prior::SelectionOf(place) if !self.state.before(place) => {
                if let Some(here) = self.here(place, index) {
                    Before::Here(here)
                } else {
                    let caret = self.caret_of(place, index)?;
                    let (range, end, collapsed) = match &caret.selection {
                        Some((selection, ..)) => (selection.clone(), true, false),
                        None => (caret.caret.clone(), false, true),
                    };
                    let (range, ..) = self.held(&range, end, collapsed);
                    Before::Held(range, end, collapsed)
                }
            }
            ref before => {
                let held = self.state.segment.resolver.baseline(before)?;
                let (range, end, collapsed) = self.found_point(&held);
                Before::Held(range, end, collapsed)
            }
        };
        let after = if let Some(here) = self.here(after, index) {
            After::Here(here)
        } else {
            let caret = self.caret_of(after, index)?;
            let selection = caret
                .selection
                .as_ref()
                .map(|(selection, ..)| selection.clone());
            let point = self.import(Import::TextRange(caret.caret.clone()));
            let selection = selection.map(|selection| self.import(Import::TextRange(selection)));
            After::Held(point, selection)
        };
        Ok((before, after))
    }

    /// A caret wait's check lowered: its element and starting points as
    /// imports.
    fn wait_check(&mut self) -> Result<Low, Ran> {
        let check = self.state.segment.resolver.wait_check()?;
        let element = self.import(Import::Element(check.text.element.clone()));
        let mut held = |position: &crate::text::Position<'_>| {
            let (range, end, _) = self.held(
                position.range,
                position.endpoint == Endpoint::End,
                position.collapsed,
            );
            (range, end)
        };
        let points = [
            held(&check.start),
            held(&check.start_selection.0),
            held(&check.start_selection.1),
        ];
        Ok(Low::WaitCheck(
            element,
            points,
            check.word,
            check.last,
            MAX_UNIT_TEXT,
        ))
    }

    /// The text `subject` names, imported once for the program.
    fn text(&mut self, subject: &Subject) -> Result<usize, Ran> {
        if let Some((_, import)) = self.texts.iter().find(|(named, _)| named == subject) {
            return Ok(*import);
        }
        let target = self.state.segment.resolver.text(subject)?;
        let import = self.import(Import::Element(target.element.clone()));
        self.texts.push((subject.clone(), import));
        Ok(import)
    }

    /// The gate of the step at `index` inside the program, by the model's
    /// one rule ([`verbatim_model::batch::Step::judge`]); its outcome when
    /// the steps it names decide it before the program runs.
    fn gate(&mut self, index: usize) -> Result<Gate, Ran> {
        let step = self.state.step(index);
        let (gate, pending) = step
            .judge(&Planned {
                planner: self,
                by: index,
            })
            .map_err(Defect::ran)?;
        if let Some((_, Check::Matched, _)) = pending.step {
            return Err(Defect("a condition in a program tests a match").ran());
        }
        let compare = match gate {
            Decided::Run => None,
            Decided::Skip => return Err(Ran::Unrun(Unrun::Skipped)),
            Decided::Compare {
                point,
                than,
                negated,
            } => {
                let Some(subject) = step.op.subject() else {
                    return Err(Defect("a comparison on a step of no subject").ran());
                };
                let text = self.text(subject)?;
                let points = (text, self.point(&point, index)?, self.point(&than, index)?);
                Some(Compare { points, negated })
            }
        };
        Ok(Gate { pending, compare })
    }

    /// The step at `index` lowered, with its gate inside the program and
    /// what it was given; its outcome when the program does not run it.
    #[expect(clippy::too_many_lines, reason = "one arm per operation")]
    fn lower(&mut self, index: usize) -> Result<(Low, Gate, Given<'a>), Ran> {
        let state: &'p State<'_, 'a> = self.state;
        let step = state.step(index);
        let resolver = state.segment.resolver;
        let place = state.place(index);
        let gate = self.gate(index)?;
        let no_fill = || Defect("an object step the resolver gives no fill").ran();
        let (low, given) = match &step.op {
            Op::Related {
                subject, relation, ..
            } => {
                let fill = resolver.fill(place).ok_or_else(no_fill)?;
                let (src, window) = self.src(subject, index, false)?;
                (
                    Low::Related(src, window, *relation, fill_key(fill)),
                    Given::Fill(fill),
                )
            }
            Op::Descendants { subject, walk } => {
                let walk = Descend::of(walk, resolver, place).ok_or_else(no_fill)?;
                let (src, _) = self.src(subject, index, true)?;
                let prior = (self.range.start..index).fold(PROLOGUE, |prior, earlier| {
                    prior + worst(state, earlier, prior)
                });
                let room = room(&walk, starts_cap(state, index), prior);
                (
                    Low::Descendants(
                        src,
                        (walk.depth, walk.prunes, walk.include_subject, room),
                        fill_key(walk.fill),
                    ),
                    Given::Walk(walk),
                )
            }
            Op::Caret { subject, .. } => (Low::Caret(self.text(subject)?), Given::Nothing),
            Op::Visible { subject } => (Low::Visible(self.text(subject)?), Given::Nothing),
            Op::Move {
                subject,
                at,
                movement,
            } => {
                let by =
                    resolver.units((None, movement.unit, 1), verbatim_model::TextDetails::NONE)?;
                let element = self.text(subject)?;
                let held = matches!(at, Point::At(TextPoint::At(_)));
                (
                    Low::Move(
                        element,
                        self.point(at, index)?,
                        (by.unit.0, movement.count),
                        held,
                    ),
                    Given::Nothing,
                )
            }
            Op::ReadText {
                subject,
                at,
                unit,
                count,
                whole: true,
                ..
            } => {
                let by = resolver.units((None, *unit, 1), verbatim_model::TextDetails::NONE)?;
                let element = self.text(subject)?;
                (
                    Low::Whole(element, self.point(at, index)?, (by.unit.0, *count)),
                    Given::Nothing,
                )
            }
            Op::ReadText {
                subject,
                at,
                movement,
                unit,
                count,
                details,
                trim,
                ..
            } => {
                let read = super::read_of(resolver, (*movement, *unit, *count), (*details, *trim))?;
                let element = self.text(subject)?;
                (
                    Low::ReadText(element, self.point(at, index)?, read_key(&read)),
                    Given::Read(read),
                )
            }
            Op::ReadRange {
                subject,
                start,
                end,
                through,
                trim,
            } => {
                let element = self.text(subject)?;
                let ends = self.ends((start, end, through.as_ref()), index)?;
                (
                    Low::ReadRange(element, ends, super::range_limit(*trim)),
                    Given::Nothing,
                )
            }
            Op::Select {
                subject,
                start,
                end,
                through,
            } => {
                let element = self.text(subject)?;
                let ends = self.ends((start, end, through.as_ref()), index)?;
                (Low::Select(element, ends), Given::Nothing)
            }
            Op::CompareSelection {
                subject,
                before,
                after,
            } => {
                let element = self.text(subject)?;
                let (before, after) = self.compared(before, *after, index)?;
                (
                    Low::Compare(element, before, after, MAX_CHANGE_TEXT),
                    Given::Nothing,
                )
            }
            Op::Locate { subject, at } => {
                let element = self.text(subject)?;
                (Low::Locate(element, self.point(at, index)?), Given::Nothing)
            }
            Op::FindText {
                subject,
                at,
                needle,
                backward,
                unit_start,
                read,
                limit,
                calls,
            } => {
                let unit = unit_start.unwrap_or(verbatim_model::TextUnit::Line);
                let by = resolver.units((None, unit, 1), verbatim_model::TextDetails::NONE)?;
                let element = self.text(subject)?;
                (
                    Low::FindText(
                        (element, self.point(at, index)?),
                        (needle.clone(), *backward),
                        (by.unit.0, unit_start.is_some(), *read),
                        (*limit, *calls),
                    ),
                    Given::Nothing,
                )
            }
            Op::WaitForCaret { .. } => (self.wait_check()?, Given::Nothing),
            Op::ReadColours {
                subject, screen, ..
            } => {
                let planned = fused_size(state, subject).zip(self.here(*screen, index));
                let Some((size, screen)) = planned else {
                    return Err(Defect("a colour read in a program that cannot read it").ran());
                };
                (Low::Colours(screen, size), Given::Colours(size))
            }
            _ => return Err(Defect("a step no program runs").ran()),
        };
        Ok((low, gate, given))
    }
}

/// The outcomes before the step at `by` as its gate in a program judges
/// them: the program's own steps it names are tested where it runs, by
/// their place in the program, but for one whose outcome was decided
/// before the program, which emitted nothing to test.
struct Planned<'v, 'p, 's, 'a> {
    planner: &'v Planner<'p, 's, 'a>,
    by: usize,
}

impl Outcomes for Planned<'_, '_, '_, '_> {
    type Pending = usize;
    type Error = Defect;

    fn known(&self, place: Place) -> Result<Known<usize>, Defect> {
        let Some(here) = self.planner.here(place, self.by) else {
            return self.planner.state.known(place, self.by);
        };
        Ok(match &self.planner.pre[here] {
            None => Known::Pending(here),
            Some(Ran::Unrun(Unrun::Skipped)) => Known::Skipped,
            Some(_) => Known::Unrun,
        })
    }

    fn check(&self, place: Place, check: Check) -> Option<bool> {
        self.planner.state.check(place, self.by, check)
    }
}

/// The nodes of a list result.
fn nodes_of(found: &Found) -> Option<Vec<&Cached<Node>>> {
    match found {
        Found::Related(related) => Some(related.nodes.iter().collect()),
        Found::Descendants(descendants) => {
            Some(descendants.nodes.iter().map(|d| &d.node).collect())
        }
        _ => None,
    }
}

/// A unit read's shape.
fn read_key(read: &UnitsRead) -> [i32; 8] {
    let (movement, by, count) = match read.movement {
        None => (0, 0, 0),
        Some(Movement::By(unit, count)) => (1, unit.0, count),
        Some(Movement::Document(count)) => (2, 0, count),
    };
    [
        movement,
        by,
        count,
        read.unit.0,
        i32::try_from(read.count).unwrap_or(i32::MAX),
        i32::try_from(read.max_total).unwrap_or(i32::MAX),
        i32::from(read.culture),
        i32::from(attribute_bits(read.formats)) | (i32::from(attribute_bits(read.learning)) << 16),
    ]
}

/// The bits of an attribute set, for a shape.
fn attribute_bits(attributes: crate::caret::Attributes) -> u16 {
    attributes
        .iter()
        .fold(0, |bits, attribute| bits | 1 << (attribute as u16))
}

/// The registers a step's result is read from.
#[derive(Clone)]
enum Regs {
    Related {
        nodes: Reg<kind::Array>,
        absents: Reg<kind::Array>,
        window: Option<Reg<kind::Int>>,
        table: Option<Reg<kind::Str>>,
        cut: Option<Reg<kind::Bool>>,
    },
    Descendants(RunRegisters),
    Caret {
        caret: CaretRegisters,
        point: Reg<kind::TextRange>,
        at_text_start: Reg<kind::Bool>,
    },
    ReadText(Box<UnitsRegs>),
    ReadRange(Reg<kind::Str>),
    Compare {
        changes: (Reg<kind::Array>, Reg<kind::Array>),
        selected: Reg<kind::Bool>,
        selection_text: Reg<kind::Str>,
        held: Reg<kind::Bool>,
        previous_text: Reg<kind::Str>,
    },
    /// Its ending: done, or refused.
    Select(Reg<kind::Int>),
    Locate(Reg<kind::Array>),
    Visible {
        range: Reg<kind::TextRange>,
        text: Reg<kind::Str>,
        at_start: Reg<kind::Bool>,
    },
    Move {
        from: Reg<kind::TextRange>,
        to: Reg<kind::TextRange>,
        moved: Reg<kind::Int>,
        /// Its ending: done, or refused outside the text.
        ending: Reg<kind::Int>,
    },
    Whole {
        range: Reg<kind::TextRange>,
        text: Reg<kind::Str>,
    },
    WaitCheck {
        check: Box<crate::watch::CheckRegisters>,
        /// Whether the steps gated on it read.
        reads_after: Reg<kind::Bool>,
        /// Whether it is the check at the bound.
        last: bool,
    },
    Colours(super::colours::FirstRows),
    FindText {
        /// Each match's units to the text's end.
        to_ends: Reg<kind::Array>,
        /// Each match's text.
        texts: Reg<kind::Array>,
    },
}

/// A unit read's registers, its first unit's formatting's, and whether
/// its unit is the character, whose one stretch learns nothing.
type UnitsRegs = (UnitsRegisters, Option<FormatRegisters>, bool);

/// An emitted step's status and registers.
#[derive(Clone)]
struct Emitted {
    status: Reg<kind::Int>,
    regs: Option<Regs>,
}

/// Emits a program of `key`'s shape, with the fills and walks of the
/// segment's steps in `range`.
fn build(
    b: &mut Builder,
    (key, imports, given): (&Key, &[Import], &[Given<'_>]),
    injected: Option<Injected>,
) -> Vec<Option<Emitted>> {
    // Every status is "not reached" before any step runs.
    let statuses: Vec<Option<Reg<kind::Int>>> = key
        .iter()
        .map(|(low, _)| {
            (*low != Low::Unemitted).then(|| {
                let status = b.new_int(NOT_REACHED);
                b.add_to_results(status)
            })
        })
        .collect();
    let imported: Vec<Imported> = imports
        .iter()
        .map(|import| match import {
            Import::Element(element) => Imported::Element(b.import_element(element)),
            Import::TextRange(range) => Imported::Range(b.import_text_range(range)),
        })
        .collect();
    let c = Constants {
        start: b.int(endpoint_number(Endpoint::Start)),
        end: b.int(endpoint_number(Endpoint::End)),
        zero: b.int(0),
        one: b.int(1),
        max_text: b.int(0),
    };
    let shared = Shared::new(b, key);
    let mut emitted: Vec<Option<Emitted>> = Vec::with_capacity(key.len());
    for (place, ((low, gate), status)) in key.iter().zip(&statuses).enumerate() {
        let Some(status) = *status else {
            emitted.push(None);
            continue;
        };
        let given = given[place];
        inject(b, injected, Injected::Stops(place));
        inject(b, injected, Injected::Overruns(place));
        let regs = {
            let e = Emit {
                c: &c,
                imported: &imported,
                emitted: &emitted,
                shared: &shared,
                caret: std::cell::Cell::new(None),
            };
            let mut regs = None;
            let body = |b: &mut Builder, regs: &mut Option<Regs>| {
                let ok = e.gate(b, gate);
                b.if_else(
                    ok,
                    |b| {
                        b.try_catch(
                            |b| {
                                inject(b, injected, Injected::StepFails(place));
                                let emitted = e.step(b, low, given);
                                let zero = b.int(DONE);
                                match &emitted {
                                    // A relation longer than a program
                                    // reads is read by calls.
                                    Regs::Related { cut: Some(cut), .. } => {
                                        let over = b.int(CUT);
                                        b.if_else(
                                            *cut,
                                            |b| b.set(status, over),
                                            |b| b.set(status, zero),
                                        );
                                    }
                                    // A step that can be refused wrote
                                    // its ending.
                                    Regs::Move { ending, .. } | Regs::Select(ending) => {
                                        b.set(status, *ending);
                                    }
                                    _ => b.set(status, zero),
                                }
                                *regs = Some(emitted);
                            },
                            |b, failed| b.set(status, failed),
                        );
                    },
                    |b| {
                        let skipped = b.int(SKIPPED);
                        b.set(status, skipped);
                    },
                );
            };
            if matches!(low, Low::Select(..)) {
                // An act runs only once every earlier step of its program
                // finished; otherwise it is not reached, for the rerun to
                // make.
                let earlier: Vec<Reg<kind::Int>> =
                    emitted.iter().flatten().map(|e| e.status).collect();
                let all = e.finished(b, &earlier);
                b.if_(all, |b| body(b, &mut regs));
            } else {
                body(b, &mut regs);
            }
            regs
        };
        emitted.push(Some(Emitted { status, regs }));
    }
    emitted
}

/// An import's register.
#[derive(Clone, Copy)]
enum Imported {
    Element(Reg<kind::Element>),
    Range(Reg<kind::TextRange>),
}

/// What a program's steps share, so that each is read from the provider
/// once a program however many steps use it: each text's pattern and
/// document range, null until the first step that needs one reads it
/// inside its own try block, and the comparisons of two points already
/// made. The document range is the text as the first step to need it
/// found it: a step that read it anew, as each did before, could see text
/// written since an earlier step of its program read.
struct Shared {
    /// For each text's element, by its import: its pattern and its
    /// document range.
    texts: Vec<(usize, Reg<kind::TextPattern>, Reg<kind::TextRange>)>,
    /// Each comparison made, by the steps and points it names, with
    /// whether its point lay at or before the other.
    compared: std::cell::RefCell<Vec<(ComparedKey, Reg<kind::Bool>)>>,
    /// For each text a `FindText` searches, by its import: whether the
    /// search for [`ABSENT`] was made, and its answer.
    absent: Vec<(usize, Reg<kind::Bool>, Reg<kind::TextRange>)>,
    /// For each needle a `FindText` seeks, by its text's import and the
    /// needle: whether its search of the whole text was made, and whether
    /// it is there ([`Emit::somewhere`]).
    present: Vec<(Sought, Reg<kind::Bool>, Reg<kind::Bool>)>,
}

/// What a `FindText` seeks: its text's import and the needle.
type Sought = (usize, String);

/// What a comparison of two points names: the steps and the points.
type ComparedKey = (Vec<usize>, (usize, Pt, Pt));

impl Shared {
    /// The registers of the texts `key`'s steps read, each null.
    fn new(b: &mut Builder, key: &Key) -> Self {
        let mut imports: Vec<usize> = key
            .iter()
            .flat_map(|(low, gate)| {
                let compared = gate.compare.as_ref().map(|compare| compare.points.0);
                [low.text(), compared]
            })
            .flatten()
            .collect();
        imports.sort_unstable();
        imports.dedup();
        let mut searched: Vec<usize> = key
            .iter()
            .filter_map(|(low, _)| match low {
                Low::FindText((text, _), ..) => Some(*text),
                _ => None,
            })
            .collect();
        searched.sort_unstable();
        searched.dedup();
        let mut needles: Vec<(usize, String)> = key
            .iter()
            .filter_map(|(low, _)| match low {
                Low::FindText((text, _), (needle, _), ..) => Some((*text, needle.clone())),
                _ => None,
            })
            .collect();
        needles.sort_unstable();
        needles.dedup();
        Self {
            present: needles
                .into_iter()
                .map(|sought| (sought, b.new_bool(false), b.new_bool(false)))
                .collect(),
            absent: searched
                .into_iter()
                .map(|import| {
                    (
                        import,
                        b.new_bool(false),
                        b.new_null().assume::<kind::TextRange>(),
                    )
                })
                .collect(),
            texts: imports
                .into_iter()
                .map(|import| {
                    (
                        import,
                        b.new_null().assume::<kind::TextPattern>(),
                        b.new_null().assume::<kind::TextRange>(),
                    )
                })
                .collect(),
            compared: std::cell::RefCell::default(),
        }
    }
}

/// The registers one `FindText` call of a search uses
/// ([`Emit::find_once`]).
struct FindCall {
    /// The span still to search, and the text sought.
    span: Reg<kind::TextRange>,
    needle: Reg<kind::Str>,
    /// Whether the search goes toward the text's start, and false.
    toward_start: Reg<kind::Bool>,
    no: Reg<kind::Bool>,
    /// The unit, more of them than any text holds, how many are read
    /// from a match, and the most text read.
    unit: Reg<kind::Int>,
    far: Reg<kind::Int>,
    read: Reg<kind::Int>,
    max: Reg<kind::Int>,
    /// Whether the search may still find a match, and how many it took.
    open: Reg<kind::Bool>,
    taken: Reg<kind::Int>,
    /// Each match's units to the text's end, and its text.
    found: (Reg<kind::Array>, Reg<kind::Array>),
}

/// The emitter's context: the constants, the imports, what the steps
/// share, and the steps emitted before.
struct Emit<'e> {
    c: &'e Constants,
    imported: &'e [Imported],
    emitted: &'e [Option<Emitted>],
    shared: &'e Shared,
    /// The caret the step read, once it read it, so that its points share
    /// one read, as the text reads' programs share it.
    caret: std::cell::Cell<Option<CaretRegisters>>,
}

impl Emit<'_> {
    fn element(&self, index: usize) -> Reg<kind::Element> {
        match self.imported[index] {
            Imported::Element(element) => element,
            Imported::Range(_) => unreachable_import(),
        }
    }

    fn range(&self, index: usize) -> Reg<kind::TextRange> {
        match self.imported[index] {
            Imported::Range(range) => range,
            Imported::Element(_) => unreachable_import(),
        }
    }

    /// The registers of the text imported at `text`.
    fn text_regs(&self, text: usize) -> (Reg<kind::TextPattern>, Reg<kind::TextRange>) {
        self.shared
            .texts
            .iter()
            .find(|(import, ..)| *import == text)
            .map_or_else(
                || unreachable_import(),
                |&(_, pattern, document)| (pattern, document),
            )
    }

    /// Emits the pattern of the text imported at `text`, read by the first
    /// step of the program that needs it.
    fn pattern(&self, b: &mut Builder, text: usize) -> Reg<kind::TextPattern> {
        let (pattern, _) = self.text_regs(text);
        let unread = b.is_null(pattern);
        b.if_(unread, |b| {
            let read = b.get_text_pattern(self.element(text), false);
            b.set(pattern, read);
        });
        pattern
    }

    /// Emits the document range of the text imported at `text`, read by
    /// the first step of the program that needs it. No step moves it: each
    /// takes a copy.
    fn document(&self, b: &mut Builder, text: usize) -> Reg<kind::TextRange> {
        let (_, document) = self.text_regs(text);
        let unread = b.is_null(document);
        b.if_(unread, |b| {
            let pattern = self.pattern(b, text);
            let read = b.text_pattern_get_document_range(pattern);
            b.set(document, read);
        });
        document
    }

    fn regs(&self, place: usize) -> &Regs {
        self.emitted[place]
            .as_ref()
            .and_then(|emitted| emitted.regs.as_ref())
            .unwrap_or_else(|| unreachable_import())
    }

    fn status(&self, place: usize) -> Reg<kind::Int> {
        self.emitted[place]
            .as_ref()
            .map_or_else(|| unreachable_import(), |emitted| emitted.status)
    }

    /// Whether every status of `statuses` is an ending whose outcome the
    /// program knows: done, skipped or refused.
    fn finished(&self, b: &mut Builder, statuses: &[Reg<kind::Int>]) -> Reg<kind::Bool> {
        let all = b.new_bool(true);
        let skipped = b.int(SKIPPED);
        for &status in statuses {
            let known = b.compare(status, skipped, Comparison::GreaterThanOrEqual);
            let ended = b.compare(status, self.c.zero, Comparison::LessThanOrEqual);
            let either = b.and(known, ended);
            b.and_assign(all, either);
        }
        all
    }

    /// Whether the gate holds: what the model's rule left to test of the
    /// program's steps ([`Pending`]), each test emitted as the rule states
    /// it, and the comparison of two points.
    fn gate(&self, b: &mut Builder, gate: &Gate) -> Reg<kind::Bool> {
        let ok = b.new_bool(true);
        for &place in &gate.pending.done {
            let done = b.equal(self.status(place), self.c.zero);
            b.and_assign(ok, done);
        }
        if let Some((place, _, negated)) = gate.pending.step {
            let runs = self.found(b, place, negated);
            b.and_assign(ok, runs);
        }
        if !gate.pending.compared.is_empty() || gate.compare.is_some() {
            let runs = self.compared(b, &gate.pending.compared, gate.compare.as_ref());
            b.and_assign(ok, runs);
        }
        ok
    }

    /// Whether a found-or-not test of the step at `place` lets its step
    /// run.
    fn found(&self, b: &mut Builder, place: usize, negated: bool) -> Reg<kind::Bool> {
        let status = self.status(place);
        let holds = b.new_bool(false);
        let done = b.equal(status, self.c.zero);
        b.if_(done, |b| {
            let found = match self.regs(place) {
                Regs::Related { nodes, .. } => {
                    let size = b.array_size(*nodes);
                    let none = b.uint(0);
                    b.compare(size, none, Comparison::GreaterThan)
                }
                Regs::WaitCheck { reads_after, .. } => *reads_after,
                Regs::Caret { caret, .. } => caret.has_selection,
                _ => b.bool(true),
            };
            b.set(holds, found);
        });
        let skipped = b.int(SKIPPED);
        let skipped = b.equal(status, skipped);
        let passes = if negated { b.not(holds) } else { holds };
        let runs = b.new_bool(false);
        b.set(runs, passes);
        b.if_(skipped, |b| {
            let no = b.bool(false);
            b.set(runs, no);
        });
        runs
    }

    /// Whether a comparison of two points lets its step run (the request
    /// list's condition table): a step of the program it names, of
    /// `names`, skipped skips it; one not done, or a comparison that
    /// fails, makes the test false. With no `compare`, the test was
    /// decided false before the program and its step runs under its
    /// negation, unless a step of `names` was skipped.
    fn compared(
        &self,
        b: &mut Builder,
        names: &[usize],
        compare: Option<&Compare>,
    ) -> Reg<kind::Bool> {
        let c = self.c;
        let none_skipped = b.new_bool(true);
        for &place in names {
            let skipped = b.int(SKIPPED);
            let unskipped = b.compare(self.status(place), skipped, Comparison::NotEqual);
            b.and_assign(none_skipped, unskipped);
        }
        let Some(compare) = compare else {
            return none_skipped;
        };
        // A comparison an earlier step of the program made is not made
        // again: the steps it names have not changed since, so neither has
        // its answer, and a step under its negation costs no call. Every
        // gate that could make one runs whenever the program does: only an
        // act's is emitted under a condition, and an act ends its program.
        let key: ComparedKey = (names.to_vec(), compare.points.clone());
        let made = self
            .shared
            .compared
            .borrow()
            .iter()
            .find(|(made, _)| *made == key)
            .map(|&(_, holds)| holds);
        let holds = made.unwrap_or_else(|| {
            let all_done = b.new_bool(true);
            for &place in names {
                let done = b.equal(self.status(place), c.zero);
                b.and_assign(all_done, done);
            }
            let holds = b.new_bool(false);
            let (text, point, than) = &compare.points;
            b.if_(all_done, |b| {
                b.try_catch(
                    |b| {
                        let point = self.point(b, *text, point);
                        let than = self.point(b, *text, than);
                        let order = b.text_range_compare_endpoints(point, c.start, than, c.start);
                        let not_after = b.compare(order, c.zero, Comparison::LessThanOrEqual);
                        b.set(holds, not_after);
                    },
                    |_, _| {},
                );
            });
            // The caret read inside the test's own block is not the
            // step's to use again.
            self.caret.set(None);
            self.shared.compared.borrow_mut().push((key, holds));
            holds
        });
        let runs = if compare.negated { b.not(holds) } else { holds };
        b.and_assign(none_skipped, runs);
        none_skipped
    }

    /// The element a node source names: one node.
    fn node(&self, b: &mut Builder, src: &Src) -> Reg<kind::Element> {
        match src {
            Src::Import(index) => self.element(*index),
            Src::Here(place) => match self.regs(*place) {
                Regs::Related { nodes, .. } => {
                    let zero = b.uint(0);
                    b.array_get_at(*nodes, zero).assume::<kind::Element>()
                }
                _ => unreachable_import(),
            },
            Src::Imports(_) => unreachable_import(),
        }
    }

    /// Emits the step's body inside its try block.
    fn step(&self, b: &mut Builder, low: &Low, given: Given<'_>) -> Regs {
        match (low, given) {
            (Low::Related(src, window, relation, _), Given::Fill(fill)) => {
                let node = self.node(b, src);
                Self::related(b, node, (*window, *relation), fill)
            }
            (Low::Descendants(src, (.., room), _), Given::Walk(walk)) => {
                let queue = b.new_array();
                match src {
                    Src::Import(index) => {
                        let element = self.element(*index);
                        b.array_append(queue, element);
                    }
                    Src::Imports(indices) => {
                        for &index in indices {
                            let element = self.element(index);
                            b.array_append(queue, element);
                        }
                    }
                    Src::Here(place) => match self.regs(*place) {
                        Regs::Related { nodes, .. } => {
                            let size = b.array_size(*nodes);
                            let index = b.new_uint(0);
                            let one = b.uint(1);
                            b.while_(
                                |b| b.compare(index, size, Comparison::LessThan),
                                |b| {
                                    let node = b.array_get_at(*nodes, index);
                                    b.array_append(queue, node);
                                    b.add_assign(index, one);
                                },
                            );
                        }
                        _ => unreachable_import(),
                    },
                }
                Regs::Descendants(emit_run(
                    b,
                    &walk,
                    (queue, None),
                    *room,
                    walk.include_subject,
                ))
            }
            (Low::Caret(text), _) => self.caret(b, *text),
            (Low::ReadText(text, at, _), Given::Read(read)) => {
                self.read_text(b, (*text, at), &read)
            }
            (Low::ReadRange(text, ends, max), _) => {
                let range = self.range_between(b, *text, ends);
                let max = b.int(*max);
                let read = b.text_range_get_text(range, max);
                Regs::ReadRange(b.add_to_results(read))
            }
            (Low::Select(text, ends), _) => {
                let range = self.range_between(b, *text, ends);
                // A refused selection is the step's refusal.
                let ending = b.new_int(REFUSED_UNSUPPORTED);
                b.try_catch(
                    |b| {
                        b.text_range_select(range);
                        b.set(ending, self.c.zero);
                    },
                    |_, _| {},
                );
                Regs::Select(ending)
            }
            (Low::Compare(_, before, after, max), _) => self.compare(b, before, after, *max),
            (Low::Locate(text, at), _) => {
                let point = self.point(b, *text, at);
                let range = b.text_range_clone(point);
                let character = b.int(TextUnit_Character.0);
                b.text_range_expand_to_enclosing_unit(range, character);
                let rectangles = b.text_range_get_bounding_rectangles(range);
                Regs::Locate(b.add_to_results(rectangles))
            }
            (Low::WaitCheck(text, points, word, last, max), _) => {
                self.wait_check(b, (*text, points), (*word, *last, *max))
            }
            (Low::Visible(text), _) => self.visible(b, *text),
            (Low::Colours(screen, size), _) => self.colours(b, *screen, *size),
            (Low::Move(text, at, by, held), _) => self.move_point(b, (*text, at), *by, *held),
            (Low::FindText((text, at), (needle, backward), read, bounds), _) => {
                self.find_text(b, (*text, at), (needle, *backward), *read, *bounds)
            }
            (Low::Whole(text, at, (unit, count)), _) => {
                let range = self.own_point(b, *text, at);
                let unit = b.int(*unit);
                let count = b.int(i32::from(*count));
                let _ = b.text_range_move_endpoint_by_unit(range, self.c.end, unit, count);
                let max = b.int(MAX_RANGE_TEXT);
                let text = b.text_range_get_text(range, max);
                Regs::Whole {
                    range: b.add_to_results(range),
                    text: b.add_to_results(text),
                }
            }
            _ => unreachable_import(),
        }
    }

    /// Emits a `Related` from `node`, whose own window is `window`:
    /// a step through the tree as the navigation step takes it, with the
    /// subject's nearest window and a cell's table; the controller-for
    /// relation's elements, at most [`CONTROLLED_PER_RUN`]; or the first
    /// selected item, a failed read of it leaving none, as
    /// `verbatim_uia::Uia::selected` reads it.
    fn related(
        b: &mut Builder,
        node: Reg<kind::Element>,
        (window, relation): (isize, Relation),
        fill: &Fill<Node>,
    ) -> Regs {
        let cache = RemoteCache::new(b, fill);
        let nodes = b.new_array();
        let nodes = b.add_to_results(nodes);
        let absents = b.new_array();
        let absents = b.add_to_results(absents);
        let take = |b: &mut Builder, element: Reg<kind::Element>| {
            let absent = cache.populate(b, element);
            b.array_append(nodes, element);
            b.array_append(absents, absent);
        };
        let mut regs = Regs::Related {
            nodes,
            absents,
            window: None,
            table: None,
            cut: None,
        };
        let direction = match relation {
            Relation::Parent => NavigationDirection::Parent,
            Relation::FirstChild => NavigationDirection::FirstChild,
            Relation::LastChild => NavigationDirection::LastChild,
            Relation::NextSibling => NavigationDirection::NextSibling,
            Relation::PreviousSibling => NavigationDirection::PreviousSibling,
            Relation::FirstSelectedItem => {
                b.try_catch(|b| emit_first_selected(b, node, &take), |_, _| {});
                return regs;
            }
            Relation::ControllerFor => {
                let over = b.new_bool(false);
                let over = b.add_to_results(over);
                let relation = b.property(node, UIA_ControllerForPropertyId.0);
                let is_array = b.is(TypeTest::Array, relation);
                b.if_(is_array, |b| {
                    let controlled = relation.assume::<kind::Array>();
                    let size = b.array_size(controlled);
                    let cap = b.uint(CONTROLLED_PER_RUN);
                    let long = b.compare(size, cap, Comparison::GreaterThan);
                    b.set(over, long);
                    let read = b.not(long);
                    b.if_(read, |b| {
                        let index = b.new_uint(0);
                        let one = b.uint(1);
                        b.while_(
                            |b| b.compare(index, size, Comparison::LessThan),
                            |b| {
                                let element = b.array_get_at(controlled, index);
                                take(b, element.assume::<kind::Element>());
                                b.add_assign(index, one);
                            },
                        );
                    });
                });
                if let Regs::Related { cut, .. } = &mut regs {
                    *cut = Some(over);
                }
                return regs;
            }
        };
        let nearest = emit_nearest_window(b, node, window);
        let neighbour = b.navigate(node, direction);
        let null = b.is_null(neighbour);
        let found = b.not(null);
        b.if_(found, |b| take(b, neighbour));
        if let Regs::Related { window, table, .. } = &mut regs {
            *window = Some(nearest);
            *table = reads_cell(fill).then(|| read_table(b, neighbour));
        }
        regs
    }

    /// Emits a `WaitForCaret` poll's check: the check of the text of the element
    /// imported at `text` against the starting `points`, and its gate's
    /// value, whether the steps gated on it read: the caret or the
    /// selection moved, or `last`.
    fn wait_check(
        &self,
        b: &mut Builder,
        (text, points): (usize, &[(usize, bool); 3]),
        (word, last, max): (bool, bool, i32),
    ) -> Regs {
        let c = Constants {
            max_text: b.int(max),
            ..*self.c
        };
        let point =
            |(range, end): (usize, bool)| (self.range(range), if end { c.end } else { c.start });
        let element = self.element(text);
        let check = emit_check(
            b,
            &c,
            element,
            (point(points[0]), point(points[1]), point(points[2])),
            word,
        );
        let reads_after = b.new_bool(last);
        let moved = b.or(check.caret_moved, check.selection_moved);
        b.or_assign(reads_after, moved);
        Regs::WaitCheck {
            check: Box::new(check),
            reads_after,
            last,
        }
    }

    /// Emits a `Caret`: the caret and the selection as the classic
    /// reads find them, a collapsed copy at the caret, and, when something
    /// is selected, whether the selection's start is at the text's start.
    fn caret(&self, b: &mut Builder, text: usize) -> Regs {
        let pattern = self.pattern(b, text);
        let caret = emit_caret(b, self.c, pattern);
        let point = self.c.collapsed(b, caret.caret);
        let point = b.add_to_results(point);
        let at_text_start = b.new_bool(false);
        let at_text_start = b.add_to_results(at_text_start);
        let c = self.c;
        b.if_(caret.has_selection, |b| {
            let document = self.document(b, text);
            let order = b.text_range_compare_endpoints(caret.selection, c.start, document, c.start);
            let at_start = b.equal(order, c.zero);
            b.set(at_text_start, at_start);
        });
        Regs::Caret {
            caret,
            point,
            at_text_start,
        }
    }

    /// Emits a `ReadColours`' first rows, `rows` rows of `width` cells from
    /// the start of the screen the program's step at `screen` read, as the
    /// colour read's first program reads them.
    fn colours(&self, b: &mut Builder, screen: usize, (rows, width): (u32, u32)) -> Regs {
        let Regs::Visible { range, .. } = self.regs(screen) else {
            unreachable_import()
        };
        let width = usize::try_from(width).unwrap_or(usize::MAX);
        Regs::Colours(super::colours::emit_from(b, *range, false, (rows, width)))
    }

    /// Emits a `Visible`: the first visible range, or the whole text's
    /// when the provider shows none, its text, and whether it starts at
    /// the text's start.
    fn visible(&self, b: &mut Builder, text: usize) -> Regs {
        let c = self.c;
        let pattern = self.pattern(b, text);
        let document = self.document(b, text);
        let screen = b.text_range_clone(document);
        let ranges = b.text_pattern_get_visible_ranges(pattern);
        let size = b.array_size(ranges);
        let none = b.uint(0);
        let some = b.compare(size, none, Comparison::GreaterThan);
        b.if_(some, |b| {
            let first = b.array_get_at(ranges, none).assume::<kind::TextRange>();
            b.set(screen, first);
        });
        let max = b.int(MAX_RANGE_TEXT);
        let read = b.text_range_get_text(screen, max);
        let order = b.text_range_compare_endpoints(document, c.start, screen, c.start);
        let at_start = b.equal(order, c.zero);
        Regs::Visible {
            range: b.add_to_results(screen),
            text: b.add_to_results(read),
            at_start: b.add_to_results(at_start),
        }
    }

    /// Emits a `Move` of the point `at` by `count` units of `unit`, from
    /// the point itself; a point Core holds (`held`) is first compared
    /// with the text's end, and the move is refused past it or on any
    /// failure, as a mark's read always was: its ending is then
    /// [`REFUSED_OUTSIDE`], which the step's status takes.
    fn move_point(
        &self,
        b: &mut Builder,
        (text, at): (usize, &Pt),
        (unit, count): (i32, i32),
        held: bool,
    ) -> Regs {
        let c = self.c;
        let from = b.new_null().assume::<kind::TextRange>();
        let from = b.add_to_results(from);
        let to = b.new_null().assume::<kind::TextRange>();
        let to = b.add_to_results(to);
        let moved = b.new_int(0);
        let moved = b.add_to_results(moved);
        let go = |b: &mut Builder, point: Reg<kind::TextRange>| {
            b.set(from, point);
            let walker = b.text_range_clone(point);
            let by = b.int(unit);
            let count = b.int(count);
            let went = b.text_range_move(walker, by, count);
            b.set(to, walker);
            b.set(moved, went);
        };
        let ending = if held {
            let ending = b.new_int(REFUSED_OUTSIDE);
            b.try_catch(
                |b| {
                    let point = self.point(b, text, at);
                    let document = self.document(b, text);
                    let order = b.text_range_compare_endpoints(point, c.start, document, c.end);
                    let within = b.compare(order, c.zero, Comparison::LessThanOrEqual);
                    b.if_(within, |b| {
                        go(b, point);
                        b.set(ending, c.zero);
                    });
                },
                |_, _| {},
            );
            ending
        } else {
            let point = self.point(b, text, at);
            go(b, point);
            c.zero
        };
        Regs::Move {
            from,
            to,
            moved,
            ending,
        }
    }

    /// Emits the answer of the text imported at `text` to a search of
    /// `document` for [`ABSENT`], made by the first `FindText` of the
    /// program that needs it.
    fn absent(
        &self,
        b: &mut Builder,
        text: usize,
        document: Reg<kind::TextRange>,
    ) -> Reg<kind::TextRange> {
        let Some(&(_, sought, nowhere)) = self
            .shared
            .absent
            .iter()
            .find(|(import, ..)| *import == text)
        else {
            unreachable_import()
        };
        let unsought = b.not(sought);
        b.if_(unsought, |b| {
            let absent = b.string(ABSENT);
            let no = b.bool(false);
            let answer = b.text_range_find_text(document, absent, no, no);
            b.set(nowhere, answer);
            let yes = b.bool(true);
            b.set(sought, yes);
        });
        nowhere
    }

    /// Emits a `FindText` from the point `at`, as its call arm searches
    /// (`calls.rs`, `find_text`), with two guards for Windows Terminal,
    /// whose search answers a miss with a range, of two kinds, on which a
    /// call fails in a way that ends the whole run whatever try block
    /// holds it (probed in Windows Terminal 1.24.12741 on Windows 11
    /// 26300, 2026-11-21).
    ///
    /// When the text is nowhere, the range compares equal to every other
    /// such range and to nothing else. So the needle is first sought in
    /// the whole text and its hit compared with the answer for [`ABSENT`]:
    /// equal, null, or a search that fails is nothing found, with no
    /// other call. In a provider that answers null for [`ABSENT`] there
    /// is no such range to compare with.
    ///
    /// When the text is elsewhere, every call on the range fails that
    /// way, while a call on a real range given it fails as any call
    /// does. So before any call on a hit, the span searched is compared
    /// with it, which fails into the try block and ends the search with
    /// what it found.
    ///
    /// The two searches of the whole text, each made once in a program
    /// for a text and needle and shared by its steps, are outside the
    /// step's `calls`.
    /// Limits: output that trims the needle from the history after the
    /// first search makes a later miss the nowhere kind, whose call ends
    /// the run, and the fallback rule runs the step by calls; and a
    /// search for exactly [`ABSENT`] finds nothing in a provider whose
    /// text holds it.
    fn find_text(
        &self,
        b: &mut Builder,
        (text, at): (usize, &Pt),
        (needle, backward): (&str, bool),
        (unit, starts, read): (i32, bool, u8),
        (limit, calls): (u8, u8),
    ) -> Regs {
        let c = self.c;
        let to_ends = b.new_array();
        let to_ends = b.add_to_results(to_ends);
        let texts = b.new_array();
        let texts = b.add_to_results(texts);
        let regs = Regs::FindText { to_ends, texts };
        if limit == 0 || calls == 0 {
            return regs;
        }
        let document = self.document(b, text);
        let at = self.point(b, text, at);
        let span = b.text_range_clone(document);
        if backward {
            b.text_range_move_endpoint_by_range(span, c.end, at, c.start);
        } else {
            b.text_range_move_endpoint_by_range(span, c.start, at, c.start);
        }
        let sought = needle;
        let needle = b.string(needle);
        let no = b.bool(false);
        // The search's own copy, which a miss or a failed call closes.
        let open = b.new_bool(false);
        let present = self.somewhere(b, text, document, (sought, needle));
        b.set(open, present);
        let call = FindCall {
            span,
            needle,
            toward_start: b.bool(backward),
            no,
            unit: b.int(unit),
            far: b.int(FAR),
            read: b.int(i32::from(read.max(1))),
            max: b.int(MAX_RANGE_TEXT),
            open,
            taken: b.new_int(0),
            found: (to_ends, texts),
        };
        let most = b.int(i32::from(limit));
        let most_calls = b.int(i32::from(calls));
        let made = b.new_int(0);
        b.while_(
            |b| {
                let more = b.compare(call.taken, most, Comparison::LessThan);
                let left = b.compare(made, most_calls, Comparison::LessThan);
                let going = b.and(more, left);
                b.and(going, open)
            },
            |b| {
                b.add_assign(made, c.one);
                b.try_catch(
                    |b| self.find_once(b, &call, (backward, starts)),
                    // A failed call ends the search with what it found.
                    |b, _| b.set(open, no),
                );
            },
        );
        regs
    }

    /// Emits whether `needle` is anywhere in `document`, the whole text
    /// imported at `text`: false when its search fails, answers null, or
    /// answers what the search for [`ABSENT`] answers. The search is made
    /// by the first `FindText` of the program that seeks this needle in
    /// this text, and its answer shared by the rest, so that a search
    /// made each way seeks it once.
    fn somewhere(
        &self,
        b: &mut Builder,
        text: usize,
        document: Reg<kind::TextRange>,
        (sought, needle): (&str, Reg<kind::Str>),
    ) -> Reg<kind::Bool> {
        let Some(&(_, searched, present)) = self
            .shared
            .present
            .iter()
            .find(|((import, wanted), ..)| *import == text && wanted == sought)
        else {
            unreachable_import()
        };
        let unsearched = b.not(searched);
        b.if_(unsearched, |b| {
            let yes = b.bool(true);
            b.set(searched, yes);
            self.search_whole(b, text, document, (needle, present));
        });
        present
    }

    /// Emits the search of `document` for `needle` that sets `present`
    /// ([`Self::somewhere`]).
    fn search_whole(
        &self,
        b: &mut Builder,
        text: usize,
        document: Reg<kind::TextRange>,
        (needle, present): (Reg<kind::Str>, Reg<kind::Bool>),
    ) {
        let no = b.bool(false);
        b.try_catch(
            |b| {
                let hit = b.text_range_find_text(document, needle, no, no);
                let missing = b.is_null(hit);
                let found = b.not(missing);
                b.if_(found, |b| {
                    let nowhere = self.absent(b, text, document);
                    let unanswered = b.is_null(nowhere);
                    b.if_else(
                        unanswered,
                        |b| b.set(present, found),
                        |b| {
                            let same = b.text_range_compare(hit, nowhere);
                            let other = b.not(same);
                            b.set(present, other);
                        },
                    );
                });
            },
            // Not searchable: nothing found.
            |_, _| {},
        );
    }

    /// Emits one `FindText` call of a search, inside the search's try
    /// block, toward the text's start when `backward`: no hit closes the
    /// search; a hit moves the span past it, and one that starts its
    /// unit, or any when `starts` is not asked, is taken, its units to
    /// the text's end and its text appended.
    fn find_once(&self, b: &mut Builder, call: &FindCall, (backward, starts): (bool, bool)) {
        let c = self.c;
        let hit = b.text_range_find_text(call.span, call.needle, call.toward_start, call.no);
        let missing = b.is_null(hit);
        b.if_else(
            missing,
            |b| b.set(call.open, call.no),
            |b| {
                // A call on the span given the hit, before any call on the
                // hit.
                let _ = b.text_range_compare_endpoints(call.span, c.start, hit, c.start);
                let row = b.text_range_clone(hit);
                if starts {
                    b.text_range_expand_to_enclosing_unit(row, call.unit);
                }
                let order = b.text_range_compare_endpoints(row, c.start, hit, c.start);
                // The next search, past this hit.
                if backward {
                    b.text_range_move_endpoint_by_range(call.span, c.end, hit, c.start);
                } else {
                    b.text_range_move_endpoint_by_range(call.span, c.start, hit, c.end);
                }
                let begins = b.equal(order, c.zero);
                b.if_(begins, |b| {
                    b.text_range_move_endpoint_by_range(row, c.end, row, c.start);
                    let walker = b.text_range_clone(row);
                    let to_end = b.text_range_move(walker, call.unit, call.far);
                    let _ = b.text_range_move_endpoint_by_unit(row, c.end, call.unit, call.read);
                    let text = b.text_range_get_text(row, call.max);
                    b.array_append(call.found.0, to_end);
                    b.array_append(call.found.1, text);
                    b.add_assign(call.taken, c.one);
                });
            },
        );
    }

    /// Emits a collapsed range at `point` in the text of `text`'s element,
    /// for a step that only reads it: it may be the range another step
    /// answered or the outpost holds, which nothing may move.
    fn point(&self, b: &mut Builder, text: usize, point: &Pt) -> Reg<kind::TextRange> {
        self.point_or_shared(b, text, point).0
    }

    /// Emits a collapsed range at `point` that is the step's own to move.
    fn own_point(&self, b: &mut Builder, text: usize, point: &Pt) -> Reg<kind::TextRange> {
        match self.point_or_shared(b, text, point) {
            (shared, true) => self.c.collapsed(b, shared),
            (own, false) => own,
        }
    }

    /// Emits a collapsed range at `point`, and whether it is a range the
    /// step shares, already collapsed where it was read: a held position
    /// known collapsed, a caret read's point, or where a move landed. Each
    /// copy not made is two calls to the provider.
    fn point_or_shared(
        &self,
        b: &mut Builder,
        text: usize,
        point: &Pt,
    ) -> (Reg<kind::TextRange>, bool) {
        let c = self.c;
        let collapsed_to_end = |b: &mut Builder, range: Reg<kind::TextRange>| {
            let copy = b.text_range_clone(range);
            b.text_range_move_endpoint_by_range(copy, c.start, copy, c.end);
            copy
        };
        let held = |b: &mut Builder, (range, end, _): (usize, bool, bool)| {
            let range = self.range(range);
            if end {
                collapsed_to_end(b, range)
            } else {
                c.collapsed(b, range)
            }
        };
        let own = |range: Reg<kind::TextRange>| (range, false);
        match point {
            Pt::Caret | Pt::SelectionStart | Pt::SelectionEnd => {
                let caret = if let Some(caret) = self.caret.get() {
                    caret
                } else {
                    let pattern = self.pattern(b, text);
                    let caret = emit_caret(b, c, pattern);
                    self.caret.set(Some(caret));
                    caret
                };
                own(self.caret_point(b, &caret, point_of(point)))
            }
            Pt::Start => {
                let document = self.document(b, text);
                own(c.collapsed(b, document))
            }
            Pt::End => {
                let document = self.document(b, text);
                own(collapsed_to_end(b, document))
            }
            Pt::At(range, _, true) => (self.range(*range), true),
            Pt::At(range, end, collapsed) => own(held(b, (*range, *end, *collapsed))),
            Pt::After {
                from,
                prefix,
                counts,
            } => {
                let base = held(b, *from);
                own(emit_after(b, c, base, (prefix, counts)))
            }
            Pt::Here(place, which) => match self.regs(*place) {
                Regs::Caret { caret, point, .. } => match which {
                    PointOf::Caret => (*point, true),
                    _ => own(self.caret_point(b, caret, *which)),
                },
                Regs::ReadText(units) => own(c.collapsed(b, units.0.first.0)),
                Regs::Whole { range, .. } => own(c.collapsed(b, *range)),
                Regs::Visible { range, .. } => own(if *which == PointOf::VisibleEnd {
                    collapsed_to_end(b, *range)
                } else {
                    c.collapsed(b, *range)
                }),
                Regs::Move { to, .. } => (*to, true),
                _ => unreachable_import(),
            },
        }
    }

    /// Emits a collapsed range at a caret read's point `which`: the
    /// selection's end it names, else the caret.
    fn caret_point(
        &self,
        b: &mut Builder,
        caret: &CaretRegisters,
        which: PointOf,
    ) -> Reg<kind::TextRange> {
        let c = self.c;
        let point = b.new_null().assume::<kind::TextRange>();
        let at_caret = |b: &mut Builder| {
            let copy = c.collapsed(b, caret.caret);
            b.set(point, copy);
        };
        match which {
            PointOf::SelectionStart | PointOf::SelectionEnd => b.if_else(
                caret.has_selection,
                |b| {
                    let copy = b.text_range_clone(caret.selection);
                    if which == PointOf::SelectionEnd {
                        b.text_range_move_endpoint_by_range(copy, c.start, copy, c.end);
                    } else {
                        b.text_range_move_endpoint_by_range(copy, c.end, copy, c.start);
                    }
                    b.set(point, copy);
                },
                at_caret,
            ),
            PointOf::Caret
            | PointOf::ChunkStart
            | PointOf::VisibleStart
            | PointOf::VisibleEnd
            | PointOf::Moved => at_caret(b),
        }
        point
    }

    /// Emits the range between a range's points, as the text reads make
    /// it: from the earlier, through the character at the later when
    /// asked.
    fn range_between(&self, b: &mut Builder, text: usize, ends: &LowEnds) -> Reg<kind::TextRange> {
        let c = self.c;
        let start = self.point(b, text, &ends.start);
        let end = self.point(b, text, &ends.end);
        let (past_start, past_end) = match &ends.through {
            Some((past_start, past_end)) => (
                self.point(b, text, past_start),
                self.point(b, text, past_end),
            ),
            None => (start, end),
        };
        emit_between(b, c, (start, end), (past_start, past_end))
    }

    /// Emits a `ReadText`, as the text reads read units, and the
    /// first unit's formatting.
    fn read_text(&self, b: &mut Builder, (text, at): (usize, &Pt), read: &UnitsRead) -> Regs {
        let c = Constants {
            max_text: b.int(MAX_UNIT_TEXT),
            ..*self.c
        };
        let from = self.point(b, text, at);
        let element = self.element(text);
        let units = emit_units(
            b,
            &c,
            (from, element),
            (read.movement, read.unit, read.count),
            (read.max_total, read.culture),
        );
        let ids: Vec<_> = read.formats.iter().collect();
        let character = read.unit == TextUnit_Character;
        let runs = (!ids.is_empty())
            .then(|| emit_runs(b, &c, units.first, character, &ids, read.learning));
        Regs::ReadText(Box::new((units, runs, character)))
    }

    /// Emits a `CompareSelection`.
    fn compare(&self, b: &mut Builder, before: &Before, after: &After, max: i32) -> Regs {
        let c = self.c;
        let old = b.new_null().assume::<kind::TextRange>();
        let old_end = b.new_int(0);
        let old_collapsed = b.new_bool(false);
        match before {
            Before::Held(range, end, collapsed) => {
                b.set(old, self.range(*range));
                b.set(old_end, if *end { c.end } else { c.start });
                let collapsed = b.bool(*collapsed);
                b.set(old_collapsed, collapsed);
            }
            Before::Here(place) => {
                let Regs::Caret { caret, .. } = self.regs(*place) else {
                    unreachable_import()
                };
                b.if_else(
                    caret.has_selection,
                    |b| {
                        b.set(old, caret.selection);
                        b.set(old_end, c.end);
                    },
                    |b| {
                        b.set(old, caret.caret);
                        b.set(old_end, c.start);
                        let yes = b.bool(true);
                        b.set(old_collapsed, yes);
                    },
                );
            }
        }
        let (has_selection, selection, point) = match after {
            After::Held(point, selection) => {
                let point = self.range(*point);
                match selection {
                    Some(selection) => (b.bool(true), self.range(*selection), point),
                    None => (b.bool(false), point, point),
                }
            }
            After::Here(place) => {
                let Regs::Caret { caret, point, .. } = self.regs(*place) else {
                    unreachable_import()
                };
                (caret.has_selection, caret.selection, *point)
            }
        };
        let old_ends: (ProgramEnd, ProgramEnd) = ((old, c.start), (old, old_end));
        let yes = b.bool(true);
        let changes = emit_changes(
            b,
            c,
            (has_selection, selection, point),
            (old_ends, old_collapsed),
            (yes, max),
        );
        let compared = b.int(MAX_COMPARED_TEXT);
        let selection_text = b.new_string("");
        let selection_text = b.add_to_results(selection_text);
        b.if_(has_selection, |b| {
            let read = b.text_range_get_text(selection, compared);
            b.set(selection_text, read);
        });
        let previous_text = b.new_string("");
        let previous_text = b.add_to_results(previous_text);
        let held = b.not(old_collapsed);
        let held = b.add_to_results(held);
        b.if_(held, |b| {
            let between = b.text_range_clone(old);
            b.text_range_move_endpoint_by_range(between, c.end, old, old_end);
            let read = b.text_range_get_text(between, compared);
            b.set(previous_text, read);
        });
        let selected = b.new_bool(false);
        b.set(selected, has_selection);
        let selected = b.add_to_results(selected);
        Regs::Compare {
            changes,
            selected,
            selection_text,
            held,
            previous_text,
        }
    }
}

/// The point of a caret read a program point names.
fn point_of(point: &Pt) -> PointOf {
    match point {
        Pt::SelectionStart => PointOf::SelectionStart,
        Pt::SelectionEnd => PointOf::SelectionEnd,
        _ => PointOf::Caret,
    }
}

/// A program's own registers or imports named wrongly: a defect of the
/// builder's, which no input reaches.
fn unreachable_import() -> ! {
    panic!("a program's step names registers of another kind than it emitted")
}

/// A failure a test makes a program meet at a fault point
/// (`verbatim_faults::Fault::ProgramStepFails` and the three after it),
/// by the place of the step in the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Injected {
    StepFails(usize),
    Stops(usize),
    Overruns(usize),
    AnswerLost,
}

/// The failure a test armed for the program of `range`, if any.
#[cfg(any(test, feature = "faults"))]
fn injected(state: &State<'_, '_>, range: &Range<usize>) -> Option<Injected> {
    use verbatim_faults::{Fault, hit};
    let places: Vec<(usize, u8)> = range
        .clone()
        .map(|index| (index - range.start, state.place(index).0))
        .collect();
    for &(at, place) in &places {
        if hit(Fault::ProgramStepFails, &place) {
            return Some(Injected::StepFails(at));
        }
        if hit(Fault::ProgramStops, &place) {
            return Some(Injected::Stops(at));
        }
        if hit(Fault::ProgramOverruns, &place) {
            return Some(Injected::Overruns(at));
        }
    }
    let first = places.first()?.1;
    hit(Fault::ProgramAnswerLost, &first).then_some(Injected::AnswerLost)
}

/// No test arms a failure in a build without fault points.
#[cfg(not(any(test, feature = "faults")))]
fn injected(_: &State<'_, '_>, _: &Range<usize>) -> Option<Injected> {
    None
}

/// Emits the failure `injected` names when it is `here`: an instruction
/// that fails, or a loop that does not end.
fn inject(b: &mut Builder, injected: Option<Injected>, here: Injected) {
    if injected != Some(here) {
        return;
    }
    if let Injected::Overruns(_) = here {
        b.while_(|b| b.bool(true), |_| {});
    } else {
        let failure = b.int(windows::Win32::Foundation::E_FAIL.0);
        b.set_operation_status(failure);
    }
}

/// How a step ended in a program.
enum Ended {
    Done,
    Refused(StepRefusal),
    Skipped,
    NotReached,
    Cut,
    Failed(i32),
}

impl Ended {
    fn of(status: i32) -> Self {
        match status {
            DONE => Self::Done,
            REFUSED_OUTSIDE => Self::Refused(StepRefusal::OutsideText),
            REFUSED_UNSUPPORTED => Self::Refused(StepRefusal::Unsupported),
            SKIPPED => Self::Skipped,
            NOT_REACHED => Self::NotReached,
            CUT => Self::Cut,
            failed => Self::Failed(failed),
        }
    }
}

impl<'a> State<'_, 'a> {
    /// Runs the steps of `range` as one program, then what the fallback
    /// rule reruns by calls; answers where the next step to run is.
    pub(super) fn program(&mut self, range: Range<usize>) -> usize {
        let plan = self.plan(&range);
        let program_holds_act = range
            .clone()
            .any(|index| matches!(self.step(index).op, Op::Select { .. }));
        if plan.key.iter().all(|(low, _)| *low == Low::Unemitted) {
            for (index, pre) in range.clone().zip(plan.pre) {
                self.ran[index] = pre;
            }
            return range.end;
        }
        let Plan {
            key,
            imports,
            given,
            pre,
        } = plan;
        let injected = injected(self, &range);
        // A program that searches holds the text it seeks, so each is
        // built for its run and none kept.
        let searches = key.iter().any(|(low, _)| matches!(low, Low::FindText(..)));
        let (operation, emitted) = if injected.is_some() || searches {
            // A program a test makes fail is built apart, never kept.
            let mut b = Builder::new();
            let emitted = build(&mut b, (&key, &imports, &given), injected);
            (b.finish(), emitted)
        } else {
            PROGRAMS.with(|programs| {
                programs.operation(
                    key.clone(),
                    || imports.clone(),
                    |b| build(b, (&key, &imports, &given), None),
                )
            })
        };
        let mut run = operation.execute();
        if injected == Some(Injected::AnswerLost) && run.is_ok() {
            run = Err(Error::Execute(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "the run's answer was lost",
            )));
        }
        match &run {
            Ok(outcome) => self.take(range, (&pre, &given), &emitted, outcome, None),
            // A run that stopped partway leaves the statuses it wrote.
            Err(error @ Error::Failed(failure))
                if matches!(
                    failure.status,
                    RunStatus::InstructionLimitExceeded | RunStatus::UnhandledException
                ) =>
            {
                if failure.status == RunStatus::InstructionLimitExceeded {
                    tracing::error!(
                        target: verbatim_model::DEFECT_TARGET,
                        location = ?failure.location,
                        "a generated program passed the instruction limit its emitters declared"
                    );
                }
                self.take(
                    range,
                    (&pre, &given),
                    &emitted,
                    &failure.partial,
                    Some(error),
                )
            }
            Err(error) if gone_or_timed_out(error) => {
                for (index, pre) in range.clone().zip(pre) {
                    self.ran[index] = Some(pre.unwrap_or_else(|| failed(error)));
                }
                range.end
            }
            Err(error) if could_not_run(error) => {
                // Nothing ran: the window is read classically from now on.
                self.read_classically = true;
                self.rerun(range.start..self.segment.steps.len(), error);
                self.segment.steps.len()
            }
            Err(error) => {
                // The run ended with no status read: what it did is not
                // known. A program holding the act reruns its own steps,
                // the act among them; any other reruns the rest.
                let end = if program_holds_act {
                    range.end
                } else {
                    self.segment.steps.len()
                };
                self.rerun(range.start..end, error);
                end
            }
        }
    }

    /// Takes a program's answer, whole or partial, and reruns by calls
    /// from its first step that failed other than by being gone or timing
    /// out, or that the run did not reach.
    fn take(
        &mut self,
        range: Range<usize>,
        (pre, given): (&[Option<Ran>], &[Given<'_>]),
        emitted: &[Option<Emitted>],
        outcome: &Outcome,
        error: Option<&Error>,
    ) -> usize {
        for (place, index) in range.clone().enumerate() {
            if let Some(ran) = &pre[place] {
                self.ran[index] = Some(clone_unrun(ran));
                continue;
            }
            let Some(emitted) = &emitted[place] else {
                continue;
            };
            let status = outcome.get(emitted.status).unwrap_or(NOT_REACHED);
            let rerun = |this: &mut Self, why: &Error| {
                this.rerun(index..this.segment.steps.len(), why);
                this.segment.steps.len()
            };
            // A colour read whose rows the program read and failed at, or
            // whose colour programs failed going on from them, is read
            // classically by its rerun: one try of the programs, and the
            // rerun's one line in the log.
            let colours = matches!(self.step(index).op, Op::ReadColours { .. });
            let ran = match Ended::of(status) {
                Ended::Done => match self.found(index, (emitted, given[place]), outcome) {
                    Ok(ran) => ran,
                    Err(why) => {
                        self.colours_failed = colours.then_some(index);
                        return rerun(self, &why);
                    }
                },
                Ended::Refused(refusal) => Ran::Unrun(Unrun::Refused(refusal)),
                Ended::Skipped => Ran::Unrun(Unrun::Skipped),
                Ended::Cut => {
                    // A list longer than a program reads: it and what
                    // follows, which its status kept from running, by
                    // calls, a size-limit path and no fallback.
                    let end = self.segment.steps.len();
                    self.calls(index..end);
                    return end;
                }
                Ended::NotReached => {
                    // Not reached: the run stopped before it, or an act an
                    // earlier step's failure kept from running.
                    let unreached = Error::Uia(windows::core::Error::new(
                        windows::Win32::Foundation::E_ABORT,
                        "the program did not reach the step",
                    ));
                    return rerun(self, error.unwrap_or(&unreached));
                }
                Ended::Failed(code) => {
                    let why = Error::Uia(windows::core::Error::from(windows::core::HRESULT(code)));
                    match failed(&why) {
                        Ran::Failed => {
                            self.colours_failed = colours.then_some(index);
                            return rerun(self, &why);
                        }
                        gone => gone,
                    }
                }
            };
            self.ran[index] = Some(ran);
        }
        range.end
    }

    /// A done step's result, read from the run's answer; a walk down the
    /// tree goes on in later runs. `Err` when the answer is missing what
    /// the step's registers name.
    #[expect(clippy::too_many_lines, reason = "one arm per operation")]
    fn found(
        &self,
        index: usize,
        (emitted, given): (&Emitted, Given<'_>),
        outcome: &Outcome,
    ) -> Result<Ran, Error> {
        let regs = emitted
            .regs
            .as_ref()
            .ok_or(Error::MissingResult(emitted.status.id()))?;
        Ok(Ran::Done(match (regs, given) {
            (
                Regs::Related {
                    nodes,
                    absents,
                    window,
                    table,
                    ..
                },
                Given::Fill(fill),
            ) => Found::Related(Related {
                nodes: claimed(outcome, (*nodes, *absents), fill)?,
                window: match window {
                    Some(window) => Some(outcome.get(*window)?)
                        .filter(|&w| w != 0)
                        .map(|w| w as isize),
                    None => None,
                },
                table: table_answered(outcome, *table)?,
            }),
            (Regs::Descendants(registers), Given::Walk(walk)) => {
                return self.walked(index, &walk, outcome, registers);
            }
            (
                Regs::Caret {
                    caret,
                    point,
                    at_text_start,
                },
                _,
            ) => Found::Caret(caret_found(outcome, caret, *point, *at_text_start)?),
            (Regs::ReadText(units), _) => Found::ReadText(units_found(outcome, units)?),
            (Regs::ReadRange(text), _) => Found::ReadRange(outcome.get_utf16(*text)?),
            (Regs::Select(_), _) => Found::Select,
            (
                Regs::Compare {
                    changes,
                    selected,
                    selection_text,
                    held,
                    previous_text,
                },
                _,
            ) => {
                let text = |reg| -> Result<Vec<u16>, Error> {
                    Ok(string_of(outcome, reg)?.encode_utf16().collect())
                };
                Found::CompareSelection(Compared {
                    changes: read_changes(outcome, changes.0, changes.1)?,
                    selection_text: outcome
                        .get(*selected)?
                        .then(|| text(*selection_text))
                        .transpose()?,
                    previous_text: outcome
                        .get(*held)?
                        .then(|| text(*previous_text))
                        .transpose()?,
                })
            }
            (Regs::WaitCheck { check, last, .. }, _) => {
                let answer = check.read(outcome)?;
                let reads_after = *last || answer.caret_moved || answer.selection_moved;
                Found::WaitCheck(super::Checked {
                    answer,
                    reads_after,
                })
            }
            (
                Regs::Visible {
                    range,
                    text,
                    at_start,
                },
                _,
            ) => Found::Visible(VisibleFound {
                range: outcome
                    .get(*range)?
                    .ok_or(Error::MissingResult(range.id()))?,
                text: outcome.get_utf16(*text)?,
                at_text_start: outcome.get(*at_start)?,
            }),
            (
                Regs::Move {
                    from, to, moved, ..
                },
                _,
            ) => Found::Move(MoveFound {
                from: outcome.get(*from)?.ok_or(Error::MissingResult(from.id()))?,
                to: outcome.get(*to)?.ok_or(Error::MissingResult(to.id()))?,
                moved: outcome.get(*moved)?,
            }),
            (Regs::Whole { range, text }, _) => Found::ReadText(UnitsFound {
                moved: 0,
                units: vec![crate::text::UnitText {
                    range: outcome
                        .get(*range)?
                        .ok_or(Error::MissingResult(range.id()))?,
                    text: outcome.get_utf16(*text)?,
                    offset: 0,
                    language: None,
                }],
                ended: false,
                runs: Vec::new(),
                unsupported: None,
            }),
            (Regs::FindText { to_ends, texts }, _) => {
                let matches = outcome
                    .get(*to_ends)?
                    .into_iter()
                    .zip(outcome.get(*texts)?)
                    .map(|found| match found {
                        (Value::Int(to_end), Value::String(text)) => Ok(TextMatch {
                            to_end: to_end.unsigned_abs(),
                            text: text.encode_utf16().collect(),
                        }),
                        _ => Err(Error::MissingResult(to_ends.id())),
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                Found::FindText(matches)
            }
            (Regs::Colours(registers), Given::Colours(read_at)) => {
                let Op::ReadColours { screen, top, .. } = &self.step(index).op else {
                    return Err(Error::MissingResult(emitted.status.id()));
                };
                let size = match super::calls::colour_size(self, index, (*screen, *top)) {
                    Ok(size) => size,
                    Err(ran) => return Ok(ran),
                };
                // The rows were read at the last read's size: at another,
                // the colour programs read this one's.
                if size != read_at {
                    return Ok(super::calls::run(self, index));
                }
                let width = usize::try_from(size.1).unwrap_or(usize::MAX);
                Found::Colours(super::colours::finish(
                    outcome,
                    *registers,
                    (size.0, width),
                )?)
            }
            (Regs::Locate(rectangles), _) => {
                Found::Locate(match outcome.get(*rectangles)?.as_slice() {
                    [Value::Double(left), Value::Double(top), ..] => Some((*left, *top)),
                    [Value::Rect { x, y, .. }, ..] => Some((f64::from(*x), f64::from(*y))),
                    _ => None,
                })
            }
            _ => return Err(Error::MissingResult(emitted.status.id())),
        }))
    }

    /// A walk's answer, its first run read from `outcome` and the rest run
    /// on.
    fn walked(
        &self,
        index: usize,
        walk: &Descend<'_>,
        outcome: &Outcome,
        registers: &RunRegisters,
    ) -> Result<Ran, Error> {
        let starts = self.walk_starts(index, outcome, registers)?;
        let mut walking = Walk::new(walk, starts);
        let taken = walking.starts();
        // A first element whose children fill no run is read by calls.
        if walking.take_run(outcome, registers, &taken)?
            && let Err(error) = walking.call_run(self.segment.uia)
        {
            return Ok(failed(&error));
        }
        Ok(match walking.run(self.segment.uia, Reads::Program) {
            Ok(()) => Ran::Done(Found::Descendants(walking.finish())),
            Err(error) => failed(&error),
        })
    }

    /// The elements a walk's first run started from.
    fn walk_starts(
        &self,
        index: usize,
        outcome: &Outcome,
        registers: &RunRegisters,
    ) -> Result<Vec<IUIAutomationElement>, Error> {
        let count = match self.step(index).op.subject() {
            Some(Subject::Result(place)) if !self.before(*place) => {
                match self.named(*place, index) {
                    Some(Ran::Done(found)) => nodes_of(found).map_or(0, |nodes| nodes.len()),
                    _ => 0,
                }
            }
            _ => 1,
        };
        let queue = outcome.get(registers.queue())?;
        queue
            .into_iter()
            .take(count)
            .map(|value| match value {
                Value::Element(element) => Ok(element),
                _ => Err(Error::MissingResult(registers.queue().id())),
            })
            .collect()
    }

    /// The program's plan for `range`: each step lowered, or its outcome.
    fn plan(&self, range: &Range<usize>) -> Plan<'a> {
        let mut planner = Planner {
            state: self,
            range: range.clone(),
            imports: Vec::new(),
            pre: Vec::with_capacity(range.len()),
            texts: Vec::new(),
            held_points: Vec::new(),
        };
        let mut key = Vec::with_capacity(range.len());
        let mut given = Vec::with_capacity(range.len());
        for index in range.clone() {
            match planner.lower(index) {
                Ok((low, gate, giving)) => {
                    key.push((low, gate));
                    given.push(giving);
                    planner.pre.push(None);
                }
                Err(ran) => {
                    given.push(Given::Nothing);
                    key.push((Low::Unemitted, Gate::OPEN));
                    planner.pre.push(Some(ran));
                }
            }
        }
        Plan {
            key,
            imports: planner.imports,
            given,
            pre: planner.pre,
        }
    }
}

/// A caret read's answer.
fn caret_found(
    outcome: &Outcome,
    caret: &CaretRegisters,
    point: Reg<kind::TextRange>,
    at_text_start: Reg<kind::Bool>,
) -> Result<CaretFound, Error> {
    let selection = if outcome.get(caret.has_selection)? {
        let range = outcome
            .get(caret.selection)?
            .ok_or(Error::MissingResult(caret.selection.id()))?;
        Some((range, outcome.get(at_text_start)?))
    } else {
        None
    };
    Ok(CaretFound {
        caret: outcome
            .get(point)?
            .ok_or(Error::MissingResult(point.id()))?,
        // With no selection at all the program leaves both unset.
        reported: selection.is_some() || outcome.get(caret.collapsed)?,
        selection,
    })
}

/// An outcome decided before the program, again.
fn clone_unrun(ran: &Ran) -> Ran {
    match ran {
        Ran::Unrun(unrun) => Ran::Unrun(*unrun),
        Ran::Gone => Ran::Gone,
        Ran::TimedOut => Ran::TimedOut,
        _ => Ran::Failed,
    }
}

/// Whether a run failed before anything ran: an import refused, an
/// instruction unsupported, the API missing, or bytecode the platform
/// cannot read.
fn could_not_run(error: &Error) -> bool {
    match error {
        Error::Import(_) | Error::Unsupported(_) | Error::Unavailable(_) => true,
        Error::Failed(failure) => failure.status == RunStatus::MalformedBytecode,
        _ => false,
    }
}

/// The elements of a list the program filled, each claimed with its absent
/// record.
fn claimed(
    outcome: &Outcome,
    (nodes, absents): (Reg<kind::Array>, Reg<kind::Array>),
    fill: &Fill<Node>,
) -> Result<Vec<Cached<Node>>, Error> {
    let absents = outcome.get(absents)?;
    outcome
        .get(nodes)?
        .into_iter()
        .zip(absents)
        .map(|(node, absent)| match (node, absent) {
            (Value::Element(element), Value::Int(bits)) => {
                Ok(fill.claim(element, absent_record(bits)))
            }
            _ => Err(Error::MissingResult(nodes.id())),
        })
        .collect()
}

/// A unit read's answer.
fn units_found(
    outcome: &Outcome,
    (units, runs, character): &UnitsRegs,
) -> Result<UnitsFound, Error> {
    let (moved, units, ended) = units.read(outcome)?;
    let (runs, unsupported) = match runs {
        Some(runs) => {
            let learns = !character && !units[0].text.is_empty();
            (
                runs.read(outcome)?,
                learns.then(|| runs.unsupported(outcome)).transpose()?,
            )
        }
        None => (Vec::new(), None),
    };
    Ok(UnitsFound {
        moved,
        units,
        ended,
        runs,
        unsupported,
    })
}
