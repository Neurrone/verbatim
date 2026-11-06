//! The failure paths of the outpost's worker and of the UIA subscription,
//! each run through its fault point (`books/dev/src/reference/crates/verbatim-faults.md`, "Failure paths"): a worker that cannot start leaves its
//! generation with none, whose programs are answered as failed until a
//! dispatch starts one; a group or focus handler whose removal failed is
//! kept and removed again at the registration's close, or when the
//! registering thread ends.
//!
//! The tests arm fault points, so only `cargo xtask faults` builds this
//! binary; each runs in a process of its own (`common/harness.rs`), so no
//! test's armed point reaches another's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use verbatim_faults::Fault;
use verbatim_model::protocol::{OutpostToCore, Query, QueryOutcome};
use verbatim_uia::{Part, PropertyEvent, Registration, Scope, Subscription, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{UIA_AutomationPropertyChangedEventId, UIA_NamePropertyId};

use common::outpost::OutpostUnderTest;

/// The outpost's worker thread's name, which its fault point's subject
/// carries.
const WORKER: &str = "verbatim-worker";

/// Arms the worker's start of `generation` to fail `times` times.
fn fail_worker_start(generation: u64, times: usize) -> verbatim_faults::Guard {
    verbatim_faults::arm(
        Fault::ThreadStart,
        move |&(name, started): &(&'static str, u64)| name == WORKER && started == generation,
        times,
    )
}

/// Focuses mockapp's button "Original Name" and has the outpost read it
/// as the focused element, as a focus-now query reads the system's focus.
fn focus_button(app: &mut common::MockApp, outpost: &OutpostUnderTest, title: &str) {
    app.send("set-focus btn1");
    let uia = Uia::new().expect("a UIA client");
    let focused = common::listener_focus(&uia, common::find_window(title).0 as isize);
    outpost.read_focus_as(focused.live().element());
}

/// The name of the focused control in the outpost's answer to the focus
/// now, which must be its next message, and the only one.
fn focused_name(outpost: &mut OutpostUnderTest) -> Option<String> {
    let (now, _) = outpost.focus_now_answer();
    now.focus.and_then(|focused| focused.node.name)
}

/// The outpost's answer to `request`, which must be its next message.
fn outcome(
    outpost: &OutpostUnderTest,
    request: verbatim_model::protocol::RequestId,
) -> QueryOutcome {
    match outpost.next() {
        OutpostToCore::Reply {
            request_id,
            outcome,
            ..
        } if request_id == request => outcome,
        other => panic!("the outpost said {other:?}, not its answer to {request:?}"),
    }
}

/// A first worker that cannot start, at the outpost's start nor at the
/// first dispatch, leaves the first focus query answered as failed; the
/// second dispatch starts it, and the second query is answered with its
/// read; the outpost shuts down.
fn a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next() {
    let title = common::unique_title("mockapp-first-worker");
    let mut app = common::spawn("small.json", "uia", &title);
    let refusal = fail_worker_start(0, 2);
    let mut outpost = OutpostUnderTest::new(&app);
    focus_button(&mut app, &outpost, &title);

    let first = outpost.ask(Query::FocusNow);
    assert_eq!(
        outcome(&outpost, first),
        QueryOutcome::Failed("the outpost failed handling the query".to_owned()),
        "no worker could be had for the first query"
    );
    outpost.settled();
    assert_eq!(
        focused_name(&mut outpost).as_deref(),
        Some("Original Name"),
        "the second query's dispatch started the worker, which read it"
    );
    drop(refusal);
    outpost.outpost.shutdown();
    app.quit();
}

/// A replacement worker that cannot start: generation 0 is abandoned at its
/// deadline, shortened from 11 seconds to 2, while mockapp holds its call,
/// and generation 1's start fails;
/// once the call is let go, the next program's dispatch starts generation
/// 1's worker, which answers it with its read.
fn a_replacement_worker_that_cannot_start_answers_the_next_program() {
    let title = common::unique_title("mockapp-replacement-worker");
    let mut app = common::spawn("small.json", "uia", &title);
    let mut outpost = OutpostUnderTest::new(&app);
    outpost
        .outpost
        .set_handling_deadline(Duration::from_secs(2));
    focus_button(&mut app, &outpost, &title);
    let refusal = fail_worker_start(1, 1);

    app.hold();
    let held = outpost.ask(Query::FocusNow);
    let call = app.held();
    assert_eq!(
        outcome(&outpost, held),
        QueryOutcome::Abandoned,
        "the held query, abandoned at its deadline ({call} held)"
    );
    app.release();
    assert_eq!(
        focused_name(&mut outpost).as_deref(),
        Some("Original Name"),
        "generation 1's worker, started at the dispatch, read the next query"
    );
    drop(refusal);
    outpost.outpost.shutdown();
    app.quit();
}

/// A registration on `scope` for name changes, sending each changed name
/// it hears.
fn name_changes(scope: Scope) -> (Registration, mpsc::Receiver<Option<String>>) {
    let (sender, names) = mpsc::channel();
    let registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(move |element, _| {
                // The test may have finished.
                let _ = sender.send(element.read().name);
            }),
        })],
        scope,
    )
    .expect("a registration");
    (registration, names)
}

