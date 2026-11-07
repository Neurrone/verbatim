//! The focus-listener runtime (architecture section 1, "The focus listener
//! (D13)"; decisions D13 and D14).
//!
//! One permanent, stateless listener process holds the subscriptions that are
//! global by nature and forwards each captured fact to Core, which routes it
//! to the target application's own outpost:
//!
//! - MSAA focus, foreground, menu-popup, alert, and tooltip events for
//!   every process whose outpost's own hook is not yet in, and the end of a
//!   menu or of the Alt+Tab switcher, which go to Core rather than to an
//!   application's outpost;
//! - the desktop-wide UIA focus subscription, each event judged and read
//!   from its own element on a thread for its application
//!   ([`crate::focus_read`]);
//! - desktop-wide UIA subscriptions for some of the events NVDA registers
//!   globally on Windows 11: an element selected, a menu opened, and
//!   notifications, each taken, as a focus event is, through its
//!   application's reading thread behind a marker, the menu opening judged
//!   and read as a focus, a selection and a notification checked by their
//!   sender's nearest window (`FocusReader::check_fact`), so all keep the
//!   order they came in. NVDA's global group also takes live regions, selection
//!   added and removed, range values, windows and tool tips opened, system
//!   alerts, layout invalidation, drag and drop, and the active text
//!   position, which Verbatim does not yet subscribe to (`docs/parity/backends.md`,
//!   "UIA event registration").
//!
//! The listener's callbacks, its writer, and its registering thread never
//! make a cross-process call (decision D13, as amended on 2026-10-11),
//! except the registering thread's registration calls themselves, which
//! reach every application's provider and which UIA's timeouts bound. A UIA
//! callback delivers the element with its properties already cached, so
//! building a fact is local memory reads (plus `GetRuntimeId`, a local read
//! on a cached element); an MSAA `WinEvent` delivers a raw window and object
//! address, forwarded untouched; the only other reads are hang-safe local
//! ones: `GetWindowThreadProcessId`, which names the owning process;
//! `GetProcessIdOfThread` on the event's own thread, which names a console
//! window's console host ([`routed_process`]); and, for a UIA focus on an
//! element with no window of its own, `GetGUIThreadInfo`, which names the
//! keyboard focus window; and, for the window in front, its class and its
//! process's integrity level (`OpenProcess`, `OpenProcessToken`,
//! `GetTokenInformation`, kernel calls that wait on no process); and, for a
//! UIA event's sender, whether its cached window is hung or ghosted
//! (`IsWindow`, `GhostWindowFromHungWindow`, `IsHungAppWindow`), which
//! drops the event as it arrives, with no marker. A foreground
//! event's fact is forwarded unchecked; the outpost's reducer thread tests
//! that its window is the foreground as it takes it in, as NVDA's
//! `processForegroundWinEvent` does. Core is told every foreground event
//! whose window `GetForegroundWindow` names, with `GetAncestor` for its root
//! owner, both local calls, as the only report that a window came to the
//! foreground ([`ListenerToCore::Foreground`]; `verbatim_listener_logic::foreground`).
//!
//! The one place the listener calls into applications is the reading
//! threads, one per application, which judge and read each UIA focus from
//! the event's own element, as NVDA does, since no other process can hold
//! that element (`books/dev/src/explanation/design/focus-pipeline.md`, section 12). The callback
//! sends the application's outpost a marker at once and queues the element
//! for its application's thread; the thread's read, or its drop, follows
//! the marker by the same route. Each read is a few calls on elements the
//! listener holds, each of which waits for the application up to UIA's
//! call timeout, and a watchdog abandons a thread whose read runs past its
//! deadline, or past half a second once the user has moved on to a window
//! of the same application on another UI thread, which it judges at the
//! half second and every half second after, so a slow or hung application
//! holds only its own thread, and a focus in an application that hangs
//! while it is read is spoken when it answers. One exposure is UIA's own and cannot be contained here: a
//! hung window named in a focus `WinEvent` holds UIA's delivery of every
//! focus event to the listener until it answers, as it holds NVDA's
//! (`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a
//! process's UIA events").
//!
//! The hand-over of MSAA events (`books/dev/src/explanation/design/focus-pipeline.md`, section
//! 12.4, "The MSAA hand-over, exactly"): every application's outpost takes
//! its focus, foreground, menu, alert, and tooltip events from its own
//! process-scoped hook, so they reach it by one route, in the order Windows
//! raised them, with its other MSAA events. The listener forwards them only
//! for an application whose outpost's hook is not yet in, so that the
//! application's first events start its outpost and reach it. When the
//! supervisor tells it an outpost's hook is in
//! ([`CoreToListener::HookInstalled`]), the listener reads the event
//! clock, forwards none of that application's events raised after it, and
//! queues "forwarding ended" naming that tick behind every fact it has
//! queued ([`ListenerToCore::ForwardingEnded`]); events raised at or
//! before it, which can still reach its hook late, it forwards still. The
//! outpost matches the two hooks' copies of an event by counting. When the
//! application has no outpost with its hook in any more
//! ([`CoreToListener::HookRemoved`]), the listener forwards its events
//! again, and its reading thread ends once the events waiting for it are
//! read. A listener that replaces another is told on its command line the
//! applications whose outposts' hooks are in, and forwards none of their
//! events from the start.
//!
//! A UIA focus that never comes is lost, as NVDA loses it: nothing waits
//! for it, here or in the outpost. The listener makes no backend verdict; it skips a focus only by its window's class
//! (`verbatim_listener_logic::focus::MSAA_CLASS`).
//!
//! Outgoing facts are coalesced with NVDA's UIA limiter rule before they are
//! sent: one waiting fact per element and kind (for a notification, per
//! element, kind, processing, and activity id; for a `WinEvent`'s fact
//! other than a menu opening, per window or address and the thread that
//! raised it, as NVDA's `WinEvent` limiter keys them), a newer one
//! replacing it
//! and moving to the back, so a flood from one busy process cannot pass through
//! the listener and Core unthrottled. A UIA focus's marker, read, and drop
//! are never replaced, since an outpost waits on the marker until its read
//! or drop arrives. Pongs and `Ready` go ahead of facts.
//!
//! The processes Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names) are held open here too, and every fact from one
//! of them is dropped before it is queued, so it is never routed and never
//! starts an outpost. Their events still reach the listener: UIA and
//! `WinEvent` delivery cannot be scoped to leave a process out.
//!
//! Run as `verbatim-listener.exe --listener --pipe-in <handle> --pipe-out
//! <handle> [--ignore-pids <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]`
//! — its own executable, with no target pid, supervised by the same
//! machinery as the outposts.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
    SetThreadpoolTimer, WaitForThreadpoolTimerCallbacks,
};
use windows::Win32::UI::Accessibility::{
    NotificationKind, NotificationProcessing, UIA_MenuOpenedEventId,
    UIA_SelectionItem_ElementSelectedEventId,
};

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEvent, WinEventKind};
use verbatim_model::{Notification, OutpostId, Pid, TraceId};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{Backlog, ChildIo, SendError, Sender, Served, Serving};
use verbatim_uia::map::{
    CachedUiaParts, ignores_notifications, notification_kind_from_uia,
    notification_processing_from_uia,
};
use verbatim_uia::{
    Agile, Cached, ListenerEvent, ListenerSelection, Part, Registration, Scope, Subscription,
};

use crate::focus_read::{FocusReader, Readers, Reading, WatchedRead};
use verbatim_ia2::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger};
use verbatim_listener_logic::focus::{FocusEvent, Judged, Meta, arrival_of};
use verbatim_listener_logic::foreground::{ForegroundWatch, Front, Step};
use verbatim_listener_logic::schedule::{GraceEffect, GraceInput, GraceState, QueuePlace};
use verbatim_model::protocol::{
    CoreToListener, DeliveredFact, EventTiming, FactKey, ListenerFact, ListenerToCore, Responder,
    UiaSnapshotFact, WinEventStamp, hop_us,
};
use verbatim_process::clock::now_us;
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::message::{decode_message, put_message};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::{Readability, WindowReadabilityReader, window_class_name};

/// How long after a menu or the Alt+Tab switcher closes the listener waits
/// before telling Core, so a focus event the end causes can arrive first:
/// NVDA's fake-focus delay (`processFakeFocusWinEvent`).
const MENU_END_GRACE: Duration = Duration::from_millis(50);

/// The priority of the listener's main thread, its I/O thread, which reads
/// Core's pipe, handles each command, and finishes the writes to Core that
/// pended, for microseconds per wake (`docs/design/communication.md`,
/// section 6.1).
const IO_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// The listener's messages to Core: written by the thread that produces
/// each one, through the transport's [`Sender`], and held in [`Waiting`]
/// only while a write is in flight, which the I/O thread then finishes
/// (`docs/design/communication.md`, sections 4.3 and 6.3).
struct Outgoing {
    sender: Sender<Waiting>,
    /// The processes whose facts are dropped.
    ignored: IgnoredProcesses,
    /// Numbers the UIA focus events' markers.
    markers: AtomicU64,
    /// Whether the desktop-wide UIA subscriptions have been installed, or
    /// their installation has failed: until then every UIA event is dropped
    /// ([`Self::uia_event_process`]).
    uia_installed: AtomicBool,
    /// The clock the facts' stamps are read from.
    clock: Clock,
    /// The end of a menu or of the Alt+Tab switcher waiting for its grace.
    menu_end: Arc<MenuEnd>,
    /// The threadpool timer that ends the grace, kept until the listener
    /// is dropped, which closes it; none in a test that fires the grace by
    /// hand ([`Outgoing::with_manual_grace`]).
    _grace: Option<GraceTimer>,
}

/// The clock the listener's stamps are read from: the shared clock,
/// [`now_us`], except in tests, which fix it.
#[derive(Clone, Copy)]
struct Clock(fn() -> u64);

impl Default for Clock {
    fn default() -> Self {
        Self(now_us)
    }
}

/// What waits while a write to Core is in flight: urgent messages first,
/// then facts, coalesced one per element and kind, and for an MSAA event
/// other than a menu opening per thread that raised it, then a menu or
/// switcher end. The backlog of the listener's [`Sender`], which every
/// message passes through, so the rules for "forwarding ended" are judged
/// under the lock every fact is sent under.
#[derive(Default)]
struct Waiting {
    urgent: VecDeque<ListenerToCore>,
    facts: VecDeque<(Option<(Pid, FactKey)>, ListenerToCore)>,
    /// The end of a menu or of the Alt+Tab switcher whose grace has passed,
    /// sent once no fact waits.
    menu_end: Option<ListenerToCore>,
    /// The applications whose outposts have their own hooks in, each with
    /// the tick on the event clock after which none of its `WinEvent`
    /// facts is forwarded.
    hooked: HashMap<Pid, u32>,
}

/// One item the listener sends Core.
enum Sent {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// or a fault.
    Urgent(ListenerToCore),
    /// A fact, with the key it is coalesced under, and, for a `WinEvent`'s,
    /// the application and the event's stamp.
    Fact(
        Option<(Pid, FactKey)>,
        ListenerToCore,
        Option<(Pid, WinEventStamp)>,
    ),
    /// "Forwarding ended" for an application whose outpost's hook is in,
    /// from the tick `through` on.
    ForwardingEnded {
        pid: Pid,
        outpost: OutpostId,
        through: u32,
    },
    /// A menu's or the switcher's end, its grace passed.
    MenuEnd(ListenerToCore),
    /// The idle barrier's answer for a round, behind everything waiting
    /// ([`Outgoing::idle_answer`]).
    IdleAnswer(u64),
    /// A message taken from what waits, to be written.
    Message(ListenerToCore),
}

impl Backlog for Waiting {
    type Item = Sent;
    type Refused = Infallible;

