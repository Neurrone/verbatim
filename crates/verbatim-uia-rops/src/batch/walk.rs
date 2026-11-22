//! A walk down the tree (`Descendants`): breadth first from
//! its starts, each run reading the children of the elements waiting, by
//! one call an element or by one program for up to [`CHILDREN_PER_RUN`]
//! children, its matchers judged in Rust between runs ([`Descend::judged`]),
//! and answered in tree order. A program's run reads on into the children
//! it found unless the walk prunes, whose judgement it waits for.
//!
//! An element gone during a run is left out with its subtree and leaves
//! the walk incomplete, its later siblings kept: a run catches a failed
//! step to an element's first child as the element's, and a failed read
//! of a child or step on from it as the child's, which ends its parent's
//! children there, those read before it kept, as a walk by calls ends
//! them. It reads every element it answers alive at its end, giving a
//! gone one back as none, since a gone element in a run's answer fails
//! the whole run.

use std::collections::VecDeque;

use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_NamePropertyId};

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
    let start = 12 + if walk.include_subject { each + 1 } else { 0 };
    // Each element of the queue costs its check, its caught step to its
    // first child and its last step, and its read alive at the run's end,
    // each child its caught read and step on: the queue holds the starts
    // and every child.
    50 + setup + starts * start + (starts + room) * 50 + room * (20 + each)
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
                    // An element the walk reached gone inside the run: the
                    // calls go on from what waits, as they meet a gone
                    // child, with no remote failure to log.
                    Err(error) if is_gone(&error) => reads = Reads::Calls,
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
        let judged = self.descend.judged(&node);
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
        let judged = self.descend.judged(&child);
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

    /// Reads the next waiting element's children by calls; a gone one, or
    /// a gone child, which ends the children there, leaves the walk
    /// incomplete.
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
            Ok((children, all)) => {
                self.complete &= all;
                children
            }
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
    /// did not reach. An element the run gave back as gone is left out
    /// with its subtree and leaves the walk incomplete. True when its first
    /// element's children filled the run alone, for calls to read.
    ///
    /// # Errors
    ///
    /// A result missing from the run's answer, or a read of an element's
    /// children that failed other than because the element was gone.
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
        for status in outcome.get(registers.statuses)? {
            let Value::Int(code) = status else {
                return Err(missing(registers.statuses.id()));
            };
            let error = Error::Uia(windows::core::Error::from(windows::core::HRESULT(code)));
            if !is_gone(&error) {
                return Err(error);
            }
            self.complete = false;
        }
        // A place the run gave back as none held an element gone.
        let gone = |index: usize| matches!(queue.get(index), Some(Value::Null));
        let element = |index: usize| match queue.get(index) {
            Some(Value::Element(element)) => Ok(element.clone()),
            _ => Err(missing(registers.queue.id())),
        };
        let absent = |index: usize| match absents.get(index) {
            Some(Value::Int(bits)) => absent_record(*bits),
            _ => absent_record(0),
        };
        self.waiting.retain(|waiting| !taken.contains(waiting));
        // Each queue entry's listed place, `None` for one left out: the
        // starts, then each child.
        let mut placed: Vec<Option<usize>> = Vec::with_capacity(queue.len());
        for (index, &at) in taken.iter().enumerate() {
            if gone(index) {
                self.listed[at].judged.matched = false;
                self.complete = false;
                placed.push(None);
                continue;
            }
            if registers.filled_starts {
                let node = self.descend.fill.claim(element(index)?, absent(index));
                self.judge_start(at, node);
            }
            placed.push(Some(at));
        }
        for index in taken.len()..queue.len() {
            let parent = match parents.get(index - taken.len()) {
                Some(Value::Uint(parent)) => *parent as usize,
                _ => return Err(missing(registers.parents.id())),
            };
            // The children of an element the run stopped inside are read
            // again with it, and a pruned element's are left out, as are
            // those of a gone one.
            let at = match placed.get(parent).copied().flatten() {
                Some(_) if gone(index) => {
                    self.complete = false;
                    None
                }
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
        for (index, &at) in taken.iter().enumerate().skip(done).rev() {
            if placed[index].is_some() && !self.listed[at].judged.pruned {
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
    /// The failure of each caught step or read, every one leaving the walk
    /// incomplete.
    statuses: Reg<kind::Array>,
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
/// stops the run. A failed step to an element's first child is caught,
/// its failure answered and the element given back as none; a failed read
/// of a child or step on from it is caught, its failure answered, and the
/// element's children end there; at the run's end
/// every element of the queue is read alive, by its name, and one gone is
/// given back as none.
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
    let statuses = b.new_array();
    let statuses = b.add_to_results(statuses);
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
                let absent = cache.populate(b, start);
                // The element filled is a new one: the queue holds it in
                // place of the start as imported.
                b.array_set_at(queue, index, start);
                absent
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
                    let child = b.new_null_element();
                    let reached = b.new_bool(false);
                    // The step to the first child fails as its element's:
                    // the element is given back as none.
                    b.try_catch(
                        |b| {
                            let first = b.navigate(element, NavigationDirection::FirstChild);
                            b.set(child, first);
                            let yes = b.bool(true);
                            b.set(reached, yes);
                        },
                        |b, status| {
                            b.array_append(statuses, status);
                            let none = b.new_null_element();
                            b.array_set_at(queue, done, none);
                        },
                    );
                    let unreached = b.not(reached);
                    b.if_else(
                        unreached,
                        |b| b.add_assign(done, one),
                        |b| {
                            b.while_(
                                |b| {
                                    let null = b.is_null(child);
                                    let some = b.not(null);
                                    let fits = b.compare(read, room, Comparison::LessThan);
                                    b.and(some, fits)
                                },
                                |b| {
                                    // A child's read or its step on fails
                                    // as the child's: it is left out if
                                    // unread, and the element's children
                                    // end there, as a walk by calls,
                                    // which steps on from each child,
                                    // ends them (Claude, 2026-11-22).
                                    b.try_catch(
                                        |b| {
                                            let absent = cache.populate(b, child);
                                            b.array_append(queue, child);
                                            b.array_append(absents, absent);
                                            b.array_append(parents, done);
                                            b.array_append(depths, below);
                                            b.add_assign(read, one);
                                            let next =
                                                b.navigate(child, NavigationDirection::NextSibling);
                                            b.set(child, next);
                                        },
                                        |b, status| {
                                            b.array_append(statuses, status);
                                            let none = b.new_null_element();
                                            b.set(child, none);
                                        },
                                    );
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
        },
    );
    // Every element of the queue read alive at the run's end.
    let index = b.new_uint(0);
    let size = b.array_size(queue);
    b.while_(
        |b| b.compare(index, size, Comparison::LessThan),
        |b| {
            let element = b.array_get_at(queue, index).assume::<kind::Element>();
            let null = b.is_null(element);
            let some = b.not(null);
            b.if_(some, |b| {
                b.try_catch(
                    |b| {
                        let _ = b.property(element, UIA_NamePropertyId.0);
                    },
                    |b, _| {
                        let none = b.new_null_element();
                        b.array_set_at(queue, index, none);
                    },
                );
            });
            b.add_assign(index, one);
        },
    );
    RunRegisters {
        queue,
        absents,
        parents,
        done,
        overflowed,
        statuses,
        filled_starts: fill_starts,
    }
}

/// Whether `error` is an element's provider answering that it is gone.
fn is_gone(error: &Error) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}
