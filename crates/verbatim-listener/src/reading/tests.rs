//! The reading threads' shell with a stand-in reading: each job is held
//! until its gate opens, and everything the shell does is logged in order.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::ThreadId;

use verbatim_listener_logic::schedule::{Ending, JobKind};
use verbatim_model::request::{CancelReason, RequestNumber};

use super::*;

/// How long a test waits for what its threads do.
const WAIT: Duration = Duration::from_secs(10);

/// One stand-in job: a request, held until its gate opens, if it has one.
struct Job {
    id: u64,
    gate: Option<Receiver<()>>,
}

/// A stand-in reading, logging each job's run with its thread, each end,
/// each answer and each thread's end.
struct Script {
    log: Mutex<Sender<String>>,
    ran_on: Mutex<Sender<(u64, ThreadId)>>,
}

impl Script {
    fn log(&self, line: String) {
        let _ = self
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(line);
    }
}

impl Reading for Script {
    type Job = Job;
    type Thread = ();
    type Done = u64;
    type Leftover = ();

    fn thread(&self, _pid: Pid) {}

    fn bar(&self, _job: &Job) -> bool {
        false
    }

    fn run(&self, (): &mut (), job: Job, _number: Option<RequestNumber>) -> u64 {
        let _ = self
            .ran_on
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send((job.id, thread::current().id()));
        if let Some(gate) = job.gate {
            let _ = gate.recv();
        }
        job.id
    }

    fn done(&self, done: u64, send: bool) {
        self.log(if send {
            format!("answer {done}")
        } else {
            format!("discarded {done}")
        });
    }

    fn ended(&self, ended: Ended<Job>) {
        self.log(format!("ended {} {:?}", ended.job.id, ended.ending));
    }

    fn abandoned(&self, number: RequestNumber) {
        self.log(format!("abandoned {}", number.0));
    }

    fn finish(&self, (): ()) {}
}

struct Fixture {
    readers: Readers<Script>,
    log: Receiver<String>,
    ran_on: Receiver<(u64, ThreadId)>,
}

impl Fixture {
    fn new() -> Self {
        let (log, logged) = mpsc::channel();
        let (ran_on, ran) = mpsc::channel();
        Self {
            readers: Readers::new(Script {
                log: Mutex::new(log),
                ran_on: Mutex::new(ran_on),
            }),
            log: logged,
            ran_on: ran,
        }
    }

    /// Queues request `id` of `pid`'s, held until the gate returned is
    /// dropped or opened.
    fn held(&self, pid: u32, id: u64) -> Sender<()> {
        let (open, gate) = mpsc::channel();
        self.push(pid, id, Some(gate));
        open
    }

    fn push(&self, pid: u32, id: u64, gate: Option<Receiver<()>>) {
        self.readers.push(
            Pid(pid),
            JobKind::Request {
                number: RequestNumber(id),
                focus: false,
            },
            Job { id, gate },
        );
    }

    fn ran(&self) -> (u64, ThreadId) {
        self.ran_on.recv_timeout(WAIT).expect("a job ran")
    }

    fn next(&self) -> String {
        self.log.recv_timeout(WAIT).expect("something logged")
    }

    /// Has the idle barrier's waiter log "idle" once what was queued before
    /// it has ended.
    fn when_idle(&self) {
        let log = self
            .readers
            .reading()
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.readers.when_idle(Box::new(move || {
            let _ = log.send("idle".to_owned());
        }));
    }
}

/// An application's requests are run in the order they came, on one
/// thread, each answered as it ends.
#[test]
fn an_applications_requests_run_in_order_on_one_thread() {
    let fixture = Fixture::new();
    let first = fixture.held(1, 1);
    fixture.push(1, 2, None);
    let (one, thread) = fixture.ran();
    assert_eq!(one, 1);
    drop(first);
    assert_eq!(fixture.ran(), (2, thread));
    assert_eq!([fixture.next(), fixture.next()], ["answer 1", "answer 2"]);
    fixture.readers.stop();
}

/// An application whose thread is held in a call holds no other.
#[test]
fn an_application_whose_thread_is_held_does_not_hold_another() {
    let fixture = Fixture::new();
    let held = fixture.held(1, 1);
    assert_eq!(fixture.ran().0, 1);
    fixture.push(2, 2, None);
    assert_eq!(fixture.ran().0, 2);
    assert_eq!(fixture.next(), "answer 2");
    drop(held);
    assert_eq!(fixture.next(), "answer 1");
    fixture.readers.stop();
}

