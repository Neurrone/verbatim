//! The supervisor's fallback: a child that does not exit within its time
//! limit after the shutdown message is killed through its job, and the
//! kill is reported in the shutdown's summary (and logged, with the
//! reason).
//!
//! The child is this test binary run again as a stand-in for the outpost
//! binary, which the supervisor launches as its focus listener: it says it
//! is ready, as the listener does, and then never reads its command pipe
//! and never exits, as a child stuck in a call into an application that
//! never answers would. It makes no UI Automation or `WinEvent` calls, so
//! the test touches nothing on the desktop. Its own `main` recognizes the
//! command line the supervisor gives it.

use std::io::Write as _;
use std::time::{Duration, Instant};

use crossbeam_channel::unbounded;
use verbatim_model::Pid;
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{OutpostToSupervisor, write_message};
use verbatim_outpost::supervisor::{OutpostMessage, ShutdownSummary, Supervisor};

/// How long the stand-in has to exit after the shutdown message: short, as
/// it never will.
const LIMIT: Duration = Duration::from_millis(500);

/// The longest the test waits for the stand-in to say it is ready.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// The stand-in: the pipe handle values from the command line the
/// supervisor built (`--listener --pipe-in <handle> --pipe-out <handle>`),
/// `Ready` written, and then nothing, for ever.
fn stuck_child(args: &[String]) -> ! {
    let value = |name: &str| -> usize {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("the command line names {name}: {args:?}"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (_commands, mut messages) =
        unsafe { verbatim_process::inherited_pipes(value("--pipe-in"), value("--pipe-out")) }
            .expect("the inherited pipes");
    write_message(
        &mut messages,
        &OutpostToSupervisor::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(0),
        },
    )
    .expect("Ready is written");
    let _ = messages.flush();
    loop {
        std::thread::park();
    }
}

fn a_child_that_does_not_exit_in_time_is_killed_and_the_kill_is_reported() {
    let (events_tx, events) = unbounded();
    let exe = std::env::current_exe().expect("this test binary's path");
    let supervisor = Supervisor::with_executable(events_tx, OutpostOptions::default(), exe, LIMIT)
        .expect("the supervisor starts");
    match events.recv_timeout(READY_TIMEOUT) {
        Ok(OutpostMessage::ListenerReady { replacement: false }) => {}
        Ok(other) => panic!("the supervisor said {other:?} before the listener was ready"),
        Err(error) => panic!("the stand-in did not say it was ready: {error}"),
    }
    let started = Instant::now();
    let summary = supervisor.shutdown();
    assert_eq!(
        summary,
        ShutdownSummary {
            clean: 0,
            exited: 0,
            killed: 1,
        },
        "the stand-in was killed, and the kill reported"
    );
    assert!(
        started.elapsed() >= LIMIT,
        "it was killed only once its time limit had passed, after {:?}",
        started.elapsed()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--listener") {
        stuck_child(&args);
    }
    // libtest's own options, such as a name filter, are accepted and
    // ignored: the binary holds one test.
    a_child_that_does_not_exit_in_time_is_killed_and_the_kill_is_reported();
    println!("test a_child_that_does_not_exit_in_time_is_killed_and_the_kill_is_reported ... ok");
}
