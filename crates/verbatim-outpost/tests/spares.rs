//! The supervisor's spare outposts (`docs/design/focus-pipeline.md`,
//! section 19): two are started once Core's own outpost is ready; a start
//! for an application takes the oldest, which is never listed again, and
//! its replacement starts once the outpost it became has answered its first
//! idle query; a fact that may not start an outpost, or one from an ignored
//! process, takes none; a spare that dies waiting is replaced; one that
//! fails after it was given, before its `Ready`, is replaced, the
//! application's held facts going to the next spare with no crash recorded
//! against the application; and three failures within a minute stop the
//! spares, after which outposts start for their applications directly.
//!
//! This test binary runs itself four ways: as an application, a process with
//! no windows that runs until its standard input closes; as a stand-in for
//! the focus listener, which says it is ready and, the first time the
//! supervisor tells it a focus was queued, sends the facts the test names
//! in [`FACTS`]; and as a stand-in for the outpost binary, started for an
//! application or as a spare (`--spare`), which says it is ready or a
//! spare, answers pings, says `Ready` for `Assign`, reports each fact
//! delivered to it and each idle query as a fault the test reads, answers
//! each idle query once the event [`HOLD_ANSWERS`] names is set, if one is
//! named, and fails as [`FAIL_FIRST_ASSIGN`] and [`SPARES_FAIL`] say.
//! Nothing makes a UI Automation or `WinEvent` call, so the test touches
//! nothing on the desktop. Its own `main` recognizes each command line.

use std::cell::RefCell;
use std::io::{BufReader, Read as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, SupervisorToOutpost,
    read_message, write_message,
};
use verbatim_outpost::supervisor::{
    EndReason, IgnoredProcesses, OutpostMessage, ShutdownSummary, Supervisor,
};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, OpenProcess, PROCESS_TERMINATE, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, TerminateProcess, WaitForSingleObject,
};
use windows::core::HSTRING;

mod common;
use common::Delivered;

/// The application's argument.
const APPLICATION: &str = "--stand-in-application";

/// Names the facts the stand-in listener sends, the first time it is told
/// a focus was queued: `focus:<pid>:<ms>` for an MSAA focus, which may
/// start an outpost, and `drop:<pid>:<ms>` for a UIA focus's drop, which
/// may not, separated by commas, each observed at `<ms>`.
const FACTS: &str = "VERBATIM_TEST_FACTS";

/// Names a manual-reset event the stand-in outposts wait on before they
/// answer an idle query.
const HOLD_ANSWERS: &str = "VERBATIM_TEST_HOLD_ANSWERS";

/// Names a file: the first stand-in spare to be given an application makes
/// it and exits with code 3 instead of saying `Ready`.
const FAIL_FIRST_ASSIGN: &str = "VERBATIM_TEST_FAIL_FIRST_ASSIGN";

/// Set, every stand-in spare exits with code 3 as it starts.
const SPARES_FAIL: &str = "VERBATIM_TEST_SPARES_FAIL";

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// The value after `name` on the command line, if it is there.
fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// The facts [`FACTS`] names, as the listener forwards them.
fn scripted_facts() -> Vec<OutpostToSupervisor> {
    let script = std::env::var(FACTS).unwrap_or_default();
    script
        .split(',')
        .filter(|fact| !fact.is_empty())
        .map(|fact| {
            let parts: Vec<&str> = fact.split(':').collect();
            let [kind, pid, ms] = parts.as_slice() else {
                panic!("a fact is kind:pid:ms: {fact}");
            };
            let fact = match *kind {
                "focus" => DeliveredFact::MsaaFocus {
                    hwnd: 1,
                    id_object: -4,
                    id_child: 0,
                },
                "drop" => DeliveredFact::UiaFocusDropped { marker: 1 },
                other => panic!("no fact is called {other}"),
            };
            OutpostToSupervisor::FocusFact {
                trace_id: TraceId::mint(),
                observed_at_ms: ms.parse().expect("a time"),
                timing: EventTiming::default(),
                fact: ListenerFact {
                    pid: Pid(pid.parse().expect("a pid")),
                    fact,
                },
                win_event: None,
            }
        })
        .collect()
}

