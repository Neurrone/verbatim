//! `UiaHasServerSideProvider` says no for a new conhost window while another
//! process has an event handler
//! (`uia-facts/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/readme.md`).
//!
//! A trial: a prober process starts a console host directly
//! (`conhost.exe cmd.exe`) and calls `UiaHasServerSideProvider` on its
//! window as soon as it finds it, then the raw probe. Each case runs 30
//! trials, as `e38`'s arms did.
//!
//! - `focus-handler`: on a desktop of the program's own, beside a third
//!   process there holding one idle desktop-wide focus handler. (A
//!   `ToolTipOpened` handler over the root of a desktop of its own fails
//!   to register, `0x8007029C`.)
//! - `no-handler`, run only when named: on the program's desktop, with no
//!   other process holding a handler. Its record is its own first run's
//!   0 of 30, judged as that count, since `e38`'s 0 of 110 is a rate of
//!   zero no run can pass.
//! - `stall`, run only when named: the provider program's window, its UI
//!   thread busy until the probe returns.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::console::Console;
use uia_facts_support::events::{self, Handler};
use uia_facts_support::precondition;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, rate, runs,
};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::{exit_now, note, uia};
use windows::Win32::UI::Accessibility::{TreeScope_Subtree, UIA_ToolTipOpenedEventId};

/// The desktop the trials run on.
const DESKTOP: &str = "uia-facts-server-side-provider";
/// Trials of each rate, as `e38`'s arms ran.
const TRIALS: usize = 30;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
            "prober" => prober_role(),
            "raw" => raw_role(),
            "starter" => starter_role(),
            "handler" => handler_role(&rest[0]),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report = Report::start(
        "uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler",
    );
    if args.wants("focus-handler") {
        report.case("focus-handler", "on a desktop of its own: 30 new console windows, each probed as soon as it is found, beside another process holding an idle focus handler");
        probed(&mut report, Some("focus"), Some(DESKTOP), (0, TRIALS));
    }
    if args.names("no-handler") {
        report.case("no-handler", "on a desktop of its own: 30 new console windows, each probed as soon as it is found, with no other process holding a handler");
        probed(&mut report, None, Some(DESKTOP), (0, TRIALS));
    }
    if args.names("stall") {
        report.case("stall", "the provider program's UI thread busy; UiaHasServerSideProvider on its window, released once it returns");
        stall(&mut report);
    }
    report.finish();
}

/// `e38`'s U: a client object made at start, connected to nothing; told
/// `probe <hwnd>`, calls `UiaHasServerSideProvider` on the window and
/// reports `probe <answer> <ms>`.
fn prober_role() -> ! {
    // The client object first, as `e38`'s U made it before the console
    // started; it has connected to nothing.
    let _client = uia::client();
    role::report("ready");
    for command in role::commands() {
        if let Some(hwnd) = command.strip_prefix("probe ") {
            let hwnd: isize = hwnd.parse().expect("a window");
            let (yes, took) = uia::timed(|| uia::has_server_side_provider(hwnd));
            role::report(&format!("probe {yes} {:.1}", uia::ms(took)));
            break;
        }
    }
    exit_now(0)
}

/// Whether this process's desktop is the unlocked input desktop, where a
/// window can be brought to the foreground.
fn crate_desktop_has_input() -> bool {
    precondition::unlocked_desktop().is_ok()
}

/// `e38`'s orchestrator's console step: starts a console host, reports
/// `window <hwnd>` as soon as it finds it, and holds it until its input
/// closes, so that neither client is the console host's parent.
fn starter_role() -> ! {
    let console = Console::start();
    role::report(&format!("window {}", console.hwnd));
    // As the orchestrator's console step did once it found the window:
    // the console brought to the foreground.
    if crate_desktop_has_input() {
        let _ = uia_facts_support::apps::foreground(console.hwnd);
    }
    for _ in role::commands() {}
    drop(console);
    exit_now(0)
}

/// `e38`'s R: a client object made at start; told `target <hwnd>`, sends
/// the raw probe to the window at once and reports `raw <answered 0x10000>`.
fn raw_role() -> ! {
    let _client = uia::client();
    role::report("ready");
    for command in role::commands() {
        if let Some(hwnd) = command.strip_prefix("target ") {
            let hwnd: isize = hwnd.parse().expect("a window");
            let raw = verbatim_uia::raw_probe(hwnd) == verbatim_uia::RawProbe::Provider;
            role::report(&format!("raw {raw}"));
            break;
        }
    }
    exit_now(0)
}

