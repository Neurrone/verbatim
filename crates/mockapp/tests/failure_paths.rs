//! The failure paths of the outpost's worker and of the UIA subscription,
//! each run through its fault point (`books/dev/src/reference/crates/verbatim-faults.md`, "Failure paths"): a worker that cannot start leaves its
//! generation with none, whose programs are answered as failed until a
//! dispatch starts one; a group or focus handler whose removal failed is
//! kept and removed again at the registration's close, or when the
//! registering thread ends; a batch step whose calls fail is answered
//! failed.
//!
//! The tests arm fault points, so only `cargo xtask faults` builds this
//! binary; each runs in a process of its own (`common/harness.rs`), so no
//! test's armed point reaches another's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, mpsc};

use verbatim_faults::Fault;
use verbatim_model::request::Outcome;
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
/// as the focused element, as Core's request for the current focus reads
/// the system's focus.
fn focus_button(app: &mut common::MockApp, outpost: &OutpostUnderTest, title: &str) {
    app.send("set-focus btn1");
    let uia = Uia::new().expect("a UIA client");
    let focused = common::listener_focus(&uia, common::find_window(title).0 as isize);
    outpost.read_focus_as(focused.live().element());
}

/// The name of the application's current focus, as Core's request for it
/// is answered, which must be the outpost's next message.
fn focused_name(outpost: &OutpostUnderTest) -> Option<String> {
    let (outcome, calls) = outpost.current_focus();
    common::outpost::Focused::of(outcome, calls)
        .name()
        .map(str::to_owned)
}

