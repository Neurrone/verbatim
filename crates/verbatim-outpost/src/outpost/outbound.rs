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

/// The sending side of the writer.
#[derive(Clone)]
pub(crate) struct Outbound {
    urgent: Sender<OutpostToSupervisor>,
    ordinary: Sender<OutpostToSupervisor>,
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
        let _ = self.ordinary.send(message);
    }
}

fn write_loop(
    mut pipe: Box<dyn Write + Send>,
    urgent: &Receiver<OutpostToSupervisor>,
    ordinary: &Receiver<OutpostToSupervisor>,
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
                    Ok(message) => message,
                    Err(_) => return,
                },
            }
        };
        if write_message(&mut pipe, &message).is_err() {
            return;
        }
    }
}
