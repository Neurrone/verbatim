//! The shell's live hang check (`docs/design/readers-tell-core-asks-build1.md`,
//! "Requests in Core"): before a request is sent, and every half second
//! for each request out and each window of Core's recovery set, the shell
//! asks Windows whether the request's window is hung, and the answer
//! enters Core's inbox. A ghost window, which Windows shows in place of a
//! hung window and which belongs to another process, is first mapped to
//! the hung window it stands in for, so the hung window, not the ghost, is
//! judged and joins the recovery set. NVDA checks each window live in the
//! same way (`UIAHandler/utils.py` 438 to 459;
//! `IAccessibleHandler/__init__.py` 556 to 576). Every call is local to
//! Windows; none reaches the application.

use verbatim_model::WindowHandle;

/// The window Windows judges for `window`: the hung window a ghost
/// window stands in for (`HungWindowFromGhostWindow`), else `window`
/// itself.
pub(crate) fn judged(window: WindowHandle) -> WindowHandle {
    verbatim_win32::window::hung_window_from_ghost(handle(window)).map_or(window, |hung| {
        WindowHandle(u64::from_ne_bytes(hung.to_ne_bytes()))
    })
}

/// Whether Windows reports `window` hung: `window`, mapped from a ghost
/// to its hung window ([`judged`]), is hung or has a ghost standing in for
/// it (`IsHungAppWindow`, `GhostWindowFromHungWindow`); a window that is
/// gone is never hung, since a call into it fails by itself.
pub(crate) fn hung(window: WindowHandle) -> bool {
    verbatim_win32::window::readability(handle(judged(window))).is_hung()
}

/// `window` as Windows' handle.
fn handle(window: WindowHandle) -> isize {
    isize::from_ne_bytes(window.0.to_ne_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window that is no ghost is judged itself, and a responsive one,
    /// the desktop window, is not hung.
    #[test]
    fn a_window_that_is_no_ghost_is_judged_itself_and_a_responsive_one_is_not_hung() {
        let desktop = WindowHandle(u64::from_ne_bytes(
            verbatim_win32::window::desktop_window().to_ne_bytes(),
        ));
        assert_eq!(judged(desktop), desktop);
        assert!(!hung(desktop));
    }

    /// A handle that names no window is never hung.
    #[test]
    fn a_handle_naming_no_window_is_not_hung() {
        assert!(!hung(WindowHandle(0)));
    }
}
