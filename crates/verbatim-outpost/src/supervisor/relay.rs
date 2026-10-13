//! The relay: the completion port the owner thread waits on, and the pipes
//! it serves through it (`docs/design/communication.md`, sections 4.7 and
//! 6.2; `docs/design/relay.md`, section 4.1).
//!
//! The owner thread is the relay. It waits only on the port, for:
//!
//! - the listener's pipe, read a frame at a time with one overlapped read in
//!   flight ([`PortReader`]), so the owner routes each fact in the pass that
//!   read it, in the order the listener wrote them;
//! - the completion of a write to a child's command pipe that pended: every
//!   child's commands are written inline by the thread that sends them
//!   ([`super::writer`]), and only a write the pipe could not take at once
//!   comes back here, where the relay writes what waited behind it, so a
//!   child that stops reading holds up only its own commands;
//! - its mailbox ([`Mailbox`]), which carries everything other threads
//!   report: launches, retirements, hang checks, the outposts' reader
//!   threads, and the app.
//!
//! An operation that completes at once is handled where it was started; only
//! one that waits comes back through the port. Nothing here blocks, and
//! nothing logs on the delivered path. An outpost's messages to Core are
//! still read by a thread of their own, until the relay reads them too
//! (`docs/design/communication.md`, section 11, step 7).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use verbatim_model::OutpostId;
use verbatim_process::overlapped::{Completion, Completions, Port};
use verbatim_process::transport::PortReader;

use super::writer::WriterHandle;

/// The key of the wake-up that says the mailbox has something. Every pipe's
/// key is at least 2, outpost ids starting at 1.
pub(super) const OWNER_KEY: usize = 0;

/// How many completions one wait takes at most.
pub(super) const COMPLETIONS_PER_WAIT: usize = 64;

/// The key the completions of writes to `outpost`'s command pipe carry:
/// even, so a pipe's direction is known from its key alone.
pub(super) fn command_key(outpost: OutpostId) -> usize {
    usize::try_from(outpost.0)
        .ok()
        .and_then(|id| id.checked_mul(2))
        .unwrap_or(usize::MAX - 1)
}

/// The key the listener `outpost`'s pipe to Core carries: odd.
pub(super) fn read_key(outpost: OutpostId) -> usize {
    command_key(outpost) + 1
}

/// The sending end of the owner's mailbox: every send wakes the relay,
/// unless a wake is already waiting to be taken.
pub(super) struct Mailbox<T> {
    sender: Sender<T>,
    port: Arc<Port>,
    posted: Arc<AtomicBool>,
}

impl<T> Clone for Mailbox<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            port: Arc::clone(&self.port),
            posted: Arc::clone(&self.posted),
        }
    }
}

/// The owner has ended, so nothing sent to it is read.
#[derive(Debug)]
pub(super) struct OwnerGone;

impl<T> Mailbox<T> {
    /// Sends `item` to the owner and wakes it.
    ///
    /// # Errors
    ///
    /// Returns [`OwnerGone`] when the owner has ended.
    pub(super) fn send(&self, item: T) -> Result<(), OwnerGone> {
        self.sender.send(item).map_err(|_| OwnerGone)?;
        if !self.posted.swap(true, Ordering::SeqCst)
            && let Err(error) = self.port.post(OWNER_KEY)
        {
            // The port lives as long as any mailbox does; not expected.
            tracing::error!(%error, "the relay could not be woken");
        }
        Ok(())
    }
}

/// The receiving end of the owner's mailbox.
pub(super) struct Inbox<T> {
    receiver: Receiver<T>,
    posted: Arc<AtomicBool>,
}

impl<T> Inbox<T> {
    /// Everything sent so far, in order, once the mailbox's wake has been
    /// taken from the port. A send after this begins wakes the relay again.
    pub(super) fn take(&self) -> impl Iterator<Item = T> + '_ {
        self.posted.store(false, Ordering::SeqCst);
        self.receiver.try_iter()
    }
}

/// The owner's mailbox, whose sends wake the relay through `port`.
pub(super) fn mailbox<T>(port: Arc<Port>) -> (Mailbox<T>, Inbox<T>) {
    let (sender, receiver) = unbounded();
    let posted = Arc::new(AtomicBool::new(false));
    (
        Mailbox {
            sender,
            port,
            posted: Arc::clone(&posted),
        },
        Inbox { receiver, posted },
    )
}

