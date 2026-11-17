//! The operation ledger's ratchet (`books/dev/src/reference/performance.md`): for each operation
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

use verbatim_model::property::{NodeDetail, NodeDetailSet};
use verbatim_model::{
    CallCounts, QueryKind, ReportedDetails, Role, SegmentContent, StateSet, WindowHandle,
};
use verbatim_model::{
    TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReadAhead, TextReply, TextThrough,
    TextUnit, Theme,
};
use verbatim_outpost::dialog_text::{UiaDialog, dialog_text};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{Anchors, TextSource, perform};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{
    Ancestor, Cached, DialogText, FOCUS_PROPERTIES, Fill, FocusInput, ListenerEvent,
    ListenerSelection, Live, Node, NodeIdRegistry, Part, PropertyEvent, Registration, Scope, Shape,
    Subscription, TextEvent, Uia,
};
use verbatim_uia_rops::{FocusAncestry, FocusQuery, focus_ancestry, focus_ancestry_classic};
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

use common::core_driver::{CoreDriver, Utterance};
use common::outpost::{Focused, ListenerFocus, OutpostUnderTest};
use verbatim_ia2::WinEventKind;
use verbatim_model::ReviewCommand;
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::RouteEnd;
use verbatim_model::request::WindowVerdict;

/// The fixture's nodes, by their index in mockapp's tree (depth first, the
/// root at 0): mockapp answers `WM_GETOBJECT` for index `i` at object id
/// `i + 1`, the address an MSAA focus event names.
const FIRST: usize = 2;
const SECOND: usize = 3;
const LIST: usize = 4;
const ITEM_ONE: usize = 5;
const ITEM_TWO: usize = 6;

/// A label's words.
fn label(name: &str) -> SegmentContent {
    SegmentContent::Label(name.to_owned())
}

/// Nothing said.
const NOTHING: [Utterance; 0] = [];

/// Long enough for a window's verdict of no UIA provider, which Core keeps
/// from the probe's start, to have lapsed.
const VERDICT_LAPSED: std::time::Duration = std::time::Duration::from_micros(
    verbatim_reader_rules::verdicts::NEGATIVE_VERDICT_LIFETIME_US + 1_000,
);

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
#[derive(Debug)]
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
             books/dev/src/reference/performance.md together.\n{}",
            self.mismatches.join("\n")
        );
    }
}

/// Moves mockapp's focus to `id`, tells Core of the MSAA focus event on
/// `index` in `hwnd`, as the listener tells it, and measures what Core's
/// requests cost: what Core said, and the cost.
fn measure_msaa_focus(
    app: &mut common::MockApp,
    hwnd: HWND,
    core: &mut CoreDriver,
    (id, index): (&str, usize),
) -> (Vec<Utterance>, Cost) {
    common::apply(app, hwnd, &format!("set-focus {id}"));
    let _ = core.outpost.take_failed_reads();
    let step = core.msaa_focus(hwnd, index);
    let cost = Cost {
        calls: step.calls,
        hits: common::read_hits(hwnd),
        failed_reads: core.outpost.take_failed_reads(),
    };
    (step.spoken(), cost)
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();

    // Core's start read the application's focus, its window object, and
    // probed the window, so the first focus event finds the window's
    // verdict in force and meets that object, which Core names among the
    // ancestors it knows: the walk compares each ancestor it reaches with
    // it by location, role and name, as NVDA's `IAccessible._isEqual`
    // compares objects (`NVDAObjects/IAccessible/__init__.py` 906 to 959),
    // reading each ancestor's location too, which a focus answer carries
    // for every node. The fixture's root has the window role, and an MSAA
    // window object above a control is layout, left out of the chain.
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("first", FIRST));
    assert_eq!(
        said,
        [
            vec![label("Settings"), SegmentContent::Role(Role::Group)],
            vec![label("First"), SegmentContent::Role(Role::Button)],
        ]
    );
    ratchet.check(
        "MSAA focus, after Core's start",
        &cost,
        calls(0, 40, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 11),
            ("get_accChild", 2),
            ("get_accName", 4),
            ("get_accValue", 4),
            ("get_accDescription", 4),
            ("get_accRole", 5),
            ("get_accState", 4),
            ("get_accKeyboardShortcut", 4),
            ("accFocus", 1),
            ("accLocation", 4),
        ],
    );

    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("second", SECOND));
    assert_eq!(
        said,
        [vec![label("Second"), SegmentContent::Role(Role::Button)]]
    );
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

    // The list's walk meets the window object Core knows, compared as the
    // first focus's walk compares it.
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("list", LIST));
    assert_eq!(
        said,
        [vec![label("Options"), SegmentContent::Role(Role::List)]]
    );
    ratchet.check(
        "MSAA focus into a list",
        &cost,
        calls(0, 40, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 7),
            ("get_accChild", 2),
            ("get_accName", 4),
            ("get_accValue", 4),
            ("get_accDescription", 4),
            ("get_accRole", 5),
            ("get_accState", 4),
            ("get_accKeyboardShortcut", 4),
            ("accFocus", 1),
            ("accSelection", 1),
            ("accLocation", 3),
        ],
    );

    let (said, _) = measure_msaa_focus(&mut app, hwnd, &mut core, ("item1", ITEM_ONE));
    assert_eq!(said, [vec![label("One")]]);
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("item2", ITEM_TWO));
    assert_eq!(
        said,
        [vec![
            label("Two"),
            SegmentContent::NegatedState(verbatim_model::State::Selected)
        ]]
    );
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
    // has passed by the test's clock, which Core and the outpost read: the
    // next focus's request carries "probe first", and the arrow back to the
    // second item then finds the verdict renewed. The two differ by the
    // probe alone, one window message and one `WM_GETOBJECT`. Each reads
    // one role more than the arrow above, whose previous focus was reached
    // from the list rather than by an arrow.
    core.pass_time(VERDICT_LAPSED);
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("item1", ITEM_ONE));
    assert_eq!(said, [vec![label("One")]]);
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
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("item2", ITEM_TWO));
    assert_eq!(
        said,
        [vec![
            label("Two"),
            SegmentContent::NegatedState(verbatim_model::State::Selected)
        ]]
    );
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

    // The same focus event again: a duplicate of the readers' last focus
    // by its ids, which Core drops with no request, as NVDA drops a focus
    // event for the object it last queued as the focus
    // (`docs/parity/focus-events.md`, "Duplicate focus suppression"), so
    // nothing is read or said where the outpost once read it in full.
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("item2", ITEM_TWO));
    assert_eq!(said, NOTHING);
    ratchet.check("MSAA focus repeated", &cost, CallCounts::default(), 0, &[]);

    // A focus event on an object that neither has the focused state nor
    // is inside one that has: NVDA reads the states, the object's and then
    // each ancestor's, and goes no further, and Core says nothing. The
    // outpost reads each object's role too, which tells it the step that
    // leaves a window, so that it never steps into a window it reads as a
    // plain window.
    common::reset_hits(hwnd);
    let step = core.msaa_focus(hwnd, FIRST);
    assert_eq!(step.said, []);
    ratchet.check(
        "MSAA focus without the focused state",
        &Cost {
            calls: step.calls,
            hits: common::read_hits(hwnd),
            failed_reads: core.outpost.take_failed_reads(),
        },
        calls(0, 14, 0),
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

    // A value change on an object that is not the focus, in the focus's
    // window, and then one on the focus, told by the listener: Core asks
    // whether each object is the focus or one of its ancestors and reads
    // its value (`docs/design/readers-tell-core-asks-build1.md`, "Changes
    // on objects", Dickson's rule of 2026-11-12). The first is not, told
    // apart from the focus by the ids of its event, as NVDA's equality
    // tells them apart before reading anything, and is read no further;
    // the second is met by NVDA's equality, its location, role and name
    // read, and its value read and spoken. The listener's progress path
    // reads each object's role, to know whether it is a progress bar,
    // among mockapp's hits.
    common::reset_hits(hwnd);
    let step = core.raise(
        &mut app,
        &["set-value first Changed", "set-value item2 Picked"],
        &[WinEventKind::ValueChange],
        2,
    );
    assert_eq!(
        step.spoken(),
        [vec![SegmentContent::Value("Picked".to_owned())]]
    );
    ratchet.check(
        "MSAA value changes off and on the focus",
        &Cost {
            calls: step.calls,
            hits: common::read_hits(hwnd),
            failed_reads: core.outpost.take_failed_reads(),
        },
        calls(0, 7, 0),
        0,
        &[
            ("WM_GETOBJECT", 5),
            ("accParent", 3),
            ("get_accChild", 4),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accRole", 3),
            ("accLocation", 2),
        ],
    );

    ratchet.finish();
    drop(core);
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
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-state-change");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();
    let _ = measure_msaa_focus(&mut app, hwnd, &mut core, ("item2", ITEM_TWO));
    core.pass_time(VERDICT_LAPSED);

    // Another application's window in front: the listener tells the
    // change with its window's facts, and Core's acceptance drops it.
    let front = common::another_applications_window();
    core.outpost.set_foreground(front);
    let _ = core.foreground_report();
    common::reset_hits(hwnd);
    let step = core.raise(
        &mut app,
        &["set-states item2 focused focusable selectable selected"],
        &[WinEventKind::StateChange],
        1,
    );
    assert_eq!((step.said, step.calls), (Vec::new(), CallCounts::default()));
    assert_eq!(common::read_hits(hwnd), [], "mockapp answered nothing");

    core.outpost.set_foreground(hwnd);
    let _ = core.foreground_report();
    // The focus's change, told by the listener, and what Core's request
    // cost: the object acquired at the event's address, met as the focus
    // by NVDA's equality, its location, role and name read, as NVDA's
    // `IAccessible._isEqual` reads them to direct an event to the focus
    // object (`IAccessibleHandler/__init__.py` 678 to 687;
    // `NVDAObjects/IAccessible/__init__.py` 906 to 959), and then the one
    // property the event names. The window's verdict is Core's, renewed
    // by the outpost's probe before the first.
    let mut change = |command: &str, kind: WinEventKind| {
        common::reset_hits(hwnd);
        let step = core.raise(&mut app, &[command], &[kind], 1);
        let cost = Cost {
            calls: step.calls,
            hits: common::read_hits(hwnd),
            failed_reads: core.outpost.take_failed_reads(),
        };
        (step.spoken(), cost)
    };
    // Only the states that differ from those last spoken are said: the
    // item, spoken "not selected" with its focus, is still not selected.
    let (said, cost) = change(
        "set-states item2 focused focusable selectable",
        WinEventKind::StateChange,
    );
    assert_eq!(said, NOTHING);
    ratchet.check(
        "MSAA state change on the focus, the probe renewed",
        &cost,
        calls(0, 6, 1),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accName", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("accLocation", 2),
        ],
    );
    let (said, cost) = change("set-name item2 Renamed", WinEventKind::NameChange);
    assert_eq!(said, [vec![label("Renamed")]]);
    ratchet.check(
        "MSAA name change on the focus",
        &cost,
        calls(0, 7, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accRole", 1),
            ("accLocation", 2),
        ],
    );
    let (said, cost) = change(
        "set-description item2 Second choice",
        WinEventKind::DescriptionChange,
    );
    assert_eq!(
        said,
        [vec![SegmentContent::Description(
            "Second choice".to_owned()
        )]]
    );
    ratchet.check(
        "MSAA description change on the focus",
        &cost,
        calls(0, 6, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accName", 1),
            ("get_accDescription", 1),
            ("get_accRole", 1),
            ("accLocation", 2),
        ],
    );
    // Selecting the focus is a change of its state, as NVDA handles it
    // (`NVDAObjects/__init__.py` 1314 to 1354), "selected" said as a
    // change (`controlTypes/processAndLabelStates.py` 70 to 83).
    let (said, cost) = change("select item2", WinEventKind::Selection);
    assert_eq!(
        said,
        [vec![SegmentContent::State(verbatim_model::State::Selected)]]
    );
    ratchet.check(
        "MSAA selection of the focus",
        &cost,
        calls(0, 6, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 1),
            ("get_accChild", 1),
            ("get_accName", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("accLocation", 2),
        ],
    );

    ratchet.finish();
    drop(core);
    // SAFETY: the window this thread made above.
    unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(front) }
        .expect("the test's window is destroyed");
    app.quit();
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();

    // Core's start read the application's focus, its window object, and
    // probed the window; the focus's walk meets that object, compared by
    // location, role and name as NVDA's `IAccessible._isEqual` compares
    // (`NVDAObjects/IAccessible/__init__.py` 906 to 959), each node's
    // location read as a focus answer carries it.
    let (said, cost) = measure_msaa_focus(&mut app, hwnd, &mut core, ("yes", YES));
    assert_eq!(
        said,
        [
            vec![
                label("Remove Theme"),
                SegmentContent::Role(Role::Dialog),
                SegmentContent::Description(QUESTION.to_owned()),
            ],
            vec![label("Yes"), SegmentContent::Role(Role::Button)],
        ]
    );
    ratchet.check(
        "MSAA focus into a message box",
        &cost,
        calls(0, 54, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 14),
            ("accChildCount", 2),
            ("get_accChild", 5),
            ("get_accName", 5),
            ("get_accValue", 5),
            ("get_accDescription", 5),
            ("get_accRole", 8),
            ("get_accState", 7),
            ("get_accKeyboardShortcut", 4),
            ("accFocus", 1),
            ("accLocation", 4),
        ],
    );

    ratchet.finish();
    drop(core);
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();

    // Core's start read the application's focus, the button at the form's
    // own window, and probed that window: the focus event in the child
    // window probes the child window alone, and its walk stops at the
    // form's client area Core knows.
    let (said, cost) = measure_msaa_focus(&mut app, child, &mut core, ("ok", FORM_OK));
    assert_eq!(
        said,
        [vec![label("OK"), SegmentContent::Role(Role::Button)]]
    );
    ratchet.check_calls(
        "MSAA focus in a form, after Core's start",
        cost.calls,
        calls(0, 48, 1),
    );

    let (said, cost) = measure_msaa_focus(&mut app, child, &mut core, ("cancel", FORM_CANCEL));
    assert_eq!(
        said,
        [vec![label("Cancel"), SegmentContent::Role(Role::Button)]]
    );
    ratchet.check_calls(
        "MSAA focus in a form, steady state",
        cost.calls,
        calls(0, 25, 0),
    );

    ratchet.finish();
    drop(core);
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();

    let _ = common::tree_view::focus_item(&mut core, tree, "Settings");
    let step = common::tree_view::focus_item(&mut core, tree, "Disks");
    assert_eq!(
        step.spoken(),
        [vec![
            SegmentContent::Level(1),
            label("Disks"),
            SegmentContent::State(verbatim_model::State::Checked),
            SegmentContent::Position {
                position: 1,
                set_size: Some(2)
            },
        ]]
    );
    ratchet.check_calls(
        "MSAA focus on a tree view item",
        step.calls,
        calls(0, 35, 12),
    );

    let _ = common::tree_view::focus_item(&mut core, tree, "Software");
    let software = common::tree_view::item(tree, "Software");
    let pid = app.pid();
    let mut state_change = |action: u32| {
        let events = common::core_driver::win_events(
            pid,
            || {
                common::tree_view::send(tree, TVM_EXPAND, action as usize, software);
            },
            &[WinEventKind::StateChange],
            1,
        );
        core.win_event(&events[0])
    };
    // The control's own state change event, told by the listener: the
    // item met as the focus by NVDA's equality, its location, role and name
    // read (`NVDAObjects/IAccessible/__init__.py` 906 to 959), then its
    // states; newly expanded, its children are counted, for Core to say
    // how many it holds, as NVDA's tree view item counts them with the
    // control's messages (`NVDAObjects/IAccessible/sysTreeView32.py`).
    let step = state_change(TVE_EXPAND.0);
    assert_eq!(
        step.spoken(),
        [
            vec![SegmentContent::State(verbatim_model::State::Expanded)],
            vec![SegmentContent::Phrase(verbatim_model::Phrase::Items(3))],
        ]
    );
    ratchet.check_calls("MSAA tree view item expanded", step.calls, calls(0, 6, 7));
    let step = state_change(TVE_COLLAPSE.0);
    assert_eq!(
        step.spoken(),
        [vec![SegmentContent::State(
            verbatim_model::State::Collapsed
        )]]
    );
    ratchet.check_calls("MSAA tree view item collapsed", step.calls, calls(0, 6, 2));

    ratchet.finish();
    drop(core);
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();
    let mut focus =
        |child: i32| core.msaa_event(EventKind::MsaaFocus, list, (OBJID_CLIENT.0, child));
    let _ = focus(2);
    let step = focus(1);
    assert_eq!(
        step.spoken(),
        [vec![
            label("readme.txt; Size: 1 KB; Type: Text Document"),
            SegmentContent::NegatedState(verbatim_model::State::Selected),
            SegmentContent::Position {
                position: 1,
                set_size: Some(2)
            },
        ]]
    );
    ratchet.check_calls(
        "MSAA focus on a report view item",
        step.calls,
        calls(0, 30, 14),
    );
    ratchet.finish();
    drop(core);
    app.quit();
}

