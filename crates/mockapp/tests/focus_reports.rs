//! How a UIA focus from mockapp's provider is judged and read by the focus
//! listener's reader and reported by a real outpost
//! (`docs/design/focus-pipeline.md`, section 12): under which node, when
//! the provider reuses a dead element's runtime id; with which states,
//! when they changed after the focus event; dropped when its element lost
//! the keyboard focus before it was read, and reported when it had it
//! though the focus has moved on since; accepted only while its nearest
//! window is in front, as NVDA's acceptance test has it; how the outpost
//! finds its own copy of the element by its path and follows it, and
//! follows nothing when it has no path, until a later focus brings one;
//! which application's focused element it takes for a focus whose walk up
//! was cut; that a query whose walk down the focus's route runs out of
//! time fails rather than answering the node gone, and one that shows it
//! gone answers it gone;
//! how a change raised after a focus event waits for the focus's read,
//! how the old focus's state change queued behind a focus inside it is
//! handled before that focus, and its value change after it;
//! that a text change raised after the focus leaves a document and returns
//! to it is heard; that a foreground event for a window read through
//! UIA is dropped, the window met only among the focus's ancestors; and
//! how the outpost reads its focus itself after a `WinEvent` focus in a
//! UIA window that no UIA focus follows, what answers that wait, and that
//! the idle barrier's answer waits for it.
//!
//! The test stands in for the listener (`common::outpost`), reading each
//! focus with the listener's own reader on this thread; mockapp's focus
//! moves with `set-focus`, which raises no event.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use verbatim_core::SrState;
use verbatim_ia2::WinEventKind;
use verbatim_model::{
    Backend, CallCounts, Effect, Fetches, FocusText, Input, NodeDetails, NodeSnapshot,
    NormalizedEvent, OutpostId, Pid, PropertyChange, QueryKind, Role, SegmentContent, State,
    StateSet, TextFormat, TraceId, WindowHandle,
};
use verbatim_outpost::focus_read::{FocusEvent, Judged, NO_WINDOW, NOT_FOCUSED};
use verbatim_outpost::protocol::{
    DeliveredFact, OutpostToSupervisor, Query, QueryOutcome, SupervisorToOutpost, UiaFocusRead,
    UiaSnapshotFact, WinEventStamp,
};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::{
    Cached, ListenerEvent, ListenerSelection, Part, RAW_PROBE_TIMEOUT_MS, Registration,
    Subscription, Uia,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, UIA_ControllerForPropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HelpTextPropertyId, UIA_NamePropertyId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_ValueValuePropertyId,
};
use windows::core::Interface;

use common::outpost::OutpostUnderTest;

/// A UIA client on this thread over mockapp's window.
struct Client {
    uia: Uia,
    hwnd: HWND,
}

impl Client {
    fn new(hwnd: HWND) -> Self {
        Self {
            uia: Uia::new().expect("a UIA client"),
            hwnd,
        }
    }

    /// The element mockapp reports as having the keyboard focus, built with
    /// the listener's cache request, as a focus event's element arrives.
    fn focused(&self) -> Cached<ListenerEvent> {
        common::listener_focus(&self.uia, self.hwnd.0 as isize)
    }

    /// The element of mockapp's named `name`, built with the listener's
    /// cache request, as a focus event's element would arrive at this
    /// moment, whether or not it has the keyboard focus.
    fn named(&self, name: &str) -> Cached<ListenerEvent> {
        let root = self
            .uia
            .window_live(self.hwnd.0 as isize)
            .expect("mockapp's window element");
        let condition = self
            .uia
            .property_condition(
                UIA_NamePropertyId,
                &windows::Win32::System::Variant::VARIANT::from(name),
            )
            .expect("a condition");
        let request = self
            .uia
            .request_fixed::<ListenerEvent>()
            .expect("the listener's cache request");
        root.find_first(
            windows::Win32::UI::Accessibility::TreeScope_Descendants,
            &condition,
            &request,
        )
        .expect("the search")
        .expect("mockapp has the element")
    }
}

/// File Explorer, going back from a subfolder, gave the parent folder's
/// item the runtime id of the subfolder's item it had just destroyed. The
/// focus listener's reader, which read the dead item as the focus before,
/// reads with the new focus whether that earlier element still has the
/// keyboard focus: it is gone, so the outpost reports the new focus under
/// a new node. UI Automation counts the new element as the dead one, whose
/// runtime id it has, but the dead one's keyboard focus cannot be read, so
/// the focus is no repeat (section 16.3); a focus event repeated on the new
/// element, which still has the focus, is. Both ways the reader reads a
/// focus: with remote operations and without.
///
/// The calls each read makes are pinned, the outpost's own included, which
/// are none: it reads nothing before it reports the focus. Before any read,
/// the listener finds the windowless item's nearest window, for NVDA's
/// native check and acceptance test, one call; then compares the sender
/// with the last focus it accepted and, UI Automation equating them, reads
/// that element's keyboard focus: two calls; then the sender's own keyboard
/// focus, read plainly as NVDA reads it, one call. The read, the classic
/// walk whichever way the outpost reads, makes four calls: the earlier
/// element's keyboard focus, the focus's own cache, and two hops up to an
/// ancestor of the focus before, with its window. The repeated focus makes
/// the nearest window, the comparison, and the keyboard focus read alone.
fn reused_runtime_id(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let gone_cost = 8;
    let title = common::unique_title(&format!("mockapp-reuse-{path}"));
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );

    app.send("set-focus delta");
    let delta = client.focused();
    let reported = outpost.uia_focus(&delta);
    assert_eq!(reported.node.name.as_deref(), Some("delta.txt"), "{path}");
    let dead_node = reported.node.id;

    app.send("take-runtime-id inner delta");
    app.send("set-focus inner");
    let inner = client.focused();
    assert_eq!(
        inner.runtime_id(),
        delta.runtime_id(),
        "{path}: mockapp gave Inner the dead item's runtime id"
    );
    let reported = outpost.uia_focus(&inner);
    assert_eq!(reported.node.name.as_deref(), Some("Inner"), "{path}");
    let new_node = reported.node.id;
    assert_ne!(new_node, dead_node, "{path}: a new node for a new element");
    assert_eq!(
        reported.calls.uia, gone_cost,
        "{path}: the calls the focus made"
    );

    let (judged, calls) = outpost.listener_read(&inner);
    assert!(
        matches!(judged, Judged::Dropped(_)),
        "{path}: the repeated focus is dropped, not {judged:?}"
    );
    assert_eq!(calls.uia, 3, "{path}: the calls the repeated focus made");
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus, None));
    outpost.settled();

    drop(outpost);
    app.quit();
}

fn a_reused_runtime_id_names_a_new_node_remote() {
    reused_runtime_id(true);
}

fn a_reused_runtime_id_names_a_new_node_classic() {
    reused_runtime_id(false);
}

/// File Explorer, going back from a subfolder, raised Inner's focus event
/// before it selected Inner, so the event's states said "not selected".
/// NVDA reads a focus's states when it handles the focus, not from the
/// event, and said "Inner 1 of 2"; the focus listener reads them afresh
/// in its read of the event's element, and its name and role from the
/// event.
fn a_focus_reports_the_states_read_when_it_is_handled() {
    let title = common::unique_title("mockapp-live-states");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus inner");
    let event = client.focused();
    app.send("select inner");
    let reported = outpost.uia_focus(&event);
    let node = reported.node;
    assert_eq!(
        (node.role, node.name.as_deref(), node.states),
        (
            Role::ListItem,
            Some("Inner"),
            [
                State::Focused,
                State::Focusable,
                State::Selectable,
                State::Selected
            ]
            .into_iter()
            .collect::<StateSet>()
        )
    );
    assert_eq!(
        (node.details.position_in_set, node.details.set_size),
        (Some(1), Some(2))
    );

    drop(outpost);
    app.quit();
}

/// A Win32 tree view taking the focus raises focus on itself and then on
/// its focused item, within its one `SetFocus` call. NVDA handles both
/// together, newest first, and announces the item with the tree as its new
/// ancestor; the outpost may read the tree's event before the item's
/// reaches it, so it asks the tree's `accFocus` and reports the item it
/// names (`docs/parity/windows-and-processes.md`, "A control's own focus with a focused child").
/// The tree's event is handed to the outpost alone, as when it is read
/// before the item's arrives, and the item's then reports nothing more.
/// Both events are raised before the outpost is made, so its own hook,
/// which takes in focus events, has neither, as when the listener forwards
/// an application's first events.
fn a_controls_own_focus_reports_its_focused_child() {
    let title = common::unique_title("mockapp-focus-child");
    let mut app = common::spawn("focus_child.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The tree is node 1, after the root, so its object id is 2; General
    // is its first child.
    let tree = 2;

    common::apply(&mut app, hwnd, "focus-child categories general");
    let outpost = OutpostUnderTest::new(&app);
    let reported = outpost.focus(DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: tree,
        id_child: 0,
    });
    assert_eq!(
        (reported.node.role, reported.node.name.as_deref()),
        (Role::TreeItem, Some("General"))
    );
    assert_eq!(
        reported
            .ancestors
            .last()
            .map(|tree| (tree.role, tree.name.as_deref())),
        Some((Role::Tree, Some("Categories")))
    );
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: tree,
        id_child: 1,
    });
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// An MSAA focus event at the address of the focus last reported is a
/// duplicate, dropped, when that focus was a full object, and reported
/// again when it was a simple child, whose child id can be reused, as
/// NVDA's `isDuplicateIAccessibleEvent` judges by the last focus's own
/// child id. Both events are raised before the outpost is made, so its own
/// hook has neither.
fn a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object() {
    let title = common::unique_title("mockapp-msaa-duplicate");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The button is node 1, after the root, so its object id is 2.
    let button = DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: 2,
        id_child: 0,
    };
    common::apply(&mut app, hwnd, "set-focus btn1");
    let outpost = OutpostUnderTest::new(&app);
    let reported = outpost.focus(button.clone());
    assert_eq!(reported.node.role, Role::Button);
    outpost.deliver(button);
    outpost.settled();
    drop(outpost);
    app.quit();

    let title = common::unique_title("mockapp-msaa-child-repeat");
    let mut app = common::spawn("focus_child.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The tree is node 1, so its object id is 2; General is child 1.
    let general = DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: 2,
        id_child: 1,
    };
    common::apply(&mut app, hwnd, "focus-child categories general");
    let outpost = OutpostUnderTest::new(&app);
    for _ in 0..2 {
        let reported = outpost.focus(general.clone());
        assert_eq!(
            (reported.node.role, reported.node.name.as_deref()),
            (Role::TreeItem, Some("General"))
        );
    }
    drop(outpost);
    app.quit();
}

/// A UIA focus event whose element has lost the keyboard focus to another
/// element by the time the listener reads it is dropped, as NVDA's
/// `shouldAllowUIAFocusEvent` drops it, and the outpost says nothing for
/// it; the newer focus is reported from its own event. (The moved-on
/// rules this replaces reported such a focus as its event said.)
fn a_focus_whose_element_lost_the_keyboard_focus_before_it_was_read_is_dropped() {
    let title = common::unique_title("mockapp-lost-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    app.send("set-focus slider1");
    let (judged, _) = outpost.listener_read(&button);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a focus that had moved on: {judged:?}");
    };
    assert_eq!(reason, "its element no longer has the keyboard focus");
    outpost.deliver(judged.into_fact(7, FocusEvent::Focus, None));
    outpost.settled();

    let slider = client.focused();
    let reported = outpost.uia_focus(&slider);
    assert_eq!(reported.node.name.as_deref(), Some("Level"));

    drop(outpost);
    app.quit();
}

/// A UIA focus's snapshot carries its raw-view parent's automation id,
/// which Core's rule for the task switcher's items matches
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane"),
/// from the listener's read and from the outpost's own read of a focus no
/// UIA focus event answered; its ancestors carry none.
fn a_focus_carries_its_parents_automation_id() {
    let title = common::unique_title("mockapp-parent-automation-id");
    let mut app = common::spawn("switcher.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus window_a");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_millis(1000));
    outpost.settle();
    let reported = outpost.reported_focus();
    assert_eq!(
        (
            reported.node.name.as_deref(),
            reported.node.details.parent_automation_id.as_deref(),
        ),
        (Some("Window A"), Some("SwitchItemListControl"))
    );

    app.send("set-focus window_b");
    let item = client.focused();
    let reported = outpost.uia_focus(&item);
    assert_eq!(
        (
            reported.node.name.as_deref(),
            reported.node.details.parent_automation_id.as_deref(),
        ),
        (Some("Window B"), Some("SwitchItemListControl"))
    );
    assert!(
        reported
            .ancestors
            .iter()
            .all(|ancestor| ancestor.details.parent_automation_id.is_none())
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus event whose element still had the keyboard focus when the
/// listener read it is reported, whatever has happened since: Windows
/// Terminal's Control+Tab gives the focus to the tab it leaves, then to
/// the tab it selects, and NVDA queues both, culling the first once the
/// second is applied (`docs/design/focus-pipeline.md`, section 10.1, item
/// 2). The focus is reported with its states as the listener read them,
/// still selected, and the item moved to after it.
fn a_focus_read_while_it_had_the_keyboard_focus_is_reported_though_the_focus_moved_on() {
    let title = common::unique_title("mockapp-left-selection");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("select item1");
    app.send("set-focus item1");
    let first = client.focused();
    let read = outpost.listener_fact(&first);

    app.send("select item2");
    app.send("set-focus item2");
    let reported = outpost.focus(read);
    assert_eq!(
        (reported.node.name.as_deref(), reported.node.states),
        (
            Some("First"),
            [
                State::Focused,
                State::Focusable,
                State::Selectable,
                State::Selected
            ]
            .into_iter()
            .collect::<StateSet>()
        )
    );
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Events Fixture"),
            Some("Options"),
            Some("First")
        ]
    );

    let second = client.focused();
    let reported = outpost.uia_focus(&second);
    assert_eq!(reported.node.name.as_deref(), Some("Second"));

    drop(outpost);
    app.quit();
}

/// Hands the outpost the listener's read of a UIA focus on `element`, as
/// the listener sends one it could not read beyond the event: its
/// ancestors and window unknown, so the outpost has no path to its own
/// copy of the element. Returns the focus it reports from the read, with
/// its ancestors unknown, which must be the only thing it says.
fn focus_without_a_path(
    outpost: &OutpostUnderTest,
    element: &Cached<ListenerEvent>,
) -> NodeSnapshot {
    let (judged, _) = outpost.listener_read(element);
    let Judged::Read(mut read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    read.ancestors = None;
    read.window = None;
    outpost.deliver(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    foreground: false,
                    ancestors,
                    ancestors_unknown: true,
                    selected_child: None,
                    text: None,
                },
            ..
        } if ancestors.is_empty() => node,
        other => panic!("the outpost said {other:?}, not a focus with its ancestors unknown"),
    };
    outpost.settled();
    node
}

/// Renames `id`, the focus, whose node is `node`, and asserts the outpost
/// hears the change by both routes it takes, UIA's own event and the
/// `WinEvent` UIA raises alongside it, and reports it: the focus's changes
/// are followed.
fn assert_followed(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    id: &str,
    node: &NodeSnapshot,
) {
    app.send(&format!("set-name {id} Renamed"));
    outpost.heard(&[
        Heard::UiaProperty(UIA_NamePropertyId.0),
        Heard::Msaa(WinEventKind::NameChange),
    ]);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Name(name),
                    child_count: None,
                },
            ..
        } => assert_eq!((node_id, name.as_deref()), (node.id, Some("Renamed"))),
        other => panic!("the outpost said {other:?}, not the focus's new name"),
    }
    outpost.settled();
}

/// The outpost finds its own copy of a windowless focus's element by the
/// path the listener read, from mockapp's window down by runtime ids,
/// after it has reported the focus, and follows the element's changes
/// from then on.
fn a_focus_is_found_by_its_path_and_followed() {
    let title = common::unique_title("mockapp-focus-path");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let reported = outpost.uia_focus(&button);
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    assert_followed(&mut app, &outpost, "btn1", &reported.node);

    drop(outpost);
    app.quit();
}

