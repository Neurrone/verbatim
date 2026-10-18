//! The lock per application around the UIA calls that collide
//! (`docs/design/focus-pipeline.md`, section 20; `verbatim_uia::lock`),
//! through the real supervisor, focus listener, and outposts, against
//! mockapp.
//!
//! From the moment a remote operation from one process arrives in an
//! application until UIA begins evaluating it there, every new UIA
//! connection from another process to the application fails without an
//! error: the provider probe answers "no" after about three seconds, and
//! the element lookup returns UIA's window proxy. In each test the
//! listener's remote operation that reads mockapp's focus is held there
//! behind mockapp's busy UI thread (`stall-after-focus-read`, after the
//! listener's plain read of the focus, as Windows Terminal's busy thread
//! held it): once its first, which UIA's one-time setup inside mockapp
//! waits for, as at an application's launch, and once a later one. An
//! outpost then connects to mockapp's window by its handle: the provider probe of a new outpost, which has seen none of
//! mockapp's windows, reading mockapp's tree. The probe must wait for
//! mockapp's lock, which the listener holds in its remote operation, and
//! answer "yes" once the hold is released, so the outpost reads mockapp's
//! own provider, and the listener's read is reported once. Without the
//! lock, the probe made during the hold answers "no" and the outpost reads
//! the window through MSAA.
//!
//! The outpost that connects is the replacement of the one mockapp's first
//! focus started, which the test ends while the read is held: the first
//! one was sent the read's marker, and its intake plans nothing that
//! arrives after a marker until the read takes the marker's place (section
//! 12.4), so it could not make its probe during the hold; its replacement
//! never saw the marker, and takes the read in where it arrives. The
//! replacement also shows the lock reaching an outpost started after a
//! failure.
//!
//! The supervisor runs in the test's process, and launches this test
//! binary as its focus listener and its outposts, which its own `main`
//! recognizes and runs as `verbatim-outpost.exe` would, the application's
//! lock included. Each child sends its lock's log, every guarded call at
//! trace level, to the test over a loopback connection, which is the
//! evidence the test waits for. Each test runs on a desktop of its own
//! (`harness::run_isolated`), where mockapp's window is made topmost, which
//! NVDA's acceptance test, the listener's and the outpost's, takes whatever
//! window is the foreground. No spare outpost is kept, since Core's own
//! outpost is never started here, so each of mockapp's outposts is started
//! for it and inherits its lock.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use verbatim_model::{NormalizedEvent, OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    LockGrant, OutpostToSupervisor, Query, QueryOutcome, QueryResult, SupervisorToOutpost,
};
use verbatim_outpost::supervisor::{
    EndReason, Frames, IgnoredProcesses, OutpostMessage, ShutdownSummary, Supervisor,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_TERMINATE, TerminateProcess,
};

mod common;
#[path = "common/harness.rs"]
mod harness;

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest a step waits for the supervisor or a child.
const WAIT: Duration = Duration::from_secs(30);

/// The environment variable naming the loopback port the children send
/// their logs to.
const LOG_PORT: &str = "VERBATIM_TEST_LOCK_LOG_PORT";

/// The child's log filter: the lock's every guarded call, and the probes'
/// verdicts.
const LOG_FILTER: &str = "verbatim_uia::lock=trace,verbatim_outpost::arbitration=debug";

