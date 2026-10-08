//! The real outpost binary, watching an application that exits: it tells
//! Core its target exited (`TargetExited`), and, asked to shut down, closes
//! its pipe and exits with code 0, as every clean shutdown does.
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

use verbatim_outpost::protocol::{
    OutpostToSupervisor, SupervisorToOutpost, read_message, write_message,
};
use verbatim_process::ChildSpec;

/// The stand-in application's argument.
const TARGET: &str = "--stand-in-application";

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

fn an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly() {
    let mut application = Command::new(std::env::current_exe().expect("this binary's path"))
        .arg(TARGET)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the stand-in application starts");
    let target_pid = application.id();

    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_verbatim-outpost"));
    let arguments = move |pipe_in: usize, pipe_out: usize| {
        format!("--pipe-in {pipe_in} --pipe-out {pipe_out} --target-pid {target_pid}")
    };
    let (outpost, pipes) = verbatim_process::launch(&ChildSpec {
        exe: &exe,
        arguments: &arguments,
        log_stem: "target-exit-test",
        memory_cap: None,
        from_child_buffer: 0,
    })
    .expect("the outpost starts in its job");

    let (messages_tx, messages) = mpsc::channel();
    let from_child = pipes.from_child;
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from_child);
        while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
            if messages_tx.send(message).is_err() {
                return;
            }
        }
    });
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

    let mut to_child = pipes.to_child;
    write_message(&mut to_child, &SupervisorToOutpost::Shutdown).expect("Shutdown is written");
    to_child.flush().expect("Shutdown is sent");
    assert_eq!(
        messages.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "the outpost said nothing more and closed its pipe"
    );
    assert!(outpost.wait_for_exit(WAIT), "the outpost exits");
    assert_eq!(outpost.exit_code(), Some(0), "it shut down cleanly");
}

fn main() {
    if std::env::args().any(|arg| arg == TARGET) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return;
    }
    an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly();
    println!("test an_outpost_whose_application_exits_says_so_and_shuts_down_cleanly ... ok");
}