/// A UIA focus with no path to its element, as Windows Terminal's tab
/// focus elements, which are in no tree, have none, is reported from the
/// listener's read, and nothing is followed. A focus event repeated on it
/// while it keeps the keyboard focus is a repeat, dropped before it is
/// read (section 16.3); once the focus has been elsewhere and comes back,
/// its focus event is read in full and finds the element: the focus is
/// reported under its node, now with its ancestors, and its changes are
/// followed from then on.
fn a_focus_without_a_path_is_followed_from_its_next_focus_event() {
    let title = common::unique_title("mockapp-focus-not-found");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let node = focus_without_a_path(&outpost, &button);
    let (judged, _) = outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Dropped(_)),
        "a repeat is dropped, not {judged:?}"
    );

    app.send("set-focus slider1");
    let _ = outpost.uia_focus(&client.focused());
    app.send("set-focus btn1");
    let reported = outpost.uia_focus(&button);
    assert_eq!(
        (reported.node.id, reported.node.name.as_deref()),
        (node.id, Some("Original Name"))
    );
    assert_followed(&mut app, &outpost, "btn1", &node);

    drop(outpost);
    app.quit();
}

/// How late the cut walk's step to the text field's parent answers: past
/// the listener's two-second read budget.
const SLOW_PARENT_MS: u32 = 2100;

/// A text field with no window of its own whose walk up the listener cut,
/// so that the chain it sends holds no window, is found by the outpost
/// as its application's focused element, read with the focus read's
/// process-id check and taken because its runtime id is the field's, before
/// the focus is reported (`docs/design/focus-pipeline-rework.md`, the
/// audit's difference 5). The field, "Notes", is four levels below its
/// window, where a walk down from the window by the runtime ids known would
/// not reach it. The read's first step to the field's parent, after the
/// nearest window's lookup, answers past the read's budget (`slow-parent`),
/// spent on the system's clock, and the chain is completed by the field's
/// nearest window as a plain window. Found, the field has no text pattern,
/// so its text is reported as none, `NoText`; not found, it would be
/// `NotRead`.
fn a_slow_windowless_text_focus_is_found_as_the_focused_element() {
    let title = common::unique_title("mockapp-cut-walk");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let notes = client.focused();
    outpost.read_focus_as(notes.live().element());
    outpost.follow_the_system_clock();
    app.send(&format!("slow-parent notes {SLOW_PARENT_MS} 1"));
    let (judged, _) = outpost.listener_read(&notes);
    app.stamped_line("slow-parent started");
    app.stamped_line("slow-parent ended");
    app.send("slow-parent notes 0");
    let Judged::Read(read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    // The step came back past the read's budget with the field's parent,
    // where the walk was cut: the chain is completed by plain windows from
    // the field's nearest window.
    assert_eq!(
        (
            read.hwnd,
            read.ancestors.as_deref().map(<[_]>::len),
            read.ancestors_end,
            read.window
        ),
        (
            0,
            Some(1),
            verbatim_outpost::protocol::AncestorsEnd::PlainFrom(hwnd.0 as isize),
            Some(hwnd.0 as isize)
        ),
        "the walk was cut below the field's nearest window"
    );
    outpost.deliver(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    ancestors,
                    ancestors_unknown: false,
                    text,
                    ..
                },
            timing,
            ..
        } => {
            assert_eq!(
                (
                    node.name.as_deref(),
                    ancestors
                        .iter()
                        .map(|ancestor| (ancestor.role, ancestor.name.as_deref()))
                        .collect::<Vec<_>>(),
                    text.as_deref()
                ),
                (
                    Some("Notes"),
                    vec![
                        (Role::Window, Some(title.as_str())),
                        (Role::Group, Some("Inner group"))
                    ],
                    Some(&FocusText::NoText)
                )
            );
            // The window's probe, one message; the focused-element read,
            // one UIA call, its stand-in counting no connection; and two on
            // the field found. Without the read the field is not found, and
            // no UIA call is made.
            assert_eq!(
                timing.calls,
                CallCounts {
                    uia: 3,
                    msaa: 0,
                    window_messages: 1,
                    connections: 0,
                }
            );
        }
        other => panic!("the outpost said {other:?}, not the field's focus"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The listener's read of mockapp's "Notes", the focus, with the chain it
/// sends cut to the field alone, as a walk up cut below the field's nearest
/// window sends it: the chain the outpost finds the field's element after a
/// cut walk for.
fn cut_read_of_notes(
    outpost: &OutpostUnderTest,
    notes: &Cached<ListenerEvent>,
) -> Box<UiaFocusRead> {
    let (judged, _) = outpost.listener_read(notes);
    let Judged::Read(mut read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    assert_eq!(read.hwnd, 0, "the field has no window of its own");
    read.ancestors = None;
    read
}

/// Hands the outpost `read`, a focus on "Notes" whose element no route
/// leads to, and asserts it reports the field with its text `text`, after
/// the calls `calls`.
fn assert_notes_reported(
    outpost: &OutpostUnderTest,
    read: Box<UiaFocusRead>,
    text: &FocusText,
    calls: CallCounts,
) {
    outpost.deliver(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node, text: found, ..
                },
            timing,
            ..
        } => assert_eq!(
            (node.name.as_deref(), found.as_deref(), timing.calls),
            (Some("Notes"), Some(text), calls)
        ),
        other => panic!("the outpost said {other:?}, not the field's focus"),
    }
    outpost.settled();
}

/// After a cut walk, the application's focused element is taken as the
/// focus's only when its runtime id is the focus's: here it is the button
/// "Deep button", the field's sibling, so the field is not found and its
/// text is not read, `NotRead`; taken, the button would give `NoText`. The
/// window's probe, one message, and the focused-element read, one UIA call.
fn a_cut_walks_focused_element_with_another_runtime_id_is_not_taken() {
    let title = common::unique_title("mockapp-cut-walk-other-id");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let read = cut_read_of_notes(&outpost, &client.focused());
    outpost.read_focus_as(client.named("Deep button").live().element());
    assert_notes_reported(
        &outpost,
        read,
        &FocusText::NotRead,
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 1,
            connections: 0,
        },
    );

    drop(outpost);
    app.quit();
}

/// After a cut walk, the application's focused element is taken only when
/// the process serving the focus window serves it: here it is the "Notes"
/// of a second mockapp, whose runtime id the focus has, read by the
/// listener there and sent as this application's, so the field is not
/// found and its text is not read, `NotRead`; taken, it would give
/// `NoText`. The window's probe, one message, and the focused-element read,
/// one UIA call.
fn a_cut_walks_focused_element_of_another_process_is_not_taken() {
    let title = common::unique_title("mockapp-cut-walk-own");
    let app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let other_title = common::unique_title("mockapp-cut-walk-other");
    let mut other = common::spawn("ancestry.json", "uia", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let other_client = Client::new(other_hwnd);
    let outpost = OutpostUnderTest::new(&app);

    other.send("set-focus notes");
    let notes = other_client.focused();
    outpost.set_foreground(other_hwnd);
    let mut read = cut_read_of_notes(&outpost, &notes);
    outpost.set_foreground(hwnd);
    read.window = Some(hwnd.0 as isize);
    outpost.read_focus_as(notes.live().element());
    assert_notes_reported(
        &outpost,
        read,
        &FocusText::NotRead,
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 1,
            connections: 0,
        },
    );

    drop(outpost);
    other.quit();
    app.quit();
}

/// A focus with no window of its own whose chain holds its ancestors, none
/// with a window either, has no route, but its walk up was not cut, so the
/// application's focused element is not read for it, though that element
/// is the field: the field is not found and its text is not read,
/// `NotRead`, with the window's probe, one message, and no UIA call; read,
/// it would give `NoText`.
fn a_focus_whose_walk_was_not_cut_is_not_read_as_the_focused_element() {
    let title = common::unique_title("mockapp-uncut-walk");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let notes = client.focused();
    let (judged, _) = outpost.listener_read(&notes);
    let Judged::Read(mut read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    assert_eq!(read.hwnd, 0, "the field has no window of its own");
    for ancestor in read.ancestors.iter_mut().flatten() {
        ancestor.hwnd = 0;
    }
    outpost.read_focus_as(notes.live().element());
    assert_notes_reported(
        &outpost,
        read,
        &FocusText::NotRead,
        CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 1,
            connections: 0,
        },
    );

    drop(outpost);
    app.quit();
}

/// An ancestor of the focus, whose element the outpost does not hold, is
/// found by the focus's route; a walk down that route that runs past its
/// time budget, as against a slow application, has not shown the element
/// gone, so a query on the ancestor fails rather than answering it gone,
/// and Core keeps its navigator there. The ancestor is "Inner group",
/// four levels below mockapp's window; the walk is the classic one, whose
/// budget is checked between its calls, and the outpost's time runs out
/// after the walk sets its budget's end.
///
/// The test counts the query's clock reads (`run_out_of_time_after(2)`):
/// a read added on the query's path before the walk sets its budget
/// (`time_limit`) needs the count updated.
fn a_route_walk_out_of_time_fails_rather_than_answering_gone() {
    let title = common::unique_title("mockapp-route-out-of-time");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );

    app.send("set-focus notes");
    let reported = outpost.uia_focus(&client.focused());
    let group = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .clone();
    // The query's entry reads the time first, and the walk's budget's end
    // is set by the next reading; every later one is past it.
    outpost.run_out_of_time_after(2);
    let request_id = outpost.ask(Query::Navigate {
        node_id: group.id,
        kind: QueryKind::FirstChild,
    });
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id: answered,
            outcome,
            ..
        } if answered == request_id => assert_eq!(
            outcome,
            QueryOutcome::Failed(
                "walking the focus's route: the walk ran past its time budget".to_owned()
            )
        ),
        other => panic!("the outpost said {other:?}, not the step's answer"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// An ancestor of the focus, whose element the outpost does not hold, is
/// answered gone when the walk down the focus's route shows it gone: once
/// mockapp has disconnected "Inner group", four levels below its window,
/// a call on the element fails as on one that is gone. The walk is the
/// classic one.
fn a_route_walk_that_finds_its_element_gone_answers_gone() {
    let title = common::unique_title("mockapp-route-gone");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );

    app.send("set-focus notes");
    let reported = outpost.uia_focus(&client.focused());
    let group = reported
        .ancestors
        .iter()
        .find(|ancestor| ancestor.name.as_deref() == Some("Inner group"))
        .expect("the focus's chain has the inner group")
        .id;
    app.send("disconnect group2");
    outpost.gone(Query::Navigate {
        node_id: group,
        kind: QueryKind::FirstChild,
    });

    drop(outpost);
    app.quit();
}

/// The listener compares a UIA focus event with the focus Core holds, as
/// NVDA compares it with the last focus it queued, whatever reported it
/// (`lastQueuedFocusObject`; decision D14, "Where the rules run"): a repeat
/// on the element of the focus Core took from the listener's read is
/// dropped; a focus the listener read and Core refused, as Core refuses a
/// shell window that only stages a switch, changes nothing, and the repeat
/// is still dropped; once Core holds a focus the listener did not read, as
/// an MSAA menu's or a foreground change's, a focus event on the element,
/// which never lost the keyboard focus, is read in full.
fn a_focus_is_no_repeat_once_core_holds_another() {
    let title = common::unique_title("mockapp-focus-queued");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let _ = outpost.uia_focus(&button);
    let (judged, _) = outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Dropped(_)),
        "a repeat of the focus Core holds is dropped, not {judged:?}"
    );
    app.send("set-focus slider1");
    let slider = client.focused();
    let _ = outpost.listener_fact(&slider);
    outpost.listener_core_refuses_last_read();
    app.send("set-focus btn1");
    let (judged, _) = outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Dropped(_)),
        "after a focus Core refused, a repeat is dropped, not {judged:?}"
    );
    outpost.listener_core_focus_elsewhere();
    let (judged, _) = outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Read(_)),
        "once Core holds another focus, the focus is read, not {judged:?}"
    );

    drop(outpost);
    app.quit();
}

/// NVDA keeps one last queued focus for every application and backend
/// (`lastQueuedFocusObject`), and its UIA duplicate test compares with it
/// only while it is a UIA object: a UIA focus repeated on the element that
/// still has the keyboard focus is dropped, and once another
/// application's MSAA focus has been queued in between, which Core tells
/// the listener, it is read in full.
fn a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus() {
    let title = common::unique_title("mockapp-uia-after-msaa");
    let mut uia_app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let uia_outpost = OutpostUnderTest::new(&uia_app);
    let title = common::unique_title("mockapp-msaa-before-uia");
    let mut msaa_app = common::spawn("small.json", "msaa", &title);
    let msaa_hwnd = common::find_window(&title);
    common::apply(&mut msaa_app, msaa_hwnd, "set-focus btn1");
    let msaa_outpost = OutpostUnderTest::new(&msaa_app);

    uia_app.send("set-focus btn1");
    let button = client.focused();
    let _ = uia_outpost.uia_focus(&button);
    let (judged, _) = uia_outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Dropped(_)),
        "a repeat with nothing between is dropped, not {judged:?}"
    );

    let reported = msaa_outpost.msaa_focus(msaa_hwnd, 1);
    assert_eq!(reported.node.role, Role::Button);
    uia_outpost.listener_core_focus_elsewhere();
    let (judged, _) = uia_outpost.listener_read(&button);
    assert!(
        matches!(judged, Judged::Read(_)),
        "after another application's MSAA focus, the focus is read, not {judged:?}"
    );

    drop(msaa_outpost);
    drop(uia_outpost);
    msaa_app.quit();
    uia_app.quit();
}

/// NVDA's MSAA duplicate test compares with the last focus queued from any
/// application (`lastQueuedFocusObject`): an MSAA focus repeated at the
/// address of the outpost's last focus is dropped, and once another
/// application's UIA focus has been queued in between, with no foreground
/// change this outpost reports, which Core tells the outpost whose focus
/// it supersedes, it is reported again.
fn a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus() {
    let title = common::unique_title("mockapp-msaa-after-uia");
    let mut msaa_app = common::spawn("small.json", "msaa", &title);
    let msaa_hwnd = common::find_window(&title);
    common::apply(&mut msaa_app, msaa_hwnd, "set-focus btn1");
    let msaa_outpost = OutpostUnderTest::new(&msaa_app);
    let title = common::unique_title("mockapp-uia-before-msaa");
    let mut uia_app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let uia_outpost = OutpostUnderTest::new(&uia_app);
    // The button is node 1, after the root, so its object id is 2.
    let button = DeliveredFact::MsaaFocus {
        hwnd: msaa_hwnd.0 as isize,
        id_object: 2,
        id_child: 0,
    };

    let reported = msaa_outpost.focus(button.clone());
    assert_eq!(reported.node.role, Role::Button);
    msaa_outpost.deliver(button.clone());
    msaa_outpost.settled();

    uia_app.send("set-focus btn1");
    let reported = uia_outpost.uia_focus(&client.focused());
    assert_eq!(reported.node.role, Role::Button);
    msaa_outpost.core_focus_elsewhere();
    let reported = msaa_outpost.focus(button);
    assert_eq!(
        (reported.node.role, reported.node.name.as_deref()),
        (Role::Button, Some("Original Name"))
    );

    drop(uia_outpost);
    drop(msaa_outpost);
    uia_app.quit();
    msaa_app.quit();
}

/// Focuses `id` in mockapp's caret watch fixture and returns the focus the
/// outpost reports, which must be the next thing it says, with its caret
/// for a document and with no text read for anything else, and nothing
/// more.
fn focus_in_caret_fixture(
    app: &mut common::MockApp,
    client: &Client,
    outpost: &OutpostUnderTest,
    id: &str,
) -> NodeSnapshot {
    app.send(&format!("set-focus {id}"));
    let element = client.focused();
    outpost.deliver(outpost.listener_fact(&element));
    let (node, text) = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, text, .. },
            ..
        } => (node, text),
        other => panic!("the outpost said {other:?}, not the focus on {id}"),
    };
    assert_eq!(
        matches!(text.as_deref(), Some(FocusText::Caret { .. })),
        node.role == Role::EditableText,
        "the caret with a document's focus alone: {text:?}"
    );
    if node.role != Role::EditableText {
        assert_eq!(text, None, "no text read for {id}");
    }
    outpost.settled();
    node
}

