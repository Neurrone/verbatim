//! What Verbatim offers an end-to-end test harness so that it waits for
//! evidence rather than for time: a named event set once Verbatim is ready
//! for input, and the barrier the control plane's `AwaitIdle` waits on.
//!
//! Readiness: a harness that launches Verbatim names an event in
//! [`READY_EVENT_ENV`], which it created before the launch. Verbatim sets it
//! the first time it is ready for input, as the control plane's status
//! reports readiness, so the harness waits on the event instead of asking
//! for the status over and over. Every change readiness depends on calls
//! [`Readiness::notify`].
//!
//! Idleness: `AwaitIdle` puts a waiter in line behind every gesture and
//! numbered key that reached the gesture router before it, and the reducer
//! answers it once it is idle (see [`IdleWaiter`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use verbatim_control::protocol::{OutpostState, OutpostStatus};
use verbatim_gui::GuiHandle;
use verbatim_input_windows::Routed;
use verbatim_model::Pid;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
use windows::core::HSTRING;

/// Names the event, created by the harness, that Verbatim sets when it is
/// first ready for input.
pub(crate) const READY_EVENT_ENV: &str = "VERBATIM_READY_EVENT";

/// The pieces of Verbatim whose readiness the control plane's status
/// reports, and the harness's event, set the first time they are all ready.
pub(crate) struct Readiness {
    pub(crate) own_pid: u32,
    pub(crate) gui_handle: Arc<OnceLock<GuiHandle>>,
    pub(crate) listener_ready: Arc<AtomicBool>,
    pub(crate) focus_known: Arc<AtomicBool>,
    pub(crate) outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    /// The harness's event, when one was named.
    event: Option<String>,
    signalled: AtomicBool,
}

impl Readiness {
    /// Readiness over these pieces, with the event [`READY_EVENT_ENV`]
    /// names, if any.
    pub(crate) fn new(
        own_pid: u32,
        gui_handle: Arc<OnceLock<GuiHandle>>,
        listener_ready: Arc<AtomicBool>,
        focus_known: Arc<AtomicBool>,
        outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    ) -> Self {
        Self {
            own_pid,
            gui_handle,
            listener_ready,
            focus_known,
            outposts,
            event: std::env::var(READY_EVENT_ENV)
                .ok()
                .filter(|name| !name.is_empty()),
            signalled: AtomicBool::new(false),
        }
    }

    /// Whether Verbatim is ready to take input: the GUI can act on
    /// gestures, the focus listener is running, the outpost reading
    /// Verbatim's own windows (its menu and dialogs) is ready, and Core
    /// knows the focus, so the first focus report cannot arrive after, and
    /// cut off, the speech of a key pressed straight away. With no
    /// foreground window at all there is no focus to wait for.
    pub(crate) fn is_ready(&self) -> bool {
        self.gui_handle.get().is_some()
            && self.listener_ready.load(Ordering::Acquire)
            && (self.focus_known.load(Ordering::Acquire) || crate::foreground_pid().is_none())
            && self
                .outposts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .any(|outpost| {
                    outpost.target_pid == Pid(self.own_pid) && outpost.state == OutpostState::Ready
                })
    }

    /// Sets the harness's event the first time Verbatim is ready. Called
    /// after every change readiness depends on; never while the outposts'
    /// lock is held.
    pub(crate) fn notify(&self) {
        let Some(name) = &self.event else {
            return;
        };
        if self.signalled.load(Ordering::Acquire) || !self.is_ready() {
            return;
        }
        if self.signalled.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: opens an existing named event for setting; the handle is
        // checked and closed below.
        match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name.as_str())) } {
            Ok(event) => {
                // SAFETY: `event` was just opened with EVENT_MODIFY_STATE.
                if let Err(error) = unsafe { SetEvent(event) } {
                    tracing::error!(name, %error, "could not set the readiness event");
                }
                close(event);
                tracing::info!(name, "ready; the readiness event is set");
            }
            Err(error) => tracing::error!(name, %error, "could not open the readiness event"),
        }
    }
}

fn close(handle: HANDLE) {
    // SAFETY: a handle this module opened, closed once.
    let _ = unsafe { CloseHandle(handle) };
}

/// One control-plane `AwaitIdle` in line: answered once the reducer has
/// handled input `after_input` (when given), its queues are empty, no
/// request to an outpost is outstanding, and the speech queue has taken in
/// everything the reducer gave it.
pub(crate) struct IdleWaiter {
    /// The numbered key to wait for.
    pub(crate) after_input: Option<u64>,
    /// Where the answer goes; a requester that gave up has dropped the
    /// receiver.
    pub(crate) reply: Sender<()>,
}

/// The waiters the control plane has put in line, by token, until the
/// reducer reaches their place in line ([`Routed::Barrier`]).
#[derive(Default)]
pub(crate) struct IdleLine {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, IdleWaiter>>,
    /// What the reducer last found outstanding, for the error a waiter that
    /// timed out reports.
    pub(crate) outstanding: Mutex<String>,
}

impl IdleLine {
    /// Puts a waiter in line behind everything sent to the gesture router
    /// before it, and waits up to `timeout` for its answer. Blocks the
    /// calling thread, a control-plane connection's own.
    pub(crate) fn wait(
        &self,
        router: &Sender<Routed>,
        after_input: Option<u64>,
        timeout: Duration,
    ) -> Result<(), String> {
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, answered) = bounded(1);
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, IdleWaiter { after_input, reply });
        if router.send(Routed::Barrier(token)).is_err() {
            self.take(token);
            return Err("the gesture router is gone".to_owned());
        }
        answered.recv_timeout(timeout).map_err(|_| {
            self.take(token);
            format!(
                "Verbatim was not idle within {timeout:?}{}: {}",
                after_input.map_or_else(String::new, |input| format!(" after input {input}")),
                self.outstanding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            )
        })
    }

    /// The waiter `token` names, taken out of line: the reducer takes it
    /// when it reaches its place.
    pub(crate) fn take(&self, token: u64) -> Option<IdleWaiter> {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&token)
    }
}
