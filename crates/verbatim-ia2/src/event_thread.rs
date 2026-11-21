//! The thread that installs MSAA `WinEvent` hooks and pumps messages so their
//! out-of-context callbacks are delivered. Both the console host's outpost
//! and the focus listener use it, differing only in their pid and
//! subscription set. The thread starts with no hook, and installs its one
//! set when its owner asks ([`EventThread::hook`]): a spare outpost starts
//! it before it has an application (`books/dev/src/reference/adr/0001-focus.md`, "Spare
//! outposts"). Its owner can also nudge it ([`Nudger`]): a message the
//! thread hands to the owner's nudge handler between events, on the thread
//! that owns the hooks, which is how an outpost unhooks some of its kinds
//! and hooks them again while it runs. And, with the `sync-marker` feature,
//! which only tests turn on, it can wait for the thread to catch up
//! (`Nudger::sync`): for every nudge sent and, once the hooks are in, every
//! `WinEvent` raised before it. Its owner's handlers may set
//! one timer of the thread's own ([`ThreadTimer`]), whose `WM_TIMER` the
//! thread hands to the owner's timer handler, as the focus listener's
//! foreground hold ends.

use std::cell::Cell;
use std::sync::Arc;
#[cfg(feature = "sync-marker")]
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SetTimer, TranslateMessage, WM_APP, WM_QUIT, WM_TIMER,
};

#[cfg(feature = "sync-marker")]
use crate::SyncMarker;
use crate::{WinEventCallback, WinEventHook, WinEventKind};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};

/// The thread message that carries a nudge's two words to the nudge
/// handler.
const WM_NUDGE: u32 = WM_APP;
/// The thread message that marks every message posted before it handled
/// ([`Nudger::sync`]), its ticket in its first word. It shows nothing
/// about `WinEvent`s, which a posted message can overtake.
#[cfg(feature = "sync-marker")]
const WM_SYNC: u32 = WM_APP + 1;
/// The thread message that installs the hooks ([`EventThread::hook`]), the
/// process id they are scoped to in its first word.
const WM_HOOK: u32 = WM_APP + 2;

/// What the event thread calls: the `WinEvent` callback its hooks deliver
/// to, for an owner that nudges it, the handler of each nudge's two
/// words, and for an owner that sets the thread's timer, the handler of
/// its `WM_TIMER`. All are made on the event thread, so they may share
/// state that only it touches.
pub struct EventHandlers {
    /// Called with each `WinEvent` the hooks deliver.
    pub win_event: WinEventCallback,
    /// Called with each nudge's two words ([`Nudger::nudge`]), for an
    /// owner that nudges the thread.
    pub nudge: Option<Box<dyn Fn(usize, isize)>>,
    /// Called for each `WM_TIMER` of the thread's own timer
    /// ([`ThreadTimer`]), a stale one that `KillTimer` left posted
    /// included, for an owner that sets it.
    pub timer: Option<Box<dyn Fn()>>,
}

/// The event thread's one timer, a thread timer (`SetTimer` with no
/// window), set and stopped only on the event thread, where its
/// `WM_TIMER` is handed to the owner's timer handler
/// ([`EventHandlers::timer`]). A thread timer goes off again and again
/// until it is stopped, so its owner stops it when no wait needs it;
/// stopping it leaves a `WM_TIMER` already posted, which the handler
/// meets as a stale one. Made on the event thread and never sent from it.
#[derive(Debug, Default)]
pub struct ThreadTimer {
    /// The timer's id while it is set, 0 when it is not.
    id: Cell<usize>,
}

