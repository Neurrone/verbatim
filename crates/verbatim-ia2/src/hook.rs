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

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SELECTION, EVENT_OBJECT_SELECTIONADD,
    EVENT_OBJECT_SELECTIONREMOVE, EVENT_OBJECT_SELECTIONWITHIN, EVENT_OBJECT_STATECHANGE,
    EVENT_OBJECT_VALUECHANGE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MENUPOPUPSTART,
    WINEVENT_OUTOFCONTEXT,
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
    /// `EVENT_OBJECT_SELECTION`, `EVENT_OBJECT_SELECTIONADD`,
    /// `EVENT_OBJECT_SELECTIONREMOVE`, or `EVENT_OBJECT_SELECTIONWITHIN` —
    /// collapsed to one normalized kind since all four report "the
    /// selection within a container changed" and the outpost reads the
    /// current selection from the event's own address regardless of which
    /// one fired (roadmap M3's selection-events bullet).
    Selection,
    /// `EVENT_SYSTEM_MENUPOPUPSTART` — a popup menu just opened. NVDA
    /// announces menus from this event; announcing from anything slower (a
    /// foreground-change retry loop, measured live) leaves a noticeable
    /// pause between opening a menu and hearing it.
    MenuPopupStart,
}

/// Every raw `WinEvent` id Verbatim subscribes to, paired with its normalized
/// kind. An install subscribes to the subset whose kind the caller asked for;
/// [`kind_of`] maps a delivered event id back to its kind against this whole
/// table. `Selection` maps four raw ids to the one kind, so a caller that
/// wants selection events gets all four hooks from naming it once.
const SUBSCRIPTIONS: [(u32, WinEventKind); 10] = [
    (EVENT_OBJECT_FOCUS, WinEventKind::Focus),
    (EVENT_SYSTEM_FOREGROUND, WinEventKind::Foreground),
    (EVENT_OBJECT_VALUECHANGE, WinEventKind::ValueChange),
    (EVENT_OBJECT_STATECHANGE, WinEventKind::StateChange),
    (EVENT_OBJECT_NAMECHANGE, WinEventKind::NameChange),
    (EVENT_OBJECT_SELECTION, WinEventKind::Selection),
    (EVENT_OBJECT_SELECTIONADD, WinEventKind::Selection),
    (EVENT_OBJECT_SELECTIONREMOVE, WinEventKind::Selection),
    (EVENT_OBJECT_SELECTIONWITHIN, WinEventKind::Selection),
    (EVENT_SYSTEM_MENUPOPUPSTART, WinEventKind::MenuPopupStart),
];

/// The per-application outpost's subscription set (decision D13): the
/// process-scoped property, value, state, and selection events. Focus and
/// menu-popup are no longer here — the focus listener owns them globally and
/// routes each back to the app outpost as a fact.
pub const APP_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::ValueChange,
    WinEventKind::StateChange,
    WinEventKind::NameChange,
    WinEventKind::Selection,
];

/// The focus listener's subscription set (decision D13): the three events
/// that are global by nature, installed with `idProcess` zero.
pub const LISTENER_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::Focus,
    WinEventKind::Foreground,
    WinEventKind::MenuPopupStart,
];

/// Called on the installing thread for each in-scope event, with the event
/// kind and its MSAA address `(hwnd, id_object, id_child)`. It must not block:
/// its job is to enqueue the address for handling elsewhere.
pub type WinEventCallback = Box<dyn Fn(WinEventKind, isize, i32, i32)>;

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
    ///
    /// # Errors
    ///
    /// Returns an error string if any hook fails to install; any hooks already
    /// installed by this call are removed before returning.
    pub fn install(
        target_pid: u32,
        kinds: &[WinEventKind],
        callback: WinEventCallback,
    ) -> Result<Self, String> {
        CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
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

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let Some(kind) = kind_of(event) else {
        return;
    };
    CALLBACK.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            callback(kind, hwnd.0 as isize, id_object, id_child);
        }
    });
}
