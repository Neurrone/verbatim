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
//! so it sees only that application's events, its focus, foreground, menu,
//! alert, and tooltip events included (`docs/design/focus-pipeline.md`,
//! section 12.4). The focus listener installs its subscriptions with
//! `idProcess` zero, so it sees those events across the whole desktop, reads
//! the owning pid from each with a hang-safe local call, and forwards a fact
//! to the target's outpost only while that outpost's own hook is not yet in;
//! the end of a menu or of the Alt+Tab switcher, which goes to Core, it
//! always takes. Which events an install subscribes to is the caller's
//! choice ([`WinEventHook::install`] takes the kind list).
//!
//! The Win32 `WINEVENTPROC` has no user-context argument, so the callback is
//! held in a thread-local. Because one process hosts one hook set on one
//! thread, a thread-local is exactly the right scope.
//!
//! A hook set may also hear its own process's sync markers
//! ([`WinEventHook::install_with_sync_marker`]): `WinEvent`s that any
//! thread of the process raises so that the hook thread knows, when one
//! arrives, that every event raised before it has arrived too.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::rc::Rc;

use crate::com::CHILDID_SELF;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{
    HWINEVENTHOOK, NotifyWinEvent, SetWinEventHook, UnhookWinEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, EVENT_AIA_START, EVENT_CONSOLE_CARET, EVENT_CONSOLE_LAYOUT,
    EVENT_CONSOLE_UPDATE_REGION, EVENT_CONSOLE_UPDATE_SCROLL, EVENT_CONSOLE_UPDATE_SIMPLE,
    EVENT_OBJECT_DESCRIPTIONCHANGE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_FOCUS,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SELECTION,
    EVENT_OBJECT_SELECTIONADD, EVENT_OBJECT_SELECTIONREMOVE, EVENT_OBJECT_SELECTIONWITHIN,
    EVENT_OBJECT_SHOW, EVENT_OBJECT_STATECHANGE, EVENT_OBJECT_TEXTSELECTIONCHANGED,
    EVENT_OBJECT_VALUECHANGE, EVENT_SYSTEM_ALERT, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MENUEND,
    EVENT_SYSTEM_MENUPOPUPEND, EVENT_SYSTEM_MENUPOPUPSTART, EVENT_SYSTEM_SWITCHEND, HWND_MESSAGE,
    OBJID_ALERT, OBJID_CARET, OBJID_CLIENT, OBJID_MENU, OBJID_SYSMENU, OBJID_WINDOW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT,
};
use windows::core::w;

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
    /// `EVENT_OBJECT_DESCRIPTIONCHANGE`.
    DescriptionChange,
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
    /// change follows, as NVDA reads the system's focus 50 ms after one
    /// when no valid focus event came with it and its focus has not
    /// changed.
    MenuEnd,
    /// `EVENT_SYSTEM_SWITCHEND` — the Alt+Tab switcher closed. Handled like
    /// [`WinEventKind::MenuEnd`], as NVDA handles it.
    SwitchEnd,
    /// `EVENT_OBJECT_DESTROY` — an object, possibly a window, was destroyed.
    Destroy,
    /// `EVENT_SYSTEM_ALERT` — an alert was generated; toast notifications
    /// arrive this way (decision D14).
    Alert,
    /// `EVENT_OBJECT_LOCATIONCHANGE` or `EVENT_OBJECT_SHOW` on the system
    /// caret (`OBJID_CARET`): the caret moved, or was shown again after a
    /// move, which NVDA handles as a caret event on its focus, whatever API
    /// the focus is read through. The location changes of every other
    /// object are dropped at the hook. A caret show is this kind only for
    /// an install that asked for it, and is never [`WinEventKind::Show`].
    Caret,
    /// `EVENT_OBJECT_TEXTSELECTIONCHANGED`: a text control's selection, or
    /// its caret, changed.
    TextSelectionChange,
    /// `EVENT_OBJECT_SHOW` from a tooltip window (`tooltips_class32`), the
    /// only show events NVDA accepts from standard controls, for the help
    /// balloons it reports. Every other show event is dropped at the hook,
    /// as it would flood.
    Show,
    /// `EVENT_CONSOLE_UPDATE_REGION`, `EVENT_CONSOLE_UPDATE_SIMPLE`,
    /// `EVENT_CONSOLE_UPDATE_SCROLL`, or `EVENT_CONSOLE_LAYOUT`: the console
    /// host's text changed, scrolled, or was laid out anew.
    ConsoleUpdate,
    /// `EVENT_CONSOLE_CARET`: the console host's cursor moved, or the moving
    /// end of a selection did. Its `idObject` holds flags
    /// (`CONSOLE_CARET_SELECTION`, `CONSOLE_CARET_VISIBLE`) and its
    /// `idChild` the cell, its column in the low word and its row in the
    /// high. The console host raises it on its cursor's blink timer, about
    /// every half second, when the cursor has moved since the last, and at
    /// once for a selection's move (`docs/crates/verbatim-outpost.md`,
    /// "Terminals").
    ConsoleCaret,
}

