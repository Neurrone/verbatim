//! Whether the foreground window belongs to a process of higher integrity
//! than Verbatim's, where the hook holds no key back.
//!
//! Windows drops input Verbatim sends to a window of higher integrity, an
//! elevated application's, without an error, since Verbatim runs without
//! the uiAccess privilege; a caret key held back there and sent again
//! would be lost. So while such a window is in front the hook passes every
//! key as it comes (`docs/design/key-holding.md`, "Elevated windows"). This
//! is a temporary guard, removed when Verbatim gains uiAccess
//! (`docs/roadmap.md`, M8.1).
//!
//! The answer is worked out once per foreground change, on the hook
//! thread, from its foreground event, and kept in the hook's state, so a
//! key costs no call.

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// Where the integrity levels compared come from: the system for the
/// hook, a fixed answer in tests.
pub(crate) trait IntegritySource {
    /// The foreground window's process's integrity level (its mandatory
    /// label's relative identifier, such as `0x2000` for medium and
    /// `0x3000` for high), or `None` when it cannot be read.
    fn foreground_level(&self) -> Option<u32>;

    /// Verbatim's own integrity level, or `None` when it cannot be read.
    fn own_level(&self) -> Option<u32>;
}

/// Whether the hook must hold no key back for the foreground window: its
/// process's integrity is higher than Verbatim's, or either cannot be
/// read, since a key lost costs more than a key judged without its caret.
/// No foreground window at all holds keys as before.
pub(crate) fn foreground_above(source: &impl IntegritySource) -> bool {
    match (source.foreground_level(), source.own_level()) {
        (Some(foreground), Some(own)) => foreground > own,
        _ => true,
    }
}

/// The system's answer for one foreground window.
pub(crate) struct WindowIntegrity(pub(crate) HWND);

impl IntegritySource for WindowIntegrity {
    fn foreground_level(&self) -> Option<u32> {
        if self.0.is_invalid() {
            return self.own_level();
        }
        let mut pid = 0;
        // SAFETY: tolerates any handle, answering 0 for one that is gone.
        unsafe { GetWindowThreadProcessId(self.0, Some(&raw mut pid)) };
        if pid == 0 {
            return None;
        }
        // SAFETY: opens a handle this function closes below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let level = process_level(process);
        // SAFETY: `process` was opened above and is closed once.
        unsafe {
            let _ = CloseHandle(process);
        }
        level
    }

    fn own_level(&self) -> Option<u32> {
        static OWN: OnceLock<Option<u32>> = OnceLock::new();
        // SAFETY: the pseudo-handle needs no closing.
        *OWN.get_or_init(|| process_level(unsafe { GetCurrentProcess() }))
    }
}

/// The integrity level of `process`'s token.
fn process_level(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a local the call fills; it is closed below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }.ok()?;
    let level = token_level(token);
    // SAFETY: opened above, closed once.
    unsafe {
        let _ = CloseHandle(token);
    }
    level
}

/// The integrity level in `token`'s mandatory label.
fn token_level(token: HANDLE) -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: a size query with no buffer; it fails, filling `size`.
    let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &raw mut size) };
    if size == 0 {
        return None;
    }
    // A buffer of u64s, so the label's pointer within it is aligned.
    let mut buffer = vec![0u64; usize::try_from(size).ok()?.div_ceil(8)];
    // SAFETY: the buffer holds at least `size` bytes.
    unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &raw mut size,
        )
    }
    .ok()?;
    // SAFETY: the call wrote a TOKEN_MANDATORY_LABEL at the buffer's start,
    // whose SID points into the same buffer, alive until this returns.
    let sid = unsafe { &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() }
        .Label
        .Sid;
    // SAFETY: `sid` is the valid SID above.
    let count = unsafe { GetSidSubAuthorityCount(sid) };
    // SAFETY: the count lives in the SID, in the buffer.
    let last = u32::from(unsafe { *count }.checked_sub(1)?);
    // SAFETY: `last` is below the SID's sub-authority count.
    let level = unsafe { GetSidSubAuthority(sid, last) };
    // SAFETY: the sub-authority lives in the SID, in the buffer.
    Some(unsafe { *level })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fixed answer: the foreground's level and Verbatim's.
    pub(crate) struct Fixed(pub(crate) Option<u32>, pub(crate) Option<u32>);

    impl IntegritySource for Fixed {
        fn foreground_level(&self) -> Option<u32> {
            self.0
        }

        fn own_level(&self) -> Option<u32> {
            self.1
        }
    }

    const LOW: u32 = 0x1000;
    const MEDIUM: u32 = 0x2000;
    const HIGH: u32 = 0x3000;

    #[test]
    fn only_a_window_of_higher_or_unknown_integrity_stops_holding() {
        assert!(foreground_above(&Fixed(Some(HIGH), Some(MEDIUM))));
        assert!(!foreground_above(&Fixed(Some(MEDIUM), Some(MEDIUM))));
        assert!(!foreground_above(&Fixed(Some(LOW), Some(MEDIUM))));
        assert!(!foreground_above(&Fixed(Some(HIGH), Some(HIGH))));
        assert!(foreground_above(&Fixed(None, Some(MEDIUM))));
        assert!(foreground_above(&Fixed(Some(MEDIUM), None)));
    }

    /// This test process's own level reads, and a window of its own
    /// process is not above it.
    #[test]
    fn the_system_reads_this_process_level() {
        let own = WindowIntegrity(HWND::default()).own_level();
        assert!(own.is_some());
        assert!(!foreground_above(&WindowIntegrity(HWND::default())));
    }
}
