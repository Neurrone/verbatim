//! The relay's part of the harness's idle barrier
//! (`docs/design/communication.md`, section 6.7): a round asks the
//! listener first, then every outpost behind every fact relayed to it
//! before, telling the app which outposts it asked before it asks them; a
//! listener that ends during the round counts as having answered; and an
//! outpost the round finds still being launched is asked too, once it is
//! ready, behind the facts held for it.
//!
//! This test binary runs itself three ways: as an application, a process
//! with no windows that runs until its standard input closes; as a
//! stand-in for the outpost binary; and as a stand-in for the focus
//! listener. Nothing makes a UI Automation or `WinEvent` call, so the test
//! touches nothing on the desktop. Its own `main` recognizes each command
//! line.
//!
//! The stand-in outpost answers pings, says it is ready only when Core
//! first sends it a `Cancel` of request 0, so the test decides when, and
//! answers each idle query with a fault saying how many facts reached it
//! since the query before, then the answer. The stand-in listener ends
//! itself on round 1's query; on any later round's, it first sends a focus
//! fact for the application the test named by a queued focus whose
//! observation time is that application's pid, if it named one, then the
//! answer.

use std::io::{BufReader, Read as _};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    read_message, write_message,
};
use verbatim_outpost::supervisor::{OutpostMessage, ShutdownSummary, Supervisor};
use verbatim_process::transport::{BlockingReader, BlockingWriter};

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
    let (commands, mut messages) = (
        BlockingReader::new(commands).expect("a reader"),
        BlockingWriter::new(messages).expect("a writer"),
    );
    let listener = args.iter().any(|arg| arg == "--listener");
    let target_pid = if listener {
        Pid(0)
    } else {
        Pid(value(args, "--target-pid").parse().expect("a pid"))
    };
    let mut send = |message: &OutpostToSupervisor| {
        write_message(&mut messages, message).expect("a message is written");
    };
    let ready = OutpostToSupervisor::Ready {
        outpost_pid: Pid(std::process::id()),
        target_pid,
    };
    if listener {
        send(&ready);
    }
    let mut commands = BufReader::new(commands);
    let mut facts = 0;
    let mut fact_for = None;
    loop {
        let command = match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(command)) => command,
            Ok(None) => std::process::exit(0),
            Err(error) => panic!("the command pipe failed: {error}"),
        };
        match command {
            SupervisorToOutpost::Ping { seq, .. } => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
                written_at_us: 0,
            }),
            SupervisorToOutpost::Shutdown => std::process::exit(0),
            SupervisorToOutpost::Cancel { request_id: 0 } if !listener => send(&ready),
            SupervisorToOutpost::DeliverFact { .. } => facts += 1,
            SupervisorToOutpost::FocusQueued { observed_at_ms, .. } => {
                fact_for = Some(Pid(u32::try_from(observed_at_ms).expect("a pid")));
            }
            SupervisorToOutpost::IdleQuery { round: 1 } if listener => std::process::exit(0),
            SupervisorToOutpost::IdleQuery { round } => {
                if let Some(pid) = fact_for.take() {
                    send(&OutpostToSupervisor::FocusFact {
                        trace_id: TraceId::mint(),
                        observed_at_ms: 0,
                        timing: EventTiming::default(),
                        fact: ListenerFact {
                            pid,
                            fact: DeliveredFact::MsaaFocus {
                                hwnd: 1,
                                id_object: -4,
                                id_child: 0,
                            },
                        },
                        win_event: None,
                    });
                }
                if !listener {
                    send(&OutpostToSupervisor::Fault {
                        detail: format!("facts before round {round}: {facts}"),
                    });
                    facts = 0;
                }
                send(&OutpostToSupervisor::IdleAnswer { round });
            }
            _ => {}
        }
    }
}

