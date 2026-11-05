//! A new connection to a stuck application times out
//! (`uia-facts/a-new-connection-to-a-stuck-application-times-out/readme.md`).
//!
//! Cases:
//!
//! - `connection`: the provider program with two windows on two UI threads;
//!   UI thread 0 busy with a name read queued on it; `ElementFromHandle` of
//!   window 1, on the idle thread, and then of window 0, each with the
//!   connection timeout at 2,000 ms, a new provider each.
//! - `description`: `ElementFromHandleBuildCache` with the provider
//!   description cached, per stall kind: `queued` (as above, window 1),
//!   `busy` (UI thread 0 busy, nothing queued, window 0) and `held` (a name
//!   read held in window 0's provider, window 1).

use std::sync::mpsc;
use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::note;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, UIA_E_TIMEOUT};
use windows::Win32::UI::Accessibility::UIA_ProviderDescriptionPropertyId;

/// The connection timeout every case sets.
const CONNECTION_MS: u32 = 2_000;
/// The transaction timeout every case sets, longer than any case's stall, so
/// the queued read waits for the release.
const TRANSACTION_MS: u32 = 30_000;
/// How long the queued read is given to reach the provider's channel
/// before the lookups: a deliberate pause, as the original runs made
/// (200 ms), since nothing marks a request queued on a busy thread.
const QUEUE_PAUSE: Duration = Duration::from_millis(200);

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("a-new-connection-to-a-stuck-application-times-out");
    if args.wants("connection") {
        report.case("connection", "making UI thread 0 busy with a read queued on it; connecting to the window on the idle thread, then to the busy thread's window, at a 2 s connection timeout");
        connection(&args, &mut report);
    }
    if args.wants("description") {
        report.case("description", "stalling the provider three ways; looking up a window with its provider description cached; classifying the answer");
        description(&args, &mut report);
    }
    report.finish();
}

/// A provider with UI thread 0 busy and, when `queue`, a name read of
/// window 0's node 1 queued on it from a thread of its own; answers the
/// provider and the queued read's outcome, once released.
fn stuck(queue: bool) -> (Provider, Option<mpsc::Receiver<String>>) {
    let mut provider = Provider::launch(&["--windows", "uia:0,uia:1"]);
    if !queue {
        provider.request("busy 0");
        return (provider, None);
    }
    let hwnd = provider.hwnd(0);
    let (fetched, was_fetched) = mpsc::channel();
    let (go, gone) = mpsc::channel::<()>();
    let (done, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        uia::mta();
        let client = uia::client();
        let root = uia::element(&client, hwnd).expect("window 0's element");
        let first = uia::node(&client, &root, 1).expect("node 1");
        fetched.send(()).expect("the case waits");
        gone.recv().expect("the case says when");
        let read = uia::name(&first);
        let _ = done.send(uia::outcome(&read));
    });
    was_fetched.recv().expect("the element is fetched");
    provider.request("busy 0");
    go.send(()).expect("the reader waits");
    (provider, Some(outcome))
}

