//! The per-application outpost (outpost redesign, "Inside an outpost";
//! decision D9). One outpost watches one application for its whole life.
//! Its parts:
//!
//! 1. Intake ([`intake`]): the MSAA hook callbacks, the UIA subscription
//!    callbacks, and the reader's routed facts and queries only add an entry
//!    to the queue and return. They never call into the application and never
//!    wait on the worker.
//! 2. The queue: one per application, with NVDA's limiter rules.
//! 3. The worker ([`worker`]): one thread that takes entries in order, the
//!    only thread that calls into the application.
//! 4. The watchdog: abandons and replaces a worker whose call hangs.
//! 5. The reader ([`Outpost::handle_command`], driven by [`run_pipe`]):
//!    answers pings itself, so a busy or hung worker never makes the outpost
//!    look dead.
//! 6. The writer ([`outbound`]): pongs and `Ready` go first; queuing never
//!    waits, and waiting messages are merged by object and kind.
//!
//! Workers run in COM's multithreaded apartment; the registries keep agile
//! references, so a replacement worker can use what an abandoned one minted.
//!
//! Held objects (outpost redesign, "Held objects"): the registries keep the
//! live UIA element or MSAA object behind every node the outpost reports.
//! Each message that carries node ids has a position, counted the same way
//! by Core; when Core reports the nodes it still holds and the position of
//! the last message it has handled, the worker releases every other node
//! reported at or before that position. A node reported later, or issued and
//! not yet reported, is kept: Core may not have seen it yet.

mod intake;
mod outbound;
mod read;
pub(crate) mod text_reads;
pub(crate) mod window;
mod worker;

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationTextRange,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};
use windows::core::AgileReference;

use verbatim_ia2::{APP_SUBSCRIPTIONS, NodeIdRegistry as MsaaRegistry, WinEventCallback};
use verbatim_model::{Backend, Fetches, NodeId, Pid, TraceId};
use verbatim_uia::map::{cached_native_window_handle, snapshot_parts_from_cached_element};
use verbatim_uia::{
    FOCUS_PROPERTIES, NodeIdRegistry as UiaRegistry, Registration, Scope, Subscription, Uia,
};

use crate::arbitration::Arbitrator;
use crate::event_thread::EventThread;
use crate::protocol::{
    EventTiming, OutpostToSupervisor, Query, QueryOutcome, SupervisorToOutpost, UiaSnapshotFact,
    now_us, read_message,
};
use crate::text::uia::UiaPos;
use crate::text::{Anchors, HeldAnchors};

use intake::{Entry, Intake, Item, UiaEvent, UiaKind};
use outbound::Outbound;
use worker::{Tracking, Watch};

pub(crate) use window::now_ms;

/// The node whose caret the worker last read, and when that read began, in
/// microseconds: a caret event observed before then changes nothing the
/// read did not see, and one observed later (while the read was in flight
/// included) may.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CaretRead(Option<(u64, u64)>);

impl CaretRead {
    /// Notes a read of node `node`'s caret that began at `started_us`.
    fn note(&mut self, node: u64, started_us: u64) {
        self.0 = Some((node, started_us));
    }

    /// Whether the last read of node `node`'s caret began at or after
    /// `observed_us`, so it saw what an event observed then reports.
    fn covers(self, node: u64, observed_us: u64) -> bool {
        self.0
            .is_some_and(|(read, at)| read == node && at >= observed_us)
    }
}

#[cfg(test)]
mod caret_read_tests {
    use super::CaretRead;

    #[test]
    fn an_event_observed_while_a_read_was_in_flight_is_not_covered() {
        let mut read = CaretRead::default();
        // A read of node 1 begins at 100 and returns at 200; a caret event
        // is observed at 150, while it was in flight.
        read.note(1, 100);
        assert!(!read.covers(1, 150));
        // One observed before the read began is covered; another node's
        // never is.
        assert!(read.covers(1, 100));
        assert!(read.covers(1, 90));
        assert!(!read.covers(2, 90));
    }
}