/// A first worker that cannot start, at the outpost's start nor at the
/// first dispatch, leaves Core's first request for the current focus
/// answered ended; the second dispatch starts it, and the second request
/// is answered with its read; the outpost shuts down.
fn a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next() {
    let title = common::unique_title("mockapp-first-worker");
    let mut app = common::spawn("small.json", "uia", &title);
    let refusal = fail_worker_start(0, 2);
    let outpost = OutpostUnderTest::new(&app);
    focus_button(&mut app, &outpost, &title);

    let (first, _) = outpost.ask_current_focus(outpost.app_window());
    assert_eq!(
        outpost.answer(first).outcome,
        Outcome::Ended,
        "no worker could be had for the first request"
    );
    outpost.settled();
    assert_eq!(
        focused_name(&outpost).as_deref(),
        Some("Original Name"),
        "the second request's dispatch started the worker, which read it"
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
    verbatim_uia::runtime_id(window.element()).expect("the window's runtime id")
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
/// The controller-for relation's step of Core's batch, through a real
/// outpost, on mockapp's search box: answered with the one element it
/// controls, and `Failed` when its calls fail, by calls alone and after
/// its program's step failed and was rerun. Core judges a failed step as
/// the whole selection read not there and says nothing
/// (`a_selected_item_whose_relation_cannot_be_read_says_nothing`), as
/// NVDA's COM error from `focus.controllerFor` ends the whole
/// `event_selection` (`docs/parity/changes-and-selection.md`, "Selection
/// in a list the focus controls"; Dickson, 2026-11-21). UI Automation
/// answers a provider's failed or mistyped `ControllerFor` as an empty
/// one, so the failure is made at the fault points.
fn a_failed_controller_for_read_is_a_failed_step() {
    use verbatim_model::batch::{Batch, Done, Op, Placement, Relation, Step, StepOutcome, Subject};
    use verbatim_model::request::{Reply, WindowVerdict};
    use verbatim_process::arguments::OutpostOptions;

    common::init_com();
    let title = common::unique_title("mockapp-controller-failed");
    let mut app = common::spawn("controller.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("set-focus search");
    let uia = Uia::new().expect("a UIA client");
    let one = |op| Batch {
        steps: vec![Step { op, when: None }],
        placement: Placement::Queue,
        spare: None,
    };
    let only = |outcome| match outcome {
        Outcome::Result(Reply::Steps(steps)) if steps.len() == 1 => {
            steps.into_iter().next().expect("one step")
        }
        other => panic!("one step's outcome, not {other:?}"),
    };
    for remote_operations in [true, false] {
        let outpost = OutpostUnderTest::with_options(&app, OutpostOptions { remote_operations });
        outpost.read_focus_as(
            common::listener_focus(&uia, hwnd.0 as isize)
                .live()
                .element(),
        );
        let focus = match only(
            outpost
                .batch(
                    hwnd,
                    WindowVerdict::Uia,
                    one(Op::FocusRead {
                        subject: Subject::Focus,
                        known: Vec::new(),
                        set: common::outpost::focus_set(),
                        details: verbatim_model::TextDetails::NONE,
                    }),
                )
                .0
                .outcome,
        ) {
            StepOutcome::Done(Done::FocusRead(read)) => read.focus.read.node.expect("a node"),
            other => panic!("the focus read: {other:?}"),
        };
        let related = || {
            only(
                outpost
                    .batch(
                        hwnd,
                        WindowVerdict::Uia,
                        one(Op::Related {
                            subject: Subject::Node(focus),
                            relation: Relation::ControllerFor,
                            set: vec![verbatim_model::property::PropertyId::Name],
                        }),
                    )
                    .0
                    .outcome,
            )
        };
        match related() {
            StepOutcome::Done(Done::Related(controlled)) => assert_eq!(
                controlled.len(),
                1,
                "remote {remote_operations}: the search box controls one element"
            ),
            other => panic!("remote {remote_operations}: the relation read, not {other:?}"),
        }
        let program = remote_operations
            .then(|| verbatim_faults::arm(Fault::ProgramStepFails, |place: &u8| *place == 0, 1));
        let calls = verbatim_faults::arm(Fault::StepCallsFail, |place: &u8| *place == 0, 1);
        assert_eq!(
            related(),
            StepOutcome::Failed,
            "remote {remote_operations}: a failed relation read"
        );
        drop(calls);
        drop(program);
        drop(outpost);
    }
    app.quit();
}

/// An activation whose `Invoke` pattern fetch times out, through a real
/// outpost, on `activation.json`'s element that has `Invoke`, `Toggle` and
/// `SelectionItem`: the step fails, and neither the next pattern nor a
/// parent is tried; mockapp sees no pattern asked for and nothing toggled
/// or selected. The next activation, with the provider answering, invokes
/// it. NVDA matches this for a fetch that fails every time: its COM error
/// from `GetCurrentPattern` ends its review activate
/// (`NVDAObjects/UIA/__init__.py` 1653 to 1656 and 2389 to 2402,
/// `globalCommands.py` 1887 to 1903). A fetch that fails once and then
/// answers is a difference pending Dickson's ruling: NVDA fetches the
/// uncached pattern again in `doAction` and invokes, where Verbatim makes
/// the one fetch (`docs/parity/review.md`, "Object activation").
fn an_activation_whose_invoke_fetch_timed_out_fails_and_tries_nothing_else() {
    use verbatim_model::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
    use verbatim_model::request::{Reply, WindowVerdict};
    use windows::Win32::UI::Accessibility::UIA_InvokePatternId;

    common::init_com();
    let title = common::unique_title("mockapp-activate-timed-out");
    let mut app = common::spawn("activation.json", "uia", &title);
    let hwnd = common::find_window(&title);
    app.send("set-focus all_three");
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::new(&app);
    outpost.read_focus_as(
        common::listener_focus(&uia, hwnd.0 as isize)
            .live()
            .element(),
    );
    let only = |outcome| match outcome {
        Outcome::Result(Reply::Steps(steps)) if steps.len() == 1 => {
            steps.into_iter().next().expect("one step")
        }
        other => panic!("one step's outcome, not {other:?}"),
    };
    let node = match only(
        outpost
            .batch(
                hwnd,
                WindowVerdict::Uia,
                Batch {
                    steps: vec![Step {
                        op: Op::FocusRead {
                            subject: Subject::Focus,
                            known: Vec::new(),
                            set: common::outpost::focus_set(),
                            details: verbatim_model::TextDetails::NONE,
                        },
                        when: None,
                    }],
                    placement: Placement::Queue,
                    spare: None,
                },
            )
            .0
            .outcome,
    ) {
        StepOutcome::Done(Done::FocusRead(read)) => read.focus.read.node.expect("a node"),
        other => panic!("the focus read: {other:?}"),
    };
    let activation_calls = || {
        common::read_hits(hwnd)
            .into_iter()
            .filter(|(method, _)| {
                matches!(
                    *method,
                    "GetPatternProvider" | "Invoke" | "Toggle" | "Select"
                )
            })
            .collect::<Vec<_>>()
    };
    let activate = || {
        only(
            outpost
                .batch(hwnd, WindowVerdict::Uia, common::outpost::activate(node))
                .0
                .outcome,
        )
    };

    let timeout = verbatim_faults::arm(
        Fault::PatternFetchTimesOut,
        |pattern: &i32| *pattern == UIA_InvokePatternId.0,
        1,
    );
    // The element's fill before its activation asks mockapp for the
    // patterns the node's cache request holds, 11 of them; each live fetch
    // that reaches mockapp asks for one more.
    common::reset_hits(hwnd);
    assert_eq!(activate(), StepOutcome::Failed, "the activation failed");
    assert_eq!(
        activation_calls(),
        vec![("GetPatternProvider", 11)],
        "no other pattern asked for, nothing toggled or selected"
    );
    drop(timeout);

    common::reset_hits(hwnd);
    assert_eq!(
        activate(),
        StepOutcome::Done(Done::Activate(Some(verbatim_model::ActionName::Invoke))),
        "answering, the element is invoked"
    );
    assert_eq!(
        activation_calls(),
        vec![("GetPatternProvider", 12), ("Invoke", 1)]
    );
    drop(outpost);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "an_activation_whose_invoke_fetch_timed_out_fails_and_tries_nothing_else",
            an_activation_whose_invoke_fetch_timed_out_fails_and_tries_nothing_else,
        ),
        (
            "a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next",
            a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next,
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
        (
            "a_failed_controller_for_read_is_a_failed_step",
            a_failed_controller_for_read_is_a_failed_step,
        ),
    ]);
}
