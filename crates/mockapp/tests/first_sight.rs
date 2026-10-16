//! UIA's one-time setup in each application, done before an outpost is
//! given (`docs/design/focus-pipeline.md`, section 20), through the real
//! supervisor, focus listener, and outposts, against mockapp.
//!
//! The first remote operation any client runs in an application makes UIA
//! set itself up inside it, and while that runs every new UIA connection
//! from another process to the application fails without an error. mockapp
//! holds that setup open on request (`hold-setup`): its window thread's
//! next COM modal loop waits until `release`. These tests show that the
//! supervisor gives no outpost while the listener's setup is held, whatever
//! started the application's record, and that the outpost then given finds
//! mockapp's own provider; that the setup runs in a child window when the
//! top-level window has no provider; and that an application that stops
//! pumping as it takes the focus stays held until it answers, and then
//! reports only its newest focus, once, with its ancestors, as NVDA's
//! capture of a stalled application shows
//! (`docs/nvda/main-loop-and-watchdog.md`, "What a live capture shows").
//!
//! The supervisor runs in the test's process, and launches this test
//! binary as its focus listener and its outposts, which its own `main`
//! recognizes and runs as `verbatim-outpost.exe` would, so the listener and
//! each outpost are processes of their own, as the collision needs. Each
//! test runs on a desktop of its own (`harness::run_isolated`), where no
//! window takes the foreground: a test that needs an outpost to take
//! mockapp's events makes mockapp's window topmost, which NVDA's
//! acceptance test, the listener's and the outpost's, takes whatever window
//! is the foreground. No spare outpost is kept, since Core's own outpost is
//! never started here, so each application's outpost is started for it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use verbatim_model::{NodeSnapshot, NormalizedEvent, OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    OutpostToSupervisor, Query, QueryOutcome, QueryResult, SupervisorToOutpost,
};
use verbatim_outpost::supervisor::{
    Frames, IgnoredProcesses, OutpostMessage, ShutdownSummary, Supervisor,
};
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

mod common;
#[path = "common/harness.rs"]
mod harness;

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest a step waits for the supervisor.
const WAIT: Duration = Duration::from_secs(30);

/// How long mockapp stalls as it takes the focus: longer than UIA's
/// 5-second call limit, so the setup's call times out while it stalls.
const STALL: Duration = Duration::from_secs(20);

/// Runs this binary as the focus listener or an outpost, as the
/// supervisor launched it, the way `verbatim-outpost.exe` runs, and ends
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
    let ended = if args.iter().any(|arg| arg == "--listener") {
        verbatim_outpost::run_listener(reader, writer, IgnoredProcesses::hold([]), &[], options)
    } else {
        let target = value("--target-pid").map(|pid| pid.parse().expect("a pid"));
        verbatim_outpost::run_pipe(reader, writer, target, options)
    };
    // SAFETY: the pseudo-handle of this process.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(this_process, u32::from(ended.is_err())) };
    std::process::abort();
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
        match run.next(WAIT) {
            OutpostMessage::ListenerReady { replacement: false } => {}
            other => panic!("the supervisor said {other:?} before the listener was ready"),
        }
        run
    }

    fn next(&self, timeout: Duration) -> OutpostMessage {
        self.events
            .recv_timeout(timeout)
            .unwrap_or_else(|| panic!("the supervisor said nothing within {timeout:?}"))
    }

    /// The next messages: `pid`'s outpost started, then ready; returns it.
    fn expect_given(&self, pid: Pid, timeout: Duration) -> OutpostId {
        let outpost = match self.next(timeout) {
            OutpostMessage::Started {
                outpost,
                target_pid,
            } if target_pid == pid => outpost,
            other => panic!("the supervisor said {other:?}, not that {pid}'s outpost started"),
        };
        match self.next(WAIT) {
            OutpostMessage::Event { message, .. } if matches!(*message, OutpostToSupervisor::Ready { target_pid, .. } if target_pid == pid) =>
                {}
            other => panic!("the supervisor said {other:?}, not that {outpost} was ready"),
        }
        outpost
    }

    /// Asks `outpost` for its application's tree and returns its root, as
    /// the outpost read it from the top-level window: its first UIA calls
    /// into the application, the provider probe and `ElementFromHandle`.
    fn dumped_root(&self, outpost: OutpostId) -> verbatim_model::TreeNode {
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
        match self.next(WAIT) {
            OutpostMessage::Event { message, .. } => match *message {
                OutpostToSupervisor::Reply {
                    outcome: QueryOutcome::Done(QueryResult::Tree(tree)),
                    ..
                } => tree.root,
                other => panic!("{outpost} said {other:?}, not the tree"),
            },
            other => panic!("the supervisor said {other:?}, not {outpost}'s tree"),
        }
    }

    fn finish(self, summary: ShutdownSummary) {
        assert_eq!(self.supervisor.shutdown(), summary);
        let rest = self.events.delivered();
        assert!(
            rest.is_empty(),
            "the supervisor said nothing more: {rest:?}"
        );
    }
}