/// Every raw `WinEvent` id Verbatim subscribes to, paired with its normalized
/// kind. An install subscribes to the subset whose kind the caller asked for;
/// [`kind_among`] maps a delivered event id back to its kind among the
/// kinds the install asked for. `StateChange` maps four raw ids
/// to the one kind, so a caller that wants state changes also gets the
/// selection add, remove, and within hooks. Each raw id appears once,
/// checked as the crate compiles.
const SUBSCRIPTIONS: [(u32, WinEventKind); 24] = [
    (EVENT_OBJECT_FOCUS, WinEventKind::Focus),
    (EVENT_SYSTEM_FOREGROUND, WinEventKind::Foreground),
    (EVENT_OBJECT_VALUECHANGE, WinEventKind::ValueChange),
    (EVENT_OBJECT_STATECHANGE, WinEventKind::StateChange),
    (EVENT_OBJECT_NAMECHANGE, WinEventKind::NameChange),
    (
        EVENT_OBJECT_DESCRIPTIONCHANGE,
        WinEventKind::DescriptionChange,
    ),
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
    (EVENT_OBJECT_SHOW, WinEventKind::Show),
    (EVENT_OBJECT_LOCATIONCHANGE, WinEventKind::Caret),
    (
        EVENT_OBJECT_TEXTSELECTIONCHANGED,
        WinEventKind::TextSelectionChange,
    ),
    (EVENT_CONSOLE_UPDATE_REGION, WinEventKind::ConsoleUpdate),
    (EVENT_CONSOLE_UPDATE_SIMPLE, WinEventKind::ConsoleUpdate),
    (EVENT_CONSOLE_UPDATE_SCROLL, WinEventKind::ConsoleUpdate),
    (EVENT_CONSOLE_LAYOUT, WinEventKind::ConsoleUpdate),
    (EVENT_CONSOLE_CARET, WinEventKind::ConsoleCaret),
];

/// The per-application outpost's subscription set (decision D13): the
/// process-scoped focus, foreground, menu opening, alert, and tooltip
/// events, so every MSAA event of the application reaches its outpost by
/// one route in the order Windows raised it (`docs/design/focus-pipeline.md`,
/// section 12.4); the name, description, value, state, and selection
/// events; the caret and text selection (milestone M4); object destruction
/// (for windows going away). The end
/// of a menu and of the Alt+Tab switcher are not here: the focus listener
/// takes them for Core, since focus returns to whichever application is
/// then in front.
pub const APP_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::Focus,
    WinEventKind::Foreground,
    WinEventKind::MenuPopupStart,
    WinEventKind::Alert,
    WinEventKind::Show,
    WinEventKind::ValueChange,
    WinEventKind::StateChange,
    WinEventKind::NameChange,
    WinEventKind::DescriptionChange,
    WinEventKind::Selection,
    WinEventKind::Destroy,
    WinEventKind::Caret,
    WinEventKind::TextSelectionChange,
    WinEventKind::ConsoleUpdate,
    WinEventKind::ConsoleCaret,
];

/// The focus listener's subscription set (decisions D13 and D14), installed
/// with `idProcess` zero: the focus, foreground, menu opening, alert, and
/// tooltip events, which it forwards for an application whose outpost's own
/// hook is not yet in, and the end of a menu or of the Alt+Tab switcher,
/// which is global because focus returns to whichever application is then
/// in front, usually not the one that owned the menu. The console's events
/// are in the set too, for the keyboard hook's view of the foreground
/// (architecture section 5), but the listener keeps them unhooked
/// ([`suspend_kinds`]) except while a console window in front has not yet
/// named its thread through one of them.
pub const LISTENER_SUBSCRIPTIONS: &[WinEventKind] = &[
    WinEventKind::Focus,
    WinEventKind::Foreground,
    WinEventKind::MenuPopupStart,
    WinEventKind::MenuEnd,
    WinEventKind::SwitchEnd,
    WinEventKind::Alert,
    WinEventKind::Show,
    WinEventKind::ConsoleUpdate,
    WinEventKind::ConsoleCaret,
];

