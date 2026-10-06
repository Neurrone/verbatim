//! MSAA/IA2 client stack (architecture section 4).
//!
//! `WinEvent`s arrive via out-of-context hooks scoped to the outpost's target
//! process; from the `IAccessible` an event names, the outpost's worker reads
//! name/role/value/state and maps into the normalized model. Every property is
//! a cross-process COM round trip, so all acquisition runs on the outpost's
//! deadline-guarded worker (risk R1). The `IAccessible2` interfaces (text,
//! hypertext, relations) are not implemented yet; roadmap M6 adds them in
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
//! - [`calls`] — the per-thread count of the cross-process calls
//!   [`acquire`] makes, which the outpost's worker takes around each entry
//!   it handles.

pub mod acquire;
pub mod calls;
mod com;
mod hook;
pub mod map;
mod registry;

pub use com::CHILDID_SELF;
pub use hook::{
    APP_SUBSCRIPTIONS, LISTENER_SUBSCRIPTIONS, WinEventCallback, WinEventHook, WinEventKind,
};
pub use registry::{MsaaKey, NodeIdRegistry, Released};
