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
//! so every probe here is made where the test says, whatever the test's own
//! speed, and every `WM_GETOBJECT` is pinned.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, mpsc};

use verbatim_model::{
    CallCounts, Fetches, NodeSnapshot, NormalizedEvent, QueryKind, Role, TreeNode, WindowHandle,
};
use verbatim_model::{
    CaretWatch, LineStyle, PreviousSelection, TextAttributes, TextMovement, TextOp, TextPoint,
    TextPosition, TextRead, TextReadAhead, TextReply, TextUnit, Theme,
};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::arbitration::NEGATIVE_VERDICT_LIFETIME;
use verbatim_outpost::dialog_text::{UiaObject, dialog_text};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, SupervisorToOutpost};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{
    Anchors, CaretSignal, TextSource, Watched, caret_report, check_caret, perform,
};
use verbatim_uia::map::snapshot_from_cached_element;
use verbatim_uia::{
    CACHED_PROPERTIES, ElementExt, FOCUS_PROPERTIES, NodeIdRegistry, Registration, Scope,
    Subscription, Uia,
};
use verbatim_uia_rops::{Attributes, FocusAncestry, FocusQuery, TextAttribute, focus_ancestry};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, TreeScope_Descendants,
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

/// The question the dialog fixture's message box asks.
const QUESTION: &str = "Remove the theme Mine? This cannot be undone.";

/// Calls by kind, in the order `CallCounts` lists them.
fn calls(uia: u32, msaa: u32, window_messages: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa,
        window_messages,
    }
}

/// What one operation cost: the client's calls and the providers' hits.
struct Cost {
    calls: CallCounts,
    hits: Vec<(&'static str, u32)>,
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
        expected_hits: &[(&str, u32)],
    ) {
        if cost.calls != expected_calls || cost.hits != expected_hits {
            self.mismatches.push(format!(
                "{operation}: measured {:?} and hits {:?}",
                cost.calls, cost.hits
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
    let outpost = OutpostUnderTest::new(app.pid());
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
        calls(0, 30, 1),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );

    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("second", SECOND));
    assert_eq!(reported.chain(), [Some("Settings"), Some("Second")]);
    assert_eq!(reported.selected_child, None);
    assert_eq!(reported.window, window);
    ratchet.check(
        "MSAA focus, steady state",
        &cost,
        calls(0, 30, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
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
        calls(0, 30, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 7),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accSelection", 1),
            ("accLocation", 3),
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
        calls(0, 30, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 3),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
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
        calls(0, 33, 1),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );
    let (reported, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    ratchet.check(
        "MSAA arrow to the next list item, the probe kept",
        &cost,
        calls(0, 33, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 3),
        ],
    );

    // The same focus event again: NVDA drops a focus event naming the
    // address of the focus it last queued before reading anything of it
    // but the role its class choice needs, and the outpost says nothing.
    // With nothing published, the calls are known only as mockapp's hits.
    common::reset_hits(hwnd);
    outpost.deliver(msaa_focus_fact(hwnd, ITEM_TWO));
    outpost.settled();
    ratchet.check(
        "MSAA focus repeated",
        &Cost {
            calls: CallCounts::default(),
            hits: common::read_hits(hwnd),
        },
        CallCounts::default(),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accRole", 1),
            ("accFocus", 1),
        ],
    );

    // A focus event on an object that neither has the focused state nor
    // is inside one that has: NVDA reads the states, the object's and then
    // each ancestor's, and goes no further, and the outpost says nothing:
    // again only mockapp's hits are known.
    common::reset_hits(hwnd);
    outpost.deliver(msaa_focus_fact(hwnd, FIRST));
    outpost.settled();
    ratchet.check(
        "MSAA focus without the focused state",
        &Cost {
            calls: CallCounts::default(),
            hits: common::read_hits(hwnd),
        },
        CallCounts::default(),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 6),
            ("get_accChild", 1),
            ("get_accRole", 1),
            ("get_accState", 3),
            ("accFocus", 1),
        ],
    );