/// One `WinEvent` as a hook delivers it.
///
/// Every hook that receives one event receives the same `kind`, address,
/// `thread`, and `time`, so two hooks' copies of one event are told apart
/// from other events by these alone, except from an identical event raised
/// by the same thread within the same tick of the clock
/// (`docs/design/focus-pipeline.md`, section 12.4, "The MSAA hand-over,
/// exactly").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinEvent {
    /// What it reports.
    pub kind: WinEventKind,
    /// Its window, `hwnd`.
    pub hwnd: isize,
    /// Its `idObject`; a window object stands for its client area, as NVDA
    /// treats it, except in a window's destruction.
    pub id_object: i32,
    /// Its `idChild`.
    pub id_child: i32,
    /// The thread that raised it, `idEventThread`: the window's real thread
    /// even for a console window, whose `GetWindowThreadProcessId` names its
    /// client's thread instead (NVDA's `consoleWindowsToThreadIDs`,
    /// `IAccessibleHandler/internalWinEventHandler.py`).
    pub thread: u32,
    /// When Windows raised it, `dwmsEventTime`, on [`event_clock`]'s clock.
    pub time: u32,
    /// How many milliseconds before the callback Windows raised it
    /// (delivery to an out-of-context hook waits for the installing
    /// thread's message loop).
    pub raised_ms_ago: u32,
}

/// Called on the installing thread for each in-scope event. It must not
/// block: its job is to enqueue the event for handling elsewhere.
pub type WinEventCallback = Box<dyn Fn(WinEvent)>;

/// The `WinEvent` id of a sync marker ([`SyncMarker`]): an arbitrary id in
/// the range Windows sets aside for events defined outside it, by the
/// Accessibility Interoperability Alliance (`EVENT_AIA_START` to
/// `EVENT_AIA_END`). The marker hook is scoped to this process, so the same
/// id raised by another process is never taken for a marker, and a hook
/// set takes only the markers raised on its own window, so another hook
/// set's markers in this process are never taken for its own; another
/// process's global hook that asks for this id sees the markers, as it
/// would any process's events in that range.
const SYNC_MARKER_EVENT: u32 = EVENT_AIA_START + 0x05B1;

/// Called on the installing thread with the ticket of each sync marker
/// raised to its hook set ([`SyncMarker::raise`]), in the order the
/// markers arrive.
pub type SyncMarkerCallback = Box<dyn Fn(u64)>;

/// Raises sync markers, from any thread of the process, to the hook set
/// that made it ([`WinEventHook::install_with_sync_marker`]). A marker
/// is a `WinEvent` raised on a message-only window of the hook thread,
/// its ticket's high half in `idObject` and its low half in `idChild`.
///
/// It exists because a thread message posted to the hook thread may be
/// handled before a `WinEvent` raised before the post has been delivered
/// to that thread: `NotifyWinEvent` can return before an out-of-context
/// event reaches the hook thread. `WinEvent`s raised one after another,
/// by different threads and processes, have been seen to arrive in the
/// order raised (none of 1,100 standalone trials out of order, where a
/// posted message overtook the event in up to half), so a marker's
/// arrival is the evidence that every event raised before it has been
/// delivered.
#[derive(Clone, Copy, Debug)]
pub struct SyncMarker {
    /// The message-only window the markers are raised on, which the hook
    /// set destroys when it is dropped.
    hwnd: isize,
}

impl SyncMarker {
    /// Raises a marker carrying `ticket`. It returns at once; the hook
    /// set's thread hands the ticket to its marker callback once every
    /// event raised before this call has been delivered to it. A marker
    /// raised after the set was dropped never arrives.
    pub fn raise(self, ticket: u64) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the ticket's two halves, put together again as it arrives"
        )]
        let (high, low) = ((ticket >> 32) as u32, ticket as u32);
        // SAFETY: raising an event only names the window; Windows checks
        // the handle, and drops an event on one that has been destroyed.
        unsafe {
            NotifyWinEvent(
                SYNC_MARKER_EVENT,
                HWND(self.hwnd as _),
                high.cast_signed(),
                low.cast_signed(),
            );
        }
    }
}

/// The clock `WinEvent`s are stamped with ([`WinEvent::time`]): milliseconds
/// since the system started, `GetTickCount`, which advances in ticks of
/// about 15.6 milliseconds and wraps after 49.7 days.
#[must_use]
pub fn event_clock() -> u32 {
    // SAFETY: GetTickCount has no preconditions.
    unsafe { GetTickCount() }
}

