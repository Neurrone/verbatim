//! Cross-process arbitration (architecture section 4, decision D1): asserts
//! that a `uia`-backend mockapp window has a server-side UIA provider and a
//! `msaa`-backend one does not, each found by a single raw probe, the one
//! `WM_GETOBJECT` the outposts and the focus listener make, and that
//! `verbatim-outpost`'s real `Arbitrator`, given that probe's answer,
//! resolves each to the matching backend, and that a window too busy to
//! answer is asked once and its "no" taken, as NVDA takes it.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::time::Duration;

use verbatim_model::CallCounts;
use verbatim_outpost::arbitration::{Arbitrator, WindowClasses};
use verbatim_uia::{RAW_PROBE_TIMEOUT_MS, RawProbe, raw_probe};

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
    assert_eq!(
        raw_probe(hwnd),
        RawProbe::Provider,
        "the raw probe finds a uia-backend mockapp window's provider"
    );
    arbitrator.record_probe(hwnd, true);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        Some(true),
        "the real arbitrator must resolve a uia-backend window to UIA"
    );

    app.quit();
}

fn a_busy_uia_window_is_asked_once_and_its_no_taken() {
    let title = common::unique_title("mockapp-arb-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;

    // Blocked past the point where the raw probe gives up
    // (`RAW_PROBE_TIMEOUT_MS`) and reports no provider: that "no" is the
    // answer, from one message, as NVDA takes it.
    let stall = Duration::from_millis(u64::from(RAW_PROBE_TIMEOUT_MS) * 2);
    app.stall(stall);
    let _ = verbatim_uia::calls::take();
    let probed = raw_probe(hwnd);
    let returned_at = common::now_us();
    let calls = verbatim_uia::calls::take();
    let stall_ended_at = app.stall_ended(stall);
    assert_eq!(
        probed,
        RawProbe::TimedOut,
        "a window too busy to answer has no provider, for now"
    );
    assert_eq!(
        calls,
        CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 1,
            connections: 0,
        },
        "the window is asked once, with no connection"
    );
    assert!(
        returned_at < stall_ended_at,
        "the answer came while the window was busy, {returned_at} after {stall_ended_at}"
    );

    app.quit();
}

fn a_busy_uia_window_is_read_once_it_answers() {
    let title = common::unique_title("mockapp-read-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = verbatim_uia::Uia::new().expect("Uia::new");
    let nodes = uia
        .request::<verbatim_uia::Node>(<verbatim_uia::Node as verbatim_uia::Shape>::all_on())
        .expect("the node's cache request");
    let name = |uia: &verbatim_uia::Uia| {
        uia.window_element(hwnd, &nodes)
            .map(|root| root.read().name)
            .expect("the root element is read")
    };
    assert_eq!(name(&uia).as_deref(), Some("Mockapp Events Fixture"));

    // Within the connection timeout, which ends a lookup of a window busy
    // for longer (`docs/design/listener-without-connections.md`, section 5).
    let stall = verbatim_uia::CONNECTION_TIMEOUT / 2;
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
        raw_probe(hwnd),
        RawProbe::Answered(0),
        "a msaa-backend mockapp window answers the raw probe with the default window procedure's answer"
    );
    arbitrator.record_probe(hwnd, false);
    assert_eq!(
        arbitrator.verdict(hwnd, &classes),
        Some(false),
        "the real arbitrator must resolve a msaa-backend window to MSAA"
    );

    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "uia_backend_has_a_server_side_provider_and_arbitrates_to_uia",
            uia_backend_has_a_server_side_provider_and_arbitrates_to_uia,
        ),
        (
            "a_busy_uia_window_is_asked_once_and_its_no_taken",
            a_busy_uia_window_is_asked_once_and_its_no_taken,
        ),
        (
            "a_busy_uia_window_is_read_once_it_answers",
            a_busy_uia_window_is_read_once_it_answers,
        ),
        (
            "msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa",
            msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa,
        ),
    ]);
}
