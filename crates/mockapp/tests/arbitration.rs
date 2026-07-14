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
    let resolution = arbitrator.resolve_with(hwnd_value, &class, |h| {
        Some(common::eventually_true(|| has_server_side_provider(h)))
    });
    assert!(
        resolution.is_uia,
        "the real arbitrator must resolve a uia-backend window to UIA"
    );
    assert!(!resolution.probe_timed_out);

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
    let resolution =
        arbitrator.resolve_with(hwnd_value, &class, |h| Some(has_server_side_provider(h)));
    assert!(
        !resolution.is_uia,
        "the real arbitrator must resolve a msaa-backend window to MSAA"
    );
    assert!(!resolution.probe_timed_out);

    app.send("quit");
}