/// Runs this binary as the focus listener or an outpost, as the
/// supervisor launched it, the way `verbatim-outpost.exe` runs, its
/// application's lock taken from `--app-lock` as that takes it, and ends
/// the process as that does, without running any DLL's detach code.
fn run_child(args: &[String]) -> ! {
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let handle = |name: &str| -> usize {
        value(name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} is a handle value: {args:?}"))
    };
    let listener = args.iter().any(|arg| arg == "--listener");
    send_logs_to_the_test(&if listener {
        "listener".to_owned()
    } else {
        format!("outpost {}", std::process::id())
    });
    let mut options = OutpostOptions::default();
    if args.iter().any(|arg| arg == "--classic-uia") {
        options.remote_operations = false;
    }
    verbatim_model::TraceId::namespace(std::process::id());
    // SAFETY: the supervisor passes the values of the overlapped pipe ends
    // it created for this process, which inherited them and uses them
    // nowhere else.
    let (reader, writer) = unsafe {
        verbatim_process::inherited_pipe_handles(handle("--pipe-in"), handle("--pipe-out"))
    }
    .expect("the inherited pipes");
    let ended = if listener {
        verbatim_outpost::run_listener(reader, writer, IgnoredProcesses::hold([]), &[], options)
    } else {
        let target: Option<u32> = value("--target-pid").map(|pid| pid.parse().expect("a pid"));
        if let (Some(target), Some(grant)) = (
            target,
            value(LockGrant::ARG).and_then(|value| LockGrant::parse_arg(&value)),
        ) {
            // SAFETY: the supervisor names the handle it had this process
            // inherit for the application's lock, used nowhere else.
            unsafe { verbatim_outpost::set_application_lock(Pid(target), grant) };
        }
        verbatim_outpost::run_pipe(reader, writer, target, options)
    };
    // SAFETY: the pseudo-handle of this process.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(this_process, u32::from(ended.is_err())) };
    std::process::abort();
}

/// Sends this child's log, filtered by [`LOG_FILTER`], to the test's port,
/// after a first line naming the child's `role`: `listener`, or `outpost`
/// and its process id.
fn send_logs_to_the_test(role: &str) {
    let port = std::env::var(LOG_PORT).expect("the test's log port");
    let mut stream =
        TcpStream::connect(("127.0.0.1", port.parse::<u16>().expect("a port"))).expect("the test");
    writeln!(stream, "{role}").expect("the role line");
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(LOG_FILTER))
        .with_ansi(false)
        .with_writer(Mutex::new(stream))
        .init();
}

/// One line of a child's lock log, as the test reads it: its message, and
/// the call and application it names.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LockLine {
    message: String,
    call: String,
    pid: u32,
}

impl LockLine {
    /// The line `line` is, when it is a line of the lock module's.
    fn parse(line: &str) -> Option<Self> {
        let (_, rest) = line.split_once(" verbatim_uia::lock: ")?;
        let (message, fields) = rest.split_once(" call=\"")?;
        let (call, fields) = fields.split_once('"')?;
        let pid = fields
            .split_whitespace()
            .find_map(|field| field.strip_prefix("pid="))?
            .parse()
            .ok()?;
        Some(Self {
            message: message.to_owned(),
            call: call.to_owned(),
            pid,
        })
    }
}

/// The children's logs, each line with the role of the child that sent it.
struct Logs {
    lines: Receiver<(String, String)>,
    /// Every line received so far, by role, in order.
    seen: RefCell<Vec<(String, String)>>,
}

impl Logs {
    /// Listens on a loopback port, which the children are told of through
    /// [`LOG_PORT`].
    fn listen() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        // SAFETY: set on this test's own thread before any child is
        // started; nothing in this process reads the variable.
        unsafe { std::env::set_var(LOG_PORT, port.to_string()) };
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let mut lines = BufReader::new(stream).lines();
                    let Some(Ok(role)) = lines.next() else { return };
                    for line in lines {
                        let Ok(line) = line else { return };
                        if tx.send((role.clone(), line)).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self {
            lines,
            seen: RefCell::new(Vec::new()),
        }
    }