/// A UIA message box's text, gathered as the outpost's worker gathers it
/// for a dialog the focus newly entered (`describe_dialogs` in
/// `verbatim-outpost`'s `read.rs`): the dialog's element is the one the
/// node registry keeps, here kept from a property event, whose cache holds
/// no detail, and is resolved on the worker's own thread as `Kept`;
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
    let (text, cost) = under_test.measure(hwnd, |under_test| {
        let uia = &under_test.uia;
        let dialog = kept.resolve().ok()?.live().clone();
        let children = uia.request_fixed::<DialogText>().ok()?;
        dialog_text(&UiaDialog::new(uia, &children, dialog))
    });
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
    let (text, cost) = under_test.measure(hwnd, |under_test| {
        let uia = &under_test.uia;
        let children = uia.request_fixed::<DialogText>().ok()?;
        dialog_text(&UiaDialog::new(uia, &children, dialog))
    });
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
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();
    // The navigator follows the focus to "First".
    let _ = measure_msaa_focus(&mut app, hwnd, &mut core, ("first", FIRST));
    let step = |core: &mut CoreDriver, command: ReviewCommand| {
        common::reset_hits(hwnd);
        let step = core.command(command);
        let cost = Cost {
            calls: step.calls,
            hits: common::read_hits(hwnd),
            failed_reads: core.outpost.take_failed_reads(),
        };
        (step.spoken(), cost)
    };

    let (said, cost) = step(&mut core, ReviewCommand::NextSibling);
    assert_eq!(
        said,
        [vec![label("Second"), SegmentContent::Role(Role::Button)]]
    );
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

    let (said, cost) = step(&mut core, ReviewCommand::Parent);
    assert_eq!(
        said,
        [vec![label("Settings"), SegmentContent::Role(Role::Group)]]
    );
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

    // A theme reporting descriptions and shortcuts as off, which the shell
    // gives Core: Core's request no longer names them, so the outpost does
    // not read them, and saves their calls. The navigator moved back to
    // the focus, which Core holds, with no call, takes the step from it
    // again.
    core.reported(ReportedDetails {
        node: NodeDetailSet::ALL
            .without(NodeDetail::Description)
            .without(NodeDetail::KeyboardShortcut),
        ..ReportedDetails::ALL
    });
    let (_, cost) = step(&mut core, ReviewCommand::ToFocus);
    assert_eq!(
        cost.calls,
        CallCounts::default(),
        "moving to the focus reads nothing"
    );
    let (said, cost) = step(&mut core, ReviewCommand::NextSibling);
    assert_eq!(
        said,
        [vec![label("Second"), SegmentContent::Role(Role::Button)]]
    );
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
    drop(core);
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
/// focused, which must be the one named `name`, and tells Core of its focus
/// event as the listener tells it, after its check thread's calls; Core
/// asks the listener's read of the kept element, then the outpost's
/// completion from its route. Measures what the check and Core's requests
/// cost, and every provider call mockapp answered for them, once the
/// outpost has handled everything Core handed it, the follow its commit
/// sends included: what Core said, and the cost.
fn measure_uia_focus(
    (app, under_test, hwnd): (&mut common::MockApp, &UiaUnderTest, HWND),
    core: &mut CoreDriver,
    (id, name): (&str, &str),
) -> (Vec<Utterance>, Cost) {
    app.send(&format!("set-focus {id}"));
    let focused = under_test.focused(hwnd);
    assert_eq!(
        focused.read().name.as_deref(),
        Some(name),
        "the element mockapp reports focused"
    );
    common::reset_hits(hwnd);
    let _ = verbatim_uia::calls::take_failed_reads();
    let _ = core.outpost.take_failed_reads();
    let step = core.uia_focus(&focused);
    let cost = Cost {
        calls: step.checked + step.calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads() + core.outpost.take_failed_reads(),
    };
    (step.spoken(), cost)
}

/// What each UIA focus change in [`uia_focus_changes`] is expected to cost.
struct FocusCosts<'a> {
    cold: (CallCounts, u32, &'a [(&'a str, u32)]),
    steady: (CallCounts, u32, &'a [(&'a str, u32)]),
    into_list: (CallCounts, u32, &'a [(&'a str, u32)]),
    next_item: (CallCounts, u32, &'a [(&'a str, u32)]),
}

/// The same focus changes, told to Core and completed by an outpost
/// reading with remote operations or the classic walk as `remote` says,
/// each checked against `expected`. The listener's check probes the window
/// at the first focus in it.
fn uia_focus_changes(remote: bool, expected: &FocusCosts<'_>) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-uia-focus-{path}"));
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut core = CoreDriver::new(
        &app,
        OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        ),
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
    let mut focus = |core: &mut CoreDriver, (id, name): (&str, &str)| {
        measure_uia_focus((&mut app, &under_test, hwnd), core, (id, name))
    };

    let (said, cost) = focus(&mut core, ("first", "First"));
    assert_eq!(
        said,
        [
            vec![
                label("Mockapp Counts Fixture"),
                SegmentContent::Role(Role::Window)
            ],
            vec![label("Settings"), SegmentContent::Role(Role::Group)],
            vec![label("First"), SegmentContent::Role(Role::Button)],
        ]
    );
    check(&mut ratchet, "cold", &cost, &expected.cold);

    let (said, cost) = focus(&mut core, ("second", "Second"));
    assert_eq!(
        said,
        [vec![label("Second"), SegmentContent::Role(Role::Button)]]
    );
    check(&mut ratchet, "steady state", &cost, &expected.steady);

    let (said, cost) = focus(&mut core, ("list", "Options"));
    assert_eq!(
        said,
        [vec![label("Options"), SegmentContent::Role(Role::List)]]
    );
    check(&mut ratchet, "into a list", &cost, &expected.into_list);

    let (said, _) = focus(&mut core, ("item1", "One"));
    assert_eq!(said, [vec![label("One")]]);
    let (said, cost) = focus(&mut core, ("item2", "Two"));
    assert_eq!(
        said,
        [vec![
            label("Two"),
            SegmentContent::NegatedState(verbatim_model::State::Selected)
        ]]
    );
    check(
        &mut ratchet,
        "arrow to the next list item",
        &cost,
        &expected.next_item,
    );

    ratchet.finish();
    drop(core);
    app.quit();
}

