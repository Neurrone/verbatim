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
//! how a change raised after a focus event waits for the focus's read;
//! that a text change raised after the focus leaves a document and returns
//! to it is heard; and that a foreground event for a window read through
//! UIA is dropped, the window met only among the focus's ancestors.
//!
//! The test stands in for the listener (`common::outpost`), reading each
//! focus with the listener's own reader on this thread; mockapp's focus
//! moves with `set-focus`, which raises no event.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{NodeSnapshot, NormalizedEvent, PropertyChange, Role, State, StateSet};
use verbatim_outpost::focus_read::Judged;
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::{ElementExt as _, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId, UIA_NamePropertyId,
};

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
    fn focused(&self) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("a cache request");
        let root = self
            .uia
            .element_from_handle(self.hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        let condition = self
            .uia
            .property_condition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
            .expect("a condition");
        root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
            .expect("the search")
            .expect("mockapp reports a focused element")
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
/// the listener compares the sender with the last focus it accepted and,
/// UI Automation equating them, reads that element's keyboard focus: two
/// calls. Remotely, the earlier element's focus is read in the focus's one
/// round trip; an earlier element that is gone fails the whole run before
/// it starts, which is run again without it, one call more. Classically,
/// the read makes five calls: the focus's keyboard focus, the earlier
/// element's, the focus's own cache, and two hops up to an ancestor of the
/// focus before, with its window. The repeated focus makes the comparison
/// and the keyboard focus read alone.
fn reused_runtime_id(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let gone_cost = if remote { 4 } else { 7 };
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
        verbatim_uia::runtime_id(&inner),
        verbatim_uia::runtime_id(&delta),
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
    assert_eq!(calls.uia, 2, "{path}: the calls the repeated focus made");
    outpost.deliver(judged.into_fact(0));
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
/// names (`docs/parity.md`, "A control's own focus with a focused child").
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
    outpost.deliver(judged.into_fact(7));
    outpost.settled();

    let slider = client.focused();
    let reported = outpost.uia_focus(&slider);
    assert_eq!(reported.node.name.as_deref(), Some("Level"));

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
    element: &IUIAutomationElement,
) -> NodeSnapshot {
    let (judged, _) = outpost.listener_read(element);
    let Judged::Read(mut read) = judged else {
        panic!("the listener dropped the focus: {judged:?}");
    };
    read.ancestors = None;
    read.window = None;
    outpost.deliver(DeliveredFact::UiaFocusRead { marker: 0, read });
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    foreground: false,
                    ancestors,
                    ancestors_unknown: true,
                    selected_child: None,
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

/// Focuses `id` in mockapp's caret watch fixture and returns the focus the
/// outpost reports, which must be the next thing it says, followed by the
/// caret it reports for a document, and nothing more.
fn focus_in_caret_fixture(
    app: &mut common::MockApp,
    client: &Client,
    outpost: &OutpostUnderTest,
    id: &str,
) -> NodeSnapshot {
    app.send(&format!("set-focus {id}"));
    let element = client.focused();
    outpost.deliver(outpost.listener_fact(&element));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node,
        other => panic!("the outpost said {other:?}, not the focus on {id}"),
    };
    if node.role == Role::EditableText {
        match outpost.next() {
            OutpostToSupervisor::Event {
                event: NormalizedEvent::CaretMoved { node_id, .. },
                ..
            } if node_id == node.id => {}
            other => panic!("the outpost said {other:?}, not the document's caret"),
        }
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
    outpost.deliver(read.into_fact(3));
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

/// A windowless UIA focus is tested by NVDA's acceptance test against its
/// nearest window, the one the listener's read found
/// (`docs/design/focus-pipeline.md`, section 16.2), not passed untested for
/// want of a window of its own: with another application's window in
/// front, mockapp's button's focus is dropped, and the outpost says
/// nothing; once mockapp's window is in front, the same read is reported.
fn a_windowless_focus_is_accepted_only_once_its_window_is_in_front() {
    let title = common::unique_title("mockapp-focus-in-front");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let read = outpost.listener_fact(&client.focused());
    let DeliveredFact::UiaFocusRead { read: button, .. } = &read else {
        panic!("the listener sent {read:?}, not a read");
    };
    assert_eq!(
        (button.hwnd, button.window),
        (0, Some(hwnd.0 as isize)),
        "a windowless focus whose nearest window is mockapp's"
    );
    outpost.set_foreground(common::another_applications_window());
    outpost.deliver(read.clone());
    outpost.settled();

    outpost.set_foreground(hwnd);
    let reported = outpost.focus(read);
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
/// foreground report before it. mockapp's window is not shown, so a
/// foreground report waiting for it to be shown would be made just before
/// that focus.
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

/// A focus event repeated on the element the listener last accepted, which
/// still has the keyboard focus, is dropped before anything of it is read,
/// as NVDA's duplicate rule drops it (`docs/design/focus-pipeline.md`,
/// section 16.3): the listener makes two calls, the comparison of the
/// sender with that element and the element's keyboard focus, and the
/// outpost says nothing. Once another focus has been accepted in between,
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
        (2, 0, 0),
        "the comparison and the keyboard focus read, and nothing else"
    );
    outpost.deliver(Judged::Dropped(reason).into_fact(0));
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

/// The console host's window is the parent of its text area, reports the
/// keyboard focus whenever the text area has it, and raises focus events
/// around the text area's; those are never the focus (NVDA refuses them
/// whatever the element reports, `consoleUIAWindow` in
/// `NVDAObjects/UIA/winConsoleUIA.py`). So the focus listener drops a UIA
/// focus on a console window's own element before it reads anything, even
/// while that element reports the keyboard focus: the text area's own
/// focus is the one reported.
fn a_console_windows_own_focus_is_never_reported() {
    let title = common::unique_title("mockapp-console-window");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus root");
    let cache = client.uia.base_cache_request().expect("a cache request");
    let window = client
        .uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let (judged, calls) = outpost.listener_read(&window);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read the console window's own focus: {judged:?}");
    };
    assert_eq!(reason, "the console window's own focus");
    assert_eq!(calls.uia, 0, "dropped before any read");
    outpost.deliver(judged.into_fact(0));
    outpost.settled();

    drop(outpost);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
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
            "a_text_change_after_the_focus_returns_to_a_document_is_heard",
            a_text_change_after_the_focus_returns_to_a_document_is_heard,
        ),
        (
            "a_change_raised_after_a_focus_event_waits_for_the_focus_read",
            a_change_raised_after_a_focus_event_waits_for_the_focus_read,
        ),
        (
            "a_windowless_focus_is_accepted_only_once_its_window_is_in_front",
            a_windowless_focus_is_accepted_only_once_its_window_is_in_front,
        ),
        (
            "a_console_windows_own_focus_is_never_reported",
            a_console_windows_own_focus_is_never_reported,
        ),
        (
            "a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read",
            a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read,
        ),
        (
            "a_foreground_event_for_a_window_read_through_uia_is_dropped",
            a_foreground_event_for_a_window_read_through_uia_is_dropped,
        ),
    ]);
}
