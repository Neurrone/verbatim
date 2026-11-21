//! A walk down the tree ([`super::Input::Descendants`]): breadth first from
//! its starts, each run reading the children of the elements waiting, by
//! one call an element or by one program for up to [`CHILDREN_PER_RUN`]
//! children, its matchers judged in Rust between runs ([`Descend::judge`]),
//! and answered in tree order. A program's run reads on into the children
//! it found unless the walk prunes, whose judgement it waits for.

use std::collections::VecDeque;

use windows::Win32::UI::Accessibility::IUIAutomationElement;

use verbatim_uia::{Cached, Node, Uia};

use super::{Descend, Descendant, Descendants, Judged};
use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::{RemoteCache, absent_record};
use crate::opcode::{Comparison, NavigationDirection};
use crate::operation::{Outcome, Value};

/// The most children one program's run reads.
pub const CHILDREN_PER_RUN: u32 = 60;

/// The most waiting elements a program's run that continues a walk starts
/// from.
pub(super) const ELEMENTS_PER_RUN: usize = 64;

/// The most instructions one run from `starts` elements reading `room`
/// children executes.
pub(super) fn run_cost(walk: &Descend<'_>, starts: u32, room: u32) -> u32 {
    let (setup, each) = RemoteCache::costs(walk.fill);
    let start = 12 + if walk.include_subject { each } else { 0 };
    // Each element of the queue costs its check and its last step, each
    // child its read: the queue holds the starts and every child.
    40 + setup + starts * start + (starts + room) * 24 + room * (20 + each)
}

/// How many children a run from `starts` elements reads within `budget`
/// instructions: as many as fit, at most [`CHILDREN_PER_RUN`] and the
/// walk's node cap.
pub(super) fn run_room(walk: &Descend<'_>, starts: u32, budget: u32) -> u32 {
    let mut room = CHILDREN_PER_RUN.min(walk.nodes);
    while room > 0 && run_cost(walk, starts, room) > budget {
        room -= 1;
    }
    room
}

/// How a walk's runs read children.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reads {
    /// One call an element.
    Calls,
    /// One program a run.
    Program,
}

/// One node a walk visited.
struct Entry {
    element: IUIAutomationElement,
    /// Read with the walk's fill; a start not judged has none.
    node: Option<Cached<Node>>,
    parent: Option<usize>,
    /// Its start's place, then its place among its parent's children at
    /// each level: tree order is the order of these.
    path: Vec<u32>,
    depth: u32,
    children: u32,
    judged: Judged,
}

/// A walk under way.
pub(super) struct Walk<'w, 'a> {
    descend: &'w Descend<'a>,
    listed: Vec<Entry>,
    waiting: VecDeque<usize>,
    complete: bool,
}

impl<'w, 'a> Walk<'w, 'a> {
    /// A walk from `starts`, none of them read yet.
    pub(super) fn new(walk: &'w Descend<'a>, starts: Vec<IUIAutomationElement>) -> Self {
        let mut this = Self {
            descend: walk,
            listed: Vec::new(),
            waiting: VecDeque::new(),
            complete: true,
        };
        for (root, element) in (0u32..).zip(starts) {
            this.listed.push(Entry {
                element,
                node: None,
                parent: None,
                path: vec![root],
                depth: 0,
                children: 0,
                judged: Judged {
                    matched: false,
                    pruned: false,
                },
            });
            this.waiting.push_back(this.listed.len() - 1);
        }
        this
    }

