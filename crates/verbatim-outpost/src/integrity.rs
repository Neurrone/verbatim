//! The integrity level of a process: the focus listener reads the
//! foreground window's process's for every foreground change, and
//! verbatim.exe reads its own, so that Core can decide whether the
//! keyboard hook may hold keys back for the window in front (architecture
//! section 5; `docs/design/key-holding.md`, "Elevated windows").
//!
//! Windows drops input Verbatim sends to a window of higher integrity, an
//! elevated application's, without an error, since Verbatim runs without
//! the uiAccess privilege; a caret key held back there and sent again
//! would be lost. This guard is temporary, removed when Verbatim gains
//! uiAccess (`docs/roadmap.md`, M8.1).
//!
//! Every read is a local call into the kernel (opening the process and its
//! token, and reading the token's label), which never waits on another
//! process, so the listener's callback may make it.

use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// The integrity level of the process `pid` (its mandatory label's
/// relative identifier, such as `0x2000` for medium and `0x3000` for
/// high), or `None` when it cannot be read.
#[must_use]
pub fn process_integrity(pid: u32) -> Option<u32> {
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

/// This process's own integrity level, read once, or `None` when it cannot
/// be read.
#[must_use]
pub fn own_integrity() -> Option<u32> {
    static OWN: OnceLock<Option<u32>> = OnceLock::new();
    // SAFETY: the pseudo-handle needs no closing.
    *OWN.get_or_init(|| process_level(unsafe { GetCurrentProcess() }))
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
mod tests {
    use super::*;

    /// This test process's own level reads, through its id as through its
    /// pseudo-handle, and a process that does not exist has none.
    #[test]
    fn the_system_reads_a_process_level() {
        assert_eq!(process_integrity(std::process::id()), own_integrity());
        assert_eq!(process_integrity(0), None);
    }
}