/// Waits until the event [`HOLD_ANSWERS`] names is set, if one is named.
fn wait_for_answers_released() {
    let Ok(name) = std::env::var(HOLD_ANSWERS) else {
        return;
    };
    // SAFETY: opens an existing named event for waiting; the handle is
    // checked and closed below.
    let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) }
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

/// A stand-in child.
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
    // each read and write.
    let (commands, mut messages) = (
        BlockingReader::new(commands).expect("a reader"),
        BlockingWriter::new(messages).expect("a writer"),
    );
    let listener = args.iter().any(|arg| arg == "--listener");
    let spare = args.iter().any(|arg| arg == "--spare");
    if spare && std::env::var_os(SPARES_FAIL).is_some() {
        std::process::exit(3);
    }
    let mut send = |message: &OutpostToSupervisor| {
        write_message(&mut messages, message).expect("a message is written");
    };
    let outpost_pid = Pid(std::process::id());
    if spare {
        send(&OutpostToSupervisor::SpareReady { outpost_pid });
    } else {
        let target_pid = match value(args, "--target-pid") {
            Some(pid) => Pid(pid.parse().expect("a pid")),
            None => Pid(0),
        };
        send(&OutpostToSupervisor::Ready {
            outpost_pid,
            target_pid,
        });
    }
    let mut facts_sent = false;
    let mut commands = BufReader::new(commands);
    loop {
        match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(SupervisorToOutpost::Ping { seq, .. })) => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(SupervisorToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(SupervisorToOutpost::FocusQueued { .. })) if listener && !facts_sent => {
                facts_sent = true;
                for fact in scripted_facts() {
                    send(&fact);
                }
            }
            Ok(Some(SupervisorToOutpost::Assign { target_pid })) => {
                if let Ok(marker) = std::env::var(FAIL_FIRST_ASSIGN)
                    && std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(marker)
                        .is_ok()
                {
                    std::process::exit(3);
                }
                send(&OutpostToSupervisor::Ready {
                    outpost_pid,
                    target_pid,
                });
            }
            Ok(Some(SupervisorToOutpost::DeliverFact { observed_at_ms, .. })) => {
                send(&OutpostToSupervisor::Fault {
                    detail: format!("fact {observed_at_ms}"),
                });
            }
            Ok(Some(SupervisorToOutpost::IdleQuery { round })) if !listener => {
                send(&OutpostToSupervisor::Fault {
                    detail: format!("idle query {round}"),
                });
                wait_for_answers_released();
                send(&OutpostToSupervisor::IdleAnswer { round });
            }
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// A spare's readiness, as the supervisor reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpareReady {
    spare: OutpostId,
    outpost_pid: Pid,
    waiting: usize,
}

/// One supervisor under test, and what it delivers.
struct Run {
    supervisor: Supervisor,
    events: Delivered,
    /// Spares' readiness, set aside as it arrives among other messages.
    spares: RefCell<Vec<SpareReady>>,
}

impl Run {
    /// A supervisor running this binary's stand-ins, ignoring `ignored`,
    /// once its listener is ready.
    fn start(ignored: &[Pid]) -> Self {
        let (events_tx, events) = unbounded();
        let supervisor = Supervisor::with_executable(
            events_tx,
            OutpostOptions::default(),
            exe(),
            LIMIT,
            Arc::new(IgnoredProcesses::hold(ignored.iter().copied())),
        )
        .expect("the supervisor starts");
        let events = Delivered::new(events, &supervisor);
        let run = Self {
            supervisor,
            events,
            spares: RefCell::new(Vec::new()),
        };
        match run.next() {
            OutpostMessage::ListenerReady { replacement: false } => {}
            other => panic!("the supervisor said {other:?} before the listener was ready"),
        }
        run
    }

    /// The next message that is not a spare's readiness, which is set
    /// aside.
    fn next(&self) -> OutpostMessage {
        loop {
            let message = self
                .events
                .recv_timeout(WAIT)
                .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"));
            match message {
                OutpostMessage::SpareReady {
                    spare,
                    outpost_pid,
                    waiting,
                } => self.spares.borrow_mut().push(SpareReady {
                    spare,
                    outpost_pid,
                    waiting,
                }),
                other => return other,
            }
        }
    }

