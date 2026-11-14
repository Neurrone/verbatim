//! The trial of how Core's "ask, wait, continue" code is written
//! (`docs/roadmap.md`, "Before the builds"): the same event queue, with a
//! UI Automation focus and a caret key, in three forms, against one fake
//! reader and one set of scenarios. Compiled only into tests; nothing in
//! the product uses it.
//!
//! - [`step::StepCore`]: step functions in a reducer.
//! - [`async_form::AsyncCore`]: async functions run on Core's thread.
//! - [`thread_form::ThreadCore`]: a logic thread that waits on each
//!   request beside an inbox thread that owns the queue and the waiting.

mod async_form;
mod harness;
mod scenarios;
mod step;
mod thread_form;
