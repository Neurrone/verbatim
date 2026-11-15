//! What the listener decides about a `WinEvent` before it tells Core of it
//! (ADR 0003, "Readers tell, Core asks"; design
//! `docs/design/readers-tell-core-asks-build1.md`, "Readers' local drops"
//! and "The listener in this build"): NVDA's local drops that cannot hang,
//! the window a tell names, the window facts acceptance reads, gathered
//! with calls to Windows only, and the foreground hold, which keeps every
//! tell behind a foreground event until `GetForegroundWindow` names its
//! window or 20 ms pass. Each is pure over the reads the listener makes,
//! so every case is a test here.

use verbatim_model::event_kind::EventKind;
use verbatim_model::tell::{AcceptanceFacts, Relation, WindowFacet};
use verbatim_model::{Pid, WindowHandle};

/// `OBJID_WINDOW`.
const OBJID_WINDOW: i32 = 0;

/// `OBJID_CLIENT`.
const OBJID_CLIENT: i32 = -4;

/// `CHILDID_SELF`.
const CHILDID_SELF: i32 = 0;

/// The class of the window whose events NVDA ignores entirely, since a
/// message to it makes Windows Live Messenger 2009 exit
/// (`IAccessibleHandler/internalWinEventHandler.py` 183 to 187).
pub const MSN_HIDDEN_CLASS: &str = "MSNHiddenWindowClass";

/// How long the foreground hold keeps tells behind a foreground event
/// whose window `GetForegroundWindow` does not yet name: NVDA's two
/// deferrals of its pump, each of up to 10 ms
/// (`IAccessibleHandler/internalWinEventHandler.py` 30, 233 to 263;
/// `core.py` 55), in microseconds.
pub const FOREGROUND_HOLD_US: u64 = 20_000;

/// The window a tell of a `WinEvent` of `kind` on `(hwnd, object, child)`
/// names, or why the event is dropped: NVDA's mapping of a window of 0 to
/// the desktop window for every event, and of an invalid window to it for
/// the end of a menu, a popup menu or the task switcher, dropping any
/// other event with an invalid window
/// (`IAccessibleHandler/internalWinEventHandler.py` 112 to 136); a
/// window's destruction keeps its own window, whatever Windows now says of
/// it, and is dropped for any object but the window's own (Verbatim has no
/// object table to clean). `is_window` asks Windows whether a handle names
/// a window; `desktop` is the desktop window.
///
/// # Errors
///
/// Why the event is dropped.
pub fn tell_window(
    kind: EventKind,
    (hwnd, object, child): (isize, i32, i32),
    is_window: impl FnOnce(isize) -> bool,
    desktop: isize,
) -> Result<isize, &'static str> {
    if kind == EventKind::Destroy {
        return if hwnd != 0 && object == OBJID_WINDOW && child == CHILDID_SELF {
            Ok(hwnd)
        } else {
            Err("the destruction of something other than a window")
        };
    }
    if hwnd == 0 {
        return Ok(desktop);
    }
    if is_window(hwnd) {
        return Ok(hwnd);
    }
    if matches!(
        kind,
        EventKind::SwitchEnd | EventKind::MenuEnd | EventKind::PopupEnd
    ) {
        Ok(desktop)
    } else {
        Err("an invalid window")
    }
}

/// Why an event on a window of class `class`, its object `object` (a
/// window object already read as its client), is dropped by its window's
/// class or its object: any event of [`MSN_HIDDEN_CLASS`]; a show of
/// anything but a client or custom object, as NVDA keeps only those
/// (`IAccessibleHandler/__init__.py` 905, 1039 to 1055), the system
/// caret's show being a caret event and not a show. `None` when it is
/// kept.
#[must_use]
pub fn dropped_by_class(kind: EventKind, class: &str, object: i32) -> Option<&'static str> {
    if class == MSN_HIDDEN_CLASS {
        return Some("a window of Windows Live Messenger's hidden class");
    }
    if kind == EventKind::Show && object != OBJID_CLIENT && object <= 0 {
        return Some("a show of neither a client nor a custom object");
    }
    None
}

