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
//! 6. The writer ([`outbound`]): pongs and `Ready` go first.
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
mod text_reads;
pub(crate) mod window;
mod worker;

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};
use windows::core::AgileReference;

use verbatim_ia2::{APP_SUBSCRIPTIONS, NodeIdRegistry as MsaaRegistry, WinEventCallback};
use verbatim_model::{Backend, NodeId, Pid, TraceId};
use verbatim_uia::map::{cached_native_window_handle, snapshot_parts_from_cached_element};
use verbatim_uia::{
    FOCUS_PROPERTIES, NodeIdRegistry as UiaRegistry, Registration, Scope, Subscription,
};

use crate::arbitration::Arbitrator;
use crate::event_thread::EventThread;
use crate::protocol::{
    EventTiming, OutpostToSupervisor, Query, QueryOutcome, SupervisorToOutpost, UiaSnapshotFact,
    now_us, read_message,
};
use crate::text::Anchors;
use crate::text::uia::UiaPos;

use intake::{Entry, Intake, Item, UiaEvent, UiaKind};
use outbound::Outbound;
use worker::{Tracking, Watch};

pub(crate) use window::now_ms;

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
    /// Caret events as they arrive, for a caret key's wait.
    caret_events: text_reads::CaretEvents,
    /// The text anchors minted in UIA text and in edit controls; both number
    /// theirs from one counter.
    uia_anchors: Mutex<Anchors<UiaPos>>,
    edit_anchors: Mutex<Anchors<u32>>,
    /// Each UIA node's text patterns, once fetched.
    patterns: Mutex<HashMap<u64, text_reads::Patterns>>,
    /// The node whose caret the worker last read, and when that read began,
    /// in microseconds: a caret event observed before it changes nothing
    /// the read did not see.
    caret_read: Mutex<Option<(u64, u64)>>,
}

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

    /// Records that the worker is reading `node`'s caret now.
    fn caret_read(&self, node: NodeId) {
        *self
            .caret_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((node.number(), now_us()));
    }

    /// Whether the worker has read `node`'s caret since `observed_us`, so
    /// a caret event observed then needs no report of its own.
    fn caret_read_since(&self, node: NodeId, observed_us: u64) -> bool {
        self.caret_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|(read, at)| read == node.number() && at >= observed_us)
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
        let (outbound, writer) = Outbound::start(pipe);
        let id_counter = Arc::new(AtomicU64::new(1));
        let anchor_counter = Arc::new(AtomicU64::new(0));
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
            caret_events: text_reads::CaretEvents::default(),
            uia_anchors: Mutex::new(Anchors::new(Arc::clone(&anchor_counter))),
            edit_anchors: Mutex::new(Anchors::new(anchor_counter)),
            patterns: Mutex::new(HashMap::new()),
            caret_read: Mutex::new(None),
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
                if matches!(
                    kind,
                    verbatim_ia2::WinEventKind::Caret
                        | verbatim_ia2::WinEventKind::TextSelectionChange
                ) {
                    context.caret_events.arrived();
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
                    // Sent ahead of ordinary messages, so the reader never
                    // waits on a full queue.
                    context.outbound.urgent(OutpostToSupervisor::Reply {
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
            SupervisorToOutpost::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            } => {
                let held = anchors.iter().copied();
                context.uia_anchors().set_held(held.clone());
                context.edit_anchors().set_held(held);
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
    match Registration::new(subscription, Scope::Nothing) {
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
/// text changes (`Text_TextSelectionChanged` and `Text_TextChanged`),
/// listening nowhere until the worker reports a focus with text. A caret
/// change also counts for a caret key's wait for evidence.
fn register_text_events(context: &Arc<Context>) -> Option<Registration> {
    let callback_context = Arc::clone(context);
    let callback = Arc::new(move |element: &IUIAutomationElement, event_id: i32| {
        let kind = if event_id == UIA_Text_TextSelectionChangedEventId.0 {
            callback_context.caret_events.arrived();
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
    match Registration::new(subscription, Scope::Nothing) {
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
/// `pipe_out` until end of stream. Core ends an outpost by closing its job
/// handle.
///
/// # Errors
///
/// Returns any I/O error reading the command stream.
pub fn run_pipe(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
    target_pid: u32,
    options: OutpostOptions,
) -> io::Result<()> {
    let outpost = Outpost::with_options(pipe_out, target_pid, options);
    let mut reader = BufReader::new(pipe_in);
    while let Some(command) = read_message::<_, SupervisorToOutpost>(&mut reader)? {
        outpost.handle_command(&command);
    }
    Ok(())
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