/// Calls by kind, as [`calls`] counts them, one of them a connection by
/// handle: the outpost's lookup of the route's window's element.
fn connected(uia: u32, msaa: u32, window_messages: u32) -> CallCounts {
    CallCounts {
        connections: 1,
        ..calls(uia, msaa, window_messages)
    }
}

/// A UIA focus costs what the listener's check thread, its read and the
/// outpost's completion make, in the order Core has them made
/// (`docs/design/readers-tell-core-asks-build1.md`, "The focus answers").
///
/// The check (`verbatim_listener_logic::check`) finds the windowless
/// sender's nearest window (`NormalizeElementBuildCache`, one call) and
/// asks the sender's keyboard focus (one call), as NVDA's focus handler
/// tests a focus before it reads it; at the cold focus it also probes the
/// window, one window message, whose verdict the listener keeps. Those
/// cost mockapp, for a button or the list, 1 `WM_GETOBJECT`, 14
/// `ProviderOptions`, 7 `GetPropertyValue`, 5 `HostRawElementProvider`, 5
/// `Navigate` and 2 `FragmentRoot`. The listener's read of the kept
/// element, made the classic way whatever the outpost's path, with no
/// connection, refreshes its cache and takes one step per ancestor up to
/// the ancestors Core names as known: two calls once Core holds a focus in
/// the same group, four for the cold focus and the list, whose read takes
/// its first selected item through `SelectionPattern2`. These six calls
/// are the listener's four and its two checks before the build.
///
/// The outpost's completion finds its own copy of the element by the
/// listener's route, from the route's window's element, which it looks up
/// by its handle, the one connection; then adds the node's event
/// registration on its registering thread and reads the node's states
/// after it, as NVDA registers in `event_gainFocus` before it speaks
/// (`NVDAObjects/UIA/__init__.py` 1623 to 1626; Dickson, 2026-11-15, the
/// design's "Changes on objects"). With remote operations that is three
/// calls: the window's element, the walk down the route, and the states'
/// program (`verbatim_uia_rops::focus_states`). Before Core asked for the
/// completion, the outpost reported the listener's read first and walked
/// to its copy after, where the count did not reach, and read no states:
/// the states' read is the 14 `GetPatternProvider`, 21
/// `GetPropertyValue`, 8 `ProviderOptions`, 5 `HostRawElementProvider`, 2
/// `Navigate`, 3 `GetRuntimeId`, 3 `FragmentRoot`, one
/// `BoundingRectangle` and, on a list item, one `IsSelected` each focus
/// costs mockapp beyond that walk. Core's `Follow` at the commit moves the
/// registration with no call. Classically the walk down the route is one
/// call per step and the states are read with the classic calls behind
/// the same signature.
fn uia_focus_changes_cost_exactly_remote() {
    uia_focus_changes(
        true,
        &FocusCosts {
            cold: (
                connected(9, 0, 1),
                0,
                &[
                    ("WM_GETOBJECT", 12),
                    ("ProviderOptions", 84),
                    ("GetPatternProvider", 78),
                    ("GetPropertyValue", 135),
                    ("HostRawElementProvider", 38),
                    ("Navigate", 24),
                    ("GetRuntimeId", 14),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 22),
                ],
            ),
            steady: (
                connected(7, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 78),
                    ("GetPatternProvider", 67),
                    ("GetPropertyValue", 115),
                    ("HostRawElementProvider", 36),
                    ("Navigate", 22),
                    ("GetRuntimeId", 15),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 22),
                ],
            ),
            into_list: (
                connected(9, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 79),
                    ("GetPatternProvider", 83),
                    ("GetPropertyValue", 134),
                    ("HostRawElementProvider", 37),
                    ("Navigate", 20),
                    ("GetRuntimeId", 15),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 23),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                connected(7, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 84),
                    ("GetPatternProvider", 67),
                    ("GetPropertyValue", 116),
                    ("HostRawElementProvider", 38),
                    ("Navigate", 24),
                    ("GetRuntimeId", 16),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 23),
                    ("IsSelected", 3),
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
                connected(12, 0, 1),
                0,
                &[
                    ("WM_GETOBJECT", 12),
                    ("ProviderOptions", 80),
                    ("GetPatternProvider", 78),
                    ("GetPropertyValue", 132),
                    ("HostRawElementProvider", 34),
                    ("Navigate", 19),
                    ("GetRuntimeId", 11),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 22),
                ],
            ),
            steady: (
                connected(11, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 75),
                    ("GetPatternProvider", 67),
                    ("GetPropertyValue", 112),
                    ("HostRawElementProvider", 33),
                    ("Navigate", 17),
                    ("GetRuntimeId", 12),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 23),
                ],
            ),
            into_list: (
                connected(12, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 76),
                    ("GetPatternProvider", 83),
                    ("GetPropertyValue", 131),
                    ("HostRawElementProvider", 34),
                    ("Navigate", 15),
                    ("GetRuntimeId", 13),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 24),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                connected(12, 0, 0),
                0,
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 82),
                    ("GetPatternProvider", 67),
                    ("GetPropertyValue", 113),
                    ("HostRawElementProvider", 36),
                    ("Navigate", 19),
                    ("GetRuntimeId", 13),
                    ("BoundingRectangle", 4),
                    ("FragmentRoot", 25),
                    ("IsSelected", 3),
                ],
            ),
        },
    );
}

/// A focus event repeated on the element that is the readers' last focus,
/// which still has the keyboard focus, is NVDA's duplicate
/// (`docs/parity/focus-events.md`), and Core ends it with no read and
/// nothing said: its cost is the listener's check alone, three calls, the
/// windowless sender's nearest window (`NormalizeElementBuildCache`, the
/// window's verdict already kept), the kept element's keyboard focus and
/// the sender's, which cost mockapp 1 `WM_GETOBJECT`, 14 `ProviderOptions`,
/// 8 `GetPropertyValue`, 6 `HostRawElementProvider`, 5 `Navigate` and 3
/// `FragmentRoot`. Before Core judged the repeat from the check's answers,
/// the listener read the repeat in full, two calls more (its cache and one
/// step up to its known parent), for Core to compare.
fn uia_repeated_focus_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-repeated-focus");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
    let mut ratchet = Ratchet::default();

    let _ = measure_uia_focus((&mut app, &under_test, hwnd), &mut core, ("first", "First"));
    let (said, cost) =
        measure_uia_focus((&mut app, &under_test, hwnd), &mut core, ("first", "First"));
    assert_eq!(said, NOTHING);
    ratchet.check(
        "UIA focus repeated on the element followed",
        &cost,
        calls(3, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 14),
            ("GetPropertyValue", 8),
            ("HostRawElementProvider", 6),
            ("Navigate", 5),
            ("FragmentRoot", 3),
        ],
    );

    ratchet.finish();
    drop(core);
    app.quit();
}

/// The focus listener makes no connection to an application's window
/// (`books/dev/src/explanation/design/listener-without-connections.md`, sections 2 and 7): its
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
        // Core asks the listener's read of the focus, then the outpost's
        // completion from its route, which commits it.
        let (read, listener_calls) = outpost.listener_focus(&focused);
        assert_eq!(
            listener_calls.connections, 0,
            "the listener's connections reading the focus on {name}"
        );
        let _ = outpost.complete_focus(read);
        outpost.settled();
    }
    drop(outpost);
    app.quit();
}

