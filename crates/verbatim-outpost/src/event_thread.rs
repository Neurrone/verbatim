//! The thread that installs MSAA `WinEvent` hooks and pumps messages so their
//! out-of-context callbacks are delivered. Both a per-application outpost and
//! the focus listener (decision D13) use it, differing only in their pid and
//! subscription set. The thread starts with no hook, and installs its one
//! set when its owner asks ([`EventThread::hook`]): a spare outpost starts
//! it before it has an application (`docs/design/focus-pipeline.md`,
//! section 19). Its owner can also nudge it ([`Nudger`]): a message the
//! thread hands to the owner's nudge handler between events, on the thread
//! that owns the hooks, which is how an outpost unhooks some of its kinds
//! and hooks them again while it runs.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, unbounded};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    TranslateMessage, WM_APP, WM_QUIT,
};

use verbatim_ia2::{WinEventCallback, WinEventHook, WinEventKind};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};

/// The thread message that carries a nudge's two words to the nudge
/// handler.
const WM_NUDGE: u32 = WM_APP;
/// The thread message that marks every message posted before it handled
/// ([`Nudger::sync`]), its ticket in its first word.
const WM_SYNC: u32 = WM_APP + 1;
/// The thread message that installs the hooks ([`EventThread::hook`]), the
/// process id they are scoped to in its first word.
const WM_HOOK: u32 = WM_APP + 2;

/// What the event thread calls: the `WinEvent` callback its hooks deliver
/// to, and, for an owner that nudges it, the handler of each nudge's two
/// words. Both are made on the event thread, so they may share state that
/// only it touches.
pub(crate) struct EventHandlers {
    pub(crate) win_event: WinEventCallback,
    pub(crate) nudge: Option<Box<dyn Fn(usize, isize)>>,
}

/// Makes an [`EventHandlers`], on the event thread.
pub(crate) type MakeHandlers = Arc<dyn Fn() -> EventHandlers + Send + Sync>;

/// The `WinEvent` thread's priority: it only moves each event on, for
/// microseconds per wake (`docs/design/communication.md`, section 6.1).
const EVENT_THREAD_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// The event thread: pumps messages so the out-of-context callbacks of its
/// MSAA hooks are delivered, and installs the requested hooks once, when
/// asked ([`EventThread::hook`]), scoped to the given pid, or global for pid
/// zero. Both a per-application outpost and the focus listener (decision
/// D13) use it, differing only in their pid and subscription set.
pub(crate) struct EventThread {
    nudger: Nudger,
    /// The outcome of installing the hooks, sent by the thread.
    hooked: Receiver<Result<(), String>>,
    join: Option<JoinHandle<()>>,
}

impl EventThread {
    /// Starts the thread, with its message queue and handlers and no hook.
    pub(crate) fn spawn(kinds: &'static [WinEventKind], make_handlers: MakeHandlers) -> Self {
        let (id_tx, id_rx) = unbounded::<u32>();
        let (hooked_tx, hooked) = unbounded();
        let synced = Arc::new(Synced::default());
        let thread_synced = Arc::clone(&synced);
        let join = thread::Builder::new()
            .name("verbatim-event".to_owned())
            .spawn(move || {
                event_thread_main(kinds, (&id_tx, &hooked_tx), &make_handlers, &thread_synced);
            })
            .expect("spawn event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            nudger: Nudger {
                thread_id,
                synced,
                tickets: Arc::default(),
            },
            hooked,
            join: Some(join),
        }
    }

    /// Installs the thread's hooks, scoped to `target_pid` (every process
    /// for zero), on the thread, behind every nudge sent before, and
    /// returns once they are in, so a caller that reports itself ready
    /// after it really sees the events from then on. A thread has one set
    /// of hooks for its whole life (decision D9).
    ///
    /// # Errors
    ///
    /// Returns why the hooks are not in: a hook failed to install, the
    /// thread already has its hooks, or the thread has ended.
    pub(crate) fn hook(&self, target_pid: u32) -> Result<(), String> {
        let word = usize::try_from(target_pid).map_err(|error| error.to_string())?;
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended.
        unsafe { PostThreadMessageW(self.nudger.thread_id, WM_HOOK, WPARAM(word), LPARAM(0)) }
            .map_err(|error| format!("the event thread could not be asked to hook: {error}"))?;
        self.hooked
            .recv()
            .map_err(|_| "the event thread ended before it hooked".to_owned())?
    }

    /// What nudges this thread, for any thread to keep.
    pub(crate) fn nudger(&self) -> Nudger {
        self.nudger.clone()
    }
}