thread_local! {
    static CALLBACK: RefCell<Option<WinEventCallback>> = const { RefCell::new(None) };
    /// The window this thread's sync markers are raised on, and the
    /// callback they are handed to, while its hook set hears them.
    static MARKER_CALLBACK: RefCell<Option<(isize, SyncMarkerCallback)>> =
        const { RefCell::new(None) };
    /// The kinds this thread's hooks were installed for.
    static KINDS: RefCell<Vec<WinEventKind>> = const { RefCell::new(Vec::new()) };
    /// The class of each window [`is_wanted`] has asked about, while this
    /// thread's hooks hear windows destroyed ([`WinEventKind::Destroy`]),
    /// which forgets a destroyed window's: a flood of events from one
    /// window asks its class once. `None` for hooks that do not hear it,
    /// such as the focus listener's, which ask each time.
    static CLASSES: RefCell<Option<WindowCache<Rc<str>>>> = const { RefCell::new(None) };
    /// This thread's hook set: the process it is scoped to, and each raw
    /// event hooked now with its hook, `None` while the thread has none.
    static HOOKS: RefCell<Option<HookSet>> = const { RefCell::new(None) };
}

/// A thread's live hooks ([`HOOKS`]).
struct HookSet {
    target_pid: u32,
    hooked: Vec<(u32, HWINEVENTHOOK)>,
    /// The hook for this process's sync markers and the window they are
    /// raised on, when the set hears them; never narrowed by
    /// [`suspend_kinds`].
    marker: Option<(HWINEVENTHOOK, HWND)>,
}

/// Hooks this process's sync markers on the calling thread, and makes the
/// message-only window they are raised on: a marker raised with no window
/// is never delivered.
fn hook_sync_markers() -> Result<(HWINEVENTHOOK, HWND), String> {
    // SAFETY: a message-only window of a system class, with no parameter
    // that points anywhere; it belongs to the calling thread.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .map_err(|error| format!("the sync marker window could not be made: {error}"))?;
    // SAFETY: GetCurrentProcessId is always sound.
    let pid = unsafe { GetCurrentProcessId() };
    match hook_event(SYNC_MARKER_EVENT, pid) {
        Ok(hook) => Ok((hook, window)),
        Err(error) => {
            destroy_marker_window(window);
            Err(error)
        }
    }
}

/// Destroys the sync markers' window, which [`hook_sync_markers`] made on
/// the calling thread.
fn destroy_marker_window(window: HWND) {
    // SAFETY: the window was made on this thread and is destroyed once.
    unsafe {
        let _ = DestroyWindow(window);
    }
}

/// Hooks raw event `event` out of context, scoped to `target_pid`, on the
/// calling thread, delivering to [`win_event_proc`].
fn hook_event(event: u32, target_pid: u32) -> Result<HWINEVENTHOOK, String> {
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
        return Err(format!("SetWinEventHook failed for event {event}"));
    }
    Ok(hook)
}

/// Removes `hook`, which [`hook_event`] made.
fn unhook_event(hook: HWINEVENTHOOK) {
    // SAFETY: the handle came from a successful SetWinEventHook and is
    // removed once.
    unsafe {
        let _ = UnhookWinEvent(hook);
    }
}

/// What a thread keeps about each window it hears events from, by the
/// window's handle, until the window is destroyed. Each entry also keeps
/// the thread that owned the window when it was learned, so a handle that
/// another thread's window has reused, the first window's destruction
/// having gone unheard, is learned afresh, as the outpost's arbitration
/// keeps its verdicts.
#[derive(Debug)]
pub struct WindowCache<T> {
    entries: HashMap<isize, (u32, T)>,
}

impl<T> Default for WindowCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<T> WindowCache<T> {
    /// What is kept about window `hwnd`, owned now by thread `owner`:
    /// learned with `learn` when nothing is kept for it, or when what is
    /// kept was learned while another thread owned the handle.
    pub fn get_or_learn(&mut self, hwnd: isize, owner: u32, learn: impl FnOnce() -> T) -> &mut T {
        match self.entries.entry(hwnd) {
            Entry::Occupied(entry) => {
                let entry = entry.into_mut();
                if entry.0 != owner {
                    *entry = (owner, learn());
                }
                &mut entry.1
            }
            Entry::Vacant(entry) => &mut entry.insert((owner, learn())).1,
        }
    }

    /// Forgets window `hwnd`: it was destroyed, and its handle may be
    /// reused by an unrelated window.
    pub fn forget(&mut self, hwnd: isize) {
        self.entries.remove(&hwnd);
    }

    /// Forgets every window.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A set of live out-of-context `WinEvent` hooks on the thread that
/// installed it. Dropping it unhooks them and clears the thread-local
/// callback.
pub struct WinEventHook {
    /// The hooks are the installing thread's ([`HOOKS`]), so the set
    /// stays on that thread.
    _thread_bound: std::marker::PhantomData<*const ()>,
}

impl WinEventHook {
    /// Installs hooks for every event whose kind appears in `kinds`, scoped to
    /// `target_pid` (zero for a desktop-global hook), on the current thread,
    /// delivering to `callback`. Call this on the thread that runs the message
    /// loop. The per-application outpost passes [`APP_SUBSCRIPTIONS`]; the
    /// focus listener passes [`LISTENER_SUBSCRIPTIONS`] with `target_pid` zero.
    /// One set of hooks at a time per thread: the thread's callback is
    /// shared by every hook on it. While the set lives, [`suspend_kinds`]
    /// unhooks some of its kinds and hooks them again.
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
        Self::install_set(target_pid, kinds, callback, None).map(|(set, _)| set)
    }

