//! The outpost runtime: the event thread, the UIA registrations, the query
//! pool, and the command loop that ties them together (architecture section 1).
//!
//! One outpost watches one application, fixed for its whole life (decision
//! D9): its target pid arrives at spawn (the command line in production mode,
//! an argument in dev-attach mode), hooks and UIA registrations install once
//! during construction, and there is no later retarget. Its UIA focus and
//! property handlers and its out-of-context MSAA `WinEvent` hooks run
//! simultaneously; the arbitration cross-filter (see [`crate::arbitration`])
//! ensures only one backend announces any given change. Trace IDs are minted
//! the moment an OS event is observed; the snapshot version increments on
//! every emitted event.

use std::ffi::c_void;
use std::io::{self, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Sender, unbounded};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, NotificationKind, NotificationProcessing, UIA_NamePropertyId,
    UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo,
    GetMessageW, GetPropW, GetWindowThreadProcessId, MSG, PostThreadMessageW, TranslateMessage,
};
use windows::core::{BOOL, HSTRING};

use verbatim_ia2::{
    CHILDID_SELF, NodeIdRegistry as MsaaRegistry, WinEventCallback, WinEventHook, WinEventKind,
};
use verbatim_model::{
    Backend, HIDDEN_FRAME_WINDOW_PROP, NodeDetails, NodeSnapshot, NormalizedEvent, Pid,
    PropertyChange, SnapshotVersion, TraceId,
};
use verbatim_uia::map::{
    cached_native_window_handle, notification_kind_from_uia, notification_processing_from_uia,
    snapshot_from_cached_element,
};
use verbatim_uia::{
    FocusRegistration, NodeIdRegistry as UiaRegistry, NotificationRegistration,
    PropertyRegistration, SelectionRegistration, has_server_side_provider, nearest_window_handle,
};

use crate::arbitration::{Arbitrator, window_class_name};
use crate::protocol::{
    DumpedTree, NavigateDirection, NavigateOutcome, OutpostToSupervisor, SupervisorToOutpost,
    read_message, write_message,
};
use crate::query_pool::{QueryPool, Worker};

/// The deadline for a single deadline-guarded query-pool call (fetch, probe,
/// synthetic focus): architecture section 1's per-call deadline.
const QUERY_DEADLINE: Duration = Duration::from_millis(300);

/// A slightly longer deadline for a synthetic focus query, which chains an
/// arbitration decision and a cross-process fetch.
const FOCUS_DEADLINE: Duration = Duration::from_millis(400);

/// Deadline for a `DumpTree` walk: generous relative to a single query call
/// since it chains an arbitration decision with a subtree walk of up to
/// [`MAX_DUMP_NODES`] nodes; a hung provider still abandons the call rather
/// than wedging the outpost.
const DUMP_TREE_DEADLINE: Duration = Duration::from_secs(5);

/// Depth cap for a `DumpTree` walk (the root is depth 0).
const MAX_DUMP_DEPTH: u32 = 64;

/// Node-count cap for a `DumpTree` walk, across the whole tree.
const MAX_DUMP_NODES: usize = 4096;

/// Deadline for an `AncestorChain` walk: generous relative to a single query
/// call since it chains up to [`MAX_ANCESTOR_HOPS`] per-hop round trips (the
/// same reasoning as [`DUMP_TREE_DEADLINE`]), but a hung provider still
/// abandons the call rather than wedging the outpost.
const ANCESTOR_CHAIN_DEADLINE: Duration = Duration::from_secs(5);

/// Cap on the number of ancestors an `AncestorChain` walk returns.
const MAX_ANCESTOR_HOPS: u32 = 64;

/// Deadline for a single object-navigation hop (parent, sibling, child)
/// issued through the reducer's `Fetch` path — a single cross-process
/// walker step, so the ordinary per-call deadline suffices.
const NAVIGATE_DEADLINE: Duration = Duration::from_millis(400);

/// The [`NavigateDirection`] a navigation [`verbatim_model::QueryKind`]
/// names, or `None` for the plain snapshot re-read (which is not a
/// navigation).
fn navigate_direction_of(kind: verbatim_model::QueryKind) -> Option<NavigateDirection> {
    use verbatim_model::QueryKind;
    match kind {
        QueryKind::Parent => Some(NavigateDirection::Parent),
        QueryKind::NextSibling => Some(NavigateDirection::NextSibling),
        QueryKind::PreviousSibling => Some(NavigateDirection::PreviousSibling),
        QueryKind::FirstChild => Some(NavigateDirection::FirstChild),
        _ => None,
    }
}

/// How many times [`Outpost::handle_announce_focus`] retries the
/// focused-control query when nothing is found yet (a control that has not
/// focused itself between the foreground change and this outpost's first
/// attempt — the second race `docs/roadmap.md`'s M3 section names).
const ANNOUNCE_RETRY_ATTEMPTS: u32 = 5;

/// Spacing between focused-control retry attempts. Four gaps across five
/// attempts spread the whole retry window across roughly two seconds.
const ANNOUNCE_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Shared state cloned into every callback and query. All fields are cheap to
/// clone (channels, atomics, and `Arc`-backed registries and the arbitrator).
#[derive(Clone)]
struct Shared {
    outbound: Sender<OutpostToSupervisor>,
    version: Arc<AtomicU64>,
    arbitrator: Arc<Mutex<Arbitrator>>,
    pool: QueryPool,
    uia_registry: UiaRegistry,
    msaa_registry: MsaaRegistry,
    /// Bumped by every `AnnounceFocus`; an in-flight retry loop compares its
    /// captured generation against the current value before each attempt
    /// and before emitting, so a superseding announce aborts stale retries
    /// (a rapid re-foreground, or Notepad's own bursty startup events —
    /// see this crate's supervisor docs for the retry-starvation history
    /// this guards against).
    generation: Arc<AtomicU64>,
}

impl Shared {
    /// Sends one normalized event, stamping it with the next snapshot version
    /// and the observation timestamp that anchors the latency timeline.
    fn emit(&self, trace: TraceId, backend: Backend, event: NormalizedEvent) {
        let version = SnapshotVersion(self.version.fetch_add(1, Ordering::Relaxed) + 1);
        let observed_at_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let _ = self.outbound.send(OutpostToSupervisor::Event {
            trace_id: trace,
            observed_at_ms,
            backend,
            version,
            event,
        });
    }

    /// Sends a fault report for logs and diagnostics.
    fn fault(&self, detail: String) {
        let _ = self.outbound.send(OutpostToSupervisor::Fault { detail });
    }

    /// Warms the arbitration cache for `hwnd` off the calling thread: runs the
    /// blocking probe on a deadline-guarded query worker, then records the
    /// verdict. A probe timeout is reported as a fault and cached as non-UIA.
    fn schedule_probe(&self, hwnd: isize, class: String) {
        let shared = self.clone();
        let _ = thread::Builder::new()
            .name("verbatim-arb-probe".to_owned())
            .spawn(move || {
                {
                    // Skip if another thread already resolved it.
                    let arbitrator = shared.lock_arbitrator();
                    if arbitrator.verdict(hwnd, &class).is_some() {
                        return;
                    }
                }
                let probed = shared.pool.run(QUERY_DEADLINE, move |_worker| {
                    has_server_side_provider(hwnd)
                });
                if let Some(is_uia) = probed {
                    shared.lock_arbitrator().record_probe(hwnd, is_uia);
                } else {
                    shared.lock_arbitrator().record_probe(hwnd, false);
                    shared.fault(format!(
                        "UiaHasServerSideProvider timed out for hwnd {hwnd}; treated as non-UIA"
                    ));
                }
            });
    }

