//! Nothing cancels a UIA call (`uia-facts/nothing-cancels-a-uia-call/readme.md`).
//!
//! The caller is a thread of the multithreaded apartment that has called
//! `CoEnableCallCancellation`, holding one client object and the elements
//! it reads, fetched before each trial. The main thread cancels with
//! `CoCancelCall(caller's thread id, 0)`, as NVDA's watchdog does.
//!
//! Kinds of call, each against the provider program, with the provider
//! method held (`hold`) or the UI thread busy (`stall`):
//!
//! - `property`: `CurrentName` of node 1;
//! - `text`: `GetText(-1)` of node 3's document range;
//! - `findall`: `FindAll` of the window's descendants;
//! - `remote`: a remote operation reading node 3's name;
//! - `connection`: `ElementFromHandle` of a new provider's window, held
//!   inside its `WM_GETOBJECT`.
//!
//! Four conditions of each kind, each judged on its own: the call's first
//! provider method held, cancelled once at once or every 50 ms, released
//! after 200 ms; or the UI thread busy for 200 ms, cancelled once the call
//! has reached it (the provider's `queued` entry), once or every 50 ms.
//! `connection` holds or stalls 800 ms against a 500 ms connection
//! timeout, so the timeout comes first. `--case <kind>` runs one kind;
//! `property` then makes the stalled read once more on a caller that never
//! enabled cancellation, cancelled once. Each kind's trials share one
//! provider program, except `connection`'s, which each need a window not
//! yet connected to, and so a provider of their own.
//!
//! Also `abandon`: a name read held for 1,000 ms while later reads run on
//! new threads (`new`), or on the stuck read's own thread once it has
//! failed on a 500 ms transaction timeout (`same`).

use std::sync::mpsc;
use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{now_us, rop};
use windows::Win32::System::Com::{CoCancelCall, CoEnableCallCancellation};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, TreeScope_Descendants, UIA_TextPatternId,
};

/// How long a call of `kind` is held, or its UI thread kept busy, and the
/// connection timeout its caller sets: for `connection` the hold is longer
/// than the timeout, so the timeout comes first.
fn hold_and_timeout(kind: Kind) -> (Duration, u32) {
    if kind == Kind::Connection {
        (Duration::from_millis(800), 500)
    } else {
        (Duration::from_millis(200), 2_000)
    }
}

/// How long `abandon` holds the stuck read.
const ABANDON_HOLD: Duration = Duration::from_millis(1_000);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Property,
    Text,
    FindAll,
    Remote,
    Connection,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Property => "property",
            Kind::Text => "text",
            Kind::FindAll => "findall",
            Kind::Remote => "remote",
            Kind::Connection => "connection",
        }
    }

    /// The call, in plain words.
    fn what(self) -> &'static str {
        match self {
            Kind::Property => "a name read of node 1",
            Kind::Text => "GetText of node 3's document range",
            Kind::FindAll => "FindAll of the window's descendants",
            Kind::Remote => "a remote operation reading node 3's name",
            Kind::Connection => "a new connection to the window",
        }
    }
}

/// One trial's result: each cancel's answer, and the call's outcome and
/// its end, with the release or the stall's end.
struct Trial {
    answers: Vec<String>,
    call: Timed,
    released: u64,
}

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("nothing-cancels-a-uia-call");
    for kind in [
        Kind::Property,
        Kind::Text,
        Kind::FindAll,
        Kind::Remote,
        Kind::Connection,
    ] {
        if args.wants(kind.name()) {
            report.case(
                kind.name(),
                &format!(
                    "{}: holding its first provider method or stalling the UI thread; cancelling it once or repeatedly; timing it{}",
                    kind.what(),
                    if kind == Kind::Property {
                        "; then cancelling it on a caller that never enabled cancellation"
                    } else {
                        ""
                    }
                ),
            );
            judge(&args, &mut report, kind);
            if kind == Kind::Property {
                disabled(&args, &mut report);
            }
        }
    }
    if args.wants("abandon") {
        report.case("abandon", "holding node 1's name read for 1 s; making later reads on new threads, or on the stuck read's own thread once it has failed on a 500 ms timeout; releasing at 1 s");
        abandon(&mut report);
    }
    report.finish();
}

