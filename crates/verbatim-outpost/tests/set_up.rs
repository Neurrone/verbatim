//! The supervisor holds every new application until the focus listener has
//! done UIA's one-time setup in it, and only then gives it an outpost
//! (`docs/design/focus-pipeline.md`, section 20.2): an application whose
//! first fact came from the listener, and one asked for by a focus-now
//! query, each wait for the listener's `SetUpDone`, while Verbatim's own
//! process is never held; an outpost replacing one that failed, in an
//! application set up already, is given at once, with no second setup; an
//! application held that exits is ended, and the listener told; and a
//! listener that replaces one that ended is asked again for every setup
//! still held.
//!
//! This test binary runs itself three ways: as an application, a process
//! with no windows that runs until its standard input closes; as a
//! stand-in for the focus listener, which says it is ready, notes each
//! setup it is asked for in a file the test reads ([`NOTES`]), with its own
//! process id, and answers it only once the test sets the application's
//! gate event, sends the facts [`FACTS`] names the first time it is told a
//! focus was queued, and notes each application whose hook it is told is
//! removed; and as a stand-in for the outpost binary, which says it is
//! ready and then reports, as a fault the test reads, whether the listener
//! had answered its application's setup when it started ([`SET_UP`] or
//! [`NOT_SET_UP`]). Nothing makes a UI Automation or `WinEvent` call, so the
//! test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::io::{BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, SetUpOutcome,
    SupervisorToOutpost, read_message, write_message,
};
use verbatim_outpost::supervisor::{
    EndReason, IgnoredProcesses, OutpostMessage, ShutdownSummary, Supervisor,
};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateSemaphoreW, OpenProcess, PROCESS_TERMINATE, ReleaseSemaphore, SetEvent,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::HSTRING;

mod common;
use common::Delivered;

/// The application's argument.
const APPLICATION: &str = "--stand-in-application";

/// Names the file the stand-in listener notes, one line each, every setup
/// it is asked for (`setup <listener pid> <application pid> <window>`) and
/// every hook it is told is removed (`removed <application pid>`); and the
/// stand-in outpost reads, for the line the listener writes as it answers
/// a setup (`done <application pid>`).
const NOTES: &str = "VERBATIM_TEST_SET_UP_NOTES";

/// Names the prefix of the events that let the stand-in listener answer an
/// application's setup: `<prefix>-<application pid>`.
const GATES: &str = "VERBATIM_TEST_SET_UP_GATES";

/// Names the facts the stand-in listener sends the first time it is told
/// a focus was queued: `<pid>:<ms>` for an MSAA focus in window 1,
/// separated by commas.
const FACTS: &str = "VERBATIM_TEST_FACTS";

/// What a stand-in outpost reports when the listener had answered its
/// application's setup as it started.
const SET_UP: &str = "started after the setup";

/// What a stand-in outpost reports when the listener had not.
const NOT_SET_UP: &str = "started before the setup";

/// Core's own outpost: the first launch after the listener's.
const OWN: OutpostId = OutpostId(2);

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits; the sweep that ends a held application that
/// has exited runs every 30 seconds.
const WAIT: Duration = Duration::from_secs(45);

/// The value after `name` on the command line, if it is there.
fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// Appends `line` to the notes file.
fn note(line: &str) {
    let path = std::env::var(NOTES).expect("the notes file is named");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the notes file opens");
    writeln!(file, "{line}").expect("a note is written");
    drop(file);
    // SAFETY: makes, or opens, the test's semaphore; the handle is checked
    // and closed below.
    let noted = unsafe { CreateSemaphoreW(None, 0, NOTES_MAX, &HSTRING::from(noted_name())) }
        .expect("the notes' semaphore opens");
    // SAFETY: the semaphore just opened; one more note to wait for.
    unsafe { ReleaseSemaphore(noted, 1, None) }.expect("the note is counted");
    close(noted);
}

/// The most notes one test writes, the semaphore's limit.
const NOTES_MAX: i32 = 1000;

/// The name of the semaphore counting the notes written, which the test
/// waits on for each.
fn noted_name() -> String {
    format!(
        "{}-noted",
        std::env::var(GATES).expect("the gates are named")
    )
}

