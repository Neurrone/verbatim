//! An application's outpost generation as the supervisor's owner thread
//! knows it (`docs/design/ownership.md`): one state per stage of its life, each holding only what
//! that stage has, and each transition a method that consumes the state
//! and returns the next with the notices to send, in order. Leaving
//! `Ready` emits the listener's `HookRemoved` in the same step, so no path
//! out of it can forget it, and no sentinel generation is ever stored.
//!
//! Generic over the child, so the transitions are tested with `()`.

#![forbid(unsafe_code)]

use std::time::Instant;

use verbatim_model::{OutpostId, Pid};

use super::EndReason;

/// A child's process from its launch: still being launched, or running.
/// The listener and the spares waiting, which have no generations, live
/// this pair of states.
#[derive(Debug)]
pub enum Run<C> {
    /// The launch helper is still at work.
    Launching,
    /// Its launch has reported.
    Running(C),
}

impl<C> Run<C> {
    /// The running child.
    pub fn running(&self) -> Option<&C> {
        match self {
            Run::Running(child) => Some(child),
            Run::Launching => None,
        }
    }

    /// The running child, taken.
    pub fn into_running(self) -> Option<C> {
        match self {
            Run::Running(child) => Some(child),
            Run::Launching => None,
        }
    }
}

/// What a running generation's liveness is judged by.
#[derive(Clone, Copy, Debug)]
pub struct Live {
    /// When it last answered a ping.
    pub last_pong_at: Instant,
    /// How many of its workers are abandoned to calls past their deadline.
    pub abandoned: usize,
    /// Whether a check of whether its application is hung is running.
    pub hang_check: bool,
}

impl Live {
    fn new(now: Instant) -> Self {
        Self {
            last_pong_at: now,
            abandoned: 0,
            hang_check: false,
        }
    }
}

/// What a given spare's generation keeps until its `Ready`.
#[derive(Debug, Default)]
pub struct GivenSpare {
    /// Its last fault, such as its hook failing.
    pub fault: Option<String>,
}

/// A generation that failed before its `Ready`, whose replacement waits
/// for the failure's backoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unready {
    /// The generation that failed.
    pub outpost: OutpostId,
    /// Why it failed.
    pub reason: EndReason,
    /// Whether the app heard it started, and so hears it ended.
    pub started: bool,
}

/// One application's outpost generation.
#[derive(Debug)]
pub enum Generation<C> {
    /// A launch for the application is at work.
    Launching {
        /// The generation being launched.
        outpost: OutpostId,
    },
    /// A spare was given whose launch has not reported.
    SpareGiven {
        /// The spare given.
        outpost: OutpostId,
    },
    /// Running, not yet `Ready`. `spare` is a given spare's, with its last
    /// fault, which a failure before its `Ready` is logged with.
    Starting {
        /// The generation.
        outpost: OutpostId,
        /// Its process.
        child: C,
        /// A given spare's record, `None` for an outpost launched for the
        /// application.
        spare: Option<GivenSpare>,
        /// What its liveness is judged by.
        live: Live,
    },
    /// Its hook is in.
    Ready {
        /// The generation.
        outpost: OutpostId,
        /// Its process.
        child: C,
        /// What its liveness is judged by.
        live: Live,
    },
    /// No generation, while a failure's backoff runs, with the generation
    /// that failed before its `Ready`, if any.
    Waiting {
        /// The generation that failed before its `Ready`, if any.
        unready: Option<Unready>,
    },
}

/// The generation a record is given next.
pub enum Next<C> {
    /// A spare, its process with it once its launch has reported.
    Spare {
        /// The spare.
        outpost: OutpostId,
        /// Its process, once its launch has reported.
        child: Option<C>,
    },
    /// An outpost to launch for the application.
    Launch {
        /// The outpost to launch.
        outpost: OutpostId,
    },
}

/// What a transition has the owner do, in order.
#[derive(Debug)]
pub enum Notice {
    /// Tell the listener the generation's hook is in.
    HookInstalled {
        /// The application.
        pid: Pid,
        /// The generation whose hook is in.
        outpost: OutpostId,
    },
    /// Tell the listener the application's hook is gone.
    HookRemoved {
        /// The application.
        pid: Pid,
    },
    /// Send the given spare `outpost` its application, and read its pipe
    /// as the application's.
    Assign {
        /// The spare given.
        outpost: OutpostId,
    },
    /// Launch an outpost for the application as `outpost`.
    Launch {
        /// The outpost to launch.
        outpost: OutpostId,
    },
    /// Tell the app, which the owner does as a sealed message of its own.
    App(Lifecycle),
}

