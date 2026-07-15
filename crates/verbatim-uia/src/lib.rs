//! UIA client stack (architecture section 4).
//!
//! Event registrations attach cache requests so events arrive with name,
//! role, states, and patterns prefetched in one round trip; callbacks arrive
//! on dedicated multithreaded-apartment threads separate from the threads that
//! make UIA calls. Everything here runs inside an outpost process and maps
//! into the normalized model ([`verbatim_model`]).
//!
//! The pieces:
//!
//! - [`Uia`] — a per-thread client wrapper; each outpost thread that talks to
//!   UIA owns one. Beyond focus/query lookups, it walks a node's ancestor
//!   chain and navigates to a neighbor ([`NavigateDirection`]) via the
//!   raw-view tree walker's per-hop `*BuildCache` methods (one cross-process
//!   round trip per hop; M4's remote-ops work replaces the per-hop walk with
//!   a single batched round trip), and activates a node through the
//!   `Invoke`/`Toggle`/legacy-`DoDefaultAction` pattern ladder.
//! - [`FocusRegistration`] — the self-contained global focus-change handler
//!   (its narrow seam is what the M3 sentinel split relocates).
//! - [`PropertyRegistration`] — name/value change handlers scoped to the target
//!   app's top-level windows.
//! - [`SelectionRegistration`] and [`NotificationRegistration`] —
//!   `SelectionItem_ElementSelected` and `AutomationNotification`, scoped
//!   the same way (roadmap M3).
//! - [`NodeIdRegistry`] — stable [`NodeId`](verbatim_model::NodeId)s from UIA
//!   runtime IDs, sharing a mint counter with the MSAA backend.
//! - [`has_server_side_provider`] — the arbitration probe (blocking; see its
//!   docs and run it only on a deadline-guarded query-pool thread).
//! - [`nearest_window_handle`] — NVDA's `getNearestWindowHandle`: resolves the
//!   window an arbitrary element belongs to, for elements (menu items, list
//!   items) that are not windows themselves. Also blocking; see its docs.
//! - [`map`] — control-type and cached-property mapping into the model.

mod cache;
mod client;
mod com;
mod events;
mod focus;
pub mod map;
mod nearest;
mod notify;
mod probe;
mod registry;

pub use cache::base_cache_request;
pub use client::{NavigateDirection, Uia};
pub use com::init_mta;
pub use events::{PropertyCallback, PropertyRegistration};
pub use focus::{FocusCallback, FocusRegistration};
pub use nearest::nearest_window_handle;
pub use notify::{
    NotificationCallback, NotificationRegistration, SelectionCallback, SelectionRegistration,
};
pub use probe::has_server_side_provider;
pub use registry::NodeIdRegistry;
