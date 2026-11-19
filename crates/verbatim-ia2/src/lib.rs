//! MSAA/IA2 client stack (architecture section 4).
//!
//! `WinEvent`s arrive via out-of-context hooks scoped to the outpost's target
//! process; from the `IAccessible` an event names, the outpost's worker reads
//! name/role/value/state and maps into the normalized model. Every property is
//! a cross-process COM round trip, so all acquisition runs on the outpost's
//! deadline-guarded worker (risk R1). The `IAccessible2` interfaces (text,
//! hypertext, relations) are not implemented yet; roadmap M7 adds them in
//! [`acquire`].
//!
//! The pieces:
//!
//! - [`WinEventHook`] — the out-of-context hooks, installed on an event thread
//!   and delivered through its message loop, for a caller-chosen set of event
//!   kinds ([`APP_SUBSCRIPTIONS`] for a per-app outpost, or
//!   [`LISTENER_SUBSCRIPTIONS`] globally for the focus listener, decision D13).
//! - [`acquire`] — acquisition and mapping on the worker, including the
//!   focus-now read.
//! - [`NodeIdRegistry`] — [`NodeId`](verbatim_model::NodeId)s for MSAA
//!   objects, with the object kept behind each, sharing a mint counter with
//!   UIA.
//! - [`map`] — role and state mapping into the model.
//! - [`class`] — a window's class name normalized as NVDA normalizes it
//!   (`verbatim_reader_rules::class`), which every class-based rule
//!   compares.
//! - [`event_thread`] — the thread that installs the `WinEvent` hooks and
//!   runs the message loop they are delivered through, which an outpost
//!   and the focus listener each run.
//! - [`calls`] — the per-thread count of the cross-process calls
//!   [`acquire`] makes, which the outpost's worker takes around each entry
//!   it handles.
//! - [`dialog`] — the objects a dialog's own text is gathered from, read
//!   as the gathering asks for each property.
//! - [`edit`] — the standard Win32 edit and rich edit controls' text,
//!   through their window messages (milestone M4), as NVDA's
//!   `EditTextInfo` reads them.
//! - [`uniscribe`] — characters and words as Uniscribe breaks them, which
//!   NVDA's `EditTextInfo` uses for an edit control's characters and a
//!   plain edit control's words.
//!
//! `unsafe` code lives only in the private modules that wrap the platform:
//! `accessible`, the safe MSAA wrappers (an `IAccessible` with its child id,
//! and the `oleacc` acquisition functions) that [`acquire`] is written
//! against; `window`, the window functions and the list view and tree view
//! messages; `hook`, the `WinEvent` hooks; [`event_thread`], its message
//! loop; [`edit`], the edit
//! controls' messages and the target memory some of them need; and
//! [`uniscribe`], its calls into Uniscribe and the C runtime.
//! [`acquire`] itself is safe code.

mod accessible;
pub mod acquire;
pub mod calls;
pub mod class;
mod com;
pub mod dialog;
pub mod edit;
pub mod equality;
pub mod event_thread;
mod hook;
pub mod list_view;
pub mod map;
mod registry;
pub mod uniscribe;
mod window;

pub use window::classic_bar_percent;

pub use com::CHILDID_SELF;
pub use hook::{
    APP_SUBSCRIPTIONS, CONSOLE_SUBSCRIPTIONS, LISTENER_SUBSCRIPTIONS, SyncMarker,
    SyncMarkerCallback, WinEvent, WinEventCallback, WinEventHook, WinEventKind, WindowCache,
    WindowShownCallback, event_clock, is_shell_foreground_class, set_window_shown, suspend_kinds,
    tell_kind,
};
pub use registry::{
    Facts, Found, Held, Interning, Kept, KeptObject, MsaaKey, NodeIdRegistry, Released,
};
