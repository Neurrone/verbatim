//! The UIA arbitration probe: the raw probe, one `WM_GETOBJECT` with
//! `UiaRootObjectId`, which the outposts make on their own application's
//! windows and the focus listener on the windows of the focuses it reads
//! (`docs/design/listener-without-connections.md`, decision 2).

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_WINDOW_HANDLE, ERROR_TIMEOUT, GetLastError, HWND, LPARAM,
    SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::UI::Accessibility::UiaRootObjectId;
use windows::Win32::UI::WindowsAndMessaging::{
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW, WM_GETOBJECT,
};

use verbatim_model::CallKind;

/// How long the raw probe waits for the window to answer: the "no" UIA's
/// own probe gives a busy window comes after about as long
/// (`design-raw-probe.md`, decision 2).
pub const RAW_PROBE_TIMEOUT_MS: u32 = 3000;

/// The one answer that means a window has a server-side provider: what
/// `UiaReturnRawElementProvider` returns for `UiaRootObjectId`. Other
/// nonzero answers, such as File Explorer's child windows'
/// `0xffffffff80040201`, are windows UIA itself says have none.
const PROVIDER_ANSWER: isize = 0x1_0000;

/// What the raw probe found: a provider, or why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawProbe {
    /// The window answered exactly `0x10000`.
    Provider,
    /// The window answered something else; 0 is `DefWindowProc`'s answer.
    Answered(isize),
    /// The window did not answer within [`RAW_PROBE_TIMEOUT_MS`], or
    /// Windows judged its thread hung and did not wait.
    TimedOut,
    /// The window is of a higher integrity level, which Windows does not
    /// let this process send it messages.
    AccessDenied,
    /// The window, or its thread, is gone.
    Gone,
    /// The message failed with another error, or with none set.
    Failed(u32),
}

impl RawProbe {
    /// Whether the window has a server-side UIA provider. Every outcome but
    /// [`RawProbe::Provider`] is a "no", kept as any "no" is.
    #[must_use]
    pub fn has_provider(self) -> bool {
        self == Self::Provider
    }
}

/// Reads the raw probe's outcome from what `SendMessageTimeoutW` gave:
/// `sent`, whether it returned nonzero; `answer`, the window's result; and
/// `last_error`, the thread's last error after a failure.
#[must_use]
pub fn raw_probe_verdict(sent: bool, answer: isize, last_error: u32) -> RawProbe {
    if sent {
        return if answer == PROVIDER_ANSWER {
            RawProbe::Provider
        } else {
            RawProbe::Answered(answer)
        };
    }
    match WIN32_ERROR(last_error) {
        ERROR_TIMEOUT => RawProbe::TimedOut,
        ERROR_ACCESS_DENIED => RawProbe::AccessDenied,
        ERROR_INVALID_WINDOW_HANDLE => RawProbe::Gone,
        _ => RawProbe::Failed(last_error),
    }
}

/// The raw probe: asks a window whether it has a native UIA server-side
/// provider with one `WM_GETOBJECT` for `UiaRootObjectId`, sign-extended
/// as UIA sends it, rather than with a UIA connection
/// (`docs/design/listener-without-connections.md`, section 3, step 4, and
/// decision 2). The outposts make it on their own application's windows
/// and the focus listener on the windows of the focuses it reads. The
/// window is asked once and its answer is the answer, as NVDA asks
/// (`docs/parity/backends.md`); a window too busy to answer in time is a
/// "no", kept as any "no" is.
/// `UiaHasServerSideProvider`, which NVDA calls, sends two `WM_GETOBJECT`s, first with a
/// nonzero wParam and then with wParam 0, and then connects (experiment
/// `e39`; `docs/design/listener-without-connections.md`, the raw probe
/// in its terms); the raw probe sends
/// only the second, with wParam 0, and nothing more. It answers correctly
/// where a new connection would meet another process's queued remote
/// operation.
///
/// The message is sent with `SMTO_BLOCK`, so the caller does not re-enter
/// while it waits; `SMTO_ABORTIFHUNG`, so a thread that has pumped nothing
/// for five seconds is not waited for; and `SMTO_ERRORONEXIT`, so it
/// returns at once when the window's thread ends. It waits at most
/// [`RAW_PROBE_TIMEOUT_MS`], and MUST be called only from a
/// deadline-guarded worker, never from an event thread. Nothing is done
/// with the window's answer but compare it: it holds no reference
/// (`design-raw-probe.md`, "The LRESULT").
#[must_use]
pub fn raw_probe(hwnd: isize) -> RawProbe {
    // UIA's first-time setup happens where it always has, though this
    // probe does not use UIA.
    let _ = crate::client::ensure_ready();
    crate::calls::count(CallKind::WindowMessage);
    let mut answer = 0usize;
    // SAFETY: sets this thread's last error, so an error read after a
    // failure is this call's.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    // SAFETY: a message whose arguments carry no pointer, to any window
    // handle; an invalid one fails the call. The answer is written to a
    // local that outlives the call.
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND(hwnd as *mut _),
            WM_GETOBJECT,
            WPARAM(0),
            LPARAM(UiaRootObjectId as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            RAW_PROBE_TIMEOUT_MS,
            Some(&raw mut answer),
        )
    };
    let sent = sent.0 != 0;
    // SAFETY: reads this thread's last error; no arguments.
    let last_error = if sent { 0 } else { unsafe { GetLastError() }.0 };
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the answer is an LRESULT, which SendMessageTimeoutW hands back as unsigned"
    )]
    let verdict = raw_probe_verdict(sent, answer as isize, last_error);
    tracing::debug!(hwnd, ?verdict, "raw probe");
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_is_no_provider() {
        let verdict = raw_probe_verdict(false, 0, ERROR_TIMEOUT.0);
        assert_eq!(verdict, RawProbe::TimedOut);
        assert!(!verdict.has_provider());
    }

    #[test]
    fn a_refused_message_is_no_provider() {
        let verdict = raw_probe_verdict(false, 0, ERROR_ACCESS_DENIED.0);
        assert_eq!(verdict, RawProbe::AccessDenied);
        assert!(!verdict.has_provider());
    }

    #[test]
    fn a_dead_window_is_no_provider() {
        let verdict = raw_probe_verdict(false, 0, ERROR_INVALID_WINDOW_HANDLE.0);
        assert_eq!(verdict, RawProbe::Gone);
        assert!(!verdict.has_provider());
    }

    #[test]
    fn an_answer_of_zero_is_no_provider() {
        let verdict = raw_probe_verdict(true, 0, 0);
        assert_eq!(verdict, RawProbe::Answered(0));
        assert!(!verdict.has_provider());
    }

    #[test]
    fn an_answer_of_0x10000_is_a_provider() {
        let verdict = raw_probe_verdict(true, 0x1_0000, 0);
        assert_eq!(verdict, RawProbe::Provider);
        assert!(verdict.has_provider());
    }

    #[test]
    fn another_nonzero_answer_is_no_provider() {
        // A File Explorer child window's answer, which UIA's own probe
        // also takes as no provider (design-raw-probe.md, amendments).
        // 0xffffffff80040201 as a signed LRESULT.
        let answer: isize = -0x7ffb_fdff;
        let verdict = raw_probe_verdict(true, answer, 0);
        assert_eq!(verdict, RawProbe::Answered(answer));
        assert!(!verdict.has_provider());
    }
}