    /// Waits for a line of `role`'s that `wanted` matches, and returns
    /// every lock line of `role`'s received so far for the application
    /// `pid`, that one included.
    fn wait_for(&self, role: &str, pid: u32, wanted: impl Fn(&str) -> bool) -> Vec<LockLine> {
        let deadline = Instant::now() + WAIT;
        let mut found = self
            .seen
            .borrow()
            .iter()
            .any(|(from, line)| from == role && wanted(line));
        while !found {
            let (from, line) = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| {
                    panic!(
                        "the {role} logged no such line within {WAIT:?}; the children logged {:#?}",
                        self.seen.borrow()
                    )
                });
            found = from == role && wanted(&line);
            self.seen.borrow_mut().push((from, line));
        }
        self.seen
            .borrow()
            .iter()
            .filter(|(from, _)| from == role)
            .filter_map(|(_, line)| LockLine::parse(line))
            .filter(|line| line.pid == pid)
            .collect()
    }

    /// Every line the children have logged so far, by role, for a failure
    /// message.
    fn logged(&self) -> String {
        while let Ok(line) = self.lines.try_recv() {
            self.seen.borrow_mut().push(line);
        }
        format!("the children logged {:#?}", self.seen.borrow())
    }
}

/// The supervisor's messages as the app handles them: each passed through
/// the supervisor's [`Frames`], so an outpost's frame comes out as the
/// message it carries.
struct Delivered {
    events: Receiver<OutpostMessage>,
    frames: RefCell<Frames>,
    ready: RefCell<VecDeque<OutpostMessage>>,
}

impl Delivered {
    fn recv_timeout(&self, timeout: Duration) -> Option<OutpostMessage> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.ready.borrow_mut().pop_front() {
                return Some(message);
            }
            let message = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()?;
            self.take(message);
        }
    }

    /// Every message delivered already, in order.
    fn delivered(&self) -> Vec<OutpostMessage> {
        while let Ok(message) = self.events.try_recv() {
            self.take(message);
        }
        self.ready.borrow_mut().drain(..).collect()
    }

    fn take(&self, message: OutpostMessage) {
        let mut out = Vec::new();
        self.frames.borrow_mut().arrived(message, &mut out);
        self.ready.borrow_mut().extend(out);
    }
}

/// A supervisor whose children are this binary, and what it delivers.
struct Run {
    supervisor: Supervisor,
    events: Delivered,
}

impl Run {
    /// A supervisor, once its listener is ready.
    fn start() -> Self {
        let (events_tx, events) = channel();
        let supervisor = Supervisor::with_executable(
            verbatim_outpost::EventSink::new(move |message| {
                events_tx
                    .send(message)
                    .map_err(|_| verbatim_outpost::Undelivered::Gone)
            }),
            OutpostOptions::default(),
            std::env::current_exe().expect("this test binary's path"),
            LIMIT,
            Arc::new(IgnoredProcesses::hold([])),
        )
        .expect("the supervisor starts");
        let events = Delivered {
            events,
            frames: RefCell::new(supervisor.frames()),
            ready: RefCell::new(VecDeque::new()),
        };
        let run = Self { supervisor, events };
        match run.next() {
            OutpostMessage::ListenerReady { replacement: false } => {}
            other => panic!("the supervisor said {other:?} before the listener was ready"),
        }
        run
    }

    fn next(&self) -> OutpostMessage {
        self.events
            .recv_timeout(WAIT)
            .unwrap_or_else(|| panic!("the supervisor said nothing within {WAIT:?}"))
    }

    /// The next message, which must be one `outpost` sent; returns it.
    fn next_from(&self, outpost: OutpostId) -> OutpostToSupervisor {
        match self.next() {
            OutpostMessage::Event {
                outpost: from,
                message,
                ..
            } if from == outpost => *message,
            other => panic!("the supervisor said {other:?}, not a message of {outpost}'s"),
        }
    }

