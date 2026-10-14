//! A queue whose every item is one packet posted to an I/O completion
//! port: how verbatim.exe's reducer takes its inputs
//! (`docs/design/communication.md`, sections 4.5 and 6.2).
//!
//! Any thread posts an item with [`Poster::send`], one
//! `PostQueuedCompletionStatus` that never waits, the item boxed and its
//! address carried as the packet's `OVERLAPPED` pointer, which the port
//! hands back untouched. The one thread holding the [`PortQueue`] waits on
//! the port and takes the items in the order the port received them: a
//! completion port's queue is first in, first out, so that order is the
//! single order of arrival across every thread that posts. The waiting
//! thread takes up to 64 packets per wait, so a burst costs it one system
//! call, not one per item.
//!
//! The queue counts the items posted and the items taken ([`Progress`]),
//! so another thread can tell whether the taking thread has stopped taking
//! while items wait: the relay's watch over the reducer (section 6.8).
//!
//! The module's rule for its `unsafe` code: a packet carrying an item is
//! made only by [`Poster::send`], from a box it gives up, on a port no
//! other code posts to, and each such packet is taken off the port exactly
//! once, by [`PortQueue`] or, for those left when everything is dropped, by
//! the queue's own drop, which turns it back into the box.

use std::collections::VecDeque;
use std::io;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::IO::{OVERLAPPED, PostQueuedCompletionStatus};

use crate::overlapped::{Completions, Port};

/// The key of a packet carrying an item.
const ITEM_KEY: usize = 1;

/// The key of the packet the last [`Poster`] posts as it is dropped.
const CLOSED_KEY: usize = 2;

/// How many packets one wait takes at most.
const TAKEN_PER_WAIT: usize = 64;

/// How many items have been posted to a queue, and how many its taker has
/// taken: the difference is what waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Items posted so far.
    pub posted: u64,
    /// Items the [`PortQueue`] has handed out so far.
    pub taken: u64,
}

impl Progress {
    /// How many items wait to be taken.
    #[must_use]
    pub fn waiting(&self) -> u64 {
        self.posted.saturating_sub(self.taken)
    }
}

/// What the posters and the taker share.
struct Shared<T> {
    port: Port,
    /// The live [`Poster`]s; the last one dropped posts [`CLOSED_KEY`].
    posters: AtomicUsize,
    posted: AtomicU64,
    taken: AtomicU64,
    /// Cleared when the [`PortQueue`] is dropped: later sends fail.
    open: AtomicBool,
    /// The queue carries `T`s from thread to thread: sendable and
    /// shareable only when `T` may be sent.
    items: PhantomData<std::sync::Mutex<T>>,
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Nothing posts any more, and nothing takes: the items still on the
        // port are taken here and dropped, so none leaks.
        let mut completions = Completions::with_capacity(TAKEN_PER_WAIT);
        while let Ok(taken) = self.port.wait(&mut completions, Some(Duration::ZERO)) {
            if taken == 0 {
                break;
            }
            for completion in completions.iter() {
                if completion.key == ITEM_KEY {
                    // SAFETY: an item packet's pointer is a box `send` gave
                    // up, taken off the port this once (the module's rule).
                    drop(unsafe { Box::from_raw(completion.pointer().cast::<T>()) });
                }
            }
        }
    }
}

/// Posts items to a [`PortQueue`]. Cloned for every thread that posts.
pub struct Poster<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for Poster<T> {
    fn clone(&self) -> Self {
        self.shared.posters.fetch_add(1, Ordering::SeqCst);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for Poster<T> {
    fn drop(&mut self) {
        if self.shared.posters.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Behind everything this and every other poster posted, so the
            // taker sees the end after the last item.
            let _ = self.shared.port.post(CLOSED_KEY);
        }
    }
}

impl<T> std::fmt::Debug for Poster<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Poster")
    }
}

impl<T: Send> Poster<T> {
    /// Posts `item`, behind everything posted before it from any thread.
    /// Never waits.
    ///
    /// # Errors
    ///
    /// Gives `item` back when the [`PortQueue`] has been dropped, or the
    /// port refuses the packet, which only a system out of memory does.
    pub fn send(&self, item: T) -> Result<(), T> {
        if !self.shared.open.load(Ordering::SeqCst) {
            return Err(item);
        }
        // Counted first, so the taken count never passes the posted one.
        self.shared.posted.fetch_add(1, Ordering::SeqCst);
        let item = Box::into_raw(Box::new(item));
        // SAFETY: the port is open; the pointer is carried, never read by
        // the system, and comes back to the queue's taker (the module's
        // rule).
        let posted = unsafe {
            PostQueuedCompletionStatus(
                self.shared.port.raw(),
                0,
                ITEM_KEY,
                Some(item.cast::<OVERLAPPED>()),
            )
        };
        if posted.is_ok() {
            return Ok(());
        }
        self.shared.posted.fetch_sub(1, Ordering::SeqCst);
        // SAFETY: the packet was not posted, so the box was never given up.
        Err(*unsafe { Box::from_raw(item) })
    }

    /// How many items have been posted and taken so far.
    #[must_use]
    pub fn progress(&self) -> Progress {
        // The taken count first: read after it, the posted count is at
        // least as large.
        let taken = self.shared.taken.load(Ordering::SeqCst);
        Progress {
            posted: self.shared.posted.load(Ordering::SeqCst),
            taken,
        }
    }
}