    fn push(&mut self, item: Sent) -> Result<(), Infallible> {
        match item {
            Sent::Urgent(message) | Sent::Message(message) => self.urgent.push_back(message),
            Sent::Fact(key, message, win_event) => {
                // Judged under the lock that queues "forwarding ended", so
                // no fact raised after its tick is ever queued behind it.
                if let Some((pid, stamp)) = win_event
                    && let Some(&through) = self.hooked.get(&pid)
                    && stamp.raised_after(through)
                {
                    return Ok(());
                }
                if let Some(key) = &key {
                    // Never across the idle barrier's answer: the fact
                    // replaced would move behind it.
                    let after_answer = self
                        .facts
                        .iter()
                        .rposition(|(_, waiting)| {
                            matches!(waiting, ListenerToCore::IdleAnswer { .. })
                        })
                        .map_or(0, |answer| answer + 1);
                    let mut index = after_answer;
                    while index < self.facts.len() {
                        if self.facts[index].0.as_ref() == Some(key) {
                            self.facts.remove(index);
                        } else {
                            index += 1;
                        }
                    }
                }
                self.facts.push_back((key, message));
            }
            Sent::ForwardingEnded {
                pid,
                outpost,
                through,
            } => {
                self.hooked.insert(pid, through);
                self.facts.push_back((
                    None,
                    ListenerToCore::ForwardingEnded {
                        pid,
                        outpost,
                        through,
                    },
                ));
            }
            Sent::MenuEnd(message) => self.menu_end = Some(message),
            Sent::IdleAnswer(round) => {
                // A menu end waiting for no fact goes ahead of the answer,
                // as it was sent before it.
                if let Some(end) = self.menu_end.take() {
                    self.facts.push_back((None, end));
                }
                self.facts
                    .push_back((None, ListenerToCore::IdleAnswer { round }));
            }
        }
        Ok(())
    }

    fn pop(&mut self) -> Option<Sent> {
        self.urgent
            .pop_front()
            .or_else(|| self.facts.pop_front().map(|(_, message)| message))
            .or_else(|| self.menu_end.take())
            .map(Sent::Message)
    }

    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.facts.is_empty() && self.menu_end.is_none()
    }

    fn encode(&mut self, item: Sent, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message = match item {
            Sent::Urgent(message)
            | Sent::Message(message)
            | Sent::Fact(_, message, _)
            | Sent::MenuEnd(message) => message,
            Sent::IdleAnswer(round) => ListenerToCore::IdleAnswer { round },
            Sent::ForwardingEnded {
                pid,
                outpost,
                through,
            } => ListenerToCore::ForwardingEnded {
                pid,
                outpost,
                through,
            },
        };
        // The write's stamp, for the latency ledger's split of the
        // listener's time from the pipe's, taken as the write begins,
        // whichever thread writes.
        message.written(now_us());
        put_message(buffer, &message)
    }
}

/// The end of a menu or of the Alt+Tab switcher, waiting for its grace:
/// when it ended, in microseconds since the Unix epoch for Core, and its
/// observed and handled stamps. NVDA fakes a focus from such an end only
/// when no focus event was validly processed with it; a focus fact can
/// still be dropped or fail to read, so Core, which knows which focus it
/// applied, decides, and every end is told.
struct MenuEnd {
    /// What waits for the grace, a pure transition
    /// ([`GraceState::step`]).
    waiting: Mutex<GraceState>,
    /// Now, in the grace's microseconds: since the listener's sending side
    /// was made, or a test's clock.
    clock: GraceClock,
    sender: Sender<Waiting>,
    /// What sets the timer that ends the grace to go off after a time, set
    /// once the timer is made, which the callback sets again when it runs
    /// before the end's grace is over; a test's records the time instead.
    timer: OnceLock<SetGraceTimer>,
}

/// The grace's clock ([`MenuEnd::clock`]).
type GraceClock = Box<dyn Fn() -> u64 + Send + Sync>;

/// What sets the grace's timer ([`MenuEnd::timer`]).
type SetGraceTimer = Box<dyn Fn(Duration) + Send + Sync>;

/// A threadpool timer's handle, shared with its own callback.
struct TimerHandle(PTP_TIMER);

impl TimerHandle {
    /// Sets the timer to go off once, `after` from now ([`set_timer`]).
    fn set(&self, after: Duration) {
        set_timer(self.0, after);
    }
}

// SAFETY: a threadpool timer may be set from any thread, its own callback
// included; the handle stays open until `GraceTimer::drop`, which stops
// the callback setting it first (`GraceInput::Close`).
unsafe impl Send for TimerHandle {}
// SAFETY: as above.
unsafe impl Sync for TimerHandle {}

impl MenuEnd {
    fn new(sender: Sender<Waiting>, clock: GraceClock) -> Self {
        Self {
            waiting: Mutex::new(GraceState::default()),
            clock,
            sender,
            timer: OnceLock::new(),
        }
    }

    /// Now, in the grace's microseconds.
    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// Steps the grace with `input` and applies its effects, under the
    /// grace's lock, so no answer can pass the end: the end is sent behind
    /// every fact waiting, then the idle barrier's answers that waited for
    /// it, and the timer is set when the step says so.
    fn step(&self, input: GraceInput) {
        let mut grace = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        for effect in grace.step(input, self.now()) {
            match effect {
                GraceEffect::SendEnd {
                    ended_at_us,
                    timing,
                } => sent(
                    &self
                        .sender
                        .send(Sent::MenuEnd(ListenerToCore::MenuOrSwitchEnded {
                            ended_at_us,
                            timing,
                        })),
                ),
                GraceEffect::SendIdleAnswer(round) => {
                    sent(&self.sender.send(Sent::IdleAnswer(round)));
                }
                GraceEffect::SetTimer(after) => {
                    if let Some(timer) = self.timer.get() {
                        timer(Duration::from_micros(after));
                    }
                }
            }
        }
    }

    /// The grace's timer went off: the end is sent once its grace is over,
    /// and a callback that runs before then sets the timer again for the
    /// rest of the grace.
    fn grace_ended(&self) {
        self.step(GraceInput::Fired);
    }
}

/// A send's outcome: once the listener is shutting down, or Core's pipe
/// has gone, what is sent is dropped.
fn sent(sent: &Result<(), SendError<Infallible>>) {
    if sent.is_err() {
        tracing::debug!("a message to Core is dropped: the pipe is closed");
    }
}

/// A threadpool timer that ends a menu end's grace
/// (`docs/design/communication.md`, section 4.7): no thread of the
/// listener's waits for it, and its callback sends the end itself.
struct GraceTimer {
    timer: PTP_TIMER,
    /// The end the callback is given, kept alive until the timer is closed.
    end: *const MenuEnd,
}

// SAFETY: a threadpool timer may be set and closed from any thread, and
// the end it points to is shared, `Sync` data kept alive by the timer.
unsafe impl Send for GraceTimer {}
// SAFETY: as above; `arm` only sets the timer, which the threadpool allows
// from any thread at any time.
unsafe impl Sync for GraceTimer {}

impl GraceTimer {
    fn new(end: &Arc<MenuEnd>) -> io::Result<Self> {
        let end = Arc::into_raw(Arc::clone(end));
        // SAFETY: the callback is given `end`, which stays alive until the
        // timer is closed in `drop`, after its callbacks have finished.
        let timer =
            unsafe { CreateThreadpoolTimer(Some(grace_ended), Some(end.cast_mut().cast()), None) };
        match timer {
            Ok(timer) => {
                // Set before the timer is first set, so before any callback.
                let handle = TimerHandle(timer);
                let _ = end_of(end)
                    .timer
                    .set(Box::new(move |after| handle.set(after)));
                Ok(Self { timer, end })
            }
            Err(error) => {
                // SAFETY: from `Arc::into_raw` above, never given out.
                drop(unsafe { Arc::from_raw(end) });
                Err(io::Error::other(error))
            }
        }
    }
}

/// The [`MenuEnd`] a [`GraceTimer`] was made with.
fn end_of<'a>(end: *const MenuEnd) -> &'a MenuEnd {
    // SAFETY: from `Arc::into_raw` in `GraceTimer::new`, released only in
    // `GraceTimer::drop`, after the last use.
    unsafe { &*end }
}

/// Sets `timer`, open until its [`GraceTimer`] is dropped, to fire once
/// `after` from now, in place of any time it was set for.
fn set_timer(timer: PTP_TIMER, after: Duration) {
    // Relative, in 100-nanosecond units.
    let due = -i64::try_from(after.as_nanos() / 100).unwrap_or(i64::MAX);
    let due = due.cast_unsigned();
    let due = FILETIME {
        dwLowDateTime: (due & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (due >> 32) as u32,
    };
    // SAFETY: the timer is open; the due time is read during the call.
    unsafe { SetThreadpoolTimer(timer, Some(&raw const due), 0, Some(0)) };
}

impl Drop for GraceTimer {
    fn drop(&mut self) {
        // The callback sets the timer no more; then the timer is stopped,
        // its callbacks waited for or cancelled, and only then closed and
        // its end released.
        end_of(self.end).step(GraceInput::Close);
        // SAFETY: the timer is open.
        unsafe { SetThreadpoolTimer(self.timer, None, 0, None) };
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, true) };
        // SAFETY: as above; no callback runs or will run.
        unsafe { CloseThreadpoolTimer(self.timer) };
        // SAFETY: from `Arc::into_raw` in `new`, released once, here, with
        // no callback left to use it.
        drop(unsafe { Arc::from_raw(self.end) });
    }
}

/// The grace timer's callback, on a threadpool thread.
unsafe extern "system" fn grace_ended(
    _instance: PTP_CALLBACK_INSTANCE,
    end: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: the timer was created with its `MenuEnd`, which outlives
    // every callback (`GraceTimer::drop`).
    let end = unsafe { &*end.cast_const().cast::<MenuEnd>() };
    end.grace_ended();
}

