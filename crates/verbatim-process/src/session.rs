//! The calling process's session: shared by Verbatim, which refuses to start
//! outside an interactive session, and the test agent, which refuses the same
//! way and reports it live.
//!
//! A screen reader in a non-interactive session can never work, the
//! "session 0" problem: `WinRM`, PowerShell Direct, and services run there,
//! with no input desktop to speak for or inject input into. Rather than let
//! that surface as an inscrutable timeout somewhere downstream, each process
//! reads its own session id, whether its window station is interactive, and
//! the input desktop's name, so the cause is diagnosable at the source.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetProcessWindowStation,
    GetUserObjectInformationW, OpenInputDesktop, UOI_FLAGS, UOI_NAME, USEROBJECTFLAGS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;

/// The calling process's session, as [`current`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The Windows session id (`ProcessIdToSessionId`).
    pub id: u32,
    /// Whether the process's window station is interactive
    /// (`GetProcessWindowStation` plus the `WSF_VISIBLE` flag from
    /// `GetUserObjectInformationW`).
    pub interactive_window_station: bool,
    /// The name of the current input desktop, when `OpenInputDesktop`
    /// succeeds. `None` when it cannot be opened, which itself is
    /// diagnostic: a non-interactive window station has no input desktop.
    pub input_desktop_name: Option<String>,
}

/// Reads the calling process's [`Session`].
///
/// # Errors
///
/// Returns an error only if `ProcessIdToSessionId` itself fails, which in
/// practice does not happen for the calling process's own pid; window
/// station and desktop lookups degrade to `false`/`None` on failure
/// instead of erroring, since "I could not determine this" is itself part
/// of the diagnosis.
pub fn current() -> windows::core::Result<Session> {
    // SAFETY: GetCurrentProcessId has no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    let mut session_id = 0u32;
    // SAFETY: session_id is a valid out-pointer for the duration of the
    // call.
    unsafe { ProcessIdToSessionId(pid, &raw mut session_id) }?;

    Ok(Session {
        id: session_id,
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
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenSessionId};
    use windows::Win32::System::StationsAndDesktops::{
        CloseWindowStation, CreateWindowStationW, SetProcessWindowStation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::WindowsAndMessaging::WINSTA_ALL_ACCESS;
    use windows::core::PCWSTR;

    /// The session id of this process's access token, read independently
    /// of [`current`].
    fn token_session_id() -> u32 {
        let mut token = HANDLE::default();
        // SAFETY: no preconditions; a pseudo-handle that needs no closing.
        let process = unsafe { GetCurrentProcess() };
        // SAFETY: this process's handle and a valid out-pointer.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }
            .expect("opens this process's token");
        let mut id = 0u32;
        let mut length = 0u32;
        // SAFETY: `id` is a u32 out-buffer, the size `TokenSessionId` writes.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenSessionId,
                Some((&raw mut id).cast()),
                u32::try_from(size_of::<u32>()).expect("fits"),
                &raw mut length,
            )
        };
        // SAFETY: the token was opened above and is not used again.
        unsafe { CloseHandle(token) }.expect("closes the token");
        read.expect("reads the token's session id");
        id
    }

    /// A window station this test creates is never the interactive one, so
    /// in it the process reports exactly what a service in session 0 does,
    /// whatever session the tests run in: its session, a window station
    /// that is not interactive, and no input desktop. (Reading an
    /// interactive session's own window station and desktop depends on
    /// where the tests run; the end-to-end suite's `session_info` test
    /// covers it, through the agent, in the interactive session it needs.)
    ///
    /// The window station is the process's, so this crate's unit tests
    /// hold no other test that reads it.
    #[test]
    fn a_process_in_a_non_interactive_window_station_reports_no_input_desktop() {
        // SAFETY: no preconditions; the process's own handle, not closed.
        let original = unsafe { GetProcessWindowStation() }.expect("the process's window station");
        // Unnamed, as a sandbox makes one: an ordinary user may not name a
        // window station, and the system names it for the logon session.
        // SAFETY: no name and no security attributes.
        let station = unsafe {
            CreateWindowStationW(
                PCWSTR::null(),
                0,
                u32::try_from(WINSTA_ALL_ACCESS).expect("an access mask"),
                None,
            )
        }
        .expect("creates a window station");
        // SAFETY: a window station handle this process holds.
        unsafe { SetProcessWindowStation(station) }.expect("moves to the new window station");

        let session = current();

        // SAFETY: the process's original window station, still open; the
        // created one is no longer the process's and is not used again.
        unsafe { SetProcessWindowStation(original) }.expect("moves back");
        // SAFETY: as above.
        unsafe { CloseWindowStation(station) }.expect("closes the created window station");

        assert_eq!(
            session.expect("reads the session"),
            Session {
                id: token_session_id(),
                interactive_window_station: false,
                input_desktop_name: None,
            }
        );
    }
}
