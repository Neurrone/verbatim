//! The supervisor's spare outposts (`docs/design/focus-pipeline.md`,
//! section 19): two are started once Core's own outpost is ready; a start
//! for an application takes the oldest, which is never listed again, and
//! no replacement starts until the outpost it became has answered its first
//! idle query, or five seconds have passed; a fact that may not start an
//! outpost, or one from an ignored process, takes none; a spare that dies
//! waiting, or misses its pings, is replaced; one that fails after it was
//! given, before its `Ready`, its hook failing included, is replaced, the
//! application's held facts, and an idle round's query, going to the next
//! spare, even one still launching, or to an outpost started for it, with no
//! crash recorded against the application, unless the application has
//! exited, which ends it as the application's exit; one that misses its
//! pings after it was given is ended as killed.
//!
//! This test binary runs itself four ways: as an application, a process with
//! no windows that runs until its standard input closes; as a stand-in for
//! the focus listener, which says it is ready, answers idle queries, and,
//! the first time the supervisor tells it a focus was queued, sends the
//! facts the test names in [`FACTS`]; and as a stand-in for the outpost
//! binary, started for an application or as a spare (`--spare`). The
//! stand-in outpost says it is ready or a spare, answers pings, says `Ready`
//! for `Assign`, reports with a fault the test reads that it was started
//! for its application ([`DIRECT`]), each fact delivered to it and each idle
//! query, answers each idle query once the event [`HOLD_ANSWERS`] names is
//! set, if one is named, and fails as [`MUTE_SPARE`] and [`Failing`]
//! say. Nothing makes a UI Automation or `WinEvent` call, so the
//! test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::cell::RefCell;
use std::io::{BufReader, Read as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    CreateEventW, OpenEventW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    SYNCHRONIZATION_SYNCHRONIZE, SetEvent, TerminateProcess, WaitForSingleObject,
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

/// Names the launch number of the one stand-in spare that answers no ping
/// while it waits.
const MUTE_SPARE: &str = "VERBATIM_TEST_MUTE_SPARE";

/// Names a file prefix: each stand-in spare given an application numbers
/// its assignment, from 1, by the first `<prefix>-<n>` it can create.
const ASSIGN_MARKERS: &str = "VERBATIM_TEST_ASSIGN_MARKERS";

/// The assignments that fail, by number, separated by commas.
const FAIL_ASSIGNS: &str = "VERBATIM_TEST_FAIL_ASSIGNS";

/// Names an event prefix: a failing assignment `n` waits until the event
/// `<prefix>-<n>` is set, if this is set.
const FAIL_GATE: &str = "VERBATIM_TEST_FAIL_GATE";

/// How a failing assignment fails ([`Failure`]).
const FAIL_AS: &str = "VERBATIM_TEST_FAIL_AS";

/// What a stand-in outpost started for its application reports after its
/// `Ready`, so the test tells it from a spare.
const DIRECT: &str = "started for its application";

/// What a stand-in spare whose hook fails reports, as the real outpost
/// reports its hook's failure (`Outpost::assign`).
const HOOK_FAULT: &str = "the WinEvent hook could not be installed: the test's failure";

/// Core's own outpost: the first launch after the listener's.
const OWN: OutpostId = OutpostId(2);

/// How long a child has to exit after the shutdown message.
const LIMIT: Duration = Duration::from_secs(5);

/// The longest any step waits.
const WAIT: Duration = Duration::from_secs(30);

/// How a failing assignment fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// It exits with code 3 instead of saying `Ready`.
    Exits,
    /// It reports its hook's failure, [`HOOK_FAULT`], and exits with code
    /// 1, as the real outpost does.
    HookFails,
    /// It reports its hook's failure, waits for its application to exit,
    /// then exits with code 1.
    HookFailsAndApplicationExits,
    /// It answers nothing more, pings included, until it is shut down.
    Mute,
}

impl Failure {
    fn name(self) -> &'static str {
        match self {
            Failure::Exits => "exits",
            Failure::HookFails => "hook",
            Failure::HookFailsAndApplicationExits => "hook-then-application",
            Failure::Mute => "mute",
        }
    }

    fn named(name: &str) -> Self {
        [
            Failure::Exits,
            Failure::HookFails,
            Failure::HookFailsAndApplicationExits,
            Failure::Mute,
        ]
        .into_iter()
        .find(|failure| failure.name() == name)
        .unwrap_or_else(|| panic!("no failure is called {name}"))
    }
}

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

