//! The focus-listener runtime (architecture section 1, "The focus listener
//! (D13)"; decisions D13 and D14; ADR 0003, "Readers tell, Core asks";
//! design `docs/design/readers-tell-core-asks-build1.md`, "The listener in
//! this build").
//!
//! One permanent listener process holds the subscriptions that are global
//! by nature and tells Core of every event it hears; Core judges each tell
//! and asks what it needs:
//!
//! - every `WinEvent` kind it hooks desktop-wide, but the console host's
//!   five, which the console host's outpost hooks, each told after NVDA's
//!   local drops, behind its foreground hold (`tells`);
//! - the desktop-wide UI Automation focus handler and the global group NVDA
//!   registers on Windows 11 (an element selected, a menu opened,
//!   notifications, windows opened), each event checked on its
//!   application's check thread with the calls NVDA's handler makes at
//!   delivery, then told, its element kept behind the tell's token
//!   (`checking`);
//! - Core's requests on what it kept, and on an MSAA object at an event's
//!   address, each answered once on the application's reading thread
//!   (`asked`, `crate::reading`), and Core's Cancel of them;
//! - the progress path: every progress bar's value change, from the
//!   classic hook's value changes and the UIA range value changes, read on
//!   the application's reading thread behind Core's requests and sent as a
//!   bar's change, unasked;
//! - the window that came to the foreground, for the keyboard hook's view
//!   ([`ListenerToCore::Foreground`]; `verbatim_listener_logic::foreground`).
//!
//! The listener's callbacks and its writer never make a cross-process call
//! (decision D13): a UI Automation callback delivers the element with its
//! properties cached, and every call into an application is made on a
//! check thread or a reading thread, one of each per application, so a
//! hung application holds only its own. One exposure is UIA's own and
//! cannot be contained here: a hung window named in a focus `WinEvent`
//! holds UIA's delivery of every focus event to the listener until it
//! answers, as it holds NVDA's (`books/dev/src/reference/uia-bugs.md`, "A
//! hung window named in a system focus event holds a process's UIA
//! events").
//!
//! Tells are merged with NVDA's limiter's rule before they are sent: one
//! waiting tell per object, kind and raising thread, a newer one replacing
//! it and moving to the back. Pongs, `Ready` and faults go ahead of
//! everything else; answers and reports go in the order they are made.
//!
//! The processes Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names) are held open here too, and their progress
//! bars are not read; their events are told, and Core drops a focus of
//! one at its answer (a test-harness rule).
//!
//! Run as `verbatim-listener.exe --listener --pipe-in <handle> --pipe-out
//! <handle> [--ignore-pids <pid,pid>]` — its own executable, with no
//! target pid, supervised by the same machinery as the outposts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender as FoundSender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
    SetThreadpoolTimer, WaitForThreadpoolTimerCallbacks,
};
use windows::Win32::UI::Accessibility::{
    NotificationKind, NotificationProcessing, UIA_MenuOpenedEventId, UIA_ProgressBarControlTypeId,
    UIA_RangeValueValuePropertyId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_Window_WindowOpenedEventId,
};

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEvent, WinEventKind};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{Answer, Ask, ObjectRef, Outcome, Purpose, Refusal, Request};
use verbatim_model::tell::Payload;
use verbatim_model::{BarChange, Percent, Pid, Role, StateSet};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{Backlog, ChildIo, SendError, Sender, Served, Serving};
use verbatim_uia::map::{
    CachedUiaParts, ignores_notifications, is_toast_candidate, notification_kind_from_uia,
    notification_processing_from_uia,
};
use verbatim_uia::served_only_by_uia_proxies;
use verbatim_uia::{
    Cached, ListenerEvent, ListenerProgress, ListenerSelection, Part, Registration, Scope,
    Subscription,
};

use crate::reading::Readers;

mod asked;
mod checking;
mod tells;

use asked::{UiaJob, UiaReading};
use checking::{CheckEvent, Checks, Kept};

use crate::value_change::{Found, Judgement, ValueChange, ValueChangeKnowledge, probed_of};
use tells::{Held, TellKey, Tells, tell_win_event};
use verbatim_ia2::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger};
use verbatim_listener_logic::focus::{
    BarObject, BarRaiser, BarVerdict, ClassicBarRead, WindowAnswer, classic_value_change,
};
use verbatim_listener_logic::foreground::{ForegroundWatch, Front, Step};
use verbatim_listener_logic::schedule::JobKind;
use verbatim_model::protocol::{
    CoreToListener, EventTiming, ListenerToCore, Responder, UiaSnapshotFact, hop_us,
};
use verbatim_process::clock::now_us;
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::message::{decode_message, put_message};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_reader_rules::class::{
    ClassVerdict, PostProbeCheck, class_verdict, has_windows_forms_class, post_probe_check,
};
use verbatim_reader_rules::verdicts::Probe;
use verbatim_win32::window::{WindowReadabilityReader, window_class_name};

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
    /// The processes whose progress bars are not read.
    ignored: IgnoredProcesses,
    /// Whether the desktop-wide UIA subscriptions have been installed, or
    /// their installation has failed: until then every UIA event is dropped
    /// ([`Self::uia_event_process`]).
    uia_installed: AtomicBool,
    /// The clock the stamps are read from.
    clock: Clock,
    /// The listener's order of tells and its foreground hold.
    tells: Arc<Tells>,
    /// The threadpool timer that ends the foreground hold, kept until the
    /// listener is dropped, which closes it; none in a test, whose hold
    /// ends only by the foreground or a later tell.
    _hold_timer: Option<PoolTimer<Tells>>,
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
/// then tells, merged one per object, kind and raising thread, with the
/// answers and reports in the order they were made. The backlog of the
/// listener's [`Sender`], which every message passes through.
#[derive(Default)]
struct Waiting {
    urgent: VecDeque<ListenerToCore>,
    /// Tells, with the key each is merged by, answers and reports, and the
    /// idle barrier's answers, in order.
    queued: VecDeque<(Option<TellKey>, ListenerToCore)>,
}

/// One item the listener sends Core.
enum Sent {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// or a fault.
    Urgent(ListenerToCore),
    /// A tell, with the key it is merged under.
    Tell(TellKey, ListenerToCore),
    /// An answer or a report, never merged.
    Report(ListenerToCore),
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
            Sent::Tell(key, message) => self.merged(key, message),
            Sent::Report(message) => self.queued.push_back((None, message)),
            Sent::IdleAnswer(round) => self
                .queued
                .push_back((None, ListenerToCore::IdleAnswer { round })),
        }
        Ok(())
    }

    fn pop(&mut self) -> Option<Sent> {
        self.urgent
            .pop_front()
            .or_else(|| self.queued.pop_front().map(|(_, message)| message))
            .map(Sent::Message)
    }

    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.queued.is_empty()
    }

    fn encode(&mut self, item: Sent, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message = match item {
            Sent::Urgent(message)
            | Sent::Message(message)
            | Sent::Tell(_, message)
            | Sent::Report(message) => message,
            Sent::IdleAnswer(round) => ListenerToCore::IdleAnswer { round },
        };
        // The write's stamp, for the latency ledger's split of the
        // listener's time from the pipe's, taken as the write begins,
        // whichever thread writes.
        message.written(now_us());
        put_message(buffer, &message)
    }
}

impl Waiting {
    /// Queues the tell `message` behind everything waiting, replacing any
    /// waiting tell merged by the same `key`, but never across the idle
    /// barrier's answer, which the one replaced would move behind.
    fn merged(&mut self, key: TellKey, message: ListenerToCore) {
        let after_answer = self
            .queued
            .iter()
            .rposition(|(_, waiting)| matches!(waiting, ListenerToCore::IdleAnswer { .. }))
            .map_or(0, |answer| answer + 1);
        let mut index = after_answer;
        while index < self.queued.len() {
            if self.queued[index].0.as_ref() == Some(&key) {
                self.queued.remove(index);
            } else {
                index += 1;
            }
        }
        self.queued.push_back((Some(key), message));
    }
}

/// A threadpool timer's handle, shared with its own callback.
struct TimerHandle(PTP_TIMER);

impl TimerHandle {
    /// Sets the timer to go off once, `after` from now ([`set_timer`]).
    fn set(&self, after: Duration) {
        set_timer(self.0, after);
    }

    /// Stops the timer: it goes off no more until it is set again.
    fn stop(&self) {
        // SAFETY: the timer is open until its `PoolTimer` is dropped.
        unsafe { SetThreadpoolTimer(self.0, None, 0, None) };
    }
}

// SAFETY: a threadpool timer may be set from any thread, its own callback
// included; the handle stays open until `PoolTimer::drop`, which stops
// the callback setting it first (`Timed::closing`).
unsafe impl Send for TimerHandle {}
// SAFETY: as above.
unsafe impl Sync for TimerHandle {}

