//! The real outpost binary, watching an application that exits: it tells
//! Core its target exited (`TargetExited`), and, asked to shut down, closes
//! its pipe and exits with code 0, as every clean shutdown does. Started as
//! a spare (`--spare`), it says `SpareReady`, takes its application from
//! `Assign` and says `Ready`; an application it cannot watch, a second
//! `Assign`'s, it reports as a fault and exits with code 1, as it does when
//! its hook cannot be installed.
//!
//! The application is this test binary run again as a stand-in, a process
//! with no windows that runs until its standard input closes, so the test
//! decides when it exits. The outpost makes no call into it, and the test
//! touches nothing on the desktop. Its own `main` recognizes the stand-in's
//! argument.

use std::io::{BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use verbatim_model::Pid;
use verbatim_outpost::protocol::{
    OutpostToSupervisor, SupervisorToOutpost, read_message, write_message,
};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::{BlockingReader, BlockingWriter, EVENT_PIPE_BUFFER};
use verbatim_process::{ChildEnds, ChildSpec};

/// The stand-in application's argument.
const TARGET: &str = "--stand-in-application";

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// Starts the stand-in application.
fn application() -> std::process::Child {
    Command::new(std::env::current_exe().expect("this binary's path"))
        .arg(TARGET)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the stand-in application starts")
}

/// Launches the real outpost binary with `arguments` after its pipes, as
/// the supervisor does: returns it, the writer to its commands, and the
/// messages it sends, read on a thread of their own.
fn launch(
    arguments: String,
    log_stem: &str,
) -> (
    verbatim_process::Contained,
    BlockingWriter,
    mpsc::Receiver<OutpostToSupervisor>,
) {
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_verbatim-outpost"));
    let arguments = move |pipe_in: usize, pipe_out: usize| {
        format!("--pipe-in {pipe_in} --pipe-out {pipe_out} {arguments}")
    };
    // Overlapped ends on both sides, as the supervisor makes them.
    let (to_child, commands) =
        pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
    let (from_child, messages) =
        pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
    let outpost = verbatim_process::launch_with_ends(
        &ChildSpec {
            exe: &exe,
            arguments: &arguments,
            log_stem,
            memory_cap: None,
        },
        ChildEnds { commands, messages },
    )
    .expect("the outpost starts in its job");

    let (messages_tx, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(BlockingReader::new(from_child).expect("a reader"));
        while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
            if messages_tx.send(message).is_err() {
                return;
            }
        }
    });
    (
        outpost,
        BlockingWriter::new(to_child).expect("a writer"),
        messages,
    )
}

/// Sends the outpost `command`.
fn send(to_child: &mut BlockingWriter, command: &SupervisorToOutpost) {
    write_message(to_child, command).expect("the command is written");
    to_child.flush().expect("the command is sent");
}

fn an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly() {
    let mut application = application();
    let target_pid = application.id();
    let (outpost, mut to_child, messages) =
        launch(format!("--target-pid {target_pid}"), "target-exit-test");
    match messages.recv_timeout(WAIT) {
        Ok(OutpostToSupervisor::Ready { .. }) => {}
        other => panic!("the outpost said {other:?}, not Ready"),
    }

    drop(application.stdin.take());
    let status = application.wait().expect("the stand-in exits");
    assert!(status.success(), "the stand-in exited with {status}");
    assert_eq!(
        messages.recv_timeout(WAIT).ok(),
        Some(OutpostToSupervisor::TargetExited),
        "the outpost says its application exited"
    );

    send(&mut to_child, &SupervisorToOutpost::Shutdown);
    assert_eq!(
        messages.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "the outpost said nothing more and closed its pipe"
    );
    assert!(outpost.wait_for_exit(WAIT), "the outpost exits");
    assert_eq!(outpost.exit_code(), Some(0), "it shut down cleanly");
}

fn a_spare_takes_its_application_from_assign_and_ends_on_one_it_cannot_watch() {
    let mut application = application();
    let target_pid = Pid(application.id());
    let (outpost, mut to_child, messages) = launch("--spare 1".to_owned(), "spare-assign-test");
    let outpost_pid = Pid(outpost.pid);
    assert_eq!(
        messages.recv_timeout(WAIT).ok(),
        Some(OutpostToSupervisor::SpareReady { outpost_pid }),
        "a spare says it is one"
    );

    send(&mut to_child, &SupervisorToOutpost::Assign { target_pid });
    assert_eq!(
        messages.recv_timeout(WAIT).ok(),
        Some(OutpostToSupervisor::Ready {
            outpost_pid,
            target_pid
        }),
        "it watches the application it was given"
    );

    // A second application is one it cannot watch: it reports why, ends
    // without a `Ready`, and exits with code 1.
    send(&mut to_child, &SupervisorToOutpost::Assign { target_pid });
    assert_eq!(
        messages.recv_timeout(WAIT).ok(),
        Some(OutpostToSupervisor::Fault {
            detail: format!("the outpost already watches {target_pid}")
        }),
        "it reports why it cannot watch the application"
    );
    assert_eq!(
        messages.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "the outpost said nothing more and closed its pipe"
    );
    assert!(outpost.wait_for_exit(WAIT), "the outpost exits");
    assert_eq!(outpost.exit_code(), Some(1), "it exited for the failure");

    drop(application.stdin.take());
    let status = application.wait().expect("the stand-in exits");
    assert!(status.success(), "the stand-in exited with {status}");
}

fn main() {
    if std::env::args().any(|arg| arg == TARGET) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return;
    }
    an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly();
    println!("test an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly ... ok");
    a_spare_takes_its_application_from_assign_and_ends_on_one_it_cannot_watch();
    println!(
        "test a_spare_takes_its_application_from_assign_and_ends_on_one_it_cannot_watch ... ok"
    );
}