    fn lock_arbitrator(&self) -> std::sync::MutexGuard<'_, Arbitrator> {
        self.arbitrator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether `hwnd` carries Core's hidden-main-frame marker property (decision
/// D9; see [`verbatim_model::HIDDEN_FRAME_WINDOW_PROP`]). `GetPropW`
/// tolerates any handle, including an invalid one, so this never blocks and
/// is safe to call from an event-callback thread as well as a query worker.
fn window_is_hidden_frame(hwnd: isize) -> bool {
    let name = HSTRING::from(HIDDEN_FRAME_WINDOW_PROP);
    // SAFETY: GetPropW reads a window property by name; an invalid or
    // property-less window simply yields a null handle.
    let value = unsafe { GetPropW(HWND(hwnd as *mut c_void), &name) };
    !value.0.is_null()
}

/// The MSAA `WinEvent` callback body: cross-filter on the event thread, then
/// hand acquisition to the query pool. Never blocks.
fn handle_msaa_event(
    shared: &Shared,
    kind: WinEventKind,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
) {
    // Hidden-frame suppression (decision D9) applies only to the window's
    // own focus event (`id_child == CHILDID_SELF`), never to a child
    // element's event on some other window that merely shares an hwnd by
    // coincidence of enumeration order.
    if kind == WinEventKind::Focus && id_child == CHILDID_SELF && window_is_hidden_frame(hwnd) {
        return;
    }
    let class = window_class_name(hwnd);
    match shared.lock_arbitrator().verdict(hwnd, &class) {
        Some(true) => return, // UIA window: MSAA is suppressed for it.
        Some(false) => {}
        None => shared.schedule_probe(hwnd, class), // provisional non-UIA; warm cache.
    }
    let trace = TraceId::mint();
    let shared = shared.clone();
    let pool = shared.pool.clone();
    pool.submit(move |worker| {
        if let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &shared.msaa_registry,
        ) {
            // Focus events are enriched with the node's ancestry right here
            // on the query worker (the walk is query-pool-only by
            // contract); every other kind maps directly.
            let event = if kind == WinEventKind::Focus {
                let (ancestors, selected_child) = focus_enrichment_query(worker, &shared, &node);
                NormalizedEvent::FocusChanged {
                    node: node.clone(),
                    ancestors,
                    selected_child,
                }
            } else {
                msaa_event(kind, &node)
            };
            shared.emit(trace, Backend::Msaa, event);
        }
    });
}

/// Maps an MSAA event kind and its acquired snapshot to a normalized event.
/// Focus is handled by the caller (it enriches with ancestry first); this
/// covers the direct mappings.
fn msaa_event(kind: WinEventKind, node: &NodeSnapshot) -> NormalizedEvent {
    match kind {
        WinEventKind::Focus => NormalizedEvent::FocusChanged {
            node: node.clone(),
            ancestors: Vec::new(),
            selected_child: None,
        },
        WinEventKind::ValueChange => NormalizedEvent::ValueChanged {
            node_id: node.id,
            value: node.value.clone(),
        },
        WinEventKind::NameChange => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::Name(node.name.clone()),
        },
        // The acquired snapshot already carries the complete new state set (read
        // from accState); forward it so the reducer can diff against its stored
        // snapshot (a check box toggling, a control becoming unavailable).
        WinEventKind::StateChange => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::States(node.states),
        },
        // EVENT_OBJECT_SELECTION and its SELECTIONADD/SELECTIONREMOVE/
        // SELECTIONWITHIN siblings all collapse to this one WinEventKind
        // (see verbatim_ia2::WinEventHook's doc); the acquired snapshot is
        // the node the event's own address named, which is the selected (or
        // most recently affected) node in every one of the four cases.
        WinEventKind::Selection => NormalizedEvent::SelectionChanged { node: node.clone() },
    }
}

/// Resolves the window an arbitrary UIA `element` belongs to and applies the
/// arbitration cross-filter in one pass, returning both the delivery verdict
/// and the resolved window handle — the handle is reused by the focus
/// callback for hidden-frame suppression instead of resolving it a second
/// time.
///
/// Most elements that raise events are not windows in their own right — a
/// menu item and a list item are children of one — so
/// [`cached_native_window_handle`] reads 0 for them, and arbitration is
/// per-window: something has to say which window's verdict a non-window
/// element's event falls under.
///
/// The M1 heuristic used the window holding the keyboard focus. That is
/// wrong for a popup menu specifically, because a popup menu never takes
/// keyboard focus: the "focused window" it found was always the menu's
/// *owner*, while the MSAA hook's event for the very same menu item carries
/// the popup window itself. The two backends then arbitrated on two
/// different windows and could both decide to defer, so nobody announced the
/// menu item at all — found by the M2 E2E suite against the VM. The
/// heuristic's compensation was to keep the event whenever no verdict was
/// cached yet for an inferred window, accepting a duplicate announcement
/// from both backends rather than risk losing it.
///
/// The real fix is attribution, not compensation: [`nearest_window_handle`]
/// is UIA's own answer to "which window is this element's", a single
/// cross-process call that walks up from the element (NVDA's
/// `getNearestWindowHandle`) rather than guessing from keyboard focus. For
/// the popup-menu case it resolves the menu item straight to the popup
/// window itself — the same window the MSAA event for that item carries — so
/// both backends arbitrate on one shared hwnd and the duplicate-avoidance
/// concession below is safe again.
///
/// # Safety
///
/// `element` must be a cached element from the base cache request.
unsafe fn resolve_window_and_filter(
    shared: &Shared,
    element: &IUIAutomationElement,
) -> (bool, Option<isize>) {
    // SAFETY: forwarded to the caller's contract; the cached native window
    // handle read does not block.
    let cached_hwnd = unsafe { cached_native_window_handle(element) };
    let hwnd = if cached_hwnd != 0 {
        Some(cached_hwnd)
    } else {
        nearest_window_handle(element)
    };
    // `nearest_window_handle` can itself fail (a COM error, or the element's
    // whole process already gone); fall back to the keyboard-focus window as
    // a last resort rather than losing the event outright, and if even that
    // finds nothing, there is no window to arbitrate on at all, so keep it.
    let Some(hwnd) = hwnd.or_else(foreground_focus_window) else {
        return (true, None);
    };
    let class = window_class_name(hwnd);
    let deliver = match shared.lock_arbitrator().verdict(hwnd, &class) {
        Some(true) => true,
        Some(false) => false,
        None => {
            shared.schedule_probe(hwnd, class);
            // No verdict cached yet for a window attribution now trusts:
            // drop. The MSAA hook sees the same window (per the popup-menu
            // reasoning above) and carries the event instead, so this costs
            // nothing; keeping it here would be the doubled-announcement bug
            // the M1 heuristic had to compensate for.
            false
        }
    };
    (deliver, Some(hwnd))
}

/// Applies the UIA cross-filter for an element on a callback thread, for
/// callers (the property-change path) that do not also need the resolved
/// window handle. See [`resolve_window_and_filter`] for the full reasoning.
///
/// # Safety
///
/// `element` must be a cached element from the base cache request.
unsafe fn uia_passes_filter(shared: &Shared, element: &IUIAutomationElement) -> bool {
    // SAFETY: forwarded.
    unsafe { resolve_window_and_filter(shared, element).0 }
}

