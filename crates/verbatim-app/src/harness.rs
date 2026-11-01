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
//! Idleness: `AwaitIdle` posts a waiter to the reducer's port, where it
//! waits until the harness's numbered input it names has been handled, and
//! the reducer answers it once a round through every child has found
//! everything idle (see [`IdleWaiter`] and [`crate::idle`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};
use verbatim_control::protocol::{OutpostState, OutpostStatus};
use verbatim_gui::GuiHandle;
use verbatim_model::Pid;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
use windows::core::HSTRING;

/// Names the event, created by the harness, that Verbatim sets when it is
/// first ready for input.
pub(crate) const READY_EVENT_ENV: &str = "VERBATIM_READY_EVENT";

/// The pieces of Verbatim whose readiness the control plane's status
/// reports, and the harness's event, set the first time they are all ready.
///
/// The parts the reducer and the supervisor's report set, and whether the
/// event has been set, are under one lock: each setter records its part and
/// decides in the same hold whether every part is ready and the event not
/// yet set, so no two setters can each miss the other's part. The GUI's
/// handle and the outposts' map stay outside it, written before their
/// setter takes this lock.
pub(crate) struct Readiness {
    pub(crate) own_pid: u32,
    pub(crate) gui_handle: Arc<OnceLock<GuiHandle>>,
    pub(crate) outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    parts: Mutex<Parts>,
    /// The harness's event, when one was named.
    event: Option<String>,
}

/// What readiness records under its lock.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each part is set once by its own setter; none is a state of another"
)]
#[derive(Default)]
struct Parts {
    /// The focus listener has reported ready.
    listener_ready: bool,
    /// Core has known a focus.
    focus_known: bool,
    /// The supervisor's spare outposts are all ready.
    spares_ready: bool,
    /// The harness's event has been set.
    signalled: bool,
}

impl Readiness {
    /// Readiness over these pieces, with the event [`READY_EVENT_ENV`]
    /// names, if any.
    pub(crate) fn new(
        own_pid: u32,
        gui_handle: Arc<OnceLock<GuiHandle>>,
        outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    ) -> Self {
        Self {
            own_pid,
            gui_handle,
            outposts,
            parts: Mutex::new(Parts::default()),
            event: std::env::var(READY_EVENT_ENV)
                .ok()
                .filter(|name| !name.is_empty()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Parts> {
        self.parts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether Verbatim is ready to take input: the GUI can act on
    /// gestures, the focus listener is running, the outpost reading
    /// Verbatim's own windows (its menu and dialogs) is ready, the
    /// supervisor's spare outposts are ready, so every application a
    /// scenario starts takes a spare (`docs/design/focus-pipeline.md`,
    /// section 19.8, item 2), and Core knows the focus, so the first focus
    /// report cannot arrive after, and cut off, the speech of a key pressed
    /// straight away. With no foreground window at all there is no focus to
    /// wait for.
    pub(crate) fn is_ready(&self) -> bool {
        self.ready_with(&self.lock())
    }

    /// Whether every part is ready, `parts` read under the lock.
    fn ready_with(&self, parts: &Parts) -> bool {
        self.gui_handle.get().is_some()
            && parts.listener_ready
            && parts.spares_ready
            && (parts.focus_known || crate::foreground_pid().is_none())
            && self
                .outposts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .any(|outpost| {
                    outpost.target_pid == Pid(self.own_pid) && outpost.state == OutpostState::Ready
                })
    }

    /// Notes that the focus listener has reported ready.
    pub(crate) fn listener_ready(&self) {
        self.record(|parts| parts.listener_ready = true);
    }

    /// Notes that Core knows a focus.
    pub(crate) fn focus_known(&self) {
        self.record(|parts| parts.focus_known = true);
    }

    /// Notes that the supervisor's spares are all ready.
    pub(crate) fn spares_ready(&self) {
        self.record(|parts| parts.spares_ready = true);
    }

    /// Sets the harness's event the first time Verbatim is ready, after the
    /// GUI's handle or the outposts' map, which stay outside the lock, has
    /// changed. Never called while the outposts' lock is held.
    pub(crate) fn notify(&self) {
        self.record(|_| {});
    }

    /// Records a part with `set`, and in the same hold decides whether
    /// every part is ready and the event not yet set; sets the event after
    /// letting go of the lock.
    fn record(&self, set: impl FnOnce(&mut Parts)) {
        let signal = {
            let mut parts = self.lock();
            set(&mut parts);
            let signal = self.event.is_some() && !parts.signalled && self.ready_with(&parts);
            if signal {
                parts.signalled = true;
            }
            signal
        };
        if signal && let Some(name) = &self.event {
            set_event(name);
        }
    }
}

/// Opens the harness's event `name` and sets it.
fn set_event(name: &str) {
    // SAFETY: opens an existing named event for setting; the handle is
    // checked and closed below.
    match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name)) } {
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

fn close(handle: HANDLE) {
    // SAFETY: a handle this module opened, closed once.
    let _ = unsafe { CloseHandle(handle) };
}

/// One control-plane `AwaitIdle` in line: answered once the reducer has
/// handled input `after_input` (when given), a round of the idle barrier
/// started after that has found the listener, every outpost and the
/// reducer idle ([`crate::idle`]), and the speech queue has taken in
/// everything the reducer gave it.
pub(crate) struct IdleWaiter {
    /// The numbered key to wait for.
    pub(crate) after_input: Option<u64>,
    /// Where the answer goes; a requester that gave up has dropped the
    /// receiver.
    pub(crate) reply: Sender<()>,
}

/// What the control plane's `AwaitIdle` waiters share with the reducer.
#[derive(Default)]
pub(crate) struct IdleLine {
    /// What the reducer last found outstanding, for the error a waiter that
    /// timed out reports.
    pub(crate) outstanding: Mutex<String>,
}

impl IdleLine {
    /// Posts a waiter to the reducer's port and waits up to `timeout` for
    /// its answer. Blocks the calling thread, a control-plane connection's
    /// own. Every harness input is a numbered key, whose end the hook posts
    /// to the same port ([`ShellCommand::InputHandled`](crate::ShellCommand)),
    /// so a waiter that reaches the reducer before input `after_input` has
    /// been handled waits for it there (`crate::idle`); one with no
    /// `after_input` follows no harness input and has nothing to wait
    /// behind. A requester that gives up drops the receiver; its waiter
    /// stays in the barrier's line until a round takes it, which is once
    /// input `after_input` or a later one has been handled, and the
    /// reducer's answer then goes nowhere.
    pub(crate) fn wait(
        &self,
        commands: &crate::Commands,
        after_input: Option<u64>,
        timeout: Duration,
    ) -> Result<(), String> {
        let (reply, answered) = bounded(1);
        commands
            .send(crate::ShellCommand::AwaitIdle(IdleWaiter {
                after_input,
                reply,
            }))
            .map_err(|_| "the reducer is gone".to_owned())?;
        answered.recv_timeout(timeout).map_err(|_| {
            format!(
                "Verbatim was not idle within {timeout:?}{}: {}",
                after_input.map_or_else(String::new, |input| format!(" after input {input}")),
                self.outstanding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            )
        })
    }
}