/// What a [`PoolTimer`] goes off for: it is given the timer's handle once,
/// before the timer can go off, and is told when it goes off and when the
/// timer is being closed, after which it sets the timer no more.
trait Timed: Send + Sync {
    /// The timer's handle, given once, before any callback.
    fn take_timer(&self, timer: TimerHandle);
    /// The timer went off, on a threadpool thread.
    fn fired(&self);
    /// The timer is being closed: it is set no more.
    fn closing(&self);
}

/// A send's outcome: once the listener is shutting down, or Core's pipe
/// has gone, what is sent is dropped.
fn sent(sent: &Result<(), SendError<Infallible>>) {
    if sent.is_err() {
        tracing::debug!("a message to Core is dropped: the pipe is closed");
    }
}

/// A threadpool timer (`docs/design/communication.md`, section 4.7): no
/// thread of the listener's waits for it, and its callback acts itself, on
/// what it was made for: a menu end's grace ([`MenuEnd`]) or the
/// foreground hold ([`Tells`]).
struct PoolTimer<T: Timed> {
    timer: PTP_TIMER,
    /// What the callback is given, kept alive until the timer is closed.
    owner: *const T,
}

// SAFETY: a threadpool timer may be set and closed from any thread, and
// what it points to is shared, `Sync` data kept alive by the timer.
unsafe impl<T: Timed> Send for PoolTimer<T> {}
// SAFETY: as above.
unsafe impl<T: Timed> Sync for PoolTimer<T> {}

impl<T: Timed> PoolTimer<T> {
    fn new(owner: &Arc<T>) -> io::Result<Self> {
        let owner = Arc::into_raw(Arc::clone(owner));
        // SAFETY: the callback is given `owner`, which stays alive until
        // the timer is closed in `drop`, after its callbacks have finished.
        let timer =
            unsafe { CreateThreadpoolTimer(Some(fired::<T>), Some(owner.cast_mut().cast()), None) };
        match timer {
            Ok(timer) => {
                // Given before the timer is first set, so before any
                // callback.
                owner_of(owner).take_timer(TimerHandle(timer));
                Ok(Self { timer, owner })
            }
            Err(error) => {
                // SAFETY: from `Arc::into_raw` above, never given out.
                drop(unsafe { Arc::from_raw(owner) });
                Err(io::Error::other(error))
            }
        }
    }
}

/// What a [`PoolTimer`] was made with.
fn owner_of<'a, T>(owner: *const T) -> &'a T {
    // SAFETY: from `Arc::into_raw` in `PoolTimer::new`, released only in
    // `PoolTimer::drop`, after the last use.
    unsafe { &*owner }
}

/// Sets `timer`, open until its [`PoolTimer`] is dropped, to fire once
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

impl<T: Timed> Drop for PoolTimer<T> {
    fn drop(&mut self) {
        // The callback sets the timer no more; then the timer is stopped,
        // its callbacks waited for or cancelled, and only then closed and
        // what it points to released.
        owner_of(self.owner).closing();
        // SAFETY: the timer is open.
        unsafe { SetThreadpoolTimer(self.timer, None, 0, None) };
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, true) };
        // SAFETY: as above; no callback runs or will run.
        unsafe { CloseThreadpoolTimer(self.timer) };
        // SAFETY: from `Arc::into_raw` in `new`, released once, here, with
        // no callback left to use it.
        drop(unsafe { Arc::from_raw(self.owner) });
    }
}

/// A pool timer's callback, on a threadpool thread.
unsafe extern "system" fn fired<T: Timed>(
    _instance: PTP_CALLBACK_INSTANCE,
    owner: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: the timer was created with its owner, which outlives every
    // callback (`PoolTimer::drop`).
    let owner = unsafe { &*owner.cast_const().cast::<T>() };
    owner.fired();
}

impl Outgoing {
    /// Sends through `sender`, reading no progress bar of the processes
    /// `ignored` names.
    ///
    /// # Errors
    ///
    /// Returns the error of creating the foreground hold's timer.
    fn new(sender: Sender<Waiting>, ignored: IgnoredProcesses) -> io::Result<Self> {
        let tells = Arc::new(Tells::new(sender.clone()));
        let hold = PoolTimer::new(&tells)?;
        Ok(Self::with_tells(sender, ignored, tells, Some(hold)))
    }

    /// [`new`](Self::new) with no timer, for a test, whose hold ends only
    /// by the foreground or a later tell.
    #[cfg(test)]
    fn untimed(sender: Sender<Waiting>, ignored: IgnoredProcesses) -> Self {
        let tells = Arc::new(Tells::new(sender.clone()));
        Self::with_tells(sender, ignored, tells, None)
    }

    fn with_tells(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        tells: Arc<Tells>,
        hold_timer: Option<PoolTimer<Tells>>,
    ) -> Self {
        Self {
            sender,
            ignored,
            uia_installed: AtomicBool::new(false),
            clock: Clock::default(),
            tells,
            _hold_timer: hold_timer,
        }
    }

    fn urgent(&self, message: ListenerToCore) {
        sent(&self.sender.send(Sent::Urgent(message)));
    }

    /// Sends Core the answer to one of its requests, in the order answers
    /// are made, never merged.
    fn answer(&self, answer: Answer) {
        sent(
            &self
                .sender
                .send(Sent::Report(ListenerToCore::Answer(answer))),
        );
    }

    /// Sends Core the window that came to the foreground, `front`, with its
    /// root owner, the process holding attention and that process's
    /// integrity level, read here with local calls (`GetAncestor`,
    /// `GetWindowThreadProcessId`, `GetProcessIdOfThread` for a console
    /// window whose thread its event named, and the token's integrity).
    /// Sent for every process, ignored ones included, since Core takes from
    /// it the keyboard hook's view, attention and its foreground record;
    /// none replaces another.
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
        sent(&self.sender.send(Sent::Report(message)));
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
    /// registered; an event with no process, or from an ignored one.
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

    /// Whether the desktop-wide UIA subscriptions have been installed, or
    /// their installation has failed: until then every UIA event is
    /// dropped.
    fn uia_installed(&self) -> bool {
        self.uia_installed.load(Ordering::Acquire)
    }

    /// Sends Core a progress bar's change, read from the event observed at
    /// `timing`'s observation.
    fn progress(&self, change: BarChange, timing: EventTiming) {
        let mut timing = timing;
        timing.listener_handled_at_us = self.now();
        sent(
            &self
                .sender
                .send(Sent::Report(ListenerToCore::Progress { change, timing })),
        );
    }

    /// Answers the idle barrier's round `round`, behind every tell placed
    /// before it, the ones the foreground hold keeps included, and every
    /// answer and report sent before it.
    fn idle_answer(&self, round: u64) {
        self.tells.place(Held::IdleAnswer(round));
    }

    /// Now, on the clock the listener's stamps are read from.
    fn now(&self) -> u64 {
        (self.clock.0)()
    }

    /// The timing of an event the listener's callback observed now.
    fn observed(&self) -> EventTiming {
        EventTiming {
            observed_at_us: self.now(),
            ..EventTiming::default()
        }
    }

    fn fault(&self, detail: String) {
        self.urgent(ListenerToCore::Fault { detail });
    }

    /// Takes nothing more, and has what waits written and the pipe closed,
    /// for the listener's shutdown.
    fn close(&self) {
        self.sender.close();
    }
}

/// What makes the listener's desktop-wide UIA subscriptions, on the
/// thread [`Listener::start`] starts for it, returning them, or `None`
/// when their installation failed.
type InstallSubscriptions = Box<
    dyn FnOnce(&Arc<Outgoing>, &Arc<Readers<UiaReading>>, &Arc<Checks>) -> Option<Registration>
        + Send,
>;

/// The focus listener: owns its subscriptions for the whole life of the
/// process.
struct Listener {
    outgoing: Arc<Outgoing>,
    /// Every application's reading thread, which answers Core's requests
    /// and reads bars.
    readers: Arc<Readers<UiaReading>>,
    /// The elements kept for tells (Table 6).
    kept: Arc<Kept>,
    /// Every application's check thread, which checks and tells its UI
    /// Automation events.
    checks: Arc<Checks>,
    /// The thread making the desktop-wide UIA registration, which returns
    /// it, or `None` when it failed ([`Listener::start`]).
    registering: JoinHandle<Option<Registration>>,
    /// What nudges the `WinEvent` thread: the idle barrier's query goes
    /// through it, behind every event delivered to it before.
    nudger: Nudger,
    _event_thread: EventThread,
}

impl Listener {
    /// Sets up the listener, sending through `sender`, with its
    /// desktop-wide UIA subscriptions ([`install_desktop_subscriptions`]):
    /// see [`Self::start`].
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    fn new(sender: Sender<Waiting>, ignored: IgnoredProcesses) -> io::Result<Self> {
        Self::start(
            sender,
            ignored,
            Box::new(|outgoing, readers, checks| {
                install_desktop_subscriptions(
                    (outgoing, readers, checks),
                    focus_subscription(outgoing, readers, checks),
                    menu_subscription(outgoing, readers, checks),
                )
            }),
        )
    }