/// A lifecycle notice for the app, as the typestates make it: plain data,
/// which only the supervisor's owner turns into the message the app takes
/// as the supervisor's own (`docs/design/logic-foundations.md`, "The
/// seal"). Each variant is one of the owner's messages, with its fields in
/// the same order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// An outpost generation started.
    Started {
        /// The new generation.
        outpost: OutpostId,
        /// The application it watches.
        target_pid: Pid,
    },
    /// An outpost generation ended.
    Ended {
        /// The generation that ended.
        outpost: OutpostId,
        /// The application it watched.
        target_pid: Pid,
        /// Why it ended.
        reason: EndReason,
        /// Whether a next generation follows for the application.
        follows: bool,
    },
    /// The generation `outpost`, given to `target_pid`, failed before its
    /// `Ready`, and the generation `next` takes its place.
    Regenerated {
        /// The generation that failed.
        outpost: OutpostId,
        /// The generation that takes its place.
        next: OutpostId,
        /// The application.
        target_pid: Pid,
    },
    /// No outpost watches `target_pid` now, nor will one until something
    /// asks for one again.
    NotWatched {
        /// The application.
        target_pid: Pid,
    },
}

/// What a failure left.
pub struct Failed<C> {
    /// The record's state from now on.
    pub next: Generation<C>,
    /// The generation that failed.
    pub outpost: OutpostId,
    /// Its process, to end, when it had one.
    pub child: Option<C>,
    /// Whether it had said it was ready.
    pub ready: bool,
    /// A given spare's last fault before its `Ready`.
    pub fault: Option<String>,
    /// The notices of its end.
    pub notices: Vec<Notice>,
}

/// What a removal left.
pub struct Removed<C> {
    /// The generation's process, to end, when it had one.
    pub child: Option<C>,
    /// The generation, whose writer goes, when there was one.
    pub outpost: Option<OutpostId>,
    /// Whether it had said it was ready.
    pub ready: bool,
    /// The notices of its end.
    pub notices: Vec<Notice>,
}

impl<C> Default for Generation<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> Generation<C> {
    /// A record's state before its first generation.
    #[must_use]
    pub fn new() -> Self {
        Generation::Waiting { unready: None }
    }

    /// The current generation, if there is one.
    pub fn outpost(&self) -> Option<OutpostId> {
        match self {
            Generation::Launching { outpost }
            | Generation::SpareGiven { outpost }
            | Generation::Starting { outpost, .. }
            | Generation::Ready { outpost, .. } => Some(*outpost),
            Generation::Waiting { .. } => None,
        }
    }

