//! The relay's failure paths, each run through its fault point
//! (`books/dev/src/reference/crates/verbatim-faults.md`, "Failure paths"): a
//! message the app's port refuses is delivered again, in order, and a
//! shutdown whose mailbox wake the relay's port refuses still ends.
//!
//! This test binary runs itself as a stand-in for the outpost binary, which
//! says it is ready, or a spare, answers pings, and shuts down when asked;
//! and as a stand-in for the focus listener, which says it is ready, answers
//! pings and shuts down when asked. Nothing makes a UI Automation or
//! `WinEvent` call, so the test touches nothing on the desktop. Its own
//! `main` recognizes each command line. It arms fault points, so only
//! `cargo xtask faults` builds it, and its tests run one after another.

use std::io::BufReader;
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_model::Pid;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::{read_message, write_message};
use verbatim_process::port_queue::port_queue;
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use verbatim_supervisor::{
    EventSink, Executables, OutpostMessage, SPARES, ShutdownSummary, Supervisor,
};

mod common;
use common::Delivered;

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// The value after `name` on the command line, if it is there.
fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// A stand-in child: the listener says `Ready`; an outpost says `Ready` for
/// its application, or, as a spare, `SpareReady`, and `Ready` when given an
/// application. Each answers pings, and exits cleanly for the shutdown
/// message or the end of its command pipe.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} is a handle value: {args:?}"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (commands, messages) = unsafe {
        verbatim_process::inherited_pipe_handles(handle("--pipe-in"), handle("--pipe-out"))
    }
    .expect("the inherited pipes");
    // The supervisor makes both ends overlapped; this stand-in blocks on
    // each read and write.
    let (commands, mut messages) = (
        BlockingReader::new(commands).expect("a reader"),
        BlockingWriter::new(messages).expect("a writer"),
    );
    let mut commands = BufReader::new(commands);
    let own_pid = Pid(std::process::id());
    if args.iter().any(|arg| arg == "--listener") {
        write_message(
            &mut messages,
            &ListenerToCore::Ready {
                outpost_pid: own_pid,
                target_pid: Pid(0),
            },
        )
        .expect("a message is written");
        loop {
            match read_message::<_, CoreToListener>(&mut commands) {
                Ok(Some(CoreToListener::Ping { seq, .. })) => write_message(
                    &mut messages,
                    &ListenerToCore::Pong {
                        seq,
                        parked_count: 0,
                        written_at_us: 0,
                    },
                )
                .expect("a message is written"),
                Ok(Some(CoreToListener::Shutdown) | None) => std::process::exit(0),
                Ok(Some(_)) => {}
                Err(error) => panic!("the command pipe failed: {error}"),
            }
        }
    }
    let mut send = |message: &OutpostToCore| {
        write_message(&mut messages, message).expect("a message is written");
    };
    match value(args, "--target-pid") {
        Some(target) => send(&OutpostToCore::Ready {
            outpost_pid: own_pid,
            target_pid: Pid(target.parse().expect("a pid")),
        }),
        None => send(&OutpostToCore::SpareReady {
            outpost_pid: own_pid,
        }),
    }
    loop {
        match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(CoreToOutpost::Ping { seq, .. })) => send(&OutpostToCore::Pong {
                seq,
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(CoreToOutpost::Assign { target_pid })) => send(&OutpostToCore::Ready {
                outpost_pid: own_pid,
                target_pid,
            }),
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// A supervisor running this binary's stand-ins, delivering to `sink`.
fn supervisor(sink: EventSink) -> Supervisor {
    let exe = std::env::current_exe().expect("this test binary's path");
    Supervisor::with_executables(
        sink,
        OutpostOptions::default(),
        Executables::both(exe),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts")
}

/// The next message from the supervisor, which must come within [`WAIT`].
fn next(events: &Delivered) -> OutpostMessage {
    events
        .recv_timeout(WAIT)
        .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
}

/// What a message is, for the order asserted: the listener's readiness, an
/// outpost's start or its `Ready`, or a spare's readiness with how many
/// wait.
fn name(message: &OutpostMessage) -> String {
    match message {
        OutpostMessage::ListenerReady { .. } => "listener ready".to_owned(),
        OutpostMessage::Started { .. } => "started".to_owned(),
        OutpostMessage::Event { message, .. }
            if matches!(**message, OutpostToCore::Ready { .. }) =>
        {
            "ready".to_owned()
        }
        OutpostMessage::SpareReady { waiting, .. } => format!("spare ready {waiting}"),
        other => format!("{other:?}"),
    }
}

/// Every child ended cleanly: the listener, Core's own outpost, and the
/// spares.
const ALL_CLEAN: ShutdownSummary = ShutdownSummary {
    clean: 2 + SPARES,
    exited: 0,
    killed: 0,
    killed_on_failure: 0,
};

/// The app's port refuses the post of `ListenerReady`, of which a run has
/// exactly one: the relay keeps it and, once the refusal has happened,
/// delivers it again at its next send, Core's own outpost's start, or at a
/// heartbeat before that, so the app takes it before that start and every
/// spare's readiness, each message once, and the shutdown ends.
fn a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends() {
    let refusal = verbatim_faults::arm(
        verbatim_faults::Fault::PortRefusesPost,
        |message: &OutpostMessage| matches!(message, OutpostMessage::ListenerReady { .. }),
        1,
    );
    let (poster, mut queue) = port_queue::<OutpostMessage>().expect("a port queue");
    let (events_tx, events) = unbounded();
    let forwarder = std::thread::spawn(move || {
        while let Some(message) = queue.recv() {
            if events_tx.send(message).is_err() {
                return;
            }
        }
    });
    let supervisor = supervisor(EventSink::posting(
        poster,
        std::convert::identity,
        std::convert::identity,
    ));
    let events = Delivered::new(events, &supervisor);
    assert!(
        refusal.wait_fired(WAIT),
        "the port refused ListenerReady, which the relay keeps"
    );
    // Core's own outpost's start is the relay's next send, behind the kept
    // message, unless a heartbeat delivered that first.
    supervisor.ensure_spawned(Pid(std::process::id()));
    let taken: Vec<String> = (0..3 + SPARES).map(|_| name(&next(&events))).collect();
    assert_eq!(
        taken,
        [
            "listener ready",
            "started",
            "ready",
            "spare ready 1",
            "spare ready 2"
        ],
        "the refused message first, then the rest in order, each once"
    );

    assert_eq!(supervisor.shutdown(), ALL_CLEAN);
    drop(supervisor);
    forwarder
        .join()
        .expect("the taker ends once the sink has gone");
    let rest: Vec<String> = events.try_iter().map(|message| name(&message)).collect();
    assert!(rest.is_empty(), "nothing more was delivered: {rest:?}");
    drop(refusal);
}

/// A shutdown whose mailbox wake the relay's port refuses still ends: once
/// every spare is ready, every launch has been taken and no thread sends
/// to the owner, so the shutdown's message goes into an empty queue and
/// its wake is the one refused; the relay's heartbeat, up to one ping
/// interval later, takes it, and every child is ended.
fn a_shutdown_whose_wake_the_port_refuses_still_ends() {
    let (events_tx, events) = unbounded();
    let supervisor = supervisor(events_tx.into());
    let events = Delivered::new(events, &supervisor);
    supervisor.ensure_spawned(Pid(std::process::id()));
    let mut waiting = 0;
    while waiting < SPARES {
        if let OutpostMessage::SpareReady { waiting: now, .. } = next(&events) {
            waiting = now;
        }
    }

    let refusal = verbatim_supervisor::refuse_shutdown_wake();
    assert_eq!(supervisor.shutdown(), ALL_CLEAN);
    drop(refusal);
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // The tests run with libtest's command line (`libtest-mimic`), so
    // cargo-nextest can list them and run each in a process of its own;
    // under `cargo test` they run one after another on the main thread,
    // since the table of armed fault points is the process's.
    let mut arguments = libtest_mimic::Arguments::from_args();
    arguments.test_threads = Some(1);
    let tests: [(&str, fn()); 2] = [
        (
            "a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends",
            a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends,
        ),
        (
            "a_shutdown_whose_wake_the_port_refuses_still_ends",
            a_shutdown_whose_wake_the_port_refuses_still_ends,
        ),
    ];
    let trials = tests
        .into_iter()
        .map(|(name, test)| {
            libtest_mimic::Trial::test(name, move || {
                test();
                Ok(())
            })
        })
        .collect();
    libtest_mimic::run(&arguments, trials).exit_code()
}
