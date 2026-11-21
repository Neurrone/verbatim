//! The supervisor's spare outposts and its outposts' ends
//! (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"): two
//! spares are started once Core's own outpost is ready; Core's start of an
//! outpost for an application takes the oldest, which is never listed
//! again, and its replacement starts at once; a start for an ignored
//! process takes none; a spare that dies waiting, or misses its pings, is
//! replaced. An outpost that fails, a given spare's hook failing before
//! its `Ready` or a crash after it, ends and the app hears it, and Core's
//! next start for its application takes the next spare; an idle round
//! that asked a starting spare that fails hears its end; one that fails
//! after its application exited ends as that exit, which follows; and an
//! outpost Core ends, before its `Ready` or after, is ended as killed. Core
//! judges what follows each end, its backoff and its missed pings among
//! it (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation record"); the
//! test
//! stands in for Core.
//!
//! This test binary runs itself four ways: as an application, a process with
//! no windows that runs until its standard input closes; as a stand-in for
//! the focus listener, which says it is ready and answers pings and idle
//! queries; and as a stand-in for the outpost
//! binary, started for an application or as a spare (`--spare`). The
//! stand-in outpost says it is ready or a spare, answers pings, says `Ready`
//! for `Assign`, reports with a fault the test reads that it was started
//! for its application ([`DIRECT`]) and each idle query, answers each idle
//! query, and fails as [`MUTE_SPARE`] and
//! [`Failing`] say. Nothing makes a UI Automation or `WinEvent` call, so
//! the test touches nothing on the desktop. Its own `main` recognizes each
//! command line.

use std::cell::RefCell;
use std::io::{BufReader, Read as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::unbounded;
use verbatim_model::protocol::{CoreToListener, CoreToOutpost, ListenerToCore, OutpostToCore};
use verbatim_model::{OutpostId, Pid};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::message::{read_message, write_message};
use verbatim_process::transport::{BlockingReader, BlockingWriter};
use verbatim_supervisor::{EndReason, Executables, OutpostMessage, ShutdownSummary, Supervisor};
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
    /// It says `Ready`, then answers nothing more, pings included, until
    /// it is shut down.
    MuteAfterReady,
}

