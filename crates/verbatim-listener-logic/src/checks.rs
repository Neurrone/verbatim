//! What a UIA focus event goes through before anything of its sender is
//! read, as one pure step function: NVDA's focus handler,
//! `IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent`
//! (`UIAHandler/__init__.py` 874 to 958), and for a menu opening its
//! automation event handler, `IUIAutomationEventHandler_HandleAutomationEvent`
//! (757 to 868), each step in its order
//! (`books/dev/src/reference/adr/0001-focus.md`, "The listener's checks, ported from NVDA's focus
//! handler").
//!
//! The listener's reading thread loops: [`next`] names the one call to make
//! ([`Step::Call`]), the thread makes it and records its answer in
//! [`Gathered`], and asks again, until [`next`] drops the event
//! ([`Step::Drop`]) or lets it through to the read ([`Step::Read`]). Every
//! verdict is a function of the answers gathered, so each is tested from
//! its inputs with no Windows, and the order lives here alone. A
//! [`Gathered`] lives for one read.
//!
//! The test of the sender's cached window for a hung application, NVDA's
//! first step (`_shouldSkipEventForHungWindow`, `UIAHandler/utils.py`
//! 438), is made as the event arrives, in the listener's callback, which
//! sends no marker and queues nothing for a hung sender, so it is not
//! among these. NVDA's duplicate test (step 3) is not made here: a focus
//! that passes the other checks is read in full, and Core, which holds the
//! focus, judges a repeat (`books/dev/src/explanation/design/ownership.md`).

use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_reader_rules::class::{ClassVerdict, WindowClasses, class_verdict};

use crate::focus::{
    COMPARISON_FAILED, CONSOLE_OWN, FocusEvent, HIDDEN_FRAME, MSAA_CLASS, NEAREST_TIMED_OUT,
    NO_WINDOW, NOT_FOCUSED, NOT_RELATED,
};

/// The one call [`next`] asks the reading thread to make, whose answer it
/// records in [`Gathered`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// For a menu opening, `CompareElements` of the sender with the
    /// current focus, the thread's own last read while it is current: a
    /// call into the application ([`Gathered::compared`]).
    Comparison,
    /// The sender's nearest window, `getNearestWindowHandle`, for a sender
    /// that is not a window of its own: a call into the application
    /// ([`Gathered::nearest`]).
    NearestWindow,
    /// The class names of the window the focus is judged with, for the
    /// class verdict: local calls ([`Gathered::classes`]).
    Classes(isize),
    /// The foreground window and NVDA's acceptance test of the window the
    /// focus is judged with against it (`shouldAcceptEvent`): local calls
    /// ([`Gathered::accepted`]).
    Acceptance(isize),
    /// The class name of the sender's own window: a local call
    /// ([`Gathered::own_class`]).
    OwnClass(isize),
    /// Whether the window is in Core's hidden frame: a local call
    /// ([`Gathered::hidden_frame`]).
    HiddenFrame(isize),
    /// The keyboard focus of the current focus's element, for a menu
    /// opening redirected to it, a plain property read: a call into the
    /// application ([`Gathered::keyboard_focus`]).
    KeyboardFocus,
}

/// What [`next`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make this call, record its answer, and ask again.
    Call(Check),
    /// The event is dropped, for this reason.
    Drop(&'static str),
    /// The event is read: judged with `window`, NVDA's object's
    /// `windowHandle`, and with `require_focus` when the read's walk is to
    /// read the sender's own keyboard focus (step 6), after the earlier
    /// element's, which it does for every event but a menu opening
    /// redirected to the current focus, whose keyboard focus the checks
    /// read.
    Read {
        /// The window the focus is judged with.
        window: isize,
        /// Whether the walk reads the sender's keyboard focus.
        require_focus: bool,
    },
}

/// Where the sender's nearest window's lookup ended
/// ([`Check::NearestWindow`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nearest {
    /// The window found.
    Found(isize),
    /// No window above the sender, or a lookup that failed other than by
    /// time.
    None,
    /// The lookup failed by UIA's transaction timeout.
    TimedOut,
}

