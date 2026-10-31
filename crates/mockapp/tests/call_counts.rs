//! The operation ledger's ratchet (`docs/performance.md`): for each operation
//! on each backend, exactly how many cross-process calls the client side
//! makes, by kind, and exactly how many calls mockapp's providers answer, by
//! method.
//!
//! An exact count is a ratchet: a change that adds a call fails here, and a
//! change that removes one must lower the number here, deliberately, in the
//! same commit as the ledger. On a mismatch the test prints every measured
//! count, so the numbers that moved are all in one run's output.
//!
//! Focus changes and object-navigation steps, on both backends, run through
//! a real outpost in this process, as `slow_application.rs` does: the test
//! hands it a focus fact or a query, as the listener and Core would, and
//! reads the calls from the event's or reply's timing. The outpost reads a
//! UIA focus's element from the system's keyboard focus, which a test must
//! not take from the desktop it runs on, so its outpost reads it from the
//! test instead ((`Outpost::with_focused_element_reader`)): the test finds
//! the element mockapp reports focused, with the listener's cache request,
//! and the outpost's read of it counts as the one call the system read is.
//! The system read's own provider calls are the only cost left out.
//! Everything after it is the outpost's own code. A UIA focus is measured
//! both ways the outpost reads it: with remote operations, as it does by
//! default, and with the classic walk, as it does with
//! `uia.remote_operations` off.
//!
//! The provider hits are read once the outpost has settled
//! (`Outpost::settle`): it has handled everything, moved its
//! focus-following subscriptions, and written every message, so they are
//! everything mockapp answered for the operation, the subscription's move
//! included, and the outpost is shown to have said nothing more than the
//! one event or reply. The calls are those the event or reply carries:
//! everything the worker made before it published.
//!
//! The text requests run the outpost's text functions on this thread with
//! a text source built as the worker builds it.
//!
//! A provider cannot tell which client called it, so the hits are kept to
//! the client under test by isolation: each test runs in a process of its
//! own on a desktop of its own (`harness::run_isolated`), where no other
//! client, a running screen reader, another test agent, or this binary's
//! other tests, sees mockapp's window or any event about it. mockapp's
//! focus moves with `set-focus`, which raises no event, so even the test's
//! own clients' event registrations are not called into while an
//! operation is measured.
//!
//! An MSAA window's verdict of no UIA provider runs out after its lifetime,
//! and the window's next event probes it again with a `WM_GETOBJECT`. The
//! outpost reads that time from the test (`OutpostUnderTest::pass_time`),
//! on the frozen observation clock (`OutpostUnderTest::on_frozen_time`),
//! so every probe here is made where the test says, whatever the test's own
//! speed, and every `WM_GETOBJECT` is pinned.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, mpsc};

use verbatim_listener_logic::focus::{FocusEvent, Judged};
use verbatim_model::protocol::{CoreToOutpost, DeliveredFact, OutpostToCore, Query};
use verbatim_model::{
    CallCounts, Fetches, FocusText, NodeSnapshot, NormalizedEvent, QueryKind, Role, WindowHandle,
};
use verbatim_model::{
    CaretWatch, LineStyle, SelectionChange, TextAttributes, TextMovement, TextOp, TextPoint,
    TextPosition, TextRead, TextReadAhead, TextReply, TextThrough, TextUnit, Theme,
};
use verbatim_outpost::arbitration::NEGATIVE_VERDICT_LIFETIME;
use verbatim_outpost::dialog_text::{UiaDialog, dialog_text};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{
    Anchors, CaretSignal, LastSelection, TextSource, Watched, caret_report, caret_report_since,
    check_caret, perform,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{
    Ancestor, BoundedClient, Cached, DialogText, FOCUS_PROPERTIES, Fill, FocusInput, ListenerEvent,
    ListenerSelection, Live, Node, NodeIdRegistry, Part, PropertyEvent, Registration, Scope, Shape,
    Subscription, TextEvent, Uia,
};
use verbatim_uia_rops::{
    Attributes, FocusAncestry, FocusQuery, TextAttribute, focus_ancestry, focus_ancestry_classic,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope, TreeScope_Children, TreeScope_Descendants,
    UIA_HasKeyboardFocusPropertyId, UIA_MenuOpenedEventId,
    UIA_SelectionItem_ElementSelectedEventId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
};
use windows::Win32::UI::Controls::{TVE_COLLAPSE, TVE_EXPAND, TVM_EXPAND};
use windows::core::AgileReference;

use common::outpost::{OutpostUnderTest, Reported};

/// The fixture's nodes, by their index in mockapp's tree (depth first, the
/// root at 0): mockapp answers `WM_GETOBJECT` for index `i` at object id
/// `i + 1`, the address an MSAA focus event names.
const FIRST: usize = 2;
const SECOND: usize = 3;
const LIST: usize = 4;
const ITEM_ONE: usize = 5;
const ITEM_TWO: usize = 6;

/// The dialog fixture's first button, by its index in mockapp's tree.
const YES: usize = 3;

/// The text of the dialog fixture's message box: its question, then the
/// question's description.
const QUESTION: &str = "Remove the theme Mine? This cannot be undone. Its sounds go with it.";

/// Calls by kind, in the order `CallCounts` lists them, none of them a
/// connection by handle.
fn calls(uia: u32, msaa: u32, window_messages: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa,
        window_messages,
        connections: 0,
    }
}

/// What one operation cost: the client's calls and the providers' hits.
struct Cost {
    calls: CallCounts,
    hits: Vec<(&'static str, u32)>,
    /// The failed cached reads (`verbatim_uia::calls`): a read of a
    /// property or pattern the element's cache request did not hold.
    failed_reads: u32,
}

/// Compares each operation's cost with the ledger's, collecting every
/// mismatch before failing.
#[derive(Default)]
struct Ratchet {
    mismatches: Vec<String>,
}

impl Ratchet {
    fn check(
        &mut self,
        operation: &str,
        cost: &Cost,
        expected_calls: CallCounts,
        expected_failed_reads: u32,
        expected_hits: &[(&str, u32)],
    ) {
        if cost.calls != expected_calls
            || cost.failed_reads != expected_failed_reads
            || cost.hits != expected_hits
        {
            self.mismatches.push(format!(
                "{operation}: measured {:?}, failed reads {}, and hits {:?}",
                cost.calls, cost.failed_reads, cost.hits
            ));
        }
    }

    /// [`check`](Self::check) for an operation whose provider is not
    /// mockapp's, so only the client's calls are compared.
    fn check_calls(&mut self, operation: &str, measured: CallCounts, expected: CallCounts) {
        if measured != expected {
            self.mismatches
                .push(format!("{operation}: measured {measured:?}"));
        }
    }

    fn finish(self) {
        assert!(
            self.mismatches.is_empty(),
            "an operation's cost moved; when that is deliberate, update this test and \
             docs/performance.md together.\n{}",
            self.mismatches.join("\n")
        );
    }
}

/// Moves mockapp's focus to `id`, hands the outpost the focus on `index`,
/// and measures it.
fn measure_msaa_focus(
    app: &mut common::MockApp,
    hwnd: HWND,
    outpost: &OutpostUnderTest,
    (id, index): (&str, usize),
) -> (Reported, Cost) {
    common::apply(app, hwnd, &format!("set-focus {id}"));
    let reported = outpost.msaa_focus(hwnd, index);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
        failed_reads: reported.failed_reads,
    };
    (reported, cost)
}

#[expect(
    clippy::too_many_lines,
    reason = "every focus change and its pinned provider hits, listed in full"
)]
fn msaa_focus_changes_cost_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-focus");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let window = Some(WindowHandle(hwnd.0 as u64));
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();

    // The fixture's root has the window role, and an MSAA window object
    // above a control is layout, left out of the chain (`msaa_ancestors`
    // in the outpost's `read.rs`).
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("first", FIRST));
    assert_eq!(reported.chain(), [Some("Settings"), Some("First")]);
    assert_eq!(reported.node.role, Role::Button);
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    ratchet.check(
        "MSAA focus, cold",
        &cost,
        calls(0, 28, 1),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 10),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 1),
        ],
    );

    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("second", SECOND));
    assert_eq!(reported.chain(), [Some("Settings"), Some("Second")]);
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    ratchet.check(
        "MSAA focus, steady state",
        &cost,
        calls(0, 20, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 12),
            ("get_accChild", 1),
            ("get_accName", 2),
            ("get_accValue", 2),
            ("get_accDescription", 2),
            ("get_accRole", 2),
            ("get_accState", 2),
            ("get_accKeyboardShortcut", 2),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );

    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("list", LIST));
    assert_eq!(reported.chain(), [Some("Options")]);
    assert_eq!(reported.node.role, Role::List);
    assert_eq!(
        reported
            .selected_child
            .and_then(|child| child.name)
            .as_deref(),
        Some("One"),
        "the list's selected item"
    );
    assert_eq!(reported.window, window);
    ratchet.check(
        "MSAA focus into a list",
        &cost,
        calls(0, 29, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 6),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accSelection", 1),
            ("accLocation", 2),
        ],
    );

    let (reported, _) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item1", ITEM_ONE));
    assert_eq!(reported.chain(), [Some("Options"), Some("One")]);
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    ratchet.check(
        "MSAA arrow to the next list item",
        &cost,
        calls(0, 20, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 9),
            ("get_accChild", 1),
            ("get_accName", 2),
            ("get_accValue", 2),
            ("get_accDescription", 2),
            ("get_accRole", 2),
            ("get_accState", 2),
            ("get_accKeyboardShortcut", 2),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );

    // The window's verdict of no UIA provider runs out once its lifetime
    // has passed by the outpost's clock, which the test moves: the next
    // focus probes the window again, and the arrow back to the second item
    // then finds the verdict renewed. The two differ by the probe alone,
    // one window message and one `WM_GETOBJECT`. Each reads one role more
    // than the arrow above, whose previous focus was reached from the list
    // rather than by an arrow.
    outpost.pass_time(NEGATIVE_VERDICT_LIFETIME);
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item1", ITEM_ONE));
    assert_eq!(reported.chain(), [Some("Options"), Some("One")]);
    ratchet.check(
        "MSAA arrow to the previous list item, the probe renewed",
        &cost,
        calls(0, 23, 1),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 9),
            ("get_accChild", 1),
            ("get_accName", 2),
            ("get_accValue", 2),
            ("get_accDescription", 2),
            ("get_accRole", 3),
            ("get_accState", 2),
            ("get_accKeyboardShortcut", 2),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    ratchet.check(
        "MSAA arrow to the next list item, the probe kept",
        &cost,
        calls(0, 23, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 9),
            ("get_accChild", 1),
            ("get_accName", 2),
            ("get_accValue", 2),
            ("get_accDescription", 2),
            ("get_accRole", 3),
            ("get_accState", 2),
            ("get_accKeyboardShortcut", 2),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );

    // The same focus event again: read in full and reported, since whether
    // it repeats the focus is Core's verdict
    // (`docs/design/ownership.md`).
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    ratchet.check(
        "MSAA focus repeated",
        &cost,
        calls(0, 23, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 9),
            ("get_accChild", 1),
            ("get_accName", 2),
            ("get_accValue", 2),
            ("get_accDescription", 2),
            ("get_accRole", 3),
            ("get_accState", 2),
            ("get_accKeyboardShortcut", 2),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );
    // Core writes the nodes it holds after the report: the outpost releases
    // the buttons it reported before, so their changes are no longer read.
    outpost.release_to_core_line();
    outpost.settled();

    // A focus event on an object that neither has the focused state nor
    // is inside one that has: NVDA reads the states, the object's and then
    // each ancestor's, and goes no further, and the outpost says nothing:
    // again only mockapp's hits are known. The outpost reads each object's
    // role too, which tells it the step that leaves a window, so that it
    // never steps into a window it reads as a plain window.
    common::reset_hits(hwnd);
    outpost.deliver(msaa_focus_fact(hwnd, FIRST));
    outpost.settled();
    ratchet.check(
        "MSAA focus without the focused state",
        &Cost {
            calls: CallCounts::default(),
            hits: common::read_hits(hwnd),
            failed_reads: outpost.take_failed_reads(),
        },
        CallCounts::default(),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 5),
            ("get_accChild", 1),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("accFocus", 1),
        ],
    );

    // A value change on an object that is not the focus is told from the
    // focus by its identity, and only its role is read, to know whether it
    // is a progress bar, whose changes are reported off the focus too: its
    // acquisition and role are counted before the focus's own value
    // change, which comes after it from the same hook and has its value
    // read alone and reported.
    common::reset_hits(hwnd);
    app.send("set-value first Changed");
    app.send("set-value item2 Picked");
    let calls_made = match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::ValueChanged { value, .. },
            timing,
            ..
        } => {
            assert_eq!(value.as_deref(), Some("Picked"));
            timing.calls
        }
        other => panic!("the outpost said {other:?}, not the focus's value change"),
    };
    outpost.settled();
    ratchet.check(
        "MSAA value changes off and on the focus",
        &Cost {
            calls: calls_made,
            hits: common::read_hits(hwnd),
            failed_reads: outpost.take_failed_reads(),
        },
        calls(0, 2, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 2),
            ("get_accChild", 2),
            ("get_accValue", 1),
            ("get_accRole", 1),
        ],
    );

    ratchet.finish();
    app.quit();
}

/// A state change on mockapp's focus while another application's window is
/// in front costs nothing at all: the outpost drops it by NVDA's acceptance
/// test before any call into mockapp, before even probing its window, whose
/// verdict of no UIA provider has run out. The same change with mockapp's
/// window in front probes the window, acquires the object, and reads the
/// focus's states alone; a name and a description change on the focus read
/// that property alone, as NVDA reads only the property its event names;
/// and a selection of the focus reads its states alone.
#[expect(
    clippy::too_many_lines,
    reason = "every change on the focus and its pinned provider hits, listed in full"
)]
fn msaa_state_change_costs_exactly() {
    use verbatim_ia2::WinEventKind;
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-state-change");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();
    let _ = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    outpost.pass_time(NEGATIVE_VERDICT_LIFETIME);

    let front = common::another_applications_window();
    outpost.set_foreground(front);
    common::reset_hits(hwnd);
    app.send("set-states item2 focusable selectable selected");
    outpost.heard(&[Heard::Msaa(WinEventKind::StateChange)]);
    outpost.settled();
    assert_eq!(common::read_hits(hwnd), [], "mockapp answered nothing");

    outpost.set_foreground(hwnd);
    // The focus's change, which must be the next thing the outpost says,
    // and what it cost.
    let mut change = |command: &str| {
        common::reset_hits(hwnd);
        app.send(command);
        let (change, calls_made) = match outpost.next() {
            OutpostToCore::Event {
                event: NormalizedEvent::PropertyChanged { change, .. },
                timing,
                ..
            } => (change, timing.calls),
            other => panic!("the outpost said {other:?}, not the focus's change"),
        };
        outpost.settled();
        let cost = Cost {
            calls: calls_made,
            hits: common::read_hits(hwnd),
            failed_reads: outpost.take_failed_reads(),
        };
        (change, cost)
    };
    let (states, cost) = change("set-states item2 focusable selectable");
    assert_eq!(
        states,
        verbatim_model::PropertyChange::States(
            [
                verbatim_model::State::Focusable,
                verbatim_model::State::Selectable
            ]
            .into_iter()
            .collect()
        )
    );
    ratchet.check(
        "MSAA state change on the focus, the probe renewed",
        &cost,
        calls(0, 2, 1),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accState", 1),
        ],
    );
    let (name, cost) = change("set-name item2 Renamed");
    assert_eq!(
        name,
        verbatim_model::PropertyChange::Name(Some("Renamed".to_owned()))
    );
    ratchet.check(
        "MSAA name change on the focus",
        &cost,
        calls(0, 2, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accName", 1),
        ],
    );
    let (description, cost) = change("set-description item2 Second choice");
    assert_eq!(
        description,
        verbatim_model::PropertyChange::Description(Some("Second choice".to_owned()))
    );
    ratchet.check(
        "MSAA description change on the focus",
        &cost,
        calls(0, 2, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accDescription", 1),
        ],
    );
    // Selecting the focus is a change of its state, as NVDA handles it.
    let (states, cost) = change("select item2");
    assert_eq!(
        states,
        verbatim_model::PropertyChange::States(
            [
                verbatim_model::State::Focusable,
                verbatim_model::State::Selectable,
                verbatim_model::State::Selected
            ]
            .into_iter()
            .collect()
        )
    );
    ratchet.check(
        "MSAA selection of the focus",
        &cost,
        calls(0, 2, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accState", 1),
        ],
    );

    ratchet.finish();
    // SAFETY: the window this thread made above.
    unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(front) }
        .expect("the test's window is destroyed");
    app.quit();
}

/// The fact the listener delivers for an MSAA focus event on mockapp's
/// node `index`.
fn msaa_focus_fact(hwnd: HWND, index: usize) -> DeliveredFact {
    DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: i32::try_from(index + 1).expect("a small index"),
        id_child: 0,
    }
}

/// The description the dialog among `ancestors` was reported with.
fn dialog_description(ancestors: &[NodeSnapshot]) -> Option<&str> {
    ancestors
        .iter()
        .find(|ancestor| ancestor.role == Role::Dialog)
        .expect("the dialog is an ancestor")
        .details
        .description
        .as_deref()
}

/// A focus entering a message box gathers its text
/// (`verbatim_outpost::dialog_text`): the dialog's children, each child's
/// role and states, and the question's name, value, and description, on top
/// of a cold focus's calls. A focus moving within it is not measured:
/// the dialog reached from the next button through `accParent` is a new
/// oleacc wrapper, with no address of its own the outpost could know it
/// by, so it is a new node to the outpost, which reads it again, as it
/// would a real windowless dialog.
fn msaa_dialog_text_costs_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-dialog");
    let mut app = common::spawn("dialog.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();

    common::apply(&mut app, hwnd, "set-focus yes");
    let reported = outpost.msaa_focus(hwnd, YES);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
        failed_reads: reported.failed_reads,
    };
    assert_eq!(reported.node.name.as_deref(), Some("Yes"));
    assert_eq!(dialog_description(&reported.ancestors), Some(QUESTION));
    ratchet.check(
        "MSAA focus into a message box",
        &cost,
        calls(0, 42, 1),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 13),
            ("accChildCount", 2),
            ("get_accChild", 4),
            ("get_accName", 4),
            ("get_accValue", 4),
            ("get_accDescription", 4),
            ("get_accRole", 6),
            ("get_accState", 6),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 1),
        ],
    );

    ratchet.finish();
    app.quit();
}

