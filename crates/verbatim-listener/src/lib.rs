//! The focus listener (decision D13; ADR 0003, "Readers tell, Core
//! asks"): one permanent process holding the desktop-global UI Automation
//! registration and global MSAA hooks, telling Core of every event it
//! hears and answering Core's requests on what it kept. [`run_listener`]
//! is its runtime ([`listener`]), its reading threads the shell of the
//! pure schedule (`reading`). [`focus_read`] is the unasked focus read
//! that mockapp's integration harness still drives. What the listener
//! decides without a call, its check threads' calls, its kept elements,
//! the reading threads' schedule, the foreground watch and the tells'
//! drops, is `verbatim-listener-logic`'s.

pub mod focus_read;
pub mod listener;
mod reading;
mod value_change;

pub use listener::run_listener;