    /// Runs the walk to its end, each run read as `reads` says; a program
    /// run that fails leaves the rest to calls, logged as a fallback.
    ///
    /// # Errors
    ///
    /// A read that failed other than because its element was gone.
    pub(super) fn run(&mut self, uia: &Uia, mut reads: Reads) -> Result<(), Error> {
        if self.descend.include_subject && self.listed.iter().all(|entry| entry.node.is_none()) {
            let starts: Vec<usize> = self.waiting.iter().copied().collect();
            for at in starts {
                let node = super::calls::filled(&self.listed[at].element, self.descend.fill)?;
                self.judge_start(at, node);
            }
        }
        while !self.done() {
            if reads == Reads::Program {
                match self.program_run() {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) if crate::caret::gone_or_timed_out(&error) => return Err(error),
                    Err(error) => {
                        super::remote_fell_back("descendants", None, &error);
                        reads = Reads::Calls;
                    }
                }
            }
            self.call_run(uia)?;
        }
        Ok(())
    }

    /// Whether the walk is over: nothing waits, the node cap is reached,
    /// or the limit is met and nothing waiting could come before the last
    /// node answered.
    fn done(&mut self) -> bool {
        if self.waiting.is_empty() {
            return true;
        }
        if self.listed.len() >= self.descend.nodes as usize {
            self.complete = false;
            return true;
        }
        let mut answered: Vec<&Vec<u32>> = self
            .listed
            .iter()
            .filter(|entry| entry.judged.matched)
            .map(|entry| &entry.path)
            .collect();
        answered.sort();
        let Some(last) = (self.descend.limit as usize)
            .checked_sub(1)
            .and_then(|at| answered.get(at))
        else {
            return false;
        };
        self.waiting.iter().all(|&at| self.listed[at].path > **last)
    }

    /// Judges start `at`, read as `node`.
    fn judge_start(&mut self, at: usize, node: Cached<Node>) {
        let judged = (self.descend.judge)(&node);
        let entry = &mut self.listed[at];
        entry.judged = judged;
        entry.node = Some(node);
        if judged.pruned {
            self.waiting.retain(|&waiting| waiting != at);
        }
    }

    /// Lists `child`, read under the listed `parent`, judged; it waits for
    /// its children when `waits`, unless it is pruned or at the walk's
    /// depth. `None` when the node cap left no room, which leaves the walk
    /// incomplete.
    fn list(&mut self, parent: usize, child: Cached<Node>, waits: bool) -> Option<usize> {
        if self.listed.len() >= self.descend.nodes as usize {
            self.complete = false;
            return None;
        }
        let judged = (self.descend.judge)(&child);
        let above = &mut self.listed[parent];
        let mut path = above.path.clone();
        path.push(above.children);
        above.children += 1;
        let depth = above.depth + 1;
        self.listed.push(Entry {
            element: child.live().element().clone(),
            node: Some(child),
            parent: Some(parent),
            path,
            depth,
            children: 0,
            judged,
        });
        let at = self.listed.len() - 1;
        if waits && !judged.pruned && depth < self.descend.depth {
            self.waiting.push_back(at);
        }
        Some(at)
    }

    /// Reads the next waiting element's children by calls; a gone one
    /// leaves the walk incomplete.
    ///
    /// # Errors
    ///
    /// A read that failed other than because its element was gone.
    pub(super) fn call_run(&mut self, uia: &Uia) -> Result<(), Error> {
        let Some(at) = self.waiting.pop_front() else {
            return Ok(());
        };
        let read = super::calls::children(
            uia,
            &self.listed[at].element,
            self.descend.view,
            self.descend.fill,
        );
        let children = match read {
            Ok(children) => children,
            Err(error) if matches!(super::failed(&error), super::Ran::Gone) => {
                self.complete = false;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        for child in children {
            if self.list(at, child, true).is_none() {
                break;
            }
        }
        Ok(())
    }

    /// One program run from up to [`ELEMENTS_PER_RUN`] waiting elements;
    /// true when its first element's children fill no run, for calls to
    /// read.
    fn program_run(&mut self) -> Result<bool, Error> {
        let count = self.waiting.len().min(ELEMENTS_PER_RUN);
        let taken: Vec<usize> = self.waiting.drain(..count).collect();
        let mut b = Builder::new();
        let queue = b.new_array();
        let mut depths = Vec::with_capacity(taken.len());
        for &at in &taken {
            let element = b.import_element(&self.listed[at].element);
            b.array_append(queue, element);
            depths.push(self.listed[at].depth);
        }
        let registers = emit_run(
            &mut b,
            self.descend,
            (queue, Some(&depths)),
            self.room(taken.len()),
            false,
        );
        match b.finish().execute() {
            Ok(outcome) => self.take_run(&outcome, &registers, &taken),
            Err(error) => {
                for &at in taken.iter().rev() {
                    self.waiting.push_front(at);
                }
                Err(error)
            }
        }
    }

    /// The room left under the node cap and the platform's limit for one
    /// run from `starts` elements.
    fn room(&self, starts: usize) -> u32 {
        let left = self.descend.nodes as usize - self.listed.len().min(self.descend.nodes as usize);
        let starts = u32::try_from(starts).unwrap_or(u32::MAX);
        u32::try_from(left).unwrap_or(u32::MAX).min(run_room(
            self.descend,
            starts,
            super::program::LIMIT - super::program::PROLOGUE,
        ))
    }

    /// The listed starts, the first run's queue.
    pub(super) fn starts(&self) -> Vec<usize> {
        self.waiting.iter().copied().collect()
    }

    /// Takes a run's answer, the run having started from the listed
    /// elements `taken`: judges the starts it filled, lists the children
    /// it read in full, and leaves waiting each element whose children it
    /// did not reach. True when its first element's children filled the
    /// run alone, for calls to read.
    ///
    /// # Errors
    ///
    /// A result missing from the run's answer.
    pub(super) fn take_run(
        &mut self,
        outcome: &Outcome,
        registers: &RunRegisters,
        taken: &[usize],
    ) -> Result<bool, Error> {
        let missing = |reg: crate::instruction::OperandId| Error::MissingResult(reg);
        let queue = outcome.get(registers.queue)?;
        let absents = outcome.get(registers.absents)?;
        let parents = outcome.get(registers.parents)?;
        let done = outcome.get(registers.done)? as usize;
        let overflowed = outcome.get(registers.overflowed)?;
        let element = |index: usize| match queue.get(index) {
            Some(Value::Element(element)) => Ok(element.clone()),
            _ => Err(missing(registers.queue.id())),
        };
        let absent = |index: usize| match absents.get(index) {
            Some(Value::Int(bits)) => absent_record(*bits),
            _ => absent_record(0),
        };
        self.waiting.retain(|waiting| !taken.contains(waiting));
        if registers.filled_starts {
            for (index, &at) in taken.iter().enumerate() {
                let node = self.descend.fill.claim(element(index)?, absent(index));
                self.judge_start(at, node);
            }
        }
        // Each queue entry's listed place: the starts, then each child.
        let mut placed: Vec<Option<usize>> = taken.iter().copied().map(Some).collect();
        for index in taken.len()..queue.len() {
            let parent = match parents.get(index - taken.len()) {
                Some(Value::Uint(parent)) => *parent as usize,
                _ => return Err(missing(registers.parents.id())),
            };
            // The children of an element the run stopped inside are read
            // again with it, and a pruned element's are left out.
            let at = match placed.get(parent).copied().flatten() {
                Some(at) if !(self.listed[at].judged.pruned || overflowed && parent == done) => {
                    let child = self.descend.fill.claim(element(index)?, absent(index));
                    self.list(at, child, index >= done)
                }
                _ => None,
            };
            placed.push(at);
        }
        // The starts whose children the run did not reach wait again,
        // first.
        for &at in taken[done.min(taken.len())..].iter().rev() {
            if !self.listed[at].judged.pruned {
                self.waiting.push_front(at);
            }
        }
        Ok(overflowed && done == 0 && self.waiting.front() == taken.first())
    }

    /// The answer: the nodes judged answered, in tree order, at most the
    /// limit, each with its parent's place when its parent is answered.
    pub(super) fn finish(self) -> Descendants {
        let mut order: Vec<usize> = (0..self.listed.len())
            .filter(|&at| self.listed[at].judged.matched)
            .collect();
        order.sort_by(|&a, &b| self.listed[a].path.cmp(&self.listed[b].path));
        order.truncate(self.descend.limit as usize);
        let parents: Vec<Option<u32>> = order
            .iter()
            .map(|&at| {
                let parent = self.listed[at].parent?;
                let place = order.iter().position(|&placed| placed == parent)?;
                u32::try_from(place).ok()
            })
            .collect();
        let mut listed: Vec<Option<Entry>> = self.listed.into_iter().map(Some).collect();
        let nodes = order
            .iter()
            .zip(parents)
            .filter_map(|(&at, parent)| {
                let entry = listed[at].take()?;
                Some(Descendant {
                    node: entry.node?,
                    parent,
                    root: entry.path[0],
                })
            })
            .collect();
        Descendants {
            nodes,
            complete: self.complete,
        }
    }
}

/// The registers a run of the walk answers in.
#[derive(Clone, Copy)]
pub(super) struct RunRegisters {
    /// The elements the run started from, then each child it found.
    queue: Reg<kind::Array>,
    /// Each one's absent record: a start's when the run filled it.
    absents: Reg<kind::Array>,
    /// Each child's parent's place in `queue`.
    parents: Reg<kind::Array>,
    /// How many of `queue`'s elements had their children read.
    done: Reg<kind::Uint>,
    /// The run stopped inside an element's children.
    overflowed: Reg<kind::Bool>,
    /// Whether the run filled and answered its starts.
    filled_starts: bool,
}

impl RunRegisters {
    /// The queue's register.
    pub(super) fn queue(&self) -> Reg<kind::Array> {
        self.queue
    }
}

/// Emits one run of the walk `walk` from the elements in `queue`, at the
/// depths `depths` gives, each 0 without it; each start filled and
/// answered when `fill_starts`; then the children of each element of the
/// queue in turn above the walk's depth, each filled inside the provider
/// and appended to it, read on into when the walk does not prune, until
/// `room` children are read; an element whose children do not all fit
/// stops the run.
#[expect(clippy::too_many_lines, reason = "one run, top to bottom as it runs")]
pub(super) fn emit_run(
    b: &mut Builder,
    walk: &Descend<'_>,
    (queue, depths_given): (Reg<kind::Array>, Option<&[u32]>),
    room: u32,
    fill_starts: bool,
) -> RunRegisters {
    let cache = RemoteCache::new(b, walk.fill);
    let queue = b.add_to_results(queue);
    let absents = b.new_array();
    let absents = b.add_to_results(absents);
    let parents = b.new_array();
    let parents = b.add_to_results(parents);
    let depths = b.new_array();
    let done = b.new_uint(0);
    let done = b.add_to_results(done);
    let overflowed = b.new_bool(false);
    let overflowed = b.add_to_results(overflowed);
    let one = b.uint(1);
    let starts = b.array_size(queue);
    for &depth in depths_given.unwrap_or_default() {
        let depth = b.uint(depth);
        b.array_append(depths, depth);
    }
    let index = b.new_uint(0);
    let zero = b.uint(0);
    let none = b.int(0);
    b.while_(
        |b| b.compare(index, starts, Comparison::LessThan),
        |b| {
            let absent = if fill_starts {
                let start = b.array_get_at(queue, index).assume::<kind::Element>();
                cache.populate(b, start)
            } else {
                none
            };
            b.array_append(absents, absent);
            if depths_given.is_none() {
                b.array_append(depths, zero);
            }
            b.add_assign(index, one);
        },
    );
    let read = b.new_uint(0);
    let room = b.uint(room);
    let deepest = b.uint(walk.depth);
    let stop = b.new_bool(false);
    b.while_(
        |b| {
            // A walk that prunes reads only its starts' children in a run,
            // so that each child is judged before its own are read.
            let size = if walk.prunes {
                starts
            } else {
                b.array_size(queue)
            };
            let more = b.compare(done, size, Comparison::LessThan);
            let go = b.not(stop);
            b.and(more, go)
        },
        |b| {
            let depth = b.array_get_at(depths, done).assume::<kind::Uint>();
            let deep = b.compare(depth, deepest, Comparison::GreaterThanOrEqual);
            b.if_else(
                deep,
                // At the walk's depth: no child is read.
                |b| b.add_assign(done, one),
                |b| {
                    let element = b.array_get_at(queue, done).assume::<kind::Element>();
                    let below = b.add(depth, one);
                    let child = b.navigate(element, NavigationDirection::FirstChild);
                    b.while_(
                        |b| {
                            let null = b.is_null(child);
                            let some = b.not(null);
                            let fits = b.compare(read, room, Comparison::LessThan);
                            b.and(some, fits)
                        },
                        |b| {
                            let absent = cache.populate(b, child);
                            b.array_append(queue, child);
                            b.array_append(absents, absent);
                            b.array_append(parents, done);
                            b.array_append(depths, below);
                            b.add_assign(read, one);
                            let next = b.navigate(child, NavigationDirection::NextSibling);
                            b.set(child, next);
                        },
                    );
                    let null = b.is_null(child);
                    b.if_else(
                        null,
                        |b| b.add_assign(done, one),
                        |b| {
                            let yes = b.bool(true);
                            b.set(overflowed, yes);
                            b.set(stop, yes);
                        },
                    );
                },
            );
        },
    );
    RunRegisters {
        queue,
        absents,
        parents,
        done,
        overflowed,
        filled_starts: fill_starts,
    }
}
