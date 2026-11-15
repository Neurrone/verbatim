//! The input desktop and the lock state (`docs/design/readers-tell-core-asks-build1.md`,
//! "The secure desktop and the lock state", Table 8), each handled at once
//! in the inbox.
//!
//! A switch to another input desktop, one Verbatim cannot open such as
//! the secure desktop, cuts speech, commits the sleeping focus (a focus
//! Core commits itself, with no source, whose `Follow` names nothing),
//! changes the hook view's desktop part so that the hook clears its
//! modifier record, drops the held keys from Core's stroke line, and asks
//! the old application nothing. NVDA: `winAPI/secureDesktop.py` 43 to 45,
//! 50, 55 to 70. A switch whose input desktop opens arms a 200 ms timer,
//! as NVDA does for every such switch (`IAccessibleHandler/__init__.py`
//! 796 to 806); when it comes due the read after the return joins the
//! back of the queue: the front application's current focus, with no
//! duplicate test and none of the ancestors Core already knows, spoken
//! with its window's title as NVDA's `gainFocus` after the sleeping focus
//! is (809 to 813).
//!
//! Core holds the lock state from the shell's session notifications and
//! its query at startup, unknown counting as locked (Verbatim-only:
//! security mitigation B3). Nothing reads it before M8.7, which brings
//! the rules that judge objects below the lock screen.

use serde::{Deserialize, Serialize};
use verbatim_model::Effect;
use verbatim_model::request::SessionLock;

use crate::focus::follow::FollowRequest;
use crate::focus::retake::CurrentFocus;
use crate::state::SrState;

/// How long after a switch to an input desktop that opens the read after
/// the return is asked: NVDA's `core.callLater(200, ...)`.
pub(crate) const RETURN_DELAY_US: u64 = 200_000;

/// Which input desktop is up, as Table 8's rows tell them apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum InputDesktop {
    /// The user's desktop.
    #[default]
    User,
    /// Another input desktop, which Verbatim cannot open: the sleeping
    /// focus is committed.
    Other,
    /// A switch whose input desktop opens was told; the read after the
    /// return is asked at `due_us`.
    ReturnPending {
        /// When the read after the return joins the queue, on the run's
        /// shared clock.
        due_us: u64,
    },
}

/// The input desktop, the lock state, and how many times the input
/// desktop has switched away, which the hook's view carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Desktop {
    /// The input desktop.
    pub(crate) input: InputDesktop,
    /// The session's lock state, as last told.
    pub(crate) lock: SessionLock,
    /// How many switches away from the user's desktop Core has handled:
    /// the hook clears its modifier record and its line when the number
    /// its view carries changes.
    pub(crate) switches_away: u32,
}

impl Desktop {
    /// Whether the session counts as locked: locked, or unknown.
    pub(crate) fn locked(self) -> bool {
        !matches!(self.lock, SessionLock::Unlocked)
    }
}

/// What a switch handled at once does beyond Core's state: what it says
/// and the `Follow`s it sends.
#[derive(Debug, Default)]
pub(crate) struct Switched {
    /// What it says.
    pub(crate) said: Vec<Effect>,
    /// The `Follow`s it sends.
    pub(crate) follows: Vec<FollowRequest>,
}

/// The input desktop switched at `now_us`; `open` when Verbatim could open
/// the new one, the user's (Table 8, columns A and B).
pub(crate) fn switched(state: &mut SrState, open: bool, now_us: u64) -> Switched {
    if open {
        state.desktop.input = InputDesktop::ReturnPending {
            due_us: now_us.saturating_add(RETURN_DELAY_US),
        };
        return Switched::default();
    }
    if state.desktop.input == InputDesktop::Other {
        return Switched::default();
    }
    state.desktop.input = InputDesktop::Other;
    state.desktop.switches_away = state.desktop.switches_away.wrapping_add(1);
    state.versions.commit_own(true);
    state.focus.sleep();
    // The keys held before the switch never reach the application: the
    // hook's line is cleared with its modifiers, and the front's
    // starting read is withdrawn as the queue settles.
    state.strokes.drop_unsent();
    Switched {
        said: vec![Effect::StopSpeech],
        follows: state.follows.committed(None, None),
    }
}

/// The session's lock state was told (Table 8, columns C, D and H).
pub(crate) fn lock(state: &mut SrState, lock: SessionLock) {
    state.desktop.lock = lock;
}

/// When the read after the return is due, if one is.
pub(crate) fn due(state: &SrState) -> Option<u64> {
    match state.desktop.input {
        InputDesktop::ReturnPending { due_us } => Some(due_us),
        InputDesktop::User | InputDesktop::Other => None,
    }
}

/// The time `now_us` came (Table 8, column G): when the read after the
/// return is due, the user's desktop is up again, and the read is
/// returned to join the back, naming the window in front, `front`;
/// nothing is read when no window is in front.
pub(crate) fn tick(
    state: &mut SrState,
    now_us: u64,
    front: Option<crate::event_queue::RequestWindow>,
) -> Option<CurrentFocus> {
    if due(state).is_none_or(|due_us| due_us > now_us) {
        return None;
    }
    state.desktop.input = InputDesktop::User;
    Some(crate::focus::retake::returned(state, front?))
}

#[cfg(test)]
#[path = "input_desktop_tests.rs"]
mod tests;