    // A value change on an object that is not the focus is told from the
    // focus by its identity, and only its role is read, to know whether it
    // is a progress bar, whose changes are reported off the focus too: its
    // acquisition and role are counted before the focus's own value
    // change, which comes after it from the same hook and is read and
    // reported.
    common::reset_hits(hwnd);
    app.send("set-value first Changed");
    app.send("set-value item2 Picked");
    let calls_made = match outpost.next() {
        OutpostToSupervisor::Event {
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
        },
        calls(0, 11, 0),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 2),
            ("get_accChild", 2),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accDescription", 1),
            ("get_accRole", 3),
            ("get_accState", 1),
            ("get_accKeyboardShortcut", 1),
            ("accLocation", 1),
        ],
    );

    ratchet.finish();
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
    let outpost = OutpostUnderTest::new(app.pid());
    let mut ratchet = Ratchet::default();

    common::apply(&mut app, hwnd, "set-focus yes");
    let reported = outpost.msaa_focus(hwnd, YES);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
    };
    assert_eq!(reported.node.name.as_deref(), Some("Yes"));
    assert_eq!(dialog_description(&reported.ancestors), Some(QUESTION));
    ratchet.check(
        "MSAA focus into a message box",
        &cost,
        calls(0, 44, 1),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 17),
            ("accChildCount", 2),
            ("get_accChild", 4),
            ("get_accName", 4),
            ("get_accValue", 4),
            ("get_accDescription", 4),
            ("get_accRole", 6),
            ("get_accState", 6),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accLocation", 3),
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
fn msaa_tree_view_costs_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-tree-view");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());
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
            OutpostToSupervisor::Event {
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
    ratchet.check_calls("MSAA tree view item expanded", calls_made, calls(0, 13, 7));
    let (count, states, calls_made) = state_change(TVE_COLLAPSE.0);
    assert!(states.contains(verbatim_model::State::Collapsed));
    assert_eq!(count, None, "nothing is counted for a collapse");
    ratchet.check_calls("MSAA tree view item collapsed", calls_made, calls(0, 13, 2));

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
    let outpost = OutpostUnderTest::new(app.pid());
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
        calls(0, 29, 14),
    );
    ratchet.finish();
    app.quit();
}

