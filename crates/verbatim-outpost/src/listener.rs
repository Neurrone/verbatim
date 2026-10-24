//! The focus-listener runtime (architecture section 1, decisions D13 and D14;
//! outpost redesign, "The focus listener").
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
//! `GetTokenInformation`, kernel calls that wait on no process). A foreground
//! event is forwarded unchecked; the outpost's worker checks that its
//! window is the foreground when it handles it, as NVDA's
//! `processForegroundWinEvent` does.
//!
//! The one place the listener calls into applications is the reading
//! threads, one per application, which judge and read each UIA focus from
//! the event's own element, as NVDA does, since no other process can hold
//! that element (`docs/design/focus-pipeline.md`, section 12). The callback
//! sends the application's outpost a marker at once and queues the element
//! for its application's thread; the thread's read, or its drop, follows
//! the marker by the same route. Each read is a few calls on elements the
//! listener holds, each of which UIA's call timeout ends, and a watchdog abandons a thread whose read
//! runs past its deadline, or past half a second once the user has moved
//! on to a window of the same application on another UI thread, which the
//! listener's foreground events wake it to judge, so a slow or hung
//! application holds only its own thread. One exposure is UIA's own and cannot be contained here: a
//! hung window named in a focus `WinEvent` holds UIA's delivery of every
//! focus event to the listener until it answers, as it holds NVDA's
//! (`docs/uia-bugs.md`, "A hung window named in a system focus event holds a
//! process's UIA events").
//!
//! The hand-over of MSAA events (`docs/design/focus-pipeline.md`, section
//! 12.4, "The MSAA hand-over, exactly"): every application's outpost takes
//! its focus, foreground, menu, alert, and tooltip events from its own
//! process-scoped hook, so they reach it by one route, in the order Windows
//! raised them, with its other MSAA events. The listener forwards them only
//! for an application whose outpost's hook is not yet in, so that the
//! application's first events start its outpost and reach it. When the
//! supervisor tells it an outpost's hook is in
//! ([`SupervisorToOutpost::HookInstalled`]), the listener reads the event
//! clock, forwards none of that application's events raised after it, and
//! queues "forwarding ended" naming that tick behind every fact it has
//! queued ([`OutpostToSupervisor::ForwardingEnded`]); events raised at or
//! before it, which can still reach its hook late, it forwards still. The
//! outpost matches the two hooks' copies of an event by counting. When the
//! application has no outpost with its hook in any more
//! ([`SupervisorToOutpost::HookRemoved`]), the listener forwards its events
//! again, and its reading thread ends once the events waiting for it are
//! read. A listener that replaces another is told on its command line the
//! applications whose outposts' hooks are in, and forwards none of their
//! events from the start.
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
//! Run as `verbatim-outpost.exe --listener --pipe-in <handle> --pipe-out
//! <handle> [--ignore-pids <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]`
//! — the same binary as a per-application outpost, with no target pid,
//! supervised by the same machinery.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
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
    ignores_notifications, notification_kind_from_uia, notification_processing_from_uia,
};
use verbatim_uia::{Agile, Cached, ListenerEvent, Part, Registration, Scope, Subscription};

use crate::arbitration::window_class_name;
use crate::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger};
use crate::focus_read::{
    Arrival, FocusEvent, FocusReader, HeldElements, Judged, Meta, READ_DEADLINE, Readers, Reading,
    WatchedRead,
};
use crate::foreground::{ForegroundWatch, Front, Step};
use crate::outpost::text_reads::CONSOLE_WINDOW_CLASS;
use crate::outpost::window::MOVED_ON_GRACE;
use crate::protocol::{
    DeliveredFact, EventTiming, FactKey, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    UiaSnapshotFact, WinEventStamp, decode_message, hop_us, now_us, put_message,
};
use crate::supervisor::IgnoredProcesses;

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
    /// The threadpool timer that ends the grace.
    grace: GraceTimer,
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
    urgent: VecDeque<OutpostToSupervisor>,
    facts: VecDeque<(Option<(Pid, FactKey)>, OutpostToSupervisor)>,
    /// The end of a menu or of the Alt+Tab switcher whose grace has passed,
    /// sent once no fact waits.
    menu_end: Option<OutpostToSupervisor>,
    /// The applications whose outposts have their own hooks in, each with
    /// the tick on the event clock after which none of its `WinEvent`
    /// facts is forwarded.
    hooked: HashMap<Pid, u32>,
}

