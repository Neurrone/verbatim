//! The relay's part of the harness's idle barrier
//! (`docs/design/communication.md`, section 6.7): a round asks the
//! listener first, then every outpost behind every command sent to it
//! before, telling the app which outposts it asked before it asks them; a
//! listener that ends during the round counts as having answered; and an
//! outpost the round finds still being launched or starting is asked too,
//! once it is ready, behind the commands sent to it before.
//!
//! This test binary runs itself three ways: as an application, a process
//! with no windows that runs until its standard input closes; as a
//! stand-in for the outpost binary; and as a stand-in for the focus
//! listener. Nothing makes a UI Automation or `WinEvent` call, so the test
//! touches nothing on the desktop. Its own `main` recognizes each command
//! line.
//!
//! The stand-in outpost answers pings, says it is ready only when Core
//! first sends it a `Cancel` of request 0, so the test decides when,
//! counts every other cancel as a command, and answers each idle query
//! with a fault saying how many commands reached it since the query
//! before, then the answer. The stand-in listener closes its pipe to Core
//! on round 1's query, which ends it as Core sees it, and exits when Core
//! then shuts it down, so it is counted as shut down cleanly however soon
//! Core looks; it answers any later round's.

use std::io::{BufReader, Read as _};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crossbeam_channel::unbounded;

mod common;
use common::Delivered;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_model::request::{Cancel, CancelReason, RequestNumber};
use verbatim_model::{OutpostId, Pid};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::{read_message, write_message};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use verbatim_supervisor::{Executables, OutpostMessage, ShutdownSummary, Supervisor};

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

/// A stand-in child, as the module says.
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
    let (commands, messages) = (
        BlockingReader::new(commands).expect("a reader"),
        std::cell::RefCell::new(Some(BlockingWriter::new(messages).expect("a writer"))),
    );
    let listener = args.iter().any(|arg| arg == "--listener");
    let mut commands = BufReader::new(commands);
    if listener {
        listener_stand_in(&mut commands, &messages);
    }
    let target_pid = Pid(value(args, "--target-pid").parse().expect("a pid"));
    // Nothing is sent once the pipe to Core is closed.
    let send = |message: &OutpostToCore| {
        if let Some(messages) = messages.borrow_mut().as_mut() {
            write_message(messages, message).expect("a message is written");
        }
    };
    let ready = OutpostToCore::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    };
    let mut commands_since = 0;
    loop {
        let command = match read_message::<_, CoreToOutpost>(&mut commands) {
            Ok(Some(command)) => command,
            Ok(None) => std::process::exit(0),
            Err(error) => panic!("the command pipe failed: {error}"),
        };
        match command {
            CoreToOutpost::Ping { .. } => send(&OutpostToCore::Pong {
                parked_count: 0,
                written_at_us: 0,
            }),
            CoreToOutpost::Shutdown => std::process::exit(0),
            CoreToOutpost::Cancel(Cancel {
                number: RequestNumber(0),
                ..
            }) => send(&ready),
            CoreToOutpost::Cancel(_) => commands_since += 1,
            CoreToOutpost::IdleQuery { round } => {
                send(&OutpostToCore::Fault {
                    detail: format!("commands before round {round}: {commands_since}"),
                });
                commands_since = 0;
                send(&OutpostToCore::IdleAnswer { round });
            }
            _ => {}
        }
    }
}

/// The stand-in listener: says it is ready, answers pings, closes its
/// pipe to Core at round 1, and answers every later round.
fn listener_stand_in(
    commands: &mut BufReader<BlockingReader>,
    messages: &std::cell::RefCell<Option<BlockingWriter>>,
) -> ! {
    // Nothing is sent once the pipe to Core is closed.
    let send = |message: &ListenerToCore| {
        if let Some(messages) = messages.borrow_mut().as_mut() {
            write_message(messages, message).expect("a message is written");
        }
    };
    send(&ListenerToCore::Ready {
        outpost_pid: Pid(std::process::id()),
    });
    loop {
        let command = match read_message::<_, CoreToListener>(commands) {
            Ok(Some(command)) => command,
            Ok(None) => std::process::exit(0),
            Err(error) => panic!("the command pipe failed: {error}"),
        };
        match command {
            CoreToListener::Ping { .. } => send(&ListenerToCore::Pong { written_at_us: 0 }),
            CoreToListener::Shutdown => std::process::exit(0),
            CoreToListener::IdleQuery { round: 1 } => {
                drop(messages.borrow_mut().take());
            }
            CoreToListener::IdleQuery { round } => {
                send(&ListenerToCore::IdleAnswer { round });
            }
            CoreToListener::ApplicationExited { .. }
            | CoreToListener::Request(_)
            | CoreToListener::Cancel(_)
            | CoreToListener::ElementsHeld { .. } => {}
        }
    }
}

/// The next message from the supervisor, which must come within [`WAIT`].
fn next(events: &Delivered) -> OutpostMessage {
    events
        .recv_timeout(WAIT)
        .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
}

/// Starts an application, a process of this binary's that runs until its
/// standard input closes.
fn application() -> (Child, Pid) {
    let exe = std::env::current_exe().expect("this test binary's path");
    let child = Command::new(exe)
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let pid = Pid(child.id());
    (child, pid)
}

/// Ends an application started by [`application`].
fn end(mut application: Child) {
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
}