/// Waits until the named event is set.
fn wait_for_event(name: &str) {
    // SAFETY: opens an existing named event for waiting; the handle is
    // checked and closed below.
    let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) }
        .expect("the test's event opens");
    // SAFETY: a wait on the event just opened.
    let waited = unsafe { WaitForSingleObject(event, u32::MAX) };
    assert_eq!(waited, WAIT_OBJECT_0, "the test's event was set");
    close(event);
}

/// Waits until the event [`HOLD_ANSWERS`] names is set, if one is named.
fn wait_for_answers_released() {
    if let Ok(name) = std::env::var(HOLD_ANSWERS) {
        wait_for_event(&name);
    }
}

/// Waits until the process `pid` has exited.
fn wait_for_exit(pid: Pid) {
    // SAFETY: opens a process for waiting; the handle is checked and
    // closed below.
    let process =
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid.0) }.expect("the process opens");
    // SAFETY: a wait on the process just opened.
    let waited = unsafe { WaitForSingleObject(process, u32::MAX) };
    assert_eq!(waited, WAIT_OBJECT_0, "the process exited");
    close(process);
}

fn close(handle: HANDLE) {
    // SAFETY: a handle this test opened, closed once.
    let _ = unsafe { CloseHandle(handle) };
}

/// How a stand-in spare's assignment fails, if it does: it numbers the
/// assignment by the first [`ASSIGN_MARKERS`] file it can create, and fails
/// if [`FAIL_ASSIGNS`] lists the number, once the gate event for it is set.
fn assignment_failure() -> Option<Failure> {
    let prefix = std::env::var(ASSIGN_MARKERS).ok()?;
    // No test makes more than a few assignments.
    let number = (1..=100)
        .find(|number| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(format!("{prefix}-{number}"))
                .is_ok()
        })
        .expect("an assignment's number");
    let failing = std::env::var(FAIL_ASSIGNS).unwrap_or_default();
    if !failing
        .split(',')
        .any(|listed| listed == number.to_string())
    {
        return None;
    }
    if let Ok(gate) = std::env::var(FAIL_GATE) {
        wait_for_event(&format!("{gate}-{number}"));
    }
    Some(Failure::named(
        &std::env::var(FAIL_AS).unwrap_or_else(|_| Failure::Exits.name().to_owned()),
    ))
}

