//! The focus-listener runtime (architecture section 1, decisions D13 and D14;
//! outpost redesign, "The focus listener").
//!
//! One permanent, stateless listener process holds the subscriptions that are
//! global by nature and forwards each captured fact to Core, which routes it
//! to the target application's own outpost:
//!
//! - MSAA focus, foreground, menu-popup, and alert events for every process,
//!   and the end of a menu or of the Alt+Tab switcher, which go to Core
//!   rather than to an application's outpost;
//! - the desktop-wide UIA focus subscription;
//! - desktop-wide UIA subscriptions for the events NVDA registers globally on
//!   Windows 11: an element selected, a menu opened, and notifications.
//!
//! The listener's one hard rule is that it never makes a cross-process call.
//! A UIA callback delivers the element with its properties already cached, so
//! building a fact is local memory reads (plus `GetRuntimeId`, a local read
//! on a cached element); an MSAA `WinEvent` delivers a raw window and object
//! address, forwarded untouched; the only other reads are hang-safe local
//! ones: `GetWindowThreadProcessId`, which names the owning process, and,
//! for a UIA focus on an element with no window of its own,
//! `GetGUIThreadInfo`, which names the keyboard focus window. A foreground
//! event is forwarded unchecked; the outpost's worker checks that its
//! window is the foreground when it handles it, as NVDA's
//! `processForegroundWinEvent` does. No cross-process calls means no
//! deadlines and no way for any application to stall focus detection for
//! the rest of the desktop.
//!
//! Outgoing facts are coalesced with NVDA's UIA limiter rule before they are
//! sent: one waiting fact per element and kind, a newer one replacing it and
//! moving to the back, so a flood from one busy process cannot pass through
//! the listener and Core unthrottled. Pongs and `Ready` go ahead of facts.
//!
//! The processes Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names) are held open here too, and every fact from one
//! of them is dropped before it is queued, so it is never routed and never
//! starts an outpost. Their events still reach the listener: UIA and
//! `WinEvent` delivery cannot be scoped to leave a process out.
//!
//! Run as `verbatim-outpost.exe --listener --pipe-in <handle> --pipe-out
//! <handle> [--ignore-pids <pid,pid>]` — the same binary as a
//! per-application outpost, with no target pid, supervised by the same
//! machinery.

use std::collections::VecDeque;
use std::io::{self, BufReader, Write};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, NotificationKind, NotificationProcessing, UIA_MenuOpenedEventId,
    UIA_SelectionItem_ElementSelectedEventId,
};

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEventCallback, WinEventKind};
use verbatim_model::{Notification, Pid, TraceId};
use verbatim_uia::map::{
    cached_native_window_handle, cached_process_id, notification_kind_from_uia,
    notification_processing_from_uia, snapshot_parts_from_cached_element,
};
use verbatim_uia::{FocusRegistration, Registration, Scope, Subscription};