/// What the outpost's threads share.
pub(crate) struct Context {
    target_pid: u32,
    outbound: Outbound,
    intake: Intake,
    watch: Watch,
    uia_registry: UiaRegistry,
    msaa_registry: MsaaRegistry,
    arbitrator: Mutex<Arbitrator>,
    tracking: Mutex<Tracking>,
    /// The focus-following UIA property subscription, which the worker moves.
    focus_properties: OnceLock<Registration>,
    /// Whether UIA reads may use remote operations ([`OutpostOptions`]).
    remote_operations: bool,
    /// Windows whose UIA elements could not be imported into a remote
    /// operation (client-side proxies), read the classic way for the
    /// window's lifetime, since a window's provider does not change. Each
    /// is kept with the thread that owned it, so a reused handle, whose
    /// destroy event was lost, is not read the classic way by mistake.
    classic_windows: Mutex<HashMap<isize, u32>>,
    /// The focus-following UIA subscription to a text focus's caret and
    /// text changes, which the worker moves (milestone M4).
    text_events: OnceLock<Registration>,
    /// The caret key's watch for evidence the worker keeps open between its
    /// entries, if one is open.
    caret_watch: Mutex<Option<text_reads::OpenWatch>>,
    /// The text anchors Core holds, set by the reader as Core's list
    /// arrives, under a lock of their own that the worker never holds
    /// across a call into the application, as it holds the anchor stores'.
    held_anchors: HeldAnchors,
    /// The text anchors minted in UIA text and in edit controls; both number
    /// theirs from one counter and keep the anchors Core holds.
    uia_anchors: Mutex<Anchors<UiaPos>>,
    edit_anchors: Mutex<Anchors<u32>>,
    /// Each UIA node's text patterns, once fetched.
    patterns: Mutex<HashMap<u64, text_reads::Patterns>>,
    /// What each UIA node's text is known to support of the text
    /// attributes, learned from its answers and kept, like its patterns,
    /// until the node is released.
    text_support: Mutex<HashMap<u64, crate::text::uia::TextSupport>>,
    /// The node whose caret the worker last read, and when that read began,
    /// in microseconds: a caret event observed before it changes nothing
    /// the read did not see.
    caret_read: Mutex<CaretRead>,
    /// The details the active theme wants read for each node
    /// ([`SupervisorToOutpost::Fetches`]); everything until Core says
    /// otherwise. UIA reads leave the properties of the others out of
    /// their cache requests; MSAA reads skip their calls, through the MSAA
    /// registry, which holds the same.
    fetches: Mutex<Fetches>,
    /// Each focused terminal's anchor and memory, by node (milestone M4
    /// item 9).
    terminals: Mutex<HashMap<u64, crate::terminal::Terminal>>,
    /// How the worker reads the element that has the keyboard focus.
    focused_element: FocusedElementReader,
    /// How the worker reads the foreground window when it records the
    /// window a focus was reported in ([`Outpost::set_foreground_reader`]).
    foreground: Mutex<ForegroundReader>,
}

/// Reads the foreground window's handle. An outpost reads the system's
/// (`GetForegroundWindow`); a test whose application cannot take the
/// foreground on the desktop it runs on supplies its own
/// ([`Outpost::set_foreground_reader`]).
pub type ForegroundReader = Arc<dyn Fn() -> isize + Send + Sync>;

/// Reads the UIA element that has the keyboard focus, built with the given
/// cache request, with the given client, on the worker's thread. An
/// outpost reads the system's keyboard focus ([`Uia::focused_element`]);
/// a test that drives a real outpost against an application without taking
/// the keyboard focus from the desktop it runs on supplies its own
/// ([`Outpost::with_focused_element_reader`]).
pub type FocusedElementReader = Arc<
    dyn Fn(&Uia, &IUIAutomationCacheRequest) -> windows::core::Result<IUIAutomationElement>
        + Send
        + Sync,