/// How many property-change registrations are live on `hwnd`, as its
/// mockapp counts them.
fn advised(hwnd: HWND) -> i64 {
    common::advised(hwnd, UIA_AutomationPropertyChangedEventId.0)
}

/// The runtime id of the window element of `hwnd`, which a registration
/// on its subtree registers on.
fn window_runtime_id(hwnd: HWND) -> Vec<i32> {
    let uia = Uia::new().expect("a UIA client");
    let window = uia
        .window_live(hwnd.0 as isize)
        .expect("the window's element");
    verbatim_uia::runtime_id(window.element())
}

/// Arms the removal of the group from the element whose runtime id is
/// `runtime_id` to fail `times` times.
fn fail_removal(runtime_id: Vec<i32>, times: usize) -> verbatim_faults::Guard {
    verbatim_faults::arm(
        Fault::RemoveGroup,
        move |(_, removed): &(u64, Vec<i32>)| *removed == runtime_id,
        times,
    )
}

/// Two mockapps counting their registrations, and their windows.
fn two_applications(name: &str) -> ([common::MockApp; 2], [HWND; 2]) {
    let titles = [
        common::unique_title(&format!("{name}-old")),
        common::unique_title(&format!("{name}-new")),
    ];
    let apps = titles
        .each_ref()
        .map(|title| common::spawn_counting_registrations("small.json", title));
    (
        apps,
        titles.each_ref().map(|title| common::find_window(title)),
    )
}

/// A group whose removal at a move failed stays live on the old element
/// after the move, and is removed by the close, whose wait is answered.
fn a_group_a_move_failed_to_remove_is_removed_at_close() {
    common::init_com();
    let (mut apps, [old, new]) = two_applications("mockapp-move-unremoved");
    let (registration, _names) = name_changes(common::window_subtree(old.0 as isize));
    assert_eq!(
        (advised(old), advised(new)),
        (1, 0),
        "registered on the old window"
    );
    let refusal = fail_removal(window_runtime_id(old), 1);

    registration.retarget(common::window_subtree(new.0 as isize));
    registration.settle();
    assert_eq!(
        (advised(old), advised(new)),
        (1, 1),
        "the old element keeps its group after the move"
    );
    registration.close();
    // The close removed both groups.
    for app in &mut apps {
        app.await_advised(UIA_AutomationPropertyChangedEventId.0, 0);
    }
    drop(refusal);
    for app in apps {
        app.quit();
    }
}