impl Failure {
    fn name(self) -> &'static str {
        match self {
            Failure::Exits => "exits",
            Failure::HookFails => "hook",
            Failure::HookFailsAndApplicationExits => "hook-then-application",
            Failure::Mute => "mute",
            Failure::MuteAfterReady => "mute-after-ready",
        }
    }

    fn named(name: &str) -> Self {
        [
            Failure::Exits,
            Failure::HookFails,
            Failure::HookFailsAndApplicationExits,
            Failure::Mute,
            Failure::MuteAfterReady,
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
fn assign(send: &impl Fn(&OutpostToCore), outpost_pid: Pid, target_pid: Pid) -> bool {
    match assignment_failure() {
        None => send(&OutpostToCore::Ready {
            outpost_pid,
            target_pid,
        }),
        Some(Failure::Exits) => std::process::exit(3),
        Some(Failure::HookFails) => {
            send(&OutpostToCore::Fault {
                detail: HOOK_FAULT.to_owned(),
            });
            std::process::exit(1);
        }
        Some(Failure::HookFailsAndApplicationExits) => {
            send(&OutpostToCore::Fault {
                detail: HOOK_FAULT.to_owned(),
            });
            wait_for_exit(target_pid);
            std::process::exit(1);
        }
        Some(Failure::Mute) => return true,
        Some(Failure::MuteAfterReady) => {
            send(&OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            });
            return true;
        }
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
    let (commands, messages) = (
        BlockingReader::new(commands).expect("a reader"),
        RefCell::new(BlockingWriter::new(messages).expect("a writer")),
    );
    let listener = args.iter().any(|arg| arg == "--listener");
    let mut commands = BufReader::new(commands);
    if listener {
        listener_stand_in(&mut commands, &messages);
    }
    let spare = value(args, "--spare");
    let send = |message: &OutpostToCore| {
        write_message(&mut *messages.borrow_mut(), message).expect("a message is written");
    };
    let outpost_pid = Pid(std::process::id());
    let target = value(args, "--target-pid").map(|pid| Pid(pid.parse().expect("a pid")));
    match target {
        Some(target_pid) => {
            send(&OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            });
            send(&OutpostToCore::Fault {
                detail: DIRECT.to_owned(),
            });
        }
        None => send(&OutpostToCore::SpareReady { outpost_pid }),
    }
    // Answers no ping: the mute spare while it waits, or a given spare
    // whose assignment fails mute.
    let mut mute = spare.is_some() && spare == std::env::var(MUTE_SPARE).ok();
    loop {
        let command = read_message::<_, CoreToOutpost>(&mut commands);
        match command {
            Ok(Some(CoreToOutpost::Shutdown) | None) => std::process::exit(0),
            Ok(Some(_)) if mute => {}
            Ok(Some(CoreToOutpost::Ping { .. })) => send(&OutpostToCore::Pong {
                parked_count: 0,
                written_at_us: 0,
            }),
            Ok(Some(CoreToOutpost::Assign { target_pid })) => {
                mute = assign(&send, outpost_pid, target_pid);
            }
            Ok(Some(CoreToOutpost::IdleQuery { round })) => {
                send(&OutpostToCore::Fault {
                    detail: format!("idle query {round}"),
                });
                send(&OutpostToCore::IdleAnswer { round });
            }
            Ok(Some(_)) => {}
            Err(error) => panic!("the command pipe failed: {error}"),
        }
    }
}

/// The stand-in listener: says it is ready, and answers pings and idle
/// queries.
fn listener_stand_in(
    commands: &mut BufReader<BlockingReader>,
    messages: &RefCell<BlockingWriter>,
) -> ! {
    let send = |message: &ListenerToCore| {
        write_message(&mut *messages.borrow_mut(), message).expect("a message is written");
    };
    send(&ListenerToCore::Ready {
        outpost_pid: Pid(std::process::id()),
    });
    loop {
        match read_message::<_, CoreToListener>(commands) {
            Ok(Some(CoreToListener::Shutdown) | None) => std::process::exit(0),
            Ok(Some(CoreToListener::Ping { .. })) => {
                send(&ListenerToCore::Pong { written_at_us: 0 });
            }
            Ok(Some(CoreToListener::IdleQuery { round })) => {
                send(&ListenerToCore::IdleAnswer { round });
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
        let supervisor = Supervisor::with_executables(
            events_tx,
            OutpostOptions::default(),
            Executables::both(exe()),
            LIMIT,
            Arc::new(IgnoredProcesses::hold(ignored.iter().copied())),
            false,
        )
        .expect("the supervisor starts");
        let events = Delivered::new(events, &supervisor);
        let run = Self {
            supervisor,
            events,
            spares: RefCell::new(Vec::new()),
        };
        match run.next() {
            OutpostMessage::ListenerReady { .. } => {}
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
                    ..
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
                ..
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
        self.supervisor.start_outpost(own);
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
                ..
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
                OutpostToCore::Ready {
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
                && matches!(&*message, OutpostToCore::Fault { detail: said } if said == detail) => {
            }
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
                && matches!(*message, OutpostToCore::IdleAnswer { round: answered } if answered == round) =>
                {}
            other => {
                panic!("the supervisor said {other:?}, not {outpost}'s answer to round {round}")
            }
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
                ..
            } if ended == outpost && pid == target_pid && why == reason => {}
            other => panic!(
                "the supervisor said {other:?}, not that {outpost} for {target_pid} ended, {reason}"
            ),
        }
    }

    /// The next message, that `target_pid`'s process exited.
    fn expect_exited(&self, target_pid: Pid) {
        match self.next() {
            OutpostMessage::ApplicationExited {
                target_pid: pid, ..
            } if pid == target_pid => {}
            other => panic!("the supervisor said {other:?}, not that {target_pid} exited"),
        }
    }

    /// The next message, that no outpost was started for `target_pid`,
    /// its application gone.
    fn expect_launch_failed(&self, target_pid: Pid) {
        match self.next() {
            OutpostMessage::LaunchFailed {
                target_pid: pid,
                application_gone: true,
                ..
            } if pid == target_pid => {}
            other => {
                panic!(
                    "the supervisor said {other:?}, not that no outpost started for {target_pid}"
                )
            }
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

/// Sets an event this test made.
fn set(event: HANDLE) {
    // SAFETY: an event this test made, still open.
    unsafe { SetEvent(event) }.expect("the event is set");
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

fn a_start_takes_the_oldest_spare_and_its_replacement_starts_at_once() {
    let run = Run::start(&[]);
    run.keep_spares();

    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3), "the oldest");
    run.expect_ready(OutpostId(3), first_pid);
    // Its replacement started as it was given, and waits with the other.
    run.expect_spare(5, 2);
    let (second, second_pid) = application_and_pid();
    run.supervisor.start_outpost(second_pid);
    assert_eq!(
        run.expect_started(second_pid),
        OutpostId(4),
        "the oldest left; a given spare is never given again"
    );
    run.expect_ready(OutpostId(4), second_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    for application in [first, second] {
        end(application);
    }
}

/// Core's start of an outpost for an ignored process takes no spare and
/// says none started; the next start takes the oldest.
fn an_ignored_processs_start_takes_no_spare() {
    let (ignored, ignored_pid) = application_and_pid();
    let (first, first_pid) = application_and_pid();
    let run = Run::start(&[ignored_pid]);
    run.keep_spares();

    run.supervisor.start_outpost(ignored_pid);
    run.expect_launch_failed(ignored_pid);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3), "the oldest");
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    for application in [ignored, first] {
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
    run.supervisor.start_outpost(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the oldest left"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
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
        killed_on_failure: 0,
    });
    clear_env(MUTE_SPARE);
}

/// A given spare that fails before its `Ready` ends, and Core's next
/// start for its application takes the next spare.
fn a_spare_that_fails_after_it_is_given_ends_and_the_next_start_takes_the_next_spare() {
    let failing = Failing::assignments(&[1], Failure::Exits, true);
    let (first, first_pid) = application_and_pid();
    let run = Run::start(&[]);
    run.keep_spares();

    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);
    failing.fail(1);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the next spare"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(2).done();
    end(first);
}

/// The idle round that asked a given spare still starting hears its end
/// when it fails, which counts as its answer: the generation Core starts
/// next is not asked for that round.
fn a_round_that_asked_a_starting_spare_that_fails_hears_its_end() {
    let failing = Failing::assignments(&[1], Failure::Exits, true);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);

    run.supervisor.query_idle(1);
    match run.next() {
        OutpostMessage::IdleQueried {
            round: 1, outposts, ..
        } => {
            let mut outposts = outposts;
            outposts.sort_unstable();
            assert_eq!(outposts, [OWN, OutpostId(3)]);
        }
        other => panic!("the supervisor said {other:?}, not which outposts round 1 asked"),
    }
    run.expect_report(OWN, "idle query 1");
    run.expect_idle_answer(OWN, 1);

    failing.fail(1);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(2).done();
    end(first);
}

/// An outpost that crashes after its `Ready` ends, and Core's next start
/// for its application takes the next spare at once.
fn an_outpost_that_crashes_after_its_ready_ends_and_the_next_start_takes_a_spare() {
    let (first, first_pid) = application_and_pid();
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    let crashing = run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);

    kill(crashing);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the next spare"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    end(first);
}

/// An outpost that stops answering pings after its `Ready`, which Core
/// ends, is ended as killed, and Core's next start for its application
/// takes the next spare. The stuck process, asked to shut down, still
/// does.
fn an_outpost_core_ends_after_its_ready_is_ended_as_killed() {
    let failing = Failing::assignments(&[1], Failure::MuteAfterReady, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);
    run.supervisor.end_outpost(OutpostId(3));
    run.expect_ended(OutpostId(3), first_pid, EndReason::Killed);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    // The stuck outpost, asked to shut down, still does.
    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(2).done();
    end(first);
}

/// A given spare whose hook fails reports it and exits before its
/// `Ready`: it ends, and Core's next start takes the next spare.
fn a_given_spare_whose_hook_fails_ends() {
    let failing = Failing::assignments(&[1], Failure::HookFails, true);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);
    failing.fail(1);
    run.expect_report(OutpostId(3), HOOK_FAULT);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(2).done();
    end(first);
}

