//! Scratch experiment, not a test: can `CoCancelCall` end an MSAA call
//! blocked in a hung application? Kept with its record under
//! `experiments/`; each experiment prints `RESULT` lines and fails only when
//! its setup does.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use common::outpost::OutpostUnderTest;
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, SupervisorToOutpost};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCancelCall, CoDisableCallCancellation, CoEnableCallCancellation};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VT_I4};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromEvent, AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, IsHungAppWindow, OBJID_CLIENT};
use windows::core::{Interface, w};

/// A job for the caller thread.
enum Job {
    /// Acquire the client object of a window with `AccessibleObjectFromWindow`
    /// and keep it as the proxy later jobs use.
    Acquire(isize),
    /// `accName(CHILDID_SELF)` on the kept proxy.
    Name,
    /// `accName` that many times, uncancelled.
    NameTimes(u32),
    /// `AccessibleObjectFromEvent(hwnd, idObject, idChild)`.
    FromEvent(isize, i32, i32),
    /// `AccessibleObjectFromWindow(hwnd, OBJID_CLIENT)`.
    FromWindow(isize),
    Enable,
    Disable,
    /// Release the kept proxy.
    Drop,
}

struct Outcome {
    started: Instant,
    ended: Instant,
    result: String,
}

/// A thread in the multithreaded apartment, as the outpost's worker is,
/// making the calls it is given one at a time.
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
            let mut proxy: Option<IAccessible> = None;
            let me = VARIANT::from(0i32);
            while let Ok(job) = job_rx.recv() {
                let started = Instant::now();
                let _ = started_tx.send(started);
                let result = match job {
                    Job::Acquire(hwnd) => match from_window(hwnd) {
                        Ok(object) => {
                            let returned = started.elapsed();
                            let swap = Instant::now();
                            proxy = Some(object);
                            format!("acquired returned_ms={} old_release_ms={}", ms(returned), ms(swap.elapsed()))
                        }
                        Err(error) => format!("failed {}", hr(&error)),
                    },
                    Job::Name => {
                        // SAFETY: a live proxy; CHILDID_SELF.
                        match unsafe { proxy.as_ref().expect("a proxy").get_accName(&me) } {
                            Ok(name) => format!("ok {name:?}"),
                            Err(error) => format!("failed {}", hr(&error)),
                        }
                    }
                    Job::NameTimes(times) => {
                        let object = proxy.as_ref().expect("a proxy");
                        let mut failed = 0;
                        for _ in 0..times {
                            // SAFETY: as above.
                            if unsafe { object.get_accName(&me) }.is_err() {
                                failed += 1;
                            }
                        }
                        format!("failed {failed}")
                    }
                    Job::FromEvent(hwnd, id_object, id_child) => {
                        let mut object: Option<IAccessible> = None;
                        let mut child = VARIANT::default();
                        // SAFETY: out-parameters are locals.
                        let result = unsafe {
                            AccessibleObjectFromEvent(
                                HWND(hwnd as *mut c_void),
                                id_object.cast_unsigned(),
                                id_child.cast_unsigned(),
                                &raw mut object,
                                &raw mut child,
                            )
                        };
                        match result {
                            Ok(()) => {
                                let returned = started.elapsed();
                                let text = format!(
                                    "ok object={} child={}",
                                    object.is_some(),
                                    describe_variant(&child)
                                );
                                let swap = Instant::now();
                                proxy = object;
                                format!("{text} returned_ms={} old_release_ms={}", ms(returned), ms(swap.elapsed()))
                            }
                            Err(error) => format!("failed {}", hr(&error)),
                        }
                    }
                    Job::FromWindow(hwnd) => match from_window(hwnd) {
                        Ok(object) => {
                            let returned = started.elapsed();
                            let swap = Instant::now();
                            proxy = Some(object);
                            format!("ok returned_ms={} old_release_ms={}", ms(returned), ms(swap.elapsed()))
                        }
                        Err(error) => format!("failed {}", hr(&error)),
                    },
                    Job::Drop => {
                        let had = proxy.is_some();
                        proxy = None;
                        format!("dropped had={had}")
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

    /// Gives the job and returns when the thread started it.
    fn start(&self, job: Job) -> Instant {
        self.jobs.send(job).unwrap();
        self.started.recv().unwrap()
    }

    fn finish(&self) -> Outcome {
        self.done
            .recv_timeout(Duration::from_secs(60))
            .expect("the call returned within a minute")
    }

    fn finish_within(&self, wait: Duration) -> Option<Outcome> {
        self.done.recv_timeout(wait).ok()
    }

    fn run(&self, job: Job) -> Outcome {
        self.start(job);
        self.finish()
    }
}

fn describe_variant(variant: &VARIANT) -> String {
    let vt = variant.vt();
    if vt == VT_I4 {
        format!("VT_I4 {}", i32::try_from(variant).unwrap_or(-1))
    } else if vt == VT_DISPATCH {
        "VT_DISPATCH".to_owned()
    } else {
        format!("vt {}", vt.0)
    }
}

fn from_window(hwnd: isize) -> windows::core::Result<IAccessible> {
    let mut object: Option<IAccessible> = None;
    // SAFETY: the out-pointer is a local Option<IAccessible>.
    unsafe {
        AccessibleObjectFromWindow(
            HWND(hwnd as *mut c_void),
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            (&raw mut object).cast::<*mut c_void>(),
        )
    }?;
    object.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

fn ms(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

/// Cancels the caller's call: returns when `CoCancelCall` was made, how
/// long it took, and what it answered.
fn cancel(caller: &Caller, timeout_s: u32) -> (Instant, Duration, String) {
    let at = Instant::now();
    // SAFETY: a thread id and a timeout.
    let answer = hr_of(unsafe { CoCancelCall(caller.thread_id, timeout_s) });
    (at, at.elapsed(), answer)
}

/// Cancels the caller's call every 50 ms, as NVDA's watchdog repeats
/// `CoCancelCall` (`RECOVER_ATTEMPT_INTERVAL`), until it returns or
/// `limit` passes; returns the outcome, if any, and each cancel's answer.
fn cancel_repeatedly(caller: &Caller, limit: Duration) -> (Option<Outcome>, Vec<String>) {
    let begun = Instant::now();
    let mut answers = Vec::new();
    loop {
        let (_, _, answer) = cancel(caller, 0);
        answers.push(answer);
        if let Some(outcome) = caller.finish_within(Duration::from_millis(50)) {
            return (Some(outcome), answers);
        }
        if begun.elapsed() >= limit {
            return (None, answers);
        }
    }
}

/// Experiment 1 and 2 with NVDA's repeated cancels: the first call on a
/// new proxy, and `AccessibleObjectFromEvent` with a child id, each with
/// its first provider call held.
fn e12_repeated_cancels() {
    common::init_com();
    // SAFETY: set before mockapp is started; it inherits it.
    unsafe { std::env::set_var("MOCKAPP_HOLD_PAST_GETOBJECT", "1") };
    let title = common::unique_title("cancel-repeated");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    for arm in ["first_call", "from_event_child1"] {
        for trial in 1..=10 {
            if arm == "first_call" {
                let _ = caller.run(Job::Acquire(hwnd.0 as isize));
            }
            common::reset_hits(hwnd);
            app.hold();
            let job = if arm == "first_call" {
                Job::Name
            } else {
                Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 1)
            };
            let started = caller.start(job);
            let held = app.held();
            let first_cancel = Instant::now();
            let (early, answers) = cancel_repeatedly(&caller, Duration::from_secs(5));
            let returned_before_release = early.is_some();
            app.release();
            let outcome = early.unwrap_or_else(|| caller.finish());
            let named = if outcome.result.starts_with("ok") { caller.run(Job::Name).result } else { "-".to_owned() };
            let hits = format!("{:?}", common::read_hits(hwnd));
            println!(
                "RESULT e12 {arm} trial={trial} held={held} cancels={} answers={answers:?} returned_before_release={returned_before_release} after_first_cancel_ms={} call_ms={} result={} its_name={named} hits_after={hits}",
                answers.len(),
                ms(outcome.ended.saturating_duration_since(first_cancel)),
                ms(outcome.ended - started),
                outcome.result,
            );
        }
    }
    app.quit();
}

/// Experiment 1, releasing a proxy while the window thread is stalled:
/// the final `Release` of a proxy, cancelled every 50 ms from 300 ms in.
fn e1_release_in_a_stall() {
    common::init_com();
    let title = common::unique_title("cancel-release");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    const STALL: Duration = Duration::from_millis(4000);
    for arm in ["child_object", "window_object"] {
        for with_cancel in [true, false] {
            for trial in 1..=5 {
                let got = if arm == "child_object" {
                    caller.run(Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 1)).result
                } else {
                    caller.run(Job::Acquire(hwnd.0 as isize)).result
                };
                let named = caller.run(Job::Name).result;
                app.stall(STALL);
                let started = caller.start(Job::Drop);
                let (outcome, answers) = if with_cancel {
                    std::thread::sleep(Duration::from_millis(300));
                    let (early, answers) = cancel_repeatedly(&caller, Duration::from_secs(2));
                    (early.unwrap_or_else(|| caller.finish()), answers)
                } else {
                    (caller.finish(), Vec::new())
                };
                let _ = app.stall_ended(STALL);
                println!(
                    "RESULT e1r {arm} cancel={with_cancel} trial={trial} got={got} name={named} release_ms={} cancels={} answers={:?} result={}",
                    ms(outcome.ended - started),
                    answers.len(),
                    answers.iter().collect::<std::collections::BTreeSet<_>>(),
                    outcome.result
                );
            }
        }
    }
    app.quit();
}

fn hung(hwnd: HWND) -> bool {
    // SAFETY: tolerates any handle.
    unsafe { IsHungAppWindow(hwnd) }.as_bool()
}

fn acc_name_hits(hwnd: HWND) -> u32 {
    common::read_hits(hwnd)
        .into_iter()
        .find(|(name, _)| *name == "get_accName")
        .map_or(0, |(_, count)| count)
}

/// Experiment 1 and 4 (timing): `accName` through the proxy, mockapp's
/// window thread stalled, cancelled 300 ms into the call with each timeout;
/// then the same proxy read again once the stall ends. And a thread that
/// did not enable cancellation, as the control.
fn e1_name_in_a_stall() {
    common::init_com();
    let title = common::unique_title("cancel-name-stall");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    println!("RESULT e1 acquire {}", caller.run(Job::Acquire(hwnd.0 as isize)).result);
    println!("RESULT e1 first {}", caller.run(Job::Name).result);
    const STALL: Duration = Duration::from_millis(4000);
    for timeout in [0u32, 1, 2] {
        for trial in 1..=10 {
            common::reset_hits(hwnd);
            app.stall(STALL);
            let stalled = Instant::now();
            let started = caller.start(Job::Name);
            std::thread::sleep(Duration::from_millis(300));
            let (cancelled, cancel_took, answer) = cancel(&caller, timeout);
            let outcome = caller.finish();
            let _ = app.stall_ended(STALL);
            let stall_end = stalled.elapsed();
            let second = caller.run(Job::Name);
            let hits = acc_name_hits(hwnd);
            println!(
                "RESULT e1 stall timeout={timeout}s trial={trial} cancel_answer={answer} cancel_took_ms={} call_ms={} after_cancel_ms={} result={} stall_seen_end_ms={} second={} second_ms={} accname_hits_after_second={hits}",
                ms(cancel_took),
                ms(outcome.ended - started),
                ms(outcome.ended.saturating_duration_since(cancelled)),
                outcome.result,
                ms(stall_end),
                second.result,
                ms(second.ended - second.started),
            );
        }
    }
    // The control: cancellation not enabled on the calling thread.
    let plain = Caller::new(false);
    println!("RESULT e1 control acquire {}", plain.run(Job::Acquire(hwnd.0 as isize)).result);
    for trial in 1..=3 {
        app.stall(Duration::from_millis(2000));
        let started = plain.start(Job::Name);
        std::thread::sleep(Duration::from_millis(300));
        let (cancelled, _, answer) = cancel(&plain, 0);
        let outcome = plain.finish();
        let _ = app.stall_ended(Duration::from_millis(2000));
        println!(
            "RESULT e1 control trial={trial} cancel_answer={answer} call_ms={} after_cancel_ms={} result={}",
            ms(outcome.ended - started),
            ms(outcome.ended.saturating_duration_since(cancelled)),
            outcome.result
        );
    }
    // Nothing outstanding: what CoCancelCall answers for an idle thread.
    let (_, _, idle) = cancel(&caller, 0);
    println!("RESULT e1 idle cancel_answer={idle}");
    app.quit();
}

/// Experiment 1, the call inside the provider: `accName` held by mockapp
/// (the window thread blocked inside the provider method), cancelled as
/// soon as mockapp says it holds it.
fn e1_name_held() {
    common::init_com();
    let title = common::unique_title("cancel-name-held");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    common::reset_hits(hwnd);
    println!("RESULT e1h acquire {}", caller.run(Job::Acquire(hwnd.0 as isize)).result);
    println!("RESULT e1h hits after acquire {:?}", common::read_hits(hwnd));
    common::reset_hits(hwnd);
    println!("RESULT e1h first {}", caller.run(Job::Name).result);
    println!("RESULT e1h hits after first call {:?}", common::read_hits(hwnd));
    for trial in 1..=10 {
        common::reset_hits(hwnd);
        app.hold();
        let started = caller.start(Job::Name);
        let held = app.held();
        let held_at = started.elapsed();
        let (cancelled, cancel_took, answer) = cancel(&caller, 0);
        println!("RESULT e1h trial={trial} held={held} cancel_answer={answer} cancel_took_ms={}", ms(cancel_took));
        let early = caller.finish_within(Duration::from_secs(15));
        let returned_before_release = early.is_some();
        let released = Instant::now();
        app.release();
        let outcome = early.unwrap_or_else(|| caller.finish());
        println!("RESULT e1h trial={trial} returned_before_release={returned_before_release} after_release_ms={}", ms(outcome.ended.saturating_duration_since(released)));
        let second = caller.run(Job::Name);
        let hits = format!("{:?}", common::read_hits(hwnd));
        println!(
            "RESULT e1h trial={trial} held={held} held_after_ms={} cancel_answer={answer} cancel_took_ms={} call_ms={} after_cancel_ms={} result={} second={} second_ms={} accname_hits={hits}",
            ms(held_at),
            ms(cancel_took),
            ms(outcome.ended - started),
            ms(outcome.ended.saturating_duration_since(cancelled)),
            outcome.result,
            second.result,
            ms(second.ended - second.started),
        );
    }
    app.quit();
}

/// Experiment 1, the first call on a new proxy: oleacc's proxy makes an
/// `accParent` call before the first call it is asked for; that call held,
/// cancelled once, then again two seconds later, then released.
fn e1_first_call_held() {
    common::init_com();
    let title = common::unique_title("cancel-first-held");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    for trial in 1..=10 {
        let acquired = caller.run(Job::Acquire(hwnd.0 as isize)).result;
        common::reset_hits(hwnd);
        app.hold();
        let started = caller.start(Job::Name);
        let held = app.held();
        let held_at = started.elapsed();
        let (_, _, answer) = cancel(&caller, 0);
        let early = caller.finish_within(Duration::from_secs(2));
        let (early, answer2) = if early.is_some() {
            (early, "-".to_owned())
        } else {
            let (_, _, again) = cancel(&caller, 0);
            (caller.finish_within(Duration::from_secs(2)), again)
        };
        let returned_before_release = early.is_some();
        let released = Instant::now();
        app.release();
        let outcome = early.unwrap_or_else(|| caller.finish());
        let after_release = outcome.ended.saturating_duration_since(released);
        let hits = format!("{:?}", common::read_hits(hwnd));
        let second = caller.run(Job::Name);
        println!(
            "RESULT e1f trial={trial} acquire={acquired} held={held} held_after_ms={} cancel_answer={answer} second_cancel_answer={answer2} returned_before_release={returned_before_release} after_release_ms={} call_ms={} result={} hits={hits} second={}",
            ms(held_at),
            ms(after_release),
            ms(outcome.ended - started),
            outcome.result,
            second.result
        );
    }
    app.quit();
}

/// Experiment 2: the acquisitions in a stall of ten seconds, past the five
/// after which Windows judges the window hung, with a cancel 300 ms in and
/// without.
fn e2_acquisitions_in_a_stall() {
    common::init_com();
    let title = common::unique_title("cancel-acquire-stall");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    const STALL: Duration = Duration::from_millis(15_000);
    let jobs: [(&str, fn(isize) -> Job); 3] = [
        ("from_event_self", |h| Job::FromEvent(h, OBJID_CLIENT.0, 0)),
        ("from_event_child1", |h| Job::FromEvent(h, OBJID_CLIENT.0, 1)),
        ("from_window", Job::FromWindow),
    ];
    for (name, job) in jobs {
        for with_cancel in [true, false] {
            for trial in 1..=3 {
                app.stall(STALL);
                let stalled = Instant::now();
                let started = caller.start(job(hwnd.0 as isize));
                let mut answer = "none".to_owned();
                let mut cancelled = None;
                if with_cancel {
                    std::thread::sleep(Duration::from_millis(300));
                    let (at, _, a) = cancel(&caller, 0);
                    answer = a;
                    cancelled = Some(at);
                }
                let outcome = caller.finish();
                let hung_at_return = hung(hwnd);
                let _ = app.stall_ended(STALL);
                let seen_end = stalled.elapsed();
                let named = if outcome.result.starts_with("ok") { caller.run(Job::Name).result } else { "-".to_owned() };
                println!(
                    "RESULT e2 {name} cancel={with_cancel} trial={trial} cancel_answer={answer} call_ms={} after_cancel_ms={} stall_seen_end_ms={} hung_at_return={hung_at_return} result={} its_name={named}",
                    ms(outcome.ended - started),
                    cancelled.map_or("-".to_owned(), |at| ms(outcome.ended.saturating_duration_since(at))),
                    ms(seen_end),
                    outcome.result
                );
            }
        }
    }
    app.quit();
}

/// Experiment 2: whether Windows judges the stalled window hung while the
/// acquisition waits, probed six seconds into a ten-second stall.
fn e2_hung_probe() {
    common::init_com();
    let title = common::unique_title("cancel-hung-probe");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    const STALL: Duration = Duration::from_millis(10_000);
    for (name, job) in [
        ("from_event_self", Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 0)),
        ("from_window", Job::FromWindow(hwnd.0 as isize)),
    ] {
        app.stall(STALL);
        let started = caller.start(job);
        let hung_at_4s = { std::thread::sleep(Duration::from_millis(4000)); hung(hwnd) };
        std::thread::sleep(Duration::from_millis(2000));
        let hung_at_6s = hung(hwnd);
        let returned_by_6s = caller.finish_within(Duration::ZERO).is_some();
        let outcome = caller.finish();
        let _ = app.stall_ended(STALL);
        println!(
            "RESULT e2p {name} hung_at_4s={hung_at_4s} hung_at_6s={hung_at_6s} returned_by_6s={returned_by_6s} call_ms={} result={}",
            ms(outcome.ended - started),
            outcome.result
        );
    }
    app.quit();
}

/// Experiment 2, the COM part of `AccessibleObjectFromEvent`: the window
/// message answered, the `accChild` it makes for a child id held.
fn e2_from_event_child_held() {
    common::init_com();
    // SAFETY: set before any other thread reads the environment's this
    // variable; mockapp inherits it.
    unsafe { std::env::set_var("MOCKAPP_HOLD_PAST_GETOBJECT", "1") };
    let title = common::unique_title("cancel-acquire-held");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(true);
    // Control: held, then released 300 ms later.
    for trial in 1..=3 {
        app.hold();
        let started = caller.start(Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 1));
        let held = app.held();
        std::thread::sleep(Duration::from_millis(300));
        app.release();
        let outcome = caller.finish();
        let named = caller.run(Job::Name).result;
        println!(
            "RESULT e2h control trial={trial} held={held} call_ms={} result={} its_name={named}",
            ms(outcome.ended - started),
            outcome.result
        );
    }
    for trial in 1..=10 {
        common::reset_hits(hwnd);
        app.hold();
        let started = caller.start(Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 1));
        let held = app.held();
        let (cancelled, _, answer) = cancel(&caller, 0);
        let early = caller.finish_within(Duration::from_secs(15));
        let returned_before_release = early.is_some();
        let released = Instant::now();
        app.release();
        let outcome = early.unwrap_or_else(|| caller.finish());
        println!("RESULT e2h trial={trial} returned_before_release={returned_before_release} after_release_ms={}", ms(outcome.ended.saturating_duration_since(released)));
        let named = if outcome.result.starts_with("ok") { caller.run(Job::Name).result } else { "-".to_owned() };
        let hits = format!("{:?}", common::read_hits(hwnd));
        let again = caller.run(Job::FromEvent(hwnd.0 as isize, OBJID_CLIENT.0, 1));
        let again_named = caller.run(Job::Name).result;
        println!(
            "RESULT e2h cancel trial={trial} held={held} cancel_answer={answer} call_ms={} after_cancel_ms={} result={} its_name={named} hits={hits} again={} again_name={again_named}",
            ms(outcome.ended - started),
            ms(outcome.ended.saturating_duration_since(cancelled)),
            outcome.result,
            again.result
        );
    }
    app.quit();
}

