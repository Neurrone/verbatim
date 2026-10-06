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
//! - [`calls`] — the per-thread count of the cross-process calls this crate
//!   makes, which the outpost's worker takes around each entry it handles.
//! - [`ElementExt`] and [`WalkerExt`] — safe wrappers over an element's
//!   cached and live reads and the tree walker's steps, each holding one
//!   documented `unsafe` call and counting its cross-process call, so the
//!   code that reads UIA, here and in the outpost, is safe Rust.
//! - [`text`] — the same kind of wrappers over the text pattern and its
//!   ranges (milestone M4), which the outpost's text protocol reads with.

mod cache;
pub mod calls;
mod checks;
mod client;
mod com;
mod element;
mod focus;
pub mod map;
mod nearest;
mod probe;
mod registry;
mod subscribe;
pub mod text;

pub use cache::{CACHED_PROPERTIES, base_cache_request};
pub use checks::{console_reports_formatting, is_windows_forms};
pub use client::{AncestorStops, AncestorWalk, Uia, selected_element};
pub use com::{element_is_gone, init_mta, runtime_id};
pub use element::{ElementExt, WalkerExt, elements_of};
pub use focus::{FocusCallback, FocusRegistration};
pub use nearest::nearest_window_handle;
pub use probe::{has_server_side_provider, probe_server_side_provider};
pub use registry::{NodeIdRegistry, Released};
pub use subscribe::{
    ElementCallback, EventCallback, FOCUS_PROPERTIES, NotificationCallback, PropertyCallback,
    Registration, Scope, Subscription,
};

/// Releases the UIA objects this crate keeps for the calling thread: the
/// client behind [`console_reports_formatting`] and [`is_windows_forms`],
/// and the walker behind [`nearest_window_handle`].
///
/// A thread that used any of those calls this before it exits. Otherwise
/// the objects are released by the thread's thread-local destructors, which
/// on Windows run with the loader lock held, where COM work must not run.
/// A later call on the same thread builds the objects again.
pub fn release_thread_state() {
    checks::release_thread_client();
    nearest::release_thread_context();
}
