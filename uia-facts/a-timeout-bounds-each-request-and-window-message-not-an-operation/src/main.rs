//! A timeout bounds each request and window message, not an operation
//! (`uia-facts/a-timeout-bounds-each-request-and-window-message-not-an-operation/readme.md`).
//!
//! Every case runs against the provider program with one UIA window.
//!
//! - `ett`: E-TT's search. The window's element is fetched with the
//!   connection timeout at 10,000 ms and the transaction timeout at
//!   1,000 ms; the provider holds the root's `GetPropertyValue`; the
//!   connection timeout is set to 1,000 ms; then `FindFirstBuildCache` over
//!   the descendants for the name `First`, caching the name.
//! - `queued-search`: another client object's name read of node 1 held; a
//!   `FindFirst` for `First` from the window's element, fetched a second
//!   before, both timeouts at 1,000 ms.
//! - `stall-sweep`: the window's UI thread busy; both timeouts at T set
//!   before the window's element is fetched; `FindFirst` for `Second`. T
//!   of 100, 200, 500 and 1,000 ms. After the release, whether the
//!   provider walked the tree for the search's `Find` request. Judged
//!   against the record for the connection recovery setting in use.
//! - `e3`: E3's `CurrentHasKeyboardFocus` of node 1, held until it fails,
//!   at transaction timeouts of 20,000 and 1,000 ms.
//! - `recovery-edge`: the stall sweep's search at T of 600 to 1,200 ms in
//!   100 ms steps: the search's time and whether its `Find` was sent,
//!   toward the edge of connection recovery's skip (Not known); run with
//!   `--recovery`.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::note;
use uia_facts_support::provider::Provider;
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
        report.case("ett", "holding the root's property read; fetching the root under a 10 s connection timeout, then searching with both timeouts at 1 s");
        ett(&args, &mut report);
    }
    if args.wants("queued-search") {
        report.case("queued-search", "holding another client object's name read; searching from an element fetched a second before, both timeouts at 1 s");
        queued_search(&args, &mut report);
    }
    if args.wants("stall-sweep") {
        report.case("stall-sweep", "making the window's UI thread busy; searching at timeouts from 100 ms to 1.2 s set before the window was fetched; checking whether the search's Find reached the provider");
        // With connection recovery on, the search skips its Find below
        // about 3.1 s of waits; with it off, it sends it at every timeout.
        let records = if uia::settings().recovery {
            [
                (100, "332 ms", 3, false),
                (200, "618 ms", 3, false),
                (500, "1,545 ms", 3, false),
                (1_000, "4,207 to 4,209 ms", 4, true),
            ]
        } else {
            [
                (100, "435 ms", 4, true),
                (200, "823 ms", 4, true),
                (500, "2,085 ms", 4, true),
                (1_000, "4,204 ms", 4, true),
            ]
        };
        for (timeout, recorded, waits, sent) in records {
            stall_sweep(&args, &mut report, timeout, recorded, waits, sent);
        }
        recovery_edge(&args, &mut report);
    }
    if args.wants("e3") {
        report.case("e3", "holding node 1's keyboard-focus read until it fails, at transaction timeouts of 20 s and 1 s");
        e3(&args, &mut report, 20_000, "22,111 ms");
        e3(&args, &mut report, 1_000, "1,092 ms");
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

fn ett(args: &Args, report: &mut Report) {
    let mut provider = launch();
    let client = uia::client();
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        uia::set_timeouts(&client, 10_000, 1_000);
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let request = uia::cache_request(&client, &[UIA_NamePropertyId]).expect("a cache request");
        let condition = uia::name_is(&client, "First").expect("a condition");
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        provider.log();
        provider.request("hold GetPropertyValue w=0 n=0");
        uia::set_timeouts(&client, 1_000, 1_000);
        note::waiting("about 13 s, the search's four waits, one on the 10 s connection timeout");
        // SAFETY: live element, condition and cache request.
        let search = Timed::of(|| unsafe {
            root.FindFirstBuildCache(TreeScope_Descendants, &condition, &request)
        });
        provider.request("release");
        provider.clear_held();
        // The provider runs the search's walk after the release; the next
        // trial waits for it, so its hold catches only its own search.
        let _ = provider.log_until(WALK_BOUND, |e| e.is_call("Navigate"));
        times.push(search.ms());
        outcomes.push(search.outcome);
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
}

fn queued_search(args: &Args, report: &mut Report) {
    let mut provider = launch();
    let client = uia::client();
    uia::set_timeouts(&client, 1_000, 1_000);
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
        // The search re-reads its start's runtime id when the element was
        // fetched 600 ms or more before, not at 300 ms; this pause makes it.
        uia::pause(
            Duration::from_secs(1),
            "a second after fetching the window element, so the search re-reads its runtime id",
        );
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
        note::waiting_ms(1_000, "the search's transaction timeout");
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

/// One stalled search at timeout `timeout`: its outcome, its time, and
/// whether the provider then walked the tree for its `Find`.
fn stalled_search(args: &Args, timeout: u32) -> (String, f64, bool) {
    let mut provider = launch();
    let client = uia::client();
    uia::set_timeouts(&client, timeout, timeout);
    let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    provider.log();
    provider.request("busy 0");
    note::waiting_ms(timeout * 4, "the stalled search's waits");
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
    let trials: Vec<(String, f64, bool)> = (0..trials())
        .map(|_| stalled_search(args, timeout))
        .collect();
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
    for _ in 0..trials() {
        provider.request("hold GetPropertyValue w=0 n=1");
        note::waiting_ms(timeout, "the held read's transaction timeout");
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
        let trials: Vec<(String, f64, bool)> = (0..trials())
            .map(|_| stalled_search(args, timeout))
            .collect();
        let times: Vec<f64> = trials.iter().map(|t| t.1).collect();
        let walked: Vec<bool> = trials.iter().map(|t| t.2).collect();
        rows.push(format!(
            "{timeout} ms: {}, Find sent {}",
            summary_ms(&times),
            runs(&walked)
        ));
    }
    report.claim(
        "the stalled search, by timeout: its time and whether its Find was sent",
        &rows.join("; "),
        "",
        Verdict::New,
    );
}
