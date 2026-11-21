//! The program builder: a run of a segment's steps lowered to a shape
//! that holds no object of UI Automation's ([`Low`]), each object an import
//! by its place, so that a run of the same shape binds a kept program to
//! its own imports ([`ProgramCache`]); the shape emitted, each step in a
//! try block with a status of its own; and its answer read back.

use std::ops::Range;

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextRange, TextUnit_Character, UIA_ControllerForPropertyId,
};

use verbatim_model::batch::{Check, PointOf, Relation, View};
use verbatim_uia::text::Endpoint;
use verbatim_uia::{Cached, Fill, Node};

use super::calls::caret_point;
use super::walk::{Reads, RunRegisters, Walk, emit_run};
use super::{
    CaretFound, Compared, Cut, Defect, Ends, Found, Fused, Input, Object, Point, Prior, Ran,
    Related, State, UnitsFound, UnitsRead, Unrun, failed,
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

/// A step's status before it runs: a run that ends early leaves it so.
const NOT_REACHED: i32 = -1;
/// A step's status when its gate did not hold.
const SKIPPED: i32 = -2;
/// A step's status when its list was longer than a program reads: it runs
/// by calls, a size-limit path and no fallback.
const CUT: i32 = -3;

/// The most controller-for elements a program reads; a longer relation is
/// read by calls.
const CONTROLLED_PER_RUN: u32 = 16;

/// The instructions every program spends outside its steps.
pub(super) const PROLOGUE: u32 = 60;

/// What the step at `index` may execute at most, from its caps, after
/// `prior` instructions of its program, pinned by
/// `crates/mockapp/tests/instruction_limit.rs`.
fn worst(state: &State<'_, '_>, index: usize, prior: u32) -> u32 {
    let input = &state.segment.steps[index].input;
    match input {
        Input::Answered(_) | Input::Ancestors(_) => 0,
        Input::Read { fill, .. } => {
            let (setup, each) = RemoteCache::costs(*fill);
            10 + setup + each
        }
        Input::Related { relation, fill, .. } => {
            let (setup, each) = RemoteCache::costs(*fill);
            match relation {
                Relation::ControllerFor => 30 + setup + CONTROLLED_PER_RUN * (8 + each),
                // The nearest window's walk, at most the tree's depth, and a
                // cell's table.
                _ => 80 + 8 * verbatim_model::batch::MAX_DEPTH + setup + each,
            }
        }
        Input::Descendants { walk, .. } => {
            let starts = starts_cap(state, index);
            super::walk::run_cost(walk, starts, room(walk, starts, prior))
        }
        Input::Caret { .. } => 60,
        Input::ReadText { read, at, .. } => {
            let formats = if read.formats.is_empty() { 0 } else { 7_700 };
            80 + point_cost(at) + 24 * read.count + formats
        }
        Input::ReadRange { ends, .. } | Input::Select { ends, .. } => {
            40 + point_cost(&ends.start)
                + point_cost(&ends.end)
                + ends
                    .through
                    .map_or(0, |(start, end)| point_cost(&start) + point_cost(&end))
        }
        Input::CompareSelection { .. } => 120,
        Input::Locate { at, .. } => 20 + point_cost(at),
        Input::WaitCheck(_) => WAIT_CHECK,
    }
}

/// What a caret wait's check may execute at most: its caret read twice,
/// three comparisons with the starting point, its line and word, and its
/// gate's test.
const WAIT_CHECK: u32 = 120;

/// What finding a point may execute at most.
fn point_cost(point: &Point<'_>) -> u32 {
    match point {
        Point::From(TextFrom::After { counts, .. }) => {
            20 + 12 * u32::try_from(counts.len()).unwrap_or(u32::MAX / 24)
        }
        _ => 40,
    }
}

/// The most starts the walk at `index` may have: one node, an earlier
/// list's nodes, or a relation of the program's, at most what a program
/// reads of one.
fn starts_cap(state: &State<'_, '_>, index: usize) -> u32 {
    let Input::Descendants { subject, .. } = &state.segment.steps[index].input else {
        return 0;
    };
    match *subject {
        Object::Held { .. } | Object::Item(..) => 1,
        Object::Result(place) => match state.named(place, index) {
            Some(Ran::Done(found)) => {
                nodes_of(found).map_or(1, |nodes| u32::try_from(nodes.len()).unwrap_or(u32::MAX))
            }
            _ => CONTROLLED_PER_RUN,
        },
    }
}

/// How many children a walk's first run reads after `prior` instructions
/// of its program: as many as the rest of the limit holds, at most
/// [`CHILDREN_PER_RUN`] and the walk's node cap.
fn room(walk: &super::Descend<'_>, starts: u32, prior: u32) -> u32 {
    super::walk::run_room(walk, starts, LIMIT.saturating_sub(prior))
}

/// Whether the step at `index` runs by calls whatever the segment's mode.
fn by_calls(state: &State<'_, '_>, index: usize) -> bool {
    let step = &state.segment.steps[index];
    let proxied = |object: &Object<'_>| match *object {
        Object::Result(place) | Object::Item(place, _) => match state.named(place, index) {
            Some(Ran::Done(Found::Read(node))) => node.served_only_by_uia_proxies() == Some(true),
            Some(Ran::Done(Found::Related(related))) => related
                .nodes
                .iter()
                .any(|node| node.served_only_by_uia_proxies() == Some(true)),
            _ => false,
        },
        Object::Held { .. } => false,
    };
    step.calls_only
        || match &step.input {
            Input::Ancestors(_) => true,
            Input::Descendants { walk, subject } => walk.view == View::Control || proxied(subject),
            Input::Read { subject, .. } | Input::Related { subject, .. } => proxied(subject),
            _ => false,
        }
}