    /// The next messages, that an outpost started for `pid` and is ready;
    /// returns it and its process.
    fn expect_given(&self, pid: Pid) -> (OutpostId, Pid) {
        let outpost = match self.next() {
            OutpostMessage::Started {
                outpost,
                target_pid,
            } if target_pid == pid => outpost,
            other => panic!("the supervisor said {other:?}, not that {pid}'s outpost started"),
        };
        match self.next_from(outpost) {
            OutpostToSupervisor::Ready {
                target_pid,
                outpost_pid,
            } if target_pid == pid => (outpost, outpost_pid),
            other => panic!("{outpost} said {other:?}, not that it was ready"),
        }
    }

    /// Ends `outpost`, whose process is `process`, as a crash would, and
    /// returns the outpost that replaces it for `pid`, and its process.
    fn replace(&self, (outpost, process): (OutpostId, Pid), pid: Pid) -> (OutpostId, Pid) {
        // SAFETY: opens the outpost's process to end it; closed below.
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, false, process.0) }
            .expect("the outpost's process opens");
        // SAFETY: ends the process just opened.
        unsafe { TerminateProcess(handle, 9) }.expect("the outpost is ended");
        // SAFETY: the handle opened above, closed once.
        let _ = unsafe { CloseHandle(handle) };
        match self.next() {
            OutpostMessage::Ended {
                outpost: ended,
                target_pid,
                reason: EndReason::Exited,
            } if ended == outpost && target_pid == pid => {}
            other => panic!("the supervisor said {other:?}, not that {outpost} ended"),
        }
        self.expect_given(pid)
    }

    /// Asks `outpost` for its application's tree, a connection to the
    /// application's window by its handle, the provider probe first.
    fn ask_tree(&self, outpost: OutpostId) {
        self.supervisor
            .send_to_outpost(
                outpost,
                SupervisorToOutpost::Query {
                    trace_id: TraceId::mint(),
                    request_id: 1,
                    query: Query::DumpTree,
                    timing: verbatim_outpost::protocol::EventTiming::default(),
                },
            )
            .expect("the query is queued");
    }

    /// The tree `outpost` answers next: its root's backend and name, UIA and
    /// mockapp's provider's root name when the outpost read mockapp's own
    /// provider, where UIA's window proxy names the window by its title and
    /// MSAA's client area is no UIA node.
    fn tree_root(&self, outpost: OutpostId) -> (verbatim_model::Backend, Option<String>) {
        match self.next_from(outpost) {
            OutpostToSupervisor::Reply {
                request_id: 1,
                outcome: QueryOutcome::Done(QueryResult::Tree(tree)),
                ..
            } => (tree.root.snapshot.backend, tree.root.snapshot.name),
            other => panic!("{outpost} said {other:?}, not its tree"),
        }
    }

    /// The focus `outpost` reports next: its node's backend and name.
    fn focus(&self, outpost: OutpostId) -> (verbatim_model::Backend, Option<String>) {
        match self.next_from(outpost) {
            OutpostToSupervisor::Event {
                event: NormalizedEvent::FocusChanged { node, .. },
                ..
            } => (node.backend, node.name),
            other => panic!("{outpost} said {other:?}, not a focus"),
        }
    }

    /// Asks every outpost for idle round `round` and waits for `outpost`'s
    /// answer: nothing else is reported before it has handled everything
    /// it was given.
    fn idle(&self, round: u64, outpost: OutpostId) {
        self.supervisor.query_idle(round);
        match self.next() {
            OutpostMessage::IdleQueried {
                round: asked,
                outposts,
            } if asked == round => {
                assert_eq!(outposts, [outpost], "the outposts round {round} asked");
            }
            other => {
                panic!("the supervisor said {other:?}, not which outposts round {round} asked")
            }
        }
        match self.next_from(outpost) {
            OutpostToSupervisor::IdleAnswer { round: answered } if answered == round => {}
            other => panic!("{outpost} said {other:?}, not its idle answer"),
        }
    }

    fn finish(self) {
        assert_eq!(
            self.supervisor.shutdown(),
            ShutdownSummary {
                clean: 2,
                exited: 1,
                killed: 0,
                killed_on_failure: 0,
            },
            "the listener and the replacement shut down, the outpost ended exited"
        );
        let rest = self.events.delivered();
        assert!(
            rest.is_empty(),
            "the supervisor said nothing more: {rest:?}"
        );
    }
}