/// Asserts that the supervisor has given no outpost, nor said anything
/// else, since the listener was ready.
fn assert_none_given(run: &Run) {
    let said = run.events.delivered();
    assert!(
        said.is_empty(),
        "the supervisor gave an outpost while the setup was held: {said:?}"
    );
}

/// The outpost given to mockapp finds its own provider: the root of the
/// tree it reads from the top-level window is mockapp's UIA root, not UIA's
/// window proxy, named by the window's title, nor MSAA's client area.
fn assert_reads_its_own_provider(run: &Run, outpost: OutpostId) {
    let root = run.dumped_root(outpost);
    assert_eq!(
        (root.snapshot.backend, root.snapshot.name.as_deref()),
        (verbatim_model::Backend::Uia, Some("Mockapp Tree Fixture")),
        "the outpost's first read of mockapp found its own provider"
    );
}

/// An application whose first fact came from the listener is given no
/// outpost while the listener's setup in it is held, and the outpost given
/// once it is released finds mockapp's own provider.
fn an_outpost_is_given_only_once_the_listeners_setup_is_done() {
    let title = common::unique_title("mockapp-first-sight-fact");
    let mut app = common::spawn_shown("tree.json", "uia", &title);
    let pid = Pid(app.pid());
    app.hold_setup();
    let run = Run::start();

    // mockapp's focus event reaches the listener, whose fact starts the
    // application's record; the listener's setup in it is then held.
    app.send("focus btn_ok");
    app.held_modal_loop();
    assert_none_given(&run);

    app.release();
    let outpost = run.expect_given(pid, WAIT);
    assert_reads_its_own_provider(&run, outpost);

    run.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    app.quit();
}

/// An application asked for by a focus-now query, not a listener fact, is
/// set up first too, and given its outpost only after.
fn an_outpost_asked_for_by_a_focus_now_query_waits_for_the_setup() {
    let title = common::unique_title("mockapp-first-sight-query");
    let mut app = common::spawn_shown("tree.json", "uia", &title);
    let pid = Pid(app.pid());
    app.hold_setup();
    let run = Run::start();

    run.supervisor.ensure_spawned(pid);
    app.held_modal_loop();
    assert_none_given(&run);

    app.release();
    let outpost = run.expect_given(pid, WAIT);
    assert_reads_its_own_provider(&run, outpost);

    run.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    app.quit();
}

/// An application whose top-level window has no UIA provider of its own,
/// a plain Win32 window, but a child window that has one, is set up in the
/// child: the setup's remote operation reaches mockapp, which holds it,
/// and the application is given its outpost once it is released.
fn the_setup_runs_in_a_child_window_when_the_top_level_has_no_provider() {
    let title = common::unique_title("mockapp-first-sight-child");
    let mut app = common::spawn_shown("boundary_win32_host.json", "win32", &title);
    let pid = Pid(app.pid());
    app.hold_setup();
    let run = Run::start();

    run.supervisor.ensure_spawned(pid);
    app.held_modal_loop();
    assert_none_given(&run);

    app.release();
    run.expect_given(pid, WAIT);

    run.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    app.quit();
}