/// The focus-following subscription moves with the focus as NVDA's local
/// group does, added to the new focus and removed from the old one alone
/// (`docs/design/focus-pipeline.md`, section 17): after the focus leaves
/// the document for a button and returns, a text change the document
/// raises is heard and reported.
fn a_text_change_after_the_focus_returns_to_a_document_is_heard() {
    let title = common::unique_title("mockapp-focus-returns");
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    let notes = focus_in_caret_fixture(&mut app, &client, &outpost, "doc");
    assert_eq!(notes.name.as_deref(), Some("Notes"));
    let ok = focus_in_caret_fixture(&mut app, &client, &outpost, "ok");
    assert_eq!(ok.name.as_deref(), Some("OK"));
    let returned = focus_in_caret_fixture(&mut app, &client, &outpost, "doc");
    assert_eq!(returned.id, notes.id);

    app.send("text-changed doc");
    // UIA raises a value change `WinEvent` alongside the text change.
    outpost.heard(&[
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::TextChanged { node_id },
            ..
        } => assert_eq!(node_id, notes.id),
        other => panic!("the outpost said {other:?}, not the document's text change"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A change raised after a UIA focus event, which the outpost's own
/// subscriptions bring, waits in the outpost behind the focus's marker
/// until the listener's read takes its place, so the focus is reported
/// first, as NVDA's UIA events pass in order through the one thread that
/// judges them (`docs/design/focus-pipeline.md`, section 12.4). The button
/// was the focus before, so its subscription hears its rename.
fn a_change_raised_after_a_focus_event_waits_for_the_focus_read() {
    let title = common::unique_title("mockapp-marker-wait");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let button = outpost.uia_focus(&button).node;

    app.send("set-focus slider1");
    let slider = client.focused();
    let (read, _) = outpost.listener_read(&slider);
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 3 });
    app.send("set-name btn1 Renamed");
    outpost.heard(&[
        Heard::UiaProperty(UIA_NamePropertyId.0),
        Heard::Msaa(WinEventKind::NameChange),
    ]);
    outpost.deliver(read.into_fact(3, FocusEvent::Focus, None));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => assert_eq!(node.name.as_deref(), Some("Level")),
        other => panic!("the outpost said {other:?}, not the slider's focus first"),
    }
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Name(name),
                    child_count: None,
                },
            ..
        } => assert_eq!((node_id, name.as_deref()), (button.id, Some("Renamed"))),
        other => panic!("the outpost said {other:?}, not the button's new name"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A combo box opening into its list raises its expand state's change
/// before its item's focus, but the change reaches the outpost through its
/// own subscription to the combo box, and the focus through the listener,
/// whose marker the outpost can take in first. The item is inside the
/// combo box, the focus before, so the combo box's change queued behind
/// the item's marker is planned ahead of the item's read, in the
/// provider's order, as NVDA's one event queue sees it
/// (`docs/design/focus-pipeline.md`, section 12.4): the outpost reports the
/// combo box's states, then the item, and Core says "expanded", then the
/// item, as NVDA does in Windows Settings. As the provider does, mockapp
/// raises the change while the combo box has the keyboard focus and then
/// moves the focus to the item; the test delivers the item's marker before
/// the change is raised and its read after the change is heard, so the
/// order the outpost takes them in is fixed.
fn a_change_of_the_old_focus_queued_behind_a_focus_inside_it_is_handled_first() {
    let title = common::unique_title("mockapp-expand-order");
    let mut app = common::spawn("combo_box.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus hour");
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let combo_box = focus_changed(outpost.next());
    outpost.settled();
    let NormalizedEvent::FocusChanged { node, .. } = &combo_box else {
        unreachable!("a focus change");
    };
    let combo_box_id = node.id;
    assert_eq!(
        reduced(&mut state, combo_box).1,
        [
            vec![
                SegmentContent::Label("Mockapp Combo Box Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Hour".to_owned()),
                SegmentContent::Role(Role::ComboBox),
                SegmentContent::Value("Nine".to_owned()),
                SegmentContent::State(State::Collapsed),
            ],
        ]
    );

    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 3 });
    // The whole state set, the keyboard focus still the combo box's.
    app.send("set-states hour focused focusable expanded");
    // UIA raises a state change `WinEvent` alongside the property change.
    outpost.heard(&[
        Heard::UiaProperty(UIA_ExpandCollapseExpandCollapseStatePropertyId.0),
        Heard::Msaa(WinEventKind::StateChange),
    ]);
    app.send("set-focus nine");
    let (read, _) = outpost.listener_read(&client.focused());
    outpost.deliver(read.into_fact(3, FocusEvent::Focus, None));
    let change = match outpost.next() {
        OutpostToSupervisor::Event { event, .. } => event,
        other => panic!("the outpost said {other:?}, not the combo box's change"),
    };
    let NormalizedEvent::PropertyChanged {
        node_id,
        change: PropertyChange::States(states),
        child_count: None,
    } = &change
    else {
        panic!("the outpost said {change:?}, not the combo box's states first");
    };
    assert_eq!(
        (*node_id, *states),
        (
            combo_box_id,
            StateSet::from_iter([State::Focused, State::Focusable, State::Expanded])
        )
    );
    let item = focus_changed(outpost.next());
    outpost.settled();
    let NormalizedEvent::FocusChanged { node, .. } = &item else {
        unreachable!("a focus change");
    };
    assert_eq!(node.name.as_deref(), Some("Nine"));

    assert_eq!(
        reduced(&mut state, change).1,
        [vec![SegmentContent::State(State::Expanded)]]
    );
    assert_eq!(
        reduced(&mut state, item).1,
        [vec![SegmentContent::Label("Nine".to_owned())]]
    );

    drop(outpost);
    app.quit();
}

/// A value change the old focus raises after the focus has moved into it
/// keeps its place behind the focus, though the outpost's subscription
/// queues it between the focus's marker and its read: only the old
/// focus's state changes are planned ahead of a focus inside it
/// (`docs/design/focus-pipeline.md`, section 12.4), since NVDA speaks a
/// value change only for the focus itself and so says nothing for this
/// one. mockapp moves the focus from the combo box to its item and then
/// changes the combo box's value; the test delivers the item's marker
/// before both and its read after the change is heard. The outpost reports
/// the item first and the combo box's value change after it, which says
/// nothing.
fn a_value_change_of_the_old_focus_queued_behind_a_focus_inside_it_keeps_its_place() {
    let title = common::unique_title("mockapp-expand-value");
    let mut app = common::spawn("combo_box.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus hour");
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let combo_box = focus_changed(outpost.next());
    outpost.settled();
    let NormalizedEvent::FocusChanged { node, .. } = &combo_box else {
        unreachable!("a focus change");
    };
    let combo_box_id = node.id;
    reduced(&mut state, combo_box);

    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 3 });
    app.send("set-focus nine");
    app.send("set-value hour Ten");
    // UIA raises a value change `WinEvent` alongside the property change.
    outpost.heard(&[
        Heard::UiaProperty(UIA_ValueValuePropertyId.0),
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    let (read, _) = outpost.listener_read(&client.focused());
    outpost.deliver(read.into_fact(3, FocusEvent::Focus, None));
    let item = focus_changed(outpost.next());
    let NormalizedEvent::FocusChanged { node, .. } = &item else {
        unreachable!("a focus change");
    };
    assert_eq!(node.name.as_deref(), Some("Nine"));
    assert_eq!(
        reduced(&mut state, item).1,
        [vec![SegmentContent::Label("Nine".to_owned())]]
    );
    let change = match outpost.next() {
        OutpostToSupervisor::Event { event, .. } => event,
        other => panic!("the outpost said {other:?}, not the combo box's value change"),
    };
    outpost.settled();
    assert_eq!(
        change,
        NormalizedEvent::ValueChanged {
            node_id: combo_box_id,
            value: Some("Ten".to_owned()),
        }
    );
    assert_eq!(
        reduced(&mut state, change).1,
        Vec::<Vec<SegmentContent>>::new()
    );

    drop(outpost);
    app.quit();
}

/// A windowless UIA focus is tested by NVDA's acceptance test against its
/// nearest window, in the listener, after the repeat check and before the
/// focus is read, as NVDA tests it before it queues a focus
/// (`docs/design/focus-pipeline.md`, sections 16.2 and 16.3): with another
/// application's window in front, mockapp's button's focus is dropped
/// after one call, the nearest window's, and the outpost, handed the drop,
/// says nothing. A dropped focus never becomes the last focus accepted, so
/// when the same element's focus comes again once its window is in front,
/// as a console's text area's does, focused before its window comes in
/// front and again after, it is no repeat: it is read and reported.
fn a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is() {
    let title = common::unique_title("mockapp-focus-in-front");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.set_foreground(common::another_applications_window());
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a focus whose window is not in front: {judged:?}");
    };
    assert_eq!(reason, "its window is not related to the foreground window");
    assert_eq!(calls.uia, 1, "the nearest window, and nothing else");
    outpost.deliver(Judged::Dropped(reason).into_fact(0, FocusEvent::Focus, None));
    outpost.settled();

    outpost.set_foreground(hwnd);
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );

    drop(outpost);
    app.quit();
}

/// A foreground event for a window read through UI Automation is dropped,
/// even with the window in front, as NVDA drops every MSAA event for such
/// a window (`docs/design/focus-pipeline.md`, section 16.4): the outpost
/// says nothing for it, and the focus inside the window that follows is
/// reported alone, the window first among its ancestors, with no
/// foreground report before it.
fn a_foreground_event_for_a_window_read_through_uia_is_dropped() {
    let title = common::unique_title("mockapp-uia-foreground");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.settled();

    app.send("set-focus btn1");
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );

    drop(outpost);
    app.quit();
}

/// The listener's copy of a `WinEvent` focus on the client object of
/// mockapp's window, a UIA window, with no UIA focus event of its own.
fn winevent_focus(outpost: &OutpostUnderTest, hwnd: HWND) {
    winevent_focus_observed(outpost, hwnd, outpost.observed_now());
}

/// [`winevent_focus`], observed at `observed_at_us`.
fn winevent_focus_observed(outpost: &OutpostUnderTest, hwnd: HWND, observed_at_us: u64) {
    outpost.deliver_win_event_observed(
        DeliveredFact::MsaaFocus {
            hwnd: hwnd.0 as isize,
            id_object: -4,
            id_child: 0,
        },
        WinEventStamp { thread: 1, time: 1 },
        observed_at_us,
    );
}

/// The focus change the outpost said, which must be one.
fn focus_changed(message: OutpostToSupervisor) -> NormalizedEvent {
    match message {
        OutpostToSupervisor::Event {
            event: event @ NormalizedEvent::FocusChanged { .. },
            ..
        } => event,
        other => panic!("the outpost said {other:?}, not a focus change"),
    }
}

/// What Core does with `event` from the outpost, and what it says.
fn reduced(
    state: &mut SrState,
    mut event: NormalizedEvent,
) -> (Vec<Effect>, Vec<Vec<SegmentContent>>) {
    event.assign_outpost(OutpostId(1));
    let effects = verbatim_core::reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event,
        },
    );
    let spoken = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance) => Some(
                utterance
                    .segments
                    .iter()
                    .map(|segment| segment.content.clone())
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    (effects, spoken)
}

