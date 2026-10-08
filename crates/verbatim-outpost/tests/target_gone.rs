//! The supervisor never starts or replaces an outpost for an application
//! that has exited: an outpost that crashes after its application exited
//! ends as the application's exit (`TargetExited`) and is not replaced,
//! though the application held attention, and asking for an outpost for it
//! afterwards starts none and says so (`NotWatched`). Likewise, once an
//! application's outposts have crashed three times within a minute, the
//! supervisor says so after the third crash, since the app may be waiting
//! for a replacement, and again whenever an outpost is asked for, until
//! the next foreground change to it.
//!
//! This test binary runs itself three ways: as the application, a process
//! with no windows that runs until its standard input closes; as a stand-in
//! for the outpost binary, which says it is ready, answers pings, shuts down
//! when asked, and crashes (exits with code 3) when sent anything else; and
//! as a stand-in for the focus listener, which says it is ready and shuts
//! down when asked. Nothing makes a UI Automation or `WinEvent` call, so the
//! test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::io::{BufReader, Read as _};
use std::process::{Command, Stdio};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use verbatim_model::{OutpostId, Pid};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    OutpostToSupervisor, SupervisorToOutpost, read_message, write_message,
};
use verbatim_outpost::supervisor::{EndReason, OutpostMessage, ShutdownSummary, Supervisor};

/// The application's argument.
const APPLICATION: &str = "--stand-in-application";

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// The value after `name` on the command line.
fn value(args: &[String], name: &str) -> String {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| panic!("the command line names {name}: {args:?}"))
}

/// A stand-in child: `Ready`, then pongs for pings, a clean exit for the
/// shutdown message or the end of the command pipe, and, for an outpost,
/// a crash for anything else.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .parse()
            .unwrap_or_else(|_| panic!("{name} is a handle value"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (commands, mut messages) =
        unsafe { verbatim_process::inherited_pipes(handle("--pipe-in"), handle("--pipe-out")) }
            .expect("the inherited pipes");
    let outpost = !args.iter().any(|arg| arg == "--listener");
    let target_pid = if outpost {
        Pid(value(args, "--target-pid").parse().expect("a pid"))
    } else {
        Pid(0)
    };
    let mut send = |message: &OutpostToSupervisor| {
        write_message(&mut messages, message).expect("a message is written");
    };
    send(&OutpostToSupervisor::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    });
    let mut commands = BufReader::new(commands);
    loop {
        match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(SupervisorToOutpost::Ping { seq })) => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
            }),
            Ok(Some(SupervisorToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) if outpost => std::process::exit(3),
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

fn an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executable(
        events_tx,
        OutpostOptions::default(),
        exe.clone(),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    match next(&events) {
        OutpostMessage::ListenerReady { replacement: false } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }

    let mut application = Command::new(&exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let target_pid = Pid(application.id());
    supervisor.note_views(Some(target_pid), std::collections::BTreeSet::new());
    supervisor.ensure_spawned(target_pid);
    let outpost = match next(&events) {
        OutpostMessage::Started {
            outpost,
            target_pid: started,
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    };
    match next(&events) {
        OutpostMessage::Event { message, .. }
            if matches!(*message, OutpostToSupervisor::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the outpost was ready"),
    }

    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
    supervisor
        .send_to_outpost(outpost, SupervisorToOutpost::Cancel { request_id: 0 })
        .expect("the command that crashes the stand-in is queued");
    match next(&events) {
        OutpostMessage::Ended {
            outpost: ended,
            target_pid: ended_pid,
            reason: EndReason::TargetExited,
        } if ended == outpost && ended_pid == target_pid => {}
        other => {
            panic!("the supervisor said {other:?}, not that the outpost ended with its application")
        }
    }

    supervisor.ensure_spawned(target_pid);
    match next(&events) {
        OutpostMessage::NotWatched { target_pid: wanted } if wanted == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that it started no outpost"),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 1,
            exited: 1,
            killed: 0,
        },
        "the listener shut down cleanly and the crashed outpost had exited"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
}

/// Waits for the outpost watching `target_pid` to start and say it is
/// ready, and returns its incarnation.
fn started_and_ready(events: &Receiver<OutpostMessage>, target_pid: Pid) -> OutpostId {
    let outpost = match next(events) {
        OutpostMessage::Started {
            outpost,
            target_pid: started,
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    };
    match next(events) {
        OutpostMessage::Event { message, .. }
            if matches!(*message, OutpostToSupervisor::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the outpost was ready"),
    }
    outpost
}

fn an_application_whose_outposts_crashed_repeatedly_is_reported_not_watched() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor =
        Supervisor::with_executable(events_tx, OutpostOptions::default(), exe.clone(), LIMIT)
            .expect("the supervisor starts");
    match next(&events) {
        OutpostMessage::ListenerReady { replacement: false } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }

    let mut application = Command::new(&exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let target_pid = Pid(application.id());
    supervisor.note_views(Some(target_pid), std::collections::BTreeSet::new());
    supervisor.ensure_spawned(target_pid);
    let mut outpost = started_and_ready(&events, target_pid);

    // Three crashes within a minute: the first two are replaced at once,
    // since the application holds attention; after the third, respawning
    // stops, and the app, which heard of the crash, is told no outpost
    // comes.
    for crash in 1..=3 {
        supervisor
            .send_to_outpost(outpost, SupervisorToOutpost::Cancel { request_id: 0 })
            .expect("the command that crashes the stand-in is queued");
        match next(&events) {
            OutpostMessage::Ended {
                outpost: ended,
                target_pid: ended_pid,
                reason: EndReason::Exited,
            } if ended == outpost && ended_pid == target_pid => {}
            other => panic!("crash {crash}: the supervisor said {other:?}, not that it ended"),
        }
        if crash < 3 {
            outpost = started_and_ready(&events, target_pid);
        }
    }
    match next(&events) {
        OutpostMessage::NotWatched { target_pid: wanted } if wanted == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that it replaced no outpost"),
    }

    // Asked for one again, it starts none, and says so.
    supervisor.ensure_spawned(target_pid);
    match next(&events) {
        OutpostMessage::NotWatched { target_pid: wanted } if wanted == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that it started no outpost"),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 1,
            exited: 3,
            killed: 0,
        },
        "the listener shut down cleanly and the three crashed outposts had exited"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
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
    // ignored: the binary runs both of its tests.
    an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited();
    println!(
        "test an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited ... ok"
    );
    an_application_whose_outposts_crashed_repeatedly_is_reported_not_watched();
    println!(
        "test an_application_whose_outposts_crashed_repeatedly_is_reported_not_watched ... ok"
    );
}