    /// Sets up the listener, sending through `sender`: installs its
    /// `WinEvent` hooks, and then has `install` make the desktop-wide UIA
    /// subscriptions on a thread of their own, which announces readiness
    /// once `install` returns, so Core knows the `WinEvent`s and the UIA
    /// events are heard from then on; Verbatim takes input only once the
    /// listener is ready. The hooks go in first, and the installation runs
    /// on its own thread, so the pipe thread answers the supervisor's pings
    /// meanwhile, because UIA reads the system's focus for the focus
    /// registration and waits on the foreground application for it, up to
    /// about 6 seconds for one that does not answer
    /// (`docs/design/cross-process-calls-study.md`, instance 2). Their
    /// failure is reported as a fault, before the announcement. Every UIA
    /// event is dropped until `install` returns
    /// ([`Outgoing::uia_event_process`]).
    ///
    /// # Errors
    ///
    /// Returns the error of creating the foreground hold's timer, or of
    /// starting the thread that installs the UIA subscriptions.
    fn start(
        sender: Sender<Waiting>,
        ignored: IgnoredProcesses,
        install: InstallSubscriptions,
    ) -> io::Result<Self> {
        let outgoing = Arc::new(Outgoing::new(sender, ignored)?);
        Self::with_outgoing(outgoing, install)
    }

    /// [`start`](Self::start) sending through `outgoing`.
    ///
    /// # Errors
    ///
    /// Returns the error of starting the thread that installs the UIA
    /// subscriptions.
    fn with_outgoing(outgoing: Arc<Outgoing>, install: InstallSubscriptions) -> io::Result<Self> {
        let kept = Arc::new(Kept::default());
        let readers = Arc::new(Readers::new(UiaReading::new(&outgoing, &kept)));
        let checks = Arc::new(Checks::new(&outgoing, &readers, &kept));
        let idle_checks = Arc::clone(&checks);
        let msaa_outgoing = Arc::clone(&outgoing);
        let idle_readers = Arc::clone(&readers);
        let make_handlers: MakeHandlers = Arc::new(move || {
            let outgoing = Arc::clone(&msaa_outgoing);
            let idle_outgoing = Arc::clone(&msaa_outgoing);
            let readers = Arc::clone(&idle_readers);
            let event_readers = Arc::clone(&readers);
            let checks = Arc::clone(&idle_checks);
            // The window in front, for the keyboard hook's view of it,
            // kept on this thread by both handlers.
            let watch = Rc::new(RefCell::new(ForegroundWatch::default()));
            let start_watch = Rc::clone(&watch);
            // What the event thread knows of windows and objects, for its
            // value changes, on this thread alone.
            let knowledge = RefCell::new(ValueChangeKnowledge::new());
            // The executable name of each application that raised an
            // event, for its tells, on this thread alone.
            let applications = RefCell::new(tells::Applications::default());
            EventHandlers {
                win_event: Box::new(move |event: WinEvent| {
                    // Every event is told first, its arrival the time the
                    // callback received it; the foreground's watch and the
                    // progress path follow.
                    tell_win_event(
                        &outgoing.tells,
                        &mut applications.borrow_mut(),
                        event,
                        outgoing.now(),
                    );
                    route_win_event(&outgoing, &event_readers, &knowledge, &watch, event);
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
                        answer_when_reads_are_done(&readers, &checks, &idle_outgoing, round as u64);
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
        let registering = install_on_its_thread(&outgoing, &readers, &checks, install)?;
        Ok(Self {
            outgoing,
            readers,
            kept,
            checks,
            registering,
            nudger,
            _event_thread: event_thread,
        })
    }

    /// Shuts the listener down cleanly ([`CoreToListener::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA subscriptions'
    /// installation is waited for if it has not ended, and the handlers
    /// removed (the registering thread waits for a callback in progress);
    /// the check and reading threads end, one inside a call let go; what
    /// waits to be written to Core is written, the I/O thread `io` serves
    /// waiting for every write in flight to complete, and the pipe closed.
    fn shutdown(self, io: &mut ChildIo) {
        let started = Instant::now();
        let Self {
            outgoing,
            readers,
            kept: _,
            checks,
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
        checks.stop();
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

    /// Handles one of Core's commands. `received_at_us` is when it was read
    /// off Core's pipe: the hop from verbatim.exe's write is logged at trace
    /// level, under `verbatim::stage`.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn handle_command(&self, command: CoreToListener, received_at_us: u64) {
        if let Some(us) = hop_us(command.sent_at_us(), received_at_us) {
            tracing::trace!(target: "verbatim::stage", stage = "verbatim.exe to the listener", us, ?command);
        }
        match command {
            CoreToListener::Ping { .. } => {
                self.outgoing.urgent(ListenerToCore::Pong {
                    parked_count: 0,
                    written_at_us: 0,
                });
            }
            // The listener hands no application's events over to an
            // outpost, and keeps no unasked read to read again: every
            // `WinEvent` is told, and Core asks what it needs. The serving
            // loop ends on `Shutdown`, before it is handled here.
            CoreToListener::HookInstalled { .. }
            | CoreToListener::HookRemoved { .. }
            | CoreToListener::LiveReread { .. }
            | CoreToListener::Shutdown => {}
            CoreToListener::ApplicationExited { pid } => {
                self.checks.application_exited(pid);
                self.readers.application_exited(pid);
                self.outgoing.urgent(ListenerToCore::ExitHandled { pid });
            }
            CoreToListener::IdleQuery { round } => self.idle_query(round),
            CoreToListener::Request(ask) => self.request(ask),
            CoreToListener::Cancel(cancel) => self.readers.cancel(cancel),
        }
    }

    /// Core's request `ask` ([`ask`]).
    fn request(&self, ask: Box<Ask>) {
        self::ask((&self.readers, &self.kept, &self.checks), ask);
    }

    /// The idle barrier's query for `round` (`docs/design/communication.md`,
    /// section 6.7): handed to the `WinEvent` thread behind every event
    /// delivered to it, then to the reading threads behind every job queued
    /// for them, then to the check threads behind every event handed to
    /// them, and answered behind every tell placed before. A `WinEvent`
    /// thread that has ended has nothing in progress.
    fn idle_query(&self, round: u64) {
        let posted = usize::try_from(round).is_ok_and(|word| self.nudger.nudge((word, 0)));
        if !posted {
            answer_when_reads_are_done(&self.readers, &self.checks, &self.outgoing, round);
        }
    }
}

/// Core's request `ask`: queued for the reading thread of its object's
/// application, or answered at once when the listener holds no such
/// object or cannot read it. A focus read first names the readers' last
/// focus, so every other element a focus read read is let go (Table 6,
/// column C).
fn ask((readers, kept, checks): (&Readers<UiaReading>, &Kept, &Checks), ask: Box<Ask>) {
    let answer = |outcome: Outcome| {
        readers.reading().outgoing.answer(Answer {
            number: ask.number,
            outcome,
            probes: Vec::new(),
            detail: String::new(),
        });
    };
    let Request::ReadObject(read) = &ask.request else {
        answer(Outcome::Refused(Refusal::Unsupported));
        return;
    };
    let focus = match read.purpose {
        Purpose::Focus { keep } => {
            for (pid, released) in kept.focus_request(keep) {
                checks.release(pid, released);
            }
            true
        }
        Purpose::Other => false,
    };
    let pid = match &read.object {
        ObjectRef::Kept(token) => kept.owner(*token),
        ObjectRef::Msaa { window, .. } => {
            Some(Pid(window_pid(asked::hwnd_of(*window)))).filter(|pid| pid.0 != 0)
        }
        ObjectRef::Node(_) => {
            answer(Outcome::Refused(Refusal::Unsupported));
            return;
        }
    };
    let Some(pid) = pid else {
        answer(Outcome::NotThere);
        return;
    };
    let number = ask.number;
    readers.push(
        pid,
        JobKind::Request { number, focus },
        UiaJob::Request(ask),
    );
}

/// What the listener does with a `WinEvent` beside its tell: a value
/// change goes the progress path's way; a console's event, the shell's
/// foreground and a foreground event feed the watch of the window in
/// front, and a focus finds a pending foreground's window in front.
fn route_win_event(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    knowledge: &RefCell<ValueChangeKnowledge>,
    watch: &RefCell<ForegroundWatch>,
    event: WinEvent,
) {
    if routed_elsewhere(outgoing, readers, knowledge, event) {
        return;
    }
    let observed_at_us = outgoing.now();
    match event.kind {
        WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret => {
            let step = watch
                .borrow_mut()
                .console_event(event.hwnd, event.thread, observed_at_us);
            foreground_step(outgoing, step);
        }
        // Program Manager or the taskbar in front: a window that came to
        // the foreground for Core.
        WinEventKind::ShellForeground => {
            let front = Front {
                window: event.hwnd,
                thread: event.thread,
                console: false,
                observed_at_us,
            };
            let step = watch.borrow_mut().foreground(front, in_front(front.window));
            foreground_step(outgoing, step);
        }
        // A window not in front at its foreground event is sent once an
        // event finds it there, before the focus that found it.
        WinEventKind::Focus if watch.borrow().has_pending() => {
            let step = watch
                .borrow_mut()
                .event_seen(front_window(), observed_at_us);
            foreground_step(outgoing, step);
        }
        WinEventKind::Foreground => {
            let front = Front {
                window: event.hwnd,
                thread: event.thread,
                console: event.hwnd != 0 && window_class_name(event.hwnd) == CONSOLE_WINDOW_CLASS,
                observed_at_us,
            };
            let step = watch.borrow_mut().foreground(front, in_front(event.hwnd));
            foreground_step(outgoing, step);
        }
        _ => {}
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
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
    install: InstallSubscriptions,
) -> io::Result<JoinHandle<Option<Registration>>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    std::thread::Builder::new()
        .name("listener UIA subscriptions".to_owned())
        .spawn(move || {
            let started = Instant::now();
            let registration = install(&outgoing, &readers, &checks);
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

/// Answers the idle barrier's round `round` once no reading thread has a
/// job waiting or running, behind the answer of every request queued
/// before ([`Readers::when_idle`]), then once no check thread has an event
/// waiting or being checked, behind the tell of every one handed to it
/// before ([`Checks::when_idle`]), behind every tell placed before it.
fn answer_when_reads_are_done(
    readers: &Readers<UiaReading>,
    checks: &Arc<Checks>,
    outgoing: &Arc<Outgoing>,
    round: u64,
) {
    let outgoing = Arc::clone(outgoing);
    let checks = Arc::clone(checks);
    readers.when_idle(Box::new(move || {
        checks.when_idle(Box::new(move || outgoing.idle_answer(round)));
    }));
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
/// cross-process call: it hands the event to its application's check
/// thread ([`arrived`]), which tells Core of it.
fn focus_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    let callback = Arc::new(move |sender: &Cached<ListenerEvent>| {
        let fields = sender.read();
        let told = (EventKind::UiaFocus, None, Payload::None);
        arrived(&outgoing, &readers, &checks, (sender, &fields), told);
    });
    Subscription::Focus { callback }
}

/// The desktop-wide subscription to UIA menu openings, each handed to its
/// application's check thread as a focus is: Core judges it, as NVDA's
/// automation event handler maps it to a focus on the menu.
fn menu_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_MenuOpenedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            let told = (EventKind::UiaMenuOpened, None, Payload::None);
            arrived(&outgoing, &readers, &checks, (sender, &fields), told);
        }),
    }
}

/// A UI Automation event of `kind` whose sender is `sender`, read as
/// `fields`, arrived: with `states` and `payload`, what its kind carries
/// beyond its cached properties. Dropped as NVDA's handlers drop it before
/// any call: before the subscriptions are installed, with no process, or
/// with its sender's cached window hung or
/// ghosted as it arrives (a sender with no cached window is not tested,
/// `UIAHandler/utils.py` 446 to 450). Otherwise handed to its
/// application's check thread, which tells Core of it. Makes no
/// cross-process call.
fn arrived(
    outgoing: &Outgoing,
    readers: &Readers<UiaReading>,
    checks: &Arc<Checks>,
    (sender, fields): (&Cached<ListenerEvent>, &ListenerEvent),
    (kind, states, payload): (EventKind, Option<StateSet>, Payload),
) {
    let at_us = outgoing.now();
    if !outgoing.uia_installed() {
        tracing::debug!(
            ?kind,
            "UIA event dropped: received before the listener's UIA subscriptions were installed"
        );
        return;
    }
    let Some(pid) = fields.process_id.filter(|&pid| pid != 0).map(Pid) else {
        return;
    };
    if window_hung(&readers.reading().readability, fields.role.hwnd) {
        tracing::debug!(%pid, ?kind, hwnd = fields.role.hwnd, "UIA event dropped as it arrived: its window is hung");
        return;
    }
    if let Some(event) = CheckEvent::new(kind, (sender, fields), pid, at_us, (states, payload)) {
        checks.hand(pid, event);
    }
}

/// Whether `window`, an event's sender's cached window, is hung or
/// ghosted, by the listener's one readability reader; a sender with no
/// window, 0, is not tested. Local calls.
fn window_hung(readability: &WindowReadabilityReader, window: isize) -> bool {
    window != 0 && readability(window).is_hung()
}

/// Installs the desktop-wide UIA subscriptions, some of those NVDA
/// registers globally on Windows 11: `focus` ([`focus_subscription`]) on
/// the registration's client, then an element selected, `menu`
/// ([`menu_subscription`]), notifications, windows opened and range
/// values, all on one client as NVDA's global handlers are
/// (`verbatim_uia::Subscription::Focus` says why they must share it), each
/// part shaped as `verbatim_uia::ListenerEvent`: NVDA's base cache
/// request, with the two properties that refine a role for dialogs and
/// toggle buttons; an element selected also carries the item's value,
/// states, and details (`verbatim_uia::ListenerSelection`). A failure is
/// reported as a fault and leaves the `WinEvent`s running on their own.
fn install_desktop_subscriptions(
    (outgoing, readers, checks): (&Arc<Outgoing>, &Arc<Readers<UiaReading>>, &Arc<Checks>),
    focus: Subscription<Cached<ListenerEvent>>,
    menu: Subscription<Cached<ListenerEvent>>,
) -> Option<Registration> {
    let selection = selection_subscription(outgoing, readers, checks);
    let notifications = notification_subscription(outgoing, readers, checks);
    let toasts = toast_subscription(outgoing, readers, checks);
    let progress = progress_subscription(outgoing, readers);
    let parts = vec![
        Part::shaped(focus),
        Part::shaped::<ListenerSelection>(selection),
        Part::shaped(menu),
        Part::shaped(notifications),
        Part::shaped(toasts),
        Part::shaped::<ListenerProgress>(progress),
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

/// The desktop-wide subscription to UIA element selected events, each
/// handed to its application's check thread with the item's states from
/// its cache ([`arrived`]).
fn selection_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerSelection>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_SelectionItem_ElementSelectedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerSelection>| {
            let fields = sender.read();
            let Some((_, snapshot)) = capture_selection(sender, &fields) else {
                return;
            };
            let narrowed = sender.narrow();
            let told = (
                EventKind::UiaElementSelected,
                Some(snapshot.states),
                Payload::None,
            );
            arrived(
                &outgoing,
                &readers,
                &checks,
                (&narrowed, &fields.event),
                told,
            );
        }),
    }
}

/// The desktop-wide subscription to UIA notifications, each handed to its
/// application's check thread with its text, kind, processing and
/// activity ([`arrived`]), but a terminal text area's, which Core's rule
/// would drop, dropped here on its cached class and automation id.
fn notification_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
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
                let told = (
                    EventKind::UiaNotification,
                    None,
                    Payload::Notification {
                        text: display_string.unwrap_or_default(),
                        kind: notification_kind_from_uia(kind),
                        processing: notification_processing_from_uia(processing),
                        activity: activity_id.unwrap_or_default(),
                    },
                );
                arrived(&outgoing, &readers, &checks, (sender, &fields), told);
            },
        ),
    }
}