/// The event thread: installs the MSAA hooks for the fixed target pid once,
/// then pumps messages so the out-of-context callbacks are delivered.
struct EventThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl EventThread {
    fn spawn(
        target_pid: u32,
        make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync>,
    ) -> Self {
        let (id_tx, id_rx) = unbounded::<u32>();
        let join = thread::Builder::new()
            .name("verbatim-event".to_owned())
            .spawn(move || event_thread_main(target_pid, &id_tx, &make_callback))
            .expect("spawn event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            thread_id,
            join: Some(join),
        }
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        // SAFETY: WM_QUIT ends the message loop on the event thread.
        unsafe {
            let _ = PostThreadMessageW(
                self.thread_id,
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn event_thread_main(
    target_pid: u32,
    id_tx: &Sender<u32>,
    make_callback: &Arc<dyn Fn() -> WinEventCallback + Send + Sync>,
) {
    // SAFETY: GetCurrentThreadId is always sound.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = id_tx.send(thread_id);
    // Installed once, for the whole life of the outpost (decision D9): a
    // second live hook set on the same thread while a first is still
    // registered has been observed to permanently kill WinEvent delivery on
    // that thread for the rest of the process, which is exactly why this
    // pid is fixed at spawn instead of rebindable.
    let hook = match WinEventHook::install(target_pid, make_callback()) {
        Ok(installed) => Some(installed),
        Err(error) => {
            tracing::warn!(error, target_pid, "failed to install WinEvent hooks");
            None
        }
    };
    let mut message = MSG::default();
    loop {
        // SAFETY: standard message loop; `message` is fully owned here.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break; // WM_QUIT (0) or error (-1).
        }
        // SAFETY: dispatching a fully owned message.
        unsafe {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    drop(hook);
}

/// The outpost: owns the shared state, event thread, and UIA registrations
/// for one target application, fixed for the outpost's whole life.
pub struct Outpost {
    pid: u32,
    target_pid: u32,
    shared: Shared,
    _event_thread: EventThread,
    focus_registration: Option<FocusRegistration>,
    property_registration: Option<PropertyRegistration>,
    selection_registration: Option<SelectionRegistration>,
    notification_registration: Option<NotificationRegistration>,
    _writer: JoinHandle<()>,
}

impl Outpost {
    /// Creates an outpost watching `target_pid` for its whole life: installs
    /// the MSAA hooks and UIA registrations once, writes outbound messages
    /// through `writer` (the pipe to Core, or stdout in dev-attach mode),
    /// and announces readiness.
    ///
    /// # Panics
    ///
    /// Panics if the outbound writer or event thread cannot be spawned, which
    /// indicates the process is out of OS thread resources.
    #[must_use]
    pub fn new(writer: Box<dyn Write + Send>, target_pid: u32) -> Self {
        let (outbound, outbound_rx) = unbounded::<OutpostToSupervisor>();
        let writer_join = thread::Builder::new()
            .name("verbatim-outbound".to_owned())
            .spawn(move || {
                let mut writer = writer;
                while let Ok(message) = outbound_rx.recv() {
                    if write_message(&mut writer, &message).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn outbound writer");

        let id_counter = Arc::new(AtomicU64::new(1));
        let shared = Shared {
            outbound,
            version: Arc::new(AtomicU64::new(0)),
            arbitrator: Arc::new(Mutex::new(Arbitrator::new(&[]))),
            pool: QueryPool::new(2),
            uia_registry: UiaRegistry::new(id_counter.clone()),
            msaa_registry: MsaaRegistry::new(id_counter),
            generation: Arc::new(AtomicU64::new(0)),
        };

        let callback_shared = shared.clone();
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let shared = callback_shared.clone();
            Box::new(move |kind, hwnd, id_object, id_child| {
                handle_msaa_event(&shared, kind, hwnd, id_object, id_child);
            })
        });
        let event_thread = EventThread::spawn(target_pid, make_callback);

        let mut outpost = Self {
            pid: std::process::id(),
            target_pid,
            shared,
            _event_thread: event_thread,
            focus_registration: None,
            property_registration: None,
            selection_registration: None,
            notification_registration: None,
            _writer: writer_join,
        };
        outpost.install_uia_registrations(target_pid);

        let _ = outpost.shared.outbound.send(OutpostToSupervisor::Ready {
            outpost_pid: Pid(outpost.pid),
            target_pid: Pid(target_pid),
        });

        outpost
    }

    /// Sets or clears a forced backend override for every window of the
    /// target application, overriding arbitration.
    fn handle_set_backend_override(&self, backend_override: Option<Backend>) {
        self.shared
            .lock_arbitrator()
            .set_forced(backend_override.map(|backend| backend == Backend::Uia));
    }

    fn install_uia_registrations(&mut self, target_pid: u32) {
        self.install_focus_registration(target_pid);
        self.install_property_registration(target_pid);
        self.install_selection_registration(target_pid);
        self.install_notification_registration(target_pid);
    }

    /// Installs the global UIA focus-change registration, filtered to the
    /// target pid.
    fn install_focus_registration(&mut self, target_pid: u32) {
        let focus_shared = self.shared.clone();
        let focus_callback = Arc::new(move |element: &IUIAutomationElement| {
            // SAFETY: `element` is a cached focus element from the base cache
            // request, so the filter, hidden-frame check, and mapping read
            // only cached values.
            unsafe {
                let (deliver, hwnd) = resolve_window_and_filter(&focus_shared, element);
                if !deliver || hwnd.is_some_and(window_is_hidden_frame) {
                    return;
                }
                let node = snapshot_from_cached_element(element, &focus_shared.uia_registry);
                // Ancestry enrichment happens on a query worker, not this
                // callback thread: the walk is cross-process per hop and
                // both walk functions are query-pool-only by contract. The
                // worker re-fetches the element by the runtime id the
                // snapshot just registered (the same dispatch the
                // AncestorChain protocol query uses) and emits the enriched
                // event from there; a failed walk degrades to no context,
                // never to a lost focus announcement.
                let trace = TraceId::mint();
                let shared = focus_shared.clone();
                focus_shared.pool.submit(move |worker| {
                    let (ancestors, selected_child) =
                        focus_enrichment_query(worker, &shared, &node);
                    shared.emit(
                        trace,
                        Backend::Uia,
                        NormalizedEvent::FocusChanged {
                            node,
                            ancestors,
                            selected_child,
                        },
                    );
                });
            }
        });
        match FocusRegistration::new(target_pid, focus_callback) {
            Ok(registration) => self.focus_registration = Some(registration),
            Err(error) => self
                .shared
                .fault(format!("UIA focus registration failed: {error}")),
        }
    }

    /// Installs the UIA name/value/state property-change registration over
    /// the target's top-level windows.
    fn install_property_registration(&mut self, target_pid: u32) {
        let property_shared = self.shared.clone();
        let property_callback =
            Arc::new(move |element: &IUIAutomationElement, property_id: i32| {
                // SAFETY: as above, the property element carries cached values.
                unsafe {
                    if !uia_passes_filter(&property_shared, element) {
                        return;
                    }
                    let node = snapshot_from_cached_element(element, &property_shared.uia_registry);
                    let event = if property_id == UIA_NamePropertyId.0 {
                        NormalizedEvent::PropertyChanged {
                            node_id: node.id,
                            change: PropertyChange::Name(node.name.clone()),
                        }
                    } else if property_id == UIA_ValueValuePropertyId.0 {
                        NormalizedEvent::ValueChanged {
                            node_id: node.id,
                            value: node.value.clone(),
                        }
                    } else {
                        // A state-bearing property (toggle, enabled,
                        // expand/collapse) changed. `node.states` was rebuilt
                        // from cached values, so forward the complete new set.
                        NormalizedEvent::PropertyChanged {
                            node_id: node.id,
                            change: PropertyChange::States(node.states),
                        }
                    };
                    property_shared.emit(TraceId::mint(), Backend::Uia, event);
                }
            });
        let windows = top_level_windows(target_pid);
        match PropertyRegistration::new(windows, property_callback) {
            Ok(registration) => self.property_registration = Some(registration),
            Err(error) => self
                .shared
                .fault(format!("UIA property registration failed: {error}")),
        }
    }

    /// Installs the UIA `SelectionItem_ElementSelected` registration over
    /// the target's top-level windows (roadmap M3's selection-events
    /// bullet). Emitted events route through the same arbitration
    /// cross-filter as every other UIA event; the reducer does not announce
    /// them yet.
    fn install_selection_registration(&mut self, target_pid: u32) {
        let selection_shared = self.shared.clone();
        let selection_callback = Arc::new(move |element: &IUIAutomationElement| {
            // SAFETY: as above, the selected element carries cached values.
            unsafe {
                if !uia_passes_filter(&selection_shared, element) {
                    return;
                }
                let node = snapshot_from_cached_element(element, &selection_shared.uia_registry);
                selection_shared.emit(
                    TraceId::mint(),
                    Backend::Uia,
                    NormalizedEvent::SelectionChanged { node },
                );
            }
        });
        let windows = top_level_windows(target_pid);
        match SelectionRegistration::new(windows, selection_callback) {
            Ok(registration) => self.selection_registration = Some(registration),
            Err(error) => self
                .shared
                .fault(format!("UIA selection registration failed: {error}")),
        }
    }

    /// Installs the UIA `AutomationNotification` registration over the
    /// target's top-level windows (roadmap M3's generic notification-event
    /// bullet — snap layouts are the motivating case). Same filter and
    /// same not-yet-announced status as the selection registration.
    fn install_notification_registration(&mut self, target_pid: u32) {
        let notification_shared = self.shared.clone();
        let notification_callback = Arc::new(
            move |element: &IUIAutomationElement,
                  kind: NotificationKind,
                  processing: NotificationProcessing,
                  display_string: Option<String>,
                  activity_id: Option<String>| {
                // SAFETY: as above, the notifying element carries cached values.
                unsafe {
                    if !uia_passes_filter(&notification_shared, element) {
                        return;
                    }
                    let node =
                        snapshot_from_cached_element(element, &notification_shared.uia_registry);
                    let notification = verbatim_model::Notification {
                        kind: notification_kind_from_uia(kind),
                        processing: notification_processing_from_uia(processing),
                        display_string,
                        activity_id,
                    };
                    notification_shared.emit(
                        TraceId::mint(),
                        Backend::Uia,
                        NormalizedEvent::Notification {
                            node_id: node.id,
                            notification,
                        },
                    );
                }
            },
        );
        let windows = top_level_windows(target_pid);
        match NotificationRegistration::new(windows, notification_callback) {
            Ok(registration) => self.notification_registration = Some(registration),
            Err(error) => self
                .shared
                .fault(format!("UIA notification registration failed: {error}")),
        }
    }

    /// Announces a foreground change to this outpost's target application
    /// (architecture section 1, decision D9): a synthetic `FocusChanged` for
    /// the top-level foreground window (single deadline-guarded attempt,
    /// suppressed if it is Core's hidden main frame), then the synthetic
    /// focus for the focused control, retried up to
    /// [`ANNOUNCE_RETRY_ATTEMPTS`] times across roughly two seconds when
    /// nothing is found yet. Runs off the command loop (its own thread) so
    /// `Ping` and `Fetch` stay responsive while retries are in flight; a
    /// generation counter bumped on every call means a superseding announce
    /// aborts any retry loop still running from a previous one.
    fn handle_announce_focus(&self, _trace_id: TraceId) {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = self.shared.clone();
        let target_pid = self.target_pid;
        let _ = thread::Builder::new()
            .name("verbatim-announce".to_owned())
            .spawn(move || run_announce(&shared, target_pid, generation));
    }

    /// Answers a fetch by re-reading the node from whichever backend owns it.
    fn handle_fetch(&self, trace: TraceId, query: verbatim_model::Query) {
        use verbatim_model::FetchResult;
        let shared = self.shared.clone();
        let node_id = query.node_id;
        let query_id = query.query_id;
        // A re-read is fire-and-forget on a worker; the object-navigation
        // kinds walk one step and can block on a hung provider, so they run
        // deadline-guarded like the other navigation queries, abandoning
        // rather than wedging the outpost. Both reply on the same
        // query-id-correlated FetchReply path so the reducer's completion
        // handling is identical for either.
        match navigate_direction_of(query.kind) {
            None => {
                self.shared.pool.submit(move |worker| {
                    let result = refetch_node(worker, &shared, node_id)
                        .map_or(FetchResult::Gone, FetchResult::Node);
                    let _ = shared.outbound.send(OutpostToSupervisor::FetchReply {
                        trace_id: trace,
                        query_id,
                        result,
                    });
                });
            }
            Some(direction) => {
                let outbound = self.shared.outbound.clone();
                let result = self
                    .shared
                    .pool
                    .run(NAVIGATE_DEADLINE, move |worker| {
                        navigate_query(worker, &shared, node_id, direction)
                    })
                    // A timed-out or failed navigation leaves the navigator
                    // put: report no neighbor rather than a spurious move.
                    .map_or(FetchResult::NoNeighbor, |outcome| match outcome {
                        Ok(NavigateOutcome::Found(node)) => FetchResult::Node(node),
                        Ok(NavigateOutcome::NoNeighbor) | Err(_) => FetchResult::NoNeighbor,
                    });
                let _ = outbound.send(OutpostToSupervisor::FetchReply {
                    trace_id: trace,
                    query_id,
                    result,
                });
            }
        }
    }

    /// Answers a `DumpTree` request by walking the target application's
    /// tree from its top-level window, on a deadline-guarded query-pool
    /// thread — the same pattern [`Self::handle_announce_focus`] uses — so a
    /// hung application abandons the call rather than wedging the outpost.
    fn handle_dump_tree(&self, trace: TraceId) {
        let target_pid = self.target_pid;
        let shared = self.shared.clone();
        let result = self
            .shared
            .pool
            .run(DUMP_TREE_DEADLINE, move |worker| {
                dump_tree(worker, target_pid, &shared)
            })
            .unwrap_or_else(|| Err("tree walk timed out".to_owned()));
        let _ = self
            .shared
            .outbound
            .send(OutpostToSupervisor::DumpTreeReply {
                trace_id: trace,
                result,
            });
    }

    /// Answers an `AncestorChain` request by walking the node's ancestors,
    /// on a deadline-guarded query-pool thread — the same pattern
    /// [`Self::handle_dump_tree`] uses.
    fn handle_ancestor_chain(&self, trace: TraceId, node_id: verbatim_model::NodeId) {
        let shared = self.shared.clone();
        let result = self
            .shared
            .pool
            .run(ANCESTOR_CHAIN_DEADLINE, move |worker| {
                ancestor_chain_query(worker, &shared, node_id)
            })
            .unwrap_or_else(|| Err("ancestor-chain walk timed out".to_owned()));
        let _ = self
            .shared
            .outbound
            .send(OutpostToSupervisor::AncestorChainReply {
                trace_id: trace,
                result,
            });
    }

    /// Answers a `Navigate` request, on a deadline-guarded query-pool
    /// thread — the same pattern [`Self::handle_dump_tree`] uses.
    fn handle_navigate(
        &self,
        trace: TraceId,
        node_id: verbatim_model::NodeId,
        direction: crate::protocol::NavigateDirection,
    ) {
        let shared = self.shared.clone();
        let result = self
            .shared
            .pool
            .run(QUERY_DEADLINE, move |worker| {
                navigate_query(worker, &shared, node_id, direction)
            })
            .unwrap_or_else(|| Err("navigation timed out".to_owned()));
        let _ = self
            .shared
            .outbound
            .send(OutpostToSupervisor::NavigateReply {
                trace_id: trace,
                result,
            });
    }

    /// Answers an `Activate` request, on a deadline-guarded query-pool
    /// thread — the same pattern [`Self::handle_dump_tree`] uses.
    fn handle_activate(&self, trace: TraceId, node_id: verbatim_model::NodeId) {
        let shared = self.shared.clone();
        let result = self
            .shared
            .pool
            .run(QUERY_DEADLINE, move |worker| {
                activate_query(worker, &shared, node_id)
            })
            .unwrap_or_else(|| Err("activation timed out".to_owned()));
        let _ = self
            .shared
            .outbound
            .send(OutpostToSupervisor::ActivateReply {
                trace_id: trace,
                result,
            });
    }

    /// Dispatches one supervisor command. Returns `false` on `Shutdown`.
    pub fn handle_command(&mut self, command: &SupervisorToOutpost) -> bool {
        match command {
            SupervisorToOutpost::SetBackendOverride { backend_override } => {
                self.handle_set_backend_override(*backend_override);
                true
            }
            SupervisorToOutpost::AnnounceFocus { trace_id } => {
                self.handle_announce_focus(*trace_id);
                true
            }
            SupervisorToOutpost::Fetch { trace_id, query } => {
                self.handle_fetch(*trace_id, *query);
                true
            }
            SupervisorToOutpost::Ping { seq } => {
                let _ = self
                    .shared
                    .outbound
                    .send(OutpostToSupervisor::Pong { seq: *seq });
                true
            }
            SupervisorToOutpost::DumpTree { trace_id } => {
                self.handle_dump_tree(*trace_id);
                true
            }
            SupervisorToOutpost::AncestorChain { trace_id, node_id } => {
                self.handle_ancestor_chain(*trace_id, *node_id);
                true
            }
            SupervisorToOutpost::Navigate {
                trace_id,
                node_id,
                direction,
            } => {
                self.handle_navigate(*trace_id, *node_id, *direction);
                true
            }
            SupervisorToOutpost::Activate { trace_id, node_id } => {
                self.handle_activate(*trace_id, *node_id);
                true
            }
            SupervisorToOutpost::Shutdown => false,
        }
    }
}

/// The retry driver behind [`Outpost::handle_announce_focus`], run on its own
/// thread. `generation` is the value captured when this call was scheduled;
/// every attempt and every emission is guarded by comparing it against
/// `shared.generation`'s live value, so a superseding `AnnounceFocus` aborts
/// this loop without any explicit cancellation channel.
///
/// Both the window announcement and the focused-control announcement share
/// one retry budget — up to [`ANNOUNCE_RETRY_ATTEMPTS`] attempts across
/// roughly two seconds — rather than the window getting a single attempt.
/// It initially seemed safe to assume the top-level window always exists by
/// the time Core observes a foreground change at all (Windows raises the
/// foreground event only once the window does), and that held for the
/// common case, but live testing against the VM under load found it does
/// not always hold: `EnumWindows` can still race a window's own creation
/// closely enough that neither `GetForegroundWindow` nor enumeration finds
/// it on the very first attempt, losing the window announcement outright
/// with no later chance to recover it. The window step now participates in
/// the same retry loop as the control step, stopping once it succeeds (or
/// is deliberately skipped, for a hidden-frame-only process).
///
/// The two steps are independent, not sequential: a first version of this
/// loop returned as soon as the control was found, which on Windows 11
/// Notepad — whose edit control focuses essentially instantly, per this
/// crate's supervisor docs — routinely happened on the very first attempt,
/// before the window step had a chance to retry a first attempt that missed
/// (confirmed live: the control announcement arrived every time, but the
/// window announcement was silently lost whenever its own first attempt
/// raced the window's creation). The loop now keeps going, attempt by
/// attempt, until *both* steps have succeeded or the attempts run out,
/// tracking each step's own completion so a step that already succeeded is
/// never attempted again.
fn run_announce(shared: &Shared, target_pid: u32, generation: u64) {
    let still_current = || shared.generation.load(Ordering::SeqCst) == generation;
    let mut window_done = false;
    let mut control_done = false;

    for attempt in 0..ANNOUNCE_RETRY_ATTEMPTS {
        if !still_current() {
            return;
        }

        if !window_done && let Some(hwnd) = active_top_level_window(target_pid) {
            let shared_for_window = shared.clone();
            let window = shared.pool.run(FOCUS_DEADLINE, move |worker| {
                window_snapshot(worker, hwnd, &shared_for_window)
            });
            if let Some(Some((backend, node))) = window {
                window_done = true;
                if still_current() {
                    shared.emit(
                        TraceId::mint(),
                        backend,
                        NormalizedEvent::FocusChanged {
                            node,
                            ancestors: Vec::new(),
                            selected_child: None,
                        },
                    );
                }
            }
        }
        // A window search that found nothing this attempt (every top-level
        // window is Core's own hidden frame, or the process has none yet)
        // is not a terminal failure; `window_done` stays false and the next
        // attempt tries again alongside the control step.

        if !control_done && still_current() {
            let shared_for_focus = shared.clone();
            let found = shared.pool.run(FOCUS_DEADLINE, move |worker| {
                focused_snapshot(worker, target_pid, &shared_for_focus)
            });
            if let Some(Some((backend, node))) = found {
                control_done = true;
                if still_current() {
                    // Enrich with the control's ancestry, deadline-guarded
                    // like every other step of this announcement; a timed
                    // out or failed walk degrades to no context.
                    let shared_for_chain = shared.clone();
                    let node_for_chain = node.clone();
                    let (ancestors, selected_child) = shared
                        .pool
                        .run(FOCUS_DEADLINE, move |worker| {
                            focus_enrichment_query(worker, &shared_for_chain, &node_for_chain)
                        })
                        .unwrap_or_default();
                    shared.emit(
                        TraceId::mint(),
                        backend,
                        NormalizedEvent::FocusChanged {
                            node,
                            ancestors,
                            selected_child,
                        },
                    );
                }
            }
        }

        if window_done && control_done {
            return;
        }
        if attempt + 1 < ANNOUNCE_RETRY_ATTEMPTS {
            thread::sleep(ANNOUNCE_RETRY_INTERVAL);
        }
    }
}

/// Runs on a query-pool thread: finds the target's top-level window (its
/// currently active window, falling back to the first top-level window
/// found), arbitrates its backend, and walks its tree, bounded by
/// [`MAX_DUMP_DEPTH`] and [`MAX_DUMP_NODES`].
fn dump_tree(worker: &mut Worker, target_pid: u32, shared: &Shared) -> Result<DumpedTree, String> {
    let hwnd = active_top_level_window(target_pid)
        .ok_or_else(|| "the target application has no top-level window".to_owned())?;
    let class = window_class_name(hwnd);
    let is_uia = decide_backend(shared, hwnd, &class);
    if is_uia {
        let uia = worker
            .uia()
            .ok_or_else(|| "could not create a UIA client".to_owned())?;
        let cache = uia
            .base_cache_request()
            .map_err(|error| format!("could not build a UIA cache request: {error}"))?;
        let element = uia
            .element_from_handle(hwnd, &cache)
            .map_err(|error| format!("could not fetch the top-level UIA element: {error}"))?;
        // SAFETY: `element` was built with `cache` immediately above.
        let (root, truncated) = unsafe {
            uia.walk_tree(
                &element,
                &cache,
                &shared.uia_registry,
                MAX_DUMP_DEPTH,
                MAX_DUMP_NODES,
            )
        }
        .map_err(|error| format!("UIA tree walk failed: {error}"))?;
        Ok(DumpedTree { root, truncated })
    } else {
        let (root, truncated) = verbatim_ia2::acquire::walk_tree(
            hwnd,
            &shared.msaa_registry,
            MAX_DUMP_DEPTH,
            MAX_DUMP_NODES,
        )
        .ok_or_else(|| "could not acquire the top-level MSAA object".to_owned())?;
        Ok(DumpedTree { root, truncated })
    }
}

/// Re-reads a node by id from whichever registry knows it.
fn refetch_node(
    worker: &mut Worker,
    shared: &Shared,
    node_id: verbatim_model::NodeId,
) -> Option<NodeSnapshot> {
    if let Some(runtime_id) = shared.uia_registry.runtime_id_of(node_id) {
        let uia = worker.uia()?;
        let cache = uia.base_cache_request().ok()?;
        let element = uia.element_by_runtime_id(&runtime_id, &cache).ok()??;
        // SAFETY: `element` was built with the base cache request.
        return Some(unsafe { snapshot_from_cached_element(&element, &shared.uia_registry) });
    }
    if let Some(key) = shared.msaa_registry.key_of(node_id) {
        return verbatim_ia2::acquire::resnapshot(key, &shared.msaa_registry);
    }
    None
}

/// Whether a focused node's role is a selection container whose selected
/// child is announced with it (roadmap M3: a list's selected item, a tab
/// control's active tab).
fn wants_selected_child(role: verbatim_model::Role) -> bool {
    matches!(
        role,
        verbatim_model::Role::List | verbatim_model::Role::TabControl
    )
}

/// Gathers a newly focused node's event enrichment on a query worker: its
/// ancestor chain (outermost first) and, for selection containers, its
/// selected child — fetching the backend element once and asking it both
/// questions. Every failure degrades to an empty chain or `None`: focus
/// enrichment must never turn a focus announcement into an error.
fn focus_enrichment_query(
    worker: &mut Worker,
    shared: &Shared,
    node: &NodeSnapshot,
) -> (Vec<NodeSnapshot>, Option<NodeSnapshot>) {
    let want_selection = wants_selected_child(node.role);
    if let Some(runtime_id) = shared.uia_registry.runtime_id_of(node.id) {
        let Some(uia) = worker.uia() else {
            return (Vec::new(), None);
        };
        let Ok(cache) = uia.base_cache_request() else {
            return (Vec::new(), None);
        };
        let Ok(Some(element)) = uia.element_by_runtime_id(&runtime_id, &cache) else {
            return (Vec::new(), None);
        };
        // SAFETY: `element` was built with `cache` immediately above.
        let ancestors = unsafe {
            uia.ancestor_chain(&element, &cache, &shared.uia_registry, MAX_ANCESTOR_HOPS)
        }
        .unwrap_or_default();
        let selected = if want_selection {
            // SAFETY: `element` was built with `cache` immediately above.
            unsafe { uia.selected_child(&element, &cache, &shared.uia_registry) }.unwrap_or(None)
        } else {
            None
        };
        return (ancestors, selected);
    }
    if let Some(key) = shared.msaa_registry.key_of(node.id) {
        let ancestors =
            verbatim_ia2::acquire::ancestor_chain(key, &shared.msaa_registry, MAX_ANCESTOR_HOPS);
        let selected = if want_selection {
            verbatim_ia2::acquire::selected_child(key, &shared.msaa_registry)
        } else {
            None
        };
        return (ancestors, selected);
    }
    (Vec::new(), None)
}

/// Walks the ancestor chain of a node by id, on a query worker: UIA via
/// [`verbatim_uia::Uia::ancestor_chain`], MSAA via
/// [`verbatim_ia2::acquire::ancestor_chain`] — whichever registry knows the
/// id, the same dispatch [`refetch_node`] uses.
fn ancestor_chain_query(
    worker: &mut Worker,
    shared: &Shared,
    node_id: verbatim_model::NodeId,
) -> Result<Vec<NodeSnapshot>, String> {
    if let Some(runtime_id) = shared.uia_registry.runtime_id_of(node_id) {
        let uia = worker
            .uia()
            .ok_or_else(|| "could not create a UIA client".to_owned())?;
        let cache = uia
            .base_cache_request()
            .map_err(|error| format!("could not build a UIA cache request: {error}"))?;
        let element = uia
            .element_by_runtime_id(&runtime_id, &cache)
            .map_err(|error| format!("could not re-fetch the node: {error}"))?
            .ok_or_else(|| "the node no longer exists".to_owned())?;
        // SAFETY: `element` was built with `cache` immediately above.
        let chain = unsafe {
            uia.ancestor_chain(&element, &cache, &shared.uia_registry, MAX_ANCESTOR_HOPS)
        }
        .map_err(|error| format!("UIA ancestor walk failed: {error}"))?;
        return Ok(chain);
    }
    if let Some(key) = shared.msaa_registry.key_of(node_id) {
        return Ok(verbatim_ia2::acquire::ancestor_chain(
            key,
            &shared.msaa_registry,
            MAX_ANCESTOR_HOPS,
        ));
    }
    Err("unknown node id".to_owned())
}

/// Navigates from a node by id in `direction`, on a query worker: UIA via
/// [`verbatim_uia::Uia::navigate`], MSAA via [`verbatim_ia2::acquire::navigate`].
/// "No such neighbor" is reported as `Ok(NavigateOutcome::NoNeighbor)`, never
/// conflated with the `Err` an acquisition failure produces.
fn navigate_query(
    worker: &mut Worker,
    shared: &Shared,
    node_id: verbatim_model::NodeId,
    direction: crate::protocol::NavigateDirection,
) -> Result<NavigateOutcome, String> {
    if let Some(runtime_id) = shared.uia_registry.runtime_id_of(node_id) {
        let uia = worker
            .uia()
            .ok_or_else(|| "could not create a UIA client".to_owned())?;
        let cache = uia
            .base_cache_request()
            .map_err(|error| format!("could not build a UIA cache request: {error}"))?;
        let element = uia
            .element_by_runtime_id(&runtime_id, &cache)
            .map_err(|error| format!("could not re-fetch the node: {error}"))?
            .ok_or_else(|| "the node no longer exists".to_owned())?;
        // SAFETY: `element` was built with `cache` immediately above.
        let found = unsafe {
            uia.navigate(
                &element,
                &cache,
                &shared.uia_registry,
                uia_navigate_direction(direction),
            )
        }
        .map_err(|error| format!("UIA navigation failed: {error}"))?;
        return Ok(found.map_or(NavigateOutcome::NoNeighbor, NavigateOutcome::Found));
    }
    if let Some(key) = shared.msaa_registry.key_of(node_id) {
        let found = verbatim_ia2::acquire::navigate(
            key,
            &shared.msaa_registry,
            msaa_navigate_direction(direction),
        );
        return Ok(found.map_or(NavigateOutcome::NoNeighbor, NavigateOutcome::Found));
    }
    Err("unknown node id".to_owned())
}

/// Maps the outpost protocol's [`crate::protocol::NavigateDirection`] to
/// `verbatim-uia`'s own copy of the same four variants (the crates do not
/// depend on each other, so each carries its own).
fn uia_navigate_direction(
    direction: crate::protocol::NavigateDirection,
) -> verbatim_uia::NavigateDirection {
    match direction {
        crate::protocol::NavigateDirection::Parent => verbatim_uia::NavigateDirection::Parent,
        crate::protocol::NavigateDirection::NextSibling => {
            verbatim_uia::NavigateDirection::NextSibling
        }
        crate::protocol::NavigateDirection::PreviousSibling => {
            verbatim_uia::NavigateDirection::PreviousSibling
        }
        crate::protocol::NavigateDirection::FirstChild => {
            verbatim_uia::NavigateDirection::FirstChild
        }
    }
}

/// Maps the outpost protocol's [`crate::protocol::NavigateDirection`] to
/// `verbatim-ia2`'s own copy of the same four variants.
fn msaa_navigate_direction(
    direction: crate::protocol::NavigateDirection,
) -> verbatim_ia2::acquire::NavigateDirection {
    match direction {
        crate::protocol::NavigateDirection::Parent => {
            verbatim_ia2::acquire::NavigateDirection::Parent
        }
        crate::protocol::NavigateDirection::NextSibling => {
            verbatim_ia2::acquire::NavigateDirection::NextSibling
        }
        crate::protocol::NavigateDirection::PreviousSibling => {
            verbatim_ia2::acquire::NavigateDirection::PreviousSibling
        }
        crate::protocol::NavigateDirection::FirstChild => {
            verbatim_ia2::acquire::NavigateDirection::FirstChild
        }
    }
}

/// Activates a node by id, on a query worker: UIA via
/// [`verbatim_uia::Uia::activate`], MSAA via [`verbatim_ia2::acquire::activate`].
fn activate_query(
    worker: &mut Worker,
    shared: &Shared,
    node_id: verbatim_model::NodeId,
) -> Result<(), String> {
    if let Some(runtime_id) = shared.uia_registry.runtime_id_of(node_id) {
        let uia = worker
            .uia()
            .ok_or_else(|| "could not create a UIA client".to_owned())?;
        let cache = uia
            .base_cache_request()
            .map_err(|error| format!("could not build a UIA cache request: {error}"))?;
        let element = uia
            .element_by_runtime_id(&runtime_id, &cache)
            .map_err(|error| format!("could not re-fetch the node: {error}"))?
            .ok_or_else(|| "the node no longer exists".to_owned())?;
        // SAFETY: `element` was built with `cache` immediately above.
        return unsafe { uia.activate(&element) }
            .map_err(|error| format!("UIA activation failed: {error}"));
    }
    if let Some(key) = shared.msaa_registry.key_of(node_id) {
        return verbatim_ia2::acquire::activate(key);
    }
    Err("unknown node id".to_owned())
}

/// Reads a single node's own snapshot for `hwnd` (no children), arbitrating
/// its backend first. Used for the top-level-window announcement, where only
/// the window's own name/role/state matters. Runs on a query worker
/// (blocking allowed).
fn window_snapshot(
    worker: &mut Worker,
    hwnd: isize,
    shared: &Shared,
) -> Option<(Backend, NodeSnapshot)> {
    let class = window_class_name(hwnd);
    let is_uia = decide_backend(shared, hwnd, &class);
    if is_uia {
        let uia = worker.uia()?;
        let cache = uia.base_cache_request().ok()?;
        let element = uia.element_from_handle(hwnd, &cache).ok()?;
        // SAFETY: `element` was built with the base cache request.
        let node = unsafe { snapshot_from_cached_element(&element, &shared.uia_registry) };
        Some((Backend::Uia, node))
    } else {
        let node = msaa_window_snapshot(hwnd, &shared.msaa_registry)?;
        Some((Backend::Msaa, node))
    }
}

/// Reads the MSAA accessible object for the window itself — `OBJID_WINDOW`,
/// not the client area [`verbatim_ia2::acquire::walk_tree`]'s root starts
/// from (`OBJID_CLIENT`) — for the top-level-window announcement
/// specifically. `OBJID_CLIENT`'s role reads as "client" (unmapped, so
/// "unknown" once spoken), not "window"; confirmed live against Windows 11
/// Notepad, whose top-level-window announcement read "Untitled - Notepad,
/// unknown" until this queried `OBJID_WINDOW` instead. A small amount of
/// direct MSAA access duplicated here rather than added to
/// `verbatim-ia2::acquire` (out of scope for this change) — `verbatim-ia2`
/// already exposes everything else this needs: `map::role_from_msaa`,
/// `map::states_from_msaa`, and the shared `NodeIdRegistry`.
fn msaa_window_snapshot(hwnd: isize, registry: &MsaaRegistry) -> Option<NodeSnapshot> {
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Variant::{
        VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4, VariantToInt32,
    };
    use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
    use windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW;
    use windows::core::Interface;

    // SAFETY: AccessibleObjectFromWindow tolerates an invalid handle by
    // failing; `acc` is read only once the call has succeeded.
    let acc: IAccessible = unsafe {
        let mut acc: Option<IAccessible> = None;
        AccessibleObjectFromWindow(
            HWND(hwnd as *mut c_void),
            OBJID_WINDOW.0.cast_unsigned(),
            &IAccessible::IID,
            (&raw mut acc).cast::<*mut c_void>(),
        )
        .ok()?;
        acc?
    };
    // CHILDID_SELF as a VT_I4 VARIANT, addressing the window object itself
    // rather than one of its children.
    let child = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: CHILDID_SELF },
            }),
        },
    };
    // SAFETY: `acc` is a live IAccessible just acquired above; `child` is a
    // valid VT_I4 VARIANT for it; each accessor tolerates an unsupported
    // property by returning an error, mapped to a neutral default.
    let (name, value, role, states) = unsafe {
        let name = acc
            .get_accName(&child)
            .ok()
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty());
        let value = acc
            .get_accValue(&child)
            .ok()
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty());
        let role = acc
            .get_accRole(&child)
            .ok()
            .and_then(|v| VariantToInt32(&raw const v).ok())
            .map_or(verbatim_model::Role::Unknown, |r| {
                verbatim_ia2::map::role_from_msaa(r.cast_unsigned())
            });
        let states = acc
            .get_accState(&child)
            .ok()
            .and_then(|v| VariantToInt32(&raw const v).ok())
            .map(|s| verbatim_ia2::map::states_from_msaa(s.cast_unsigned()))
            .unwrap_or_default();
        (name, value, role, states)
    };
    Some(NodeSnapshot {
        id: registry.id_for((hwnd, OBJID_WINDOW.0, CHILDID_SELF)),
        backend: Backend::Msaa,
        role,
        name,
        value,
        states,
        details: NodeDetails::default(),
    })
}

