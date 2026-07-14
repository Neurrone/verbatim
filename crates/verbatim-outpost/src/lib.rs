//! Outpost library: the Core-outpost IPC vocabulary and framing, the outpost
//! runtime, and the Core-side supervisor.
//!
//! The [`protocol`] module (frozen in phase 1) is the wire vocabulary shared by
//! both ends. [`runtime`] is the per-application outpost actor — the event
//! thread, UIA registrations, query pool, and command loop — used by the
//! `verbatim-outpost` binary. [`supervisor`] and [`foreground`] are the
//! Core-side pieces (job-object process management and the foreground trigger),
//! wired into `verbatim-app` in phase 3. [`arbitration`] holds the per-window
//! backend arbitration shared by the runtime.

pub mod arbitration;
pub mod foreground;
pub mod protocol;
mod query_pool;
pub mod runtime;
pub mod supervisor;

pub use foreground::{ForegroundCallback, ForegroundTrigger};
pub use runtime::{Outpost, run_attach, run_pipe};
pub use supervisor::{OutpostMessage, Supervisor};
