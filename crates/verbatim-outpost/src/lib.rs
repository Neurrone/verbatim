//! The outpost (decision D9): one process per application, watching it for
//! its whole life, used by the `verbatim-outpost` binary.
//!
//! [`outpost`] is its runtime: the intake, one worker and its watchdog, the
//! reader, and the writer around its reducer, which is
//! `verbatim-outpost-logic`'s. [`arbitration`] holds the per-window backend
//! arbitration the outpost uses. [`dialog_text`] gathers a dialog's own
//! text, such as a message box's question, which the outpost reports as the
//! dialog's description. [`text`] is the outpost's side of the text
//! protocol (milestone M4): reading, waiting for the caret, and selecting in
//! a node's text, over UIA's text pattern and the Win32 edit controls'
//! messages. [`terminal`] finds a terminal's new output by an anchored diff
//! of its text (milestone M4 item 9). The message layer every end shares is
//! `verbatim_process::message`.

pub mod arbitration;
pub mod dialog_text;
mod gates;
pub mod outpost;
pub mod terminal;
pub mod text;

pub use outpost::{
    FocusWindowReader, FocusedElementReader, GapTimer, Heard, HeardObserver, Outpost, run_attach,
    run_pipe,
};