/// The desktop-wide subscription to UIA window openings, kept only for a
/// candidate for a Windows toast by what the event's cache answers of
/// NVDA's rule ([`is_toast_candidate`]), which Core's rule would otherwise
/// drop; a candidate is handed to its application's check thread
/// ([`arrived`]). It runs once per window opened anywhere and makes no
/// call.
fn toast_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_Window_WindowOpenedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            if !is_toast_candidate(fields.role.control_type, fields.automation_id.as_deref()) {
                tracing::debug!(
                    control_type = ?fields.role.control_type,
                    automation_id = ?fields.automation_id,
                    "a window opened is no toast"
                );
                return;
            }
            let told = (EventKind::UiaWindowOpened, None, Payload::None);
            arrived(&outgoing, &readers, &checks, (sender, &fields), told);
        }),
    }
}

/// The desktop-wide subscription to UIA range value changes, the listener's
/// path for every UIA progress bar, foreground or background
/// (`docs/design/progress-bars.md`, "Where each part runs"). Its callback
/// makes no cross-process call: an element whose cached control type is not
/// a progress bar, whose window's provider is not native (UIA's proxy for a
/// classic window, heard by the hook instead), or whose window is hung is
/// dropped, and a range value that is not a number too, parsed once here;
/// the bar is then queued for its application's reading thread, behind
/// Core's requests, which reads where it is and sends it on, unasked.
fn progress_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
) -> Subscription<Cached<ListenerProgress>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    Subscription::Properties {
        properties: vec![UIA_RangeValueValuePropertyId],
        callback: Arc::new(move |sender: &Cached<ListenerProgress>, _| {
            let fields = sender.read();
            let (Some(pid), Some(percent)) = (
                outgoing.uia_event_process(fields.event.process_id),
                bar_percent(&fields),
            ) else {
                return;
            };
            let hwnd = fields.event.role.hwnd;
            if window_hung(&readers.reading().readability, hwnd) {
                tracing::debug!(%pid, hwnd, "a progress bar's change dropped: its window is hung");
                return;
            }
            let timing = outgoing.observed();
            match sender.agile() {
                Ok(element) => readers.push(
                    pid,
                    JobKind::UiaBar {
                        key: sender.runtime_id(),
                    },
                    UiaJob::UiaBar {
                        element,
                        hwnd,
                        percent,
                        report: report_bar(&outgoing, timing),
                    },
                ),
                Err(error) => {
                    tracing::warn!(%error, "a progress bar's element could not be kept");
                }
            }
        }),
    }
}