/// The lock lines of an outpost's probe of mockapp's window as it waits for
/// the lock the listener holds: every guarded call it made into mockapp so
/// far, with the application's pid left out.
fn the_probe_waiting() -> Vec<LockLine> {
    let line = |message: &str| LockLine {
        message: message.to_owned(),
        call: "UiaHasServerSideProvider".to_owned(),
        pid: 0,
    };
    vec![
        line("a guarded UIA call takes its application's lock"),
        line("a guarded UIA call waits for its application's lock"),
    ]
}

/// Whether `line` is a lock line saying a guarded call waits.
fn waits_for_the_lock(line: &str) -> bool {
    LockLine::parse(line)
        .is_some_and(|line| line.message == "a guarded UIA call waits for its application's lock")
}

/// Whether `line` is a lock line saying the remote operation `Execute`
/// takes the lock.
fn executes(line: &str) -> bool {
    LockLine::parse(line).is_some_and(|line| {
        line.message == "a guarded UIA call takes its application's lock" && line.call == "Execute"
    })
}

/// `lines` with their application's pid left out, to compare with
/// [`the_probe_waiting`].
fn without_pids(lines: Vec<LockLine>) -> Vec<LockLine> {
    lines
        .into_iter()
        .map(|line| LockLine { pid: 0, ..line })
        .collect()
}

/// The verdict of `role`'s provider probe of the window `hwnd`, from its
/// log, once it has logged it.
fn probe_verdict(logs: &Logs, role: &str, pid: Pid, hwnd: isize) -> Option<String> {
    let probed = |line: &str| {
        line.split_once(" verbatim_outpost::arbitration: arbitration probed ")
            .map(|(_, fields)| fields.to_owned())
    };
    let hwnd_field = format!("hwnd={hwnd}");
    let of_the_window =
        |fields: &String| fields.split_whitespace().next() == Some(hwnd_field.as_str());
    let _ = logs.wait_for(role, pid.0, |line| {
        probed(line).is_some_and(|fields| of_the_window(&fields))
    });
    logs.seen
        .borrow()
        .iter()
        .filter(|(from, _)| from == role)
        .filter_map(|(_, line)| probed(line))
        .find(of_the_window)
        .and_then(|fields| {
            fields
                .split_whitespace()
                .find_map(|field| field.strip_prefix("is_uia="))
                .map(str::to_owned)
        })
}

/// While the listener's read of mockapp's focus is held: the outpost that
/// was started for mockapp is replaced, and the replacement asked for the
/// tree; its probe, its first guarded call, waits for the lock. Then the
/// hold is released: the probe answers "yes", the tree is mockapp's own
/// provider's, and the focus the listener read, `focused`, is reported
/// once.
fn connect_during_the_hold(
    run: Run,
    (logs, app): (&Logs, &mut common::MockApp),
    (first, pid, hwnd): ((OutpostId, Pid), Pid, isize),
    focused: &str,
) {
    let (next, process) = run.replace(first, pid);
    run.ask_tree(next);
    let role = format!("outpost {process}");
    let held = logs.wait_for(&role, pid.0, waits_for_the_lock);
    assert_eq!(
        without_pids(held),
        the_probe_waiting(),
        "the new outpost's guarded calls into mockapp during the hold"
    );
    app.release();
    assert_eq!(
        probe_verdict(logs, &role, pid, hwnd).as_deref(),
        Some("true"),
        "the new outpost's probe's verdict"
    );
    assert_eq!(
        run.tree_root(next),
        (
            verbatim_model::Backend::Uia,
            Some("Mockapp Tree Fixture".to_owned())
        ),
        "the new outpost read mockapp's own provider"
    );
    assert_eq!(
        run.focus(next),
        (verbatim_model::Backend::Uia, Some(focused.to_owned())),
        "the focus the listener read"
    );
    run.idle(1, next);
    run.finish();
}