/// The listener's walks stop below the process boundary
/// (`books/dev/src/explanation/design/listener-without-connections.md`, sections 3 and 7): one
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
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"). With the walk's boundary
/// check removed, or the owner left
/// out of it, the walk's step up into the other window shows in that
/// window's calls, and without the owner the outpost's chain ends at the
/// child's window. Core's read of the current focus at its start
/// (`Request::CurrentFocus`, by the remote program or the outpost's own
/// walk classically) gives the same chain; without the remote read's
/// completion from its outermost window, it ends at the child's window.
/// The outpost probes, reads and walks no window of the other process; the
/// parent's calls during that read are pinned. With the outpost's walk
/// (`uia_ancestors`) left to step across the boundary, the classic read's
/// parent calls show the step.
///
/// The parent still receives 2 `WM_GETOBJECT`s while the listener's check
/// and read run, each with `UiaRootObjectId` and a nonzero wParam, the
/// message a UIA connection begins with (logged from mockapp's window
/// procedure on 2026-10-19): one when the check's nearest-window lookup
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
        let outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        place_below(hwnd, parent, owned);
        app.send("set-focus first");
        let focused = under_test.focused(hwnd);

        common::reset_hits(parent);
        // Core asks the listener's read of the focus, naming the nearest
        // window the check's walk found at delivery.
        let (read, listener_calls) = outpost.listener_focus(&focused);
        // As a cold focus in a top-level window: the read refreshes the
        // element's cache and takes two steps, to the group and to the
        // child's window, and none to the desktop's root, where a
        // top-level window's walk takes three. The check's two calls
        // before it, the nearest window and the keyboard focus, are
        // pinned by `uia_focus_changes_cost_exactly_remote`.
        assert_eq!(
            listener_calls,
            calls(3, 0, 0),
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
        // A child's walk ends below the boundary; an owned window is
        // top-level, its parent the desktop's root, and its owner is
        // reached by the completion alone.
        let end = if owned {
            RouteEnd::Root
        } else {
            RouteEnd::ProcessBoundary
        };
        assert_eq!(
            listener_chain(&read),
            (
                vec![Some("Settings"), Some("Mockapp Counts Fixture")],
                Some(end)
            ),
            "the listener's chain and where its walk ended ({path})"
        );

        common::reset_hits(parent);
        let (outcome, outpost_calls) = outpost.complete_focus(read);
        let reported = Focused::of(outcome, outpost_calls);
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
        // The completion's own calls, in its own application, as for a
        // focus in a top-level window
        // (`uia_focus_changes_cost_exactly_remote`): the window's element,
        // the walk down the route, one call by the remote program and one
        // per step classically, and the states' read. The child's window
        // is judged by Core's verdict, which the request carries: before
        // the completion took it, the outpost probed the window again with
        // its own arbitrator, one window message more, to judge whether
        // the chain went on above it through UI Automation.
        assert_eq!(
            reported.calls,
            if remote {
                connected(3, 0, 0)
            } else {
                connected(6, 0, 0)
            },
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

        // Core's read of the current focus gives the same chain, the
        // parent or owner a plain window: the remote program and the classic read
        // (`uia_ancestors`) both stop below the boundary, and neither calls
        // into the other process.
        outpost.settled();
        outpost.read_focus_as(focused.live().element());
        // Core's read of the current focus at its start, with no
        // ancestor known.
        outpost.forget_focus();
        common::reset_hits(parent);
        let (outcome, now_calls) = outpost.current_focus();
        let now = Focused::of(outcome, now_calls);
        outpost.settled();
        assert_eq!(
            common::read_hits(parent),
            FOCUS_NOW_PARENT_HITS[usize::from(owned)],
            "the parent's calls during the current focus's read ({path})"
        );
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(u32::from(!owned), 0, &[]),
            "the parent's WM_GETOBJECTs during the current focus's read ({path})"
        );
        assert_eq!(
            now.chain(),
            expected_chain,
            "the current focus's chain ({path})"
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
/// (`books/dev/src/explanation/design/listener-without-connections.md`, section 10, step 6).
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
            .request::<Ancestor>(verbatim_uia::AncestorGates {
                node: verbatim_uia::NodeGates {
                    counts: None,
                    ..Node::all_on()
                },
                ..Ancestor::all_on()
            })
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
            require_focus: false,
            refresh: false,
            nearest_window: Some(hwnd.0 as isize),
            before_call: None,
            value_available: None,
            is_cell: None,
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
/// read of the current focus, for the child and for the owned window, by either
/// read: none of the outpost's, the parent or owner being a plain window,
/// the classic read stopping below the boundary as the remote one does. A
/// parent receives only the `WM_GETOBJECT` with `UiaRootObjectId` and a
/// nonzero `wParam` that UIA's client side sends as the read returns the
/// child window's element (decision 13 of
/// `books/dev/src/explanation/design/listener-without-connections.md`). Before the outposts
/// stopped below the boundary, the classic read's step up into the parent
/// cost it 6 `WM_GETOBJECT`s and 27 MSAA calls (5 and 27 for an owner);
/// before the parent was a plain window, the MSAA read of its client area
/// cost it one `WM_GETOBJECT` and 10 MSAA calls more.
const FOCUS_NOW_PARENT_HITS: [&[(&str, u32)]; 2] = [&[("WM_GETOBJECT", 1)], &[]];

/// Above a parent window of another process that has a UIA provider, as a
/// host of web content can have, the outpost names the parent as a plain
/// window, by its text, and makes no call into it: no probe, no lookup by
/// its handle, no UIA step into it and no MSAA read of it
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"). UIA's client side still
/// connects to the parent as the
/// outpost's read returns the child window's element (decision 13 of
/// `books/dev/src/explanation/design/listener-without-connections.md`), which the parent's
/// `WM_GETOBJECT`s, split by kind, show, and nothing else reaches it. Both
/// of the outpost's reads, the listener's chain completed and Core's read
/// of the current focus, by the remote program and classically. With the parent read
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
        let outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        place_below(hwnd, parent, false);
        app.send("set-focus first");
        let focused = under_test.focused(hwnd);
        let (read, _) = outpost.listener_focus(&focused);
        common::reset_hits(parent);
        let (outcome, outpost_calls) = outpost.complete_focus(read);
        let reported = Focused::of(outcome, outpost_calls);
        // The parent a plain window, named by its title, with no call.
        let expected_chain = [
            Some(parent_title.as_str()),
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First"),
        ];
        // As in `uia_focus_chains_stop_below_the_process_boundary`, the
        // child's window judged by Core's verdict.
        assert_eq!(
            reported.calls,
            if remote {
                connected(3, 0, 0)
            } else {
                connected(6, 0, 0)
            },
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
        outpost.settled();
        outpost.read_focus_as(focused.live().element());
        // Core's read of the current focus at its start, with no ancestor
        // known.
        outpost.forget_focus();
        common::reset_hits(parent);
        let (outcome, now_calls) = outpost.current_focus();
        let now = Focused::of(outcome, now_calls);
        outpost.settled();
        assert_eq!(
            now.chain(),
            expected_chain,
            "the current focus's chain ({path})"
        );
        // The read connects to the parent's provider once, as the
        // child window's element is returned, by either path, and nothing
        // else reaches the parent.
        assert_eq!(
            common::read_get_objects(parent),
            get_objects(1, 1, &[]),
            "the parent's WM_GETOBJECTs during the current focus's read ({path})"
        );
        assert_eq!(
            common::read_hits(parent),
            [
                ("WM_GETOBJECT", 2),
                ("ProviderOptions", 6),
                ("HostRawElementProvider", 2),
                ("Navigate", 2)
            ],
            "the parent's calls during the current focus's read ({path})"
        );
        drop(outpost);
        app.quit();
        parent_app.quit();
    }
}

/// The names of the ancestors the listener's read of a focus carries,
/// innermost first, and where its walk ended.
fn listener_chain(read: &ListenerFocus) -> (Vec<Option<&str>>, Option<RouteEnd>) {
    (
        read.object
            .ancestors
            .iter()
            .map(|ancestor| {
                match common::outpost::value(
                    &ancestor.values,
                    verbatim_model::property::PropertyId::Name,
                ) {
                    Some(verbatim_model::property::PropertyValue::Text(name)) => {
                        Some(name.as_str())
                    }
                    _ => None,
                }
            })
            .collect(),
        read.object.route.as_ref().map(|route| route.end.clone()),
    )
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
/// UIA provider, here as Core's first request naming it asks: the raw probe
/// alone, one `WM_GETOBJECT` with a zero `wParam`, and no connection to the
/// window after it finds the provider
/// (`books/dev/src/explanation/design/listener-without-connections.md`, decision 2). The probe
/// costs mockapp one `WM_GETOBJECT`, two `ProviderOptions`, one
/// `HostRawElementProvider` and one `Navigate`. Core keeps the verdict, so
/// its second request carries it and costs nothing.
fn an_outposts_probe_that_finds_a_provider_makes_no_connection() {
    let title = common::unique_title("mockapp-counts-probe-alone");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut ratchet = Ratchet::default();
    // Core's first request naming the window, which has no verdict in
    // force, carries "probe first"; its answer's probe gives Core the
    // verdict, which its next request carries. A request that makes no
    // call of its own, `Follow` naming nothing, stands for any.
    for (label, verdict, probes, expected_get_objects, expected_hits) in [
        (
            "first",
            WindowVerdict::ProbeFirst,
            1,
            get_objects(0, 1, &[]),
            vec![
                ("WM_GETOBJECT", 1),
                ("ProviderOptions", 2),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
            ],
        ),
        (
            "second",
            WindowVerdict::Uia,
            0,
            get_objects(0, 0, &[]),
            vec![],
        ),
    ] {
        common::reset_hits(hwnd);
        let number = outpost.request_with(
            hwnd,
            verdict,
            verbatim_model::request::Request::Follow(verbatim_model::request::Follow {
                node: None,
            }),
        );
        let answer = outpost.answer(number);
        assert_eq!(
            answer
                .probes
                .iter()
                .map(|probe| probe.provider)
                .collect::<Vec<_>>(),
            vec![true; probes],
            "the {label} request's probes"
        );
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
/// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title"; the
/// decision of 2026-10-29 in "Calls that reach another application"; the
/// setup of `experiments/the-msaa-continuation-waits-on-the-parent-windows-application/`):
/// a mockapp UIA window made the child of a child window of another
/// mockapp's window, whose thread is then stalled for four seconds, has the
/// listener's read of its focus, made before the stall, reported while the
/// parent is still stalled, and the parent's process counts no provider
/// call at all, only the connection messages UIA's client side sends it as
/// the outpost's reads return the child window's element (decision 13 of
/// `books/dev/src/explanation/design/listener-without-connections.md`). Each window above the
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
    let (read, _) = outpost.listener_focus(&focused);
    common::reset_hits(parent);
    parent_app.stall(stall);
    // Core asks the outpost to complete the listener's read from its
    // route.
    let (outcome, calls) = outpost.complete_focus(read);
    let reported_at = common::now_us();
    let ended = parent_app.stall_ended(stall);
    assert!(
        reported_at < ended,
        "the focus was answered at {reported_at}, before the parent's stall ended at {ended}"
    );
    let reported = Focused::of(outcome, calls);
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
    let above_boundary = |reported: &Focused| {
        reported
            .read
            .ancestors
            .iter()
            .rev()
            .take(2)
            .map(|ancestor| {
                (
                    common::outpost::role_of(&ancestor.node),
                    common::outpost::value(
                        &ancestor.node.read.values,
                        verbatim_model::property::PropertyId::NativeWindow,
                    )
                    .cloned(),
                )
            })
            .collect::<Vec<_>>()
    };
    let window = |hwnd: HWND| {
        Some(verbatim_model::property::PropertyValue::Window(
            WindowHandle(u64::from_ne_bytes((hwnd.0 as isize).to_ne_bytes())),
        ))
    };
    assert_eq!(
        above_boundary(&reported),
        [(Role::Pane, window(parent)), (Role::Pane, window(between))],
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
    let reported = outpost.uia_focus_answer(&focused);
    assert_eq!(
        above_boundary(&reported),
        [
            (Role::Window, window(parent)),
            (Role::Pane, window(between))
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
/// and no message. The run is answered before the stall ends, so its
/// walk waited on nothing in the parent.
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
    // The button, index 1, without the focused state, tried as Core asks
    // an MSAA run of its focus event: refused, with no ancestor in the
    // window holding the focused state.
    let (run, _) = outpost.msaa_run(&[(
        common::outpost::node_ids(hwnd, 1),
        verbatim_model::request::CandidateKind::Focus,
    )]);
    let judged_at = common::now_us();
    assert_eq!(
        run.tried,
        [verbatim_model::request::Tried::Refused(
            verbatim_model::request::Refusal::NotFocused
        )]
    );
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
/// completed by plain windows (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"; the
/// setup of
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
    let (read, _) = outpost.listener_focus(&focused);
    assert_eq!(
        listener_chain(&read),
        (
            vec![Some("Settings"), Some("Mockapp Counts Fixture")],
            Some(RouteEnd::ProcessBoundary)
        ),
        "the listener's chain, the step's parent not kept, and where its walk ended"
    );
    assert_eq!(
        common::read_get_objects(parent),
        get_objects(1, 1, &[("OBJID_CLIENT", 2)]),
        "the step's connection to the other window, and its MSAA proxy's"
    );
    common::reset_hits(parent);
    let (outcome, calls_made) = outpost.complete_focus(read);
    let reported = Focused::of(outcome, calls_made);
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
    let outpost = OutpostUnderTest::new(&app);
    common::reset_hits(other);
    let (outcome, _) = outpost.current_focus();
    assert_eq!(
        outcome,
        verbatim_model::request::Outcome::NotThere,
        "the current focus's focused control"
    );
    assert_eq!(
        (common::read_hits(other), common::read_get_objects(other)),
        (vec![("WM_GETOBJECT", 1)], vec![("OBJID_CLIENT", 1)]),
        "the other window's calls and messages"
    );
    drop(outpost);
    app.quit();
    other_app.quit();
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
/// `remote` says: Core's next sibling of `First`, then the parent of
/// `Second`, from the navigator that followed the focus on `First`, with
/// the default theme's details, as the shell gives Core them; what Core said of
/// each, and its cost.
fn uia_navigation_steps(remote: bool) -> [(Vec<Utterance>, Cost); 2] {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-uia-navigation-{path}"));
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut core = CoreDriver::new(
        &app,
        OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        ),
    );
    core.reported(Theme::builtin_default().reported());
    // The navigator follows the focus to "First".
    let _ = measure_uia_focus((&mut app, &under_test, hwnd), &mut core, ("first", "First"));
    let mut step = |command: ReviewCommand| {
        common::reset_hits(hwnd);
        let _ = verbatim_uia::calls::take_failed_reads();
        let step = core.command(command);
        let cost = Cost {
            calls: step.calls,
            hits: common::read_hits(hwnd),
            failed_reads: verbatim_uia::calls::take_failed_reads()
                + core.outpost.take_failed_reads(),
        };
        (step.spoken(), cost)
    };
    let next_sibling = step(ReviewCommand::NextSibling);
    let parent = step(ReviewCommand::Parent);
    drop(core);
    app.quit();
    [next_sibling, parent]
}

fn uia_navigation_steps_cost_exactly() {
    let mut ratchet = Ratchet::default();

    let [(second, cost), (group, parent_cost)] = uia_navigation_steps(true);
    assert_eq!(
        (second, group),
        (
            vec![vec![label("Second"), SegmentContent::Role(Role::Button)]],
            vec![vec![label("Settings"), SegmentContent::Role(Role::Group)]]
        )
    );
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
            ("GetPatternProvider", 15),
            ("GetPropertyValue", 25),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );
    ratchet.check(
        "UIA parent, remotely",
        &parent_cost,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 15),
            ("GetPropertyValue", 25),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );

    let [(second, cost), (group, parent_cost)] = uia_navigation_steps(false);
    assert_eq!(
        (second, group),
        (
            vec![vec![label("Second"), SegmentContent::Role(Role::Button)]],
            vec![vec![label("Settings"), SegmentContent::Role(Role::Group)]]
        )
    );
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

/// A UIA node whose element the outpost does not keep, and whose route
/// does not lead to it, is answered not there by object navigation, a read
/// of its properties and activation, after the route's walk alone: no
/// `FindFirst` over the application's windows by its runtime id, which
/// would enter any child window another process serves
/// (`docs/design/cross-process-calls-study.md`, decision 3). The node is a
/// focus on the button `First`, which has no window of its own, completed
/// from the listener's read with its route cut to the element alone and
/// going on from its window's parent, as a walk up that failed below the
/// element's nearest window sends it: the completion finds no element by
/// that route and keeps none.
///
/// Every UI Automation node is now minted by a completion from the
/// listener's route, which the outpost keeps with it
/// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
/// build"), so a node with no route at all, which answered with no call,
/// is no longer made. Each answer finds the element gone by walking its
/// route, two calls: the route's window's element, looked up by its
/// handle, the one connection, and the walk down from it. Activation walks
/// once more, for the step to the gone node's parent that NVDA's review
/// activate takes when the navigator performs no action
/// (`globalCommands.py` 1880 to 1901), which finds the node gone again.
fn a_node_whose_route_does_not_lead_to_it_is_gone_by_its_route_alone() {
    let title = common::unique_title("mockapp-counts-uia-gone");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus first");
    let (mut read, _) = outpost.listener_focus(&under_test.focused(hwnd));
    let route = read
        .object
        .route
        .as_mut()
        .expect("a focus read answers its route");
    assert_eq!(
        route.steps.first().and_then(|step| step.window),
        None,
        "the button has no window of its own"
    );
    route.steps.truncate(1);
    route.end = RouteEnd::ContinueFrom(WindowHandle(
        u64::try_from(verbatim_uia::parent_window(hwnd.0 as isize)).expect("a window handle"),
    ));
    read.object.ancestors.clear();
    let (outcome, calls_made) = outpost.complete_focus(read);
    let focused = Focused::of(outcome, calls_made);
    assert_eq!(focused.name(), Some("First"));
    let node = focused.node();
    outpost.settled();

    let mut ratchet = Ratchet::default();
    // One walk of the route, its patterns as the request's cache request
    // holds them.
    let walk = |patterns: u32| {
        [
            ("WM_GETOBJECT", 11),
            ("ProviderOptions", 38),
            ("GetPatternProvider", patterns),
            ("GetPropertyValue", 15),
            ("HostRawElementProvider", 13),
            ("Navigate", 17),
            ("GetRuntimeId", 2),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 2),
        ]
    };
    for (operation, request, expected_calls, expected_hits) in [
        (
            "UIA next sibling of a node gone",
            verbatim_model::request::Request::Navigate {
                node,
                direction: QueryKind::NextSibling,
                set: vec![verbatim_model::property::PropertyId::Name],
            },
            connected(2, 0, 0),
            walk(11),
        ),
        (
            "UIA properties of a node gone",
            verbatim_model::request::Request::Properties {
                node,
                set: vec![verbatim_model::property::PropertyId::Name],
            },
            connected(2, 0, 0),
            walk(14),
        ),
        (
            "UIA activation of a node gone",
            verbatim_model::request::Request::Activate { node },
            connected(2, 0, 0),
            walk(11),
        ),
    ] {
        common::reset_hits(hwnd);
        // Core's verdict for the window is in force since its focus.
        let number = outpost.request_with(hwnd, WindowVerdict::Uia, request);
        let (answer, calls_made) = outpost.answer_with_calls(number);
        assert_eq!(
            answer.outcome,
            verbatim_model::request::Outcome::NotThere,
            "{operation}"
        );
        let cost = Cost {
            calls: calls_made,
            hits: common::read_hits(hwnd),
            failed_reads: outpost.take_failed_reads(),
        };
        ratchet.check(operation, &cost, expected_calls, 0, &expected_hits);
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
/// focus alone, and moved to it again, which costs nothing. Each is
/// measured warm, with UI Automation's copy of the element's runtime id
/// fresh. The outpost moves its registration to elements it keeps, so one
/// held past UI Automation's sweep is cold, and costs 9 provider calls for
/// the registration instead of 3, and 6 for the move to the same element
/// instead of 0 (`books/dev/src/reference/uia-bugs.md`, "UI Automation's copy of an
/// element's runtime id lapses").
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

    // UI Automation's copy of an element's runtime id lapses on a periodic
    // sweep, and a registration on an element whose copy has lapsed reads it
    // from the provider again: 9 provider calls in place of 3 here, and
    // provider hits on the move below that otherwise costs none
    // (`books/dev/src/reference/uia-bugs.md`, "UI Automation's copy of an element's runtime id
    // lapses"). Reading the runtime id just before each measured step,
    // outside the measure, renews the copy, so each step is measured warm.
    warm_runtime_id(under_test.element("First"));
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
    // group where it is (`books/dev/src/reference/adr/0001-focus.md`, "Focus-following
    // subscriptions").
    warm_runtime_id(under_test.element("First"));
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

/// Reads `element`'s runtime id, which renews UI Automation's copy of it
/// (`uia_event_registrations_cost_exactly` says why).
fn warm_runtime_id(element: &IUIAutomationElement) {
    assert!(
        !verbatim_uia::runtime_id(element).is_empty(),
        "the element's runtime id"
    );
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
        .request::<Node>(verbatim_uia::NodeGates {
            counts: None,
            ..Node::all_on()
        })
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
            .request::<Ancestor>(verbatim_uia::AncestorGates {
                node: verbatim_uia::NodeGates {
                    counts: None,
                    ..Node::all_on()
                },
                ..Ancestor::all_on()
            })
            .expect("a cache request"),
    );
    let mut ratchet = Ratchet::default();

    // The program's hits beyond the selected child's, the same for both.
    let program = |extra_pattern: u32, selection: (&'static str, u32)| {
        vec![
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 24),
            ("GetPatternProvider", 27 + extra_pattern),
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
                    require_focus: true,
                    refresh: false,
                    nearest_window: None,
                    before_call: None,
                    value_available: None,
                    is_cell: None,
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
    // The property changes' request caches whether the element has the
    // Text pattern, one pattern more, which the outpost's tell carries:
    // Core says nothing for a value change of an element with text, as
    // NVDA's UIA value change handler returns for one
    // (`NVDAObjects/UIA/__init__.py` 2511 to 2514, added with the tells
    // of a followed node's property changes;
    // `docs/design/readers-tell-core-asks-build1.md`, "Selection and the
    // caret's text facts").
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
            ("GetPatternProvider", 12),
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

/// The caret's report, read as a caret tell's read reads it, at
/// `read_at_us`.
fn caret_report<S: TextSource>(
    source: &mut S,
    anchors: &mut verbatim_outpost::text::NodeText<'_, S::Pos>,
    read_at_us: u64,
) -> verbatim_model::CaretReport {
    verbatim_outpost::text::held::read_selection(source, anchors, read_at_us, None)
        .expect("the caret")
        .report
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
/// say-all moves it, and the caret's location.
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
    let report = caret_report(&mut source, &mut anchors, 0);
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
    assert!(matches!(reply, TextReply::Moved { .. }), "{reply:?}");
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
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 3),
                ("CompareEndpoints", 2),
                ("GetText", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(6, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
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
    let collapsed = caret_report(&mut source, &mut anchors, 1);
    let at = TextPosition {
        anchor: collapsed.line.start,
        offset: collapsed.line.offset,
    };
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
    // compared, the selection read before and after, its caret the
    // selection's start, as NVDA's is, the select, the old selection's
    // ends compared remotely, as NVDA judges its old range as it now is,
    // and the change's ends compared and its text read, and the selection's
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
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 2),
                ("Clone", 9),
                ("CompareEndpoints", 6),
                ("GetText", 5),
                ("MoveEndpointByUnit", 3),
                ("MoveEndpointByRange", 10),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    } else {
        (
            calls(28, 0, 0),
            &[
                ("ITextProvider::GetSelection", 2),
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

/// What a caret tell's read costs, through Core: mockapp's "Notes" document
/// focused and committed, then a selection no key made, "alpha", and a UI
/// Automation caret event on the document, which the outpost's `Follow`
/// registration delivers and the outpost tells Core of with no read of its
/// own; Core asks the read of the selection, compared with the baseline
/// the focus answer minted (`docs/design/readers-tell-core-asks-build1.md`,
/// "Selection and the caret's text facts" and Table 17), and says what it
/// selected. With remote operations and classically: what Core said, and
/// the read's calls.
fn caret_tell_read(remote: bool) -> (Vec<Utterance>, CallCounts) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-caret-tell-{path}"));
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(
        &app,
        OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        ),
    );
    app.send("set-focus doc");
    let uia = Uia::new().expect("a UIA client");
    let _ = core.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    let step = core.told(1);
    drop(core);
    app.quit();
    (step.spoken(), step.calls)
}

/// The program of a caret tell's read on this thread, as the outpost's
/// worker runs it (`verbatim_outpost::text::held::read_selection`), with
/// `verbatim_uia_rops`'s counting on: mockapp's "Notes" document, the
/// caret at `caret`'s command, compared with the baseline read before it
/// at the text's start. The instructions its one program executed.
fn caret_tell_program(caret: &str) -> u32 {
    let title = common::unique_title("mockapp-counts-caret-tell-program");
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(&mut app, hwnd, "caret doc 0");
    let baseline = verbatim_outpost::text::held::read_selection(&mut source, &mut anchors, 1, None)
        .expect("the baseline's read")
        .baseline;
    common::apply(&mut app, hwnd, caret);
    verbatim_uia_rops::counting::start();
    verbatim_outpost::text::held::read_selection(&mut source, &mut anchors, 2, Some(baseline))
        .expect("the caret tell's read");
    let counts = verbatim_uia_rops::counting::stop();
    app.quit();
    match counts.as_slice() {
        [Some(count)] => *count,
        counts => panic!("one counted program, not {counts:?}"),
    }
}

/// A caret tell's read, the cost of each UI Automation caret event on the
/// committed focus (`caret_tell_read`): remotely one program, which reads
/// the caret, the selection, the caret's line, the selection's text and
/// its changes from the baseline, the element and its text pattern being
/// the focus's, held since its completion. Before the selection's text and
/// changes were read in the program, they were read from the ranges after
/// it, six calls more. Classically the read is the classic calls behind the
/// program's signature, 19, the caret being the selection's start with no
/// `TextPattern2` caret or search for where the selection starts.
/// The program's instructions, under UI Automation's limit of 10,000
/// (`instruction_limit.rs`): 90 for the selection "alpha" in a line of
/// plain text, compared with a baseline at the text's start, and 48 for the
/// caret alone moved inside the line; the caret read's worst case, the same
/// program with every part asked, is pinned by `instruction_limit.rs`'s
/// `caret_reads_execute_exactly`.
fn caret_tell_reads_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    let selected = vec![vec![SegmentContent::Phrase(
        verbatim_model::Phrase::Selected(verbatim_model::SelectionText::Text("alpha".to_owned())),
    )]];
    for (operation, remote, expected) in [
        ("UIA caret tell's read, remotely", true, calls(1, 0, 0)),
        ("UIA caret tell's read, classically", false, calls(19, 0, 0)),
    ] {
        let (said, measured) = caret_tell_read(remote);
        assert_eq!(said, selected, "{operation}: what Core said");
        ratchet.check_calls(operation, measured, expected);
    }
    assert_eq!(
        [
            caret_tell_program("caret doc 0 5"),
            caret_tell_program("caret doc 3")
        ],
        [90, 48],
        "the caret tell's read's program, a selection and the caret alone"
    );
    ratchet.finish();
}

/// A held caret key's starting read (`TextOp::StartingRead`,
/// `books/dev/src/explanation/design/key-holding.md`), the read each held
/// key waits for before its stroke is sent: the caret, its line and the
/// selection, and the key's unit at the caret, a character here, with no
/// word, as the outpost's worker runs it
/// (`verbatim_outpost::text::held::starting_read`) on mockapp's "Notes"
/// document, the caret inside its first line, through UI Automation
/// remotely and classically and through an edit control's window
/// messages. Remotely one program; classically the caret report's 8
/// calls, the unit read from the line already read; in the edit control
/// the caret report's 5 window messages.
fn starting_reads_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for (operation, remote, expected) in [
        ("UIA starting read, remotely", true, calls(1, 0, 0)),
        ("UIA starting read, classically", false, calls(8, 0, 0)),
    ] {
        let title = common::unique_title("mockapp-counts-starting-read");
        let mut app = common::spawn("text.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let mut source = uia_notes(hwnd).remote(remote);
        let measured = starting_read_calls(&mut app, hwnd, &mut source, verbatim_uia::calls::take);
        app.quit();
        ratchet.check_calls(operation, measured, expected);
    }
    let title = common::unique_title("mockapp-counts-starting-read-edit");
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
    let measured = starting_read_calls(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    app.quit();
    ratchet.check_calls("Edit control starting read", measured, calls(0, 0, 5));
    ratchet.finish();
}

/// The calls of one starting read of `source`, mockapp's "Notes"
/// document, its caret moved to the second character, as `take` counts
/// them; the read must find the character there.
fn starting_read_calls<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    take: fn() -> CallCounts,
) -> CallCounts {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 1");
    let _ = take();
    let started = verbatim_outpost::text::held::starting_read(
        source,
        &mut anchors,
        1,
        (TextUnit::Character, false),
    )
    .expect("the starting read");
    let measured = take();
    assert_eq!(
        (started.caret.line.offset, started.unit.text.as_str()),
        (1, "l"),
        "the caret and the character at it"
    );
    measured
}

/// A line key's watch check that finds the caret moved, the key's final
/// read made with it (`verbatim_outpost::text::check::check`), its line's
/// formatting read with the details the default theme reports and with
/// every detail, and with none: remotely one program each way, the
/// formatting read in it; classically the check's 13 calls, and 8 more for
/// the formatting of mockapp's one-stretch line, whose annotation types
/// and attributes are read with the default theme's details or every
/// detail alike.
fn watch_checks_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for (operation, remote, details, expected) in [
        (
            "UIA line key's check, no details, classically",
            false,
            verbatim_model::TextDetails::NONE,
            calls(13, 0, 0),
        ),
        (
            "UIA line key's check, default details, remotely",
            true,
            Theme::builtin_default().reported().text,
            calls(1, 0, 0),
        ),
        (
            "UIA line key's check, default details, classically",
            false,
            Theme::builtin_default().reported().text,
            calls(21, 0, 0),
        ),
        (
            "UIA line key's check, every detail, remotely",
            true,
            verbatim_model::TextDetails::ALL,
            calls(1, 0, 0),
        ),
        (
            "UIA line key's check, every detail, classically",
            false,
            verbatim_model::TextDetails::ALL,
            calls(21, 0, 0),
        ),
    ] {
        let title = common::unique_title("mockapp-counts-watch-check");
        let mut app = common::spawn("text.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let mut source = uia_notes(hwnd).remote(remote).details(details);
        let mut store = Anchors::new(Arc::default());
        let mut anchors = store.node(1);
        common::apply(&mut app, hwnd, "caret doc 0");
        let started = verbatim_outpost::text::held::starting_read(
            &mut source,
            &mut anchors,
            1,
            (TextUnit::Line, false),
        )
        .expect("the starting read");
        common::apply(&mut app, hwnd, "caret doc 11");
        let _ = verbatim_uia::calls::take();
        let watched = verbatim_outpost::text::check::check(
            &mut source,
            &mut anchors,
            verbatim_outpost::text::check::CheckAsk {
                point: started.point,
                motion: verbatim_model::CaretMotion::NextLine,
                word: false,
                last: false,
                final_read: verbatim_model::WatchFinalRead::Unit {
                    unit: TextUnit::Line,
                    details,
                },
            },
            2,
        )
        .expect("the check");
        let measured = verbatim_uia::calls::take();
        app.quit();
        assert!(
            matches!(&watched, Some(verbatim_model::Watched::Effect(read)) if read.caret.line.text == "gamma\n"),
            "{operation}: {watched:?}"
        );
        ratchet.check_calls(operation, measured, expected);
    }
    ratchet.finish();
}

/// An edit control's review copy select and caret location, each by its
/// window messages.
fn edit_control_select_and_location_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
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

    // The review copy's first press: "alpha" selected from a collapsed
    // caret, the selection read before and after and compared.
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(&mut app, hwnd, "caret doc 0");
    let report = caret_report(&mut source, &mut anchors, 0);
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

    // The caret's location, from a request of its own: `EM_GETSEL`, the
    // text's length (`WM_GETTEXTLENGTH`) for the percentage, and
    // `EM_POSFROMCHAR`; `tests/text.rs` pins the place itself.
    let mut source = EditText::new(edit.0 as isize, 0);
    let _ = verbatim_ia2::calls::take();
    let reply = perform(
        &mut source,
        &mut anchors,
        &TextOp::Location(TextPoint::Caret),
    );
    let measured = verbatim_ia2::calls::take();
    assert!(
        matches!(
            reply,
            TextReply::OffsetLocation {
                offset: 0,
                length: 19,
                point: Some(_),
            }
        ),
        "{reply:?}"
    );
    ratchet.check_calls("Edit control caret location", measured, calls(0, 0, 3));
    app.quit();

    ratchet.finish();
}

/// The text of mockapp's "Notes" document through UIA, as the outpost's
/// worker builds it from the node's element and its text patterns, with
/// the default theme's formatting: spelling and grammar errors.
fn uia_notes(hwnd: HWND) -> UiaText {
    let under_test = UiaUnderTest::new(hwnd);
    let notes = under_test.element("Notes").clone();
    let pattern = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, false).details(Theme::builtin_default().reported().text)
}

/// A focus onto a combo box without the `Value` pattern, whose first
/// selected item is its value, and the selection of another of its items,
/// a change of that value, told to Core and read by an outpost with remote
/// operations or the classic walk as `remote` says
/// (`books/dev/src/reference/performance.md`, "A focus change onto a combo
/// box without the Value pattern, UIA"). The first focus in the window, on
/// the combo box with the pattern, is not measured.
///
/// The focus costs what any UIA focus does
/// (`uia_focus_changes_cost_exactly_remote`): the check's two calls, the
/// listener's read, the classic way whatever the outpost's path, with the
/// selected item's three calls through the `Selection` pattern, since
/// mockapp's combo box has no `SelectionPattern2`, and the outpost's
/// completion, its states read after its registration, one
/// `ExpandCollapseState` more.
///
/// The selection takes the path Core gives a UI Automation selection not
/// on the committed focus (`docs/design/readers-tell-core-asks-build1.md`,
/// Table 12; Dickson, 2026-11-15): the check finds the item's nearest
/// window, one call; the listener reads the item it keeps, its cache and
/// one step to the combo box Core knows, two; the outpost completes it
/// from the route, the route's window's element, the walk down the route
/// and the item's relation to the focus read live, three and the
/// connection; and, its parent being the committed combo box, Core asks
/// the combo box's value, read from its first selected item as NVDA's
/// `ComboBoxWithoutValuePattern` reads it (`NVDAObjects/UIA/__init__.py`
/// 1380 to 1392, 2786 to 2800), four: its cache and the selected item's
/// three. Before Core judged the selection, the outpost judged it from the
/// listener's fact in two calls, the controlled-list test and one
/// `FindFirstBuildCache` over the combo box's children, read no states and
/// did not read the value again.
///
/// The selection is mockapp's own `ElementSelected` event, which a
/// registration shaped as the listener's receives with the item's element;
/// the providers' hits are counted from its arrival, so the event's own
/// cache filling, pinned by `uia_event_caches_cost_exactly`, is left out.
fn uia_combo_box_costs(remote: bool, expected: [(CallCounts, &[(&str, u32)]); 2]) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-combo-box-{path}"));
    let mut app = common::spawn("combo_box.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::without_elements();
    let mut core = CoreDriver::new(
        &app,
        OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        ),
    );
    let mut ratchet = Ratchet::default();
    app.send("set-focus size");
    let size = under_test.focused_child(hwnd);
    assert_eq!(size.read().name.as_deref(), Some("Size"));
    assert_eq!(
        core.uia_focus(&size).spoken(),
        [
            vec![
                label("Mockapp Combo Box Fixture"),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                label("Size"),
                SegmentContent::Role(Role::ComboBox),
                SegmentContent::Value("Large".to_owned()),
                SegmentContent::State(verbatim_model::State::Collapsed)
            ],
        ]
    );
    // The combo box is the window's first child, so the search over the
    // window's descendants finds it before any item below it.
    app.send("set-focus hour");
    let hour = under_test.focused(hwnd);
    assert_eq!(hour.read().name.as_deref(), Some("Hour"));
    common::reset_hits(hwnd);
    let _ = verbatim_uia::calls::take_failed_reads();
    let _ = core.outpost.take_failed_reads();
    let step = core.uia_focus(&hour);
    let cost = Cost {
        calls: step.checked + step.calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads() + core.outpost.take_failed_reads(),
    };
    assert_eq!(
        step.spoken(),
        [vec![
            label("Hour"),
            SegmentContent::Role(Role::ComboBox),
            SegmentContent::Value("Nine".to_owned()),
            SegmentContent::State(verbatim_model::State::Collapsed)
        ]]
    );
    let [focus, selection] = expected;
    ratchet.check(
        &format!("UIA focus onto a combo box without the Value pattern ({path})"),
        &cost,
        focus.0,
        0,
        focus.1,
    );
    // mockapp selects "Ten", and the listener's registration receives the
    // event with the item's element, which UIA has just handed out.
    let selections = ElementsSelected::new(hwnd);
    app.send("select ten");
    let (ten, states) = selections.next("Ten");
    common::reset_hits(hwnd);
    let step = core.uia_event(
        EventKind::UiaElementSelected,
        &ten,
        Some(states),
        verbatim_model::tell::Payload::None,
    );
    let cost = Cost {
        calls: step.checked + step.calls,
        hits: common::read_hits(hwnd),
        failed_reads: verbatim_uia::calls::take_failed_reads() + core.outpost.take_failed_reads(),
    };
    assert_eq!(
        step.spoken(),
        [vec![SegmentContent::Value("Ten".to_owned())]]
    );
    ratchet.check(
        &format!("UIA selection in a combo box without the Value pattern ({path})"),
        &cost,
        selection.0,
        0,
        selection.1,
    );
    ratchet.finish();
    drop(selections);
    drop(core);
    app.quit();
}

/// mockapp's element selected events under its window, subscribed to with
/// the focus listener's cache request: each event's element as the
/// listener's callback narrows it, with the states its cache holds, as the
/// listener's own `capture_selection` reads them.
struct ElementsSelected {
    _registration: Registration,
    events: mpsc::Receiver<(
        verbatim_uia::Agile<ListenerSelection>,
        Option<String>,
        StateSet,
    )>,
}

impl ElementsSelected {
    fn new(hwnd: HWND) -> Self {
        let (sent, events) = mpsc::channel();
        let registration = Registration::with_parts(
            vec![Part::shaped::<ListenerSelection>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element: &Cached<ListenerSelection>| {
                    let (_, snapshot) =
                        verbatim_listener::listener::capture_selection(element, &element.read())
                            .expect("mockapp's element has its process");
                    let agile = element.agile().expect("an agile reference");
                    // The test may have finished.
                    let _ = sent.send((agile, snapshot.name, snapshot.states));
                }),
            })],
            common::window_subtree(hwnd.0 as isize),
        )
        .expect("a subscription to element selected events");
        Self {
            _registration: registration,
            events,
        }
    }

    /// The next selection event, which must be of the item named `name`:
    /// its element as the listener tells it, and its cached states.
    fn next(&self, name: &str) -> (Cached<ListenerEvent>, StateSet) {
        let (agile, cached_name, states) = self
            .events
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("no element selected event for {name}"));
        assert_eq!(cached_name.as_deref(), Some(name), "the selected item");
        let element = agile.resolve().expect("the event's element");
        (element.narrow(), states)
    }
}

