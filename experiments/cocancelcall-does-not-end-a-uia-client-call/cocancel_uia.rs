//! Scratch experiment, not a test: does `CoCancelCall` end a UIA client
//! call blocked in an application that does not answer? Kept with its
//! record under `experiments/`; each experiment prints `RESULT` lines and
//! fails only when its setup does.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::os::windows::io::AsRawHandle;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use verbatim_uia_rops::{Builder, Error as RopError};
use windows::Win32::Foundation::{HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCancelCall, CoCreateInstance, CoDisableCallCancellation,
    CoEnableCallCancellation,
};
use windows::Win32::System::Threading::{GetCurrentThreadId, WaitForSingleObject};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextRange, TreeScope_Descendants, UIA_NamePropertyId, UIA_TextPatternId,
};
use windows::core::BSTR;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// `CurrentName` on the text element.
    Property,
    /// `IUIAutomation::GetFocusedElement`.
    Focus,
    /// `GetText(-1)` on the text element's document range.
    Text,
    /// `FindAll(TreeScope_Descendants, true)` from the window's element.
    FindAll,
    /// A remote operation reading the text element's name.
    RemoteOp,
    /// `ElementFromHandle` on a window this client has not connected to.
    NewConnection,
}

enum Job {
    /// Creates the client; connects to the window and finds the text
    /// element and its range unless told not to.
    Setup(isize, bool),
    Call(Kind),
    /// 30 follow-up calls: the element's name and a new lookup's name.
    FollowUps,
    /// `CurrentName` on the text element that many times.
    Names(u32),
    Enable,
    Disable,
}

struct Outcome {
    started: Instant,
    ended: Instant,
    result: String,
}

struct Caller {
    jobs: Sender<Job>,
    started: Receiver<Instant>,
    done: Receiver<Outcome>,
    thread_id: u32,
}

fn hr(error: &windows::core::Error) -> String {
    format!("{:#010x}", error.code().0.cast_unsigned())
}

