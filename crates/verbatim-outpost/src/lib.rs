//! Outpost library: the Core-outpost IPC vocabulary and framing, the outpost
//! runtime, the focus listener, and the Core-side supervisor.
//!
//! The [`protocol`] module is the wire vocabulary shared by both ends.
//! [`outpost`] is the per-application outpost — intake, one worker and its
//! watchdog, the reader, and the writer — used by the `verbatim-outpost`
//! binary, watching one application for its whole life (decision D9).
//! [`listener`] is the focus-listener runtime (decision D13): one permanent,
//! stateless process holding the desktop-global UIA focus registration and
//! global MSAA hooks, forwarding each focus fact to Core. [`supervisor`] is
//! the Core-side piece (the lifecycle owner, launches, writers, fact routing,
//! crash and hang handling, and retirement), wired into `verbatim-app`.
//! [`arbitration`] holds the per-window backend arbitration the outpost
//! uses.

pub mod arbitration;
mod event_thread;
pub mod listener;
pub mod outpost;
pub mod protocol;
pub mod supervisor;

pub use listener::run_listener;
pub use outpost::{Outpost, run_attach, run_pipe};
pub use supervisor::{OutpostMessage, Supervisor};