impl Outgoing {
    /// Sends through `sender`, dropping the facts of the processes
    /// `ignored` names.
    ///
    /// # Errors
    ///
    /// Returns the error of creating the menu end's grace timer.
    fn new(sender: Sender<Waiting>, ignored: IgnoredProcesses) -> io::Result<Self> {
        let origin = Instant::now();
        let menu_end = Arc::new(MenuEnd::new(
            sender.clone(),
            Box::new(move || u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX)),
        ));
        let grace = GraceTimer::new(&menu_end)?;
        Ok(Self::with_menu_end(sender, ignored, menu_end, Some(grace)))
    }

    /// [`new`](Self::new) with the grace read from `clock` and its timer
    /// set by `timer`, for a test that moves the clock and steps the
    /// timer's going off by hand ([`MenuEnd::grace_ended`]), so no
    /// threadpool timer races the test's own steps.
    #[cfg(test)]
    fn with_manual_grace(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        clock: GraceClock,
        timer: SetGraceTimer,
    ) -> Self {
        let menu_end = Arc::new(MenuEnd::new(sender.clone(), clock));
        let _ = menu_end.timer.set(timer);
        Self::with_menu_end(sender, ignored, menu_end, None)
    }

    fn with_menu_end(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        menu_end: Arc<MenuEnd>,
        grace: Option<GraceTimer>,
    ) -> Self {
        Self {
            sender,
            ignored,
            markers: AtomicU64::new(0),
            uia_installed: AtomicBool::new(false),
            clock: Clock::default(),
            menu_end,
            _grace: grace,
        }
    }

    fn urgent(&self, message: ListenerToCore) {
        sent(&self.sender.send(Sent::Urgent(message)));
    }

    /// Sends a fact, replacing any waiting fact for the same element and
    /// kind. `raised_ms_ago` is how long before now Windows raised the event,
    /// and `win_event` which thread raised it and when, for a `WinEvent`'s
    /// fact, and `observed_at_us` when the listener's callback began on it.
    /// A `WinEvent`'s fact raised after the outpost of `pid` has its own
    /// hook in is dropped: that hook took it in.
    fn fact(
        &self,
        pid: Pid,
        fact: DeliveredFact,
        raised_ms_ago: Option<u32>,
        win_event: Option<WinEventStamp>,
        observed_at_us: u64,
    ) {
        let timing = EventTiming {
            raised_ms_ago,
            observed_at_us,
            ..EventTiming::default()
        };
        self.queue(pid, fact, (TraceId::mint(), timing), win_event);
    }

    /// Sends `fact`, part of the read of the UIA focus event `meta`
    /// describes: its marker, its read, or its drop, carrying the trace and
    /// times of the event itself. Never replaced, since an outpost waits on
    /// the marker until its read or drop arrives.
    fn focus_read(&self, pid: Pid, fact: DeliveredFact, meta: Meta) {
        self.queue(pid, fact, (meta.trace, meta.timing), None);
    }

    /// Sends Core the window that came to the foreground, `front`, with its
    /// root owner, the process its facts go to, and that process's
    /// integrity level, read here with local calls (`GetAncestor`,
    /// `GetWindowThreadProcessId`, `GetProcessIdOfThread` for a console
    /// window whose thread its event named, and the token's integrity).
    /// Sent for every process, ignored ones included, since Core takes from
    /// it the keyboard hook's view, attention and its foreground record;
    /// none replaces another (`docs/design/focus-pipeline-rework-remaining.md`,
    /// "One foreground change, from the listener").
    fn foreground(&self, front: Front) {
        let owner = window_pid(front.window);
        let process = if front.console && front.thread != 0 {
            process_of_thread(front.thread).unwrap_or(owner)
        } else {
            owner
        };
        let message = ListenerToCore::Foreground {
            window: front.window,
            root_owner: verbatim_win32::window::root_owner_of(front.window),
            process,
            observed_at_us: front.observed_at_us,
            thread: front.thread,
            console: front.console,
            integrity: verbatim_win32::integrity::process_integrity(owner),
        };
        sent(&self.sender.send(Sent::Fact(None, message, None)));
    }

    /// Marks the desktop-wide UIA subscriptions as installed, or as failed:
    /// UIA events are taken from then on.
    fn uia_installation_ended(&self) {
        self.uia_installed.store(true, Ordering::Release);
    }

    /// The process a UIA event from the process `process_id` is taken for,
    /// or `None` when it is dropped: an event received before the
    /// desktop-wide subscriptions were all installed, as NVDA's UIA
    /// handlers ignore every event received before the handler's
    /// initialization has finished, after its global handlers are
    /// registered (`books/dev/src/explanation/design/focus-pipeline.md`, section 18). Such an
    /// event is the focus UI Automation reports to a new focus
    /// registration, which can be an element that only holds its own
    /// framework's focus, such as the taskbar's Start button while the
    /// desktop is in front; Verbatim's start asks the foreground
    /// application's outpost for its focus instead, as NVDA sets its
    /// initial focus when it took no focus event. An event with no
    /// process, or from an ignored one, is dropped too.
    fn uia_event_process(&self, process_id: Option<u32>) -> Option<Pid> {
        if !self.uia_installed.load(Ordering::Acquire) {
            tracing::debug!(
                ?process_id,
                "UIA event dropped: received before the listener's UIA subscriptions were installed"
            );
            return None;
        }
        let pid = process_id.filter(|&pid| pid != 0).map(Pid)?;
        (!self.ignored.contains(pid)).then_some(pid)
    }

    /// A new marker's number.
    fn next_marker(&self) -> u64 {
        self.markers.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A UIA event of kind `kind` the listener's callback received now: a
    /// new marker and trace, and its observation, in microseconds from one
    /// clock read, by which the focus it becomes is ordered.
    fn received(&self, kind: FocusEvent) -> Meta {
        Meta {
            marker: self.next_marker(),
            kind,
            trace: TraceId::mint(),
            timing: EventTiming {
                observed_at_us: self.now(),
                ..EventTiming::default()
            },
        }
    }

    /// Sends `fact` with its trace, the time it was observed, and its
    /// timing, under the rules [`fact`](Self::fact) states. A fact no
    /// reading thread finished with is stamped as handled now, as it is
    /// sent.
    fn queue(
        &self,
        pid: Pid,
        fact: DeliveredFact,
        (trace_id, mut timing): (TraceId, EventTiming),
        win_event: Option<WinEventStamp>,
    ) {
        if self.ignored.contains(pid) {
            return;
        }
        if timing.listener_handled_at_us == 0 {
            timing.listener_handled_at_us = self.now();
        }
        let key = fact.key(win_event).map(|key| (pid, key));
        let message = ListenerToCore::FocusFact {
            trace_id,
            timing,
            fact: ListenerFact { pid, fact },
            win_event,
        };
        sent(&self.sender.send(Sent::Fact(
            key,
            message,
            win_event.map(|stamp| (pid, stamp)),
        )));
    }

    /// The outpost `outpost` watching `pid` has its own hook in: none of
    /// `pid`'s `WinEvent` facts raised after `through`, a tick on the event
    /// clock read now or earlier, is forwarded from now on, and "forwarding
    /// ended" is sent behind every fact already waiting. Facts raised at
    /// or before it are still forwarded, since the outpost's hook may not
    /// have taken them in, and the outpost matches any it did by counting.
    fn hook_installed(&self, pid: Pid, outpost: OutpostId, through: u32) {
        sent(&self.sender.send(Sent::ForwardingEnded {
            pid,
            outpost,
            through,
        }));
    }

    /// `pid` has no outpost with its hook in any more: its `WinEvent` facts
    /// are forwarded again.
    fn hook_removed(&self, pid: Pid) {
        self.sender
            .with_backlog(|waiting| waiting.hooked.remove(&pid));
    }

    /// Notes that a menu or the Alt+Tab switcher closed, which the
    /// listener's callback observed at `observed_at_us`, and starts its
    /// grace, which a later end starts again.
    fn menu_or_switch_ended(&self, observed_at_us: u64) {
        self.note_menu_end(observed_at_us, MENU_END_GRACE);
    }

    /// Notes that a menu or the Alt+Tab switcher closed, observed at
    /// `observed_at_us`, to be sent once its grace, `grace` from now, is
    /// over, and sets the timer for it, in place of any grace running.
    fn note_menu_end(&self, observed_at_us: u64, grace: Duration) {
        let timing = EventTiming {
            observed_at_us,
            listener_handled_at_us: self.now(),
            ..EventTiming::default()
        };
        self.menu_end.step(GraceInput::Ended {
            ended_at_us: observed_at_us,
            timing,
            grace: u64::try_from(grace.as_micros()).unwrap_or(u64::MAX),
        });
    }

    /// Answers the idle barrier's round `round`, behind every fact sent
    /// before, never merged and never ahead as urgent; behind a menu end
    /// still in its grace too, once that is sent.
    fn idle_answer(&self, round: u64) {
        self.menu_end.step(GraceInput::IdleAnswer(round));
    }

    /// Now, on the clock the listener's stamps are read from.
    fn now(&self) -> u64 {
        (self.clock.0)()
    }

    fn fault(&self, detail: String) {
        self.urgent(ListenerToCore::Fault { detail });
    }

    /// Takes nothing more, and has what waits written and the pipe closed,
    /// for the listener's shutdown; a menu end still in its grace is
    /// dropped.
    fn close(&self) {
        self.sender.close();
    }
}

/// What makes the listener's desktop-wide UIA subscriptions, on the
/// thread [`Listener::start`] starts for it, returning them, or `None`
/// when their installation failed.
type InstallSubscriptions =
    Box<dyn FnOnce(&Arc<Outgoing>, &Arc<Readers<UiaFocusReading>>) -> Option<Registration> + Send>;

/// The focus listener: owns its subscriptions for the whole life of the
/// process.
struct Listener {
    outgoing: Arc<Outgoing>,
    readers: Arc<Readers<UiaFocusReading>>,
    /// The thread making the desktop-wide UIA registration, which returns
    /// it, or `None` when it failed ([`Listener::start`]).
    registering: JoinHandle<Option<Registration>>,
    /// What nudges the `WinEvent` thread: the idle barrier's query goes
    /// through it, behind every event delivered to it before.
    nudger: Nudger,
    _event_thread: EventThread,
}

/// The reading threads' work in the listener: each UIA focus event judged
/// and read from its own element ([`FocusReader`]), and what became of it
/// sent to its application's outpost behind its marker, through the
/// answer owed for it ([`MarkerOwed`]).
struct UiaFocusReading {
    /// The listener's window readability reader: the one judge of whether
    /// a window is hung, by Windows' own state, which each reading thread's
    /// reader, for the windows its walk reaches, and the UIA callbacks'
    /// test on arrival are given ([`Readers::window_hung`]). The system's
    /// ([`verbatim_win32::window::readability`]), unless a test gives its
    /// own.
    readability: WindowReadabilityReader,
}

impl UiaFocusReading {
    /// The listener's reading, judging windows by the system's state.
    fn new() -> Self {
        Self {
            readability: Arc::new(verbatim_win32::window::readability),
        }
    }
}

impl Reading for UiaFocusReading {
    /// The event's element, its own window handle from the event's cache
    /// (0 for an element that is not a window), its runtime id from the
    /// event's cache, the kind of event, and, for a selection or a
    /// notification, the fact captured from it.
    type Job = (
        Agile<ListenerEvent>,
        isize,
        Vec<i32>,
        FocusEvent,
        Option<DeliveredFact>,
    );
    type Reader = FocusReader;

    fn reader(&self, _pid: Pid) -> Option<FocusReader> {
        match FocusReader::new() {
            Ok(mut reader) => {
                reader.set_window_readability_reader(Arc::clone(&self.readability));
                Some(reader)
            }
            Err(error) => {
                tracing::warn!(%error, "a reading thread could not create a UIA client");
                None
            }
        }
    }

    fn read(
        &self,
        reader: Option<&mut FocusReader>,
        meta: Meta,
        (element, _, _, kind, fact): Self::Job,
        (watched, queue): (&WatchedRead, QueuePlace),
    ) -> Judged {
        let Some(reader) = reader else {
            return Judged::Dropped("its reading thread has no UIA client");
        };
        match element.resolve() {
            Ok(sender) => reader.judge_watched(
                &sender,
                (kind, fact),
                (arrival_of(meta, queue), watched.clone()),
            ),
            Err(_) => Judged::Dropped("its element could not be resolved"),
        }
    }

    /// A focus's or a menu opening's sender's own window, from the event's
    /// cache: the moved-on rule judges the read of a focus once its window
    /// is known, here or from the nearest-window lookup, and never a
    /// selection's or a notification's check, which NVDA makes on UIA's
    /// callback thread, whose calls its watchdog never cancels
    /// (`docs/parity/focus-events.md`, "A slow read the user has moved on
    /// from").
    fn window(&self, (_, hwnd, _, kind, _): &Self::Job) -> Option<isize> {
        match kind {
            FocusEvent::Focus | FocusEvent::MenuOpened => Some(*hwnd),
            FocusEvent::Selection | FocusEvent::Notification => None,
        }
    }

    fn forget_last(&self, reader: &mut FocusReader) {
        reader.forget_last();
    }

    fn readability(&self, window: isize) -> Readability {
        (self.readability)(window)
    }

    fn kind(&self, (_, _, _, kind, _): &Self::Job) -> FocusEvent {
        *kind
    }

    fn key<'a>(
        &self,
        (_, _, runtime_id, kind, _): &'a Self::Job,
    ) -> Option<(FocusEvent, &'a [i32])> {
        // A notification is replaced only by one of the same kind,
        // processing, and activity, as the intake replaces it.
        (*kind != FocusEvent::Notification).then_some((*kind, runtime_id.as_slice()))
    }

    fn finish(&self, reader: Option<FocusReader>) {
        let joined = reader.is_some();
        drop(reader);
        verbatim_uia::release_thread_state();
        if joined {
            // Leaves the apartment the reader's client joined.
            verbatim_uia::leave_mta();
        }
    }

    type Owed = MarkerOwed;

    fn outcome(&self, pid: Pid, meta: Meta, owed: MarkerOwed, judged: Judged) {
        if let Some(reason) = judged.dropped_reason() {
            tracing::debug!(%pid, marker = meta.marker, reason, "UIA focus dropped");
        }
        owed.0.answer(judged.into_fact(meta.marker, meta.kind));
    }
}

/// The answer the listener owes an application's outpost for the UIA
/// focus event whose marker it sent: made as the event is queued for its
/// reading thread ([`queue_for_reading`]), kept by the readers' shell, and
/// consumed by the event's outcome, which sends the read or the drop.
/// Dropped unanswered, as when the readers are dropped with the event
/// still waiting, it sends the marker's drop, as the drop of an element
/// that could not be kept for its read does, so the outpost never waits on
/// the marker.
pub(crate) struct MarkerOwed(Responder<DeliveredFact>);

impl MarkerOwed {
    /// The answer owed for `pid`'s event `meta`, sent through `outgoing`.
    fn new(outgoing: &Arc<Outgoing>, pid: Pid, meta: Meta) -> Self {
        let outgoing = Arc::clone(outgoing);
        Self(Responder::new(move |fact: Option<DeliveredFact>| {
            let fact = fact.unwrap_or_else(|| {
                Judged::Dropped("the listener ended before it was read")
                    .into_fact(meta.marker, meta.kind)
            });
            outgoing.focus_read(pid, fact, meta);
        }))
    }
}

