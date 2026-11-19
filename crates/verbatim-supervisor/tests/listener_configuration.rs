//! The listener's configuration (`CoreToListener::Configuration`): the
//! owner launches every listener generation with the value Core last
//! configured, since it relaunches the listener without Core, and writes a
//! command that arrives while a generation launches after that launch, so
//! the value a generation judges under is always Core's last. The launch
//! is held at a fault point to send a command while it is at work, so only
//! `cargo xtask faults` builds this test.
//!
//! This test binary runs itself as a stand-in for the focus listener,
//! which says it is ready, reports the value its command line carries and
//! each configuration command it reads as window destructions, a code the
//! test reads (101 for a launch with background progress bars reported,
//! 100 for one without; 201 and 200 for the commands), answers pings and
//! shuts down when asked; its first generation exits once it has reported
//! a command, so the owner relaunches it. Nothing makes a UI Automation or
//! `WinEvent` call, so the test touches nothing on the desktop. Its own
//! `main` recognizes each command line.

use std::io::BufReader;
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_faults::Fault;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_model::{OutpostId, Pid, WindowHandle};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::{read_message, write_message};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use verbatim_supervisor::{Executables, OutpostMessage, Supervisor};

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

/// The stand-in's report of `code`, as a window's destruction.
fn report(code: u64) -> ListenerToCore {
    ListenerToCore::Destroyed {
        window: WindowHandle(code),
        process: None,
        at_us: 0,
    }
}

/// The code of a background bars value, `base` plus one when reported.
fn code(base: u64, reported: bool) -> u64 {
    base + u64::from(reported)
}

/// A stand-in child: the listener says `Ready`, reports its launch value
/// and each configuration command; a spare says `SpareReady`. Each answers
/// pings, and exits cleanly for the shutdown message or the end of its
/// command pipe; the first listener generation also exits after its first
/// command.
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
    let (commands, mut messages) = (
        BlockingReader::new(commands).expect("a reader"),
        BlockingWriter::new(messages).expect("a writer"),
    );
    let mut commands = BufReader::new(commands);
    let own_pid = Pid(std::process::id());
    if args.iter().any(|arg| arg == "--listener") {
        let launched = match value(args, "--background-bars").as_deref() {
            Some("on") => true,
            Some("off") => false,
            other => panic!("a listener is launched with background bars on or off: {other:?}"),
        };
        let first = value(args, "--generation").as_deref() == Some("1");
        let mut send = |message: &ListenerToCore| {
            write_message(&mut messages, message).expect("a message is written");
        };
        send(&ListenerToCore::Ready {
            outpost_pid: own_pid,
            target_pid: Pid(0),
        });
        send(&report(code(100, launched)));
        loop {
            match read_message::<_, CoreToListener>(&mut commands) {
                Ok(Some(CoreToListener::Ping { .. })) => send(&ListenerToCore::Pong {
                    parked_count: 0,
                    written_at_us: 0,
                }),
                Ok(Some(CoreToListener::Configuration {
                    report_background_progress_bars,
                })) => {
                    send(&report(code(200, report_background_progress_bars)));
                    if first {
                        std::process::exit(0);
                    }
                }
                Ok(Some(CoreToListener::Shutdown) | None) => std::process::exit(0),
                Ok(Some(_)) => {}
                Err(error) => panic!("the command pipe failed: {error}"),
            }
        }
    }
    write_message(
        &mut messages,
        &OutpostToCore::SpareReady {
            outpost_pid: own_pid,
        },
    )
    .expect("a message is written");
    loop {
        match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(CoreToOutpost::Ping { .. })) => write_message(
                &mut messages,
                &OutpostToCore::Pong {
                    parked_count: 0,
                    parked_on_hung: 0,
                    written_at_us: 0,
                },
            )
            .expect("a message is written"),
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// The next of the listener's reports, by its generation, within
/// [`WAIT`]; every other message is passed by.
fn next_report(events: &Delivered) -> (OutpostId, u64) {
    loop {
        if let OutpostMessage::Listener { listener, message } = events
            .recv_timeout(WAIT)
            .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
        {
            match *message {
                ListenerToCore::Destroyed { window, .. } => return (listener, window.0),
                other => panic!("the stand-in sent {other:?}"),
            }
        }
    }
}

/// A supervisor of this binary's stand-ins, its first listener launched
/// with background progress bars not reported, and what it delivers.
fn supervisor() -> (Supervisor, Delivered) {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe),
        LIMIT,
        std::sync::Arc::default(),
        false,
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    (supervisor, events)
}

/// Shuts `supervisor` down and checks that the listener reported nothing
/// more.
fn finish(supervisor: &Supervisor, events: &Delivered) {
    let _ = supervisor.shutdown();
    let more: Vec<OutpostMessage> = events
        .try_iter()
        .filter(|message| matches!(message, OutpostMessage::Listener { .. }))
        .collect();
    assert!(more.is_empty(), "nothing more was reported: {more:?}");
}

/// Launched with background bars not reported, the first listener says
/// so; configured on, it reads the command and exits; its replacement is
/// launched with the last value, on, and a command sent once it has
/// reported its launch, off, is written to it as it runs.
fn a_relaunch_carries_the_last_configuration() {
    let (supervisor, events) = supervisor();
    let first = next_report(&events);
    supervisor.configure_listener(true);
    let before = [first, next_report(&events), next_report(&events)];
    supervisor.configure_listener(false);
    let replacement = OutpostId(2);
    assert_eq!(
        [before[0], before[1], before[2], next_report(&events)],
        [
            (OutpostId(1), 100),
            (OutpostId(1), 201),
            (replacement, 101),
            (replacement, 200),
        ]
    );
    finish(&supervisor, &events);
}

/// A command sent while the listener launches is held and written after
/// the launch: the replacement's launch is held at the point where its
/// helper reports it ([`Fault::ListenerLaunchReported`]), the command is
/// sent there, ahead of the launch's report in the owner's mailbox, so the
/// owner takes it while the listener is still launching; released, the
/// replacement reads its launch value, on, then the held command, off.
fn a_command_sent_while_the_listener_launches_is_written_after_the_launch() {
    let (supervisor, events) = supervisor();
    let replacement = OutpostId(2);
    let held = verbatim_faults::arm(
        Fault::ListenerLaunchReported,
        move |generation: &u64| *generation == replacement.0,
        1,
    );
    let first = next_report(&events);
    supervisor.configure_listener(true);
    let configured = next_report(&events);
    assert!(held.wait_fired(WAIT), "the replacement's launch is held");
    supervisor.configure_listener(false);
    drop(held);
    assert_eq!(
        [
            first,
            configured,
            next_report(&events),
            next_report(&events)
        ],
        [
            (OutpostId(1), 100),
            (OutpostId(1), 201),
            (replacement, 101),
            (replacement, 200),
        ]
    );
    finish(&supervisor, &events);
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // One test at a time: the held point is process-wide.
    let mut arguments = libtest_mimic::Arguments::from_args();
    arguments.test_threads = Some(1);
    let tests: [(&str, fn()); 2] = [
        (
            "a_relaunch_carries_the_last_configuration",
            a_relaunch_carries_the_last_configuration,
        ),
        (
            "a_command_sent_while_the_listener_launches_is_written_after_the_launch",
            a_command_sent_while_the_listener_launches_is_written_after_the_launch,
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
