//! Out-of-context `WinEvent` hooks.
//!
//! MSAA events reach a Verbatim process as `WinEvent`s. The hooks are
//! installed with `WINEVENT_OUTOFCONTEXT`, so callbacks are delivered
//! asynchronously on the thread that installed them — the event thread — via
//! its message loop, with no code injected into the target (architecture
//! section 4). That thread never makes a blocking call into the target: the
//! callback only captures the event address and hands it on.
//!
//! Two callers install these, with two scopes (decision D13). A per-application
//! outpost installs its subscriptions with `idProcess` set to the target pid,
//! so it sees only that application's events. The focus listener installs its
//! subscriptions with `idProcess` zero, so it sees the events that are global
//! by nature — focus, foreground, and menu-popup — across the whole desktop,
//! reads the owning pid from each with a hang-safe local call, and forwards a
//! fact to the target's own outpost. Which events an install subscribes to is
//! the caller's choice ([`WinEventHook::install`] takes the kind list), since
//! the two callers want disjoint sets.
//!
//! The Win32 `WINEVENTPROC` has no user-context argument, so the callback is
//! held in a thread-local. Because one process hosts one hook set on one
//! thread, a thread-local is exactly the right scope.

use std::cell::RefCell;

use crate::com::CHILDID_SELF;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_DESTROY, EVENT_OBJECT_FOCUS, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE,
    EVENT_OBJECT_SELECTION, EVENT_OBJECT_SELECTIONADD, EVENT_OBJECT_SELECTIONREMOVE,
    EVENT_OBJECT_SELECTIONWITHIN, EVENT_OBJECT_STATECHANGE, EVENT_OBJECT_TEXTSELECTIONCHANGED,
    EVENT_OBJECT_VALUECHANGE, EVENT_SYSTEM_ALERT, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MENUEND,
    EVENT_SYSTEM_MENUPOPUPEND, EVENT_SYSTEM_MENUPOPUPSTART, EVENT_SYSTEM_SWITCHEND, OBJID_ALERT,
    OBJID_CARET, OBJID_CLIENT, OBJID_MENU, OBJID_SYSMENU, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
};

/// Which MSAA change a `WinEvent` reports. Events outside this set are dropped
/// at the hook boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinEventKind {
    /// `EVENT_OBJECT_FOCUS`.
    Focus,
    /// `EVENT_SYSTEM_FOREGROUND` — a new window became the foreground window.
    /// The focus listener subscribes to this globally (decision D13),
    /// absorbing the foreground trigger that previously ran inside Core.
    Foreground,
    /// `EVENT_OBJECT_VALUECHANGE`.
    ValueChange,
    /// `EVENT_OBJECT_STATECHANGE`.
    StateChange,
    /// `EVENT_OBJECT_NAMECHANGE`.
    NameChange,
    /// `EVENT_OBJECT_SELECTION`: an item became the selection. The other
    /// three selection events are reported as [`WinEventKind::StateChange`],
    /// as NVDA handles them.
    Selection,
    /// `EVENT_SYSTEM_MENUPOPUPSTART` — a popup menu just opened. NVDA
    /// announces menus from this event; announcing from anything slower (a
    /// foreground-change retry loop, measured live) leaves a noticeable
    /// pause between opening a menu and hearing it.
    MenuPopupStart,
    /// `EVENT_SYSTEM_MENUPOPUPEND` or `EVENT_SYSTEM_MENUEND` — a popup menu
    /// closed, or menu mode ended. The focus listener forwards it to Core,
    /// which reads the real focus of the foreground application if no focus
    /// change follows, as NVDA does.
    MenuEnd,
    /// `EVENT_SYSTEM_SWITCHEND` — the Alt+Tab switcher closed. Handled like
    /// [`WinEventKind::MenuEnd`], as NVDA handles it.
    SwitchEnd,
    /// `EVENT_OBJECT_DESTROY` — an object, possibly a window, was destroyed.
    Destroy,
    /// `EVENT_SYSTEM_ALERT` — an alert was generated; toast notifications
    /// arrive this way (decision D14).
    Alert,
    /// `EVENT_OBJECT_LOCATIONCHANGE` on the system caret (`OBJID_CARET`):
    /// the caret moved, as NVDA hears it for edit controls (milestone M4).
    /// The location changes of every other object are dropped at the hook.
    Caret,
    /// `EVENT_OBJECT_TEXTSELECTIONCHANGED`: a text control's selection, or
    /// its caret, changed.
    TextSelectionChange,
}

