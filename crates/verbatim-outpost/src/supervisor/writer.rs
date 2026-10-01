//! Each child's writer: a bounded queue of commands and the thread that
//! writes them to its pipe, so nothing that makes a decision ever waits on a
//! pipe (outpost redesign, "The supervisor").
//!
//! When the queue is full, each kind of command has its own policy:
//!
//! - a ping always gets through, past the bound, since a missed ping would
//!   get a healthy outpost killed;
//! - a routed fact replaces any older fact still waiting for the same object
//!   and kind; if there is none, the oldest waiting fact is dropped to make
//!   room, so the newest facts survive;
//! - anything else, a query above all, fails at once, so its asker gets an
//!   outcome straight away instead of waiting behind a stuck outpost.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use crate::protocol::{SupervisorToOutpost, write_message};

use crate::protocol::FactKey;

/// How many commands may wait for one child before the overload policy
/// applies.
pub(super) const WRITER_CAPACITY: usize = 64;

/// One command waiting to be written, with what the overload policy needs
/// to know about it.
#[derive(Debug)]
pub(super) enum Outgoing {
    /// A liveness ping.
    Ping(SupervisorToOutpost),
    /// A routed fact, keyed by the object and kind it concerns (`None` for
    /// a notification, which nothing replaces).
    Fact(Option<FactKey>, SupervisorToOutpost),
    /// Anything else: a query, an activation, a configuration command.
    Other(SupervisorToOutpost),
}

impl Outgoing {
    fn command(&self) -> &SupervisorToOutpost {
        match self {
            Outgoing::Ping(command) | Outgoing::Fact(_, command) | Outgoing::Other(command) => {
                command
            }
        }
    }
}

/// Why a command was not queued.
#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    /// The queue is full and the command is not one that replaces or gets
    /// through.
    Full,
    /// The child has ended and its writer is closed.
    Closed,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            QueueError::Full => "the outpost's command queue is full",
            QueueError::Closed => "the outpost has ended",
        })
    }
}

/// The waiting commands. Pure, so the overload policy is tested on its own.
#[derive(Debug, Default)]
pub(super) struct Queue {
    items: VecDeque<Outgoing>,
    closed: bool,
}

impl Queue {
    /// Adds `item` under the overload policy for `capacity`.
    pub(super) fn push(&mut self, item: Outgoing, capacity: usize) -> Result<(), QueueError> {
        if self.closed {
            return Err(QueueError::Closed);
        }
        match item {
            Outgoing::Ping(_) => {}
            Outgoing::Fact(ref key, _) => {
                let same = key.as_ref().and_then(|key| {
                    self.items.iter().position(
                        |waiting| matches!(waiting, Outgoing::Fact(Some(other), _) if other == key),
                    )
                });
                if let Some(index) = same {
                    self.items.remove(index);
                } else if self.items.len() >= capacity
                    && let Some(oldest) = self
                        .items
                        .iter()
                        .position(|waiting| matches!(waiting, Outgoing::Fact(..)))
                {
                    self.items.remove(oldest);
                }
                if self.items.len() >= capacity {
                    return Err(QueueError::Full);
                }
            }
            Outgoing::Other(_) => {
                if self.items.len() >= capacity {
                    return Err(QueueError::Full);
                }
            }
        }
        self.items.push_back(item);
        Ok(())
    }

    fn pop(&mut self) -> Option<Outgoing> {
        self.items.pop_front()
    }
}

/// The shared end of one child's queue: the owner and the reducer thread
/// push, the writer thread pops.
#[derive(Clone)]
pub(crate) struct WriterHandle {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl WriterHandle {
    /// Queues `item` without waiting.
    pub(super) fn push(&self, item: Outgoing) -> Result<(), QueueError> {
        let (queue, ready) = &*self.shared;
        let result = queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item, WRITER_CAPACITY);
        if result.is_ok() {
            ready.notify_one();
        }
        result
    }

    /// Closes the queue: nothing more is accepted, and the writer thread
    /// exits once the queue is empty.
    pub(super) fn close(&self) {
        let (queue, ready) = &*self.shared;
        queue.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        ready.notify_one();
    }
}

/// Starts the writer thread for a child's command pipe, returning the handle
/// commands are queued through. The thread exits when the queue is closed
/// and drained, or when a write fails because the child has gone.
pub(super) fn start(to_child: File, name: &str) -> std::io::Result<WriterHandle> {
    let handle = WriterHandle {
        shared: Arc::new((Mutex::new(Queue::default()), Condvar::new())),
    };
    let shared = Arc::clone(&handle.shared);
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut pipe = BufWriter::new(to_child);
            loop {
                let next = {
                    let (queue, ready) = &*shared;
                    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    loop {
                        if let Some(item) = queue.pop() {
                            break Some(item);
                        }
                        if queue.closed {
                            break None;
                        }
                        queue = ready.wait(queue).unwrap_or_else(PoisonError::into_inner);
                    }
                };
                let Some(item) = next else { return };
                if write_message(&mut pipe, item.command())
                    .and_then(|()| pipe.flush())
                    .is_err()
                {
                    return;
                }
            }
        })?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;

    fn ping() -> Outgoing {
        Outgoing::Ping(SupervisorToOutpost::Ping { seq: 1 })
    }

    fn query() -> Outgoing {
        Outgoing::Other(SupervisorToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id: 1,
            query: crate::protocol::Query::DumpTree,
        })
    }

    fn fact(hwnd: isize) -> Outgoing {
        Outgoing::Fact(
            Some(FactKey::Foreground(hwnd)),
            SupervisorToOutpost::Ping {
                seq: hwnd.cast_unsigned() as u64,
            },
        )
    }

    fn seqs(queue: &mut Queue) -> Vec<u64> {
        std::iter::from_fn(|| queue.pop())
            .map(|item| match item.command() {
                SupervisorToOutpost::Ping { seq } => *seq,
                _ => 0,
            })
            .collect()
    }

    #[test]
    fn a_full_queue_fails_a_query_at_once_but_lets_a_ping_through() {
        let mut queue = Queue::default();
        queue.push(query(), 1).expect("room for one");
        assert_eq!(queue.push(query(), 1), Err(QueueError::Full));
        assert_eq!(queue.push(ping(), 1), Ok(()));
    }

    #[test]
    fn a_fact_replaces_a_waiting_fact_for_the_same_object_and_moves_to_the_back() {
        let mut queue = Queue::default();
        queue.push(fact(1), 8).expect("queued");
        queue.push(fact(2), 8).expect("queued");
        queue.push(fact(1), 8).expect("queued");
        assert_eq!(seqs(&mut queue), vec![2, 1]);
    }

    #[test]
    fn a_new_fact_into_a_full_queue_drops_the_oldest_fact() {
        let mut queue = Queue::default();
        queue.push(fact(1), 2).expect("queued");
        queue.push(fact(2), 2).expect("queued");
        queue.push(fact(3), 2).expect("the oldest fact makes room");
        assert_eq!(seqs(&mut queue), vec![2, 3]);
    }

    #[test]
    fn a_closed_queue_refuses_everything() {
        let mut queue = Queue {
            closed: true,
            ..Queue::default()
        };
        assert_eq!(queue.push(ping(), 8), Err(QueueError::Closed));
    }
}