fn uia_combo_box_costs_exactly() {
    uia_combo_box_costs(
        true,
        [
            (
                connected(10, 0, 0),
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 73),
                    ("GetPatternProvider", 84),
                    ("GetPropertyValue", 133),
                    ("HostRawElementProvider", 36),
                    ("Navigate", 18),
                    ("GetRuntimeId", 14),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 23),
                    ("ExpandCollapseState", 3),
                    ("IsSelected", 1),
                    ("GetSelection", 1),
                ],
            ),
            (
                connected(10, 0, 0),
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 91),
                    ("GetPatternProvider", 84),
                    ("GetPropertyValue", 120),
                    ("HostRawElementProvider", 42),
                    ("Navigate", 22),
                    ("GetRuntimeId", 19),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 27),
                    ("ExpandCollapseState", 2),
                    ("IsSelected", 3),
                    ("GetSelection", 1),
                ],
            ),
        ],
    );
    uia_combo_box_costs(
        false,
        [
            (
                connected(12, 0, 0),
                &[
                    ("WM_GETOBJECT", 11),
                    ("ProviderOptions", 69),
                    ("GetPatternProvider", 84),
                    ("GetPropertyValue", 130),
                    ("HostRawElementProvider", 32),
                    ("Navigate", 13),
                    ("GetRuntimeId", 12),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 23),
                    ("ExpandCollapseState", 3),
                    ("IsSelected", 1),
                    ("GetSelection", 1),
                ],
            ),
            (
                connected(14, 0, 0),
                &[
                    ("WM_GETOBJECT", 10),
                    ("ProviderOptions", 89),
                    ("GetPatternProvider", 84),
                    ("GetPropertyValue", 118),
                    ("HostRawElementProvider", 41),
                    ("Navigate", 18),
                    ("GetRuntimeId", 17),
                    ("BoundingRectangle", 5),
                    ("FragmentRoot", 29),
                    ("ExpandCollapseState", 2),
                    ("IsSelected", 3),
                    ("GetSelection", 1),
                ],
            ),
        ],
    );
}

