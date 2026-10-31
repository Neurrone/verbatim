//! NVDA's test for accepting an event before anything is read of it,
//! ported rule by rule from `shouldAcceptEvent` (`source/eventHandler.py`):
//! an event from a window unrelated to the foreground window is dropped
//! before any call into the application (`docs/parity/focus-events.md`, "Event
//! acceptance"). The outpost's worker makes it for every
//! event but a UIA focus, which the focus listener's reading thread tests
//! with it before reading the focus.
//!
//! NVDA makes the test per event as it processes it, against the foreground
//! window of that moment, not in its event callback, because filtering there
//! lost focus events of applications starting up before the foreground had
//! changed (NVDA issue 4001, `pumpAll` in
//! `source/IAccessibleHandler/__init__.py`). The worker makes it the same
//! way: as it handles each entry.
//!
//! It is defined once, here, for the outposts and the focus listener alike,
//! and its last part, how the window stands to the foreground window
//! ([`foreground_relation`]), is what an outpost reports in an event's
//! window facts, which the reducer judges a focus by after its read
//! (decision D14, "Where the rules run": a rule ported from NVDA is defined
//! once in the crate the judges before a read share).
//!
//! Its window queries come through [`WindowQueries`], which
//! `verbatim_win32::window::SystemWindows` answers with local calls (class
//! names, window ancestry, styles, and the system's foreground and active
//! windows), so the test never waits on the application. NVDA's one rule that calls into an application, for a
//! windowless Chromium document under a Chrome embedding window, is not
//! ported: Verbatim has no Chromium support yet.

use std::collections::HashSet;

use verbatim_core::focus_rules::ForegroundRelation;

/// An event, by the name NVDA gives it when it tests it for acceptance.
/// NVDA's hide and desktop switch events have no counterpart: Verbatim hooks
/// neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventName {
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
    /// A UIA text control's caret or selection moved.
    Caret,
    /// The system caret's location change, and its show, which NVDA
    /// tests by its focus's own rule instead and Verbatim tests as a
    /// location change (`docs/parity/text.md`).
    LocationChange,
    /// A UIA text control's text changed.
    TextChange,
}

/// The events an application's support asked to have from any window of
/// its process, whatever the foreground: NVDA's `requestEvents`, by event,
/// process id, and window class. Nothing asks yet, so it is empty; it is
/// where application support will ask.
#[derive(Debug, Default)]
pub struct RequestedEvents {
    requested: HashSet<(EventName, u32, String)>,
}

impl RequestedEvents {
    fn contains(&self, event: EventName, pid: u32, class: &str) -> bool {
        self.requested.contains(&(event, pid, class.to_owned()))
    }
}

/// The window queries the test makes, all local calls.
pub trait WindowQueries {
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
/// `shouldAcceptEvent`, its rules in its order. One difference: with no
/// window in the foreground, an event from an invalid window is refused,
/// where NVDA's comparison of the two root owners, both 0, accepts it;
/// nothing is spoken differently, since such a window cannot be read.
/// `window` 0 means the event has no window, which cannot be filtered.
pub fn accepts_event(
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
    // progress bars. Verbatim has no such setting yet (M4.3): a background progress
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
    foreground_relation(window, &class, windows).related()
}

/// How `window`, of class `class`, stands to the foreground window: the
/// rules `shouldAcceptEvent` ends with, made for the acceptance test and
/// for the window facts an outpost reports with an event
/// (the outpost's `window_facts`), which the reducer judges a focus by after its
/// read (`ForegroundRelation::related`), so the rule has one definition
/// (decision D14, "Where the rules run"). Every query is a local call.
pub fn foreground_relation(
    window: isize,
    class: &str,
    windows: &impl WindowQueries,
) -> ForegroundRelation {
    let foreground = windows.foreground();
    let root_owner = windows.root_owner(window);
    ForegroundRelation {
        // Windows sharing the foreground window's root owner, as the Office
        // ribbon's (NVDA issues 5504, 14916 and 15432); the foreground
        // application, and windows owned from inside it, as context menus
        // (NVDA issues 3899 and 3905). With no foreground window nothing
        // is in it: an invalid window's root owner reads as 0, as the
        // missing foreground's does, and would otherwise match it.
        in_foreground: foreground != 0
            && (root_owner == windows.root_owner(foreground)
                || is_descendant(windows, foreground, window)
                || is_descendant(windows, foreground, root_owner)),
        // Menus, combo box drop-downs, and the task switching list.
        topmost: windows.is_topmost(window) || windows.is_topmost(windows.root(window)),
        // A UWP window is under the input thread's active window, not under
        // the foreground window (NVDA issue 6713).
        under_active_window: class
            .starts_with("Windows.UI.Core")
            .then(|| is_descendant(windows, windows.active_window(), window)),
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
    fn the_system_carets_event_is_tested_as_a_location_change() {
        let desk = Desk::new();
        assert!(desk.accepts(EventName::LocationChange, FOREGROUND_CHILD));
        assert!(!desk.accepts(EventName::LocationChange, BACKGROUND_CHILD));
    }

    #[test]
    fn the_relation_an_outpost_reports_is_the_one_the_test_ends_with() {
        let mut desk = Desk::new();
        desk.top_level(30, "Windows.UI.Core.CoreWindow", 300);
        desk.active = 30;
        assert_eq!(
            foreground_relation(30, "Windows.UI.Core.CoreWindow", &desk),
            ForegroundRelation {
                in_foreground: false,
                topmost: false,
                under_active_window: Some(true),
            }
        );
        assert_eq!(
            foreground_relation(FOREGROUND_CHILD, "Edit", &desk),
            ForegroundRelation {
                in_foreground: true,
                topmost: false,
                under_active_window: None,
            }
        );
        assert!(!foreground_relation(BACKGROUND_CHILD, "Edit", &desk).related());
    }

    #[test]
    fn an_invalid_window_is_not_in_a_missing_foreground() {
        /// A handle no window has.
        const INVALID: isize = 99;
        let desk = Desk {
            foreground: 0,
            ..Desk::new()
        };
        assert!(!foreground_relation(INVALID, "", &desk).in_foreground);
    }

    #[test]
    fn any_other_window_is_dropped() {
        let desk = Desk::new();
        assert!(!desk.accepts(EventName::GainFocus, BACKGROUND));
        assert!(!desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }
}