    /// The next spare's readiness, set aside or the next message.
    fn next_spare(&self) -> SpareReady {
        if !self.spares.borrow().is_empty() {
            return self.spares.borrow_mut().remove(0);
        }
        match self
            .events
            .recv_timeout(WAIT)
            .unwrap_or_else(|error| panic!("the supervisor said nothing: {error}"))
        {
            OutpostMessage::SpareReady {
                spare,
                outpost_pid,
                waiting,
            } => SpareReady {
                spare,
                outpost_pid,
                waiting,
            },
            other => panic!("the supervisor said {other:?}, not that a spare was ready"),
        }
    }

    /// Starts Core's own outpost, as Verbatim does at its start, and waits
    /// for it: the spares start once it is ready.
    fn start_own_outpost(&self) {
        let own = Pid(std::process::id());
        self.supervisor.ensure_spawned(own);
        let outpost = self.expect_started(own);
        self.expect_ready(outpost, own);
    }

    /// [`Run::start_own_outpost`], then waits for the two spares, and
    /// returns them oldest first.
    fn keep_spares(&self) -> [SpareReady; 2] {
        self.start_own_outpost();
        let first = self.next_spare();
        let second = self.next_spare();
        assert_eq!(
            [first.waiting, second.waiting],
            [1, 2],
            "each spare's readiness counts the ready ones"
        );
        assert_ne!(first.spare, second.spare);
        if first.spare < second.spare {
            [first, second]
        } else {
            [second, first]
        }
    }

    /// The next message, which says an outpost started for `target_pid`;
    /// returns it.
    fn expect_started(&self, target_pid: Pid) -> OutpostId {
        match self.next() {
            OutpostMessage::Started {
                outpost,
                target_pid: started,
            } if started == target_pid => outpost,
            other => {
                panic!("the supervisor said {other:?}, not that {target_pid}'s outpost started")
            }
        }
    }

    /// The next message, `outpost`'s `Ready` for `target_pid`.
    fn expect_ready(&self, outpost: OutpostId, target_pid: Pid) {
        match self.next() {
            OutpostMessage::Event {
                pid,
                outpost: from,
                message,
                ..
            } if from == outpost
                && pid == target_pid
                && matches!(*message, OutpostToSupervisor::Ready { target_pid: ready, .. } if ready == target_pid) =>
                {}
            other => panic!(
                "the supervisor said {other:?}, not that {outpost} was ready for {target_pid}"
            ),
        }
    }

    /// The next message, the fault a stand-in outpost `outpost` reports
    /// what it was sent with.
    fn expect_report(&self, outpost: OutpostId, detail: &str) {
        match self.next() {
            OutpostMessage::Event {
                outpost: from,
                message,
                ..
            } if from == outpost
                && matches!(&*message, OutpostToSupervisor::Fault { detail: said } if said == detail) =>
                {}
            other => panic!("the supervisor said {other:?}, not {outpost}'s report {detail:?}"),
        }
    }

    /// Shuts the supervisor down, expecting `summary`, and nothing more
    /// said.
    fn finish(self, summary: ShutdownSummary) {
        assert_eq!(self.supervisor.shutdown(), summary);
        let rest: Vec<OutpostMessage> = self.events.try_iter().collect();
        assert!(
            rest.is_empty(),
            "the supervisor said nothing more: {rest:?}"
        );
        assert!(
            self.spares.borrow().is_empty(),
            "no spare's readiness was left unread: {:?}",
            self.spares.borrow()
        );
    }
}

fn exe() -> PathBuf {
    std::env::current_exe().expect("this test binary's path")
}

/// Starts a stand-in application.
fn application() -> Child {
    Command::new(exe())
        .arg(APPLICATION)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("the application starts")
}