/// A stand-in spare's assignment to `target_pid`: says `Ready`, or fails
/// as [`assignment_failure`] says. Returns whether it goes mute.
fn assign(send: &impl Fn(&OutpostToSupervisor), outpost_pid: Pid, target_pid: Pid) -> bool {
    match assignment_failure() {
        None => send(&OutpostToSupervisor::Ready {
            outpost_pid,
            target_pid,
        }),
        Some(Failure::Exits) => std::process::exit(3),
        Some(Failure::HookFails) => {
            send(&OutpostToSupervisor::Fault {
                detail: HOOK_FAULT.to_owned(),
            });
            std::process::exit(1);
        }
        Some(Failure::HookFailsAndApplicationExits) => {
            send(&OutpostToSupervisor::Fault {
                detail: HOOK_FAULT.to_owned(),
            });
            wait_for_exit(target_pid);
            std::process::exit(1);
        }
        Some(Failure::Mute) => return true,
    }
    false
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
    // The writer is shared with the thread that answers a held idle query,
    // so pings are answered while it waits, as the real outpost's I/O
    // thread answers them while its worker is busy.
    let (commands, messages) = (
        BlockingReader::new(commands).expect("a reader"),
        Arc::new(Mutex::new(BlockingWriter::new(messages).expect("a writer"))),
    );
    let listener = args.iter().any(|arg| arg == "--listener");
    let spare = value(args, "--spare");
    let writer = Arc::clone(&messages);
    let send = move |message: &OutpostToSupervisor| {
        let mut writer = writer.lock().expect("the writer");
        write_message(&mut *writer, message).expect("a message is written");
    };
    let outpost_pid = Pid(std::process::id());
    let target = value(args, "--target-pid").map(|pid| Pid(pid.parse().expect("a pid")));
    match target {
        Some(target_pid) => {
            send(&OutpostToSupervisor::Ready {
                outpost_pid,
                target_pid,
            });
            send(&OutpostToSupervisor::Fault {
                detail: DIRECT.to_owned(),
            });
        }
        None if spare.is_some() => send(&OutpostToSupervisor::SpareReady { outpost_pid }),
        None => send(&OutpostToSupervisor::Ready {
            outpost_pid,
            target_pid: Pid(0),
        }),
    }
    // Answers no ping: the mute spare while it waits, or a given spare
    // whose assignment fails mute.
    let mut mute = spare.is_some() && spare == std::env::var(MUTE_SPARE).ok();
    let mut facts_sent = false;
    let mut commands = BufReader::new(commands);
    loop {
        match read_message::<_, SupervisorToOutpost>(&mut commands) {
            Ok(Some(SupervisorToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) if mute => {}
            Ok(Some(SupervisorToOutpost::Ping { seq, .. })) => send(&OutpostToSupervisor::Pong {
                seq,
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(SupervisorToOutpost::FocusQueued { .. })) if listener && !facts_sent => {
                facts_sent = true;
                for fact in scripted_facts() {
                    send(&fact);
                }
            }
            Ok(Some(SupervisorToOutpost::Assign { target_pid })) => {
                mute = assign(&send, outpost_pid, target_pid);
            }
            Ok(Some(SupervisorToOutpost::DeliverFact { observed_at_ms, .. })) => {
                send(&OutpostToSupervisor::Fault {
                    detail: format!("fact {observed_at_ms}"),
                });
            }
            Ok(Some(SupervisorToOutpost::IdleQuery { round })) if listener => {
                send(&OutpostToSupervisor::IdleAnswer { round });
            }
            Ok(Some(SupervisorToOutpost::IdleQuery { round })) => {
                send(&OutpostToSupervisor::Fault {
                    detail: format!("idle query {round}"),
                });
                let messages = Arc::clone(&messages);
                std::thread::spawn(move || {
                    wait_for_answers_released();
                    let mut writer = messages.lock().expect("the writer");
                    write_message(&mut *writer, &OutpostToSupervisor::IdleAnswer { round })
                        .expect("the answer is written");
                });
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

    /// The next spare's readiness, which is `spare`'s, with `waiting`
    /// spares ready.
    fn expect_spare(&self, spare: u64, waiting: usize) {
        let ready = self.next_spare();
        assert_eq!(
            (ready.spare, ready.waiting),
            (OutpostId(spare), waiting),
            "{ready:?}"
        );
    }

    /// The next two spares' readiness, which are `spares`', ready in
    /// either order: whichever is ready first counts one ready, the other
    /// two.
    fn expect_two_spares(&self, spares: [u64; 2]) {
        let ready = [self.next_spare(), self.next_spare()];
        let mut ids = ready.map(|ready| ready.spare);
        ids.sort_unstable();
        assert_eq!(ids, spares.map(OutpostId), "{ready:?}");
        assert_eq!(ready.map(|ready| ready.waiting), [1, 2], "{ready:?}");
    }

    /// Starts Core's own outpost, as Verbatim does at its start, and waits
    /// for it: the spares start once it is ready.
    fn start_own_outpost(&self) {
        let own = Pid(std::process::id());
        self.supervisor.ensure_spawned(own);
        assert_eq!(self.expect_started(own), OWN);
        self.expect_ready(OWN, own);
        self.expect_report(OWN, DIRECT);
    }

    /// [`Run::start_own_outpost`], then waits for the two spares, the
    /// launches after it.
    fn keep_spares(&self) {
        self.start_own_outpost();
        self.expect_two_spares([3, 4]);
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

    /// The next message, `outpost`'s `Ready` for `target_pid`; returns the
    /// outpost's process id.
    fn expect_ready(&self, outpost: OutpostId, target_pid: Pid) -> Pid {
        match self.next() {
            OutpostMessage::Event {
                pid,
                outpost: from,
                message,
                ..
            } if from == outpost && pid == target_pid => match *message {
                OutpostToSupervisor::Ready {
                    outpost_pid,
                    target_pid: ready,
                } if ready == target_pid => outpost_pid,
                other => panic!("{outpost} said {other:?}, not that it was ready for {target_pid}"),
            },
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

    /// The next message, `outpost`'s answer to idle round `round`.
    fn expect_idle_answer(&self, outpost: OutpostId, round: u64) {
        match self.next() {
            OutpostMessage::Event {
                outpost: from,
                message,
                ..
            } if from == outpost
                && matches!(*message, OutpostToSupervisor::IdleAnswer { round: answered } if answered == round) =>
                {}
            other => {
                panic!("the supervisor said {other:?}, not {outpost}'s answer to round {round}")
            }
        }
    }

    /// The next message, that the spare `outpost` given to `target_pid`
    /// failed and `next` holds its facts.
    fn expect_reincarnated(&self, outpost: OutpostId, next: OutpostId, target_pid: Pid) {
        match self.next() {
            OutpostMessage::Reincarnated {
                outpost: failed,
                next: then,
                target_pid: pid,
            } if failed == outpost && then == next && pid == target_pid => {}
            other => panic!(
                "the supervisor said {other:?}, not that {outpost}'s place for {target_pid} went to {next}"
            ),
        }
    }

    /// The next message, that `outpost`, watching `target_pid`, ended for
    /// `reason`.
    fn expect_ended(&self, outpost: OutpostId, target_pid: Pid, reason: EndReason) {
        match self.next() {
            OutpostMessage::Ended {
                outpost: ended,
                target_pid: pid,
                reason: why,
            } if ended == outpost && pid == target_pid && why == reason => {}
            other => panic!(
                "the supervisor said {other:?}, not that {outpost} for {target_pid} ended, {reason}"
            ),
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

/// A stand-in application, with its process id.
fn application_and_pid() -> (Child, Pid) {
    let application = application();
    let pid = Pid(application.id());
    (application, pid)
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

/// A manual-reset event, not set, named `name` for this test process.
fn named_event(name: &str) -> (HANDLE, String) {
    let name = format!("Local\\verbatim-spares-test-{name}-{}", std::process::id());
    // SAFETY: creates a named event; the handle is closed by the caller.
    let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(name.as_str())) }
        .expect("the event is made");
    (event, name)
}

/// Sets an event this test made.
fn set(event: HANDLE) {
    // SAFETY: an event this test made, still open.
    unsafe { SetEvent(event) }.expect("the event is set");
}

/// A manual-reset event the stand-in outposts wait on before they answer an
/// idle query, named in [`HOLD_ANSWERS`] until [`HeldAnswers::done`].
struct HeldAnswers(HANDLE);

impl HeldAnswers {
    fn hold() -> Self {
        let (event, name) = named_event("answers");
        set_env(HOLD_ANSWERS, &name);
        Self(event)
    }

    fn release(&self) {
        set(self.0);
    }

    fn done(self) {
        clear_env(HOLD_ANSWERS);
        close(self.0);
    }
}

/// The given spares' assignments that fail, as [`Failure`] says, each,
/// when `gated`, once the test lets it ([`Failing::fail`]).
struct Failing {
    prefix: PathBuf,
    count: usize,
    gates: Vec<HANDLE>,
}

impl Failing {
    /// The assignments numbered `failing` fail as `failure`; when `gated`,
    /// each only once the test lets it.
    fn assignments(failing: &[usize], failure: Failure, gated: bool) -> Self {
        let prefix = std::env::temp_dir().join(format!(
            "verbatim-spares-test-assign-{}",
            std::process::id()
        ));
        let listed: Vec<String> = failing.iter().map(ToString::to_string).collect();
        set_env(ASSIGN_MARKERS, &prefix);
        set_env(FAIL_ASSIGNS, listed.join(","));
        set_env(FAIL_AS, failure.name());
        let mut events = Vec::new();
        if gated {
            let gate = format!("Local\\verbatim-spares-test-gate-{}", std::process::id());
            for number in 1..=failing.iter().copied().max().unwrap_or(0) {
                // SAFETY: creates a named event; the handle is closed in
                // `done`.
                let event = unsafe {
                    CreateEventW(
                        None,
                        true,
                        false,
                        &HSTRING::from(format!("{gate}-{number}")),
                    )
                }
                .expect("the gate is made");
                events.push(event);
            }
            set_env(FAIL_GATE, gate);
        }
        Self {
            prefix,
            count: 0,
            gates: events,
        }
    }

    /// Lets the failing assignment `number` fail.
    fn fail(&self, number: usize) {
        set(self.gates[number - 1]);
    }

    /// Notes that `count` assignments were made in all.
    fn made(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    /// Checks that exactly the assignments counted were numbered, and
    /// clears everything the stand-ins read.
    fn done(self) {
        for number in 1..=self.count {
            std::fs::remove_file(format!("{}-{number}", self.prefix.display()))
                .unwrap_or_else(|error| panic!("assignment {number} was numbered: {error}"));
        }
        assert!(
            !PathBuf::from(format!("{}-{}", self.prefix.display(), self.count + 1)).exists(),
            "no more than {} assignments were made",
            self.count
        );
        for gate in self.gates {
            close(gate);
        }
        for name in [ASSIGN_MARKERS, FAIL_ASSIGNS, FAIL_GATE, FAIL_AS] {
            clear_env(name);
        }
    }
}

fn a_start_takes_the_oldest_spare_and_no_replacement_starts_before_its_first_idle_answer() {
    let answers = HeldAnswers::hold();
    let run = Run::start(&[]);
    run.keep_spares();

    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3), "the oldest");
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_report(OutpostId(3), "idle query 0");
    let (second, second_pid) = application_and_pid();
    run.supervisor.ensure_spawned(second_pid);
    assert_eq!(
        run.expect_started(second_pid),
        OutpostId(4),
        "the oldest left"
    );
    run.expect_ready(OutpostId(4), second_pid);
    run.expect_report(OutpostId(4), "idle query 0");
    // Both first idle answers are held: no replacement has started, so no
    // spare waits, and the next start is an outpost started for its
    // application.
    let (third, third_pid) = application_and_pid();
    run.supervisor.ensure_spawned(third_pid);
    assert_eq!(run.expect_started(third_pid), OutpostId(5));
    run.expect_ready(OutpostId(5), third_pid);
    run.expect_report(OutpostId(5), DIRECT);

    // Each answer starts its replacement.
    answers.release();
    run.expect_two_spares([6, 7]);
    // A start takes the oldest spare left; a given spare is never given
    // again.
    let (fourth, fourth_pid) = application_and_pid();
    run.supervisor.ensure_spawned(fourth_pid);
    assert_eq!(run.expect_started(fourth_pid), OutpostId(6));
    run.expect_ready(OutpostId(6), fourth_pid);
    run.expect_report(OutpostId(6), "idle query 0");
    run.expect_spare(8, 2);

    run.finish(ShutdownSummary {
        clean: 8,
        exited: 0,
        killed: 0,
    });
    answers.done();
    for application in [first, second, third, fourth] {
        end(application);
    }
}

fn a_fact_that_may_not_start_an_outpost_or_an_ignored_processs_takes_no_spare() {
    let (ignored, ignored_pid) = application_and_pid();
    let other = application();
    let (first, first_pid) = application_and_pid();
    set_env(
        FACTS,
        format!(
            "focus:{ignored_pid}:1,drop:{}:2,focus:{first_pid}:3",
            other.id()
        ),
    );
    let run = Run::start(&[ignored_pid]);
    run.keep_spares();

    run.supervisor.focus_queued(TraceId::mint(), 0);
    // Neither the ignored process's focus nor the other's drop took the
    // oldest spare.
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_report(OutpostId(3), "fact 3");
    run.expect_report(OutpostId(3), "idle query 0");
    run.expect_spare(5, 2);

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
    run.start_own_outpost();
    let ready = [run.next_spare(), run.next_spare()];
    let oldest = ready
        .iter()
        .find(|ready| ready.spare == OutpostId(3))
        .unwrap_or_else(|| panic!("the oldest spare was ready: {ready:?}"));
    kill(oldest.outpost_pid);
    run.expect_spare(5, 2);

    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the oldest left"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "idle query 0");
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    end(first);
}

fn a_spare_that_misses_its_pings_while_it_waits_is_replaced() {
    set_env(MUTE_SPARE, "4");
    let run = Run::start(&[]);
    run.keep_spares();
    // Ended after three ping intervals without a pong, and replaced.
    run.expect_spare(5, 2);

    // The mute spare, asked to shut down, still does: all five are clean.
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
    });
    clear_env(MUTE_SPARE);
}

fn a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next() {
    let failing = Failing::assignments(&[1], Failure::Exits, false);
    let (first, first_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{first_pid}:7"));
    // No application holds attention: an outpost's crash would not be
    // replaced, so a replacement is the spare's failure handled.
    let run = Run::start(&[]);
    run.keep_spares();

    run.supervisor.focus_queued(TraceId::mint(), 0);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the next spare"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "fact 7");
    run.expect_report(OutpostId(4), "idle query 0");
    // One replacement for the failed spare, one for the given one.
    run.expect_two_spares([5, 6]);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    clear_env(FACTS);
    failing.made(2).done();
    end(first);
}

/// The idle round that asked a given spare still starting waits, when the
/// spare fails, for the answer of the incarnation that takes its facts and
/// its query: the app hears that incarnation named before the failed
/// one's end, so the end does not answer the round.
fn a_round_that_asked_a_failed_spare_waits_for_the_next_incarnations_answer() {
    let failing = Failing::assignments(&[1], Failure::Exits, true);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));

    run.supervisor.query_idle(1);
    match run.next() {
        OutpostMessage::IdleQueried { round: 1, outposts } => {
            let mut outposts = outposts;
            outposts.sort_unstable();
            assert_eq!(outposts, [OWN, OutpostId(3)]);
        }
        other => panic!("the supervisor said {other:?}, not which outposts round 1 asked"),
    }
    run.expect_report(OWN, "idle query 1");
    run.expect_idle_answer(OWN, 1);

    failing.fail(1);
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "idle query 1");
    run.expect_idle_answer(OutpostId(4), 1);
    run.expect_two_spares([5, 6]);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    failing.made(2).done();
    end(first);
}

/// A given spare that fails while no spare waits hands its application to
/// the replacement just started for it, still launching: it is sent its
/// `Assign`, and the app `Started`, once its launch reports, and its pipe
/// is read as the application's.
fn a_failed_given_spare_hands_its_application_to_a_spare_still_launching() {
    let answers = HeldAnswers::hold();
    let failing = Failing::assignments(&[2], Failure::Exits, false);
    let run = Run::start(&[]);
    run.keep_spares();
    // The first application's spare owes its replacement while its first
    // idle answer is held.
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_report(OutpostId(3), "idle query 0");

    // The second application's spare fails: no spare waits, so the one
    // launched to replace it is given while it launches.
    let (second, second_pid) = application_and_pid();
    run.supervisor.ensure_spawned(second_pid);
    assert_eq!(run.expect_started(second_pid), OutpostId(4));
    run.expect_reincarnated(OutpostId(4), OutpostId(5), second_pid);
    run.expect_ended(OutpostId(4), second_pid, EndReason::Exited);
    assert_eq!(run.expect_started(second_pid), OutpostId(5));
    run.expect_ready(OutpostId(5), second_pid);
    run.expect_report(OutpostId(5), "idle query 0");

    answers.release();
    run.expect_two_spares([6, 7]);

    run.finish(ShutdownSummary {
        clean: 6,
        exited: 1,
        killed: 0,
    });
    answers.done();
    failing.made(3).done();
    for application in [first, second] {
        end(application);
    }
}

/// Spares given to one application that fail before their `Ready`, as
/// many times as stop an application's outposts' respawning when they
/// crash, record no crash against it (`docs/design/focus-pipeline.md`,
/// section 19.5): each is replaced, the application is served by the
/// next, and, when that one crashes after its `Ready`, by the next one
/// asked for.
fn spares_given_to_one_application_that_fail_record_no_crash_against_it() {
    let failing = Failing::assignments(&[1, 2, 3], Failure::Exits, true);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));

    // Each failure hands the application to the oldest spare waiting, and
    // a replacement is started for the failed one.
    for (failure, given, next) in [(1, 3, 4), (2, 4, 5), (3, 5, 6)] {
        failing.fail(failure);
        run.expect_reincarnated(OutpostId(given), OutpostId(next), first_pid);
        run.expect_ended(OutpostId(given), first_pid, EndReason::Exited);
        assert_eq!(run.expect_started(first_pid), OutpostId(next));
        if failure < 3 {
            run.expect_spare(next + 1, 1);
        }
    }
    let served = run.expect_ready(OutpostId(6), first_pid);
    run.expect_report(OutpostId(6), "idle query 0");
    run.expect_two_spares([7, 8]);

    // Its crash is the application's first: no application holds
    // attention, so it is not replaced, and the next start is not refused.
    kill(served);
    run.expect_ended(OutpostId(6), first_pid, EndReason::Exited);
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(7));
    run.expect_ready(OutpostId(7), first_pid);
    run.expect_report(OutpostId(7), "idle query 0");
    run.expect_spare(9, 2);

    // The three failed spares and the killed outpost had exited.
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 4,
        killed: 0,
    });
    failing.made(5).done();
    end(first);
}