    /// Installs the hooks as [`WinEventHook::install`] does, and with them
    /// a hook for this process's sync markers, which hands each marker's
    /// ticket to `on_marker` on this thread rather than to `callback`.
    /// Returns the set and what raises its markers. The marker hook is part
    /// of the one set: [`suspend_kinds`] leaves it alone, and dropping the
    /// set removes it.
    ///
    /// # Errors
    ///
    /// As [`WinEventHook::install`], and if the marker hook or the window
    /// the markers are raised on cannot be made.
    pub fn install_with_sync_marker(
        target_pid: u32,
        kinds: &[WinEventKind],
        callback: WinEventCallback,
        on_marker: SyncMarkerCallback,
    ) -> Result<(Self, SyncMarker), String> {
        let (set, marker) = Self::install_set(target_pid, kinds, callback, Some(on_marker))?;
        let marker = marker.ok_or("the sync marker hook was not installed")?;
        Ok((set, marker))
    }

    /// Installs the set, with the sync marker hook when `on_marker` is
    /// given.
    fn install_set(
        target_pid: u32,
        kinds: &[WinEventKind],
        callback: WinEventCallback,
        on_marker: Option<SyncMarkerCallback>,
    ) -> Result<(Self, Option<SyncMarker>), String> {
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
        KINDS.with(|slot| *slot.borrow_mut() = kinds.to_vec());
        CLASSES.with(|slot| {
            *slot.borrow_mut() = kinds
                .contains(&WinEventKind::Destroy)
                .then(WindowCache::default);
        });
        let undo = |hooked: Vec<(u32, HWINEVENTHOOK)>| {
            for (_, hook) in hooked {
                unhook_event(hook);
            }
            CALLBACK.with(|slot| *slot.borrow_mut() = None);
            KINDS.with(|slot| slot.borrow_mut().clear());
            CLASSES.with(|slot| *slot.borrow_mut() = None);
        };
        let mut hooked: Vec<(u32, HWINEVENTHOOK)> = Vec::with_capacity(SUBSCRIPTIONS.len());
        for (event, kind) in SUBSCRIPTIONS {
            if !kinds.contains(&kind) {
                continue;
            }
            match hook_event(event, target_pid) {
                Ok(hook) => hooked.push((event, hook)),
                Err(error) => {
                    undo(hooked);
                    return Err(error);
                }
            }
        }
        let marker = match on_marker {
            Some(on_marker) => match hook_sync_markers() {
                Ok(marker) => {
                    let window = marker.1.0 as isize;
                    MARKER_CALLBACK.with(|slot| *slot.borrow_mut() = Some((window, on_marker)));
                    Some(marker)
                }
                Err(error) => {
                    undo(hooked);
                    return Err(error);
                }
            },
            None => None,
        };
        let raiser = marker.map(|(_, window)| SyncMarker {
            hwnd: window.0 as isize,
        });
        HOOKS.with(|slot| {
            *slot.borrow_mut() = Some(HookSet {
                target_pid,
                hooked,
                marker,
            });
        });
        Ok((
            Self {
                _thread_bound: std::marker::PhantomData,
            },
            raiser,
        ))
    }
}