/// Reads and arbitrates the currently focused element of `target_pid`, using
/// UIA or MSAA per the verdict. Treats Core's hidden main frame as "nothing
/// focused" (decision D9), so a caller retrying on `None` naturally retries
/// past it. Runs on a query worker (blocking allowed).
fn focused_snapshot(
    worker: &mut Worker,
    target_pid: u32,
    shared: &Shared,
) -> Option<(Backend, NodeSnapshot)> {
    let hwnd = focused_window(target_pid)?;
    if window_is_hidden_frame(hwnd) {
        return None;
    }
    let class = window_class_name(hwnd);
    let is_uia = decide_backend(shared, hwnd, &class);
    if is_uia {
        let uia = worker.uia()?;
        let cache = uia.base_cache_request().ok()?;
        let element = uia.focused_element(&cache).ok()?;
        // SAFETY: `element` was built with the base cache request.
        let node = unsafe { snapshot_from_cached_element(&element, &shared.uia_registry) };
        Some((Backend::Uia, node))
    } else {
        let node = verbatim_ia2::acquire::focused_snapshot(target_pid, &shared.msaa_registry)?;
        Some((Backend::Msaa, node))
    }
}

/// Decides a window's backend on a query worker, probing inline when the class
/// lists and cache do not decide it (the outer query deadline guards the probe).
fn decide_backend(shared: &Shared, hwnd: isize, class: &str) -> bool {
    if let Some(verdict) = shared.lock_arbitrator().verdict(hwnd, class) {
        return verdict;
    }
    let is_uia = has_server_side_provider(hwnd);
    shared.lock_arbitrator().record_probe(hwnd, is_uia);
    is_uia
}

