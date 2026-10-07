//! A test process that ends without unwinding still ends the `mockapp` it
//! started (`common::contain_children`), so a failing test fails promptly
//! rather than leaving `cargo test` waiting on the standard error `mockapp`
//! inherited.
//!
//! The test runs this binary again as a subprocess, which starts a
//! `mockapp` and then panics where a panic cannot unwind, as an assertion
//! failing in a `WinEvent` hook's or a UIA handler's callback does: the
//! subprocess aborts and no destructor runs, so `MockApp`'s drop never ends
//! the `mockapp`. The test asserts that the subprocess failed with the
//! panic, and that its standard error, which the `mockapp` inherited, is
//! closed: every process holding it has ended.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::AsRawHandle;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::WaitForSingleObject;

/// The argument that makes this binary the subprocess.
const SUBPROCESS: &str = "--panic-holding-mockapp";

/// What the subprocess panics with.
const PANIC: &str = "a deliberate panic that cannot unwind";

/// The subprocess: starts a `mockapp`, prints its process id, and panics in
/// a function that cannot unwind, which aborts the process.
fn panic_holding_mockapp() -> ! {
    let title = common::unique_title("mockapp-child-cleanup");
    let app = common::spawn("counts.json", "msaa", &title);
    println!("{}", app.pid());
    callback();
    unreachable!("the panic aborted the process");
}

/// A function with the system's calling convention, as a callback the
/// system calls has, which a panic cannot unwind out of.
extern "system" fn callback() {
    panic!("{PANIC}");
}

fn a_test_process_that_aborts_ends_its_mockapp() {
    common::contain_children();
    let exe = std::env::current_exe().expect("this test binary's path");
    let mut subprocess = Command::new(exe)
        .arg(SUBPROCESS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the subprocess starts");
    let stdout = subprocess.stdout.take().expect("stdout was piped");
    let mut stderr = subprocess.stderr.take().expect("stderr was piped");

    // Standard error is read to its end on a thread of its own, so the wait
    // for its end can be bounded: it ends only once every process that
    // holds it, the subprocess and its mockapp, has ended.
    let (closed_tx, closed) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = stderr.read_to_string(&mut text);
        let _ = closed_tx.send((read.map(|_| ()), text));
    });

    let mut lines = BufReader::new(stdout).lines();
    let pid: u32 = lines
        .next()
        .expect("the subprocess printed its mockapp's process id")
        .expect("the subprocess's output is read")
        .parse()
        .expect("a process id");
    assert_ne!(pid, 0, "the subprocess started a mockapp");

    let handle = HANDLE(subprocess.as_raw_handle());
    let timeout = u32::try_from(common::WAIT_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the subprocess's process handle, live while `subprocess` is.
    let waited = unsafe { WaitForSingleObject(handle, timeout) };
    assert_eq!(
        waited,
        WAIT_OBJECT_0,
        "the subprocess did not end within {:?}",
        common::WAIT_TIMEOUT
    );
    let status = subprocess.wait().expect("the subprocess's exit is read");
    assert!(!status.success(), "the subprocess failed, with {status}");

    let (read, text) = closed.recv_timeout(common::WAIT_TIMEOUT).unwrap_or_else(|_| {
        panic!(
            "the subprocess's standard error was still open {:?} after it ended: its mockapp ({pid}) is still running",
            common::WAIT_TIMEOUT
        )
    });
    read.expect("the subprocess's standard error is read");
    assert!(
        text.contains(PANIC),
        "the subprocess failed with the deliberate panic; its standard error was {text:?}"
    );
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some(SUBPROCESS) {
        panic_holding_mockapp();
    }
    harness::run(&[(
        "a_test_process_that_aborts_ends_its_mockapp",
        a_test_process_that_aborts_ends_its_mockapp,
    )]);
}