impl ThreadTimer {
    /// Sets the timer to go off `after` from now, in place of any time it
    /// was set for. Windows takes at least 10 ms, and goes off at the
    /// system timer's resolution.
    pub fn arm(&self, after: Duration) {
        let ms = u32::try_from(after.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: a thread timer with no callback, its WM_TIMER posted to
        // this thread's queue; an id this thread set before is replaced.
        let id = unsafe { SetTimer(None, self.id.get(), ms, None) };
        if id == 0 {
            tracing::warn!("the event thread's timer could not be set");
        }
        self.id.set(id);
    }

    /// Stops the timer, when it is set.
    pub fn disarm(&self) {
        let id = self.id.replace(0);
        if id != 0 {
            // SAFETY: a thread timer this thread set.
            let _ = unsafe { KillTimer(None, id) };
        }
    }
}

/// Makes an [`EventHandlers`], on the event thread.
pub type MakeHandlers = Arc<dyn Fn() -> EventHandlers + Send + Sync>;

/// The `WinEvent` thread's priority: it only moves each event on, for
/// microseconds per wake (`docs/design/communication.md`, section 6.1).
/// The event recorder's `WinEvent` thread runs at it too.
pub const EVENT_THREAD_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// The event thread: pumps messages so the out-of-context callbacks of its
/// MSAA hooks are delivered, and installs the requested hooks once, when
/// asked ([`EventThread::hook`]), scoped to the given pid, or global for pid
/// zero. Both the console host's outpost and the focus listener use it,
/// differing only in their pid and subscription set.
pub struct EventThread {
    nudger: Nudger,
    join: Option<JoinHandle<()>>,
}

impl EventThread {
    /// Starts the thread, with its message queue and handlers and no hook.
    ///
    /// # Panics
    ///
    /// When the thread cannot be spawned.
    #[must_use]
    pub fn spawn(kinds: &'static [WinEventKind], make_handlers: MakeHandlers) -> Self {
        let (id_tx, id_rx) = unbounded::<u32>();
        let (hooked_tx, hooked) = unbounded();
        #[cfg(feature = "sync-marker")]
        let thread_syncs = Syncs::default();
        #[cfg(not(feature = "sync-marker"))]
        let thread_syncs = Syncs;
        #[cfg(feature = "sync-marker")]
        let syncs = thread_syncs.clone();
        let join = thread::Builder::new()
            .name("verbatim-event".to_owned())
            .spawn(move || {
                event_thread_main(kinds, (&id_tx, &hooked_tx), &make_handlers, &thread_syncs);
            })
            .expect("spawn event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            nudger: Nudger {
                thread_id,
                #[cfg(feature = "sync-marker")]
                syncs,
                hooked,
            },
            join: Some(join),
        }
    }

    /// Installs the thread's hooks ([`Nudger::hook`]).
    ///
    /// # Errors
    ///
    /// As [`Nudger::hook`].
    pub fn hook(&self, target_pid: u32) -> Result<(), String> {
        self.nudger.hook(target_pid)
    }

    /// What nudges this thread, for any thread to keep.
    #[must_use]
    pub fn nudger(&self) -> Nudger {
        self.nudger.clone()
    }
}

/// Nudges an event thread from any thread: each nudge reaches its owner's
/// nudge handler on the event thread, in the order nudges were sent.
#[derive(Clone)]
pub struct Nudger {
    thread_id: u32,
    /// What the syncs share with the thread.
    #[cfg(feature = "sync-marker")]
    syncs: Syncs,
    /// The outcome of installing the hooks, sent by the thread.
    hooked: Receiver<Result<(), String>>,
}

/// What [`Nudger::sync`] shares with the event thread.
#[cfg(feature = "sync-marker")]
#[derive(Clone, Default)]
struct Syncs {
    /// How far the thread has come through the syncs.
    synced: Arc<Synced>,
    /// The tickets handed out.
    tickets: Arc<Mutex<Tickets>>,
}

/// Nothing: without the `sync-marker` feature there are no syncs.
#[cfg(not(feature = "sync-marker"))]
#[derive(Clone, Default)]
struct Syncs;

/// The syncs handed out ([`Nudger::sync`]).
#[cfg(feature = "sync-marker")]
#[derive(Default)]
struct Tickets {
    /// The last ticket handed out.
    last: u64,
    /// What raises the thread's sync markers, from when its hooks are in
    /// until it ends. Read under the same lock as the ticket is taken, so
    /// a sync racing the hooks' install either raises a marker the hook
    /// will deliver, or raises none and waits for none.
    marker: Option<SyncMarker>,
}

/// How far the event thread has come through the syncs: the last ticket
/// whose message it has handled, and the last whose marker has reached
/// it, both `u64::MAX` once it has ended.
#[cfg(feature = "sync-marker")]
#[derive(Default)]
struct Synced {
    reached: Mutex<Reached>,
    changed: Condvar,
}

/// The tickets [`Synced`] keeps.
#[cfg(feature = "sync-marker")]
#[derive(Default)]
struct Reached {
    posted: u64,
    marked: u64,
}

