//! The supervisor's spare outposts and its one failure path
//! (`docs/design/focus-pipeline.md`, section 19): two spares are started
//! once Core's own outpost is ready; a start for an application takes the
//! oldest, which is never listed again, and its replacement starts at once;
//! a fact that may not start an outpost, or one from an ignored process,
//! takes none; a spare that dies waiting, or misses its pings, is replaced.
//! An outpost that fails, whatever the cause, is ended, and its application
//! is given the next spare: before its `Ready`, its hook failing or its
//! pings missed included, the application's held facts, and an idle
//! round's query, go to the next, unless the application has exited, which
//! ends it as the application's exit; after its `Ready`, crashing or
//! missing its pings, the application's later facts go to the next, whether
//! or not it holds attention. Repeated failures for one application are
//! never given up, and are spaced by the backoff, whose count a foreground
//! change to the application starts again. A queued focus is told to the
//! outpost it supersedes as well as to the listener, and to no other
//! outpost, and never to a superseded incarnation that has ended.
//!
//! This test binary runs itself four ways: as an application, a process with
//! no windows that runs until its standard input closes; as a stand-in for
//! the focus listener, which says it is ready, answers idle queries, and,
//! the first time the supervisor tells it a focus was queued, sends the
//! facts the test names in [`FACTS`]; and as a stand-in for the outpost
//! binary, started for an application or as a spare (`--spare`). The
//! stand-in outpost says it is ready or a spare, answers pings, says `Ready`
//! for `Assign`, reports with a fault the test reads that it was started
//! for its application ([`DIRECT`]), each fact delivered to it, each queued
//! focus it is told of, and each idle query, answers each idle query, and fails as [`MUTE_SPARE`] and
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
use verbatim_model::{OutpostId, Pid, TraceId};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, LockGrant, OutpostToSupervisor, SupervisorToOutpost,
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
/// start an outpost, `drop:<pid>:<ms>` for a UIA focus's drop, which may
/// not, and `foreground:<pid>:<ms>` for a foreground change, separated by
/// commas, each observed at `<ms>`.
const FACTS: &str = "VERBATIM_TEST_FACTS";

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

/// Names a file prefix: with it set, each stand-in listener writes every
/// command it is sent but pings, its applications' locks among them, to
/// the first `<prefix>-<n>` it can create, after a line naming its process
/// ([`ListenerLog`]), and each stand-in outpost reports, as a fault, the
/// lock it was given ([`lock_report`]): `docs/design/focus-pipeline.md`,
/// section 20.4.
const LOCK_LOG: &str = "VERBATIM_TEST_LOCK_LOG";

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
                "foreground" => DeliveredFact::Foreground { hwnd: 1 },
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
        Some(Failure::MuteAfterReady) => {
            send(&OutpostToSupervisor::Ready {
                outpost_pid,
                target_pid,
            });
            return true;
        }
    }
    false
}

/// A stand-in listener's record of the commands it is sent
/// ([`LOCK_LOG`]).
struct ListenerLog {
    file: std::fs::File,
}

impl ListenerLog {
    /// The log of a stand-in `listener`, when the test asks for one.
    fn for_listener(listener: bool) -> Option<Self> {
        let prefix = std::env::var(LOCK_LOG).ok().filter(|_| listener)?;
        Some(Self::create(&prefix))
    }

    /// The first `<prefix>-<n>` file this listener can create, beginning
    /// with its process id.
    fn create(prefix: &str) -> Self {
        let file = (1..=100)
            .find_map(|number| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(format!("{prefix}-{number}"))
                    .ok()
            })
            .expect("a listener's log");
        let mut log = Self { file };
        log.line(&format!("listener {}", std::process::id()));
        log
    }

    fn line(&mut self, line: &str) {
        use std::io::Write as _;
        writeln!(self.file, "{line}").expect("the listener's log is written");
    }

    /// Notes `command`, unless it is a ping: a lock with its application,
    /// creation time, and handle, a lock closed, an idle query, or the kind
    /// of any other command.
    fn note(&mut self, command: &SupervisorToOutpost) {
        let line = match command {
            SupervisorToOutpost::Ping { .. } => return,
            SupervisorToOutpost::AppLock { pid, created, lock } => format!(
                "app-lock {} {} {}",
                pid.0,
                created.map_or_else(|| "-".to_owned(), |created| created.to_string()),
                lock.map_or_else(|| "-".to_owned(), |lock| lock.to_string()),
            ),
            SupervisorToOutpost::AppLockClosed { pid, created } => {
                format!("app-lock-closed {} {created}", pid.0)
            }
            SupervisorToOutpost::IdleQuery { round } => format!("idle {round}"),
            other => {
                let kind = format!("{other:?}");
                let kind = kind
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                format!("command {kind}")
            }
        };
        self.line(&line);
    }
}