/// The notes written so far, line by line.
fn notes() -> Vec<String> {
    let path = std::env::var(NOTES).expect("the notes file is named");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Waits until the named event is set, making it if the test has not
/// yet.
fn wait_for_event(name: &str) {
    // SAFETY: makes, or opens, a named manual-reset event; the handle is
    // checked and closed below.
    let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(name)) }
        .expect("the test's event opens");
    // SAFETY: a wait on the event just opened.
    let waited = unsafe { WaitForSingleObject(event, u32::MAX) };
    assert_eq!(waited, WAIT_OBJECT_0, "the test's event was set");
    close(event);
}

fn close(handle: HANDLE) {
    // SAFETY: a handle this test opened, closed once.
    let _ = unsafe { CloseHandle(handle) };
}

/// The name of `pid`'s gate.
fn gate_name(pid: Pid) -> String {
    format!(
        "{}-{}",
        std::env::var(GATES).expect("the gates are named"),
        pid.0
    )
}

/// The facts [`FACTS`] names.
fn scripted_facts() -> Vec<OutpostToSupervisor> {
    std::env::var(FACTS)
        .unwrap_or_default()
        .split(',')
        .filter(|fact| !fact.is_empty())
        .map(|fact| {
            let (pid, ms) = fact.split_once(':').expect("a fact is pid:ms");
            OutpostToSupervisor::FocusFact {
                trace_id: TraceId::mint(),
                observed_at_ms: ms.parse().expect("a time"),
                timing: EventTiming::default(),
                fact: ListenerFact {
                    pid: Pid(pid.parse().expect("a pid")),
                    fact: DeliveredFact::MsaaFocus {
                        hwnd: 1,
                        id_object: -4,
                        id_child: 0,
                    },
                },
                win_event: None,
            }
        })
        .collect()
}