/// mockapp's line for the post of the listener's remote operation's
/// arrival to its stalled window thread: `WM_USER`, to the window COM keeps
/// for the thread's single-threaded apartment, as COM delivers a call into
/// one.
const OPERATION_ARRIVAL: &str = "stall message posted 0x0400 to OleMainThreadWndClass";

/// The listener's first remote operation into mockapp, the read of its
/// first focus, which sets UIA up there, waits behind mockapp's busy UI
/// thread, which stopped taking messages once it answered the listener's
/// plain read of the focus, as at an application's launch; a new outpost's
/// probe of mockapp's window waits for mockapp's lock, its only guarded
/// call during the stall, and answers "yes" once the thread is free.
fn a_probe_waits_for_the_first_operation_behind_a_busy_ui_thread() {
    let logs = Logs::listen();
    let title = common::unique_title("mockapp-lock-first");
    let mut app = common::spawn_shown("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    common::make_topmost(hwnd);
    let pid = Pid(app.pid());
    let run = Run::start();

    // The second read of its keyboard focus from now: the first fills the
    // menu opening's cache as UIA delivers it to the listener, the second
    // is the listener's plain read before its first remote operation.
    app.send("stall-after-focus-read 2");
    app.send("set-focus root");
    app.send("menu-opened root");
    let first = run.expect_given(pid);
    app.expect_line("stall started", || logs.logged());
    app.expect_line(OPERATION_ARRIVAL, || logs.logged());
    let _ = logs.wait_for("listener", pid.0, executes);
    connect_during_the_hold(
        run,
        (&logs, &mut app),
        (first, pid, hwnd.0 as isize),
        "Mockapp Tree Fixture",
    );
    app.quit();
}

/// The listener's remote operation into mockapp, set up already, waits
/// behind mockapp's busy UI thread, which stopped taking messages once it
/// answered the listener's plain read of the focus; a new outpost's probe
/// of mockapp's window waits for mockapp's lock, its only guarded call
/// during the stall, and answers "yes" once the thread is free.
fn a_probe_waits_for_an_operation_behind_a_busy_ui_thread() {
    let logs = Logs::listen();
    let title = common::unique_title("mockapp-lock-busy");
    let mut app = common::spawn_shown("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    common::make_topmost(hwnd);
    let pid = Pid(app.pid());
    let run = Run::start();
    // The first focus: UIA's setup inside mockapp, by the listener's read.
    app.send("set-focus root");
    app.send("menu-opened root");
    let first = run.expect_given(pid);
    assert_eq!(
        run.focus(first.0),
        (
            verbatim_model::Backend::Uia,
            Some("Mockapp Tree Fixture".to_owned())
        )
    );
    run.idle(1, first.0);

    // The second read of its keyboard focus from now: the first fills the
    // menu opening's cache as UIA delivers it to the listener, the second
    // is the listener's plain read before its remote operation.
    app.send("stall-after-focus-read 2");
    app.send("set-focus btn_ok");
    app.send("menu-opened btn_ok");
    app.expect_line("stall started", || logs.logged());
    app.expect_line(OPERATION_ARRIVAL, || logs.logged());
    connect_during_the_hold(run, (&logs, &mut app), (first, pid, hwnd.0 as isize), "OK");
    app.quit();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        run_child(&args);
    }
    harness::run_isolated(&[
        (
            "a_probe_waits_for_the_first_operation_behind_a_busy_ui_thread",
            a_probe_waits_for_the_first_operation_behind_a_busy_ui_thread,
        ),
        (
            "a_probe_waits_for_an_operation_behind_a_busy_ui_thread",
            a_probe_waits_for_an_operation_behind_a_busy_ui_thread,
        ),
    ]);
}