>;

/// How an outpost reads its application, fixed for its whole life. The
/// supervisor passes it on each outpost's command line, from
/// `settings.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutpostOptions {
    /// Whether a UIA focus's ancestors are read with a remote operation
    /// where the window's provider supports one (`uia.remote_operations`
    /// in `settings.toml`, on by default); off forces the classic walk.
    pub remote_operations: bool,
}

impl Default for OutpostOptions {
    fn default() -> Self {
        Self {
            remote_operations: true,
        }
    }
}

impl Context {
    fn arbitrator(&self) -> MutexGuard<'_, Arbitrator> {
        self.arbitrator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The foreground window's handle, by the reader the outpost was given
    /// ([`Outpost::set_foreground_reader`]).
    fn foreground_window(&self) -> isize {
        let reader = Arc::clone(
            &self
                .foreground
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        reader()
    }

    fn tracking(&self) -> MutexGuard<'_, Tracking> {
        self.tracking.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn classic_windows(&self) -> MutexGuard<'_, HashMap<isize, u32>> {
        self.classic_windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn uia_anchors(&self) -> MutexGuard<'_, Anchors<UiaPos>> {
        self.uia_anchors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn edit_anchors(&self) -> MutexGuard<'_, Anchors<u32>> {
        self.edit_anchors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn patterns(&self) -> MutexGuard<'_, HashMap<u64, text_reads::Patterns>> {
        self.patterns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn text_support(&self) -> MutexGuard<'_, HashMap<u64, crate::text::uia::TextSupport>> {
        self.text_support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that the worker is reading `node`'s caret now.
    /// The details the active theme wants read for each node.
    fn fetches(&self) -> Fetches {
        *self.fetches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn terminals(&self) -> MutexGuard<'_, HashMap<u64, crate::terminal::Terminal>> {
        self.terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// How many of a change's newest lines a terminal read takes
    /// ([`SupervisorToOutpost::TerminalLines`]), which also bounds the
    /// output combined while it waits to be sent.
    fn terminal_lines(&self) -> u16 {
        self.outbound.terminal_lines()
    }

    /// The UIA cache request for the details the active theme wants.
    fn uia_cache(&self, uia: &Uia) -> windows::core::Result<IUIAutomationCacheRequest> {
        uia.cache_request_for(self.fetches())
    }

    /// The caret key's watch the worker keeps open, if any.
    fn caret_watch(&self) -> MutexGuard<'_, Option<text_reads::OpenWatch>> {
        self.caret_watch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn caret_read(&self, node: NodeId) {
        self.caret_read_from(node, now_us());
    }

    /// Notes that a read of `node`'s caret began at `started_us`, for a
    /// read stamped once it has returned: the time it began, never the
    /// time it ended, since a caret event observed while it was in flight
    /// may come from a move the read did not see.
    fn caret_read_from(&self, node: NodeId, started_us: u64) {
        self.caret_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note(node.number(), started_us);
    }

    /// Whether the worker has read `node`'s caret since `observed_us`, so
    /// a caret event observed then needs no report of its own.
    fn caret_read_since(&self, node: NodeId, observed_us: u64) -> bool {
        self.caret_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .covers(node.number(), observed_us)
    }

    /// Whether a UIA read for an element of `hwnd` should try a remote
    /// operation: the setting allows it and no import for the window has
    /// failed.
    fn tries_remote(&self, hwnd: Option<isize>) -> bool {
        self.remote_operations
            && hwnd.is_none_or(|hwnd| {
                self.classic_windows().get(&hwnd) != Some(&window::window_thread(hwnd))
            })
    }

    /// Reads `hwnd`'s UIA elements the classic way from now on.
    fn read_classically(&self, hwnd: isize) {
        self.classic_windows()
            .insert(hwnd, window::window_thread(hwnd));
    }

    /// Forgets what was learned about a destroyed window, whose handle may
    /// be reused.
    fn forget_window(&self, hwnd: isize) {
        self.classic_windows().remove(&hwnd);
    }

    /// Releases every object the outpost holds, for its shutdown, once no
    /// worker runs: every node leaves both registries, and its text
    /// patterns, what its text is known to support, its anchors, and its
    /// terminal memory go with it. Returns how many nodes there were.
    fn release_everything(&self) -> usize {
        let nodes: Vec<u64> = self
            .uia_registry
            .ids()
            .into_iter()
            .chain(self.msaa_registry.ids())
            .map(NodeId::number)
            .collect();
        let objects = (
            self.uia_registry.retain(|_| false),
            self.msaa_registry.retain(|_| false),
        );
        text_reads::forget(self, nodes.iter().copied());
        self.patterns().clear();
        self.text_support().clear();
        self.terminals().clear();
        *self.caret_watch() = None;
        drop(objects);
        nodes.len()
    }

    fn push(&self, item: Item, trace: TraceId, observed_at_ms: u64, timing: EventTiming) {
        self.intake.push(Entry {
            item,
            trace,
            observed_at_ms,
            timing,
        });
    }
}

/// The outpost: owns the context and the event thread for one target
/// application, fixed for the outpost's whole life.
pub struct Outpost {
    context: Arc<Context>,
    _event_thread: EventThread,
    _writer: JoinHandle<()>,
}

impl Outpost {
    /// Creates an outpost watching `target_pid` for its whole life, with
    /// the default [`OutpostOptions`]; see [`Outpost::with_options`].
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources.
    #[must_use]
    pub fn new(pipe: Box<dyn Write + Send>, target_pid: u32) -> Self {
        Self::with_options(pipe, target_pid, OutpostOptions::default())
    }

    /// Creates an outpost watching `target_pid` for its whole life: starts
    /// the writer, the worker, and the watchdog, installs the MSAA hooks and
    /// the focus-following UIA property subscription, and announces
    /// readiness.
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources.
    #[must_use]
    pub fn with_options(
        pipe: Box<dyn Write + Send>,
        target_pid: u32,
        options: OutpostOptions,
    ) -> Self {
        Self::with_focused_element_reader(pipe, target_pid, options, Arc::new(Uia::focused_element))
    }

    /// [`Outpost::with_options`], reading the element that has the keyboard
    /// focus with `focused_element` rather than from the system: for a test
    /// that hands the outpost a UIA focus in an application that does not
    /// have the keyboard focus, and measures everything the outpost does
    /// with it.
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources.
    #[must_use]
    pub fn with_focused_element_reader(
        pipe: Box<dyn Write + Send>,
        target_pid: u32,
        options: OutpostOptions,
        focused_element: FocusedElementReader,
    ) -> Self {
        let (outbound, writer) = Outbound::start(pipe);
        let id_counter = Arc::new(AtomicU64::new(1));
        let anchor_counter = Arc::new(AtomicU64::new(0));
        let held_anchors = HeldAnchors::default();
        let context = Arc::new(Context {
            target_pid,
            outbound,
            intake: Intake::default(),
            watch: Watch::default(),
            uia_registry: UiaRegistry::new(Arc::clone(&id_counter)),
            msaa_registry: MsaaRegistry::new(id_counter),
            arbitrator: Mutex::new(Arbitrator::new(&[])),
            tracking: Mutex::new(Tracking::default()),
            focus_properties: OnceLock::new(),
            remote_operations: options.remote_operations,
            classic_windows: Mutex::new(HashMap::new()),
            text_events: OnceLock::new(),
            caret_watch: Mutex::new(None),
            held_anchors: Arc::clone(&held_anchors),
            uia_anchors: Mutex::new(Anchors::sharing(
                Arc::clone(&anchor_counter),
                Arc::clone(&held_anchors),
            )),
            edit_anchors: Mutex::new(Anchors::sharing(anchor_counter, held_anchors)),
            patterns: Mutex::new(HashMap::new()),
            text_support: Mutex::new(HashMap::new()),
            caret_read: Mutex::new(CaretRead::default()),
            fetches: Mutex::new(Fetches::default()),
            terminals: Mutex::new(HashMap::new()),
            focused_element,
            foreground: Mutex::new(Arc::new(window::foreground_window_handle)),
        });
        if let Some(registration) = register_focus_properties(&context) {
            let _ = context.focus_properties.set(registration);
        }
        if let Some(registration) = register_text_events(&context) {
            let _ = context.text_events.set(registration);
        }

        worker::start(&context);

        // The MSAA hooks: process-scoped value, state, name, selection, and
        // destroy events (decision D13). A destroy is queued only for
        // a window, the one kind of object whose end the outpost tracks.
        let hook_context = Arc::clone(&context);
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let context = Arc::clone(&hook_context);
            Box::new(move |kind, hwnd, id_object, id_child, raised_ms_ago| {
                if kind == verbatim_ia2::WinEventKind::Destroy
                    && (id_object != windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW.0
                        || id_child != verbatim_ia2::CHILDID_SELF)
                {
                    return;
                }
                context.push(
                    Item::Msaa {
                        kind,
                        hwnd,
                        id_object,
                        id_child,
                    },
                    TraceId::mint(),
                    now_ms(),
                    EventTiming {
                        raised_ms_ago: Some(raised_ms_ago),
                        observed_at_us: now_us(),
                        ..EventTiming::default()
                    },
                );
            })
        });
        let event_thread = EventThread::spawn(target_pid, APP_SUBSCRIPTIONS, make_callback);

        let outpost = Self {
            context,
            _event_thread: event_thread,
            _writer: writer,
        };
        outpost.context.outbound.urgent(OutpostToSupervisor::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(target_pid),
        });
        outpost
    }

    /// Waits until the worker has handled everything queued before this
    /// call and every follow-up those entries queued, the focus-following
    /// subscriptions have made every move the worker asked of them, and
    /// every message the worker published has been written to the pipe. A
    /// test that measures what the outpost's handling cost the application,
    /// or that asserts the outpost said nothing more, waits on this for its
    /// evidence. Returns early if the worker is abandoned meanwhile.
    pub fn settle(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        self.context.push(
            Item::Settle(done_tx),
            TraceId::mint(),
            now_ms(),
            EventTiming::default(),
        );
        // An error means the worker dropped the request, abandoned.
        let _ = done_rx.recv();
    }

    /// Shuts the outpost down cleanly ([`SupervisorToOutpost::Shutdown`]),
    /// on the calling thread, returning once it is done:
    ///
    /// 1. The intake closes: nothing new is taken, and what was waiting is
    ///    dropped.
    /// 2. The `WinEvent` hooks are removed, with the thread that pumped
    ///    them.
    /// 3. Every UIA event handler is removed: each subscription's client
    ///    removes everything it registered (`RemoveAllEventHandlers`).
    /// 4. The worker finishes the entry in hand, and every abandoned worker
    ///    returns from its call, however long the application takes to
    ///    answer or UIA takes to end the call (its connection and
    ///    transaction timeouts). No call is cut off.
    /// 5. Every object held is released: the registries' UIA elements and
    ///    MSAA objects, and the text patterns and ranges kept for text and
    ///    terminals.
    /// 6. The writer writes what is queued and closes the pipe.
    /// 7. The calling thread leaves COM, and the process's hold on the
    ///    multithreaded apartment is given back.
    ///
    /// The calling thread must be in COM's multithreaded apartment, or able
    /// to join it, since it releases UIA objects. What happens to the
    /// process afterwards is the caller's: the outpost binary ends itself
    /// without running DLL detach code (`docs/architecture.md`, "Process
    /// lifetime").
    pub fn shutdown(self) {
        let started = std::time::Instant::now();
        let joined = verbatim_uia::init_mta();
        let Self {
            context,
            _event_thread: event_thread,
            _writer: writer,
        } = self;
        drop(context.intake.close());
        drop(event_thread);
        if let Some(registration) = context.focus_properties.get() {
            registration.close();
        }
        if let Some(registration) = context.text_events.get() {
            registration.close();
        }
        let handlers_removed = started.elapsed();
        let workers = worker::stop(&context);
        let calls_finished = started.elapsed();
        let objects = context.release_everything();
        verbatim_uia::release_thread_state();
        context.outbound.close();
        let _ = writer.join();
        if joined.is_ok() {
            verbatim_uia::leave_mta();
        }
        verbatim_uia::release_mta_usage();
        tracing::info!(
            target_pid = context.target_pid,
            handlers_removed_ms = handlers_removed.as_millis(),
            calls_finished_ms = calls_finished.as_millis(),
            elapsed_ms = started.elapsed().as_millis(),
            workers,
            objects,
            "the outpost shut down"
        );
    }

    /// Reads the time for arbitration's kept verdicts from `clock` from now
    /// on, in place of the system's ([`Arbitrator::set_clock`]): a test
    /// that counts an operation's calls decides when a window's verdict of
    /// no UIA provider runs out and its probe is made again.
    pub fn set_arbitration_clock(&self, clock: crate::arbitration::Clock) {
        self.context.arbitrator().set_clock(clock);
    }

    /// Reads the foreground window with `reader` from now on, in place of
    /// the system's, when recording the window a focus was reported in,
    /// whose own object NVDA holds as the foreground and so never speaks a
    /// state change on as an ancestor: a test whose application runs on a
    /// desktop where no window can take the foreground says which window
    /// is the foreground.
    pub fn set_foreground_reader(&self, reader: ForegroundReader) {
        *self
            .context
            .foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = reader;
    }

    /// Handles one command from Core, on the reader thread. Pings are
    /// answered here; everything that reads the application is queued for
    /// the worker.
    pub fn handle_command(&self, command: &SupervisorToOutpost) {
        let context = &self.context;
        match command {
            SupervisorToOutpost::SetBackendOverride { backend_override } => {
                context
                    .arbitrator()
                    .set_forced(backend_override.map(|backend| backend == Backend::Uia));
            }
            SupervisorToOutpost::DeliverFact {
                trace_id,
                observed_at_ms,
                timing,
                fact,
            } => context.push(
                Item::Fact(fact.clone()),
                *trace_id,
                *observed_at_ms,
                EventTiming {
                    relayed_at_us: now_us(),
                    ..*timing
                },
            ),
            SupervisorToOutpost::Query {
                trace_id,
                request_id,
                query,
            } => context.push(
                Item::Query {
                    request_id: *request_id,
                    query: unstamped(query),
                },
                *trace_id,
                now_ms(),
                EventTiming {
                    relayed_at_us: now_us(),
                    ..EventTiming::default()
                },
            ),
            SupervisorToOutpost::Cancel { request_id } => {
                if context.intake.cancel(*request_id) {
                    // Queuing never waits, so the answer keeps its place
                    // among the others.
                    context.outbound.send(OutpostToSupervisor::Reply {
                        trace_id: TraceId::mint(),
                        request_id: *request_id,
                        outcome: QueryOutcome::NotStarted,
                        timing: EventTiming::default(),
                    });
                }
            }
            SupervisorToOutpost::Ping { seq } => {
                context.outbound.urgent(OutpostToSupervisor::Pong {
                    seq: *seq,
                    parked_count: context.watch.abandoned(),
                });
            }
            SupervisorToOutpost::Fetches(fetches) => {
                *context
                    .fetches
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = *fetches;
                context.msaa_registry.set_fetches(*fetches);
            }
            SupervisorToOutpost::TerminalLines(lines) => {
                context
                    .outbound
                    .set_terminal_lines((*lines).clamp(1, verbatim_model::MAX_TERMINAL_LINES));
            }
            // `run_pipe` ends its loop on it and calls `shutdown`, which
            // takes the outpost.
            SupervisorToOutpost::Shutdown => {}
            SupervisorToOutpost::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            } => {
                // Never the anchor stores' own locks, which the worker holds
                // while it reads text: the reader would wait for the read.
                *context
                    .held_anchors
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = anchors.iter().copied().collect();
                context.push(
                    Item::NodesHeld {
                        nodes: nodes.clone(),
                        acknowledged: *acknowledged,
                    },
                    TraceId::mint(),
                    now_ms(),
                    EventTiming::default(),
                );
            }
        }
    }
}