impl Listener {
    /// Sets up the listener, sending through `sender`, with its
    /// desktop-wide UIA subscriptions ([`install_desktop_subscriptions`]):
    /// see [`Self::start`].
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    fn new(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        hooks_in: &[(Pid, OutpostId)],
    ) -> io::Result<Self> {
        Self::start(
            sender,
            ignored,
            hooks_in,
            Box::new(|outgoing, readers| {
                install_desktop_subscriptions(
                    outgoing,
                    readers,
                    focus_subscription(outgoing, readers),
                    menu_subscription(outgoing, readers),
                )
            }),
        )
    }

    /// Sets up the listener, sending through `sender`: installs its
    /// `WinEvent` hooks, and then has `install` make the desktop-wide UIA
    /// subscriptions on a thread of their own, which announces readiness
    /// once `install` returns, so the supervisor and Core know the
    /// `WinEvent`s, menus included, and the UIA events are seen from then
    /// on; Verbatim takes input only once the listener is ready, so a
    /// scenario's first key waits for it. The hooks go in first, so MSAA
    /// facts flow during the installation, and it runs on its own thread,
    /// so the pipe thread answers the supervisor's pings meanwhile,
    /// because UIA reads the system's focus for the focus registration and
    /// waits on the foreground application for it, up to about 6 seconds
    /// for one that does not answer
    /// (`docs/design/cross-process-calls-study.md`, instance 2). Their
    /// failure is reported as a fault, before the announcement. Every UIA
    /// event is dropped until `install` returns
    /// ([`Outgoing::uia_event_process`]); a `WinEvent` focus whose UIA
    /// focus is dropped meanwhile is lost, as NVDA loses one.
    ///
    /// The applications in `hooks_in` have outposts with their own hooks
    /// in already: none of their `WinEvent`s is forwarded, and each outpost
    /// is told so first, ending a hand-over the listener this one replaces
    /// may have left unfinished.
    ///
    /// # Errors
    ///
    /// Returns the error of creating the menu end's grace timer, or of
    /// starting the thread that installs the UIA subscriptions.
    fn start(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        hooks_in: &[(Pid, OutpostId)],
        install: InstallSubscriptions,
    ) -> io::Result<Self> {
        let outgoing = Arc::new(Outgoing::new(sender, ignored)?);
        // Every event this listener's hook takes in is raised after the
        // hook is installed, so after this tick: none is forwarded for
        // these applications.
        let before_the_hook = verbatim_ia2::event_clock().wrapping_sub(1);
        for &(pid, outpost) in hooks_in {
            outgoing.hook_installed(pid, outpost, before_the_hook);
        }
        let readers = Arc::new(Readers::new(
            UiaFocusReading::new(),
            verbatim_model::FOCUS_READ_DEADLINE,
        ));
        let msaa_outgoing = Arc::clone(&outgoing);
        let idle_readers = Arc::clone(&readers);
        let make_handlers: MakeHandlers = Arc::new(move || {
            let outgoing = Arc::clone(&msaa_outgoing);
            let idle_outgoing = Arc::clone(&msaa_outgoing);
            let readers = Arc::clone(&idle_readers);
            let foreground_readers = Arc::clone(&readers);
            // The window in front, for the keyboard hook's view of it,
            // kept on this thread by both handlers.
            let watch = Rc::new(RefCell::new(ForegroundWatch::default()));
            let start_watch = Rc::clone(&watch);
            EventHandlers {
                win_event: Box::new(move |event: WinEvent| {
                    if matches!(
                        event.kind,
                        WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret
                    ) {
                        let step = watch.borrow_mut().console_event(
                            event.hwnd,
                            event.thread,
                            outgoing.now(),
                        );
                        foreground_step(&outgoing, step);
                        return;
                    }
                    // Program Manager or the taskbar in front: no foreground
                    // change to forward, as NVDA takes none, but a window
                    // that came to the foreground for Core.
                    if event.kind == WinEventKind::ShellForeground {
                        let front = Front {
                            window: event.hwnd,
                            thread: event.thread,
                            console: false,
                            observed_at_us: outgoing.now(),
                        };
                        let step = watch.borrow_mut().foreground(front, in_front(front.window));
                        foreground_step(&outgoing, step);
                        return;
                    }
                    // Forwarded first, so the event's observation time is
                    // taken before anything else the callback does; its
                    // window's class, read to route it, tells a console.
                    let (kind, window, thread) = (event.kind, event.hwnd, event.thread);
                    let forwarded = forward_msaa_event(&outgoing, event);
                    if kind == WinEventKind::Foreground {
                        let front = Front {
                            window,
                            thread,
                            console: forwarded.console,
                            observed_at_us: forwarded.observed_at_us,
                        };
                        let step = watch.borrow_mut().foreground(front, in_front(window));
                        foreground_step(&outgoing, step);
                        // A foreground change moves away from a reading
                        // thread's last read of another window, for menu
                        // currentness; it judges no read in flight.
                        foreground_readers.foreground_changed(window);
                    } else if kind == WinEventKind::Focus && watch.borrow().has_pending() {
                        // A window not in front at its foreground event is
                        // sent once an event finds it there.
                        let step = watch
                            .borrow_mut()
                            .event_seen(front_window(), forwarded.observed_at_us);
                        foreground_step(&outgoing, step);
                    }
                    let focus_of = forwarded.focus_of;
                    // Another process's MSAA focus moves away from every
                    // other application's last read, for menu currentness.
                    if let Some(pid) = focus_of {
                        foreground_readers.msaa_focus(pid);
                    }
                }),
                // The idle barrier's query, its round in the first word:
                // every event delivered before it has been handled, so the
                // reading threads are waited for next. Or, once, the start
                // of the foreground's watch, once the hooks are in.
                nudge: Some(Box::new(move |round, kind| {
                    if kind == NUDGE_FOREGROUND_START {
                        let step = start_watch
                            .borrow_mut()
                            .start(front_now(idle_outgoing.now()));
                        foreground_step(&idle_outgoing, step);
                    } else {
                        answer_when_reads_are_done(&readers, &idle_outgoing, round as u64);
                    }
                })),
            }
        });
        let event_thread = EventThread::spawn(LISTENER_SUBSCRIPTIONS, make_handlers);
        if let Err(error) = event_thread.hook(0) {
            tracing::warn!(error, "failed to install WinEvent hooks");
        }
        let nudger = event_thread.nudger();
        // The window in front now, reported from the event thread behind the
        // hooks' install, so a foreground event raised since is reported
        // after it.
        nudger.nudge((0, NUDGE_FOREGROUND_START));

        let registering = install_on_its_thread(&outgoing, &readers, install)?;

        Ok(Self {
            outgoing,
            readers,
            registering,
            nudger,
            _event_thread: event_thread,
        })
    }

    /// Shuts the listener down cleanly ([`CoreToListener::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA subscriptions'
    /// installation is waited for if it has not ended, the desktop-wide UIA
    /// focus handler and the
    /// other desktop-wide handlers are removed (the registering thread
    /// waits for a callback in progress), every reading thread finishes the read in hand, each abandoned
    /// one returning once UIA gives up on its call, what waits to be
    /// written to Core is written, the I/O thread `io` serves waiting for
    /// every write in flight to complete, and the pipe is closed, and the
    /// process's hold on COM's multithreaded apartment is given back, the
    /// registering thread releasing its client and leaving COM first.
    fn shutdown(self, io: &mut ChildIo) {
        let started = Instant::now();
        let Self {
            outgoing,
            readers,
            registering,
            nudger: _,
            _event_thread: event_thread,
        } = self;
        drop(event_thread);
        let registration = registering
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let registration_failed_reads = registration.as_ref().map_or(0, Registration::failed_reads);
        drop(registration);
        let reading_threads = readers.stop();
        let failed_reads = registration_failed_reads.saturating_add(readers.failed_reads());
        outgoing.close();
        io.finish(&outgoing.sender);
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            reading_threads,
            failed_reads,
            "the focus listener shut down"
        );
    }

    /// Answers `Ping` with a `Pong` (the listener never abandons a worker, so
    /// its count is always zero), takes the hand-over's news of outposts'
    /// hooks, starts the idle barrier's round
    /// ([`Self::idle_query`]). An application with no outpost
    /// any more has its reading thread ended too
    /// ([`Readers::outpost_ended`]). `received_at_us` is when the command
    /// was read off Core's pipe: the hop from verbatim.exe's write is
    /// logged at trace level, under `verbatim::stage`.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn handle_command(&self, command: &CoreToListener, received_at_us: u64) {
        if let Some(us) = hop_us(command.sent_at_us(), received_at_us) {
            tracing::trace!(target: "verbatim::stage", stage = "verbatim.exe to the listener", us, ?command);
        }
        match command {
            CoreToListener::Ping { seq, .. } => {
                self.outgoing.urgent(ListenerToCore::Pong {
                    seq: *seq,
                    parked_count: 0,
                    written_at_us: 0,
                });
            }
            CoreToListener::HookInstalled { pid, outpost, .. } => {
                self.outgoing
                    .hook_installed(*pid, *outpost, verbatim_ia2::event_clock());
            }
            CoreToListener::HookRemoved { pid, .. } => {
                self.outgoing.hook_removed(*pid);
                self.readers.outpost_ended(*pid);
            }
            CoreToListener::IdleQuery { round } => self.idle_query(*round),
            // The serving loop ends on `Shutdown`, before it is handled here.
            CoreToListener::Shutdown => {}
        }
    }

    /// The idle barrier's query for `round` (`docs/design/communication.md`,
    /// section 6.7): handed to the `WinEvent` thread behind every event
    /// delivered to it, then to the reading threads behind every event
    /// queued for them, and answered once both have nothing in progress.
    /// A `WinEvent` thread that has ended has nothing in progress.
    fn idle_query(&self, round: u64) {
        let posted = usize::try_from(round).is_ok_and(|word| self.nudger.nudge((word, 0)));
        if !posted {
            answer_when_reads_are_done(&self.readers, &self.outgoing, round);
        }
    }
}

/// Starts the thread that makes the listener's desktop-wide UIA
/// subscriptions with `install`, takes UIA events from then on
/// ([`Outgoing::uia_installation_ended`]), logs the time it took, says
/// the listener is `Ready`, whether or not they were installed, and
/// returns them.
///
/// # Errors
///
/// Returns the error of starting the thread.
fn install_on_its_thread(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
    install: InstallSubscriptions,
) -> io::Result<JoinHandle<Option<Registration>>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    std::thread::Builder::new()
        .name("listener UIA subscriptions".to_owned())
        .spawn(move || {
            let started = Instant::now();
            let registration = install(&outgoing, &readers);
            outgoing.uia_installation_ended();
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                installed = registration.is_some(),
                "the listener's desktop-wide UIA subscriptions ended their installation"
            );
            // The listener has no target application; `target_pid` is a
            // sentinel the supervisor only logs.
            outgoing.urgent(ListenerToCore::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(0),
            });
            registration
        })
}

/// Answers the idle barrier's round `round` once no reading thread has an
/// event waiting or being read, behind the outcome of every one queued
/// before ([`Readers::when_idle`]).
fn answer_when_reads_are_done(
    readers: &Readers<UiaFocusReading>,
    outgoing: &Arc<Outgoing>,
    round: u64,
) {
    let outgoing = Arc::clone(outgoing);
    readers.when_idle(Box::new(move || outgoing.idle_answer(round)));
}

/// What a UIA callback captures from an event's element, `sender`, read
/// as `fields`: its cached window handle and snapshot parts. `None` for an
/// element with no owning process. Public so that mockapp's tests make a
/// notification's fact from mockapp's own event as the listener makes it.
#[must_use]
pub fn capture(
    sender: &Cached<ListenerEvent>,
    fields: &ListenerEvent,
) -> Option<(isize, UiaSnapshotFact)> {
    fields.process_id.filter(|&pid| pid != 0)?;
    Some((
        fields.role.hwnd,
        snapshot_fact(fields.parts(sender.runtime_id())),
    ))
}