/// A focus on an item of a real tree view, comctl32's under a Windows Forms
/// class name (`tests/fixtures/tree_view.json`), read through comctl32's
/// MSAA implementation and the control's `TVM_*` messages, after a focus on
/// another root item: the item, its logical parent, and the tree view, met
/// there as the previous focus's container. The provider is comctl32's, so
/// only the client's calls are pinned: mockapp's scripted root above it is
/// not read, and the hits it counts are other clients', the UIA clients of
/// this process's other tests among them, answering the control's
/// creation events at times of their own.
/// A focus on a button in a child window of a window whose client area
/// is reached through `accParent` from the child window's window object
/// with no identity string, as a Windows Forms form's
/// (`tests/fixtures/form.json`, `form-client`): the walk acquires that
/// client area at its own address and reads it there, marked as a
/// top-level window's client, one call more than reading the object
/// `accParent` gave. A second focus in the window meets the first's chain
/// below the client area and reads it no more.
fn msaa_form_focus_costs_exactly() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-form");
    let mut app = common::spawn("form.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "child-window");
    common::apply(&mut app, hwnd, "child-client-identity");
    common::apply(&mut app, hwnd, "form-client");
    // SAFETY: a local search of the form's children.
    let child = unsafe { FindWindowExW(Some(hwnd), None, None, None) }
        .expect("mockapp made the child window");
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();

    let (reported, cost) = measure_msaa_focus(&mut app, child, &outpost, ("ok", FORM_OK));
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Form Fixture"),
            Some("Mockapp Form Fixture"),
            Some("OK")
        ]
    );
    assert_eq!(
        reported
            .ancestors
            .iter()
            .map(|node| node.details.top_level_client)
            .collect::<Vec<_>>(),
        [true, false],
        "the form's client area, read at its address, and the child window's"
    );
    ratchet.check_calls("MSAA focus in a form, cold", cost.calls, calls(0, 57, 2));
    outpost.settled();

    let (reported, cost) = measure_msaa_focus(&mut app, child, &outpost, ("cancel", FORM_CANCEL));
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Form Fixture"),
            Some("Mockapp Form Fixture"),
            Some("Cancel")
        ]
    );
    ratchet.check_calls(
        "MSAA focus in a form, steady state",
        cost.calls,
        calls(0, 25, 0),
    );
    outpost.settled();

    ratchet.finish();
    app.quit();
}

/// The form fixture's buttons, by their indexes in mockapp's tree.
const FORM_OK: usize = 1;
const FORM_CANCEL: usize = 2;

fn msaa_tree_view_costs_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-tree-view");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();

    let _ = common::tree_view::focus_item(&outpost, tree, "Settings");
    let reported = common::tree_view::focus_item(&outpost, tree, "Disks");
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Tree View Fixture"),
            None,
            Some("Hardware"),
            Some("Disks")
        ]
    );
    ratchet.check_calls(
        "MSAA focus on a tree view item",
        reported.calls,
        calls(0, 35, 12),
    );

    // The focus expands, and its children are counted with the change, for
    // Core to say how many it holds; collapsing it counts nothing. The
    // control raises its own state change event for each, which the
    // outpost hears through its hooks.
    let _ = common::tree_view::focus_item(&outpost, tree, "Software");
    let software = common::tree_view::item(tree, "Software");
    let state_change = |action: u32| {
        common::tree_view::send(tree, TVM_EXPAND, action as usize, software);
        let change = match outpost.next() {
            OutpostToCore::Event {
                event:
                    NormalizedEvent::PropertyChanged {
                        child_count,
                        change: verbatim_model::PropertyChange::States(states),
                        ..
                    },
                timing,
                ..
            } => (child_count, states, timing.calls),
            other => panic!("the outpost said {other:?}, not the focus's state change"),
        };
        outpost.settled();
        change
    };
    let (count, states, calls_made) = state_change(TVE_EXPAND.0);
    assert!(states.contains(verbatim_model::State::Expanded));
    assert_eq!(count, Some(3), "Software holds three items");
    ratchet.check_calls("MSAA tree view item expanded", calls_made, calls(0, 2, 7));
    let (count, states, calls_made) = state_change(TVE_COLLAPSE.0);
    assert!(states.contains(verbatim_model::State::Collapsed));
    assert_eq!(count, None, "nothing is counted for a collapse");
    ratchet.check_calls("MSAA tree view item collapsed", calls_made, calls(0, 2, 2));

    ratchet.finish();
    app.quit();
}

/// A focus on an item of a real list view in the report view
/// (`tests/fixtures/list_view.json`), after a focus on another item: the
/// item, named by its columns through the control's messages, as NVDA
/// names it, and the list view, met there as the previous focus's
/// container. The provider is comctl32's, so only the client's calls are
/// pinned, as for the tree view.
fn msaa_list_view_costs_exactly() {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT};
    use windows::core::w;
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-list-view");
    let app = common::spawn("list_view.json", "msaa", &title);
    let host = common::find_window(&title);
    // SAFETY: a local search of the host window's children.
    let list = unsafe {
        FindWindowExW(
            Some(host),
            None,
            w!("SysListView32"),
            windows::core::PCWSTR::null(),
        )
    }
    .expect("mockapp made the list view");
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();
    let focus = |child: i32| {
        outpost.focus(DeliveredFact::MsaaFocus {
            hwnd: list.0 as isize,
            id_object: OBJID_CLIENT.0,
            id_child: child,
        })
    };
    let _ = focus(2);
    let reported = focus(1);
    assert_eq!(
        reported.node.name.as_deref(),
        Some("readme.txt; Size: 1 KB; Type: Text Document")
    );
    ratchet.check_calls(
        "MSAA focus on a report view item",
        reported.calls,
        calls(0, 30, 14),
    );
    ratchet.finish();
    app.quit();
}

/// A UIA message box's text, gathered as the outpost's worker gathers it
/// for a dialog the focus newly entered (`describe_dialogs` in
/// `verbatim-outpost`'s `read.rs`): the dialog's element is the one the
/// node registry keeps, here kept from a property event, whose cache holds
/// no detail, and is resolved on the bounded client's thread as `Kept`;
/// one call reads the dialog's children with their properties cached,
/// scoped to the children alone as NVDA fetches them, so the dialog's own
/// element is filled with nothing and nothing of it is read, and no cached
/// read fails.
/// Each child is read with its description whatever the theme, as NVDA's
/// dialog text includes it, so the question's description is in the text
/// with descriptions off as with them on: no theme gate reaches the
/// gathering.
fn uia_dialog_text_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-dialog");
    let app = common::spawn("dialog.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let property_event = under_test
        .uia
        .request_fixed::<PropertyEvent>()
        .expect("the property event's cache request");
    let dialog = Live::new(under_test.element("Remove Theme").clone())
        .refresh(&property_event)
        .expect("the dialog's element as a property event delivers it");
    let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let node = registry.id_for_element(&dialog.runtime_id(), &dialog.narrow());
    let kept = registry
        .element_of(node)
        .expect("the dialog's kept element");
    let bounded = BoundedClient::default();
    let (text, cost) = under_test.measure(hwnd, |_| {
        bounded
            .run(common::WAIT_TIMEOUT, move |uia| {
                let dialog = kept.resolve().ok()?.live().clone();
                let children = uia.request_fixed::<DialogText>().ok()?;
                dialog_text(&UiaDialog::new(uia, &children, dialog))
            })
            .expect("the bounded client answers")
    });
    bounded.close();
    assert_eq!(text.as_deref(), Some(QUESTION));
    ratchet.check(
        "UIA message box text",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 26),
            ("GetPatternProvider", 33),
            ("GetPropertyValue", 43),
            ("HostRawElementProvider", 11),
            ("Navigate", 9),
            ("GetRuntimeId", 6),
            ("FragmentRoot", 7),
        ],
    );
    ratchet.finish();
    app.quit();
}

/// A UIA message box that is its own top-level window, its text gathered
/// as for [`uia_dialog_text_costs_exactly`]: the window's first child is the
/// title bar UIA adds for the window's frame, which the gathering reads
/// with the other children and gives no text, and whose location or
/// off-screen state would have UIA ask the window's thread
/// (`WM_GETTITLEBARINFOEX`), which Windows answers for one window at a time
/// across the desktop. Each child is read with its states but whether it is
/// off the screen, so mockapp takes no such request.
fn uia_dialog_window_text_reads_nothing_of_its_title_bar() {
    let title = common::unique_title("mockapp-counts-uia-dialog-window");
    let app = common::spawn("dialog_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let dialog = under_test
        .uia
        .window_live(hwnd.0 as isize)
        .expect("the dialog's window element");
    let bounded = BoundedClient::default();
    let dialog = AgileReference::new(dialog.element()).expect("the dialog, agile");
    let (text, cost) = under_test.measure(hwnd, |_| {
        bounded
            .run(common::WAIT_TIMEOUT, move |uia| {
                let dialog = Live::new(dialog.resolve().ok()?);
                let children = uia.request_fixed::<DialogText>().ok()?;
                dialog_text(&UiaDialog::new(uia, &children, dialog))
            })
            .expect("the bounded client answers")
    });
    bounded.close();
    assert_eq!(text.as_deref(), Some(QUESTION));
    ratchet.check(
        "UIA message box window text",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 8),
            ("ProviderOptions", 37),
            ("GetPatternProvider", 33),
            ("GetPropertyValue", 42),
            ("HostRawElementProvider", 13),
            ("Navigate", 12),
            ("GetRuntimeId", 6),
            ("FragmentRoot", 6),
        ],
    );
    ratchet.finish();
    app.quit();
}

fn msaa_navigation_steps_cost_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-navigation");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();
    let (first, _) = measure_msaa_focus(&mut app, hwnd, &outpost, ("first", FIRST));
    let first = first.node;

    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.id, QueryKind::NextSibling);
    assert_eq!(second.name.as_deref(), Some("Second"));
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    ratchet.check(
        "MSAA next sibling",
        &cost,
        calls(0, 10, 0),
        0,
        &[
            ("accParent", 1),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accDescription", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("get_accKeyboardShortcut", 1),
            ("accLocation", 1),
            ("accNavigate", 1),
        ],
    );

    common::reset_hits(hwnd);
    let (group, calls_made) = outpost.navigate(second.id, QueryKind::Parent);
    assert_eq!(group.name.as_deref(), Some("Settings"));
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    ratchet.check(
        "MSAA parent",
        &cost,
        calls(0, 10, 0),
        0,
        &[
            ("accParent", 8),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accDescription", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("get_accKeyboardShortcut", 1),
            ("accLocation", 1),
        ],
    );

    // A theme reporting descriptions and shortcuts as off: the outpost no
    // longer asks for them, and saves their calls. The reader thread takes
    // the change at once, before the step is queued.
    outpost
        .outpost
        .handle_command(&CoreToOutpost::Fetches(Fetches {
            description: false,
            shortcut: false,
            ..Fetches::default()
        }));
    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.id, QueryKind::NextSibling);
    assert_eq!(second.name.as_deref(), Some("Second"));
    assert_eq!(second.details.description, None);
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    ratchet.check(
        "MSAA next sibling, descriptions and shortcuts off",
        &cost,
        calls(0, 8, 0),
        0,
        &[
            ("accParent", 1),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("accLocation", 1),
            ("accNavigate", 1),
        ],
    );

    ratchet.finish();
    app.quit();
}

/// A UIA client on this thread over mockapp's tree, with every element the
/// tree holds, by name, for the operations to start from. The elements are
/// found by one search of the window's descendants and named by one read
/// each, which reads nothing else of them, so the window's title bar is
/// passed by: reading its location or states has UIA ask the window's
/// thread (`WM_GETTITLEBARINFOEX`), which Windows answers for one window at
/// a time across the desktop, seconds on a busy machine.
struct UiaUnderTest {
    uia: Uia,
    elements: HashMap<String, IUIAutomationElement>,
}

impl UiaUnderTest {
    fn new(hwnd: HWND) -> Self {
        let uia = Uia::new().expect("a UIA client");
        let root = uia
            .window_live(hwnd.0 as isize)
            .expect("mockapp's root element");
        // SAFETY: a live client; the condition takes no arguments.
        let condition = unsafe { uia.client().CreateTrueCondition() }.expect("a condition");
        // SAFETY: `root` and `condition` are live and this thread's.
        let found = unsafe { root.element().FindAll(TreeScope_Descendants, &condition) }
            .expect("the window's descendants");
        let mut elements = HashMap::new();
        for element in verbatim_uia::elements_of(&found) {
            // SAFETY: `element` is live and this thread's.
            let name = unsafe { element.CurrentName() }.expect("an element's name");
            if !name.is_empty() {
                elements.insert(name.to_string(), element);
            }
        }
        Self { uia, elements }
    }

    /// A UIA client on this thread over mockapp's tree that has found none
    /// of its elements, so UIA has seen none of them before an operation
    /// finds them.
    fn without_elements() -> Self {
        Self {
            uia: Uia::new().expect("a UIA client"),
            elements: HashMap::new(),
        }
    }

    fn element(&self, name: &str) -> &IUIAutomationElement {
        &self.elements[name]
    }

    /// Runs `operation` on this thread and measures it.
    fn measure<T>(&self, hwnd: HWND, operation: impl FnOnce(&Self) -> T) -> (T, Cost) {
        let _ = verbatim_uia::calls::take();
        let _ = verbatim_uia::calls::take_failed_reads();
        common::reset_hits(hwnd);
        let result = operation(self);
        let cost = Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
            failed_reads: verbatim_uia::calls::take_failed_reads(),
        };
        (result, cost)
    }

    /// The element mockapp reports as having the keyboard focus, found
    /// under its window by that property and built with the listener's
    /// cache request, as a focus event's element arrives: what the system's
    /// focused element would be if mockapp had the keyboard focus.
    fn focused(&self, hwnd: HWND) -> Cached<ListenerEvent> {
        self.focused_in(hwnd, TreeScope_Descendants)
    }

    /// [`Self::focused`], searched for among the window's children alone,
    /// so the search passes by no element below them.
    fn focused_child(&self, hwnd: HWND) -> Cached<ListenerEvent> {
        self.focused_in(hwnd, TreeScope_Children)
    }

    /// The element mockapp reports as having the keyboard focus, searched
    /// for in `scope` of its window.
    fn focused_in(&self, hwnd: HWND, scope: TreeScope) -> Cached<ListenerEvent> {
        let root = self
            .uia
            .window_live(hwnd.0 as isize)
            .expect("mockapp's root element");
        let condition = self
            .uia
            .property_condition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
            .expect("a condition");
        let request = self
            .uia
            .request_fixed::<ListenerEvent>()
            .expect("the listener's cache request");
        root.find_first(scope, &condition, &request)
            .expect("the search")
            .expect("mockapp reports a focused element")
    }
}

/// Moves mockapp's focus to `id`, finds the element mockapp then reports
/// focused, which must be the one named `name`, has the focus listener's
/// reader judge and read it and hands the outpost the read, and measures
/// both: the listener's calls and the outpost's until it reported the
/// focus, and every provider call mockapp answered for either, the
/// outpost's walk to its own copy of the element after the report
/// included; and the times each took mockapp's lock, the listener on this
/// thread and the outpost on its own (the process's takings less this
/// thread's).
fn measure_uia_focus(
    (app, under_test, hwnd): (&mut common::MockApp, &UiaUnderTest, HWND),
    outpost: &OutpostUnderTest,
    (id, name): (&str, &str),
) -> (Reported, Cost) {
    app.send(&format!("set-focus {id}"));
    let focused = under_test.focused(hwnd);
    assert_eq!(
        focused.read().name.as_deref(),
        Some(name),
        "the element mockapp reports focused"
    );
    common::reset_hits(hwnd);
    let reported = outpost.uia_focus(&focused);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
        failed_reads: reported.failed_reads,
    };
    (reported, cost)
}

/// What each UIA focus change in [`uia_focus_changes`] is expected to cost.
struct FocusCosts<'a> {
    cold: (CallCounts, u32, &'a [(&'a str, u32)]),
    steady: (CallCounts, u32, &'a [(&'a str, u32)]),
    into_list: (CallCounts, u32, &'a [(&'a str, u32)]),
    next_item: (CallCounts, u32, &'a [(&'a str, u32)]),
}

/// The same focus changes, through an outpost reading them with remote
/// operations or the classic walk as `remote` says, each checked against
/// `expected`. The first focus in the window probes it for arbitration.
fn uia_focus_changes(remote: bool, expected: &FocusCosts<'_>) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-uia-focus-{path}"));
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let window = Some(WindowHandle(hwnd.0 as u64));
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time_with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    let mut ratchet = Ratchet::default();
    let check = |ratchet: &mut Ratchet,
                 label: &str,
                 cost: &Cost,
                 expected: &(CallCounts, u32, &[(&str, u32)])| {
        ratchet.check(
            &format!("UIA focus ({path}), {label}"),
            cost,
            expected.0,
            expected.1,
            expected.2,
        );
    };

    let (reported, cost) =
        measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("first", "First"));
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First")
        ]
    );
    assert_eq!(reported.node.role, Role::Button);
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    check(&mut ratchet, "cold", &cost, &expected.cold);

    let (reported, cost) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        &outpost,
        ("second", "Second"),
    );
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("Second")
        ]
    );
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    check(&mut ratchet, "steady state", &cost, &expected.steady);

    let (reported, cost) =
        measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("list", "Options"));
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Counts Fixture"), Some("Options")]
    );
    assert_eq!(reported.node.role, Role::List);
    assert_eq!(
        reported
            .selected_child
            .and_then(|child| child.name)
            .as_deref(),
        Some("One"),
        "the list's selected item"
    );
    assert_eq!(reported.window, window);
    check(&mut ratchet, "into a list", &cost, &expected.into_list);

    let (reported, _) =
        measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("item1", "One"));
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Counts Fixture"), Some("Options"), Some("One")]
    );
    let (reported, cost) =
        measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("item2", "Two"));
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Counts Fixture"), Some("Options"), Some("Two")]
    );
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    check(
        &mut ratchet,
        "arrow to the next list item",
        &cost,
        &expected.next_item,
    );

    ratchet.finish();
    drop(outpost);
    app.quit();
}

