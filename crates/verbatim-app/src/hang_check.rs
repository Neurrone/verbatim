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
use verbatim_model::request::{Front, GhostFront};

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

/// Windows' `handle` as a window.
fn window_handle(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

/// The reads [`front`] makes of Windows, each a local call that sends no
/// application a message.
trait FrontReads {
    /// `GetForegroundWindow`, 0 for none.
    fn foreground(&self) -> isize;
    /// The thread that owns `window` (`GetWindowThreadProcessId`).
    fn thread_of(&self, window: isize) -> u32;
    /// `thread`'s focus window (`GetGUIThreadInfo`'s `hwndFocus`).
    fn thread_focus(&self, thread: u32) -> Option<isize>;
    /// Whether `window` names a window (`IsWindow`).
    fn exists(&self, window: isize) -> bool;
    /// The hung window `window` stands for when it is a ghost
    /// (`HungWindowFromGhostWindow`).
    fn ghost_of(&self, window: isize) -> Option<isize>;
    /// Whether `window` is hung or ghosted (`IsHungAppWindow`,
    /// `GhostWindowFromHungWindow`).
    fn hung(&self, window: isize) -> bool;
    /// `window`'s text (`InternalGetWindowText`), empty when it has none.
    fn text(&self, window: isize) -> String;
    /// Whether `window` has the `WS_DISABLED` style.
    fn disabled(&self, window: isize) -> bool;
}

/// The reads of the running system.
struct Windows;

impl FrontReads for Windows {
    fn foreground(&self) -> isize {
        verbatim_win32::window::foreground_window_handle()
    }

    fn thread_of(&self, window: isize) -> u32 {
        verbatim_win32::window::window_thread(window)
    }

    fn thread_focus(&self, thread: u32) -> Option<isize> {
        verbatim_win32::window::thread_focus_window(thread)
    }

    fn exists(&self, window: isize) -> bool {
        verbatim_win32::window::window_exists(window)
    }

    fn ghost_of(&self, window: isize) -> Option<isize> {
        verbatim_win32::window::hung_window_from_ghost(window)
    }

    fn hung(&self, window: isize) -> bool {
        verbatim_win32::window::readability(window).is_hung()
    }

    fn text(&self, window: isize) -> String {
        verbatim_win32::window::window_text(window).unwrap_or_default()
    }

    fn disabled(&self, window: isize) -> bool {
        /// `WS_DISABLED`.
        const DISABLED: isize = 0x0800_0000;
        verbatim_win32::window::style(window) & DISABLED != 0
    }
}

/// What is in front, read live when a read of an application's current
/// focus is refused as hung (`docs/design/readers-tell-core-asks-build1.md`,
/// "Requests in Core"): the foreground window F, then W, F's thread's focus
/// window or F itself, as NVDA's reads of the focus take it
/// (`NVDAObjects/window/__init__.py` 162 to 181). W is a ghost with its
/// facts, hung with no ghost, responsive, or nothing when no window is in
/// front or W is gone. The shell decides nothing: Core does.
pub(crate) fn front() -> Front {
    front_of(&Windows)
}

/// [`front`] over `reads`.
fn front_of(reads: &impl FrontReads) -> Front {
    let foreground = reads.foreground();
    if foreground == 0 {
        return Front::Nothing;
    }
    let window = reads
        .thread_focus(reads.thread_of(foreground))
        .unwrap_or(foreground);
    if !reads.exists(window) {
        return Front::Nothing;
    }
    if let Some(stands_for) = reads.ghost_of(window) {
        return Front::Ghost(GhostFront {
            ghost: window_handle(window),
            stands_for: window_handle(stands_for),
            name: reads.text(window),
            unavailable: reads.disabled(window),
        });
    }
    if reads.hung(window) {
        Front::Hung
    } else {
        Front::Responsive
    }
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

    /// The windows as a test sets them.
    #[derive(Default)]
    struct Reads {
        foreground: isize,
        /// Every thread's focus window.
        focus: Option<isize>,
        /// The windows that exist.
        existing: Vec<isize>,
        /// A ghost and the hung window it stands for.
        ghost: Option<(isize, isize)>,
        /// The hung windows.
        hung: Vec<isize>,
        disabled: bool,
    }

    impl FrontReads for Reads {
        fn foreground(&self) -> isize {
            self.foreground
        }

        fn thread_of(&self, window: isize) -> u32 {
            u32::try_from(window).unwrap()
        }

        fn thread_focus(&self, _: u32) -> Option<isize> {
            self.focus
        }

        fn exists(&self, window: isize) -> bool {
            self.existing.contains(&window)
        }

        fn ghost_of(&self, window: isize) -> Option<isize> {
            self.ghost
                .filter(|(ghost, _)| *ghost == window)
                .map(|(_, hung)| hung)
        }

        fn hung(&self, window: isize) -> bool {
            self.hung.contains(&window)
        }

        fn text(&self, window: isize) -> String {
            format!("window {window}")
        }

        fn disabled(&self, _: isize) -> bool {
            self.disabled
        }
    }

    /// A ghost in front, whose thread has no focus window, is the ghost
    /// itself, with its text, the window it stands for and its disabled
    /// style.
    #[test]
    fn a_ghost_in_front_is_read_with_its_facts() {
        let reads = Reads {
            foreground: 7,
            existing: vec![7],
            ghost: Some((7, 9)),
            hung: vec![7, 9],
            disabled: true,
            ..Reads::default()
        };
        assert_eq!(
            front_of(&reads),
            Front::Ghost(GhostFront {
                ghost: WindowHandle(7),
                stands_for: WindowHandle(9),
                name: "window 7".to_owned(),
                unavailable: true,
            })
        );
    }

    /// The foreground thread's focus window is read in place of the
    /// foreground window, as NVDA's `kwargsFromSuper` takes it.
    #[test]
    fn the_foreground_threads_focus_window_is_read() {
        let reads = Reads {
            foreground: 7,
            focus: Some(8),
            existing: vec![7, 8],
            hung: vec![8],
            ..Reads::default()
        };
        assert_eq!(front_of(&reads), Front::Hung);
    }

    /// A hung window in front with no ghost is hung; a responsive one is
    /// responsive.
    #[test]
    fn a_window_in_front_with_no_ghost_is_hung_or_responsive() {
        let hung = Reads {
            foreground: 7,
            existing: vec![7],
            hung: vec![7],
            ..Reads::default()
        };
        assert_eq!(front_of(&hung), Front::Hung);
        let responsive = Reads {
            foreground: 7,
            existing: vec![7],
            ..Reads::default()
        };
        assert_eq!(front_of(&responsive), Front::Responsive);
    }

    /// No window in front, or one that is gone, is nothing.
    #[test]
    fn no_window_in_front_or_a_gone_one_is_nothing() {
        assert_eq!(front_of(&Reads::default()), Front::Nothing);
        let gone = Reads {
            foreground: 7,
            focus: Some(8),
            existing: vec![7],
            ..Reads::default()
        };
        assert_eq!(front_of(&gone), Front::Nothing);
    }
}
