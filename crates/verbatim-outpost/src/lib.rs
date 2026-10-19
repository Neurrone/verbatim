//! Outpost library: the Core-outpost IPC vocabulary and framing, the outpost
//! runtime, the focus listener, and the Core-side supervisor.
//!
//! The [`protocol`] module is the wire vocabulary shared by both ends.
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

mod app_locks;
pub mod arbitration;
pub mod dialog_text;
mod event_thread;
pub mod focus_read;
mod gates;
pub mod listener;
pub mod outpost;
pub mod protocol;
pub mod supervisor;
pub mod terminal;
pub mod text;

pub use listener::run_listener;
pub use outpost::{
    FocusWindowReader, FocusedElementReader, ForegroundReader, Heard, HeardObserver, Outpost,
    OutpostOptions, run_attach, run_pipe, set_application_lock,
};
pub use supervisor::{EventSink, OutpostMessage, Supervisor, Undelivered};
/// The process that serves a window's UI Automation, and a console client's
/// console host, read locally: re-exported so Core can route a console
/// client's focus-now query to its console host without depending on
/// `verbatim-uia` (`docs/design/listener-without-connections.md`, section
/// 4.3).
pub use verbatim_uia::{console_host_of, provider_process};