/// A given spare whose hook fails reports it and exits before its
/// `Ready`: a spare's failure, its application's facts going to the next.
fn a_given_spare_whose_hook_fails_is_a_spares_failure() {
    let failing = Failing::assignments(&[1], Failure::HookFails, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_report(OutpostId(3), HOOK_FAULT);
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "idle query 0");
    run.expect_two_spares([5, 6]);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
    });
    failing.made(2).done();
    end(first);
}

/// A given spare whose hook fails, ending after its application has
/// exited, ends as the application's exit: nothing takes its place.
fn a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it() {
    let failing = Failing::assignments(&[1], Failure::HookFailsAndApplicationExits, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_report(OutpostId(3), HOOK_FAULT);
    end(first);
    run.expect_ended(OutpostId(3), first_pid, EndReason::TargetExited);
    run.expect_spare(5, 2);

    run.finish(ShutdownSummary {
        clean: 4,
        exited: 1,
        killed: 0,
    });
    failing.made(1).done();
}

/// A given spare that answers no ping before its `Ready` is ended after
/// three ping intervals, as killed, and its application's facts go to the
/// next.
fn a_given_spare_that_misses_its_pings_is_ended_as_killed() {
    let failing = Failing::assignments(&[1], Failure::Mute, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Killed);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "idle query 0");
    // The mute spare's replacement started at the limit, before it was
    // ended, while the next waited: two were ready then; the failed one's
    // replacement started as it ended, with the first still waiting.
    run.expect_spare(5, 2);
    run.expect_spare(6, 2);

    // The mute spare, asked to shut down, still does.
    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
    });
    failing.made(2).done();
    end(first);
}