/// The taking end: one thread waits on the port and takes the items in
/// the order they were posted.
pub struct PortQueue<T> {
    shared: Arc<Shared<T>>,
    completions: Completions,
    /// Items taken off the port and not yet handed out, in order.
    ready: VecDeque<T>,
    /// Every poster has gone, and every item before the end was taken.
    closed: bool,
}

// SAFETY: the taker owns its room for completions outright; the pointers
// in it are the items of packets already taken into `ready`, never read
// again, so moving the taker to another thread moves nothing shared but
// the `T`s, which are `Send`.
unsafe impl<T: Send> Send for PortQueue<T> {}

impl<T> Drop for PortQueue<T> {
    fn drop(&mut self) {
        self.shared.open.store(false, Ordering::SeqCst);
    }
}

impl<T> std::fmt::Debug for PortQueue<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PortQueue")
    }
}

/// A new queue on a port of its own: its poster, to be cloned for every
/// thread that posts, and its taker.
///
/// # Errors
///
/// Returns the error if the port cannot be made.
pub fn port_queue<T: Send>() -> io::Result<(Poster<T>, PortQueue<T>)> {
    let shared = Arc::new(Shared {
        port: Port::new()?,
        posters: AtomicUsize::new(1),
        posted: AtomicU64::new(0),
        taken: AtomicU64::new(0),
        open: AtomicBool::new(true),
        items: PhantomData,
    });
    Ok((
        Poster {
            shared: Arc::clone(&shared),
        },
        PortQueue {
            shared,
            completions: Completions::with_capacity(TAKEN_PER_WAIT),
            ready: VecDeque::new(),
            closed: false,
        },
    ))
}

impl<T: Send> PortQueue<T> {
    /// The next item, waiting for one as long as it takes; `None` once
    /// every [`Poster`] has gone and every item has been taken.
    pub fn recv(&mut self) -> Option<T> {
        self.next(None)
    }

    /// The next item, waiting up to `timeout` for one; `None` when none
    /// came, or when every [`Poster`] has gone and every item has been
    /// taken.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<T> {
        self.next(Some(Instant::now() + timeout))
    }

    /// The next item if one is already on the port, without waiting: a
    /// zero-timeout dequeue.
    pub fn try_recv(&mut self) -> Option<T> {
        self.next(Some(Instant::now()))
    }

    /// The next item, waiting until `deadline`, or for ever when `None`.
    fn next(&mut self, deadline: Option<Instant>) -> Option<T> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                self.shared.taken.fetch_add(1, Ordering::SeqCst);
                return Some(item);
            }
            if self.closed {
                return None;
            }
            let timeout =
                deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            match self.shared.port.wait(&mut self.completions, timeout) {
                Ok(0) => return None,
                Ok(_) => self.take_completions(),
                Err(error) => {
                    tracing::error!(%error, "a queue's port could not be waited on");
                    return None;
                }
            }
        }
    }

    /// Moves the items the last wait took into `ready`, in order.
    fn take_completions(&mut self) {
        for completion in self.completions.iter() {
            match completion.key {
                ITEM_KEY => {
                    // SAFETY: an item packet's pointer is a box `send` gave
                    // up, taken off the port this once (the module's rule).
                    let item = unsafe { Box::from_raw(completion.pointer().cast::<T>()) };
                    self.ready.push_back(*item);
                }
                CLOSED_KEY if self.shared.posters.load(Ordering::SeqCst) == 0 => {
                    self.closed = true;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a test waits for an item it posted.
    const BOUND: Duration = Duration::from_secs(10);

    #[test]
    fn items_posted_from_several_threads_are_taken_in_the_order_posted() {
        let (poster, mut queue) = port_queue::<String>().expect("a queue");
        poster.send("first".to_owned()).expect("posted");
        let other = poster.clone();
        std::thread::spawn(move || other.send("second".to_owned()).expect("posted"))
            .join()
            .expect("the thread ends");
        poster.send("third".to_owned()).expect("posted");
        let taken: Vec<String> = (0..3)
            .map(|_| queue.recv_timeout(BOUND).expect("an item"))
            .collect();
        assert_eq!(taken, ["first", "second", "third"]);
        assert_eq!(queue.try_recv(), None);
    }

    #[test]
    fn progress_counts_what_waits_until_it_is_taken() {
        let (poster, mut queue) = port_queue::<u32>().expect("a queue");
        poster.send(1).expect("posted");
        poster.send(2).expect("posted");
        assert_eq!(
            poster.progress(),
            Progress {
                posted: 2,
                taken: 0
            }
        );
        assert_eq!(queue.try_recv(), Some(1));
        assert_eq!(poster.progress().waiting(), 1);
        assert_eq!(queue.try_recv(), Some(2));
        assert_eq!(poster.progress().waiting(), 0);
    }

    #[test]
    fn the_queue_ends_after_the_last_item_once_every_poster_has_gone() {
        let (poster, mut queue) = port_queue::<u32>().expect("a queue");
        let other = poster.clone();
        other.send(7).expect("posted");
        drop(other);
        drop(poster);
        assert_eq!(queue.recv(), Some(7));
        assert_eq!(queue.recv(), None);
    }

    #[test]
    fn a_send_after_the_taker_has_gone_gives_the_item_back() {
        let (poster, queue) = port_queue::<u32>().expect("a queue");
        drop(queue);
        assert_eq!(poster.send(3), Err(3));
    }

    /// Counts its drops.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn items_never_taken_are_dropped_with_the_queue() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let (poster, queue) = port_queue::<Counted>().expect("a queue");
        poster
            .send(Counted(Arc::clone(&dropped)))
            .map_err(|_| "refused")
            .expect("posted");
        drop(queue);
        drop(poster);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
