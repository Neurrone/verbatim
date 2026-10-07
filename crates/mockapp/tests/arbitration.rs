//! Cross-process arbitration (architecture section 4, decision D1): asserts
//! that a `uia`-backend mockapp window has a server-side UIA provider and a
//! `msaa`-backend one does not, each found by a single probe, and that
//! `verbatim-outpost`'s real `Arbitrator`, given that probe's answer,
//! resolves each to the matching backend.
//!
//! Two tests depend on how long UIA's own check waits for a busy window
//! before it gives up. They do not assume it: the probe reports how many
//! times it asked, so a stall that UIA waited out, rather than gave up on,
//! fails the test instead of passing it without exercising the second ask.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::time::{Duration, Instant};

use verbatim_outpost::arbitration::{Arbitrator, WindowClasses};
use verbatim_uia::{PROBE_BUDGET, Probe, probe};

fn uia_backend_has_a_server_side_provider_and_arbitrates_to_uia() {
    let title = common::unique_title("mockapp-arb-uia");
    let app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;

    let classes = WindowClasses::of(hwnd);
    let mut arbitrator = Arbitrator::new(&[]);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        None,
        "only the probe decides"
    );
    let probed = probe(hwnd);
    assert_eq!(
        probed,
        Probe {
            answer: Some(true),
            asks: 1
        },
        "a uia-backend mockapp window has a server-side UIA provider, at the first ask"
    );
    arbitrator.record_probe(hwnd, true);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        Some(true),
        "the real arbitrator must resolve a uia-backend window to UIA"
    );

    app.quit();
}

fn a_busy_uia_window_still_has_a_server_side_provider() {
    let title = common::unique_title("mockapp-arb-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;

    // Block the window thread past the point where UIA's own check gives up
    // (about five seconds here, three for Notepad starting up) and reports
    // no provider: the probe must wait for the window's real answer. The
    // second ask is the evidence that UIA did give up.
    let stall = Duration::from_secs(6);
    app.stall(stall);
    let probed = probe(hwnd);
    let returned_at = common::now_us();
    let stall_ended_at = app.stall_ended(stall);
    assert_eq!(
        probed,
        Probe {
            answer: Some(true),
            asks: 2
        },
        "a window too busy to answer at once is asked again once it answers"
    );
    assert!(
        returned_at >= stall_ended_at,
        "the answer came from the window once it answered, {returned_at} before {stall_ended_at}"
    );

    app.quit();
}

fn a_busy_uia_window_is_read_once_it_answers() {
    let title = common::unique_title("mockapp-read-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = verbatim_uia::Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let name = |uia: &verbatim_uia::Uia| {
        uia.element_from_handle(hwnd, &cache)
            .map(|root| verbatim_uia::map::snapshot_parts_from_cached_element(&root).name)
            .expect("the root element is read")
    };
    assert_eq!(name(&uia).as_deref(), Some("Mockapp Events Fixture"));

    // Past UIA's default two-second connection timeout, after which the read
    // failed (or, for a focused element, came back as UIA's stand-in).
    let stall = Duration::from_secs(4);
    app.stall(stall);
    let read = name(&uia);
    let returned_at = common::now_us();
    let stall_ended_at = app.stall_ended(stall);
    assert_eq!(
        read.as_deref(),
        Some("Mockapp Events Fixture"),
        "a read of a busy window waits for the application's own answer"
    );
    assert!(
        returned_at >= stall_ended_at,
        "the read returned once the window answered, {returned_at} before {stall_ended_at}"
    );

    app.quit();
}

fn msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa() {
    let title = common::unique_title("mockapp-arb-msaa");
    let app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;

    let classes = WindowClasses::of(hwnd);
    let mut arbitrator = Arbitrator::new(&[]);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        None,
        "only the probe decides"
    );
    // An answer, not silence: "no provider" said by the window itself.
    assert_eq!(
        probe(hwnd),
        Probe {
            answer: Some(false),
            asks: 1
        },
        "a msaa-backend mockapp window answers that it has no server-side UIA provider"
    );
    arbitrator.record_probe(hwnd, false);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        Some(false),
        "the real arbitrator must resolve a msaa-backend window to MSAA"
    );

    app.quit();
}

fn a_window_that_never_answers_gets_no_verdict_within_the_budget() {
    let title = common::unique_title("mockapp-arb-silent");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;

    // Silent for longer than the probe may take: no answer, rather than a
    // "no" that would be kept, and well inside the outpost's ten-second
    // deadline for handling the event. The probe asked once, UIA gave up,
    // and the window never answered the wait that followed, so it was not
    // asked again.
    let stall = Duration::from_secs(12);
    app.stall(stall);
    let started = Instant::now();
    let probed = probe(hwnd);
    let elapsed = started.elapsed();
    assert_eq!(
        probed,
        Probe {
            answer: None,
            asks: 1
        }
    );
    // It waited out its whole budget for the window; the second bound only
    // stops a hang.
    assert!(
        elapsed >= PROBE_BUDGET,
        "the probe waits its budget for the window to answer, gave up after {elapsed:?}"
    );
    assert!(
        elapsed < PROBE_BUDGET + Duration::from_secs(1),
        "the probe stays within its budget, took {elapsed:?}"
    );

    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
        (
            "uia_backend_has_a_server_side_provider_and_arbitrates_to_uia",
            uia_backend_has_a_server_side_provider_and_arbitrates_to_uia,
        ),
        (
            "a_busy_uia_window_still_has_a_server_side_provider",
            a_busy_uia_window_still_has_a_server_side_provider,
        ),
        (
            "a_busy_uia_window_is_read_once_it_answers",
            a_busy_uia_window_is_read_once_it_answers,
        ),
        (
            "msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa",
            msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa,
        ),
        (
            "a_window_that_never_answers_gets_no_verdict_within_the_budget",
            a_window_that_never_answers_gets_no_verdict_within_the_budget,
        ),
    ]);
}