/// The property read, its UI thread stalled, with cancellation never
/// enabled on the caller, cancelled once.
fn disabled(args: &Args, report: &mut Report) {
    {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        let trials: Vec<Trial> = (0..trials())
            .map(|_| trial(args, &mut provider, Kind::Property, true, false, false))
            .collect();
        let answers: Vec<Vec<String>> = trials.iter().map(|t| t.answers.clone()).collect();
        report.claim(
            "a caller that never enabled cancellation: each cancel's answer",
            &runs(&answers),
            "RPC_E_NO_CONTEXT",
            deterministic(&answers, &vec!["RPC_E_NO_CONTEXT".to_owned()]),
        );
    }
}

fn judge(args: &Args, report: &mut Report, kind: Kind) {
    let name = kind.name();
    // One provider for the kind's trials; a new connection needs a window
    // not yet connected to, so each `connection` trial launches its own.
    let mut shared = (kind != Kind::Connection).then(|| Provider::launch(&["--windows", "uia:0"]));
    for (stall, repeat) in [(false, false), (false, true), (true, false), (true, true)] {
        let condition = format!(
            "{name}, {}, cancelled {}",
            if stall { "stalled" } else { "held" },
            if repeat { "every 50 ms" } else { "once" }
        );
        let mut answers = Vec::new();
        let mut count = 0;
        let mut after_release = Vec::new();
        let mut outcomes = Vec::new();
        let mut times = Vec::new();
        for _ in 0..trials() {
            let mut own;
            let provider = if let Some(provider) = shared.as_mut() {
                provider
            } else {
                own = Provider::launch(&["--windows", "uia:0"]);
                &mut own
            };
            let trial = trial(args, provider, kind, stall, repeat, true);
            count += trial.answers.len();
            answers.push(
                trial.answers.iter().all(|a| a == "RPC_E_NO_CONTEXT") && !trial.answers.is_empty(),
            );
            outcomes.push(trial.call.outcome.clone());
            times.push(trial.call.ms());
            if kind == Kind::Connection {
                after_release.push(trial.call.end < trial.released);
            } else {
                after_release.push(trial.call.end >= trial.released);
            }
        }
        report.claim(
            &format!("{condition}: every cancel answers RPC_E_NO_CONTEXT"),
            &format!(
                "{} ({count} cancels in {} trials)",
                runs(&answers),
                answers.len()
            ),
            "5,614 of 5,614 over 203 trials",
            relation(&answers),
        );
        if kind == Kind::Connection {
            report.claim(
                &format!("{condition}: the call fails with UIA_E_TIMEOUT"),
                &runs(&outcomes),
                "UIA_E_TIMEOUT",
                deterministic(&outcomes, &"UIA_E_TIMEOUT".to_owned()),
            );
            report.claim(
                &format!("{condition}: it fails before the release or the stall's end"),
                &runs(&after_release),
                "true",
                relation(&after_release),
            );
            let window = band(&[f64::from(hold_and_timeout(kind).1)]);
            report.claim(
                &format!("{condition}: it fails at the connection timeout, not at a cancel"),
                &summary_ms(&times),
                &format!(
                    "2,000.5 to 2,015.1 ms at a 2,000 ms timeout, {}",
                    band_text(window)
                ),
                in_band(&times, window),
            );
        } else {
            report.claim(
                &format!("{condition}: the call succeeds"),
                &runs(&outcomes),
                "ok",
                deterministic(&outcomes, &"ok".to_owned()),
            );
            report.claim(
                &format!("{condition}: it returns only at the release or the stall's end"),
                &runs(&after_release),
                "true",
                relation(&after_release),
            );
        }
    }
}

