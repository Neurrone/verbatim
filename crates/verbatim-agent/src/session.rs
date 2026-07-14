//! Session diagnostics for [`crate::protocol::Request::SessionInfo`].
//!
//! Exists because a screen reader test driven from a non-interactive
//! session can never work — the "session 0" problem. Rather than let that
//! surface as an inscrutable timeout somewhere downstream, the agent
//! reports its own session id, whether its window station is interactive,
//! and the input desktop's name, so a broken test environment is
//! diagnosable at the source.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetProcessWindowStation,
    GetUserObjectInformationW, OpenInputDesktop, UOI_FLAGS, UOI_NAME, USEROBJECTFLAGS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;

use crate::protocol::SessionInfo;

/// Gathers [`SessionInfo`] for the calling process.
///
/// # Errors
///
/// Returns an error only if `ProcessIdToSessionId` itself fails, which in
/// practice does not happen for the calling process's own pid; window
/// station and desktop lookups degrade to `false`/`None` on failure
/// instead of erroring, since "I could not determine this" is itself part
/// of the diagnosis.
pub fn current() -> windows::core::Result<SessionInfo> {
    // SAFETY: GetCurrentProcessId has no preconditions; session_id is a
    // valid out-pointer for the duration of the call.
    let session_id = unsafe {
        let pid = GetCurrentProcessId();
        let mut session_id = 0u32;
        ProcessIdToSessionId(pid, &raw mut session_id)?;
        session_id
    };

    Ok(SessionInfo {
        session_id,
        interactive_window_station: window_station_is_interactive(),
        input_desktop_name: input_desktop_name(),
    })
}

/// Whether the calling process's window station is interactive: it has the
/// `WSF_VISIBLE` flag, which only the `WinSta0` window station a real
/// interactive logon owns carries. Session 0 services get a non-visible
/// window station.
fn window_station_is_interactive() -> bool {
    // SAFETY: GetProcessWindowStation has no preconditions and returns a
    // handle owned by the process (not closed by the caller).
    let Ok(station) = (unsafe { GetProcessWindowStation() }) else {
        return false;
    };
    if station.0.is_null() {
        return false;
    }
    let mut flags = USEROBJECTFLAGS::default();
    let needed = &mut 0u32;
    // SAFETY: `station` is a valid window station handle from the call
    // above; `flags` is a correctly sized out-buffer for USEROBJECTFLAGS,
    // matching the UOI_FLAGS information class.
    let result = unsafe {
        GetUserObjectInformationW(
            HANDLE(station.0),
            UOI_FLAGS,
            Some((&raw mut flags).cast()),
            u32::try_from(size_of::<USEROBJECTFLAGS>()).unwrap_or(0),
            Some(needed),
        )
    };
    result.is_ok() && (flags.dwFlags & u32::try_from(WSF_VISIBLE).unwrap_or(0)) != 0
}

/// The current input desktop's name, or `None` when it cannot be opened —
/// which itself means the window station has no input desktop, the other
/// half of the session-0 diagnosis.
fn input_desktop_name() -> Option<String> {
    // SAFETY: constant, well-formed arguments; the returned handle is
    // closed below on every path.
    let desktop =
        unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }.ok()?;

    let name = (|| {
        let handle = HANDLE(desktop.0);
        let mut needed = 0u32;
        // SAFETY: a null buffer with `needed` as the size out-pointer is
        // the documented way to ask for the required buffer size; expected
        // to report an insufficient-buffer error.
        let _ =
            unsafe { GetUserObjectInformationW(handle, UOI_NAME, None, 0, Some(&raw mut needed)) };
        if needed == 0 {
            return None;
        }
        // Buffer sized in u16 elements; GetUserObjectInformationW's
        // `needed` is a byte count.
        let mut buffer = vec![0u16; needed.div_ceil(2) as usize];
        // SAFETY: `buffer` is sized to `needed` bytes (rounded up) as
        // reported above, and valid for writes of that length.
        let result = unsafe {
            GetUserObjectInformationW(
                handle,
                UOI_NAME,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                Some(&raw mut needed),
            )
        };
        result.ok()?;
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    })();

    // SAFETY: `desktop` was opened by `OpenInputDesktop` above and is not
    // used again after this point.
    unsafe {
        let _ = CloseDesktop(desktop);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test runs inside the developer's own interactive desktop
    /// session, so it doubles as a sanity check that the happy path
    /// resolves to plausible values, not just that the calls do not panic.
    #[test]
    fn current_session_reports_this_process_session() {
        let info = current().expect("queries session info for this process");
        // `cargo test` runs as a normal user process; expect an
        // interactive window station and a resolvable desktop name.
        assert!(info.interactive_window_station);
        assert!(info.input_desktop_name.is_some());
    }
}