use crate::event_thread::EventThread;
use crate::outpost::now_ms;
use crate::protocol::{
    DeliveredFact, EventTiming, FactKey, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    UiaSnapshotFact, now_us, read_message, write_message,
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
    /// when it says.
    fn fact(&self, pid: Pid, fact: DeliveredFact, raised_ms_ago: Option<u32>) {
        if self.ignored.contains(pid) {
            return;
        }
        let key = fact.key().map(|key| (pid, key));
        let message = OutpostToSupervisor::FocusFact {
            trace_id: TraceId::mint(),
            observed_at_ms: now_ms(),
            timing: EventTiming {
                raised_ms_ago,
                observed_at_us: now_us(),
                ..EventTiming::default()
            },
            fact: ListenerFact { pid, fact },
        };
        let mut state = self.lock();
        if let Some(key) = &key {
            state
                .facts
                .retain(|(waiting, _)| waiting.as_ref() != Some(key));
        }
        state.facts.push_back((key, message));
        drop(state);
        self.ready.notify_one();
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
    _focus_registration: Option<FocusRegistration>,
    _registration: Option<Registration>,
    _event_thread: EventThread,
    _writer: JoinHandle<()>,
}

impl Listener {
    /// Sets up the listener: starts the writer, installs every subscription,
    /// reporting a failure as a fault, and only then announces readiness,
    /// so the supervisor and Core know focus and menus are seen from then
    /// on. Installing the UIA focus registration can take hundreds of
    /// milliseconds; announcing readiness first let events in that time go
    /// unseen, such as a menu opened right after Verbatim started.
    ///
    /// # Panics
    ///
    /// Panics if the writer thread cannot be spawned, which means the
    /// process is out of OS thread resources.
    fn new(mut pipe: Box<dyn Write + Send>, ignored: IgnoredProcesses) -> Self {
        let outgoing = Arc::new(Outgoing {
            ignored,
            ..Outgoing::default()
        });
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

        let focus_registration = install_focus_registration(&outgoing);
        let registration = install_desktop_subscriptions(&outgoing);

        let msaa_outgoing = Arc::clone(&outgoing);
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let outgoing = Arc::clone(&msaa_outgoing);
            Box::new(move |kind, hwnd, id_object, id_child, raised_ms_ago| {
                forward_msaa_event(&outgoing, kind, hwnd, id_object, id_child, raised_ms_ago);
            })
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
            _focus_registration: focus_registration,
            _registration: registration,
            _event_thread: event_thread,
            _writer: writer,
        }
    }

    /// Shuts the listener down cleanly ([`SupervisorToOutpost::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA focus handler and the
    /// other desktop-wide handlers are removed (each subscription's thread
    /// waits for a callback in progress, releases its client, and leaves
    /// COM), the writer writes what is queued and closes the pipe, and the
    /// process's hold on COM's multithreaded apartment is given back. The
    /// listener makes no call into an application, so nothing else can be
    /// in progress.
    fn shutdown(self) {
        let started = Instant::now();
        let Self {
            outgoing,
            _focus_registration: focus_registration,
            _registration: registration,
            _event_thread: event_thread,
            _writer: writer,
        } = self;
        drop(event_thread);
        drop(focus_registration);
        drop(registration);
        outgoing.close();
        let _ = writer.join();
        verbatim_uia::release_mta_usage();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "the focus listener shut down"
        );
    }

    /// Answers `Ping` with a `Pong` (the listener never abandons a worker, so
    /// its count is always zero) and ignores everything else.
    fn handle_command(&self, command: &SupervisorToOutpost) {
        if let SupervisorToOutpost::Ping { seq } = command {
            self.outgoing.urgent(OutpostToSupervisor::Pong {
                seq: *seq,
                parked_count: 0,
            });
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
    let parts = snapshot_parts_from_cached_element(element);
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

/// The fact the listener forwards for a UIA focus event on `element`, which
/// carries the base cache request's properties (`Uia::base_cache_request`),
/// as an event's element does: `None` for an element with no owning
/// process. Local reads only: a windowless element is in the keyboard focus
/// window of its own process, if that process still has the focus.
#[must_use]
pub fn uia_focus_fact(element: &IUIAutomationElement) -> Option<ListenerFact> {
    let (pid, hwnd, snapshot) = capture(element)?;
    let focus_window = focus_window_for(pid, hwnd);
    Some(ListenerFact {
        pid,
        fact: DeliveredFact::UiaFocus {
            hwnd,
            focus_window,
            snapshot,
        },
    })
}

/// The window that stands in for a windowless element's own, for the
/// outpost's acceptance test against the foreground window: the keyboard
/// focus window of the element's process `pid` now, a local read, when the
/// element's cached window handle `hwnd` is 0 and the process has the
/// keyboard focus; else 0.
fn focus_window_for(pid: Pid, hwnd: isize) -> isize {
    if hwnd == 0 {
        crate::outpost::window::focus_window_of(pid.0).unwrap_or(0)
    } else {
        0
    }
}

/// Installs the desktop-global UIA focus registration. A failure is reported
/// as a fault and leaves the MSAA path running on its own.
fn install_focus_registration(outgoing: &Arc<Outgoing>) -> Option<FocusRegistration> {
    let callback_outgoing = Arc::clone(outgoing);
    let callback = Arc::new(move |element: &IUIAutomationElement| {
        // A cached focus element from the registration's base cache request.
        if let Some(ListenerFact { pid, fact }) = uia_focus_fact(element) {
            callback_outgoing.fact(pid, fact, None);
        }
    });
    match FocusRegistration::new(callback) {
        Ok(registration) => Some(registration),
        Err(error) => {
            outgoing.fault(format!("listener UIA focus registration failed: {error}"));
            None
        }
    }
}

/// Installs the desktop-wide UIA subscriptions NVDA registers globally on
/// Windows 11, an element selected, a menu opened, and notifications, as
/// one event handler group, as NVDA's global handlers also are.
fn install_desktop_subscriptions(outgoing: &Arc<Outgoing>) -> Option<Registration> {
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
                );
            }
        }),
    };

    let menu_outgoing = Arc::clone(outgoing);
    let menu = Subscription::Event {
        event: UIA_MenuOpenedEventId,
        callback: Arc::new(move |element: &IUIAutomationElement| {
            if let Some((pid, hwnd, snapshot)) = capture(element) {
                menu_outgoing.fact(pid, DeliveredFact::UiaMenuOpened { hwnd, snapshot }, None);
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
                    );
                }
            },
        ),
    };

    match Registration::new(vec![selection, menu, notifications], Scope::Desktop) {
        Ok(registration) => Some(registration),
        Err(error) => {
            outgoing.fault(format!(
                "listener desktop-wide UIA subscriptions failed: {error}"
            ));
            None
        }
    }
}

