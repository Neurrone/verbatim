//! What the supervisor's tests share: the app's side of what the
//! supervisor delivers.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use verbatim_model::{Backend, NodeId, OutpostId, Role, StateSet, TraceId};
use verbatim_outpost::protocol::{HeldFocus, HeldNode};
use verbatim_outpost::supervisor::{Frames, OutpostMessage, Supervisor};

/// A focus Core holds, of a node of outpost `outpost`, observed at
/// `observed_at_ms`: what a test has the supervisor tell the listener and
/// the outposts (`Supervisor::core_focus`), whose fakes read the
/// observation as the test's message.
#[allow(
    dead_code,
    reason = "not every test of the supervisor tells Core's focus"
)]
pub fn held_focus(outpost: OutpostId, observed_at_ms: u64) -> HeldFocus {
    HeldFocus {
        trace_id: TraceId::mint(),
        observed: (observed_at_ms, 0),
        focus: HeldNode {
            id: NodeId::new(1).with_outpost(outpost),
            backend: Backend::Uia,
            role: Role::Button,
            name: None,
            states: StateSet::new(),
        },
        ancestors: Vec::new(),
        top_level: None,
        alive: true,
    }
}

/// The supervisor's messages as the app handles them: every one passed
/// through the supervisor's [`Frames`], so each outpost's frame comes out
/// as the message it carries, in each outpost's order.
pub struct Delivered {
    events: Receiver<OutpostMessage>,
    frames: RefCell<Frames>,
    ready: RefCell<VecDeque<OutpostMessage>>,
}

impl Delivered {
    /// What `supervisor` delivers to `events`.
    pub fn new(events: Receiver<OutpostMessage>, supervisor: &Supervisor) -> Self {
        Self {
            events,
            frames: RefCell::new(supervisor.frames()),
            ready: RefCell::new(VecDeque::new()),
        }
    }

    /// The next message, waiting up to `timeout` for it.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<OutpostMessage, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.ready.borrow_mut().pop_front() {
                return Ok(message);
            }
            let message = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            self.take(message);
        }
    }

    /// Every message already delivered, in order.
    pub fn try_iter(&self) -> impl Iterator<Item = OutpostMessage> + '_ {
        while let Ok(message) = self.events.try_recv() {
            self.take(message);
        }
        std::iter::from_fn(|| self.ready.borrow_mut().pop_front())
    }

    fn take(&self, message: OutpostMessage) {
        let mut out = Vec::new();
        self.frames.borrow_mut().arrived(message, &mut out);
        self.ready.borrow_mut().extend(out);
    }
}