/// A UIA focus's cost includes moving the outpost's focus-following
/// property subscription to the new focus, made on the subscription's own
/// thread once the focus is reported: one `HostRawElementProvider` and two
/// `FragmentRoot` hits, as `uia_event_registrations_cost_exactly` pins the
/// registration alone. It makes no call the worker counts.
///
/// The focus listener reads the focus, and the outpost reads nothing
/// before it reports it. Its walk to its own copy of the element comes
/// after the report, where the worker's count does not reach, and is seen
/// in the provider calls alone; that walk is a remote operation or the
/// classic walk as the test's `remote` says, which governs the outpost
/// only. The listener reads every focus the classic way, with no remote
/// operation and no connection, and without asking again for the keyboard
/// focus and the nearest window it has just read
/// (`docs/design/listener-without-connections.md`, section 3, step 8): the
/// refreshed cache and one step per ancestor, so a steady-state focus
/// costs 4 calls by both of the outpost's paths.
///
/// Since the listener tests acceptance before it reads a focus
/// (`docs/design/focus-pipeline.md`, section 16.2), each focus costs,
/// before its read, the nearest window of the windowless element
/// (`NormalizeElementBuildCache`, one call). The nearest window cost
/// mockapp, for a button or the list, 1 `WM_GETOBJECT`, 14
/// `ProviderOptions`, 6 `GetPropertyValue`, 4 `HostRawElementProvider`, 5
/// `Navigate`, and 1 `FragmentRoot`, and for the list's item 1, 10, 4, 3,
/// 3, and 1. The listener compares a focus with no earlier focus: Core
/// judges repeats (`docs/design/ownership.md`), which
/// took the comparison with the last focus accepted (`CompareElements`,
/// one call, which mockapp answers nothing for) out of every focus after
/// the first.
///
/// The listener makes no probe (`docs/design/focus-pipeline-rework.md`,
/// "Who owns each duplicated fact", the verdicts): the outpost alone
/// judges the window, and probes it at the cold focus only, before it
/// reports the listener's read, with the raw probe
/// (`docs/design/listener-without-connections.md`, section 3, step 4): one
/// window message, which costs mockapp one `WM_GETOBJECT`, two
/// `ProviderOptions`, one `HostRawElementProvider` and one `Navigate`.
fn uia_focus_changes_cost_exactly_remote() {
    uia_focus_changes(
        true,
        &FocusCosts {
            cold: (
                calls(6, 0, 1),
                0,
                &[
                    ("WM_GETOBJECT", 12),
                    ("ProviderOptions", 76),
                    ("GetPatternProvider", 64),
                    ("GetPropertyValue", 114),
                    ("HostRawElementProvider", 33),
                    ("Navigate", 22),
                    ("GetRuntimeId", 11),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 19),
                ],
            ),
            steady: (
                calls(4, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 70),
                    ("GetPatternProvider", 53),
                    ("GetPropertyValue", 94),
                    ("HostRawElementProvider", 31),
                    ("Navigate", 20),
                    ("GetRuntimeId", 12),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 19),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`.
            into_list: (
                calls(6, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 71),
                    ("GetPatternProvider", 69),
                    ("GetPropertyValue", 113),
                    ("HostRawElementProvider", 32),
                    ("Navigate", 18),
                    ("GetRuntimeId", 12),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 20),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(4, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 76),
                    ("GetPatternProvider", 53),
                    ("GetPropertyValue", 95),
                    ("HostRawElementProvider", 33),
                    ("Navigate", 22),
                    ("GetRuntimeId", 13),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 20),
                    ("IsSelected", 2),
                ],
            ),
        },
    );
}

fn uia_focus_changes_cost_exactly_classic() {
    uia_focus_changes(
        false,
        &FocusCosts {
            cold: (
                calls(6, 0, 1),
                0,
                &[
                    ("WM_GETOBJECT", 12),
                    ("ProviderOptions", 75),
                    ("GetPatternProvider", 64),
                    ("GetPropertyValue", 113),
                    ("HostRawElementProvider", 31),
                    ("Navigate", 19),
                    ("GetRuntimeId", 9),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 19),
                ],
            ),
            steady: (
                calls(4, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 70),
                    ("GetPatternProvider", 53),
                    ("GetPropertyValue", 93),
                    ("HostRawElementProvider", 30),
                    ("Navigate", 17),
                    ("GetRuntimeId", 10),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 20),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`'s
            // `FirstSelectedItem`, fetched with the `Selection` pattern in
            // one call, and its cache, one more.
            into_list: (
                calls(6, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 71),
                    ("GetPatternProvider", 69),
                    ("GetPropertyValue", 112),
                    ("HostRawElementProvider", 31),
                    ("Navigate", 15),
                    ("GetRuntimeId", 11),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 21),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(4, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 77),
                    ("GetPatternProvider", 53),
                    ("GetPropertyValue", 94),
                    ("HostRawElementProvider", 33),
                    ("Navigate", 19),
                    ("GetRuntimeId", 11),
                    ("BoundingRectangle", 3),
                    ("FragmentRoot", 22),
                    ("IsSelected", 2),
                ],
            ),
        },
    );
}

/// A focus event repeated on the element the listener last read, which
/// still has the keyboard focus, is read in full, and Core judges the
/// repeat (`docs/design/ownership.md`): the listener
/// finds the windowless sender's nearest window, for NVDA's native check
/// (one call, `NormalizeElementBuildCache`, the window's verdict already
/// kept), reads the sender's keyboard focus, then reads it as any focus,
/// asking first whether the element it read last under the same runtime id
/// still has the keyboard focus, then the sender's cache and one step up to
/// its known parent: five calls. They cost mockapp 1 `WM_GETOBJECT`, 32
/// `ProviderOptions`, 25 `GetPatternProvider`, 52 `GetPropertyValue`, 15
/// `HostRawElementProvider`, 7 `Navigate`, 5 `GetRuntimeId`, 1
/// `BoundingRectangle` and 11 `FragmentRoot`. Before, the listener dropped
/// the repeat after three calls, the nearest window, the comparison with
/// the focus Core held and that element's keyboard focus, which cost
/// mockapp 1 `WM_GETOBJECT`, 14 `ProviderOptions`, 7 `GetPropertyValue`, 5
/// `HostRawElementProvider`, 5 `Navigate` and 2 `FragmentRoot`.
fn uia_repeated_focus_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-repeated-focus");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();

    let _ = measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("first", "First"));
    let focused = under_test.focused(hwnd);
    common::reset_hits(hwnd);
    let (judged, listener_calls) = outpost.listener_read(&focused);
    let verbatim_listener_logic::focus::Judged::Read(_) = &judged else {
        panic!("the listener did not read a repeated focus: {judged:?}");
    };
    let cost = Cost {
        calls: listener_calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };
    ratchet.check(
        "UIA focus repeated on the element followed",
        &cost,
        calls(5, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 25),
            ("GetPropertyValue", 52),
            ("HostRawElementProvider", 15),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 11),
        ],
    );

    ratchet.finish();
    drop(outpost);
    app.quit();
}

/// The focus listener makes no connection to an application's window
/// (`docs/design/listener-without-connections.md`, sections 2 and 7): its
/// real reader judges and reads a cold focus in a mockapp window, its
/// window probed, then a steady-state focus, a focus into a list and one to
/// a list item, and the calls it counted on this thread include no call
/// that connects to a window by its handle (`CallCounts::connections`).
/// The count, not the absence of a log line, is the evidence. With the
/// window's element looked up by its handle inside the read, as the
/// listener's `mark_window` once did, the count is 1 and the test fails.
fn the_listener_makes_no_connection() {
    let title = common::unique_title("mockapp-counts-listener-no-connection");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    for (id, name) in [
        ("first", "First"),
        ("second", "Second"),
        ("list", "Options"),
        ("item1", "One"),
    ] {
        app.send(&format!("set-focus {id}"));
        let focused = under_test.focused(hwnd);
        assert_eq!(
            focused.read().name.as_deref(),
            Some(name),
            "the element mockapp reports focused"
        );
        let (judged, listener_calls) = outpost.listener_read(&focused);
        let verbatim_listener_logic::focus::Judged::Read(_) = &judged else {
            panic!("the listener did not read the focus on {name}: {judged:?}");
        };
        assert_eq!(
            listener_calls.connections, 0,
            "the listener's connections reading the focus on {name}"
        );
        let _ = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
    }
    drop(outpost);
    app.quit();
}

/// The listener's walks stop below the process boundary
/// (`docs/design/listener-without-connections.md`, sections 3 and 7): one
/// mockapp's UIA window is made the child of another mockapp's MSAA
/// window, by `SetParent` from this process, as the Settings page's content
/// is the child of `ApplicationFrameHost`'s frame; or, staying top-level,
/// is given that window, shown, as its owner (`GWLP_HWNDPARENT`, set from
/// this process, which `GetWindow` with `GW_OWNER` then answers), as a
/// dialog is owned by another process's window, under which UIA's raw view
/// puts it. The listener's read of a focus in the child, the classic walk
/// whichever way the outpost reads, ends its chain at the child's window
/// and takes no step into the other process. The outpost then completes
/// the chain above it with the parent or owner as a plain window, named by
/// its text, with no call into the other process
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"). With the walk's boundary check removed, or the owner left
/// out of it, the walk's step up into the other window shows in that
/// window's calls, and without the owner the outpost's chain ends at the
/// child's window. A focus-now query's read (`uia_remote_enrichment` by the
/// remote program, the outpost's own walk classically) gives the same
/// chain; without the remote read's completion from its outermost window,
/// it ends at the child's window. The outpost probes, reads and walks no
/// window of the other process; the parent's calls during the query are
/// pinned. With the outpost's walk (`uia_ancestors`) left to step across
/// the boundary, the classic focus-now query's parent calls show the
/// step.
///
/// The parent still receives 2 `WM_GETOBJECT`s while the listener reads,
/// each with `UiaRootObjectId` and a nonzero wParam, the message a UIA
/// connection begins with (logged from mockapp's window procedure on
/// 2026-10-19): one when the nearest-window lookup
/// (`NormalizeElementBuildCache`) returns the child window's element, and
/// one when the read returns it as an ancestor. UIA's client side sends
/// them itself whenever a call on a held element hands it an element of a
/// window whose parent is another process's; no walk step is made. An
/// owner receives none. The outpost's reads of the focus return the child
/// window's element too, so the parent receives the same message from
/// them, which the parent's `WM_GETOBJECT`s, split by kind, show
/// (decision 13).
#[expect(
    clippy::too_many_lines,
    reason = "each read's calls and the parent's, pinned in full for both placements and paths"
)]
fn uia_focus_chains_stop_below_the_process_boundary() {
    for (owned, remote) in [(false, true), (false, false), (true, true), (true, false)] {
        let path = match (owned, remote) {
            (false, true) => "child, remote",
            (false, false) => "child, classic",
            (true, true) => "owned, remote",
            (true, false) => "owned, classic",
        };
        let label = path.replace(", ", "-");
        let (parent_app, parent) = spawn_parent(&label, owned);
        let title = common::unique_title(&format!("mockapp-boundary-child-{label}"));
        let mut app = common::spawn("counts.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let under_test = UiaUnderTest::new(hwnd);
        // Made while the child is still a top-level window, which it finds
        // by its title, and reads as the foreground window.
        let mut outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        place_below(hwnd, parent, owned);
        app.send("set-focus first");
        let focused = under_test.focused(hwnd);

        common::reset_hits(parent);
        let (judged, listener_calls) = outpost.listener_read(&focused);
        // As a cold focus in a top-level window: the read takes two steps,
        // to the group and to the child's window, and none to the
        // desktop's root, where a top-level window's walk takes three.
        assert_eq!(
            listener_calls,
            calls(5, 0, 0),
            "the listener's calls ({path})"
        );
        // UIA's client side sends a parent window 2 `WM_GETOBJECT`s as the
        // child window's element is handed back, and an owner none.
        let expected_hits: &[(&str, u32)] = if owned { &[] } else { &[("WM_GETOBJECT", 2)] };
        assert_eq!(
            common::read_hits(parent),
            expected_hits,
            "the parent's calls while the listener read ({path})"
        );
        let verbatim_listener_logic::focus::Judged::Read(read) = &judged else {
            panic!("the listener did not read the focus ({path}): {judged:?}");
        };
        let ancestors: Vec<(Option<&str>, isize)> = read
            .ancestors
            .iter()
            .map(|ancestor| (ancestor.snapshot.name.as_deref(), ancestor.hwnd))
            .collect();
        assert_eq!(
            ancestors,
            [
                (Some("Settings"), 0),
                (Some("Mockapp Counts Fixture"), hwnd.0 as isize)
            ],
            "the listener's chain ({path})"
        );

        common::reset_hits(parent);
        let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
        // The parent or owner as a plain window, with no call into the
        // other process. UIA's client side still sends a parent its
        // connection message as the outpost's read of the focus returns
        // the child window's element, once by the remote program and twice
        // classically, as it does for the listener (decision 13); an owner
        // receives none.
        let connection_messages = match (owned, remote) {
            (true, _) => 0,
            (false, true) => 1,
            (false, false) => 2,
        };
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(connection_messages, 0, &[]),
            "the parent's WM_GETOBJECTs while the outpost read ({path})"
        );
        // And the outpost's own probe of the focus's window, its own
        // application's, which it judges with its own verdict.
        assert_eq!(
            reported.calls,
            calls(0, 0, 1),
            "the outpost's calls ({path})"
        );
        let parent_title = parent_app.title.clone();
        let expected_chain = [
            Some(parent_title.as_str()),
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First"),
        ];
        assert_eq!(
            reported.chain(),
            expected_chain,
            "the outpost's chain ({path})"
        );

        // A focus-now query's own read gives the same chain, the parent or
        // owner a plain window: the remote program and the classic read
        // (`uia_ancestors`) both stop below the boundary, and neither calls
        // into the other process.
        outpost.read_focus_as(focused.live().element());
        common::reset_hits(parent);
        let now = outpost
            .focus_now()
            .unwrap_or_else(|| panic!("the focus now has a focused control ({path})"));
        assert_eq!(
            common::read_hits(parent),
            FOCUS_NOW_PARENT_HITS[usize::from(owned)],
            "the parent's calls during the focus-now query ({path})"
        );
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(u32::from(!owned), 0, &[]),
            "the parent's WM_GETOBJECTs during the focus-now query ({path})"
        );
        assert_eq!(
            now.ancestors
                .iter()
                .chain(std::iter::once(&now.node))
                .map(|node| node.name.as_deref())
                .collect::<Vec<_>>(),
            expected_chain,
            "the focus now's chain ({path})"
        );
        drop(outpost);
        app.quit();
        parent_app.quit();
    }
}

/// The classic walk from a window's own element held without its handle,
/// runtime id `[42, window]` with no window handle in its cache, whose
/// window is the child of another process's window, stops before any
/// step, as the focus listener reads it, given the nearest window its
/// checks found: the element's own window, recognized by its runtime id
/// (`docs/design/listener-without-connections.md`, section 10, step 6).
/// The walk makes no call and the parent receives nothing. Without that
/// recognition the walk steps into the parent, and the check after the
/// step drops it, so the chain is the same but the step shows: a call,
/// and the parent's messages. The listener's own nearest-window lookup
/// passes over an element that reports no handle, so its reader drops
/// such a sender at its native check before the walk; the walk is called
/// here with the nearest window given.
fn uia_walk_from_a_window_element_held_without_its_handle_stops_below_the_boundary() {
    let (parent_app, parent) = spawn_parent("window-element", false);
    let title = common::unique_title("mockapp-boundary-window-element");
    let app = common::spawn_hiding_window_handle("counts.json", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    place_below(hwnd, parent, false);
    let request = under_test
        .uia
        .request_fixed::<FocusInput>()
        .expect("the focus input's cache request");
    let window = under_test
        .uia
        .window_element(hwnd.0 as isize, &request)
        .expect("mockapp's window element");
    let input = window.read();
    assert_eq!(
        (input.hwnd, window.runtime_id()),
        (0, vec![42, hwnd.0 as i32]),
        "the window's own element, held without its handle"
    );
    let ancestors = under_test
        .uia
        .request::<Ancestor>(common::ancestor_gates())
        .expect("the ancestors' cache request");
    let fill = Fill::new(&ancestors);
    let element_fill = Fill::new(
        &under_test
            .uia
            .request::<Ancestor>(Ancestor::all_on())
            .expect("the focus's cache request"),
    );
    common::reset_hits(parent);
    let _ = verbatim_uia::calls::take();
    let ancestry = focus_ancestry_classic(
        &under_test.uia,
        &FocusQuery {
            element: &window,
            known: &[],
            previous: None,
            depth_limit: 64,
            fill: &fill,
            element_fill: &element_fill,
            out_of_time: None,
            require_focus: false,
            refresh: false,
            nearest_window: Some(hwnd.0 as isize),
            before_call: None,
            value_available: None,
            plain: None,
            progress: None,
        },
    )
    .expect("the walk");
    let walk_calls = verbatim_uia::calls::take();
    let FocusAncestry::Focused(ancestry) = ancestry else {
        panic!("the walk required no focus");
    };
    assert_eq!(
        (ancestry.ancestors.len(), ancestry.window),
        (0, Some(hwnd.0 as isize)),
        "the walk's ancestors and window"
    );
    assert_eq!(walk_calls, calls(0, 0, 0), "the walk's calls");
    assert_eq!(
        (common::read_hits(parent), common::read_get_objects(parent)),
        (vec![], vec![]),
        "the parent's calls and messages during the walk"
    );
    app.quit();
    parent_app.quit();
}

/// The parent's or owner's provider calls during the boundary test's
/// focus-now query, for the child and for the owned window, by either
/// read: none of the outpost's, the parent or owner being a plain window,
/// the classic read stopping below the boundary as the remote one does. A
/// parent receives only the `WM_GETOBJECT` with `UiaRootObjectId` and a
/// nonzero `wParam` that UIA's client side sends as the read returns the
/// child window's element (decision 13 of
/// `docs/design/listener-without-connections.md`). Before the outposts
/// stopped below the boundary, the classic read's step up into the parent
/// cost it 6 `WM_GETOBJECT`s and 27 MSAA calls (5 and 27 for an owner);
/// before the parent was a plain window, the MSAA read of its client area
/// cost it one `WM_GETOBJECT` and 10 MSAA calls more.
const FOCUS_NOW_PARENT_HITS: [&[(&str, u32)]; 2] = [&[("WM_GETOBJECT", 1)], &[]];

/// Above a parent window of another process that has a UIA provider, as a
/// host of web content can have, the outpost names the parent as a plain
/// window, by its text, and makes no call into it: no probe, no lookup by
/// its handle, no UIA step into it and no MSAA read of it
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"). UIA's client side still connects to the parent as the
/// outpost's read returns the child window's element (decision 13 of
/// `docs/design/listener-without-connections.md`), which the parent's
/// `WM_GETOBJECT`s, split by kind, show, and nothing else reaches it. Both
/// of the outpost's reads, the listener's chain completed and a focus-now
/// query, by the remote program and classically. With the parent read
/// through MSAA, as before the plain window, its standard proxy's
/// `WM_GETOBJECT`s show.
fn uia_focus_chains_name_a_parent_with_a_provider_as_a_plain_window() {
    for remote in [true, false] {
        let path = if remote { "remote" } else { "classic" };
        let parent_title = common::unique_title(&format!("mockapp-boundary-uia-parent-{path}"));
        let parent_app = common::spawn("small.json", "uia", &parent_title);
        let parent = common::find_window(&parent_title);
        let title = common::unique_title(&format!("mockapp-boundary-uia-child-{path}"));
        let mut app = common::spawn("counts.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let under_test = UiaUnderTest::new(hwnd);
        let mut outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        place_below(hwnd, parent, false);
        app.send("set-focus first");
        let focused = under_test.focused(hwnd);
        let (judged, _) = outpost.listener_read(&focused);
        common::reset_hits(parent);
        let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
        // The parent a plain window, named by its title, with no call.
        let expected_chain = [
            Some(parent_title.as_str()),
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First"),
        ];
        assert_eq!(
            reported.calls,
            calls(0, 0, 1),
            "the outpost's calls ({path})"
        );
        assert_eq!(
            reported.chain(),
            expected_chain,
            "the outpost's chain ({path})"
        );
        // As the outpost's read returns the child window's element, UIA's
        // client side connects to the parent's provider, once by the
        // remote program and twice classically, as it does for the
        // listener (decision 13), and nothing else reaches the parent.
        let connections = if remote { 1 } else { 2 };
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(connections, connections, &[]),
            "the parent's WM_GETOBJECTs while the outpost read ({path})"
        );
        assert_eq!(
            common::read_hits(parent),
            [
                ("WM_GETOBJECT", 2 * connections),
                ("ProviderOptions", 6 * connections),
                ("HostRawElementProvider", 2 * connections),
                ("Navigate", 2 * connections)
            ],
            "the parent's calls while the outpost read ({path})"
        );
        outpost.read_focus_as(focused.live().element());
        common::reset_hits(parent);
        let now = outpost
            .focus_now()
            .unwrap_or_else(|| panic!("the focus now has a focused control ({path})"));
        assert_eq!(
            now.ancestors
                .iter()
                .chain(std::iter::once(&now.node))
                .map(|node| node.name.as_deref())
                .collect::<Vec<_>>(),
            expected_chain,
            "the focus now's chain ({path})"
        );
        // The query's read connects to the parent's provider once, as the
        // child window's element is returned, by either path, and nothing
        // else reaches the parent.
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(1, 1, &[]),
            "the parent's WM_GETOBJECTs during the focus-now query ({path})"
        );
        assert_eq!(
            common::read_hits(parent),
            [
                ("WM_GETOBJECT", 2),
                ("ProviderOptions", 6),
                ("HostRawElementProvider", 2),
                ("Navigate", 2)
            ],
            "the parent's calls during the focus-now query ({path})"
        );
        drop(outpost);
        app.quit();
        parent_app.quit();
    }
}