/// A UIA message box's text, gathered as the outpost's worker gathers it
/// for a dialog the focus newly entered (`describe_dialogs` in
/// `verbatim-outpost`'s `read.rs`): one call reads the dialog's children
/// with their properties cached.
fn uia_dialog_text_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-dialog");
    let app = common::spawn("dialog.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let dialog = under_test.element("Remove Theme").clone();
    let (text, cost) = under_test.measure(hwnd, |under_test| {
        dialog_text(&UiaObject::new(&under_test.uia, CACHED_PROPERTIES, dialog))
    });
    assert_eq!(text.as_deref(), Some(QUESTION));
    ratchet.check(
        "UIA message box text",
        &cost,
        calls(1, 0, 0),
        &[
            ("ProviderOptions", 29),
            ("GetPatternProvider", 44),
            ("GetPropertyValue", 92),
            ("HostRawElementProvider", 13),
            ("Navigate", 9),
            ("GetRuntimeId", 8),
            ("BoundingRectangle", 4),
            ("FragmentRoot", 9),
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
    let mut outpost = OutpostUnderTest::new(app.pid());
    let mut ratchet = Ratchet::default();
    let (first, _) = measure_msaa_focus(&mut app, hwnd, &outpost, ("first", FIRST));
    let first = first.node;

    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.id, QueryKind::NextSibling);
    assert_eq!(second.name.as_deref(), Some("Second"));
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
    };
    ratchet.check(
        "MSAA next sibling",
        &cost,
        calls(0, 10, 0),
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
    };
    ratchet.check(
        "MSAA parent",
        &cost,
        calls(0, 10, 0),
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
        .handle_command(&SupervisorToOutpost::Fetches(Fetches {
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
    };
    ratchet.check(
        "MSAA next sibling, descriptions and shortcuts off",
        &cost,
        calls(0, 8, 0),
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
/// tree holds, by name, for the operations to start from.
struct UiaUnderTest {
    uia: Uia,
    cache: IUIAutomationCacheRequest,
    registry: NodeIdRegistry,
    elements: HashMap<String, IUIAutomationElement>,
}

impl UiaUnderTest {
    fn new(hwnd: HWND) -> Self {
        let uia = Uia::new().expect("a UIA client");
        let cache = uia.base_cache_request().expect("a cache request");
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let root = uia
            .element_from_handle(hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        let (tree, _) = uia
            .walk_tree(&root, &cache, &registry, 8, 64)
            .expect("mockapp's tree");
        let mut elements = HashMap::new();
        collect(&tree, &registry, &mut elements);
        Self {
            uia,
            cache,
            registry,
            elements,
        }
    }

    fn element(&self, name: &str) -> &IUIAutomationElement {
        &self.elements[name]
    }

    /// Runs `operation` on this thread and measures it.
    fn measure<T>(&self, hwnd: HWND, operation: impl FnOnce(&Self) -> T) -> (T, Cost) {
        let _ = verbatim_uia::calls::take();
        common::reset_hits(hwnd);
        let result = operation(self);
        let cost = Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
        };
        (result, cost)
    }

    /// The element mockapp reports as having the keyboard focus, found
    /// under its window by that property and built with the listener's
    /// cache request, as a focus event's element arrives: what the system's
    /// focused element would be if mockapp had the keyboard focus.
    fn focused(&self, hwnd: HWND) -> IUIAutomationElement {
        let root = self
            .uia
            .element_from_handle(hwnd.0 as isize, &self.cache)
            .expect("mockapp's root element");
        let condition = self
            .uia
            .property_condition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
            .expect("a condition");
        root.find_first_build_cache(TreeScope_Descendants, &condition, &self.cache)
            .expect("the search")
            .expect("mockapp reports a focused element")
    }
}

/// Every named node's live element, from the registry the walk filled.
fn collect(
    node: &TreeNode,
    registry: &NodeIdRegistry,
    elements: &mut HashMap<String, IUIAutomationElement>,
) {
    if let Some(name) = &node.snapshot.name
        && let Some(element) = registry
            .element_of(node.snapshot.id)
            .and_then(|agile| agile.resolve().ok())
    {
        elements.insert(name.clone(), element);
    }
    for child in &node.children {
        collect(child, registry, elements);
    }
}

/// Moves mockapp's focus to `id`, finds the element mockapp then reports
/// focused, which must be the one named `name`, hands the outpost the
/// focus, and measures the outpost's handling of it.
fn measure_uia_focus(
    (app, under_test, hwnd): (&mut common::MockApp, &UiaUnderTest, HWND),
    outpost: &OutpostUnderTest,
    (id, name): (&str, &str),
) -> (Reported, Cost) {
    app.send(&format!("set-focus {id}"));
    let focused = under_test.focused(hwnd);
    assert_eq!(
        snapshot_from_cached_element(&focused, &under_test.registry)
            .name
            .as_deref(),
        Some(name),
        "the element mockapp reports focused"
    );
    common::reset_hits(hwnd);
    let reported = outpost.uia_focus(&focused);
    let cost = Cost {
        calls: reported.calls,
        hits: common::read_hits(hwnd),
    };
    (reported, cost)
}

/// What each UIA focus change in [`uia_focus_changes`] is expected to cost.
struct FocusCosts<'a> {
    cold: (CallCounts, &'a [(&'a str, u32)]),
    steady: (CallCounts, &'a [(&'a str, u32)]),
    into_list: (CallCounts, &'a [(&'a str, u32)]),
    next_item: (CallCounts, &'a [(&'a str, u32)]),
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
    let outpost = OutpostUnderTest::with_options(
        app.pid(),
        OutpostOptions {
            remote_operations: remote,
        },
    );
    let mut ratchet = Ratchet::default();
    let check = |ratchet: &mut Ratchet,
                 label: &str,
                 cost: &Cost,
                 expected: &(CallCounts, &[(&str, u32)])| {
        ratchet.check(
            &format!("UIA focus ({path}), {label}"),
            cost,
            expected.0,
            expected.1,
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
fn uia_focus_changes_cost_exactly_remote() {
    uia_focus_changes(
        true,
        &FocusCosts {
            cold: (
                calls(2, 0, 1),
                &[
                    ("WM_GETOBJECT", 4),
                    ("ProviderOptions", 36),
                    ("GetPatternProvider", 22),
                    ("GetPropertyValue", 50),
                    ("HostRawElementProvider", 14),
                    ("Navigate", 11),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 6),
                ],
            ),
            steady: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 22),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 10),
                    ("Navigate", 7),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 6),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`.
            into_list: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 2),
                    ("ProviderOptions", 24),
                    ("GetPatternProvider", 23),
                    ("GetPropertyValue", 49),
                    ("HostRawElementProvider", 11),
                    ("Navigate", 8),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 5),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 22),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 10),
                    ("Navigate", 7),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 6),
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
                &[
                    ("WM_GETOBJECT", 4),
                    ("ProviderOptions", 44),
                    ("GetPatternProvider", 22),
                    ("GetPropertyValue", 51),
                    ("HostRawElementProvider", 16),
                    ("Navigate", 12),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 9),
                ],
            ),
            steady: (
                calls(3, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 27),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 11),
                    ("Navigate", 7),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 8),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`'s
            // `FirstSelectedItem`, fetched with the `Selection` pattern in
            // one call, and its cache, one more (6 calls and 143 provider
            // calls through the `Selection` pattern alone, before).
            into_list: (
                calls(5, 0, 0),
                &[
                    ("WM_GETOBJECT", 2),
                    ("ProviderOptions", 34),
                    ("GetPatternProvider", 24),
                    ("GetPropertyValue", 49),
                    ("HostRawElementProvider", 15),
                    ("Navigate", 7),
                    ("GetRuntimeId", 5),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 11),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(3, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 27),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 11),
                    ("Navigate", 7),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 8),
                ],
            ),
        },
    );
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
    let mut outpost = OutpostUnderTest::with_options(
        app.pid(),
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::Fetches(
            Theme::builtin_default().fetches(),
        ));
    let (first, _) = measure_uia_focus((&mut app, &under_test, hwnd), &outpost, ("first", "First"));

    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.node.id, QueryKind::NextSibling);
    let next_sibling = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
    };
    common::reset_hits(hwnd);
    let (group, calls_made) = outpost.navigate(second.id, QueryKind::Parent);
    let parent = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
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
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 26),
            ("GetPatternProvider", 11),
            ("GetPropertyValue", 28),
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
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 11),
            ("GetPropertyValue", 28),
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
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 31),
            ("GetPatternProvider", 22),
            ("GetPropertyValue", 51),
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
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 22),
            ("GetPropertyValue", 51),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );

    ratchet.finish();
}