/// Forwards one global MSAA `WinEvent` as a fact. Reads the owning pid with
/// `GetWindowThreadProcessId` (a hang-safe local call) and drops a null
/// window or a pid-zero owner; the raw object address is forwarded untouched,
/// and the app outpost does the acquisition.
fn forward_msaa_event(
    outgoing: &Outgoing,
    kind: WinEventKind,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    raised_ms_ago: u32,
) {
    if matches!(kind, WinEventKind::MenuEnd | WinEventKind::SwitchEnd) {
        // Not routed to an outpost: if no focus event follows, Core reads
        // the focus of whichever application is then in front. NVDA accepts
        // these from any window, even an invalid one.
        outgoing.menu_or_switch_ended();
        return;
    }
    if hwnd == 0 {
        return;
    }
    let pid = window_pid(hwnd);
    if pid == 0 {
        return;
    }
    let fact = match kind {
        WinEventKind::Focus => DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        },
        // Forwarded unchecked: whether the window is still the foreground
        // window is checked later, by the outpost's worker when it handles
        // the fact, as NVDA checks it when it processes the event rather
        // than in its event callback.
        WinEventKind::Foreground => DeliveredFact::Foreground { hwnd },
        WinEventKind::MenuPopupStart => DeliveredFact::MenuPopup {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Alert => DeliveredFact::Alert {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Show => DeliveredFact::Show {
            hwnd,
            id_object,
            id_child,
        },
        // The listener subscribes to nothing else (LISTENER_SUBSCRIPTIONS).
        _ => return,
    };
    outgoing.fact(Pid(pid), fact, Some(raised_ms_ago));
}

/// The owning process id of `hwnd`, or 0 for an invalid or ownerless window.
fn window_pid(hwnd: isize) -> u32 {
    crate::outpost::window::window_owner(hwnd).1
}

/// Runs the focus listener driven by the Core pipes: reads commands from
/// `pipe_in` and writes facts and replies to `pipe_out` until Core asks it
/// to shut down ([`SupervisorToOutpost::Shutdown`]) or the command stream
/// ends, then shuts it down cleanly and returns. Facts from the processes
/// `ignored` holds are dropped.
///
/// # Errors
///
/// Returns any I/O error reading the command stream, after the shutdown.
pub fn run_listener(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
    ignored: IgnoredProcesses,
) -> io::Result<()> {
    let listener = Listener::new(pipe_out, ignored);
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

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
    fn a_fact_from_an_ignored_process_is_never_queued() {
        // This process stands for the owner's Windows Terminal.
        let own = Pid(std::process::id());
        let outgoing = Outgoing {
            ignored: IgnoredProcesses::hold([own]),
            ..Outgoing::default()
        };
        outgoing.fact(own, DeliveredFact::Foreground { hwnd: 1 }, None);
        outgoing.fact(own, msaa_focus(), None);
        outgoing.fact(Pid(5), msaa_focus(), None);
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
        outgoing.fact(Pid(5), msaa_focus(), None);
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
