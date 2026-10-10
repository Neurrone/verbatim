//! UIA's time limits against a provider that never answers
//! (`docs/design/focus-pipeline.md`, section 6): mockapp holds the next
//! provider call (`hold`) until the test releases it, and each limit is
//! shown to end the wait while the call is still held.
//!
//! - A property read, one provider call, ends at UIA's call timeout, which
//!   the process's first client set ([`verbatim_uia::CALL_TIMEOUT`]).
//! - A search, many provider calls, ends at its own deadline on a
//!   [`BoundedClient`], before any call timeout, its thread abandoned.
//! - A subscription's move that does not finish is abandoned when the
//!   registration is settled, at [`MOVE_DEADLINE`], and its thread
//!   replaced.
//!
//! Each test runs in a process of its own on a desktop of its own
//! (`common/harness.rs`), since the timeouts belong to the process.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_uia::{
    BoundedClient, CALL_TIMEOUT, ElementExt as _, MOVE_DEADLINE, Registration, Scope, Subscription,
    Uia, Unanswered,
};
use windows::Win32::UI::Accessibility::UIA_NamePropertyId;
use windows::core::AgileReference;

/// How long a search may take in the test: well under UIA's call timeout,
/// so the search's end is its deadline's, not UIA's.
const SEARCH_DEADLINE: Duration = Duration::from_secs(1);

/// A property read held in the provider fails with `UIA_E_TIMEOUT` once
/// UIA's call timeout has passed, while the call is still held: one
/// timeout, not UIA's default transaction timeout of 20 seconds.
fn a_held_property_read_ends_at_the_call_timeout() {
    common::init_com();
    let title = common::unique_title("mockapp-held-read");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let (ready_tx, ready) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    let reader = std::thread::spawn(move || {
        common::init_com();
        let uia = Uia::new().expect("a UIA client");
        let cache = uia.base_cache_request().expect("a cache request");
        // Fetched before the hold, so the held call is a transaction.
        let window = uia
            .element_from_handle(hwnd, &cache)
            .expect("mockapp's window element");
        ready_tx.send(()).expect("the test waits");
        go_rx.recv().expect("the test says when");
        let started = Instant::now();
        let answer = window.has_keyboard_focus();
        (answer, started.elapsed())
    });
    ready.recv().expect("the reader is ready");
    app.hold();
    go.send(()).expect("the reader waits");
    let held = app.held();
    let (answer, elapsed) = reader.join().expect("the reader returns");
    app.release();
    assert_eq!(held, "GetPropertyValue", "the read was the call held");
    let error = answer.expect_err("the held read failed");
    assert!(
        verbatim_uia::timed_out(&error),
        "the read ended by UIA's timeout: {error}"
    );
    assert!(
        elapsed >= CALL_TIMEOUT && elapsed < CALL_TIMEOUT * 2,
        "the read ended after one call timeout: {elapsed:?}"
    );
    app.quit();
}

/// A search held in the provider is given up at its own deadline, before
/// UIA's call timeout would end any of its calls, and its thread is
/// abandoned; the abandoned thread ends once the call is released.
fn a_held_search_ends_at_its_deadline() {
    common::init_com();
    let title = common::unique_title("mockapp-held-search");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let cache = uia.base_cache_request().expect("a cache request");
    let window = uia
        .element_from_handle(hwnd, &cache)
        .expect("mockapp's window element");
    let window = AgileReference::new(&window).expect("an agile reference");
    let bounded = BoundedClient::default();
    // The thread and its client are made before the hold, by a search that
    // answers.
    let warm = window.clone();
    let found = bounded.run(common::WAIT_TIMEOUT, move |uia| {
        let cache = uia.base_cache_request()?;
        uia.element_by_runtime_id(&warm.resolve()?, &[42, 42], &cache)
            .map(|found| found.is_some())
    });
    assert!(matches!(found, Ok(Ok(false))), "{found:?}");

    app.hold();
    let started = Instant::now();
    let found = bounded.run(SEARCH_DEADLINE, move |uia| {
        let cache = uia.base_cache_request()?;
        uia.element_by_runtime_id(&window.resolve()?, &[42, 42], &cache)
            .map(|found| found.is_some())
    });
    let elapsed = started.elapsed();
    let held = app.held();
    assert!(
        matches!(found, Err(Unanswered::DeadlinePassed)),
        "{found:?}"
    );
    assert!(
        elapsed >= SEARCH_DEADLINE && elapsed < CALL_TIMEOUT,
        "the search ended at its deadline: {elapsed:?}"
    );
    assert!(!held.is_empty(), "a call of the search was held");
    assert_eq!(bounded.abandoned(), 1);
    app.release();
    assert_eq!(bounded.close(), 1, "the abandoned thread ended");
    app.quit();
}

/// A subscription moved onto a held window three times over, so that its
/// move makes three calls that each wait out UIA's call timeout, longer in
/// all than [`MOVE_DEADLINE`], is abandoned when settled once the deadline
/// has passed, and replaced; closing the registration waits for the
/// abandoned thread, which ends once the call is released. (One held call
/// alone ends at the call timeout, within the deadline.)
fn a_held_subscription_move_is_abandoned_at_its_deadline() {
    common::init_com();
    let title = common::unique_title("mockapp-held-move");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let registration = Registration::new(
        vec![Subscription::Properties {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(|_, _| {}),
        }],
        Scope::Nothing,
    )
    .expect("a registration");

    app.hold();
    registration.retarget(Scope::Windows(vec![hwnd; 3]));
    let held = app.held();
    let started = Instant::now();
    registration.settle();
    let elapsed = started.elapsed();
    assert!(!held.is_empty(), "a call of the move was held");
    assert!(
        elapsed >= MOVE_DEADLINE && elapsed < MOVE_DEADLINE + CALL_TIMEOUT,
        "the settle ended at the move's deadline: {elapsed:?}"
    );
    assert_eq!(registration.abandoned(), 1);
    app.release();
    registration.close();
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_held_property_read_ends_at_the_call_timeout",
            a_held_property_read_ends_at_the_call_timeout,
        ),
        (
            "a_held_search_ends_at_its_deadline",
            a_held_search_ends_at_its_deadline,
        ),
        (
            "a_held_subscription_move_is_abandoned_at_its_deadline",
            a_held_subscription_move_is_abandoned_at_its_deadline,
        ),
    ]);
}