/// A stand-in child, as the module says.
fn stand_in(args: &[String]) -> ! {
    let handle = |name| {
        value(args, name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{name} is a handle value: {args:?}"))
    };
    // SAFETY: the supervisor passes the values of the pipe ends it created
    // for this process, which inherited them and uses them nowhere else.
    let (commands, messages) = unsafe {
        verbatim_process::inherited_pipe_handles(handle("--pipe-in"), handle("--pipe-out"))
    }
    .expect("the inherited pipes");
    // The supervisor makes both ends overlapped; this stand-in blocks on
    // each read and write. The listener answers setups from threads of
    // its own.
    let commands = BlockingReader::new(commands).expect("a reader");
    let messages = Arc::new(Mutex::new(BlockingWriter::new(messages).expect("a writer")));
    let send = {
        let messages = Arc::clone(&messages);
        move |message: &OutpostToSupervisor| {
            let mut messages = messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            write_message(&mut *messages, message).expect("a message is written");
        }
    };
    let listener = args.iter().any(|arg| arg == "--listener");
    let own = Pid(std::process::id());
    // An outpost says it is ready for its application, and whether the
    // listener had answered the application's setup then.
    let serve = |target_pid: Pid| {
        send(&OutpostToSupervisor::Ready {
            outpost_pid: own,
            target_pid,
        });
        let answered = notes()
            .iter()
            .any(|line| *line == format!("done {}", target_pid.0));
        send(&OutpostToSupervisor::Fault {
            detail: (if answered { SET_UP } else { NOT_SET_UP }).to_owned(),
        });
    };
    match value(args, "--target-pid").map(|pid| Pid(pid.parse().expect("a pid"))) {
        Some(target_pid) => serve(target_pid),
        None if value(args, "--spare").is_some() => {
            send(&OutpostToSupervisor::SpareReady { outpost_pid: own });
        }
        None => send(&OutpostToSupervisor::Ready {
            outpost_pid: own,
            target_pid: Pid(0),
        }),
    }
    let mut facts_sent = false;
    let mut commands = BufReader::new(commands);
    loop {
        match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(SupervisorToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(SupervisorToOutpost::Ping { seq, .. })) => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(SupervisorToOutpost::SetUp { pid, hwnd })) => {
                note(&format!("setup {} {} {hwnd}", own.0, pid.0));
                let send = send.clone();
                std::thread::spawn(move || {
                    wait_for_event(&gate_name(pid));
                    note(&format!("done {}", pid.0));
                    send(&OutpostToSupervisor::SetUpDone {
                        pid,
                        outcome: SetUpOutcome::Done,
                    });
                });
            }
            Ok(Some(SupervisorToOutpost::HookRemoved { pid, .. })) if listener => {
                note(&format!("removed {}", pid.0));
            }
            Ok(Some(SupervisorToOutpost::FocusQueued { .. })) if listener && !facts_sent => {
                facts_sent = true;
                for fact in scripted_facts() {
                    send(&fact);
                }
            }
            Ok(Some(SupervisorToOutpost::Assign { target_pid })) => serve(target_pid),
            Ok(Some(SupervisorToOutpost::IdleQuery { round })) => {
                send(&OutpostToSupervisor::IdleAnswer { round });
            }
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// Sets the environment variable `name`, which the supervisor's children
/// inherit as they start.
fn set_env(name: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: the tests run one after another on the main thread, and no
    // other thread reads the environment while it is changed.
    unsafe { std::env::set_var(name, value) };
}

fn clear_env(name: &str) {
    // SAFETY: as in `set_env`.
    unsafe { std::env::remove_var(name) };
}

/// One test's notes file and gates, named in the environment its
/// children inherit.
struct Scripted {
    notes: PathBuf,
    gates: Vec<HANDLE>,
    /// The semaphore the children count each note on, and how many notes
    /// the test has waited for.
    noted: HANDLE,
    seen: std::cell::Cell<usize>,
}

impl Scripted {
    /// A new notes file, and gates named for this test.
    fn new(test: &str) -> Self {
        let notes = std::env::temp_dir().join(format!(
            "verbatim-set-up-test-{test}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&notes);
        set_env(NOTES, &notes);
        set_env(
            GATES,
            format!("Local\\verbatim-set-up-test-{test}-{}", std::process::id()),
        );
        // SAFETY: makes a named semaphore; the handle is closed in `done`.
        let counter = unsafe { CreateSemaphoreW(None, 0, NOTES_MAX, &HSTRING::from(noted_name())) }
            .expect("the notes' semaphore is made");
        Self {
            notes,
            gates: Vec::new(),
            noted: counter,
            seen: std::cell::Cell::new(0),
        }
    }

    /// Lets the listener answer `pid`'s setup.
    fn open(&mut self, pid: Pid) {
        // SAFETY: creates, or opens, a named event; the handle is closed in
        // `done`.
        let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(gate_name(pid))) }
            .expect("the gate is made");
        // SAFETY: the event just made.
        unsafe { SetEvent(event) }.expect("the gate opens");
        self.gates.push(event);
    }

    /// Waits until `count` notes have been written, each counted on the
    /// semaphore as it is, and returns the notes.
    fn wait_for_notes(&self, count: usize) -> Vec<String> {
        let timeout = u32::try_from(WAIT.as_millis()).unwrap_or(u32::MAX);
        while self.seen.get() < count {
            // SAFETY: a wait on the semaphore this test made, still open.
            let waited = unsafe { WaitForSingleObject(self.noted, timeout) };
            assert_eq!(
                waited,
                WAIT_OBJECT_0,
                "a note was written within {WAIT:?}; the notes hold {:?}",
                notes()
            );
            self.seen.set(self.seen.get() + 1);
        }
        notes()
    }

    fn done(self) {
        close(self.noted);
        for gate in self.gates {
            close(gate);
        }
        let _ = std::fs::remove_file(self.notes);
        for name in [NOTES, GATES, FACTS] {
            clear_env(name);
        }
    }
}

/// One supervisor under test, and what it delivers.
struct Run {
    supervisor: Supervisor,
    events: Delivered,
}

impl Run {
    /// A supervisor running this binary's stand-ins, once its listener is
    /// ready.
    fn start() -> Self {
        let (events_tx, events) = unbounded();
        let supervisor = Supervisor::with_executable(
            events_tx,
            OutpostOptions::default(),
            exe(),
            LIMIT,
            Arc::new(IgnoredProcesses::hold([])),
        )
        .expect("the supervisor starts");
        let events = Delivered::new(events, &supervisor);
        let run = Self { supervisor, events };
        run.expect_listener_ready(false);
        run
    }

    /// The next message, which spares' readiness never is: no spare is
    /// kept here but where a test starts Core's own outpost, which then
    /// skips them ([`Run::next`] does not).
    fn next(&self) -> OutpostMessage {
        loop {
            match self
                .events
                .recv_timeout(WAIT)
                .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
            {
                OutpostMessage::SpareReady { .. } => {}
                other => return other,
            }
        }
    }

    fn expect_listener_ready(&self, replacement: bool) {
        match self.next() {
            OutpostMessage::ListenerReady { replacement: said } if said == replacement => {}
            other => panic!("the supervisor said {other:?}, not that the listener was ready"),
        }
    }

    /// The next messages: `target_pid`'s outpost started, ready, and its
    /// report `detail`; returns the outpost and its process id.
    fn expect_served(&self, target_pid: Pid, detail: &str) -> (OutpostId, Pid) {
        let outpost = match self.next() {
            OutpostMessage::Started {
                outpost,
                target_pid: started,
            } if started == target_pid => outpost,
            other => {
                panic!("the supervisor said {other:?}, not that {target_pid}'s outpost started")
            }
        };
        let process = match self.next() {
            OutpostMessage::Event { message, .. } => match *message {
                OutpostToSupervisor::Ready { outpost_pid, .. } => outpost_pid,
                other => panic!("{outpost} said {other:?}, not that it was ready"),
            },
            other => panic!("the supervisor said {other:?}, not that {outpost} was ready"),
        };
        match self.next() {
            OutpostMessage::Event { message, .. } if matches!(&*message, OutpostToSupervisor::Fault { detail: said } if said == detail) =>
                {}
            other => panic!("the supervisor said {other:?}, not {outpost}'s report {detail:?}"),
        }
        (outpost, process)
    }

    fn finish(self, summary: ShutdownSummary) {
        assert_eq!(self.supervisor.shutdown(), summary);
        let rest: Vec<OutpostMessage> = self
            .events
            .try_iter()
            .filter(|message| !matches!(message, OutpostMessage::SpareReady { .. }))
            .collect();
        assert!(
            rest.is_empty(),
            "the supervisor said nothing more: {rest:?}"
        );
    }
}

fn exe() -> PathBuf {
    std::env::current_exe().expect("this test binary's path")
}

/// Starts a stand-in application, with its process id.
fn application() -> (Child, Pid) {
    let application = Command::new(exe())
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts");
    let pid = Pid(application.id());
    (application, pid)
}

/// Ends a stand-in application, which must exit cleanly.
fn end(mut application: Child) {
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
}

/// Ends the process `pid` as a crash would.
fn kill(pid: Pid) {
    // SAFETY: opens a process for termination; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid.0) }.expect("opens it");
    // SAFETY: ends the process just opened.
    unsafe { TerminateProcess(process, 9) }.expect("ends it");
    close(process);
}

/// A note of a setup: the listener's process id, the application's, and
/// the window named.
fn setup_note(line: &str) -> (u32, u32, isize) {
    let parts: Vec<&str> = line.split(' ').collect();
    let ["setup", listener, pid, window] = parts.as_slice() else {
        panic!("{line:?} is not a setup's note");
    };
    (
        listener.parse().expect("a pid"),
        pid.parse().expect("a pid"),
        window.parse().expect("a window"),
    )
}

/// An application whose first fact came from the listener, and one asked
/// for by a focus-now query, are each given an outpost only once the
/// listener has answered their setup, each told the window its first fact
/// named, or none; Verbatim's own process is given its outpost with no
/// setup.
fn an_application_is_given_its_outpost_only_once_its_setup_is_done() {
    let mut scripted = Scripted::new("given");
    let (by_fact, by_fact_pid) = application();
    let (by_query, by_query_pid) = application();
    set_env(FACTS, format!("{by_fact_pid}:7"));
    let run = Run::start();
    let own = Pid(std::process::id());
    run.supervisor.ensure_spawned(own);
    let (outpost, _) = run.expect_served(own, NOT_SET_UP);
    assert_eq!(outpost, OWN);

    run.supervisor.focus_queued(TraceId::mint(), 0);
    let lines = scripted.wait_for_notes(1);
    let (_, pid, window) = setup_note(&lines[0]);
    assert_eq!((pid, window), (by_fact_pid.0, 1));
    scripted.open(by_fact_pid);
    run.expect_served(by_fact_pid, SET_UP);

    run.supervisor.ensure_spawned(by_query_pid);
    let lines = scripted.wait_for_notes(3);
    let (_, pid, window) = setup_note(&lines[2]);
    assert_eq!((pid, window), (by_query_pid.0, 0));
    scripted.open(by_query_pid);
    run.expect_served(by_query_pid, SET_UP);

    assert_eq!(
        notes().len(),
        4,
        "two setups and their answers; none for Verbatim's own process"
    );
    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
    });
    scripted.done();
    end(by_fact);
    end(by_query);
}