#[cfg(feature = "sync-marker")]
impl Reached {
    /// Whether the sync with `ticket` may return: the thread has handled
    /// its message and, when the sync raised a marker (`marked`), the
    /// marker has reached the thread. A later ticket stands for an earlier
    /// one, since a later sync took its ticket after the earlier one's
    /// call began.
    fn passed(&self, ticket: u64, marked: bool) -> bool {
        self.posted >= ticket && (!marked || self.marked >= ticket)
    }
}

#[cfg(feature = "sync-marker")]
impl Synced {
    /// The thread has handled the message of the sync with `ticket`.
    fn reach(&self, ticket: u64) {
        self.update(|reached| reached.posted = reached.posted.max(ticket));
    }

    /// The marker of the sync with `ticket` has reached the thread.
    fn mark(&self, ticket: u64) {
        self.update(|reached| reached.marked = reached.marked.max(ticket));
    }

    /// The thread has ended: no sync waits on it.
    fn end(&self) {
        self.update(|reached| {
            reached.posted = u64::MAX;
            reached.marked = u64::MAX;
        });
    }

    fn update(&self, change: impl FnOnce(&mut Reached)) {
        let mut reached = self.reached.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut reached);
        self.changed.notify_all();
    }

    /// Waits until the sync with `ticket` may return.
    fn wait(&self, ticket: u64, marked: bool) {
        let mut reached = self.reached.lock().unwrap_or_else(PoisonError::into_inner);
        while !reached.passed(ticket, marked) {
            reached = self
                .changed
                .wait(reached)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Nudger {
    /// Installs the thread's hooks, scoped to `target_pid` (every process
    /// for zero), on the thread, behind every nudge sent before, and
    /// returns once they are in, so a caller that reports itself ready
    /// after it really sees the events from then on. A thread has one set
    /// of hooks for its whole life (decision D9), which includes the hook
    /// for this process's sync markers with the `sync-marker` feature
    /// (`Nudger::sync`).
    ///
    /// # Errors
    ///
    /// Returns why the hooks are not in: a hook failed to install, the
    /// thread already has its hooks, or the thread has ended.
    pub fn hook(&self, target_pid: u32) -> Result<(), String> {
        let word = usize::try_from(target_pid).map_err(|error| error.to_string())?;
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended.
        unsafe { PostThreadMessageW(self.thread_id, WM_HOOK, WPARAM(word), LPARAM(0)) }
            .map_err(|error| format!("the event thread could not be asked to hook: {error}"))?;
        self.hooked
            .recv()
            .map_err(|_| "the event thread ended before it hooked".to_owned())?
    }

    /// Hands the two words to the event thread's nudge handler, behind
    /// every event already delivered to the thread. Returns at once, saying
    /// whether the thread took the nudge: one that has ended takes none.
    #[expect(
        clippy::must_use_candidate,
        reason = "a caller that need not know whether the thread took the nudge ignores the answer"
    )]
    pub fn nudge(&self, (word, hwnd): (usize, isize)) -> bool {
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended.
        unsafe { PostThreadMessageW(self.thread_id, WM_NUDGE, WPARAM(word), LPARAM(hwnd)) }.is_ok()
    }

    /// Returns once the event thread has handled every nudge sent before
    /// this call and, once its hooks are in, every `WinEvent` its hooks
    /// take that was raised before this call, or has ended.
    ///
    /// Nudges are thread messages, so a thread message posted behind them
    /// (`WM_SYNC`) shows they are handled. It shows nothing about
    /// `WinEvent`s: `NotifyWinEvent` can return before an out-of-context
    /// event reaches the hook thread, and a message posted after it has
    /// been handled first. So once the hooks are in, the sync also raises a
    /// sync marker, a `WinEvent` of this process's that the thread's own
    /// hook set hears (`crate::SyncMarker`), and waits for it too:
    /// a `WinEvent` arrives behind every one raised before it. This costs
    /// one `NotifyWinEvent` and up to one more wake of the thread.
    #[cfg(feature = "sync-marker")]
    pub fn sync(&self) {
        let Syncs { synced, tickets } = &self.syncs;
        let (ticket, marker) = {
            let mut tickets = tickets.lock().unwrap_or_else(PoisonError::into_inner);
            tickets.last += 1;
            (tickets.last, tickets.marker)
        };
        let Ok(word) = usize::try_from(ticket) else {
            return;
        };
        // SAFETY: as in `nudge`.
        if unsafe { PostThreadMessageW(self.thread_id, WM_SYNC, WPARAM(word), LPARAM(0)) }.is_err()
        {
            return;
        }
        if let Some(marker) = marker {
            marker.raise(ticket);
        }
        synced.wait(ticket, marker.is_some());
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended or has no queue.
        let posted =
            unsafe { PostThreadMessageW(self.nudger.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let Some(join) = self.join.take() else {
            return;
        };
        // The thread makes its message queue before it reports its id, so a
        // failed post means it has already ended, and joining it returns at
        // once. Should it still be running, nothing would end its loop, so
        // it is left to end with the process rather than waited on forever.
        if posted.is_ok() || join.is_finished() {
            let _ = join.join();
        } else {
            tracing::warn!("the event thread could not be told to stop; it is not waited on");
        }
    }
}

fn event_thread_main(
    kinds: &'static [WinEventKind],
    (id_tx, hooked_tx): (&Sender<u32>, &Sender<Result<(), String>>),
    make_handlers: &MakeHandlers,
    syncs: &Syncs,
) {
    prioritize_role("WinEvent", EVENT_THREAD_PRIORITY);
    // SAFETY: GetCurrentThreadId is always sound.
    let thread_id = unsafe { GetCurrentThreadId() };
    // Makes this thread's message queue now, before its id is reported, so
    // the WM_QUIT that `Drop` posts always has a queue to land in.
    let mut message = MSG::default();
    // SAFETY: `message` is owned here; the call only peeks, removing
    // nothing.
    let _ = unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE) };
    let EventHandlers {
        win_event,
        nudge,
        timer,
    } = make_handlers();
    // The callback, until the hooks are installed with it.
    let mut win_event = Some(win_event);
    let mut hook = None;
    let _ = id_tx.send(thread_id);
    loop {
        // SAFETY: standard message loop; `message` is fully owned here.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break; // WM_QUIT (0) or error (-1).
        }
        // Thread messages have no window, and are this thread's nudges.
        if message.hwnd.is_invalid() {
            match message.message {
                WM_NUDGE => {
                    if let Some(nudge) = &nudge {
                        nudge(message.wParam.0, message.lParam.0);
                    }
                    continue;
                }
                #[cfg(feature = "sync-marker")]
                WM_SYNC => {
                    syncs.synced.reach(message.wParam.0 as u64);
                    continue;
                }
                WM_TIMER => {
                    if let Some(timer) = &timer {
                        timer();
                    }
                    continue;
                }
                WM_HOOK => {
                    // Installed once, for the whole life of the process
                    // (decision D9): a second live hook set on the same
                    // thread while a first is still registered has been
                    // observed to permanently kill WinEvent delivery on
                    // that thread for the rest of the process, which is
                    // why the application is set once, at the outpost's
                    // start or at its assignment, and never rebound.
                    // Unhooking some of the set's kinds and hooking them
                    // again (`crate::suspend_kinds`) changes one raw
                    // event's hook at a time within this one set, as the
                    // focus listener hooks and unhooks the console's
                    // events.
                    let target_pid = u32::try_from(message.wParam.0).unwrap_or(0);
                    let outcome = match win_event.take() {
                        Some(callback) => install(target_pid, kinds, callback, syncs)
                            .map(|installed| hook = Some(installed)),
                        None => Err("this thread already has its WinEvent hooks".to_owned()),
                    };
                    let _ = hooked_tx.send(outcome);
                    continue;
                }
                _ => {}
            }
        }
        // SAFETY: translating a fully owned message.
        let _ = unsafe { TranslateMessage(&raw const message) };
        // SAFETY: dispatching a fully owned message.
        unsafe { DispatchMessageW(&raw const message) };
    }
    ending(hook, syncs);
}