/// Every raw `WinEvent` id Verbatim subscribes to, paired with its normalized
/// kind. An install subscribes to the subset whose kind the caller asked for;
/// [`kind_of`] maps a delivered event id back to its kind against this whole
/// table. `StateChange` maps four raw ids to the one kind, so a caller that
/// wants state changes also gets the selection add, remove, and within
/// hooks.
const SUBSCRIPTIONS: [(u32, WinEventKind); 17] = [
    (EVENT_OBJECT_FOCUS, WinEventKind::Focus),
    (EVENT_SYSTEM_FOREGROUND, WinEventKind::Foreground),
    (EVENT_OBJECT_VALUECHANGE, WinEventKind::ValueChange),
    (EVENT_OBJECT_STATECHANGE, WinEventKind::StateChange),
    (EVENT_OBJECT_NAMECHANGE, WinEventKind::NameChange),
    (EVENT_OBJECT_SELECTION, WinEventKind::Selection),
    // Only a plain selection announces a newly selected item; NVDA handles
    // an item added to or removed from a selection, or a selection within a
    // container, as a change of state.
    (EVENT_OBJECT_SELECTIONADD, WinEventKind::StateChange),
    (EVENT_OBJECT_SELECTIONREMOVE, WinEventKind::StateChange),
    (EVENT_OBJECT_SELECTIONWITHIN, WinEventKind::StateChange),
    (EVENT_SYSTEM_MENUPOPUPSTART, WinEventKind::MenuPopupStart),
    (EVENT_SYSTEM_MENUPOPUPEND, WinEventKind::MenuEnd),
    (EVENT_SYSTEM_MENUEND, WinEventKind::MenuEnd),
    (EVENT_SYSTEM_SWITCHEND, WinEventKind::SwitchEnd),
    (EVENT_OBJECT_DESTROY, WinEventKind::Destroy),
    (EVENT_SYSTEM_ALERT, WinEventKind::Alert),
    (EVENT_OBJECT_LOCATIONCHANGE, WinEventKind::Caret),
    (
        EVENT_OBJECT_TEXTSELECTIONCHANGED,
        WinEventKind::TextSelectionChange,
    ),
];

/// The per-application outpost's subscription set (decision D13): the
/// process-scoped property, value, state, and selection events, the caret
/// and text selection (milestone M4), and object destruction (for windows
/// going away). Focus, menu-popup, and the end of a menu are not here — the
/// focus listener owns them globally.
pub const APP_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::ValueChange,
    WinEventKind::StateChange,
    WinEventKind::NameChange,
    WinEventKind::Selection,
    WinEventKind::Destroy,
    WinEventKind::Caret,
    WinEventKind::TextSelectionChange,
];

/// The focus listener's subscription set (decisions D13 and D14): the
/// events that are global by nature, installed with `idProcess` zero. The
/// end of a menu or of the Alt+Tab switcher is global because focus returns
/// to whichever application is then in front, usually not the one that
/// owned the menu.
pub const LISTENER_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::Focus,
    WinEventKind::Foreground,
    WinEventKind::MenuPopupStart,
    WinEventKind::MenuEnd,
    WinEventKind::SwitchEnd,
    WinEventKind::Alert,
];

/// Called on the installing thread for each in-scope event, with the event
/// kind, its MSAA address `(hwnd, id_object, id_child)`, and how many
/// milliseconds ago Windows raised it (delivery to an out-of-context hook
/// waits for this thread's message loop). It must not block: its job is to
/// enqueue the address for handling elsewhere.
pub type WinEventCallback = Box<dyn Fn(WinEventKind, isize, i32, i32, u32)>;

thread_local! {
    static CALLBACK: RefCell<Option<WinEventCallback>> = const { RefCell::new(None) };
}

/// A set of live out-of-context `WinEvent` hooks. Dropping it unhooks them and
/// clears the thread-local callback.
pub struct WinEventHook {
    hooks: Vec<HWINEVENTHOOK>,
}