/// A `WinEvent` focus in a UIA window that no UIA focus follows, as UI
/// Automation sometimes leaves a new process's focus events undelivered
/// (`docs/design/listener-without-connections.md`, risk 2): the outpost,
/// which dropped the copy for UIA, opens its gap record, and once a second
/// has passed, its timer reaches the bound and it reads its application's
/// focus itself and reports it with its ancestors (decision 11), once. The
/// time is the test's, which nothing moves but the test.
fn a_winevent_focus_no_uia_focus_follows_is_read_once_its_bound_has_passed() {
    let title = common::unique_title("mockapp-unanswered-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_millis(999));
    outpost.settled();

    outpost.pass_time(Duration::from_millis(1));
    outpost.settle();
    let reported = outpost.reported_focus();
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window that its UIA focus follows is
/// reported once, from the listener's read, and nothing is read after the
/// bound.
fn a_winevent_focus_its_uia_focus_follows_is_reported_once() {
    let title = common::unique_title("mockapp-answered-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window whose UIA focus event came but was
/// dropped because its sender had no window above it, as a new File
/// Explorer window's first focuses are once their senders are gone
/// (`docs/design/listener-without-connections.md`, risk 13): the drop
/// answers the outpost's record as its marker resolves, the record's read
/// taking the marker's place, and the outpost reads its focus without
/// waiting for the bound and reports it once.
fn a_winevent_focus_whose_uia_focus_has_no_window_is_read_at_once() {
    let title = common::unique_title("mockapp-gone-sender");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 4 });
    outpost.listener_outcome(
        4,
        Judged::Dropped(NO_WINDOW),
        (outpost.observed_now(), None),
    );
    let reported = outpost.reported_focus();
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The focused element a focus-now query reads, when it belongs to another
/// process than the one serving the focus window, as
/// `GetFocusedElementBuildCache` returns once the keyboard focus has moved
/// to another application (`experiments/getfocusedelement-after-the-focus-moves/`):
/// the outpost checks its cached process before anything else is done with
/// it, answers no focused control, and sends the other application
/// nothing after the read itself, no remote operation, walk or
/// subscription. The read is the outpost's reader returning the other
/// mockapp's focused element, its cache filled by the outpost's request.
fn a_focused_element_of_another_process_is_not_read_further() {
    let title = common::unique_title("mockapp-focused-here");
    let app = common::spawn("small.json", "uia", &title);
    let other_title = common::unique_title("mockapp-focused-elsewhere");
    let mut other = common::spawn("small.json", "uia", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let other_client = Client::new(other_hwnd);
    let mut outpost = OutpostUnderTest::new(&app);

    other.send("set-focus btn1");
    outpost.read_focus_as(other_client.focused().live().element());
    common::reset_hits(other_hwnd);
    let (now, _) = outpost.focus_now_answer();
    assert_eq!(now.focus, None, "no focused control is answered");
    assert_eq!(
        common::read_hits(other_hwnd),
        [
            ("ProviderOptions", 5),
            ("GetPatternProvider", 14),
            ("GetPropertyValue", 20),
            ("HostRawElementProvider", 3),
            ("GetRuntimeId", 2),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
        "the other application's calls: the read's cache alone"
    );

    drop(outpost);
    other.quit();
    app.quit();
}

/// An MSAA focus event naming another application's window, here another
/// mockapp's, in front, as any process can raise one with
/// `NotifyWinEvent`: the outpost neither probes nor reads that window, so
/// the other mockapp answers nothing, no `WM_GETOBJECT` and no provider
/// call, and nothing is reported.
fn a_focus_event_naming_another_applications_window_is_not_read() {
    let title = common::unique_title("mockapp-event-here");
    let app = common::spawn("small.json", "uia", &title);
    let other_title = common::unique_title("mockapp-event-elsewhere");
    let other = common::spawn("small.json", "uia", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let outpost = OutpostUnderTest::new(&app);
    outpost.set_foreground(other_hwnd);

    common::reset_hits(other_hwnd);
    outpost.deliver_win_event(
        DeliveredFact::MsaaFocus {
            hwnd: other_hwnd.0 as isize,
            id_object: -4,
            id_child: 0,
        },
        WinEventStamp { thread: 1, time: 1 },
    );
    outpost.settled();
    assert_eq!(
        common::read_hits(other_hwnd),
        [],
        "the other application's calls"
    );

    drop(outpost);
    other.quit();
    app.quit();
}

/// A UIA focus the listener read in a window this outpost judges MSAA,
/// here by Core's backend override, is dropped: the outpost's own verdict
/// decides which backend's focus counts in each of its windows, and in an
/// MSAA window its MSAA focus speaks for it.
fn a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped() {
    let title = common::unique_title("mockapp-report-in-msaa-window");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let read = outpost.listener_fact(&client.focused());
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::SetBackendOverride {
            backend_override: Some(Backend::Msaa),
            pid: None,
        });
    outpost.deliver(read);
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window whose UIA focus event came but was
/// dropped because its element no longer had the keyboard focus, as in
/// ordinary fast navigation: the drop leaves the outpost's record open
/// (Dickson, 2026-10-21), and the focus is read at the bound.
fn a_winevent_focus_whose_uia_focus_is_no_longer_focused_is_read_at_its_bound() {
    let title = common::unique_title("mockapp-unfocused-drop");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 7 });
    outpost.listener_outcome(
        7,
        Judged::Dropped(NOT_FOCUSED),
        (outpost.observed_now(), None),
    );
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    outpost.settle();
    let reported = outpost.reported_focus();
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window whose UIA focus's marker waits while
/// the bound passes two seconds after the copy: the bound's entry waits
/// behind the marker, and the marker's drop, which leaves the record open,
/// lets it through, so the focus is read with the time unmoved.
fn a_winevent_focus_observed_past_its_bound_is_read_at_once() {
    let title = common::unique_title("mockapp-old-copy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 3 });
    outpost.pass_time(Duration::from_secs(2));
    outpost.listener_outcome(
        3,
        Judged::Dropped(NOT_FOCUSED),
        (outpost.observed_now(), None),
    );
    let reported = outpost.reported_focus();
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window that no UIA focus follows: at the
/// bound the outpost reads the focus, and reports it stamped with the
/// copy's observation, not the time of its read. The UIA
/// focus that then comes late is reported under the read's node, which
/// Core's duplicate rule takes silently, so the element is heard once in
/// all.
fn a_winevent_focus_no_uia_focus_follows_is_read_at_its_bound_and_heard_once_when_its_uia_focus_comes_late()
 {
    let title = common::unique_title("mockapp-late-uia-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    let copy_observed_at_us = outpost.observed_now();
    winevent_focus_observed(&outpost, hwnd, copy_observed_at_us);
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    let message = outpost.next();
    let OutpostToSupervisor::Event { observed_at_us, .. } = &message else {
        panic!("the outpost said {message:?}, not a focus change");
    };
    assert_eq!(*observed_at_us, copy_observed_at_us, "the copy's stamp");
    let read = focus_changed(message);
    outpost.settled();
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let late = focus_changed(outpost.next());
    outpost.settled();

    let node_of = |event: &NormalizedEvent| match event {
        NormalizedEvent::FocusChanged { node, .. } => node.id,
        _ => unreachable!("a focus change"),
    };
    assert_eq!(node_of(&late), node_of(&read));
    let mut state = SrState::new();
    let (_, spoken) = reduced(&mut state, read);
    assert_eq!(
        spoken,
        [
            vec![
                SegmentContent::Label("Mockapp Events Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Original Name".to_owned()),
                SegmentContent::Role(Role::Button),
            ],
        ]
    );
    // Core's duplicate rule: nothing at all.
    assert_eq!(reduced(&mut state, late).0, []);

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a console window that no UIA focus follows, as in
/// `conhost_two_windows`'s second window: the read at the bound reports the
/// terminal with the console window, which Core has not been told of, as
/// its one ancestor, as the listener's read of the same focus reports it,
/// so Core announces the window before the terminal. In the console host
/// itself the window's element read inside its provider has no handle or
/// title, and the outpost reads it again through the window's proxy;
/// mockapp's root provider names its host, so its window element has both
/// there too, and that fault is not reproduced here (`docs/uia-bugs.md`,
/// "Inside a remote operation, conhost's window has no handle or title").
fn a_winevent_focus_in_a_console_window_no_uia_focus_follows_is_read_with_its_window() {
    let title = common::unique_title("mockapp-unanswered-console");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus text");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    outpost.settle();
    let read = focus_changed(outpost.next());
    let NormalizedEvent::FocusChanged {
        node,
        ancestors,
        ancestors_unknown,
        ..
    } = &read
    else {
        unreachable!("a focus change");
    };
    assert_eq!((node.role, node.name.as_deref()), (Role::Terminal, None));
    assert_eq!(
        (
            ancestors
                .iter()
                .map(|ancestor| (ancestor.role, ancestor.name.as_deref()))
                .collect::<Vec<_>>(),
            *ancestors_unknown
        ),
        (
            vec![(Role::Window, Some("Mockapp Console Window Fixture"))],
            false
        )
    );
    let NormalizedEvent::FocusChanged {
        text: Some(text), ..
    } = &read
    else {
        panic!("the terminal's focus came without its caret: {read:?}");
    };
    assert!(
        matches!(**text, FocusText::Caret { .. }),
        "the focus came with {text:?}"
    );
    outpost.first_terminal_read(node.id);
    outpost.settled();

    let mut state = SrState::new();
    let (_, spoken) = reduced(&mut state, read);
    assert_eq!(
        spoken,
        [
            vec![
                SegmentContent::Label("Mockapp Console Window Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![SegmentContent::Role(Role::Terminal)],
            vec![
                SegmentContent::Format(TextFormat::FontName("Consolas".to_owned())),
                SegmentContent::Format(TextFormat::FontSize("11.0 pt".to_owned())),
                SegmentContent::Format(TextFormat::Color("black".to_owned())),
                SegmentContent::Text("ready>".to_owned()),
            ],
        ]
    );

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window whose UIA focus's marker is still
/// waiting for its read when the bound passes: the bound's entry waits
/// behind the marker, and the marker's read then answers the record, so the
/// entry finds it closed and the focus is reported once.
fn a_winevent_focus_whose_uia_focus_waits_for_its_read_at_the_bound_is_reported_from_that_read() {
    let title = common::unique_title("mockapp-marker-at-bound");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 5 });
    outpost.pass_time(Duration::from_secs(1));
    let (judged, _) = outpost.listener_read(&client.focused());
    let reported = outpost.focus(judged.into_fact(5, FocusEvent::Focus, None));
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window, after which the focus goes to
/// another application, which the outpost does not see: at the bound the
/// foreground thread's focus window is that application's, so the gap's
/// read is refused with local calls, and nothing is read or said
/// (`docs/design/focus-pipeline-rework.md`, the gap's evidence, in place
/// of the acceptance filter).
fn a_record_whose_focus_went_to_another_application_reads_nothing_at_its_bound() {
    let title = common::unique_title("mockapp-focus-elsewhere");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.set_foreground(common::another_applications_window());
    common::reset_hits(hwnd);
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();
    assert_eq!(common::read_hits(hwnd), [], "refused with local calls");

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a window this outpost judges MSAA is spoken through
/// MSAA and opens no gap record: nothing judges its window again, and no
/// timer is set.
fn a_copy_handled_through_msaa_opens_no_record() {
    let title = common::unique_title("mockapp-msaa-copy");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    let reported = outpost.msaa_focus(hwnd, 1);
    assert_eq!(reported.node.name.as_deref(), Some("Original Name"));
    assert!(!outpost.gap_timer_set(), "no record opened");
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// An event and its UIA focus, observed 10 ms apart, are judged by the one
/// verdict that stood at their observations, however late the second is
/// handled (`docs/design/focus-pipeline-rework.md`, the verdicts): the
/// foreground change's probe of mockapp's busy window times out, so the
/// window is unknown, read through MSAA, and kept so for half a second
/// from the probe's end; the listener's read, handled after the stall and
/// after the keep has passed by the time it is handled, is judged at its
/// own observation by that unknown and dropped, though the window now
/// answers its probe with its provider. Judged when handled, it would be
/// probed again and spoken a second time.
fn an_event_and_its_uia_focus_handled_after_the_keep_are_judged_by_one_verdict() {
    let title = common::unique_title("mockapp-one-verdict");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    let read = outpost.listener_fact(&client.focused());
    let observed = outpost.observed_now();
    let stall = Duration::from_millis(u64::from(RAW_PROBE_TIMEOUT_MS) + 1_500);
    app.stall(stall);
    outpost.deliver_stamped(
        DeliveredFact::Foreground {
            hwnd: hwnd.0 as isize,
        },
        observed,
    );
    let message = outpost
        .messages
        .recv_timeout(stall + common::WAIT_TIMEOUT)
        .expect("the outpost reports the foreground window");
    app.stall_ended(stall);
    let OutpostToSupervisor::Event {
        backend, timing, ..
    } = &message
    else {
        panic!("the outpost said {message:?}, not the foreground window");
    };
    assert_eq!(
        (*backend, timing.calls.window_messages),
        (Backend::Msaa, 1),
        "read through MSAA after one probe that timed out"
    );
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    outpost.deliver_stamped(read, observed + 10_000);
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The user's return within the bound, as a later `WinEvent` focus in the
/// record's top-level window that the outpost drops for UIA, restamps the
/// record, its bound unchanged: the read at the bound, a second from the
/// first copy, is stamped with the return's copy, so Core's stale rule
/// does not take it as older than the focus the user left for.
fn a_return_within_the_bound_is_read_with_the_returns_stamp() {
    let title = common::unique_title("mockapp-return-restamp");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_millis(800));
    let returned_at_us = outpost.observed_now();
    winevent_focus_observed(&outpost, hwnd, returned_at_us);
    outpost.settled();
    outpost.pass_time(Duration::from_millis(200));
    let message = outpost.next();
    let OutpostToSupervisor::Event { observed_at_us, .. } = &message else {
        panic!("the outpost said {message:?}, not a focus change");
    };
    assert_eq!(*observed_at_us, returned_at_us, "the return's stamp");
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A foreground change into the record's top-level window that the outpost
/// accepts restamps the record, at the point after its acceptance, front,
/// repeat and readability tests and before its window's verdict, which
/// then drops it for UIA: the read at the bound is stamped with the
/// foreground change.
fn an_accepted_foreground_change_into_the_records_window_restamps_it() {
    let title = common::unique_title("mockapp-foreground-restamp");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_millis(800));
    let returned_at_us = outpost.observed_now();
    outpost.deliver_stamped(
        DeliveredFact::Foreground {
            hwnd: hwnd.0 as isize,
        },
        returned_at_us,
    );
    outpost.settled();
    outpost.pass_time(Duration::from_millis(200));
    let message = outpost.next();
    let OutpostToSupervisor::Event { observed_at_us, .. } = &message else {
        panic!("the outpost said {message:?}, not a focus change");
    };
    assert_eq!(
        *observed_at_us, returned_at_us,
        "the foreground change's stamp"
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The gap record's timer is set when a `WinEvent` focus the outpost drops
/// for UIA opens the record, and cancelled when the focus's UIA read
/// answers it as its marker resolves.
fn an_answered_record_cancels_its_timer() {
    let title = common::unique_title("mockapp-gap-timer-cancelled");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    assert!(outpost.gap_timer_set(), "the record opened");
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(reported.node.name.as_deref(), Some("Original Name"));
    assert!(!outpost.gap_timer_set(), "the read answered the record");
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The gap's read takes the foreground thread's focus window again once it
/// has read the focused element, and drops the read when the focus window
/// has changed to one the outpost does not judge UIA, here another
/// application's: a focus that moves between the check and
/// `GetFocusedElementBuildCache` is that window's to speak.
fn a_gap_read_whose_focus_window_changes_during_the_read_is_dropped() {
    let title = common::unique_title("mockapp-gap-read-moved");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.focus_moves_during_the_read(common::another_applications_window());
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A UIA focus whose window's class is on NVDA's MSAA class list is skipped
/// by the listener, its nearest window the one call made, as the outpost
/// judges that window MSAA by the same table; a backend override forcing
/// UIA reaches the listener as it reaches the outpost, so the same focus
/// is then read and reported.
fn a_forced_uia_override_has_the_listener_read_a_class_listed_window() {
    let title = common::unique_title("mockapp-class-listed");
    let mut app = common::spawn("msaa_class_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a focus in a class-listed window: {judged:?}");
    };
    assert_eq!(
        reason,
        "its window's class is on NVDA's MSAA list, so its MSAA events speak for it"
    );
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (1, 0, 0),
        "the nearest window alone"
    );

    outpost.listener_forces_uia(true);
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::SetBackendOverride {
            backend_override: Some(Backend::Uia),
            pid: None,
        });
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(reported.node.name.as_deref(), Some("Original Name"));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus the listener forwarded before the outpost's hook was
/// in, observed two seconds before the outpost handles it, as a new
/// process's first focus is, opens the starting outpost's record like its
/// own hook's copy: its bound has already passed, so its timer reaches it at
/// once, and the focus is read, stamped with the copy's observation.
fn a_forwarded_copy_opens_the_starting_outposts_record() {
    let title = common::unique_title("mockapp-forwarded-copy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    let copied_at_us = outpost.observed_now() - 2_000_000;
    winevent_focus_observed(&outpost, hwnd, copied_at_us);
    let message = outpost.next();
    let OutpostToSupervisor::Event { observed_at_us, .. } = &message else {
        panic!("the outpost said {message:?}, not a focus change");
    };
    assert_eq!(*observed_at_us, copied_at_us, "the copy's stamp");
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The idle barrier's round waits while the outpost's record of a
/// `WinEvent` focus in a UIA window is open (`docs/design/communication.md`,
/// section 6.7): the outpost reaches the round's query and holds its
/// answer, and once the bound is reached it reads the focus and answers
/// behind that read.
fn the_idle_answer_waits_for_the_wait_for_a_uia_focus() {
    let title = common::unique_title("mockapp-idle-gap");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.idle_round(4);
    // The outpost holds its answer, and nothing is said.
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    outpost.settle();
    let NormalizedEvent::FocusChanged { node, .. } = focus_changed(outpost.next()) else {
        unreachable!("a focus change");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    assert_eq!(outpost.next(), OutpostToSupervisor::IdleAnswer { round: 4 });
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A focus event repeated on the element the listener last accepted, which
/// still has the keyboard focus, is dropped before anything of it is read,
/// as NVDA's duplicate rule drops it (`docs/design/focus-pipeline.md`,
/// section 16.3): the listener makes three calls, the windowless sender's
/// nearest window for NVDA's native check, which comes first (section 18),
/// the comparison of the sender with that element, and the element's
/// keyboard focus, and the outpost says nothing. Once another focus has been accepted in between,
/// the same element's focus is read and reported again.
fn a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read() {
    let title = common::unique_title("mockapp-repeated-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = outpost.uia_focus(&client.focused());
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a repeated focus: {judged:?}");
    };
    assert_eq!(
        reason,
        "a repeat of the last focus, which still has the keyboard focus"
    );
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (3, 0, 0),
        "the nearest window, the comparison, and the keyboard focus read, and nothing else"
    );
    outpost.deliver(Judged::Dropped(reason).into_fact(0, FocusEvent::Focus, None));
    outpost.settled();

    app.send("set-focus slider1");
    let slider = outpost.uia_focus(&client.focused());
    assert_eq!(slider.node.name.as_deref(), Some("Level"));
    app.send("set-focus btn1");
    let again = outpost.uia_focus(&client.focused());
    assert_eq!(
        (again.node.id, again.node.name.as_deref()),
        (button.node.id, Some("Original Name"))
    );

    drop(outpost);
    app.quit();
}

/// A focus the listener read that Core has not judged yet is no repeat's
/// evidence (decision D14, "Where the rules run"): Core may yet refuse it,
/// so a second event on the same element, still focused, is read again,
/// with no comparison made, and Core's own duplicate rule then drops it.
fn a_focus_waiting_for_core_s_judgement_is_no_repeat() {
    let title = common::unique_title("mockapp-repeat-unjudged");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let _ = outpost.listener_fact(&client.focused());
    let (judged, _) = outpost.listener_read(&client.focused());
    assert!(
        matches!(judged, Judged::Read(_)),
        "a focus waiting for Core's judgement is read again, not {judged:?}"
    );

    drop(outpost);
    app.quit();
}

/// A UIA menu opening is judged and read as a focus on the menu, as NVDA's
/// automation event handler maps it to one, by NVDA's checks but the
/// duplicate test, which is its focus handler's alone: an opening on the
/// element that is the last focus, and still has the keyboard focus, is
/// read in full; one whose element has lost the keyboard focus is dropped,
/// as NVDA's `shouldAllowUIAFocusEvent` drops it.
fn a_menu_opening_is_judged_as_a_focus_on_the_menu() {
    let title = common::unique_title("mockapp-menu-opening");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let reported = outpost.uia_focus(&button);
    let (judged, _) = outpost.listener_read_as(&button, FocusEvent::MenuOpened);
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the menu opening: {judged:?}");
    };
    assert_eq!(
        (read.snapshot.name.as_deref(), read.ancestors.is_some()),
        (reported.node.name.as_deref(), true),
        "read with its ancestors"
    );

    app.send("set-focus slider1");
    let (judged, _) = outpost.listener_read_as(&button, FocusEvent::MenuOpened);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a menu without the keyboard focus: {judged:?}");
    };
    assert_eq!(reason, "its element no longer has the keyboard focus");

    drop(outpost);
    app.quit();
}

/// A UIA menu opening whose sender UI Automation counts as the current
/// focus, the last focus the listener accepted with no newer focus queued
/// since, is judged as that focus, as NVDA's automation event handler
/// redirects it to its focus object (`docs/design/focus-pipeline.md`,
/// section 18): with the focus's window and without NVDA's native check,
/// so the windowless sender's nearest window is not read. Five calls: the
/// comparison (`CompareElements`), the focus's keyboard focus, and the
/// read, which is the focus's own element again: its keyboard focus as the
/// focus before, its cache, and one step up to its known parent.
fn a_menu_opening_on_the_current_focus_is_judged_as_that_focus() {
    let title = common::unique_title("mockapp-menu-opening-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let _ = outpost.uia_focus(&button);
    let (judged, calls) = outpost.listener_read_as(&button, FocusEvent::MenuOpened);
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the menu opening: {judged:?}");
    };
    assert_eq!(read.snapshot.name.as_deref(), Some("Original Name"));
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (5, 0, 0),
        "the comparison, the focus's keyboard focus, and the read, and no nearest window"
    );

    drop(outpost);
    app.quit();
}

/// A UIA menu opening whose sender is not the current focus goes through
/// NVDA's native check, its windowless sender's nearest window read: five
/// calls, the comparison with the current focus, the nearest window, the
/// sender's keyboard focus, and the read's two, its cache and one step up
/// to its known parent. Once Core has queued a focus the listener did not
/// accept, as an MSAA focus, the current focus is no UIA element the
/// listener holds, as NVDA's focus object is then no UIA object, and
/// nothing is compared: five calls, the read now also asking whether the
/// slider it read just before, under the same runtime id, still has the
/// keyboard focus.
fn a_menu_opening_on_another_element_is_judged_by_its_own_window() {
    let title = common::unique_title("mockapp-menu-opening-other");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let _ = outpost.uia_focus(&client.focused());
    app.send("set-focus slider1");
    let slider = client.focused();
    let (judged, calls) = outpost.listener_read_as(&slider, FocusEvent::MenuOpened);
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the menu opening: {judged:?}");
    };
    assert_eq!(read.snapshot.name.as_deref(), Some("Level"));
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (5, 0, 0),
        "the comparison, the nearest window, the keyboard focus, and the read"
    );

    outpost.listener_core_focus_elsewhere();
    let (judged, calls) = outpost.listener_read_as(&slider, FocusEvent::MenuOpened);
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the menu opening: {judged:?}");
    };
    assert_eq!(read.snapshot.name.as_deref(), Some("Level"));
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (5, 0, 0),
        "the nearest window, the keyboard focus, and the read, and no comparison"
    );

    drop(outpost);
    app.quit();
}

/// A windowless UIA focus whose read's time is up once its nearest window
/// has been looked up is judged by NVDA's native check from its window's
/// class alone, which needs no call (`FocusReader::native_window`): on no
/// class list, it is read on, its keyboard focus taken from its event's
/// cache, and reported from its event, its nearest window the one call
/// made: no probe, which would be a window message, and nothing else once
/// the time is up.
fn a_focus_out_of_time_is_dropped_only_by_its_windows_class() {
    let title = common::unique_title("mockapp-focus-kept-verdict");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus slider1");
    let _ = outpost.uia_focus(&client.focused());
    app.send("set-focus btn1");
    // The read's first reading of the time sets its budget's end; every
    // later one is past it.
    outpost.run_out_of_time_after(1);
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the focus: {judged:?}");
    };
    assert_eq!(read.snapshot.name.as_deref(), Some("Original Name"));
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (1, 0, 0),
        "the nearest window alone: no probe, keyboard focus read, or read's call"
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus out of time whose event's cache says its element did not
/// have the keyboard focus is dropped, the cache standing in for NVDA's
/// live read of it (`shouldAllowUIAFocusEvent`), which the time no longer
/// allows: the nearest window is the one call made.
fn a_focus_out_of_time_whose_event_says_it_lacked_the_keyboard_focus_is_dropped() {
    let title = common::unique_title("mockapp-focus-out-of-time-unfocused");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus slider1");
    let _ = outpost.uia_focus(&client.focused());
    let button = client.named("Original Name");
    // The read's first reading of the time sets its budget's end; every
    // later one is past it.
    outpost.run_out_of_time_after(1);
    let (judged, calls) = outpost.listener_read(&button);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a focus its event said was not focused: {judged:?}");
    };
    assert_eq!(
        reason,
        "its event says it did not have the keyboard focus, and its read's time was up"
    );
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (1, 0, 0),
        "the nearest window alone"
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus whose live keyboard focus read fails by UIA's transaction
/// timeout is let through, as NVDA lets any COM error of that read through
/// (`shouldAllowUIAFocusEvent`), though its event's cache says it lacked the
/// keyboard focus, and is reported from its event, as the focus with its
/// ancestors unknown, once its read, still held, fails too. The sender is
/// mockapp's window element, whose window's verdict an earlier focus kept,
/// so that the live read is the read's first provider call, which mockapp
/// holds past the timeout.
fn a_focus_whose_keyboard_focus_read_times_out_is_reported() {
    const TIMEOUT_MS: u32 = 300;
    let title = common::unique_title("mockapp-focus-read-timed-out");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus slider1");
    let _ = outpost.uia_focus(&client.focused());
    let request = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let window = client
        .uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element")
        .refresh(&request)
        .expect("the window's element, filled as a focus event's");
    assert!(
        window.read().lacked_keyboard_focus(),
        "the event's cache says the window's element lacked the keyboard focus"
    );
    let timeouts: IUIAutomation2 = client.uia.client().cast().expect("IUIAutomation2");
    // SAFETY: reading a timeout returns a plain integer.
    let usual = unsafe { timeouts.TransactionTimeout() }.expect("the transaction timeout");
    // SAFETY: setting a timeout takes a plain integer.
    unsafe { timeouts.SetTransactionTimeout(TIMEOUT_MS) }.expect("the timeout set");
    app.hold();
    let (judged, _) = outpost.listener_read(&window);
    // SAFETY: as above.
    unsafe { timeouts.SetTransactionTimeout(usual) }.expect("the timeout restored");
    assert_eq!(app.held(), "GetPropertyValue");
    app.release();
    let Judged::Read(read) = judged else {
        panic!(
            "the listener did not report a focus whose keyboard focus read timed out: {judged:?}"
        );
    };
    // Reported whole as its event's cache gives it, its own class and
    // window included, with the Focused state, its ancestors unknown, in
    // its own window.
    assert_eq!(
        *read,
        UiaFocusRead {
            hwnd: hwnd.0 as isize,
            snapshot: UiaSnapshotFact {
                runtime_id: window.runtime_id(),
                role: Role::Window,
                name: Some("Mockapp Events Fixture".to_owned()),
                value: None,
                states: [State::Focused].into_iter().collect::<StateSet>(),
                details: NodeDetails {
                    class_name: Some("VerbatimMockAppWindow".to_owned()),
                    window: Some(handle(hwnd)),
                    ..NodeDetails::default()
                },
            },
            ancestors: None,
            ancestors_end: verbatim_outpost::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: Some(hwnd.0 as isize),
            same_as_last: None,
            bound_us: 2_000_000,
        }
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus repeated on the last focus accepted, which still has the
/// keyboard focus, is reported when its read's time is up once its
/// nearest window has been looked up: NVDA's repeat check needs that
/// element's keyboard focus read live, which the time no longer allows, so
/// it is not made, a deliberate divergence from NVDA, which would drop it
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
fn a_repeated_focus_out_of_time_is_reported() {
    let title = common::unique_title("mockapp-repeat-out-of-time");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let _ = outpost.uia_focus(&client.focused());
    // The read's first reading of the time sets its budget's end; every
    // later one is past it.
    outpost.run_out_of_time_after(1);
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the repeated focus: {judged:?}");
    };
    assert_eq!(read.snapshot.name.as_deref(), Some("Original Name"));
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (1, 0, 0),
        "the nearest window alone: no comparison or keyboard focus read"
    );

    drop(outpost);
    app.quit();
}

/// A UIA menu opening compared with the current focus, which it is not,
/// asks the read's time before it looks up its windowless sender's nearest
/// window, the comparison having been the read's first call: once the time
/// is up, it is dropped after the comparison alone.
fn a_menu_opening_out_of_time_after_its_comparison_is_dropped_before_its_window() {
    let title = common::unique_title("mockapp-menu-opening-out-of-time");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let _ = outpost.uia_focus(&client.focused());
    app.send("set-focus slider1");
    // The read's first reading of the time sets its budget's end and its
    // second, before the comparison, is within it; every later one is past
    // it.
    outpost.run_out_of_time_after(2);
    let (judged, calls) = outpost.listener_read_as(&client.focused(), FocusEvent::MenuOpened);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a menu opening out of time: {judged:?}");
    };
    assert_eq!(
        reason,
        "its read ran out of time before its window was looked up"
    );
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (1, 0, 0),
        "the comparison, and no nearest window"
    );

    drop(outpost);
    app.quit();
}

/// A focus on a menu item that no pattern makes checkable reads its legacy
/// MSAA state, where a Windows Forms menu item says it is checked, with
/// one live read after the focus's classic read, made for an element the
/// read maps to a menu item: seven calls, the nearest window, the
/// comparison, the keyboard focus, the read's three (the item's cache, and
/// steps up to the menu and to the window the focus before had), and the
/// state.
fn a_menu_items_legacy_state_is_read_with_its_focus() {
    let title = common::unique_title("mockapp-menu-item-focus");
    let mut app = common::spawn("modal_owner.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus close");
    let _ = outpost.uia_focus(&client.focused());
    app.send("set-focus copy");
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(
        (reported.node.name.as_deref(), reported.node.role),
        (Some("Copy"), Role::MenuItem)
    );
    assert_eq!(
        (
            reported.calls.uia,
            reported.calls.msaa,
            reported.calls.window_messages
        ),
        (7, 0, 0),
        "the nearest window, the comparison, the keyboard focus, the read, and the legacy state"
    );

    drop(outpost);
    app.quit();
}

/// The console host's window is the parent of its text area, reports the
/// keyboard focus whenever the text area has it, and raises focus events
/// around the text area's; those are never the focus (NVDA refuses them
/// whatever the element reports, `consoleUIAWindow` in
/// `NVDAObjects/UIA/winConsoleUIA.py`). So the focus listener drops a UIA
/// focus on a console window's own element before it reads the element,
/// even while that element reports the keyboard focus: the text area's own
/// focus is the one reported. The refusal is the console window's class,
/// read locally, so no call into the console is made at all: the native
/// check is its class alone, the check of the console's provider being its
/// outpost's (`docs/design/focus-pipeline-rework.md`, the verdicts), and
/// nothing of the element is read.
fn a_console_windows_own_focus_is_never_reported() {
    let title = common::unique_title("mockapp-console-window");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus root");
    let listener = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let window = client
        .uia
        .window_element(hwnd.0 as isize, &listener)
        .expect("mockapp's root element");
    let (judged, calls) = outpost.listener_read(&window);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read the console window's own focus: {judged:?}");
    };
    assert_eq!(reason, "the console window's own focus");
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (0, 0, 0),
        "no call"
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus, None));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A console's text area's focus is read with no check of the console's
/// provider, which is its outpost's (`docs/design/focus-pipeline-rework.md`,
/// the verdicts): five calls, the sender's nearest window, the keyboard
/// focus, and the read's three (the text area's cache, and steps up to the
/// console window and to the desktop's root), and no window message. The
/// outpost, which checks the console's provider by the window's handle,
/// reports it as the console's terminal, and its caret.
fn a_console_text_areas_focus_is_checked_from_the_text_area() {
    let title = common::unique_title("mockapp-console-text-area");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus text");
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Read(_) = &judged else {
        panic!("the listener did not read the text area's focus: {judged:?}");
    };
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (5, 0, 0),
        "the nearest window, the keyboard focus, and the read"
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus, None));
    let (node, ancestors) = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    ancestors,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => (node, ancestors),
        other => panic!("the outpost said {other:?}, not the text area's focus with its caret"),
    };
    assert_eq!((node.role, node.name), (Role::Terminal, None));
    assert_eq!(
        ancestors
            .iter()
            .map(|ancestor| ancestor.name.as_deref())
            .collect::<Vec<_>>(),
        [Some("Mockapp Console Window Fixture")]
    );
    outpost.first_terminal_read(node.id);
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A UIA focus in a window with no UIA provider, on no class list, through
/// UIA's MSAA proxy: the listener makes no backend verdict and reads it,
/// three calls, the keyboard focus and the read's two (the window's cache
/// and the step up to the desktop's root), with no window message, for
/// which the proxy makes fourteen calls into the application's
/// `IAccessible`, and the outpost, whose verdict on the window is
/// MSAA, drops the read, the window's MSAA focus speaking for it
/// (`docs/design/focus-pipeline-rework.md`, the verdicts: the listener's
/// reads in MSAA windows off the class lists are an accepted cost).
fn a_focus_in_a_window_with_no_uia_provider_is_read_and_dropped_by_its_outpost() {
    let title = common::unique_title("mockapp-msaa-window-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    let listener = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    app.send("set-focus root");
    let window = client
        .uia
        .any_window_element(hwnd.0 as isize, &listener)
        .expect("mockapp's window element, through the MSAA proxy");
    common::reset_hits(hwnd);
    let (judged, calls) = outpost.listener_read(&window);
    let Judged::Read(_) = judged else {
        panic!("the listener did not read the focus: {judged:?}");
    };
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (3, 0, 0),
        "the keyboard focus and the read's two, the window's cache and the step to the desktop's root, and no window message"
    );
    // What the proxy asks of the application's `IAccessible` for them, the
    // listener's exposure to a busy MSAA application.
    assert_eq!(
        common::read_hits(hwnd),
        [
            ("get_accName", 1),
            ("get_accValue", 3),
            ("get_accRole", 1),
            ("get_accState", 6),
            ("get_accHelp", 1),
            ("get_accKeyboardShortcut", 1),
            ("accLocation", 1),
        ]
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus, None));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The focus's help text and `ControllerFor` relation are followed, as
/// NVDA's local group follows them: a help text change is reported as the
/// focus's new description, and a search box's relation naming its list
/// and then nothing as its suggestions opening and closing, each change
/// in one call, the refresh of the box's cache, which reads the relation
/// with the rest (`docs/performance.md`).
fn a_focus_s_help_text_and_suggestions_are_followed() {
    let title = common::unique_title("mockapp-suggestions");
    let mut app = common::spawn("controller.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    // The search box has no text pattern in mockapp, so its focus carries
    // the outpost's word that it has no text.
    app.send("set-focus search");
    let (judged, _) = outpost.listener_read(&client.focused());
    assert!(matches!(judged, Judged::Read(_)), "dropped: {judged:?}");
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus, None));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::NoText) => node,
        other => panic!("the outpost said {other:?}, not the search box's focus with no text"),
    };
    outpost.settled();

    app.send("set-description search Type to search");
    outpost.heard(&[Heard::UiaProperty(UIA_HelpTextPropertyId.0)]);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Description(description),
                    child_count: None,
                },
            ..
        } => assert_eq!(
            (node_id, description.as_deref()),
            (node.id, Some("Type to search"))
        ),
        other => panic!("the outpost said {other:?}, not the focus's new description"),
    }
    outpost.settled();

    for (command, open) in [
        ("set-controller-for search results", true),
        ("set-controller-for search", false),
    ] {
        app.send(command);
        outpost.heard(&[Heard::UiaProperty(UIA_ControllerForPropertyId.0)]);
        match outpost.next() {
            OutpostToSupervisor::Event {
                event:
                    NormalizedEvent::PropertyChanged {
                        node_id,
                        change: PropertyChange::Suggestions(reported),
                        child_count: None,
                    },
                timing,
                ..
            } => assert_eq!(
                (node_id, reported, timing.calls.uia),
                (node.id, open, 1),
                "{command}"
            ),
            other => panic!("the outpost said {other:?}, not the suggestions after {command}"),
        }
        outpost.settled();
    }

    drop(outpost);
    app.quit();
}