/// Nudges an event thread from any thread: each nudge reaches its owner's
/// nudge handler on the event thread, in the order nudges were sent.
#[derive(Clone)]
pub(crate) struct Nudger {
    thread_id: u32,
    synced: Arc<Synced>,
    /// The last sync ticket handed out.
    tickets: Arc<Mutex<u64>>,
}

/// The last sync ticket the event thread has reached, `u64::MAX` once it
/// has ended.
#[derive(Default)]
struct Synced {
    reached: Mutex<u64>,
    changed: Condvar,
}

impl Synced {
    fn reach(&self, ticket: u64) {
        let mut reached = self.reached.lock().unwrap_or_else(PoisonError::into_inner);
        *reached = (*reached).max(ticket);
        self.changed.notify_all();
    }
}

impl Nudger {
    /// Hands the two words to the event thread's nudge handler, behind
    /// every event already delivered to the thread. Returns at once, saying
    /// whether the thread took the nudge: one that has ended takes none.
    pub(crate) fn nudge(&self, (word, hwnd): (usize, isize)) -> bool {
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended.
        unsafe { PostThreadMessageW(self.thread_id, WM_NUDGE, WPARAM(word), LPARAM(hwnd)) }.is_ok()
    }

    /// Returns once the event thread has handled every nudge sent before
    /// this call, or has ended.
    pub(crate) fn sync(&self) {
        let ticket = {
            let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
            *tickets += 1;
            *tickets
        };
        let Ok(word) = usize::try_from(ticket) else {
            return;
        };
        // SAFETY: as in `nudge`.
        if unsafe { PostThreadMessageW(self.thread_id, WM_SYNC, WPARAM(word), LPARAM(0)) }.is_err()
        {
            return;
        }
        let mut reached = self
            .synced
            .reached
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while *reached < ticket {
            reached = self
                .synced
                .changed
                .wait(reached)
                .unwrap_or_else(PoisonError::into_inner);
        }
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
    synced: &Synced,
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
    let EventHandlers { win_event, nudge } = make_handlers();
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
                WM_SYNC => {
                    synced.reach(message.wParam.0 as u64);
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
                    // again (`verbatim_ia2::suspend_kinds`) changes one raw
                    // event's hook at a time within this one set, as the
                    // keyboard hook thread hooks and unhooks the console's
                    // events.
                    let target_pid = u32::try_from(message.wParam.0).unwrap_or(0);
                    let outcome = match win_event.take() {
                        Some(callback) => WinEventHook::install(target_pid, kinds, callback)
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
    drop(hook);
    // No sync waits on a thread that has ended.
    synced.reach(u64::MAX);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows::Win32::UI::Accessibility::NotifyWinEvent;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_VALUECHANGE,
        OBJID_CLIENT, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    /// The kinds the test's event thread hooks.
    const KINDS: &[WinEventKind] = &[WinEventKind::NameChange, WinEventKind::ValueChange];

    /// How long a wait for the event thread may take before the test is
    /// taken to hang.
    const WAIT: Duration = Duration::from_secs(10);

    /// An event thread hooking this process's name and value changes, its
    /// nudge `1` suspending the name changes and `0` hooking every kind
    /// again: a change of its hooks while it runs, as an outpost makes
    /// them. Each event delivered is sent on `events`, and the outcome of
    /// each change on `changes`.
    fn event_thread(
        events: Sender<WinEventKind>,
        changes: Sender<Result<(), String>>,
    ) -> EventThread {
        let make: MakeHandlers = Arc::new(move || {
            let events = events.clone();
            let changes = changes.clone();
            EventHandlers {
                win_event: Box::new(move |event| {
                    let _ = events.send(event.kind);
                }),
                nudge: Some(Box::new(move |word, _| {
                    let suspended: &[WinEventKind] = if word == 1 {
                        &[WinEventKind::NameChange]
                    } else {
                        &[]
                    };
                    let _ = changes.send(verbatim_ia2::suspend_kinds(suspended));
                })),
            }
        });
        let thread = EventThread::spawn(KINDS, make);
        thread
            .hook(std::process::id())
            .expect("the hooks are installed");
        thread
    }

    #[test]
    fn kinds_are_unhooked_and_hooked_again_while_the_thread_runs() {
        let (events_tx, events) = unbounded();
        let (changes_tx, changes) = unbounded();
        let thread = event_thread(events_tx, changes_tx);
        // SAFETY: a hidden static control of this thread's, with no
        // parent, destroyed below on the same thread.
        let window = unsafe {
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
        .expect("a window to raise events from");
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
