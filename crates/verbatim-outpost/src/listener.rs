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
//!   notifications. NVDA's global group also takes live regions, selection
//!   added and removed, range values, windows and tool tips opened, system
//!   alerts, layout invalidation, drag and drop, and the active text
//!   position, which Verbatim does not yet subscribe to (`docs/parity.md`,
//!   "UIA event registration").
//!
//! The listener's callbacks, its writer, and its registration threads never
//! make a cross-process call (decision D13, as amended on 2026-10-11). A UIA
//! callback delivers the element with its properties already cached, so
//! building a fact is local memory reads (plus `GetRuntimeId`, a local read
//! on a cached element); an MSAA `WinEvent` delivers a raw window and object
//! address, forwarded untouched; the only other reads are hang-safe local
//! ones: `GetWindowThreadProcessId`, which names the owning process;
//! `GetProcessIdOfThread` on the event's own thread, which names a console
//! window's console host ([`routed_process`]); and, for a UIA focus on an
//! element with no window of its own, `GetGUIThreadInfo`, which names the
//! keyboard focus window. A foreground
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
//! the marker by the same route. Each read is one remote operation, which
//! UIA's call timeout ends, and a watchdog abandons a thread whose read
//! runs past its deadline, so a slow or hung application holds only its
//! own thread. One exposure is UIA's own and cannot be contained here: a
//! hung window named in a focus `WinEvent` holds UIA's delivery of every
//! focus event to the listener until it answers (section 12.11), as it
//! holds NVDA's.
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
//! element, kind, processing, and activity id), a newer one replacing it
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
//! <handle> [--ignore-pids <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]
//! [--classic-uia]` — the same binary as a per-application outpost, with no
//! target pid, supervised by the same machinery.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, NotificationKind, NotificationProcessing, UIA_MenuOpenedEventId,
    UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::AgileReference;

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEvent, WinEventCallback, WinEventKind};
use verbatim_model::{Notification, OutpostId, Pid, TraceId};
use verbatim_uia::map::{
    cached_native_window_handle, cached_process_id, notification_kind_from_uia,
    notification_processing_from_uia,
};
use verbatim_uia::{Part, Registration, Scope, Subscription};

use crate::OutpostOptions;
use crate::arbitration::window_class_name;
use crate::event_thread::EventThread;
use crate::focus_read::{
    FocusEvent, FocusReader, Judged, LastAccepted, Meta, READ_DEADLINE, Readers, Reading,
};
use crate::outpost::now_ms;
use crate::outpost::text_reads::CONSOLE_WINDOW_CLASS;
use crate::protocol::{
    DeliveredFact, EventTiming, FactKey, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    UiaSnapshotFact, WinEventStamp, now_us, read_message, write_message,
};
use crate::supervisor::IgnoredProcesses;

/// How long after a menu or the Alt+Tab switcher closes the listener waits
/// before telling Core, so a focus event the end causes can arrive first:
/// NVDA's fake-focus delay (`processFakeFocusWinEvent`).
const MENU_END_GRACE: Duration = Duration::from_millis(50);

/// The listener's outgoing queue: urgent messages first, then facts,
/// coalesced one per element and kind.
#[derive(Default)]
struct Outgoing {
    state: Mutex<OutgoingState>,
    ready: Condvar,
    /// The processes whose facts are dropped.
    ignored: IgnoredProcesses,
    /// Numbers the UIA focus events' markers.
    markers: AtomicU64,
}

#[derive(Default)]
struct OutgoingState {
    urgent: VecDeque<OutpostToSupervisor>,
    facts: VecDeque<(Option<(Pid, FactKey)>, OutpostToSupervisor)>,
    /// When a menu or the Alt+Tab switcher last closed, not yet told to
    /// Core: as an instant for the grace, and in milliseconds since the Unix
    /// epoch for Core. NVDA fakes a focus from such an end only when no
    /// focus event was validly processed with it; a focus fact can still be
    /// dropped or fail to read, so Core, which knows which focus it applied,
    /// decides, and every end is told.
    menu_end_at: Option<(Instant, u64)>,
    /// The applications whose outposts have their own hooks in, each with
    /// the tick on the event clock after which none of its `WinEvent`
    /// facts is forwarded.
    hooked: HashMap<Pid, u32>,
    /// Whether the listener is shutting down: what is queued is written,
    /// then the writer ends.
    closed: bool,
}