/// What the selection callback captures from the selected item, `sender`,
/// read as `fields`: as [`capture`], with the item's value, states, and
/// details from its event's cache ([`ListenerSelection`]), which Core
/// speaks of an item selected inside a focused list or tab control. Public
/// so that mockapp's tests make a selection's fact from mockapp's own
/// event as the listener makes it.
#[must_use]
pub fn capture_selection(
    sender: &Cached<ListenerSelection>,
    fields: &ListenerSelection,
) -> Option<(isize, UiaSnapshotFact)> {
    fields.event.process_id.filter(|&pid| pid != 0)?;
    Some((
        fields.event.role.hwnd,
        snapshot_fact(fields.parts(sender.runtime_id())),
    ))
}

/// A fact's snapshot of an element's cached parts.
fn snapshot_fact(parts: CachedUiaParts) -> UiaSnapshotFact {
    UiaSnapshotFact {
        runtime_id: parts.runtime_id,
        role: parts.role,
        name: parts.name,
        value: parts.value,
        states: parts.states,
        details: parts.details,
    }
}

/// The desktop-wide UIA focus subscription. Its callback makes no
/// cross-process call: it drops an event whose sender's cached window is
/// hung ([`queue_for_reading`]), and otherwise sends the application's
/// outpost a marker for the event at once and queues the event's element
/// for the application's reading thread ([`Readers`]), whose read or drop
/// follows the marker.
fn focus_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let callback = Arc::new(move |sender: &Cached<ListenerEvent>| {
        queue_for_reading(
            &outgoing,
            &readers,
            sender,
            &sender.read(),
            FocusEvent::Focus,
            None,
        );
    });
    Subscription::Focus { callback }
}

/// The desktop-wide subscription to UIA menu openings: each is judged and
/// read as a focus on the menu by its application's reading thread, as
/// NVDA's automation event handler maps it to a focus and judges it
/// (`books/dev/src/explanation/design/focus-pipeline.md`, section 18), unless a focus is
/// pending as it arrives, when NVDA drops it.
fn menu_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    Subscription::Event {
        event: UIA_MenuOpenedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            // The event's process from its cache, the pid its focuses are
            // read under: no call.
            if readers.focus_pending(fields.process_id.map(Pid)) {
                tracing::debug!("UIA menu opening dropped: a focus is pending");
                return;
            }
            queue_for_reading(
                &outgoing,
                &readers,
                sender,
                &fields,
                FocusEvent::MenuOpened,
                None,
            );
        }),
    }
}

/// Queues the UIA event `kind` whose sender is `sender`, read as `fields`,
/// for its application's reading thread, after sending its marker; `fact`,
/// for a selection or a notification, is what was captured from it. An
/// event whose sender's cached window the listener's readability reader
/// answers hung or ghosted is dropped here, as it arrives, with no marker
/// sent and nothing queued, as NVDA's UIA handlers drop it
/// (`_shouldSkipEventForHungWindow`, `UIAHandler/utils.py` 438 to 455); a
/// sender with no cached window is not tested. Makes no cross-process
/// call: the test is local calls.
fn queue_for_reading(
    outgoing: &Arc<Outgoing>,
    readers: &Readers<UiaFocusReading>,
    sender: &Cached<ListenerEvent>,
    fields: &ListenerEvent,
    kind: FocusEvent,
    fact: Option<DeliveredFact>,
) {
    let Some(pid) = outgoing.uia_event_process(fields.process_id) else {
        return;
    };
    if readers.window_hung(fields.role.hwnd) {
        tracing::debug!(%pid, ?kind, hwnd = fields.role.hwnd, "UIA event dropped as it arrived: its window is hung");
        return;
    }
    let meta = outgoing.received(kind);
    let runtime_id = sender.runtime_id();
    // Every event's arrival, by its marker, whatever its read makes of it,
    // so a delay before the listener received it shows.
    tracing::debug!(%pid, marker = meta.marker, ?kind, ?runtime_id, "UIA focus event received");
    // The marker goes first, so the read or the drop can only follow it.
    outgoing.focus_read(
        pid,
        DeliveredFact::UiaFocusMarker {
            marker: meta.marker,
        },
        meta,
    );
    match sender.agile() {
        Ok(agile) => readers.push(
            pid,
            meta,
            (agile, fields.role.hwnd, runtime_id, kind, fact),
            MarkerOwed::new(outgoing, pid, meta),
        ),
        Err(error) => {
            tracing::warn!(%error, "a UIA focus's element could not be kept for its read");
            // Its drop resolves its marker.
            outgoing.focus_read(
                pid,
                Judged::Dropped("its element could not be kept for its read")
                    .into_fact(meta.marker, kind),
                meta,
            );
        }
    }
}

/// Installs the desktop-wide UIA subscriptions, some of those NVDA
/// registers globally on Windows 11 (the module's documentation lists the
/// rest): `focus` ([`focus_subscription`]) on the registration's
/// client, then an element selected, `menu` ([`menu_subscription`]), and
/// notifications as
/// one event handler group on the desktop's root, all on one client as
/// NVDA's global handlers are (`verbatim_uia::Subscription::Focus` says why
/// they must share it), each part shaped as `verbatim_uia::ListenerEvent`:
/// NVDA's base cache request, with the two properties that refine a role
/// for dialogs and toggle buttons. The facts carry the
/// element's process, window, runtime id, role, and name from that cache;
/// anything else in their snapshots is not read here and is left empty,
/// but for an element selected, whose part is shaped as
/// `verbatim_uia::ListenerSelection`, which also carries the item's value,
/// states, and details. A failure is reported as a fault and leaves the
/// MSAA path running on its own.
fn install_desktop_subscriptions(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
    focus: Subscription<Cached<ListenerEvent>>,
    menu: Subscription<Cached<ListenerEvent>>,
) -> Option<Registration> {
    let selection = selection_subscription(outgoing, readers);
    let notifications = notification_subscription(outgoing, readers);
    // NVDA's base cache request, as its global group carries, with what
    // refines a role (`ListenerEvent`): UIA fills only these in the
    // application for each event, and for an element selected also what
    // Core speaks of it (`ListenerSelection`), so the outpost reports the
    // item with no call.
    let parts = vec![
        Part::shaped(focus),
        Part::shaped::<ListenerSelection>(selection),
        Part::shaped(menu),
        Part::shaped(notifications),
    ];
    match Registration::with_parts(parts, Scope::Desktop) {
        Ok(registration) => Some(registration),
        Err(error) => {
            outgoing.fault(format!(
                "listener desktop-wide UIA subscriptions failed: {error}"
            ));
            None
        }
    }
}

/// The desktop-wide subscription to UIA element selected events. A
/// selection goes through its application's reading thread, behind a
/// marker, for NVDA's checks of its sender's nearest window
/// (`FocusReader::check_fact`), in the order it came with the focus
/// events, unless its sender's cached window is hung as it arrives
/// ([`queue_for_reading`]).
fn selection_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
) -> Subscription<Cached<ListenerSelection>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    Subscription::Event {
        event: UIA_SelectionItem_ElementSelectedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerSelection>| {
            let fields = sender.read();
            if let Some((hwnd, snapshot)) = capture_selection(sender, &fields) {
                let fact = DeliveredFact::UiaSelection {
                    hwnd,
                    focus_window: 0,
                    snapshot,
                };
                queue_for_reading(
                    &outgoing,
                    &readers,
                    &sender.narrow(),
                    &fields.event,
                    FocusEvent::Selection,
                    Some(fact),
                );
            }
        }),
    }
}

/// The desktop-wide subscription to UIA notifications, which go through
/// their application's reading thread as a selection does
/// ([`selection_subscription`]).
fn notification_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    Subscription::Notifications {
        callback: Arc::new(
            move |sender: &Cached<ListenerEvent>,
                  kind: NotificationKind,
                  processing: NotificationProcessing,
                  display_string: Option<String>,
                  activity_id: Option<String>| {
                let fields = sender.read();
                if ignores_notifications(
                    fields.role.class_name.as_deref(),
                    fields.automation_id.as_deref(),
                ) {
                    return;
                }
                if let Some((hwnd, snapshot)) = capture(sender, &fields) {
                    let notification = Notification {
                        kind: notification_kind_from_uia(kind),
                        processing: notification_processing_from_uia(processing),
                        display_string,
                        activity_id,
                    };
                    queue_for_reading(
                        &outgoing,
                        &readers,
                        sender,
                        &fields,
                        FocusEvent::Notification,
                        Some(DeliveredFact::UiaNotification {
                            hwnd,
                            snapshot,
                            notification,
                        }),
                    );
                }
            },
        ),
    }
}

/// Forwards one global MSAA `WinEvent` as a fact for the process
/// [`routed_process`] names, with the thread that raised it and when, unless
/// that process's outpost has its own hook in and took it in itself
/// ([`Outgoing::fact`]). Drops a null window or a pid-zero owner; the raw
/// object address is forwarded untouched, and the app outpost does the
/// acquisition. Returns whether the event's window is a console host
/// window, by the class read to route it, `false` for an event dropped
/// before that read, whose window is null or has no owner; for an MSAA
/// focus routed to a process, that process; and when the event was
/// observed.
fn forward_msaa_event(outgoing: &Outgoing, event: WinEvent) -> Forwarded {
    // The observation, before the callback's local reads, which the
    // listener's handling stage then holds.
    let observed_at_us = outgoing.now();
    let forwarded = |console, focus_of| Forwarded {
        console,
        focus_of,
        observed_at_us,
    };
    let WinEvent {
        kind,
        hwnd,
        id_object,
        id_child,
        thread,
        time,
        raised_ms_ago,
    } = event;
    if matches!(kind, WinEventKind::MenuEnd | WinEventKind::SwitchEnd) {
        // Not routed to an outpost: if no focus event follows, Core reads
        // the focus of whichever application is then in front. NVDA accepts
        // these from any window, even an invalid one.
        outgoing.menu_or_switch_ended(observed_at_us);
        return forwarded(false, None);
    }
    // NVDA takes a null window as the desktop window
    // (`internalWinEventHandler.py` 113 to 129), whose process is the
    // client server, whose objects NVDA's focused-state test refuses
    // (`shouldAllowIAccessibleFocusEvent` on the desktop window's object)
    // and Verbatim cannot open: dropping it here is the same outcome.
    if hwnd == 0 {
        return forwarded(false, None);
    }
    let owner = window_pid(hwnd);
    if owner == 0 {
        return forwarded(false, None);
    }
    let class = window_class_name(hwnd);
    let console = class == CONSOLE_WINDOW_CLASS;
    let Some(pid) = routed_process(&class, owner, || process_of_thread(thread)) else {
        log_unread_console_once(hwnd, thread);
        return forwarded(console, None);
    };
    if pid != owner {
        if outgoing.ignored.contains(Pid(owner)) {
            // The console of a process Verbatim ignores, whose events were
            // dropped by its owner before they went to its console host.
            return forwarded(console, None);
        }
        tracing::debug!(
            hwnd,
            ?kind,
            console_host = pid,
            client = owner,
            "a console window's event goes to its console host"
        );
    }
    // Every kind the listener subscribes to (LISTENER_SUBSCRIPTIONS) that
    // reaches here is a fact: the console's and the shell's foreground
    // events are handled before this.
    let Some(fact) = verbatim_ia2::fact_of_win_event(kind, (hwnd, id_object, id_child)) else {
        return forwarded(console, None);
    };
    outgoing.fact(
        Pid(pid),
        fact,
        Some(raised_ms_ago),
        Some(WinEventStamp { thread, time }),
        observed_at_us,
    );
    let focus_of =
        (kind == WinEventKind::Focus && !outgoing.ignored.contains(Pid(pid))).then_some(Pid(pid));
    forwarded(console, focus_of)
}

/// What [`forward_msaa_event`] found of an event it forwarded or dropped.
#[derive(Clone, Copy)]
struct Forwarded {
    /// Whether the event's window is a console host window.
    console: bool,
    /// For an MSAA focus routed to a process, that process.
    focus_of: Option<Pid>,
    /// When the listener's callback observed the event, in microseconds on
    /// the run's shared clock.
    observed_at_us: u64,
}

/// The listener's nudge that starts the watch of the window in front
/// ([`ForegroundWatch::start`]), in the nudge's second word; the idle
/// barrier's carry 0.
const NUDGE_FOREGROUND_START: isize = 1;