/// Experiment 2, oleacc's own in-process proxy for a real Win32 button:
/// its `accName` reads the button's text with a window message, made in
/// this process, with no COM call to cancel.
fn e2_native_proxy_in_a_stall() {
    common::init_com();
    let title = common::unique_title("cancel-native");
    let mut app = common::spawn("group_box.json", "msaa", &title);
    let host = common::find_window(&title);
    // SAFETY: a local search of the host window's children.
    let button = unsafe { FindWindowExW(Some(host), None, w!("Button"), w!("Apply")) }
        .expect("mockapp made the button");
    let caller = Caller::new(true);
    println!("RESULT e2n acquire {}", caller.run(Job::Acquire(button.0 as isize)).result);
    println!("RESULT e2n first {}", caller.run(Job::Name).result);
    const STALL: Duration = Duration::from_millis(10_000);
    for with_cancel in [true, false] {
        for trial in 1..=3 {
            app.stall(STALL);
            let stalled = Instant::now();
            let started = caller.start(Job::Name);
            let mut answer = "none".to_owned();
            let mut cancelled = None;
            if with_cancel {
                std::thread::sleep(Duration::from_millis(300));
                let (at, _, a) = cancel(&caller, 0);
                answer = a;
                cancelled = Some(at);
            }
            let outcome = caller.finish();
            let hung_at_return = hung(button);
            let _ = app.stall_ended(STALL);
            println!(
                "RESULT e2n cancel={with_cancel} trial={trial} cancel_answer={answer} call_ms={} after_cancel_ms={} stall_seen_end_ms={} hung_at_return={hung_at_return} result={}",
                ms(outcome.ended - started),
                cancelled.map_or("-".to_owned(), |at| ms(outcome.ended.saturating_duration_since(at))),
                ms(stalled.elapsed()),
                outcome.result
            );
        }
    }
    app.quit();
}

