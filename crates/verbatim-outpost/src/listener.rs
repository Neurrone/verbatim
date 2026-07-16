//! The focus-listener runtime (architecture section 1, decision D13).
//!
//! One permanent, stateless listener process holds the subscriptions that are
//! global by nature — the single desktop-global UIA focus registration and the
//! global MSAA `WinEvent` hooks for focus, foreground, and menu-popup opens
//! (process id zero) — and forwards each captured focus fact to Core, which
//! routes it to the target application's own outpost for acquisition,
//! arbitration, enrichment, and announcement.
//!
//! The listener's one hard rule is that it never makes a cross-process call.
//! A UIA focus callback delivers the element with its properties already
//! cached, so building a fact is local memory reads (plus `GetRuntimeId`, a
//! local read on a cached element); an MSAA `WinEvent` delivers a raw window
//! and object address, forwarded untouched; the only other read is the
//! hang-safe local `GetWindowThreadProcessId` that names the owning process.
//! No cross-process calls means no deadlines, no query pool, no parked
//! threads, and no way for any application to stall focus detection for the
//! rest of the desktop. The listener holds no per-application state, so a
//! crash respawns into full capability instantly.
//!
//! Run as `verbatim-outpost.exe --listener --pipe-in <handle> --pipe-out
//! <handle>` — the same binary as a per-application outpost, with no target
//! pid, spawned and supervised by the same machinery (job object, heartbeat,
//! respawn).

use std::ffi::c_void;
use std::io::{self, BufReader, Write};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, unbounded};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEventCallback, WinEventKind};
use verbatim_model::{Pid, TraceId};
use verbatim_uia::FocusRegistration;
use verbatim_uia::map::{
    cached_native_window_handle, cached_process_id, snapshot_parts_from_cached_element,
};

use crate::protocol::{
    ListenerFact, OutpostToSupervisor, SupervisorToOutpost, UiaSnapshotFact, read_message,
    write_message,
};
use crate::runtime::{EventThread, now_ms};

/// The focus listener: owns the desktop-global UIA focus registration, the
/// global MSAA hooks, and the outbound writer, for the whole life of the
/// process. Dropping it tears them all down.
struct Listener {
    outbound: Sender<OutpostToSupervisor>,
    _focus_registration: Option<FocusRegistration>,
    _event_thread: EventThread,
    _writer: JoinHandle<()>,
}

