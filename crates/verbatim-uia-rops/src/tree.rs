//! The tree dump, a diagnostic for the control plane (`verbatim-inspect
//! dump-tree`): an element's raw-view subtree, each element listed with its
//! children read in one call, as NVDA reads an object's children
//! (`NVDAObjects/UIA/__init__.py`, `_get_UIAChildren`: a cache request
//! scoped to the children, then `GetCachedChildren`), so a child gone
//! before the call is simply absent and a child gone after it keeps its
//! later siblings. A remote program reads the children of many elements in
//! one round trip, breadth first; the classic implementation behind the
//! same signature reads one element's children per call. [`tree_dump`] is
//! the one function callers use.
//!
//! Every element is filled by the query's fill, which for a read of many
//! elements leaves out their place, the rectangle and whether they are off
//! the screen, unless the caller asks for them (`verbatim_uia::Place`).

use std::collections::VecDeque;

use windows::Win32::UI::Accessibility::{
    IUIAutomationTreeWalker, UIA_E_ELEMENTNOTAVAILABLE, UIA_NamePropertyId,
};
use windows::core::HRESULT;

use verbatim_model::{NodeSnapshot, TreeNode};
use verbatim_uia::{Cached, Direction, Fill, Live, Node, NodeIdRegistry, Uia};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::focus::{Path, RemoteCache, absent_record};
use crate::opcode::{Comparison, NavigationDirection};
use crate::operation::{Outcome, Value};

/// The most children one run of the program reads, which keeps its worst
/// case under half the platform's limit on the instructions one run may
/// execute (`crates/mockapp/tests/instruction_limit.rs`).
pub const CHILDREN_PER_RUN: u32 = 60;

/// The most waiting elements one run of the program starts from.
const ELEMENTS_PER_RUN: usize = 16;

/// What [`tree_dump`] and its two implementations are asked.
#[derive(Clone, Copy)]
pub struct TreeQuery<'a> {
    /// Where the dump starts, already read as a [`Node`].
    pub root: &'a Cached<Node>,
    /// How every other element's cache is filled: as a [`Node`].
    pub fill: &'a Fill<Node>,
    /// The registry that keeps each element and mints its node id.
    pub registry: &'a NodeIdRegistry,
    /// The deepest level whose elements are listed, the root at 0; an
    /// element there with children marks the dump incomplete.
    pub max_depth: u32,
    /// The most elements listed, the root among them; one more marks the
    /// dump incomplete.
    pub max_nodes: usize,
}