/// An outpost that fails in an application set up already is replaced at
/// once, with no second setup.
fn a_replacement_in_an_application_set_up_is_given_at_once() {
    let mut scripted = Scripted::new("replacement");
    let (first, first_pid) = application();
    let run = Run::start();
    run.supervisor.ensure_spawned(first_pid);
    scripted.wait_for_notes(1);
    scripted.open(first_pid);
    let (outpost, process) = run.expect_served(first_pid, SET_UP);

    kill(process);
    match run.next() {
        OutpostMessage::Ended {
            outpost: ended,
            target_pid,
            reason: EndReason::Exited,
        } if ended == outpost && target_pid == first_pid => {}
        other => panic!("the supervisor said {other:?}, not that {outpost} ended"),
    }
    run.expect_served(first_pid, SET_UP);
    let setups = notes()
        .iter()
        .filter(|line| line.starts_with("setup "))
        .count();
    assert_eq!(setups, 1, "the replacement needed no setup");

    run.finish(ShutdownSummary {
        clean: 2,
        exited: 1,
        killed: 0,
    });
    scripted.done();
    end(first);
}

/// An application held for its setup that exits is ended, by the sweep:
/// the listener is told, so it ends the application's reading thread, and
/// the app hears that no outpost watches it.
fn a_held_application_that_exits_is_ended() {
    let scripted = Scripted::new("exits");
    let (first, first_pid) = application();
    let run = Run::start();
    run.supervisor.ensure_spawned(first_pid);
    scripted.wait_for_notes(1);
    end(first);
    match run.next() {
        OutpostMessage::NotWatched { target_pid } if target_pid == first_pid => {}
        other => panic!("the supervisor said {other:?}, not that {first_pid} is not watched"),
    }
    let lines = scripted.wait_for_notes(2);
    assert_eq!(lines[1], format!("removed {}", first_pid.0));

    run.finish(ShutdownSummary {
        clean: 1,
        exited: 0,
        killed: 0,
    });
    scripted.done();
}