/// A query with its node ids as this outpost issued them: they arrive
/// stamped with Core's outpost id.
fn unstamped(query: &Query) -> Query {
    match query {
        Query::Navigate { node_id, kind } => Query::Navigate {
            node_id: node_id.unstamped(),
            kind: *kind,
        },
        Query::Activate { node_id } => Query::Activate {
            node_id: node_id.unstamped(),
        },
        Query::Ancestors { node_id } => Query::Ancestors {
            node_id: node_id.unstamped(),
        },
        Query::Text { node_id, op } => Query::Text {
            node_id: node_id.unstamped(),
            op: op.clone(),
        },
        other => other.clone(),
    }
}

/// What a UIA callback captures: the element's cached parts, its cached
/// window handle, and an agile reference for anything the worker must ask
/// it. No call reaches the application: both reads are cached.
fn capture(element: &IUIAutomationElement, kind: UiaKind) -> UiaEvent {
    let (parts, hwnd) = (
        snapshot_parts_from_cached_element(element),
        cached_native_window_handle(element),
    );
    UiaEvent {
        kind,
        parts: UiaSnapshotFact {
            runtime_id: parts.runtime_id,
            role: parts.role,
            name: parts.name,
            value: parts.value,
            states: parts.states,
            details: parts.details,
        },
        hwnd,
        element: AgileReference::new(element).ok(),
    }
}