/// A selection from a windowless element is given its nearest window by
/// the listener's reading thread, as NVDA walks up to it
/// (`getNearestWindowHandle`), for the outpost's window and acceptance
/// tests; a notification from one that has a window above it is passed on
/// unchanged.
fn a_windowless_selection_is_given_its_nearest_window() {
    let title = common::unique_title("mockapp-nearest-window");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item1");
    let item = client.focused();
    let snapshot = || verbatim_outpost::protocol::UiaSnapshotFact {
        runtime_id: item.runtime_id(),
        role: Role::ListItem,
        name: None,
        value: None,
        states: verbatim_model::StateSet::new(),
        details: verbatim_model::NodeDetails::default(),
    };
    let selection = DeliveredFact::UiaSelection {
        hwnd: 0,
        focus_window: 0,
        snapshot: snapshot(),
    };
    match outpost.listener_check(&item, selection) {
        Judged::Passed(fact) => assert!(
            matches!(*fact, DeliveredFact::UiaSelection { focus_window, .. } if focus_window == hwnd.0 as isize),
            "{fact:?}"
        ),
        other => panic!("the selection was not passed on: {other:?}"),
    }
    let notification = DeliveredFact::UiaNotification {
        hwnd: 0,
        snapshot: snapshot(),
        notification: verbatim_model::Notification {
            kind: verbatim_model::NotificationKind::Other,
            processing: verbatim_model::NotificationProcessing::All,
            display_string: Some("saved".to_owned()),
            activity_id: None,
        },
    };
    assert!(
        matches!(outpost.listener_check(&item, notification.clone()), Judged::Passed(fact) if *fact == notification),
        "the notification passes unchanged"
    );

    drop(outpost);
    app.quit();
}

