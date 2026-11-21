//! The focus listener (decision D13; ADR 0003, "Readers tell, Core
//! asks"): one permanent process holding the desktop-global UI Automation
//! registration and global MSAA hooks, telling Core of every event it
//! hears and answering Core's requests on what it kept. [`run_listener`]
//! is its runtime ([`listener`]), its reading threads the shell of the
//! pure schedule (`reading`). [`read_object`] is its read of a kept
//! element and [`read_msaa_object`] its read of an MSAA object at an
//! event's address, for a test that stands in for it. What the listener
//! decides without a call, its check threads' calls, its kept elements,
//! the reading threads' schedule, the foreground watch and the tells'
//! drops, is `verbatim-listener-logic`'s.

mod call_log;
pub mod listener;
mod reading;

pub use call_log::{CallObserver, read_kept_object, read_object, read_object_observed};
pub use listener::asked::read_msaa_object;
pub use listener::run_listener;
