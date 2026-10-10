//! NVDA's test for accepting an event before anything is read of it,
//! ported rule by rule from `shouldAcceptEvent` (`source/eventHandler.py`):
//! an event from a window unrelated to the foreground window is dropped
//! before any call into the application (`docs/parity.md`, "Event
//! acceptance in the outpost").
//!
//! NVDA makes the test per event as it processes it, against the foreground
//! window of that moment, not in its event callback, because filtering there
//! lost focus events of applications starting up before the foreground had
//! changed (NVDA issue 4001, `pumpAll` in
//! `source/IAccessibleHandler/__init__.py`). The worker makes it the same
//! way: as it handles each entry.
//!
//! Every query is a local call (class names, window ancestry, styles, and
//! the system's foreground and active windows), so the test never waits on
//! the application. NVDA's one rule that calls into an application, for a
//! windowless Chromium document under a Chrome embedding window, is not
//! ported: Verbatim has no Chromium support yet.

use std::collections::HashSet;
use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GA_ROOT, GA_ROOTOWNER, GET_ANCESTOR_FLAGS, GUITHREADINFO, GWL_EXSTYLE, GetAncestor,
    GetDesktopWindow, GetGUIThreadInfo, GetWindowLongW, IsChild, WS_EX_TOPMOST,
};

use verbatim_ia2::WinEventKind;

use crate::arbitration::window_class_name;

/// An event, by the name NVDA gives it when it tests it for acceptance.
/// NVDA's hide and desktop switch events have no counterpart: Verbatim hooks
/// neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EventName {
    /// A focus or foreground change, or a UIA menu opening.
    GainFocus,
    /// A popup menu opening.
    MenuStart,
    /// A menu, or menu mode, ending.
    MenuEnd,
    /// The Alt+Tab switcher closing.
    SwitchEnd,
    /// An object shown.
    Show,
    /// An alert.
    Alert,
    /// A value change.
    ValueChange,
    /// A state change, or an item added to or removed from a selection.
    StateChange,
    /// A name change.
    NameChange,
    /// A description change.
    DescriptionChange,
    /// An item becoming the selection.
    Selection,
}

impl EventName {
    /// The name NVDA gives a `WinEvent` of `kind`, or `None` for a kind the
    /// worker handles outside the test: an object destroyed, which NVDA
    /// handles in its event callback, and Verbatim's own kinds, the caret and
    /// text selection of edit controls, the console host's updates, and a
    /// top-level window shown (`docs/parity.md`, "Event acceptance in the
    /// outpost").
    pub(crate) fn of_win_event(kind: WinEventKind) -> Option<Self> {
        Some(match kind {
            WinEventKind::Focus | WinEventKind::Foreground => Self::GainFocus,
            WinEventKind::MenuPopupStart => Self::MenuStart,
            WinEventKind::MenuEnd => Self::MenuEnd,
            WinEventKind::SwitchEnd => Self::SwitchEnd,
            WinEventKind::Show => Self::Show,
            WinEventKind::Alert => Self::Alert,
            WinEventKind::ValueChange => Self::ValueChange,
            WinEventKind::StateChange => Self::StateChange,
            WinEventKind::NameChange => Self::NameChange,
            WinEventKind::DescriptionChange => Self::DescriptionChange,
            WinEventKind::Selection => Self::Selection,
            WinEventKind::Destroy
            | WinEventKind::Caret
            | WinEventKind::TextSelectionChange
            | WinEventKind::ConsoleUpdate
            | WinEventKind::WindowShown => return None,
        })
    }
}

/// The events an application's support asked to have from any window of
/// its process, whatever the foreground: NVDA's `requestEvents`, by event,
/// process id, and window class. Nothing asks yet, so it is empty; it is
/// where application support will ask.
#[derive(Debug, Default)]
pub(crate) struct RequestedEvents {
    requested: HashSet<(EventName, u32, String)>,
}

impl RequestedEvents {
    fn contains(&self, event: EventName, pid: u32, class: &str) -> bool {
        self.requested.contains(&(event, pid, class.to_owned()))
    }
}