/// Ends a stand-in application, which must exit cleanly.
fn end(mut application: Child) {
    drop(application.stdin.take());
    let status = application.wait().expect("the application exits");
    assert!(status.success(), "the application exited with {status}");
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

/// A manual-reset event named for this test process, not set.
fn answers_event() -> (HANDLE, String) {
    let name = format!("Local\\verbatim-spares-test-answers-{}", std::process::id());
    // SAFETY: creates a named event; the handle is closed by the caller.
    let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(name.as_str())) }
        .expect("the event is made");
    (event, name)
}

fn a_start_takes_the_oldest_spare_and_its_replacement_follows_its_first_idle_answer() {
    let (event, name) = answers_event();
    set_env(HOLD_ANSWERS, &name);
    let run = Run::start(&[]);
    let [oldest, second_oldest] = run.keep_spares();
    let first = application();
    let first_pid = Pid(first.id());

    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        oldest.spare,
        "the oldest spare"
    );
    run.expect_ready(oldest.spare, first_pid);
    run.expect_report(oldest.spare, "idle query 0");
    // Held at its idle query until the event is set; its answer starts
    // the replacement.
    // SAFETY: the event made above, set once.
    unsafe { SetEvent(event) }.expect("the event is set");
    let third = run.next_spare();
    assert!(third.spare > second_oldest.spare, "a new spare, {third:?}");
    assert_eq!(third.waiting, 2);

    // Each later start takes the oldest spare left; a given spare is never
    // given again.
    let second = application();
    let second_pid = Pid(second.id());
    run.supervisor.ensure_spawned(second_pid);
    assert_eq!(run.expect_started(second_pid), second_oldest.spare);
    run.expect_ready(second_oldest.spare, second_pid);
    run.expect_report(second_oldest.spare, "idle query 0");
    let fourth = run.next_spare();
    assert!(
        fourth.spare > third.spare && fourth.waiting == 2,
        "{fourth:?}"
    );
    let third_application = application();
    let third_pid = Pid(third_application.id());
    run.supervisor.ensure_spawned(third_pid);
    assert_eq!(run.expect_started(third_pid), third.spare);
    run.expect_ready(third.spare, third_pid);
    run.expect_report(third.spare, "idle query 0");
    let fifth = run.next_spare();
    assert!(
        fifth.spare > fourth.spare && fifth.waiting == 2,
        "{fifth:?}"
    );

    run.finish(ShutdownSummary {
        clean: 7,
        exited: 0,
        killed: 0,
    });
    clear_env(HOLD_ANSWERS);
    close(event);
    for application in [first, second, third_application] {
        end(application);
    }
}

fn a_fact_that_may_not_start_an_outpost_or_an_ignored_processs_takes_no_spare() {
    let ignored = application();
    let ignored_pid = Pid(ignored.id());
    let other = application();
    let first = application();
    let first_pid = Pid(first.id());
    set_env(
        FACTS,
        format!(
            "focus:{ignored_pid}:1,drop:{}:2,focus:{first_pid}:3",
            other.id()
        ),
    );
    let run = Run::start(&[ignored_pid]);
    let [oldest, _] = run.keep_spares();

    run.supervisor.focus_queued(TraceId::mint(), 0);
    // Neither the ignored process's focus nor the other's drop took the
    // oldest spare.
    assert_eq!(run.expect_started(first_pid), oldest.spare);
    run.expect_ready(oldest.spare, first_pid);
    run.expect_report(oldest.spare, "fact 3");
    run.expect_report(oldest.spare, "idle query 0");
    assert_eq!(run.next_spare().waiting, 2, "the replacement");

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
    });
    clear_env(FACTS);
    for application in [ignored, other, first] {
        end(application);
    }
}

/// Ends the process `pid` as a crash would.
fn kill(pid: Pid) {
    // SAFETY: opens a process for termination; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid.0) }.expect("opens it");
    // SAFETY: ends the process just opened.
    unsafe { TerminateProcess(process, 9) }.expect("ends it");
    close(process);
}

