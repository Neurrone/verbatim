//! The relay: the completion port the owner thread waits on, and the pipes
//! it serves through it (`docs/design/communication.md`, sections 4.7 and
//! 6.2; `docs/archive/relay.md`, section 4.1).
//!
//! The owner thread is the relay. It waits only on the port, for:
//!
//! - the listener's pipe and every outpost's, each read a frame at a time
//!   with one overlapped read in flight ([`PortReader`]), so the owner
//!   routes each of the listener's facts in the pass that read it, in the
//!   order the listener wrote them, and hands each outpost's frame on to
//!   the app in the pass that read it;
//! - the completion of a write to a child's command pipe that pended: every
//!   child's commands are written inline by the thread that sends them
//!   ([`super::writer`]), and only a write the pipe could not take at once
//!   comes back here, where the relay writes what waited behind it, so a
//!   child that stops reading holds up only its own commands;
//! - its mailbox ([`Mailbox`]), which carries everything other threads
//!   report: launches, retirements, hang checks, and the app.
//!
//! An operation that completes at once is handled where it was started; only
//! one that waits comes back through the port. Nothing here blocks, and
//! nothing logs on the delivered path.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use verbatim_model::{OutpostId, Pid};
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

/// The key the child `outpost`'s pipe to Core carries: odd.
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
        wake(&self.posted, || self.port.post(OWNER_KEY));
        Ok(())
    }
}

/// Posts the mailbox's wake with `post` unless one is already waiting to be
/// taken, as `posted` says. A wake the port refuses, which only a system
/// out of memory does, is not counted as waiting, so the next send tries
/// again; and the relay takes its inbox at every heartbeat as well, so
/// what was sent meanwhile waits at most a ping interval.
fn wake(posted: &AtomicBool, post: impl FnOnce() -> io::Result<()>) {
    if posted.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(error) = post() {
        posted.store(false, Ordering::SeqCst);
        tracing::error!(%error, "the relay could not be woken; it takes its mailbox at its next heartbeat");
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
    /// A child's pipe to Core, under this key: its read completed, or a
    /// wake asks the relay to read it.
    Read(usize),
}

/// Whose pipe to Core a reading is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// The focus listener's.
    Listener,
    /// An outpost's, watching this application.
    Outpost(Pid),
}

/// A child's pipe to Core, being read.
pub(super) struct Reader {
    /// The child's incarnation.
    pub(super) outpost: OutpostId,
    pub(super) source: Source,
    pub(super) pipe: PortReader,
}

/// The relay's port and the pipes it serves. Owned by the owner thread.
pub(super) struct Relay {
    port: Arc<Port>,
    commands: CommandPipes,
    /// The children's pipes to Core, by key.
    readers: HashMap<usize, Reader>,
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
            readers: HashMap::new(),
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

    /// Reads `pipe`, the child `outpost`'s pipe to Core, an overlapped
    /// end, from the relay's next pass on: a wake carrying its key starts
    /// the reading after whatever the owner is doing now.
    ///
    /// # Errors
    ///
    /// Returns the error of associating the pipe with the port, or of
    /// posting the wake.
    pub(super) fn add_reader(
        &mut self,
        outpost: OutpostId,
        source: Source,
        pipe: OwnedHandle,
    ) -> io::Result<()> {
        let key = read_key(outpost);
        let file = self.port.associate(pipe, key)?;
        // Nothing is in flight until the wake is taken, so a pipe whose
        // wake could not be posted is simply dropped.
        self.port.post(key)?;
        self.readers.insert(
            key,
            Reader {
                outpost,
                source,
                pipe: PortReader::new(file),
            },
        );
        Ok(())
    }

    /// Takes the reader under `key` out to read it, if it is still read: a
    /// wake can come for one that has ended.
    pub(super) fn take_reader(&mut self, key: usize) -> Option<Reader> {
        self.readers.remove(&key)
    }

    /// Puts back a reader taken out to be read, which goes on.
    pub(super) fn put_reader(&mut self, key: usize, reader: Reader) {
        self.readers.insert(key, reader);
    }

    /// Whether every pipe has finished: every writer written out or failed,
    /// and every child's pipe to Core ended.
    pub(super) fn is_idle(&self) -> bool {
        self.readers.is_empty() && self.commands.all_finished()
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
            Taken::Read(completion.key)
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

    /// A wake the port refuses is not counted as waiting: the next send
    /// posts again.
    #[test]
    fn a_refused_wake_is_posted_again_by_the_next_send() {
        let posted = AtomicBool::new(false);
        wake(&posted, || Err(io::Error::other("refused")));
        let mut posts = 0;
        wake(&posted, || {
            posts += 1;
            Ok(())
        });
        assert_eq!(posts, 1, "the next send posted");
        wake(&posted, || {
            posts += 1;
            Ok(())
        });
        assert_eq!(posts, 1, "one wake waits, so no other is posted");
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
            .push(Outgoing::Other(
                SupervisorToOutpost::NodesHeld {
                    nodes: (0..2_000).collect(),
                    anchors: Vec::new(),
                    acknowledged: 1,
                },
                Received::now(),
            ))
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
            stuck.push(Outgoing::Ping(SupervisorToOutpost::Ping {
                seq: 1,
                sent_at_us: 0,
            })),
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
        relay
            .add_reader(outpost, Source::Listener, core)
            .expect("added");
        let mut listener = File::from(listener);
        for seq in [1, 2] {
            write_message(
                &mut listener,
                &OutpostToSupervisor::Pong {
                    seq,
                    parked_count: 0,
                    written_at_us: 0,
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
                let Taken::Read(key) = relay.take(&completion) else {
                    panic!("only the listener's pipe is served");
                };
                let mut read = relay.take_reader(key).expect("still read");
                assert_eq!(read.outpost, outpost);
                let port = Arc::clone(relay.port());
                match read.pipe.take(&completion, &port, |body| {
                    let message: OutpostToSupervisor =
                        crate::protocol::decode_message(body).expect("a message");
                    if let OutpostToSupervisor::Pong { seq, .. } = message {
                        seqs.push(seq);
                    }
                    Ok(())
                }) {
                    verbatim_process::transport::Reading::Ended(result) => ended = Some(result),
                    _ => relay.put_reader(key, read),
                }
            }
        }
        assert_eq!(seqs, [1, 2]);
        assert!(matches!(ended, Some(Ok(()))), "{ended:?}");
        assert!(relay.is_idle());
    }
}
