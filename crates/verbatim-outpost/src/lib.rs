//! Outpost library: the Core-outpost IPC vocabulary and framing, the outpost
//! runtime, the focus listener, and the Core-side supervisor.
//!
//! The message layer every end shares is `verbatim_process::message`.
//! [`outpost`] is the per-application outpost — intake, one worker and its
//! watchdog, the reader, and the writer — used by the `verbatim-outpost`
//! binary, watching one application for its whole life (decision D9).
//! [`listener`] is the focus-listener runtime (decision D13): one permanent
//! process holding the desktop-global UIA focus registration and global
//! MSAA hooks, forwarding each focus fact to Core; [`focus_read`] is its
//! judging and reading of each UIA focus from the event's own element, on a
//! thread per application. [`supervisor`] is
//! the Core-side piece (the lifecycle owner, launches, writers, fact routing,
//! crash and hang handling, and retirement), wired into `verbatim-app`.
//! [`arbitration`] holds the per-window backend arbitration the outpost
//! uses. [`dialog_text`] gathers a dialog's own text, such as a message
//! box's question, which the outpost reports as the dialog's description.
//! [`text`] is the outpost's side of the text protocol (milestone M4):
//! reading, waiting for the caret, and selecting in a node's text, over
//! UIA's text pattern and the Win32 edit controls' messages. [`terminal`]
//! finds a terminal's new output by an anchored diff of its text
//! (milestone M4 item 9).

pub mod arbitration;
pub mod dialog_text;
pub mod focus_read;
mod foreground;
mod gates;
pub mod listener;
pub mod outpost;
mod read_schedule;
pub mod supervisor;
pub mod terminal;
pub mod text;

pub use listener::run_listener;
pub use outpost::{
    FocusWindowReader, FocusedElementReader, GapTimer, Heard, HeardObserver, Outpost, run_attach,
    run_pipe,
};
pub use supervisor::{EventSink, ForegroundReport, OutpostMessage, Supervisor, Undelivered};