/// `read_get_objects`'s answer for `nonzero` `UiaRootObjectId` messages
/// with a nonzero `wParam`, the message a UIA connection begins with,
/// `zero` with a zero one, and then `others`, in counter order.
fn get_objects(
    nonzero: u32,
    zero: u32,
    others: &[(&'static str, u32)],
) -> Vec<(&'static str, u32)> {
    [
        ("UiaRootObjectId, nonzero wParam", nonzero),
        ("UiaRootObjectId, zero wParam", zero),
    ]
    .into_iter()
    .filter(|&(_, count)| count != 0)
    .chain(others.iter().copied())
    .collect()
}

/// An outpost's first look at a window of its own application that has a
/// UIA provider, here as it takes a foreground event for it: the raw probe
/// alone, one `WM_GETOBJECT` with a zero `wParam`, and no connection to the
/// window after it finds the provider
/// (`docs/design/listener-without-connections.md`, decision 2). The probe
/// costs mockapp one `WM_GETOBJECT`, two `ProviderOptions`, one
/// `HostRawElementProvider` and one `Navigate`. Its verdict is kept for the
/// window's lifetime, so a second foreground event costs nothing.
fn an_outposts_probe_that_finds_a_provider_makes_no_connection() {
    let title = common::unique_title("mockapp-counts-probe-alone");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();
    for (label, expected_get_objects, expected_hits) in [
        (
            "first",
            get_objects(0, 1, &[]),
            vec![
                ("WM_GETOBJECT", 1),
                ("ProviderOptions", 2),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
            ],
        ),
        ("second", get_objects(0, 0, &[]), vec![]),
    ] {
        common::reset_hits(hwnd);
        outpost.deliver(DeliveredFact::Foreground {
            hwnd: hwnd.0 as isize,
        });
        outpost.settled();
        let get_objects = common::read_get_objects(hwnd);
        let hits = common::read_hits(hwnd);
        if get_objects != expected_get_objects || hits != expected_hits {
            ratchet.mismatches.push(format!(
                "the outpost's {label} look at its UIA window: WM_GETOBJECTs {get_objects:?}, hits {hits:?}"
            ));
        }
    }
    ratchet.finish();
    drop(outpost);
    app.quit();
}

/// A window above the process boundary is read with no call into its
/// process, as NVDA's MSAA client object for it
/// (`docs/design/focus-pipeline-rework.md`, "The foreground wait"; the
/// decision of 2026-10-29 in "Calls that reach another application"; the
/// setup of `experiments/the-msaa-continuation-waits-on-the-parent-windows-application/`):
/// a mockapp UIA window made the child of a child window of another
/// mockapp's window, whose thread is then stalled for four seconds, has the
/// listener's read of its focus, made before the stall, reported while the
/// parent is still stalled, and the parent's process counts no provider
/// call at all, only the connection messages UIA's client side sends it as
/// the outpost's reads return the child window's element (decision 13 of
/// `docs/design/listener-without-connections.md`). Each window above the
/// boundary is a pane carrying its handle: the top-level window, which has
/// a system menu, marked a top-level window's client and named by its
/// text, and the child window between, which has none, not marked. Read by
/// a second outpost that judges the top-level window hung, that window is
/// a plain window, role window, as NVDA's object for a hung window is, and
/// the child window still a pane. With the parent read through MSAA, as
/// before, the report waits for the stall's end and the parent counts its
/// client area's calls.
fn a_window_above_the_process_boundary_is_a_plain_window_with_no_call() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let stall = std::time::Duration::from_secs(4);
    let (mut parent_app, parent) = spawn_parent("stalled", false);
    let title = common::unique_title("mockapp-boundary-stalled-child");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    parent_app.send("child-window");
    // SAFETY: a local search of the parent window's children.
    let between = unsafe {
        FindWindowExW(
            Some(parent),
            None,
            windows::core::w!("VerbatimMockAppWindow"),
            windows::core::PCWSTR::null(),
        )
    }
    .expect("mockapp made the child window");
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    // The second outpost finds mockapp's window as a top-level window.
    let judging_hung = OutpostUnderTest::new(&app);
    place_below(hwnd, between, false);
    app.send("set-focus first");
    let focused = under_test.focused(hwnd);
    let (judged, _) = outpost.listener_read(&focused);
    common::reset_hits(parent);
    parent_app.stall(stall);
    let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
    let reported_at = common::now_us();
    let ended = parent_app.stall_ended(stall);
    assert!(
        reported_at < ended,
        "the focus was reported at {reported_at}, before the parent's stall ended at {ended}"
    );
    assert_eq!(
        reported.chain(),
        [
            Some(parent_app.title.as_str()),
            None,
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First")
        ],
        "the outpost's chain"
    );
    let above_boundary = |reported: &common::outpost::Reported| {
        reported.ancestors[..2]
            .iter()
            .map(|node| {
                (
                    node.role,
                    node.details.top_level_client,
                    node.details.window,
                )
            })
            .collect::<Vec<_>>()
    };
    let window = |hwnd: HWND| {
        Some(WindowHandle(u64::from_ne_bytes(
            (hwnd.0 as isize).to_ne_bytes(),
        )))
    };
    assert_eq!(
        above_boundary(&reported),
        [
            (Role::Pane, true, window(parent)),
            (Role::Pane, false, window(between))
        ],
        "the windows above the boundary, as NVDA's MSAA clients for them"
    );
    outpost.settled();
    assert_eq!(
        (common::read_hits(parent), common::read_get_objects(parent)),
        (vec![], vec![]),
        "the parent's calls and messages"
    );
    drop(outpost);
    let outpost = judging_hung;
    outpost.judge_hung(parent);
    let (judged, _) = outpost.listener_read(&focused);
    let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
    assert_eq!(
        above_boundary(&reported),
        [
            (Role::Window, false, window(parent)),
            (Role::Pane, false, window(between))
        ],
        "the hung top-level window, a plain window"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
    parent_app.quit();
}

/// NVDA's focused-state test of an MSAA focus walks up the object's
/// parents (`shouldAllowIAccessibleFocusEvent`); above the process
/// boundary it goes on by plain windows, with no call into the other
/// process (`docs/design/cross-process-calls-study.md`, instance 6): a
/// mockapp MSAA dialog (`save_as.json`), whose client area answers its
/// window object as its parent (`client-identity`), made the child of
/// another mockapp's window whose thread is then stalled, has a focus on a
/// button that nothing in the window has the focused state for judged, and
/// dropped, before the stall ends, and the parent's process counts no call
/// and no message.
/// With the walk stepping on from the child window's own window object, it
/// asks the parent for its client area and waits for the stall's end.
fn the_focused_state_walk_makes_no_call_above_the_process_boundary() {
    let stall = std::time::Duration::from_secs(4);
    let (mut parent_app, parent) = spawn_parent("focused-state", false);
    let title = common::unique_title("mockapp-boundary-focused-state-child");
    let mut app = common::spawn("save_as.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    app.send("client-identity");
    app.send("set-states save focusable");
    let outpost = OutpostUnderTest::new(&app);
    place_below(hwnd, parent, false);
    common::reset_hits(parent);
    parent_app.stall(stall);
    // The button, index 1, without the focused state.
    outpost.deliver(msaa_focus_fact(hwnd, 1));
    outpost.settled();
    let judged_at = common::now_us();
    let ended = parent_app.stall_ended(stall);
    assert!(
        judged_at < ended,
        "the focus was judged at {judged_at}, before the parent's stall ended at {ended}"
    );
    assert_eq!(
        (common::read_hits(parent), common::read_get_objects(parent)),
        (vec![], vec![]),
        "the parent's calls and messages"
    );
    drop(outpost);
    app.quit();
    parent_app.quit();
}

/// A walk's step to a parent is a call whose target the system picks as it
/// is made, so a window reparented under another process's window between
/// the walk's boundary check and its step has the step go into that
/// process; the parent it returns is not read further, and the chain is
/// completed by plain windows (`docs/design/focus-pipeline-rework.md`, "A
/// window's name, and the one fallback"; the setup of
/// `experiments/a-walk-up-whose-window-changes-after-its-check/`): the
/// listener's walk up from `First` in one mockapp's top-level window, which
/// is made another mockapp's window's child just before its third step,
/// the one from the window's own element, keeps the chain below the step
/// (`stepped_across`), and the outpost completes it with the other window
/// as a plain window and makes no call in its process: only the step's own
/// connection reaches it. Without the check after the step, the other
/// window's element is in the listener's chain.
fn a_parent_step_into_another_process_is_not_read_further() {
    use windows::Win32::UI::WindowsAndMessaging::SetParent;
    let (parent_app, parent) = spawn_parent("walk-race", false);
    let title = common::unique_title("mockapp-boundary-walk-race");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    app.send("set-focus first");
    let focused = under_test.focused(hwnd);
    let steps = Arc::new(AtomicU64::new(0));
    let (child, other) = (hwnd.0 as isize, parent.0 as isize);
    outpost.listener_observes_calls(Arc::new(move |call| {
        if call != "a step to the parent"
            || steps.fetch_add(1, std::sync::atomic::Ordering::Relaxed) != 2
        {
            return;
        }
        // SAFETY: two live windows of other processes; the call only
        // changes the first one's parent.
        unsafe { SetParent(HWND(child as *mut _), Some(HWND(other as *mut _))) }
            .expect("the child window takes the other process's window as its parent");
        common::reset_hits(HWND(other as *mut _));
    }));
    let (judged, _) = outpost.listener_read(&focused);
    let Judged::Read(read) = &judged else {
        panic!("the listener did not read the focus: {judged:?}");
    };
    assert_eq!(
        read.ancestors
            .iter()
            .map(|ancestor| ancestor.snapshot.name.as_deref())
            .collect::<Vec<_>>(),
        [Some("Settings"), Some("Mockapp Counts Fixture")],
        "the listener's chain, the step's parent not kept"
    );
    assert_eq!(
        common::read_get_objects(parent),
        get_objects(1, 1, &[("OBJID_CLIENT", 2)]),
        "the step's connection to the other window, and its MSAA proxy's"
    );
    common::reset_hits(parent);
    let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus, None));
    assert_eq!(
        reported.chain(),
        [
            Some(parent_app.title.as_str()),
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First")
        ],
        "the outpost's chain"
    );
    outpost.settled();
    assert_eq!(
        (common::read_hits(parent), common::read_get_objects(parent)),
        (vec![("WM_GETOBJECT", 1)], get_objects(1, 0, &[])),
        "the parent's calls and messages after the listener's read"
    );
    drop(outpost);
    app.quit();
    parent_app.quit();
}

/// `accFocus` names whatever object it names when it is called, so the
/// MSAA focus-now read checks the window of the object it names as served
/// by the outpost's application before reading it, and answers no focused
/// control for another process's (`docs/design/cross-process-calls-study.md`,
/// instance 8): a mockapp MSAA window whose own `accFocus` answers the
/// client area of a second mockapp's window (`acc-focus-window`), acquired
/// by the first mockapp, whose wrapper `WindowFromAccessibleObject` finds
/// the second window from. The second window counts only that
/// acquisition's message, and none of a read. Without the check, the
/// second window's client area is read and answered as the focus.
fn an_acc_focus_served_by_another_process_answers_no_focused_control() {
    let other_title = common::unique_title("mockapp-acc-focus-other");
    let other_app = common::spawn("small.json", "msaa", &other_title);
    let other = common::find_window(&other_title);
    let title = common::unique_title("mockapp-acc-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    app.send(&format!("acc-focus-window {}", other.0 as isize));
    let mut outpost = OutpostUnderTest::new(&app);
    common::reset_hits(other);
    let (answer, _) = outpost.focus_now_answer();
    assert_eq!(answer.focus, None, "the focus now's focused control");
    assert_eq!(
        (common::read_hits(other), common::read_get_objects(other)),
        (vec![("WM_GETOBJECT", 1)], vec![("OBJID_CLIENT", 1)]),
        "the other window's calls and messages"
    );
    drop(outpost);
    app.quit();
    other_app.quit();
}

/// The focus-now answer onto a hung window is the plain window, with no
/// call into the application: the window speaks by its text, a deliberate
/// difference (Dickson, 2026-10-29), where NVDA says nothing
/// (`experiments/focus-pipeline-rework-measurements/readme.md`,
/// "Focus-now onto a hung window"). mockapp's focus window, judged hung, is
/// answered as the focus, a plain window named by its title, with nothing
/// above it, and mockapp counts no call or message. Judged readable, the
/// answer reads the window's client area and its focus. The answer's
/// foreground window is the system's, which mockapp's hidden window never
/// is, so it is not asserted.
fn focus_now_onto_a_hung_window_answers_its_plain_window() {
    let title = common::unique_title("mockapp-focus-now-hung");
    let app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::new(&app);
    outpost.judge_hung(hwnd);
    common::reset_hits(hwnd);
    let (answer, outpost_calls) = outpost.focus_now_answer();
    let focus = answer.focus.as_ref().map(|focus| {
        (
            focus.node.role,
            focus.node.name.as_deref(),
            focus.ancestors.len(),
        )
    });
    assert_eq!(
        focus,
        Some((Role::Window, Some(title.as_str()), 0)),
        "the focus now's focus"
    );
    assert_eq!(outpost_calls, calls(0, 0, 0), "the outpost's calls");
    assert_eq!(
        (common::read_hits(hwnd), common::read_get_objects(hwnd)),
        (vec![], vec![]),
        "mockapp's calls and messages"
    );
    drop(outpost);
    app.quit();
}

/// A mockapp MSAA window for another mockapp's window to be placed below
/// ([`place_below`]), and its handle; shown without being activated when
/// it is to be an owner, since UIA's raw view puts an owned window under
/// its owner only while the owner is visible.
fn spawn_parent(label: &str, owner: bool) -> (common::MockApp, HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
    let title = common::unique_title(&format!("mockapp-boundary-parent-{label}"));
    let app = common::spawn("small.json", "msaa", &title);
    let parent = common::find_window(&title);
    if owner {
        // SAFETY: a live window of another process; the answer is only
        // whether it was visible before.
        let _ = unsafe { ShowWindow(parent, SW_SHOWNOACTIVATE) };
    }
    (app, parent)
}

/// Puts mockapp's top-level window `hwnd` below `parent`, another
/// process's window, from this process: as its child (`SetParent`), or,
/// when `owned`, still top-level with `parent` as its owner
/// (`GWLP_HWNDPARENT`), which `GetWindow` with `GW_OWNER` then answers.
fn place_below(hwnd: HWND, parent: HWND, owned: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GWLP_HWNDPARENT, GetWindow, SetParent, SetWindowLongPtrW,
    };
    if owned {
        // SAFETY: two live windows of other processes; the call changes
        // only the first one's owner.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, parent.0 as isize) };
        // SAFETY: a live window; the call only reads its owner.
        let answered = unsafe { GetWindow(hwnd, GW_OWNER) };
        assert_eq!(
            answered.ok(),
            Some(parent),
            "the child takes the other process's window as its owner"
        );
    } else {
        // SAFETY: two live windows of other processes; the call only
        // changes the first one's parent.
        unsafe { SetParent(hwnd, Some(parent)) }
            .expect("the child window takes the other process's window as its parent");
    }
}

/// One object-navigation step's cost through an outpost reading UIA as
/// `remote` says: the next sibling of `First`, then the parent of
/// `Second`, from the nodes a focus on `First` and the first step reported,
/// with the default theme's fetches, as Core sends them.
fn uia_navigation_steps(remote: bool) -> [(NodeSnapshot, Cost); 2] {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-uia-navigation-{path}"));
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut outpost = OutpostUnderTest::on_frozen_time_with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost
        .outpost
        .handle_command(&CoreToOutpost::Fetches(Theme::builtin_default().fetches()));
    let (first, _) = measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("first", "First"));

    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.node.id, QueryKind::NextSibling);
    let next_sibling = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    common::reset_hits(hwnd);
    let (group, calls_made) = outpost.navigate(second.id, QueryKind::Parent);
    let parent = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    drop(outpost);
    app.quit();
    [(second, next_sibling), (group, parent)]
}