/// Experiment 4, cost: a thousand `accName` calls that answer, with
/// cancellation enabled on the thread and without, alternating.
fn e4_cost() {
    common::init_com();
    let title = common::unique_title("cancel-cost");
    let app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let caller = Caller::new(false);
    println!("RESULT e4 acquire {}", caller.run(Job::Acquire(hwnd.0 as isize)).result);
    // Warm up.
    let _ = caller.run(Job::NameTimes(200));
    for round in 1..=6 {
        let off = caller.run(Job::NameTimes(1000));
        let enable = caller.run(Job::Enable);
        let on = caller.run(Job::NameTimes(1000));
        let disable = caller.run(Job::Disable);
        println!(
            "RESULT e4 round={round} disabled_ms={} disabled_result={} enable={} enable_us={:.1} enabled_ms={} enabled_result={} disable={}",
            ms(off.ended - off.started),
            off.result,
            enable.result,
            (enable.ended - enable.started).as_secs_f64() * 1e6,
            ms(on.ended - on.started),
            on.result,
            disable.result,
        );
    }
    app.quit();
}

/// Lines the outpost logs, forwarded to a channel and to stderr, with the
/// time each was written.
struct LogLines;

static LOG: std::sync::OnceLock<std::sync::Mutex<Sender<(Instant, String)>>> = std::sync::OnceLock::new();