/// A listener that replaces one that ended is asked again for every setup
/// still held, and the application is given its outpost once it answers.
fn a_replacement_listener_is_asked_for_every_setup_held() {
    let mut scripted = Scripted::new("listener");
    let (first, first_pid) = application();
    let run = Run::start();
    run.supervisor.ensure_spawned(first_pid);
    let lines = scripted.wait_for_notes(1);
    let (ended, pid, _) = setup_note(&lines[0]);
    assert_eq!(pid, first_pid.0);

    kill(Pid(ended));
    run.expect_listener_ready(true);
    let lines = scripted.wait_for_notes(2);
    let (asked, pid, _) = setup_note(&lines[1]);
    assert_eq!(pid, first_pid.0);
    assert_ne!(asked, ended, "the replacement was asked");
    scripted.open(first_pid);
    run.expect_served(first_pid, SET_UP);

    // The listener that ended had exited; its replacement, and the outpost.
    run.finish(ShutdownSummary {
        clean: 2,
        exited: 1,
        killed: 0,
    });
    scripted.done();
    end(first);
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
    let tests: [(&str, fn()); 4] = [
        (
            "an_application_is_given_its_outpost_only_once_its_setup_is_done",
            an_application_is_given_its_outpost_only_once_its_setup_is_done,
        ),
        (
            "a_replacement_in_an_application_set_up_is_given_at_once",
            a_replacement_in_an_application_set_up_is_given_at_once,
        ),
        (
            "a_held_application_that_exits_is_ended",
            a_held_application_that_exits_is_ended,
        ),
        (
            "a_replacement_listener_is_asked_for_every_setup_held",
            a_replacement_listener_is_asked_for_every_setup_held,
        ),
    ];
    let filter = args.iter().skip(1).find(|arg| !arg.starts_with('-'));
    for (name, test) in tests {
        if filter.is_some_and(|filter| !name.contains(filter.as_str())) {
            continue;
        }
        test();
        println!("test {name} ... ok");
    }
}