/// Installs the thread's hooks, scoped to `target_pid`, with the hook for
/// this process's sync markers as one more of the set.
#[cfg(feature = "sync-marker")]
fn install(
    target_pid: u32,
    kinds: &[WinEventKind],
    callback: WinEventCallback,
    Syncs { synced, tickets }: &Syncs,
) -> Result<WinEventHook, String> {
    let reached = Arc::clone(synced);
    let (installed, marker) = WinEventHook::install_with_sync_marker(
        target_pid,
        kinds,
        callback,
        Box::new(move |ticket| reached.mark(ticket)),
    )?;
    tickets
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .marker = Some(marker);
    Ok(installed)
}

/// Installs the thread's hooks, scoped to `target_pid`.
#[cfg(not(feature = "sync-marker"))]
fn install(
    target_pid: u32,
    kinds: &[WinEventKind],
    callback: WinEventCallback,
    Syncs: &Syncs,
) -> Result<WinEventHook, String> {
    WinEventHook::install(target_pid, kinds, callback)
}

/// Removes the thread's hooks as it ends: no sync raises a marker once
/// they are going, and none waits on the thread.
#[cfg(feature = "sync-marker")]
fn ending(hook: Option<WinEventHook>, Syncs { synced, tickets }: &Syncs) {
    tickets
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .marker = None;
    drop(hook);
    synced.end();
}