/// A given spare's replacement starts after `REPLACEMENT_LIMIT`, five
/// seconds, while its first idle answer is still held.
fn a_replacement_starts_after_the_limit_while_the_first_answer_waits() {
    let answers = HeldAnswers::hold();
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    let given = Instant::now();
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_report(OutpostId(3), "idle query 0");
    run.expect_spare(5, 2);
    assert!(
        given.elapsed() >= Duration::from_secs(5),
        "not before the limit: {:?}",
        given.elapsed()
    );
    // Its answer, released now, owes no replacement any more.
    answers.release();

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
    });
    answers.done();
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
    let tests: [(&str, fn()); 12] = [
        (
            "a_start_takes_the_oldest_spare_and_no_replacement_starts_before_its_first_idle_answer",
            a_start_takes_the_oldest_spare_and_no_replacement_starts_before_its_first_idle_answer,
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
            "a_spare_that_misses_its_pings_while_it_waits_is_replaced",
            a_spare_that_misses_its_pings_while_it_waits_is_replaced,
        ),
        (
            "a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next",
            a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next,
        ),
        (
            "a_round_that_asked_a_failed_spare_waits_for_the_next_incarnations_answer",
            a_round_that_asked_a_failed_spare_waits_for_the_next_incarnations_answer,
        ),
        (
            "a_failed_given_spare_hands_its_application_to_a_spare_still_launching",
            a_failed_given_spare_hands_its_application_to_a_spare_still_launching,
        ),
        (
            "spares_given_to_one_application_that_fail_record_no_crash_against_it",
            spares_given_to_one_application_that_fail_record_no_crash_against_it,
        ),
        (
            "a_given_spare_whose_hook_fails_is_a_spares_failure",
            a_given_spare_whose_hook_fails_is_a_spares_failure,
        ),
        (
            "a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it",
            a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it,
        ),
        (
            "a_given_spare_that_misses_its_pings_is_ended_as_killed",
            a_given_spare_that_misses_its_pings_is_ended_as_killed,
        ),
        (
            "a_replacement_starts_after_the_limit_while_the_first_answer_waits",
            a_replacement_starts_after_the_limit_while_the_first_answer_waits,
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