/// Starts the focus-following UIA property subscription (outpost redesign,
/// "Focus-following UIA subscriptions"): name, value, and state changes on the
/// focused element and its ancestors only, moved by the worker each time it
/// reports a new focus. Selections and notifications come desktop-wide from
/// the listener instead.
fn register_focus_properties(context: &Arc<Context>) -> Option<Registration> {
    let callback_context = Arc::clone(context);
    let callback = Arc::new(move |element: &IUIAutomationElement, property_id: i32| {
        // The property element carries cached values.
        let event = capture(element, UiaKind::Property(property_id));
        callback_context.push(
            Item::Uia(event),
            TraceId::mint(),
            now_ms(),
            EventTiming {
                observed_at_us: now_us(),
                ..EventTiming::default()
            },
        );
    });
    let subscription = Subscription::Properties {
        properties: FOCUS_PROPERTIES.to_vec(),
        callback,
    };
    match Registration::new(vec![subscription], Scope::Nothing) {
        Ok(registration) => Some(registration),
        Err(error) => {
            fault(
                context,
                format!("UIA property subscription failed: {error}"),
            );
            None
        }
    }
}

/// Starts the focus-following UIA subscription to a text focus's caret and
/// text changes (`Text_TextSelectionChanged` and `Text_TextChanged`) and
/// its active text position changes, one event handler group listening
/// nowhere until the worker reports a focus with text. A caret or text
/// change also has the worker check a caret key's watch.
fn register_text_events(context: &Arc<Context>) -> Option<Registration> {
    let callback_context = Arc::clone(context);
    let callback = Arc::new(move |element: &IUIAutomationElement, event_id: i32| {
        let kind = if event_id == UIA_Text_TextSelectionChangedEventId.0 {
            UiaKind::TextSelection
        } else {
            UiaKind::TextChanged
        };
        callback_context.push(
            Item::Uia(capture(element, kind)),
            TraceId::mint(),
            now_ms(),
            EventTiming {
                observed_at_us: now_us(),
                ..EventTiming::default()
            },
        );
    });
    let subscription = Subscription::Events {
        events: vec![
            UIA_Text_TextSelectionChangedEventId,
            UIA_Text_TextChangedEventId,
        ],
        callback,
    };
    let position_context = Arc::clone(context);
    let position = Subscription::ActiveTextPosition {
        callback: Arc::new(
            move |element: &IUIAutomationElement, range: Option<&IUIAutomationTextRange>| {
                let range = range
                    .and_then(|range| AgileReference::new(range).ok())
                    .map(intake::ActiveRange);
                position_context.push(
                    Item::Uia(capture(element, UiaKind::ActiveTextPosition(range))),
                    TraceId::mint(),
                    now_ms(),
                    EventTiming {
                        observed_at_us: now_us(),
                        ..EventTiming::default()
                    },
                );
            },
        ),
    };
    match Registration::new(vec![subscription, position], Scope::Nothing) {
        Ok(registration) => Some(registration),
        Err(error) => {
            fault(context, format!("UIA text subscription failed: {error}"));
            None
        }
    }
}

