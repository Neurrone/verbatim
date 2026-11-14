//! Async functions run on Core's thread.

#![deny(clippy::await_holding_refcell_ref)]

mod driver;
mod logic;
mod request;

pub use driver::AsyncCore;