/// A UIA focus on a text field, told to Core and completed by an outpost
/// reading with remote operations or the classic walk as `remote` says:
/// the field `id` of `fixture`, the first focus in its window, once Core
/// has started. Each of `setup`'s mockapp commands is sent before the
/// focus. What Core said, and every call the focus costs: the listener's
/// check, the cold focus's probe among them, its read, and the outpost's
/// completion, its states and then its caret read with it.
fn uia_text_focus(
    fixture: &str,
    id: &str,
    remote: bool,
    setup: &[&str],
) -> (Vec<Utterance>, CallCounts) {
    uia_text_focus_reporting(fixture, id, (remote, None), setup)
}

/// [`uia_text_focus`], with Core's settings reporting `reported` when
/// given, every detail otherwise.
fn uia_text_focus_reporting(
    fixture: &str,
    id: &str,
    (remote, reported): (bool, Option<verbatim_model::ReportedDetails>),
    setup: &[&str],
) -> (Vec<Utterance>, CallCounts) {
    let title = common::unique_title("mockapp-counts-text-focus");
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(
        &app,
        OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        ),
    );
    if let Some(reported) = reported {
        core.reported(reported);
    }
    for command in setup {
        app.send(command);
    }
    app.send(&format!("set-focus {id}"));
    let uia = Uia::new().expect("a UIA client");
    let step = core.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    drop(core);
    app.quit();
    (step.spoken(), step.checked + step.calls)
}