fn hr_of(result: windows::core::Result<()>) -> String {
    match result {
        Ok(()) => "S_OK".to_owned(),
        Err(error) => hr(&error),
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

struct Client {
    automation: IUIAutomation,
    hwnd: HWND,
    element: Option<IUIAutomationElement>,
    root: Option<IUIAutomationElement>,
    range: Option<IUIAutomationTextRange>,
}

impl Client {
    fn new(hwnd: isize, connect: bool) -> windows::core::Result<Self> {
        // SAFETY: an in-process class.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
        let hwnd = HWND(hwnd as *mut std::ffi::c_void);
        let mut client = Self {
            automation,
            hwnd,
            element: None,
            root: None,
            range: None,
        };
        if connect {
            // SAFETY: live interfaces throughout.
            unsafe {
                let root = client.automation.ElementFromHandle(hwnd)?;
                let value = VARIANT::from(BSTR::from("Notes"));
                let condition = client
                    .automation
                    .CreatePropertyCondition(UIA_NamePropertyId, &value)?;
                let doc = root.FindFirst(TreeScope_Descendants, &condition)?;
                let pattern: IUIAutomationTextPattern =
                    doc.GetCurrentPatternAs(UIA_TextPatternId)?;
                client.range = Some(pattern.DocumentRange()?);
                client.element = Some(doc);
                client.root = Some(root);
            }
        }
        Ok(client)
    }

    fn call(&mut self, kind: Kind) -> String {
        // SAFETY: live interfaces throughout.
        unsafe {
            match kind {
                Kind::Property => match self.element.as_ref().unwrap().CurrentName() {
                    Ok(name) => format!("ok {name}"),
                    Err(error) => format!("failed {}", hr(&error)),
                },
                Kind::Focus => match self.automation.GetFocusedElement() {
                    Ok(_) => "ok".to_owned(),
                    Err(error) => format!("failed {}", hr(&error)),
                },
                Kind::Text => match self.range.as_ref().unwrap().GetText(-1) {
                    Ok(text) => format!("ok {:?}", text.to_string()),
                    Err(error) => format!("failed {}", hr(&error)),
                },
                Kind::FindAll => {
                    let condition = self.automation.CreateTrueCondition().unwrap();
                    match self
                        .root
                        .as_ref()
                        .unwrap()
                        .FindAll(TreeScope_Descendants, &condition)
                    {
                        Ok(found) => format!("ok {}", found.Length().unwrap_or(-1)),
                        Err(error) => format!("failed {}", hr(&error)),
                    }
                }
                Kind::RemoteOp => {
                    let mut b = Builder::new();
                    let element = b.import_element(self.element.as_ref().unwrap());
                    let name = b.property(element, UIA_NamePropertyId.0);
                    b.add_to_results(name);
                    match b.finish().execute() {
                        Ok(_) => "ok".to_owned(),
                        Err(RopError::Execute(error)) => format!("failed execute {}", hr(&error)),
                        Err(RopError::Import(error)) => format!("failed import {}", hr(&error)),
                        Err(RopError::Failed(failure)) => format!(
                            "failed status {:?} {:#010x}",
                            failure.status,
                            failure.extended_error.0.cast_unsigned()
                        ),
                        Err(other) => format!("failed {other}"),
                    }
                }
                Kind::NewConnection => match self.automation.ElementFromHandle(self.hwnd) {
                    Ok(element) => {
                        // The provider's own element is named by the
                        // fixture's root; UIA's window proxy by the title.
                        let described = element
                            .CurrentProviderDescription()
                            .map(|d| d.to_string())
                            .unwrap_or_default();
                        let proxy = described.contains("Proxy");
                        self.element = Some(element);
                        format!("ok proxy={proxy}")
                    }
                    Err(error) => format!("failed {}", hr(&error)),
                },
            }
        }
    }

    fn follow_ups(&self) -> String {
        let mut element_failed = 0;
        let mut client_failed = 0;
        let mut errors = std::collections::BTreeSet::new();
        let mut slowest = Duration::ZERO;
        for _ in 0..30 {
            let at = Instant::now();
            // SAFETY: live interfaces.
            unsafe {
                match self.element.as_ref().map(|e| e.CurrentName()) {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        element_failed += 1;
                        errors.insert(hr(&error));
                    }
                    None => element_failed += 1,
                }
                match self
                    .automation
                    .ElementFromHandle(self.hwnd)
                    .and_then(|e| e.CurrentName())
                {
                    Ok(_) => {}
                    Err(error) => {
                        client_failed += 1;
                        errors.insert(hr(&error));
                    }
                }
            }
            slowest = slowest.max(at.elapsed());
        }
        format!(
            "element_failed={element_failed} client_failed={client_failed} errors={errors:?} slowest_pair_ms={}",
            ms(slowest)
        )
    }
}

impl Caller {
    fn new(enable: bool) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (started_tx, started) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let (id_tx, id_rx) = mpsc::channel();
        std::thread::spawn(move || {
            common::init_com();
            if enable {
                // SAFETY: no reserved pointer.
                unsafe { CoEnableCallCancellation(None) }.expect("CoEnableCallCancellation");
            }
            // SAFETY: no arguments.
            id_tx.send(unsafe { GetCurrentThreadId() }).unwrap();
            let mut client: Option<Client> = None;
            while let Ok(job) = job_rx.recv() {
                let started = Instant::now();
                let _ = started_tx.send(started);
                let result = match job {
                    Job::Setup(hwnd, connect) => match Client::new(hwnd, connect) {
                        Ok(made) => {
                            client = Some(made);
                            "ok".to_owned()
                        }
                        Err(error) => format!("failed {}", hr(&error)),
                    },
                    Job::Call(kind) => client.as_mut().unwrap().call(kind),
                    Job::FollowUps => client.as_ref().unwrap().follow_ups(),
                    Job::Names(times) => {
                        let element = client.as_ref().unwrap().element.clone().unwrap();
                        let mut failed = 0;
                        for _ in 0..times {
                            // SAFETY: a live element.
                            if unsafe { element.CurrentName() }.is_err() {
                                failed += 1;
                            }
                        }
                        format!("failed {failed}")
                    }
                    // SAFETY: no reserved pointer.
                    Job::Enable => hr_of(unsafe { CoEnableCallCancellation(None) }),
                    // SAFETY: no reserved pointer.
                    Job::Disable => hr_of(unsafe { CoDisableCallCancellation(None) }),
                };
                let ended = Instant::now();
                if done_tx
                    .send(Outcome {
                        started,
                        ended,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        let thread_id = id_rx.recv().unwrap();
        Self {
            jobs,
            started,
            done,
            thread_id,
        }
    }

    fn start(&self, job: Job) -> Instant {
        self.jobs.send(job).unwrap();
        self.started.recv().unwrap()
    }

    fn finish(&self) -> Outcome {
        self.done
            .recv_timeout(Duration::from_secs(90))
            .expect("the call returned within 90 s")
    }

    fn finish_within(&self, wait: Duration) -> Option<Outcome> {
        self.done.recv_timeout(wait).ok()
    }

    fn run(&self, job: Job) -> Outcome {
        self.start(job);
        self.finish()
    }
}

fn cancel(caller: &Caller) -> String {
    // SAFETY: a thread id and a timeout of 0, as NVDA passes.
    hr_of(unsafe { CoCancelCall(caller.thread_id, 0) })
}

/// Cancels once, or every 50 ms as NVDA's watchdog repeats `CoCancelCall`
/// (`RECOVER_ATTEMPT_INTERVAL`), until the call returns or `limit` passes.
fn cancel_for(caller: &Caller, repeat: bool, limit: Duration) -> (Option<Outcome>, Vec<String>) {
    let begun = Instant::now();
    let mut answers = vec![cancel(caller)];
    loop {
        let wait = if repeat {
            Duration::from_millis(50)
        } else {
            limit.saturating_sub(begun.elapsed())
        };
        if let Some(outcome) = caller.finish_within(wait) {
            return (Some(outcome), answers);
        }
        if begun.elapsed() >= limit {
            return (None, answers);
        }
        answers.push(cancel(caller));
    }
}

fn summarize(answers: &[String]) -> String {
    let mut counts = std::collections::BTreeMap::<&str, u32>::new();
    for answer in answers {
        *counts.entry(answer.as_str()).or_default() += 1;
    }
    format!("{counts:?}")
}

const STALL: Duration = Duration::from_millis(3000);
const HOLD_LIMIT: Duration = Duration::from_secs(5);
const TRIALS: u32 = 10;

fn fixture(prefix: &str) -> (common::MockApp, HWND) {
    let title = common::unique_title(prefix);
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("focus doc");
    (app, hwnd)
}

/// One trial of `kind` under `arm`: `hold-once`, `hold-repeat`,
/// `stall-once`, `stall-repeat`, or `control` (held, released after 300 ms,
/// no cancel).
fn trial(app: &mut common::MockApp, hwnd: HWND, caller: &Caller, kind: Kind, arm: &str, n: u32) {
    common::reset_hits(hwnd);
    let repeat = arm.ends_with("repeat");
    let line;
    if arm.starts_with("hold") || arm == "control" {
        app.hold();
        let started = caller.start(Job::Call(kind));
        let held = app.held();
        let held_after = started.elapsed();
        if arm == "control" {
            std::thread::sleep(Duration::from_millis(300));
            app.release();
            let outcome = caller.finish();
            line = format!(
                "held={held} held_after_ms={} call_ms={} result={}",
                ms(held_after),
                ms(outcome.ended - outcome.started),
                outcome.result
            );
        } else {
            let first = Instant::now();
            let (early, answers) = cancel_for(caller, repeat, HOLD_LIMIT);
            let returned_before_release = early.is_some();
            app.release();
            let released = Instant::now();
            let outcome = early.unwrap_or_else(|| caller.finish());
            line = format!(
                "held={held} held_after_ms={} cancels={} answers={} returned_before_release={returned_before_release} after_first_cancel_ms={} after_release_ms={} result={}",
                ms(held_after),
                answers.len(),
                summarize(&answers),
                ms(outcome.ended.saturating_duration_since(first)),
                if returned_before_release { "-".to_owned() } else { ms(outcome.ended.saturating_duration_since(released)) },
                outcome.result
            );
        }
    } else {
        app.stall(STALL);
        let stalled = Instant::now();
        let started = caller.start(Job::Call(kind));
        std::thread::sleep(Duration::from_millis(300));
        let first = Instant::now();
        let (early, answers) = cancel_for(caller, repeat, STALL + Duration::from_secs(1));
        let outcome = early.unwrap_or_else(|| caller.finish());
        app.stall_ended(STALL);
        let stall_end = stalled.elapsed();
        line = format!(
            "cancels={} answers={} after_first_cancel_ms={} call_ms={} stall_seen_end_ms={} result={}",
            answers.len(),
            summarize(&answers),
            ms(outcome.ended.saturating_duration_since(first)),
            ms(outcome.ended - started),
            ms(stall_end),
            outcome.result
        );
    }
    // What the provider ran for the trial, the cancelled call's remainder
    // included, once it has had time to go on.
    std::thread::sleep(Duration::from_millis(200));
    let hits = format!("{:?}", common::read_hits(hwnd));
    let follow = caller.run(Job::FollowUps).result;
    println!("RESULT {kind:?} {arm} trial={n} {line} hits_after={hits} follow_ups: {follow}");
}

fn run_kind(kind: Kind) {
    common::init_com();
    let (mut app, hwnd) = fixture("cancel-uia");
    let caller = Caller::new(true);
    let setup = caller.run(Job::Setup(hwnd.0 as isize, true)).result;
    println!("RESULT {kind:?} setup {setup}");
    let first = caller.run(Job::Call(kind)).result;
    println!("RESULT {kind:?} uncancelled first {first}");
    for arm in ["control", "hold-once", "hold-repeat", "stall-once", "stall-repeat"] {
        let trials = if arm == "control" { 3 } else { TRIALS };
        for n in 1..=trials {
            trial(&mut app, hwnd, &caller, kind, arm, n);
        }
    }
    app.quit();
}

fn e_property() {
    run_kind(Kind::Property);
}
fn e_focus() {
    run_kind(Kind::Focus);
}
fn e_text() {
    run_kind(Kind::Text);
}
fn e_findall() {
    run_kind(Kind::FindAll);
}
fn e_remote_op() {
    run_kind(Kind::RemoteOp);
}

/// The first call on a new connection: a new mockapp for every trial.
fn e_new_connection() {
    common::init_com();
    let caller = Caller::new(true);
    for arm in ["control", "hold-once", "hold-repeat", "stall-once", "stall-repeat"] {
        let trials = if arm == "control" { 3 } else { TRIALS };
        for n in 1..=trials {
            let (mut app, hwnd) = fixture("cancel-uia-new");
            let setup = caller.run(Job::Setup(hwnd.0 as isize, false)).result;
            assert_eq!(setup, "ok");
            trial(&mut app, hwnd, &caller, Kind::NewConnection, arm, n);
            app.quit();
        }
    }
}

/// The control: a caller that did not enable cancellation, held.
fn e_disabled_control() {
    common::init_com();
    let (mut app, hwnd) = fixture("cancel-uia-disabled");
    let caller = Caller::new(false);
    let _ = caller.run(Job::Setup(hwnd.0 as isize, true));
    for n in 1..=3 {
        trial(&mut app, hwnd, &caller, Kind::Property, "hold-once", n);
    }
    println!("RESULT idle-cancel answer={}", cancel(&caller));
    app.quit();
}

/// 1,000 names through one element, cancellation disabled and enabled in
/// turn, 6 rounds.
fn e_cost() {
    common::init_com();
    let (app, hwnd) = fixture("cancel-uia-cost");
    let caller = Caller::new(false);
    let _ = caller.run(Job::Setup(hwnd.0 as isize, true));
    let _ = caller.run(Job::Names(200));
    for round in 1..=6 {
        for enabled in [false, true] {
            let toggle = caller.run(if enabled { Job::Enable } else { Job::Disable });
            let outcome = caller.run(Job::Names(1000));
            println!(
                "RESULT cost round={round} enabled={enabled} toggle={} toggle_us={:.1} names_ms={} {}",
                toggle.result,
                (toggle.ended - toggle.started).as_secs_f64() * 1e6,
                ms(outcome.ended - outcome.started),
                outcome.result
            );
        }
    }
    app.quit();
}

/// A child: 10 held name reads, each cancelled every 50 ms (or, for the
/// control, released after 300 ms), then a normal exit that runs DLL
/// detach code.
fn exit_child(cancelled: bool) {
    common::init_com();
    let (mut app, hwnd) = fixture("cancel-uia-exit");
    let caller = Caller::new(true);
    let _ = caller.run(Job::Setup(hwnd.0 as isize, true));
    for _ in 0..10 {
        app.hold();
        caller.start(Job::Call(Kind::Property));
        let _ = app.held();
        if cancelled {
            let (early, _) = cancel_for(&caller, true, Duration::from_millis(500));
            app.release();
            let _ = early.unwrap_or_else(|| caller.finish());
        } else {
            std::thread::sleep(Duration::from_millis(300));
            app.release();
            let _ = caller.finish();
        }
        let _ = caller.run(Job::FollowUps);
    }
    app.quit();
    std::process::exit(0);
}

fn exit_child_cancelled() {
    exit_child(true);
}
fn exit_child_control() {
    exit_child(false);
}

/// 10 children of each kind, each waited for up to 60 s.
fn e_exit() {
    let exe = std::env::current_exe().unwrap();
    for name in ["exit_child_cancelled", "exit_child_control"] {
        for n in 1..=10 {
            let started = Instant::now();
            let mut child = std::process::Command::new(&exe)
                .args(["--isolated-test", name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let handle = HANDLE(child.as_raw_handle());
            // SAFETY: the child's live process handle.
            let waited = unsafe { WaitForSingleObject(handle, 60_000) };
            let status = if waited == WAIT_OBJECT_0 {
                let code = child.wait().unwrap().code().unwrap_or(-1);
                format!("exited {:#010x}", code.cast_unsigned())
            } else {
                let _ = child.kill();
                let _ = child.wait();
                "hung, killed after 60 s".to_owned()
            };
            println!(
                "RESULT exit {name} run={n} {status} after_ms={}",
                ms(started.elapsed())
            );
        }
    }
}


/// Where a held call waits: the stacks of this process's threads, taken
/// with cdb (non-invasive) while a name read and a FindAll are held.
fn e_stacks() {
    common::init_com();
    let (mut app, hwnd) = fixture("cancel-uia-stacks");
    let caller = Caller::new(true);
    let _ = caller.run(Job::Setup(hwnd.0 as isize, true));
    let cdb = std::env::var("CDB").expect("CDB names cdb.exe");
    for kind in [Kind::Property, Kind::FindAll] {
        app.hold();
        caller.start(Job::Call(kind));
        let held = app.held();
        let answer = cancel(&caller);
        println!("RESULT stacks {kind:?} held={held} caller_thread={:#x} cancel_answer={answer}", caller.thread_id);
        let output = std::process::Command::new(&cdb)
            .args([
                "-pv",
                "-p",
                &std::process::id().to_string(),
                "-y",
                r"srv*C:\Users\Dickson\Desktop\projects\cancel-out\symbols*https://msdl.microsoft.com/download/symbols",
                "-c",
                "~*k 40; qd",
            ])
            .output()
            .expect("cdb ran");
        println!("{}", String::from_utf8_lossy(&output.stdout));
        app.release();
        let outcome = caller.finish();
        println!("RESULT stacks {kind:?} result={}", outcome.result);
    }
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        ("e_property", e_property),
        ("e_focus", e_focus),
        ("e_text", e_text),
        ("e_findall", e_findall),
        ("e_remote_op", e_remote_op),
        ("e_new_connection", e_new_connection),
        ("e_disabled_control", e_disabled_control),
        ("e_cost", e_cost),
        ("e_exit", e_exit),
        ("e_stacks", e_stacks),
        ("exit_child_cancelled", exit_child_cancelled),
        ("exit_child_control", exit_child_control),
    ]);
}