/// One trial of `kind`: the call made on a caller thread, cancelled from
/// this thread.
fn trial(
    args: &Args,
    provider: &mut Provider,
    kind: Kind,
    stall: bool,
    repeat: bool,
    enable: bool,
) -> Trial {
    let (hold, connection_ms) = hold_and_timeout(kind);
    let hwnd = provider.hwnd(0);
    let (ready, is_ready) = mpsc::channel();
    let (go, gone) = mpsc::channel::<()>();
    let caller = std::thread::spawn(move || {
        uia::mta();
        if enable {
            // SAFETY: no precondition; cancellation for this thread only.
            unsafe { CoEnableCallCancellation(None) }.expect("cancellation is enabled");
        }
        let client = uia::client();
        uia::set_timeouts(&client, connection_ms, 30_000);
        let (root, first, notes, range) = if kind == Kind::Connection {
            (None, None, None, None)
        } else {
            let root = uia::element(&client, hwnd).expect("the window's element");
            let first = uia::node(&client, &root, 1).expect("node 1");
            let notes = uia::node(&client, &root, 3).expect("node 3");
            // SAFETY: a live element; the text pattern's interface.
            let pattern: IUIAutomationTextPattern =
                unsafe { notes.GetCurrentPatternAs(UIA_TextPatternId) }.expect("the text pattern");
            // SAFETY: a live pattern.
            let range = unsafe { pattern.DocumentRange() }.expect("the document range");
            (Some(root), Some(first), Some(notes), Some(range))
        };
        // SAFETY: no precondition.
        ready
            .send(unsafe { GetCurrentThreadId() })
            .expect("the trial waits");
        gone.recv().expect("the trial says when");
        match kind {
            Kind::Property => Timed::of(|| uia::name(first.as_ref().expect("node 1"))),
            // SAFETY: a live range.
            Kind::Text => Timed::of(|| unsafe { range.as_ref().expect("the range").GetText(-1) }),
            Kind::FindAll => Timed::of(|| {
                let condition = {
                    let client = uia::client();
                    // SAFETY: a live client object.
                    unsafe { client.CreateTrueCondition() }
                }?;
                // SAFETY: live element and condition.
                unsafe {
                    root.as_ref()
                        .expect("the root")
                        .FindAll(TreeScope_Descendants, &condition)
                }
            }),
            Kind::Remote => Timed::of(|| rop::name(notes.as_ref().expect("node 3"))),
            Kind::Connection => Timed::of(|| uia::element(&client, hwnd)),
        }
    });
    let thread = is_ready.recv().expect("the caller is ready");
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    provider.log();
    let busy_from = now_us();
    if stall {
        provider.request(&format!("busy 0 {}", hold.as_millis()));
    } else if kind == Kind::Connection {
        provider.request("hold GetObject");
    } else {
        provider.request("hold *");
    }
    let start = now_us();
    go.send(()).expect("the caller waits");
    if stall {
        // The busy thread logs each message that reaches it: the call's
        // own, made after its start, is a sent `WM_GETOBJECT` for a new
        // connection and a posted COM call for the rest.
        let message = if kind == Kind::Connection {
            "sent"
        } else {
            "posted"
        };
        let (_, reached) = provider.log_until(Duration::from_secs(30), |e| {
            e.kind == "queued" && e.name == message && e.start >= start
        });
        assert!(reached, "the call reaches the busy thread");
    } else {
        provider
            .held(Duration::from_secs(30))
            .expect("the call reaches the provider");
    }
    let mut answers = vec![cancel(thread)];
    let release_at = start + u64::try_from(hold.as_micros()).expect("a short hold");
    while repeat && now_us() < release_at {
        uia::pause(Duration::from_millis(50), "before looking again");
        answers.push(cancel(thread));
    }
    uia::pause(
        Duration::from_micros(release_at.saturating_sub(now_us())),
        "the call held until its release",
    );
    let released = if stall {
        busy_from + u64::try_from(hold.as_micros()).expect("a short hold")
    } else {
        let at = now_us();
        provider.request("release");
        at
    };
    let call = caller.join().expect("the call returns");
    // A held call returns at the release and a stalled one once the busy
    // thread takes it, so the provider is idle for the next trial.
    provider.clear_held();
    Trial {
        answers,
        call,
        released,
    }
}

