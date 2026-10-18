//! `docs/uia-bugs.md`, "A registration that times out reports success"
//! (E-TT in its experiments folder): with short timeouts,
//! `AddEventHandlerGroup` on a window whose provider is held in its call
//! returns success while the provider still has not answered, where a
//! single read fails with `UIA_E_TIMEOUT`.
//!
//! mockapp holds its provider's next call until the test releases it
//! (`hold`), and says which call it holds: the evidence that the client's
//! call has reached the provider. The window's element is fetched before
//! the hold. Both of the process's timeouts are set short: the read is
//! bounded by the transaction timeout, and the registration, measured
//! while these tests were written, by the connection timeout.
//!
//! A registration that timed out delivers nothing until about 2 seconds
//! after it returned, however long either timeout is (e57): an event
//! raised in between is lost, and one raised after it is heard. Its test
//! registers while mockapp's window thread takes no message at all
//! (`stall-until-release quiet`), and shows only the loss: nothing marks
//! the end of the 2 seconds, so the test waits for no later event. It is
//! kept out of `cargo xtask ci` until it has passed on GitHub's x64 and
//! ARM64 runners (`main.rs` lists it as ignored).

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomation6, IUIAutomationElement, TreeScope_Subtree, UIA_E_TIMEOUT,
    UIA_MenuOpenedEventId,
};
use windows::core::{AgileReference, HRESULT, Interface};

use crate::common;
use crate::support::{self, Handler};

/// The connection and transaction timeouts these tests set, short so that
/// the waits are too.
const TIMEOUT: Duration = Duration::from_millis(500);

/// mockapp's window element, read before anything is held, on a client
/// whose process's timeouts are both [`TIMEOUT`].
fn window_with_short_timeouts(hwnd: HWND) -> AgileReference<IUIAutomationElement> {
    let uia = support::client();
    let client: IUIAutomation2 = uia.client().cast().expect("an IUIAutomation2 client");
    let milliseconds = u32::try_from(TIMEOUT.as_millis()).expect("the timeout fits");
    // SAFETY: a live client; a plain integer.
    unsafe { client.SetConnectionTimeout(milliseconds) }.expect("the connection timeout set");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(milliseconds) }.expect("the transaction timeout set");
    AgileReference::new(&support::element(uia.client(), hwnd)).expect("an agile reference")
}

/// Runs `call` with the window's element on a thread of its own, with a
/// client of its own, and returns how it ended once it has.
fn on_a_thread<T: Send + 'static>(
    window: AgileReference<IUIAutomationElement>,
    call: impl FnOnce(&verbatim_uia::Uia, &IUIAutomationElement) -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (done, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let uia = support::client();
        let window = window
            .resolve()
            .expect("the window's element on this thread");
        let _ = done.send(call(&uia, &window));
    });
    ended
}

/// An event handler group registered on the window, its provider held in
/// `FragmentRoot`, returns success while the provider is still held.
pub fn a_group_registered_while_its_provider_is_held_reports_success() {
    let (mut app, hwnd) = support::mockapp("timeout-registration", "uia");
    let window = window_with_short_timeouts(hwnd);
    app.hold();
    let registered = on_a_thread(window, move |uia, window| {
        let client: IUIAutomation6 = uia.client().cast().expect("an IUIAutomation6 client");
        let cache = support::cache(uia.client());
        let handler = Handler::new();
        // SAFETY: a local call on a live client.
        let group = unsafe { client.CreateEventHandlerGroup() }.expect("an event handler group");
        // SAFETY: a live group, cache request and handler.
        unsafe {
            group.AddAutomationEventHandler(
                UIA_MenuOpenedEventId,
                TreeScope_Subtree,
                &cache,
                &handler.events,
            )
        }
        .expect("the event in the group");
        // SAFETY: a live client, element and group.
        unsafe { client.AddEventHandlerGroup(window, &group) }.map_err(|error| error.code())
    });
    assert_eq!(app.held(), "FragmentRoot");
    let registered = registered
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the registration returns while the provider is held");
    assert_eq!(registered, Ok(()));
    app.release();
    app.quit();
}

/// The environment variable that makes this binary the second process's
/// listener, naming mockapp's window.
pub const LISTENER: &str = "VERBATIM_TEST_FOCUS_LISTENER";