/// Finds the target's currently foreground top-level window, falling back to
/// its first top-level window — skipping Core's hidden main frame in both
/// cases (decision D9), so resolution naturally lands on a real window (a
/// popup menu, a dialog) when the frame happens to be transiently active or
/// first in enumeration order.
///
/// Deliberately `GetForegroundWindow`, not [`focused_window`]'s
/// `GetGUIThreadInfo`-based resolution: `GetForegroundWindow` is guaranteed
/// to name a genuine top-level window, while `hwndFocus` can legitimately
/// name a non-top-level descendant that still belongs to the target
/// process — observed live against Windows 11's modern Notepad, whose text
/// area is hosted in its own child `hwnd` distinct from the frame, which
/// made the window-level announcement below read the edit control's own
/// snapshot ("Text Area", role edit) instead of the window's ("Notepad",
/// role window). [`focused_snapshot`] below still wants `hwndFocus`
/// specifically — it is answering "what control is focused", not "what is
/// the top-level window", and those are genuinely different questions for
/// an app like this one.
fn active_top_level_window(target_pid: u32) -> Option<isize> {
    genuine_foreground_window(target_pid)
        .filter(|&hwnd| !window_is_hidden_frame(hwnd))
        .or_else(|| {
            top_level_windows(target_pid)
                .into_iter()
                .find(|&hwnd| !window_is_hidden_frame(hwnd))
        })
}