/// The window queries the test makes, all local calls.
pub(crate) trait WindowQueries {
    /// The window's class name, empty for an invalid window.
    fn class_name(&self, window: isize) -> String;
    /// The id of the process that owns the window, 0 for none.
    fn process(&self, window: isize) -> u32;
    /// `GetAncestor` with `GA_PARENT`, `GA_ROOT`, or `GA_ROOTOWNER`: 0 for
    /// none.
    fn parent(&self, window: isize) -> isize;
    /// See [`WindowQueries::parent`].
    fn root(&self, window: isize) -> isize;
    /// See [`WindowQueries::parent`].
    fn root_owner(&self, window: isize) -> isize;
    /// Whether `child` is a child window of `parent` (`IsChild`).
    fn is_child(&self, parent: isize, child: isize) -> bool;
    /// Whether the window has the topmost extended style.
    fn is_topmost(&self, window: isize) -> bool;
    /// The desktop window.
    fn desktop(&self) -> isize;
    /// The active window of the thread that has the keyboard input, 0 for
    /// none.
    fn active_window(&self) -> isize;
    /// The foreground window, 0 for none.
    fn foreground(&self) -> isize;
}

/// NVDA's `isDescendantWindow`: `child` is `parent` or one of its child
/// windows.
fn is_descendant(windows: &impl WindowQueries, parent: isize, child: isize) -> bool {
    parent == child || windows.is_child(parent, child)
}

/// Whether an event named `event` from `window` is accepted: NVDA's
/// `shouldAcceptEvent`, its rules in its order. `window` 0 means the event
/// has no window, which cannot be filtered.
pub(crate) fn accepts_event(
    event: EventName,
    window: isize,
    windows: &impl WindowQueries,
    requested: &RequestedEvents,
) -> bool {
    if window == 0 {
        return true;
    }
    let class = windows.class_name(window);
    if requested.contains(event, windows.process(window), &class) {
        return true;
    }
    // NVDA accepts a value change from anywhere when it reports background
    // progress bars. Verbatim has no such setting: a background progress
    // bar is silent, as NVDA's is by default (Dickson, 2026-10-10).
    match event {
        // NVDA's hide rule, never accepted, has no event here.
        EventName::Show => {
            return matches!(
                class.as_str(),
                "Frame Notification Bar"
                    | "tooltips_class32"
                    | "mscandui21.candidate"
                    | "mscandui40.candidate"
                    | "MSCandUIWindow_Candidate"
                    | "TTrayAlert"
            );
        }
        // A toast.
        EventName::Alert
            if windows.class_name(windows.parent(window)) == "ToastChildWindowClass" =>
        {
            return true;
        }
        // Fired on the desktop window or windows that would otherwise be
        // blocked, and turned into focus events (NVDA issues 5302 and 5462).
        EventName::MenuEnd | EventName::SwitchEnd => return true,
        _ => {}
    }
    // The cursor's events come from the desktop window (NVDA issue 5595).
    if window == windows.desktop() {
        return true;
    }
    // A UWP window is under the input thread's active window, not under the
    // foreground window (NVDA issue 6713).
    if class.starts_with("Windows.UI.Core")
        && is_descendant(windows, windows.active_window(), window)
    {
        return true;
    }
    let foreground = windows.foreground();
    // Windows sharing the foreground window's root owner, as the Office
    // ribbon's (NVDA issues 5504, 14916 and 15432).
    if windows.root_owner(window) == windows.root_owner(foreground) {
        return true;
    }
    // The foreground application, and windows owned from inside it, as
    // context menus (NVDA issues 3899 and 3905).
    if is_descendant(windows, foreground, window)
        || is_descendant(windows, foreground, windows.root_owner(window))
    {
        return true;
    }
    // Menus, combo box drop-downs, and the task switching list.
    windows.is_topmost(window) || windows.is_topmost(windows.root(window))
}

/// The system's windows, read with local calls, with the foreground window
/// given: the outpost's foreground reader's.
pub(crate) struct SystemWindows {
    pub(crate) foreground: isize,
}

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

fn ancestor(window: isize, flags: GET_ANCESTOR_FLAGS) -> isize {
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    unsafe { GetAncestor(hwnd(window), flags) }.0 as isize
}

impl WindowQueries for SystemWindows {
    fn class_name(&self, window: isize) -> String {
        window_class_name(window)
    }

    fn process(&self, window: isize) -> u32 {
        super::window::window_owner(window).1
    }

    fn parent(&self, window: isize) -> isize {
        ancestor(window, GA_PARENT)
    }

    fn root(&self, window: isize) -> isize {
        ancestor(window, GA_ROOT)
    }