/// The second process's listener, run as this binary with [`LISTENER`]
/// naming mockapp's window: registers a group with UIA's focus event on
/// the window, writes `ready`, and then `heard <name>` for each focus
/// event it hears, named by its sender. It ends without running any DLL's
/// detach code once its standard input closes, as the test runner does
/// (`common/harness.rs`).
pub fn listener(window: &str) -> ! {
    let hwnd = HWND(window.parse::<isize>().expect("a window handle") as *mut _);
    let uia = support::client();
    let element = support::element(uia.client(), hwnd);
    let focus = Handler::new();
    let _group = support::group_on(uia.client(), &element, &focus, false);
    let heard = focus.into_receiver();
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() {
                break;
            }
        }
        // SAFETY: the pseudo-handle of this process; no preconditions.
        let this_process = unsafe { GetCurrentProcess() };
        // SAFETY: ends this process; nothing runs after it.
        let _ = unsafe { TerminateProcess(this_process, 0) };
    });
    let mut out = std::io::stdout();
    writeln!(out, "ready").expect("written");
    for (event, ..) in heard {
        writeln!(out, "heard {}", event.name).expect("written");
    }
    std::process::abort();
}

/// The second process's listener, started for mockapp's window, and its
/// lines.
struct Listener {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Listener {
    fn start(hwnd: HWND) -> Self {
        common::contain_children();
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .env(LISTENER, (hwnd.0 as isize).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the listener started");
        let stdin = child.stdin.take().expect("the listener's input");
        let stdout = child.stdout.take().expect("the listener's output");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let listener = Self {
            child,
            stdin,
            lines,
        };
        assert_eq!(listener.line(), "ready");
        listener
    }

    fn line(&self) -> String {
        self.lines
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|error| panic!("the listener wrote nothing: {error}"))
    }

    /// Closes the listener's input, which ends it, and waits for it,
    /// asserting it wrote nothing more.
    fn end(self) {
        drop(self.stdin);
        let mut child = self.child;
        let status = child.wait().expect("the listener ended");
        assert!(status.success(), "the listener ended with {status}");
        let more: Vec<String> = self.lines.iter().collect();
        assert_eq!(more, Vec::<String>::new(), "the listener's further lines");
    }
}

/// A group with the focus event, registered on mockapp's window while its
/// window thread takes no message, returns success before the thread is
/// released, and has not heard a focus event raised there as soon as the
/// thread is released, once both of these have heard theirs: a group with
/// the focus event on the same window, registered before the stall in a
/// second process, hears that event; and then a group with the focus
/// event on a second mockapp's window, in this process, hears a focus
/// event raised there after it. Within this process, the second mockapp's
/// event is the evidence that UIA has delivered what came before it: with
/// the group registered with no stall, the first event was heard by then
/// in each of 30 runs.
pub fn a_registration_that_timed_out_misses_an_event_raised_just_after_the_stall() {
    let (mut app, hwnd) = support::mockapp("timeout-late", "uia");
    let (mut other, other_hwnd) = support::mockapp("timeout-late-other", "uia");
    let window = window_with_short_timeouts(hwnd)
        .resolve()
        .expect("the window's element on this thread");
    let listener = Listener::start(hwnd);
    let uia = support::client();
    let on_other = Handler::new();
    let _other_group = support::group_on(
        uia.client(),
        &support::element(uia.client(), other_hwnd),
        &on_other,
        false,
    );

    app.write("stall-until-release quiet");
    app.expect_line("stall started", String::new);
    let focus = Handler::new();
    let _group = support::group_on(uia.client(), &window, &focus, false);
    app.release();
    app.send("focus btn1");
    assert_eq!(listener.line(), "heard Original Name");
    other.send("focus btn1");
    assert_eq!(on_other.next().0, support::from(&other, "Original Name"));
    focus.heard_nothing_more();
    on_other.heard_nothing_more();
    listener.end();
    app.quit();
    other.quit();
}

/// A read of the window's name, its provider held, fails with
/// `UIA_E_TIMEOUT` while the provider is still held.
pub fn a_read_while_its_provider_is_held_fails() {
    let (mut app, hwnd) = support::mockapp("timeout-read", "uia");
    let window = window_with_short_timeouts(hwnd);
    app.hold();
    let read = on_a_thread(window, |_, window| {
        // SAFETY: a live element; a cross-process read.
        unsafe { window.CurrentName() }
            .map(|name| name.to_string())
            .map_err(|error| error.code())
    });
    assert_eq!(app.held(), "GetPropertyValue");
    let read = read
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the read returns while the provider is held");
    assert_eq!(read, Err(HRESULT(UIA_E_TIMEOUT.cast_signed())));
    app.release();
    app.quit();
}
