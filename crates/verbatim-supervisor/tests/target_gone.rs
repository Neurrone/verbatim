//! The supervisor never starts or replaces an outpost for an application
//! that has exited: an outpost that crashes after its application exited
//! ends as the application's exit (`TargetExited`) and is not replaced,
//! though the application held attention, and asking for an outpost for it
//! afterwards starts none and says so (`NotWatched`). And an outpost's end
//! that the app refuses for now, as a completion port refusing its packet does, is not
//! lost: it reaches the app later, still before its replacement's start.
//!
//! This test binary runs itself three ways: as the application, a process
//! with no windows that runs until its standard input closes; as a stand-in
//! for the outpost binary, which says it is ready, answers pings, shuts down
//! when asked, and crashes (exits with code 3) when sent anything else; and
//! as a stand-in for the focus listener, which says it is ready, answers an
//! application's exit, and shuts down when asked, cleanly only if it was
//! told of one where a test names it ([`EXIT_TOLD`]). Nothing makes a UI Automation or `WinEvent` call, so the
//! test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::io::{BufReader, Read as _};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use verbatim_process::transport::{BlockingReader, BlockingWriter};

use crossbeam_channel::unbounded;

mod common;
use common::Delivered;
use verbatim_model::protocol::{
    CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore, RequestId,
};
use verbatim_model::{OutpostId, Pid};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::{read_message, write_message};
use verbatim_supervisor::{EndReason, Executables, OutpostMessage, ShutdownSummary, Supervisor};
use verbatim_supervisor::{EventSink, Undelivered};

/// The application's argument.
const APPLICATION: &str = "--stand-in-application";

/// Set for a test whose listener stand-in shuts down cleanly only if it
/// was told an application's exit.
const EXIT_TOLD: &str = "VERBATIM_TEST_EXIT_TOLD";

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
    let outpost = !args.iter().any(|arg| arg == "--listener");
    let mut commands = BufReader::new(commands);
    if !outpost {
        write_message(
            &mut messages,
            &ListenerToCore::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(0),
            },
        )
        .expect("a message is written");
        let exit_wanted = std::env::var_os(EXIT_TOLD).is_some();
        let mut told = false;
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
                Ok(Some(CoreToListener::ApplicationExited { pid })) => {
                    told = true;
                    write_message(&mut messages, &ListenerToCore::ExitHandled { pid })
                        .expect("a message is written");
                }
                Ok(Some(CoreToListener::Shutdown) | None) => {
                    std::process::exit(if exit_wanted && !told { 3 } else { 0 })
                }
                Ok(Some(_)) => {}
                Err(error) => panic!("the command pipe failed: {error}"),
            }
        }
    }
    let target_pid = Pid(value(args, "--target-pid").parse().expect("a pid"));
    let mut send = |message: &OutpostToCore| {
        write_message(&mut messages, message).expect("a message is written");
    };
    send(&OutpostToCore::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    });
    loop {
        match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(CoreToOutpost::Ping { seq, .. })) => send(&OutpostToCore::Pong {
                seq,
                parked_count: 0,
                parked_on_hung: 0,
                written_at_us: 0,
            }),
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) => std::process::exit(3),
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

fn an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited() {
    // SAFETY: set before the supervisor starts its first child, and read
    // only by those children; no other thread reads the environment.
    unsafe { std::env::set_var(EXIT_TOLD, "1") };
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe.clone()),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    match next(&events) {
        OutpostMessage::ListenerReady {
            replacement: false, ..
        } => {}
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
            ..
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    };
    match next(&events) {
        OutpostMessage::Event { message, .. }
            if matches!(*message, OutpostToCore::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the outpost was ready"),
    }

    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
    supervisor
        .send_to_outpost(
            outpost,
            CoreToOutpost::Cancel {
                request_id: RequestId::shell(0),
            },
        )
        .expect("the command that crashes the stand-in is queued");
    match next(&events) {
        OutpostMessage::Ended {
            outpost: ended,
            target_pid: ended_pid,
            reason: EndReason::TargetExited,
            follows: false,
            ..
        } if ended == outpost && ended_pid == target_pid => {}
        other => {
            panic!("the supervisor said {other:?}, not that the outpost ended with its application")
        }
    }
    match next(&events) {
        OutpostMessage::NotWatched {
            target_pid: gone, ..
        } if gone == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that nothing watches the application"),
    }

    supervisor.ensure_spawned(target_pid);
    match next(&events) {
        OutpostMessage::NotWatched {
            target_pid: wanted, ..
        } if wanted == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that it started no outpost"),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 1,
            exited: 1,
            killed: 0,
            killed_on_failure: 0,
        },
        "the listener, told of the application's exit, shut down cleanly and the crashed outpost had exited"
    );
    // SAFETY: as where it is set; no child is left to read it.
    unsafe { std::env::remove_var(EXIT_TOLD) };
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
}