/// The system's foreground window now, read locally; 0 for none.
fn front_window() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: a local read with no preconditions.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Whether `window` is the system's foreground window now, the test a
/// foreground event's window passes before it is sent, a local call.
fn in_front(window: isize) -> bool {
    front_window() == window
}

/// The window in the foreground now, with the thread Windows names as its
/// owner, read locally and observed at `observed_at_us`; `None` when no
/// window is in front.
fn front_now(observed_at_us: u64) -> Option<Front> {
    let window = front_window();
    if window == 0 {
        return None;
    }
    Some(Front {
        window,
        thread: verbatim_win32::window::window_owner(window).0,
        console: window_class_name(window) == CONSOLE_WINDOW_CLASS,
        observed_at_us,
    })
}

/// Carries out a step of the foreground's watch, on the `WinEvent` thread:
/// sends Core the window that came to the foreground, with its root owner,
/// its process and that process's integrity level read locally
/// ([`ListenerToCore::Foreground`]), and hooks or unhooks every console's
/// events.
fn foreground_step(outgoing: &Outgoing, step: Step) {
    if let Some(front) = step.report {
        outgoing.foreground(front);
    }
    if let Some(hooked) = step.console_events {
        let suspended: &[WinEventKind] = if hooked {
            &[]
        } else {
            &[WinEventKind::ConsoleUpdate, WinEventKind::ConsoleCaret]
        };
        if let Err(error) = verbatim_ia2::suspend_kinds(suspended) {
            tracing::warn!(
                error,
                hooked,
                "the consoles' events could not be hooked or unhooked"
            );
        }
    }
}

/// The owning process id of `hwnd`, or 0 for an invalid or ownerless window.
fn window_pid(hwnd: isize) -> u32 {
    verbatim_win32::window::window_owner(hwnd).1
}

/// The process whose outpost a `WinEvent` goes to, for a window of class
/// `class` that `GetWindowThreadProcessId` says `owner` owns. For a console
/// window it is the console host, the process of the thread that raised
/// the event, which `host` reads: Windows names the console's client (such
/// as `powershell.exe`) as a console window's owner, but the window, its
/// UIA provider and its text area are the console host's, and a `WinEvent`
/// always names the window's real thread (NVDA's
/// `consoleWindowsToThreadIDs`, `IAccessibleHandler/internalWinEventHandler.py`
/// 266 to 273; `UIA._get_processID`, `NVDAObjects/UIA/__init__.py` 2336 to
/// 2346). Every event for a console window then reaches the outpost that
/// receives its UIA events, which reports the client as the application
/// (`books/dev/src/explanation/design/focus-pipeline.md`, section 5.4). `None` when the host
/// cannot be read: the event is dropped rather than sent to the client,
/// whose outpost would otherwise take the console host's window as its
/// application's (Dickson, 2026-10-19;
/// `books/dev/src/explanation/design/listener-without-connections.md`, section 10, step 3).
fn routed_process(class: &str, owner: u32, host: impl FnOnce() -> Option<u32>) -> Option<u32> {
    if class == CONSOLE_WINDOW_CLASS {
        host()
    } else {
        Some(owner)
    }
}

/// Logs, at debug and once per window, that the events of the console
/// window `hwnd` are dropped because the process of its thread `thread`,
/// its console host, cannot be read. Called only on the `WinEvent` thread.
fn log_unread_console_once(hwnd: isize, thread: u32) {
    thread_local! {
        static LOGGED: std::cell::RefCell<std::collections::HashSet<isize>> =
            std::cell::RefCell::default();
    }
    if LOGGED.with_borrow_mut(|logged| logged.insert(hwnd)) {
        tracing::debug!(
            hwnd,
            thread,
            "a console window's host cannot be read; its events are dropped"
        );
    }
}

/// The process `thread` belongs to, read with `GetProcessIdOfThread`, a
/// local call; `None` when the thread cannot be opened, such as one that
/// has ended.
fn process_of_thread(thread: u32) -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenThread has no preconditions; the handle is owned here and
    // closed below.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) }.ok()?;
    // SAFETY: `handle` is an open thread handle with the access the call
    // needs.
    let pid = unsafe { GetProcessIdOfThread(handle) };
    // SAFETY: `handle` was opened above and is closed once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    (pid != 0).then_some(pid)
}

