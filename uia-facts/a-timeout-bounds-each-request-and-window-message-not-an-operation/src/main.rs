//! A timeout bounds each request and window message, not an operation
//! (`uia-facts/a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md`).
//!
//! Every arm runs against the provider program with one UIA window.
//!
//! - `ett`: E-TT's search. The window's element is fetched with the
//!   connection timeout at 10,000 ms and the transaction timeout at
//!   1,000 ms; the provider holds the root's `GetPropertyValue`; the
//!   connection timeout is set to 1,000 ms; then `FindFirstBuildCache` over
//!   the descendants for the name `First`, caching the name. 5 trials. The
//!   provider log after the release gives the order of the waits.
//! - `queued-search`: another client object's name read of node 1 held; a
//!   `FindFirst` for `First` from the window's element, fetched a second
//!   before, both timeouts at 1,000 ms. 5 trials.
//! - `stall-sweep`: the window's UI thread busy; both timeouts at T set
//!   before the window's element is fetched; connection recovery on;
//!   `FindFirst` for `Second`. T of 100, 200, 500 and 1,000 ms, 5 trials
//!   each. After the release, whether the provider walked the tree for the
//!   search's `Find` request.
//! - `e3`: E3's `CurrentHasKeyboardFocus` of node 1, held until it fails,
//!   at transaction timeouts of 20,000 and 1,000 ms. 5 trials each.
//! - `recovery-edge`: the stall sweep's search at T of 600 to 1,200 ms in
//!   100 ms steps, 3 trials each: the search's time and whether its `Find`
//!   was sent, toward the edge of connection recovery's skip (Not known).

use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, band, band_text, deterministic, in_band, runs};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, Timed};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_NamePropertyId,
};

const TIMEOUT: &str = "UIA_E_TIMEOUT";
/// How long the provider log is watched after a release for the search's
/// walk: the walk follows the release within milliseconds when it comes,
/// so its absence over this bound is the evidence that no `Find` was sent.
const WALK_BOUND: Duration = Duration::from_secs(2);

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report =
        Report::start("a-timeout-bounds-each-request-and-window-message-not-an-operation");
    if args.wants("ett") {
        ett(&args, &mut report);
    }
    if args.wants("queued-search") {
        queued_search(&args, &mut report);
    }
    if args.wants("stall-sweep") {
        for (timeout, recorded, waits, sent) in [
            (100, "332 ms", 3, false),
            (200, "618 ms", 3, false),
            (500, "1,545 ms", 3, false),
            (1_000, "4,207 to 4,209 ms", 4, true),
        ] {
            stall_sweep(&args, &mut report, timeout, recorded, waits, sent);
        }
    }
    if args.wants("e3") {
        e3(&args, &mut report, 20_000, "22,111 ms");
        e3(&args, &mut report, 1_000, "1,092 ms");
    }
    if args.wants("recovery-edge") {
        recovery_edge(&args, &mut report);
    }
    report.finish();
}

fn launch() -> Provider {
    Provider::launch(&["--windows", "uia:0"])
}

fn find(root: &IUIAutomationElement, name: &str) -> windows::core::Result<IUIAutomationElement> {
    let client = uia::client();
    let condition = uia::name_is(&client, name)?;
    // SAFETY: live element and condition.
    unsafe { root.FindFirst(TreeScope_Descendants, &condition) }
}

/// The first log entry of each kind, in order of time: the held test,
/// `CB_GETCOMBOBOXINFO`, each `WM_GETOBJECT` object id, and the walk's
/// first `Navigate`.
fn order(log: &[Entry]) -> Vec<String> {
    let mut seen: Vec<(u64, String)> = Vec::new();
    let mut note = |time: u64, what: String| {
        if !seen.iter().any(|(_, w)| *w == what) {
            seen.push((time, what));
        }
    };
    for entry in log {
        if entry.is_call("GetPropertyValue") && entry.node == Some(0) {
            note(entry.start, "the condition's test".to_owned());
        } else if entry.kind == "msg" && entry.name == "CB_GETCOMBOBOXINFO" {
            note(entry.start, "CB_GETCOMBOBOXINFO".to_owned());
        } else if entry.kind == "msg" && entry.name == "WM_GETOBJECT" {
            note(entry.start, format!("WM_GETOBJECT {}", entry.extra));
        } else if entry.is_call("Navigate") {
            note(entry.start, "the Find's walk".to_owned());
        }
    }
    seen.sort_by_key(|(time, _)| *time);
    seen.into_iter().map(|(_, what)| what).collect()
}