fn uia_navigation_steps_cost_exactly() {
    let mut ratchet = Ratchet::default();

    let [(second, cost), (group, parent_cost)] = uia_navigation_steps(true);
    assert_eq!(second.name.as_deref(), Some("Second"));
    assert_eq!(second.role, Role::Button);
    // One program: the step and the nearest window, the neighbor's cache
    // filled inside the provider; the kept element is not refreshed first.
    ratchet.check(
        "UIA next sibling, remotely",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 26),
            ("GetPatternProvider", 14),
            ("GetPropertyValue", 25),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );
    assert_eq!(group.name.as_deref(), Some("Settings"));
    assert_eq!(group.role, Role::Group);
    ratchet.check(
        "UIA parent, remotely",
        &parent_cost,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 14),
            ("GetPropertyValue", 25),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );

    let [(second, cost), (group, parent_cost)] = uia_navigation_steps(false);
    assert_eq!(second.name.as_deref(), Some("Second"));
    assert_eq!(second.role, Role::Button);
    ratchet.check(
        "UIA next sibling, classically",
        &cost,
        calls(3, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 31),
            ("GetPatternProvider", 28),
            ("GetPropertyValue", 45),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );
    assert_eq!(group.name.as_deref(), Some("Settings"));
    assert_eq!(group.role, Role::Group);
    ratchet.check(
        "UIA parent, classically",
        &parent_cost,
        calls(3, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 28),
            ("GetPropertyValue", 45),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );

    ratchet.finish();
}

/// A focus's ancestors as the outpost reads them itself, for Core's
/// ancestors query, with a theme that reports descriptions as off: the
/// focus's element filled as a node and each ancestor as an ancestor, so
/// the provider is not asked for the focus's description, nor for an
/// ancestor's beyond the one the layout judgment reads, and no cached read
/// fails (`docs/design/uia-shapes.md`, section 4.2).
fn uia_ancestors_with_descriptions_off_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-ancestors");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut outpost = OutpostUnderTest::on_frozen_time(&app);
    outpost
        .outpost
        .handle_command(&CoreToOutpost::Fetches(Fetches {
            description: false,
            ..Fetches::default()
        }));
    let mut ratchet = Ratchet::default();
    let (first, _) = measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("first", "First"));

    common::reset_hits(hwnd);
    let (ancestors, calls_made) = outpost.ancestors(first.node.id);
    let names: Vec<Option<&str>> = ancestors
        .iter()
        .map(|ancestor| ancestor.name.as_deref())
        .collect();
    assert_eq!(names, [Some("Mockapp Counts Fixture"), Some("Settings")]);
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    ratchet.check(
        "UIA ancestors of a focus, descriptions off",
        &cost,
        calls(5, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 28),
            ("GetPatternProvider", 36),
            ("GetPropertyValue", 60),
            ("HostRawElementProvider", 12),
            ("Navigate", 5),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 9),
        ],
    );

    ratchet.finish();
    drop(outpost);
    app.quit();
}

/// A UIA node whose element the outpost does not keep and that the
/// focus's route does not lead to is answered gone by object navigation,
/// the ancestors query and activation, with no call and no provider hit:
/// no `FindFirst` over the application's windows by its runtime id, which
/// would enter any child window another process serves
/// (`docs/design/cross-process-calls-study.md`, decision 3). The node is a
/// focus on the button `First`, which has no window of its own, sent by
/// the listener with no ancestor read and no window known, its walk cut, so
/// the outpost has no route to its element.
fn a_node_with_no_kept_element_and_no_route_is_gone_with_no_call() {
    let title = common::unique_title("mockapp-counts-uia-gone");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus first");
    let (judged, _) = outpost.listener_read(&under_test.focused(hwnd));
    let Judged::Read(mut read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    assert_eq!(read.hwnd, 0, "the button has no window of its own");
    read.ancestors.clear();
    read.ancestors_end = verbatim_model::protocol::AncestorsEnd::PlainFrom(0);
    read.window = None;
    outpost.deliver(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    let node = match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::FocusChanged {
                node, ancestors, ..
            },
            ..
        } => {
            assert_eq!(ancestors, [], "no window to complete its chain from");
            node
        }
        other => panic!("the outpost said {other:?}, not the focus"),
    };
    assert_eq!(node.name.as_deref(), Some("First"));
    outpost.settled();

    let mut ratchet = Ratchet::default();
    for (operation, query) in [
        (
            "UIA next sibling of a node gone",
            Query::Navigate {
                node_id: node.id,
                kind: QueryKind::NextSibling,
            },
        ),
        (
            "UIA ancestors of a node gone",
            Query::Ancestors { node_id: node.id },
        ),
        (
            "UIA activation of a node gone",
            Query::Activate { node_id: node.id },
        ),
    ] {
        common::reset_hits(hwnd);
        let calls_made = outpost.gone(query);
        let cost = Cost {
            calls: calls_made,
            hits: common::read_hits(hwnd),
            failed_reads: outpost.take_failed_reads(),
        };
        ratchet.check(operation, &cost, calls(0, 0, 0), 0, &[]);
    }
    ratchet.finish();
    drop(outpost);
    app.quit();
}

/// What registering for events costs mockapp. A registration is made on its
/// own thread and is the same with remote operations on or off, so it
/// makes no counted call on this thread either way; what it costs is the
/// provider calls UIA makes while registering, pinned here: the focus
/// listener's desktop-wide group (an element selected, a menu opened, and
/// notifications), and an outpost's focus-following property subscription
/// moved to a focus, `First` inside the group `Settings`, registered on the
/// focus alone, and moved to it again, which costs nothing.
fn uia_event_registrations_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-registrations");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::with_parts(
            vec![
                Part::shaped::<ListenerSelection>(Subscription::Event {
                    event: UIA_SelectionItem_ElementSelectedEventId,
                    callback: Arc::new(|_| {}),
                }),
                Part::shaped::<ListenerEvent>(Subscription::Event {
                    event: UIA_MenuOpenedEventId,
                    callback: Arc::new(|_| {}),
                }),
                Part::shaped::<ListenerEvent>(Subscription::Notifications {
                    callback: Arc::new(|_, _, _, _, _| {}),
                }),
            ],
            Scope::Desktop,
        )
        .expect("the listener's registration")
    });
    drop(registration);
    ratchet.check(
        "UIA desktop-wide registration, one group",
        &cost,
        calls(0, 0, 0),
        0,
        &[],
    );

    let focus = AgileReference::new(under_test.element("First")).expect("an agile reference");
    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::with_parts(
            vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
                properties: FOCUS_PROPERTIES.to_vec(),
                callback: Arc::new(|_, _| {}),
            })],
            Scope::Elements(vec![focus]),
        )
        .expect("the focus-following registration")
    });
    ratchet.check(
        "UIA focus-following registration",
        &cost,
        calls(0, 0, 0),
        0,
        &[("HostRawElementProvider", 1), ("FragmentRoot", 2)],
    );

    // A move to the element already followed, by another reference to it,
    // makes no call: the registration compares the elements and keeps the
    // group where it is (`docs/design/focus-pipeline.md`, section 17).
    let again = AgileReference::new(under_test.element("First")).expect("an agile reference");
    let ((), cost) = under_test.measure(hwnd, |_| {
        registration.retarget(Scope::Elements(vec![again]));
        registration.settle();
    });
    drop(registration);
    ratchet.check(
        "UIA focus-following move to the element already followed",
        &cost,
        calls(0, 0, 0),
        0,
        &[],
    );

    ratchet.finish();
    drop(app);
}

/// A container's selected child, through `SelectionPattern2` (mockapp's
/// list has it) and, for a provider without it (mockapp's tab control),
/// through the `Selection` pattern, both ways the outpost reads it: inside
/// the focus's remote program, and classically. The calls of the focus
/// ancestry around it are those of the focus ledger above; these are the
/// selected child's alone classically, and the whole program remotely.
#[expect(
    clippy::too_many_lines,
    reason = "both containers' pinned provider hits, listed in full"
)]
fn uia_selected_children_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-selection");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let nodes = under_test
        .uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    let ancestors = Fill::new(
        &under_test
            .uia
            .request::<Ancestor>(common::ancestor_gates())
            .expect("a cache request"),
    );
    let element_fill = Fill::new(
        &under_test
            .uia
            .request::<Ancestor>(Ancestor::all_on())
            .expect("a cache request"),
    );
    let mut ratchet = Ratchet::default();

    // The program's hits beyond the selected child's, the same for both.
    let program = |extra_pattern: u32, selection: (&'static str, u32)| {
        vec![
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 24),
            ("GetPatternProvider", 26 + extra_pattern),
            ("GetPropertyValue", 48),
            ("HostRawElementProvider", 10),
            ("Navigate", 8),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
            ("IsSelected", 1),
            selection,
        ]
    };
    for (id, name, selected, classic_calls, classic_hits, remote_hits) in [
        // `FirstSelectedItem`, cached in the call that also caches the
        // `Selection` pattern, and the item's cache.
        (
            "fruits",
            "Fruits",
            "Banana",
            2,
            vec![
                ("ProviderOptions", 14),
                ("GetPatternProvider", 16),
                ("GetPropertyValue", 20),
                ("HostRawElementProvider", 8),
                ("Navigate", 1),
                ("GetRuntimeId", 5),
                ("BoundingRectangle", 1),
                ("FragmentRoot", 7),
                ("IsSelected", 1),
                ("FirstSelectedItem", 1),
            ],
            program(0, ("FirstSelectedItem", 1)),
        ),
        // `FirstSelectedItem` answered "not supported" in the same call
        // that cached the `Selection` pattern, then its selection and the
        // item's cache: 3, as the `Selection` pattern alone took. The
        // program asks for `SelectionPattern2` once more.
        (
            "pages",
            "Pages",
            "General",
            3,
            vec![
                ("ProviderOptions", 14),
                ("GetPatternProvider", 17),
                ("GetPropertyValue", 20),
                ("HostRawElementProvider", 9),
                ("Navigate", 1),
                ("GetRuntimeId", 5),
                ("BoundingRectangle", 1),
                ("FragmentRoot", 8),
                ("IsSelected", 1),
                ("GetSelection", 1),
            ],
            program(1, ("GetSelection", 1)),
        ),
    ] {
        common::apply(&mut app, hwnd, &format!("set-focus {id}"));
        let container = Live::new(under_test.element(name).clone());
        let (child, cost) = under_test.measure(hwnd, |under_test| {
            under_test
                .uia
                .selected(&container, &nodes)
                .expect("the selection is read")
        });
        assert_eq!(
            child.expect("a selected child").read().name.as_deref(),
            Some(selected)
        );
        ratchet.check(
            &format!("UIA selected child of {name}, classically"),
            &cost,
            calls(classic_calls, 0, 0),
            0,
            &classic_hits,
        );
        // The focus's element as the outpost reads a focus, before the
        // measurement.
        let input = container
            .refresh(&nodes)
            .expect("the container's cache")
            .narrow();
        let (ancestry, cost) = under_test.measure(hwnd, |under_test| {
            focus_ancestry(
                &under_test.uia,
                &FocusQuery {
                    element: &input,
                    known: &[],
                    previous: None,
                    depth_limit: 64,
                    fill: &ancestors,
                    element_fill: &element_fill,
                    out_of_time: None,
                    require_focus: true,
                    refresh: false,
                    nearest_window: None,
                    before_call: None,
                    value_available: None,
                    plain: None,
                    progress: None,
                },
                true,
            )
            .expect("the focus ancestry")
        });
        let FocusAncestry::Focused(ancestry) = ancestry.0 else {
            panic!("{name} has the focus");
        };
        assert_eq!(
            ancestry
                .selected_child
                .expect("a selected child")
                .read()
                .node
                .name
                .as_deref(),
            Some(selected)
        );
        ratchet.check(
            &format!("UIA focus on {name} with its selected child, remotely"),
            &cost,
            calls(1, 0, 0),
            0,
            &remote_hits,
        );
    }

    ratchet.finish();
    app.quit();
}

/// The provider work each event of the outpost's focus-following
/// registrations asks of mockapp for its element's cache, measured as UIA
/// fills that cache request on demand (`BuildUpdatedCache`): a text focus's
/// caret and text change events carry little more than their window, since
/// nothing is spoken from them (`TextEvent`), and a property change of the
/// focus carries what its name, value, and states are read from
/// (`PropertyEvent`), while every desktop-wide event of the listener's
/// registration carries what the listener judges and reads a focus from
/// (`ListenerEvent`), and an element selected also what Core speaks of the
/// item (`ListenerSelection`); every event was registered with the base
/// cache request, 34 properties, before. Measured on demand because the calls UIA makes to
/// route an event to its registration vary from one delivery to another
/// with how busy the machine is, while a cache request's calls do not.
fn uia_event_caches_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-event-caches");
    let app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let notes = under_test.element("Notes");
    let text = under_test
        .uia
        .request_fixed::<TextEvent>()
        .expect("the text event's request");
    let property = under_test
        .uia
        .request_fixed::<PropertyEvent>()
        .expect("the property event's request");
    let listener = under_test
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener event's request");
    let selection = under_test
        .uia
        .request_fixed::<ListenerSelection>()
        .expect("the listener selection's request");
    let live = Live::new(notes.clone());
    let (_, cost) = under_test.measure(hwnd, |_| {
        live.refresh(&text).expect("the element's cache is filled")
    });
    ratchet.check(
        "UIA event cache, a text focus's caret and text changes",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 5),
            ("GetPropertyValue", 3),
            ("HostRawElementProvider", 3),
            ("GetRuntimeId", 2),
            ("FragmentRoot", 3),
        ],
    );
    let (_, cost) = under_test.measure(hwnd, |_| {
        live.refresh(&property)
            .expect("the element's cache is filled")
    });
    ratchet.check(
        "UIA event cache, the focus's property changes",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 5),
            ("GetPatternProvider", 11),
            ("GetPropertyValue", 12),
            ("HostRawElementProvider", 3),
            ("GetRuntimeId", 2),
            ("FragmentRoot", 3),
        ],
    );
    let (_, cost) = under_test.measure(hwnd, |_| {
        live.refresh(&listener)
            .expect("the element's cache is filled")
    });
    ratchet.check(
        "UIA event cache, a desktop-wide event",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 5),
            ("GetPatternProvider", 3),
            ("GetPropertyValue", 15),
            ("HostRawElementProvider", 3),
            ("GetRuntimeId", 2),
            ("FragmentRoot", 3),
        ],
    );
    let (_, cost) = under_test.measure(hwnd, |_| {
        live.refresh(&selection)
            .expect("the element's cache is filled")
    });
    ratchet.check(
        "UIA event cache, an element selected",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 5),
            ("GetPatternProvider", 16),
            ("GetPropertyValue", 27),
            ("HostRawElementProvider", 3),
            ("GetRuntimeId", 2),
            ("FragmentRoot", 3),
        ],
    );
    ratchet.finish();
    app.quit();
}

/// The active text position changed event: registering for it, with a text
/// focus's caret and text changes, as one group on the focus, and handling
/// one, which keeps the event's range as a position in the focus's text and
/// reads nothing, the same with remote operations on or off.
fn uia_active_text_position_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-active-position");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let (seen, ranges) = mpsc::channel();
    let notes = AgileReference::new(under_test.element("Notes")).expect("an agile reference");
    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::with_parts(
            vec![
                Part::shaped::<TextEvent>(Subscription::Events {
                    events: vec![
                        UIA_Text_TextSelectionChangedEventId,
                        UIA_Text_TextChangedEventId,
                    ],
                    callback: Arc::new(|_, _| {}),
                }),
                Part::shaped::<TextEvent>(Subscription::ActiveTextPosition {
                    callback: Arc::new(move |_, range| {
                        let range = range.and_then(|range| AgileReference::new(range).ok());
                        // The test may have finished.
                        let _ = seen.send(range);
                    }),
                }),
            ],
            Scope::Elements(vec![notes]),
        )
        .expect("the text focus's registration")
    });
    ratchet.check(
        "UIA text focus registration",
        &cost,
        calls(0, 0, 0),
        0,
        &[("HostRawElementProvider", 1), ("FragmentRoot", 2)],
    );

    app.send("active-text-position doc 6 10");
    let range = ranges
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the active text position changed event")
        .expect("the event carries its range")
        .resolve()
        .expect("the event's range");
    let mut store = Anchors::new(Arc::default());
    let (_, cost) = under_test.measure(hwnd, |_| {
        let start = UiaPos::start_of(&range).expect("the range's start");
        store.node(1).position_at(start)
    });
    ratchet.check(
        "UIA active text position change",
        &cost,
        calls(0, 0, 0),
        0,
        &[],
    );

    drop(registration);
    ratchet.finish();
    app.quit();
}

/// A check of a caret key's watch, as the worker makes one: when the
/// request arrives (no caret event) or as a caret event prompts it,
/// counting the caret's reads.
#[derive(Default)]
struct Check {
    caret_event: bool,
    reads: u32,
}

impl CaretSignal for Check {
    fn caret_event(&mut self) -> bool {
        self.caret_event
    }

    fn now_ms(&mut self) -> u64 {
        0
    }

    fn reading(&mut self) {
        self.reads += 1;
    }
}

/// The caret reply a check of a key's watch answered with.
fn answered(watched: Watched) -> verbatim_model::CaretReply {
    match watched {
        Watched::Answered(TextReply::Caret(reply)) => *reply,
        other => panic!("a caret reply, not {other:?}"),
    }
}

/// Right Arrow's watch, with the caret Core knew before the key.
fn right_arrow(before: &verbatim_model::CaretReport) -> CaretWatch {
    CaretWatch {
        landing: false,
        pressed_at_ms: 0,
        since: Some(TextPosition {
            anchor: before.line.start,
            offset: before.line.offset,
        }),
        unit: TextUnit::Character,
        compare: None,
        deletes: false,
    }
}

