//! The local window and process queries more than one of Verbatim's roles
//! makes: an outpost, the focus listener, the MSAA and UIA clients, and
//! Core's supervisor. None of them waits on an application, so each is safe
//! on any thread, a hung application's windows included.
//!
//! - [`window`]: window queries by handle, and the system's windows as
//!   NVDA's acceptance test asks for them
//!   ([`window::SystemWindows`]).
//! - [`provider`]: the process that serves a window's UI Automation, which
//!   for a console window is its console host, not the client Windows names
//!   as its owner.
//! - [`integrity`]: a process's integrity level.

pub mod integrity;
pub mod provider;
pub mod window;
