//! UIA's time limits against a provider that never answers
//! (`docs/design/focus-pipeline.md`, section 6): mockapp holds the next
//! provider call (`hold`) until the test releases it, and each limit is
//! shown to end the wait while the call is still held.
//!
//! - A property read, one provider call on a held element, ends at UIA's
//!   transaction timeout, which the process's first client set
//!   ([`verbatim_uia::TRANSACTION_TIMEOUT`]).
//! - A search, many provider calls, ends at its own deadline on a
//!   [`BoundedClient`], before any transaction timeout, its thread abandoned.
//! - A subscription's move whose registrations each wait out UIA's
//!   connection timeout and do not finish is abandoned when the
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
    BoundedClient, CONNECTION_TIMEOUT, ElementExt as _, MOVE_DEADLINE, Node, Part, PropertyEvent,
    Registration, Scope, Shape, Subscription, TRANSACTION_TIMEOUT, Uia, Unanswered,
};
use windows::Win32::UI::Accessibility::UIA_NamePropertyId;

/// How long a search may take in the test: well under UIA's transaction timeout,
/// so the search's end is its deadline's, not UIA's.
const SEARCH_DEADLINE: Duration = Duration::from_secs(1);

/// A property read held in the provider fails with `UIA_E_TIMEOUT` once
/// UIA's transaction timeout has passed, while the call is still held: one
/// timeout, not UIA's default transaction timeout of 20 seconds.
fn a_held_property_read_ends_at_the_transaction_timeout() {
    common::init_com();
    let title = common::unique_title("mockapp-held-read");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let (ready_tx, ready) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    let reader = std::thread::spawn(move || {
        common::init_com();
        let uia = Uia::new().expect("a UIA client");
        let nodes = uia
            .request::<Node>(Node::all_on())
            .expect("a cache request");
        // Fetched before the hold, so the held call is a transaction.
        let window = uia
            .window_element(hwnd, &nodes)
            .expect("mockapp's window element");
        ready_tx.send(()).expect("the test waits");
        go_rx.recv().expect("the test says when");
        let started = Instant::now();
        let answer = window.live().element().has_keyboard_focus();
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
        elapsed >= TRANSACTION_TIMEOUT && elapsed < TRANSACTION_TIMEOUT * 2,
        "the read ended after one transaction timeout: {elapsed:?}"
    );
    app.quit();
}

/// A search held in the provider is given up at its own deadline, before
/// UIA's transaction timeout would end any of its calls, and its thread is
/// abandoned; the abandoned thread ends once the call is released.
fn a_held_search_ends_at_its_deadline() {
    common::init_com();
    let title = common::unique_title("mockapp-held-search");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let nodes = uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    let window = uia
        .window_element(hwnd, &nodes)
        .expect("mockapp's window element")
        .agile()
        .expect("an agile reference");
    let bounded = BoundedClient::default();
    // The thread and its client are made before the hold, by a search that
    // answers.
    let warm = window.clone();
    let found = bounded.run(common::WAIT_TIMEOUT, move |uia| {
        let nodes = uia.request::<Node>(Node::all_on())?;
        uia.find_by_runtime_id(warm.resolve()?.live(), &[42, 42], &nodes)
            .map(|found| found.is_some())
    });
    assert!(matches!(found, Ok(Ok(false))), "{found:?}");

    app.hold();
    let started = Instant::now();
    let found = bounded.run(SEARCH_DEADLINE, move |uia| {
        let nodes = uia.request::<Node>(Node::all_on())?;
        uia.find_by_runtime_id(window.resolve()?.live(), &[42, 42], &nodes)
            .map(|found| found.is_some())
    });
    let elapsed = started.elapsed();
    let held = app.held();
    assert!(
        matches!(found, Err(Unanswered::DeadlinePassed)),
        "{found:?}"
    );
    assert!(
        elapsed >= SEARCH_DEADLINE && elapsed < TRANSACTION_TIMEOUT,
        "the search ended at its deadline: {elapsed:?}"
    );
    assert!(!held.is_empty(), "a call of the search was held");
    assert_eq!(bounded.abandoned(), 1);
    app.release();
    assert_eq!(bounded.close(), 1, "the abandoned thread ended");
    app.quit();
}

/// A subscription moved onto the subtrees of [`HELD_WINDOWS`] held
/// windows, each of a mockapp of its own, their elements looked up before
/// the holds, so that its move makes that many registrations that each
/// wait out UIA's connection timeout, longer in all than
/// [`MOVE_DEADLINE`], is abandoned when settled once the deadline has
/// passed, and replaced; closing the registration waits for the abandoned
/// thread, which ends once the calls are released. (One held registration
/// alone ends at the connection timeout, within the deadline.)
/// The windows are three applications' because the move's own calls are
/// its registrations, and registering on a second element of an
/// application whose registration timed out makes no call there.
/// How many held windows a move registers on: one more than the
/// connection timeouts that fit in [`MOVE_DEADLINE`].
const HELD_WINDOWS: u32 = 6;

fn a_held_subscription_move_is_abandoned_at_its_deadline() {
    assert!(
        CONNECTION_TIMEOUT * (HELD_WINDOWS - 1) >= MOVE_DEADLINE,
        "the held registrations outlast the move's deadline"
    );
    common::init_com();
    let mut apps: Vec<common::MockApp> = (0..HELD_WINDOWS)
        .map(|index| {
            let title = common::unique_title(&format!("mockapp-held-move-{index}"));
            common::spawn("small.json", "uia", &title)
        })
        .collect();
    let registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(|_, _| {}),
        })],
        Scope::Nothing,
    )
    .expect("a registration");

    let elements = apps
        .iter()
        .flat_map(|app| {
            let hwnd = common::find_window(app.title()).0 as isize;
            let Scope::Subtrees(window) = common::window_subtree(hwnd) else {
                unreachable!("a window's subtree")
            };
            window
        })
        .collect();
    for app in &mut apps {
        app.hold();
    }
    registration.retarget(Scope::Subtrees(elements));
    let held = apps[0].held();
    let started = Instant::now();
    registration.settle();
    let elapsed = started.elapsed();
    assert!(!held.is_empty(), "a call of the move was held");
    assert!(
        elapsed >= MOVE_DEADLINE && elapsed < MOVE_DEADLINE + CONNECTION_TIMEOUT,
        "the settle ended at the move's deadline: {elapsed:?}"
    );
    assert_eq!(registration.abandoned(), 1);
    // The abandoned move went on to the others.
    for app in &mut apps[1..] {
        assert!(!app.held().is_empty(), "a call of the move was held");
    }
    for app in &mut apps {
        app.release();
    }
    registration.close();
    for app in apps {
        app.quit();
    }
}
/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_held_property_read_ends_at_the_transaction_timeout",
            a_held_property_read_ends_at_the_transaction_timeout,
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