/// What the callback of [`progress_subscription`] makes of ields, an
/// event's cache: the percentage, when the element is a progress bar with a
/// native provider and a range that is a number; `None` for anything else,
/// a slider's range value among them. Public so that mockapp's tests judge
/// mockapp's own event as the listener judges it.
#[must_use]
pub fn bar_percent(fields: &ListenerProgress) -> Option<Percent> {
    percent_of_bar(
        fields.event.role.control_type,
        fields.event.provider_description.as_deref(),
        (fields.value, fields.minimum, fields.maximum),
    )
}

/// [`bar_percent`], from what a callback reads of an event's cache.
fn percent_of_bar(
    control_type: Option<i32>,
    provider: Option<&str>,
    (value, minimum, maximum): (Option<f64>, Option<f64>, Option<f64>),
) -> Option<Percent> {
    if control_type != Some(UIA_ProgressBarControlTypeId.0)
        || provider.is_some_and(served_only_by_uia_proxies)
    {
        return None;
    }
    Percent::of_range(value?, minimum?, maximum?)
}

/// What a bar's change is reported through: sent to Core when the read
/// answers it, dropped when it does not, with its stamp from the event's
/// timing, handled now.
fn report_bar(outgoing: &Arc<Outgoing>, timing: EventTiming) -> Responder<BarChange> {
    let reported = Arc::clone(outgoing);
    Responder::new(move |change: Option<BarChange>| {
        if let Some(change) = change {
            reported.progress(change, timing);
        }
    })
}

/// A classic value change, for the listener's progress bar path (the
/// hook's filter, `docs/design/progress-bars.md`, "Rules"), with no call
/// into the application: its window is routed to its process, a console
/// window's to its console host ([`routed_process`]), and a hung window,
/// an ignored process, a hidden window of a listed class, and what
/// `knowledge` says is dropped are dropped here with no lock; the rest
/// queued on the window's application's reading thread, behind Core's
/// requests, one held per object, a newer change replacing it.
fn value_change_event(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    knowledge: &mut ValueChangeKnowledge,
    event: WinEvent,
) {
    knowledge.drain();
    let WinEvent {
        hwnd,
        id_object,
        id_child,
        thread,
        ..
    } = event;
    if hwnd == 0 {
        return;
    }
    let (owner_thread, owner) = verbatim_win32::window::window_owner(hwnd);
    if owner == 0 {
        return;
    }
    let class = window_class_name(hwnd);
    let Some(pid) = routed_process(&class, owner, || process_of_thread(thread)).map(Pid) else {
        return;
    };
    if outgoing.ignored.contains(pid) || outgoing.ignored.contains(Pid(owner)) {
        return;
    }
    if window_hung(&readers.reading().readability, hwnd) {
        tracing::debug!(%pid, hwnd, "a value change dropped: its window is hung");
        return;
    }
    let object = BarObject {
        window: hwnd,
        object: id_object,
        child: id_child,
    };
    let at_us = outgoing.now();
    let listed = verbatim_reader_rules::class::is_progress_bar_class(
        &verbatim_reader_rules::class::normalize_class_name(&class),
    );
    let (read, window) = if listed {
        let visible = verbatim_win32::window::window_is_visible(hwnd);
        let Some(read) = classic_value_change(true, visible, None) else {
            return;
        };
        (read, nothing_asked())
    } else {
        match knowledge.judge(&ValueChange {
            pid,
            object,
            owner: owner_thread,
            at_us,
        }) {
            Judgement::Dropped => return,
            Judgement::Read(read) => (read, nothing_asked()),
            Judgement::Ask(job) => (
                ClassicBarRead::Probe,
                window_answer(
                    outgoing,
                    knowledge.sender(),
                    pid,
                    (hwnd, owner_thread, at_us),
                    job,
                ),
            ),
        }
    };
    let timing = outgoing.observed();
    readers.push(
        pid,
        JobKind::ClassicBar {
            raiser: BarRaiser {
                raised_by: thread,
                focused: knowledge.is_focused(object),
            },
            key: bar_key(object).to_vec(),
        },
        UiaJob::ClassicBar {
            object,
            read,
            report: report_bar(outgoing, timing),
            learn: learn_bar(knowledge.sender(), pid, object, owner_thread),
            window,
        },
    );
}

/// What answers a job that asks no window: it is dropped unanswered.
fn nothing_asked() -> Responder<WindowAnswer> {
    Responder::new(|_: Option<WindowAnswer>| {})
}

/// What job number `job`'s window answer goes through, for window `hwnd`,
/// owned by thread `owner`, its keep judged from `from_us`: the event
/// thread's record takes the answer, with its keep ended from the probe's
/// end, or, dropped unanswered on any path that ends the job, learns that
/// the window is no longer being asked.
fn window_answer(
    outgoing: &Arc<Outgoing>,
    found: FoundSender<Found>,
    pid: Pid,
    (hwnd, owner, from_us): (isize, u32, u64),
    job: u64,
) -> Responder<WindowAnswer> {
    let clock = outgoing.clock;
    Responder::new(move |answer: Option<WindowAnswer>| {
        let found_now = match answer {
            Some(answer) => Found::Window {
                pid,
                job,
                probe: Probe {
                    hwnd,
                    owner,
                    from_us,
                    probed: probed_of(answer, (clock.0)()),
                },
            },
            None => Found::NotAsked { pid, hwnd, job },
        };
        let _ = found.send(found_now);
    })
}

/// The limiter's key for a classic value change: its window and object.
fn bar_key(object: BarObject) -> [i32; 4] {
    let window = object.window.to_ne_bytes();
    [
        i32::from_ne_bytes([window[0], window[1], window[2], window[3]]),
        i32::from_ne_bytes([window[4], window[5], window[6], window[7]]),
        object.object,
        object.child,
    ]
}

/// What the event thread's record is told an object was found to be.
fn learn_bar(
    found: FoundSender<Found>,
    pid: Pid,
    object: BarObject,
    owner: u32,
) -> Responder<BarVerdict> {
    Responder::new(move |verdict: Option<BarVerdict>| {
        if let Some(verdict) = verdict {
            let _ = found.send(Found::Object {
                pid,
                object,
                owner,
                verdict,
            });
        }
    })
}

/// Reads one classic value change on its application's reading thread, as
/// `read` says, sends a bar's change through `report`, what a read found
/// the object to be through `learn`, and what a [`ClassicBarRead::Probe`]
/// found its window to be through `window`, dropped unanswered when the
/// probe learned nothing. Public so that mockapp's tests read mockapp's own
/// windows as the listener reads them.
pub fn read_classic_bar(
    object: BarObject,
    read: ClassicBarRead,
    report: Responder<BarChange>,
    learn: Responder<BarVerdict>,
    window: Responder<WindowAnswer>,
) {
    if read == ClassicBarRead::Listed {
        if let Some(percent) = verbatim_ia2::classic_bar_percent(object.window)
            && let Some(place) = window_centre(object.window)
        {
            report.answer(BarChange {
                percent,
                place,
                in_foreground: in_foreground(object.window),
            });
        }
        return;
    }
    // A window's answer: a native UIA window's bar comes through the group,
    // as NVDA's own hook drops this event (`internalWinEventHandler.py` 570
    // to 583).
    let read = if read == ClassicBarRead::Probe {
        let Some(answer) = window_judged(object.window) else {
            return;
        };
        window.answer(answer);
        if answer == WindowAnswer::Uia {
            return;
        }
        ClassicBarRead::Unknown
    } else {
        read
    };
    let Some(mut bar) =
        verbatim_ia2::acquire::event_object(object.window, object.object, object.child)
    else {
        return;
    };
    if read == ClassicBarRead::Unknown && bar.role() != Role::ProgressBar {
        learn.answer(BarVerdict::NotABar);
        return;
    }
    if read == ClassicBarRead::Unknown {
        learn.answer(BarVerdict::Bar);
    }
    if let Some(reading) = bar.bar_reading()
        && let Some(percent) = reading.value.as_deref().and_then(Percent::parse)
    {
        report.answer(BarChange {
            percent,
            place: reading.rect.map_or((0, 0), |rect| {
                (
                    rect.left.saturating_add(rect.width / 2),
                    rect.top.saturating_add(rect.height / 2),
                )
            }),
            in_foreground: in_foreground(object.window),
        });
    }
}