/// Every child's command writer on the relay's port, by key, from the
/// moment it is made, so a completion of a write that pended always finds
/// its writer, whichever thread started the write. Shared with the launch
/// helpers, which make the writers.
#[derive(Clone)]
pub(super) struct CommandPipes {
    port: Arc<Port>,
    writers: Arc<Mutex<HashMap<usize, WriterHandle>>>,
}

impl CommandPipes {
    fn lock(&self) -> MutexGuard<'_, HashMap<usize, WriterHandle>> {
        self.writers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The writer for `outpost`'s command pipe, `pipe`, an overlapped end,
    /// whose writes that pend complete on the relay's port.
    ///
    /// # Errors
    ///
    /// Returns the error of associating the pipe with the port.
    pub(super) fn open(&self, outpost: OutpostId, pipe: OwnedHandle) -> io::Result<WriterHandle> {
        let key = command_key(outpost);
        let writer = WriterHandle::on_port(pipe, Arc::clone(&self.port), key)?;
        self.lock().insert(key, writer.clone());
        Ok(writer)
    }

    /// Finishes the write `completion` ends, and writes what waited behind
    /// it; lets the writer go once it has finished.
    fn complete(&self, completion: &Completion) {
        let writer = self.lock().get(&completion.key).cloned();
        let Some(writer) = writer else {
            // A writer is let go only once it has finished, with nothing in
            // flight, so every completion has its writer.
            tracing::error!(
                key = completion.key,
                "a write completed for a pipe the relay does not have"
            );
            return;
        };
        writer.complete(completion);
        if writer.is_finished() {
            self.lock().remove(&completion.key);
        }
    }

    /// Lets go of every writer that has finished: closed and written out,
    /// or failed because its child has gone. No completion comes for them.
    pub(super) fn forget_finished(&self) {
        self.lock().retain(|_, writer| !writer.is_finished());
    }

    /// Whether every writer has finished.
    fn all_finished(&self) -> bool {
        self.forget_finished();
        self.lock().is_empty()
    }
}

/// What a completion taken from the port is for.
pub(super) enum Taken {
    /// The mailbox has something.
    Mailbox,
    /// A write to a child's command pipe, finished by the relay already.
    Write,
    /// The listener's pipe, under this key: its read completed, or a wake
    /// asks the relay to read it.
    Listener(usize),
}

/// The relay's port and the pipes it serves. Owned by the owner thread.
pub(super) struct Relay {
    port: Arc<Port>,
    commands: CommandPipes,
    /// The listeners' pipes to Core, by key, each with its incarnation.
    listeners: HashMap<usize, (OutpostId, PortReader)>,
}

impl Relay {
    /// A relay with its port and no pipes.
    ///
    /// # Errors
    ///
    /// Returns the error if the port cannot be made.
    pub(super) fn new() -> io::Result<Self> {
        let port = Arc::new(Port::new()?);
        Ok(Self {
            commands: CommandPipes {
                port: Arc::clone(&port),
                writers: Arc::default(),
            },
            port,
            listeners: HashMap::new(),
        })
    }

    /// The port, which the mailbox wakes.
    pub(super) fn port(&self) -> &Arc<Port> {
        &self.port
    }

    /// The command writers, which launch helpers add to.
    pub(super) fn commands(&self) -> &CommandPipes {
        &self.commands
    }

    /// Reads `pipe`, the listener `outpost`'s pipe to Core, an overlapped
    /// end, from the relay's next pass on: a wake carrying its key starts
    /// the reading after whatever the owner is doing now.
    ///
    /// # Errors
    ///
    /// Returns the error of associating the pipe with the port, or of
    /// posting the wake.
    pub(super) fn add_listener(&mut self, outpost: OutpostId, pipe: OwnedHandle) -> io::Result<()> {
        let key = read_key(outpost);
        let file = self.port.associate(pipe, key)?;
        // Nothing is in flight until the wake is taken, so a pipe whose
        // wake could not be posted is simply dropped.
        self.port.post(key)?;
        self.listeners.insert(key, (outpost, PortReader::new(file)));
        Ok(())
    }

