//! The outpost runtime: the event thread, the UIA registrations, the query
//! pool, and the command loop that ties them together (architecture section 1).
//!
//! One outpost watches one application. Its UIA focus and property handlers and
//! its out-of-context MSAA `WinEvent` hooks run simultaneously; the arbitration
//! cross-filter (see [`crate::arbitration`]) ensures only one backend announces
//! any given change. Trace IDs are minted the moment an OS event is observed;
//! the snapshot version increments on every emitted event.

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
    IUIAutomationElement, UIA_NamePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GUITHREADINFO, GetGUIThreadInfo, GetMessageW,
    GetWindowThreadProcessId, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
};
use windows::core::BOOL;

use verbatim_ia2::{NodeIdRegistry as MsaaRegistry, WinEventCallback, WinEventHook, WinEventKind};
use verbatim_model::{
    Backend, NodeSnapshot, NormalizedEvent, Pid, PropertyChange, SnapshotVersion, TraceId,
};
use verbatim_uia::map::{cached_native_window_handle, snapshot_from_cached_element};
use verbatim_uia::{
    FocusRegistration, NodeIdRegistry as UiaRegistry, PropertyRegistration,
    has_server_side_provider, nearest_window_handle,
};

use crate::arbitration::{Arbitrator, window_class_name};
use crate::protocol::{
    DumpedTree, OutpostToSupervisor, SupervisorToOutpost, read_message, write_message,
};
use crate::query_pool::{QueryPool, Worker};

/// The deadline for a single deadline-guarded query-pool call (fetch, probe,
/// synthetic focus): architecture section 1's per-call deadline.
const QUERY_DEADLINE: Duration = Duration::from_millis(300);

/// A slightly longer deadline for the synthetic focus query, which chains an
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

/// Custom thread message: re-read the desired target pid and rebind hooks.
const WM_REBIND: u32 = WM_APP + 1;

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

/// The MSAA `WinEvent` callback body: cross-filter on the event thread, then
/// hand acquisition to the query pool. Never blocks.
fn handle_msaa_event(
    shared: &Shared,
    kind: WinEventKind,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
) {
    let class = window_class_name(hwnd);
    match shared.lock_arbitrator().verdict(hwnd, &class) {
        Some(true) => return, // UIA window: MSAA is suppressed for it.
        Some(false) => {}
        None => shared.schedule_probe(hwnd, class), // provisional non-UIA; warm cache.
    }
    let trace = TraceId::mint();
    let shared = shared.clone();
    let pool = shared.pool.clone();
    pool.submit(move |_worker| {
        if let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &shared.msaa_registry,
        ) {
            shared.emit(trace, Backend::Msaa, msaa_event(kind, &node));
        }
    });
}

/// Maps an MSAA event kind and its acquired snapshot to a normalized event.
fn msaa_event(kind: WinEventKind, node: &NodeSnapshot) -> NormalizedEvent {
    match kind {
        WinEventKind::Focus => NormalizedEvent::FocusChanged { node: node.clone() },
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
    }
}

/// Applies the UIA cross-filter for an element on a callback thread. Returns
/// whether the event should be delivered.
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
unsafe fn uia_passes_filter(shared: &Shared, element: &IUIAutomationElement) -> bool {
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
        return true;
    };
    let class = window_class_name(hwnd);
    match shared.lock_arbitrator().verdict(hwnd, &class) {
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
    }
}

/// The event thread: installs the MSAA hooks and pumps messages so the
/// out-of-context callbacks are delivered, rebinding to a new pid on request.
struct EventThread {
    thread_id: u32,
    desired_pid: Arc<Mutex<Option<u32>>>,
    join: Option<JoinHandle<()>>,
}

impl EventThread {
    fn spawn(make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync>) -> Self {
        let desired_pid = Arc::new(Mutex::new(None));
        let (id_tx, id_rx) = unbounded::<u32>();
        let thread_desired = desired_pid.clone();
        let join = thread::Builder::new()
            .name("verbatim-event".to_owned())
            .spawn(move || event_thread_main(&id_tx, &thread_desired, &make_callback))
            .expect("spawn event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            thread_id,
            desired_pid,
            join: Some(join),
        }
    }