/// An item selected inside a focused list is reported with its states and
/// its position in its set from the event's cache, which the listener's
/// registration fills with the event (`verbatim_uia::ListenerSelection`):
/// the outpost reads nothing of the item, its one call the test every
/// selection is given first for a list the focus controls, a read of the
/// focus's `ControllerFor`. The selection is mockapp's own event, caught by
/// a subscription shaped as the listener's and captured by its own
/// `capture_selection`, then checked by the listener's reading thread's
/// check.
fn a_selection_in_a_focused_list_carries_its_states_and_position_from_the_cache() {
    let title = common::unique_title("mockapp-selection-from-cache");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus folder");
    let focus = outpost.uia_focus(&client.focused());
    assert_eq!(
        (focus.node.role, focus.node.name.as_deref()),
        (Role::List, Some("Items View"))
    );

    let (sent, events) = mpsc::channel();
    let registration = Registration::with_parts(
        vec![Part::shaped::<ListenerSelection>(Subscription::Event {
            event: UIA_SelectionItem_ElementSelectedEventId,
            callback: Arc::new(move |element: &Cached<ListenerSelection>| {
                let captured =
                    verbatim_outpost::listener::capture_selection(element, &element.read())
                        .expect("mockapp's element has its process");
                let agile = element.agile().expect("an agile reference");
                // The test may have finished.
                let _ = sent.send((agile, captured));
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("a subscription to element selected events");
    app.send("select alpha");
    let (agile, (window, snapshot)) = events
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("an element selected event for alpha.txt");
    let element = agile.resolve().expect("the event's element");
    let selection = DeliveredFact::UiaSelection {
        hwnd: window,
        focus_window: 0,
        snapshot,
    };
    let fact = match outpost.listener_check(&element.narrow(), selection) {
        Judged::Passed(fact) => *fact,
        other => panic!("the listener's check made {other:?} of the selection"),
    };

    common::reset_hits(hwnd);
    outpost.deliver(fact);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::SelectionChanged { node },
            timing,
            ..
        } => assert_eq!(
            (
                node.role,
                node.name.as_deref(),
                node.states,
                node.details.position_in_set,
                node.details.set_size,
                timing.calls,
                common::read_hits(hwnd),
            ),
            (
                Role::ListItem,
                Some("alpha.txt"),
                StateSet::from_iter([State::Focusable, State::Selectable, State::Selected]),
                Some(2),
                Some(2),
                CallCounts {
                    uia: 1,
                    ..CallCounts::default()
                },
                vec![
                    ("ProviderOptions", 2),
                    ("GetPropertyValue", 2),
                    ("HostRawElementProvider", 1),
                    ("Navigate", 1),
                ],
            )
        ),
        other => panic!("the outpost said {other:?}, not the item's selection"),
    }
    outpost.settled();

    drop(registration);
    drop(outpost);
    app.quit();
}

/// An unnamed grouping with a description is focus context whatever the
/// theme, as NVDA's layout judgment reads the description whatever its
/// setting (`docs/design/uia-shapes.md`, decision 4): with a theme that
/// reports descriptions as off, the outpost's own read of a focus's
/// ancestors, for Core's ancestors query, reports the grouping, without
/// the description the theme does not report.
fn an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off() {
    let title = common::unique_title("mockapp-described-group");
    let mut app = common::spawn("described_group.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut outpost = OutpostUnderTest::new(&app);
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::Fetches(Fetches {
            description: false,
            ..Fetches::default()
        }));

    app.send("set-focus louder");
    let focus = outpost.uia_focus(&client.focused());
    assert_eq!(focus.node.name.as_deref(), Some("Louder"));
    let (ancestors, _) = outpost.ancestors(focus.node.id);
    let reported: Vec<(Role, Option<&str>, Option<&str>)> = ancestors
        .iter()
        .map(|ancestor| {
            (
                ancestor.role,
                ancestor.name.as_deref(),
                ancestor.details.description.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        [
            (Role::Window, Some("Mockapp Described Group Fixture"), None),
            (Role::Group, None, None),
        ]
    );

    drop(outpost);
    app.quit();
}

/// A protected (password) field focused with something selected comes
/// with its caret and its selected text, as any field's does: Core reads a
/// protected field's text, masked as the control gives it
/// (`docs/parity/focus-announcement.md`, "Text on focus").
fn a_protected_fields_focus_comes_with_its_caret_and_selected_text() {
    let title = common::unique_title("mockapp-protected-focus");
    let mut app = common::spawn("text.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-states doc focusable protected");
    app.send("caret doc 0 5");
    app.send("set-focus doc");
    let reported = outpost.uia_focus(&client.focused());
    let Some(FocusText::Caret {
        caret,
        selected_text,
    }) = &reported.text
    else {
        panic!("the focus came without its caret: {:?}", reported.text);
    };
    assert_eq!(
        (
            reported.node.states.contains(State::Protected),
            caret.line.text.as_str(),
            caret.selection.is_some(),
            selected_text.as_deref(),
        ),
        (true, "alpha beta\n", true, Some("alpha"))
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A UIA focus the listener read is reported with the listener's stamp,
/// the one clock read the listener's callback took, so Core orders it to
/// the microsecond against another application's focus
/// (`docs/parity/focus-events.md`, "Stale focus events").
fn a_listener_read_focus_is_reported_with_the_listeners_stamp() {
    let title = common::unique_title("mockapp-uia-focus-stamp");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let fact = outpost.listener_fact(&client.focused());
    let received_at_us = verbatim_process::clock::now_us();
    outpost.deliver_stamped(fact, received_at_us);
    match outpost.next() {
        OutpostToSupervisor::Event {
            observed_at_us,
            event: NormalizedEvent::FocusChanged { .. },
            ..
        } => assert_eq!(observed_at_us, received_at_us),
        other => panic!("the outpost said {other:?}, not the focus"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A focus-now answer is stamped when its read began, to the microsecond,
/// so Core orders the focus it becomes to the microsecond.
fn a_focus_now_answer_is_stamped_to_the_microsecond() {
    let title = common::unique_title("mockapp-focus-now-stamp");
    let mut app = common::spawn("small.json", "uia", &title);
    let mut outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let asked_at_us = verbatim_process::clock::now_us();
    let (answer, _) = outpost.focus_now_answer();
    let answered_at_us = verbatim_process::clock::now_us();
    assert!(
        (asked_at_us..=answered_at_us).contains(&answer.observed_at_us),
        "the read began at {} us, not between the question at {asked_at_us} us and its answer at {answered_at_us} us",
        answer.observed_at_us
    );

    drop(outpost);
    app.quit();
}

/// mockapp's MSAA dialog of `save_as.json`, titled after `label`, its
/// window and a child window of it that answers for the dialog's nodes at
/// their object ids (`child-window`), as a control's window is inside a
/// dialog's.
fn save_as_dialog(label: &str) -> (common::MockApp, HWND, HWND) {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let title = common::unique_title(label);
    let mut app = common::spawn("save_as.json", "msaa", &title);
    let host = common::find_window(&title);
    app.send("child-window");
    // SAFETY: a local search of the host window's children.
    let child = unsafe {
        FindWindowExW(
            Some(host),
            None,
            windows::core::w!("VerbatimMockAppWindow"),
            windows::core::PCWSTR::null(),
        )
    }
    .expect("mockapp made the child window");
    (app, host, child)
}

/// The roles and names of a reported focus's chain, outermost first.
fn roles_and_names(reported: &common::outpost::Reported) -> Vec<(Role, Option<&str>)> {
    reported
        .ancestors
        .iter()
        .chain(std::iter::once(&reported.node))
        .map(|node| (node.role, node.name.as_deref()))
        .collect()
}

/// The windows of a reported focus's chain, outermost first, as its nodes
/// carry them (`NodeDetails::window`).
fn windows_of(reported: &common::outpost::Reported) -> Vec<Option<WindowHandle>> {
    reported
        .ancestors
        .iter()
        .chain(std::iter::once(&reported.node))
        .map(|node| node.details.window)
        .collect()
}

/// `hwnd` as the model's window handle.
fn handle(hwnd: HWND) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes((hwnd.0 as isize).to_ne_bytes()))
}

/// A UIA element that is its window's own carries its window, and an
/// element inside it carries none: the "Original Name" button in mockapp's
/// window (`small.json`), read by the listener, is reported under the
/// window's element, which carries the window, while the button, which
/// has only a nearest window, carries none.
fn a_uia_window_element_carries_its_window_and_its_content_none() {
    let title = common::unique_title("mockapp-uia-window-handle");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    app.send("set-focus btn1");
    let reported = outpost.uia_focus(&client.focused());
    assert_eq!(
        (roles_and_names(&reported), windows_of(&reported)),
        (
            vec![
                (Role::Window, Some("Mockapp Events Fixture")),
                (Role::Button, Some("Original Name"))
            ],
            vec![Some(handle(hwnd)), None]
        ),
        "the outpost's chain and its windows"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The windows of the chain of a focus on the "Save" button of mockapp's
/// "Save As" dialog (`save_as.json`), whose client area names its address
/// when `identity` is given (`client-identity`).
fn save_as_chain_windows(identity: bool) -> (Vec<Option<WindowHandle>>, Option<WindowHandle>) {
    let title = common::unique_title("mockapp-msaa-window-handle");
    let mut app = common::spawn("save_as.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    if identity {
        app.send("client-identity");
    }
    let outpost = OutpostUnderTest::new(&app);
    // The button, index 1, at its object id in the dialog's window.
    let reported = outpost.msaa_focus(hwnd, 1);
    assert_eq!(
        roles_and_names(&reported),
        [
            (Role::Dialog, Some("Save As")),
            (Role::Button, Some("Save"))
        ],
        "the outpost's chain"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
    (windows_of(&reported), Some(handle(hwnd)))
}

/// An MSAA client area reached by a walk carries its window when the walk
/// knows it is the window's client area, by the address it names, and an
/// object inside a window carries none: the "Save" button's chain in
/// mockapp's dialog has the client area carry the window when it names its
/// address, and nothing when it does not, as an object the walk keys at an
/// address it made up may be any object inside the window. The window
/// object above the client area is not among a chain's nodes; its window
/// is asserted where it is read
/// (`msaa_window_and_client_objects_carry_their_window`, `msaa_tree.rs`).
fn an_msaa_client_area_reached_by_a_walk_carries_its_window_and_its_content_none() {
    let (named, window) = save_as_chain_windows(true);
    assert_eq!(named, [window, None], "with the client area's address");
    let (unnamed, _) = save_as_chain_windows(false);
    assert_eq!(unnamed, [None, None], "without it");
}

/// A failed `accParent` is not the end of a chain: it goes on from the
/// parent window of the object's window, by that window's object, as
/// NVDA's parent falls back to the window hierarchy (`Window._get_parent`;
/// the audit's difference 2): the "Save" button, in a child window of
/// mockapp's "Save As" dialog, whose `accParent` fails, is reported with
/// the dialog's client area, role dialog, as its container, which the
/// chain then reaches, as "Save As dialog" is said after a failed
/// `accParent` in a real Save As dialog. Ended at the failure, the chain
/// would be empty.
fn a_focus_after_a_failed_acc_parent_reaches_its_dialog() {
    let (mut app, _, child) = save_as_dialog("mockapp-failed-acc-parent");
    let outpost = OutpostUnderTest::new(&app);
    app.send("fail-acc-parent save");
    // The button, index 1, at its object id in the child window.
    let reported = outpost.msaa_focus(child, 1);
    assert_eq!(
        roles_and_names(&reported),
        [
            (Role::Dialog, Some("Save As")),
            (Role::Button, Some("Save"))
        ],
        "the outpost's chain"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// NVDA's focused-state test of an MSAA focus walks up the object's
/// parents (`shouldAllowIAccessibleFocusEvent`), and where an `accParent`
/// fails it goes on from the parent window's own object
/// (`Window._get_parent`), reading its states and then its parents', a
/// plain window never focused (the audit's difference 19): the "Save"
/// button, without the focused state, in a child window of mockapp's
/// dialog, whose `accParent` fails, has the dialog's client area read,
/// which has no focused state, then the dialog's window object, whose
/// parent is the desktop, and is dropped with nothing said. The dialog's
/// window has its thread's focus (`window-focus`), read so independently
/// first, which does not make it focused. Read as a plain window focused
/// as its thread's focus window, the dialog's window had the button
/// reported.
fn the_focused_state_test_continues_past_a_failed_acc_parent() {
    let (mut app, host, child) = save_as_dialog("mockapp-focused-state-past-failure");
    app.send("set-states save focusable");
    app.send("window-focus");
    assert_eq!(
        thread_focus_window(host),
        Some(host),
        "the dialog's window has its thread's focus"
    );
    let outpost = OutpostUnderTest::new(&app);
    app.send("fail-acc-parent save");
    common::reset_hits(host);
    // The button, index 1, at its object id in the child window.
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: child.0 as isize,
        id_object: 2,
        id_child: 0,
    });
    outpost.settled();
    assert_eq!(
        common::read_get_objects(host)
            .into_iter()
            .filter(|&(kind, _)| kind == "OBJID_CLIENT")
            .collect::<Vec<_>>(),
        [("OBJID_CLIENT", 1)],
        "the dialog's client area is asked for once"
    );
    drop(outpost);
    app.quit();
}

/// The focus window of the thread that owns `hwnd`, read from Windows
/// (`GetGUIThreadInfo`).
fn thread_focus_window(hwnd: HWND) -> Option<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId,
    };
    // SAFETY: a live window; no out-parameter for its process.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, None) };
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).expect("a small structure"),
        ..Default::default()
    };
    // SAFETY: `info` is a local with `cbSize` set, which the call fills in.
    unsafe { GetGUIThreadInfo(thread, &raw mut info) }.expect("the thread's GUI state");
    (!info.hwndFocus.0.is_null()).then_some(info.hwndFocus)
}

/// A walk cut at the depth limit of 64 is completed by plain windows from
/// its outermost element's nearest window, included, a deliberate
/// difference from NVDA's fallback (`docs/design/focus-pipeline-rework.md`,
/// "Decisions"): the button "Far", under 66 groups of its own in mockapp's
/// window (`nested.json`), is read by the listener up to its 64th ancestor,
/// which names the window to complete from, and reported with the window,
/// a plain window named by its title, above the 64 groups read. Without the
/// completion, the outermost group read is the chain's top.
fn a_chain_cut_at_the_depth_limit_is_completed_by_its_windows() {
    let title = common::unique_title("mockapp-depth-limit");
    let app = common::spawn("nested.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    let far = client.focused();
    let (judged, _) = outpost.listener_read(&far);
    let Judged::Read(read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    assert_eq!(
        (
            read.ancestors.as_deref().map(<[_]>::len),
            read.ancestors_end
        ),
        (
            Some(64),
            verbatim_outpost::protocol::AncestorsEnd::PlainFrom(hwnd.0 as isize)
        ),
        "the listener's walk, cut at its depth limit"
    );
    let reported = outpost.focus(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    let chain = roles_and_names(&reported);
    assert_eq!(
        (chain.len(), chain[0], chain[1], chain[64], chain[65]),
        (
            66,
            (Role::Window, Some(title.as_str())),
            (Role::Group, Some("Nest 3")),
            (Role::Group, Some("Nest 66")),
            (Role::Button, Some("Far"))
        ),
        "the outpost's chain"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A window a focus's walk has not reached by its bound is a plain window,
/// named by its text (`docs/design/focus-pipeline-rework.md`, "A window's
/// name, and the one fallback"), and the bound is measured from the
/// event's observation: `MOVED_ON_GRACE`, 500 ms, for a focus whose
/// top-level window is not that of the focus before it, and the read's
/// budget, 2 seconds, otherwise. On the frozen observation clock, each
/// focus waits before its read: "First", the listener's first focus,
/// observed 500 ms before, reads none of its ancestors, and is reported
/// below mockapp's window as a plain window named by its title, its list
/// "Items" not reached; "Second", in the same window, observed 500 ms
/// before, reads its whole chain, the window's UIA element named "Mockapp
/// Tree Fixture" by its provider; and "First" again, observed 2 seconds
/// before, reads none.
fn a_window_not_reached_by_the_bound_is_named_by_its_text() {
    let title = common::unique_title("mockapp-bound-text");
    let mut app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    let mut report = |id: &str, waited: Duration| {
        app.send(&format!("set-focus {id}"));
        let Judged::Read(read) = outpost.listener_read_after(&client.focused(), waited) else {
            panic!("the listener dropped {id}");
        };
        let reported = outpost.focus(DeliveredFact::UiaFocusRead {
            marker: 0,
            read,
            focus: true,
        });
        roles_and_names(&reported)
            .into_iter()
            .map(|(role, name)| (role, name.map(str::to_owned)))
            .collect::<Vec<_>>()
    };
    let window = (Role::Window, Some(title.clone()));
    let first = (Role::ListItem, Some("First".to_owned()));
    let uia_window = (Role::Window, Some("Mockapp Tree Fixture".to_owned()));
    assert_eq!(
        report("item1", Duration::from_millis(500)),
        [window.clone(), first.clone()],
        "a new window's focus at its bound of 500 ms"
    );
    assert_eq!(
        report("item2", Duration::from_millis(500)),
        [
            uia_window,
            (Role::List, Some("Items".to_owned())),
            (Role::ListItem, Some("Second".to_owned()))
        ],
        "a focus in the same window within its bound of 2 seconds"
    );
    assert_eq!(
        report("item1", Duration::from_secs(2)),
        [window, first],
        "a focus in the same window at its bound of 2 seconds"
    );
    drop(outpost);
    app.quit();
}

/// How late the stuck step to "First"'s parent answers: far past the
/// listener's bound, short of the five seconds after which Windows judges
/// mockapp's window hung.
const STUCK_PARENT_MS: u32 = 3000;

/// A read that has passed NVDA's checks and whose step to a parent is
/// stuck is published at its bound by the listener's watchdog, from what it
/// has read, and its late result is discarded
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"): with the listener's reading threads run here
/// (`ReadingThreads`), their readers on the frozen clocks, so no read's
/// walk reaches its bound between its calls and the watchdog's 500 ms run
/// from each read's arming, the listener's first focus, "First", whose
/// walk's first step to its parent answers 3 seconds late (`slow-parent`),
/// is published before that step returns, with no ancestors read and its
/// chain to be completed by plain windows from mockapp's window. Its
/// thread is not abandoned: once the step returns, it reads the
/// application's next focus, "Second", whose whole chain is its outcome,
/// and nothing else is said of "First".
fn a_stuck_parent_step_is_published_at_the_bound_and_its_late_result_discarded() {
    use verbatim_outpost::focus_read::ReadingThreads;
    let title = common::unique_title("mockapp-bound-stuck");
    let mut app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let window = hwnd.0 as isize;
    let client = Client::new(hwnd);
    let frozen = std::time::Instant::now();
    let frozen_us = common::now_us();
    let (threads, outcomes) = ReadingThreads::new(Box::new(move |reader| {
        reader.set_clock(Arc::new(move || frozen));
        reader.set_observation_clock(Arc::new(move || frozen_us));
        // No window takes the foreground where the test runs.
        reader.set_foreground_reader(Arc::new(move || window));
    }));
    let pid = Pid(app.pid());
    app.send("set-focus item1");
    let first = client.focused();
    app.send(&format!("slow-parent item1 {STUCK_PARENT_MS} 1"));
    let marker = threads
        .push(pid, &first, frozen_us)
        .expect("the focus's element kept for its read");
    let (published, judged) = outcomes
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the focus published");
    let published_at = common::now_us();
    app.stamped_line("slow-parent started");
    let returned = app.stamped_line("slow-parent ended");
    assert!(
        published_at < returned,
        "published at {published_at}, before the step returned at {returned}"
    );
    let Judged::Read(read) = judged else {
        panic!("the stuck read was not published: {judged:?}");
    };
    assert_eq!(
        (
            published,
            read.snapshot.name.as_deref(),
            read.ancestors.as_deref().map(<[_]>::len),
            read.ancestors_end
        ),
        (
            marker,
            Some("First"),
            Some(0),
            verbatim_outpost::protocol::AncestorsEnd::PlainFrom(window)
        ),
        "the focus as read before its stuck step"
    );
    app.send("slow-parent item1 0");
    app.send("set-focus item2");
    let next = threads
        .push(pid, &client.focused(), frozen_us)
        .expect("the next focus's element kept for its read");
    let (read_next, judged) = outcomes
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the next focus read");
    let Judged::Read(read) = judged else {
        panic!("the next focus was not read: {judged:?}");
    };
    assert_eq!(
        (
            read_next,
            read.snapshot.name.as_deref(),
            read.ancestors.as_ref().map(|ancestors| ancestors
                .iter()
                .map(|ancestor| ancestor.snapshot.name.as_deref())
                .collect::<Vec<_>>()),
            read.ancestors_end
        ),
        (
            next,
            Some("Second"),
            Some(vec![Some("Items"), Some("Mockapp Tree Fixture")]),
            verbatim_outpost::protocol::AncestorsEnd::Top
        ),
        "the next focus, read by the same thread once the step returned"
    );
    drop(threads);
    assert!(
        outcomes.try_recv().is_err(),
        "nothing more is said of either focus"
    );
    app.quit();
}

/// The outpost's watchdog publishes an MSAA focus whose walk is stuck at
/// the walk's bound, and its worker's late result is discarded
/// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the one
/// fallback"): on the frozen clocks, so the walk is never cut between its
/// calls and the bound, 500 ms for a focus whose top-level window is not
/// that of a focus before it, runs from the walk's start, the OK button of
/// mockapp's MSAA tree, whose own states pass NVDA's focused-state test,
/// has its walk's step to its parent, "Group One", stuck: the step finds
/// the window the parent is in (`WindowFromAccessibleObject`), which asks
/// the parent's own `accParent`, answered 3 seconds late
/// (`slow-acc-parent`). The focus is published before that call returns,
/// with no ancestor read and its own window, a plain window named by its
/// title, above it; once the call returns, the worker adopts the published
/// focus and says nothing more. The button's own `accParent` is not the
/// one made late, since its object's acquisition calls it before NVDA's
/// checks, which the bound does not cover.
fn an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound() {
    let title = common::unique_title("mockapp-msaa-bound-stuck");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus btn_ok");
    app.send(&format!("slow-acc-parent group1 {STUCK_PARENT_MS}"));
    // The OK button, node 2 of the tree, at its object id.
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: 3,
        id_child: 0,
    });
    let published = focus_changed(outpost.next());
    let published_at = common::now_us();
    app.stamped_line("slow-acc-parent started");
    // Answered at once from the call after this one, which waits for it.
    app.write("slow-acc-parent group1 0");
    let returned = app.stamped_line("slow-acc-parent ended");
    app.expect_line("applied", || "the late call's end".to_owned());
    assert!(
        published_at < returned,
        "published at {published_at}, before the call returned at {returned}"
    );
    assert_eq!(
        chain_of(&published),
        [
            (Role::Window, Some(title.clone())),
            (Role::Button, Some("OK".to_owned()))
        ],
        "the focus as read before its stuck step, below its window"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The roles and names of a focus change's chain, its ancestors outermost
/// first, then the focus.
fn chain_of(event: &NormalizedEvent) -> Vec<(Role, Option<String>)> {
    let NormalizedEvent::FocusChanged {
        node, ancestors, ..
    } = event
    else {
        panic!("{event:?} is not a focus change");
    };
    ancestors
        .iter()
        .chain(std::iter::once(node))
        .map(|node| (node.role, node.name.clone()))
        .collect()
}

/// The outpost's watchdog publishes the gap's read at its walk's bound
/// when the walk is stuck, as it publishes an MSAA focus
/// ([`an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound`]): a
/// `WinEvent` focus on mockapp's UIA window that no UIA focus follows has
/// the outpost read its application's focus, "First", once the gap's bound
/// has passed, and the walk's step to its parent answers 3 seconds late
/// (`slow-parent`), after the remote program's first step up, which finds
/// the focus's window, has passed at once. On the frozen clocks the walk's
/// bound, 500 ms from the read's start, is met only by the watchdog: the
/// focus is published before the step returns, with its window, a plain
/// window named by its title, above it, and nothing more is said once the
/// step returns.
fn a_gap_read_whose_walk_is_stuck_is_published_at_its_bound() {
    let title = common::unique_title("mockapp-gap-bound-stuck");
    let mut app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus item1");
    outpost.read_focus_as(client.focused().live().element());
    app.send(&format!("slow-parent item1 {STUCK_PARENT_MS} 1"));
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.pass_time(Duration::from_secs(1));
    let published = focus_changed(outpost.next());
    let published_at = common::now_us();
    app.stamped_line("slow-parent started");
    // Answered at once from the step after this one, which waits for it.
    app.write("slow-parent item1 0");
    let returned = app.stamped_line("slow-parent ended");
    app.expect_line("applied", || "the late step's end".to_owned());
    assert!(
        published_at < returned,
        "published at {published_at}, before the step returned at {returned}"
    );
    assert_eq!(
        chain_of(&published),
        [
            (Role::Window, Some(title.clone())),
            (Role::ListItem, Some("First".to_owned()))
        ],
        "the focus as read before its stuck step, below its window"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The outpost's continuation of a listener's chain ends at the read's
/// bound (`UiaFocusRead::bound_us`), measured from the event's
/// observation: the listener's read of "First" stands in for one whose
/// first step to a parent failed, its chain to go on from mockapp's window
/// (`AncestorsEnd::ContinueFrom`). Delivered as observed its bound before,
/// the continuation reads nothing and the window is a plain window, named
/// by its title; delivered as observed now, it reads the window's UIA
/// element, named "Mockapp Tree Fixture" by its provider.
fn the_continuation_of_a_listeners_chain_ends_at_the_reads_bound() {
    let title = common::unique_title("mockapp-continuation-bound");
    let mut app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::on_frozen_time(&app);
    app.send("set-focus item1");
    let Judged::Read(mut read) = outpost.listener_read(&client.focused()).0 else {
        panic!("the listener dropped the focus");
    };
    read.ancestors = Some(Vec::new());
    read.ancestors_end = verbatim_outpost::protocol::AncestorsEnd::ContinueFrom(hwnd.0 as isize);
    let report = |observed_at_us: u64| {
        outpost.deliver_stamped(
            DeliveredFact::UiaFocusRead {
                marker: 0,
                read: read.clone(),
                focus: true,
            },
            observed_at_us,
        );
        let reported = outpost.reported_focus();
        roles_and_names(&reported)
            .into_iter()
            .map(|(role, name)| (role, name.map(str::to_owned)))
            .collect::<Vec<_>>()
    };
    let first = (Role::ListItem, Some("First".to_owned()));
    assert_eq!(
        report(outpost.observed_now() - read.bound_us),
        [(Role::Window, Some(title.clone())), first.clone()],
        "observed its bound before"
    );
    assert_eq!(
        report(outpost.observed_now()),
        [
            (Role::Window, Some("Mockapp Tree Fixture".to_owned())),
            first
        ],
        "observed now"
    );
    drop(outpost);
    app.quit();
}

/// A hung application's window in a focus's ancestry is a plain window,
/// named by its text, with no call, by NVDA's rule
/// (`NVDAObjects/window/__init__.py` 85 to 97): a button in a child window
/// of a mockapp MSAA dialog (`child-window`, `save_as.json`), whose
/// `accParent` fails, has its chain go on from the dialog's window, which
/// the outpost judges hung, as Windows judges a window whose thread has not
/// answered for five seconds; the window is named by its title and the
/// dialog's client area is not asked for. Judged readable, its client
/// area is read, the dialog ([`a_focus_after_a_failed_acc_parent_reaches_its_dialog`]).
fn a_hung_ancestor_window_is_named_by_its_text_with_no_call() {
    let (mut app, host, child) = save_as_dialog("mockapp-hung-ancestor");
    let outpost = OutpostUnderTest::new(&app);
    outpost.judge_hung(host);
    app.send("fail-acc-parent save");
    common::reset_hits(host);
    // The button, index 1, at its object id in the child window.
    let reported = outpost.msaa_focus(child, 1);
    assert_eq!(
        reported
            .ancestors
            .iter()
            .chain(std::iter::once(&reported.node))
            .map(|node| (node.role, node.name.as_deref()))
            .collect::<Vec<_>>(),
        [
            (Role::Window, Some(app.title.as_str())),
            (Role::Button, Some("Save"))
        ],
        "the outpost's chain"
    );
    assert_eq!(
        windows_of(&reported),
        [Some(handle(host)), None],
        "the plain window carries its window, the button none"
    );
    assert_eq!(
        common::read_get_objects(host)
            .into_iter()
            .filter(|&(kind, _)| kind == "OBJID_CLIENT")
            .collect::<Vec<_>>(),
        [],
        "the dialog's client area is not asked for"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// mockapp's UIA window of `uia_island.json`, titled after `label`, its
/// window, and the child window that is a native UIA provider of its own
/// (`"native": "uia_window"`), which holds the focused button "OK" under
/// its root, the grouping "Embedded", as a XAML island is inside an
/// application's frame.
fn uia_island(label: &str) -> (common::MockApp, HWND, HWND) {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let title = common::unique_title(label);
    let app = common::spawn("uia_island.json", "uia", &title);
    let host = common::find_window(&title);
    // SAFETY: a local search of the host window's children.
    let island = unsafe {
        FindWindowExW(
            Some(host),
            None,
            windows::core::w!("VerbatimMockAppUiaWindow"),
            windows::core::PCWSTR::null(),
        )
    }
    .expect("mockapp made the UIA window");
    (app, host, island)
}

/// A hung application's window above a UIA focus ends the listener's walk
/// at the step into it, by NVDA's rule (`NVDAObjects/window/__init__.py`
/// 85 to 97): NVDA makes that one step, reads the window as a plain window
/// and goes on by the window hierarchy with no further call. The button
/// "OK", in mockapp's UIA child window, has its ancestors read by the
/// listener up to the host window's element, which the step reaches, and
/// the host window is judged hung, as Windows judges a window whose thread
/// has not answered for five seconds: the listener keeps "Embedded" and not
/// the host's element, and names the host window to go on from, which the
/// outpost reports as a plain window, named by its title. The listener
/// makes five calls: the button's nearest window, its keyboard focus, its
/// fresh cache, and the two steps, to "Embedded" and to the host's
/// element. Without the rule, it steps on from the host's element, a sixth
/// call, and the host's element, named by its fixture, is the outermost
/// ancestor.
fn a_hung_window_above_a_uia_focus_ends_the_listeners_walk() {
    let (app, host, island) = uia_island("mockapp-hung-uia-listener");
    let outpost = OutpostUnderTest::new(&app);
    outpost.judge_hung(host);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    let (judged, listener_calls) = outpost.listener_read(&ok);
    let Judged::Read(read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    assert_eq!(
        (
            read.ancestors.as_ref().map(|ancestors| ancestors
                .iter()
                .map(|ancestor| ancestor.snapshot.name.as_deref())
                .collect::<Vec<_>>()),
            read.ancestors_end,
            listener_calls.uia
        ),
        (
            Some(vec![Some("Embedded")]),
            verbatim_outpost::protocol::AncestorsEnd::ContinueFrom(host.0 as isize),
            5
        ),
        "the listener's walk and its calls"
    );
    let reported = outpost.focus(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    assert_eq!(
        roles_and_names(&reported),
        [
            (Role::Window, Some(app.title.as_str())),
            (Role::Group, Some("Embedded")),
            (Role::Button, Some("OK"))
        ],
        "the outpost's chain"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// [`a_hung_window_above_a_uia_focus_ends_the_listeners_walk`] for a
/// focus-now query, which the outpost reads itself, with a remote operation
/// when `remote`: a remote program cannot judge a window between its steps,
/// so with the host window above the focus judged hung the outpost reads
/// the ancestors by the classic walk, which stops at its step into the
/// host's element, and reports the host window as a plain window; without
/// remote operations its own walk stops there too, and the host window is
/// not probed for its verdict. The outpost makes, after the focused
/// element's read, two steps, and with `remote` the focus's keyboard focus
/// read first, which the classic walk of a focus-now query makes, and one
/// window message, the child window's probe; the host window takes only
/// the three requests for its root provider with a zero `wParam` that the
/// reads of the child window's elements and the step bring, and no
/// connection. Without the rule, the remote program reads on into the
/// host's element and connects to the host window twice, which the
/// assertion on the calls and the host window's messages catches: the
/// chain is the same, since the outpost still takes the host's element it
/// read to the host window as a plain window. The classic walk without
/// the rule probes the host window and steps on from its element, which
/// the chain's assertion catches: the host's element, named by its
/// fixture, is the outermost ancestor.
fn hung_window_above_a_focus_now(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let (app, host, island) = uia_island(&format!("mockapp-hung-uia-focus-now-{path}"));
    let mut outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost.judge_hung(host);
    // The keyboard focus is in the UIA child window.
    outpost.set_foreground(island);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    outpost.read_focus_as(ok.live().element());
    common::reset_hits(host);
    let (answer, calls) = outpost.focus_now_answer();
    let focus = answer.focus.expect("the focus now's focused control");
    assert_eq!(
        focus.ancestors.as_ref().map(|ancestors| ancestors
            .iter()
            .chain(std::iter::once(&focus.node))
            .map(|node| (node.role, node.name.as_deref()))
            .collect::<Vec<_>>()),
        Some(vec![
            (Role::Window, Some(app.title.as_str())),
            (Role::Group, Some("Embedded")),
            (Role::Button, Some("OK"))
        ]),
        "{path}: the focus now's chain"
    );
    assert_eq!(
        (
            (calls.uia, calls.window_messages),
            common::read_get_objects(host)
        ),
        (
            (if remote { 4 } else { 3 }, 1),
            vec![("UiaRootObjectId, zero wParam", 3)]
        ),
        "{path}: the outpost's calls and the host window's messages"
    );
    drop(outpost);
    app.quit();
}

fn a_hung_window_above_a_focus_now_is_a_plain_window_remote() {
    hung_window_above_a_focus_now(true);
}

fn a_hung_window_above_a_focus_now_is_a_plain_window_classic() {
    hung_window_above_a_focus_now(false);
}

/// A focus whose own nearest window is judged hung keeps the ancestors the
/// listener read, by NVDA's rule (`NVDAObjects/window/__init__.py` 85 to
/// 97), which switches API only where a parent's window differs from its
/// child's: the button "OK" of `uia_island.json`, with the UIA child window
/// that holds it judged hung, has its chain read through UIA, the host
/// window's element named by its fixture, the child window's root
/// "Embedded", and the button. Without the rule, the outpost takes
/// "Embedded" to the child window and puts that window, as a plain window
/// with no name, in its place.
fn a_focus_in_a_hung_nearest_window_keeps_the_listeners_ancestors() {
    let (app, _host, island) = uia_island("mockapp-hung-nearest-listener");
    let outpost = OutpostUnderTest::new(&app);
    outpost.judge_hung(island);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    let (judged, _) = outpost.listener_read(&ok);
    let Judged::Read(read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    let reported = outpost.focus(DeliveredFact::UiaFocusRead {
        marker: 0,
        read,
        focus: true,
    });
    assert_eq!(
        roles_and_names(&reported),
        [
            (Role::Window, Some("Mockapp Island Fixture")),
            (Role::Group, Some("Embedded")),
            (Role::Button, Some("OK"))
        ],
        "the outpost's chain"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    harness::run_isolated(&[
        (
            "a_focus_after_a_failed_acc_parent_reaches_its_dialog",
            a_focus_after_a_failed_acc_parent_reaches_its_dialog,
        ),
        (
            "a_hung_ancestor_window_is_named_by_its_text_with_no_call",
            a_hung_ancestor_window_is_named_by_its_text_with_no_call,
        ),
        (
            "a_uia_window_element_carries_its_window_and_its_content_none",
            a_uia_window_element_carries_its_window_and_its_content_none,
        ),
        (
            "an_msaa_client_area_reached_by_a_walk_carries_its_window_and_its_content_none",
            an_msaa_client_area_reached_by_a_walk_carries_its_window_and_its_content_none,
        ),
        (
            "the_focused_state_test_continues_past_a_failed_acc_parent",
            the_focused_state_test_continues_past_a_failed_acc_parent,
        ),
        (
            "a_hung_window_above_a_uia_focus_ends_the_listeners_walk",
            a_hung_window_above_a_uia_focus_ends_the_listeners_walk,
        ),
        (
            "a_hung_window_above_a_focus_now_is_a_plain_window_remote",
            a_hung_window_above_a_focus_now_is_a_plain_window_remote,
        ),
        (
            "a_hung_window_above_a_focus_now_is_a_plain_window_classic",
            a_hung_window_above_a_focus_now_is_a_plain_window_classic,
        ),
        (
            "a_focus_in_a_hung_nearest_window_keeps_the_listeners_ancestors",
            a_focus_in_a_hung_nearest_window_keeps_the_listeners_ancestors,
        ),
        (
            "a_chain_cut_at_the_depth_limit_is_completed_by_its_windows",
            a_chain_cut_at_the_depth_limit_is_completed_by_its_windows,
        ),
        (
            "a_window_not_reached_by_the_bound_is_named_by_its_text",
            a_window_not_reached_by_the_bound_is_named_by_its_text,
        ),
        (
            "a_stuck_parent_step_is_published_at_the_bound_and_its_late_result_discarded",
            a_stuck_parent_step_is_published_at_the_bound_and_its_late_result_discarded,
        ),
        (
            "an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound",
            an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound,
        ),
        (
            "a_gap_read_whose_walk_is_stuck_is_published_at_its_bound",
            a_gap_read_whose_walk_is_stuck_is_published_at_its_bound,
        ),
        (
            "the_continuation_of_a_listeners_chain_ends_at_the_reads_bound",
            the_continuation_of_a_listeners_chain_ends_at_the_reads_bound,
        ),
        (
            "a_listener_read_focus_is_reported_with_the_listeners_stamp",
            a_listener_read_focus_is_reported_with_the_listeners_stamp,
        ),
        (
            "a_focus_now_answer_is_stamped_to_the_microsecond",
            a_focus_now_answer_is_stamped_to_the_microsecond,
        ),
        (
            "a_protected_fields_focus_comes_with_its_caret_and_selected_text",
            a_protected_fields_focus_comes_with_its_caret_and_selected_text,
        ),
        (
            "a_winevent_focus_no_uia_focus_follows_is_read_once_its_bound_has_passed",
            a_winevent_focus_no_uia_focus_follows_is_read_once_its_bound_has_passed,
        ),
        (
            "a_winevent_focus_its_uia_focus_follows_is_reported_once",
            a_winevent_focus_its_uia_focus_follows_is_reported_once,
        ),
        (
            "a_winevent_focus_whose_uia_focus_has_no_window_is_read_at_once",
            a_winevent_focus_whose_uia_focus_has_no_window_is_read_at_once,
        ),
        (
            "a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped",
            a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped,
        ),
        (
            "a_focused_element_of_another_process_is_not_read_further",
            a_focused_element_of_another_process_is_not_read_further,
        ),
        (
            "a_focus_event_naming_another_applications_window_is_not_read",
            a_focus_event_naming_another_applications_window_is_not_read,
        ),
        (
            "a_winevent_focus_no_uia_focus_follows_is_read_at_its_bound_and_heard_once_when_its_uia_focus_comes_late",
            a_winevent_focus_no_uia_focus_follows_is_read_at_its_bound_and_heard_once_when_its_uia_focus_comes_late,
        ),
        (
            "a_winevent_focus_in_a_console_window_no_uia_focus_follows_is_read_with_its_window",
            a_winevent_focus_in_a_console_window_no_uia_focus_follows_is_read_with_its_window,
        ),
        (
            "a_winevent_focus_whose_uia_focus_waits_for_its_read_at_the_bound_is_reported_from_that_read",
            a_winevent_focus_whose_uia_focus_waits_for_its_read_at_the_bound_is_reported_from_that_read,
        ),
        (
            "a_record_whose_focus_went_to_another_application_reads_nothing_at_its_bound",
            a_record_whose_focus_went_to_another_application_reads_nothing_at_its_bound,
        ),
        (
            "a_winevent_focus_whose_uia_focus_is_no_longer_focused_is_read_at_its_bound",
            a_winevent_focus_whose_uia_focus_is_no_longer_focused_is_read_at_its_bound,
        ),
        (
            "a_winevent_focus_observed_past_its_bound_is_read_at_once",
            a_winevent_focus_observed_past_its_bound_is_read_at_once,
        ),
        (
            "a_copy_handled_through_msaa_opens_no_record",
            a_copy_handled_through_msaa_opens_no_record,
        ),
        (
            "an_event_and_its_uia_focus_handled_after_the_keep_are_judged_by_one_verdict",
            an_event_and_its_uia_focus_handled_after_the_keep_are_judged_by_one_verdict,
        ),
        (
            "a_return_within_the_bound_is_read_with_the_returns_stamp",
            a_return_within_the_bound_is_read_with_the_returns_stamp,
        ),
        (
            "an_accepted_foreground_change_into_the_records_window_restamps_it",
            an_accepted_foreground_change_into_the_records_window_restamps_it,
        ),
        (
            "an_answered_record_cancels_its_timer",
            an_answered_record_cancels_its_timer,
        ),
        (
            "a_gap_read_whose_focus_window_changes_during_the_read_is_dropped",
            a_gap_read_whose_focus_window_changes_during_the_read_is_dropped,
        ),
        (
            "a_forced_uia_override_has_the_listener_read_a_class_listed_window",
            a_forced_uia_override_has_the_listener_read_a_class_listed_window,
        ),
        (
            "a_forwarded_copy_opens_the_starting_outposts_record",
            a_forwarded_copy_opens_the_starting_outposts_record,
        ),
        (
            "the_idle_answer_waits_for_the_wait_for_a_uia_focus",
            the_idle_answer_waits_for_the_wait_for_a_uia_focus,
        ),
        (
            "an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off",
            an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off,
        ),
        (
            "a_focus_is_no_repeat_once_core_holds_another",
            a_focus_is_no_repeat_once_core_holds_another,
        ),
        (
            "a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus",
            a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus,
        ),
        (
            "a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus",
            a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus,
        ),
        (
            "a_windowless_selection_is_given_its_nearest_window",
            a_windowless_selection_is_given_its_nearest_window,
        ),
        (
            "a_selection_in_a_focused_list_carries_its_states_and_position_from_the_cache",
            a_selection_in_a_focused_list_carries_its_states_and_position_from_the_cache,
        ),
        (
            "a_menu_opening_is_judged_as_a_focus_on_the_menu",
            a_menu_opening_is_judged_as_a_focus_on_the_menu,
        ),
        (
            "a_menu_opening_on_the_current_focus_is_judged_as_that_focus",
            a_menu_opening_on_the_current_focus_is_judged_as_that_focus,
        ),
        (
            "a_menu_opening_on_another_element_is_judged_by_its_own_window",
            a_menu_opening_on_another_element_is_judged_by_its_own_window,
        ),
        (
            "a_focus_out_of_time_is_dropped_only_by_its_windows_class",
            a_focus_out_of_time_is_dropped_only_by_its_windows_class,
        ),
        (
            "a_focus_out_of_time_whose_event_says_it_lacked_the_keyboard_focus_is_dropped",
            a_focus_out_of_time_whose_event_says_it_lacked_the_keyboard_focus_is_dropped,
        ),
        (
            "a_focus_whose_keyboard_focus_read_times_out_is_reported",
            a_focus_whose_keyboard_focus_read_times_out_is_reported,
        ),
        (
            "a_repeated_focus_out_of_time_is_reported",
            a_repeated_focus_out_of_time_is_reported,
        ),
        (
            "a_menu_opening_out_of_time_after_its_comparison_is_dropped_before_its_window",
            a_menu_opening_out_of_time_after_its_comparison_is_dropped_before_its_window,
        ),
        (
            "a_menu_items_legacy_state_is_read_with_its_focus",
            a_menu_items_legacy_state_is_read_with_its_focus,
        ),
        (
            "a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object",
            a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object,
        ),
        (
            "a_focus_s_help_text_and_suggestions_are_followed",
            a_focus_s_help_text_and_suggestions_are_followed,
        ),
        (
            "a_reused_runtime_id_names_a_new_node_remote",
            a_reused_runtime_id_names_a_new_node_remote,
        ),
        (
            "a_reused_runtime_id_names_a_new_node_classic",
            a_reused_runtime_id_names_a_new_node_classic,
        ),
        (
            "a_focus_reports_the_states_read_when_it_is_handled",
            a_focus_reports_the_states_read_when_it_is_handled,
        ),
        (
            "a_controls_own_focus_reports_its_focused_child",
            a_controls_own_focus_reports_its_focused_child,
        ),
        (
            "a_focus_carries_its_parents_automation_id",
            a_focus_carries_its_parents_automation_id,
        ),
        (
            "a_focus_whose_element_lost_the_keyboard_focus_before_it_was_read_is_dropped",
            a_focus_whose_element_lost_the_keyboard_focus_before_it_was_read_is_dropped,
        ),
        (
            "a_focus_read_while_it_had_the_keyboard_focus_is_reported_though_the_focus_moved_on",
            a_focus_read_while_it_had_the_keyboard_focus_is_reported_though_the_focus_moved_on,
        ),
        (
            "a_focus_is_found_by_its_path_and_followed",
            a_focus_is_found_by_its_path_and_followed,
        ),
        (
            "a_focus_without_a_path_is_followed_from_its_next_focus_event",
            a_focus_without_a_path_is_followed_from_its_next_focus_event,
        ),
        (
            "a_slow_windowless_text_focus_is_found_as_the_focused_element",
            a_slow_windowless_text_focus_is_found_as_the_focused_element,
        ),
        (
            "a_cut_walks_focused_element_with_another_runtime_id_is_not_taken",
            a_cut_walks_focused_element_with_another_runtime_id_is_not_taken,
        ),
        (
            "a_cut_walks_focused_element_of_another_process_is_not_taken",
            a_cut_walks_focused_element_of_another_process_is_not_taken,
        ),
        (
            "a_focus_whose_walk_was_not_cut_is_not_read_as_the_focused_element",
            a_focus_whose_walk_was_not_cut_is_not_read_as_the_focused_element,
        ),
        (
            "a_route_walk_out_of_time_fails_rather_than_answering_gone",
            a_route_walk_out_of_time_fails_rather_than_answering_gone,
        ),
        (
            "a_route_walk_that_finds_its_element_gone_answers_gone",
            a_route_walk_that_finds_its_element_gone_answers_gone,
        ),
        (
            "a_text_change_after_the_focus_returns_to_a_document_is_heard",
            a_text_change_after_the_focus_returns_to_a_document_is_heard,
        ),
        (
            "a_change_raised_after_a_focus_event_waits_for_the_focus_read",
            a_change_raised_after_a_focus_event_waits_for_the_focus_read,
        ),
        (
            "a_change_of_the_old_focus_queued_behind_a_focus_inside_it_is_handled_first",
            a_change_of_the_old_focus_queued_behind_a_focus_inside_it_is_handled_first,
        ),
        (
            "a_value_change_of_the_old_focus_queued_behind_a_focus_inside_it_keeps_its_place",
            a_value_change_of_the_old_focus_queued_behind_a_focus_inside_it_keeps_its_place,
        ),
        (
            "a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is",
            a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is,
        ),
        (
            "a_console_windows_own_focus_is_never_reported",
            a_console_windows_own_focus_is_never_reported,
        ),
        (
            "a_console_text_areas_focus_is_checked_from_the_text_area",
            a_console_text_areas_focus_is_checked_from_the_text_area,
        ),
        (
            "a_focus_in_a_window_with_no_uia_provider_is_read_and_dropped_by_its_outpost",
            a_focus_in_a_window_with_no_uia_provider_is_read_and_dropped_by_its_outpost,
        ),
        (
            "a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read",
            a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read,
        ),
        (
            "a_focus_waiting_for_core_s_judgement_is_no_repeat",
            a_focus_waiting_for_core_s_judgement_is_no_repeat,
        ),
        (
            "a_foreground_event_for_a_window_read_through_uia_is_dropped",
            a_foreground_event_for_a_window_read_through_uia_is_dropped,
        ),
    ]);
}