/// What a stand-in outpost reports of the lock it was given: `lock <handle>
/// <created> <access>`, the handle's access as `NtQueryObject` reads it, or
/// `lock none`. The handle is kept open for the stand-in's life.
fn lock_report(grant: Option<LockGrant>) -> String {
    let Some(grant) = grant else {
        return "lock none".to_owned();
    };
    format!(
        "lock {} {} {:#x}",
        grant.handle,
        grant.created,
        granted_access(handle_value(grant.handle))
    )
}

/// The handle a value of the protocol's names in this process.
fn handle_value(value: u64) -> HANDLE {
    HANDLE(usize::try_from(value).expect("a handle's value") as *mut _)
}

/// The access `handle`, one of this process's, was opened with.
fn granted_access(handle: HANDLE) -> u32 {
    /// `PUBLIC_OBJECT_BASIC_INFORMATION`.
    #[repr(C)]
    #[derive(Default)]
    struct BasicInformation {
        attributes: u32,
        granted_access: u32,
        handle_count: u32,
        pointer_count: u32,
        reserved: [u32; 10],
    }
    let mut information = BasicInformation::default();
    // SAFETY: `information` is the class's structure, sized; the handle is
    // this process's own.
    let status = unsafe {
        windows::Wdk::Foundation::NtQueryObject(
            Some(handle),
            windows::Wdk::Foundation::ObjectBasicInformation,
            Some((&raw mut information).cast()),
            u32::try_from(size_of::<BasicInformation>()).unwrap_or(0),
            None,
        )
    };
    assert!(status.is_ok(), "the handle's information: {status:?}");
    information.granted_access
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
    let spare = value(args, "--spare");
    let send = |message: &OutpostToSupervisor| {
        write_message(&mut *messages.borrow_mut(), message).expect("a message is written");
    };
    let outpost_pid = Pid(std::process::id());
    let target = value(args, "--target-pid").map(|pid| Pid(pid.parse().expect("a pid")));
    let lock_log = std::env::var(LOCK_LOG).ok();
    let mut listener_log = ListenerLog::for_listener(listener);
    match target {
        Some(target_pid) => {
            send(&OutpostToSupervisor::Ready {
                outpost_pid,
                target_pid,
            });
            send(&OutpostToSupervisor::Fault {
                detail: DIRECT.to_owned(),
            });
            if lock_log.is_some() {
                let grant =
                    value(args, LockGrant::ARG).and_then(|grant| LockGrant::parse_arg(&grant));
                send(&OutpostToSupervisor::Fault {
                    detail: lock_report(grant),
                });
            }
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
        let command = read_message::<_, SupervisorToOutpost>(&mut commands);
        if let (Some(log), Ok(Some(command))) = (listener_log.as_mut(), &command) {
            log.note(command);
        }
        match command {
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
            Ok(Some(SupervisorToOutpost::FocusQueued { observed_at_ms, .. })) if !listener => {
                send(&OutpostToSupervisor::Fault {
                    detail: format!("focus queued {observed_at_ms}"),
                });
            }
            Ok(Some(SupervisorToOutpost::Assign { target_pid, lock })) => {
                if lock_log.is_some() {
                    send(&OutpostToSupervisor::Fault {
                        detail: lock_report(lock),
                    });
                }
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
    fn start_own_outpost(&self) -> Option<String> {
        let own = Pid(std::process::id());
        self.supervisor.ensure_spawned(own);
        assert_eq!(self.expect_started(own), OWN);
        self.expect_ready(OWN, own);
        self.expect_report(OWN, DIRECT);
        // The lock it inherited, when the test asks for it.
        std::env::var_os(LOCK_LOG).map(|_| self.next_report(OWN))
    }

    /// [`Run::start_own_outpost`], then waits for the two spares, the
    /// launches after it.
    fn keep_spares(&self) {
        let _ = self.start_own_outpost();
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
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3), "the oldest");
    run.expect_ready(OutpostId(3), first_pid);
    // Its replacement started as it was given, and waits with the other.
    run.expect_spare(5, 2);
    let (second, second_pid) = application_and_pid();
    run.supervisor.ensure_spawned(second_pid);
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

    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    // Neither the ignored process's focus nor the other's drop took the
    // oldest spare.
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_report(OutpostId(3), "fact 3");
    run.expect_spare(5, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
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

fn a_spare_that_fails_after_it_is_given_hands_the_held_facts_to_the_next() {
    let failing = Failing::assignments(&[1], Failure::Exits, true);
    let (first, first_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{first_pid}:7"));
    // No application holds attention.
    let run = Run::start(&[]);
    run.keep_spares();

    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);
    failing.fail(1);
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the next spare"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_report(OutpostId(4), "fact 7");
    run.expect_spare(6, 2);

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
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
    run.expect_spare(5, 2);

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

/// An outpost that crashes after its `Ready`, in an application that does
/// not hold attention, is replaced by the next spare at once, and the
/// application's later facts reach the new one.
fn an_outpost_that_crashes_after_its_ready_is_replaced_by_a_spare_at_once() {
    let (first, first_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{first_pid}:7"));
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    let crashing = run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);

    kill(crashing);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(
        run.expect_started(first_pid),
        OutpostId(4),
        "the next spare"
    );
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);
    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    run.expect_report(OutpostId(4), "fact 7");

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    clear_env(FACTS);
    end(first);
}

/// An outpost that stops answering pings after its `Ready` is stuck: it is
/// ended after three ping intervals, as killed, and its application is
/// given the next spare.
fn a_stuck_outpost_is_ended_and_replaced() {
    let failing = Failing::assignments(&[1], Failure::MuteAfterReady, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Killed);
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

/// The next messages: an outpost started for each of `applications`, as
/// no spare is kept, each followed by its `Ready` and its report that it
/// was started for its application. Each outpost is launched on a thread of
/// its own and is a process of its own, so the app hears them start in the
/// order their launches finish, and one's messages may come between
/// another's; the order the supervisor decided to start them in is the
/// order of their incarnations' numbers. Returns each application's outpost
/// and its process id.
fn served<const N: usize>(run: &Run, applications: [Pid; N]) -> [(OutpostId, Pid); N] {
    // Each application's outpost, process, and how many of its three
    // messages have come, once it has started.
    let mut seen: [Option<(OutpostId, Pid, usize)>; N] = [None; N];
    while seen
        .iter()
        .any(|entry| entry.is_none_or(|(_, _, step)| step < 3))
    {
        match run.next() {
            OutpostMessage::Started {
                outpost,
                target_pid,
            } if applications
                .iter()
                .position(|&pid| pid == target_pid)
                .is_some_and(|index| seen[index].is_none()) =>
            {
                let index = applications
                    .iter()
                    .position(|&pid| pid == target_pid)
                    .expect("found above");
                seen[index] = Some((outpost, Pid(0), 1));
            }
            OutpostMessage::Event {
                outpost, message, ..
            } => {
                let Some(entry) = seen
                    .iter_mut()
                    .flatten()
                    .find(|(started, _, _)| *started == outpost)
                else {
                    panic!("{outpost}, not started, said {message:?}");
                };
                match (*message, entry.2) {
                    (OutpostToSupervisor::Ready { outpost_pid, .. }, 1) => {
                        *entry = (outpost, outpost_pid, 2);
                    }
                    (OutpostToSupervisor::Fault { detail }, 2) if detail == DIRECT => {
                        entry.2 = 3;
                    }
                    (other, step) => panic!("{outpost} said {other:?} at its step {step}"),
                }
            }
            other => panic!(
                "the supervisor said {other:?}, not that the outposts of {applications:?} started"
            ),
        }
    }
    seen.map(|entry| {
        let (outpost, process, _) = entry.expect("every one served");
        (outpost, process)
    })
}

/// The next `count` reports of stand-in outposts, each outpost's in the
/// order it sent them.
fn reports(run: &Run, count: usize) -> Vec<(OutpostId, Vec<String>)> {
    let mut reports: Vec<(OutpostId, Vec<String>)> = Vec::new();
    for _ in 0..count {
        match run.next() {
            OutpostMessage::Event {
                outpost, message, ..
            } => match *message {
                OutpostToSupervisor::Fault { detail } => {
                    match reports.iter_mut().find(|(from, _)| *from == outpost) {
                        Some((_, details)) => details.push(detail),
                        None => reports.push((outpost, vec![detail])),
                    }
                }
                other => panic!("{outpost} said {other:?}, not a report"),
            },
            other => panic!("the supervisor said {other:?}, not an outpost's report"),
        }
    }
    reports.sort_by_key(|(outpost, _)| *outpost);
    reports
}

/// A focus Core queued after another outpost's is told to the listener
/// and to that outpost, whose last focus it supersedes, and to no other
/// outpost (`docs/design/focus-pipeline.md`, section 16.3). The listener,
/// told, sends a fact to each application's outpost, which reaches each
/// behind anything told to it before.
fn a_queued_focus_is_told_to_the_outpost_it_supersedes_and_no_other() {
    let (first, first_pid) = application_and_pid();
    let (second, second_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{first_pid}:9,focus:{second_pid}:10"));
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);
    run.supervisor.ensure_spawned(second_pid);
    assert_eq!(run.expect_started(second_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), second_pid);
    run.expect_spare(6, 2);

    run.supervisor
        .focus_queued(TraceId::mint(), 42, Some(OutpostId(3)));
    assert_eq!(
        reports(&run, 3),
        [
            (
                OutpostId(3),
                vec!["focus queued 42".to_owned(), "fact 9".to_owned()]
            ),
            (OutpostId(4), vec!["fact 10".to_owned()]),
        ]
    );

    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    clear_env(FACTS);
    for application in [first, second] {
        end(application);
    }
}

/// A queued focus that supersedes an outpost that has since ended is told
/// to the listener alone: the incarnation that replaced it for its
/// application reported no focus, and hears nothing of it.
fn a_queued_focus_is_not_told_to_a_superseded_outpost_that_ended() {
    let (first, first_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{first_pid}:9"));
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    let crashing = run.expect_ready(OutpostId(3), first_pid);
    run.expect_spare(5, 2);
    kill(crashing);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
    assert_eq!(run.expect_started(first_pid), OutpostId(4));
    run.expect_ready(OutpostId(4), first_pid);
    run.expect_spare(6, 2);

    run.supervisor
        .focus_queued(TraceId::mint(), 42, Some(OutpostId(3)));
    assert_eq!(
        reports(&run, 1),
        [(OutpostId(4), vec!["fact 9".to_owned()])]
    );

    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    clear_env(FACTS);
    end(first);
}

/// Repeated failures of one application's outposts, which does not hold
/// attention, are each replaced and never given up, and are spaced by the
/// backoff: the first is replaced at once; the second waits a second, and
/// the third two, so a start for another application asked for after each
/// of their ends is decided first. A foreground change to the application
/// ends the fourth's wait, its fact held meanwhile reaching the next
/// outpost, and starts the count again, so the fifth is replaced at once,
/// decided before a start asked for after its end. No spare is kept, so
/// every outpost is started for its application.
fn repeated_failures_are_spaced_by_the_backoff_until_a_foreground_change() {
    let (first, first_pid) = application_and_pid();
    let others: Vec<(Child, Pid)> = (0..3).map(|_| application_and_pid()).collect();
    set_env(FACTS, format!("foreground:{first_pid}:7"));
    let run = Run::start(&[]);
    run.supervisor.ensure_spawned(first_pid);
    let [(mut outpost, mut process)] = served(&run, [first_pid]);

    // The first failure: replaced at once.
    kill(process);
    run.expect_ended(outpost, first_pid, EndReason::Exited);
    [(outpost, process)] = served(&run, [first_pid]);

    // The second and third: each waits, and another application's start
    // asked for after the end comes first.
    for (_, other_pid) in &others[..2] {
        kill(process);
        run.expect_ended(outpost, first_pid, EndReason::Exited);
        run.supervisor.ensure_spawned(*other_pid);
        let other;
        [other, (outpost, process)] = served(&run, [*other_pid, first_pid]);
        assert!(other.0 < outpost, "{other:?} was started first");
    }

    // The fourth waits four seconds, which the foreground change ends; the
    // change, held meanwhile, reaches the next outpost.
    kill(process);
    run.expect_ended(outpost, first_pid, EndReason::Exited);
    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    [(outpost, process)] = served(&run, [first_pid]);
    run.expect_report(outpost, "fact 7");

    // The count started again: the fifth is replaced at once, decided
    // before a start asked for after its end.
    kill(process);
    run.expect_ended(outpost, first_pid, EndReason::Exited);
    let (_, last_pid) = &others[2];
    run.supervisor.ensure_spawned(*last_pid);
    let [(replacement, _), (last, _)] = served(&run, [first_pid, *last_pid]);
    assert!(replacement < last, "{replacement} was started first");

    // The listener and the four live outposts; the five failed had exited.
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 5,
        killed: 0,
        killed_on_failure: 0,
    });
    clear_env(FACTS);
    end(first);
    for (other, _) in others {
        end(other);
    }
}

/// A given spare whose hook fails reports it and exits before its
/// `Ready`: its application's facts go to the next.
fn a_given_spare_whose_hook_fails_is_replaced() {
    let failing = Failing::assignments(&[1], Failure::HookFails, true);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_spare(5, 2);
    failing.fail(1);
    run.expect_report(OutpostId(3), HOOK_FAULT);
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Exited);
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
/// exited, ends as the application's exit: nothing takes its place.
fn a_given_spare_whose_hook_fails_after_its_application_exited_ends_with_it() {
    let failing = Failing::assignments(&[1], Failure::HookFailsAndApplicationExits, false);
    let run = Run::start(&[]);
    run.keep_spares();
    let (first, first_pid) = application_and_pid();
    run.supervisor.ensure_spawned(first_pid);
    assert_eq!(run.expect_started(first_pid), OutpostId(3));
    run.expect_report(OutpostId(3), HOOK_FAULT);
    run.expect_spare(5, 2);
    end(first);
    run.expect_ended(OutpostId(3), first_pid, EndReason::TargetExited);

    run.finish(ShutdownSummary {
        clean: 4,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
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
    run.expect_spare(5, 2);
    run.expect_reincarnated(OutpostId(3), OutpostId(4), first_pid);
    run.expect_ended(OutpostId(3), first_pid, EndReason::Killed);
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

/// The stand-in listeners' logs of a test ([`LOCK_LOG`]), set for its
/// children while it lives.
struct LockLogs {
    prefix: PathBuf,
}

impl LockLogs {
    fn new() -> Self {
        let prefix =
            std::env::temp_dir().join(format!("verbatim-spares-test-locks-{}", std::process::id()));
        set_env(LOCK_LOG, &prefix);
        Self { prefix }
    }

    /// Each listener's log, in the order the listeners started: its process
    /// id, and every command it noted.
    fn listeners(&self) -> Vec<(Pid, Vec<String>)> {
        (1..)
            .map_while(|number| {
                std::fs::read_to_string(format!("{}-{number}", self.prefix.display())).ok()
            })
            .map(|log| {
                let mut lines = log.lines().map(str::to_owned);
                let pid = lines
                    .next()
                    .and_then(|first| first.strip_prefix("listener ").map(str::to_owned))
                    .and_then(|pid| pid.parse().ok())
                    .map(Pid)
                    .expect("a listener's log names its process");
                (pid, lines.collect())
            })
            .collect()
    }

    /// The lock lines of a listener's commands, in order.
    fn locks(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.starts_with("app-lock"))
            .cloned()
            .collect()
    }

    fn done(self) {
        for number in 1.. {
            if std::fs::remove_file(format!("{}-{number}", self.prefix.display())).is_err() {
                break;
            }
        }
        clear_env(LOCK_LOG);
    }
}

/// A lock a stand-in listener noted for `pid`, created at `created`, its
/// handle `handle` in that listener: `app-lock <pid> <created> <handle>`.
fn lock_line(pid: Pid, created: u64, handle: u64) -> String {
    format!("app-lock {} {created} {handle}", pid.0)
}

/// The handle of the lock a listener's log noted for `pid`.
fn listener_handle(lines: &[String], pid: Pid) -> u64 {
    lines
        .iter()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            match fields.as_slice() {
                ["app-lock", noted, _, handle] if *noted == pid.0.to_string() => {
                    handle.parse().ok()
                }
                _ => None,
            }
        })
        .unwrap_or_else(|| panic!("the listener was sent {pid}'s lock: {lines:?}"))
}

/// `pid`'s process creation time, as the supervisor reads it.
fn created_of(pid: Pid) -> u64 {
    verbatim_uia::lock::open_process(pid.0)
        .expect("the process opens")
        .1
}

/// The handle `value` of the process `owner`, duplicated into this one.
fn handle_of(owner: Pid, value: u64) -> HANDLE {
    use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows::Win32::System::Threading::{GetCurrentProcess, PROCESS_DUP_HANDLE};
    // SAFETY: opens a process to duplicate a handle from; closed below.
    let process =
        unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, owner.0) }.expect("the process opens");
    // SAFETY: the pseudo-handle of this process.
    let this = unsafe { GetCurrentProcess() };
    let mut copy = HANDLE::default();
    // SAFETY: both processes are open; `copy` is a local the call writes.
    unsafe {
        DuplicateHandle(
            process,
            handle_value(value),
            this,
            &raw mut copy,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    }
    .expect("the handle is duplicated");
    close(process);
    copy
}

/// Whether the handles `first` and `second` of this process name the same
/// object.
fn same_object(first: HANDLE, second: HANDLE) -> bool {
    // SAFETY: two handles of this process's, open.
    unsafe { windows::Win32::Foundation::CompareObjectHandles(first, second) }.as_bool()
}

/// The fields of a stand-in outpost's lock report ([`lock_report`]): the
/// handle, the creation time, and the access.
fn reported_lock(detail: &str) -> (u64, u64, String) {
    let fields: Vec<&str> = detail.split(' ').collect();
    match fields.as_slice() {
        ["lock", handle, created, access] => (
            handle.parse().expect("a handle"),
            created.parse().expect("a creation time"),
            (*access).to_owned(),
        ),
        _ => panic!("{detail:?} is no lock report"),
    }
}

/// The access every holder of a lock is given: to wait and release
/// (`SYNCHRONIZE | MUTEX_MODIFY_STATE`).
const WAIT_AND_RELEASE: &str = "0x100001";

impl Run {
    /// The next message, a fault `outpost` sent; returns its detail.
    fn next_report(&self, outpost: OutpostId) -> String {
        match self.next() {
            OutpostMessage::Event {
                outpost: from,
                message,
                ..
            } if from == outpost => match *message {
                OutpostToSupervisor::Fault { detail } => detail,
                other => panic!("{outpost} said {other:?}, not a report"),
            },
            other => panic!("the supervisor said {other:?}, not a report of {outpost}'s"),
        }
    }

    /// The next messages, the spare `outpost`'s start for `target_pid`, its
    /// report of the lock `Assign` carried, and its `Ready`; returns the
    /// report and the outpost's process.
    fn expect_given(&self, outpost: OutpostId, target_pid: Pid) -> (String, Pid) {
        assert_eq!(self.expect_started(target_pid), outpost);
        let report = self.next_report(outpost);
        (report, self.expect_ready(outpost, target_pid))
    }
}

/// A lock is made with an application's first record, whichever path
/// starts it, Core's own outpost, a focus-now query, or a listener fact,
/// and the listener is sent each as it is made, with its application's
/// process creation time; the outposts are given the same applications'
/// locks.
fn every_way_an_application_is_first_recorded_makes_its_lock_and_tells_the_listener() {
    let logs = LockLogs::new();
    let (queried, queried_pid) = application_and_pid();
    let (focused, focused_pid) = application_and_pid();
    set_env(FACTS, format!("focus:{focused_pid}:1"));
    let own = Pid(std::process::id());
    let run = Run::start(&[]);
    let own_report = run.start_own_outpost().expect("the own outpost's lock");
    run.expect_two_spares([3, 4]);
    run.supervisor.ensure_spawned(queried_pid);
    let (queried_report, _) = run.expect_given(OutpostId(3), queried_pid);
    run.expect_spare(5, 2);
    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    let (focused_report, _) = run.expect_given(OutpostId(4), focused_pid);
    run.expect_report(OutpostId(4), "fact 1");
    run.expect_spare(6, 2);
    run.finish(ShutdownSummary {
        clean: 6,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });

    let listeners = logs.listeners();
    let [(_, lines)] = listeners.as_slice() else {
        panic!("one listener: {listeners:?}");
    };
    let expected: Vec<String> = [own, queried_pid, focused_pid]
        .into_iter()
        .map(|pid| lock_line(pid, created_of(pid), listener_handle(lines, pid)))
        .collect();
    assert_eq!(
        LockLogs::locks(lines),
        expected,
        "the listener's locks, as made"
    );
    for (report, pid) in [
        (own_report, own),
        (queried_report, queried_pid),
        (focused_report, focused_pid),
    ] {
        assert_eq!(
            reported_lock(&report).1,
            created_of(pid),
            "{pid}'s lock names its process"
        );
    }
    clear_env(FACTS);
    logs.done();
    for application in [queried, focused] {
        end(application);
    }
}

/// The outpost started for its application, Core's own, inherits a handle
/// to the same mutex the listener is sent, with only the rights to wait
/// and release, and the supervisor's own copy of it is closed once the
/// launch returns; a spare's `Assign` carries one too.
fn an_inherited_and_an_assigned_lock_are_the_listeners_mutex_with_wait_and_release_only() {
    let logs = LockLogs::new();
    let (queried, queried_pid) = application_and_pid();
    let own = Pid(std::process::id());
    let run = Run::start(&[]);
    run.supervisor.ensure_spawned(own);
    assert_eq!(run.expect_started(own), OWN);
    let own_outpost = run.expect_ready(OWN, own);
    run.expect_report(OWN, DIRECT);
    let (inherited, _, inherited_access) = reported_lock(&run.next_report(OWN));
    // The value the outpost inherited is the supervisor's copy's: in this
    // process it names no mutex any more.
    assert!(
        !names_a_mutex(handle_value(inherited)),
        "the supervisor's inheritable copy is closed"
    );
    run.expect_two_spares([3, 4]);
    run.supervisor.ensure_spawned(queried_pid);
    let (assigned_report, spare) = run.expect_given(OutpostId(3), queried_pid);
    let (assigned, _, assigned_access) = reported_lock(&assigned_report);
    run.expect_spare(5, 2);

    let listeners = logs.listeners();
    let [(listener, lines)] = listeners.as_slice() else {
        panic!("one listener: {listeners:?}");
    };
    let own_lock = handle_of(*listener, listener_handle(lines, own));
    let queried_lock = handle_of(*listener, listener_handle(lines, queried_pid));
    let inherited_lock = handle_of(own_outpost, inherited);
    let assigned_lock = handle_of(spare, assigned);
    assert_eq!(
        (
            same_object(own_lock, inherited_lock),
            same_object(queried_lock, assigned_lock),
            same_object(own_lock, queried_lock),
        ),
        (true, true, false),
        "each application's outpost and the listener hold its one mutex"
    );
    assert_eq!(
        (inherited_access.as_str(), assigned_access.as_str()),
        (WAIT_AND_RELEASE, WAIT_AND_RELEASE)
    );
    for handle in [own_lock, queried_lock, inherited_lock, assigned_lock] {
        close(handle);
    }
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    logs.done();
    end(queried);
}

/// Whether `handle`, a value in this process, is an open handle to a mutex.
fn names_a_mutex(handle: HANDLE) -> bool {
    let mut buffer = vec![0usize; 128];
    let length = u32::try_from(buffer.len() * size_of::<usize>()).unwrap_or(0);
    // SAFETY: the buffer outlives the call, which writes at most `length`
    // bytes of the type's information.
    let status = unsafe {
        windows::Wdk::Foundation::NtQueryObject(
            Some(handle),
            windows::Wdk::Foundation::OBJECT_INFORMATION_CLASS(2),
            Some(buffer.as_mut_ptr().cast()),
            length,
            None,
        )
    };
    if status.is_err() {
        return false;
    }
    // The information begins with the type's name, a `UNICODE_STRING`:
    // its length in bytes, then a pointer to its characters.
    let bytes = usize::from(u16::try_from(buffer[0] & 0xFFFF).unwrap_or(0));
    let characters = buffer[1] as *const u16;
    // SAFETY: the name lies in the buffer the call filled, `bytes` long.
    let name = unsafe { std::slice::from_raw_parts(characters, bytes / 2) };
    String::from_utf16_lossy(name) == "Mutant"
}

/// A replacement listener is sent every live lock, Core's own included,
/// before any other command.
fn a_replacement_listener_is_sent_every_live_lock_before_any_other_command() {
    let logs = LockLogs::new();
    let (queried, queried_pid) = application_and_pid();
    let own = Pid(std::process::id());
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(queried_pid);
    let _ = run.expect_given(OutpostId(3), queried_pid);
    run.expect_spare(5, 2);
    let first = logs.listeners()[0].0;
    kill(first);
    // Each outpost is told the ended listener's reads ended, which a
    // stand-in reports as a fact, around the replacement's readiness.
    let mut told = Vec::new();
    loop {
        match run.next() {
            OutpostMessage::ListenerReady { replacement: true } => break,
            OutpostMessage::Event {
                outpost, message, ..
            } if matches!(&*message, OutpostToSupervisor::Fault { detail } if detail.starts_with("fact ")) =>
            {
                told.push(outpost);
            }
            other => panic!("the supervisor said {other:?}, not that its listener was replaced"),
        }
    }
    for _ in told.len()..2 {
        match run.next() {
            OutpostMessage::Event {
                outpost, message, ..
            } if matches!(&*message, OutpostToSupervisor::Fault { detail } if detail.starts_with("fact ")) =>
            {
                told.push(outpost);
            }
            other => panic!("the supervisor said {other:?}, not that the listener's reads ended"),
        }
    }
    told.sort_unstable();
    assert_eq!(
        told,
        [OWN, OutpostId(3)],
        "the outposts told the reads ended"
    );
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });

    let listeners = logs.listeners();
    let [_, (_, lines)] = listeners.as_slice() else {
        panic!("two listeners: {listeners:?}");
    };
    let mut first_two: Vec<String> = lines.iter().take(2).cloned().collect();
    first_two.sort_unstable();
    let mut expected: Vec<String> = [own, queried_pid]
        .into_iter()
        .map(|pid| lock_line(pid, created_of(pid), listener_handle(lines, pid)))
        .collect();
    expected.sort_unstable();
    assert_eq!(first_two, expected, "the replacement's first commands");
    assert_eq!(
        LockLogs::locks(lines).len(),
        2,
        "and no other lock: {lines:?}"
    );
    logs.done();
    end(queried);
}

/// An outpost's replacement after its failure is given the same mutex as
/// the outpost that failed.
fn an_outposts_replacement_after_a_failure_gets_the_same_mutex() {
    let logs = LockLogs::new();
    let (application, pid) = application_and_pid();
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(pid);
    let (failed_report, failing) = run.expect_given(OutpostId(3), pid);
    run.expect_spare(5, 2);
    let listener = logs.listeners()[0].0;
    // The failing outpost's lock, held here, before it is ended.
    let failed_lock = handle_of(failing, reported_lock(&failed_report).0);
    kill(failing);
    run.expect_ended(OutpostId(3), pid, EndReason::Exited);
    let (next_report, next) = run.expect_given(OutpostId(4), pid);
    run.expect_spare(6, 2);
    let next_lock = handle_of(next, reported_lock(&next_report).0);
    assert!(
        same_object(failed_lock, next_lock),
        "the replacement holds the same mutex"
    );
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 1,
        killed: 0,
        killed_on_failure: 0,
    });
    let listeners = logs.listeners();
    let [(_, lines)] = listeners.as_slice() else {
        panic!("one listener: {listeners:?}");
    };
    let _ = listener;
    assert_eq!(
        LockLogs::locks(lines)
            .iter()
            .filter(|line| line.starts_with(&format!("app-lock {} ", pid.0)))
            .count(),
        1,
        "one lock for the application: {lines:?}"
    );
    for handle in [failed_lock, next_lock] {
        close(handle);
    }
    logs.done();
    end(application);
}

