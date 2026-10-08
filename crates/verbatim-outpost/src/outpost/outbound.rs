//! The outpost's writer: one thread that writes messages to Core's pipe.
//!
//! Pongs and `Ready` go ahead of everything else, so heartbeat delivery
//! never waits behind event traffic and a busy outpost is never mistaken for
//! a dead one. Ordinary messages pass through a bounded queue; when it is
//! full the worker waits, which is safe because the intake queue's own
//! limits bound what accumulates behind it.

#![forbid(unsafe_code)]

use std::io::Write;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};

use crate::protocol::{OutpostToSupervisor, write_message};

/// How many ordinary messages may wait for the pipe.
const OUTBOUND_CAPACITY: usize = 256;

/// What waits in the ordinary queue.
#[expect(
    clippy::large_enum_variant,
    reason = "messages travel unboxed, as they did before the rare flush marker joined them"
)]
enum Ordinary {
    /// A message to write.
    Message(OutpostToSupervisor),
    /// Answered once every ordinary message queued before it is written.
    Flushed(std::sync::mpsc::Sender<()>),
    /// Ends the writer once every message queued before it is written,
    /// closing the pipe: the outpost is shutting down.
    Close,
}

/// The sending side of the writer.
#[derive(Clone)]
pub(crate) struct Outbound {
    urgent: Sender<OutpostToSupervisor>,
    ordinary: Sender<Ordinary>,
}

impl Outbound {
    /// Starts the writer thread over `pipe`.
    ///
    /// # Panics
    ///
    /// Panics if the thread cannot be spawned, which means the process is
    /// out of OS thread resources.
    pub(crate) fn start(pipe: Box<dyn Write + Send>) -> (Self, JoinHandle<()>) {
        let (urgent, urgent_rx) = unbounded();
        let (ordinary, ordinary_rx) = bounded(OUTBOUND_CAPACITY);
        let join = thread::Builder::new()
            .name("verbatim-outbound".to_owned())
            .spawn(move || write_loop(pipe, &urgent_rx, &ordinary_rx))
            .expect("spawn the outbound writer");
        (Self { urgent, ordinary }, join)
    }

    /// Queues a message that must not wait behind others: a pong or `Ready`.
    pub(crate) fn urgent(&self, message: OutpostToSupervisor) {
        let _ = self.urgent.send(message);
    }

    /// Queues an ordinary message, waiting while the queue is full.
    pub(crate) fn send(&self, message: OutpostToSupervisor) {
        let _ = self.ordinary.send(Ordinary::Message(message));
    }

    /// Has the writer write every message queued before this call and
    /// then end, closing the pipe, for the outpost's shutdown. Whatever is
    /// sent afterwards is dropped.
    pub(crate) fn close(&self) {
        let _ = self.ordinary.send(Ordinary::Close);
    }

    /// Waits until every ordinary message queued before this call has been
    /// written to the pipe, or the writer has stopped.
    pub(crate) fn flush(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        if self.ordinary.send(Ordinary::Flushed(done_tx)).is_ok() {
            // An error means the writer stopped, with nothing left to write.
            let _ = done_rx.recv();
        }
    }
}

fn write_loop(
    mut pipe: Box<dyn Write + Send>,
    urgent: &Receiver<OutpostToSupervisor>,
    ordinary: &Receiver<Ordinary>,
) {
    loop {
        let message = if let Ok(message) = urgent.try_recv() {
            message
        } else {
            select! {
                recv(urgent) -> message => match message {
                    Ok(message) => message,
                    Err(_) => return,
                },
                recv(ordinary) -> message => match message {
                    Ok(Ordinary::Message(message)) => message,
                    Ok(Ordinary::Flushed(done)) => {
                        let _ = done.send(());
                        continue;
                    }
                    Ok(Ordinary::Close) => {
                        // Urgent messages queued before the close go too.
                        while let Ok(message) = urgent.try_recv() {
                            if write_message(&mut pipe, &message).is_err() {
                                return;
                            }
                        }
                        let _ = pipe.flush();
                        return;
                    }
                    Err(_) => return,
                },
            }
        };
        if write_message(&mut pipe, &message).is_err() {
            return;
        }
    }
}