/// What NVDA's test of `hwnd`'s backend answers, on a reading thread: by
/// its class lists, then the raw probe and the checks after it, as the
/// outposts' arbitrator judges it. `None` when the probe learned nothing.
/// A console with a provider is answered [`WindowAnswer::Uia`] with no check
/// of its provider: `crate::value_change` says why.
fn window_judged(hwnd: isize) -> Option<WindowAnswer> {
    let classes = verbatim_win32::window::window_classes(hwnd);
    match class_verdict(&classes) {
        ClassVerdict::Uia => return Some(WindowAnswer::Uia),
        ClassVerdict::NonUia => return Some(WindowAnswer::Excluded),
        ClassVerdict::Unknown => {}
    }
    let probed = verbatim_uia::raw_probe(hwnd);
    if probed == verbatim_uia::RawProbe::Abandoned {
        return None;
    }
    if probed == verbatim_uia::RawProbe::TimedOut {
        return Some(WindowAnswer::TimedOut);
    }
    if !probed.has_provider() {
        return Some(WindowAnswer::NotUia);
    }
    Some(match post_probe_check(&classes) {
        Some(PostProbeCheck::Console) | None => WindowAnswer::Uia,
        Some(PostProbeCheck::WindowsFormsListView) if has_windows_forms_class(&classes.raw) => {
            WindowAnswer::Uia
        }
        Some(PostProbeCheck::WindowsFormsListView) => WindowAnswer::Excluded,
    })
}

/// The centre of `window` on the screen, a local call; `None` for a window
/// already gone.
fn window_centre(window: isize) -> Option<(i32, i32)> {
    let rect = verbatim_win32::window::rect(window)?;
    Some((
        rect.left.saturating_add((rect.right - rect.left) / 2),
        rect.top.saturating_add((rect.bottom - rect.top) / 2),
    ))
}

/// Whether `window` is the foreground window or beneath it, as NVDA judges
/// a bar's (`NVDAObjects/window/__init__.py` 295 to 297), read here where
/// the event arrives, since the relation is a local call only there.
fn in_foreground(window: isize) -> bool {
    let front = front_window();
    front != 0 && (window == front || verbatim_win32::window::is_child(front, window))
}