fn fault(context: &Context, detail: String) {
    context.outbound.send(OutpostToSupervisor::Fault { detail });
}

/// Runs an outpost driven by the Core pipes, watching `target_pid` for its
/// whole life: reads commands from `pipe_in` and writes outbound messages to
/// `pipe_out` until Core asks it to shut down
/// ([`SupervisorToOutpost::Shutdown`]) or the command stream ends, then
/// shuts it down cleanly ([`Outpost::shutdown`]) and returns. When the
/// target application exits, Core is told
/// ([`OutpostToSupervisor::TargetExited`]) and asks for the shutdown.
///
/// # Errors
///
/// Returns any I/O error reading the command stream, after the shutdown.
pub fn run_pipe(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
    target_pid: u32,
    options: OutpostOptions,
) -> io::Result<()> {
    let outpost = Outpost::with_options(pipe_out, target_pid, options);
    watch_target(&outpost.context);
    let mut reader = BufReader::new(pipe_in);
    let ended = loop {
        match read_message::<_, SupervisorToOutpost>(&mut reader) {
            Ok(Some(SupervisorToOutpost::Shutdown)) => {
                tracing::info!("shutting down, as Core asked");
                break Ok(());
            }
            Ok(Some(command)) => outpost.handle_command(&command),
            Ok(None) => {
                tracing::warn!("Core's command pipe closed; shutting down");
                break Ok(());
            }
            Err(error) => break Err(error),
        }
    };
    outpost.shutdown();
    ended
}

