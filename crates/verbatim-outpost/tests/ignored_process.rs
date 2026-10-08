//! A process Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names in `VERBATIM_IGNORE_PIDS`: the owner's own
//! Windows Terminal) gets no outpost: the listener is told its pid, a focus
//! fact from it is routed nowhere, a request to start an outpost for it
//! starts none and says so (`NotWatched`), and a fact from another
//! application, routed after it, is delivered as usual.
//!
//! This test binary runs itself four ways: as two applications, processes
//! with no windows that run until their standard input closes, one of them
//! ignored; as a stand-in for the focus listener, which checks it was told
//! the ignored pid, says it is ready, and sends a focus fact from the
//! ignored application and then one from the other; and as a stand-in for
//! the outpost binary, which says it is ready, answers pings, and shuts
//! down when asked. Nothing makes a UI Automation or `WinEvent` call, so the
//! test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::io::{BufReader, Read as _};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use verbatim_model::{Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    read_message, write_message,
};
use verbatim_outpost::supervisor::{IgnoredProcesses, OutpostMessage, ShutdownSummary, Supervisor};

/// An application's argument.
const APPLICATION: &str = "--stand-in-application";

/// The environment variable that tells the stand-in listener the pid of
/// the application that is not ignored.
const OTHER_APPLICATION: &str = "VERBATIM_TEST_OTHER_APPLICATION";

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

/// A focus fact from `pid`, as the listener forwards one.
fn focus_fact(pid: u32) -> OutpostToSupervisor {
    OutpostToSupervisor::FocusFact {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        timing: EventTiming::default(),
        fact: ListenerFact {
            pid: Pid(pid),
            fact: DeliveredFact::MsaaFocus {
                hwnd: 1,
                id_object: -4,
                id_child: 0,
            },
        },
    }
}

/// A stand-in child: the listener sends its facts after `Ready`; both
/// answer pings and exit cleanly for the shutdown message or the end of the
/// command pipe.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} is a handle value: {args:?}"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (commands, mut messages) =
        unsafe { verbatim_process::inherited_pipes(handle("--pipe-in"), handle("--pipe-out")) }
            .expect("the inherited pipes");
    let listener = args.iter().any(|arg| arg == "--listener");
    let target_pid = if listener {
        Pid(0)
    } else {
        Pid(value(args, "--target-pid")
            .and_then(|pid| pid.parse().ok())
            .expect("an outpost is told its application's pid"))
    };
    let mut send = |message: &OutpostToSupervisor| {
        write_message(&mut messages, message).expect("a message is written");
    };
    send(&OutpostToSupervisor::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    });
    if listener {
        let ignored =
            value(args, "--ignore-pids").expect("the listener is told the pids to ignore");
        let other: u32 = std::env::var(OTHER_APPLICATION)
            .ok()
            .and_then(|pid| pid.parse().ok())
            .expect("the test names the other application");
        for pid in ignored.split(',') {
            send(&focus_fact(pid.parse().expect("a pid")));
        }
        send(&focus_fact(other));
    }
    let mut commands = BufReader::new(commands);
    loop {
        match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(SupervisorToOutpost::Ping { seq })) => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
            }),
            Ok(Some(SupervisorToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// The next message from the supervisor, which must come within [`WAIT`].
fn next(events: &Receiver<OutpostMessage>) -> OutpostMessage {
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

fn an_ignored_process_gets_no_outpost_and_its_facts_go_nowhere() {
    let exe = std::env::current_exe().expect("this test binary's path");
    let ignored_application = application(&exe);
    let other_application = application(&exe);
    let ignored_pid = Pid(ignored_application.id());
    let other_pid = Pid(other_application.id());
    // SAFETY: set before the supervisor starts its first child, and read
    // only by those children; no other thread reads the environment.
    unsafe { std::env::set_var(OTHER_APPLICATION, other_pid.0.to_string()) };

    let (events_tx, events) = unbounded();
    let supervisor = Supervisor::with_executable(
        events_tx,
        OutpostOptions::default(),
        exe,
        LIMIT,
        Arc::new(IgnoredProcesses::hold([ignored_pid])),
    )
    .expect("the supervisor starts");
    assert!(supervisor.is_ignored(ignored_pid));
    assert!(!supervisor.is_ignored(other_pid));
    match next(&events) {
        OutpostMessage::ListenerReady { replacement: false } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }
    // The ignored application's fact came first: an outpost started for it
    // would be announced before the other's.
    match next(&events) {
        OutpostMessage::Started { target_pid, .. } if target_pid == other_pid => {}
        other => panic!("the supervisor said {other:?}, not that the other's outpost started"),
    }
    match next(&events) {
        OutpostMessage::Event { pid, message, .. }
            if pid == other_pid && matches!(*message, OutpostToSupervisor::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the other's outpost was ready"),
    }

    supervisor.ensure_spawned(ignored_pid);
    match next(&events) {
        OutpostMessage::NotWatched { target_pid } if target_pid == ignored_pid => {}
        other => panic!("the supervisor said {other:?}, not that it started no outpost"),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 2,
            exited: 0,
            killed: 0,
        },
        "the listener and the other application's outpost shut down cleanly"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
    end(ignored_application);
    end(other_application);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == APPLICATION) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return;
    }
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // libtest's own options, such as a name filter, are accepted and
    // ignored: the binary holds one test.
    an_ignored_process_gets_no_outpost_and_its_facts_go_nowhere();
    println!("test an_ignored_process_gets_no_outpost_and_its_facts_go_nowhere ... ok");
}