/// One caret move answered, as the outpost's worker answers a caret key
/// with `text_reads::check_watch` once it has the node's text: the caret is
/// reported (Core's knowledge), mockapp's caret moves one character on, as
/// Right Arrow moves it, and the watch's first check finds the evidence and
/// reports the line and the character there. Then the caret moves on again
/// and is reported alone, as the worker reports it for a caret event, after
/// a typed character above all, with `text_reads::report_caret`: compared
/// with the selection last reported, the answer's, for how it changed.
/// Then a selection no key made is reported the same way, with how it
/// changed. Then the caret, moved on with nothing selected, is read as the
/// outpost reads it before a held caret key is passed on
/// (`TextOp::ReadCaret`, `docs/design/key-holding.md`): the report's read,
/// leaving the kept selection alone. Returns the calls and hits of the
/// answer, of the report, of the report with a selection change, and of
/// the read before a held key.
fn measure_caret_move<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    take: fn() -> CallCounts,
) -> (Cost, Cost, Cost, Cost) {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 0");
    let (before, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 1");
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    let reply = answered(check_caret(
        source,
        &mut anchors,
        &right_arrow(&before),
        &mut Check::default(),
    ));
    let calls = take();
    assert!(reply.moved);
    assert_eq!(reply.unit.expect("the character").text, "l");
    let answer = Cost {
        calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };

    common::apply(app, hwnd, "caret doc 2");
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    let read = caret_report_since(
        source,
        &mut anchors,
        &mut || 0,
        (false, None, LastSelection::Compare),
    )
    .expect("the caret");
    let calls = take();
    assert_eq!(read.report.line.offset, 2);
    assert_eq!(read.selection_changes, []);
    let report = Cost {
        calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };

    // A selection no key made, from the caret: "al" selected.
    common::apply(app, hwnd, "caret doc 0 2");
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    let read = caret_report_since(
        source,
        &mut anchors,
        &mut || 0,
        (false, None, LastSelection::Compare),
    )
    .expect("the caret");
    let calls = take();
    assert_eq!(
        read.selection_changes,
        [SelectionChange {
            selected: true,
            text: "al".to_owned(),
            characters: 2,
        }]
    );
    let selected = Cost {
        calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };

    common::apply(app, hwnd, "caret doc 3");
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    let read = caret_report_since(
        source,
        &mut anchors,
        &mut || 0,
        (false, None, LastSelection::Ignore),
    )
    .expect("the caret");
    let calls = take();
    assert_eq!(read.report.line.offset, 3);
    assert_eq!(read.selection_changes, []);
    let before_key = Cost {
        calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };
    (answer, report, selected, before_key)
}

/// What a remote caret read costs the provider beyond its text calls: the
/// element's import and its text pattern.
const IMPORT_HITS: [(&str, u32); 5] = [
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
];

/// The text calls of a remote caret read that a classic one made as
/// `hits`: inside the provider the program also copies the collapsed caret
/// before using it, one clone and one endpoint move more.
fn plus_copy(hits: &[(&'static str, u32)]) -> Vec<(&'static str, u32)> {
    hits.iter()
        .map(|&(name, count)| match name {
            "Clone" | "MoveEndpointByRange" => (name, count + 1),
            _ => (name, count),
        })
        .collect()
}

/// The hits of `times` remote caret reads that each made `text` calls.
fn remote_hits(times: u32, text: &[(&'static str, u32)]) -> Vec<(&'static str, u32)> {
    IMPORT_HITS
        .iter()
        .chain(text)
        .map(|&(name, count)| (name, count * times))
        .collect()
}

/// The report after a focus, whose line is spoken, with the line's
/// formatting: mockapp's first line has four stretches (bold "alpha", a
/// space, the misspelt "beta", the line feed), each read as `attributes`
/// says, given whether the stretch is bold and whether it is misspelt.
fn measure_focus_report(
    hwnd: HWND,
    source: &mut UiaText,
    attributes: impl Fn(bool, bool) -> TextAttributes,
) -> Cost {
    let mut store = Anchors::new(Arc::default());
    let _ = verbatim_uia::calls::take();
    let _ = verbatim_uia::calls::take_failed_reads();
    common::reset_hits(hwnd);
    let (report, _) = caret_report(source, &mut store.node(1), &mut || 0, true).expect("the caret");
    let cost = Cost {
        calls: verbatim_uia::calls::take(),
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };
    let runs: Vec<(u32, u32, TextAttributes)> = report
        .line
        .formats
        .iter()
        .map(|run| (run.start, run.end, run.attributes.clone()))
        .collect();
    assert_eq!(
        runs,
        [
            (0, 5, attributes(true, false)),
            (5, 6, attributes(false, false)),
            (6, 10, attributes(false, true)),
            (10, 11, attributes(false, false)),
        ]
    );
    cost
}

/// The attributes the default theme reads: the errors, and links, which
/// mockapp's text without styles does not support.
fn errors_only(_bold: bool, spelling_error: bool) -> TextAttributes {
    TextAttributes {
        spelling_error,
        ..TextAttributes::default()
    }
}

/// Every attribute mockapp's text without styles reports, as the outpost
/// reads it: strikethrough, the background color, bullets, and links are
/// not supported, and read as none.
fn every_attribute(bold: bool, spelling_error: bool) -> TextAttributes {
    TextAttributes {
        spelling_error,
        grammar_error: false,
        font_name: Some("Consolas".to_owned()),
        font_size: Some("11.0 pt".to_owned()),
        color: Some("black".to_owned()),
        bold: Some(bold),
        italic: Some(false),
        underline: Some(false),
        underline_style: Some(LineStyle::None),
        strikethrough: None,
        background_color: None,
        bullet: None,
        link: false,
    }
}

/// A caret move, a caret report, the report after a focus with its
/// line's formatting, and a wait that finds nothing, through UIA, remotely
/// or classically, with the default theme's formatting (spelling and
/// grammar errors, and links). The report after a focus is measured twice
/// for the same text, as the outpost keeps what it learned of the control
/// between reads: the first read learns that mockapp's text without styles
/// does not support links (or, with every indication on, strikethrough,
/// the background color, and bullets too), and the second no longer asks
/// for them.
#[expect(
    clippy::too_many_lines,
    reason = "both ways' pinned provider hits, listed in full"
)]
fn check_uia_caret_costs(ratchet: &mut Ratchet, remote: bool) {
    let title = common::unique_title("mockapp-counts-uia-caret");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd).remote(remote);
    let (answer, report, selected, before_key) =
        measure_caret_move(&mut app, hwnd, &mut source, verbatim_uia::calls::take);
    let (nothing, later) = measure_watch(&mut app, hwnd, &mut source, verbatim_uia::calls::take);
    let focus_report = measure_focus_report(hwnd, &mut source, errors_only);
    let focus_again = measure_focus_report(hwnd, &mut source, errors_only);
    // The same report with every formatting indication on: eleven
    // attributes per stretch, the annotation types, font name and size,
    // weight, italic, underline style, strikethrough style, color,
    // background color, bullet style, and link, until the four mockapp
    // does not support are learned.
    let mut formatted = uia_notes(hwnd).remote(remote).fetches(Fetches::default());
    let formatted_report = measure_focus_report(hwnd, &mut formatted, every_attribute);
    let formatted_again = measure_focus_report(hwnd, &mut formatted, every_attribute);
    assert_eq!(
        formatted.known_support().unsupported,
        Attributes::of(&[
            TextAttribute::StrikethroughStyle,
            TextAttribute::BackgroundColor,
            TextAttribute::BulletStyle,
            TextAttribute::Link,
        ])
    );
    // The caret's read, the evidence, the line and the caret's offset
    // in it, the character's spelling error and link, which a
    // character's read never learns are not supported, and the caret read
    // again and compared, to tell whether it moved while it was read.
    let move_hits = [
        ("ITextProvider::GetSelection", 2),
        ("Clone", 3),
        ("CompareEndpoints", 4),
        ("ExpandToEnclosingUnit", 2),
        ("GetAttributeValue", 2),
        ("GetText", 2),
        ("MoveEndpointByRange", 1),
    ];
    let report_hits = [
        ("ITextProvider::GetSelection", 1),
        ("Clone", 2),
        ("CompareEndpoints", 1),
        ("ExpandToEnclosingUnit", 1),
        ("GetText", 2),
        ("MoveEndpointByRange", 1),
    ];
    // The report's; the line's annotation types, which it has, and, while
    // their support is not known, the attributes being learned, asked of
    // the line; and the line walked by the format unit: four stretches,
    // each compared with the line's end (the last cut there), read, and its
    // attributes read, and the walk moved on after each but the last. 39
    // classically before the text range audit of 2026-10-07, which found
    // that the comparison of where the next stretch starts repeated the one
    // before it.
    let with_attributes = |reads: u32| -> Vec<(&'static str, u32)> {
        [
            ("ITextProvider::GetSelection", 1),
            ("Clone", 7),
            ("CompareEndpoints", 5),
            ("ExpandToEnclosingUnit", 1),
            ("GetAttributeValue", reads),
            ("GetText", 6),
            ("MoveEndpointByUnit", 4),
            ("MoveEndpointByRange", 6),
        ]
        .to_vec()
    };
    // The line's annotation types and link, then two attributes per
    // stretch; once links are known to be unsupported, the line's
    // annotation types and one per stretch.
    let focus_hits = with_attributes(2 + 4 * 2);
    let focus_again_hits = with_attributes(1 + 4);
    // The line's annotation types and the ten others being learned, then
    // eleven attributes per stretch; once four are known to be
    // unsupported and the rest supported, the line's annotation types and
    // seven per stretch.
    let formatted_hits = with_attributes(11 + 4 * 11);
    let formatted_again_hits = with_attributes(1 + 4 * 7);
    if remote {
        // One round trip each, the program copying the collapsed caret
        // (`plus_copy`).
        ratchet.check(
            "UIA caret move, remotely",
            &answer,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&move_hits)),
        );
        ratchet.check(
            "UIA caret report, remotely",
            &report,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&report_hits)),
        );
        // The caret report's round trip, comparing no selection.
        ratchet.check(
            "UIA caret read before a held key, remotely",
            &before_key,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&report_hits)),
        );
        // Still one round trip: the program compares the selection with
        // the one last reported, reads the change it finds, reads the new
        // selection's text, kept with it for the next comparison, and
        // where the selection starts, here the text's start. Inside the
        // provider, the classic read's calls (below), the program's copy of
        // the caret (`plus_copy`), three comparisons and one pattern
        // request more, and the element's import. The last selection
        // holding nothing, the change is the whole new selection, so the
        // two comparisons that place a change on each side are not made
        // (since 2026-10-23).
        ratchet.check(
            "UIA caret report with a selection change, remotely",
            &selected,
            calls(1, 0, 0),
            0,
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 2),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("DocumentRange", 1),
                ("GetCaretRange", 1),
                ("Clone", 5),
                ("CompareEndpoints", 7),
                ("ExpandToEnclosingUnit", 1),
                ("GetText", 5),
                ("MoveEndpointByRange", 5),
            ],
        );
        ratchet.check(
            "UIA caret report after a focus, remotely",
            &focus_report,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&focus_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus, links learned, remotely",
            &focus_again,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&focus_again_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute, remotely",
            &formatted_report,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&formatted_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute learned, remotely",
            &formatted_again,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&formatted_again_hits)),
        );
        // One round trip for each check, the whole read: the check that
        // finds the evidence is the answer. 11 before 2026-10-08, one read
        // every 10 milliseconds until 100 had passed.
        ratchet.check(
            "UIA caret watch finding nothing, remotely",
            &nothing,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&move_hits)),
        );
        ratchet.check(
            "UIA caret watch answered by a later caret event, remotely",
            &later,
            calls(1, 0, 0),
            0,
            &remote_hits(1, &plus_copy(&move_hits)),
        );
    } else {
        ratchet.check(
            "UIA caret move, classically",
            &answer,
            calls(15, 0, 0),
            0,
            &move_hits,
        );
        ratchet.check(
            "UIA caret report, classically",
            &report,
            calls(8, 0, 0),
            0,
            &report_hits,
        );
        // The caret report's 8, comparing no selection.
        ratchet.check(
            "UIA caret read before a held key, classically",
            &before_key,
            calls(8, 0, 0),
            0,
            &report_hits,
        );
        // The caret report's 8 with the selection's caret
        // (`GetCaretRange`); its ends compared with the old caret's (2);
        // the old selection holding nothing, no comparison to place the
        // change, which is the whole new selection (four before
        // 2026-10-23); the change's text read (a copy, its two ends moved,
        // its text); the new selection's text, kept with it for the next
        // comparison; and where the selection starts, since
        // `TextPattern2`'s caret may be its other end (the start compared
        // with the line's, a copy of the line, its end moved to the start,
        // its text), and, the start being the line's, whether it is the
        // text's (its document range, compared): 21 (25 before
        // 2026-10-23, 18 before 2026-10-22, 19 before the selection's
        // start was read). A selection starting inside a line costs 19. The report of a caret with nothing
        // selected, after one with nothing selected, compares nothing (8,
        // above).
        ratchet.check(
            "UIA caret report with a selection change, classically",
            &selected,
            calls(21, 0, 0),
            0,
            &[
                ("ITextProvider::GetSelection", 1),
                ("DocumentRange", 1),
                ("GetCaretRange", 1),
                ("Clone", 4),
                ("CompareEndpoints", 4),
                ("ExpandToEnclosingUnit", 1),
                ("GetText", 5),
                ("MoveEndpointByRange", 4),
            ],
        );
        // The caret report's 8, one call asking the line for its
        // annotation types and the attributes being learned, and the walk's
        // 26, with one `GetAttributeValues` call per stretch for all its
        // attributes (`IUIAutomationTextRange3`), which the provider
        // answers one attribute at a time: 35 however many attributes are
        // read (34 before the line was asked first; 63 with seven
        // attributes when each attribute was a call; 39 before the text
        // range audit).
        ratchet.check(
            "UIA caret report after a focus, classically",
            &focus_report,
            calls(35, 0, 0),
            0,
            &focus_hits,
        );
        ratchet.check(
            "UIA caret report after a focus, links learned, classically",
            &focus_again,
            calls(35, 0, 0),
            0,
            &focus_again_hits,
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute, classically",
            &formatted_report,
            calls(35, 0, 0),
            0,
            &formatted_hits,
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute learned, classically",
            &formatted_again,
            calls(35, 0, 0),
            0,
            &formatted_again_hits,
        );
        // A caret move's reads for each check. 134 before 2026-10-08, 12
        // for each of eleven reads and 2 comparing the caret with the
        // document's end; 12 for each check before the caret was read
        // again (2026-10-14).
        ratchet.check(
            "UIA caret watch finding nothing, classically",
            &nothing,
            calls(15, 0, 0),
            0,
            &move_hits,
        );
        ratchet.check(
            "UIA caret watch answered by a later caret event, classically",
            &later,
            calls(15, 0, 0),
            0,
            &move_hits,
        );
    }
    app.quit();
}

/// One text request answered as the outpost's worker answers it
/// (`text_reads::answer`), with its calls and hits.
fn measure_text(
    hwnd: HWND,
    source: &mut UiaText,
    anchors: &mut verbatim_outpost::text::NodeText<'_, verbatim_outpost::text::uia::UiaPos>,
    op: &TextOp,
) -> (TextReply, Cost) {
    let _ = verbatim_uia::calls::take();
    let _ = verbatim_uia::calls::take_failed_reads();
    common::reset_hits(hwnd);
    let reply = perform(source, anchors, op);
    (
        reply,
        Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
            failed_reads: verbatim_uia::calls::take_failed_reads(),
        },
    )
}