/// The focus the outpost reports next: the node and its ancestors' names,
/// outermost first.
fn reported_focus(message: &OutpostMessage) -> Option<(Option<String>, Vec<Option<String>>)> {
    let OutpostMessage::Event { message, .. } = message else {
        return None;
    };
    let OutpostToSupervisor::Event {
        event: NormalizedEvent::FocusChanged {
            node, ancestors, ..
        },
        ..
    } = &**message
    else {
        return None;
    };
    let name = |node: &NodeSnapshot| node.name.clone();
    Some((name(node), ancestors.iter().map(name).collect()))
}

/// An application that stops pumping as it takes the focus, longer than
/// UIA's call limit, stays held: the listener's setup times out, and no
/// outpost is given until mockapp pumps again. Of the two focuses it raised
/// while stalled, the one superseded is not reported, and the newest is
/// reported once, with its ancestors, as NVDA does at its resume
/// (`docs/design/focus-pipeline.md`, section 20.3).
fn an_application_hung_at_first_sight_is_held_until_it_answers() {
    let title = common::unique_title("mockapp-first-sight-hung");
    let mut app = common::spawn_shown("first_sight.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let pid = Pid(app.pid());
    common::make_topmost(hwnd);
    let run = Run::start();

    app.write(&format!(
        "stall {} 0 focus one ; 50 focus two",
        STALL.as_millis()
    ));
    assert_eq!(app.next(common::WAIT_TIMEOUT), "stall started");
    assert_eq!(app.next(common::WAIT_TIMEOUT), "applied", "focus one");
    assert_eq!(app.next(common::WAIT_TIMEOUT), "applied", "focus two");

    let outpost = run.expect_given(pid, STALL + WAIT);
    let ended = app
        .printed()
        .expect("mockapp had ended its stall when its outpost was given");
    assert!(
        ended.starts_with("stall ended "),
        "mockapp said {ended:?}, not that its stall ended"
    );

    let focus = reported_focus(&run.next(WAIT)).expect("the outpost reported a focus");
    assert_eq!(
        focus,
        (Some("Two".to_owned()), vec![Some("Choices".to_owned())])
    );

    // Nothing else is reported before the outpost has handled everything
    // it was given.
    run.supervisor.query_idle(1);
    match run.next(WAIT) {
        OutpostMessage::IdleQueried { round: 1, outposts } => assert_eq!(outposts, [outpost]),
        other => panic!("the supervisor said {other:?}, not which outposts round 1 asked"),
    }
    match run.next(WAIT) {
        OutpostMessage::Event { message, .. }
            if matches!(*message, OutpostToSupervisor::IdleAnswer { round: 1 }) => {}
        other => panic!("the supervisor said {other:?}, not the outpost's idle answer"),
    }

    run.finish(ShutdownSummary {
        clean: 2,
        exited: 0,
        killed: 0,
    });
    app.quit();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        run_child(&args);
    }
    harness::run_isolated(&[
        (
            "an_outpost_is_given_only_once_the_listeners_setup_is_done",
            an_outpost_is_given_only_once_the_listeners_setup_is_done,
        ),
        (
            "an_outpost_asked_for_by_a_focus_now_query_waits_for_the_setup",
            an_outpost_asked_for_by_a_focus_now_query_waits_for_the_setup,
        ),
        (
            "the_setup_runs_in_a_child_window_when_the_top_level_has_no_provider",
            the_setup_runs_in_a_child_window_when_the_top_level_has_no_provider,
        ),
        (
            "an_application_hung_at_first_sight_is_held_until_it_answers",
            an_application_hung_at_first_sight_is_held_until_it_answers,
        ),
    ]);
}
