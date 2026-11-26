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
//!
//! The shell's one other read of Windows for a decision of Core's is here
//! too: the foreground window as Core's wake carries it for the check after
//! a menu, popup or switcher end ([`live_front`]; Dickson, 2026-11-26).

use verbatim_model::WindowHandle;
use verbatim_model::request::{DesktopFront, Front, FrontWindow, GhostFront, LiveFront};

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
    /// `GetDesktopWindow`.
    fn desktop(&self) -> isize;
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
    /// The thread and process that own `window`
    /// (`GetWindowThreadProcessId`).
    fn owner(&self, window: isize) -> (u32, u32);
    /// The process whose outpost reads `window`: its owner, or a console
    /// window's console host; `None` for a window that is gone and for a
    /// console window whose host cannot be read
    /// (`verbatim_win32::provider::provider_process`).
    fn provider(&self, window: isize) -> Option<u32>;
    /// The top of `window`'s owner chain (`GetAncestor`, `GA_ROOTOWNER`).
    fn root_owner(&self, window: isize) -> isize;
    /// `window`'s class (`GetClassNameW`).
    fn class(&self, window: isize) -> String;
}

/// The reads of the running system.
struct Windows;

impl FrontReads for Windows {
    fn foreground(&self) -> isize {
        verbatim_win32::window::foreground_window_handle()
    }

