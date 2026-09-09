//! Input (architecture section 5): the operating-system-free half.
//!
//! The swallow-or-pass decision for every keystroke runs in microseconds
//! against a read-only, lock-free snapshot of the gesture map; the hook that
//! feeds it only decides and enqueues, and gesture semantics run outside it,
//! accessibility scripts on the reducer thread and imperative commands in
//! the shell's gesture router.
//!
//! This crate holds the vocabulary ([`KeyEvent`], [`KeyDecision`], and the
//! [`keys`] name table shared with the control plane's key injection), the
//! pure decision [`state`] machine, the [`map`] of bound gestures, and the
//! [`scripts`] tables. It depends on nothing from the operating system, so
//! the whole of Verbatim's keyboard behaviour is testable with scripted key
//! streams. The thin, never-blocking hook thread that installs the real
//! `WH_KEYBOARD_LL` hook and drives the machine is `verbatim-input-windows`.

pub mod keys;
pub mod map;
pub mod scripts;
pub mod state;

pub use map::{GestureMap, SharedGestureMap};
pub use scripts::{KeyboardLayout, ScriptAction, bindings_for, gesture_map_for};
pub use state::{Decision, DecisionConfig, DecisionMachine, EmittedGesture};

/// One raw key transition as seen by the low-level hook, before any
/// interpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// Virtual-key code.
    pub vk: u16,
    /// Hardware scan code.
    pub scan_code: u32,
    /// The extended-key flag, which distinguishes navigation-cluster keys
    /// from their numpad twins (insert versus numpad insert).
    pub extended: bool,
    /// Set for injected input (`SendInput`), including Verbatim's own key
    /// injection; the decision logic treats injected keys like physical
    /// ones, NVDA's handle-injected-keys behavior.
    pub injected: bool,
    /// True for key down (and auto-repeat), false for key up.
    pub pressed: bool,
}

/// What the hook tells Windows to do with a key transition.
///
/// Swallowed keys never reach the focused application or the rest of the
/// OS — this is what keeps caps lock from toggling while it acts as the
/// Verbatim key, and it applies to the key-up of every consumed key too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDecision {
    /// Consume the transition; the application never sees it.
    Swallow,
    /// Pass the transition through unchanged.
    Pass,
}