/// Tells Core when the target application exits, from a thread of its own
/// that waits on the application's process. The thread is not waited for:
/// it holds nothing of COM's and ends with the process.
fn watch_target(context: &Arc<Context>) {
    let context = Arc::clone(context);
    let spawned = thread::Builder::new()
        .name("verbatim-target-watch".to_owned())
        .spawn(move || {
            if window::wait_for_process_exit(context.target_pid) {
                tracing::info!(
                    target_pid = context.target_pid,
                    "the target application exited"
                );
                context.outbound.urgent(OutpostToSupervisor::TargetExited);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the target application's exit is not watched");
    }
}

/// Runs an outpost in dev-attach mode: writes outbound messages as JSON lines
/// to stdout, watches the given target for its whole life, asks for the
/// current focus, and streams events until the process is killed.
///
/// # Errors
///
/// Returns an I/O error only if dev-mode setup fails before the event loop
/// begins; once running it blocks until the process is terminated.
pub fn run_attach(target_pid: u32, options: OutpostOptions) -> io::Result<()> {
    let outpost = Outpost::with_options(Box::new(io::stdout()), target_pid, options);
    outpost.handle_command(&SupervisorToOutpost::Query {
        trace_id: TraceId::mint(),
        request_id: 0,
        query: Query::FocusNow,
    });
    loop {
        thread::park();
    }
}