    fn desktop(&self) -> isize {
        verbatim_win32::window::desktop_window()
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

    fn owner(&self, window: isize) -> (u32, u32) {
        verbatim_win32::window::window_owner(window)
    }

    fn provider(&self, window: isize) -> Option<u32> {
        verbatim_win32::provider::provider_process(window)
    }

    fn root_owner(&self, window: isize) -> isize {
        verbatim_win32::window::root_owner_of(window)
    }

    fn class(&self, window: isize) -> String {
        verbatim_win32::window::window_class_name(window)
    }
}

/// What is in front, read live when the shell answers a read of an
/// application's current focus unsent
/// (`docs/design/readers-tell-core-asks-build1.md`, "Requests in Core"):
/// F, the foreground window, or the desktop window when there is none,
/// then W, F's thread's focus window or F itself, as NVDA's reads of the
/// focus take it (`NVDAObjects/window/__init__.py` 162 to 181). W is the
/// desktop window with its owner, a ghost with its facts, hung with no
/// ghost, responsive, or gone. The shell decides nothing: Core does.
pub(crate) fn front() -> Front {
    front_of(&Windows)
}

/// Whether no window is in front (`GetForegroundWindow` names none): the
/// check after a menu, popup or switcher end is then answered with what
/// is in front, as NVDA's fake focus reads the desktop.
pub(crate) fn nothing_in_front() -> bool {
    Windows.foreground() == 0
}

/// The window Windows names in front, read as the shell makes Core's wake
/// while a check after a menu, popup or switcher end waits for its time
/// (`SrState::menu_check_waits`): the wake carries it and the check asks
/// by it, as NVDA's fake focus asks Windows for the foreground window when
/// its timer runs (`NVDAObjects/window/__init__.py` 162 to 181), where
/// Core's foreground record waits for the listener's report (Dickson,
/// 2026-11-26). A ghost is mapped to the hung window it stands for, as the
/// listener's report is. The shell decides nothing: Core does.
pub(crate) fn live_front() -> LiveFront {
    live_front_of(&Windows)
}

/// [`live_front`] over `reads`.
fn live_front_of(reads: &impl FrontReads) -> LiveFront {
    let foreground = reads.foreground();
    if foreground == 0 {
        return LiveFront::Nothing;
    }
    let window = reads.ghost_of(foreground).unwrap_or(foreground);
    let Some(process) = reads.provider(window) else {
        return LiveFront::Gone;
    };
    let (thread, _) = reads.owner(window);
    LiveFront::Window(FrontWindow {
        window: window_handle(window),
        root_owner: window_handle(reads.root_owner(window)),
        process: verbatim_model::Pid(process),
        thread,
        class: reads.class(window),
    })
}

/// [`front`] over `reads`.
fn front_of(reads: &impl FrontReads) -> Front {
    let foreground = match reads.foreground() {
        0 => reads.desktop(),
        foreground => foreground,
    };
    let window = reads
        .thread_focus(reads.thread_of(foreground))
        .unwrap_or(foreground);
    if !reads.exists(window) {
        return Front::Gone;
    }
    if window == reads.desktop() {
        let (thread, process) = reads.owner(window);
        return Front::Desktop(DesktopFront {
            desktop: window_handle(window),
            process: verbatim_model::Pid(process),
            thread,
        });
    }
    if let Some(stands_for) = reads.ghost_of(window) {
        let (thread, process) = reads.owner(stands_for);
        return Front::Ghost(GhostFront {
            ghost: window_handle(window),
            stands_for: window_handle(stands_for),
            process: verbatim_model::Pid(process),
            thread,
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

    /// The desktop window, as a test sets it.
    const DESKTOP: isize = 3;

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

        fn desktop(&self) -> isize {
            DESKTOP
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

        fn owner(&self, window: isize) -> (u32, u32) {
            let window = u32::try_from(window).unwrap();
            (window + 100, window + 1_000)
        }

        fn provider(&self, window: isize) -> Option<u32> {
            self.existing
                .contains(&window)
                .then(|| u32::try_from(window).unwrap() + 2_000)
        }

        fn root_owner(&self, window: isize) -> isize {
            window + 10
        }

        fn class(&self, window: isize) -> String {
            format!("class {window}")
        }
    }

    /// The window in front is read with the process whose outpost reads
    /// it, which is not its owner for a console window and so is read
    /// apart from it, its owning thread, its root owner and its class; a
    /// ghost as the hung window it stands for.
    #[test]
    fn the_live_front_is_the_window_in_front_or_the_hung_window_its_ghost_stands_for() {
        let window = |window: u64| {
            let small = u32::try_from(window).unwrap();
            LiveFront::Window(FrontWindow {
                window: WindowHandle(window),
                root_owner: WindowHandle(window + 10),
                process: verbatim_model::Pid(small + 2_000),
                thread: small + 100,
                class: format!("class {window}"),
            })
        };
        let plain = Reads {
            foreground: 7,
            existing: vec![7],
            ..Reads::default()
        };
        assert_eq!(live_front_of(&plain), window(7));
        let ghost = Reads {
            foreground: 7,
            existing: vec![7, 9],
            ghost: Some((7, 9)),
            ..Reads::default()
        };
        assert_eq!(live_front_of(&ghost), window(9));
    }

    /// No window in front is nothing, and a window in front with no
    /// process to ask is gone.
    #[test]
    fn the_live_front_with_no_window_is_nothing_and_an_unreadable_one_is_gone() {
        assert_eq!(live_front_of(&Reads::default()), LiveFront::Nothing);
        let gone = Reads {
            foreground: 7,
            ..Reads::default()
        };
        assert_eq!(live_front_of(&gone), LiveFront::Gone);
    }

    /// A ghost in front, whose thread has no focus window, is the ghost
    /// itself, with its text, the window it stands for, that window's
    /// application and thread, and its disabled style.
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
                process: verbatim_model::Pid(1_009),
                thread: 109,
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

    /// No window in front reads the desktop window, with its owner, as
    /// NVDA's `kwargsFromSuper` takes it; its thread's focus window would be
    /// read in its place.
    #[test]
    fn no_window_in_front_is_the_desktop_window() {
        let nothing = Reads {
            existing: vec![DESKTOP],
            ..Reads::default()
        };
        assert_eq!(
            front_of(&nothing),
            Front::Desktop(DesktopFront {
                desktop: WindowHandle(3),
                process: verbatim_model::Pid(1_003),
                thread: 103,
            })
        );
        let focused = Reads {
            focus: Some(8),
            existing: vec![DESKTOP, 8],
            ..Reads::default()
        };
        assert_eq!(front_of(&focused), Front::Responsive);
    }

    /// A focus window that is gone is gone.
    #[test]
    fn a_gone_window_in_front_is_gone() {
        let gone = Reads {
            foreground: 7,
            focus: Some(8),
            existing: vec![7],
            ..Reads::default()
        };
        assert_eq!(front_of(&gone), Front::Gone);
    }
}