impl Drop for WinEventHook {
    fn drop(&mut self) {
        if let Some(set) = HOOKS.with(|slot| slot.borrow_mut().take()) {
            for (_, hook) in set.hooked {
                unhook_event(hook);
            }
            if let Some((hook, window)) = set.marker {
                unhook_event(hook);
                destroy_marker_window(window);
            }
        }
        CALLBACK.with(|slot| *slot.borrow_mut() = None);
        MARKER_CALLBACK.with(|slot| *slot.borrow_mut() = None);
        KINDS.with(|slot| slot.borrow_mut().clear());
        CLASSES.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Narrows the calling thread's set of hooks while it runs: every raw
/// event of the kinds it was installed for is hooked, except one whose
/// installed kinds are all in `suspended`, which is unhooked. An empty
/// `suspended` hooks every kind again. Events raised while their kind is
/// unhooked never reach the thread, and the thread is not woken for them.
/// Each raw event is hooked and unhooked on its own, within the one set,
/// so this is never a second set on the thread; it may be called from the
/// thread's own callback.
///
/// # Errors
///
/// Returns an error string if the thread has no set of hooks, or if a
/// hook fails to install; the hooks changed before the failure stay
/// changed, and a later call makes the rest.
pub fn suspend_kinds(suspended: &[WinEventKind]) -> Result<(), String> {
    let kinds = KINDS.with(|slot| slot.borrow().clone());
    HOOKS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(set) = slot.as_mut() else {
            return Err("this thread has no set of WinEvent hooks".to_owned());
        };
        for (event, _) in SUBSCRIPTIONS {
            let Some(kind) = kind_among(event, &kinds) else {
                continue;
            };
            let wanted = !suspended.contains(&kind);
            let at = set.hooked.iter().position(|&(id, _)| id == event);
            match (wanted, at) {
                (true, None) => {
                    let hook = hook_event(event, set.target_pid)?;
                    set.hooked.push((event, hook));
                }
                (false, Some(at)) => unhook_event(set.hooked.swap_remove(at).1),
                _ => {}
            }
        }
        Ok(())
    })
}

/// The window class of a standard tooltip, help balloons included.
pub const TOOLTIP_CLASS: &str = "tooltips_class32";

// Each raw event id appears once in `SUBSCRIPTIONS`, so it has one kind.
const _: () = {
    let mut at = 0;
    while at < SUBSCRIPTIONS.len() {
        let mut count = 0;
        let mut other = 0;
        while other < SUBSCRIPTIONS.len() {
            if SUBSCRIPTIONS[other].0 == SUBSCRIPTIONS[at].0 {
                count += 1;
            }
            other += 1;
        }
        assert!(count == 1);
        at += 1;
    }
};

/// The [`WinEventKind`] the raw event `event` on (`hwnd`, `id_object`,
/// `id_child`) has among `kinds`, if [`is_wanted`] keeps it, `None` for
/// events we do not want. It is called for every event, so it allocates
/// nothing. A window's class is asked of `classes` when it keeps
/// them, with the window's owning thread (`window.0`) and its class read
/// from Windows (`window.1`) when it is learned, and of Windows each time
/// otherwise. A window's own destruction forgets its class first.
fn wanted_kind(
    (event, hwnd, id_object, id_child): (u32, isize, i32, i32),
    kinds: &[WinEventKind],
    classes: &mut Option<WindowCache<Rc<str>>>,
    (owner_of, class_of): (impl Fn(isize) -> u32, impl Fn(isize) -> String),
) -> Option<WinEventKind> {
    if event == EVENT_OBJECT_DESTROY
        && id_object == OBJID_WINDOW.0
        && id_child == CHILDID_SELF
        && let Some(classes) = classes.as_mut()
    {
        classes.forget(hwnd);
    }
    // The system caret's show is a caret event, as NVDA handles it, not a
    // tooltip's show.
    if event == EVENT_OBJECT_SHOW && id_object == OBJID_CARET.0 {
        return kinds
            .contains(&WinEventKind::Caret)
            .then_some(WinEventKind::Caret);
    }
    let mut class = || match classes.as_mut() {
        Some(classes) => {
            Rc::clone(classes.get_or_learn(hwnd, owner_of(hwnd), || Rc::from(class_of(hwnd))))
        }
        None => Rc::from(class_of(hwnd)),
    };
    kind_among(event, kinds).filter(|&kind| is_wanted(kind, id_object, id_child, &mut class))
}

/// Maps a raw event id to its [`WinEventKind`] when that kind is among
/// `kinds`.
fn kind_among(event: u32, kinds: &[WinEventKind]) -> Option<WinEventKind> {
    SUBSCRIPTIONS
        .iter()
        .find_map(|&(id, kind)| (id == event && kinds.contains(&kind)).then_some(kind))
}

