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
//!   chain and navigates to a neighbor (a navigation `QueryKind`) via the
//!   raw-view tree walker's per-hop `*BuildCache` methods (one cross-process
//!   round trip per hop; M4's remote-ops work replaces the per-hop walk with
//!   a single batched round trip), and activates a node through the
//!   `Invoke`/`Toggle`/legacy-`DoDefaultAction` pattern ladder.
//! - [`FocusRegistration`] — the self-contained global focus-change handler
//!   (its narrow seam is what the M3 sentinel split relocates).
//! - [`Registration`] — a property-change, automation-event, or notification
//!   subscription over a [`Scope`] (given windows, the whole desktop, or
//!   given elements) that can be moved without waiting: the listener's
//!   desktop-wide subscriptions and an outpost's focus-following property
//!   subscription.
//! - [`NodeIdRegistry`] — stable [`NodeId`](verbatim_model::NodeId)s from UIA
//!   runtime IDs, sharing a mint counter with the MSAA backend.
//! - [`has_server_side_provider`] — the arbitration probe (blocking; see its
//!   docs and run it only on a deadline-guarded worker).
//! - [`nearest_window_handle`] — NVDA's `getNearestWindowHandle`: resolves the
//!   window an arbitrary element belongs to, for elements (menu items, list
//!   items) that are not windows themselves. Also blocking; see its docs.
//! - [`map`] — control-type and cached-property mapping into the model.

mod cache;
mod checks;
mod client;
mod com;
mod focus;
pub mod map;
mod nearest;
mod probe;
mod registry;
mod subscribe;

pub use cache::base_cache_request;
pub use checks::{console_reports_formatting, is_windows_forms};
pub use client::{AncestorStops, AncestorWalk, Uia};
pub use com::init_mta;
pub use focus::{FocusCallback, FocusRegistration};
pub use nearest::nearest_window_handle;
pub use probe::{has_server_side_provider, probe_server_side_provider};
pub use registry::{NodeIdRegistry, Released};
pub use subscribe::{
    ElementCallback, FOCUS_PROPERTIES, NotificationCallback, PropertyCallback, Registration, Scope,
    Subscription,
};