    /// Rebinds the hooks to `pid`. The message loop reinstalls on the next
    /// [`WM_REBIND`].
    fn rebind(&self, pid: u32) {
        *self
            .desired_pid
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(pid);
        // SAFETY: posting a thread message to our own event thread.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_REBIND, WPARAM(0), LPARAM(0));
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
    id_tx: &Sender<u32>,
    desired_pid: &Arc<Mutex<Option<u32>>>,
    make_callback: &Arc<dyn Fn() -> WinEventCallback + Send + Sync>,
) {
    // SAFETY: GetCurrentThreadId is always sound.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = id_tx.send(thread_id);
    let mut hook: Option<WinEventHook> = None;
    let mut message = MSG::default();
    loop {
        // SAFETY: standard message loop; `message` is fully owned here.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break; // WM_QUIT (0) or error (-1).
        }
        if message.message == WM_REBIND {
            let pid = *desired_pid.lock().unwrap_or_else(PoisonError::into_inner);
            hook = None; // Unhook the previous target before rebinding.
            if let Some(pid) = pid {
                match WinEventHook::install(pid, make_callback()) {
                    Ok(installed) => hook = Some(installed),
                    Err(error) => tracing::warn!(error, "failed to install WinEvent hooks"),
                }
            }
        }
        // SAFETY: dispatching a fully owned message.
        unsafe {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    drop(hook);
}

/// The outpost: owns the shared state, event thread, and UIA registrations for
/// one target application.
pub struct Outpost {
    pid: u32,
    shared: Shared,
    event_thread: EventThread,
    focus_registration: Option<FocusRegistration>,
    property_registration: Option<PropertyRegistration>,
    target_pid: Option<u32>,
    _writer: JoinHandle<()>,
}

impl Outpost {
    /// Creates an outpost that writes outbound messages through `writer` (the
    /// pipe to Core, or stdout in dev-attach mode). Spawns the writer thread,
    /// the query pool, and the event thread.
    ///
    /// # Panics
    ///
    /// Panics if the outbound writer or event thread cannot be spawned, which
    /// indicates the process is out of OS thread resources.
    #[must_use]
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
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
        };

        let callback_shared = shared.clone();
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let shared = callback_shared.clone();
            Box::new(move |kind, hwnd, id_object, id_child| {
                handle_msaa_event(&shared, kind, hwnd, id_object, id_child);
            })
        });
        let event_thread = EventThread::spawn(make_callback);

        Self {
            pid: std::process::id(),
            shared,
            event_thread,
            focus_registration: None,
            property_registration: None,
            target_pid: None,
            _writer: writer_join,
        }
    }

    /// (Re)targets the outpost: rebinds MSAA hooks, rebuilds UIA registrations,
    /// announces readiness, and emits a synthetic focus event for the currently
    /// focused element so the focus change that triggered this is announced.
    pub fn configure(&mut self, target_pid: u32, backend_override: Option<Backend>) {
        self.target_pid = Some(target_pid);
        self.shared
            .lock_arbitrator()
            .set_forced(backend_override.map(|backend| backend == Backend::Uia));

        // Rebind MSAA hooks to the new target.
        self.event_thread.rebind(target_pid);

        // Rebuild UIA registrations unless MSAA is forced.
        self.focus_registration = None;
        self.property_registration = None;
        if backend_override != Some(Backend::Msaa) {
            self.install_uia_registrations(target_pid);
        }

        let _ = self.shared.outbound.send(OutpostToSupervisor::Ready {
            outpost_pid: Pid(self.pid),
            target_pid: Pid(target_pid),
        });

        self.emit_synthetic_focus(target_pid);
    }

    fn install_uia_registrations(&mut self, target_pid: u32) {
        let focus_shared = self.shared.clone();
        let focus_callback = Arc::new(move |element: &IUIAutomationElement| {
            // SAFETY: `element` is a cached focus element from the base cache
            // request, so the filter and mapping read only cached values.
            unsafe {
                if !uia_passes_filter(&focus_shared, element) {
                    return;
                }
                let node = snapshot_from_cached_element(element, &focus_shared.uia_registry);
                focus_shared.emit(
                    TraceId::mint(),
                    Backend::Uia,
                    NormalizedEvent::FocusChanged { node },
                );
            }
        });
        match FocusRegistration::new(target_pid, focus_callback) {
            Ok(registration) => self.focus_registration = Some(registration),
            Err(error) => self
                .shared
                .fault(format!("UIA focus registration failed: {error}")),
        }

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

    fn emit_synthetic_focus(&self, target_pid: u32) {
        let shared = self.shared.clone();
        let result = self.shared.pool.run(FOCUS_DEADLINE, move |worker| {
            focused_snapshot(worker, target_pid, &shared)
        });
        if let Some(Some((backend, node))) = result {
            self.shared.emit(
                TraceId::mint(),
                backend,
                NormalizedEvent::FocusChanged { node },
            );
        }
    }

    /// Answers a fetch by re-reading the node from whichever backend owns it.
    fn handle_fetch(&self, trace: TraceId, query: verbatim_model::Query) {
        use verbatim_model::FetchResult;
        // M1 supports only QueryKind::NodeSnapshot (re-read the node).
        let shared = self.shared.clone();
        let node_id = query.node_id;
        let query_id = query.query_id;
        self.shared.pool.submit(move |worker| {
            let result =
                refetch_node(worker, &shared, node_id).map_or(FetchResult::Gone, FetchResult::Node);
            let _ = shared.outbound.send(OutpostToSupervisor::FetchReply {
                trace_id: trace,
                query_id,
                result,
            });
        });
    }

    /// Answers a `DumpTree` request by walking the target application's
    /// tree from its top-level window, on a deadline-guarded query-pool
    /// thread — the same pattern [`Self::emit_synthetic_focus`] uses — so a
    /// hung application abandons the call rather than wedging the outpost.
    fn handle_dump_tree(&self, trace: TraceId) {
        let Some(target_pid) = self.target_pid else {
            let _ = self
                .shared
                .outbound
                .send(OutpostToSupervisor::DumpTreeReply {
                    trace_id: trace,
                    result: Err("outpost has no target application configured".to_owned()),
                });
            return;
        };
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

    /// Dispatches one supervisor command. Returns `false` on `Shutdown`.
    pub fn handle_command(&mut self, command: &SupervisorToOutpost) -> bool {
        match command {
            SupervisorToOutpost::Configure {
                target_pid,
                backend_override,
            } => {
                self.configure(target_pid.0, *backend_override);
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
            SupervisorToOutpost::Shutdown => false,
        }
    }
}

/// Runs on a query-pool thread: finds the target's top-level window (its
/// currently active window, falling back to the first top-level window
/// found), arbitrates its backend, and walks its tree, bounded by
/// [`MAX_DUMP_DEPTH`] and [`MAX_DUMP_NODES`].
fn dump_tree(worker: &mut Worker, target_pid: u32, shared: &Shared) -> Result<DumpedTree, String> {
    let hwnd = focused_window(target_pid)
        .or_else(|| top_level_windows(target_pid).into_iter().next())
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

/// Reads and arbitrates the currently focused element of `target_pid`, using
/// UIA or MSAA per the verdict. Runs on a query worker (blocking allowed).
fn focused_snapshot(
    worker: &mut Worker,
    target_pid: u32,
    shared: &Shared,
) -> Option<(Backend, NodeSnapshot)> {
    let hwnd = focused_window(target_pid)?;
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
/// thread — see [`uia_passes_filter`], its reason for existing.
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

/// Runs an outpost driven by the Core pipes: reads commands from `pipe_in`,
/// writes outbound messages to `pipe_out`, until `Shutdown` or end of stream.
///
/// # Errors
///
/// Returns any I/O error reading the command stream.
pub fn run_pipe(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
) -> io::Result<()> {
    let mut outpost = Outpost::new(pipe_out);
    let mut reader = BufReader::new(pipe_in);
    while let Some(command) = read_message::<_, SupervisorToOutpost>(&mut reader)? {
        if !outpost.handle_command(&command) {
            break;
        }
    }
    Ok(())
}

/// Runs an outpost in dev-attach mode: writes outbound messages as JSON lines
/// to stdout, configures the given target, and streams events until the process
/// is killed. Does not return under normal operation.
///
/// # Errors
///
/// Returns an I/O error only if dev-mode setup fails before the event loop
/// begins; once running it blocks until the process is terminated.
pub fn run_attach(target_pid: u32) -> io::Result<()> {
    let mut outpost = Outpost::new(Box::new(io::stdout()));
    outpost.configure(target_pid, None);
    loop {
        thread::park();
    }
}