/// `<kind>`: holds one idle handler, `tooltip` (`ToolTipOpened` over the
/// desktop root) or `focus` (a desktop-wide focus handler), reporting
/// `ready`, until its input closes.
fn handler_role(kind: &str) -> ! {
    let client = uia::client();
    let (handler, _reports) = Handler::make();
    let cache = uia::cache_request(&client, &[]).expect("a cache request");
    if kind == "tooltip" {
        // SAFETY: a live client object.
        let root = unsafe { client.GetRootElement() }.expect("the desktop root");
        // SAFETY: live client, element, cache request and handler.
        unsafe {
            client.AddAutomationEventHandler(
                UIA_ToolTipOpenedEventId,
                &root,
                TreeScope_Subtree,
                &cache,
                &handler,
            )
        }
        .expect("the tooltip handler");
    } else {
        let focus = events::as_focus_handler(&handler);
        // SAFETY: live client, cache request and handler.
        unsafe { client.AddFocusChangedEventHandler(&cache, &focus) }.expect("the focus handler");
    }
    role::report("ready");
    for _ in role::commands() {}
    exit_now(0)
}

/// 30 trials on `desktop` (the user's when `None`), beside a handler of
/// `kind` on the same desktop when given; judged against `record`, a
/// recorded k of n: a rate, or for a recorded absence the count itself.
fn probed(report: &mut Report, kind: Option<&str>, desktop: Option<&str>, record: (usize, usize)) {
    let handler = kind.map(|kind| {
        let handler = Role::start_on(desktop, "handler", &[kind.to_owned()]);
        assert_eq!(handler.expect_line(), "ready", "the handler is registered");
        handler
    });
    let mut no = 0;
    let mut no_ms = Vec::new();
    let mut raw = Vec::new();
    for _ in 0..TRIALS {
        // As `e38` ran it: U probes and R sends the raw probe at the same
        // moment, each told the window as soon as it is found.
        let mut prober = Role::start_on(desktop, "prober", &[]);
        let mut r = Role::start_on(desktop, "raw", &[]);
        assert_eq!(prober.expect_line(), "ready", "the prober is ready");
        assert_eq!(r.expect_line(), "ready", "R is ready");
        let starter = Role::start_on(desktop, "starter", &[]);
        let found = starter.expect_line();
        let hwnd = found.strip_prefix("window ").expect("the console's window");
        r.send(&format!("target {hwnd}"));
        prober.send(&format!("probe {hwnd}"));
        let line = prober
            .line(Duration::from_secs(30))
            .expect("the prober reports");
        let words: Vec<&str> = line.split(' ').collect();
        if words[1] == "false" {
            no += 1;
            no_ms.push(words[2].parse::<f64>().expect("a time"));
        }
        raw.push(r.expect_line() == "raw true");
    }
    drop(handler);
    let beside = kind.map_or_else(
        || "with no other process's handler".to_owned(),
        |k| format!("beside an idle {k} handler"),
    );
    let claim = format!("trials in which UiaHasServerSideProvider answered no {beside}");
    let observed = format!("{no} of {TRIALS}");
    // A recorded absence is judged over the program's own 30 trials, as
    // its first run counted it: a rate of zero cannot pass.
    let (k, n) = record;
    let verdict = if k == 0 {
        deterministic(&[no], &0)
    } else {
        rate(no, TRIALS, k, n)
    };
    report.claim(&claim, &observed, &format!("{k} of {n}"), verdict);
    if !no_ms.is_empty() {
        report.claim(
            "those answers' times",
            &summary_ms(&no_ms),
            "3,022 to 3,058 ms",
            Verdict::Trend,
        );
    }
    let raw_all = raw.iter().all(|r| *r);
    report.claim(
        "the raw probe sent by R at the same moment answered 0x10000",
        &format!("{} of {}", raw.iter().filter(|r| **r).count(), raw.len()),
        "every one",
        if raw_all {
            Verdict::Pass
        } else {
            Verdict::Differ
        },
    );
}

fn stall(report: &mut Report) {
    let mut answers = Vec::new();
    let mut times = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        provider.request("busy 0");
        note::waiting("up to about 5 s, UiaHasServerSideProvider on a window that does not pump");
        let (yes, took) = uia::timed(|| uia::has_server_side_provider(provider.hwnd(0)));
        provider.request("release");
        answers.push(yes);
        times.push(uia::ms(took));
    }
    report.claim(
        "UiaHasServerSideProvider on a window that does not pump answers",
        &runs(&answers),
        "false",
        deterministic(&answers, &false),
    );
    let wait = band(&[5_000.0]);
    report.claim(
        "after",
        &summary_ms(&times),
        &band_text(wait),
        in_band(&times, wait),
    );
}