/// The signature both implementations share: the tree, and whether it is
/// incomplete, a cap reached or an element whose children could not be
/// read because it was gone.
pub type TreeDumpFn = fn(&Uia, &TreeQuery<'_>) -> Result<(TreeNode, bool), Error>;

/// The tree dump, the one function call sites use: the remote program when
/// `remote` is true, the rest of the dump read classically from where a
/// run failed when it fails other than by UIA's transaction timeout, and
/// the classic reads alone when `remote` is false. Says which path
/// answered.
///
/// # Errors
///
/// The [`Error`] of a read that failed other than because its element was
/// gone: an element whose children could not be read is never shown as
/// having none.
pub fn tree_dump(
    uia: &Uia,
    query: &TreeQuery<'_>,
    remote: bool,
) -> Result<((TreeNode, bool), Path), Error> {
    let mut dump = Dump::new(query);
    let mut path = if remote { Path::Remote } else { Path::Classic };
    while !dump.waiting.is_empty() {
        if matches!(path, Path::Remote) {
            match dump.next(uia, true) {
                Ok(()) => {}
                Err(Stop::Run(error)) if !is_timeout(&error) => path = Path::Fallback(error),
                Err(Stop::Run(error) | Stop::Read(error)) => return Err(error),
            }
        } else {
            dump.next(uia, false).map_err(Stop::into_error)?;
        }
    }
    Ok((dump.finish(), path))
}

/// The tree dump in remote programs alone, each reading the children of
/// up to [`CHILDREN_PER_RUN`] elements breadth first; an element whose
/// children one run cannot hold is read classically, one call.
///
/// # Errors
///
/// The first run's [`Error`] that fails as a whole, or as
/// [`tree_dump_classic`]'s.
pub fn tree_dump_remote(uia: &Uia, query: &TreeQuery<'_>) -> Result<(TreeNode, bool), Error> {
    let mut dump = Dump::new(query);
    while !dump.waiting.is_empty() {
        dump.next(uia, true).map_err(Stop::into_error)?;
    }
    Ok(dump.finish())
}

/// The tree dump the classic way, the fallback and the reference: one call
/// per listed element above the deepest level, `BuildUpdatedCache` scoped
/// to its children (`verbatim_uia::Uia::children`), and one uncached step
/// to the first child for an element at the deepest level, to tell
/// whether the dump stops above children.
///
/// # Errors
///
/// [`Error::Uia`] when a read fails other than because its element is
/// gone.
pub fn tree_dump_classic(uia: &Uia, query: &TreeQuery<'_>) -> Result<(TreeNode, bool), Error> {
    let mut dump = Dump::new(query);
    while !dump.waiting.is_empty() {
        dump.next(uia, false).map_err(Stop::into_error)?;
    }
    Ok(dump.finish())
}

/// Whether a run or read ended at UIA's transaction timeout.
fn is_timeout(error: &Error) -> bool {
    use windows::Win32::UI::Accessibility::UIA_E_TIMEOUT;
    error
        .hresult()
        .is_some_and(|code| code == HRESULT(UIA_E_TIMEOUT.cast_signed()))
}

/// Whether a failure says its element is gone.
fn is_gone(code: HRESULT) -> bool {
    code == HRESULT(UIA_E_ELEMENTNOTAVAILABLE.cast_signed())
}

/// The registers a run of the program answers in.
#[derive(Clone, Copy)]
struct Registers {
    dropped: Reg<kind::Bool>,
    found: Reg<kind::Array>,
    parents: Reg<kind::Array>,
    absents: Reg<kind::Array>,
    failed: Reg<kind::Array>,
    statuses: Reg<kind::Array>,
    deeper: Reg<kind::Bool>,
    done: Reg<kind::Int>,
    overflowed: Reg<kind::Bool>,
}

/// Why a step of the dump stopped it.
enum Stop {
    /// A run of the program failed as a whole: the elements it took wait
    /// again, for the classic reads to answer.
    Run(Error),
    /// A read failed other than because its element was gone: the dump
    /// fails.
    Read(Error),
}

impl Stop {
    fn into_error(self) -> Error {
        match self {
            Self::Run(error) | Self::Read(error) => error,
        }
    }
}

/// One listed element: its snapshot and its children's places in the
/// dump's list.
struct Listed {
    snapshot: NodeSnapshot,
    children: Vec<usize>,
}

/// An element listed whose children are not read yet.
struct Waiting {
    /// Its place in the dump's list.
    listed: usize,
    /// The element.
    live: Live,
    /// Its level, the root at 0.
    depth: u32,
}

/// A dump under way: the elements listed so far, those whose children are
/// still to be read, in breadth-first order, and whether it is incomplete.
struct Dump<'q, 'a> {
    query: &'q TreeQuery<'a>,
    listed: Vec<Listed>,
    waiting: VecDeque<Waiting>,
    truncated: bool,
    walker: Option<IUIAutomationTreeWalker>,
}

impl<'q, 'a> Dump<'q, 'a> {
    fn new(query: &'q TreeQuery<'a>) -> Self {
        let mut dump = Self {
            query,
            listed: Vec::new(),
            waiting: VecDeque::new(),
            truncated: false,
            walker: None,
        };
        dump.list(query.root.snapshot(query.registry), query.root.live(), 0);
        dump
    }

    /// Lists an element, to have its children read.
    fn list(&mut self, snapshot: NodeSnapshot, live: &Live, depth: u32) -> usize {
        let listed = self.listed.len();
        self.listed.push(Listed {
            snapshot,
            children: Vec::new(),
        });
        self.waiting.push_back(Waiting {
            listed,
            live: live.clone(),
            depth,
        });
        listed
    }

    /// Lists `children` of the listed element `parent` at `depth`, as many
    /// as the node cap leaves room for; one more marks the dump
    /// incomplete.
    fn list_children(&mut self, parent: usize, depth: u32, children: Vec<Cached<Node>>) {
        for child in children {
            if self.listed.len() >= self.query.max_nodes {
                self.truncated = true;
                return;
            }
            let at = self.list(child.snapshot(self.query.registry), child.live(), depth);
            self.listed[parent].children.push(at);
        }
    }

    /// Reads the next waiting elements' children, by a run of the program
    /// when `remote` is true, else classically; once the node cap is
    /// reached, only whether any waiting element has a child, which marks
    /// the dump incomplete, one uncached step each until one has.
    fn next(&mut self, uia: &Uia, remote: bool) -> Result<(), Stop> {
        if self.listed.len() >= self.query.max_nodes {
            while !self.truncated
                && let Some(waiting) = self.waiting.pop_front()
            {
                self.peek(uia, &waiting.live).map_err(Stop::Read)?;
            }
            self.waiting.clear();
            return Ok(());
        }
        if remote {
            self.run_program(uia)
        } else {
            self.read_one_classically(uia).map_err(Stop::Read)
        }
    }

    /// Reads the next waiting element's children classically: one call.
    fn read_one_classically(&mut self, uia: &Uia) -> Result<(), Error> {
        let Some(waiting) = self.waiting.pop_front() else {
            return Ok(());
        };
        if waiting.depth >= self.query.max_depth {
            return self.peek(uia, &waiting.live);
        }
        match uia.children(&waiting.live, self.query.fill.request()) {
            Ok(children) => {
                self.list_children(waiting.listed, waiting.depth + 1, children);
                Ok(())
            }
            Err(error) if is_gone(error.code()) => {
                self.truncated = true;
                Ok(())
            }
            Err(error) => Err(Error::Uia(error)),
        }
    }

    /// Whether an element at the deepest level has a child, which marks
    /// the dump incomplete: one uncached step.
    fn peek(&mut self, uia: &Uia, live: &Live) -> Result<(), Error> {
        let walker = if let Some(walker) = &self.walker {
            walker.clone()
        } else {
            let walker = uia.raw_view_walker()?;
            self.walker = Some(walker.clone());
            walker
        };
        match live.step(&walker, Direction::FirstChild) {
            Ok(_) => self.truncated = true,
            // No child: the null element, an error with no failure code.
            Err(error) if error.code().is_ok() => {}
            Err(error) if is_gone(error.code()) => self.truncated = true,
            Err(error) => return Err(Error::Uia(error)),
        }
        Ok(())
    }

    /// One run of the program, from up to [`ELEMENTS_PER_RUN`] waiting
    /// elements: for each element in turn, those and the children it lists
    /// after them, its children are read, each filled inside the provider,
    /// until [`CHILDREN_PER_RUN`] children or the node cap's room are read.
    /// An element whose children fail to be read is caught and reported
    /// with the failure, and, when the run found it, given back as none and
    /// left out of the dump, since a gone element in a run's answer fails
    /// the whole run, as is any element found that a live read of its name
    /// finds gone at the run's end; one whose children the run cannot hold
    /// all of ends the run, and is read classically.
    ///
    /// # Errors
    ///
    /// A run that fails as a whole, or a child read that failed other than
    /// because its element was gone. Whatever a run that failed as a whole
    /// took from the waiting elements is put back.
    #[expect(
        clippy::too_many_lines,
        reason = "one program, read top to bottom as it runs, then its answer"
    )]
    fn run_program(&mut self, uia: &Uia) -> Result<(), Stop> {
        let taken: Vec<Waiting> = {
            let count = self.waiting.len().min(ELEMENTS_PER_RUN);
            self.waiting.drain(..count).collect()
        };
        let room = self.query.max_nodes.saturating_sub(self.listed.len());
        let limit = u32::try_from(room)
            .unwrap_or(u32::MAX)
            .min(CHILDREN_PER_RUN);
        let mut b = Builder::new();
        let queue = b.new_array();
        let depths = b.new_array();
        // The queue's length, kept as it grows, and its imported start.
        let queued = b.new_int(i32::try_from(taken.len()).unwrap_or(i32::MAX));
        let imported = b.int(i32::try_from(taken.len()).unwrap_or(i32::MAX));
        for waiting in &taken {
            let element = b.import_element(waiting.live.element());
            b.array_append(queue, element);
            let depth = b.int(i32::try_from(waiting.depth).unwrap_or(i32::MAX));
            b.array_append(depths, depth);
        }
        let cache = RemoteCache::new(&mut b, self.query.fill);
        // Each child read, in order, with its parent's place in the queue
        // and its absent record.
        let found = b.new_array();
        let found = b.add_to_results(found);
        let parents = b.new_array();
        let parents = b.add_to_results(parents);
        let absents = b.new_array();
        let absents = b.add_to_results(absents);
        // Each element whose children failed to be read, with the failure.
        let failed = b.new_array();
        let failed = b.add_to_results(failed);
        let statuses = b.new_array();
        let statuses = b.add_to_results(statuses);
        // Whether an element at the deepest level has a child.
        let deeper = b.new_bool(false);
        let deeper = b.add_to_results(deeper);
        // How many of the queue's elements were done: those before it.
        let done = b.new_int(0);
        let done = b.add_to_results(done);
        // Whether the run ended inside an element's children.
        let overflowed = b.new_bool(false);
        let overflowed = b.add_to_results(overflowed);
        let read = b.new_int(0);
        let limit = b.int(i32::try_from(limit).unwrap_or(i32::MAX));
        let max_depth = b.int(i32::try_from(self.query.max_depth).unwrap_or(i32::MAX));
        let one = b.int(1);
        let stop = b.new_bool(false);
        b.while_(
            |b| {
                let more = b.compare(done, queued, Comparison::LessThan);
                let go = b.not(stop);
                b.and(more, go)
            },
            |b| {
                let element = b.array_get_at(queue, done).assume::<kind::Element>();
                let depth = b.array_get_at(depths, done).assume::<kind::Int>();
                let deepest = b.compare(depth, max_depth, Comparison::GreaterThanOrEqual);
                b.if_else(
                    deepest,
                    |b| {
                        let first = b.navigate(element, NavigationDirection::FirstChild);
                        let none = b.is_null(first);
                        let some = b.not(none);
                        b.if_(some, |b| {
                            let yes = b.bool(true);
                            b.set(deeper, yes);
                        });
                        b.add_assign(done, one);
                    },
                    |b| {
                        let full = b.compare(read, limit, Comparison::GreaterThanOrEqual);
                        b.if_else(
                            full,
                            |b| {
                                let yes = b.bool(true);
                                b.set(stop, yes);
                            },
                            |b| {
                                let below = b.add(depth, one);
                                b.try_catch(
                                    |b| {
                                        let child =
                                            b.navigate(element, NavigationDirection::FirstChild);
                                        b.while_(
                                            |b| {
                                                let none = b.is_null(child);
                                                let some = b.not(none);
                                                let room =
                                                    b.compare(read, limit, Comparison::LessThan);
                                                b.and(some, room)
                                            },
                                            |b| {
                                                let absent = cache.populate(b, child);
                                                b.array_append(found, child);
                                                b.array_append(parents, done);
                                                b.array_append(absents, absent);
                                                b.array_append(queue, child);
                                                b.array_append(depths, below);
                                                b.add_assign(queued, one);
                                                b.add_assign(read, one);
                                                let next = b.navigate(
                                                    child,
                                                    NavigationDirection::NextSibling,
                                                );
                                                b.set(child, next);
                                            },
                                        );
                                        let none = b.is_null(child);
                                        b.if_else(
                                            none,
                                            |b| b.add_assign(done, one),
                                            |b| {
                                                let yes = b.bool(true);
                                                b.set(overflowed, yes);
                                                b.set(stop, yes);
                                            },
                                        );
                                    },
                                    |b, status| {
                                        b.array_append(failed, done);
                                        b.array_append(statuses, status);
                                        // An element this run found is
                                        // given back as none: a gone
                                        // element in its answer would
                                        // fail the whole run.
                                        let own = b.compare(
                                            done,
                                            imported,
                                            Comparison::GreaterThanOrEqual,
                                        );
                                        b.if_(own, |b| {
                                            let at = b.subtract(done, imported);
                                            let none = b.new_null_element();
                                            b.array_set_at(found, at, none);
                                        });
                                        b.add_assign(done, one);
                                    },
                                );
                            },
                        );
                    },
                );
            },
        );
        // Every element the run found is checked alive, by a live read of
        // its name, and one gone since it was found, as an element is that
        // dies when asked for its children, is given back as none: a gone
        // element in a run's answer fails the whole run.
        let dropped = b.new_bool(false);
        let dropped = b.add_to_results(dropped);
        let index = b.new_int(0);
        let count = b.subtract(queued, imported);
        b.while_(
            |b| b.compare(index, count, Comparison::LessThan),
            |b| {
                let child = b.array_get_at(found, index).assume::<kind::Element>();
                let none = b.is_null(child);
                let some = b.not(none);
                b.if_(some, |b| {
                    b.try_catch(
                        |b| {
                            let _ = b.property(child, UIA_NamePropertyId.0);
                        },
                        |b, _status| {
                            let none = b.new_null_element();
                            b.array_set_at(found, index, none);
                            let yes = b.bool(true);
                            b.set(dropped, yes);
                        },
                    );
                });
                b.add_assign(index, one);
            },
        );
        let registers = Registers {
            dropped,
            found,
            parents,
            absents,
            failed,
            statuses,
            deeper,
            done,
            overflowed,
        };
        let outcome = match b.finish().execute() {
            Ok(outcome) => outcome,
            Err(error) => {
                for waiting in taken.into_iter().rev() {
                    self.waiting.push_front(waiting);
                }
                return Err(Stop::Run(error));
            }
        };
        self.answer(uia, &outcome, taken, registers)
    }

    /// Lists what a run of the program read ([`Self::run_program`]).
    fn answer(
        &mut self,
        uia: &Uia,
        outcome: &Outcome,
        taken: Vec<Waiting>,
        registers: Registers,
    ) -> Result<(), Stop> {
        let Registers {
            dropped,
            found,
            parents,
            absents,
            failed,
            statuses,
            deeper,
            done,
            overflowed,
        } = registers;
        let ints = |values: Vec<Value>| -> Vec<i32> {
            values
                .into_iter()
                .map(|value| match value {
                    Value::Int(value) => value,
                    _ => -1,
                })
                .collect()
        };
        let found = outcome.get(found).map_err(Stop::Run)?;
        let parents = ints(outcome.get(parents).map_err(Stop::Run)?);
        let absents = ints(outcome.get(absents).map_err(Stop::Run)?);
        let failed = ints(outcome.get(failed).map_err(Stop::Run)?);
        let statuses = ints(outcome.get(statuses).map_err(Stop::Run)?);
        let done = usize::try_from(outcome.get(done).map_err(Stop::Run)?).unwrap_or(0);
        let overflowed = outcome.get(overflowed).map_err(Stop::Run)?;
        if outcome.get(deeper).map_err(Stop::Run)? || outcome.get(dropped).map_err(Stop::Run)? {
            self.truncated = true;
        }
        for status in &statuses {
            let code = HRESULT(*status);
            if !is_gone(code) {
                return Err(Stop::Read(Error::Uia(windows::core::Error::from(code))));
            }
            self.truncated = true;
        }
        // Each queue place's element, listed, with its level; `None` for
        // one left out: a child of an element whose children failed, or
        // were not all read, or that was left out itself, or one past the
        // node cap.
        let mut places: Vec<Option<Waiting>> = taken.into_iter().map(Some).collect();
        for ((value, parent), absent) in found.into_iter().zip(parents).zip(absents) {
            let parent = usize::try_from(parent).unwrap_or(usize::MAX);
            let finished = parent < done
                && !failed
                    .iter()
                    .any(|&index| usize::try_from(index) == Ok(parent));
            let under = places
                .get(parent)
                .and_then(Option::as_ref)
                .map(|waiting| (waiting.listed, waiting.depth));
            let place = match (value, under, finished) {
                (Value::Element(element), Some((listed, depth)), true) => {
                    if self.listed.len() >= self.query.max_nodes {
                        self.truncated = true;
                        None
                    } else {
                        let child = self.query.fill.claim(element, absent_record(absent));
                        let at = self.listed.len();
                        self.listed.push(Listed {
                            snapshot: child.snapshot(self.query.registry),
                            children: Vec::new(),
                        });
                        self.listed[listed].children.push(at);
                        Some(Waiting {
                            listed: at,
                            live: child.live().clone(),
                            depth: depth + 1,
                        })
                    }
                }
                _ => None,
            };
            places.push(place);
        }
        // Every listed element whose children the run did not read waits
        // again, ahead of those it did not take, in queue order; the one it
        // stopped inside, when its children did not fit, is read
        // classically now.
        let stopped_inside = places
            .get(done)
            .and_then(Option::as_ref)
            .map(|waiting| waiting.listed)
            .filter(|_| overflowed);
        let again: Vec<Waiting> = places.into_iter().skip(done).flatten().collect();
        for waiting in again.into_iter().rev() {
            self.waiting.push_front(waiting);
        }
        if let Some(listed) = stopped_inside
            && self
                .waiting
                .front()
                .is_some_and(|waiting| waiting.listed == listed)
        {
            self.read_one_classically(uia).map_err(Stop::Read)?;
        }
        Ok(())
    }

    /// The listed elements as a tree, and whether it is incomplete.
    fn finish(self) -> (TreeNode, bool) {
        let mut listed: Vec<Option<Listed>> = self.listed.into_iter().map(Some).collect();
        (assemble(&mut listed, 0), self.truncated)
    }
}

/// The tree under the listed element `at`, taken out of `listed`.
fn assemble(listed: &mut [Option<Listed>], at: usize) -> TreeNode {
    let Some(node) = listed[at].take() else {
        unreachable!("each element is listed under one parent");
    };
    TreeNode {
        snapshot: node.snapshot,
        children: node
            .children
            .into_iter()
            .map(|child| assemble(listed, child))
            .collect(),
    }
}
