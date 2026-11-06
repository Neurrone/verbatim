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
//! - `tooltip`, on the unlocked desktop, run only when named: beside a
//!   third process holding one idle `ToolTipOpened` handler over the
//!   desktop root, as `e38` ran it; each console window opens on the
//!   user's desktop.
//! - `no-handler`, run only when named: on the program's desktop, with no
//!   other process holding a handler.
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
            "handler" => handler_role(&rest[0]),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report = Report::start(
        "uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler",
    );
    if args.wants("focus-handler") {
        report.case("focus-handler", "on a desktop of its own: 30 new console windows, each probed as soon as it is found, beside another process holding an idle focus handler");
        probed(&mut report, Some("focus"), Some(DESKTOP), None);
    }
    if args.names("tooltip") {
        report.case("tooltip", "on the unlocked desktop: 30 new console windows, each probed as soon as it is found, beside another process holding an idle tooltip handler");
        match precondition::unlocked_desktop() {
            Ok(()) => probed(&mut report, Some("tooltip"), None, Some((7, 30))),
            Err(reason) => report.unmet("the probe beside an idle tooltip handler", &reason),
        }
    }
    if args.names("no-handler") {
        report.case("no-handler", "on a desktop of its own: 30 new console windows, each probed as soon as it is found, with no other process holding a handler");
        probed(&mut report, None, Some(DESKTOP), Some((0, 110)));
    }
    if args.names("stall") {
        report.case("stall", "the provider program's UI thread busy; UiaHasServerSideProvider on its window, released once it returns");
        stall(&mut report);
    }
    report.finish();
}

/// Starts a console host, probes its window at once with
/// `UiaHasServerSideProvider` and then the raw probe, and reports
/// `probe <answer> <ms> <raw probe answered 0x10000>`.
fn prober_role() -> ! {
    let console = Console::start();
    let (yes, took) = uia::timed(|| uia::has_server_side_provider(console.hwnd));
    let raw = verbatim_uia::raw_probe(console.hwnd) == verbatim_uia::RawProbe::Provider;
    role::report(&format!("probe {yes} {:.1} {raw}", uia::ms(took)));
    drop(console);
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
/// recorded k of n, or new.
fn probed(
    report: &mut Report,
    kind: Option<&str>,
    desktop: Option<&str>,
    record: Option<(usize, usize)>,
) {
    let handler = kind.map(|kind| {
        let handler = Role::start_on(desktop, "handler", &[kind.to_owned()]);
        assert_eq!(handler.expect_line(), "ready", "the handler is registered");
        handler
    });
    let mut no = 0;
    let mut no_ms = Vec::new();
    let mut raw = Vec::new();
    for _ in 0..TRIALS {
        let prober = Role::start_on(desktop, "prober", &[]);
        let line = prober
            .line(Duration::from_secs(30))
            .expect("the prober reports");
        let words: Vec<&str> = line.split(' ').collect();
        if words[1] == "false" {
            no += 1;
            no_ms.push(words[2].parse::<f64>().expect("a time"));
        }
        raw.push(words[3] == "true");
    }
    drop(handler);
    let beside = kind.map_or_else(
        || "with no other process's handler".to_owned(),
        |k| format!("beside an idle {k} handler"),
    );
    let claim = format!("trials in which UiaHasServerSideProvider answered no {beside}");
    let observed = format!("{no} of {TRIALS}");
    match record {
        Some((k, n)) => report.claim(
            &claim,
            &observed,
            &format!("{k} of {n}"),
            rate(no, TRIALS, k, n),
        ),
        None => report.claim(&claim, &observed, "", Verdict::New),
    }
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
        "the raw probe made after each answered 0x10000",
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