    /// Takes the listener's reader under `key` out to read it, if it is
    /// still read: a wake can come for one that has ended.
    pub(super) fn take_listener(&mut self, key: usize) -> Option<(OutpostId, PortReader)> {
        self.listeners.remove(&key)
    }

    /// Puts back a listener's reader taken out to be read, which goes on.
    pub(super) fn put_listener(&mut self, key: usize, outpost: OutpostId, reader: PortReader) {
        self.listeners.insert(key, (outpost, reader));
    }

    /// Whether every pipe has finished: every writer written out or failed,
    /// and every listener's pipe ended.
    pub(super) fn is_idle(&self) -> bool {
        self.listeners.is_empty() && self.commands.all_finished()
    }

    /// Waits up to `timeout`, or for ever when `None`, for completions and
    /// wakes, and takes those queued into `completions`, which the waiting
    /// thread owns. Returns how many it took.
    ///
    /// # Errors
    ///
    /// Returns the error if the wait fails.
    pub(super) fn wait(
        &self,
        completions: &mut Completions,
        timeout: Option<Duration>,
    ) -> io::Result<usize> {
        self.port.wait(completions, timeout)
    }

    /// What `completion`, taken from the port, is for; a write's is handled
    /// here and then.
    pub(super) fn take(&self, completion: &Completion) -> Taken {
        if completion.key == OWNER_KEY {
            Taken::Mailbox
        } else if completion.key.is_multiple_of(2) {
            if !completion.is_posted() {
                self.commands.complete(completion);
            }
            Taken::Write
        } else {
            Taken::Listener(completion.key)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;

    use verbatim_model::TraceId;
    use verbatim_process::overlapped::{End, Flow, pipe};

    use super::*;
    use crate::protocol::{
        DeliveredFact, EventTiming, OutpostToSupervisor, SupervisorToOutpost, read_message,
        write_message,
    };
    use crate::supervisor::policy::{HeldFact, HeldFacts};
    use crate::supervisor::writer::{Outgoing, QueueError, Received};

    /// How long a test waits for a completion it caused before it fails.
    const BOUND: Duration = Duration::from_secs(10);

    /// A child's command pipe on `relay`, with a buffer of `buffer` bytes:
    /// its writer, and the child's end.
    fn command_pipe(relay: &Relay, outpost: u64, buffer: u32) -> (WriterHandle, BufReader<File>) {
        let (core, child) = pipe(Flow::Outbound, buffer, End::Synchronous).expect("a pipe");
        let writer = relay
            .commands()
            .open(OutpostId(outpost), core)
            .expect("a writer");
        (writer, BufReader::new(File::from(child)))
    }

    fn held(observed_at_ms: u64, hwnd: isize) -> HeldFact {
        HeldFact {
            trace_id: TraceId::mint(),
            observed_at_ms,
            timing: EventTiming::default(),
            fact: DeliveredFact::Foreground { hwnd },
            win_event: None,
            received: Received::now(),
        }
    }

    /// What a command read off a child's pipe is, for the order asserted.
    fn name(command: &SupervisorToOutpost) -> String {
        match command {
            SupervisorToOutpost::DeliverFact { observed_at_ms, .. } => {
                format!("fact {observed_at_ms}")
            }
            SupervisorToOutpost::ForwardingEnded { through } => format!("ended {through}"),
            other => format!("{other:?}"),
        }
    }

    fn read_names(child: &mut BufReader<File>, count: usize) -> Vec<String> {
        (0..count)
            .map(|_| {
                let command: SupervisorToOutpost = read_message(child)
                    .expect("a message")
                    .expect("not the end");
                name(&command)
            })
            .collect()
    }

    /// The owner's order through the relay's writer: facts held while the
    /// outpost started go out first, in their order, then a fact read after
    /// its `Ready`, then the listener's "forwarding ended" behind them all.
    #[test]
    fn held_facts_then_later_facts_then_forwarding_ended_reach_the_outpost_in_that_order() {
        let relay = Relay::new().expect("a relay");
        let (writer, mut child) = command_pipe(&relay, 1, 0);
        let mut starting = HeldFacts::default();
        starting.hold(held(1, 10));
        starting.hold(held(2, 20));
        for fact in starting.take() {
            super::super::owner::deliver(&writer, fact);
        }
        super::super::owner::deliver(&writer, held(3, 30));
        writer
            .push(Outgoing::HandOver(SupervisorToOutpost::ForwardingEnded {
                through: 99,
            }))
            .expect("queued");
        assert_eq!(
            read_names(&mut child, 4),
            ["fact 1", "fact 2", "fact 3", "ended 99"]
        );
    }

    /// A child that stops reading holds up only its own commands: the
    /// other child's are written at once. When the stuck child goes with a
    /// write in flight, the write fails through the port, the relay lets
    /// its writer go, and the writer refuses everything after.
    #[test]
    fn a_stuck_child_delays_no_other_and_its_end_mid_write_closes_its_writer() {
        let relay = Relay::new().expect("a relay");
        let (stuck, stuck_child) = command_pipe(&relay, 1, 4096);
        let (other, mut other_child) = command_pipe(&relay, 2, 4096);
        // More than the pipe's 4 KiB buffer, so its write waits for a
        // reader that never comes.
        stuck
            .push(Outgoing::NodesHeld(SupervisorToOutpost::NodesHeld {
                nodes: (0..2_000).collect(),
                anchors: Vec::new(),
                acknowledged: 1,
            }))
            .expect("written, and pending");
        super::super::owner::deliver(&stuck, held(1, 10));
        super::super::owner::deliver(&other, held(2, 20));
        assert_eq!(read_names(&mut other_child, 1), ["fact 2"]);

        drop(stuck_child);
        let mut completions = Completions::with_capacity(4);
        assert_eq!(
            relay.wait(&mut completions, Some(BOUND)).expect("waited"),
            1
        );
        let completion = completions.iter().next().expect("one");
        assert!(
            matches!(relay.take(&completion), Taken::Write),
            "the stuck write's completion"
        );
        assert_eq!(
            stuck.push(Outgoing::Ping(SupervisorToOutpost::Ping { seq: 1 })),
            Err(QueueError::Closed)
        );
        assert!(!relay.is_idle(), "the other child's writer is still open");
        other.close();
        assert!(relay.is_idle());
    }

    /// The listener's pipe is read from the wake its adding posts: each
    /// frame is handed out in order, and the pipe's end is reported once,
    /// after them.
    #[test]
    fn the_listeners_frames_are_read_in_order_and_its_end_follows_them() {
        let mut relay = Relay::new().expect("a relay");
        let (core, listener) = pipe(Flow::Inbound, 0, End::Synchronous).expect("a pipe");
        let outpost = OutpostId(5);
        relay.add_listener(outpost, core).expect("added");
        let mut listener = File::from(listener);
        for seq in [1, 2] {
            write_message(
                &mut listener,
                &OutpostToSupervisor::Pong {
                    seq,
                    parked_count: 0,
                },
            )
            .expect("written");
        }
        drop(listener);
        let mut seqs = Vec::new();
        let mut ended = None;
        let mut completions = Completions::with_capacity(4);
        while ended.is_none() {
            let count = relay.wait(&mut completions, Some(BOUND)).expect("waited");
            assert!(count > 0, "a completion within the bound");
            for completion in completions.iter() {
                let Taken::Listener(key) = relay.take(&completion) else {
                    panic!("only the listener's pipe is served");
                };
                let (read, mut reader) = relay.take_listener(key).expect("still read");
                assert_eq!(read, outpost);
                let port = Arc::clone(relay.port());
                match reader.take(&completion, &port, |body| {
                    let message: OutpostToSupervisor =
                        crate::protocol::decode_message(body).expect("a message");
                    if let OutpostToSupervisor::Pong { seq, .. } = message {
                        seqs.push(seq);
                    }
                    Ok(())
                }) {
                    verbatim_process::transport::Reading::Ended(result) => ended = Some(result),
                    _ => relay.put_listener(key, read, reader),
                }
            }
        }
        assert_eq!(seqs, [1, 2]);
        assert!(matches!(ended, Some(Ok(()))), "{ended:?}");
        assert!(relay.is_idle());
    }
}