/// The calls a focus on a text field costs, its caret read with it
/// (`books/dev/src/reference/performance.md`, "A focus on a text field"): a
/// UIA document with nothing selected, with a selection, whose text is
/// read with the caret, and protected with a selection, whose text is read
/// as any field's, and a terminal, each remotely and classically, and a
/// classic edit control through MSAA, and Core's read of the current focus
/// on the UIA document ([`text_field_focus_now_costs`]). Each is the first
/// focus in its window, so a UIA focus makes the one connection that finds
/// the outpost's own element, and the listener's check probes the window,
/// one window message.
///
/// Since Core asks the completion, a focus costs what
/// `uia_focus_changes_cost_exactly_remote` describes: the check's two
/// calls, the listener's read, here three, and the completion's window
/// element, route walk and states' read, which the outpost reads after its
/// registration (Dickson, 2026-11-15). The caret is read after the states
/// as the focus answer's selection baseline read
/// (`docs/design/readers-tell-core-asks-build1.md`, Table 17): the line at
/// the caret and the selection, with the selected text when there is one.
/// Remotely that is the text pattern's fetch and one program, two calls,
/// the selected text read in the program; the focus was 9 calls with its
/// caret and 10 with a selection before Core asked the completion and read
/// the states after the registration. Classically the read is the classic
/// calls behind the same signature: 46 and 51 calls, the line's
/// formatting read with every attribute, where the caret
/// report the outpost sent with its own focus report took 45 and 57. A terminal's caret is read with its
/// focus too, so that the focus says its line (`docs/parity/terminals.md`),
/// and its first full read is build 3's baseline read, which Core sends
/// with `Follow` as the focus commits
/// (`docs/design/readers-tell-core-asks-build3.md`, "The focused
/// terminal's life"): the focus, its caret and its baseline cost 11 calls
/// remotely and 57 classically. The line's formatting, which the focus
/// says, is read in the caret's program remotely, at no call of its own.
#[expect(
    clippy::too_many_lines,
    reason = "every text field focus and its pinned counts, listed in full"
)]
fn text_field_focuses_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    let first_focus = |uia| CallCounts {
        connections: 1,
        ..calls(uia, 0, 1)
    };
    let selected: &[&str] = &["caret doc 0 5"];
    let protected_selected: &[&str] = &["set-states doc focusable protected", "caret doc 0 5"];
    let notes = |protected: bool| {
        let mut said = vec![label("Notes"), SegmentContent::Role(Role::EditableText)];
        if protected {
            said.push(SegmentContent::State(verbatim_model::State::Protected));
        }
        said
    };
    let window = |name: &str| vec![label(name), SegmentContent::Role(Role::Window)];
    // The line with its formatting, every detail being reported, as NVDA
    // speaks a focus's line (`speech/speech.py` 904 to 922).
    let line = vec![
        SegmentContent::Format(verbatim_model::TextFormat::FontName("Consolas".to_owned())),
        SegmentContent::Format(verbatim_model::TextFormat::FontSize("11.0 pt".to_owned())),
        SegmentContent::Format(verbatim_model::TextFormat::Color("black".to_owned())),
        SegmentContent::Format(verbatim_model::TextFormat::Bold),
        SegmentContent::Text("alpha".to_owned()),
        SegmentContent::Format(verbatim_model::TextFormat::NotBold),
        SegmentContent::Format(verbatim_model::TextFormat::SpellingError),
        SegmentContent::Text("beta".to_owned()),
    ];
    let alpha_selected = vec![SegmentContent::Phrase(verbatim_model::Phrase::Preselected(
        verbatim_model::SelectionText::Text("alpha".to_owned()),
    ))];
    let text_said = |protected: bool, last: &Utterance| {
        vec![
            window("Mockapp Text Fixture"),
            notes(protected),
            last.clone(),
        ]
    };
    let terminal_said = vec![
        window("Mockapp Terminal Reading Fixture"),
        vec![label("Terminal"), SegmentContent::Role(Role::Terminal)],
        vec![
            SegmentContent::Format(verbatim_model::TextFormat::FontName("Consolas".to_owned())),
            SegmentContent::Format(verbatim_model::TextFormat::FontSize("11.0 pt".to_owned())),
            SegmentContent::Format(verbatim_model::TextFormat::Color("black".to_owned())),
            SegmentContent::Text("one".to_owned()),
        ],
    ];
    for (operation, fixture, id, remote, setup, expected_said, expected) in [
        (
            "UIA text field focus with its caret, remotely",
            "text.json",
            "doc",
            true,
            &[][..],
            text_said(false, &line),
            first_focus(10),
        ),
        (
            "UIA text field focus with its caret, classically",
            "text.json",
            "doc",
            false,
            &[][..],
            text_said(false, &line),
            first_focus(46),
        ),
        (
            "UIA text field focus with a selection, remotely",
            "text.json",
            "doc",
            true,
            selected,
            text_said(false, &alpha_selected),
            first_focus(10),
        ),
        (
            "UIA text field focus with a selection, classically",
            "text.json",
            "doc",
            false,
            selected,
            text_said(false, &alpha_selected),
            first_focus(51),
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
            text_said(true, &alpha_selected),
            first_focus(10),
        ),
        (
            "UIA protected text field focus with a selection, classically",
            "text.json",
            "doc",
            false,
            protected_selected,
            text_said(true, &alpha_selected),
            first_focus(51),
        ),
        (
            "UIA terminal focus, remotely",
            "terminal_reading.json",
            "term",
            true,
            &[][..],
            terminal_said.clone(),
            first_focus(11),
        ),
        (
            "UIA terminal focus, classically",
            "terminal_reading.json",
            "term",
            false,
            &[][..],
            terminal_said.clone(),
            first_focus(57),
        ),
    ] {
        let (said, measured) = uia_text_focus(fixture, id, remote, setup);
        assert_eq!(said, expected_said, "{operation}: what Core said");
        ratchet.check_calls(operation, measured, expected);
    }
    // With the default theme's details, as most users have them: the
    // line's formatting read classically costs what it does with every
    // detail, its annotation types and attributes asked of the line
    // either way.
    let (said, measured) = uia_text_focus_reporting(
        "text.json",
        "doc",
        (false, Some(Theme::builtin_default().reported())),
        &[],
    );
    assert_eq!(
        said,
        text_said(
            false,
            &vec![
                SegmentContent::Text("alpha".to_owned()),
                SegmentContent::Format(verbatim_model::TextFormat::SpellingError),
                SegmentContent::Text("beta".to_owned()),
            ]
        ),
        "default theme: what Core said"
    );
    ratchet.check_calls(
        "UIA text field focus with its caret, default details, classically",
        measured,
        first_focus(46),
    );

    let title = common::unique_title("mockapp-counts-edit-focus");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The keyboard focus cannot be given to the control on a desktop of
    // its own, so its host has the focused state, which an MSAA focus
    // needs on the object or an ancestor.
    app.send("set-focus root");
    let mut core = CoreDriver::new(&app, OutpostUnderTest::on_frozen_time(&app));
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
    // The control's window is judged by its class, NVDA's `Edit`, which
    // Core's request carries, so no probe is made, where the outpost once
    // probed it, one window message; its five messages are the control's
    // text and caret. The run meets the root Core's start read committed,
    // compared by location, role and name as NVDA's `IAccessible._isEqual`
    // compares objects (`NVDAObjects/IAccessible/__init__.py` 906 to 959),
    // and reads each node's location, which a focus answer carries, as
    // `msaa_focus_changes_cost_exactly` pins for a focus after Core's
    // start: 53 MSAA calls, where the focus the outpost reported alone
    // took 40.
    let step = core.msaa_event(
        EventKind::MsaaFocus,
        edit,
        (windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0, 0),
    );
    assert_eq!(
        step.spoken(),
        [
            vec![
                SegmentContent::Role(Role::EditableText),
                SegmentContent::State(verbatim_model::State::Multiline)
            ],
            vec![SegmentContent::Text("alpha beta".to_owned())],
        ],
        "the edit control's focus, with its line"
    );
    ratchet.check_calls(
        "edit control focus with its caret",
        step.calls,
        calls(0, 53, 5),
    );
    drop(core);
    app.quit();
    text_field_focus_now_costs(&mut ratchet);
    ratchet.finish();
}

