//! The connection timeout applies to the whole process
//! (`uia-facts/the-connection-timeout-applies-to-the-whole-process/readme.md`):
//! the value one client object reads back is the one another client
//! object's calls wait by.
//!
//! `whole-process`, each trial a new provider program, its UI thread busy:
//! client object A sets the connection timeout to 500 ms, then client
//! object B, on another thread, to 1,000 ms; A's explicit read
//! (`ElementFromHandle`, then `CurrentName`) of the busy window. Then the
//! reverse, A at 1,000 ms and B at 500 ms.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::note;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, band, band_text, deterministic, in_band, runs};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia;

/// The transaction timeout both client objects set, longer than any wait.
const TRANSACTION_MS: u32 = 30_000;

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("the-connection-timeout-applies-to-the-whole-process");
    if args.wants("whole-process") {
        report.case("whole-process", "making the window's UI thread busy; client object A sets one connection timeout and B, on another thread, another, 500 ms and 1 s; timing A's new connection, in both orders");
        order(
            &args,
            &mut report,
            500,
            1_000,
            "2,008 ms at 2,000 ms; h5_main500_other10000 10,013 ms at 10,000 ms",
        );
        order(
            &args,
            &mut report,
            1_000,
            500,
            "515 ms; h5_main10000_other500 508 ms",
        );
    }
    report.finish();
}

fn order(args: &Args, report: &mut Report, ours: u32, theirs: u32, recorded: &str) {
    let a = uia::client();
    let mut waits = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        uia::set_timeouts(&a, ours, TRANSACTION_MS);
        std::thread::spawn(move || {
            uia::mta();
            let b = uia::client();
            uia::set_timeouts(&b, theirs, TRANSACTION_MS);
        })
        .join()
        .expect("the other client object sets its timeout");
        provider.request("busy 0");
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        note::waiting_ms(theirs, "the connection timeout the other client object set");
        let (result, took) =
            uia::timed(|| uia::element(&a, provider.hwnd(0)).and_then(|e| uia::name(&e)));
        waits.push(uia::ms(took));
        outcomes.push(uia::outcome(&result));
        provider.request("release");
    }
    let claim =
        format!("a read on the client object that set {ours} ms, after another set {theirs} ms");
    report.claim(
        &format!("{claim}, fails with UIA_E_TIMEOUT"),
        &runs(&outcomes),
        "UIA_E_TIMEOUT",
        deterministic(&outcomes, &"UIA_E_TIMEOUT".to_owned()),
    );
    let window = band(&[f64::from(theirs)]);
    report.claim(
        &format!("{claim}, waits the other's timeout"),
        &summary_ms(&waits),
        &format!("{recorded}, {}", band_text(window)),
        in_band(&waits, window),
    );
}