/// The reads of Windows' own state the window facts are gathered with,
/// every one a call to Windows and none into an application. A handle of
/// 0 is no window.
pub trait WindowReads {
    /// The window's class name, empty for no window.
    fn class(&self, window: isize) -> String;
    /// The thread and process that own the window by
    /// `GetWindowThreadProcessId`, zeros for no window.
    fn owner(&self, window: isize) -> (u32, u32);
    /// The process that serves a console window, its console host, `None`
    /// when it cannot be read.
    fn console_host(&self, window: isize) -> Option<u32>;
    /// `GetAncestor` with `GA_PARENT`.
    fn parent(&self, window: isize) -> isize;
    /// `GetAncestor` with `GA_ROOT`.
    fn root(&self, window: isize) -> isize;
    /// `GetAncestor` with `GA_ROOTOWNER`.
    fn root_owner(&self, window: isize) -> isize;
    /// Whether `child` is a child window of `parent` (`IsChild`).
    fn is_child(&self, parent: isize, child: isize) -> bool;
    /// Whether the window has the topmost extended style.
    fn topmost(&self, window: isize) -> bool;
    /// The desktop window.
    fn desktop(&self) -> isize;
    /// The foreground window, 0 for none.
    fn foreground(&self) -> isize;
    /// The active window of the foreground thread, 0 for none.
    fn active_window(&self) -> isize;
    /// Whether the window is Verbatim's hidden frame.
    fn hidden_frame(&self, window: isize) -> bool;
}

/// The class of a console host's window.
const CONSOLE_CLASS: &str = "ConsoleWindowClass";

/// The part of a tell window's facts that depends on the window alone,
/// gathered as the event arrives, before a hold could let the window go.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one read, each a yes or no, as the tell carries them"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPart {
    /// The window's handle and its root owner's, which the foreground's
    /// part relates to the foreground.
    handles: (isize, isize),
    window: WindowFacet,
    parent_class: String,
    root: WindowFacet,
    root_owner: WindowFacet,
    window_topmost: bool,
    root_topmost: bool,
    desktop: bool,
    hidden_frame: bool,
    hidden_frame_owned: bool,
}

/// `window` as the model names a window.
fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

/// `window` as acceptance reads it: a console window's process is its
/// console host, `host` when the event's own thread named it, and its
/// thread the event's; any other window's, its owner's.
fn facet(reads: &impl WindowReads, window: isize, host: Option<(u32, u32)>) -> WindowFacet {
    let class = reads.class(window);
    let (thread, owner) = reads.owner(window);
    let (thread, process) = if class == CONSOLE_CLASS {
        match host {
            Some((thread, process)) => (thread, process),
            None => (thread, reads.console_host(window).unwrap_or(0)),
        }
    } else {
        (thread, owner)
    };
    WindowFacet {
        handle: handle(window),
        class,
        process: Pid(process),
        thread,
    }
}

impl WindowPart {
    /// The facts of `window` that depend on it alone, read now. `host` is
    /// the thread that raised the event and its process, which for a
    /// console window name its console host.
    #[must_use]
    pub fn read(reads: &impl WindowReads, window: isize, host: Option<(u32, u32)>) -> Self {
        let root = reads.root(window);
        let root_owner = reads.root_owner(window);
        let hidden_frame = reads.hidden_frame(window);
        let host_of = |handle: isize| (handle == window).then_some(host).flatten();
        Self {
            handles: (window, root_owner),
            window: facet(reads, window, host),
            parent_class: match reads.parent(window) {
                0 => String::new(),
                parent => reads.class(parent),
            },
            root: facet(reads, root, host_of(root)),
            root_owner: facet(reads, root_owner, host_of(root_owner)),
            window_topmost: reads.topmost(window),
            root_topmost: reads.topmost(root),
            desktop: window == reads.desktop(),
            hidden_frame,
            hidden_frame_owned: !hidden_frame && root_owner != 0 && reads.hidden_frame(root_owner),
        }
    }

    /// The whole facts, with the foreground's part read now, as the tell
    /// is placed in the listener's order of tells.
    #[must_use]
    pub fn with_foreground(self, reads: &impl WindowReads) -> AcceptanceFacts {
        let (window, root_owner) = self.handles;
        let foreground = reads.foreground();
        let foreground_root_owner = if foreground == 0 {
            0
        } else {
            reads.root_owner(foreground)
        };
        let active = reads.active_window();
        let relation = |parent: isize, child: isize| {
            if parent == 0 || child == 0 {
                Relation::Outside
            } else if parent == child {
                Relation::Equal
            } else if reads.is_child(parent, child) {
                Relation::Under
            } else {
                Relation::Outside
            }
        };
        AcceptanceFacts {
            window_to_foreground: relation(foreground, window),
            root_owner_to_foreground: relation(foreground, root_owner),
            root_owner_is_foregrounds: foreground != 0 && root_owner == foreground_root_owner,
            window_to_active: relation(active, window),
            foreground: handle(foreground),
            foreground_root_owner: handle(foreground_root_owner),
            active_window: handle(active),
            window: self.window,
            parent_class: self.parent_class,
            root: self.root,
            root_owner: self.root_owner,
            window_topmost: self.window_topmost,
            root_topmost: self.root_topmost,
            desktop: self.desktop,
            hidden_frame: self.hidden_frame,
            hidden_frame_owned: self.hidden_frame_owned,
        }
    }
}