/// The next message from the supervisor, which must come within [`WAIT`].
fn next(events: &Receiver<OutpostMessage>) -> OutpostMessage {
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
fn started(events: &Receiver<OutpostMessage>, target_pid: Pid) -> OutpostId {
    match next(events) {
        OutpostMessage::Started {
            outpost,
            target_pid: started,
        } if started == target_pid => outpost,
        other => panic!("the supervisor said {other:?}, not that the outpost started"),
    }
}

/// Asserts the next thing the supervisor says is `expected` from
/// `outpost`.
fn from_outpost(
    events: &Receiver<OutpostMessage>,
    outpost: OutpostId,
    expected: &OutpostToSupervisor,
) {
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
fn queried(events: &Receiver<OutpostMessage>, round: u64, outposts: &[OutpostId]) {
    match next(events) {
        OutpostMessage::IdleQueried {
            round: asked,
            outposts: mut which,
        } if asked == round => {
            which.sort_unstable();
            assert_eq!(which, outposts, "the outposts round {round} asked");
        }
        other => panic!("the supervisor said {other:?}, not that it asked for round {round}"),
    }
}

/// The outpost's fault saying how many facts reached it before `round`.
fn facts_before(facts: u32, round: u64) -> OutpostToSupervisor {
    OutpostToSupervisor::Fault {
        detail: format!("facts before round {round}: {facts}"),
    }
}

/// Has `outpost`, a stand-in for `target_pid` waiting for its cue, say it
/// is ready, which must be the next thing the supervisor says.
fn make_ready(
    supervisor: &Supervisor,
    events: &Receiver<OutpostMessage>,
    outpost: OutpostId,
    target_pid: Pid,
) {
    supervisor
        .send_to_outpost(outpost, SupervisorToOutpost::Cancel { request_id: 0 })
        .expect("the stand-in's cue is queued");
    match next(events) {
        OutpostMessage::Event {
            outpost: from,
            message,
            ..
        } if from == outpost
            && matches!(*message, OutpostToSupervisor::Ready { target_pid: ready, .. } if ready == target_pid) =>
            {}
        other => panic!("the supervisor said {other:?}, not that {outpost} was ready"),
    }
}

fn a_round_asks_the_listener_then_every_outpost_behind_the_facts_before_it() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executable(
        events_tx,
        OutpostOptions::default(),
        exe,
        LIMIT,
        std::sync::Arc::default(),
    )
    .expect("the supervisor starts");
    match next(&events) {
        OutpostMessage::ListenerReady { replacement: false } => {}
        other => panic!("the supervisor said {other:?} before the listener was ready"),
    }

    // Round 1: the listener ends on the query, which counts as its answer;
    // there is no outpost to ask. Its replacement starts after.
    supervisor.query_idle(1);
    queried(&events, 1, &[]);
    match next(&events) {
        OutpostMessage::ListenerReady { replacement: true } => {}
        other => panic!("the supervisor said {other:?}, not that the listener was replaced"),
    }

    // Round 2: the listener sends a fact for an application whose outpost
    // is ready, then its answer; the outpost is asked behind the fact.
    let (first_app, first_pid) = application();
    supervisor.ensure_spawned(first_pid);
    let first = started(&events, first_pid);
    make_ready(&supervisor, &events, first, first_pid);
    supervisor.focus_queued(TraceId::mint(), u64::from(first_pid.0));
    supervisor.query_idle(2);
    queried(&events, 2, &[first]);
    from_outpost(&events, first, &facts_before(1, 2));
    from_outpost(
        &events,
        first,
        &OutpostToSupervisor::IdleAnswer { round: 2 },
    );

    // Round 3: the listener's fact is for an application with no outpost,
    // which starts one, holding the fact for it. The round asks both
    // outposts once the launch has reported; the new one is asked once it
    // is ready, behind the fact.
    let (second_app, second_pid) = application();
    supervisor.focus_queued(TraceId::mint(), u64::from(second_pid.0));
    supervisor.query_idle(3);
    let second = started(&events, second_pid);
    queried(&events, 3, &[first, second]);
    from_outpost(&events, first, &facts_before(0, 3));
    from_outpost(
        &events,
        first,
        &OutpostToSupervisor::IdleAnswer { round: 3 },
    );
    make_ready(&supervisor, &events, second, second_pid);
    from_outpost(&events, second, &facts_before(1, 3));
    from_outpost(
        &events,
        second,
        &OutpostToSupervisor::IdleAnswer { round: 3 },
    );

    assert_eq!(
        supervisor.shutdown(),
        ShutdownSummary {
            clean: 4,
            exited: 0,
            killed: 0,
        },
        "both listeners and both outposts ended cleanly, the first listener with code 0 on its own"
    );
    let rest: Vec<OutpostMessage> = events.try_iter().collect();
    assert!(
        rest.is_empty(),
        "the supervisor said nothing more: {rest:?}"
    );
    end(first_app);
    end(second_app);
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
    // ignored: the binary runs its one test.
    a_round_asks_the_listener_then_every_outpost_behind_the_facts_before_it();
    println!("test a_round_asks_the_listener_then_every_outpost_behind_the_facts_before_it ... ok");
}