impl std::io::Write for LogLines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf).trim_end().to_owned();
        if let Some(log) = LOG.get() {
            let _ = log.lock().unwrap().send((Instant::now(), line));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_logs() -> Receiver<(Instant, String)> {
    let (tx, rx) = mpsc::channel();
    LOG.set(std::sync::Mutex::new(tx)).unwrap();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_ansi(false)
        .with_writer(|| LogLines)
        .init();
    rx
}

fn ping(outpost: &OutpostUnderTest, seq: u64) -> usize {
    outpost.outpost.handle_command(&SupervisorToOutpost::Ping { seq, sent_at_us: 0 });
    loop {
        match outpost.next() {
            OutpostToSupervisor::Pong { seq: s, parked_count, .. } if s == seq => return parked_count,
            other => println!("RESULT e3 message while pinging: {other:?}"),
        }
    }
}

/// Experiment 3, the baseline: the outpost's own handling of an MSAA focus
/// whose first `IAccessible` call mockapp holds, and of one whose
/// acquisition meets a stalled window thread.
fn e3_outpost_baseline() {
    common::init_com();
    let logs = capture_logs();
    if std::env::var_os("E3_HOLD_PAST_GETOBJECT").is_some() {
        // SAFETY: set before mockapp is started; it inherits it.
        unsafe { std::env::set_var("MOCKAPP_HOLD_PAST_GETOBJECT", "1") };
    }
    let title = common::unique_title("cancel-outpost");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    outpost.follow_the_system_clock();
    let btn1 = DeliveredFact::MsaaFocus { hwnd: hwnd.0 as isize, id_object: 2, id_child: 0 };
    let slider = DeliveredFact::MsaaFocus { hwnd: hwnd.0 as isize, id_object: 3, id_child: 0 };
    let list = DeliveredFact::MsaaFocus { hwnd: hwnd.0 as isize, id_object: 4, id_child: 0 };

    // Arm H: the first provider call held.
    app.send("set-focus btn1");
    app.hold();
    let delivered = Instant::now();
    outpost.deliver(btn1.clone());
    let held = app.held();
    println!("RESULT e3 hold held={held} after_ms={}", ms(delivered.elapsed()));
    let (at, line) = loop {
        let (at, line) = logs
            .recv_timeout(Duration::from_secs(30))
            .expect("the outpost logs within 30 seconds");
        println!("RESULT e3 log +{}ms {line}", ms(at - delivered));
        if line.contains("abandoned and replaced") {
            break (at, line);
        }
    };
    println!("RESULT e3 hold abandoned_after_ms={} line={line}", ms(at - delivered));
    println!("RESULT e3 hold parked_while_held={}", ping(&outpost, 1));
    match outpost.messages.try_recv() {
        Ok(message) => println!("RESULT e3 hold said {message:?}"),
        Err(error) => println!("RESULT e3 hold said nothing ({error:?})"),
    }
    let released = Instant::now();
    app.release();
    app.send("set-focus slider1");
    outpost.deliver(slider);
    let next = outpost.next();
    println!("RESULT e3 hold next_focus_after_release_ms={} {next:?}", ms(released.elapsed()));
    outpost.settle();
    println!("RESULT e3 hold parked_after={}", ping(&outpost, 2));
    while let Ok((at, line)) = logs.try_recv() {
        println!("RESULT e3 log +{}ms {line}", ms(at - delivered));
    }

    // Arm S: the window thread stalled for 12 seconds, the acquisition's
    // window message waiting on it.
    const STALL: Duration = Duration::from_millis(12_000);
    app.send("set-focus list1");
    app.stall(STALL);
    let delivered = Instant::now();
    outpost.deliver(list);
    outpost.settle();
    println!("RESULT e3 stall settled_after_ms={} hung_now={}", ms(delivered.elapsed()), hung(hwnd));
    match outpost.messages.try_recv() {
        Ok(message) => println!("RESULT e3 stall said {message:?}"),
        Err(error) => println!("RESULT e3 stall said nothing ({error:?})"),
    }
    let _ = app.stall_ended(STALL);
    println!("RESULT e3 stall stall_seen_end_ms={} parked={}", ms(delivered.elapsed()), ping(&outpost, 3));
    while let Ok((at, line)) = logs.try_recv() {
        println!("RESULT e3 log +{}ms {line}", ms(at - delivered));
    }
    drop(outpost);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        ("e1_name_in_a_stall", e1_name_in_a_stall),
        ("e1_name_held", e1_name_held),
        ("e1_first_call_held", e1_first_call_held),
        ("e2_acquisitions_in_a_stall", e2_acquisitions_in_a_stall),
        ("e2_hung_probe", e2_hung_probe),
        ("e2_from_event_child_held", e2_from_event_child_held),
        ("e2_native_proxy_in_a_stall", e2_native_proxy_in_a_stall),
        ("e12_repeated_cancels", e12_repeated_cancels),
        ("e1_release_in_a_stall", e1_release_in_a_stall),
        ("e4_cost", e4_cost),
        ("e3_outpost_baseline", e3_outpost_baseline),
    ]);
}