/// The text protocol's other requests through UIA, remotely or
/// classically: the review cursor's next line, a word from a position
/// inside the line (found by the text before it), say-all's read ahead of
/// every line, the selected text read for a copy, the caret moved as
/// say-all moves it, the caret's location, and a caret key's answer that
/// reports a selection's change.
#[expect(
    clippy::too_many_lines,
    reason = "each request measured in turn, as one session makes them"
)]
fn check_uia_text_costs(ratchet: &mut Ratchet, remote: bool) {
    let way = if remote { "remotely" } else { "classically" };
    let title = common::unique_title("mockapp-counts-uia-text");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd).remote(remote);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(&mut app, hwnd, "caret doc 0");
    let (report, _) = caret_report(&mut source, &mut anchors, &mut || 0, false).expect("the caret");
    let caret = TextPosition {
        anchor: report.line.start,
        offset: report.line.offset,
    };

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Read(TextRead {
            at: TextPoint::At(caret),
            movement: Some(TextMovement {
                unit: TextUnit::Line,
                count: 1,
            }),
            unit: TextUnit::Line,
        }),
    );
    let TextReply::Read { chunk: line, .. } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(line.text, "gamma\n");
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 3),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 1),
                ("Move", 1),
                ("MoveEndpointByRange", 2),
            ],
        )
    } else {
        (
            calls(8, 0, 0),
            &[
                ("Clone", 2),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 1),
                ("Move", 1),
                ("MoveEndpointByRange", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA review next line, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition {
                anchor: report.line.start,
                offset: 6,
            }),
            movement: None,
            unit: TextUnit::Word,
        }),
    );
    let TextReply::Read { chunk: word, .. } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(word.text, "beta");
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 4),
                ("ExpandToEnclosingUnit", 1),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByUnit", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(12, 0, 0),
            &[
                ("Clone", 3),
                ("ExpandToEnclosingUnit", 1),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByUnit", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    };
    ratchet.check(
        &format!("UIA review word inside the line, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::ReadAhead(TextReadAhead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
            count: 16,
        }),
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(texts, ["alpha beta\n", "gamma\n", ""]);
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 7),
                ("CompareEndpoints", 3),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 1),
                ("GetText", 4),
                ("Move", 3),
                ("MoveEndpointByRange", 6),
            ],
        )
    } else {
        (
            calls(26, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("Clone", 6),
                ("CompareEndpoints", 3),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 1),
                ("GetText", 4),
                ("Move", 3),
                ("MoveEndpointByRange", 5),
            ],
        )
    };
    ratchet.check(
        &format!("UIA say-all read ahead, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::MoveCaret(TextPoint::At(TextPosition::at(chunks[1].start))),
    );
    assert_eq!(reply, TextReply::Done);
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 2),
                ("CompareEndpoints", 1),
                ("MoveEndpointByRange", 2),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    } else {
        (
            calls(4, 0, 0),
            &[
                ("Clone", 1),
                ("CompareEndpoints", 1),
                ("MoveEndpointByRange", 1),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA say-all caret move, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Location(TextPoint::Caret),
    );
    assert_eq!(reply, TextReply::Location { x: 100, y: 216 });
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 3),
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 2),
                ("Navigate", 1),
                ("FragmentRoot", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 2),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 1),
                ("GetBoundingRectangles", 1),
                ("MoveEndpointByRange", 1),
            ],
        )
    } else {
        (
            calls(5, 0, 0),
            &[
                ("ProviderOptions", 1),
                ("HostRawElementProvider", 1),
                ("FragmentRoot", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 1),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 1),
                ("GetBoundingRectangles", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA caret location, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    common::apply(&mut app, hwnd, "caret doc 0 5");
    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::ReadRange {
            start: TextPoint::SelectionStart,
            end: TextPoint::SelectionEnd,
            through: None,
        },
    );
    assert_eq!(
        reply,
        TextReply::Range {
            text: "alpha".to_owned(),
            truncated: false
        }
    );
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 2),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 3),
                ("CompareEndpoints", 2),
                ("GetText", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(7, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 1),
                ("CompareEndpoints", 2),
                ("GetText", 1),
                ("MoveEndpointByRange", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA selected text, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    common::apply(&mut app, hwnd, "caret doc 0");
    let (collapsed, _) =
        caret_report(&mut source, &mut anchors, &mut || 0, false).expect("the caret");
    let at = TextPosition {
        anchor: collapsed.line.start,
        offset: collapsed.line.offset,
    };
    common::apply(&mut app, hwnd, "caret doc 0 5");
    let _ = verbatim_uia::calls::take();
    let _ = verbatim_uia::calls::take_failed_reads();
    let reply = answered(check_caret(
        &mut source,
        &mut anchors,
        &CaretWatch {
            landing: false,
            pressed_at_ms: 0,
            since: Some(at),
            unit: TextUnit::Character,
            compare: None,
            deletes: false,
        },
        &mut Check::default(),
    ));
    let cost = Cost {
        calls: verbatim_uia::calls::take(),
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };
    let changes: Vec<(bool, &str, u32)> = reply
        .selection_changes
        .iter()
        .map(|change| (change.selected, change.text.as_str(), change.characters))
        .collect();
    assert_eq!(changes, [(true, "alpha", 5)]);
    // The character's spelling error and link: two attribute reads (one
    // before the default theme reported links). The caret read again at
    // the end, the selection and `TextPattern2`'s caret, and compared. The
    // new selection's text, kept with it for the next comparison (one
    // `GetText` more since 2026-10-22), and where the selection starts,
    // here the text's start (six calls more classically: 28 before, four
    // for a start inside a line). The key compares with the selection the
    // outpost kept, its caret report's, which held nothing, so the change
    // is the whole new selection and no comparison places it (two fewer
    // remotely and four fewer classically since 2026-10-23).
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 3),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 2),
                ("DocumentRange", 1),
                ("GetCaretRange", 2),
                ("Clone", 6),
                ("CompareEndpoints", 10),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 2),
                ("GetText", 5),
                ("MoveEndpointByRange", 5),
            ],
        )
    } else {
        (
            calls(30, 0, 0),
            &[
                ("ITextProvider::GetSelection", 2),
                ("DocumentRange", 1),
                ("GetCaretRange", 2),
                ("Clone", 5),
                ("CompareEndpoints", 8),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 2),
                ("GetText", 5),
                ("MoveEndpointByRange", 4),
            ],
        )
    };
    ratchet.check(
        &format!("UIA caret key selecting, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );

    // The review copy's first press: "alpha" selected from a collapsed
    // caret, the selection read before and after and compared. As Core
    // sends it: the marker on the first "a" and the review cursor on the
    // last, each with the point just past its character.
    common::apply(&mut app, hwnd, "caret doc 0");
    let line_at = |offset| {
        TextPoint::At(TextPosition {
            anchor: collapsed.line.start,
            offset,
        })
    };
    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Select {
            start: TextPoint::At(at),
            end: line_at(4),
            through: Some(TextThrough {
                start: line_at(at.offset + 1),
                end: line_at(5),
            }),
        },
    );
    assert_eq!(selected_texts(&reply), [(true, "alpha", 5)]);
    // The three points past the start each found by the text before it
    // (a `MoveEndpointByUnit` and a `GetText` each), the start and the end
    // compared, the selection read before and after (the second with
    // `TextPattern2`'s caret, text now being selected), the select, and
    // the change's ends compared and its text read, and the selection's
    // text read after, up to 513 units, kept for the next comparison (one
    // `GetText` more since 2026-10-22). The selection before holding
    // nothing, the change is the whole new selection, with no comparison
    // to place it (two fewer remotely, four classically, since
    // 2026-10-23). Before 2026-10-15 the
    // read before was classic and the read after a caret read of its own:
    // 4 calls remotely, 30 classically; and the range was the marker to
    // the point past the review cursor's character, two points, before
    // the points past both ends' characters were sent for a range read
    // either way round: 22 calls classically.
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 2),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 2),
                ("GetCaretRange", 1),
                ("Clone", 9),
                ("CompareEndpoints", 5),
                ("GetText", 5),
                ("MoveEndpointByUnit", 3),
                ("MoveEndpointByRange", 10),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    } else {
        (
            calls(29, 0, 0),
            &[
                ("ITextProvider::GetSelection", 2),
                ("GetCaretRange", 1),
                ("Clone", 5),
                ("CompareEndpoints", 3),
                ("GetText", 5),
                ("MoveEndpointByUnit", 3),
                ("MoveEndpointByRange", 9),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA review copy select, {way}"),
        &cost,
        expected.0,
        0,
        expected.1,
    );
    app.quit();
}

/// The changes a [`TextReply::Selected`] holds, as (selected, text,
/// characters).
fn selected_texts(reply: &TextReply) -> Vec<(bool, &str, u32)> {
    let TextReply::Selected(changes) = reply else {
        panic!("a selection's changes, not {reply:?}");
    };
    changes
        .iter()
        .map(|change| (change.selected, change.text.as_str(), change.characters))
        .collect()
}

fn uia_text_requests_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for remote in [true, false] {
        check_uia_text_costs(&mut ratchet, remote);
    }
    ratchet.finish();
}

/// Say-all's batches at their full size, as Core asks for them
/// (`say_all::read_next`): twenty lines from the caret, then twenty more a
/// line on from the start of the last line read, over mockapp's text
/// replaced by forty-five short lines, remotely or classically.
#[expect(
    clippy::too_many_lines,
    reason = "both batches' pinned provider hits, listed in full both ways"
)]
fn check_uia_say_all_batch_costs(ratchet: &mut Ratchet, remote: bool) {
    let way = if remote { "remotely" } else { "classically" };
    let title = common::unique_title("mockapp-counts-uia-say-all");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let lines: Vec<String> = (1..=45).map(|line| format!("line {line}\n")).collect();
    common::apply(
        &mut app,
        hwnd,
        &format!("set-text doc {}", lines.concat().replace('\n', "\\n")),
    );
    common::apply(&mut app, hwnd, "caret doc 0");
    let mut source = uia_notes(hwnd).remote(remote);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    let batch = |at: TextPoint, movement: Option<TextMovement>| {
        TextOp::ReadAhead(TextReadAhead {
            at,
            movement,
            unit: TextUnit::Line,
            count: 20,
        })
    };

    let (reply, first) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &batch(TextPoint::Caret, None),
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(texts, lines[..20]);
    assert!(chunks.iter().all(|chunk| !chunk.last));

    let (reply, second) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &batch(
            TextPoint::At(TextPosition::at(chunks[19].start)),
            Some(TextMovement {
                unit: TextUnit::Line,
                count: 1,
            }),
        ),
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(texts, lines[20..40]);
    assert!(chunks.iter().all(|chunk| !chunk.last));

    // Classically, the first batch: the caret (2), the first line and the
    // caret's offset in it (5), each later line a copy collapsed, moved,
    // expanded, compared with the line before it, and read (6 each, 114),
    // the move that finds a next line after the twentieth (3), and one
    // `Culture` read over the whole batch (3). The later batch starts from
    // a held position moved by a line (4) and needs no offset (3 for its
    // first line). The comparison, since 2026-10-23, ends a batch at a line
    // that starts before the one read before it, as the console host's
    // expansion below its last line with text gives (109 and 108 before).
    // 165 and 166 before 2026-10-07: two collapses and two copies per line,
    // and a `Culture` read per line.
    let expected: [(CallCounts, &[(&str, u32)]); 2] = if remote {
        [
            (
                calls(1, 0, 0),
                &[
                    ("ProviderOptions", 2),
                    ("GetPatternProvider", 1),
                    ("GetPropertyValue", 1),
                    ("HostRawElementProvider", 1),
                    ("Navigate", 1),
                    ("ITextProvider::GetSelection", 1),
                    ("Clone", 24),
                    ("CompareEndpoints", 20),
                    ("ExpandToEnclosingUnit", 20),
                    ("GetAttributeValue", 1),
                    ("GetText", 21),
                    ("Move", 20),
                    ("MoveEndpointByRange", 23),
                ],
            ),
            (
                calls(1, 0, 0),
                &[
                    ("Clone", 24),
                    ("CompareEndpoints", 19),
                    ("ExpandToEnclosingUnit", 21),
                    ("GetAttributeValue", 1),
                    ("GetText", 20),
                    ("Move", 21),
                    ("MoveEndpointByRange", 23),
                ],
            ),
        ]
    } else {
        [
            (
                calls(128, 0, 0),
                &[
                    ("ITextProvider::GetSelection", 1),
                    ("Clone", 23),
                    ("CompareEndpoints", 20),
                    ("ExpandToEnclosingUnit", 20),
                    ("GetAttributeValue", 1),
                    ("GetText", 21),
                    ("Move", 20),
                    ("MoveEndpointByRange", 22),
                ],
            ),
            (
                calls(127, 0, 0),
                &[
                    ("Clone", 23),
                    ("CompareEndpoints", 19),
                    ("ExpandToEnclosingUnit", 21),
                    ("GetAttributeValue", 1),
                    ("GetText", 20),
                    ("Move", 21),
                    ("MoveEndpointByRange", 22),
                ],
            ),
        ]
    };
    ratchet.check(
        &format!("UIA say-all first batch of twenty lines, {way}"),
        &first,
        expected[0].0,
        0,
        expected[0].1,
    );
    ratchet.check(
        &format!("UIA say-all later batch of twenty lines, {way}"),
        &second,
        expected[1].0,
        0,
        expected[1].1,
    );
    app.quit();
}

/// A say-all batch whose lines are in three languages, over mockapp's
/// `languages.json`: the one `Culture` read over the batch answers "mixed",
/// so each line's is read, remotely or classically.
fn check_uia_say_all_language_costs(ratchet: &mut Ratchet, remote: bool) {
    let way = if remote { "remotely" } else { "classically" };
    let title = common::unique_title("mockapp-counts-uia-languages");
    let mut app = common::spawn("languages.json", "uia", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "caret doc 0");
    let mut source = uia_notes(hwnd).remote(remote);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    let (reply, mixed) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::ReadAhead(TextReadAhead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
            count: 20,
        }),
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    let languages: Vec<Vec<&str>> = chunks
        .iter()
        .map(|chunk| {
            chunk
                .languages
                .iter()
                .map(|run| run.language.as_str())
                .collect()
        })
        .collect();
    // The empty last line has no text for a language to cover.
    assert_eq!(
        languages,
        [vec!["en-US"], vec!["fr-FR"], vec!["de-DE"], Vec::new()]
    );
    // The batch's one read and a read for each of its four lines.
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 8),
                ("CompareEndpoints", 4),
                ("ExpandToEnclosingUnit", 4),
                ("GetAttributeValue", 5),
                ("GetText", 5),
                ("Move", 4),
                ("MoveEndpointByRange", 7),
            ],
        )
    } else {
        (
            calls(36, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("Clone", 7),
                ("CompareEndpoints", 4),
                ("ExpandToEnclosingUnit", 4),
                ("GetAttributeValue", 5),
                ("GetText", 5),
                ("Move", 4),
                ("MoveEndpointByRange", 6),
            ],
        )
    };
    ratchet.check(
        &format!("UIA say-all batch in three languages, {way}"),
        &mixed,
        expected.0,
        0,
        expected.1,
    );
    app.quit();
}

fn uia_say_all_batches_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for remote in [true, false] {
        check_uia_say_all_batch_costs(&mut ratchet, remote);
        check_uia_say_all_language_costs(&mut ratchet, remote);
    }
    ratchet.finish();
}

/// An operation's expected calls and provider hits.
type Expected = (CallCounts, &'static [(&'static str, u32)]);

/// The report after a focus on a line whose one format stretch reads as
/// "mixed" for italics (mockapp's `italic.json`, "plain italic text"):
/// read again by its four words, and the mixed word "italic " by its seven
/// characters, ten stretches in all, with every formatting indication on,
/// remotely and classically.
fn uia_mixed_stretch_costs_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    let title = common::unique_title("mockapp-counts-uia-mixed");
    let mut app = common::spawn("italic.json", "uia", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "caret doc 0");
    for remote in [true, false] {
        let mut source = uia_notes(hwnd).remote(remote).fetches(Fetches::default());
        let mut store = Anchors::new(Arc::default());
        let _ = verbatim_uia::calls::take();
        let _ = verbatim_uia::calls::take_failed_reads();
        common::reset_hits(hwnd);
        let (report, _) =
            caret_report(&mut source, &mut store.node(1), &mut || 0, true).expect("the caret");
        let cost = Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
            failed_reads: verbatim_uia::calls::take_failed_reads(),
        };
        let italics: Vec<(u32, u32, Option<bool>)> = report
            .line
            .formats
            .iter()
            .map(|run| (run.start, run.end, run.attributes.italic))
            .collect();
        let mut expected = vec![(0, 6, Some(false))];
        expected.extend((6..12).map(|at| (at, at + 1, Some(true))));
        expected.extend([
            (12, 13, Some(false)),
            (13, 17, Some(false)),
            (17, 18, Some(false)),
        ]);
        assert_eq!(italics, expected);
        // The line's annotation types, which it has none of, and the ten
        // other attributes, being learned; and twelve stretches walked (the
        // line's one, its four words, the mixed word's seven characters),
        // each a copy, an end moved one unit on, a comparison with the
        // span's end, and its attributes but the annotation types, ten; the
        // ten appended also read their text.
        // Before the line's annotation types were asked first, each
        // stretch read them too, and there were seven attributes in all
        // (84 reads, 82 calls classically). Before the text range audit of
        // 2026-10-07 the line was one stretch whose italics were none.
        let (way, expected): (&str, Expected) = if remote {
            (
                "remotely",
                (
                    calls(1, 0, 0),
                    &[
                        ("ProviderOptions", 2),
                        ("GetPatternProvider", 1),
                        ("GetPropertyValue", 1),
                        ("HostRawElementProvider", 1),
                        ("Navigate", 1),
                        ("ITextProvider::GetSelection", 1),
                        ("Clone", 18),
                        ("CompareEndpoints", 13),
                        ("ExpandToEnclosingUnit", 1),
                        ("GetAttributeValue", 131),
                        ("GetText", 12),
                        ("MoveEndpointByUnit", 12),
                        ("MoveEndpointByRange", 15),
                    ],
                ),
            )
        } else {
            (
                "classically",
                (
                    calls(83, 0, 0),
                    &[
                        ("ITextProvider::GetSelection", 1),
                        ("Clone", 17),
                        ("CompareEndpoints", 13),
                        ("ExpandToEnclosingUnit", 1),
                        ("GetAttributeValue", 131),
                        ("GetText", 12),
                        ("MoveEndpointByUnit", 12),
                        ("MoveEndpointByRange", 14),
                    ],
                ),
            )
        };
        ratchet.check(
            &format!("UIA caret report after a focus on a mixed stretch, {way}"),
            &cost,
            expected.0,
            0,
            expected.1,
        );
    }
    app.quit();
    ratchet.finish();
}

fn caret_moves_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for remote in [true, false] {
        check_uia_caret_costs(&mut ratchet, remote);
    }

    let title = common::unique_title("mockapp-counts-edit-caret");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    let mut source = EditText::new(edit.0 as isize, 0);
    let (answer, report, selected, before_key) =
        measure_caret_move(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    // The answer reads the selection again (`EM_GETSEL`), so a caret read
    // before the key's effect is never paired with a line read after it.
    ratchet.check("Edit control caret move", &answer, calls(0, 0, 6), 0, &[]);
    ratchet.check("Edit control caret report", &report, calls(0, 0, 5), 0, &[]);
    // The caret report's 5.
    ratchet.check(
        "Edit control caret read before a held key",
        &before_key,
        calls(0, 0, 5),
        0,
        &[],
    );
    // The report's 5 and the change's text (`WM_GETTEXTLENGTH`,
    // `WM_GETTEXT`); the comparisons are of offsets, with no message.
    ratchet.check(
        "Edit control caret report with a selection change",
        &selected,
        calls(0, 0, 7),
        0,
        &[],
    );
    // A caret move's messages for each check; 55 for the wait that found
    // nothing before 2026-10-08.
    let (nothing, later) = measure_watch(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    ratchet.check(
        "Edit control caret watch finding nothing",
        &nothing,
        calls(0, 0, 5),
        0,
        &[],
    );
    ratchet.check(
        "Edit control caret watch answered by a later caret event",
        &later,
        calls(0, 0, 6),
        0,
        &[],
    );

    // The review copy's first press: "alpha" selected from a collapsed
    // caret, the selection read before and after and compared.
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(&mut app, hwnd, "caret doc 0");
    let (report, _) = caret_report(&mut source, &mut anchors, &mut || 0, false).expect("the caret");
    let at = |offset| {
        TextPoint::At(TextPosition {
            anchor: report.line.start,
            offset,
        })
    };
    let _ = verbatim_ia2::calls::take();
    let reply = perform(
        &mut source,
        &mut anchors,
        &TextOp::Select {
            start: at(0),
            end: at(4),
            through: Some(TextThrough {
                start: at(1),
                end: at(5),
            }),
        },
    );
    let measured = verbatim_ia2::calls::take();
    assert_eq!(selected_texts(&reply), [(true, "alpha", 5)]);
    // `EM_GETSEL`, `EM_SETSEL` and `EM_SCROLLCARET`, `EM_GETSEL` again,
    // and the change's text (`WM_GETTEXTLENGTH`, `WM_GETTEXT`).
    ratchet.check_calls("Edit control review copy select", measured, calls(0, 0, 6));
    app.quit();

    ratchet.finish();
}

/// A caret key's watch checked as the worker checks it: when the request
/// arrives the key has moved nothing yet, so the check's one read finds no
/// evidence and the watch stays open, with no wait; then the application
/// moves the caret one character on and reports it, and the check the
/// caret event prompts answers the key with the character there. Returns
/// the calls and hits of each check.
fn measure_watch<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    take: fn() -> CallCounts,
) -> (Cost, Cost) {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 1");
    let (before, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    let watch = right_arrow(&before);
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    common::reset_hits(hwnd);
    let mut check = Check::default();
    let watched = check_caret(source, &mut anchors, &watch, &mut check);
    assert!(matches!(watched, Watched::Watching), "{watched:?}");
    let nothing = Cost {
        calls: take(),
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };

    common::apply(app, hwnd, "caret doc 2");
    let _ = take();
    let _ = verbatim_uia::calls::take_failed_reads();
    check.caret_event = true;
    let reply = answered(check_caret(source, &mut anchors, &watch, &mut check));
    assert!(reply.moved);
    assert_eq!(reply.unit.expect("the character").text, "p");
    assert_eq!(check.reads, 2, "one read for each check");
    let later = Cost {
        calls: take(),
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads(),
    };
    (nothing, later)
}

/// The text of mockapp's "Notes" document through UIA, as the outpost's
/// worker builds it from the node's element and its text patterns, with
/// the default theme's formatting: spelling and grammar errors.
fn uia_notes(hwnd: HWND) -> UiaText {
    let under_test = UiaUnderTest::new(hwnd);
    let notes = under_test.element("Notes").clone();
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, pattern2, false).fetches(Theme::builtin_default().fetches())
}

