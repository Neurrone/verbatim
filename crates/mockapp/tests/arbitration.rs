//! Cross-process arbitration (architecture section 4, decision D1): asserts
//! that a `uia`-backend mockapp window has a server-side UIA provider and a
//! `msaa`-backend one does not, and that `verbatim-outpost`'s real
//! `Arbitrator`, driven by the real probe, resolves each to the matching
//! backend.

mod common;

use verbatim_outpost::arbitration::{Arbitrator, window_class_name};
use verbatim_uia::has_server_side_provider;

#[test]
fn uia_backend_has_a_server_side_provider_and_arbitrates_to_uia() {
    let title = common::unique_title("mockapp-arb-uia");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let hwnd_value = hwnd.0 as isize;

    assert!(
        common::eventually_true(|| has_server_side_provider(hwnd_value)),
        "a uia-backend mockapp window must expose a server-side UIA provider"
    );

    let class = window_class_name(hwnd_value);
    let mut arbitrator = Arbitrator::new(&[]);
    assert_eq!(
        arbitrator.verdict(hwnd_value, &class),
        None,
        "only the probe decides"
    );
    arbitrator.record_probe(
        hwnd_value,
        common::eventually_true(|| has_server_side_provider(hwnd_value)),
    );
    assert_eq!(
        arbitrator.verdict(hwnd_value, &class),
        Some(true),
        "the real arbitrator must resolve a uia-backend window to UIA"
    );

    app.send("quit");
}

#[test]
fn a_busy_uia_window_still_has_a_server_side_provider() {
    let title = common::unique_title("mockapp-arb-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    assert!(common::eventually_true(|| has_server_side_provider(hwnd)));

    // Block the window thread past the point where UIA's own check gives up
    // (about five seconds here, three for Notepad starting up) and reports
    // no provider: the probe must wait for the window's real answer.
    app.send("stall 6000");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        has_server_side_provider(hwnd),
        "a window too busy to answer at once is asked again once it answers"
    );

    app.send("quit");
}

#[test]
fn a_busy_uia_window_is_read_once_it_answers() {
    let title = common::unique_title("mockapp-read-busy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = verbatim_uia::Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let name = |uia: &verbatim_uia::Uia| {
        uia.element_from_handle(hwnd, &cache)
            // SAFETY: built with the base cache request.
            .map(
                |root| unsafe { verbatim_uia::map::snapshot_parts_from_cached_element(&root) }.name,
            )
    };
    assert_eq!(
        name(&uia).ok().flatten().as_deref(),
        Some("Mockapp Events Fixture")
    );

    // Past UIA's default two-second connection timeout, after which the read
    // failed (or, for a focused element, came back as UIA's stand-in).
    app.send("stall 4000");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        name(&uia).ok().flatten().as_deref(),
        Some("Mockapp Events Fixture"),
        "a read of a busy window waits for the application's own answer"
    );

    app.send("quit");
}

#[test]
fn msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa() {
    let title = common::unique_title("mockapp-arb-msaa");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let hwnd_value = hwnd.0 as isize;

    assert!(
        !has_server_side_provider(hwnd_value),
        "a msaa-backend mockapp window must not expose a server-side UIA provider"
    );

    let class = window_class_name(hwnd_value);
    let mut arbitrator = Arbitrator::new(&[]);
    assert_eq!(
        arbitrator.verdict(hwnd_value, &class),
        None,
        "only the probe decides"
    );
    arbitrator.record_probe(hwnd_value, has_server_side_provider(hwnd_value));
    assert_eq!(
        arbitrator.verdict(hwnd_value, &class),
        Some(false),
        "the real arbitrator must resolve a msaa-backend window to MSAA"
    );

    app.send("quit");
}

#[test]
fn a_window_that_never_answers_gets_no_verdict_within_the_budget() {
    let title = common::unique_title("mockapp-arb-silent");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    assert!(common::eventually_true(|| has_server_side_provider(hwnd)));

    // Silent for longer than the probe may take: no answer, rather than a
    // "no" that would be kept, and well inside the outpost's ten-second
    // deadline for handling the event.
    app.send("stall 12000");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let started = std::time::Instant::now();
    assert_eq!(verbatim_uia::probe_server_side_provider(hwnd), None);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(9),
        "the probe stays within its budget, took {:?}",
        started.elapsed()
    );

    app.send("quit");
}