/// Waits for the outpost watching `target_pid` to start and say it is
/// ready, and returns its generation.
fn started_and_ready(events: &Delivered, target_pid: Pid) -> OutpostId {
    let outpost = match next(events) {
        OutpostMessage::Started {
            outpost,
            target_pid: started,
            ..
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    };
    match next(events) {
        OutpostMessage::Event { message, .. }
            if matches!(*message, OutpostToCore::Ready { .. }) => {}
        other => panic!("the supervisor said {other:?}, not that the outpost was ready"),
    }
    outpost
}

/// An outpost's end that the app refuses for now is delivered again, and
/// nothing about its application overtakes it. The sink refuses the first
/// `Ended` once, giving it back as a port that refuses a packet does. The
/// outpost crashes, so the relay starts a replacement at once, the
/// application's first failure, with the end still undelivered; the app
/// hears the end, then the replacement's start and its `Ready`, in that
/// order.
fn an_end_the_app_refuses_for_now_still_comes_before_its_replacement() {
    let (events_tx, events) = unbounded();
    let refused = Arc::new(AtomicBool::new(false));
    let refusing = Arc::clone(&refused);
    let sink = EventSink::new(move |message| {
        if matches!(message, OutpostMessage::Ended { .. }) && !refusing.swap(true, Ordering::SeqCst)
        {
            return Err(Undelivered::Refused(message));
        }
        events_tx.send(message).map_err(|_| Undelivered::Gone)
    });
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        sink,
        OutpostOptions::default(),
        Executables::both(exe.clone()),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    match next(&events) {
        OutpostMessage::ListenerReady {
            replacement: false, ..
        } => {}
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
    let outpost = started_and_ready(&events, target_pid);

    supervisor
        .send_to_outpost(
            outpost,
            CoreToOutpost::Cancel {
                request_id: RequestId::shell(0),
            },
        )
        .expect("the command that crashes the stand-in is queued");
    match next(&events) {
        OutpostMessage::Ended {
            outpost: ended,
            target_pid: ended_pid,
            reason: EndReason::Exited,
            ..
        } if ended == outpost && ended_pid == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that the outpost ended"),
    }
    assert!(
        refused.load(Ordering::SeqCst),
        "the sink refused the end once"
    );
    let replacement = started_and_ready(&events, target_pid);
    assert_ne!(replacement, outpost, "a new generation replaced it");

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 2,
            exited: 1,
            killed: 0,
            killed_on_failure: 0,
        },
        "the listener and the replacement shut down cleanly and the crashed outpost had exited"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more: {rest:?}"
    );
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
}

/// Crashes `outpost`, the stand-in, and waits for its end.
fn crash(supervisor: &Supervisor, events: &Delivered, outpost: OutpostId, target_pid: Pid) {
    supervisor
        .send_to_outpost(
            outpost,
            CoreToOutpost::Cancel {
                request_id: RequestId::shell(0),
            },
        )
        .expect("the command that crashes the stand-in is queued");
    match next(events) {
        OutpostMessage::Ended {
            outpost: ended,
            target_pid: ended_pid,
            reason: EndReason::Exited,
            ..
        } if ended == outpost && ended_pid == target_pid => {}
        other => panic!("the supervisor said {other:?}, not that the outpost ended"),
    }
}

/// An application whose outpost failed a second time, and whose
/// replacement waits out the backoff, exits meanwhile: when the backoff
/// ends, the supervisor finds the application gone as it decides, starts
/// nothing for it and says nothing watches it, before its sweep, half a
/// minute apart, would find it. The backoffs are judged by a clock the test
/// moves, past the second failure's backoff only once the application has
/// exited, so the order does not depend on how quickly it exits.
fn a_replacement_waiting_out_its_backoff_is_not_started_for_an_application_that_exited() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe.clone()),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    match next(&events) {
        OutpostMessage::ListenerReady {
            replacement: false, ..
        } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }
    let start = verbatim_process::clock::now_us();
    let moved = Arc::new(AtomicU64::new(0));
    let clock_moved = Arc::clone(&moved);
    supervisor.set_backoff_clock(Arc::new(move || start + clock_moved.load(Ordering::SeqCst)));

    let mut application = Command::new(&exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let target_pid = Pid(application.id());
    supervisor.note_views(Some(target_pid), std::collections::BTreeSet::new());
    supervisor.ensure_spawned(target_pid);
    let outpost = started_and_ready(&events, target_pid);
    // The first failure is replaced at once; the second waits a second.
    crash(&supervisor, &events, outpost, target_pid);
    let replacement = started_and_ready(&events, target_pid);
    crash(&supervisor, &events, replacement, target_pid);
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
    // The second failure's backoff, a second, ends; the views wake the
    // supervisor to decide at once.
    moved.store(1_000_000, Ordering::SeqCst);
    supervisor.note_views(Some(target_pid), std::collections::BTreeSet::new());
    match next(&events) {
        OutpostMessage::NotWatched {
            target_pid: gone, ..
        } if gone == target_pid => {}
        other => panic!(
            "the supervisor said {other:?}, not that nothing watches the application that exited"
        ),
    }

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 1,
            exited: 2,
            killed: 0,
            killed_on_failure: 0,
        },
        "the listener shut down cleanly and both crashed outposts had exited"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more, and started no outpost: {rest:?}"
    );
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
    let tests: [(&str, fn()); 3] = [
        (
            "an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited",
            an_outpost_is_never_started_or_replaced_for_an_application_that_has_exited,
        ),
        (
            "an_end_the_app_refuses_for_now_still_comes_before_its_replacement",
            an_end_the_app_refuses_for_now_still_comes_before_its_replacement,
        ),
        (
            "a_replacement_waiting_out_its_backoff_is_not_started_for_an_application_that_exited",
            a_replacement_waiting_out_its_backoff_is_not_started_for_an_application_that_exited,
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