/// Removes the thread's hooks as it ends.
#[cfg(not(feature = "sync-marker"))]
fn ending(hook: Option<WinEventHook>, Syncs: &Syncs) {
    drop(hook);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[cfg(feature = "sync-marker")]
    use windows::Win32::UI::Accessibility::NotifyWinEvent;
    #[cfg(feature = "sync-marker")]
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_VALUECHANGE,
        OBJID_CLIENT, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    #[cfg(feature = "sync-marker")]
    use windows::core::w;

    /// The kinds the test's event thread hooks.
    const KINDS: &[WinEventKind] = &[WinEventKind::NameChange, WinEventKind::ValueChange];

    /// How long a wait for the event thread may take before the test is
    /// taken to hang.
    const WAIT: Duration = Duration::from_secs(10);

    /// An event thread hooking this process's name and value changes, its
    /// nudge `1` suspending the name changes and `0` hooking every kind
    /// again: a change of its hooks while it runs, as an outpost makes
    /// them. Each event the calling thread raised is sent on `events` as
    /// it is delivered, and the outcome of each change on `changes`.
    #[cfg(feature = "sync-marker")]
    fn event_thread(
        events: Sender<WinEventKind>,
        changes: Sender<Result<(), String>>,
    ) -> EventThread {
        // Only the calling test's events: the hooks see every thread of
        // this process, and the tests run at once.
        // SAFETY: GetCurrentThreadId is always sound.
        let test_thread = unsafe { GetCurrentThreadId() };
        let make: MakeHandlers = Arc::new(move || {
            let events = events.clone();
            let changes = changes.clone();
            EventHandlers {
                win_event: Box::new(move |event, _class| {
                    if event.thread == test_thread {
                        let _ = events.send(event.kind);
                    }
                }),
                nudge: Some(Box::new(move |word, _| {
                    let suspended: &[WinEventKind] = if word == 1 {
                        &[WinEventKind::NameChange]
                    } else {
                        &[]
                    };
                    let _ = changes.send(crate::suspend_kinds(suspended));
                })),
                timer: None,
            }
        });
        let thread = EventThread::spawn(KINDS, make);
        thread
            .hook(std::process::id())
            .expect("the hooks are installed");
        thread
    }

    /// A hidden static control of this thread's, with no parent, to raise
    /// events from; the caller destroys it on the same thread.
    #[cfg(feature = "sync-marker")]
    fn window() -> windows::Win32::Foundation::HWND {
        // SAFETY: a window with no parent, made on this thread.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a window to raise events from")
    }

    /// A `WinEvent` raised before a sync is delivered before the sync
    /// returns, though the sync's thread message may be handled before the
    /// event reaches the thread: the sync waits for its marker too, which
    /// arrives behind the event. A check behind a sync relies on this to
    /// see every event the application raised before it.
    #[cfg(feature = "sync-marker")]
    #[test]
    fn an_event_raised_before_a_sync_is_delivered_before_it_returns() {
        let (events_tx, events) = unbounded();
        let (changes_tx, _changes) = unbounded();
        let thread = event_thread(events_tx, changes_tx);
        let window = window();
        // SAFETY: raising an event for a live window of this thread.
        unsafe { NotifyWinEvent(EVENT_OBJECT_VALUECHANGE, window, OBJID_CLIENT.0, 0) };
        thread.nudger().sync();
        assert_eq!(events.try_recv(), Ok(WinEventKind::ValueChange));
        drop(thread);
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }

    /// The thread's timer, set by a nudge's handler, goes off on the event
    /// thread and is handed to the timer handler, which stops it.
    #[test]
    fn the_threads_timer_goes_off_to_the_timer_handler() {
        let (fired_tx, fired) = unbounded();
        let make: MakeHandlers = Arc::new(move || {
            let timer = std::rc::Rc::new(ThreadTimer::default());
            let armed = std::rc::Rc::clone(&timer);
            let fired_tx = fired_tx.clone();
            EventHandlers {
                win_event: Box::new(|_, _| {}),
                nudge: Some(Box::new(move |_, _| armed.arm(Duration::from_millis(10)))),
                timer: Some(Box::new(move || {
                    timer.disarm();
                    let _ = fired_tx.send(());
                })),
            }
        });
        let thread = EventThread::spawn(KINDS, make);
        assert!(thread.nudger().nudge((0, 0)));
        assert_eq!(fired.recv_timeout(WAIT), Ok(()));
    }

    /// The last ticket whose marker has reached `thread`.
    #[cfg(feature = "sync-marker")]
    fn marked(thread: &EventThread) -> u64 {
        thread
            .nudger
            .syncs
            .synced
            .reached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .marked
    }

    /// Once the hooks are in, a sync returns only when its own marker has
    /// reached the thread: each sync leaves the thread's marked ticket at
    /// its own.
    #[cfg(feature = "sync-marker")]
    #[test]
    fn a_sync_returns_once_its_marker_has_reached_the_thread() {
        let (events_tx, _events) = unbounded();
        let (changes_tx, _changes) = unbounded();
        let thread = event_thread(events_tx, changes_tx);
        let nudger = thread.nudger();
        nudger.sync();
        assert_eq!(marked(&thread), 1);
        nudger.sync();
        assert_eq!(marked(&thread), 2);
    }

    /// A hook set takes only the markers raised on its own window, though
    /// its marker hook hears the whole process: another event thread's
    /// sync leaves this thread's marked ticket alone. The value change is
    /// raised after the other thread's marker, so it arrives behind it.
    #[cfg(feature = "sync-marker")]
    #[test]
    fn a_sync_of_another_event_thread_marks_nothing_here() {
        let (events_tx, events) = unbounded();
        let (changes_tx, _changes) = unbounded();
        let watching = event_thread(events_tx, changes_tx);
        let (other_events_tx, _other_events) = unbounded();
        let (other_changes_tx, _other_changes) = unbounded();
        let syncing = event_thread(other_events_tx, other_changes_tx);
        let window = window();
        syncing.nudger().sync();
        // SAFETY: raising an event for a live window of this thread.
        unsafe { NotifyWinEvent(EVENT_OBJECT_VALUECHANGE, window, OBJID_CLIENT.0, 0) };
        assert_eq!(events.recv_timeout(WAIT), Ok(WinEventKind::ValueChange));
        assert_eq!(marked(&watching), 0);
        drop(syncing);
        drop(watching);
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }

    /// A sync that raised a marker may not return once its thread message
    /// is handled, until its marker has reached the thread too; one that
    /// raised none returns on its message.
    #[cfg(feature = "sync-marker")]
    #[test]
    fn a_sync_that_raised_a_marker_waits_for_the_marker_as_well() {
        let synced = Synced::default();
        let passed = |marked| synced.reached.lock().unwrap().passed(1, marked);
        synced.reach(1);
        assert!(!passed(true));
        assert!(passed(false));
        synced.mark(1);
        assert!(passed(true));
    }

    #[cfg(feature = "sync-marker")]
    #[test]
    fn kinds_are_unhooked_and_hooked_again_while_the_thread_runs() {
        let (events_tx, events) = unbounded();
        let (changes_tx, changes) = unbounded();
        let thread = event_thread(events_tx, changes_tx);
        let window = window();
        // A name change, then a value change, from the window's client
        // area: delivered in the order raised.
        let raise_both = || {
            for event in [EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_VALUECHANGE] {
                // SAFETY: raising an event for a live window of this thread.
                unsafe { NotifyWinEvent(event, window, OBJID_CLIENT.0, 0) };
            }
        };
        let next = || events.recv_timeout(WAIT).expect("an event before the wait");
        let change = |word: usize| {
            let nudger = thread.nudger();
            nudger.nudge((word, 0));
            nudger.sync();
            assert_eq!(changes.try_recv(), Ok(Ok(())));
        };

        raise_both();
        assert_eq!(
            [next(), next()],
            [WinEventKind::NameChange, WinEventKind::ValueChange]
        );
        change(1);
        raise_both();
        // The value change comes first: the name change raised before it
        // was never delivered.
        assert_eq!(next(), WinEventKind::ValueChange);
        change(0);
        raise_both();
        assert_eq!(
            [next(), next()],
            [WinEventKind::NameChange, WinEventKind::ValueChange]
        );
        drop(thread);
        assert_eq!(
            events.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        );
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }
}