/// Whether a program holding the steps from `start` cannot take the step
/// at `index`: it names a node of an earlier list of the program by its
/// place, which a program would have to check against the list's length.
fn names_item_inside(state: &State<'_, '_>, start: usize, index: usize) -> bool {
    let object = match &state.segment.steps[index].input {
        Input::Read { subject, .. }
        | Input::Related { subject, .. }
        | Input::Descendants { subject, .. } => *subject,
        _ => return false,
    };
    matches!(object, Object::Item(place, _) if state.index(place, index).is_some_and(|at| at >= start))
}

/// What runs next from step `at`: a program of the steps from it, or one
/// step by calls.
pub(super) fn cut(state: &State<'_, '_>, at: usize) -> Cut {
    let steps = &state.segment.steps;
    if matches!(steps[at].input, Input::Answered(_)) || by_calls(state, at) {
        return Cut::Calls(at);
    }
    let mut end = at;
    let mut cost = PROLOGUE;
    while end < steps.len() {
        let input = &steps[end].input;
        let step = worst(state, end, cost);
        let walk_fits = match input {
            Input::Descendants { walk, .. } => room(walk, starts_cap(state, end), cost) > 0,
            _ => true,
        };
        if end > at
            && (by_calls(state, end)
                || names_item_inside(state, at, end)
                || !walk_fits
                || cost + step > LIMIT)
        {
            break;
        }
        cost += step;
        end += 1;
        if matches!(input, Input::Select { .. } | Input::Descendants { .. }) {
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
    Read(Src, FillKey),
    Related(Src, isize, Relation, FillKey),
    Descendants(Src, (u32, bool, bool, u32), FillKey),
    Caret(usize),
    ReadText(usize, Pt, [i32; 9]),
    ReadRange(usize, LowEnds, i32),
    Compare(usize, Before, After, i32),
    Select(usize, LowEnds),
    Locate(usize, Pt),
    /// The element, the starting caret's and selection's ends as imports
    /// and whether each is its range's end, whether the word is read,
    /// whether it is the check at the bound, and the text read's limit.
    WaitCheck(usize, [(usize, bool); 3], bool, bool, i32),
}

/// A step's condition on a step of the same program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Gate {
    /// The steps of the program it names, by place in the program, whose
    /// statuses must be done: at most four.
    names: [Option<usize>; 4],
    /// Its found-or-not test on a step of the program, and whether it is
    /// negated.
    found: Option<(usize, bool)>,
}

/// A program's shape, its cache's key.
type Key = Vec<(Low, Gate)>;

/// A program planned: its shape, its imports in order, and each step's
/// outcome decided before it ran.
struct Plan {
    key: Key,
    imports: Vec<Import>,
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
}

impl<'p> Planner<'p, '_, '_> {
    fn import(&mut self, import: Import) -> usize {
        self.imports.push(import);
        self.imports.len() - 1
    }

    /// The program's place of the segment's `place`, named by the step at
    /// `by`, when it is in the program.
    fn here(&self, place: verbatim_model::batch::Place, by: usize) -> Option<usize> {
        self.state
            .index(place, by)
            .filter(|index| self.range.contains(index))
            .map(|index| index - self.range.start)
    }

    fn done(&self, place: verbatim_model::batch::Place, by: usize) -> Result<&'p Found, Ran> {
        let state: &'p State<'_, '_> = self.state;
        match state.named(place, by) {
            Some(Ran::Done(found)) => Ok(found),
            Some(_) => Err(Ran::Unrun(Unrun::Skipped)),
            None => Err(Defect("a step names one not before it").ran()),
        }
    }

    fn src(&mut self, object: &Object<'_>, by: usize, many: bool) -> Result<(Src, isize), Ran> {
        match *object {
            Object::Held { element, window } => Ok((
                Src::Import(self.import(Import::Element(element.clone()))),
                window,
            )),
            Object::Result(place) => {
                if let Some(here) = self.here(place, by) {
                    return Ok((Src::Here(here), 0));
                }
                match self.done(place, by)? {
                    Found::Read(node) => Ok((
                        Src::Import(self.import(Import::Element(node.live().element().clone()))),
                        node.read().role.hwnd,
                    )),
                    found if many => {
                        let nodes = nodes_of(found)
                            .ok_or_else(|| Defect("a walk starts from no list").ran())?;
                        let imports = nodes
                            .into_iter()
                            .map(|node| self.import(Import::Element(node.live().element().clone())))
                            .collect();
                        Ok((Src::Imports(imports), 0))
                    }
                    _ => Err(Defect("a step names no node").ran()),
                }
            }
            Object::Item(place, item) => {
                let found = self.done(place, by)?;
                let node = nodes_of(found)
                    .ok_or_else(|| Defect("an item of no list").ran())?
                    .get(item as usize)
                    .copied()
                    .ok_or(Ran::Unrun(Unrun::NotThere(
                        verbatim_model::batch::Missing::Object,
                    )))?;
                let window = node.read().role.hwnd;
                let element = node.live().element().clone();
                Ok((Src::Import(self.import(Import::Element(element))), window))
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

    fn found_point(&mut self, point: &FoundPoint) -> Pt {
        let (range, end, collapsed) = self.held(
            &point.range,
            point.endpoint == Endpoint::End,
            point.collapsed,
        );
        Pt::At(range, end, collapsed)
    }

    fn point(&mut self, point: &Point<'_>, by: usize) -> Result<Pt, Ran> {
        Ok(match *point {
            Point::From(TextFrom::Caret) => Pt::Caret,
            Point::From(TextFrom::SelectionStart) => Pt::SelectionStart,
            Point::From(TextFrom::SelectionEnd) => Pt::SelectionEnd,
            Point::From(TextFrom::Start) => Pt::Start,
            Point::From(TextFrom::End) => Pt::End,
            Point::From(TextFrom::At(position)) => {
                let (range, end, collapsed) = self.held(
                    position.range,
                    position.endpoint == Endpoint::End,
                    position.collapsed,
                );
                Pt::At(range, end, collapsed)
            }
            Point::From(TextFrom::After {
                from,
                prefix,
                counts,
            }) => Pt::After {
                from: self.held(from.range, from.endpoint == Endpoint::End, from.collapsed),
                prefix: prefix.to_vec(),
                counts: counts.to_vec(),
            },
            Point::Of(place, which) => {
                if let Some(here) = self.here(place, by) {
                    return Ok(Pt::Here(here, which));
                }
                match (self.done(place, by)?, which) {
                    (Found::Caret(caret), _) => {
                        let point = caret_point(caret, which);
                        self.found_point(&point)
                    }
                    (Found::ReadText(read), PointOf::ChunkStart) => {
                        let range = read.units[0].range.clone();
                        Pt::At(self.import(Import::TextRange(range)), false, false)
                    }
                    _ => return Err(Defect("a point of no text read").ran()),
                }
            }
        })
    }

    fn ends(&mut self, ends: &Ends<'_>, by: usize) -> Result<LowEnds, Ran> {
        Ok(LowEnds {
            start: self.point(&ends.start, by)?,
            end: self.point(&ends.end, by)?,
            through: match &ends.through {
                Some((start, end)) => Some((self.point(start, by)?, self.point(end, by)?)),
                None => None,
            },
        })
    }

    fn caret_of(
        &self,
        place: verbatim_model::batch::Place,
        by: usize,
    ) -> Result<&'p CaretFound, Ran> {
        match self.done(place, by)? {
            Found::Caret(caret) => Ok(caret),
            _ => Err(Defect("a comparison names no caret read").ran()),
        }
    }

    /// A comparison's two selections, by the program's registers or as
    /// imports.
    fn compared(
        &mut self,
        before: &Prior<'_>,
        after: verbatim_model::batch::Place,
        index: usize,
    ) -> Result<(Before, After), Ran> {
        let before = match *before {
            Prior::Baseline(position) => {
                let (range, end, collapsed) = self.held(
                    position.range,
                    position.endpoint == Endpoint::End,
                    position.collapsed,
                );
                Before::Held(range, end, collapsed)
            }
            Prior::SelectionOf(place) => {
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
    fn wait_check(&mut self, check: &super::WaitCheck<'_>) -> Low {
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
        Low::WaitCheck(element, points, check.word, check.last, check.max_text)
    }

    /// The step at `index` lowered, with its gate inside the program; its
    /// outcome when the program does not run it.
    fn lower(&mut self, index: usize) -> Result<(Low, Gate), Ran> {
        let step: &Fused<'_> = &self.state.segment.steps[index];
        let mut gate = Gate {
            names: [None; 4],
            found: None,
        };
        // References outside the program must be done; inside it, the
        // program tests their statuses.
        for (slot, place) in super::references(&step.input).into_iter().enumerate() {
            match self.here(place, index) {
                Some(here) => gate.names[slot.min(3)] = Some(here),
                None => {
                    self.done(place, index)?;
                }
            }
        }
        if let Some(condition) = step.when {
            match (self.here(condition.on, index), condition.check) {
                (Some(here), Check::Found) => gate.found = Some((here, condition.negated)),
                (Some(_), Check::Matched) => {
                    return Err(Defect("a condition in a program tests a match").ran());
                }
                (None, _) => match self.state.holds(index) {
                    Ok(true) => {}
                    Ok(false) => return Err(Ran::Unrun(Unrun::Skipped)),
                    Err(defect) => return Err(defect.ran()),
                },
            }
        }
        let text = |planner: &mut Self, target: &crate::text::TextTarget<'_>| {
            planner.import(Import::Element(target.element.clone()))
        };
        let low = match &step.input {
            Input::Answered(unrun) => return Err(Ran::Unrun(*unrun)),
            Input::Ancestors(_) => return Err(Defect("ancestors in a program").ran()),
            Input::Read { subject, fill } => {
                Low::Read(self.src(subject, index, false)?.0, fill_key(fill))
            }
            Input::Related {
                subject,
                relation,
                fill,
            } => {
                let (src, window) = self.src(subject, index, false)?;
                Low::Related(src, window, *relation, fill_key(fill))
            }
            Input::Descendants { subject, walk } => {
                let (src, _) = self.src(subject, index, true)?;
                let prior = (self.range.start..index).fold(PROLOGUE, |prior, earlier| {
                    prior + worst(self.state, earlier, prior)
                });
                let room = room(walk, starts_cap(self.state, index), prior);
                Low::Descendants(
                    src,
                    (walk.depth, walk.prunes, walk.include_subject, room),
                    fill_key(walk.fill),
                )
            }
            Input::Caret { text: target } => Low::Caret(text(self, target)),
            Input::ReadText {
                text: target,
                at,
                read,
            } => {
                let element = text(self, target);
                Low::ReadText(element, self.point(at, index)?, read_key(read))
            }
            Input::ReadRange {
                text: target,
                ends,
                max_text,
            } => {
                let element = text(self, target);
                Low::ReadRange(element, self.ends(ends, index)?, *max_text)
            }
            Input::Select { text: target, ends } => {
                let element = text(self, target);
                Low::Select(element, self.ends(ends, index)?)
            }
            Input::CompareSelection {
                text: target,
                before,
                after,
                max_text,
            } => {
                let element = text(self, target);
                let (before, after) = self.compared(before, *after, index)?;
                Low::Compare(element, before, after, *max_text)
            }
            Input::Locate { text: target, at } => {
                let element = text(self, target);
                Low::Locate(element, self.point(at, index)?)
            }
            Input::WaitCheck(check) => self.wait_check(check),
        };
        Ok((low, gate))
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
fn read_key(read: &UnitsRead) -> [i32; 9] {
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
        read.max_text,
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
    Read {
        node: Reg<kind::Element>,
        absent: Reg<kind::Int>,
    },
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
        edges: [Reg<kind::Bool>; 6],
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
    Select(Reg<kind::Bool>),
    Locate(Reg<kind::Array>),
    WaitCheck {
        check: Box<crate::watch::CheckRegisters>,
        /// Whether the steps gated on it read.
        reads_after: Reg<kind::Bool>,
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
    state: &State<'_, '_>,
    range: &Range<usize>,
    key: &Key,
    imports: &[Import],
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
    let mut emitted: Vec<Option<Emitted>> = Vec::with_capacity(key.len());
    for (place, ((low, gate), status)) in key.iter().zip(&statuses).enumerate() {
        let Some(status) = *status else {
            emitted.push(None);
            continue;
        };
        let input = &state.segment.steps[range.start + place].input;
        inject(b, injected, Injected::Stops(place));
        inject(b, injected, Injected::Overruns(place));
        let regs = {
            let e = Emit {
                c: &c,
                imported: &imported,
                emitted: &emitted,
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
                                let emitted = e.step(b, low, input);
                                let zero = b.int(0);
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

/// The emitter's context: the constants, the imports, and the steps
/// emitted before.
struct Emit<'e> {
    c: &'e Constants,
    imported: &'e [Imported],
    emitted: &'e [Option<Emitted>],
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

    /// Whether every status of `statuses` is done or skipped.
    fn finished(&self, b: &mut Builder, statuses: &[Reg<kind::Int>]) -> Reg<kind::Bool> {
        let all = b.new_bool(true);
        let skipped = b.int(SKIPPED);
        for &status in statuses {
            let done = b.equal(status, self.c.zero);
            let passed = b.equal(status, skipped);
            let either = b.or(done, passed);
            b.and_assign(all, either);
        }
        all
    }

    /// Whether the gate holds: every step of the program it names done,
    /// then its found-or-not test, a failed step's taken as not found, a
    /// skipped one skipping it.
    fn gate(&self, b: &mut Builder, gate: &Gate) -> Reg<kind::Bool> {
        let ok = b.new_bool(true);
        for &place in gate.names.iter().flatten() {
            let done = b.equal(self.status(place), self.c.zero);
            b.and_assign(ok, done);
        }
        let Some((place, negated)) = gate.found else {
            return ok;
        };
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
        b.and_assign(ok, runs);
        ok
    }

    /// The element a node source names: one node.
    fn node(&self, b: &mut Builder, src: &Src) -> Reg<kind::Element> {
        match src {
            Src::Import(index) => self.element(*index),
            Src::Here(place) => match self.regs(*place) {
                Regs::Read { node, .. } => *node,
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
    fn step(&self, b: &mut Builder, low: &Low, input: &Input<'_>) -> Regs {
        match (low, input) {
            (Low::Read(src, _), Input::Read { fill, .. }) => {
                let node = self.node(b, src);
                let cache = RemoteCache::new(b, *fill);
                let absent = cache.populate(b, node);
                Regs::Read {
                    node: b.add_to_results(node),
                    absent: b.add_to_results(absent),
                }
            }
            (Low::Related(src, window, relation, _), Input::Related { fill, .. }) => {
                let node = self.node(b, src);
                Self::related(b, node, (*window, *relation), fill)
            }
            (Low::Descendants(src, (.., room), _), Input::Descendants { walk, .. }) => {
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
                        Regs::Read { node, .. } => b.array_append(queue, *node),
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
                    walk,
                    (queue, None),
                    *room,
                    walk.include_subject,
                ))
            }
            (Low::Caret(text), _) => self.caret(b, *text),
            (Low::ReadText(text, at, _), Input::ReadText { read, .. }) => {
                self.read_text(b, (*text, at), read)
            }
            (Low::ReadRange(text, ends, max), _) => {
                let range = self.range_between(b, *text, ends);
                let max = b.int(*max);
                let read = b.text_range_get_text(range, max);
                Regs::ReadRange(b.add_to_results(read))
            }
            (Low::Select(text, ends), _) => {
                let range = self.range_between(b, *text, ends);
                let selected = b.new_bool(false);
                let selected = b.add_to_results(selected);
                // A refused selection is the step's refusal.
                b.try_catch(
                    |b| {
                        b.text_range_select(range);
                        let yes = b.bool(true);
                        b.set(selected, yes);
                    },
                    |_, _| {},
                );
                Regs::Select(selected)
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
            _ => unreachable_import(),
        }
    }

    /// Emits [`Input::Related`] from `node`, whose own window is `window`:
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

    /// Emits [`Input::WaitCheck`]: the check of the text of the element
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
        }
    }

    /// Emits [`Input::Caret`]: the caret and the selection as the classic
    /// reads find them, a collapsed copy at the caret, and whether each
    /// point is at the text's start or end.
    fn caret(&self, b: &mut Builder, text: usize) -> Regs {
        let element = self.element(text);
        let pattern = b.get_text_pattern(element, false);
        let caret = emit_caret(b, self.c, pattern);
        let point = self.c.collapsed(b, caret.caret);
        let point = b.add_to_results(point);
        let document = b.text_pattern_get_document_range(pattern);
        let edges = [(); 6].map(|()| {
            let edge = b.new_bool(false);
            b.add_to_results(edge)
        });
        let c = self.c;
        let at = |b: &mut Builder,
                  range,
                  end,
                  (start_edge, end_edge): (Reg<kind::Bool>, Reg<kind::Bool>)| {
            let order = b.text_range_compare_endpoints(range, end, document, c.start);
            let at_start = b.equal(order, c.zero);
            b.set(start_edge, at_start);
            let order = b.text_range_compare_endpoints(range, end, document, c.end);
            let at_end = b.equal(order, c.zero);
            b.set(end_edge, at_end);
        };
        at(b, point, c.start, (edges[0], edges[1]));
        b.if_(caret.has_selection, |b| {
            at(b, caret.selection, c.start, (edges[2], edges[3]));
            at(b, caret.selection, c.end, (edges[4], edges[5]));
        });
        Regs::Caret {
            caret,
            point,
            edges,
        }
    }

    /// Emits a collapsed range at `point` in the text of `text`'s element.
    fn point(&self, b: &mut Builder, text: usize, point: &Pt) -> Reg<kind::TextRange> {
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
        let document = |b: &mut Builder| {
            let element = self.element(text);
            let pattern = b.get_text_pattern(element, false);
            b.text_pattern_get_document_range(pattern)
        };
        match point {
            Pt::Caret | Pt::SelectionStart | Pt::SelectionEnd => {
                let caret = if let Some(caret) = self.caret.get() {
                    caret
                } else {
                    let element = self.element(text);
                    let pattern = b.get_text_pattern(element, false);
                    let caret = emit_caret(b, c, pattern);
                    self.caret.set(Some(caret));
                    caret
                };
                self.caret_point(b, &caret, point_of(point))
            }
            Pt::Start => {
                let document = document(b);
                c.collapsed(b, document)
            }
            Pt::End => {
                let document = document(b);
                collapsed_to_end(b, document)
            }
            Pt::At(range, end, collapsed) => held(b, (*range, *end, *collapsed)),
            Pt::After {
                from,
                prefix,
                counts,
            } => {
                let base = held(b, *from);
                emit_after(b, c, base, (prefix, counts))
            }
            Pt::Here(place, which) => match self.regs(*place) {
                Regs::Caret { caret, point, .. } => match which {
                    PointOf::Caret => c.collapsed(b, *point),
                    _ => self.caret_point(b, caret, *which),
                },
                Regs::ReadText(units) => c.collapsed(b, units.0.first.0),
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
            PointOf::Caret | PointOf::ChunkStart => at_caret(b),
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

    /// Emits [`Input::ReadText`], as the text reads read units, and the
    /// first unit's formatting.
    fn read_text(&self, b: &mut Builder, (text, at): (usize, &Pt), read: &UnitsRead) -> Regs {
        let c = Constants {
            max_text: b.int(read.max_text),
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

    /// Emits [`Input::CompareSelection`].
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
        .map(|index| (index - range.start, state.segment.steps[index].place.0))
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
    Skipped,
    NotReached,
    Cut,
    Failed(i32),
}

impl Ended {
    fn of(status: i32) -> Self {
        match status {
            0 => Self::Done,
            SKIPPED => Self::Skipped,
            NOT_REACHED => Self::NotReached,
            CUT => Self::Cut,
            failed => Self::Failed(failed),
        }
    }
}

impl State<'_, '_> {
    /// Runs the steps of `range` as one program, then what the fallback
    /// rule reruns by calls; answers where the next step to run is.
    pub(super) fn program(&mut self, range: Range<usize>) -> usize {
        let plan = self.plan(&range);
        let program_holds_act = range
            .clone()
            .any(|index| matches!(self.segment.steps[index].input, Input::Select { .. }));
        if plan.key.iter().all(|(low, _)| *low == Low::Unemitted) {
            for (index, pre) in range.clone().zip(plan.pre) {
                self.ran[index] = pre;
            }
            return range.end;
        }
        let Plan { key, imports, pre } = plan;
        let injected = injected(self, &range);
        let (operation, emitted) = if injected.is_some() {
            // A program a test makes fail is built apart, never kept.
            let mut b = Builder::new();
            let emitted = build(&mut b, self, &range, &key, &imports, injected);
            (b.finish(), emitted)
        } else {
            PROGRAMS.with(|programs| {
                programs.operation(
                    key.clone(),
                    || imports.clone(),
                    |b| build(b, self, &range, &key, &imports, None),
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
            Ok(outcome) => self.take(range, &pre, &emitted, outcome, None),
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
                self.take(range, &pre, &emitted, &failure.partial, Some(error))
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
        pre: &[Option<Ran>],
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
            let ran = match Ended::of(status) {
                Ended::Done => match self.found(index, emitted, outcome) {
                    Ok(ran) => ran,
                    Err(why) => return rerun(self, &why),
                },
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
                        Ran::Failed => return rerun(self, &why),
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
    fn found(&self, index: usize, emitted: &Emitted, outcome: &Outcome) -> Result<Ran, Error> {
        let regs = emitted
            .regs
            .as_ref()
            .ok_or(Error::MissingResult(emitted.status.id()))?;
        let input = &self.segment.steps[index].input;
        Ok(Ran::Done(match (regs, input) {
            (Regs::Read { node, absent }, Input::Read { fill, .. }) => {
                let element = outcome.get(*node)?.ok_or(Error::MissingResult(node.id()))?;
                Found::Read(fill.claim(element, absent_record(outcome.get(*absent)?)))
            }
            (
                Regs::Related {
                    nodes,
                    absents,
                    window,
                    table,
                    ..
                },
                Input::Related { fill, .. },
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
            (Regs::Descendants(registers), Input::Descendants { subject, walk }) => {
                return self.walked(index, (subject, walk), outcome, registers);
            }
            (
                Regs::Caret {
                    caret,
                    point,
                    edges,
                },
                _,
            ) => Found::Caret(caret_found(outcome, caret, *point, edges)?),
            (Regs::ReadText(units), _) => Found::ReadText(units_found(outcome, units)?),
            (Regs::ReadRange(text), _) => {
                Found::ReadRange(string_of(outcome, *text)?.encode_utf16().collect())
            }
            (Regs::Select(selected), _) => {
                if !outcome.get(*selected)? {
                    return Ok(Ran::Unrun(Unrun::Refused(
                        verbatim_model::batch::StepRefusal::Unsupported,
                    )));
                }
                Found::Select
            }
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
            (Regs::WaitCheck { check, .. }, Input::WaitCheck(input)) => {
                let answer = check.read(outcome)?;
                let reads_after = input.last || answer.caret_moved || answer.selection_moved;
                Found::WaitCheck(super::Checked {
                    answer,
                    reads_after,
                })
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
        (subject, walk): (&Object<'_>, &super::Descend<'_>),
        outcome: &Outcome,
        registers: &RunRegisters,
    ) -> Result<Ran, Error> {
        let starts = self.walk_starts(index, subject, outcome, registers)?;
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
        subject: &Object<'_>,
        outcome: &Outcome,
        registers: &RunRegisters,
    ) -> Result<Vec<IUIAutomationElement>, Error> {
        let count = match *subject {
            Object::Held { .. } | Object::Item(..) => 1,
            Object::Result(place) => match self.named(place, index) {
                Some(Ran::Done(Found::Read(_))) => 1,
                Some(Ran::Done(found)) => nodes_of(found).map_or(0, |nodes| nodes.len()),
                _ => 0,
            },
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
    fn plan(&self, range: &Range<usize>) -> Plan {
        let mut planner = Planner {
            state: self,
            range: range.clone(),
            imports: Vec::new(),
        };
        let mut key = Vec::with_capacity(range.len());
        let mut pre = Vec::with_capacity(range.len());
        for index in range.clone() {
            match planner.lower(index) {
                Ok(lowered) => {
                    key.push(lowered);
                    pre.push(None);
                }
                Err(ran) => {
                    key.push((
                        Low::Unemitted,
                        Gate {
                            names: [None; 4],
                            found: None,
                        },
                    ));
                    pre.push(Some(ran));
                }
            }
        }
        Plan {
            key,
            imports: planner.imports,
            pre,
        }
    }
}

/// A caret read's answer.
fn caret_found(
    outcome: &Outcome,
    caret: &CaretRegisters,
    point: Reg<kind::TextRange>,
    edges: &[Reg<kind::Bool>; 6],
) -> Result<CaretFound, Error> {
    let edge = |start, end| -> Result<_, Error> {
        Ok(verbatim_model::batch::Edges {
            start: outcome.get(start)?,
            end: outcome.get(end)?,
        })
    };
    let selection = if outcome.get(caret.has_selection)? {
        let range = outcome
            .get(caret.selection)?
            .ok_or(Error::MissingResult(caret.selection.id()))?;
        Some((range, edge(edges[2], edges[3])?, edge(edges[4], edges[5])?))
    } else {
        None
    };
    Ok(CaretFound {
        caret: outcome
            .get(point)?
            .ok_or(Error::MissingResult(point.id()))?,
        edges: edge(edges[0], edges[1])?,
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