/// Returns `GetForegroundWindow()` if it belongs to `target_pid`, else
/// `None`. Always a genuine top-level window when it returns `Some` — see
/// [`active_top_level_window`]'s doc comment for why that guarantee matters.
fn genuine_foreground_window(target_pid: u32) -> Option<isize> {
    // SAFETY: GetForegroundWindow and GetWindowThreadProcessId both fail
    // safely (a null or stale handle) rather than blocking or crashing.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        (pid == target_pid).then_some(hwnd.0 as isize)
    }
}

/// Returns the focused window of `target_pid`, or `None` if the foreground
/// focus is not in that process.
fn focused_window(target_pid: u32) -> Option<isize> {
    let hwnd = foreground_focus_window()?;
    // SAFETY: `hwnd` came from `GetGUIThreadInfo`; the call fails safely on a
    // window that has since been destroyed.
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(HWND(hwnd as *mut c_void), Some(&raw mut pid));
    }
    (pid == target_pid).then_some(hwnd)
}

/// The window with the keyboard focus right now (falling back to the active
/// window), whatever process it belongs to.
///
/// A pure Win32 query: no COM, no cross-process call, and it cannot block on a
/// hung application, which is what makes it safe to call from an event-callback
/// thread — see [`resolve_window_and_filter`], its reason for existing.
fn foreground_focus_window() -> Option<isize> {
    // SAFETY: `info` has cbSize set before the call; the call fails safely.
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        GetGUIThreadInfo(0, &raw mut info).ok()?;
        let hwnd = if info.hwndFocus.0.is_null() {
            info.hwndActive
        } else {
            info.hwndFocus
        };
        (!hwnd.0.is_null()).then_some(hwnd.0 as isize)
    }
}

