//! A process Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names in `VERBATIM_IGNORE_PIDS`: the owner's own
//! Windows Terminal) gets no outpost: the listener is told its pid, and
//! Core's start of an outpost for it starts none and says so
//! (`LaunchFailed`, the application counted as gone), while a start for
//! another application, after it, starts one as usual.
//!
//! This test binary runs itself four ways: as two applications, processes
//! with no windows that run until their standard input closes, one of them
//! ignored; as a stand-in for the focus listener, which checks it was told
//! the ignored pid, says it is ready, answers pings and shuts down when
//! asked; and as a stand-in for the outpost binary, which says it is
//! ready, answers pings, and shuts down when asked. Nothing makes a UI
//! Automation or `WinEvent` call, so the test touches nothing on the
//! desktop. Its own `main` recognizes each command line.

use std::io::{BufReader, Read as _};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use verbatim_process::transport::{BlockingReader, BlockingWriter};

use crossbeam_channel::unbounded;

mod common;
use common::Delivered;
use verbatim_model::Pid;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::message::{read_message, write_message};
use verbatim_supervisor::{Executables, OutpostMessage, ShutdownSummary, Supervisor};

/// An application's argument.
const APPLICATION: &str = "--stand-in-application";

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

/// A stand-in child: both answer pings and exit cleanly for the shutdown
/// message or the end of the command pipe; the listener only once it was
/// told the processes to ignore.
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
    let listener = args.iter().any(|arg| arg == "--listener");
    let mut commands = BufReader::new(commands);
    if listener {
        let mut send = |message: &ListenerToCore| {
            write_message(&mut messages, message).expect("a message is written");
        };
        send(&ListenerToCore::Ready {
            outpost_pid: Pid(std::process::id()),
        });
        let told = value(args, "--ignore-pids").is_some();
        loop {
            match read_message::<_, CoreToListener>(&mut commands) {
                Ok(Some(CoreToListener::Ping { .. })) => {
                    send(&ListenerToCore::Pong { written_at_us: 0 });
                }
                Ok(Some(CoreToListener::Shutdown) | None) => {
                    std::process::exit(if told { 0 } else { 3 })
                }
                Ok(Some(_)) => {}
                Err(error) => panic!("the command pipe failed: {error}"),
            }
        }
    }
    let target_pid = Pid(value(args, "--target-pid")
        .and_then(|pid| pid.parse().ok())
        .expect("an outpost is told its application's pid"));
    let mut send = |message: &OutpostToCore| {
        write_message(&mut messages, message).expect("a message is written");
    };
    send(&OutpostToCore::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    });
    loop {
        match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(CoreToOutpost::Ping { .. })) => send(&OutpostToCore::Pong {
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// The next message from the supervisor, which must come within [`WAIT`].
fn next(events: &Delivered) -> OutpostMessage {
    events
        .recv_timeout(WAIT)
        .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
}

/// Starts a stand-in application.
fn application(exe: &std::path::Path) -> Child {
    Command::new(exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts")
}

/// Ends a stand-in application, which must exit cleanly.
fn end(mut application: Child) {
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
}

fn an_ignored_process_gets_no_outpost() {
    let exe = std::env::current_exe().expect("this test binary's path");
    let ignored_application = application(&exe);
    let other_application = application(&exe);
    let ignored_pid = Pid(ignored_application.id());
    let other_pid = Pid(other_application.id());
    let (events_tx, events) = unbounded();
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe),
        LIMIT,
        Arc::new(IgnoredProcesses::hold([ignored_pid])),
        false,
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    assert!(supervisor.is_ignored(ignored_pid));
    assert!(!supervisor.is_ignored(other_pid));
    match next(&events) {
        OutpostMessage::ListenerReady { .. } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }
    supervisor.start_outpost(ignored_pid);
    match next(&events) {
        OutpostMessage::LaunchFailed {
            target_pid,
            application_gone: true,
            ..
        } if target_pid == ignored_pid => {}
        other => panic!("the supervisor said {other:?}, not that it started no outpost"),
    }
    supervisor.start_outpost(other_pid);
    match next(&events) {
        OutpostMessage::Started { target_pid, .. } if target_pid == other_pid => {}
        other => panic!("the supervisor said {other:?}, not that the other's outpost started"),
    }
    match next(&events) {
        OutpostMessage::Event { pid, message, .. }
            if pid == other_pid && matches!(*message, OutpostToCore::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the other's outpost was ready"),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 2,
            exited: 0,
            killed: 0,
            killed_on_failure: 0,
        },
        "the listener, told the ignored application, and the other application's outpost shut down cleanly"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
    end(ignored_application);
    end(other_application);
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == APPLICATION) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return std::process::ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // The tests run with libtest's command line (`libtest-mimic`), so
    // cargo-nextest can list them and run each in a process of its own;
    // under `cargo test` they run one after another on the main thread,
    // since each test's children inherit this process's environment.
    let mut arguments = libtest_mimic::Arguments::from_args();
    arguments.test_threads = Some(1);
    let tests: [(&str, fn()); 1] = [(
        "an_ignored_process_gets_no_outpost",
        an_ignored_process_gets_no_outpost,
    )];
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
