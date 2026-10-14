//! Events, and a wait on several at once: what a thread that waits for a
//! pipe and for a signal from another thread together waits on, as the
//! synth thread waits for its host's next frame and for its utterance's
//! cancel, and a synthesizer host for its write to Core and for Core's
//! next command (`docs/design/communication.md`, sections 7.3 and 7.4).

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::overlapped::win32_error;

/// An unnamed manual-reset event: set, it stays set until reset, or until
/// an overlapped operation it is given to starts.
#[derive(Debug)]
pub struct Event {
    handle: OwnedHandle,
}

impl Event {
    /// A new event, not set.
    ///
    /// # Errors
    ///
    /// Returns the error if the event cannot be created.
    pub fn new() -> io::Result<Self> {
        // SAFETY: a manual-reset event, unnamed, with default security; the
        // handle returned is owned below.
        let event = unsafe { CreateEventW(None, true, false, None) }.map_err(win32_error)?;
        Ok(Self {
            // SAFETY: a valid handle just returned, owned by nothing else.
            handle: unsafe { OwnedHandle::from_raw_handle(event.0 as RawHandle) },
        })
    }

    /// Sets the event, waking every thread waiting on it. Never blocks.
    pub fn set(&self) {
        // SAFETY: the event this value owns, open for its life. Setting an
        // open event cannot fail.
        let _ = unsafe { SetEvent(self.raw()) };
    }

    /// Resets the event.
    pub fn reset(&self) {
        // SAFETY: as for `set`.
        let _ = unsafe { ResetEvent(self.raw()) };
    }

    /// Whether the event is set, with a zero wait.
    #[must_use]
    pub fn is_set(&self) -> bool {
        // SAFETY: as for `set`; a zero timeout only reads the state.
        unsafe { WaitForSingleObject(self.raw(), 0) == WAIT_OBJECT_0 }
    }

    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.handle.as_raw_handle())
    }
}

/// Waits until any of `events` is set, at most `timeout` (`None` waits for
/// as long as it takes), and returns the index of the first one set, the
/// lowest when several are; `None` when the time ran out. A thread blocked
/// here uses no processor time and wakes for nothing else.
///
/// # Errors
///
/// Returns the wait's error.
///
/// # Panics
///
/// Panics when `events` is empty or holds more than the system's limit of
/// 64.
pub fn wait_any(events: &[&Event], timeout: Option<Duration>) -> io::Result<Option<usize>> {
    assert!(
        (1..=64).contains(&events.len()),
        "a wait takes one to 64 events"
    );
    let handles: Vec<HANDLE> = events.iter().map(|event| event.raw()).collect();
    let milliseconds = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_millis()).unwrap_or(INFINITE - 1)
    });
    // SAFETY: every handle is an event the caller's references keep open
    // across the call.
    let waited = unsafe { WaitForMultipleObjects(&handles, false, milliseconds) };
    if waited == WAIT_TIMEOUT {
        return Ok(None);
    }
    if waited == WAIT_FAILED {
        return Err(io::Error::last_os_error());
    }
    let index = waited.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    if index < handles.len() {
        Ok(Some(index))
    } else {
        Err(io::Error::other(format!(
            "a wait on events ended with {:#x}",
            waited.0
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_returns_the_lowest_event_set_and_none_when_the_time_runs_out() {
        let (first, second) = (Event::new().unwrap(), Event::new().unwrap());
        assert_eq!(
            wait_any(&[&first, &second], Some(Duration::ZERO)).unwrap(),
            None
        );
        second.set();
        assert_eq!(wait_any(&[&first, &second], None).unwrap(), Some(1));
        first.set();
        assert_eq!(wait_any(&[&first, &second], None).unwrap(), Some(0));
        first.reset();
        assert!(!first.is_set());
        assert!(second.is_set());
    }
}