/// The calls Core's read of the current focus costs on a text field, the
/// read Core asks at its start, after a replaced outpost, and after a menu
/// closes or Alt+Tab (`books/dev/src/reference/performance.md`, "The
/// focus-now answer on a text field"), with no ancestor known: mockapp's
/// UIA document with nothing selected, remotely and classically, its caret
/// read with the focus as the focus answer's selection baseline read
/// (`docs/design/readers-tell-core-asks-build1.md`, Table 17), its line
/// "alpha beta". Remotely 4 UIA calls and the probe, as before; classically
/// 13, where the caret report the focus-now answer carried took 40.
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
            calls(13, 0, 1),
        ),
    ] {
        let title = common::unique_title("mockapp-counts-text-focus-now");
        let mut app = common::spawn("text.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let outpost = OutpostUnderTest::on_frozen_time_with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        app.send("set-focus doc");
        let uia = Uia::new().expect("a UIA client");
        let focused = common::listener_focus(&uia, hwnd.0 as isize);
        outpost.read_focus_as(focused.live().element());
        let (outcome, measured) = outpost.current_focus();
        let now = Focused::of(outcome, measured);
        let verbatim_model::request::FocusCaret::Read(read) = &now.read.caret else {
            panic!(
                "{operation}: the answer came without its caret: {:?}",
                now.read.caret
            );
        };
        assert_eq!(
            (
                read.caret.line.text.as_str(),
                read.caret.line.offset,
                read.selected_text.as_deref()
            ),
            ("alpha beta\n", 0, None),
            "{operation}: the caret's line"
        );
        ratchet.check_calls(operation, now.calls, expected);
        drop(outpost);
        app.quit();
    }
}

/// Core's read of the current focus stops its UI Automation walk up at
/// the first ancestor Core already knows, as NVDA's focus ancestry stops
/// where it meets the previous focus's ancestors: after "Deep button" was
/// read as the current focus, a second read, of its sibling "Remember",
/// meets their group, which Core knows, and reads nothing above it. The
/// answer names the group's place among the known ancestors and carries
/// no ancestor. Classically the read is two calls, the focused element and
/// one step, to the group, and mockapp answers 42 property reads, where a
/// walk to the top took 8 calls and 129 reads; remotely it is the focused
/// element and the program, which reads the group alone, 54 property
/// reads where a walk to the top took 140.
fn a_focus_now_walk_stops_at_an_ancestor_core_knows() {
    for (path, remote, expected) in [
        ("remotely", true, (calls(2, 0, 0), 54)),
        ("classically", false, (calls(2, 0, 0), 42)),
    ] {
        let title = common::unique_title("mockapp-counts-focus-now-known");
        let mut app = common::spawn("ancestry.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let outpost = OutpostUnderTest::with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        app.send("set-focus deep");
        let focused = common::listener_focus(&uia, hwnd.0 as isize);
        outpost.read_focus_as(focused.live().element());
        let (outcome, measured) = outpost.current_focus();
        let _ = Focused::of(outcome, measured);
        outpost.settled();

        app.send("set-focus check");
        let focused = common::listener_focus(&uia, hwnd.0 as isize);
        outpost.read_focus_as(focused.live().element());
        common::reset_hits(hwnd);
        let (outcome, measured) = outpost.current_focus();
        let now = Focused::of(outcome, measured);
        assert_eq!(
            (now.name(), now.ancestor_names(), now.read.met),
            (Some("Remember"), Vec::<Option<&str>>::new(), Some(2)),
            "{path}: the focus, no ancestor, and the group's place"
        );
        let property_reads = common::read_hits(hwnd)
            .iter()
            .find(|(call, _)| *call == "GetPropertyValue")
            .map_or(0, |&(_, count)| count);
        assert_eq!(
            (now.calls, property_reads),
            expected,
            "{path}: the read's calls and mockapp's property reads"
        );
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
            "edit_control_select_and_location_cost_exactly",
            edit_control_select_and_location_cost_exactly,
        ),
        (
            "caret_tell_reads_cost_exactly",
            caret_tell_reads_cost_exactly,
        ),
        ("starting_reads_cost_exactly", starting_reads_cost_exactly),
        ("watch_checks_cost_exactly", watch_checks_cost_exactly),
        (
            "a_focus_now_walk_stops_at_an_ancestor_core_knows",
            a_focus_now_walk_stops_at_an_ancestor_core_knows,
        ),
        (
            "an_outposts_probe_that_finds_a_provider_makes_no_connection",
            an_outposts_probe_that_finds_a_provider_makes_no_connection,
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
            "a_node_whose_route_does_not_lead_to_it_is_gone_by_its_route_alone",
            a_node_whose_route_does_not_lead_to_it_is_gone_by_its_route_alone,
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
        ("a_cells_table_costs_exactly", a_cells_table_costs_exactly),
    ]);
}

/// What reading a cell's table costs: classically one call, only for a
/// cell, and in the focus's program no call beyond the program's own.
#[expect(
    clippy::too_many_lines,
    reason = "the classic and the program's pinned provider hits, listed in full"
)]
fn a_cells_table_costs_exactly() {
    let title = common::unique_title("mockapp-counts-grid");
    let mut app = common::spawn("grid.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let nodes = under_test
        .uia
        .request::<Node>(verbatim_uia::NodeGates {
            counts: None,
            ..Node::all_on()
        })
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
            .request::<Ancestor>(verbatim_uia::AncestorGates {
                node: verbatim_uia::NodeGates {
                    counts: None,
                    ..Node::all_on()
                },
                ..Ancestor::all_on()
            })
            .expect("a cache request"),
    );
    let mut ratchet = Ratchet::default();

    common::apply(&mut app, hwnd, "set-focus first");
    let cell = Live::new(under_test.element("First").clone())
        .refresh(&nodes)
        .expect("the cell's cache");
    let (table, cost) = under_test.measure(hwnd, |_| cell.table());
    assert!(table.is_some(), "a cell has a table");
    ratchet.check(
        "UIA table of a cell, classically",
        &cost,
        calls(1, 0, 0),
        0,
        &[
            ("ProviderOptions", 4),
            ("GetPatternProvider", 1),
            ("GetPropertyValue", 1),
            ("HostRawElementProvider", 3),
            ("Navigate", 1),
            ("GetRuntimeId", 1),
            ("ContainingGrid", 1),
            ("FragmentRoot", 2),
        ],
    );

    let plain = Live::new(under_test.element("Plain").clone())
        .refresh(&nodes)
        .expect("the button's cache");
    let (table, cost) = under_test.measure(hwnd, |_| plain.table());
    assert_eq!(table, None);
    ratchet.check("UIA table of no cell", &cost, calls(0, 0, 0), 0, &[]);

    let input = cell.narrow();
    let program = |is_cell| {
        under_test.measure(hwnd, |under_test| {
            focus_ancestry(
                &under_test.uia,
                &FocusQuery {
                    element: &input,
                    known: &[],
                    previous: None,
                    depth_limit: 64,
                    fill: &ancestors,
                    element_fill: &element_fill,
                    require_focus: true,
                    refresh: false,
                    nearest_window: None,
                    before_call: None,
                    value_available: None,
                    is_cell: Some(is_cell),
                    plain: None,
                    progress: None,
                },
                true,
            )
            .expect("the focus ancestry")
        })
    };
    let (_, without) = program(false);
    let (answer, with) = program(true);
    let FocusAncestry::Focused(ancestry) = answer.0 else {
        panic!("the cell has the focus");
    };
    assert!(ancestry.element_table.is_some(), "the program read it");
    ratchet.check(
        "UIA focus program of no cell",
        &without,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 29),
            ("GetPatternProvider", 22),
            ("GetPropertyValue", 48),
            ("HostRawElementProvider", 11),
            ("Navigate", 9),
            ("GetRuntimeId", 4),
            ("FragmentRoot", 4),
        ],
    );
    ratchet.check(
        "UIA focus program of a cell",
        &with,
        calls(1, 0, 0),
        0,
        &[
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 24),
            ("GetPropertyValue", 49),
            ("HostRawElementProvider", 13),
            ("Navigate", 10),
            ("GetRuntimeId", 5),
            ("ContainingGrid", 1),
            ("FragmentRoot", 5),
        ],
    );
    ratchet.finish();
    app.quit();
}