/// What is known of one event before its read: what the callback and the
/// thread knew of it, and each call's answer once made, `None` until then.
#[derive(Clone, Debug)]
pub struct Gathered {
    /// The event's kind: a focus or a menu opening.
    pub kind: FocusEvent,
    /// The sender's own window from the event's cache, 0 for none.
    pub hwnd: isize,
    /// For a menu opening, the window of the thread's own last read when
    /// that read is current for it (`QueuePlace::current_since`): the
    /// current focus it is compared with; `None` for no current focus.
    pub current: Option<isize>,
    /// Whether a test has the reader skip no window by its class
    /// (`FocusReader::forces_uia`).
    pub forced_uia: bool,
    /// The comparison with the current focus: whether UI Automation counts
    /// the sender as it, or `Err` when the comparison failed.
    pub compared: Option<Result<bool, ()>>,
    /// The sender's nearest window's lookup.
    pub nearest: Option<Nearest>,
    /// The judged window's class names.
    pub classes: Option<WindowClasses>,
    /// Whether NVDA's acceptance test lets the judged window's event
    /// through, against the foreground window.
    pub accepted: Option<bool>,
    /// The class name of the sender's own window.
    pub own_class: Option<String>,
    /// Whether the window is in Core's hidden frame.
    pub hidden_frame: Option<bool>,
    /// The current focus's keyboard focus, read live, or `Err` when the
    /// read failed.
    pub keyboard_focus: Option<Result<bool, ()>>,
}

impl Gathered {
    /// An event of `kind` whose sender's own window is `hwnd`, 0 for none,
    /// with `current` the window of the current focus a menu opening is
    /// compared with, and `forced_uia` a test's choice that no window is
    /// skipped by its class: nothing gathered yet.
    #[must_use]
    pub fn new(kind: FocusEvent, hwnd: isize, current: Option<isize>, forced_uia: bool) -> Self {
        Self {
            kind,
            hwnd,
            current,
            forced_uia,
            compared: None,
            nearest: None,
            classes: None,
            accepted: None,
            own_class: None,
            hidden_frame: None,
            keyboard_focus: None,
        }
    }
}