/// Runs the focus listener on the transport's child side
/// (`docs/design/communication.md`, sections 4.6 and 6.3): the calling
/// thread becomes its I/O thread, which reads Core's commands from
/// `pipe_in` and finishes the writes to `pipe_out` that pend, while every
/// thread that produces a fact writes it itself. Returns once Core asks it
/// to shut down or its command pipe ends, after a clean shutdown that
/// waits for every write in flight. Both ends must be overlapped.
/// `ignored` names the processes whose facts are dropped, `hooks_in` the
/// applications whose outposts have their own hooks in.
///
/// # Errors
///
/// Returns the error of setting up the pipes or the listener, or any I/O
/// error reading the command stream, after the shutdown.
pub fn run_listener(
    pipe_in: OwnedHandle,
    pipe_out: OwnedHandle,
    ignored: IgnoredProcesses,
    hooks_in: &[(Pid, OutpostId)],
) -> io::Result<()> {
    let (mut io, sender) = ChildIo::new(pipe_in, pipe_out, Waiting::default())?;
    let listener = Listener::new(sender.clone(), ignored, hooks_in)?;
    // This thread is the listener's I/O thread from here on.
    prioritize_role("listener I/O", IO_PRIORITY);
    let served = io.serve(&sender, |body| {
        // The command's receipt, one clock read, for the hop from
        // verbatim.exe's write.
        let received_at_us = now_us();
        match decode_message::<CoreToListener>(body)? {
            CoreToListener::Shutdown => return Ok(Serving::Stop),
            command => listener.handle_command(&command, received_at_us),
        }
        Ok(Serving::Continue)
    });
    let ended = match served {
        Ok(Served::Stopped) => {
            tracing::info!("shutting down, as Core asked");
            Ok(())
        }
        Ok(Served::CommandsEnded) => {
            tracing::warn!("Core's command pipe closed; shutting down");
            Ok(())
        }
        Err(error) => Err(error),
    };
    listener.shutdown(&mut io);
    // The process has finished with UIA: only its exit gives back the
    // process-wide hold ([`verbatim_uia::release_mta_usage`]).
    verbatim_uia::release_mta_usage();
    ended
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::mpsc;

    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::EVENT_PIPE_BUFFER;

    use super::*;
    use verbatim_process::message::read_message;

    /// An `Outgoing` writing to a pipe with a buffer of `buffer` bytes, its
    /// writes that pend finished by a thread of the sender's own, as the
    /// I/O thread finishes them, dropping the facts of the processes
    /// `ignored` holds; and the pipe's other end, which the test reads.
    fn outgoing_with(buffer: u32, ignored: IgnoredProcesses) -> (Outgoing, BufReader<File>) {
        let (ours, theirs) = pipe(Flow::Outbound, buffer, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        (
            Outgoing::new(sender, ignored).expect("the listener's sending side"),
            BufReader::new(File::from(theirs)),
        )
    }

    fn outgoing() -> (Outgoing, BufReader<File>) {
        outgoing_with(EVENT_PIPE_BUFFER, IgnoredProcesses::default())
    }

    /// A menu end's grace the test runs by hand: the clock it reads, in
    /// microseconds, which the test moves, and every time its timer was
    /// set for, which no timer acts on, so the test says when it goes off
    /// ([`MenuEnd::grace_ended`]).
    struct ManualGrace {
        now: Arc<AtomicU64>,
        set: Arc<Mutex<Vec<u64>>>,
    }

    impl ManualGrace {
        /// Moves the clock to `now`.
        fn at(&self, now: u64) {
            self.now.store(now, Ordering::Relaxed);
        }

        /// Every time the timer was set for, in microseconds, in order.
        fn set(&self) -> Vec<u64> {
            self.set
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// [`outgoing_with`], its grace run by hand from a clock at 0.
    fn outgoing_with_manual_grace(buffer: u32) -> (Outgoing, BufReader<File>, ManualGrace) {
        let (ours, theirs) = pipe(Flow::Outbound, buffer, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        let grace = ManualGrace {
            now: Arc::new(AtomicU64::new(0)),
            set: Arc::new(Mutex::new(Vec::new())),
        };
        let (now, set) = (Arc::clone(&grace.now), Arc::clone(&grace.set));
        let outgoing = Outgoing::with_manual_grace(
            sender,
            IgnoredProcesses::default(),
            Box::new(move || now.load(Ordering::Relaxed)),
            Box::new(move |after| {
                set.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(u64::try_from(after.as_micros()).unwrap_or(u64::MAX));
            }),
        );
        (outgoing, BufReader::new(File::from(theirs)), grace)
    }

    /// The facts by process and the menu ends `messages` holds, in order.
    fn facts_and_ends(messages: Vec<ListenerToCore>) -> Vec<String> {
        messages
            .into_iter()
            .map(|message| match message {
                ListenerToCore::FocusFact { fact, .. } => format!("fact {}", fact.pid.0),
                ListenerToCore::MenuOrSwitchEnded { .. } => "menu end".to_owned(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// Everything `outgoing` sent, which is closed first, as Core reads it.
    fn sent(outgoing: &Outgoing, reader: &mut BufReader<File>) -> Vec<ListenerToCore> {
        outgoing.close();
        std::iter::from_fn(|| read_message(reader).expect("read")).collect()
    }

    /// The stamp of an MSAA event raised at `time`.
    fn raised_at(time: u32) -> WinEventStamp {
        WinEventStamp { thread: 9, time }
    }

    #[test]
    fn a_win_event_raised_after_the_outposts_hook_is_in_is_not_forwarded() {
        let (outgoing, mut reader) = outgoing();
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(90)), 1_000);
        outgoing.hook_installed(Pid(5), OutpostId(3), 100);
        // Raised after the tick: the outpost's hook took it in.
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(101)), 1_000);
        // Raised at the tick, and reaching the listener late: forwarded,
        // for the outpost to match by counting.
        outgoing.fact(
            Pid(5),
            DeliveredFact::Foreground { hwnd: 1 },
            None,
            Some(raised_at(100)),
            1_000,
        );
        // Another application's, and a UIA fact: forwarded.
        outgoing.fact(Pid(6), msaa_focus(), None, Some(raised_at(101)), 1_000);
        outgoing.fact(
            Pid(5),
            DeliveredFact::UiaSelection {
                hwnd: 1,
                focus_window: 0,
                snapshot: uia_snapshot(),
            },
            None,
            None,
            1_000,
        );
        let kinds: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::FocusFact {
                    fact, win_event, ..
                } => {
                    format!("{} {:?}", fact.pid.0, fact.fact.key(win_event))
                }
                ListenerToCore::ForwardingEnded {
                    pid,
                    outpost,
                    through,
                } => format!("ended {} {} {through}", pid.0, outpost.0),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "5 Some(MsaaFocus(1, -4, 0, 9))",
                "ended 5 3 100",
                "5 Some(Foreground(1, 9))",
                "6 Some(MsaaFocus(1, -4, 0, 9))",
                "5 Some(UiaSelection([42]))",
            ],
            "the fact sent first goes ahead of forwarding ended"
        );
    }

    #[test]
    fn a_focus_reads_marker_read_and_drop_are_never_replaced() {
        let (outgoing, mut reader) = outgoing();
        let meta = |marker| Meta {
            marker,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            timing: EventTiming::default(),
        };
        // Two focus events on one element, the first read, the second
        // dropped: an outpost waits on each marker, so all four are sent,
        // in order.
        outgoing.focus_read(Pid(5), DeliveredFact::UiaFocusMarker { marker: 1 }, meta(1));
        outgoing.focus_read(Pid(5), DeliveredFact::UiaFocusMarker { marker: 2 }, meta(2));
        outgoing.focus_read(
            Pid(5),
            Judged::Dropped("read").into_fact(1, FocusEvent::Focus),
            meta(1),
        );
        outgoing.focus_read(
            Pid(5),
            DeliveredFact::UiaFocusDropped {
                marker: 2,
                focus: true,
            },
            meta(2),
        );
        let facts: Vec<DeliveredFact> = sent(&outgoing, &mut reader)
            .into_iter()
            .filter_map(|message| match message {
                ListenerToCore::FocusFact { fact, .. } => Some(fact.fact),
                _ => None,
            })
            .collect();
        assert_eq!(
            facts,
            [
                DeliveredFact::UiaFocusMarker { marker: 1 },
                DeliveredFact::UiaFocusMarker { marker: 2 },
                DeliveredFact::UiaFocusDropped {
                    marker: 1,
                    focus: true,
                },
                DeliveredFact::UiaFocusDropped {
                    marker: 2,
                    focus: true,
                },
            ]
        );
    }

    #[test]
    fn waiting_facts_are_coalesced_one_per_element_and_kind_behind_a_write_in_flight() {
        // A pipe with a 4 KiB buffer, so a large fact's write pends until
        // the test reads, and the facts sent meanwhile wait.
        let (outgoing, mut reader) = outgoing_with(4096, IgnoredProcesses::default());
        let mut large = uia_snapshot();
        large.name = Some("x".repeat(20_000));
        outgoing.fact(
            Pid(7),
            DeliveredFact::UiaSelection {
                hwnd: 1,
                focus_window: 0,
                snapshot: large,
            },
            None,
            None,
            1_000,
        );
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        outgoing.fact(Pid(6), msaa_focus(), None, None, 1_000);
        // Replaces the waiting one for the same element and kind, and goes
        // to the back.
        outgoing.fact(Pid(5), msaa_focus(), None, None, 2_000);
        outgoing.urgent(ListenerToCore::Pong {
            seq: 4,
            parked_count: 0,
            written_at_us: 0,
        });
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::FocusFact { fact, timing, .. } => {
                    format!("{} {}", fact.pid.0, timing.observed_at_us)
                }
                ListenerToCore::Pong { seq, .. } => format!("pong {seq}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(order, ["7 1000", "pong 4", "6 1000", "5 2000"]);
    }

    #[test]
    fn a_win_event_is_forwarded_again_once_the_hook_is_removed() {
        let (outgoing, mut reader) = outgoing();
        outgoing.hook_installed(Pid(5), OutpostId(3), 100);
        outgoing.hook_removed(Pid(5));
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(101)), 1_000);
        let sent = sent(&outgoing, &mut reader);
        assert!(
            matches!(
                sent.as_slice(),
                [
                    ListenerToCore::ForwardingEnded { .. },
                    ListenerToCore::FocusFact { .. }
                ]
            ),
            "{sent:?}"
        );
    }

    fn uia_snapshot() -> UiaSnapshotFact {
        UiaSnapshotFact {
            runtime_id: vec![42],
            role: verbatim_model::Role::ListItem,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        }
    }

    /// Reads the messages `reader` brings on another thread, since a read
    /// blocks while nothing has been written.
    fn drain(mut reader: BufReader<File>) -> mpsc::Receiver<ListenerToCore> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut reader) {
                if tx.send(message).is_err() {
                    return;
                }
            }
        });
        rx
    }

    fn msaa_focus() -> DeliveredFact {
        DeliveredFact::MsaaFocus {
            hwnd: 1,
            id_object: -4,
            id_child: 0,
        }
    }

    #[test]
    fn a_console_windows_event_goes_to_the_process_of_its_thread() {
        // Windows names the client, 20, as the console window's owner; the
        // event's thread is the console host's, in 30.
        assert_eq!(
            routed_process(CONSOLE_WINDOW_CLASS, 20, || Some(30)),
            Some(30)
        );
    }

    #[test]
    fn a_console_windows_event_whose_thread_cannot_be_read_goes_nowhere() {
        assert_eq!(routed_process(CONSOLE_WINDOW_CLASS, 20, || None), None);
    }

    #[test]
    fn any_other_windows_event_goes_to_its_owner_without_reading_its_thread() {
        assert_eq!(
            routed_process("Notepad", 20, || panic!("the thread is not read")),
            Some(20)
        );
    }

    #[test]
    fn a_threads_process_is_read_from_the_thread() {
        // SAFETY: GetCurrentThreadId has no preconditions.
        let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        assert_eq!(process_of_thread(thread), Some(std::process::id()));
    }

    #[test]
    fn a_fact_from_an_ignored_process_is_never_sent() {
        // This process stands for the owner's Windows Terminal.
        let own = Pid(std::process::id());
        let (outgoing, mut reader) =
            outgoing_with(EVENT_PIPE_BUFFER, IgnoredProcesses::hold([own]));
        outgoing.fact(
            own,
            DeliveredFact::Foreground { hwnd: 1 },
            None,
            None,
            1_000,
        );
        outgoing.fact(own, msaa_focus(), None, None, 1_000);
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        let sent = sent(&outgoing, &mut reader);
        let [ListenerToCore::FocusFact { fact, .. }] = sent.as_slice() else {
            panic!("one fact is sent, the other process's: {sent:?}");
        };
        assert_eq!(fact.pid, Pid(5));
    }

    /// The desktop window, which the desktop's root element has as its
    /// own window.
    fn desktop_window() -> isize {
        // SAFETY: GetDesktopWindow has no preconditions.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize
    }

    /// The desktop's root element as the listener's registration delivers
    /// an event's sender of shape `S`: with its own window, the desktop
    /// window, and its process cached.
    fn desktop_sender<S: verbatim_uia::Shape + verbatim_uia::Ungated>(
        uia: &verbatim_uia::Uia,
    ) -> Cached<S> {
        let request = uia
            .request_fixed::<S>()
            .expect("the listener's cache request");
        uia.any_window_element::<S>(desktop_window(), &request)
            .expect("the desktop's root element")
    }

    /// The listener's sending side, taking UIA events, its pipe read on a
    /// thread of its own, and its readers, whose readability reader
    /// answers the `n`th window it is asked, counted from 0, as `answer`
    /// says, each window asked kept in `asked`.
    fn listening(
        answer: fn(usize) -> Readability,
        asked: &Arc<Mutex<Vec<isize>>>,
    ) -> (
        Arc<Outgoing>,
        Arc<Readers<UiaFocusReading>>,
        mpsc::Receiver<ListenerToCore>,
    ) {
        let (outgoing, reader) = outgoing();
        outgoing.uia_installation_ended();
        let asked = Arc::clone(asked);
        let readability: WindowReadabilityReader = Arc::new(move |window| {
            let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
            asked.push(window);
            answer(asked.len() - 1)
        });
        let readers = Arc::new(Readers::new(
            UiaFocusReading { readability },
            verbatim_model::FOCUS_READ_DEADLINE,
        ));
        (Arc::new(outgoing), readers, drain(reader))
    }

    /// The facts the listener sent once its readers are stopped and let go,
    /// each event still queued then answering its marker, and its sending
    /// side closed: every fact it sent, in order.
    fn facts_once_stopped(
        outgoing: &Outgoing,
        readers: Arc<Readers<UiaFocusReading>>,
        sent: &mpsc::Receiver<ListenerToCore>,
    ) -> Vec<DeliveredFact> {
        readers.stop();
        drop(readers);
        outgoing.close();
        sent.iter()
            .filter_map(|message| match message {
                ListenerToCore::FocusFact { fact, .. } => Some(fact.fact),
                _ => None,
            })
            .collect()
    }

    /// A UIA focus whose sender's cached window the listener's reader
    /// answers hung as it arrives is dropped in the callback: no marker is
    /// sent and nothing is queued, so nothing is sent once the readers stop,
    /// and the reader was asked of the sender's window once.
    #[test]
    fn a_focus_whose_window_is_hung_as_it_arrives_is_dropped_in_the_callback() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        assert!(
            sender.read().process_id.is_some_and(|pid| pid != 0),
            "the sender has a process, so only its window can drop it"
        );
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (outgoing, readers, sent) = listening(|_| Readability::Hung, &asked);
        let Subscription::Focus { callback } = focus_subscription(&outgoing, &readers) else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        assert_eq!(facts_once_stopped(&outgoing, readers, &sent), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()]
        );
    }

    /// A UIA focus whose sender's window hangs after it arrives is queued
    /// behind its marker and read with no second test of its window: its
    /// reading thread tests no window before its calls, so the reader is
    /// asked once, as the focus arrived, and the read's outcome, here its
    /// drop, takes the marker's place.
    #[test]
    fn a_focus_whose_window_hangs_after_it_arrives_is_read() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (outgoing, readers, sent) = listening(
            |nth| {
                if nth == 0 {
                    Readability::Readable
                } else {
                    Readability::Hung
                }
            },
            &asked,
        );
        let Subscription::Focus { callback } = focus_subscription(&outgoing, &readers) else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let fact = |message: ListenerToCore| match message {
            ListenerToCore::FocusFact { fact, .. } => fact.fact,
            other => panic!("the listener sent {other:?}, not a fact"),
        };
        let timeout = Duration::from_secs(10);
        let marker = sent.recv_timeout(timeout).map(fact).expect("the marker");
        let outcome = sent
            .recv_timeout(timeout)
            .map(fact)
            .expect("the read's outcome");
        assert_eq!(
            [marker, outcome],
            [
                DeliveredFact::UiaFocusMarker { marker: 1 },
                DeliveredFact::UiaFocusDropped {
                    marker: 1,
                    focus: true,
                },
            ]
        );
        assert_eq!(facts_once_stopped(&outgoing, readers, &sent), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()],
            "asked as it arrived, and not by its read"
        );
    }

    /// A selection and a notification whose sender's cached window the
    /// listener's reader answers hung as they arrive are dropped in their
    /// callbacks: no marker is sent and nothing is queued.
    #[test]
    fn a_selection_and_a_notification_whose_window_is_hung_are_dropped_in_their_callbacks() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let selected = desktop_sender::<ListenerSelection>(&uia);
        let notifying = desktop_sender::<ListenerEvent>(&uia);
        assert!(
            capture_selection(&selected, &selected.read()).is_some()
                && capture(&notifying, &notifying.read()).is_some(),
            "both are captured, so only their window can drop them"
        );
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (outgoing, readers, sent) = listening(|_| Readability::Hung, &asked);
        let Subscription::Event { callback, .. } = selection_subscription(&outgoing, &readers)
        else {
            unreachable!("the selection subscription");
        };
        callback(&selected);
        drop(callback);
        let Subscription::Notifications { callback } =
            notification_subscription(&outgoing, &readers)
        else {
            unreachable!("the notification subscription");
        };
        callback(
            &notifying,
            windows::Win32::UI::Accessibility::NotificationKind_Other,
            windows::Win32::UI::Accessibility::NotificationProcessing_All,
            Some("saved".to_owned()),
            None,
        );
        drop(callback);
        assert_eq!(facts_once_stopped(&outgoing, readers, &sent), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window(), desktop_window()]
        );
    }

    #[test]
    fn a_uia_event_received_before_the_subscriptions_are_installed_is_dropped() {
        let (outgoing, _reader) = outgoing();
        assert_eq!(outgoing.uia_event_process(Some(5)), None);
        outgoing.uia_installation_ended();
        assert_eq!(outgoing.uia_event_process(Some(5)), Some(Pid(5)));
    }

    /// The listener says `Ready` only once its UIA subscriptions'
    /// installation returns, as on a foreground application that does not
    /// answer, with its hooks in during the wait: an MSAA event raised
    /// meanwhile is forwarded with no `Ready` before it, UIA events are
    /// dropped, and `Ready` arrives once the installation is released.
    #[test]
    fn the_listener_is_ready_once_its_uia_subscriptions_are_installed() {
        use windows::Win32::UI::Accessibility::NotifyWinEvent;
        use windows::Win32::UI::WindowsAndMessaging::{
            CHILDID_SELF, CreateWindowExW, DestroyWindow, EVENT_SYSTEM_ALERT, OBJID_CLIENT,
            WINDOW_EX_STYLE, WS_OVERLAPPED,
        };
        use windows::core::w;

        let (ours, theirs) =
            pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        let mut reader = BufReader::new(File::from(theirs));
        let (release, released) = mpsc::channel::<()>();
        let install: InstallSubscriptions = Box::new(move |_, _| {
            // Released once the test has read the alert below; the bound
            // only ends a run whose alert is never forwarded, which then
            // fails on the `Ready` that follows.
            let _ = released.recv_timeout(Duration::from_secs(10));
            None
        });
        let listener = Listener::start(sender, IgnoredProcesses::default(), &[], install)
            .expect("the listener starts");

        // A hidden window of this thread's, with no parent, to raise an
        // alert from while the installation waits.
        // SAFETY: a window with no parent, made on this thread.
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
        .expect("a window to raise an alert from");
        // SAFETY: raising an event for a live window of this thread.
        unsafe {
            NotifyWinEvent(
                EVENT_SYSTEM_ALERT,
                window,
                OBJID_CLIENT.0,
                CHILDID_SELF.cast_signed(),
            );
        };
        let alert = ListenerFact {
            pid: Pid(std::process::id()),
            fact: DeliveredFact::Alert {
                hwnd: window.0 as isize,
                id_object: OBJID_CLIENT.0,
                id_child: CHILDID_SELF.cast_signed(),
            },
        };
        // The global hooks forward this desktop's events too: the first of
        // the alert and `Ready` to arrive, `Ready` as `None`.
        let first =
            std::iter::from_fn(|| read_message(&mut reader).expect("read")).find_map(|message| {
                match message {
                    ListenerToCore::Ready { .. } => Some(None),
                    ListenerToCore::FocusFact { fact, .. } if fact == alert => Some(Some(fact)),
                    _ => None,
                }
            });
        assert_eq!(
            first,
            Some(Some(alert)),
            "the alert, with no Ready before it"
        );
        assert_eq!(listener.outgoing.uia_event_process(Some(5)), None);

        release.send(()).expect("the installation waits");
        let ready = std::iter::from_fn(|| read_message(&mut reader).expect("read"))
            .find(|message| matches!(message, ListenerToCore::Ready { .. }));
        assert_eq!(
            ready,
            Some(ListenerToCore::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(0),
            })
        );
        assert_eq!(listener.outgoing.uia_event_process(Some(5)), Some(Pid(5)));

        let Listener {
            outgoing,
            readers,
            registering,
            _event_thread: event_thread,
            ..
        } = listener;
        drop(event_thread);
        assert!(registering.join().expect("the installation ends").is_none());
        readers.stop();
        outgoing.close();
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }

    /// A menu end no focus follows is sent when its timer goes off at the
    /// end of its grace, and not when it goes off a microsecond before,
    /// which sets the timer again for the microsecond left.
    #[test]
    fn a_menu_end_no_focus_follows_reaches_core_after_the_grace() {
        let (outgoing, mut reader, grace) = outgoing_with_manual_grace(EVENT_PIPE_BUFFER);
        outgoing.menu_or_switch_ended(1_000);
        grace.at(49_999);
        outgoing.menu_end.grace_ended();
        grace.at(50_000);
        outgoing.menu_end.grace_ended();
        assert_eq!(grace.set(), [50_000, 1], "the timer set for the grace");
        assert_eq!(facts_and_ends(sent(&outgoing, &mut reader)), ["menu end"]);
    }

    /// A grace timer's callback that runs as a newer end comes, taking the
    /// lock only once that end is noted, sends nothing and sets the timer
    /// for the whole grace again: the newer end is sent once its own grace
    /// is over, after a fact sent meanwhile, and only once.
    #[test]
    fn a_menu_end_is_not_sent_before_its_grace_by_a_callback_of_an_earlier_one() {
        let (outgoing, mut reader, grace) = outgoing_with_manual_grace(EVENT_PIPE_BUFFER);
        outgoing.menu_or_switch_ended(1_000);
        // As an earlier grace's callback does, run just as this end came.
        outgoing.menu_end.grace_ended();
        outgoing.fact(Pid(6), msaa_focus(), None, None, 1_000);
        // As the timer does once the grace is over.
        grace.at(50_000);
        outgoing.menu_end.grace_ended();
        assert_eq!(grace.set(), [50_000, 50_000], "the timer set for the grace");
        assert_eq!(
            facts_and_ends(sent(&outgoing, &mut reader)),
            ["fact 6", "menu end"]
        );
    }

    #[test]
    fn a_menu_end_a_focus_follows_is_sent_after_the_focus() {
        // Whether the focus was usable is Core's to judge: it may yet be
        // dropped or fail to read.
        let (outgoing, mut reader, grace) = outgoing_with_manual_grace(EVENT_PIPE_BUFFER);
        outgoing.menu_or_switch_ended(1_000);
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        // As the timer does once the grace is over.
        grace.at(50_000);
        outgoing.menu_end.grace_ended();
        assert_eq!(grace.set(), [50_000], "the timer set for the grace");
        assert_eq!(
            facts_and_ends(sent(&outgoing, &mut reader)),
            ["fact 5", "menu end"]
        );
    }

    #[test]
    fn a_menu_end_whose_grace_ends_behind_a_write_in_flight_goes_after_every_waiting_fact() {
        // The grace timer's callback sends the end through the sender the
        // facts go through, so it waits behind the write in flight, and
        // goes after every fact waiting when it is written, one sent after
        // it included.
        let (outgoing, mut reader, _grace) = outgoing_with_manual_grace(4096);
        let mut large = uia_snapshot();
        large.name = Some("x".repeat(20_000));
        outgoing.fact(
            Pid(7),
            DeliveredFact::UiaSelection {
                hwnd: 1,
                focus_window: 0,
                snapshot: large,
            },
            None,
            None,
            1_000,
        );
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        outgoing.note_menu_end(1_000, Duration::ZERO);
        // As the timer's callback does when the grace ends.
        outgoing.menu_end.grace_ended();
        outgoing.fact(Pid(6), msaa_focus(), None, None, 1_000);
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::FocusFact { fact, .. } => format!("fact {}", fact.pid.0),
                ListenerToCore::MenuOrSwitchEnded { .. } => "menu end".to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(order, ["fact 7", "fact 5", "fact 6", "menu end"]);
    }

    /// The idle barrier's answer, sent behind a write in flight: it goes
    /// after every fact waiting before it, a pong sent after it going
    /// ahead of both; a newer fact for an element whose fact waits before
    /// it is not coalesced into that one across it; and an answer sent
    /// while a menu end waits for its grace goes after the end.
    #[test]
    fn the_idle_answer_waits_behind_every_fact_and_menu_end_before_it() {
        let (outgoing, mut reader, _grace) = outgoing_with_manual_grace(4096);
        let mut large = uia_snapshot();
        large.name = Some("x".repeat(20_000));
        outgoing.fact(
            Pid(7),
            DeliveredFact::UiaSelection {
                hwnd: 1,
                focus_window: 0,
                snapshot: large,
            },
            None,
            None,
            1_000,
        );
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        outgoing.idle_answer(1);
        outgoing.fact(Pid(5), msaa_focus(), None, None, 2_000);
        outgoing.urgent(ListenerToCore::Pong {
            seq: 4,
            parked_count: 0,
            written_at_us: 0,
        });
        outgoing.note_menu_end(3_000, Duration::ZERO);
        outgoing.idle_answer(2);
        outgoing.fact(Pid(6), msaa_focus(), None, None, 3_000);
        // As the timer's callback does when the grace ends.
        outgoing.menu_end.grace_ended();
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::FocusFact { fact, timing, .. } => {
                    format!("{} {}", fact.pid.0, timing.observed_at_us)
                }
                ListenerToCore::Pong { seq, .. } => format!("pong {seq}"),
                ListenerToCore::IdleAnswer { round } => format!("idle {round}"),
                ListenerToCore::MenuOrSwitchEnded { .. } => "menu end".to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "7 1000", "pong 4", "5 1000", "idle 1", "5 2000", "6 3000", "menu end", "idle 2"
            ]
        );
    }

    /// An `Outgoing` whose clock always reads 7,000.
    fn at_seven_thousand() -> (Outgoing, BufReader<File>) {
        let (mut outgoing, reader) = outgoing();
        outgoing.clock = Clock(|| 7_000);
        (outgoing, reader)
    }

    /// The timing of the one fact `outgoing` sends.
    fn timing_of_the_fact(outgoing: &Outgoing, reader: &mut BufReader<File>) -> EventTiming {
        let sent = sent(outgoing, reader);
        let [ListenerToCore::FocusFact { timing, .. }] = sent.as_slice() else {
            panic!("one fact is sent: {sent:?}");
        };
        *timing
    }

    #[test]
    fn a_win_events_fact_is_observed_by_its_callback_and_handled_as_it_is_sent() {
        let (outgoing, mut reader) = at_seven_thousand();
        outgoing.fact(Pid(5), msaa_focus(), None, None, 6_000);
        let timing = timing_of_the_fact(&outgoing, &mut reader);
        assert_eq!(
            (timing.observed_at_us, timing.listener_handled_at_us),
            (6_000, 7_000)
        );
    }

    /// A `WinEvent`'s fact is stamped, in microseconds, at its callback's
    /// observation, so the focus it becomes is ordered by when it happened.
    #[test]
    fn a_win_events_fact_is_stamped_at_its_callbacks_observation() {
        let (outgoing, mut reader) = at_seven_thousand();
        outgoing.fact(Pid(5), msaa_focus(), None, None, 6_543_210);
        let sent = sent(&outgoing, &mut reader);
        let [ListenerToCore::FocusFact { timing, .. }] = sent.as_slice() else {
            panic!("one fact is sent: {sent:?}");
        };
        assert_eq!(timing.observed_at_us, 6_543_210);
    }

    /// A UIA event is stamped, in microseconds, from one clock read as the
    /// callback received it, and its facts carry that stamp, so the focus
    /// it becomes is ordered by when it happened.
    #[test]
    fn a_uia_events_facts_carry_one_clock_reads_stamp() {
        static CLOCK: AtomicU64 = AtomicU64::new(6_543_900);
        let (mut outgoing, mut reader) = outgoing();
        // Each read of the clock is 200 microseconds after the last, so a
        // second read would stamp another time.
        outgoing.clock = Clock(|| CLOCK.fetch_add(200, Ordering::Relaxed));
        let meta = outgoing.received(FocusEvent::Focus);
        outgoing.focus_read(
            Pid(5),
            DeliveredFact::UiaFocusMarker {
                marker: meta.marker,
            },
            meta,
        );
        let sent = sent(&outgoing, &mut reader);
        let [ListenerToCore::FocusFact { timing, .. }] = sent.as_slice() else {
            panic!("one fact is sent: {sent:?}");
        };
        assert_eq!(timing.observed_at_us, 6_543_900);
    }

    #[test]
    fn a_marker_is_handled_as_it_is_sent() {
        let (outgoing, mut reader) = at_seven_thousand();
        let meta = Meta {
            marker: 1,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            timing: EventTiming {
                observed_at_us: 6_000,
                ..EventTiming::default()
            },
        };
        outgoing.focus_read(Pid(5), DeliveredFact::UiaFocusMarker { marker: 1 }, meta);
        assert_eq!(
            timing_of_the_fact(&outgoing, &mut reader).listener_handled_at_us,
            7_000
        );
    }

    #[test]
    fn a_reading_threads_outcome_keeps_the_threads_stamp() {
        let (outgoing, mut reader) = at_seven_thousand();
        let meta = Meta {
            marker: 1,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            timing: EventTiming {
                observed_at_us: 6_000,
                listener_handled_at_us: 6_500,
                ..EventTiming::default()
            },
        };
        outgoing.focus_read(
            Pid(5),
            DeliveredFact::UiaFocusDropped {
                marker: 1,
                focus: true,
            },
            meta,
        );
        assert_eq!(
            timing_of_the_fact(&outgoing, &mut reader).listener_handled_at_us,
            6_500
        );
    }

    /// A menu's or the switcher's end is stamped when the callback observed
    /// it, the time Core compares the focuses applied since with.
    #[test]
    fn a_menu_end_is_stamped_at_its_observation() {
        let (outgoing, reader) = at_seven_thousand();
        let messages = drain(reader);
        outgoing.menu_or_switch_ended(6_543_210);
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        let ListenerToCore::MenuOrSwitchEnded { ended_at_us, .. } = message else {
            panic!("a menu end: {message:?}");
        };
        assert_eq!(ended_at_us, 6_543_210);
    }

    #[test]
    fn a_menu_end_carries_its_observation_and_handling() {
        let (outgoing, reader) = at_seven_thousand();
        let messages = drain(reader);
        outgoing.menu_or_switch_ended(6_000);
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        let ListenerToCore::MenuOrSwitchEnded { timing, .. } = message else {
            panic!("a menu end: {message:?}");
        };
        assert_eq!(
            (timing.observed_at_us, timing.listener_handled_at_us),
            (6_000, 7_000)
        );
    }
}