/// What registering for events costs mockapp. A registration is made on its
/// own thread and is the same with remote operations on or off, so it
/// makes no counted call on this thread either way; what it costs is the
/// provider calls UIA makes while registering, pinned here: the focus
/// listener's desktop-wide group (an element selected, a menu opened, and
/// notifications), and an outpost's focus-following property subscription
/// moved to a focus, `First` inside the group `Settings`, registered on the
/// focus alone.
fn uia_event_registrations_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-registrations");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::new(
            vec![
                Subscription::Event {
                    event: UIA_SelectionItem_ElementSelectedEventId,
                    callback: Arc::new(|_| {}),
                },
                Subscription::Event {
                    event: UIA_MenuOpenedEventId,
                    callback: Arc::new(|_| {}),
                },
                Subscription::Notifications {
                    callback: Arc::new(|_, _, _, _, _| {}),
                },
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
        &[],
    );

    let focus = AgileReference::new(under_test.element("First")).expect("an agile reference");
    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::new(
            vec![Subscription::Properties {
                properties: FOCUS_PROPERTIES.to_vec(),
                callback: Arc::new(|_, _| {}),
            }],
            Scope::Elements(vec![focus]),
        )
        .expect("the focus-following registration")
    });
    drop(registration);
    ratchet.check(
        "UIA focus-following registration",
        &cost,
        calls(0, 0, 0),
        &[("HostRawElementProvider", 1), ("FragmentRoot", 2)],
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
    let cache = under_test
        .uia
        .cache_request(CACHED_PROPERTIES)
        .expect("a cache request");
    let mut ratchet = Ratchet::default();
    let selected_name = |selected: Option<IUIAutomationElement>| {
        let selected = selected.expect("a selected child");
        snapshot_from_cached_element(&selected, &under_test.registry).name
    };

    // The program's hits beyond the selected child's, the same for both.
    let program = |extra_pattern: u32, selection: (&'static str, u32)| {
        vec![
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 24),
            ("GetPatternProvider", 23 + extra_pattern),
            ("GetPropertyValue", 49),
            ("HostRawElementProvider", 10),
            ("Navigate", 8),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 2),
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
                ("GetPatternProvider", 13),
                ("GetPropertyValue", 23),
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
                ("GetPatternProvider", 14),
                ("GetPropertyValue", 23),
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
        let container = under_test.element(name);
        let (child, cost) = under_test.measure(hwnd, |under_test| {
            under_test
                .uia
                .selected_element(container, &cache)
                .expect("the selection is read")
        });
        assert_eq!(selected_name(child).as_deref(), Some(selected));
        ratchet.check(
            &format!("UIA selected child of {name}, classically"),
            &cost,
            calls(classic_calls, 0, 0),
            &classic_hits,
        );
        let (ancestry, cost) = under_test.measure(hwnd, |under_test| {
            focus_ancestry(
                &under_test.uia,
                &FocusQuery {
                    element: container,
                    known: &[],
                    previous: None,
                    depth_limit: 64,
                    properties: CACHED_PROPERTIES,
                    deadline: None,
                    require_focus: true,
                },
                true,
            )
            .expect("the focus ancestry")
        });
        let FocusAncestry::Focused(ancestry) = ancestry.0 else {
            panic!("{name} has the focus");
        };
        assert_eq!(
            selected_name(ancestry.selected_child).as_deref(),
            Some(selected)
        );
        ratchet.check(
            &format!("UIA focus on {name} with its selected child, remotely"),
            &cost,
            calls(1, 0, 0),
            &remote_hits,
        );
    }

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
        Registration::new(
            vec![
                Subscription::Events {
                    events: vec![
                        UIA_Text_TextSelectionChangedEventId,
                        UIA_Text_TextChangedEventId,
                    ],
                    callback: Arc::new(|_, _| {}),
                },
                Subscription::ActiveTextPosition {
                    callback: Arc::new(move |_, range| {
                        let range = range.and_then(|range| AgileReference::new(range).ok());
                        // The test may have finished.
                        let _ = seen.send(range);
                    }),
                },
            ],
            Scope::Elements(vec![notes]),
        )
        .expect("the text focus's registration")
    });
    ratchet.check(
        "UIA text focus registration",
        &cost,
        calls(0, 0, 0),
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
        previous_selection: None,
    }
}