impl Outgoing {
    fn urgent(&self, message: OutpostToSupervisor) {
        self.lock().urgent.push_back(message);
        self.ready.notify_one();
    }

    /// Queues a fact, replacing any waiting fact for the same element and
    /// kind. `raised_ms_ago` is how long before now Windows raised the event,
    /// and `win_event` which thread raised it and when, for a `WinEvent`'s
    /// fact. A `WinEvent`'s fact raised after the outpost of `pid` has its
    /// own hook in is dropped: that hook took it in.
    fn fact(
        &self,
        pid: Pid,
        fact: DeliveredFact,
        raised_ms_ago: Option<u32>,
        win_event: Option<WinEventStamp>,
    ) {
        let timing = EventTiming {
            raised_ms_ago,
            observed_at_us: now_us(),
            ..EventTiming::default()
        };
        self.queue(pid, fact, (TraceId::mint(), now_ms(), timing), win_event);
    }

    /// Queues `fact`, part of the read of the UIA focus event `meta`
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

    /// A new marker's number.
    fn next_marker(&self) -> u64 {
        self.markers.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Queues `fact` with its trace, the time it was observed, and its
    /// timing, under the rules [`fact`](Self::fact) states.
    fn queue(
        &self,
        pid: Pid,
        fact: DeliveredFact,
        (trace_id, observed_at_ms, timing): (TraceId, u64, EventTiming),
        win_event: Option<WinEventStamp>,
    ) {
        if self.ignored.contains(pid) {
            return;
        }
        let key = fact.key().map(|key| (pid, key));
        let message = OutpostToSupervisor::FocusFact {
            trace_id,
            observed_at_ms,
            timing,
            fact: ListenerFact { pid, fact },
            win_event,
        };
        let mut state = self.lock();
        // Judged under the lock that queues "forwarding ended", so no fact
        // raised after its tick is ever queued behind it.
        if let Some(stamp) = win_event
            && let Some(&through) = state.hooked.get(&pid)
            && stamp.raised_after(through)
        {
            return;
        }
        if let Some(key) = &key {
            state
                .facts
                .retain(|(waiting, _)| waiting.as_ref() != Some(key));
        }
        state.facts.push_back((key, message));
        drop(state);
        self.ready.notify_one();
    }

    /// The outpost `outpost` watching `pid` has its own hook in: none of
    /// `pid`'s `WinEvent` facts raised after `through`, a tick on the event
    /// clock read now or earlier, is forwarded from now on, and "forwarding
    /// ended" is queued behind every fact already queued. Facts raised at
    /// or before it are still forwarded, since the outpost's hook may not
    /// have taken them in, and the outpost matches any it did by counting.
    fn hook_installed(&self, pid: Pid, outpost: OutpostId, through: u32) {
        let mut state = self.lock();
        state.hooked.insert(pid, through);
        state.facts.push_back((
            None,
            OutpostToSupervisor::ForwardingEnded {
                pid,
                outpost,
                through,
            },
        ));
        drop(state);
        self.ready.notify_one();
    }

    /// `pid` has no outpost with its hook in any more: its `WinEvent` facts
    /// are forwarded again.
    fn hook_removed(&self, pid: Pid) {
        self.lock().hooked.remove(&pid);
    }

    /// Notes that a menu or the Alt+Tab switcher closed.
    fn menu_or_switch_ended(&self) {
        self.lock().menu_end_at = Some((Instant::now(), now_ms()));
        self.ready.notify_one();
    }

    fn fault(&self, detail: String) {
        self.urgent(OutpostToSupervisor::Fault { detail });
    }

    /// Ends the writer once what is queued is written, for the listener's
    /// shutdown.
    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_one();
    }