    fn root_owner(&self, window: isize) -> isize {
        ancestor(window, GA_ROOTOWNER)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        // SAFETY: IsChild tolerates any pair of handles.
        unsafe { IsChild(hwnd(parent), hwnd(child)) }.as_bool()
    }

    fn is_topmost(&self, window: isize) -> bool {
        // SAFETY: GetWindowLongW reads a window's style word; an invalid
        // handle yields 0.
        let style = unsafe { GetWindowLongW(hwnd(window), GWL_EXSTYLE) };
        style.cast_unsigned() & WS_EX_TOPMOST.0 != 0
    }

    fn desktop(&self) -> isize {
        // SAFETY: GetDesktopWindow has no preconditions.
        unsafe { GetDesktopWindow() }.0 as isize
    }

    fn active_window(&self) -> isize {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `info` has cbSize set before the call, which fails safely.
        if unsafe { GetGUIThreadInfo(0, &raw mut info) }.is_err() {
            return 0;
        }
        info.hwndActive.0 as isize
    }

    fn foreground(&self) -> isize {
        self.foreground
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A made-up window: its class, process, parent, root, root owner, and
    /// whether it is topmost.
    #[derive(Clone, Default)]
    struct Window {
        class: &'static str,
        process: u32,
        parent: isize,
        root: isize,
        root_owner: isize,
        topmost: bool,
    }

    /// Made-up windows, the foreground and active windows among them.
    #[derive(Default)]
    struct Desk {
        windows: HashMap<isize, Window>,
        foreground: isize,
        active: isize,
    }

    const DESKTOP: isize = 1;
    const FOREGROUND: isize = 10;
    /// A child window of the foreground window.
    const FOREGROUND_CHILD: isize = 11;
    /// A top-level window unrelated to the foreground window.
    const BACKGROUND: isize = 20;
    const BACKGROUND_CHILD: isize = 21;

    impl Desk {
        /// The foreground window and a child, and a background window and
        /// a child, in processes 100 and 200.
        fn new() -> Self {
            let mut desk = Self {
                foreground: FOREGROUND,
                ..Self::default()
            };
            desk.top_level(FOREGROUND, "Frame", 100);
            desk.child(FOREGROUND_CHILD, FOREGROUND, "Edit");
            desk.top_level(BACKGROUND, "Frame", 200);
            desk.child(BACKGROUND_CHILD, BACKGROUND, "Edit");
            desk
        }

        fn top_level(&mut self, handle: isize, class: &'static str, process: u32) {
            self.windows.insert(
                handle,
                Window {
                    class,
                    process,
                    parent: DESKTOP,
                    root: handle,
                    root_owner: handle,
                    topmost: false,
                },
            );
        }

        fn child(&mut self, handle: isize, parent: isize, class: &'static str) {
            let of = self.windows[&parent].clone();
            self.windows.insert(
                handle,
                Window {
                    class,
                    parent,
                    topmost: false,
                    ..of
                },
            );
        }

        fn window(&mut self, handle: isize) -> &mut Window {
            self.windows.get_mut(&handle).expect("a made-up window")
        }

        fn accepts(&self, event: EventName, window: isize) -> bool {
            accepts_event(event, window, self, &RequestedEvents::default())
        }
    }

    impl WindowQueries for Desk {
        fn class_name(&self, window: isize) -> String {
            self.windows
                .get(&window)
                .map_or_else(String::new, |window| window.class.to_owned())
        }

        fn process(&self, window: isize) -> u32 {
            self.windows.get(&window).map_or(0, |window| window.process)
        }

        fn parent(&self, window: isize) -> isize {
            self.windows.get(&window).map_or(0, |window| window.parent)
        }

        fn root(&self, window: isize) -> isize {
            self.windows.get(&window).map_or(0, |window| window.root)
        }

        fn root_owner(&self, window: isize) -> isize {
            self.windows
                .get(&window)
                .map_or(0, |window| window.root_owner)
        }

        fn is_child(&self, parent: isize, child: isize) -> bool {
            let mut at = child;
            while let Some(window) = self.windows.get(&at) {
                if window.parent == parent && parent != DESKTOP {
                    return true;
                }
                if window.root == at {
                    return false;
                }
                at = window.parent;
            }
            false
        }

        fn is_topmost(&self, window: isize) -> bool {
            self.windows
                .get(&window)
                .is_some_and(|window| window.topmost)
        }

        fn desktop(&self) -> isize {
            DESKTOP
        }

        fn active_window(&self) -> isize {
            self.active
        }

        fn foreground(&self) -> isize {
            self.foreground
        }
    }

    #[test]
    fn an_event_with_no_window_is_accepted() {
        assert!(Desk::new().accepts(EventName::StateChange, 0));
    }

    #[test]
    fn a_requested_event_is_accepted_from_a_background_window() {
        let desk = Desk::new();
        let requested = RequestedEvents {
            requested: HashSet::from([(EventName::Selection, 200, "Edit".to_owned())]),
        };
        assert!(accepts_event(
            EventName::Selection,
            BACKGROUND_CHILD,
            &desk,
            &requested
        ));
        assert!(
            !accepts_event(EventName::NameChange, BACKGROUND_CHILD, &desk, &requested),
            "only the event asked for"
        );
    }

    #[test]
    fn a_background_value_change_is_dropped() {
        assert!(!Desk::new().accepts(EventName::ValueChange, BACKGROUND_CHILD));
    }

    #[test]
    fn a_show_is_accepted_only_from_the_listed_classes() {
        let mut desk = Desk::new();
        desk.top_level(30, "tooltips_class32", 200);
        desk.top_level(31, "TTrayAlert", 200);
        assert!(desk.accepts(EventName::Show, 30));
        assert!(desk.accepts(EventName::Show, 31));
        assert!(
            !desk.accepts(EventName::Show, FOREGROUND_CHILD),
            "not even from the foreground window"
        );
    }

    #[test]
    fn a_toasts_alert_is_accepted_from_the_background() {
        let mut desk = Desk::new();
        desk.top_level(30, "ToastChildWindowClass", 200);
        desk.child(31, 30, "DirectUIHWND");
        assert!(desk.accepts(EventName::Alert, 31));
        assert!(!desk.accepts(EventName::Alert, BACKGROUND_CHILD));
    }

    #[test]
    fn a_menu_or_switch_end_is_accepted_from_anywhere() {
        let desk = Desk::new();
        assert!(desk.accepts(EventName::MenuEnd, BACKGROUND_CHILD));
        assert!(desk.accepts(EventName::SwitchEnd, BACKGROUND_CHILD));
    }

    #[test]
    fn an_event_of_the_desktop_window_is_accepted() {
        assert!(Desk::new().accepts(EventName::NameChange, DESKTOP));
    }

    #[test]
    fn a_ui_core_window_under_the_active_window_is_accepted() {
        let mut desk = Desk::new();
        desk.top_level(30, "ApplicationFrameWindow", 300);
        desk.child(31, 30, "Windows.UI.Core.CoreWindow");
        assert!(!desk.accepts(EventName::GainFocus, 31));
        desk.active = 30;
        assert!(desk.accepts(EventName::GainFocus, 31));
    }

    #[test]
    fn a_window_sharing_the_foregrounds_root_owner_is_accepted() {
        let mut desk = Desk::new();
        desk.window(FOREGROUND).root_owner = 5;
        desk.top_level(30, "Popup", 100);
        desk.window(30).root_owner = 5;
        assert!(desk.accepts(EventName::StateChange, 30));
    }

    #[test]
    fn a_window_inside_the_foreground_window_is_accepted() {
        let desk = Desk::new();
        assert!(desk.accepts(EventName::StateChange, FOREGROUND));
        assert!(desk.accepts(EventName::StateChange, FOREGROUND_CHILD));
    }

    #[test]
    fn a_window_whose_root_owner_is_inside_the_foreground_window_is_accepted() {
        let mut desk = Desk::new();
        desk.top_level(30, "Popup", 100);
        desk.window(30).root_owner = FOREGROUND_CHILD;
        assert!(desk.accepts(EventName::NameChange, 30));
    }

    #[test]
    fn a_topmost_window_is_accepted() {
        let mut desk = Desk::new();
        desk.window(BACKGROUND_CHILD).topmost = true;
        assert!(desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }

    #[test]
    fn a_window_whose_root_is_topmost_is_accepted() {
        let mut desk = Desk::new();
        desk.window(BACKGROUND).topmost = true;
        assert!(desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }

    #[test]
    fn any_other_window_is_dropped() {
        let desk = Desk::new();
        assert!(!desk.accepts(EventName::GainFocus, BACKGROUND));
        assert!(!desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }
}
