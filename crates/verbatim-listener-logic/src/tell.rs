//! What the listener decides about a `WinEvent` before it tells Core of it
//! (ADR 0003, "Readers tell, Core asks"; design
//! `docs/design/readers-tell-core-asks-build1.md`, "Readers' local drops"
//! and "The listener in this build"): NVDA's local drops that cannot hang,
//! the window a tell names, and the foreground hold, which keeps every
//! MSAA tell behind a foreground event until `GetForegroundWindow` names
//! its window or 20 ms pass; a UI Automation tell is never held. Each is
//! pure over the reads the listener makes, so every case is a test here.

use verbatim_model::event_kind::EventKind;

/// `OBJID_WINDOW`.
const OBJID_WINDOW: i32 = 0;

/// `OBJID_CLIENT`.
const OBJID_CLIENT: i32 = -4;

/// `CHILDID_SELF`.
const CHILDID_SELF: i32 = 0;

/// `OBJID_CURSOR`, the mouse pointer.
pub const OBJID_CURSOR: i32 = -9;

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

/// Whether an event of `kind` is dropped when its window is hung or
/// ghosted, as NVDA tests for a hang only in `winEventToNVDAEvent`
/// (`IAccessibleHandler/__init__.py` 556 to 576). The kinds that never
/// reach that test answer false: a window's destruction, handled on its
/// own (`processDestroyWinEvent`, 911); the end of a menu, a popup menu
/// or the task switcher, which go to the fake focus (1082 to 1084); the
/// system caret's location change and show, which `processGenericWinEvent`
/// queues as a caret event (636 to 667); and the desktop switch (1076 to
/// 1077). Every other kind is tested, a menu start, a popup start and a
/// caret hide, which is no system caret event, among them. Core asks of an
/// untested kind's hung window as of any other, and its shell's live hang
/// check refuses the request (Dickson, 2026-11-19). For a caret event the
/// not-responding sound plays while the window holds attention, which
/// matches NVDA, whose caret handling blocks on the hung application. For
/// a menu's end the check 50 ms later is refused with the window in front,
/// and a ghost there is spoken as NVDA's fake focus speaks the plain
/// window object it makes of it, with no call into the application
/// (`NVDAObjects/window/__init__.py` 83 to 98;
/// `IAccessibleHandler/__init__.py` 976 to 1002), with no sound (Dickson,
/// 2026-11-19).
#[must_use]
pub fn hang_tested(kind: EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Destroy
            | EventKind::MenuEnd
            | EventKind::PopupEnd
            | EventKind::SwitchEnd
            | EventKind::SystemCaret
            | EventKind::DesktopSwitch
    )
}

/// Why an event on a window of class `class`, its object `object` (a
/// window object already read as its client), is dropped by its window's
/// class or its object: any event of [`MSN_HIDDEN_CLASS`]; a show of
/// anything but a client or custom object, as NVDA keeps only those
/// (`IAccessibleHandler/__init__.py` 905, 1039 to 1055), the system
/// caret's show being a caret event and not a show; and a name change of
/// the mouse pointer ([`OBJID_CURSOR`]), whose name is its shape, which
/// NVDA hands to its mouse-shape reporting before any other handling and
/// speaks only with that reporting on, off by default
/// (`IAccessibleHandler/__init__.py` 673 to 676, `config/configSpec.py`
/// 165): Verbatim has no such reporting. `None` when it is kept.
#[must_use]
pub fn dropped_by_class(kind: EventKind, class: &str, object: i32) -> Option<&'static str> {
    if class == MSN_HIDDEN_CLASS {
        return Some("a window of Windows Live Messenger's hidden class");
    }
    if kind == EventKind::NameChange && object == OBJID_CURSOR {
        return Some("a change of the mouse pointer's shape");
    }
    if kind == EventKind::Show && object != OBJID_CLIENT && object <= 0 {
        return Some("a show of neither a client nor a custom object");
    }
    None
}

