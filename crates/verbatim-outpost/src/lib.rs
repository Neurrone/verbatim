//! Outpost library: the Core-outpost IPC vocabulary and framing, the outpost
//! runtime, the focus listener, and the Core-side supervisor.
//!
//! The [`protocol`] module is the wire vocabulary shared by both ends.
//! [`runtime`] is the per-application outpost actor — the event thread, UIA
//! registrations, query pool, and command loop — used by the
//! `verbatim-outpost` binary, watching one application for its whole life
//! (decision D9). [`listener`] is the focus-listener runtime (decision D13):
//! one permanent, stateless process holding the desktop-global UIA focus
//! registration and global MSAA hooks, forwarding each focus fact to Core.
//! [`supervisor`] is the Core-side piece (job-object process management, the
//! multi-outpost map, the dedicated listener slot, idle retirement, and fact
//! routing), wired into `verbatim-app`. [`arbitration`] holds the per-window
//! backend arbitration shared by the runtime.

pub mod arbitration;
pub mod listener;
pub mod protocol;
mod query_pool;
pub mod runtime;
pub mod supervisor;

pub use listener::run_listener;
pub use runtime::{Outpost, run_attach, run_pipe};
pub use supervisor::{OutpostMessage, Supervisor};
