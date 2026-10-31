//! The focus listener (decision D13): one permanent process holding the
//! desktop-global UIA focus registration and global MSAA hooks, forwarding
//! each focus fact to Core, which routes it to the target application's
//! own outpost. [`run_listener`] is its runtime ([`listener`]);
//! [`focus_read`] is its judging and reading of each UIA focus from the
//! event's own element, on a thread per application. What both decide
//! without a call, the read scheduler, the menu end's grace, the
//! foreground watch and a focus event's vocabulary, is
//! `verbatim-listener-logic`'s.

pub mod focus_read;
pub mod listener;

pub use listener::run_listener;