fn cancel(thread: u32) -> String {
    // SAFETY: a thread id of this process; a zero timeout.
    let answer = unsafe { CoCancelCall(thread, 0) };
    uia::hresult(answer.err().map_or(windows::core::HRESULT(0), |e| e.code()))
}

fn abandon(report: &mut Report) {
    for mode in ["new", "same"] {
        let mut answered_after = Vec::new();
        let mut first_after = Vec::new();
        for _ in 0..trials() {
            let mut provider = Provider::launch(&["--windows", "uia:0"]);
            let client = uia::client();
            // In `same`, the stuck read fails on a transaction timeout
            // shorter than the hold, so its thread is free while the
            // provider is still stuck; in `new`, it waits for the release.
            let transaction = if mode == "same" { 500 } else { 30_000 };
            uia::set_timeouts(&client, 2_000, transaction);
            let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
            let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
            let second = uia::share(&uia::node(&client, &root, 2).expect("node 2"));
            provider.request("hold GetPropertyValue w=0 n=1");
            let later: Vec<_> = if mode == "new" {
                let stuck = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&first))));
                provider
                    .held(Duration::from_secs(30))
                    .expect("the stuck read is held");
                // Later reads at 250, 500 and 750 ms, each on a new thread,
                // and the release at 1,000 ms.
                let step = ABANDON_HOLD / 4;
                let mut later = Vec::new();
                for _ in 0..3 {
                    uia::pause(step, "before the next later read");
                    let second = second.clone();
                    later.push(uia::spawn(move || {
                        Timed::of(|| uia::name(&uia::resolve(&second)))
                    }));
                }
                uia::pause(step, "before the release");
                let at = now_us();
                provider.request("release");
                let _ = stuck.join();
                later
                    .into_iter()
                    .map(|t| (t.join().expect("a later read"), at))
                    .collect()
            } else {
                let thread = uia::spawn(move || {
                    let stuck = Timed::of(|| uia::name(&uia::resolve(&first)));
                    // The stuck read has failed on its 500 ms timeout; the
                    // later reads get 30 s, so they wait for the release
                    // rather than racing it. The timeout is the process's.
                    uia::set_timeouts(&uia::client(), 2_000, 30_000);
                    let second = uia::resolve(&second);
                    let later: Vec<Timed> =
                        (0..3).map(|_| Timed::of(|| uia::name(&second))).collect();
                    (stuck, later)
                });
                provider
                    .held(Duration::from_secs(30))
                    .expect("the stuck read is held");
                uia::pause(
                    ABANDON_HOLD,
                    "the stuck read failed, the later reads made on its thread, before the release",
                );
                let at = now_us();
                provider.request("release");
                let (_, later) = thread.join().expect("the reads return");
                later.into_iter().map(|t| (t, at)).collect()
            };
            // With connection recovery on, a read made on the thread whose
            // read just timed out fails at once instead of waiting.
            let fail_at_once = mode == "same" && uia::settings().recovery;
            answered_after.push(later.iter().all(|(t, at)| {
                if fail_at_once {
                    t.outcome == "UIA_E_TIMEOUT" && t.end < *at
                } else {
                    t.end >= *at && t.outcome == "ok"
                }
            }));
            let first_answer = later.iter().map(|(t, _)| t.end).min().unwrap_or(0);
            first_after.push(us_to_ms(first_answer.saturating_sub(later[0].1)));
        }
        if mode == "same" && uia::settings().recovery {
            report.claim(
                "abandon (same thread), with connection recovery on: every later read fails at once with UIA_E_TIMEOUT, before the release",
                &runs(&answered_after),
                "true (each in 0.1 ms)",
                relation(&answered_after),
            );
        } else {
            report.claim(
                &format!(
                    "abandon ({mode} thread): every later read succeeds, and only after the release"
                ),
                &runs(&answered_after),
                "true",
                relation(&answered_after),
            );
            report.claim(
                &format!("abandon ({mode} thread): the first later answer after the release"),
                &summary_ms(&first_after),
                "within 2 ms",
                Verdict::Trend,
            );
        }
    }
}
