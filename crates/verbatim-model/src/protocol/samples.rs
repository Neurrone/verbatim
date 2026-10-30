//! One sample of every variant of [`OutpostToCore`], for the tests of the
//! crates that handle them: a test that must cover every variant takes
//! them from here, where a new variant must be given its sample, since the
//! chain that makes them names every variant without a wildcard.

use super::{EventTiming, OutpostToCore, QueryOutcome};
use crate::{Backend, NodeId, NormalizedEvent, Pid, TraceId};

/// One sample of each [`OutpostToCore`] variant, in declaration order.
#[must_use]
pub fn outpost_to_core() -> Vec<OutpostToCore> {
    let mut samples: Vec<OutpostToCore> = Vec::new();
    while let Some(sample) = next(samples.last()) {
        samples.push(sample);
    }
    samples
}

/// An event of node 1.
fn event() -> OutpostToCore {
    OutpostToCore::Event {
        trace_id: TraceId::mint(),
        observed_at_us: 0,
        backend: Backend::Uia,
        window: None,
        application: None,
        timing: EventTiming::default(),
        event: NormalizedEvent::TextChanged {
            node_id: NodeId::new(1),
        },
    }
}

/// One sample of each [`OutpostToCore`] variant, after `previous`.
fn next(previous: Option<&OutpostToCore>) -> Option<OutpostToCore> {
    let Some(previous) = previous else {
        return Some(OutpostToCore::Ready {
            outpost_pid: Pid(1),
            target_pid: Pid(2),
        });
    };
    Some(match previous {
        OutpostToCore::Ready { .. } => OutpostToCore::SpareReady {
            outpost_pid: Pid(1),
        },
        OutpostToCore::SpareReady { .. } => event(),
        OutpostToCore::Event { .. } => OutpostToCore::Reply {
            trace_id: TraceId::mint(),
            request_id: 1,
            outcome: QueryOutcome::Gone,
            timing: EventTiming::default(),
        },
        OutpostToCore::Reply { .. } => OutpostToCore::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        },
        OutpostToCore::Pong { .. } => OutpostToCore::Fault {
            detail: "a fault".to_owned(),
        },
        OutpostToCore::Fault { .. } => OutpostToCore::TargetExited,
        OutpostToCore::TargetExited => OutpostToCore::IdleAnswer { round: 1 },
        OutpostToCore::IdleAnswer { .. } => return None,
    })
}
