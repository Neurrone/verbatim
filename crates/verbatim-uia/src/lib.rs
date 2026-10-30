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
//! - [`BoundedClient`] — a client on a thread of its own, for an operation
//!   made of many provider calls (a search, a dialog's text) that needs a
//!   deadline of its own: UIA's timeouts ([`TRANSACTION_TIMEOUT`]) bound each call,
//!   not the operation, and a call cannot be cancelled, so an operation
//!   that passes its deadline is abandoned with its thread.
//! - [`Registration`] — a property-change, automation-event, or notification
//!   subscription over a [`Scope`] (given windows, the whole desktop, or
//!   given elements) that can be moved without waiting, as two event
//!   handler groups built once ([`Group`], [`Part`]): the listener's
//!   desktop-wide subscriptions, with the global focus handler on the same
//!   client ([`Subscription::Focus`]), and an outpost's focus-following
//!   subscription.
//! - [`NodeIdRegistry`] — stable [`NodeId`](verbatim_model::NodeId)s from UIA
//!   runtime IDs, sharing a mint counter with the MSAA backend.
//! - [`raw_probe`] — the arbitration probe the outposts and the focus
//!   listener make, one window message rather than a UIA connection, its
//!   outcome read by [`raw_probe_verdict`] (blocking; see its docs and run
//!   it only on a deadline-guarded worker).
//! - [`nearest_window_handle`] — NVDA's `getNearestWindowHandle`: resolves the
//!   window an arbitrary element belongs to, for elements (menu items, list
//!   items) that are not windows themselves, and [`nearest_window_lookup`],
//!   the same lookup with a timeout told from no window. Also blocking; see
//!   their docs.
//! - [`map`] — control-type, role, and state mapping into the model.
//! - The shape module ([`Shape`], [`Cached`], [`Request`], [`Live`]) — every
//!   cached read of an element, declared together with the cache request
//!   that fills it (`docs/design/uia-shapes.md`): the only cached reads in
//!   the workspace.
//! - [`provider_process`], [`served_by`] and [`console_host_of`] — the
//!   process that serves a window's UIA, a console window's being its
//!   console host, read with local calls only;
//!   [`parent_across_process_boundary`], the
//!   parent window, or a top-level window's visible owner, served by
//!   another process, where a walk up stops.
//! - [`calls`] — the per-thread count of the cross-process calls this crate
//!   makes, which the outpost's worker takes around each entry it handles.
//! - [`ElementExt`] and [`WalkerExt`] — safe wrappers over an element's
//!   live reads and the tree walker's steps, each holding one documented
//!   `unsafe` call and counting its cross-process call, so the code that
//!   reads UIA, here and in the outpost, is safe Rust.
//! - [`text`] — the same kind of wrappers over the text pattern and its
//!   ranges (milestone M4), which the outpost's text protocol reads with.

mod answered;
mod bounded;
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
mod provider;
mod proxy;
mod registry;
mod shape;
mod subscribe;
pub mod text;

pub use bounded::{BoundedClient, Unanswered};
pub use checks::{CONSOLE_TEXT_AREA, console_reports_formatting, held_console_reports_formatting};
pub use client::{
    AncestorParts, AncestorStops, AncestorWalk, CONNECTION_TIMEOUT, FailedHop, TRANSACTION_TIMEOUT,
    Uia, release_mta_usage,
};
pub use com::{
    element_is_gone, init_mta, is_mixed, is_not_supported, leave_mta, runtime_id, timed_out,
    variant_element, variant_f64, variant_i32, variant_i32_array, variant_optional_bool,
    variant_string,
};
pub use element::{ElementExt, WalkerExt, elements_of};
pub use focus::FocusCallback;
pub use nearest::{nearest_window_handle, nearest_window_lookup};
pub use probe::{RAW_PROBE_TIMEOUT_MS, RawProbe, raw_probe, raw_probe_verdict};
pub use provider::{
    console_host_of, is_top_level_window, outermost_nearest_window, parent_across_process_boundary,
    parent_window, plain_windows_start, provider_process, raw_view_parent_window, served_by,
};
pub use registry::{Interning, NodeIdRegistry, Released};
pub use shape::{
    Absent, Agile, Ancestor, AncestorGates, Cached, Cell, Covers, Description, DialogText,
    Direction, ElementValue, Fill, FocusInput, HelpTextRefresh, Kept, LEFT_OUT_WHEN_UNSUPPORTED,
    ListenerEvent, ListenerSelection, Live, Node, NodeGates, Place, Position, PropertyEvent,
    Request, RoleCore, SelectedItem, SelectionContainer, Shape, ShellButton, Shortcut, States,
    SuggestionsRefresh, TextAreaPattern, TextEvent, Ungated, ValueSet, WindowLookup,
};
pub use subscribe::{
    ActiveTextPositionCallback, ElementCallback, EventCallback, FOCUS_PROPERTIES, Group,
    NotificationCallback, Part, PropertyCallback, Registration, Scope, Subscription,
    registering_thread_runs,
};

/// Releases the UIA objects this crate keeps for the calling thread: the
/// client behind [`console_reports_formatting`], and the walker behind
/// [`nearest_window_handle`].
///
/// A thread that used any of those calls this before it exits. Otherwise
/// the objects are released by the thread's thread-local destructors, which
/// on Windows run with the loader lock held, where COM work must not run.
/// A later call on the same thread builds the objects again.
pub fn release_thread_state() {
    checks::release_thread_client();
    nearest::release_thread_context();
}
