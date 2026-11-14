//! Measurement prototype (branch `prototype/background-events`): Core's
//! side of the focus listener's tells, for measuring what ADR 0003's limit
//! "Every background event reaches Core" costs. Each tell is parsed on the
//! reducer's thread, pushed into a queue that stands for Core's event
//! queue, merging a waiting entry for the same object, kind and raising
//! thread with the newest keeping its place, and popped at once. Nothing is
//! spoken and nothing is asked. Every five seconds while tells arrive, the
//! count taken in that span is logged at info, with how long each took from
//! the listener's observation to the reducer.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use verbatim_model::protocol::{ListenerToCore, Tell, TellKind};
use verbatim_process::clock::now_us;
use verbatim_supervisor::TellFrame;

/// How often the count of tells taken is logged.
const SPAN: Duration = Duration::from_secs(5);

/// What merges two waiting tells: the object, the kind, and the raising
/// thread.
type TellKey = (isize, i32, i32, TellKind, u32);

/// The stand-in for Core's event queue: waiting tells in arrival order,
/// and where each object's waiting tell is.
#[derive(Default)]
struct TellQueue {
    waiting: BTreeMap<u64, Tell>,
    by_key: HashMap<TellKey, u64>,
    next: u64,
}

impl TellQueue {
    /// Queues `tell` at the back, replacing a waiting tell for the same
    /// object, kind and thread. Returns whether one was replaced.
    fn push(&mut self, tell: Tell) -> bool {
        let key = (
            tell.window,
            tell.id_object,
            tell.id_child,
            tell.kind,
            tell.thread,
        );
        let place = self.next;
        self.next += 1;
        let merged = match self.by_key.insert(key, place) {
            Some(earlier) => self.waiting.remove(&earlier).is_some(),
            None => false,
        };
        self.waiting.insert(place, tell);
        merged
    }

    /// The front tell, taken out.
    fn pop(&mut self) -> Option<Tell> {
        let (_, tell) = self.waiting.pop_first()?;
        let key = (
            tell.window,
            tell.id_object,
            tell.id_child,
            tell.kind,
            tell.thread,
        );
        self.by_key.remove(&key);
        Some(tell)
    }
}

/// Counts of one span.
#[derive(Default)]
struct Span {
    tells: u64,
    merged: u64,
    unreadable: u64,
    /// From the listener's observation to the reducer taking it, summed and
    /// at most, in microseconds.
    observed_to_core_us: u64,
    observed_to_core_max_us: u64,
    /// From the relay's read to the reducer taking it, summed and at most.
    relay_to_core_us: u64,
    relay_to_core_max_us: u64,
}

/// The reducer thread's tells.
pub(crate) struct Tells {
    queue: TellQueue,
    started: Instant,
    span: Span,
}

impl Default for Tells {
    fn default() -> Self {
        Self {
            queue: TellQueue::default(),
            started: Instant::now(),
            span: Span::default(),
        }
    }
}

impl Tells {
    /// Takes the tell in `frame`: parses it, queues it and takes it off
    /// the queue again, and logs the span's counts once it has lasted
    /// five seconds.
    pub(crate) fn take(&mut self, frame: &TellFrame) {
        let taken_at_us = now_us();
        match frame.parse() {
            Ok(ListenerToCore::Tell { tell, timing }) => {
                if self.queue.push(tell) {
                    self.span.merged += 1;
                }
                let _ = self.queue.pop();
                self.span.tells += 1;
                let observed = taken_at_us.saturating_sub(timing.observed_at_us);
                self.span.observed_to_core_us += observed;
                self.span.observed_to_core_max_us = self.span.observed_to_core_max_us.max(observed);
                let relayed = taken_at_us.saturating_sub(frame.read_at_us());
                self.span.relay_to_core_us += relayed;
                self.span.relay_to_core_max_us = self.span.relay_to_core_max_us.max(relayed);
            }
            Ok(_) | Err(_) => self.span.unreadable += 1,
        }
        let elapsed = self.started.elapsed();
        if elapsed >= SPAN {
            let span = std::mem::take(&mut self.span);
            let mean = |sum: u64| sum.checked_div(span.tells).unwrap_or(0);
            tracing::info!(
                target: "verbatim::prototype",
                span_ms = elapsed.as_millis(),
                tells = span.tells,
                merged = span.merged,
                unreadable = span.unreadable,
                observed_to_core_mean_us = mean(span.observed_to_core_us),
                observed_to_core_max_us = span.observed_to_core_max_us,
                relay_to_core_mean_us = mean(span.relay_to_core_us),
                relay_to_core_max_us = span.relay_to_core_max_us,
                "tells taken by Core in the span"
            );
            self.started = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{Pid, WindowFacts, WindowHandle};

    fn tell(window: isize, kind: TellKind, thread: u32) -> Tell {
        Tell {
            kind,
            application: Pid(1),
            window,
            id_object: -4,
            id_child: 0,
            thread,
            raised_at: 0,
            facts: WindowFacts {
                top_level: WindowHandle(1),
                root_owner: WindowHandle(1),
                topmost: false,
                under_active_window: None,
                in_foreground: false,
                foreground_window: WindowHandle(2),
                class: String::new(),
                application: String::new(),
                alt_held: false,
                foreground_is_shell: false,
            },
        }
    }

    /// A tell for an object, kind and thread already waiting replaces it,
    /// the newest taking the back; any other difference is not merged.
    #[test]
    fn a_waiting_tell_for_the_same_object_kind_and_thread_is_replaced_by_the_newest() {
        let mut queue = TellQueue::default();
        assert!(!queue.push(tell(1, TellKind::NameChange, 7)));
        assert!(!queue.push(tell(2, TellKind::NameChange, 7)));
        assert!(!queue.push(tell(1, TellKind::StateChange, 7)));
        assert!(!queue.push(tell(1, TellKind::NameChange, 8)));
        assert!(queue.push(tell(1, TellKind::NameChange, 7)));
        let order: Vec<(isize, TellKind, u32)> = std::iter::from_fn(|| queue.pop())
            .map(|tell| (tell.window, tell.kind, tell.thread))
            .collect();
        assert_eq!(
            order,
            [
                (2, TellKind::NameChange, 7),
                (1, TellKind::StateChange, 7),
                (1, TellKind::NameChange, 8),
                (1, TellKind::NameChange, 7),
            ]
        );
    }
}