fn a_spare_that_dies_waiting_is_replaced() {
    let run = Run::start(&[]);
    let [oldest, second_oldest] = run.keep_spares();
    kill(oldest.outpost_pid);
    let third = run.next_spare();
    assert!(
        third.spare > second_oldest.spare && third.waiting == 2,
        "{third:?}"
    );

    let first = application();
    let first_pid = Pid(first.id());
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        second_oldest.spare,
        "the oldest left"
    );
    run.expect_ready(second_oldest.spare, first_pid);
    run.expect_report(second_oldest.spare, "idle query 0");
    let fourth = run.next_spare();
    assert!(
        fourth.spare > third.spare && fourth.waiting == 2,
        "{fourth:?}"
    );

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    end(first);
}

fn a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next() {
    let marker = std::env::temp_dir().join(format!(
        "verbatim-spares-test-assign-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    set_env(FAIL_FIRST_ASSIGN, &marker);
    let first = application();
    let first_pid = Pid(first.id());
    set_env(FACTS, format!("focus:{first_pid}:7"));
    // No application holds attention: an outpost's crash would not be
    // replaced, so a replacement is the spare's failure handled.
    let run = Run::start(&[]);
    let [oldest, second_oldest] = run.keep_spares();

    run.supervisor.focus_queued(TraceId::mint(), 0);
    assert_eq!(run.expect_started(first_pid), oldest.spare);
    match run.next() {
        OutpostMessage::Ended {
            outpost,
            target_pid,
            reason: EndReason::Exited,
        } if outpost == oldest.spare && target_pid == first_pid => {}
        other => panic!("the supervisor said {other:?}, not that the given spare ended"),
    }
    assert_eq!(
        run.expect_started(first_pid),
        second_oldest.spare,
        "the next spare"
    );
    run.expect_ready(second_oldest.spare, first_pid);
    run.expect_report(second_oldest.spare, "fact 7");
    run.expect_report(second_oldest.spare, "idle query 0");
    // One replacement for the failed spare, one for the given one.
    let replacements = [run.next_spare(), run.next_spare()];
    assert_eq!(replacements.map(|ready| ready.waiting), [1, 2]);
    assert!(
        replacements
            .iter()
            .all(|ready| ready.spare > second_oldest.spare)
            && replacements[0].spare != replacements[1].spare,
        "{replacements:?}"
    );

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    clear_env(FAIL_FIRST_ASSIGN);
    clear_env(FACTS);
    std::fs::remove_file(&marker).expect("the first given spare made the marker");
    end(first);
}

fn three_spare_failures_within_a_minute_stop_the_spares() {
    set_env(SPARES_FAIL, "1");
    let run = Run::start(&[]);
    run.start_own_outpost();
    match run.next() {
        OutpostMessage::SparesStopped => {}
        other => panic!("the supervisor said {other:?}, not that it stopped keeping spares"),
    }
    // An outpost is started for its application directly.
    let first = application();
    let first_pid = Pid(first.id());
    run.supervisor.ensure_spawned(first_pid);
    let outpost = run.expect_started(first_pid);
    run.expect_ready(outpost, first_pid);

    // Two spares and the two replacements for the first two failures: the
    // third failure stopped the spares, and the fourth spare was ended.
    run.finish(ShutdownSummary {
        clean: 3,
        exited: 4,
        killed: 0,
    });
    clear_env(SPARES_FAIL);
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
    // libtest's own options, such as a name filter, are accepted and
    // ignored: the binary runs all of its tests.
    let tests: [(&str, fn()); 5] = [
        (
            "a_start_takes_the_oldest_spare_and_its_replacement_follows_its_first_idle_answer",
            a_start_takes_the_oldest_spare_and_its_replacement_follows_its_first_idle_answer,
        ),
        (
            "a_fact_that_may_not_start_an_outpost_or_an_ignored_processs_takes_no_spare",
            a_fact_that_may_not_start_an_outpost_or_an_ignored_processs_takes_no_spare,
        ),
        (
            "a_spare_that_dies_waiting_is_replaced",
            a_spare_that_dies_waiting_is_replaced,
        ),
        (
            "a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next",
            a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next,
        ),
        (
            "three_spare_failures_within_a_minute_stop_the_spares",
            three_spare_failures_within_a_minute_stop_the_spares,
        ),
    ];
    for (name, test) in tests {
        test();
        println!("test {name} ... ok");
    }
}