/// The next step for an event, from what is gathered of it, in NVDA's
/// order:
///
/// - for a menu opening with a current focus, the comparison with it,
///   whose failure drops the event, as NVDA catches no error from it; a
///   sender UI Automation counts as the current focus is judged with that
///   focus's window and without the native check (`UIAHandler/__init__.py`
///   806 to 824);
/// - step 2, the native check (`isNativeUIAElement`, 1515 to 1575): the
///   sender's own window, or its nearest window looked up, a lookup that
///   times out dropping the event ([`NEAREST_TIMED_OUT`]) and one that
///   finds none dropping it ([`NO_WINDOW`]), as NVDA makes no object
///   without a window; then that window's classes, a window the class
///   verdict every outpost's arbitrator gives calls MSAA being skipped
///   ([`MSAA_CLASS`]);
/// - step 4, NVDA's acceptance test against the foreground window
///   ([`NOT_RELATED`]);
/// - step 5, the refusals of NVDA's object before its keyboard focus is
///   read: the console host's own window element (`consoleUIAWindow`,
///   [`CONSOLE_OWN`]), and a focus in Core's hidden frame
///   ([`HIDDEN_FRAME`]);
/// - step 6, the keyboard focus (`shouldAllowUIAFocusEvent`): the sender's
///   is the read's walk's to make, after the earlier element's; a
///   redirected menu opening's is the current focus's, read here, only a
///   read that answers false dropping it ([`NOT_FOCUSED`]), and one that
///   fails letting it through, as NVDA's answers true on a COM error.
#[must_use]
pub fn next(gathered: &Gathered) -> Step {
    let redirected = match (gathered.kind, gathered.current) {
        (FocusEvent::MenuOpened, Some(current)) => match gathered.compared {
            None => return Step::Call(Check::Comparison),
            Some(Err(())) => return Step::Drop(COMPARISON_FAILED),
            Some(Ok(true)) => Some(current),
            Some(Ok(false)) => None,
        },
        _ => None,
    };
    let window = if let Some(window) = redirected {
        window
    } else {
        let window = if gathered.hwnd != 0 {
            gathered.hwnd
        } else {
            match gathered.nearest {
                None => return Step::Call(Check::NearestWindow),
                Some(Nearest::TimedOut) => return Step::Drop(NEAREST_TIMED_OUT),
                Some(Nearest::None) => return Step::Drop(NO_WINDOW),
                Some(Nearest::Found(window)) => window,
            }
        };
        let Some(classes) = &gathered.classes else {
            return Step::Call(Check::Classes(window));
        };
        if !gathered.forced_uia && class_verdict(classes) == ClassVerdict::NonUia {
            return Step::Drop(MSAA_CLASS);
        }
        window
    };
    match gathered.accepted {
        None => return Step::Call(Check::Acceptance(window)),
        Some(false) => return Step::Drop(NOT_RELATED),
        Some(true) => {}
    }
    if gathered.hwnd != 0 {
        match &gathered.own_class {
            None => return Step::Call(Check::OwnClass(gathered.hwnd)),
            Some(class) if class == CONSOLE_WINDOW_CLASS => return Step::Drop(CONSOLE_OWN),
            Some(_) => {}
        }
    }
    let frame_window = if gathered.hwnd != 0 {
        gathered.hwnd
    } else {
        window
    };
    match gathered.hidden_frame {
        None => return Step::Call(Check::HiddenFrame(frame_window)),
        Some(true) => return Step::Drop(HIDDEN_FRAME),
        Some(false) => {}
    }
    if redirected.is_none() {
        return Step::Read {
            window,
            require_focus: true,
        };
    }
    match gathered.keyboard_focus {
        None => Step::Call(Check::KeyboardFocus),
        Some(Ok(false)) => Step::Drop(NOT_FOCUSED),
        Some(Ok(true) | Err(())) => Step::Read {
            window,
            require_focus: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sender's own window.
    const OWN: isize = 0x10;
    /// A windowless sender's nearest window.
    const NEAREST: isize = 0x20;
    /// The current focus's window.
    const CURRENT: isize = 0x30;

    /// Classes on no class list.
    fn plain_classes() -> WindowClasses {
        WindowClasses::new("VerbatimTestWindow", "VerbatimTestWindow")
    }

    /// Classes on NVDA's MSAA class list.
    fn msaa_classes() -> WindowClasses {
        WindowClasses::new("SysTreeView32", "VerbatimTestWindow")
    }

    /// A focus of a windowless sender that passed every check up to the
    /// keyboard focus.
    fn windowless_passed() -> Gathered {
        Gathered {
            nearest: Some(Nearest::Found(NEAREST)),
            classes: Some(plain_classes()),
            accepted: Some(true),
            hidden_frame: Some(false),
            ..Gathered::new(FocusEvent::Focus, 0, None, false)
        }
    }

    /// Each call in NVDA's order for a windowless focus: its nearest
    /// window, its classes, the acceptance test and the hidden frame, then
    /// the read with the sender's keyboard focus left to the walk.
    #[test]
    fn a_windowless_focus_asks_each_check_in_order_and_is_read() {
        let mut gathered = Gathered::new(FocusEvent::Focus, 0, None, false);
        assert_eq!(next(&gathered), Step::Call(Check::NearestWindow));
        gathered.nearest = Some(Nearest::Found(NEAREST));
        assert_eq!(next(&gathered), Step::Call(Check::Classes(NEAREST)));
        gathered.classes = Some(plain_classes());
        assert_eq!(next(&gathered), Step::Call(Check::Acceptance(NEAREST)));
        gathered.accepted = Some(true);
        assert_eq!(next(&gathered), Step::Call(Check::HiddenFrame(NEAREST)));
        gathered.hidden_frame = Some(false);
        assert_eq!(
            next(&gathered),
            Step::Read {
                window: NEAREST,
                require_focus: true
            }
        );
    }

    /// A sender with a window of its own looks no nearest window up, and
    /// is tested for the console's own element and the hidden frame by
    /// its own window.
    #[test]
    fn a_sender_with_its_own_window_asks_its_class_and_is_read() {
        let mut gathered = Gathered::new(FocusEvent::Focus, OWN, None, false);
        assert_eq!(next(&gathered), Step::Call(Check::Classes(OWN)));
        gathered.classes = Some(plain_classes());
        assert_eq!(next(&gathered), Step::Call(Check::Acceptance(OWN)));
        gathered.accepted = Some(true);
        assert_eq!(next(&gathered), Step::Call(Check::OwnClass(OWN)));
        gathered.own_class = Some("VerbatimTestWindow".to_owned());
        assert_eq!(next(&gathered), Step::Call(Check::HiddenFrame(OWN)));
        gathered.hidden_frame = Some(false);
        assert_eq!(
            next(&gathered),
            Step::Read {
                window: OWN,
                require_focus: true
            }
        );
    }

    /// A nearest window's lookup that timed out drops the focus.
    #[test]
    fn a_timed_out_nearest_window_drops_the_focus() {
        let gathered = Gathered {
            nearest: Some(Nearest::TimedOut),
            ..Gathered::new(FocusEvent::Focus, 0, None, false)
        };
        assert_eq!(next(&gathered), Step::Drop(NEAREST_TIMED_OUT));
    }

    /// A sender with no window above it is dropped.
    #[test]
    fn a_sender_with_no_window_is_dropped() {
        let gathered = Gathered {
            nearest: Some(Nearest::None),
            ..Gathered::new(FocusEvent::Focus, 0, None, false)
        };
        assert_eq!(next(&gathered), Step::Drop(NO_WINDOW));
    }

    /// A window on NVDA's MSAA class list is skipped, unless a test forces
    /// UIA.
    #[test]
    fn a_window_on_the_msaa_class_list_is_skipped_unless_forced() {
        let mut gathered = Gathered {
            classes: Some(msaa_classes()),
            ..windowless_passed()
        };
        assert_eq!(next(&gathered), Step::Drop(MSAA_CLASS));
        gathered.forced_uia = true;
        assert_eq!(
            next(&gathered),
            Step::Read {
                window: NEAREST,
                require_focus: true
            }
        );
    }

    /// A window the acceptance test refuses drops the focus.
    #[test]
    fn a_window_the_acceptance_test_refuses_is_dropped() {
        let gathered = Gathered {
            accepted: Some(false),
            ..windowless_passed()
        };
        assert_eq!(next(&gathered), Step::Drop(NOT_RELATED));
    }

    /// The console window's own element is dropped.
    #[test]
    fn the_console_windows_own_element_is_dropped() {
        let gathered = Gathered {
            classes: Some(plain_classes()),
            accepted: Some(true),
            own_class: Some(CONSOLE_WINDOW_CLASS.to_owned()),
            ..Gathered::new(FocusEvent::Focus, OWN, None, false)
        };
        assert_eq!(next(&gathered), Step::Drop(CONSOLE_OWN));
    }

    /// A focus in Core's hidden frame is dropped.
    #[test]
    fn a_focus_in_the_hidden_frame_is_dropped() {
        let gathered = Gathered {
            hidden_frame: Some(true),
            ..windowless_passed()
        };
        assert_eq!(next(&gathered), Step::Drop(HIDDEN_FRAME));
    }

    /// A menu opening with a current focus is compared with it first; one
    /// whose comparison failed is dropped.
    #[test]
    fn a_menu_openings_failed_comparison_drops_it() {
        let mut gathered = Gathered::new(FocusEvent::MenuOpened, 0, Some(CURRENT), false);
        assert_eq!(next(&gathered), Step::Call(Check::Comparison));
        gathered.compared = Some(Err(()));
        assert_eq!(next(&gathered), Step::Drop(COMPARISON_FAILED));
    }

    /// A menu opening UI Automation counts as the current focus is judged
    /// with the focus's window, with no native check, and its keyboard
    /// focus read here: false drops it, true or a failed read lets it
    /// through, its walk reading no keyboard focus.
    #[test]
    fn a_redirected_menu_opening_is_judged_by_the_current_focus() {
        let mut gathered = Gathered::new(FocusEvent::MenuOpened, 0, Some(CURRENT), false);
        gathered.compared = Some(Ok(true));
        assert_eq!(next(&gathered), Step::Call(Check::Acceptance(CURRENT)));
        gathered.accepted = Some(true);
        assert_eq!(next(&gathered), Step::Call(Check::HiddenFrame(CURRENT)));
        gathered.hidden_frame = Some(false);
        assert_eq!(next(&gathered), Step::Call(Check::KeyboardFocus));
        let read = Step::Read {
            window: CURRENT,
            require_focus: false,
        };
        gathered.keyboard_focus = Some(Ok(false));
        assert_eq!(next(&gathered), Step::Drop(NOT_FOCUSED));
        gathered.keyboard_focus = Some(Ok(true));
        assert_eq!(next(&gathered), read);
        gathered.keyboard_focus = Some(Err(()));
        assert_eq!(next(&gathered), read);
    }

    /// A menu opening on another element than the current focus, or with no
    /// current focus, is judged by its own window, as a focus is.
    #[test]
    fn a_menu_opening_on_another_element_is_judged_by_its_own_window() {
        let compared = Gathered {
            compared: Some(Ok(false)),
            ..Gathered::new(FocusEvent::MenuOpened, 0, Some(CURRENT), false)
        };
        assert_eq!(next(&compared), Step::Call(Check::NearestWindow));
        let alone = Gathered::new(FocusEvent::MenuOpened, 0, None, false);
        assert_eq!(next(&alone), Step::Call(Check::NearestWindow));
    }
}