/// One caret move answered, as the outpost's worker answers a caret key
/// with `text_reads::check_watch` once it has the node's text: the caret is
/// reported (Core's knowledge), mockapp's caret moves one character on, as
/// Right Arrow moves it, and the watch's first check finds the evidence and
/// reports the line and the character there. Then the caret moves on again
/// and is reported alone, as the worker reports it for a caret event, after
/// a typed character above all, with `text_reads::report_caret`. Returns
/// the calls and hits of the answer, and of the report.
fn measure_caret_move<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    take: fn() -> CallCounts,
) -> (Cost, Cost) {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 0");
    let (before, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 1");
    let _ = take();
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
    };

    common::apply(app, hwnd, "caret doc 2");
    let _ = take();
    let (report, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    let calls = take();
    assert_eq!(report.line.offset, 2);
    let report = Cost {
        calls,
        hits: common::read_hits(hwnd),
    };
    (answer, report)
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
    common::reset_hits(hwnd);
    let (report, _) = caret_report(source, &mut store.node(1), &mut || 0, true).expect("the caret");
    let cost = Cost {
        calls: verbatim_uia::calls::take(),
        hits: common::read_hits(hwnd),
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
    let (answer, report) =
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
    // in it, and the character's spelling error and link, which a
    // character's read never learns are not supported.
    let move_hits = [
        ("ITextProvider::GetSelection", 1),
        ("Clone", 3),
        ("CompareEndpoints", 2),
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
            &remote_hits(1, &plus_copy(&move_hits)),
        );
        ratchet.check(
            "UIA caret report, remotely",
            &report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&report_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus, remotely",
            &focus_report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&focus_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus, links learned, remotely",
            &focus_again,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&focus_again_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute, remotely",
            &formatted_report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&formatted_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute learned, remotely",
            &formatted_again,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&formatted_again_hits)),
        );
        // One round trip for each check, the whole read: the check that
        // finds the evidence is the answer. 11 before 2026-10-08, one read
        // every 10 milliseconds until 100 had passed.
        ratchet.check(
            "UIA caret watch finding nothing, remotely",
            &nothing,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&move_hits)),
        );
        ratchet.check(
            "UIA caret watch answered by a later caret event, remotely",
            &later,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&move_hits)),
        );
    } else {
        ratchet.check(
            "UIA caret move, classically",
            &answer,
            calls(12, 0, 0),
            &move_hits,
        );
        ratchet.check(
            "UIA caret report, classically",
            &report,
            calls(8, 0, 0),
            &report_hits,
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
            &focus_hits,
        );
        ratchet.check(
            "UIA caret report after a focus, links learned, classically",
            &focus_again,
            calls(35, 0, 0),
            &focus_again_hits,
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute, classically",
            &formatted_report,
            calls(35, 0, 0),
            &formatted_hits,
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute learned, classically",
            &formatted_again,
            calls(35, 0, 0),
            &formatted_again_hits,
        );
        // A caret move's reads for each check. 134 before 2026-10-08, 12
        // for each of eleven reads and 2 comparing the caret with the
        // document's end.
        ratchet.check(
            "UIA caret watch finding nothing, classically",
            &nothing,
            calls(12, 0, 0),
            &move_hits,
        );
        ratchet.check(
            "UIA caret watch answered by a later caret event, classically",
            &later,
            calls(12, 0, 0),
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
    common::reset_hits(hwnd);
    let reply = perform(source, anchors, op);
    (
        reply,
        Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
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
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 1),
                ("GetText", 4),
                ("Move", 3),
                ("MoveEndpointByRange", 6),
            ],
        )
    } else {
        (
            calls(24, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("Clone", 6),
                ("CompareEndpoints", 1),
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
    let reply = answered(check_caret(
        &mut source,
        &mut anchors,
        &CaretWatch {
            landing: false,
            pressed_at_ms: 0,
            since: Some(at),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: Some(PreviousSelection { start: at, end: at }),
        },
        &mut Check::default(),
    ));
    let cost = Cost {
        calls: verbatim_uia::calls::take(),
        hits: common::read_hits(hwnd),
    };
    let changes: Vec<(bool, &str, u32)> = reply
        .selection_changes
        .iter()
        .map(|change| (change.selected, change.text.as_str(), change.characters))
        .collect();
    assert_eq!(changes, [(true, "alpha", 5)]);
    // The character's spelling error and link: two attribute reads (one
    // before the default theme reported links).
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
                ("Clone", 5),
                ("CompareEndpoints", 8),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 2),
                ("GetText", 3),
                ("MoveEndpointByRange", 4),
            ],
        )
    } else {
        (
            calls(23, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 4),
                ("CompareEndpoints", 8),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 2),
                ("GetText", 3),
                ("MoveEndpointByRange", 3),
            ],
        )
    };
    ratchet.check(
        &format!("UIA caret key selecting, {way}"),
        &cost,
        expected.0,
        expected.1,
    );
    app.quit();
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
    // expanded, and read (5 each, 95), the move that finds a next line
    // after the twentieth (3), and one `Culture` read over the whole batch
    // (3). The later batch starts from a held position moved by a line (4)
    // and needs no offset (3 for its first line). 165 and 166 before
    // (2026-10-07): two collapses and two copies per line, and a `Culture`
    // read per line.
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
                    ("CompareEndpoints", 1),
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
                calls(109, 0, 0),
                &[
                    ("ITextProvider::GetSelection", 1),
                    ("Clone", 23),
                    ("CompareEndpoints", 1),
                    ("ExpandToEnclosingUnit", 20),
                    ("GetAttributeValue", 1),
                    ("GetText", 21),
                    ("Move", 20),
                    ("MoveEndpointByRange", 22),
                ],
            ),
            (
                calls(108, 0, 0),
                &[
                    ("Clone", 23),
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
        expected[0].1,
    );
    ratchet.check(
        &format!("UIA say-all later batch of twenty lines, {way}"),
        &second,
        expected[1].0,
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
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 4),
                ("GetAttributeValue", 5),
                ("GetText", 5),
                ("Move", 4),
                ("MoveEndpointByRange", 7),
            ],
        )
    } else {
        (
            calls(33, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("Clone", 7),
                ("CompareEndpoints", 1),
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
        common::reset_hits(hwnd);
        let (report, _) =
            caret_report(&mut source, &mut store.node(1), &mut || 0, true).expect("the caret");
        let cost = Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
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
    let (answer, report) =
        measure_caret_move(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    // The answer reads the selection again (`EM_GETSEL`), so a caret read
    // before the key's effect is never paired with a line read after it.
    ratchet.check("Edit control caret move", &answer, calls(0, 0, 6), &[]);
    ratchet.check("Edit control caret report", &report, calls(0, 0, 5), &[]);
    // A caret move's messages for each check; 55 for the wait that found
    // nothing before 2026-10-08.
    let (nothing, later) = measure_watch(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    ratchet.check(
        "Edit control caret watch finding nothing",
        &nothing,
        calls(0, 0, 5),
        &[],
    );
    ratchet.check(
        "Edit control caret watch answered by a later caret event",
        &later,
        calls(0, 0, 6),
        &[],
    );
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
    common::reset_hits(hwnd);
    let mut check = Check::default();
    let watched = check_caret(source, &mut anchors, &watch, &mut check);
    assert!(matches!(watched, Watched::Watching), "{watched:?}");
    let nothing = Cost {
        calls: take(),
        hits: common::read_hits(hwnd),
    };

    common::apply(app, hwnd, "caret doc 2");
    let _ = take();
    check.caret_event = true;
    let reply = answered(check_caret(source, &mut anchors, &watch, &mut check));
    assert!(reply.moved);
    assert_eq!(reply.unit.expect("the character").text, "p");
    assert_eq!(check.reads, 2, "one read for each check");
    let later = Cost {
        calls: take(),
        hits: common::read_hits(hwnd),
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

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        ("caret_moves_cost_exactly", caret_moves_cost_exactly),
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
        ("msaa_tree_view_costs_exactly", msaa_tree_view_costs_exactly),
        ("msaa_list_view_costs_exactly", msaa_list_view_costs_exactly),
        (
            "uia_dialog_text_costs_exactly",
            uia_dialog_text_costs_exactly,
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
            "uia_navigation_steps_cost_exactly",
            uia_navigation_steps_cost_exactly,
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
    ]);
}