struct EnumContext {
    target_pid: u32,
    hwnds: Vec<isize>,
}

/// Returns the top-level windows belonging to `target_pid`.
fn top_level_windows(target_pid: u32) -> Vec<isize> {
    let mut context = EnumContext {
        target_pid,
        hwnds: Vec::new(),
    };
    // SAFETY: `enum_proc` reads the context pointer we pass and nothing else;
    // the context outlives the synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM((&raw mut context) as isize));
    }
    context.hwnds
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the EnumContext pointer passed by top_level_windows,
    // valid for the duration of the enumeration.
    unsafe {
        let context = &mut *(lparam.0 as *mut EnumContext);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        if pid == context.target_pid {
            context.hwnds.push(hwnd.0 as isize);
        }
    }
    BOOL(1)
}

/// Runs an outpost driven by the Core pipes, watching `target_pid` for its
/// whole life: reads commands from `pipe_in`, writes outbound messages to
/// `pipe_out`, until `Shutdown` or end of stream.
///
/// # Errors
///
/// Returns any I/O error reading the command stream.
pub fn run_pipe(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
    target_pid: u32,
) -> io::Result<()> {
    let mut outpost = Outpost::new(pipe_out, target_pid);
    let mut reader = BufReader::new(pipe_in);
    while let Some(command) = read_message::<_, SupervisorToOutpost>(&mut reader)? {
        if !outpost.handle_command(&command) {
            break;
        }
    }
    Ok(())
}

/// Runs an outpost in dev-attach mode: writes outbound messages as JSON lines
/// to stdout, watches the given target for its whole life, and immediately
/// announces its focus, streaming events until the process is killed. Does
/// not return under normal operation.
///
/// # Errors
///
/// Returns an I/O error only if dev-mode setup fails before the event loop
/// begins; once running it blocks until the process is terminated.
pub fn run_attach(target_pid: u32) -> io::Result<()> {
    let outpost = Outpost::new(Box::new(io::stdout()), target_pid);
    outpost.handle_announce_focus(TraceId::mint());
    loop {
        thread::park();
    }
}
