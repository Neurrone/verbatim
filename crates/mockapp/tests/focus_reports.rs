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
    StateSet, TextFormat, TraceId,
};
use verbatim_outpost::focus_read::{FocusEvent, Judged, NO_WINDOW, NOT_FOCUSED};
use verbatim_outpost::protocol::{
    DeliveredFact, DroppedVerdict, OutpostToSupervisor, Query, QueryOutcome, SupervisorToOutpost,
    UiaFocusRead, UiaSnapshotFact, VerdictDrop, WinEventStamp,
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
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
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
    outpost.deliver(judged.into_fact(7, FocusEvent::Focus));
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
    let outpost = OutpostUnderTest::new(&app);

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
/// so that the chain it sends holds only the field, is found by the outpost
/// as its application's focused element, read with the focus read's
/// process-id check and taken because its runtime id is the field's, before
/// the focus is reported (`docs/design/focus-pipeline-rework.md`, the
/// audit's difference 5). The field, "Notes", is four levels below its
/// window, where a walk down from the window by the runtime ids known would
/// not reach it. The read's first step to the field's parent, after the
/// nearest window's lookup, answers past the read's budget (`slow-parent`),
/// spent on the system's clock. Found, the field has no text pattern, so its
/// text is reported as none, `NoText`; not found, it would be `NotRead`.
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
    assert_eq!(
        (read.hwnd, read.ancestors.is_none(), read.window),
        (0, true, Some(hwnd.0 as isize)),
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
                    ancestors_unknown: true,
                    text,
                    ..
                },
            timing,
            ..
        } => {
            assert_eq!(
                (node.name.as_deref(), ancestors.as_slice(), text.as_deref()),
                (Some("Notes"), [].as_slice(), Some(&FocusText::NoText))
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
    outpost.deliver(read.into_fact(3, FocusEvent::Focus));
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
    outpost.deliver(read.into_fact(3, FocusEvent::Focus));
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
    outpost.deliver(read.into_fact(3, FocusEvent::Focus));
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
    outpost.deliver(Judged::Dropped(reason).into_fact(0, FocusEvent::Focus));
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
    winevent_focus_observed(outpost, hwnd, 0);
}

/// [`winevent_focus`], observed at `observed_at_us` (0 for none).
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
/// (`docs/design/listener-without-connections.md`, risk 2): once a second
/// has passed, the listener sends focus-missing, and the outpost reads its
/// application's focus itself and reports it with its ancestors (decision
/// 11), once. The time is the test's, which nothing moves but the test.
fn a_winevent_focus_no_uia_focus_follows_is_read_once_its_bound_has_passed() {
    let title = common::unique_title("mockapp-unanswered-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

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
    let outpost = OutpostUnderTest::new(&app);

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
/// answers the listener's record, which sends focus-missing at once, and
/// the outpost reads its focus without waiting for the bound and reports
/// it once.
fn a_winevent_focus_whose_uia_focus_has_no_window_is_read_at_once() {
    let title = common::unique_title("mockapp-gone-sender");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 4 });
    outpost.listener_outcome(4, Judged::Dropped(NO_WINDOW), 0);
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

/// A UIA focus the listener dropped on its own verdict on its window, here
/// unknown, its probe not answered in time, in a window this outpost judges
/// UIA: the outpost reads its application's focus at once, in the drop's
/// place, and reports it stamped with the dropped event's observation. The
/// drop answers the listener's gap record in that top-level window, so the
/// `WinEvent` focus observed 20 ms after the dropped event, as a UIA focus
/// can reach the listener before its `WinEvent`, opens no record, and
/// nothing more is read at its bound.
fn a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read() {
    let title = common::unique_title("mockapp-verdict-drop");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 5 });
    let dropped_observed_at_us = common::outpost::now_us();
    outpost.deliver_stamped(
        DeliveredFact::UiaFocusDropped {
            marker: 5,
            focus: true,
            on_verdict: Some(VerdictDrop {
                window: hwnd.0 as isize,
                verdict: DroppedVerdict::Unknown,
            }),
        },
        dropped_observed_at_us,
    );
    let message = outpost.next();
    let OutpostToSupervisor::Event { observed_at_us, .. } = &message else {
        panic!("the outpost said {message:?}, not a focus change");
    };
    assert_eq!(
        *observed_at_us, dropped_observed_at_us,
        "the dropped event's stamp"
    );
    let NormalizedEvent::FocusChanged {
        node, ancestors, ..
    } = focus_changed(message)
    else {
        unreachable!("a focus change");
    };
    let chain: Vec<Option<&str>> = ancestors
        .iter()
        .chain(std::iter::once(&node))
        .map(|node| node.name.as_deref())
        .collect();
    assert_eq!(
        chain,
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.settled();
    winevent_focus_observed(&outpost, hwnd, dropped_observed_at_us + 20_000);
    outpost.settled();
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
        });
    outpost.deliver(read);
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window whose UIA focus event came but was
/// dropped because its element no longer had the keyboard focus, as in
/// ordinary fast navigation: the drop leaves the listener's record open
/// (Dickson, 2026-10-21), and the focus is read at the bound.
fn a_winevent_focus_whose_uia_focus_is_no_longer_focused_is_read_at_its_bound() {
    let title = common::unique_title("mockapp-unfocused-drop");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 7 });
    outpost.listener_outcome(7, Judged::Dropped(NOT_FOCUSED), 0);
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
/// the bound passes two seconds after the copy: the listener's bound waits
/// for the application's reads, and the marker's drop, which leaves the
/// record open, arms it past its bound, so focus-missing is sent at once
/// and the focus read with the time unmoved.
fn a_winevent_focus_observed_past_its_bound_is_read_at_once() {
    let title = common::unique_title("mockapp-old-copy");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 3 });
    outpost.pass_time(Duration::from_secs(2));
    outpost.listener_outcome(3, Judged::Dropped(NOT_FOCUSED), 0);
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
/// bound the focus is read for the listener's focus-missing, and reported
/// stamped with the copy's observation, not the time of its read. The UIA
/// focus that then comes late is reported under the read's node, which
/// Core's duplicate rule takes silently, so the element is heard once in
/// all.
fn a_winevent_focus_no_uia_focus_follows_is_read_at_its_bound_and_heard_once_when_its_uia_focus_comes_late()
 {
    let title = common::unique_title("mockapp-late-uia-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    let copy_observed_at_us = common::outpost::now_us();
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
    let outpost = OutpostUnderTest::new(&app);

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
/// waiting for its read when the bound passes: the listener sends nothing
/// while its application's read waits, and the marker's read then answers
/// the record and is reported once.
fn a_winevent_focus_whose_uia_focus_waits_for_its_read_at_the_bound_is_reported_from_that_read() {
    let title = common::unique_title("mockapp-marker-at-bound");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 5 });
    outpost.pass_time(Duration::from_secs(1));
    let (judged, _) = outpost.listener_read(&client.focused());
    let reported = outpost.focus(judged.into_fact(5, FocusEvent::Focus));
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A `WinEvent` focus in a UIA window after which the listener's hook
/// observes a focus in another application: that focus answers the
/// record, and nothing is read once the bound has passed.
fn a_focus_queued_from_another_application_after_the_copy_answers_the_wait() {
    let title = common::unique_title("mockapp-focus-elsewhere");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.win_event_in_another_application(1);
    outpost.pass_time(Duration::from_secs(1));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The idle barrier's round waits while the listener's record of a
/// `WinEvent` focus in a UIA window is open (`docs/design/communication.md`,
/// section 6.7): the outpost is asked only once the record is sent as
/// focus-missing, and its answer goes behind the read that replaces the
/// UIA focus.
fn the_idle_answer_waits_for_the_wait_for_a_uia_focus() {
    let title = common::unique_title("mockapp-idle-gap");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.read_focus_as(client.focused().live().element());
    winevent_focus(&outpost, hwnd);
    outpost.settled();
    outpost.idle_round(4);
    // The listener holds the round, and nothing is said.
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
    outpost.deliver(Judged::Dropped(reason).into_fact(0, FocusEvent::Focus));
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
/// has been looked up is still judged by NVDA's native check, from the
/// window's verdict kept from an earlier focus's probe, with no probe made
/// (`FocusReader::native_window`), its keyboard focus taken from its
/// event's cache, and is reported from its event, its nearest window the
/// one call made: no probe, which would be a window message, and nothing
/// else once the time is up.
fn a_focus_out_of_time_is_judged_by_its_windows_kept_verdict() {
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
    // Reported whole as its event's cache gives it, its own class
    // included, with the Focused state, its ancestors unknown, in its own
    // window.
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
                    ..NodeDetails::default()
                },
            },
            ancestors: None,
            selected_child: None,
            value_from_selection: false,
            window: Some(hwnd.0 as isize),
            same_as_last: None,
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

/// The raw probe's wait does not count against a focus read's time budget
/// (`Budget`, 2 seconds): a sender with a window of its own, whose
/// window's verdict is not yet known, has the probe as its read's first
/// call, and with mockapp's window stalled for longer than the budget but
/// less than the probe's wait (`RAW_PROBE_TIMEOUT_MS`), the probe alone
/// outlasts the budget. The read still has its whole budget after it, so
/// the sender's keyboard focus is read live and the focus is read with its
/// ancestors: the probe, the keyboard focus, and the read's two calls (the
/// window's cache, and the step up to the desktop's root, where its
/// ancestors end). The time is the system's, so that the stall spends it.
fn a_focus_whose_windows_probe_outlasts_the_read_budget_is_read_in_time() {
    const STALL: Duration = Duration::from_millis(2_500);
    assert!(STALL < Duration::from_millis(u64::from(RAW_PROBE_TIMEOUT_MS)));
    let title = common::unique_title("mockapp-probe-outlasts-budget");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    outpost.follow_the_system_clock();

    app.send("set-focus root");
    let listener = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let window = client
        .uia
        .window_element(hwnd.0 as isize, &listener)
        .expect("mockapp's root element");
    app.stall(STALL);
    let (judged, calls) = outpost.listener_read(&window);
    let _ = app.stall_ended(STALL);
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the focus: {judged:?}");
    };
    assert_eq!(
        read.snapshot.name.as_deref(),
        Some("Mockapp Events Fixture")
    );
    assert_eq!(read.ancestors, Some(Vec::new()), "its ancestors were read");
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (3, 0, 1),
        "the probe, the keyboard focus, and the read"
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
/// focus is the one reported. As in NVDA, the refusal is the object's, so
/// it comes after the native check, which asks whether the console's
/// provider reports formatting (NVDA's check for a console's UIA; section
/// 18): from the held window element, with no connection to the window
/// (`docs/design/listener-without-connections.md`, section 3, step 5), four
/// calls, the text area's search, its text pattern, its visible ranges and
/// the range's font, where the check by the window's handle made five; and
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
        calls.uia, 4,
        "the console check, and no read of the element"
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A console's text area's focus is checked as native UIA from the text
/// area itself, the held sender, known by its cached automation id, with
/// no connection to the console window
/// (`docs/design/listener-without-connections.md`, section 3, step 5), and
/// read: seven calls, the sender's nearest window, the console check's two
/// (the visible ranges of the text pattern the sender's cache holds, and
/// the range's font, where the check by the window's handle made five), the
/// keyboard focus, and the read's three (the text area's cache, and steps
/// up to the console window and to the desktop's root); and one window
/// message, the raw probe. The
/// outpost reports it as the console's terminal, and its caret.
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
        (7, 0, 1),
        "the probe, the console check from the text area, and the read"
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
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

/// A UIA focus whose window has no UIA provider of its own, here an MSAA
/// window's element as UI Automation's MSAA proxy gives it, is not native
/// UIA, as NVDA's `isNativeUIAElement` decides (`docs/design/focus-pipeline.md`,
/// section 18): the listener drops it once it has asked the window for its
/// provider, one window message and no UIA call, before anything of the
/// element is read, and names the window it judged, and the window's MSAA
/// focus speaks for it.
fn a_focus_whose_window_has_no_uia_provider_is_dropped() {
    let title = common::unique_title("mockapp-msaa-window-focus");
    let app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    let listener = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let window = client
        .uia
        .any_window_element(hwnd.0 as isize, &listener)
        .expect("mockapp's window element, through the MSAA proxy");
    let (judged, calls) = outpost.listener_read(&window);
    let Judged::DroppedOnVerdict {
        window: judged_window,
        verdict,
    } = judged
    else {
        panic!("the listener read a focus in a window without UIA: {judged:?}");
    };
    assert_eq!(
        (judged_window, verdict),
        (hwnd.0 as isize, DroppedVerdict::NotUia)
    );
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (0, 0, 1),
        "the window's provider asked for, and nothing read"
    );
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
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
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
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

#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    harness::run_isolated(&[
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
            "a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read",
            a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read,
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
            "a_focus_queued_from_another_application_after_the_copy_answers_the_wait",
            a_focus_queued_from_another_application_after_the_copy_answers_the_wait,
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
            "a_focus_out_of_time_is_judged_by_its_windows_kept_verdict",
            a_focus_out_of_time_is_judged_by_its_windows_kept_verdict,
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
            "a_focus_whose_windows_probe_outlasts_the_read_budget_is_read_in_time",
            a_focus_whose_windows_probe_outlasts_the_read_budget_is_read_in_time,
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
            "a_focus_whose_window_has_no_uia_provider_is_dropped",
            a_focus_whose_window_has_no_uia_provider_is_dropped,
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
