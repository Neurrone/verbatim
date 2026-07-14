//! MSAA/IA2 client stack (architecture section 4).
//!
//! `WinEvent`s arrive via out-of-context hooks scoped to the outpost's target
//! process; from the `IAccessible` an event names, the query pool reads
//! name/role/value/state and maps into the normalized model. Every property is
//! a cross-process COM round trip, so all acquisition runs on deadline-guarded
//! query-pool threads (risk R1). The `IAccessible2` interfaces (text,
//! hypertext, relations) are acquired via `IServiceProvider::QueryService` in
//! M3; that boundary is marked in [`acquire`].
//!
//! The pieces:
//!
//! - [`WinEventHook`] — the out-of-context focus/value/state/name hooks,
//!   installed on the outpost event thread and delivered through its message
//!   loop.
//! - [`acquire`] — query-pool acquisition and mapping, including the synthetic
//!   focus query.
//! - [`NodeIdRegistry`] — stable [`NodeId`](verbatim_model::NodeId)s from MSAA
//!   `(hwnd, object, child)` addresses, sharing a mint counter with UIA.
//! - [`map`] — role and state mapping into the model.

pub mod acquire;
mod com;
mod hook;
pub mod map;
mod registry;

pub use hook::{WinEventCallback, WinEventHook, WinEventKind};
pub use registry::{MsaaKey, NodeIdRegistry};
