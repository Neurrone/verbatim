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
//! Arms of each kind, 3 trials each: `hold-once` and `hold-repeat` (the
//! call's first provider method held; cancelled once at once, or every
//! 50 ms; released after 2,500 ms), `stall-once` and `stall-repeat` (the
//! UI thread busy for 2,500 ms; cancelled 300 ms in, once or every 50 ms).
//! `--arm <kind>` runs one kind.
//!
//! Also: `disabled`, the property read held with cancellation never enabled
//! on the caller, cancelled once, 3 trials; and `abandon`, a name read held
//! for 2,000 ms while later reads run on new threads (`new`) or on the
//! stuck read's own thread (`same`), 3 trials each.

use std::sync::mpsc;
use std::time::Duration;

use uia_facts_support::args::Args;
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

/// How long a held call is held, or the UI thread kept busy: longer than
/// the connection timeout, so the new connection's timeout comes first.
const HOLD: Duration = Duration::from_millis(2_500);

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
            judge(&args, &mut report, kind);
        }
    }
    if args.wants("disabled") {
        let trials: Vec<Trial> = (0..3)
            .map(|_| trial(&args, Kind::Property, true, false, false))
            .collect();
        let answers: Vec<Vec<String>> = trials.iter().map(|t| t.answers.clone()).collect();
        report.claim(
            "a caller that never enabled cancellation: each cancel's answer",
            &runs(&answers),
            "RPC_E_NO_CONTEXT",
            deterministic(&answers, &vec!["RPC_E_NO_CONTEXT".to_owned()]),
        );
    }
    if args.wants("abandon") {
        abandon(&mut report);
    }
    report.finish();
}

fn judge(args: &Args, report: &mut Report, kind: Kind) {
    let mut answers = Vec::new();
    let mut count = 0;
    let mut after_release = Vec::new();
    let mut outcomes = Vec::new();
    let mut connection_times = Vec::new();
    for (stall, repeat) in [(false, false), (false, true), (true, false), (true, true)] {
        for _ in 0..3 {
            let trial = trial(args, kind, stall, repeat, true);
            count += trial.answers.len();
            answers.push(
                trial.answers.iter().all(|a| a == "RPC_E_NO_CONTEXT") && !trial.answers.is_empty(),
            );
            outcomes.push(trial.call.outcome.clone());
            if kind == Kind::Connection {
                connection_times.push(trial.call.ms());
                after_release.push(trial.call.end < trial.released);
            } else {
                after_release.push(trial.call.end >= trial.released);
            }
        }
    }
    let name = kind.name();
    report.claim(
        &format!(
            "{name}: every cancel, once or every 50 ms, held or stalled, answers RPC_E_NO_CONTEXT"
        ),
        &format!("{} ({count} cancels in 12 trials)", runs(&answers)),
        "5,614 of 5,614 over 203 trials",
        relation(&answers),
    );
    if kind == Kind::Connection {
        report.claim(
            "connection: the call fails with UIA_E_TIMEOUT",
            &runs(&outcomes),
            "UIA_E_TIMEOUT",
            deterministic(&outcomes, &"UIA_E_TIMEOUT".to_owned()),
        );
        report.claim(
            "connection: it fails before the release or the stall's end",
            &runs(&after_release),
            "true",
            relation(&after_release),
        );
        let window = band(&[2_000.0]);
        report.claim(
            "connection: it fails at the connection timeout, not at a cancel",
            &summary_ms(&connection_times),
            &format!("2,000.5 to 2,015.1 ms, {}", band_text(window)),
            in_band(&connection_times, window),
        );
    } else {
        report.claim(
            &format!("{name}: the call succeeds"),
            &runs(&outcomes),
            "ok",
            deterministic(&outcomes, &"ok".to_owned()),
        );
        report.claim(
            &format!("{name}: it returns only at the release or the stall's end"),
            &runs(&after_release),
            "true",
            relation(&after_release),
        );
    }
}

/// One trial of `kind`: the call made on a caller thread, cancelled from
/// this thread.
fn trial(args: &Args, kind: Kind, stall: bool, repeat: bool, enable: bool) -> Trial {
    let mut provider = Provider::launch(&["--windows", "uia:0"]);
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
        uia::set_timeouts(&client, 2_000, 30_000);
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
    let busy_from = now_us();
    if stall {
        provider.request(&format!("busy 0 {}", HOLD.as_millis()));
    } else if kind == Kind::Connection {
        provider.request("hold GetObject");
    } else {
        provider.request("hold *");
    }
    let start = now_us();
    go.send(()).expect("the caller waits");
    if stall {
        uia::pause(Duration::from_millis(300));
    } else {
        provider
            .held(Duration::from_secs(30))
            .expect("the call reaches the provider");
    }
    let mut answers = vec![cancel(thread)];
    let release_at = start + u64::try_from(HOLD.as_micros()).expect("a short hold");
    while repeat && now_us() < release_at {
        uia::pause(Duration::from_millis(50));
        answers.push(cancel(thread));
    }
    uia::pause(Duration::from_micros(release_at.saturating_sub(now_us())));
    let released = if stall {
        busy_from + u64::try_from(HOLD.as_micros()).expect("a short hold")
    } else {
        let at = now_us();
        provider.request("release");
        at
    };
    let call = caller.join().expect("the call returns");
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
        for _ in 0..3 {
            let mut provider = Provider::launch(&["--windows", "uia:0"]);
            let client = uia::client();
            uia::set_timeouts(&client, 2_000, 30_000);
            let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
            let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
            let second = uia::share(&uia::node(&client, &root, 2).expect("node 2"));
            provider.request("hold GetPropertyValue w=0 n=1");
            let later: Vec<_> = if mode == "new" {
                let stuck = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&first))));
                provider
                    .held(Duration::from_secs(30))
                    .expect("the stuck read is held");
                // Later reads at 500, 1,000 and 1,500 ms, each on a new
                // thread, and the release at 2,000 ms.
                let mut later = Vec::new();
                for _ in 0..3 {
                    uia::pause(Duration::from_millis(500));
                    let second = second.clone();
                    later.push(uia::spawn(move || {
                        Timed::of(|| uia::name(&uia::resolve(&second)))
                    }));
                }
                uia::pause(Duration::from_millis(500));
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
                    let second = uia::resolve(&second);
                    let later: Vec<Timed> =
                        (0..3).map(|_| Timed::of(|| uia::name(&second))).collect();
                    (stuck, later)
                });
                provider
                    .held(Duration::from_secs(30))
                    .expect("the stuck read is held");
                uia::pause(Duration::from_millis(2_000));
                let at = now_us();
                provider.request("release");
                let (_, later) = thread.join().expect("the reads return");
                later.into_iter().map(|t| (t, at)).collect()
            };
            answered_after.push(
                later
                    .iter()
                    .all(|(t, at)| t.end >= *at && t.outcome == "ok"),
            );
            let first_answer = later.iter().map(|(t, _)| t.end).min().unwrap_or(0);
            first_after.push(us_to_ms(first_answer.saturating_sub(later[0].1)));
        }
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
