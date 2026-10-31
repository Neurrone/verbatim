//! The arbitrator's record of probed verdicts
//! (`docs/design/ownership.md`): each window's
//! kept verdicts, held by the reducer and given to every program, and to
//! the event thread through the push-time copies, as an immutable snapshot
//! the reducer replaces on change, so a copy costs a reference count.
//!
//! A program records the verdicts it probes in its commits, posted as each
//! probe returns, and applies each to its own copy, so it uses them for the
//! rest of its walk. The reducer records a probe only from the worker
//! generation in charge, but a probe that timed out, and a destroyed
//! window's forgetting, from any generation, so a window is not probed
//! again more often than NVDA probes it
//! (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
//! fact", the verdicts).
//!
//! What a verdict is, how its keep is judged by the observation of the
//! event it judges, and why each window keeps its previous verdict are the
//! arbitrator's (`crate::arbitration`); the record holds no class list and
//! no override, which stay configuration. A window's owning thread, which
//! a reused handle is told apart by, is read by the caller and passed in.

use std::collections::HashMap;

/// A kept probe result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probed {
    /// The window has a UIA provider, for its whole lifetime.
    Uia,
    /// No provider was found, kept until this observation, the probe's end
    /// and the negative verdict's lifetime after it.
    NotUia(u64),
    /// The probe did not answer in time: no provider, kept until this
    /// observation as [`Probed::NotUia`] is.
    TimedOut(u64),
    /// The window has a provider NVDA does not use, for its whole lifetime.
    Excluded,
}

impl Probed {
    /// Whether it holds for the window's lifetime.
    fn lasts(self) -> bool {
        matches!(self, Self::Uia | Self::Excluded)
    }
}

/// One probe's verdict on a window, as a program commits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The window probed.
    pub hwnd: isize,
    /// The thread that owned it when it was probed.
    pub owner: u32,
    /// The observation the judgement that probed was made at: the verdict
    /// judges no event observed before it.
    pub from_us: u64,
    /// What the probe found.
    pub probed: Probed,
}

/// A change of the record, as a program commits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictChange {
    /// A probe's verdict, kept.
    Probed(Probe),
    /// The window was destroyed: its verdicts are forgotten, as its handle
    /// may be reused by an unrelated window.
    Forget(isize),
}

impl VerdictChange {
    /// Whether the reducer records it from a generation no longer in
    /// charge: a probe that timed out, and a destroyed window's forgetting.
    pub(crate) fn kept_from_any_generation(&self) -> bool {
        matches!(
            self,
            Self::Forget(_)
                | Self::Probed(Probe {
                    probed: Probed::TimedOut(_),
                    ..
                })
        )
    }
}

/// One kept verdict on a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kept {
    owner: u32,
    from_us: u64,
    probed: Probed,
}

impl Kept {
    /// The verdict, when it judges an event observed at `at_us`: observed
    /// at or after its probe began and before its keep ended.
    fn judges(&self, at_us: u64) -> Option<Probed> {
        if at_us < self.from_us {
            return None;
        }
        match self.probed {
            Probed::NotUia(until) | Probed::TimedOut(until) if at_us >= until => None,
            probed => Some(probed),
        }
    }
}

/// A window's kept verdicts: the newest, and the one before it for the
/// events observed before the newest's probe began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    current: Kept,
    previous: Option<Kept>,
}

/// Every window's kept verdicts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdicts {
    windows: HashMap<isize, Window>,
}

impl Verdicts {
    /// The verdict kept for an event of `hwnd`'s observed at `at_us`, the
    /// window now owned by thread `owner`: the newest, for an observation
    /// at or after its probe began, else the one before it; `None` when
    /// that one's keep has ended or it began later still, or when the
    /// window is now owned by another thread than the one it was recorded
    /// for, as its handle has been reused.
    #[must_use]
    pub(crate) fn kept_at(&self, hwnd: isize, owner: u32, at_us: u64) -> Option<Probed> {
        let window = self.windows.get(&hwnd)?;
        if window.current.owner != owner {
            return None;
        }
        if at_us >= window.current.from_us {
            window.current.judges(at_us)
        } else {
            window.previous.and_then(|previous| previous.judges(at_us))
        }
    }

    /// Whether `hwnd`, now owned by thread `owner`, has a newest verdict
    /// that holds for its lifetime.
    #[must_use]
    pub(crate) fn lasts(&self, hwnd: isize, owner: u32) -> bool {
        self.windows
            .get(&hwnd)
            .is_some_and(|window| window.current.owner == owner && window.current.probed.lasts())
    }

    /// Applies `change`; whether the record changed. A probe judging from
    /// an observation at or after the newest's becomes the newest, the one
    /// it replaces kept for earlier observations; one judging from an
    /// earlier observation becomes the one before it. A probe that timed
    /// out leaves a verdict that holds for the window's lifetime as it is,
    /// since a provider does not go away.
    pub(crate) fn apply(&mut self, change: &VerdictChange) -> bool {
        let probe = match change {
            VerdictChange::Forget(hwnd) => return self.windows.remove(hwnd).is_some(),
            VerdictChange::Probed(probe) => *probe,
        };
        if matches!(probe.probed, Probed::TimedOut(_)) && self.lasts(probe.hwnd, probe.owner) {
            return false;
        }
        let kept = Kept {
            owner: probe.owner,
            from_us: probe.from_us,
            probed: probe.probed,
        };
        match self.windows.get_mut(&probe.hwnd) {
            Some(window) if window.current.owner == kept.owner => {
                if kept.from_us >= window.current.from_us {
                    window.previous = Some(std::mem::replace(&mut window.current, kept));
                } else {
                    window.previous = Some(kept);
                }
            }
            _ => {
                self.windows.insert(
                    probe.hwnd,
                    Window {
                        current: kept,
                        previous: None,
                    },
                );
            }
        }
        true
    }
}