    /// The running generation's process.
    pub fn child(&self) -> Option<&C> {
        match self {
            Generation::Starting { child, .. } | Generation::Ready { child, .. } => Some(child),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// The ready generation's process.
    pub fn ready_child(&self) -> Option<&C> {
        match self {
            Generation::Ready { child, .. } => Some(child),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Starting { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// The ready generation's process when it is `outpost`: where the
    /// listener's "forwarding ended" for `outpost` goes.
    pub fn forwarding_target(&self, outpost: OutpostId) -> Option<&C> {
        match self {
            Generation::Ready {
                outpost: ready,
                child,
                ..
            } if *ready == outpost => Some(child),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Starting { .. }
            | Generation::Ready { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// The running generation's liveness.
    pub fn live_mut(&mut self) -> Option<&mut Live> {
        match self {
            Generation::Starting { live, .. } | Generation::Ready { live, .. } => Some(live),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// The running generation's liveness.
    pub fn live(&self) -> Option<&Live> {
        match self {
            Generation::Starting { live, .. } | Generation::Ready { live, .. } => Some(live),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// Whether its hook is in.
    pub fn is_ready(&self) -> bool {
        matches!(self, Generation::Ready { .. })
    }

    /// Whether its process is still being launched: it has a generation
    /// and no child yet.
    pub fn is_launching(&self) -> bool {
        matches!(
            self,
            Generation::Launching { .. } | Generation::SpareGiven { .. }
        )
    }

    /// Whether it is a spare given whose launch has not reported.
    pub fn is_spare_given(&self) -> bool {
        matches!(self, Generation::SpareGiven { .. })
    }

    /// Where a given spare's fault before its `Ready` is kept.
    pub fn spare_fault(&mut self) -> Option<&mut Option<String>> {
        match self {
            Generation::Starting {
                spare: Some(GivenSpare { fault }),
                ..
            } => Some(fault),
            Generation::Launching { .. }
            | Generation::SpareGiven { .. }
            | Generation::Starting { .. }
            | Generation::Ready { .. }
            | Generation::Waiting { .. } => None,
        }
    }

    /// From `Waiting`, at a record's start or a backoff's end: `pid` is
    /// given `next`. When the generation before failed before its `Ready`,
    /// the app hears first that `next` takes its place, then the failed
    /// one's end if it heard it started; then a spare running is assigned
    /// and announced, and an outpost to launch is launched. Any other state
    /// is kept, with nothing to do.
    pub fn given(self, pid: Pid, next: Next<C>, now: Instant) -> (Self, Vec<Notice>) {
        let Generation::Waiting { unready } = self else {
            debug_assert!(false, "a generation given while one is current");
            return (self, Vec::new());
        };
        let next_outpost = match &next {
            Next::Spare { outpost, .. } | Next::Launch { outpost } => *outpost,
        };
        let mut notices = Vec::new();
        if let Some(failed) = unready {
            notices.push(Notice::App(Lifecycle::Regenerated {
                outpost: failed.outpost,
                next: next_outpost,
                target_pid: pid,
            }));
            if failed.started {
                notices.push(ended(failed.outpost, pid, failed.reason, true));
            }
        }
        let state = match next {
            Next::Spare {
                outpost,
                child: Some(child),
            } => {
                notices.push(Notice::Assign { outpost });
                notices.push(started(outpost, pid));
                Generation::Starting {
                    outpost,
                    child,
                    spare: Some(GivenSpare::default()),
                    live: Live::new(now),
                }
            }
            Next::Spare {
                outpost,
                child: None,
            } => Generation::SpareGiven { outpost },
            Next::Launch { outpost } => {
                notices.push(Notice::Launch { outpost });
                Generation::Launching { outpost }
            }
        };
        (state, notices)
    }

    /// The launch of an outpost for `pid` reported `child`: it is running,
    /// and the app hears it started. In any other state the child is given
    /// back.
    ///
    /// # Errors
    ///
    /// The state and the child, given back, in any state but `Launching`.
    pub fn launched(
        self,
        pid: Pid,
        child: C,
        now: Instant,
    ) -> Result<(Self, Vec<Notice>), (Self, C)> {
        match self {
            Generation::Launching { outpost } => Ok((
                Generation::Starting {
                    outpost,
                    child,
                    spare: None,
                    live: Live::new(now),
                },
                vec![started(outpost, pid)],
            )),
            other => Err((other, child)),
        }
    }

    /// The launch of the spare given to `pid` reported `child`: it is
    /// assigned and announced. In any other state the child is given back.
    ///
    /// # Errors
    ///
    /// The state and the child, given back, in any state but `SpareGiven`.
    pub fn assigned(
        self,
        pid: Pid,
        child: C,
        now: Instant,
    ) -> Result<(Self, Vec<Notice>), (Self, C)> {
        match self {
            Generation::SpareGiven { outpost } => Ok((
                Generation::Starting {
                    outpost,
                    child,
                    spare: Some(GivenSpare::default()),
                    live: Live::new(now),
                },
                vec![Notice::Assign { outpost }, started(outpost, pid)],
            )),
            other => Err((other, child)),
        }
    }

    /// The running generation `outpost` of `pid`'s said it is ready: its
    /// hook is in, which the listener is told. Any other state is kept.
    pub fn ready(self, pid: Pid, outpost: OutpostId) -> (Self, Vec<Notice>) {
        match self {
            Generation::Starting {
                outpost: starting,
                child,
                live,
                ..
            } if starting == outpost => (
                Generation::Ready {
                    outpost,
                    child,
                    live,
                },
                vec![Notice::HookInstalled { pid, outpost }],
            ),
            other => (other, Vec::new()),
        }
    }

    /// The current generation of `pid`'s failed, for `reason`, and the
    /// record is kept for a next generation, at once or after a backoff: a
    /// ready one's hook goes, and the app hears it ended, a generation
    /// following; one that failed before its `Ready` is kept to be replaced
    /// when the next is given. An error while `Waiting`, which has no
    /// generation to fail.
    ///
    /// # Errors
    ///
    /// The state, kept, while `Waiting`.
    pub fn failed(self, pid: Pid, reason: EndReason) -> Result<Failed<C>, Self> {
        let (outpost, child, ready, fault) = match self {
            Generation::Ready { outpost, child, .. } => (outpost, Some(child), true, None),
            Generation::Starting {
                outpost,
                child,
                spare,
                ..
            } => (
                outpost,
                Some(child),
                false,
                spare.and_then(|spare| spare.fault),
            ),
            Generation::Launching { outpost } | Generation::SpareGiven { outpost } => {
                (outpost, None, false, None)
            }
            waiting @ Generation::Waiting { .. } => return Err(waiting),
        };
        let (next, notices) = if ready {
            (
                Generation::Waiting { unready: None },
                vec![
                    Notice::HookRemoved { pid },
                    ended(outpost, pid, reason, true),
                ],
            )
        } else {
            (
                Generation::Waiting {
                    unready: Some(Unready {
                        outpost,
                        reason,
                        started: child.is_some(),
                    }),
                },
                Vec::new(),
            )
        };
        Ok(Failed {
            next,
            outpost,
            child,
            ready,
            fault,
            notices,
        })
    }

    /// `pid`'s record is removed: its application exited, its outpost
    /// retired, or Verbatim is exiting. A ready generation's hook goes.
    /// With `reason`, the app hears the end of every generation it heard
    /// started, the unready one waiting for its replacement among them, no
    /// generation following, and then that the application is not
    /// watched; without one, Verbatim is exiting and the app hears
    /// nothing.
    pub fn removed(self, pid: Pid, reason: Option<EndReason>) -> Removed<C> {
        let mut notices = Vec::new();
        let (child, outpost, ready, heard) = match self {
            Generation::Ready { outpost, child, .. } => {
                notices.push(Notice::HookRemoved { pid });
                (Some(child), Some(outpost), true, Some(outpost))
            }
            Generation::Starting { outpost, child, .. } => {
                (Some(child), Some(outpost), false, Some(outpost))
            }
            Generation::Launching { outpost } | Generation::SpareGiven { outpost } => {
                (None, Some(outpost), false, None)
            }
            Generation::Waiting { unready } => (
                None,
                None,
                false,
                unready
                    .filter(|unready| unready.started)
                    .map(|unready| unready.outpost),
            ),
        };
        if let Some(reason) = reason {
            if let Some(outpost) = heard {
                notices.push(ended(outpost, pid, reason, false));
            }
            notices.push(Notice::App(Lifecycle::NotWatched { target_pid: pid }));
        }
        Removed {
            child,
            outpost,
            ready,
            notices,
        }
    }
}

fn started(outpost: OutpostId, target_pid: Pid) -> Notice {
    Notice::App(Lifecycle::Started {
        outpost,
        target_pid,
    })
}

fn ended(outpost: OutpostId, target_pid: Pid, reason: EndReason, follows: bool) -> Notice {
    Notice::App(Lifecycle::Ended {
        outpost,
        target_pid,
        reason,
        follows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: Pid = Pid(40);

    /// Each notice, named.
    fn named(notices: &[Notice]) -> Vec<String> {
        notices
            .iter()
            .map(|notice| match notice {
                Notice::HookInstalled { outpost, .. } => format!("HookInstalled {}", outpost.0),
                Notice::HookRemoved { .. } => "HookRemoved".to_owned(),
                Notice::Assign { outpost } => format!("Assign {}", outpost.0),
                Notice::Launch { outpost } => format!("Launch {}", outpost.0),
                Notice::App(Lifecycle::Started { outpost, .. }) => {
                    format!("Started {}", outpost.0)
                }
                Notice::App(Lifecycle::Ended {
                    outpost,
                    reason,
                    follows,
                    ..
                }) => format!("Ended {} {reason}, follows {follows}", outpost.0),
                Notice::App(Lifecycle::Regenerated { outpost, next, .. }) => {
                    format!("Regenerated {} as {}", outpost.0, next.0)
                }
                Notice::App(Lifecycle::NotWatched { .. }) => "NotWatched".to_owned(),
            })
            .collect()
    }

    fn spare(outpost: u64) -> Next<()> {
        Next::Spare {
            outpost: OutpostId(outpost),
            child: Some(()),
        }
    }

    /// A generation given spare `outpost`, running.
    fn starting(outpost: u64) -> Generation<()> {
        Generation::new()
            .given(PID, spare(outpost), Instant::now())
            .0
    }

    /// A generation given spare `outpost`, ready.
    fn ready(outpost: u64) -> Generation<()> {
        starting(outpost).ready(PID, OutpostId(outpost)).0
    }

    fn failed(generation: Generation<()>, reason: EndReason) -> Failed<()> {
        generation
            .failed(PID, reason)
            .unwrap_or_else(|_| panic!("a generation fails"))
    }

    #[test]
    fn a_spare_given_and_failed_before_its_ready_is_regenerated_then_ended_then_the_next_started() {
        let failure = failed(starting(3), EndReason::Exited);
        assert_eq!(named(&failure.notices), Vec::<String>::new());
        let (next, notices) = failure.next.given(PID, spare(4), Instant::now());
        assert_eq!(
            named(&notices),
            [
                "Regenerated 3 as 4",
                "Ended 3 exited, follows true",
                "Assign 4",
                "Started 4"
            ]
        );
        assert_eq!(next.outpost(), Some(OutpostId(4)));
    }

    #[test]
    fn a_spare_given_and_failed_with_its_launch_pending_has_no_end() {
        let given = Generation::<()>::new()
            .given(
                PID,
                Next::Spare {
                    outpost: OutpostId(3),
                    child: None,
                },
                Instant::now(),
            )
            .0;
        assert!(given.is_spare_given());
        let failure = failed(given, EndReason::Exited);
        let (_, notices) = failure.next.given(PID, spare(4), Instant::now());
        assert_eq!(
            named(&notices),
            ["Regenerated 3 as 4", "Assign 4", "Started 4"]
        );
    }

    #[test]
    fn a_ready_generations_failure_with_no_wait_removes_its_hook_ends_it_and_starts_the_next() {
        let failure = failed(ready(3), EndReason::Killed);
        assert_eq!(
            named(&failure.notices),
            ["HookRemoved", "Ended 3 killed, follows true"]
        );
        // No wait: the next generation is given at once.
        let (next, notices) = failure.next.given(PID, spare(4), Instant::now());
        assert_eq!(named(&notices), ["Assign 4", "Started 4"]);
        let (_, notices) = next.ready(PID, OutpostId(4));
        assert_eq!(named(&notices), ["HookInstalled 4"]);
    }

    #[test]
    /// The wait itself is the backoff's, which the supervisor's process
    /// tests keep (`tests/spares.rs`).
    fn a_ready_generations_failure_leaves_no_generation_and_removes_its_hook_then_ends_it_with_a_follower()
     {
        let failure = failed(ready(3), EndReason::Exited);
        assert_eq!(
            named(&failure.notices),
            ["HookRemoved", "Ended 3 exited, follows true"]
        );
        assert!(
            matches!(failure.next, Generation::Waiting { unready: None }),
            "no generation while it waits"
        );
    }

    #[test]
    fn a_target_exit_while_waiting_ends_the_unready_generation_heard_started_then_is_not_watched() {
        let waiting = failed(starting(3), EndReason::Exited).next;
        let removed = waiting.removed(PID, Some(EndReason::TargetExited));
        assert_eq!(
            named(&removed.notices),
            ["Ended 3 target exited, follows false", "NotWatched"]
        );
        let unheard = Generation::<()>::new().removed(PID, Some(EndReason::TargetExited));
        assert_eq!(named(&unheard.notices), ["NotWatched"]);
    }

    #[test]
    fn a_retirement_removes_the_hook_ends_the_generation_and_is_not_watched() {
        let removed = ready(3).removed(PID, Some(EndReason::Retired));
        assert_eq!(
            named(&removed.notices),
            [
                "HookRemoved",
                "Ended 3 retired, follows false",
                "NotWatched"
            ]
        );
        let quiet = ready(3).removed(PID, None);
        assert_eq!(named(&quiet.notices), ["HookRemoved"]);
    }

    #[test]
    fn forwarding_ended_naming_another_generation_is_not_relayed() {
        let generation = ready(3);
        assert!(generation.forwarding_target(OutpostId(3)).is_some());
        assert!(generation.forwarding_target(OutpostId(2)).is_none());
        assert!(starting(3).forwarding_target(OutpostId(3)).is_none());
    }
}