/// The outpost the supervisor says it started for `target_pid`, which must
/// be the next thing it says.
fn started(events: &Delivered, target_pid: Pid) -> OutpostId {
    match next(events) {
        OutpostMessage::Started {
            outpost,
            target_pid: started,
            ..
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    }
}

/// Asserts the next thing the supervisor says is `expected` from
/// `outpost`.
fn from_outpost(events: &Delivered, outpost: OutpostId, expected: &OutpostToCore) {
    match next(events) {
        OutpostMessage::Event {
            outpost: from,
            message,
            ..
        } if from == outpost && *message == *expected => {}
        other => panic!("the supervisor said {other:?}, not {expected:?} from {outpost}"),
    }
}

/// Asserts the next thing the supervisor says is that it asked exactly
/// `outposts` for round `round`.
fn queried(events: &Delivered, round: u64, outposts: &[OutpostId]) {
    match next(events) {
        OutpostMessage::IdleQueried {
            round: asked,
            outposts: mut which,
            ..
        } if asked == round => {
            which.sort_unstable();
            assert_eq!(which, outposts, "the outposts round {round} asked");
        }
        other => panic!("the supervisor said {other:?}, not that it asked for round {round}"),
    }
}

/// The outpost's fault saying how many commands reached it before
/// `round`.
fn commands_before(commands: u32, round: u64) -> OutpostToCore {
    OutpostToCore::Fault {
        detail: format!("commands before round {round}: {commands}"),
    }
}

/// Sends `outpost` a command the stand-in counts.
fn command(supervisor: &Supervisor, outpost: OutpostId) {
    supervisor
        .send_to_outpost(
            outpost,
            CoreToOutpost::Cancel(Cancel {
                number: RequestNumber(1),
                reason: CancelReason::Withdrawn,
            }),
        )
        .expect("the command is queued");
}

/// Has `outpost`, a stand-in for `target_pid` waiting for its cue, say it
/// is ready, which must be the next thing the supervisor says.
fn make_ready(supervisor: &Supervisor, events: &Delivered, outpost: OutpostId, target_pid: Pid) {
    supervisor
        .send_to_outpost(
            outpost,
            CoreToOutpost::Cancel(Cancel {
                number: RequestNumber(0),
                reason: CancelReason::Withdrawn,
            }),
        )
        .expect("the stand-in's cue is queued");
    match next(events) {
        OutpostMessage::Event {
            outpost: from,
            message,
            ..
        } if from == outpost
            && matches!(*message, OutpostToCore::Ready { target_pid: ready, .. } if ready == target_pid) =>
            {}
        other => panic!("the supervisor said {other:?}, not that {outpost} was ready"),
    }
}

fn a_round_asks_the_listener_then_every_outpost_behind_the_commands_before_it() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executables(
        events_tx,
        OutpostOptions::default(),
        Executables::both(exe),
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    let events = Delivered::new(events, &supervisor);
    match next(&events) {
        OutpostMessage::ListenerReady { .. } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }

    // Round 1: the listener's pipe ends on the query, which counts as its
    // answer; there is no outpost to ask. Core hears it ended, and its
    // replacement starts after.
    supervisor.query_idle(1);
    match next(&events) {
        OutpostMessage::ListenerEnded { listener, .. } => {
            assert_eq!(listener, OutpostId(1), "the first listener ended");
        }
        other => panic!("the supervisor said {other:?}, not that the listener ended"),
    }
    queried(&events, 1, &[]);
    match next(&events) {
        OutpostMessage::ListenerReady { .. } => {}
        other => panic!("the supervisor said {other:?}, not that the listener was replaced"),
    }

    // Round 2: an outpost that is ready is asked behind the command sent
    // to it before the round.
    let (first_app, first_pid) = application();
    supervisor.start_outpost(first_pid);
    let first = started(&events, first_pid);
    make_ready(&supervisor, &events, first, first_pid);
    command(&supervisor, first);
    supervisor.query_idle(2);
    queried(&events, 2, &[first]);
    from_outpost(&events, first, &commands_before(1, 2));
    from_outpost(&events, first, &OutpostToCore::IdleAnswer { round: 2 });

    // Round 3: an outpost Core starts just before the round is asked once
    // its launch has reported, and only once it is ready, behind the
    // command sent to it before.
    let (second_app, second_pid) = application();
    supervisor.start_outpost(second_pid);
    supervisor.query_idle(3);
    let second = started(&events, second_pid);
    queried(&events, 3, &[first, second]);
    from_outpost(&events, first, &commands_before(0, 3));
    from_outpost(&events, first, &OutpostToCore::IdleAnswer { round: 3 });
    command(&supervisor, second);
    make_ready(&supervisor, &events, second, second_pid);
    from_outpost(&events, second, &commands_before(1, 3));
    from_outpost(&events, second, &OutpostToCore::IdleAnswer { round: 3 });

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 4,
            exited: 0,
            killed: 0,
            killed_on_failure: 0,
        },
        "both listeners and both outposts ended cleanly, the first listener once its pipe had closed"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more: {rest:?}"
    );
    end(first_app);
    end(second_app);
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
        "a_round_asks_the_listener_then_every_outpost_behind_the_commands_before_it",
        a_round_asks_the_listener_then_every_outpost_behind_the_commands_before_it,
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