/// A group whose removal failed at the move and again at the close
/// outlives the close, while the new element's is removed; it is removed
/// when the registering thread ends, and a registration made afterwards
/// starts a fresh thread and hears its events.
fn a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends() {
    common::init_com();
    let (mut apps, [old, new]) = two_applications("mockapp-close-unremoved");
    let (registration, _names) = name_changes(common::window_subtree(old.0 as isize));
    let refusal = fail_removal(window_runtime_id(old), 2);

    registration.retarget(common::window_subtree(new.0 as isize));
    registration.settle();
    registration.close();
    // The new element's removal has reached its provider, and the old
    // element's group, whose removal failed, outlives the close.
    apps[1].await_advised(UIA_AutomationPropertyChangedEventId.0, 0);
    assert_eq!(
        advised(old),
        1,
        "the old element's group outlives the close"
    );
    verbatim_uia::release_mta_usage();
    assert!(
        !verbatim_uia::registering_thread_runs(),
        "the registering thread ended"
    );
    // The old group went with the registering thread.
    apps[0].await_advised(UIA_AutomationPropertyChangedEventId.0, 0);
    drop(refusal);

    let (_later, names) = name_changes(common::window_subtree(new.0 as isize));
    assert!(
        verbatim_uia::registering_thread_runs(),
        "the later registration started a fresh registering thread"
    );
    apps[1].send("set-name btn1 Renamed");
    assert_eq!(
        names.recv_timeout(common::WAIT_TIMEOUT),
        Ok(Some("Renamed".to_owned())),
        "the later registration hears its events"
    );
    for app in apps {
        app.quit();
    }
}

/// A part listening for focus changes, which hears nothing the test reads.
fn focus_part() -> Part {
    Part::shaped::<PropertyEvent>(Subscription::Focus {
        callback: Arc::new(|_| {}),
    })
}

/// A registration whose second focus handler cannot be added, and whose
/// first handler's removal then fails, is answered with the add's error,
/// and the registering thread keeps it until it ends; a registration made
/// afterwards is made.
fn a_focus_handler_left_by_a_failed_setting_up_is_kept_until_the_registering_thread_ends() {
    common::init_com();
    let failed_add = verbatim_faults::arm(
        Fault::AddFocusHandler,
        |&(_, index): &(u64, usize)| index == 1,
        1,
    );
    let failed_removal = verbatim_faults::arm(
        Fault::RemoveFocusHandler,
        |&(_, index): &(u64, usize)| index == 0,
        1,
    );
    let error = Registration::with_parts(vec![focus_part(), focus_part()], Scope::Nothing)
        .err()
        .expect("the registration failed");
    assert_eq!(
        error.code(),
        windows::Win32::Foundation::E_FAIL,
        "answered with the add's error"
    );
    assert_eq!(
        verbatim_uia::kept_for_tests(),
        1,
        "the registration whose handler's removal failed is kept"
    );
    drop(failed_add);
    drop(failed_removal);

    verbatim_uia::release_mta_usage();
    assert_eq!(
        verbatim_uia::kept_for_tests(),
        0,
        "the registering thread ended and let it go"
    );
    let later = Registration::with_parts(vec![focus_part()], Scope::Nothing);
    assert!(
        later.is_ok(),
        "a later registration is made: {:?}",
        later.err()
    );
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next",
            a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next,
        ),
        (
            "a_replacement_worker_that_cannot_start_answers_the_next_program",
            a_replacement_worker_that_cannot_start_answers_the_next_program,
        ),
        (
            "a_group_a_move_failed_to_remove_is_removed_at_close",
            a_group_a_move_failed_to_remove_is_removed_at_close,
        ),
        (
            "a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends",
            a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends,
        ),
        (
            "a_focus_handler_left_by_a_failed_setting_up_is_kept_until_the_registering_thread_ends",
            a_focus_handler_left_by_a_failed_setting_up_is_kept_until_the_registering_thread_ends,
        ),
    ]);
}