/// A given spare whose hook fails, ending after its application has
/// exited, ends as the application's exit, which follows.
fn a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it() {
    let failing = Failing::assignments(&[1], Failure::HookFailsAndApplicationExits, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_report(OutpostId(3), HOOK_FAULT);
    run.expect_spare(5, 2);
    end(first);
    run.expect_ended(OutpostId(3), first_pid, EndReason::TargetExited);
    run.expect_exited(first_pid);

    run.finish(ShutdownSummary {
        clean: 4,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(1).done();
}

/// A given spare that answers no ping before its `Ready`, which Core ends,
/// is ended as killed, and Core's next start takes the next spare.
fn an_outpost_core_ends_before_its_ready_is_ended_as_killed() {
    let failing = Failing::assignments(&[1], Failure::Mute, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);
    run.supervisor.end_outpost(OutpostId(3));
    run.expect_ended(OutpostId(3), first_pid, EndReason::Killed);
    run.supervisor.start_outpost(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    // The mute spare, asked to shut down, still does.
    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    failing.made(2).done();
    end(first);
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == APPLICATION) {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return std::process::ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "--pipe-in") {
        stand_in(&args);
    }
    // The tests run with libtest's command line (`libtest-mimic`), so
    // cargo-nextest can list them and run each in a process of its own;
    // under `cargo test` they run one after another on the main thread,
    // since each test configures its stand-ins through this process's
    // environment.
    let mut arguments = libtest_mimic::Arguments::from_args();
    arguments.test_threads = Some(1);
    let tests: [(&str, fn()); 11] = [
        (
            "a_start_takes_the_oldest_spare_and_its_replacement_starts_at_once",
            a_start_takes_the_oldest_spare_and_its_replacement_starts_at_once,
        ),
        (
            "an_ignored_processs_start_takes_no_spare",
            an_ignored_processs_start_takes_no_spare,
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
            "a_spare_that_fails_after_it_is_given_ends_and_the_next_start_takes_the_next_spare",
            a_spare_that_fails_after_it_is_given_ends_and_the_next_start_takes_the_next_spare,
        ),
        (
            "a_round_that_asked_a_starting_spare_that_fails_hears_its_end",
            a_round_that_asked_a_starting_spare_that_fails_hears_its_end,
        ),
        (
            "an_outpost_that_crashes_after_its_ready_ends_and_the_next_start_takes_a_spare",
            an_outpost_that_crashes_after_its_ready_ends_and_the_next_start_takes_a_spare,
        ),
        (
            "an_outpost_core_ends_after_its_ready_is_ended_as_killed",
            an_outpost_core_ends_after_its_ready_is_ended_as_killed,
        ),
        (
            "a_given_spare_whose_hook_fails_ends",
            a_given_spare_whose_hook_fails_ends,
        ),
        (
            "a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it",
            a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it,
        ),
        (
            "an_outpost_core_ends_before_its_ready_is_ended_as_killed",
            an_outpost_core_ends_before_its_ready_is_ended_as_killed,
        ),
    ];
    let trials = tests
        .into_iter()
        .map(|(name, test)| {
            libtest_mimic::Trial::test(name, move || {
                test();
                Ok(())
            })
        })
        .collect();
    libtest_mimic::run(&arguments, trials).exit_code()
}