impl Listener {
    /// Sets up the listener: starts the outbound writer, announces readiness,
    /// installs the desktop-global UIA focus registration and the global MSAA
    /// hooks, and reports either failure as a [`OutpostToSupervisor::Fault`].
    ///
    /// # Panics
    ///
    /// Panics if the outbound writer thread cannot be spawned, which indicates
    /// the process is out of OS thread resources.
    #[must_use]
    fn new(writer: Box<dyn Write + Send>) -> Self {
        let (outbound, outbound_rx) = unbounded::<OutpostToSupervisor>();
        let writer_join = thread::Builder::new()
            .name("verbatim-listener-outbound".to_owned())
            .spawn(move || {
                let mut writer = writer;
                while let Ok(message) = outbound_rx.recv() {
                    if write_message(&mut writer, &message).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn listener outbound writer");

        // The listener has no target application; `target_pid` is a sentinel
        // the supervisor only logs (it keeps the listener in a dedicated slot,
        // never the per-pid map).
        let _ = outbound.send(OutpostToSupervisor::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(0),
        });

        let focus_registration = install_focus_registration(&outbound);

        let msaa_outbound = outbound.clone();
        let make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync> = Arc::new(move || {
            let outbound = msaa_outbound.clone();
            Box::new(move |kind, hwnd, id_object, id_child| {
                forward_msaa_event(&outbound, kind, hwnd, id_object, id_child);
            })
        });
        let event_thread = EventThread::spawn(0, LISTENER_SUBSCRIPTIONS, make_callback);

        Self {
            outbound,
            _focus_registration: focus_registration,
            _event_thread: event_thread,
            _writer: writer_join,
        }
    }

    /// Dispatches one supervisor command. The listener answers `Ping` with a
    /// `Pong` (it never parks a thread, so its parked count is always zero),
    /// exits on `Shutdown`, and ignores everything else — it has no target to
    /// fetch from, arbitrate for, or announce to. Returns `false` on
    /// `Shutdown`.
    fn handle_command(&self, command: &SupervisorToOutpost) -> bool {
        match command {
            SupervisorToOutpost::Ping { seq } => {
                let _ = self.outbound.send(OutpostToSupervisor::Pong {
                    seq: *seq,
                    parked_count: 0,
                });
                true
            }
            SupervisorToOutpost::Shutdown => false,
            _ => true,
        }
    }
}

/// Installs the desktop-global UIA focus registration, forwarding each focus
/// change as a [`ListenerFact::UiaFocus`] built entirely from cached reads.
/// A registration failure is reported as a fault and leaves the MSAA path
/// running on its own.
fn install_focus_registration(outbound: &Sender<OutpostToSupervisor>) -> Option<FocusRegistration> {
    let focus_outbound = outbound.clone();
    let focus_callback = Arc::new(move |element: &IUIAutomationElement| {
        // SAFETY: `element` is a cached focus element from the registration's
        // base cache request, so every read below is a cached local read and
        // never a cross-process call — the listener's hard rule (decision D13).
        unsafe {
            let Some(pid) = cached_process_id(element) else {
                return;
            };
            if pid == 0 {
                return;
            }
            let hwnd = cached_native_window_handle(element);
            let parts = snapshot_parts_from_cached_element(element);
            let snapshot = UiaSnapshotFact {
                runtime_id: parts.runtime_id,
                role: parts.role,
                name: parts.name,
                value: parts.value,
                states: parts.states,
                details: parts.details,
            };
            let _ = focus_outbound.send(OutpostToSupervisor::FocusFact {
                trace_id: TraceId::mint(),
                observed_at_ms: now_ms(),
                fact: ListenerFact::UiaFocus {
                    pid: Pid(pid),
                    hwnd,
                    snapshot,
                },
            });
        }
    });
    match FocusRegistration::new(focus_callback) {
        Ok(registration) => Some(registration),
        Err(error) => {
            let _ = outbound.send(OutpostToSupervisor::Fault {
                detail: format!("listener UIA focus registration failed: {error}"),
            });
            None
        }
    }
}

/// Forwards one global MSAA `WinEvent` as a [`ListenerFact`]. Reads the owning
/// pid with `GetWindowThreadProcessId` (a hang-safe local call) and drops a
/// null window or a pid-zero owner; the raw object address is forwarded
/// untouched — the app outpost does the acquisition.
fn forward_msaa_event(
    outbound: &Sender<OutpostToSupervisor>,
    kind: WinEventKind,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
) {
    if hwnd == 0 {
        return;
    }
    let pid = window_pid(hwnd);
    if pid == 0 {
        return;
    }
    let fact = match kind {
        WinEventKind::Focus => ListenerFact::MsaaFocus {
            pid: Pid(pid),
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Foreground => ListenerFact::Foreground {
            pid: Pid(pid),
            hwnd,
        },
        WinEventKind::MenuPopupStart => ListenerFact::MenuPopup {
            pid: Pid(pid),
            hwnd,
            id_object,
            id_child,
        },
        // The listener subscribes to nothing else (LISTENER_SUBSCRIPTIONS).
        _ => return,
    };
    let _ = outbound.send(OutpostToSupervisor::FocusFact {
        trace_id: TraceId::mint(),
        observed_at_ms: now_ms(),
        fact,
    });
}

/// The owning process id of `hwnd`, read with the hang-safe local
/// `GetWindowThreadProcessId`. Returns 0 for an invalid or ownerless window.
fn window_pid(hwnd: isize) -> u32 {
    // SAFETY: GetWindowThreadProcessId tolerates any window handle, writing 0
    // for an invalid one.
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(HWND(hwnd as *mut c_void), Some(&raw mut pid));
    }
    pid
}

/// Runs the focus listener driven by the Core pipes: reads commands from
/// `pipe_in`, writes facts and replies to `pipe_out`, until `Shutdown` or end
/// of stream (decision D13).
///
/// # Errors
///
/// Returns any I/O error reading the command stream.
pub fn run_listener(
    pipe_in: Box<dyn io::Read + Send>,
    pipe_out: Box<dyn Write + Send>,
) -> io::Result<()> {
    let listener = Listener::new(pipe_out);
    let mut reader = BufReader::new(pipe_in);
    while let Some(command) = read_message::<_, SupervisorToOutpost>(&mut reader)? {
        if !listener.handle_command(&command) {
            break;
        }
    }
    Ok(())
}
