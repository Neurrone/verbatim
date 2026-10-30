//! A focus walk's progress, which the worker shares with the watchdog so
//! that the watchdog publishes the focus at the walk's bound should one of
//! the walk's calls still be under way then, or the worker itself should
//! the walk return past its bound unpublished
//! (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
//! fallback"), as the listener's watchdog publishes a stuck UIA focus read
//! (`crate::focus_read`, `ReadProgress`).
//!
//! The worker arms it once a focus has passed every check NVDA makes before
//! it queues a focus and has been read, for an MSAA focus and the gap's
//! read, unless the walk is already past its bound: the focus, its event,
//! and the bound. The walk then tells it each
//! ancestor it reads and the window from which plain windows would
//! complete the chain ([`super::read::WalkStep`]). Plain data, kept under
//! the watch lock, so the watchdog's thread can make the chain from it with
//! local calls ([`chain_at_bound`]).

use std::time::{Duration, Instant};

use verbatim_model::{Backend, NodeSnapshot, TraceId};

use crate::protocol::EventTiming;

use super::Context;
use super::intake::Object;
use super::read::{WalkStep, plain_windows_from};

/// What a focus is reported with besides its chain: as the worker would
/// report it.
#[derive(Clone)]
pub(super) struct BoundFocus {
    /// The trace of the event it came from.
    pub(super) trace: TraceId,
    /// The observation it is stamped with.
    pub(super) observed_at_us: u64,
    /// Its entry's timing so far.
    pub(super) timing: EventTiming,
    /// The batch its entry belongs to.
    pub(super) batch: u64,
    /// The backend it was read through.
    pub(super) backend: Backend,
    /// The window it is reported in.
    pub(super) window: Option<isize>,
    /// The focus as read.
    pub(super) node: NodeSnapshot,
    /// Its object, which the outpost follows.
    pub(super) object: Option<Object>,
    /// For an MSAA focus, its event's address and whether it was acquired
    /// as a simple child ([`super::held::Focus`]).
    pub(super) msaa_event: Option<((isize, i32, i32), bool)>,
}

/// A focus walk's progress (the module's documentation).
#[derive(Clone)]
pub(super) struct FocusProgress {
    /// The worker it is the walk of.
    pub(super) generation: u64,
    /// Which arming of the walk it is, so a publication made from a copy
    /// claims only that walk.
    pub(super) armed: u64,
    /// The walk's bound, by the system's clock.
    pub(super) bound_at: Instant,
    /// The focus.
    pub(super) focus: BoundFocus,
    /// The ancestors read, nearest first.
    ancestors: Vec<NodeSnapshot>,
    /// The window from which plain windows, included, complete the chain;
    /// 0 for none.
    from: isize,
    /// The chain, outermost first, once the walk has ended.
    ended: Option<Vec<NodeSnapshot>>,
}

impl FocusProgress {
    /// The progress of a walk of `focus` that has `left` of its bound left,
    /// its chain completed by plain windows from `from` until it reads
    /// more.
    pub(super) fn new(generation: u64, focus: BoundFocus, from: isize, left: Duration) -> Self {
        Self {
            generation,
            armed: 0,
            bound_at: Instant::now() + left,
            focus,
            ancestors: Vec::new(),
            from,
            ended: None,
        }
    }

    /// Takes in what the walk has just read.
    pub(super) fn take_in(&mut self, step: &WalkStep<'_>) {
        match step {
            // Arming makes a new progress.
            WalkStep::Focus { .. } => {}
            WalkStep::Ancestor(ancestor, from) => {
                if let Some(ancestor) = ancestor {
                    self.ancestors.push((*ancestor).clone());
                }
                self.from = *from;
            }
            WalkStep::Chain(chain, Some(from)) => {
                self.ancestors = chain.iter().rev().cloned().collect();
                self.from = *from;
            }
            WalkStep::Chain(chain, None) => self.ended = Some(chain.to_vec()),
        }
    }
}

/// The focus's ancestors, outermost first, as a focus walk's progress
/// publishes them at its bound: the chain once the walk has ended, else the
/// ancestors read, completed above by plain windows from the window the
/// walk had reached, included, named by their text. The one function the
/// watchdog makes a focus's chain from. Local calls only.
pub(super) fn chain_at_bound(context: &Context, progress: &FocusProgress) -> Vec<NodeSnapshot> {
    if let Some(chain) = &progress.ended {
        return chain.clone();
    }
    plain_windows_from(context, progress.from)
        .into_iter()
        .chain(progress.ancestors.iter().rev().cloned())
        .collect()
}