/// NVDA's early filters for `WinEvent`s, all local checks: a location
/// change matters only for the caret; object ids at or below `OBJID_ALERT`
/// are not accessible objects; a focus on a menu bar object itself is not a
/// real focus; Program Manager and the taskbar never report a foreground
/// change; the IME candidate window's menu events are not menus; and
/// Excel's `EXCEL7` window's events on its child objects, which are its UIA
/// events proxied into MSAA, are dropped before anything asks the window
/// for its provider, which freezes some builds of Excel 2016 (NVDA's
/// `winEventCallback`, `internalWinEventHandler.py` 139 to 150, and its
/// event limiter). `class` gives the window's class.
///
/// None of these applies to the console host's events, whose object and
/// child ids carry data rather than an object: a scroll's horizontal
/// distance, a cell, or flags. NVDA hooks them apart from its other events
/// (`winConsoleHandler.py`), where none of these filters runs, so a scroll
/// of ten or more columns to the left, whose object id is at or below
/// `OBJID_ALERT`, is kept. They have no console-specific check here: the
/// worker takes them only from the focused console terminal's window.
fn is_wanted(
    kind: WinEventKind,
    id_object: i32,
    id_child: i32,
    class: &mut impl FnMut() -> Rc<str>,
) -> bool {
    if matches!(
        kind,
        WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret
    ) {
        return true;
    }
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
    if id_object > 0 && &*class() == "EXCEL7" {
        return false;
    }
    match kind {
        WinEventKind::Foreground => !matches!(&*class(), "Progman" | "Shell_TrayWnd"),
        WinEventKind::MenuPopupStart | WinEventKind::MenuEnd => {
            &*class() != "Microsoft.IME.UIManager.CandidateWindow.Host"
        }
        // NVDA accepts a show event only from a few window classes, of
        // which the tooltip is the standard control's, and only for a
        // client or custom object; a window object stands for the client.
        WinEventKind::Show => {
            (id_object == OBJID_CLIENT.0 || id_object == OBJID_WINDOW.0 || id_object > 0)
                && &*class() == TOOLTIP_CLASS
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
    thread: u32,
    time: u32,
) {
    if event == SYNC_MARKER_EVENT {
        let ticket =
            (u64::from(id_object.cast_unsigned()) << 32) | u64::from(id_child.cast_unsigned());
        // The hook hears every marker of the process; only those raised on
        // this set's own window are its own.
        MARKER_CALLBACK.with(|slot| {
            if let Some((window, on_marker)) = slot.borrow().as_ref()
                && *window == hwnd.0 as isize
            {
                on_marker(ticket);
            }
        });
        return;
    }
    // `time` is on the event clock.
    let raised_ms_ago = event_clock().wrapping_sub(time);
    let wanted = KINDS.with(|kinds| {
        CLASSES.with(|classes| {
            wanted_kind(
                (event, hwnd.0 as isize, id_object, id_child),
                &kinds.borrow(),
                &mut classes.borrow_mut(),
                (crate::window::thread_of, crate::window::class_name),
            )
        })
    });
    if let Some(kind) = wanted {
        // An event on a window object stands for its client area, as NVDA's
        // event hook treats it, so a window's foreground report and a focus
        // on its client area name one object. A window's destruction keeps
        // the window object, which is how its end is recognized.
        // A console event's ids are coordinates, counts, or flags, never an
        // object, and are kept as they are.
        let id_object = if !matches!(
            kind,
            WinEventKind::Destroy | WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret
        ) && id_object == OBJID_WINDOW.0
            && id_child == CHILDID_SELF
        {
            OBJID_CLIENT.0
        } else {
            id_object
        };
        // Every wanted event's arrival with how long ago it was raised, so
        // a delay before it reached this process shows whatever becomes of
        // it, and its raw event id, which tells apart the events one kind
        // stands for, such as the four console updates.
        tracing::debug!(
            ?kind,
            event = format_args!("{event:#06x}"),
            hwnd = hwnd.0 as isize,
            id_object,
            id_child,
            raised_ms_ago,
            "WinEvent received"
        );
        let delivered = WinEvent {
            kind,
            hwnd: hwnd.0 as isize,
            id_object,
            id_child,
            thread,
            time,
            raised_ms_ago,
        };
        CALLBACK.with(|slot| {
            if let Some(callback) = slot.borrow().as_ref() {
                callback(delivered);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// [`is_wanted`] for window `hwnd`, its class read from Windows.
    fn wanted(kind: WinEventKind, hwnd: HWND, id_object: i32, id_child: i32) -> bool {
        let hwnd = hwnd.0 as isize;
        let mut class = || Rc::from(crate::window::class_name(hwnd));
        is_wanted(kind, id_object, id_child, &mut class)
    }

    /// The kind, if any, an outpost's hook keeps of the raw event `event` on object
    /// `id_object` of window 42, its classes kept in `classes`, the window
    /// owned by thread 7 and of class `Edit`, each class read counted in
    /// `reads`.
    fn outpost_kinds(
        event: u32,
        id_object: i32,
        classes: &mut Option<WindowCache<Rc<str>>>,
        reads: &Cell<u32>,
    ) -> Vec<WinEventKind> {
        let class_of = |_| {
            reads.set(reads.get() + 1);
            "Edit".to_owned()
        };
        wanted_kind(
            (event, 42, id_object, CHILDID_SELF),
            APP_SUBSCRIPTIONS,
            classes,
            (|_| 7, class_of),
        )
        .into_iter()
        .collect()
    }

    #[test]
    fn a_windows_class_is_read_once_for_its_events() {
        // Value changes of child objects, whose window NVDA's Excel check
        // asks the class of.
        let mut classes = Some(WindowCache::default());
        let reads = Cell::new(0);
        for _ in 0..3 {
            assert_eq!(
                outpost_kinds(EVENT_OBJECT_VALUECHANGE, 5, &mut classes, &reads),
                [WinEventKind::ValueChange]
            );
        }
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn a_destroyed_windows_class_is_forgotten() {
        let mut classes = Some(WindowCache::default());
        let reads = Cell::new(0);
        outpost_kinds(EVENT_OBJECT_VALUECHANGE, 5, &mut classes, &reads);
        assert_eq!(
            outpost_kinds(EVENT_OBJECT_DESTROY, OBJID_WINDOW.0, &mut classes, &reads),
            [WinEventKind::Destroy]
        );
        outpost_kinds(EVENT_OBJECT_VALUECHANGE, 5, &mut classes, &reads);
        assert_eq!(reads.get(), 2, "read again once its window was destroyed");
    }

    #[test]
    fn only_a_plain_selection_announces_a_newly_selected_item() {
        assert_eq!(
            kind_among(EVENT_OBJECT_SELECTION, APP_SUBSCRIPTIONS),
            Some(WinEventKind::Selection)
        );
        for removed_or_added in [
            EVENT_OBJECT_SELECTIONADD,
            EVENT_OBJECT_SELECTIONREMOVE,
            EVENT_OBJECT_SELECTIONWITHIN,
        ] {
            assert_eq!(
                kind_among(removed_or_added, APP_SUBSCRIPTIONS),
                Some(WinEventKind::StateChange)
            );
        }
    }

    #[test]
    fn only_the_carets_location_changes_are_wanted() {
        let any = HWND::default();
        assert_eq!(
            kind_among(EVENT_OBJECT_LOCATIONCHANGE, APP_SUBSCRIPTIONS),
            Some(WinEventKind::Caret)
        );
        assert!(wanted(
            WinEventKind::Caret,
            any,
            OBJID_CARET.0,
            CHILDID_SELF
        ));
        assert!(!wanted(
            WinEventKind::Caret,
            any,
            OBJID_WINDOW.0,
            CHILDID_SELF
        ));
    }

    #[test]
    fn the_carets_show_is_a_caret_event_for_an_outpost_only() {
        let mut classes = Some(WindowCache::default());
        let reads = Cell::new(0);
        assert_eq!(
            outpost_kinds(EVENT_OBJECT_SHOW, OBJID_CARET.0, &mut classes, &reads),
            [WinEventKind::Caret]
        );
        assert_eq!(reads.get(), 0, "no class is read for the caret");
        // The listener hooks shows for tooltips, not the caret.
        assert_eq!(
            wanted_kind(
                (EVENT_OBJECT_SHOW, 42, OBJID_CARET.0, CHILDID_SELF),
                LISTENER_SUBSCRIPTIONS,
                &mut classes,
                (|_| 7, |_| "Edit".to_owned()),
            ),
            None
        );
    }

    #[test]
    fn a_show_is_wanted_only_from_a_tooltip_window() {
        // A window that is no tooltip: the null handle, whose class reads
        // as empty.
        assert!(!wanted(
            WinEventKind::Show,
            HWND::default(),
            OBJID_CLIENT.0,
            CHILDID_SELF
        ));
    }

    #[test]
    fn a_console_scroll_of_ten_columns_left_is_kept() {
        // `EVENT_CONSOLE_UPDATE_SCROLL` carries its horizontal distance in
        // the object id: ten columns to the left is -10, `OBJID_ALERT`.
        let mut classes = Some(WindowCache::default());
        let reads = Cell::new(0);
        assert_eq!(
            outpost_kinds(
                EVENT_CONSOLE_UPDATE_SCROLL,
                OBJID_ALERT.0,
                &mut classes,
                &reads
            ),
            [WinEventKind::ConsoleUpdate]
        );
    }

    #[test]
    fn events_nvda_ignores_are_filtered_out() {
        let any = HWND::default();
        assert!(!wanted(WinEventKind::NameChange, any, OBJID_ALERT.0, 0));
        assert!(!wanted(
            WinEventKind::Focus,
            any,
            OBJID_MENU.0,
            CHILDID_SELF
        ));
        assert!(!wanted(
            WinEventKind::Focus,
            any,
            OBJID_SYSMENU.0,
            CHILDID_SELF
        ));
        assert!(
            wanted(WinEventKind::Focus, any, OBJID_MENU.0, 3),
            "a menu item"
        );
        assert!(wanted(
            WinEventKind::Focus,
            any,
            OBJID_CLIENT.0,
            CHILDID_SELF
        ));
    }
}