impl WinEventHook {
    /// Installs hooks for every event whose kind appears in `kinds`, scoped to
    /// `target_pid` (zero for a desktop-global hook), on the current thread,
    /// delivering to `callback`. Call this on the thread that runs the message
    /// loop. The per-application outpost passes [`APP_SUBSCRIPTIONS`]; the
    /// focus listener passes [`LISTENER_SUBSCRIPTIONS`] with `target_pid` zero.
    /// One set of hooks at a time per thread: the thread's callback is
    /// shared by every hook on it.
    ///
    /// # Errors
    ///
    /// Returns an error string if this thread already has a set of hooks, or
    /// if any hook fails to install; any hooks already installed by this
    /// call are removed before returning.
    pub fn install(
        target_pid: u32,
        kinds: &[WinEventKind],
        callback: WinEventCallback,
    ) -> Result<Self, String> {
        let installed = CALLBACK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return false;
            }
            *slot = Some(callback);
            true
        });
        if !installed {
            return Err("this thread already has a set of WinEvent hooks".to_owned());
        }
        let mut hooks = Vec::with_capacity(SUBSCRIPTIONS.len());
        for (event, kind) in SUBSCRIPTIONS {
            if !kinds.contains(&kind) {
                continue;
            }
            // SAFETY: a null module and out-of-context flag are the documented
            // combination for a hook with a same-process proc; `win_event_proc`
            // has the required signature.
            let hook = unsafe {
                SetWinEventHook(
                    event,
                    event,
                    None,
                    Some(win_event_proc),
                    target_pid,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if hook.0.is_null() {
                for installed in hooks {
                    // SAFETY: each handle came from a successful SetWinEventHook.
                    unsafe {
                        let _ = UnhookWinEvent(installed);
                    }
                }
                CALLBACK.with(|slot| *slot.borrow_mut() = None);
                return Err(format!("SetWinEventHook failed for event {event}"));
            }
            hooks.push(hook);
        }
        Ok(Self { hooks })
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            // SAFETY: each handle came from a successful SetWinEventHook.
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
        CALLBACK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Maps a raw event id to a [`WinEventKind`], `None` for events we do not want.
fn kind_of(event: u32) -> Option<WinEventKind> {
    SUBSCRIPTIONS
        .iter()
        .find_map(|&(id, kind)| (id == event).then_some(kind))
}

/// NVDA's early filters for `WinEvent`s, all local checks: a location
/// change matters only for the caret; object ids at or below `OBJID_ALERT`
/// are not accessible objects; a focus on a menu bar object itself is not a
/// real focus; Program Manager and the taskbar never report a foreground
/// change; and the IME candidate window's menu events are not menus (NVDA's
/// `winEventCallback` and its event limiter).
fn is_wanted(kind: WinEventKind, hwnd: HWND, id_object: i32, id_child: i32) -> bool {
    if kind == WinEventKind::Caret {
        return id_object == OBJID_CARET.0;
    }
    if id_object <= OBJID_ALERT.0 {
        return false;
    }
    if kind == WinEventKind::Focus
        && (id_object == OBJID_MENU.0 || id_object == OBJID_SYSMENU.0)
        && id_child == CHILDID_SELF
    {
        return false;
    }
    let class = || crate::window::class_name(hwnd.0 as isize);
    match kind {
        WinEventKind::Foreground => !matches!(class().as_str(), "Progman" | "Shell_TrayWnd"),
        WinEventKind::MenuPopupStart | WinEventKind::MenuEnd => {
            class() != "Microsoft.IME.UIManager.CandidateWindow.Host"
        }
        _ => true,
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    time: u32,
) {
    // SAFETY: GetTickCount has no preconditions; `time` is on the same clock.
    let raised_ms_ago = unsafe { GetTickCount() }.wrapping_sub(time);
    let Some(kind) = kind_of(event) else {
        return;
    };
    if !is_wanted(kind, hwnd, id_object, id_child) {
        return;
    }
    // An event on a window object stands for its client area, as NVDA's
    // event hook treats it, so a window's foreground report and a focus on
    // its client area name one object. A window's destruction keeps the
    // window object, which is how its end is recognized.
    let id_object =
        if kind != WinEventKind::Destroy && id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF
        {
            OBJID_CLIENT.0
        } else {
            id_object
        };
    CALLBACK.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            callback(kind, hwnd.0 as isize, id_object, id_child, raised_ms_ago);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_selection_announces_a_newly_selected_item() {
        assert_eq!(
            kind_of(EVENT_OBJECT_SELECTION),
            Some(WinEventKind::Selection)
        );
        for removed_or_added in [
            EVENT_OBJECT_SELECTIONADD,
            EVENT_OBJECT_SELECTIONREMOVE,
            EVENT_OBJECT_SELECTIONWITHIN,
        ] {
            assert_eq!(kind_of(removed_or_added), Some(WinEventKind::StateChange));
        }
    }

    #[test]
    fn only_the_carets_location_changes_are_wanted() {
        let any = HWND::default();
        assert_eq!(
            kind_of(EVENT_OBJECT_LOCATIONCHANGE),
            Some(WinEventKind::Caret)
        );
        assert!(is_wanted(
            WinEventKind::Caret,
            any,
            OBJID_CARET.0,
            CHILDID_SELF
        ));
        assert!(!is_wanted(
            WinEventKind::Caret,
            any,
            OBJID_WINDOW.0,
            CHILDID_SELF
        ));
    }

    #[test]
    fn events_nvda_ignores_are_filtered_out() {
        let any = HWND::default();
        assert!(!is_wanted(WinEventKind::NameChange, any, OBJID_ALERT.0, 0));
        assert!(!is_wanted(
            WinEventKind::Focus,
            any,
            OBJID_MENU.0,
            CHILDID_SELF
        ));
        assert!(!is_wanted(
            WinEventKind::Focus,
            any,
            OBJID_SYSMENU.0,
            CHILDID_SELF
        ));
        assert!(
            is_wanted(WinEventKind::Focus, any, OBJID_MENU.0, 3),
            "a menu item"
        );
        assert!(is_wanted(
            WinEventKind::Focus,
            any,
            OBJID_CLIENT.0,
            CHILDID_SELF
        ));
    }
}