fn ett(args: &Args, report: &mut Report) {
    let mut provider = launch();
    let client = uia::client();
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    let mut orders = Vec::new();
    for _ in 0..5 {
        uia::set_timeouts(&client, 10_000, 1_000);
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let request = uia::cache_request(&client, &[UIA_NamePropertyId]).expect("a cache request");
        let condition = uia::name_is(&client, "First").expect("a condition");
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        provider.log();
        provider.request("hold GetPropertyValue w=0 n=0");
        uia::set_timeouts(&client, 1_000, 1_000);
        // SAFETY: live element, condition and cache request.
        let search = Timed::of(|| unsafe {
            root.FindFirstBuildCache(TreeScope_Descendants, &condition, &request)
        });
        provider.request("release");
        provider.clear_held();
        let (log, _) = provider.log_until(WALK_BOUND, |e| e.is_call("Navigate"));
        times.push(search.ms());
        outcomes.push(search.outcome);
        orders.push(order(&log));
    }
    report.claim(
        "E-TT's search fails with UIA_E_TIMEOUT",
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    let window = band(&[1_000.0, 1_000.0, 10_000.0, 1_000.0]);
    report.claim(
        "E-TT's search waits four bounds in turn, one the connection timeout at the fetch",
        &summary_ms(&times),
        &format!("13,208 ms, {}", band_text(window)),
        in_band(&times, window),
    );
    // The window messages that timed out never reach the window procedure
    // after the hold, so this log shows only what was delivered; the order
    // of the four waits is the original trace's (readme, "Method").
    report.claim(
        "what reaches the provider after the release, in order",
        &runs(&orders),
        "",
        Verdict::Trend,
    );
}

fn queued_search(args: &Args, report: &mut Report) {
    let mut provider = launch();
    let client = uia::client();
    uia::set_timeouts(&client, 1_000, 1_000);
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
        // The search re-reads its start's runtime id when the element was
        // fetched more than about 0.65 s before; this pause makes it.
        uia::pause(Duration::from_secs(1));
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        provider.request("hold GetPropertyValue w=0 n=1");
        let held = uia::spawn(move || {
            let other = uia::client();
            let _ = uia::name(&uia::resolve(&first));
            drop(other);
        });
        provider
            .held(Duration::from_secs(30))
            .expect("the other read reaches the provider");
        let search = Timed::of(|| find(&root, "First"));
        provider.request("release");
        provider.clear_held();
        let _ = held.join();
        times.push(search.ms());
        outcomes.push(search.outcome);
    }
    report.claim(
        "a search queued behind another client's held read fails with UIA_E_TIMEOUT",
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    let window = band(&[1_000.0]);
    report.claim(
        "it fails after one wait, its start's runtime id",
        &summary_ms(&times),
        &format!("1,083 to 1,106 ms, {}", band_text(window)),
        in_band(&times, window),
    );
}

/// One stalled search at timeout `timeout` with recovery on: its outcome,
/// its time, and whether the provider then walked the tree for its `Find`.
fn stalled_search(args: &Args, timeout: u32) -> (String, f64, bool) {
    let mut provider = launch();
    let client = uia::client();
    uia::set_recovery(&client, true);
    uia::set_timeouts(&client, timeout, timeout);
    let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    provider.log();
    provider.request("busy 0");
    let search = Timed::of(|| find(&root, "Second"));
    provider.request("release");
    let (_, walked) = provider.log_until(WALK_BOUND, |e| e.is_call("Navigate"));
    (search.outcome.clone(), search.ms(), walked)
}

fn stall_sweep(
    args: &Args,
    report: &mut Report,
    timeout: u32,
    recorded: &str,
    waits: usize,
    sent: bool,
) {
    let trials: Vec<(String, f64, bool)> = (0..5).map(|_| stalled_search(args, timeout)).collect();
    let outcomes: Vec<String> = trials.iter().map(|t| t.0.clone()).collect();
    let times: Vec<f64> = trials.iter().map(|t| t.1).collect();
    let walked: Vec<bool> = trials.iter().map(|t| t.2).collect();
    report.claim(
        &format!("the stalled search at {timeout} ms fails with UIA_E_TIMEOUT"),
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    let window = band(&vec![f64::from(timeout); waits]);
    report.claim(
        &format!("it waits {waits} bounds of {timeout} ms in turn"),
        &summary_ms(&times),
        &format!("{recorded}, {}", band_text(window)),
        in_band(&times, window),
    );
    report.claim(
        &format!("at {timeout} ms its Find request is sent"),
        &runs(&walked),
        &sent.to_string(),
        deterministic(&walked, &sent),
    );
}

fn e3(args: &Args, report: &mut Report, timeout: u32, recorded: &str) {
    let mut provider = launch();
    let client = uia::client();
    uia::set_timeouts(&client, 2_000, timeout);
    let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    let first = uia::node(&client, &root, 1).expect("node 1");
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        provider.request("hold GetPropertyValue w=0 n=1");
        // SAFETY: a live element.
        let read = Timed::of(|| unsafe { first.CurrentHasKeyboardFocus() });
        provider.request("release");
        provider.clear_held();
        times.push(read.ms());
        outcomes.push(read.outcome);
    }
    report.claim(
        &format!("E3's held HasKeyboardFocus read at {timeout} ms fails with UIA_E_TIMEOUT"),
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    let window = band(&[f64::from(timeout)]);
    report.claim(
        &format!("it fails after about 1.1 times {timeout} ms"),
        &summary_ms(&times),
        &format!("{recorded}, {}", band_text(window)),
        in_band(&times, window),
    );
}

fn recovery_edge(args: &Args, report: &mut Report) {
    let mut rows = Vec::new();
    for timeout in (600..=1_200).step_by(100) {
        let trials: Vec<(String, f64, bool)> =
            (0..3).map(|_| stalled_search(args, timeout)).collect();
        let times: Vec<f64> = trials.iter().map(|t| t.1).collect();
        let walked: Vec<bool> = trials.iter().map(|t| t.2).collect();
        rows.push(format!(
            "{timeout} ms: {}, Find sent {}",
            summary_ms(&times),
            runs(&walked)
        ));
    }
    report.claim(
        "the stalled search with recovery on, by timeout: its time and whether its Find was sent",
        &rows.join("; "),
        "",
        Verdict::New,
    );
}