/// A fact for a process that has exited starts no outpost, and the
/// listener is answered with no lock and the process's creation time,
/// which the supervisor read from the process it could still open.
fn a_fact_for_an_exited_process_is_answered_with_no_lock_and_its_creation_time() {
    let logs = LockLogs::new();
    let (mut exited, pid) = application_and_pid();
    let created = created_of(pid);
    drop(exited.stdin.take());
    assert!(exited.wait().expect("the application exits").success());
    // `exited` still holds the process open, so its id is not reused.
    set_env(FACTS, format!("focus:{pid}:1"));
    let run = Run::start(&[]);
    run.supervisor.focus_queued(TraceId::mint(), 0, None);
    match run.next() {
        OutpostMessage::NotWatched { target_pid } if target_pid == pid => {}
        other => panic!("the supervisor said {other:?}, not that {pid} is not watched"),
    }
    run.finish(ShutdownSummary {
        clean: 1,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    let listeners = logs.listeners();
    let [(_, lines)] = listeners.as_slice() else {
        panic!("one listener: {listeners:?}");
    };
    assert_eq!(
        LockLogs::locks(lines),
        [format!("app-lock {} {created} -", pid.0)]
    );
    clear_env(FACTS);
    logs.done();
    drop(exited);
}

/// An application's exit closes its lock in the supervisor and tells the
/// listener, behind the lock it was sent.
fn an_applications_exit_closes_its_lock_and_tells_the_listener() {
    let logs = LockLogs::new();
    let (application, pid) = application_and_pid();
    let created = created_of(pid);
    let run = Run::start(&[]);
    run.keep_spares();
    run.supervisor.ensure_spawned(pid);
    let _ = run.expect_given(OutpostId(3), pid);
    run.expect_spare(5, 2);
    end(application);
    run.expect_ended(OutpostId(3), pid, EndReason::TargetExited);
    run.finish(ShutdownSummary {
        clean: 5,
        exited: 0,
        killed: 0,
        killed_on_failure: 0,
    });
    let listeners = logs.listeners();
    let [(_, lines)] = listeners.as_slice() else {
        panic!("one listener: {listeners:?}");
    };
    let own = Pid(std::process::id());
    assert_eq!(
        LockLogs::locks(lines),
        [
            lock_line(own, created_of(own), listener_handle(lines, own)),
            lock_line(pid, created, listener_handle(lines, pid)),
            format!("app-lock-closed {} {created}", pid.0),
        ]
    );
    logs.done();
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
    let tests: [(&str, fn()); 20] = [
        (
            "every_way_an_application_is_first_recorded_makes_its_lock_and_tells_the_listener",
            every_way_an_application_is_first_recorded_makes_its_lock_and_tells_the_listener,
        ),
        (
            "an_inherited_and_an_assigned_lock_are_the_listeners_mutex_with_wait_and_release_only",
            an_inherited_and_an_assigned_lock_are_the_listeners_mutex_with_wait_and_release_only,
        ),
        (
            "a_replacement_listener_is_sent_every_live_lock_before_any_other_command",
            a_replacement_listener_is_sent_every_live_lock_before_any_other_command,
        ),
        (
            "an_outposts_replacement_after_a_failure_gets_the_same_mutex",
            an_outposts_replacement_after_a_failure_gets_the_same_mutex,
        ),
        (
            "a_fact_for_an_exited_process_is_answered_with_no_lock_and_its_creation_time",
            a_fact_for_an_exited_process_is_answered_with_no_lock_and_its_creation_time,
        ),
        (
            "an_applications_exit_closes_its_lock_and_tells_the_listener",
            an_applications_exit_closes_its_lock_and_tells_the_listener,
        ),
        (
            "a_start_takes_the_oldest_spare_and_its_replacement_starts_at_once",
            a_start_takes_the_oldest_spare_and_its_replacement_starts_at_once,
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
            "an_outpost_that_crashes_after_its_ready_is_replaced_by_a_spare_at_once",
            an_outpost_that_crashes_after_its_ready_is_replaced_by_a_spare_at_once,
        ),
        (
            "a_stuck_outpost_is_ended_and_replaced",
            a_stuck_outpost_is_ended_and_replaced,
        ),
        (
            "repeated_failures_are_spaced_by_the_backoff_until_a_foreground_change",
            repeated_failures_are_spaced_by_the_backoff_until_a_foreground_change,
        ),
        (
            "a_given_spare_whose_hook_fails_is_replaced",
            a_given_spare_whose_hook_fails_is_replaced,
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
            "a_queued_focus_is_told_to_the_outpost_it_supersedes_and_no_other",
            a_queued_focus_is_told_to_the_outpost_it_supersedes_and_no_other,
        ),
        (
            "a_queued_focus_is_not_told_to_a_superseded_outpost_that_ended",
            a_queued_focus_is_not_told_to_a_superseded_outpost_that_ended,
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