/// A focus onto a combo box without the `Value` pattern, whose first
/// selected item is its value, and the selection of another of its items,
/// a change of that value, through an outpost reading with remote
/// operations or the classic walk as `remote` says
/// (`docs/performance.md`, "A focus change onto a combo box without the
/// Value pattern, UIA"). The first focus in the window, on the combo box
/// with the pattern, probes it for arbitration and is not measured. The
/// listener reads the focus the classic way whatever the outpost's path:
/// the five calls of a steady-state focus and the selected item's three
/// through the `Selection` pattern. The selection is two calls in the
/// outpost: the test for a selection in a list the focus controls, which
/// every selection is given first, as NVDA's `event_selection` reads its
/// focus's `ControllerFor` first, one remote operation or the relation's
/// read, and one `FindFirstBuildCache` over the combo box's children.
///
/// The selected item is one UIA has never seen when the outpost searches
/// for it, so its cost holds whatever time passes before the search: UIA
/// keeps state for an element it has handed out for about 500 ms, and a
/// search that meets an element whose state has lapsed asks the provider 6
/// calls more. So the test searches for no element below the window's
/// children before the focus onto the combo box, neither focus's search
/// passes by the combo box's items, and mockapp raises no selection event,
/// whose raising would hand the item out to the outpost's registrations;
/// the outpost reads the item's name and keyboard focus, never its
/// selected state. The fact's runtime id is made rather than read: the
/// item's is the combo box's with its last part the item's place in the
/// fixture. The combo box is the focus the outpost has just read, as it
/// is for a selection that follows its focus within that time; one that
/// comes later meets the combo box's lapsed state, 6 provider calls more.
fn uia_combo_box_costs(remote: bool, expected: [(CallCounts, &[(&str, u32)]); 2]) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-combo-box-{path}"));
    let mut app = common::spawn("combo_box.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::without_elements();
    let outpost = OutpostUnderTest::on_frozen_time_with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    let mut ratchet = Ratchet::default();
    app.send("set-focus size");
    let size = under_test.focused_child(hwnd);
    assert_eq!(size.read().name.as_deref(), Some("Size"));
    let reported = outpost.uia_focus(&size);
    assert_eq!(reported.node.value.as_deref(), Some("Large"));
    // The combo box is the window's first child, so the search over the
    // window's descendants finds it before any item below it.
    app.send("set-focus hour");
    let hour = under_test.focused(hwnd);
    assert_eq!(hour.read().name.as_deref(), Some("Hour"));
    common::reset_hits(hwnd);
    let reported = outpost.uia_focus(&hour);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
        failed_reads: reported.failed_reads,
    };
    assert_eq!(reported.node.value.as_deref(), Some("Nine"));
    assert_eq!(reported.selected_child, None);
    let [focus, selection] = expected;
    ratchet.check(
        &format!("UIA focus onto a combo box without the Value pattern ({path})"),
        &cost,
        focus.0,
        0,
        focus.1,
    );
    // Core writes the nodes it holds after the report: the outpost releases
    // the combo box it reported before, whose relation a selection would
    // otherwise be read against too.
    outpost.release_to_core_line();
    outpost.settled();
    // "Ten" is the fixture's third node below the window, after "Hour"
    // and "Nine".
    let mut ten = verbatim_uia::runtime_id(hour.live().element());
    *ten.last_mut().expect("the combo box's runtime id") = 3;
    common::reset_hits(hwnd);
    outpost.deliver(DeliveredFact::UiaSelection {
        hwnd: hwnd.0 as isize,
        focus_window: hwnd.0 as isize,
        snapshot: verbatim_model::protocol::UiaSnapshotFact {
            runtime_id: ten,
            role: Role::ListItem,
            name: Some("Ten".to_owned()),
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
    });
    let calls = match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::ValueChanged { value, .. },
            timing,
            ..
        } => {
            assert_eq!(value.as_deref(), Some("Ten"));
            timing.calls
        }
        other => panic!("the outpost said {other:?}, not the combo box's new value"),
    };
    outpost.settled();
    let cost = Cost {
        calls,
        hits: common::read_hits(hwnd),
        failed_reads: outpost.take_failed_reads(),
    };
    ratchet.check(
        &format!("UIA selection in a combo box without the Value pattern ({path})"),
        &cost,
        selection.0,
        0,
        selection.1,
    );
    ratchet.finish();
    drop(outpost);
    app.quit();
}

fn uia_combo_box_costs_exactly() {
    uia_combo_box_costs(
        true,
        [
            (
                calls(7, 0, 0),
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 65),
                    ("GetPatternProvider", 70),
                    ("GetPropertyValue", 112),
                    ("HostRawElementProvider", 31),
                    ("Navigate", 16),
                    ("GetRuntimeId", 11),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 20),
                    ("ExpandCollapseState", 2),
                    ("IsSelected", 1),
                    ("GetSelection", 1),
                ],
            ),
            (
                calls(2, 0, 0),
                &[
                    ("ProviderOptions", 20),
                    ("GetPropertyValue", 11),
                    ("HostRawElementProvider", 11),
                    ("Navigate", 5),
                    ("GetRuntimeId", 6),
                    ("FragmentRoot", 8),
                ],
            ),
        ],
    );
    uia_combo_box_costs(
        false,
        [
            (
                calls(7, 0, 0),
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 64),
                    ("GetPatternProvider", 70),
                    ("GetPropertyValue", 111),
                    ("HostRawElementProvider", 29),
                    ("Navigate", 13),
                    ("GetRuntimeId", 10),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 20),
                    ("ExpandCollapseState", 2),
                    ("IsSelected", 1),
                    ("GetSelection", 1),
                ],
            ),
            (
                calls(2, 0, 0),
                &[
                    ("ProviderOptions", 18),
                    ("GetPropertyValue", 10),
                    ("HostRawElementProvider", 11),
                    ("Navigate", 4),
                    ("GetRuntimeId", 6),
                    ("FragmentRoot", 9),
                ],
            ),
        ],
    );
}

/// A UIA focus on a text field, read by the listener, through an outpost
/// reading with remote operations or the classic walk as `remote` says:
/// the field `id` of `fixture`, the first focus in its window. Its caret
/// is read with it, so the one focus message carries every call the focus
/// costs: the listener's read, the outpost's own element found by the
/// listener's path, the focus's enrichment, and the caret's read. Each of
/// `setup`'s mockapp commands is sent before the focus. The outpost judges
/// the window first, at a foreground change it drops as UIA's, so the
/// focus's calls hold no probe, which a setup command's event would
/// otherwise make first or not, as its event reaches the outpost before
/// the focus or after it; the outpost's probe at a cold focus is pinned by
/// [`uia_focus_changes`], and the listener makes none.
fn uia_text_focus(fixture: &str, id: &str, remote: bool, setup: &[&str]) -> Reported {
    let title = common::unique_title("mockapp-counts-text-focus");
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time_with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.settled();
    for command in setup {
        app.send(command);
    }
    app.send(&format!("set-focus {id}"));
    let uia = Uia::new().expect("a UIA client");
    let reported = outpost.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    drop(outpost);
    app.quit();
    reported
}

/// The calls a focus on a text field costs, its caret read with it and
/// counted in the focus's one message (`docs/performance.md`, "A focus on
/// a text field"): a UIA document with nothing selected, with a selection,
/// whose text is read with the caret, and protected with a selection,
/// whose text is read as any field's, and a terminal, each remotely and
/// classically, and a classic edit control through MSAA, and the
/// focus-now answer on the UIA document ([`text_field_focus_now_costs`]).
/// Each is the first focus in its window, so a UIA focus makes the one
/// connection that finds the outpost's own element.
#[expect(
    clippy::too_many_lines,
    reason = "every text field focus and its pinned counts, listed in full"
)]
fn text_field_focuses_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    let with_connection = |uia| CallCounts {
        connections: 1,
        ..calls(uia, 0, 0)
    };
    let selected: &[&str] = &["caret doc 0 5"];
    let protected_selected: &[&str] = &["set-states doc focusable protected", "caret doc 0 5"];
    for (operation, fixture, id, remote, setup, selected_text, expected) in [
        (
            "UIA text field focus with its caret, remotely",
            "text.json",
            "doc",
            true,
            &[][..],
            None,
            with_connection(9),
        ),
        (
            "UIA text field focus with its caret, classically",
            "text.json",
            "doc",
            false,
            &[][..],
            None,
            with_connection(45),
        ),
        (
            "UIA text field focus with a selection, remotely",
            "text.json",
            "doc",
            true,
            selected,
            Some("alpha"),
            with_connection(10),
        ),
        // 50 before 2026-10-22, when the caret read began reading the
        // selection's text to keep with it for the next comparison, and 51
        // before it read where the selection starts, here the text's start
        // (six calls; four for a start inside a line).
        (
            "UIA text field focus with a selection, classically",
            "text.json",
            "doc",
            false,
            selected,
            Some("alpha"),
            with_connection(57),
        ),
        // A protected field's selected text is read as any field's, which
        // Core says masked as the control gives it, so its focus costs
        // what an unprotected field's with a selection does.
        (
            "UIA protected text field focus with a selection, remotely",
            "text.json",
            "doc",
            true,
            protected_selected,
            Some("alpha"),
            with_connection(10),
        ),
        (
            "UIA protected text field focus with a selection, classically",
            "text.json",
            "doc",
            false,
            protected_selected,
            Some("alpha"),
            with_connection(57),
        ),
        (
            "UIA terminal focus with its caret, remotely",
            "terminal_reading.json",
            "term",
            true,
            &[][..],
            None,
            with_connection(9),
        ),
        (
            "UIA terminal focus with its caret, classically",
            "terminal_reading.json",
            "term",
            false,
            &[][..],
            None,
            with_connection(27),
        ),
    ] {
        let reported = uia_text_focus(fixture, id, remote, setup);
        let Some(FocusText::Caret {
            selected_text: read,
            ..
        }) = &reported.text
        else {
            panic!(
                "{operation}: the focus came without its caret: {:?}",
                reported.text
            );
        };
        assert_eq!(
            read.as_deref(),
            selected_text,
            "{operation}: the selected text"
        );
        ratchet.check_calls(operation, reported.calls, expected);
    }

    let title = common::unique_title("mockapp-counts-edit-focus");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The keyboard focus cannot be given to the control on a desktop of
    // its own, so its host has the focused state, which an MSAA focus
    // needs on the object or an ancestor.
    app.send("set-focus root");
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    let reported = outpost.focus(DeliveredFact::MsaaFocus {
        hwnd: edit.0 as isize,
        id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        id_child: 0,
    });
    assert!(
        matches!(reported.text, Some(FocusText::Caret { .. })),
        "the edit control's focus came without its caret: {:?}",
        reported.text
    );
    ratchet.check_calls(
        "edit control focus with its caret",
        reported.calls,
        calls(0, 40, 6),
    );
    drop(outpost);
    app.quit();
    text_field_focus_now_costs(&mut ratchet);
    ratchet.finish();
}

/// The calls a focus-now query on a text field costs, the answer Core
/// asks for at its start, after a replaced outpost, and after a menu
/// closes or Alt+Tab (`docs/performance.md`, "The focus-now answer on a
/// text field"): mockapp's UIA document with nothing selected, remotely
/// and classically, its caret read with the focus by the reported
/// focus's reading code and carried in the answer, its line "alpha
/// beta". The caret read adds what it adds to a reported focus's
/// read, measured in both: the text pattern's fetch and one program
/// remotely, 2, and the fetch and 35 calls classically, 36, to the
/// query's 2 UIA calls and 1 window message without it.
fn text_field_focus_now_costs(ratchet: &mut Ratchet) {
    for (operation, remote, expected) in [
        (
            "UIA text field focus-now answer with its caret, remotely",
            true,
            calls(4, 0, 1),
        ),
        (
            "UIA text field focus-now answer with its caret, classically",
            false,
            calls(40, 0, 1),
        ),
    ] {
        let title = common::unique_title("mockapp-counts-text-focus-now");
        let mut app = common::spawn("text.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let mut outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        app.send("set-focus doc");
        let uia = Uia::new().expect("a UIA client");
        let focused = common::listener_focus(&uia, hwnd.0 as isize);
        outpost.read_focus_as(focused.live().element());
        let (now, measured) = outpost.focus_now_with_calls();
        let now = now.unwrap_or_else(|| panic!("{operation}: the focus now has a focused control"));
        let Some(FocusText::Caret {
            caret,
            selected_text: None,
        }) = now.text.as_deref()
        else {
            panic!(
                "{operation}: the answer came without its caret: {:?}",
                now.text
            );
        };
        assert_eq!(
            (caret.line.text.as_str(), caret.line.offset),
            ("alpha beta\n", 0),
            "{operation}: the caret's line"
        );
        ratchet.check_calls(operation, measured, expected);
        drop(outpost);
        app.quit();
    }
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    harness::run_isolated(&[
        (
            "a_window_above_the_process_boundary_is_a_plain_window_with_no_call",
            a_window_above_the_process_boundary_is_a_plain_window_with_no_call,
        ),
        (
            "the_focused_state_walk_makes_no_call_above_the_process_boundary",
            the_focused_state_walk_makes_no_call_above_the_process_boundary,
        ),
        (
            "a_parent_step_into_another_process_is_not_read_further",
            a_parent_step_into_another_process_is_not_read_further,
        ),
        (
            "an_acc_focus_served_by_another_process_answers_no_focused_control",
            an_acc_focus_served_by_another_process_answers_no_focused_control,
        ),
        (
            "focus_now_onto_a_hung_window_answers_its_plain_window",
            focus_now_onto_a_hung_window_answers_its_plain_window,
        ),
        ("caret_moves_cost_exactly", caret_moves_cost_exactly),
        (
            "an_outposts_probe_that_finds_a_provider_makes_no_connection",
            an_outposts_probe_that_finds_a_provider_makes_no_connection,
        ),
        (
            "uia_mixed_stretch_costs_exactly",
            uia_mixed_stretch_costs_exactly,
        ),
        (
            "uia_text_requests_cost_exactly",
            uia_text_requests_cost_exactly,
        ),
        (
            "uia_say_all_batches_cost_exactly",
            uia_say_all_batches_cost_exactly,
        ),
        (
            "msaa_focus_changes_cost_exactly",
            msaa_focus_changes_cost_exactly,
        ),
        (
            "msaa_navigation_steps_cost_exactly",
            msaa_navigation_steps_cost_exactly,
        ),
        (
            "msaa_dialog_text_costs_exactly",
            msaa_dialog_text_costs_exactly,
        ),
        (
            "msaa_state_change_costs_exactly",
            msaa_state_change_costs_exactly,
        ),
        ("msaa_tree_view_costs_exactly", msaa_tree_view_costs_exactly),
        (
            "msaa_form_focus_costs_exactly",
            msaa_form_focus_costs_exactly,
        ),
        ("msaa_list_view_costs_exactly", msaa_list_view_costs_exactly),
        (
            "uia_dialog_text_costs_exactly",
            uia_dialog_text_costs_exactly,
        ),
        (
            "uia_dialog_window_text_reads_nothing_of_its_title_bar",
            uia_dialog_window_text_reads_nothing_of_its_title_bar,
        ),
        (
            "uia_focus_changes_cost_exactly_remote",
            uia_focus_changes_cost_exactly_remote,
        ),
        (
            "uia_focus_changes_cost_exactly_classic",
            uia_focus_changes_cost_exactly_classic,
        ),
        (
            "uia_repeated_focus_costs_exactly",
            uia_repeated_focus_costs_exactly,
        ),
        ("uia_combo_box_costs_exactly", uia_combo_box_costs_exactly),
        (
            "uia_focus_chains_stop_below_the_process_boundary",
            uia_focus_chains_stop_below_the_process_boundary,
        ),
        (
            "uia_walk_from_a_window_element_held_without_its_handle_stops_below_the_boundary",
            uia_walk_from_a_window_element_held_without_its_handle_stops_below_the_boundary,
        ),
        (
            "uia_focus_chains_name_a_parent_with_a_provider_as_a_plain_window",
            uia_focus_chains_name_a_parent_with_a_provider_as_a_plain_window,
        ),
        (
            "the_listener_makes_no_connection",
            the_listener_makes_no_connection,
        ),
        (
            "uia_navigation_steps_cost_exactly",
            uia_navigation_steps_cost_exactly,
        ),
        (
            "uia_ancestors_with_descriptions_off_cost_exactly",
            uia_ancestors_with_descriptions_off_cost_exactly,
        ),
        (
            "a_node_with_no_kept_element_and_no_route_is_gone_with_no_call",
            a_node_with_no_kept_element_and_no_route_is_gone_with_no_call,
        ),
        (
            "uia_event_registrations_cost_exactly",
            uia_event_registrations_cost_exactly,
        ),
        (
            "uia_selected_children_cost_exactly",
            uia_selected_children_cost_exactly,
        ),
        (
            "uia_active_text_position_costs_exactly",
            uia_active_text_position_costs_exactly,
        ),
        (
            "uia_event_caches_cost_exactly",
            uia_event_caches_cost_exactly,
        ),
        (
            "text_field_focuses_cost_exactly",
            text_field_focuses_cost_exactly,
        ),
    ]);
}
