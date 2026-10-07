//! Named events a launched process sets when it reaches a point a test
//! waits for, for [`Request::CreateEvent`](crate::protocol::Request::CreateEvent)
//! and [`Request::WaitForEvent`](crate::protocol::Request::WaitForEvent).
//!
//! The agent creates the event before the launch, so the process only has
//! to open and set it, and keeps the handle until a wait on it ends. A wait
//! blocks on the event and on the process's handle together, so a process
//! that exits before setting it ends the wait at once.

use std::collections::BTreeMap;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    WaitForMultipleObjects,
};
use windows::core::HSTRING;

use crate::protocol::{EventOutcome, ProcessState};

/// The events created and not yet waited on, by name.
static EVENTS: Mutex<BTreeMap<String, OwnedHandle>> = Mutex::new(BTreeMap::new());

/// Creates the manual-reset event `name`, not yet set.
///
/// # Errors
///
/// Returns an error if the event cannot be created.
pub(crate) fn create(name: &str) -> io::Result<()> {
    // SAFETY: default security, manual reset, not set; the handle is owned
    // below.
    let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(name)) }
        .map_err(io::Error::other)?;
    // SAFETY: a handle just created, owned here alone.
    let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
    EVENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_owned(), event);
    Ok(())
}

/// Waits up to `timeout` for the event `name` to be set, or process `pid`
/// to exit first, and forgets the event.
///
/// # Errors
///
/// Returns an error if no such event was created, the process cannot be
/// opened, or the wait fails.
pub(crate) fn wait(name: &str, pid: u32, timeout: Duration) -> io::Result<EventOutcome> {
    let event = EVENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(name)
        .ok_or_else(|| io::Error::other(format!("no event named {name:?} was created")))?;
    // SAFETY: a query-and-wait open of a plain process id; owned below.
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .map_err(|error| io::Error::other(format!("process {pid} cannot be waited on: {error}")))?;
    // SAFETY: a handle just opened, owned here alone.
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    let handles = [
        HANDLE(event.as_raw_handle()),
        HANDLE(process.as_raw_handle()),
    ];
    let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: both handles are open for the call.
    let waited = unsafe { WaitForMultipleObjects(&handles, false, milliseconds) };
    if waited == WAIT_OBJECT_0 {
        return Ok(EventOutcome::Signalled);
    }
    if waited.0 == WAIT_OBJECT_0.0 + 1 {
        let exit_code = match crate::process::exit_state(handles[1])? {
            ProcessState::Exited { exit_code } => exit_code,
            ProcessState::Running => None,
        };
        return Ok(EventOutcome::Exited { exit_code });
    }
    if waited == WAIT_TIMEOUT {
        return Ok(EventOutcome::TimedOut);
    }
    Err(io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_never_set_times_out_and_is_forgotten() {
        let name = format!("Local\\verbatim-agent-test-{}", std::process::id());
        create(&name).expect("creates");
        assert_eq!(
            wait(&name, std::process::id(), Duration::from_millis(10)).expect("waits"),
            EventOutcome::TimedOut
        );
        assert!(wait(&name, std::process::id(), Duration::from_millis(10)).is_err());
    }
}
