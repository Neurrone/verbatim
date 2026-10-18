//! `docs/uia-bugs.md`, "A new connection fails while another process's
//! remote operation is queued" (`e1` and `e2` in its experiments folder):
//! while a remote operation from another process waits in an application
//! for its busy UI thread, a new connection to the application's window by
//! its handle, here the provider probe, answers that the window has no
//! provider.
//!
//! The operation's sender is this test binary started again
//! ([`operator`]), which sets UIA up in mockapp with a first remote
//! operation and sends a second when the test says. mockapp's window thread
//! is stalled until the test releases it (`stall-until-release posted`),
//! which names the first message posted to it and each client's message
//! sent to it: the operation's arrival is posted to the
//! thread, `WM_USER` to the window COM keeps for the thread's apartment
//! ([`ARRIVAL`]), and the probe sends `WM_GETOBJECT`, the evidence that
//! each is waiting for the thread.

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use verbatim_uia_rops::Builder;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::Accessibility::UIA_NamePropertyId;

use crate::{common, support};

/// The environment variable that makes this binary the operation's
/// sender, naming mockapp's window.
pub const OPERATOR: &str = "VERBATIM_TEST_REMOTE_OPERATOR";

/// mockapp's line for the post of a remote operation's arrival to the
/// stalled window thread: `WM_USER`, to the window COM keeps for the
/// thread's single-threaded apartment, as COM delivers a call into one.
const ARRIVAL: &str = "stall message posted 0x0400 to OleMainThreadWndClass";

/// A remote operation that reads `element`'s name, run in its provider's
/// process; the name it read.
fn read_name_remotely(element: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> String {
    let mut b = Builder::new();
    let element = b.import_element(element);
    let name = b.property(element, UIA_NamePropertyId.0);
    let name = b.add_to_results(name);
    let outcome = b.finish().execute().expect("the remote operation ran");
    format!("{:?}", outcome.get(name))
}

/// The operation's sender, run as this binary with [`OPERATOR`] naming
/// mockapp's window: looks the window up, runs a first remote operation
/// reading its name, which sets UIA up in mockapp, and writes `ready`;
/// then, for each line on its standard input, runs the operation again and
/// writes `read` and what it read. It ends without running any DLL's
/// detach code once its standard input closes, as the test runner does
/// (`common/harness.rs`).
pub fn operator(window: &str) -> ! {
    let hwnd = HWND(window.parse::<isize>().expect("a window handle") as *mut _);
    let uia = support::client();
    let element = support::element(uia.client(), hwnd);
    let _ = read_name_remotely(&element);
    let mut out = std::io::stdout();
    writeln!(out, "ready").expect("written");
    for line in std::io::stdin().lock().lines() {
        if line.is_err() {
            break;
        }
        writeln!(out, "read {}", read_name_remotely(&element)).expect("written");
    }
    // SAFETY: the pseudo-handle of this process; no preconditions.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(this_process, 0) };
    std::process::abort();
}

/// The operation's sender, started for mockapp's window, and its lines.
struct Operator {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Operator {
    fn start(hwnd: HWND) -> Self {
        common::contain_children();
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .env(OPERATOR, (hwnd.0 as isize).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the operation's sender started");
        let stdin = child.stdin.take().expect("the sender's input");
        let stdout = child.stdout.take().expect("the sender's output");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let operator = Self {
            child,
            stdin,
            lines,
        };
        assert_eq!(operator.line(), "ready");
        operator
    }

    fn line(&self) -> String {
        self.lines
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|error| panic!("the sender wrote nothing: {error}"))
    }

    /// Has the sender run its operation again, without waiting for it.
    fn send_operation(&mut self) {
        writeln!(self.stdin, "go").expect("the sender takes the line");
        self.stdin.flush().expect("the line sent");
    }

    /// Closes the sender's input, which ends it, and waits for it.
    fn end(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("the sender ended");
        assert!(status.success(), "the sender ended with {status}");
    }
}

/// While the sender's operation waits for mockapp's stalled window thread,
/// the provider probe of mockapp's window from this process answers that
/// it has no provider, though the window answers once released: the
/// operation reads the window's name from mockapp's own provider, and a
/// probe made once it has finished answers that the window has a provider.
/// UIA's first-time setup in this process is done before the stall, so the
/// probe is only the probe.
pub fn a_probe_during_another_processs_queued_operation_answers_no() {
    let (mut app, hwnd) = support::mockapp("collide", "uia");
    let _setup = support::client();
    let mut operator = Operator::start(hwnd);
    let hwnd = hwnd.0 as isize;

    app.write("stall-until-release posted");
    app.expect_line("stall started", String::new);
    operator.send_operation();
    app.expect_line(ARRIVAL, String::new);
    let (done, probed) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(verbatim_uia::has_server_side_provider(hwnd));
    });
    app.expect_line(
        &support::root_object_request(support::MOCKAPP_WINDOW),
        String::new,
    );
    app.release();
    let probed = probed
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the probe answers");
    assert_eq!(
        (probed, operator.line()),
        (
            false,
            r#"read Ok(String("Mockapp Events Fixture"))"#.to_owned()
        )
    );
    assert!(verbatim_uia::has_server_side_provider(hwnd));
    operator.end();
    app.quit();
}

/// The control: with no operation queued, a probe made while mockapp's
/// window thread is stalled, and released once the probe's message has
/// reached the thread, answers that the window has a provider, so the
/// stall alone does not make the probe answer no.
pub fn a_probe_during_a_plain_stall_answers_yes() {
    let (mut app, hwnd) = support::mockapp("collide-control", "uia");
    let _setup = support::client();
    let hwnd = hwnd.0 as isize;

    app.write("stall-until-release");
    app.expect_line("stall started", String::new);
    let (done, probed) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(verbatim_uia::has_server_side_provider(hwnd));
    });
    app.expect_line(
        &support::root_object_request(support::MOCKAPP_WINDOW),
        String::new,
    );
    app.release();
    let probed = probed
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the probe answers");
    assert!(probed, "the probe answered no");
    app.quit();
}
