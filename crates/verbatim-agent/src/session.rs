//! Session diagnostics for [`crate::protocol::Request::SessionInfo`]: the
//! agent's own session, read by [`verbatim_process::session`], so a test
//! driven from a non-interactive session (the "session 0" problem) fails
//! with a diagnosis at the source.

use crate::protocol::SessionInfo;

/// Gathers [`SessionInfo`] for the calling process.
///
/// # Errors
///
/// As [`verbatim_process::session::current`].
pub fn current() -> windows::core::Result<SessionInfo> {
    let session = verbatim_process::session::current()?;
    Ok(SessionInfo {
        session_id: session.id,
        interactive_window_station: session.interactive_window_station,
        input_desktop_name: session.input_desktop_name,
    })
}