/// One item the listener sends Core.
enum Sent {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// or a fault.
    Urgent(OutpostToSupervisor),
    /// A fact, with the key it is coalesced under, and, for a `WinEvent`'s,
    /// the application and the event's stamp.
    Fact(
        Option<(Pid, FactKey)>,
        OutpostToSupervisor,
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
    MenuEnd(OutpostToSupervisor),
    /// The idle barrier's answer for a round, behind everything waiting
    /// ([`Outgoing::idle_answer`]).
    IdleAnswer(u64),
    /// A message taken from what waits, to be written.
    Message(OutpostToSupervisor),
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
                        .rposition(|(_, waiting)| waiting.is_idle_answer())
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
                    OutpostToSupervisor::ForwardingEnded {
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
                    .push_back((None, OutpostToSupervisor::IdleAnswer { round }));
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
            Sent::IdleAnswer(round) => OutpostToSupervisor::IdleAnswer { round },
            Sent::ForwardingEnded {
                pid,
                outpost,
                through,
            } => OutpostToSupervisor::ForwardingEnded {
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
/// when it ended, in milliseconds since the Unix epoch for Core, and its
/// observed and handled stamps. NVDA fakes a focus from such an end only
/// when no focus event was validly processed with it; a focus fact can
/// still be dropped or fail to read, so Core, which knows which focus it
/// applied, decides, and every end is told.
struct MenuEnd {
    waiting: Mutex<Grace>,
    sender: Sender<Waiting>,
    /// The timer that ends the grace, set once it is made, which the
    /// callback sets again when it runs before the end's grace is over.
    timer: OnceLock<TimerHandle>,
}

/// A threadpool timer's handle, shared with its own callback.
struct TimerHandle(PTP_TIMER);

// SAFETY: a threadpool timer may be set from any thread, its own callback
// included; the handle stays open until `GraceTimer::drop`, which stops
// the callback setting it first (`Grace::closed`).
unsafe impl Send for TimerHandle {}
// SAFETY: as above.
unsafe impl Sync for TimerHandle {}

/// What waits for a menu end's grace to pass.
#[derive(Default)]
struct Grace {
    /// The end, when it ended, its stamps, and when its grace is over.
    end: Option<(u64, EventTiming, Instant)>,
    /// The idle barrier's rounds answered once the end is sent, behind it:
    /// the end is work the listener still owes Core.
    answers: Vec<u64>,
    /// Whether the timer is being closed, so the callback sets it no more.
    closed: bool,
}

impl MenuEnd {
    fn lock(&self) -> std::sync::MutexGuard<'_, Grace> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The grace has passed: the end waiting is sent, behind every fact
    /// waiting, and then the idle barrier's answers that waited for it.
    /// Sent under the grace's lock, so no answer can pass the end. A
    /// callback that runs before the end's grace is over, one of a grace an
    /// earlier end began that took the lock only once a newer end had come,
    /// or a timer that fired a little early, sends nothing and sets the
    /// timer again for the rest of the grace.
    fn grace_ended(&self) {
        let mut grace = self.lock();
        if let Some((_, _, over)) = grace.end {
            let now = Instant::now();
            if now < over {
                if !grace.closed
                    && let Some(timer) = self.timer.get()
                {
                    set_timer(timer.0, over - now);
                }
                return;
            }
        }
        if let Some((ended_at_ms, timing, _)) = grace.end.take() {
            sent(
                &self
                    .sender
                    .send(Sent::MenuEnd(OutpostToSupervisor::MenuOrSwitchEnded {
                        ended_at_ms,
                        timing,
                    })),
            );
        }
        for round in grace.answers.drain(..) {
            sent(&self.sender.send(Sent::IdleAnswer(round)));
        }
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
                let _ = end_of(end).timer.set(TimerHandle(timer));
                Ok(Self { timer, end })
            }
            Err(error) => {
                // SAFETY: from `Arc::into_raw` above, never given out.
                drop(unsafe { Arc::from_raw(end) });
                Err(io::Error::other(error))
            }
        }
    }

    /// Ends the grace [`MENU_END_GRACE`] from now, in place of any grace
    /// already running.
    fn arm(&self) {
        set_timer(self.timer, MENU_END_GRACE);
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
        end_of(self.end).lock().closed = true;
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
        let menu_end = Arc::new(MenuEnd {
            waiting: Mutex::new(Grace::default()),
            sender: sender.clone(),
            timer: OnceLock::new(),
        });
        let grace = GraceTimer::new(&menu_end)?;
        Ok(Self {
            sender,
            ignored,
            markers: AtomicU64::new(0),
            uia_installed: AtomicBool::new(false),
            clock: Clock::default(),
            menu_end,
            grace,
        })
    }

    fn urgent(&self, message: OutpostToSupervisor) {
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
        self.queue(
            pid,
            fact,
            (TraceId::mint(), observed_at_us / 1_000, timing),
            win_event,
        );
    }

    /// Sends `fact`, part of the read of the UIA focus event `meta`
    /// describes: its marker, its read, or its drop, carrying the trace and
    /// times of the event itself. Never replaced, since an outpost waits on
    /// the marker until its read or drop arrives.
    fn focus_read(&self, pid: Pid, fact: DeliveredFact, meta: Meta) {
        self.queue(
            pid,
            fact,
            (meta.trace, meta.observed_at_ms, meta.timing),
            None,
        );
    }

    /// Sends Core the window in front, `front`, with the integrity level of
    /// the process that owns it, read here, a local call; only the newest
    /// such report waits. Sent for every process, ignored ones included,
    /// since the keyboard hook needs to know of every window in front.
    fn foreground_window(&self, front: Front) {
        let owner = window_pid(front.window);
        let message = OutpostToSupervisor::ForegroundWindow {
            window: front.window,
            thread: front.thread,
            console: front.console,
            integrity: crate::integrity::process_integrity(owner),
        };
        sent(&self.sender.send(Sent::Fact(
            Some((Pid(0), FactKey::ForegroundWindow)),
            message,
            None,
        )));
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
    /// registered (`docs/design/focus-pipeline.md`, section 18). Such an
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
    /// new marker and trace, and its observation, whose milliseconds and
    /// microseconds one clock read stamps, so the focus it becomes is
    /// ordered to the microsecond.
    fn received(&self, kind: FocusEvent) -> Meta {
        let observed_at_us = self.now();
        Meta {
            marker: self.next_marker(),
            kind,
            trace: TraceId::mint(),
            observed_at_ms: observed_at_us / 1_000,
            timing: EventTiming {
                observed_at_us,
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
        (trace_id, observed_at_ms, mut timing): (TraceId, u64, EventTiming),
        win_event: Option<WinEventStamp>,
    ) {
        if self.ignored.contains(pid) {
            return;
        }
        if timing.listener_handled_at_us == 0 {
            timing.listener_handled_at_us = self.now();
        }
        let key = fact.key(win_event).map(|key| (pid, key));
        let message = OutpostToSupervisor::FocusFact {
            trace_id,
            observed_at_ms,
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
        self.note_menu_end(observed_at_us, Instant::now() + MENU_END_GRACE);
        self.grace.arm();
    }

    /// Notes that a menu or the Alt+Tab switcher closed, observed at
    /// `observed_at_us`, to be sent once its grace is over, at `over`.
    fn note_menu_end(&self, observed_at_us: u64, over: Instant) {
        let timing = EventTiming {
            observed_at_us,
            listener_handled_at_us: self.now(),
            ..EventTiming::default()
        };
        self.menu_end.lock().end = Some((observed_at_us / 1_000, timing, over));
    }

    /// Answers the idle barrier's round `round`, behind every fact sent
    /// before, never merged and never ahead as urgent; behind a menu end
    /// still in its grace too, once that is sent.
    fn idle_answer(&self, round: u64) {
        let mut grace = self.menu_end.lock();
        if grace.end.is_some() {
            grace.answers.push(round);
        } else {
            sent(&self.sender.send(Sent::IdleAnswer(round)));
        }
    }

    /// Now, on the clock the listener's stamps are read from.
    fn now(&self) -> u64 {
        (self.clock.0)()
    }

    fn fault(&self, detail: String) {
        self.urgent(OutpostToSupervisor::Fault { detail });
    }

    /// Takes nothing more, and has what waits written and the pipe closed,
    /// for the listener's shutdown; a menu end still in its grace is
    /// dropped.
    fn close(&self) {
        self.sender.close();
    }
}

/// The focus listener: owns its subscriptions for the whole life of the
/// process.
struct Listener {
    outgoing: Arc<Outgoing>,
    readers: Arc<Readers<UiaFocusReading>>,
    /// The elements of the focuses the reading threads accepted, by how
    /// Core judged them ([`SupervisorToOutpost::CoreFocus`]).
    held_elements: HeldElements,
    _registration: Option<Registration>,
    /// What nudges the `WinEvent` thread: the idle barrier's query goes
    /// through it, behind every event delivered to it before.
    nudger: Nudger,
    _event_thread: EventThread,
}

/// The reading threads' work in the listener: each UIA focus event judged
/// and read from its own element ([`FocusReader`]), and what became of it
/// sent to its application's outpost behind its marker.
struct UiaFocusReading {
    outgoing: Arc<Outgoing>,
    /// The elements of the focuses the reading threads accepted, by how
    /// Core judged them, which they share.
    held_elements: HeldElements,
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
        match FocusReader::new(self.held_elements.clone()) {
            Ok(reader) => Some(reader),
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
        watched: &WatchedRead,
    ) -> Judged {
        let Some(reader) = reader else {
            return Judged::Dropped("its reading thread has no UIA client");
        };
        match element.resolve() {
            Ok(sender) => reader.judge_watched(
                &sender,
                (kind, fact),
                (
                    Arrival {
                        trace: meta.trace,
                        observed_at_ms: meta.observed_at_ms,
                        observed_at_us: meta.timing.observed_at_us,
                    },
                    watched.clone(),
                ),
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

    fn abandoned(&self, meta: &Meta) {
        self.held_elements.abandoned(meta.trace);
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

    fn outcome(&self, pid: Pid, meta: Meta, judged: Judged) {
        if let Judged::Dropped(reason) = &judged {
            tracing::debug!(%pid, marker = meta.marker, reason, "UIA focus dropped");
        }
        self.outgoing
            .focus_read(pid, judged.into_fact(meta.marker, meta.kind), meta);
    }
}

impl Listener {
    /// Sets up the listener, sending through `sender`: installs every
    /// subscription,
    /// reporting a failure as a fault, and only then announces readiness,
    /// so the supervisor and Core know focus and menus are seen from then
    /// on. Installing the UIA focus registration can take hundreds of
    /// milliseconds; announcing readiness first let events in that time go
    /// unseen, such as a menu opened right after Verbatim started.
    ///
    /// The applications in `hooks_in` have outposts with their own hooks
    /// in already: none of their `WinEvent`s is forwarded, and each outpost
    /// is told so first, ending a hand-over the listener this one replaces
    /// may have left unfinished.
    ///
    /// # Errors
    ///
    /// Returns the error of creating the menu end's grace timer.
    fn new(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        hooks_in: &[(Pid, OutpostId)],
    ) -> io::Result<Self> {
        let outgoing = Arc::new(Outgoing::new(sender, ignored)?);
        // Every event this listener's hook takes in is raised after the
        // hook is installed, so after this tick: none is forwarded for
        // these applications.
        let before_the_hook = verbatim_ia2::event_clock().wrapping_sub(1);
        for &(pid, outpost) in hooks_in {
            outgoing.hook_installed(pid, outpost, before_the_hook);
        }
        let held_elements = HeldElements::default();
        let readers = Arc::new(Readers::new(
            UiaFocusReading {
                outgoing: Arc::clone(&outgoing),
                held_elements: held_elements.clone(),
            },
            READ_DEADLINE,
            MOVED_ON_GRACE,
        ));
        let registration = install_desktop_subscriptions(
            &outgoing,
            &readers,
            focus_subscription(&outgoing, &readers),
            menu_subscription(&outgoing, &readers),
        );
        outgoing.uia_installation_ended();

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
                        let step = watch.borrow_mut().console_event(event.hwnd, event.thread);
                        foreground_step(&outgoing, step);
                        return;
                    }
                    // Program Manager or the taskbar in front: no foreground
                    // change to forward, as NVDA takes none, but the window
                    // in front for the keyboard hook's view.
                    if event.kind == WinEventKind::ShellForeground {
                        let step = watch.borrow_mut().foreground(Front {
                            window: event.hwnd,
                            thread: event.thread,
                            console: false,
                        });
                        foreground_step(&outgoing, step);
                        return;
                    }
                    let foreground = (event.kind == WinEventKind::Foreground).then_some(Front {
                        window: event.hwnd,
                        thread: event.thread,
                        console: false,
                    });
                    // Forwarded first, so the event's observation time is
                    // taken before anything else the callback does; its
                    // window's class, read to route it, tells a console.
                    let console = forward_msaa_event(&outgoing, event);
                    if let Some(front) = foreground {
                        let step = watch.borrow_mut().foreground(Front { console, ..front });
                        foreground_step(&outgoing, step);
                        // A foreground change is the evidence that the
                        // user may have moved on from a slow read's window
                        // (`focus_read`'s watchdog).
                        foreground_readers.foreground_changed();
                    }
                }),
                // The idle barrier's query, its round in the first word:
                // every event delivered before it has been handled, so the
                // reading threads are waited for next. Or, once, the start
                // of the foreground's watch, once the hooks are in.
                nudge: Some(Box::new(move |round, kind| {
                    if kind == NUDGE_FOREGROUND_START {
                        let step = start_watch.borrow_mut().start(front_now());
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

        // The listener has no target application; `target_pid` is a sentinel
        // the supervisor only logs.
        outgoing.urgent(OutpostToSupervisor::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(0),
        });

        Ok(Self {
            outgoing,
            readers,
            held_elements,
            _registration: registration,
            nudger,
            _event_thread: event_thread,
        })
    }

    /// Shuts the listener down cleanly ([`SupervisorToOutpost::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA focus handler and the
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
            held_elements: _,
            _registration: registration,
            nudger: _,
            _event_thread: event_thread,
        } = self;
        drop(event_thread);
        let registration_failed_reads = registration.as_ref().map_or(0, Registration::failed_reads);
        drop(registration);
        let reading_threads = readers.stop();
        let failed_reads = registration_failed_reads.saturating_add(readers.failed_reads());
        outgoing.close();
        io.finish(&outgoing.sender);
        verbatim_uia::release_mta_usage();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            reading_threads,
            failed_reads,
            "the focus listener shut down"
        );
    }

    /// Answers `Ping` with a `Pong` (the listener never abandons a worker, so
    /// its count is always zero), takes the hand-over's news of outposts'
    /// hooks, starts the idle barrier's round ([`Self::idle_query`]), and
    /// ignores everything else. An application with no outpost
    /// any more has its reading thread ended too
    /// ([`Readers::outpost_ended`]). `received_at_us` is when the command
    /// was read off Core's pipe: the hop from verbatim.exe's write is
    /// logged at trace level, under `verbatim::stage`.
    fn handle_command(&self, command: &SupervisorToOutpost, received_at_us: u64) {
        if let Some(us) = hop_us(command.sent_at_us(), received_at_us) {
            tracing::trace!(target: "verbatim::stage", stage = "verbatim.exe to the listener", us, ?command);
        }
        match command {
            SupervisorToOutpost::Ping { seq, .. } => {
                self.outgoing.urgent(OutpostToSupervisor::Pong {
                    seq: *seq,
                    parked_count: 0,
                    written_at_us: 0,
                });
            }
            SupervisorToOutpost::HookInstalled { pid, outpost, .. } => {
                self.outgoing
                    .hook_installed(*pid, *outpost, verbatim_ia2::event_clock());
            }
            SupervisorToOutpost::HookRemoved { pid, .. } => {
                self.outgoing.hook_removed(*pid);
                self.readers.outpost_ended(*pid);
            }
            SupervisorToOutpost::CoreFocus { judged, focus, .. } => self.held_elements.core_focus(
                *judged,
                focus.as_ref().map(|focus| (focus.trace_id, focus.alive)),
            ),
            SupervisorToOutpost::IdleQuery { round } => self.idle_query(*round),
            _ => {}
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
/// selection's fact from mockapp's own event as the listener makes it.
#[must_use]
pub fn capture(
    sender: &Cached<ListenerEvent>,
    fields: &ListenerEvent,
) -> Option<(isize, UiaSnapshotFact)> {
    fields.process_id.filter(|&pid| pid != 0)?;
    let parts = fields.parts(sender.runtime_id());
    Some((
        fields.role.hwnd,
        UiaSnapshotFact {
            runtime_id: parts.runtime_id,
            role: parts.role,
            name: parts.name,
            value: parts.value,
            states: parts.states,
            details: parts.details,
        },
    ))
}

/// The desktop-wide UIA focus subscription. Its callback makes no call: it
/// sends the application's outpost a marker for the event at once and
/// queues the event's element for the application's reading thread
/// ([`Readers`]), whose read or drop follows the marker.
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
/// (`docs/design/focus-pipeline.md`, section 18), unless a focus is
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
/// for a selection or a notification, is what was captured from it. Makes
/// no cross-process call.
fn queue_for_reading(
    outgoing: &Outgoing,
    readers: &Readers<UiaFocusReading>,
    sender: &Cached<ListenerEvent>,
    fields: &ListenerEvent,
    kind: FocusEvent,
    fact: Option<DeliveredFact>,
) {
    let Some(pid) = outgoing.uia_event_process(fields.process_id) else {
        return;
    };
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
        Ok(agile) => readers.push(pid, meta, (agile, fields.role.hwnd, runtime_id, kind, fact)),
        Err(error) => {
            tracing::warn!(%error, "a UIA focus's element could not be kept for its read");
            outgoing.focus_read(
                pid,
                DeliveredFact::UiaFocusDropped {
                    marker: meta.marker,
                    evidence: crate::focus_read::drop_evidence(
                        kind,
                        "its element could not be kept for its read",
                    ),
                },
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
/// anything else in their snapshots is not read here and is left empty. A
/// failure is reported as a fault and leaves the MSAA path running on its
/// own.
fn install_desktop_subscriptions(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
    focus: Subscription<Cached<ListenerEvent>>,
    menu: Subscription<Cached<ListenerEvent>>,
) -> Option<Registration> {
    // A selection and a notification go through their application's
    // reading thread, behind a marker, for NVDA's checks of their sender's
    // nearest window (`FocusReader::check_fact`), in the order they came
    // with the focus events.
    let selection_outgoing = Arc::clone(outgoing);
    let selection_readers = Arc::clone(readers);
    let selection = Subscription::Event {
        event: UIA_SelectionItem_ElementSelectedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            if let Some((hwnd, snapshot)) = capture(sender, &fields) {
                let fact = DeliveredFact::UiaSelection {
                    hwnd,
                    focus_window: 0,
                    snapshot,
                };
                queue_for_reading(
                    &selection_outgoing,
                    &selection_readers,
                    sender,
                    &fields,
                    FocusEvent::Selection,
                    Some(fact),
                );
            }
        }),
    };

    let notification_outgoing = Arc::clone(outgoing);
    let notification_readers = Arc::clone(readers);
    let notifications = Subscription::Notifications {
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
                        &notification_outgoing,
                        &notification_readers,
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
    };

    // NVDA's base cache request, as its global group carries, with what
    // refines a role (`ListenerEvent`): UIA fills only these in the
    // application for each event, and the outpost reads what more it needs
    // of a selected item when it handles the event.
    let parts = [focus, selection, menu, notifications]
        .into_iter()
        .map(Part::shaped::<ListenerEvent>)
        .collect();
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

/// Forwards one global MSAA `WinEvent` as a fact for the process
/// [`routed_process`] names, with the thread that raised it and when, unless
/// that process's outpost has its own hook in and took it in itself
/// ([`Outgoing::fact`]). Drops a null window or a pid-zero owner; the raw
/// object address is forwarded untouched, and the app outpost does the
/// acquisition. Returns whether the event's window is a console host
/// window, by the class read to route it; `false` for an event dropped
/// before that read, whose window is null or has no owner.
fn forward_msaa_event(outgoing: &Outgoing, event: WinEvent) -> bool {
    // The observation, before the callback's local reads, which the
    // listener's handling stage then holds.
    let observed_at_us = outgoing.now();
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
        return false;
    }
    // NVDA takes a null window as the desktop window
    // (`internalWinEventHandler.py` 113 to 129), whose process is the
    // client server, whose objects NVDA's focused-state test refuses
    // (`shouldAllowIAccessibleFocusEvent` on the desktop window's object)
    // and Verbatim cannot open: dropping it here is the same outcome.
    if hwnd == 0 {
        return false;
    }
    let owner = window_pid(hwnd);
    if owner == 0 {
        return false;
    }
    let class = window_class_name(hwnd);
    let console = class == CONSOLE_WINDOW_CLASS;
    let Some(pid) = routed_process(&class, owner, || process_of_thread(thread)) else {
        log_unread_console_once(hwnd, thread);
        return console;
    };
    if pid != owner {
        if outgoing.ignored.contains(Pid(owner)) {
            // The console of a process Verbatim ignores, whose events were
            // dropped by its owner before they went to its console host.
            return console;
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
    let Some(fact) = DeliveredFact::from_win_event(kind, (hwnd, id_object, id_child)) else {
        return console;
    };
    outgoing.fact(
        Pid(pid),
        fact,
        Some(raised_ms_ago),
        Some(WinEventStamp { thread, time }),
        observed_at_us,
    );
    console
}

/// The listener's nudge that starts the watch of the window in front
/// ([`ForegroundWatch::start`]), in the nudge's second word; the idle
/// barrier's carry 0.
const NUDGE_FOREGROUND_START: isize = 1;

/// The window in the foreground now, with the thread Windows names as its
/// owner, read locally; `None` when no window is in front.
fn front_now() -> Option<Front> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: a local read with no preconditions.
    let window = unsafe { GetForegroundWindow() }.0 as isize;
    if window == 0 {
        return None;
    }
    Some(Front {
        window,
        thread: crate::outpost::window::window_owner(window).0,
        console: window_class_name(window) == CONSOLE_WINDOW_CLASS,
    })
}

/// Carries out a step of the foreground's watch, on the `WinEvent` thread:
/// sends Core the window in front, with its process's integrity level read
/// locally ([`OutpostToSupervisor::ForegroundWindow`]), and hooks or
/// unhooks every console's events.
fn foreground_step(outgoing: &Outgoing, step: Step) {
    if let Some(front) = step.report {
        outgoing.foreground_window(front);
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
    crate::outpost::window::window_owner(hwnd).1
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
/// (`docs/design/focus-pipeline.md`, section 5.4). `None` when the host
/// cannot be read: the event is dropped rather than sent to the client,
/// whose outpost would otherwise take the console host's window as its
/// application's (Dickson, 2026-10-19;
/// `docs/design/listener-without-connections.md`, section 10, step 3).
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
        match decode_message::<SupervisorToOutpost>(body)? {
            SupervisorToOutpost::Shutdown => return Ok(Serving::Stop),
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
    ended
}

/// The command-line option that tells a listener the applications whose
/// outposts have their own hooks in.
pub(crate) const HOOKS_IN_ARG: &str = "--hooks-in";

/// The applications and outposts in `list`, as the listener's `--hooks-in`
/// option gives them: `pid:outpost` pairs separated by commas. Anything
/// that is not such a pair is skipped.
#[must_use]
pub fn parse_hooks_in(list: &str) -> Vec<(Pid, OutpostId)> {
    list.split(',')
        .filter_map(|pair| {
            let (pid, outpost) = pair.trim().split_once(':')?;
            Some((Pid(pid.parse().ok()?), OutpostId(outpost.parse().ok()?)))
        })
        .collect()
}

/// `hooks_in` as [`HOOKS_IN_ARG`] gives it.
pub(crate) fn format_hooks_in(hooks_in: &[(Pid, OutpostId)]) -> String {
    hooks_in
        .iter()
        .map(|(pid, outpost)| format!("{}:{}", pid.0, outpost.0))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::mpsc;

    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::EVENT_PIPE_BUFFER;

    use super::*;
    use crate::protocol::{DropEvidence, read_message};

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

    /// Everything `outgoing` sent, which is closed first, as Core reads it.
    fn sent(outgoing: &Outgoing, reader: &mut BufReader<File>) -> Vec<OutpostToSupervisor> {
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
                OutpostToSupervisor::FocusFact {
                    fact, win_event, ..
                } => {
                    format!("{} {:?}", fact.pid.0, fact.fact.key(win_event))
                }
                OutpostToSupervisor::ForwardingEnded {
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
            observed_at_ms: 0,
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
                evidence: DropEvidence::Focus,
            },
            meta(2),
        );
        let facts: Vec<DeliveredFact> = sent(&outgoing, &mut reader)
            .into_iter()
            .filter_map(|message| match message {
                OutpostToSupervisor::FocusFact { fact, .. } => Some(fact.fact),
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
                    evidence: DropEvidence::Inconclusive,
                },
                DeliveredFact::UiaFocusDropped {
                    marker: 2,
                    evidence: DropEvidence::Focus,
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
        outgoing.urgent(OutpostToSupervisor::Pong {
            seq: 4,
            parked_count: 0,
            written_at_us: 0,
        });
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                OutpostToSupervisor::FocusFact { fact, timing, .. } => {
                    format!("{} {}", fact.pid.0, timing.observed_at_us)
                }
                OutpostToSupervisor::Pong { seq, .. } => format!("pong {seq}"),
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
                    OutpostToSupervisor::ForwardingEnded { .. },
                    OutpostToSupervisor::FocusFact { .. }
                ]
            ),
            "{sent:?}"
        );
    }

    #[test]
    fn the_applications_with_hooks_in_round_trip_through_the_command_line() {
        let hooks_in = [(Pid(12), OutpostId(3)), (Pid(40), OutpostId(7))];
        assert_eq!(parse_hooks_in(&format_hooks_in(&hooks_in)), hooks_in);
        assert_eq!(parse_hooks_in(" 12:3,x,9, 40:7 "), hooks_in);
        assert_eq!(parse_hooks_in(""), Vec::<(Pid, OutpostId)>::new());
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
    fn drain(mut reader: BufReader<File>) -> mpsc::Receiver<OutpostToSupervisor> {
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
        let [OutpostToSupervisor::FocusFact { fact, .. }] = sent.as_slice() else {
            panic!("one fact is sent, the other process's: {sent:?}");
        };
        assert_eq!(fact.pid, Pid(5));
    }

    #[test]
    fn a_uia_event_received_before_the_subscriptions_are_installed_is_dropped() {
        let (outgoing, _reader) = outgoing();
        assert_eq!(outgoing.uia_event_process(Some(5)), None);
        outgoing.uia_installation_ended();
        assert_eq!(outgoing.uia_event_process(Some(5)), Some(Pid(5)));
    }

    #[test]
    fn a_menu_end_no_focus_follows_reaches_core_after_the_grace() {
        let (outgoing, reader) = outgoing();
        let messages = drain(reader);
        let ended = Instant::now();
        outgoing.menu_or_switch_ended(1_000);
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        assert!(matches!(
            message,
            OutpostToSupervisor::MenuOrSwitchEnded { .. }
        ));
        assert!(ended.elapsed() >= MENU_END_GRACE, "not before the grace");
    }

    /// A grace timer's callback that runs as a newer end comes, taking the
    /// lock only once that end is noted, sends nothing: the newer end is
    /// sent once its own grace is over, and only once.
    #[test]
    fn a_menu_end_is_not_sent_before_its_grace_by_a_callback_of_an_earlier_one() {
        let (outgoing, reader) = outgoing();
        let messages = drain(reader);
        let ended = Instant::now();
        outgoing.menu_or_switch_ended(1_000);
        // As an earlier grace's callback does, run just as this end came.
        outgoing.menu_end.grace_ended();
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        assert!(matches!(
            message,
            OutpostToSupervisor::MenuOrSwitchEnded { .. }
        ));
        assert!(ended.elapsed() >= MENU_END_GRACE, "not before the grace");
        outgoing.close();
        assert_eq!(messages.iter().count(), 0, "the end is sent once");
    }

    #[test]
    fn a_menu_end_a_focus_follows_is_sent_after_the_focus() {
        // Whether the focus was usable is Core's to judge: it may yet be
        // dropped or fail to read.
        let (outgoing, reader) = outgoing();
        outgoing.menu_or_switch_ended(1_000);
        outgoing.fact(Pid(5), msaa_focus(), None, None, 1_000);
        let messages = drain(reader);
        let first = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the focus fact is sent");
        assert!(matches!(first, OutpostToSupervisor::FocusFact { .. }));
        let second = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent too");
        assert!(matches!(
            second,
            OutpostToSupervisor::MenuOrSwitchEnded { .. }
        ));
    }

    #[test]
    fn a_menu_end_whose_grace_ends_behind_a_write_in_flight_goes_after_every_waiting_fact() {
        // The grace timer's callback sends the end through the sender the
        // facts go through, so it waits behind the write in flight, and
        // goes after every fact waiting when it is written, one sent after
        // it included.
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
        outgoing.note_menu_end(1_000, Instant::now());
        // As the timer's callback does when the grace ends.
        outgoing.menu_end.grace_ended();
        outgoing.fact(Pid(6), msaa_focus(), None, None, 1_000);
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                OutpostToSupervisor::FocusFact { fact, .. } => format!("fact {}", fact.pid.0),
                OutpostToSupervisor::MenuOrSwitchEnded { .. } => "menu end".to_owned(),
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
        outgoing.idle_answer(1);
        outgoing.fact(Pid(5), msaa_focus(), None, None, 2_000);
        outgoing.urgent(OutpostToSupervisor::Pong {
            seq: 4,
            parked_count: 0,
            written_at_us: 0,
        });
        outgoing.note_menu_end(3_000, Instant::now());
        outgoing.idle_answer(2);
        outgoing.fact(Pid(6), msaa_focus(), None, None, 3_000);
        // As the timer's callback does when the grace ends.
        outgoing.menu_end.grace_ended();
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                OutpostToSupervisor::FocusFact { fact, timing, .. } => {
                    format!("{} {}", fact.pid.0, timing.observed_at_us)
                }
                OutpostToSupervisor::Pong { seq, .. } => format!("pong {seq}"),
                OutpostToSupervisor::IdleAnswer { round } => format!("idle {round}"),
                OutpostToSupervisor::MenuOrSwitchEnded { .. } => "menu end".to_owned(),
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
        let [OutpostToSupervisor::FocusFact { timing, .. }] = sent.as_slice() else {
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

    /// A `WinEvent`'s fact is stamped, in milliseconds, at its callback's
    /// observation, from the same clock read as its microseconds, so the
    /// focus it becomes is ordered by when it happened.
    #[test]
    fn a_win_events_fact_is_stamped_at_its_callbacks_observation() {
        let (outgoing, mut reader) = at_seven_thousand();
        outgoing.fact(Pid(5), msaa_focus(), None, None, 6_543_210);
        let sent = sent(&outgoing, &mut reader);
        let [
            OutpostToSupervisor::FocusFact {
                observed_at_ms,
                timing,
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("one fact is sent: {sent:?}");
        };
        assert_eq!((*observed_at_ms, timing.observed_at_us), (6_543, 6_543_210));
    }

    /// A UIA event is stamped, in milliseconds and microseconds, from one
    /// clock read as the callback received it, and its facts carry that
    /// stamp, so the focus it becomes is ordered to the microsecond.
    #[test]
    fn a_uia_events_facts_carry_one_clock_reads_stamp() {
        static CLOCK: AtomicU64 = AtomicU64::new(6_543_900);
        let (mut outgoing, mut reader) = outgoing();
        // Each read of the clock is 200 microseconds after the last, so a
        // second read would be in another millisecond.
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
        let [
            OutpostToSupervisor::FocusFact {
                observed_at_ms,
                timing,
                ..
            },
        ] = sent.as_slice()
        else {
            panic!("one fact is sent: {sent:?}");
        };
        assert_eq!((*observed_at_ms, timing.observed_at_us), (6_543, 6_543_900));
    }

    #[test]
    fn a_marker_is_handled_as_it_is_sent() {
        let (outgoing, mut reader) = at_seven_thousand();
        let meta = Meta {
            marker: 1,
            kind: FocusEvent::Focus,
            trace: TraceId::mint(),
            observed_at_ms: 0,
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
            observed_at_ms: 0,
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
                evidence: DropEvidence::Focus,
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
        let OutpostToSupervisor::MenuOrSwitchEnded { ended_at_ms, .. } = message else {
            panic!("a menu end: {message:?}");
        };
        assert_eq!(ended_at_ms, 6_543);
    }

    #[test]
    fn a_menu_end_carries_its_observation_and_handling() {
        let (outgoing, reader) = at_seven_thousand();
        let messages = drain(reader);
        outgoing.menu_or_switch_ended(6_000);
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        let OutpostToSupervisor::MenuOrSwitchEnded { timing, .. } = message else {
            panic!("a menu end: {message:?}");
        };
        assert_eq!(
            (timing.observed_at_us, timing.listener_handled_at_us),
            (6_000, 7_000)
        );
    }
}