/// A value change, taken by the progress path: whether `event` was one,
/// already handled. An MSAA focus is noted for the value changes' limits
/// and goes on.
fn routed_elsewhere(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    knowledge: &RefCell<ValueChangeKnowledge>,
    event: WinEvent,
) -> bool {
    match event.kind {
        WinEventKind::Focus => {
            knowledge.borrow_mut().focus_moved(BarObject {
                window: event.hwnd,
                object: event.id_object,
                child: event.id_child,
            });
            false
        }
        WinEventKind::ValueChange => {
            value_change_event(outgoing, readers, &mut knowledge.borrow_mut(), event);
            true
        }
        _ => false,
    }
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
/// (`books/dev/src/reference/crates/verbatim-outpost.md`, "Console windows and frames of another
/// process"). `None` when the host
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
/// thread that produces a message writes it itself. Returns once Core asks
/// it to shut down or its command pipe ends, after a clean shutdown that
/// waits for every write in flight. Both ends must be overlapped.
/// `ignored` names the processes none of whose events is told.
///
/// # Errors
///
/// Returns the error of setting up the pipes or the listener, or any I/O
/// error reading the command stream, after the shutdown.
pub fn run_listener(
    pipe_in: OwnedHandle,
    pipe_out: OwnedHandle,
    ignored: IgnoredProcesses,
) -> io::Result<()> {
    let (mut io, sender) = ChildIo::new(pipe_in, pipe_out, Waiting::default())?;
    let listener = Listener::new(sender.clone(), ignored)?;
    // This thread is the listener's I/O thread from here on.
    prioritize_role("listener I/O", IO_PRIORITY);
    let served = io.serve(&sender, |body| {
        // The command's receipt, one clock read, for the hop from
        // verbatim.exe's write.
        let received_at_us = now_us();
        match decode_message::<CoreToListener>(body)? {
            CoreToListener::Shutdown => return Ok(Serving::Stop),
            command => listener.handle_command(command, received_at_us),
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
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::sync::{Mutex, PoisonError};

    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::EVENT_PIPE_BUFFER;

    use super::*;
    use verbatim_model::WindowHandle;
    use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, RuntimeId};
    use verbatim_model::request::{NodeRead, ObjectRead, ReadObject, Reply, RequestNumber};
    use verbatim_model::tell::{ElementToken, EventObject, TellWindow};
    use verbatim_process::message::read_message;
    use verbatim_win32::window::Readability;

    /// An `Outgoing` writing to a pipe with a buffer of `buffer` bytes, its
    /// writes that pend finished by a thread of the sender's own, as the
    /// I/O thread finishes them, telling nothing of the processes
    /// `ignored` holds; and the pipe's other end, which the test reads.
    fn outgoing_with(buffer: u32, ignored: IgnoredProcesses) -> (Outgoing, BufReader<File>) {
        let (ours, theirs) = pipe(Flow::Outbound, buffer, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        (
            Outgoing::untimed(sender, ignored),
            BufReader::new(File::from(theirs)),
        )
    }

    fn outgoing() -> (Outgoing, BufReader<File>) {
        outgoing_with(EVENT_PIPE_BUFFER, IgnoredProcesses::default())
    }

    /// Everything `outgoing` sent, which is closed first, as Core reads it.
    fn sent(outgoing: &Outgoing, reader: &mut BufReader<File>) -> Vec<ListenerToCore> {
        outgoing.close();
        std::iter::from_fn(|| read_message(reader).expect("read")).collect()
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

    /// The desktop window, which the desktop's root element has as its
    /// own window.
    fn desktop_window() -> isize {
        // SAFETY: GetDesktopWindow has no preconditions.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize
    }

    /// The desktop window as the model names it.
    fn desktop_handle() -> WindowHandle {
        WindowHandle(u64::try_from(desktop_window()).expect("a handle"))
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

    /// A provider that is UIA's proxy for a classic window is dropped, so
    /// a classic bar proxied through UIA is heard once, from the hook; a
    /// slider by its control type; and a range that is no number.
    #[test]
    fn the_callback_keeps_only_a_native_numeric_progress_bar() {
        const PROXIED: &str = "Main:Microsoft: MSAA Proxy (unmanaged:uiautomationcore.dll)";
        const NATIVE: &str = "Main:App provider (unmanaged:app.dll)";
        let bar = Some(UIA_ProgressBarControlTypeId.0);
        let of = |control_type, provider, value| {
            percent_of_bar(
                control_type,
                Some(provider),
                (value, Some(0.0), Some(1000.0)),
            )
        };
        assert_eq!(of(bar, NATIVE, Some(500.0)), Percent::parse("50"));
        assert_eq!(of(bar, PROXIED, Some(500.0)), None, "a proxied classic bar");
        assert_eq!(of(Some(0), NATIVE, Some(500.0)), None, "a slider");
        assert_eq!(of(bar, NATIVE, None), None, "a non-numeric value");
    }

    /// Whether `readers` hold no job: the idle barrier's waiter runs at
    /// once, under their lock.
    fn nothing_queued(readers: &Readers<UiaReading>) -> bool {
        let ran = Arc::new(AtomicBool::new(false));
        let set = Arc::clone(&ran);
        readers.when_idle(Box::new(move || set.store(true, Ordering::SeqCst)));
        ran.load(Ordering::SeqCst)
    }

    /// A hung window's value change is dropped in the hook, with no read
    /// queued, as a UIA event from one is dropped as it arrives.
    #[test]
    fn a_hung_windows_value_change_is_dropped_in_the_hook() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let hwnd = desktop_window();
        value_change_event(
            &parts.outgoing,
            &parts.readers,
            &mut ValueChangeKnowledge::new(),
            value_change(hwnd),
        );
        assert_eq!(
            asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            [hwnd]
        );
        assert!(nothing_queued(&parts.readers), "no read was queued");
        parts.stop();
    }

    /// A value change on a window of an ignored process is dropped before
    /// the readability test.
    #[test]
    fn an_ignored_processes_value_change_is_dropped_in_the_hook() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
        };
        // SAFETY: a plain window of a system class, destroyed below.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                None,
                WINDOW_STYLE(0),
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
        .expect("a window of this process");
        let hwnd = window.0 as isize;
        let (outgoing, _reader) = outgoing_with(
            EVENT_PIPE_BUFFER,
            IgnoredProcesses::hold([Pid(window_pid(hwnd))]),
        );
        let outgoing = Arc::new(outgoing);
        let kept = Arc::new(Kept::default());
        let readers = Arc::new(Readers::new(UiaReading::new(&outgoing, &kept)));
        value_change_event(
            &outgoing,
            &readers,
            &mut ValueChangeKnowledge::new(),
            value_change(hwnd),
        );
        assert!(nothing_queued(&readers), "no read was queued");
        readers.stop();
        // SAFETY: the window this test made, on its own thread.
        unsafe { DestroyWindow(window) }.expect("the window is destroyed");
    }

    /// What a job that asks a window answers through: dropped unanswered,
    /// as on every path that ends a job with no probe, the window is asked
    /// no more and nothing is recorded; answered, its answer is kept.
    #[test]
    fn a_jobs_window_answer_clears_asking_on_every_ending() {
        const WINDOW: isize = 0x10;
        let (outgoing, _reader) = outgoing_with(EVENT_PIPE_BUFFER, IgnoredProcesses::default());
        let outgoing = Arc::new(outgoing);
        let mut knowledge = ValueChangeKnowledge::new();
        let change = |at_us| ValueChange {
            pid: Pid(1),
            object: BarObject {
                window: WINDOW,
                object: 5,
                child: 0,
            },
            owner: 7,
            at_us,
        };
        let answer = |knowledge: &ValueChangeKnowledge, job| {
            window_answer(&outgoing, knowledge.sender(), Pid(1), (WINDOW, 7, 0), job)
        };

        assert_eq!(knowledge.judge(&change(0)), Judgement::Ask(1));
        assert_eq!(knowledge.judge(&change(1)), Judgement::Dropped, "asked");
        drop(answer(&knowledge, 1));
        knowledge.drain();
        assert_eq!(knowledge.judge(&change(2)), Judgement::Ask(2), "cleared");

        answer(&knowledge, 2).answer(WindowAnswer::Uia);
        knowledge.drain();
        assert_eq!(knowledge.judge(&change(3)), Judgement::Dropped, "kept");
    }

    /// A value change at the client of `hwnd`, as the global hook delivers it.
    fn value_change(hwnd: isize) -> WinEvent {
        WinEvent {
            kind: WinEventKind::ValueChange,
            event: windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_VALUECHANGE,
            hwnd,
            id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
            id_child: 0,
            thread: 0,
            time: 0,
            raised_ms_ago: 0,
        }
    }

    /// The listener's sending side, taking UIA events, its kept elements,
    /// readers and check threads, and what it sends, read on a thread of
    /// its own.
    struct Listening {
        outgoing: Arc<Outgoing>,
        kept: Arc<Kept>,
        readers: Arc<Readers<UiaReading>>,
        checks: Arc<Checks>,
        sent: mpsc::Receiver<ListenerToCore>,
    }

    impl Listening {
        /// Stops the check and reading threads and closes the sending
        /// side: every message sent from now on, in order.
        fn stop(self) -> Vec<ListenerToCore> {
            self.checks.stop();
            self.readers.stop();
            self.outgoing.close();
            self.sent.iter().collect()
        }

        /// The next message sent within 20 seconds.
        fn next(&self) -> ListenerToCore {
            self.sent
                .recv_timeout(Duration::from_secs(20))
                .expect("a message")
        }
    }

    /// [`Listening`], whose readability reader answers the `n`th window it
    /// is asked, counted from 0, as `answer` says, each window asked kept in
    /// `asked`, and whose MSAA name reader is the system's.
    fn listening(answer: fn(usize) -> Readability, asked: &Arc<Mutex<Vec<isize>>>) -> Listening {
        listening_named(answer, asked, verbatim_ia2::acquire::name_at_event)
    }

    /// [`listening`] with the MSAA name reader `name_at_event`.
    fn listening_named(
        answer: fn(usize) -> Readability,
        asked: &Arc<Mutex<Vec<isize>>>,
        name_at_event: fn(isize, i32, i32) -> Option<String>,
    ) -> Listening {
        let (outgoing, reader) = outgoing();
        outgoing.uia_installation_ended();
        let asked = Arc::clone(asked);
        let readability: WindowReadabilityReader = Arc::new(move |window| {
            let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
            asked.push(window);
            answer(asked.len() - 1)
        });
        let outgoing = Arc::new(outgoing);
        let kept = Arc::new(Kept::default());
        let readers = Arc::new(Readers::new(UiaReading {
            outgoing: Arc::clone(&outgoing),
            kept: Arc::clone(&kept),
            readability,
            name_at_event,
        }));
        let checks = Arc::new(Checks::new(&outgoing, &readers, &kept));
        Listening {
            outgoing,
            kept,
            readers,
            checks,
            sent: drain(reader),
        }
    }

    /// Core's generic read of `object`, the request `number`, naming
    /// `window`.
    fn read_of(number: u64, window: WindowHandle, read: ReadObject) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window,
            verdict: verbatim_model::request::WindowVerdict::Uia,
            request: Request::ReadObject(read),
        })
    }

    /// The desktop window's name at its own address, which the injected
    /// name reader answers "Desktop 2", is read for Core's request on the
    /// reading thread of the desktop window's process, csrss's, and
    /// answered with the window's facts; a set beyond the name is refused,
    /// and an address with no name answered not there.
    #[test]
    fn the_desktop_windows_name_is_read_for_cores_request() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening_named(
            |_| Readability::Readable,
            &asked,
            |hwnd, object, child| {
                (hwnd == desktop_window()
                    && object == windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0
                    && child == 0)
                    .then(|| "Desktop 2".to_owned())
            },
        );
        let name_of = |object, set| ReadObject {
            object: ObjectRef::Msaa {
                window: desktop_handle(),
                object,
                child: 0,
            },
            set,
            relation: None,
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        };
        let client = windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0;
        let request = |number, read| {
            ask(
                (&parts.readers, &parts.kept, &parts.checks),
                read_of(number, desktop_handle(), read),
            );
        };
        request(1, name_of(client, vec![PropertyId::Name]));
        assert_eq!(
            parts.next(),
            ListenerToCore::Answer(Answer {
                number: RequestNumber(1),
                outcome: Outcome::Result(Reply::Object(Box::new(ObjectRead {
                    object: NodeRead {
                        node: None,
                        window: verbatim_listener_logic::tell::node_window(
                            &tells::SystemReads,
                            desktop_window()
                        ),
                        values: verbatim_model::property::PropertyValues(vec![PropertyAnswer {
                            id: PropertyId::Name,
                            value: Some(PropertyValue::Text("Desktop 2".to_owned())),
                        }]),
                    },
                    relation: None,
                    descendants: Vec::new(),
                    ancestors: Vec::new(),
                    met: None,
                    route: None,
                }))),
                probes: Vec::new(),
                detail: String::new(),
            })
        );
        request(2, name_of(client, vec![PropertyId::Name, PropertyId::Role]));
        let window = windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW.0;
        request(3, name_of(window, vec![PropertyId::Name]));
        let outcomes: Vec<(u64, Outcome)> = [parts.next(), parts.next()]
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => (answer.number.0, answer.outcome),
                other => panic!("an answer: {other:?}"),
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                (2, Outcome::Refused(Refusal::Unsupported)),
                (3, Outcome::NotThere)
            ]
        );
        assert_eq!(parts.stop(), []);
    }

    /// A request on an element the listener does not keep, or a node it
    /// does not hold, is answered at once.
    #[test]
    fn a_request_on_nothing_the_listener_holds_is_answered_at_once() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let read = |object| ReadObject {
            object,
            set: vec![PropertyId::Name],
            relation: None,
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        };
        ask(
            (&parts.readers, &parts.kept, &parts.checks),
            read_of(1, desktop_handle(), read(ObjectRef::Kept(ElementToken(9)))),
        );
        ask(
            (&parts.readers, &parts.kept, &parts.checks),
            read_of(
                2,
                desktop_handle(),
                read(ObjectRef::Node(verbatim_model::NodeId::new(1))),
            ),
        );
        let outcomes: Vec<(u64, Outcome)> = parts
            .stop()
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => (answer.number.0, answer.outcome),
                other => panic!("an answer: {other:?}"),
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                (1, Outcome::NotThere),
                (2, Outcome::Refused(Refusal::Unsupported))
            ]
        );
    }

    /// A UIA focus whose sender's cached window the listener's reader
    /// answers hung as it arrives is dropped in the callback: nothing is
    /// told, and the reader was asked of the sender's window once.
    #[test]
    fn a_focus_whose_window_is_hung_as_it_arrives_is_dropped_in_the_callback() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        assert!(
            sender.read().process_id.is_some_and(|pid| pid != 0),
            "the sender has a process, so only its window can drop it"
        );
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()]
        );
    }

    /// A UIA focus whose sender's window hangs after it arrives is checked
    /// and told with no second test of its window by the listener's
    /// readability reader: asked once, as the focus arrived.
    #[test]
    fn a_focus_whose_window_hangs_after_it_arrives_is_told() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(
            |nth| {
                if nth == 0 {
                    Readability::Readable
                } else {
                    Readability::Hung
                }
            },
            &asked,
        );
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let ListenerToCore::Tell(tell) = parts.next() else {
            panic!("a tell");
        };
        assert_eq!(tell.kind, EventKind::UiaFocus);
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()],
            "asked as it arrived, and not by its check"
        );
    }

    /// The next tell `parts` sends.
    fn next_tell(parts: &Listening) -> Box<verbatim_model::tell::Event> {
        match parts.next() {
            ListenerToCore::Tell(tell) => tell,
            other => panic!("the listener sent {other:?}, not a tell"),
        }
    }

    /// A UIA focus is checked on its application's check thread and told
    /// with what the check found: the walk to its nearest window, here the
    /// desktop's own, and the probe of that window, which is on neither of
    /// NVDA's lists and has no provider, so the check makes no other call,
    /// as NVDA's handler returns at a window read through MSAA; its element
    /// is kept behind the token the tell names. A second focus on the same
    /// element finds the probe's answer in the thread's record and makes no
    /// call past the walk.
    #[test]
    fn a_focus_is_told_from_its_check_thread_with_what_its_check_found() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        let first = next_tell(&parts);
        callback(&sender);
        let second = next_tell(&parts);
        drop(callback);
        let runtime_id = RuntimeId(sender.runtime_id());
        let desktop = desktop_handle();
        let (desktop_thread, _) = verbatim_win32::window::window_owner(desktop_window());
        assert_eq!(first.kind, EventKind::UiaFocus);
        assert_eq!(
            first.object,
            EventObject::Uia {
                token: ElementToken(1),
                runtime_id: runtime_id.clone(),
            }
        );
        assert_eq!(first.thread, desktop_thread);
        let TellWindow::Found(facts) = &first.window else {
            panic!("the desktop window: {:?}", first.window);
        };
        assert_eq!(facts.window.handle, desktop);
        assert!(facts.desktop);
        let checked = first.checked.as_ref().expect("checked");
        let probe = checked.probe.expect("the desktop window probed");
        assert_eq!(
            (probe.window, probe.thread, probe.provider),
            (desktop, desktop_thread, false)
        );
        assert_eq!(checked.keyboard_focus, None);
        assert_eq!(checked.kept, Vec::new());
        assert_eq!(checked.previous_sibling, None);
        assert_eq!(
            second.object,
            EventObject::Uia {
                token: ElementToken(2),
                runtime_id,
            }
        );
        assert_eq!(
            second.checked,
            Some(verbatim_model::tell::Checked {
                probe: None,
                keyboard_focus: None,
                kept: Vec::new(),
                previous_sibling: None,
            })
        );
        parts.stop();
    }

    /// Core's read of a kept element, here the desktop's root element a
    /// focus event named, is answered on its application's reading thread
    /// with its properties by the set, from the same element the event
    /// carried; a read that is not a focus read lets the element go, so a
    /// second read of its token is answered not there.
    #[test]
    fn a_kept_element_is_read_once_for_cores_request() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let tell = next_tell(&parts);
        let EventObject::Uia { token, .. } = tell.object else {
            panic!("a kept element: {:?}", tell.object);
        };
        let set = vec![
            PropertyId::Name,
            PropertyId::NativeWindow,
            PropertyId::Process,
            PropertyId::RuntimeId,
        ];
        let read = ReadObject {
            object: ObjectRef::Kept(token),
            set: set.clone(),
            relation: None,
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        };
        for number in [1, 2] {
            ask(
                (&parts.readers, &parts.kept, &parts.checks),
                read_of(number, desktop_handle(), read.clone()),
            );
        }
        let fields = sender.read();
        let pid = Pid(fields.process_id.expect("a process"));
        let expected = verbatim_model::property::PropertyValues(vec![
            PropertyAnswer {
                id: PropertyId::Name,
                value: fields
                    .parts(sender.runtime_id())
                    .name
                    .map(PropertyValue::Text),
            },
            PropertyAnswer {
                id: PropertyId::NativeWindow,
                value: Some(PropertyValue::Window(desktop_handle())),
            },
            PropertyAnswer {
                id: PropertyId::Process,
                value: Some(PropertyValue::Process(pid)),
            },
            PropertyAnswer {
                id: PropertyId::RuntimeId,
                value: Some(PropertyValue::RuntimeId(RuntimeId(sender.runtime_id()))),
            },
        ]);
        let ListenerToCore::Answer(first) = parts.next() else {
            panic!("an answer");
        };
        let Outcome::Result(Reply::Object(object)) = first.outcome else {
            panic!("a result: {:?}", first.outcome);
        };
        assert_eq!(first.number, RequestNumber(1));
        assert_eq!(object.object.values, expected);
        assert_eq!(
            object.object.window,
            verbatim_listener_logic::tell::node_window(&tells::SystemReads, desktop_window())
        );
        assert_eq!(object.route, None, "no walk asked");
        assert_eq!(
            parts.next(),
            ListenerToCore::Answer(Answer {
                number: RequestNumber(2),
                outcome: Outcome::NotThere,
                probes: Vec::new(),
                detail: String::new(),
            })
        );
        parts.stop();
    }

    /// A selection and a notification whose sender's cached window the
    /// listener's reader answers hung as they arrive are dropped in their
    /// callbacks: nothing is told.
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
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Event { callback, .. } =
            selection_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the selection subscription");
        };
        callback(&selected);
        drop(callback);
        let Subscription::Notifications { callback } =
            notification_subscription(&parts.outgoing, &parts.readers, &parts.checks)
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
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window(), desktop_window()]
        );
    }

    /// A window opened that is not a toast by NVDA's rule costs only the
    /// callback: nothing is told and no window is tested.
    #[test]
    fn a_window_opened_that_is_no_toast_is_not_told() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let opened = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Event { callback, .. } =
            toast_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the window-opened subscription");
        };
        callback(&opened);
        drop(callback);
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            Vec::<isize>::new()
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
    /// meanwhile is told with no `Ready` before it, UIA events are dropped,
    /// and `Ready` arrives once the installation is released.
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
        let install: InstallSubscriptions = Box::new(move |_, _, _| {
            // Released once the test has read the alert below; the bound
            // only ends a run whose alert is never told, which then fails
            // on the `Ready` that follows.
            let _ = released.recv_timeout(Duration::from_secs(10));
            None
        });
        let listener = Listener::start(sender, IgnoredProcesses::default(), install)
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
        let alert = EventObject::Msaa {
            window: WindowHandle(u64::try_from(window.0 as isize).expect("a handle")),
            object: OBJID_CLIENT.0,
            child: CHILDID_SELF.cast_signed(),
        };
        // The global hooks tell this desktop's events too: the first of
        // the alert and `Ready` to arrive, `Ready` as `None`.
        let first =
            std::iter::from_fn(|| read_message(&mut reader).expect("read")).find_map(|message| {
                match message {
                    ListenerToCore::Ready { .. } => Some(None),
                    ListenerToCore::Tell(tell)
                        if tell.kind == EventKind::Alert && tell.object == alert =>
                    {
                        Some(Some(tell.object))
                    }
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
            checks,
            registering,
            _event_thread: event_thread,
            ..
        } = listener;
        drop(event_thread);
        assert!(registering.join().expect("the installation ends").is_none());
        checks.stop();
        readers.stop();
        outgoing.close();
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }

    /// The idle barrier's answer waits behind every answer and report sent
    /// before it, a large one included whose write pends, and a pong goes
    /// ahead of them.
    #[test]
    fn the_idle_answer_waits_behind_every_answer_and_report_before_it() {
        let (outgoing, mut reader) = outgoing_with(4096, IgnoredProcesses::default());
        let answer = |number, detail: String| Answer {
            number: RequestNumber(number),
            outcome: Outcome::NotThere,
            probes: Vec::new(),
            detail,
        };
        outgoing.answer(answer(1, "x".repeat(20_000)));
        outgoing.answer(answer(2, String::new()));
        outgoing.idle_answer(1);
        outgoing.progress(
            BarChange {
                percent: Percent::parse("41").expect("a number"),
                place: (1, 2),
                in_foreground: true,
            },
            EventTiming::default(),
        );
        outgoing.urgent(ListenerToCore::Pong {
            parked_count: 0,
            written_at_us: 0,
        });
        outgoing.idle_answer(2);
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => format!("answer {}", answer.number.0),
                ListenerToCore::Progress { .. } => "progress".to_owned(),
                ListenerToCore::Pong { .. } => "pong".to_owned(),
                ListenerToCore::IdleAnswer { round } => format!("idle {round}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "answer 1", "pong", "answer 2", "idle 1", "progress", "idle 2"
            ]
        );
    }

    /// A bar's change carries its observation and is stamped handled as it
    /// is sent.
    #[test]
    fn a_bars_change_carries_its_observation_and_handling() {
        let (mut outgoing, mut reader) = outgoing();
        outgoing.clock = Clock(|| 7_000);
        outgoing.progress(
            BarChange {
                percent: Percent::parse("41").expect("a number"),
                place: (1, 2),
                in_foreground: true,
            },
            EventTiming {
                observed_at_us: 1_000,
                ..EventTiming::default()
            },
        );
        let sent = sent(&outgoing, &mut reader);
        let [ListenerToCore::Progress { timing, .. }] = sent.as_slice() else {
            panic!("one progress: {sent:?}");
        };
        assert_eq!(
            (timing.observed_at_us, timing.listener_handled_at_us),
            (1_000, 7_000)
        );
    }
}
