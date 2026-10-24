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
//! - A subscription's move onto a held window ends at UIA's connection
//!   timeout, and the registration's next move is made after it, as is
//!   another registration's move, since every registration of a process
//!   shares its registering thread.
//! - A tree walk whose step to an element's first child is held fails with
//!   the step's `UIA_E_TIMEOUT`, rather than reading the element as having
//!   no children.
//!
//! Each test runs in a process of its own on a desktop of its own
//! (`common/harness.rs`), since the timeouts belong to the process.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_uia::{
    BoundedClient, CONNECTION_TIMEOUT, ElementExt as _, Fill, Node, NodeIdRegistry, Part,
    PropertyEvent, Registration, Scope, Shape, Subscription, TRANSACTION_TIMEOUT, Uia, Unanswered,
};
use verbatim_uia_rops::{TreeQuery, tree_dump_classic};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    TreeScope_Children, UIA_AutomationPropertyChangedEventId, UIA_E_TIMEOUT, UIA_NamePropertyId,
};
use windows::core::BSTR;

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

/// A subscription moved onto the subtree of a window whose provider holds
/// its call, the window's element looked up before the hold, and moved
/// again, while that call is held, onto another application's window: both
/// moves are made, the next registering on the other window, as that
/// application counts it, between one and two of UIA's connection
/// timeouts after the first was asked for, though the held call is still
/// held. So the held registration ended at the connection timeout, and
/// the second move's removal of the group from the held window cost no
/// timeout of its own.
fn a_held_subscription_move_returns_at_the_connection_timeout() {
    common::init_com();
    let held_title = common::unique_title("mockapp-held-move");
    let mut held_app = common::spawn("small.json", "uia", &held_title);
    let next_title = common::unique_title("mockapp-next-move");
    let next_app = common::spawn_counting_registrations("small.json", &next_title);
    let registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(|_, _| {}),
        })],
        Scope::Nothing,
    )
    .expect("a registration");
    let held_window = common::window_subtree(common::find_window(&held_title).0 as isize);
    let next_hwnd = common::find_window(&next_title);
    let next_window = common::window_subtree(next_hwnd.0 as isize);
    let advised = || common::advised(next_hwnd, UIA_AutomationPropertyChangedEventId.0);
    assert_eq!(
        advised(),
        0,
        "nothing is registered on the other window yet"
    );

    held_app.hold();
    let started = Instant::now();
    registration.retarget(held_window);
    let held = held_app.held();
    registration.retarget(next_window);
    registration.settle();
    let elapsed = started.elapsed();
    assert_eq!(held, "FragmentRoot", "the registration's call was held");
    assert!(
        elapsed >= CONNECTION_TIMEOUT && elapsed < CONNECTION_TIMEOUT * 2,
        "both moves were made after one connection timeout: {elapsed:?}"
    );
    assert_eq!(advised(), 1, "the next move registered on the other window");
    held_app.release();
    registration.close();
    held_app.quit();
    next_app.quit();
}
/// Two registrations of one process share its registering thread, so a
/// second registration's commands wait behind the first's: its move and
/// settle, asked for while the first registration's move onto a window
/// whose provider holds its call is held, wait until that move has returned
/// at UIA's connection timeout, so the second's settle returns no sooner
/// than one connection timeout after the first move was asked for, and
/// before two; and its move has then registered on the other application's
/// window, as that application counts it. Whether the second move was made
/// after the held one rather than before it is not shown: the settle waits
/// for both either way.
fn a_second_registrations_commands_wait_behind_the_firsts_held_move() {
    common::init_com();
    let held_title = common::unique_title("mockapp-held-first");
    let mut held_app = common::spawn("small.json", "uia", &held_title);
    let next_title = common::unique_title("mockapp-second-registration");
    let next_app = common::spawn_counting_registrations("small.json", &next_title);
    let registration = || {
        Registration::with_parts(
            vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
                properties: vec![UIA_NamePropertyId],
                callback: Arc::new(|_, _| {}),
            })],
            Scope::Nothing,
        )
        .expect("a registration")
    };
    let first = registration();
    let second = registration();
    let held_window = common::window_subtree(common::find_window(&held_title).0 as isize);
    let next_hwnd = common::find_window(&next_title);
    let next_window = common::window_subtree(next_hwnd.0 as isize);
    let advised = || common::advised(next_hwnd, UIA_AutomationPropertyChangedEventId.0);
    assert_eq!(
        advised(),
        0,
        "nothing is registered on the other window yet"
    );

    held_app.hold();
    let started = Instant::now();
    first.retarget(held_window);
    let held = held_app.held();
    second.retarget(next_window);
    second.settle();
    let elapsed = started.elapsed();
    assert_eq!(
        held, "FragmentRoot",
        "the first registration's call was held"
    );
    assert!(
        elapsed >= CONNECTION_TIMEOUT && elapsed < CONNECTION_TIMEOUT * 2,
        "the second registration's settle waited behind the first's held move: {elapsed:?}"
    );
    assert_eq!(
        advised(),
        1,
        "the second registration's move registered on the other window"
    );
    held_app.release();
    first.close();
    second.close();
    held_app.quit();
    next_app.quit();
}

/// A classic tree dump from mockapp's list, whose read of the list's
/// children is held at its first provider call, the list's `FragmentRoot`,
/// which UIA asks before it navigates, fails with that read's
/// `UIA_E_TIMEOUT` once UIA's transaction timeout has passed, rather than
/// reading the list as having no items.
fn a_tree_walk_held_at_a_child_step_fails_with_the_timeout() {
    common::init_com();
    let title = common::unique_title("mockapp-held-tree");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let uia = Uia::new().expect("a UIA client");
    let nodes = uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    // The list, a child of the window, found and walked before the hold, so
    // the held call is the held walk's first step. It is found by a search
    // of the window's children, not by a walk of the window's tree, which
    // would read the window's title bar: UIA asks the window's thread for
    // the title bar's location (`WM_GETTITLEBARINFOEX`), and Windows answers
    // that request for one window at a time across the desktop, so on a
    // busy machine the read can outlast the transaction timeout.
    let window = uia.window_live(hwnd).expect("mockapp's window element");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Options")))
        .expect("a condition");
    let list = window
        .find_first(TreeScope_Children, &condition, &nodes)
        .expect("the search")
        .expect("the window's list");
    let fill = Fill::new(&nodes);
    let query = TreeQuery {
        root: &list,
        fill: &fill,
        registry: &registry,
        max_depth: 8,
        max_nodes: 64,
    };
    let (unheld, _) = tree_dump_classic(&uia, &query).expect("the list's tree");
    assert_eq!(unheld.children.len(), 2, "the list's two items, unheld");

    app.hold();
    let walked = tree_dump_classic(&uia, &query);
    let held = app.held();
    app.release();
    assert_eq!(
        held, "FragmentRoot",
        "the read of the list's children was held"
    );
    let error = walked.expect_err("the held dump failed");
    assert_eq!(
        error.hresult(),
        Some(windows::core::HRESULT(UIA_E_TIMEOUT.cast_signed())),
        "the dump ended by UIA's timeout: {error}"
    );
    app.quit();
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
            "a_held_subscription_move_returns_at_the_connection_timeout",
            a_held_subscription_move_returns_at_the_connection_timeout,
        ),
        (
            "a_second_registrations_commands_wait_behind_the_firsts_held_move",
            a_second_registrations_commands_wait_behind_the_firsts_held_move,
        ),
        (
            "a_tree_walk_held_at_a_child_step_fails_with_the_timeout",
            a_tree_walk_held_at_a_child_step_fails_with_the_timeout,
        ),
    ]);
}
