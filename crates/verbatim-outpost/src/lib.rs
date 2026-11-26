//! The outpost (decision D9): one process per application, watching it for
//! its whole life, used by the `verbatim-outpost` binary.
//!
//! [`outpost`] is its runtime: the mailbox, the worker generations, the
//! reader, and the writer around its reducer, which is
//! `verbatim-outpost-logic`'s. [`arbitration`] holds the per-window backend
//! arbitration the outpost uses. [`text`] is the outpost's side of the text
//! protocol (milestone M4): reading, waiting for the caret, and selecting in
//! a node's text, over UIA's text pattern and the Win32 edit controls'
//! messages. [`driver`] sends an outpost Core's requests where no Core
//! runs: the attach mode and mockapp's harness. The message layer every end shares is
//! `verbatim_process::message`.

pub mod arbitration;
pub mod driver;
mod gates;
pub mod outpost;
pub mod text;

#[cfg(test)]
mod conversion_measure;

pub use outpost::{
    FocusWindowReader, FocusedElementReader, Heard, HeardObserver, Outpost, RequestCallsObserver,
    run_attach, run_pipe,
};