/// The idle barrier's waiter runs after the answer of every request queued
/// before it, and not after one queued after it.
#[test]
fn the_idle_waiter_runs_after_every_answer_before_it_alone() {
    let fixture = Fixture::new();
    let first = fixture.held(1, 1);
    assert_eq!(fixture.ran().0, 1);
    fixture.when_idle();
    let later = fixture.held(2, 2);
    assert_eq!(fixture.ran().0, 2);
    drop(first);
    assert_eq!([fixture.next(), fixture.next()], ["answer 1", "idle"]);
    drop(later);
    assert_eq!(fixture.next(), "answer 2");
    fixture.readers.stop();
}

/// A queued request cancelled is answered cancelled and never run.
#[test]
fn a_queued_request_cancelled_is_answered_and_never_run() {
    let fixture = Fixture::new();
    let first = fixture.held(1, 1);
    assert_eq!(fixture.ran().0, 1);
    fixture.push(1, 2, None);
    fixture.readers.cancel(Cancel {
        number: RequestNumber(2),
        reason: CancelReason::DueTime,
    });
    assert_eq!(fixture.next(), format!("ended 2 {:?}", Ending::Cancelled));
    drop(first);
    assert_eq!(fixture.next(), "answer 1");
    fixture.readers.stop();
    assert!(fixture.ran_on.try_recv().is_err(), "request 2 never ran");
}

/// A running request cancelled for the foreground is answered abandoned,
/// a fresh thread runs what waits, and the abandoned call's result is
/// discarded when it returns.
#[test]
fn a_running_request_cancelled_for_the_foreground_is_abandoned_to_a_fresh_thread() {
    let fixture = Fixture::new();
    let held = fixture.held(1, 1);
    let (_, abandoned) = fixture.ran();
    fixture.push(1, 2, None);
    fixture.readers.cancel(Cancel {
        number: RequestNumber(1),
        reason: CancelReason::Foreground,
    });
    assert_eq!(fixture.next(), "abandoned 1");
    let (two, fresh) = fixture.ran();
    assert_eq!(two, 2);
    assert_ne!(fresh, abandoned);
    assert_eq!(fixture.next(), "answer 2");
    drop(held);
    assert_eq!(fixture.next(), "discarded 1");
    fixture.readers.stop();
}

/// A running request cancelled for a hung window is left alone: its
/// answer is sent when its call returns, and what waits runs after it.
#[test]
fn a_running_request_cancelled_as_hung_is_left_to_answer_late() {
    let fixture = Fixture::new();
    let held = fixture.held(1, 1);
    let (_, thread) = fixture.ran();
    fixture.push(1, 2, None);
    fixture.readers.cancel(Cancel {
        number: RequestNumber(1),
        reason: CancelReason::Hung,
    });
    drop(held);
    assert_eq!(fixture.next(), "answer 1");
    assert_eq!(fixture.ran(), (2, thread));
    assert_eq!(fixture.next(), "answer 2");
    fixture.readers.stop();
}

/// A process's exit gives its id's next process a thread of its own, the
/// exited one's thread finishing what it held.
#[test]
fn an_applications_exit_gives_a_reused_id_a_thread_of_its_own() {
    let fixture = Fixture::new();
    let held = fixture.held(1, 1);
    let (_, first) = fixture.ran();
    fixture.readers.application_exited(Pid(1));
    fixture.push(1, 2, None);
    let (two, second) = fixture.ran();
    assert_eq!(two, 2);
    assert_ne!(first, second);
    assert_eq!(fixture.next(), "answer 2");
    drop(held);
    assert_eq!(fixture.next(), "answer 1");
    fixture.readers.stop();
}

/// The stop ends what waits unrun and drops a running request's answer.
#[test]
fn the_stop_ends_what_waits_and_drops_a_running_answer() {
    let fixture = Fixture::new();
    let held = fixture.held(1, 1);
    assert_eq!(fixture.ran().0, 1);
    fixture.push(1, 2, None);
    fixture.readers.stop();
    assert_eq!(fixture.next(), format!("ended 2 {:?}", Ending::Stopped));
    drop(held);
    assert_eq!(fixture.next(), "discarded 1");
}