/// The foreground hold: no pending foreground, or the window a foreground
/// event named that `GetForegroundWindow` did not yet name, with every
/// item placed since held behind it, in order, until `GetForegroundWindow`
/// names that window or [`FOREGROUND_HOLD_US`] pass, whichever is first. A
/// newer foreground event during the hold awaits its own window and
/// counts its time again, as NVDA sets the deferred window and resets its
/// count at each foreground event
/// (`IAccessibleHandler/internalWinEventHandler.py` 175 to 177, 233 to
/// 263). Every item goes through it, so the listener's order of tells is
/// the order they were placed in.
#[derive(Debug)]
pub struct ForegroundHold<T> {
    hold: Option<Hold<T>>,
}

impl<T> Default for ForegroundHold<T> {
    fn default() -> Self {
        Self { hold: None }
    }
}

/// A hold in force.
#[derive(Debug)]
struct Hold<T> {
    /// The window awaited.
    window: isize,
    /// When the hold ends whatever the foreground, in microseconds.
    until_us: u64,
    /// What it holds, in order.
    held: Vec<T>,
}

/// How the hold's one timer changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerChange {
    /// As it is.
    Keep,
    /// Set to go off this many microseconds from now.
    Arm(u64),
    /// Stopped: no hold waits for it.
    Disarm,
}

/// What a step of the hold sends, in order, and how its timer changes.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed<T> {
    /// What is sent now, in order.
    pub send: Vec<T>,
    /// The timer.
    pub timer: TimerChange,
}

impl<T> ForegroundHold<T> {
    /// A foreground event for `window`, its tell `item` (`None` for one a
    /// local drop took, which still awaits its window, as NVDA sets the
    /// deferred window before it judges the event), with `front` the
    /// window `GetForegroundWindow` names now and `now` the time: sent at
    /// once, with whatever was held, when `front` is its window; else held,
    /// its window awaited for [`FOREGROUND_HOLD_US`] from now.
    pub fn foreground(
        &mut self,
        window: isize,
        item: Option<T>,
        front: isize,
        now: u64,
    ) -> Placed<T> {
        let had = self.hold.is_some();
        let mut held = self.hold.take().map_or_else(Vec::new, |hold| hold.held);
        held.extend(item);
        if front == window {
            return Placed {
                send: held,
                timer: if had {
                    TimerChange::Disarm
                } else {
                    TimerChange::Keep
                },
            };
        }
        self.hold = Some(Hold {
            window,
            until_us: now.saturating_add(FOREGROUND_HOLD_US),
            held,
        });
        Placed {
            send: Vec::new(),
            timer: TimerChange::Arm(FOREGROUND_HOLD_US),
        }
    }

    /// Any other item, with `front` and `now` as for
    /// [`foreground`](Self::foreground): sent at once when nothing is held;
    /// else held behind the rest, and everything held is sent when `front`
    /// is the awaited window or the hold's time is over.
    pub fn place(&mut self, item: T, front: isize, now: u64) -> Placed<T> {
        let Some(hold) = self.hold.as_mut() else {
            return Placed {
                send: vec![item],
                timer: TimerChange::Keep,
            };
        };
        hold.held.push(item);
        if front == hold.window || now >= hold.until_us {
            let send = self.hold.take().map_or_else(Vec::new, |hold| hold.held);
            return Placed {
                send,
                timer: TimerChange::Disarm,
            };
        }
        Placed {
            send: Vec::new(),
            timer: TimerChange::Keep,
        }
    }