fn connection(args: &Args, report: &mut Report) {
    let client = uia::client();
    uia::set_timeouts(&client, CONNECTION_MS, TRANSACTION_MS);
    let mut idle = Vec::new();
    let mut busy = Vec::new();
    let mut idle_out = Vec::new();
    let mut busy_out = Vec::new();
    for _ in 0..trials() {
        let (mut provider, queued) = stuck(true);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        uia::pause(QUEUE_PAUSE, "for the queued read to reach the busy thread");
        note::waiting_ms(CONNECTION_MS, "each new connection's connection timeout");
        let (result, took) = uia::timed(|| uia::element(&client, provider.hwnd(1)));
        idle.push(uia::ms(took));
        idle_out.push(uia::outcome(&result));
        let (result, took) = uia::timed(|| uia::element(&client, provider.hwnd(0)));
        busy.push(uia::ms(took));
        busy_out.push(uia::outcome(&result));
        provider.request("release");
        let _ = queued.map(|q| q.recv());
    }
    let timeout = "UIA_E_TIMEOUT".to_owned();
    let window = band(&[f64::from(CONNECTION_MS)]);
    report.claim(
        "a new connection to the idle UI thread's window fails with UIA_E_TIMEOUT",
        &runs(&idle_out),
        "UIA_E_TIMEOUT",
        deterministic(&idle_out, &timeout),
    );
    report.claim(
        "the idle thread's connection fails after the connection timeout",
        &summary_ms(&idle),
        &format!("2.0 to 2.2 s, {}", band_text(window)),
        in_band(&idle, window),
    );
    report.claim(
        "a new connection to the busy UI thread's window fails with UIA_E_TIMEOUT",
        &runs(&busy_out),
        "UIA_E_TIMEOUT",
        deterministic(&busy_out, &timeout),
    );
    if uia::settings().recovery {
        // With connection recovery on, the connection made after the idle
        // thread's had failed fails at once.
        let at_once: Vec<bool> = busy
            .iter()
            .map(|ms| *ms < f64::from(CONNECTION_MS))
            .collect();
        report.claim(
            "with connection recovery on, the busy thread's connection, made after the idle thread's failed, fails before the connection timeout",
            &format!("{} ({})", runs(&at_once), summary_ms(&busy)),
            "true, 0.2 ms",
            relation(&at_once),
        );
    } else {
        report.claim(
            "the busy thread's connection fails after the connection timeout",
            &summary_ms(&busy),
            &format!("2.0 to 2.2 s, {}", band_text(window)),
            in_band(&busy, window),
        );
    }
}

/// What a lookup with the provider description cached answered.
fn lookup(client: &windows::Win32::UI::Accessibility::IUIAutomation6, hwnd: isize) -> String {
    let request =
        uia::cache_request(client, &[UIA_ProviderDescriptionPropertyId]).expect("a cache request");
    match uia::element_cached(client, hwnd, &request) {
        Err(error) if error.code() == UIA_E_TIMEOUT => "timeout".to_owned(),
        Err(error) => format!("error {}", uia::hresult(error.code())),
        Ok(element) => match uia::description(&element) {
            Ok(text) if uia::served_only_by_uia_proxies(&text) => "proxies only".to_owned(),
            Ok(_) => "its own provider".to_owned(),
            Err(error) => format!("no description (0x{:08X})", error.code().0),
        },
    }
}

fn description(args: &Args, report: &mut Report) {
    let client = uia::client();
    uia::set_timeouts(&client, CONNECTION_MS, TRANSACTION_MS);
    let mut queued = Vec::new();
    let mut busy = Vec::new();
    let mut held = Vec::new();
    for _ in 0..trials() {
        let (mut provider, read) = stuck(true);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        uia::pause(QUEUE_PAUSE, "for the queued read to reach the busy thread");
        queued.push(lookup(&client, provider.hwnd(1)));
        provider.request("release");
        let _ = read.map(|r| r.recv());

        let (mut provider, _) = stuck(false);
        busy.push(lookup(&client, provider.hwnd(0)));
        provider.request("release");

        let mut provider = Provider::launch(&["--windows", "uia:0,uia:1"]);
        let root = uia::element(&client, provider.hwnd(0)).expect("window 0's element");
        let first = uia::node(&client, &root, 1).expect("node 1");
        let agile = uia::share(&first);
        provider.request("hold GetPropertyValue w=0 n=1");
        let read = uia::spawn(move || uia::outcome(&uia::name(&uia::resolve(&agile))));
        provider
            .held(Duration::from_secs(30))
            .expect("the name read reaches the provider");
        held.push(lookup(&client, provider.hwnd(1)));
        provider.request("release");
        let _ = read.join();
    }
    report.claim(
        "a lookup with the description cached, a call queued on a busy UI thread, the other window",
        &format!("{queued:?}"),
        "timeout",
        deterministic(&queued, &"timeout".to_owned()),
    );
    report.claim(
        "the same lookup, its own window's UI thread busy with nothing queued",
        &format!("{busy:?}"),
        "",
        Verdict::New,
    );
    report.claim(
        "the same lookup, a name read held in the provider, the other window",
        &format!("{held:?}"),
        "",
        Verdict::New,
    );
}