/// The foreground hold: no pending foreground, or the window a foreground
/// event named that `GetForegroundWindow` did not yet name, with every
/// item placed since held behind it, in order, until `GetForegroundWindow`
/// names that window or [`FOREGROUND_HOLD_US`] pass, whichever is first. A
/// newer foreground event during the hold awaits its own window and
/// counts its time again, as NVDA sets the deferred window and resets its
/// count at each foreground event
/// (`IAccessibleHandler/internalWinEventHandler.py` 175 to 177, 233 to
/// 263). Every MSAA item goes through it, so the order of those tells is
/// the order they were placed in; a UI Automation tell passes it by
/// ([`ForegroundHold::passing`]), as NVDA's hold defers its `WinEvent`
/// pump alone, and is sent at once (Dickson, 2026-11-17).
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

    /// A UI Automation tell `item`: sent at once, never held, whatever the
    /// hold keeps. Its foreground facts are read as it is sent, so a focus
    /// raised while Windows still names the old window is judged against
    /// that window, as NVDA judges a UI Automation event at its delivery.
    pub fn passing(&self, item: T) -> Placed<T> {
        Placed {
            send: vec![item],
            timer: TimerChange::Keep,
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
    use super::*;

    const DESKTOP: isize = 0x10;

    /// Every kind the listener tells of a `WinEvent`, with whether NVDA
    /// tests its window for a hang: only those `winEventToNVDAEvent`
    /// reaches.
    #[test]
    fn a_hung_window_drops_only_the_kinds_nvda_tests() {
        let rows = [
            (EventKind::MsaaFocus, true),
            (EventKind::Foreground, true),
            (EventKind::ShellForeground, true),
            (EventKind::MenuStart, true),
            (EventKind::MenuEnd, false),
            (EventKind::PopupStart, true),
            (EventKind::PopupEnd, false),
            (EventKind::SwitchEnd, false),
            (EventKind::DesktopSwitch, false),
            (EventKind::Alert, true),
            (EventKind::NameChange, true),
            (EventKind::ValueChange, true),
            (EventKind::StateChange, true),
            (EventKind::DescriptionChange, true),
            (EventKind::Selection, true),
            (EventKind::SelectionAdd, true),
            (EventKind::SelectionRemove, true),
            (EventKind::SelectionWithin, true),
            (EventKind::Show, true),
            (EventKind::Hide, true),
            (EventKind::Destroy, false),
            (EventKind::SystemCaret, false),
        ];
        let answered: Vec<(EventKind, bool)> = rows
            .iter()
            .map(|&(kind, _)| (kind, hang_tested(kind)))
            .collect();
        assert_eq!(answered, rows);
    }

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

    /// The recorded pointer change: a name change on the desktop window,
    /// object `OBJID_CURSOR`, child 0, its name the shape ("Normal").
    #[test]
    fn a_name_change_of_the_mouse_pointer_is_dropped_and_the_desktops_own_is_kept() {
        assert_eq!(
            dropped_by_class(EventKind::NameChange, "#32769", OBJID_CURSOR),
            Some("a change of the mouse pointer's shape")
        );
        assert_eq!(
            dropped_by_class(EventKind::NameChange, "#32769", OBJID_CLIENT),
            None
        );
        assert_eq!(
            dropped_by_class(EventKind::NameChange, "#32769", OBJID_WINDOW),
            None
        );
        // Only the name change: the pointer's other events are judged as
        // any object's.
        assert_eq!(
            dropped_by_class(EventKind::StateChange, "#32769", OBJID_CURSOR),
            None
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

    /// A UI Automation focus placed while an MSAA foreground is held is
    /// sent at once, ahead of the held foreground, which still awaits its
    /// window (change B).
    #[test]
    fn a_uia_tell_passes_a_held_foreground_and_is_sent_at_once() {
        let mut hold = ForegroundHold::default();
        hold.foreground(1, Some("foreground"), 2, 0);
        assert_eq!(
            hold.passing("uia focus"),
            Placed {
                send: vec!["uia focus"],
                timer: TimerChange::Keep
            }
        );
        assert!(hold.holding());
        assert_eq!(
            hold.place("msaa focus", 1, 5_000).send,
            ["foreground", "msaa focus"]
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