    /// The timer went off, with `front` and `now` as for
    /// [`foreground`](Self::foreground): everything held is sent when the
    /// hold's time is over or `front` is its window; a timer that went off
    /// early is set again for the rest.
    pub fn fired(&mut self, front: isize, now: u64) -> Placed<T> {
        let Some(hold) = self.hold.as_ref() else {
            return Placed {
                send: Vec::new(),
                timer: TimerChange::Keep,
            };
        };
        if front == hold.window || now >= hold.until_us {
            let send = self.hold.take().map_or_else(Vec::new, |hold| hold.held);
            return Placed {
                send,
                timer: TimerChange::Keep,
            };
        }
        Placed {
            send: Vec::new(),
            timer: TimerChange::Arm(hold.until_us.saturating_sub(now)),
        }
    }

    /// Whether a foreground's window is awaited.
    #[must_use]
    pub fn holding(&self) -> bool {
        self.hold.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const DESKTOP: isize = 0x10;

    #[test]
    fn a_window_of_0_is_the_desktop_window_for_every_kind() {
        for kind in [
            EventKind::MsaaFocus,
            EventKind::NameChange,
            EventKind::SwitchEnd,
        ] {
            assert_eq!(
                tell_window(kind, (0, -4, 0), |_| panic!("not asked"), DESKTOP),
                Ok(DESKTOP)
            );
        }
    }

    #[test]
    fn an_invalid_window_is_the_desktop_for_an_end_and_dropped_for_the_rest() {
        for kind in [
            EventKind::SwitchEnd,
            EventKind::MenuEnd,
            EventKind::PopupEnd,
        ] {
            assert_eq!(
                tell_window(kind, (7, -4, 0), |_| false, DESKTOP),
                Ok(DESKTOP)
            );
        }
        for kind in [
            EventKind::MsaaFocus,
            EventKind::Foreground,
            EventKind::PopupStart,
            EventKind::StateChange,
        ] {
            assert_eq!(
                tell_window(kind, (7, -4, 0), |_| false, DESKTOP),
                Err("an invalid window")
            );
        }
        assert_eq!(
            tell_window(EventKind::MsaaFocus, (7, -4, 0), |_| true, DESKTOP),
            Ok(7)
        );
    }

    #[test]
    fn only_a_windows_own_destruction_is_told_whatever_windows_says_of_it() {
        assert_eq!(
            tell_window(EventKind::Destroy, (7, OBJID_WINDOW, 0), |_| false, DESKTOP),
            Ok(7)
        );
        for address in [
            (7, OBJID_CLIENT, 0),
            (7, OBJID_WINDOW, 3),
            (7, 5, 0),
            (0, OBJID_WINDOW, 0),
        ] {
            assert_eq!(
                tell_window(EventKind::Destroy, address, |_| true, DESKTOP),
                Err("the destruction of something other than a window"),
                "{address:?}"
            );
        }
    }

    #[test]
    fn messengers_hidden_window_and_a_show_of_another_object_are_dropped() {
        assert!(dropped_by_class(EventKind::MsaaFocus, MSN_HIDDEN_CLASS, OBJID_CLIENT).is_some());
        assert_eq!(
            dropped_by_class(EventKind::MsaaFocus, "Edit", OBJID_CLIENT),
            None
        );
        assert_eq!(
            dropped_by_class(EventKind::Show, "TTrayAlert", OBJID_CLIENT),
            None
        );
        assert_eq!(dropped_by_class(EventKind::Show, "TTrayAlert", 3), None);
        assert!(dropped_by_class(EventKind::Show, "TTrayAlert", -2).is_some());
        assert_eq!(dropped_by_class(EventKind::SystemCaret, "Edit", -8), None);
    }

    /// A made-up desktop: each window's class, owner, parent, owner and
    /// style.
    #[derive(Default)]
    struct Desk {
        windows: BTreeMap<isize, Made>,
        foreground: isize,
        active: isize,
        frame: isize,
    }

    #[derive(Clone)]
    struct Made {
        class: &'static str,
        thread: u32,
        process: u32,
        parent: isize,
        owner: isize,
        topmost: bool,
    }

    impl Desk {
        fn add(&mut self, window: isize, class: &'static str, parent: isize, owner: isize) {
            self.windows.insert(
                window,
                Made {
                    class,
                    thread: 100,
                    process: 200,
                    parent,
                    owner,
                    topmost: false,
                },
            );
        }

        fn made(&self, window: isize) -> Option<&Made> {
            self.windows.get(&window)
        }
    }

    impl WindowReads for Desk {
        fn class(&self, window: isize) -> String {
            self.made(window)
                .map_or_else(String::new, |made| made.class.to_owned())
        }
        fn owner(&self, window: isize) -> (u32, u32) {
            self.made(window)
                .map_or((0, 0), |made| (made.thread, made.process))
        }
        fn console_host(&self, _window: isize) -> Option<u32> {
            Some(300)
        }
        fn parent(&self, window: isize) -> isize {
            self.made(window).map_or(0, |made| made.parent)
        }
        fn root(&self, window: isize) -> isize {
            let mut at = window;
            while let Some(made) = self.made(at)
                && made.parent != 0
                && made.parent != DESKTOP
            {
                at = made.parent;
            }
            at
        }
        fn root_owner(&self, window: isize) -> isize {
            let mut at = self.root(window);
            while let Some(made) = self.made(at)
                && made.owner != 0
            {
                at = self.root(made.owner);
            }
            at
        }
        fn is_child(&self, parent: isize, child: isize) -> bool {
            let mut at = child;
            while let Some(made) = self.made(at)
                && made.parent != 0
                && made.parent != DESKTOP
            {
                if made.parent == parent {
                    return true;
                }
                at = made.parent;
            }
            false
        }
        fn topmost(&self, window: isize) -> bool {
            self.made(window).is_some_and(|made| made.topmost)
        }
        fn desktop(&self) -> isize {
            DESKTOP
        }
        fn foreground(&self) -> isize {
            self.foreground
        }
        fn active_window(&self) -> isize {
            self.active
        }
        fn hidden_frame(&self, window: isize) -> bool {
            window != 0 && window == self.frame
        }
    }

    #[test]
    fn a_child_of_the_foreground_window_is_under_it_and_shares_its_root_owner() {
        let mut desk = Desk::default();
        desk.add(1, "Notepad", DESKTOP, 0);
        desk.add(2, "Edit", 1, 0);
        desk.foreground = 1;
        desk.active = 1;
        desk.windows.get_mut(&1).unwrap().topmost = true;
        let facts = WindowPart::read(&desk, 2, None).with_foreground(&desk);
        assert_eq!(facts.window.class, "Edit");
        assert_eq!(facts.parent_class, "Notepad");
        assert_eq!(facts.root.handle, WindowHandle(1));
        assert_eq!(facts.root_owner.handle, WindowHandle(1));
        assert_eq!(facts.window_to_foreground, Relation::Under);
        assert_eq!(facts.root_owner_to_foreground, Relation::Equal);
        assert!(facts.root_owner_is_foregrounds);
        assert_eq!(facts.window_to_active, Relation::Under);
        assert!(!facts.window_topmost);
        assert!(facts.root_topmost);
        assert!(!facts.desktop);
        assert!(!facts.hidden_frame);
        assert!(!facts.hidden_frame_owned);
    }

    #[test]
    fn a_window_outside_the_foreground_and_none_in_front_relate_to_nothing() {
        let mut desk = Desk::default();
        desk.add(1, "Notepad", DESKTOP, 0);
        desk.add(3, "Other", DESKTOP, 0);
        desk.foreground = 3;
        let facts = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert_eq!(facts.window_to_foreground, Relation::Outside);
        assert_eq!(facts.root_owner_to_foreground, Relation::Outside);
        assert!(!facts.root_owner_is_foregrounds);
        assert_eq!(facts.window_to_active, Relation::Outside);
        desk.foreground = 0;
        let facts = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert_eq!(facts.foreground, WindowHandle(0));
        assert!(!facts.root_owner_is_foregrounds);
    }

    #[test]
    fn a_window_owned_by_the_hidden_frame_carries_the_mark_and_the_frame_its_own() {
        let mut desk = Desk::default();
        desk.add(1, "VerbatimFrame", DESKTOP, 0);
        desk.add(5, "#32768", DESKTOP, 1);
        desk.frame = 1;
        let menu = WindowPart::read(&desk, 5, None).with_foreground(&desk);
        assert!(menu.hidden_frame_owned);
        assert!(!menu.hidden_frame);
        assert_eq!(menu.root_owner.handle, WindowHandle(1));
        let frame = WindowPart::read(&desk, 1, None).with_foreground(&desk);
        assert!(frame.hidden_frame);
        assert!(!frame.hidden_frame_owned);
    }

    #[test]
    fn a_console_windows_process_is_its_host_named_by_the_events_thread() {
        let mut desk = Desk::default();
        desk.add(9, CONSOLE_CLASS, DESKTOP, 0);
        let told = WindowPart::read(&desk, 9, Some((77, 400))).with_foreground(&desk);
        assert_eq!((told.window.thread, told.window.process), (77, Pid(400)));
        assert_eq!(told.root.process, Pid(400));
        let unnamed = WindowPart::read(&desk, 9, None).with_foreground(&desk);
        assert_eq!(unnamed.window.process, Pid(300));
    }

    #[test]
    fn the_desktop_window_is_marked() {
        let mut desk = Desk::default();
        desk.add(DESKTOP, "#32769", 0, 0);
        assert!(
            WindowPart::read(&desk, DESKTOP, None)
                .with_foreground(&desk)
                .desktop
        );
    }

    #[test]
    fn with_its_window_in_front_a_foreground_and_what_follows_are_sent_at_once() {
        let mut hold = ForegroundHold::default();
        assert_eq!(
            hold.foreground(1, Some("foreground"), 1, 0),
            Placed {
                send: vec!["foreground"],
                timer: TimerChange::Keep
            }
        );
        assert_eq!(hold.place("focus", 1, 10).send, ["focus"]);
        assert!(!hold.holding());
    }

    #[test]
    fn a_held_foreground_is_sent_before_the_focus_that_finds_its_window_in_front() {
        let mut hold = ForegroundHold::default();
        assert_eq!(
            hold.foreground(1, Some("foreground"), 2, 0),
            Placed {
                send: vec![],
                timer: TimerChange::Arm(FOREGROUND_HOLD_US)
            }
        );
        assert_eq!(
            hold.place("old window's change", 2, 5_000),
            Placed {
                send: vec![],
                timer: TimerChange::Keep
            }
        );
        assert_eq!(
            hold.place("focus", 1, 8_000),
            Placed {
                send: vec!["foreground", "old window's change", "focus"],
                timer: TimerChange::Disarm
            }
        );
        assert!(!hold.holding());
    }

    #[test]
    fn the_hold_ends_at_twenty_milliseconds_whatever_the_foreground() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, Some("foreground"), 2, 1_000);
        assert_eq!(
            hold.fired(2, 15_000),
            Placed {
                send: vec![],
                timer: TimerChange::Arm(6_000)
            }
        );
        hold.place("focus", 2, 16_000);
        assert_eq!(
            hold.fired(2, 21_000),
            Placed {
                send: vec!["foreground", "focus"],
                timer: TimerChange::Keep
            }
        );
        assert_eq!(hold.fired(2, 30_000).send, Vec::<&str>::new());
        assert_eq!(hold.place("later", 2, 31_000).send, ["later"]);
    }

    #[test]
    fn an_item_placed_after_the_holds_time_releases_it() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, Some("foreground"), 2, 0);
        assert_eq!(
            hold.place("focus", 2, FOREGROUND_HOLD_US),
            Placed {
                send: vec!["foreground", "focus"],
                timer: TimerChange::Disarm
            }
        );
    }

    #[test]
    fn a_newer_foreground_restarts_the_hold_for_its_own_window() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, Some("first"), 3, 0);
        assert_eq!(
            hold.foreground(2, Some("second"), 3, 15_000),
            Placed {
                send: vec![],
                timer: TimerChange::Arm(FOREGROUND_HOLD_US)
            }
        );
        // The first window in front no longer ends the hold.
        assert_eq!(hold.place("focus", 1, 16_000).send, Vec::<&str>::new());
        // Nor does the first hold's time.
        assert_eq!(hold.fired(1, 21_000).timer, TimerChange::Arm(14_000));
        assert_eq!(
            hold.place("focus in second", 2, 22_000).send,
            ["first", "second", "focus", "focus in second"]
        );
    }

    #[test]
    fn a_newer_foreground_already_in_front_sends_everything_held() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, Some("first"), 3, 0);
        hold.place("change", 3, 1_000);
        assert_eq!(
            hold.foreground(2, Some("second"), 2, 2_000),
            Placed {
                send: vec!["first", "change", "second"],
                timer: TimerChange::Disarm
            }
        );
    }

    #[test]
    fn a_dropped_foreground_still_awaits_its_window() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, None, 2, 0);
        assert!(hold.holding());
        assert_eq!(hold.place("focus", 2, 1_000).send, Vec::<&str>::new());
        assert_eq!(hold.place("next", 1, 2_000).send, ["focus", "next"]);
    }
}