    /// The next message to write, urgent ones first, then facts, then a
    /// menu or switcher end that no focus event followed. Blocks while there
    /// is none; `None` once the queue is closed and empty.
    fn next(&self) -> Option<OutpostToSupervisor> {
        let mut state = self.lock();
        loop {
            if let Some(message) = state.urgent.pop_front() {
                return Some(message);
            }
            if let Some((_, message)) = state.facts.pop_front() {
                return Some(message);
            }
            if state.closed {
                return None;
            }
            let Some((ended, ended_at_ms)) = state.menu_end_at else {
                state = self
                    .ready
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let due = ended + MENU_END_GRACE;
            let now = Instant::now();
            if now >= due {
                state.menu_end_at = None;
                return Some(OutpostToSupervisor::MenuOrSwitchEnded { ended_at_ms });
            }
            state = self
                .ready
                .wait_timeout(state, due - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutgoingState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The focus listener: owns its subscriptions and the writer for the whole
/// life of the process.
struct Listener {
    outgoing: Arc<Outgoing>,
    readers: Arc<Readers<UiaFocusReading>>,
    _registration: Option<Registration>,
    _event_thread: EventThread,
    _writer: JoinHandle<()>,
}

/// The reading threads' work in the listener: each UIA focus event judged
/// and read from its own element ([`FocusReader`]), and what became of it
/// sent to its application's outpost behind its marker.
struct UiaFocusReading {
    /// Whether reads may use remote operations ([`OutpostOptions`]).
    remote: bool,
    outgoing: Arc<Outgoing>,
    /// The last focus any reading thread accepted, which they share.
    last_accepted: LastAccepted,
}

impl Reading for UiaFocusReading {
    /// The event's element, its application's keyboard focus window when
    /// it arrived, for an element that is not a window, its runtime id
    /// from the event's cache, and the kind of event.
    type Job = (
        AgileReference<IUIAutomationElement>,
        isize,
        Vec<i32>,
        FocusEvent,
    );
    type Reader = FocusReader;

    fn reader(&self) -> Option<FocusReader> {
        match FocusReader::new(self.remote, self.last_accepted.clone()) {
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
        (element, focus_window, _, kind): Self::Job,
    ) -> Judged {
        let Some(reader) = reader else {
            return Judged::Dropped("its reading thread has no UIA client");
        };
        match element.resolve() {
            Ok(element) => reader.read(&element, focus_window, kind),
            Err(_) => Judged::Dropped("its element could not be resolved"),
        }
    }

    fn key<'a>(&self, (_, _, runtime_id, kind): &'a Self::Job) -> Option<(FocusEvent, &'a [i32])> {
        Some((*kind, runtime_id))
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
            .focus_read(pid, judged.into_fact(meta.marker), meta);
    }
}

impl Listener {
    /// Sets up the listener: starts the writer, installs every subscription,
    /// reporting a failure as a fault, and only then announces readiness,
    /// so the supervisor and Core know focus and menus are seen from then
    /// on. Installing the UIA focus registration can take hundreds of
    /// milliseconds; announcing readiness first let events in that time go
    /// unseen, such as a menu opened right after Verbatim started.
    ///
    /// The applications in `hooks_in` have outposts with their own hooks
    /// in already: none of their `WinEvent`s is forwarded, and each outpost
    /// is told so first, ending a hand-over the listener this one replaces
    /// may have left unfinished. `options` says whether UIA focus reads
    /// may use remote operations.
    ///
    /// # Panics
    ///
    /// Panics if the writer thread cannot be spawned, which means the
    /// process is out of OS thread resources.
    fn new(
        mut pipe: Box<dyn Write + Send>,
        ignored: IgnoredProcesses,
        hooks_in: &[(Pid, OutpostId)],
        options: OutpostOptions,
    ) -> Self {
        let outgoing = Arc::new(Outgoing {
            ignored,
            ..Outgoing::default()
        });
        // Every event this listener's hook takes in is raised after the
        // hook is installed, so after this tick: none is forwarded for
        // these applications.
        let before_the_hook = verbatim_ia2::event_clock().wrapping_sub(1);
        for &(pid, outpost) in hooks_in {
            outgoing.hook_installed(pid, outpost, before_the_hook);
        }
        let writer_outgoing = Arc::clone(&outgoing);
        let writer = thread::Builder::new()
            .name("verbatim-listener-outbound".to_owned())
            .spawn(move || {
                while let Some(message) = writer_outgoing.next() {
                    if write_message(&mut pipe, &message).is_err() {
                        return;
                    }
                }
                let _ = pipe.flush();
            })
            .expect("spawn the listener writer");

        let readers = Arc::new(Readers::new(
            UiaFocusReading {
                remote: options.remote_operations,
                outgoing: Arc::clone(&outgoing),
                last_accepted: LastAccepted::default(),
            },
            READ_DEADLINE,
        ));
        let registration = install_desktop_subscriptions(
            &outgoing,
            focus_subscription(&outgoing, &readers),
            menu_subscription(&outgoing, &readers),
        );

        let msaa_outgoing = Arc::clone(&outgoing);
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let outgoing = Arc::clone(&msaa_outgoing);
            Box::new(move |event: WinEvent| forward_msaa_event(&outgoing, event))
        });
        let event_thread = EventThread::spawn(0, LISTENER_SUBSCRIPTIONS, make_callback);

        // The listener has no target application; `target_pid` is a sentinel
        // the supervisor only logs.
        outgoing.urgent(OutpostToSupervisor::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(0),
        });

        Self {
            outgoing,
            readers,
            _registration: registration,
            _event_thread: event_thread,
            _writer: writer,
        }
    }

    /// Shuts the listener down cleanly ([`SupervisorToOutpost::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA focus handler and the
    /// other desktop-wide handlers are removed (the registration's thread
    /// waits for a callback in progress, releases its client, and leaves
    /// COM), every reading thread finishes the read in hand, each abandoned
    /// one returning once UIA gives up on its call, the writer writes what
    /// is queued and closes the pipe, and the process's hold on COM's
    /// multithreaded apartment is given back.
    fn shutdown(self) {
        let started = Instant::now();
        let Self {
            outgoing,
            readers,
            _registration: registration,
            _event_thread: event_thread,
            _writer: writer,
        } = self;
        drop(event_thread);
        drop(registration);
        let reading_threads = readers.stop();
        outgoing.close();
        let _ = writer.join();
        verbatim_uia::release_mta_usage();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            reading_threads,
            "the focus listener shut down"
        );
    }

    /// Answers `Ping` with a `Pong` (the listener never abandons a worker, so
    /// its count is always zero), takes the hand-over's news of outposts'
    /// hooks, and ignores everything else. An application with no outpost
    /// any more has its reading thread ended too
    /// ([`Readers::outpost_ended`]).
    fn handle_command(&self, command: &SupervisorToOutpost) {
        match command {
            SupervisorToOutpost::Ping { seq } => {
                self.outgoing.urgent(OutpostToSupervisor::Pong {
                    seq: *seq,
                    parked_count: 0,
                });
            }
            SupervisorToOutpost::HookInstalled { pid, outpost } => {
                self.outgoing
                    .hook_installed(*pid, *outpost, verbatim_ia2::event_clock());
            }
            SupervisorToOutpost::HookRemoved { pid } => {
                self.outgoing.hook_removed(*pid);
                self.readers.outpost_ended(*pid);
            }
            _ => {}
        }
    }
}

/// What a UIA callback captures from a cached element: its owning pid, its
/// cached window handle, and its cached snapshot parts. `None` for an element
/// with no owning process. `element` comes with the registration's base
/// cache request, so every read is a cached local read.
fn capture(element: &IUIAutomationElement) -> Option<(Pid, isize, UiaSnapshotFact)> {
    let pid = cached_process_id(element).filter(|&pid| pid != 0)?;
    let hwnd = cached_native_window_handle(element);
    let parts = crate::focus_read::event_parts(element);
    Some((
        Pid(pid),
        hwnd,
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

/// The window that stands in for a windowless element's own: for a
/// selection, in the outpost's acceptance test against the foreground
/// window; for a focus, in the reading thread's checks before its read and
/// as the window the outpost reports the focus in when the read found
/// none, its acceptance test running against the nearest window the read
/// found. The keyboard focus window of the element's process `pid` now, a
/// local read, when the element's cached window handle `hwnd` is 0 and the
/// process has the keyboard focus; else 0.
fn focus_window_for(pid: Pid, hwnd: isize) -> isize {
    if hwnd == 0 {
        crate::outpost::window::focus_window_of(pid.0).unwrap_or(0)
    } else {
        0
    }
}

/// The desktop-wide UIA focus subscription. Its callback makes no call: it
/// sends the application's outpost a marker for the event at once and
/// queues the event's element for the application's reading thread
/// ([`Readers`]), whose read or drop follows the marker.
fn focus_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaFocusReading>>,
) -> Subscription {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let callback = Arc::new(move |element: &IUIAutomationElement| {
        queue_for_reading(&outgoing, &readers, element, FocusEvent::Focus);
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
) -> Subscription {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    Subscription::Event {
        event: UIA_MenuOpenedEventId,
        callback: Arc::new(move |element: &IUIAutomationElement| {
            if readers.focus_pending() {
                tracing::debug!("UIA menu opening dropped: a focus is pending");
                return;
            }
            queue_for_reading(&outgoing, &readers, element, FocusEvent::MenuOpened);
        }),
    }
}

/// Queues the UIA event `kind` whose sender is `element`, a cached element
/// from the registration's base cache request, for its application's
/// reading thread, after sending its marker. Makes no cross-process call.
fn queue_for_reading(
    outgoing: &Outgoing,
    readers: &Readers<UiaFocusReading>,
    element: &IUIAutomationElement,
    kind: FocusEvent,
) {
    let Some(pid) = cached_process_id(element).filter(|&pid| pid != 0).map(Pid) else {
        return;
    };
    if outgoing.ignored.contains(pid) {
        return;
    }
    let focus_window = focus_window_for(pid, cached_native_window_handle(element));
    let meta = Meta {
        marker: outgoing.next_marker(),
        trace: TraceId::mint(),
        observed_at_ms: now_ms(),
        timing: EventTiming {
            observed_at_us: now_us(),
            ..EventTiming::default()
        },
    };
    // The marker goes first, so the read or the drop can only follow it.
    outgoing.focus_read(
        pid,
        DeliveredFact::UiaFocusMarker {
            marker: meta.marker,
        },
        meta,
    );
    match AgileReference::new(element) {
        Ok(agile) => readers.push(
            pid,
            meta,
            (agile, focus_window, verbatim_uia::runtime_id(element), kind),
        ),
        Err(error) => {
            tracing::warn!(%error, "a UIA focus's element could not be kept for its read");
            outgoing.focus_read(
                pid,
                DeliveredFact::UiaFocusDropped {
                    marker: meta.marker,
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
/// they must share it), with NVDA's base cache request
/// (`verbatim_uia::nvda_base_cache_request`). The facts carry the
/// element's process, window, runtime id, role, and name from that cache;
/// anything else in their snapshots is not read here and is left empty. A
/// failure is reported as a fault and leaves the MSAA path running on its
/// own.
fn install_desktop_subscriptions(
    outgoing: &Arc<Outgoing>,
    focus: Subscription,
    menu: Subscription,
) -> Option<Registration> {
    let selection_outgoing = Arc::clone(outgoing);
    let selection = Subscription::Event {
        event: UIA_SelectionItem_ElementSelectedEventId,
        callback: Arc::new(move |element: &IUIAutomationElement| {
            // A cached element from the registration's cache request.
            if let Some((pid, hwnd, snapshot)) = capture(element) {
                let focus_window = focus_window_for(pid, hwnd);
                selection_outgoing.fact(
                    pid,
                    DeliveredFact::UiaSelection {
                        hwnd,
                        focus_window,
                        snapshot,
                    },
                    None,
                    None,
                );
            }
        }),
    };

    let notification_outgoing = Arc::clone(outgoing);
    let notifications = Subscription::Notifications {
        callback: Arc::new(
            move |element: &IUIAutomationElement,
                  kind: NotificationKind,
                  processing: NotificationProcessing,
                  display_string: Option<String>,
                  activity_id: Option<String>| {
                if verbatim_uia::map::cached_ignores_notifications(element) {
                    return;
                }
                if let Some((pid, hwnd, snapshot)) = capture(element) {
                    let notification = Notification {
                        kind: notification_kind_from_uia(kind),
                        processing: notification_processing_from_uia(processing),
                        display_string,
                        activity_id,
                    };
                    notification_outgoing.fact(
                        pid,
                        DeliveredFact::UiaNotification {
                            hwnd,
                            snapshot,
                            notification,
                        },
                        None,
                        None,
                    );
                }
            },
        ),
    };

    // NVDA's base cache request, as its global group carries: UIA fills
    // only these in the application for each event, and the outpost reads
    // what more it needs of a selected item when it handles the event.
    let parts = [focus, selection, menu, notifications]
        .into_iter()
        .map(|subscription| {
            Part::new(subscription, verbatim_uia::NVDA_BASE_PROPERTIES).with_text_pattern()
        })
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
/// acquisition.
fn forward_msaa_event(outgoing: &Outgoing, event: WinEvent) {
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
        outgoing.menu_or_switch_ended();
        return;
    }
    // NVDA takes a null window as the desktop window
    // (`internalWinEventHandler.py` 113 to 129), whose process is the
    // client server, whose objects NVDA's focused-state test refuses
    // (`shouldAllowIAccessibleFocusEvent` on the desktop window's object)
    // and Verbatim cannot open: dropping it here is the same outcome.
    if hwnd == 0 {
        return;
    }
    let owner = window_pid(hwnd);
    if owner == 0 {
        return;
    }
    let pid = routed_process(&window_class_name(hwnd), owner, || {
        process_of_thread(thread)
    });
    if pid != owner {
        if outgoing.ignored.contains(Pid(owner)) {
            // The console of a process Verbatim ignores, whose events were
            // dropped by its owner before they went to its console host.
            return;
        }
        tracing::debug!(
            hwnd,
            ?kind,
            console_host = pid,
            client = owner,
            "a console window's event goes to its console host"
        );
    }
    // The listener subscribes to nothing else (LISTENER_SUBSCRIPTIONS).
    let Some(fact) = DeliveredFact::from_win_event(kind, (hwnd, id_object, id_child)) else {
        return;
    };
    outgoing.fact(
        Pid(pid),
        fact,
        Some(raised_ms_ago),
        Some(WinEventStamp { thread, time }),
    );
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
/// (`docs/design/focus-pipeline.md`, section 5.4). When the host cannot be
/// read, the owner, as NVDA falls back to the window's own thread for a
/// console window it has seen no event from.
fn routed_process(class: &str, owner: u32, host: impl FnOnce() -> Option<u32>) -> u32 {
    if class == CONSOLE_WINDOW_CLASS {
        host().unwrap_or(owner)
    } else {
        owner
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
    let handle = match unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) } {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!(thread, %error, "a console window's thread could not be opened");
            return None;
        }
    };
    // SAFETY: `handle` is an open thread handle with the access the call
    // needs.
    let pid = unsafe { GetProcessIdOfThread(handle) };
    // SAFETY: `handle` was opened above and is closed once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    (pid != 0).then_some(pid)
}

/// Runs the focus listener driven by the Core pipes: reads commands from
/// `pipe_in` and writes facts and replies to `pipe_out` until Core asks it
/// to shut down ([`SupervisorToOutpost::Shutdown`]) or the command stream
/// ends, then shuts it down cleanly and returns. Facts from the processes
/// `ignored` holds are dropped, and so are the `WinEvent` facts of the
/// applications in `hooks_in`, whose outposts have their own hooks in.
/// `options` says whether UIA focus reads may use remote operations.
///
/// # Errors
///
/// Returns any I/O error reading the command stream, after the shutdown.
pub fn run_listener(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
    ignored: IgnoredProcesses,
    hooks_in: &[(Pid, OutpostId)],
    options: OutpostOptions,
) -> io::Result<()> {
    let listener = Listener::new(pipe_out, ignored, hooks_in, options);
    let mut reader = BufReader::new(pipe_in);
    let ended = loop {
        match read_message::<_, SupervisorToOutpost>(&mut reader) {
            Ok(Some(SupervisorToOutpost::Shutdown)) => {
                tracing::info!("shutting down, as Core asked");
                break Ok(());
            }
            Ok(Some(command)) => listener.handle_command(&command),
            Ok(None) => {
                tracing::warn!("Core's command pipe closed; shutting down");
                break Ok(());
            }
            Err(error) => break Err(error),
        }
    };
    listener.shutdown();
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
    use std::sync::mpsc;

    use super::*;

    /// Everything queued in `outgoing`, which is closed first.
    fn sent(outgoing: &Outgoing) -> Vec<OutpostToSupervisor> {
        outgoing.close();
        std::iter::from_fn(|| outgoing.next()).collect()
    }

    /// The stamp of an MSAA event raised at `time`.
    fn raised_at(time: u32) -> WinEventStamp {
        WinEventStamp { thread: 9, time }
    }

    #[test]
    fn a_win_event_raised_after_the_outposts_hook_is_in_is_not_forwarded() {
        let outgoing = Outgoing::default();
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(90)));
        outgoing.hook_installed(Pid(5), OutpostId(3), 100);
        // Raised after the tick: the outpost's hook took it in.
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(101)));
        // Raised at the tick, and reaching the listener late: forwarded,
        // for the outpost to match by counting.
        outgoing.fact(
            Pid(5),
            DeliveredFact::Foreground { hwnd: 1 },
            None,
            Some(raised_at(100)),
        );
        // Another application's, and a UIA fact: forwarded.
        outgoing.fact(Pid(6), msaa_focus(), None, Some(raised_at(101)));
        outgoing.fact(
            Pid(5),
            DeliveredFact::UiaSelection {
                hwnd: 1,
                focus_window: 0,
                snapshot: uia_snapshot(),
            },
            None,
            None,
        );
        let kinds: Vec<String> = sent(&outgoing)
            .into_iter()
            .map(|message| match message {
                OutpostToSupervisor::FocusFact { fact, .. } => {
                    format!("{} {:?}", fact.pid.0, fact.fact.key())
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
                "5 Some(MsaaFocus(1, -4, 0))",
                "ended 5 3 100",
                "5 Some(Foreground(1))",
                "6 Some(MsaaFocus(1, -4, 0))",
                "5 Some(UiaSelection([42]))",
            ],
            "the fact queued first goes ahead of forwarding ended"
        );
    }

    #[test]
    fn a_focus_reads_marker_read_and_drop_are_never_replaced() {
        let outgoing = Outgoing::default();
        let meta = |marker| Meta {
            marker,
            trace: TraceId::mint(),
            observed_at_ms: 0,
            timing: EventTiming::default(),
        };
        // Two focus events on one element, the first read, the second
        // dropped: an outpost waits on each marker, so all four are sent,
        // in order.
        outgoing.focus_read(Pid(5), DeliveredFact::UiaFocusMarker { marker: 1 }, meta(1));
        outgoing.focus_read(Pid(5), DeliveredFact::UiaFocusMarker { marker: 2 }, meta(2));
        outgoing.focus_read(Pid(5), Judged::Dropped("read").into_fact(1), meta(1));
        outgoing.focus_read(
            Pid(5),
            DeliveredFact::UiaFocusDropped { marker: 2 },
            meta(2),
        );
        let facts: Vec<DeliveredFact> = sent(&outgoing)
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
                DeliveredFact::UiaFocusDropped { marker: 1 },
                DeliveredFact::UiaFocusDropped { marker: 2 },
            ]
        );
    }

    #[test]
    fn a_win_event_is_forwarded_again_once_the_hook_is_removed() {
        let outgoing = Outgoing::default();
        outgoing.hook_installed(Pid(5), OutpostId(3), 100);
        outgoing.hook_removed(Pid(5));
        outgoing.fact(Pid(5), msaa_focus(), None, Some(raised_at(101)));
        let sent = sent(&outgoing);
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

    /// Takes messages from `outgoing` on another thread, since `next`
    /// blocks while there is nothing to send.
    fn drain(outgoing: &Arc<Outgoing>) -> mpsc::Receiver<OutpostToSupervisor> {
        let (tx, rx) = mpsc::channel();
        let outgoing = Arc::clone(outgoing);
        thread::spawn(move || {
            while let Some(message) = outgoing.next() {
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
        assert_eq!(routed_process(CONSOLE_WINDOW_CLASS, 20, || Some(30)), 30);
    }

    #[test]
    fn a_console_windows_event_whose_thread_cannot_be_read_goes_to_its_owner() {
        assert_eq!(routed_process(CONSOLE_WINDOW_CLASS, 20, || None), 20);
    }

    #[test]
    fn any_other_windows_event_goes_to_its_owner_without_reading_its_thread() {
        assert_eq!(
            routed_process("Notepad", 20, || panic!("the thread is not read")),
            20
        );
    }

    #[test]
    fn a_threads_process_is_read_from_the_thread() {
        // SAFETY: GetCurrentThreadId has no preconditions.
        let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        assert_eq!(process_of_thread(thread), Some(std::process::id()));
    }

    #[test]
    fn a_fact_from_an_ignored_process_is_never_queued() {
        // This process stands for the owner's Windows Terminal.
        let own = Pid(std::process::id());
        let outgoing = Outgoing {
            ignored: IgnoredProcesses::hold([own]),
            ..Outgoing::default()
        };
        outgoing.fact(own, DeliveredFact::Foreground { hwnd: 1 }, None, None);
        outgoing.fact(own, msaa_focus(), None, None);
        outgoing.fact(Pid(5), msaa_focus(), None, None);
        outgoing.close();
        let mut sent = Vec::new();
        while let Some(message) = outgoing.next() {
            sent.push(message);
        }
        let [OutpostToSupervisor::FocusFact { fact, .. }] = sent.as_slice() else {
            panic!("one fact is sent, the other process's: {sent:?}");
        };
        assert_eq!(fact.pid, Pid(5));
    }

    #[test]
    fn a_menu_end_no_focus_follows_reaches_core_after_the_grace() {
        let outgoing = Arc::new(Outgoing::default());
        let messages = drain(&outgoing);
        let ended = Instant::now();
        outgoing.menu_or_switch_ended();
        let message = messages
            .recv_timeout(Duration::from_secs(5))
            .expect("the menu end is sent");
        assert!(matches!(
            message,
            OutpostToSupervisor::MenuOrSwitchEnded { .. }
        ));
        assert!(ended.elapsed() >= MENU_END_GRACE, "not before the grace");
    }

    #[test]
    fn a_menu_end_a_focus_follows_is_sent_after_the_focus() {
        // Whether the focus was usable is Core's to judge: it may yet be
        // dropped or fail to read.
        let outgoing = Arc::new(Outgoing::default());
        outgoing.menu_or_switch_ended();
        outgoing.fact(Pid(5), msaa_focus(), None, None);
        let messages = drain(&outgoing);
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
}
