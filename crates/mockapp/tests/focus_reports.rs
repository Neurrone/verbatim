//! How a UIA focus from mockapp's provider is judged and read by the focus
//! listener's reader and reported by a real outpost
//! (`books/dev/src/explanation/design/focus.md`, "One UI Automation focus, end to end"): under
//! which node, when
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

use std::sync::{Arc, Mutex, PoisonError, mpsc};

use verbatim_core::SrState;
use verbatim_listener_logic::focus::{FocusEvent, Judged, NOT_FOCUSED, NOT_THE_LAST_READ};
use verbatim_model::protocol::{
    CoreToOutpost, DeliveredFact, OutpostToCore, UiaFocusRead, UiaSnapshotFact, WinEventStamp,
};
use verbatim_model::{
    CallCounts, EarlierElement, Effect, Fetches, FocusText, Input, NodeDetails, NormalizedEvent,
    OutpostId, Pid, Role, SegmentContent, State, StateSet, WindowHandle,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{
    Cached, ListenerEvent, ListenerSelection, Part, Registration, Subscription, Uia,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, UIA_NamePropertyId, UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::Interface;

use common::core_driver::CoreDriver;
use common::outpost::OutpostUnderTest;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{FocusCaret, MsaaIds, RouteEnd};
use verbatim_model::tell::{EventObject, Payload};

use common::outpost::Focused;

/// No speech.
const NOTHING: [Vec<SegmentContent>; 0] = [];

/// What Core says of mockapp's button "Original Name" focused as the
/// window's first focus: the window, then the button.
fn window_and_button() -> Vec<Vec<SegmentContent>> {
    vec![
        vec![
            SegmentContent::Label("Mockapp Events Fixture".to_owned()),
            SegmentContent::Role(Role::Window),
        ],
        vec![
            SegmentContent::Label("Original Name".to_owned()),
            SegmentContent::Role(Role::Button),
        ],
    ]
}

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
/// outpost's completion of the new focus finds the element it kept under
/// that runtime id gone, evicts it, and answers the focus under the
/// runtime id's node, saying the id was reused, which tells Core it is a
/// new element (`docs/parity/focus-events.md`, "Duplicate focus
/// suppression"). A focus repeated on the new element, which still has the
/// focus, is read in full and answered under the same node, the id no
/// longer reused, for Core to judge. Both ways the outpost reads a focus:
/// with remote operations and without.
fn reused_runtime_id(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
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
    let reported = outpost.uia_focus_answer(&delta);
    assert_eq!(reported.name(), Some("delta.txt"), "{path}");
    let dead_node = reported.node();

    app.send("take-runtime-id inner delta");
    app.send("set-focus inner");
    let inner = client.focused();
    assert_eq!(
        inner.runtime_id(),
        delta.runtime_id(),
        "{path}: mockapp gave Inner the dead item's runtime id"
    );
    let reported = outpost.uia_focus_answer(&inner);
    assert_eq!(
        (
            reported.name(),
            reported.node(),
            reported.read.runtime_id_reused
        ),
        (Some("Inner"), dead_node, true),
        "{path}: the runtime id keeps its node, the element it named gone"
    );

    let again = outpost.uia_focus_answer(&inner);
    assert_eq!(
        (again.name(), again.node(), again.read.runtime_id_reused),
        (Some("Inner"), dead_node, false),
        "{path}: the same node again, its element its own"
    );

    drop(outpost);
    app.quit();
}

fn a_reused_runtime_id_keeps_its_node_with_its_element_gone_remote() {
    reused_runtime_id(true);
}

fn a_reused_runtime_id_keeps_its_node_with_its_element_gone_classic() {
    reused_runtime_id(false);
}

/// The mockapp command that gives node `second` the runtime id of node
/// `first`, and the evidence the focus listener's reader then reads of
/// `first`: gone when `first` dies (`take-runtime-id`), not focused when it
/// lives on (`share-runtime-id`).
fn reuse_of_first(alive: bool) -> (&'static str, EarlierElement) {
    if alive {
        ("share-runtime-id second first", EarlierElement::NotFocused)
    } else {
        ("take-runtime-id second first", EarlierElement::Gone)
    }
}

/// The line at a focus's caret, when its answer read one.
fn caret_line(focused: &Focused) -> Option<&str> {
    match &focused.read.caret {
        FocusCaret::Read(read) => Some(read.caret.line.text.as_str()),
        FocusCaret::NoText | FocusCaret::NotRead => None,
    }
}

/// An outpost reading text classically, whose caret read takes the text
/// pattern kept for the node, where the remote read fetches it from the
/// node's element inside its program.
fn classic_outpost(app: &common::MockApp) -> OutpostUnderTest {
    OutpostUnderTest::with_options(
        app,
        OutpostOptions {
            remote_operations: false,
        },
    )
}

/// A text field focused, then a second whose application gave it the
/// first's runtime id: the second is answered under the first's node, the
/// id reused, and with its own line, read from its own element, since the
/// node's text read from the first is dropped before the caret read
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression"); the
/// second's own element stays kept for the node while the first's is
/// evicted, so its changes are followed.
fn a_text_focus_under_a_reused_runtime_id_reads_its_own_line(alive: bool) {
    let (reuse, _) = reuse_of_first(alive);
    let title = common::unique_title("mockapp-reuse-text");
    let mut app = common::spawn("reuse_text.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = classic_outpost(&app);

    app.send("set-focus first");
    let first = client.focused();
    let reported = outpost.uia_focus_answer(&first);
    assert_eq!(
        (reported.name(), caret_line(&reported)),
        (Some("First"), Some("alpha line\n"))
    );
    let node = reported.node();

    app.send(reuse);
    app.send("set-focus second");
    let second = client.focused();
    assert_eq!(
        second.runtime_id(),
        first.runtime_id(),
        "mockapp gave the second field the first's runtime id"
    );
    let reported = outpost.uia_focus_answer(&second);
    assert_eq!(
        (
            reported.name(),
            reported.node(),
            reported.read.runtime_id_reused,
            caret_line(&reported)
        ),
        (Some("Second"), node, true, Some("beta line\n"))
    );
    assert_followed(&mut app, &outpost, "second", &second);

    drop(outpost);
    app.quit();
}

fn a_text_focus_under_the_runtime_id_of_a_field_that_lost_the_focus_reads_its_own_line() {
    a_text_focus_under_a_reused_runtime_id_reads_its_own_line(true);
}

fn a_text_focus_under_the_runtime_id_of_a_field_gone_reads_its_own_line() {
    a_text_focus_under_a_reused_runtime_id_reads_its_own_line(false);
}

/// A text field focused, then a second given the first's runtime id while
/// the first lives on, whose own element the outpost cannot find, its
/// group dying as the outpost's walk down asks for its first child: the
/// node still keeps the first field's element, whose text is not the
/// second's, so the focus is answered with its text not read, never the
/// first field's line.
fn a_text_focus_whose_element_is_not_found_under_a_reused_runtime_id_is_not_read() {
    let title = common::unique_title("mockapp-reuse-text-unfound");
    let mut app = common::spawn("reuse_text.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = classic_outpost(&app);

    app.send("set-focus first");
    let reported = outpost.uia_focus_answer(&client.focused());
    assert_eq!(caret_line(&reported), Some("alpha line\n"));
    let node = reported.node();

    app.send("share-runtime-id second first");
    app.send("set-focus second");
    let second = client.focused();
    app.send("disconnect-at-first-child right");
    let reported = outpost.uia_focus_answer(&second);
    assert_eq!(
        (reported.name(), reported.node(), &reported.read.caret),
        (Some("Second"), node, &FocusCaret::NotRead)
    );

    drop(outpost);
    app.quit();
}

/// A terminal focused and read, then a second whose application gave it
/// the first's runtime id while the first lives on: the second is
/// reported under the first's node, not focused the evidence, and its
/// first read is the first of a terminal of its own, numbered 1, since the
/// node's terminal entry went with the first's element in the report's
/// commit.
fn a_terminal_under_the_runtime_id_of_one_that_lost_the_focus_is_read_afresh() {
    let title = common::unique_title("mockapp-reuse-terminal");
    let mut app = common::spawn("reuse_terminal.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);
    let focus_and_first_read = |earlier: Option<EarlierElement>| {
        outpost.deliver(outpost.listener_fact(&client.focused()));
        let node = match outpost.next() {
            OutpostToCore::Event {
                event:
                    NormalizedEvent::FocusChanged {
                        node, earlier: e, ..
                    },
                ..
            } if e == earlier => node,
            other => panic!("the outpost said {other:?}, not the terminal's focus"),
        };
        let read = match outpost.next() {
            OutpostToCore::Event {
                event: NormalizedEvent::TerminalSnapshot { node_id, snapshot },
                ..
            } if node_id == node.id => snapshot.read,
            other => panic!("the outpost said {other:?}, not the terminal's first read"),
        };
        outpost.settled();
        (node.role, node.id, read)
    };

    app.send("set-focus first");
    let (role, node, read) = focus_and_first_read(None);
    assert_eq!((role, read), (Role::Terminal, 1));

    app.send("share-runtime-id second first");
    app.send("set-focus second");
    assert_eq!(
        focus_and_first_read(Some(EarlierElement::NotFocused)),
        (Role::Terminal, node, 1)
    );

    drop(outpost);
    app.quit();
}

/// File Explorer, going back from a subfolder, raised Inner's focus event
/// before it selected Inner, so the event's states said "not selected".
/// NVDA reads a focus's states when it handles the focus, not from the
/// event, and said "Inner 1 of 2"; the outpost's completion reads them
/// afresh after the focus's registration, and Core says them.
fn a_focus_reports_the_states_read_when_it_is_handled() {
    let title = common::unique_title("mockapp-live-states");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus inner");
    let event = client.focused();
    app.send("select inner");
    let reported = outpost.uia_focus_answer(&event);
    assert_eq!(
        (reported.role(), reported.name(), reported.states()),
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
    let values = &reported.read.focus.read.values;
    assert_eq!(
        (
            common::outpost::value(values, PropertyId::PositionInSet),
            common::outpost::value(values, PropertyId::SetSize)
        ),
        (
            Some(&PropertyValue::Count(1)),
            Some(&PropertyValue::Count(2))
        )
    );

    drop(outpost);
    app.quit();
}

/// A Win32 tree view taking the focus raises focus on itself and then on
/// its focused item, within its one `SetFocus` call. NVDA handles both
/// together, newest first, and announces the item with the tree as its new
/// ancestor; a run of the tree's event alone, as when it is read before
/// the item's arrives, asks the tree's `accFocus` and answers the item it
/// names (`docs/parity/windows-and-processes.md`, "A control's own focus
/// with a focused child"), with the child's ids the redirect reached, so
/// that the item's own event, its duplicate, says nothing more.
fn a_controls_own_focus_reports_its_focused_child() {
    let title = common::unique_title("mockapp-focus-child");
    let mut app = common::spawn("focus_child.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The tree is node 1, after the root, so its object id is 2; General
    // is its first child.
    let tree = 2;

    common::apply(&mut app, hwnd, "focus-child categories general");
    let outpost = OutpostUnderTest::new(&app);
    let reported = outpost.msaa_focus_answer(MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: tree,
        child: 0,
    });
    assert_eq!(
        (reported.role(), reported.name()),
        (Role::TreeItem, Some("General"))
    );
    assert_eq!(
        reported.read.ancestors.first().map(|tree| (
            common::outpost::role_of(&tree.node),
            common::outpost::name_of(&tree.node)
        )),
        Some((Role::Tree, Some("Categories")))
    );
    assert_eq!(
        reported.read.redirected_to,
        Some(MsaaIds {
            window: WindowHandle(hwnd.0 as u64),
            object: tree,
            child: 1,
        }),
        "the item the redirect reached, which its own event names"
    );

    drop(outpost);
    app.quit();
}

/// An MSAA focus event at the address of the focus last committed is read
/// in full and answered again, a full object under the same node and a
/// simple child too, since whether it repeats the focus is Core's verdict
/// (`books/dev/src/explanation/design/ownership.md`).
fn a_repeated_msaa_focus_is_read_in_full() {
    let title = common::unique_title("mockapp-msaa-duplicate");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The button is node 1, after the root, so its object id is 2.
    let button = MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: 2,
        child: 0,
    };
    common::apply(&mut app, hwnd, "set-focus btn1");
    let outpost = OutpostUnderTest::new(&app);
    let reported = outpost.msaa_focus_answer(button);
    assert_eq!(reported.role(), Role::Button);
    let again = outpost.msaa_focus_answer(button);
    assert_eq!(
        (again.node(), again.role()),
        (reported.node(), Role::Button),
        "the same object, under the same node"
    );
    drop(outpost);
    app.quit();

    let title = common::unique_title("mockapp-msaa-child-repeat");
    let mut app = common::spawn("focus_child.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The tree is node 1, so its object id is 2; General is child 1.
    let general = MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: 2,
        child: 1,
    };
    common::apply(&mut app, hwnd, "focus-child categories general");
    let outpost = OutpostUnderTest::new(&app);
    for _ in 0..2 {
        let reported = outpost.msaa_focus_answer(general);
        assert_eq!(
            (reported.role(), reported.name()),
            (Role::TreeItem, Some("General"))
        );
    }
    drop(outpost);
    app.quit();
}

/// A UIA focus event whose element has lost the keyboard focus to another
/// element by the time the listener checks it is dropped, as NVDA's
/// `shouldAllowUIAFocusEvent` drops it: Core asks nothing for it and says
/// nothing; the newer focus is spoken from its own event.
fn a_focus_whose_element_lost_the_keyboard_focus_before_it_was_read_is_dropped() {
    let title = common::unique_title("mockapp-lost-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let button = client.focused();
    app.send("set-focus slider1");
    let step = core.uia_focus(&button);
    assert_eq!(
        (step.said, step.calls.uia),
        (Vec::new(), 0),
        "nothing said, and nothing read past the listener's check"
    );

    let slider = client.focused();
    let said = core.uia_focus(&slider).spoken();
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Events Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                SegmentContent::Label("Level".to_owned()),
                SegmentContent::Role(Role::Slider),
                SegmentContent::Value("1".to_owned())
            ],
        ]
    );

    drop(core);
    app.quit();
}

/// A UIA focus on the task switcher's item, a `ListViewItem` list item,
/// carries its raw-view parent's automation id among its cached
/// properties, read by the check thread with one call, which Core's rule
/// for the switcher's items matches (`docs/parity/windows-and-processes.md`,
/// "The Alt+Tab switcher's pane"); a focus of another class carries none.
fn a_focus_carries_its_parents_automation_id() {
    let title = common::unique_title("mockapp-parent-automation-id");
    let mut app = common::spawn("switcher.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus window_b");
    let item = client.focused();
    let tell = core.uia_tell(EventKind::UiaFocus, &item, None, Payload::None);
    assert_eq!(
        tell.cached
            .answer(PropertyId::ParentAutomationId)
            .and_then(|answer| answer.value.clone()),
        Some(PropertyValue::Text("SwitchItemListControl".to_owned()))
    );

    app.send("set-focus switch_list");
    let list = client.focused();
    let tell = core.uia_tell(EventKind::UiaFocus, &list, None, Payload::None);
    assert_eq!(
        tell.cached
            .answer(PropertyId::ParentAutomationId)
            .map(|answer| answer.value.clone()),
        Some(None)
    );

    drop(core);
    app.quit();
}

/// A UIA focus event whose element still had the keyboard focus when the
/// listener checked it is read and spoken, whatever has happened since:
/// Windows Terminal's Control+Tab gives the focus to the tab it leaves,
/// then to the tab it selects, and NVDA queues both, culling the first
/// once the second is applied (`docs/nvda/editable-text-and-terminals.md`,
/// "Terminals"). The first focus is told, with its check made, before the
/// focus moves on; it is read and spoken, its states read when the outpost
/// completes it, after the selection moved on, as NVDA reads them when it
/// speaks, so it is "not selected"; then the item moved to after it.
/// "First not selected" awaits a `verbatim-event-record` recording of
/// Windows Terminal's Control+Tab and an NVDA capture of it, which will
/// confirm or correct the expectation.
fn a_focus_read_while_it_had_the_keyboard_focus_is_reported_though_the_focus_moved_on() {
    let title = common::unique_title("mockapp-left-selection");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("select item1");
    app.send("set-focus item1");
    let first = core.uia_tell(EventKind::UiaFocus, &client.focused(), None, Payload::None);

    app.send("select item2");
    app.send("set-focus item2");
    let said = core.tell(first).spoken();
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Events Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                SegmentContent::Label("Options".to_owned()),
                SegmentContent::Role(Role::List)
            ],
            vec![
                SegmentContent::Label("First".to_owned()),
                SegmentContent::NegatedState(State::Selected)
            ],
        ]
    );

    let second = client.focused();
    let said = core.uia_focus(&second).spoken();
    assert_eq!(said, [vec![SegmentContent::Label("Second".to_owned())]]);

    drop(core);
    app.quit();
}

/// Completes the listener's read of a UIA focus on `element` with its
/// route cut to the element alone and no window known above it, as for a
/// focus whose element is in no tree, so the outpost has no route to its
/// own copy of the element. Returns the focus it answers.
fn focus_without_a_path(outpost: &OutpostUnderTest, element: &Cached<ListenerEvent>) -> Focused {
    let (mut focus, _) = outpost.listener_focus(element);
    let route = focus
        .object
        .route
        .as_mut()
        .expect("a focus read answers its route");
    route.steps.truncate(1);
    route.end = RouteEnd::Root;
    focus.object.ancestors.clear();
    let (outcome, calls) = outpost.complete_focus(focus);
    Focused::of(outcome, calls)
}

/// Renames `id`, the focus, whose element is `element`, and asserts the
/// outpost tells Core of the change with the name it carried: the focus's
/// changes are followed.
fn assert_followed(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    id: &str,
    element: &Cached<ListenerEvent>,
) {
    app.send(&format!("set-name {id} Renamed"));
    match outpost.next() {
        OutpostToCore::Tell(tell) => assert_eq!(
            (tell.kind, tell.object, tell.payload),
            (
                EventKind::UiaPropertyChanged,
                EventObject::Followed {
                    runtime_id: RuntimeId(element.runtime_id())
                },
                Payload::PropertyChanged {
                    property: PropertyId::Name,
                    value: Some(PropertyValue::Text("Renamed".to_owned())),
                }
            )
        ),
        other => panic!("the outpost said {other:?}, not the focus's new name"),
    }
}

/// The outpost finds its own copy of a windowless focus's element by the
/// route the listener read, from mockapp's window down by runtime ids,
/// and follows the element's changes from then on.
fn a_focus_is_found_by_its_path_and_followed() {
    let title = common::unique_title("mockapp-focus-path");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let reported = outpost.uia_focus_answer(&button);
    assert_eq!(
        reported.chain(),
        [Some("Mockapp Events Fixture"), Some("Original Name")]
    );
    assert_followed(&mut app, &outpost, "btn1", &button);

    drop(outpost);
    app.quit();
}

/// A windowless focus whose walk up failed below its nearest window, so
/// that its route holds no window of its own, is found by the outpost from
/// that nearest window, which the route carries, down by the route's
/// runtime ids, and its changes are followed: mockapp's button, a child of
/// its window, with its route cut to the button alone, going on from its
/// window's parent, as a walk whose first step failed sends it.
fn a_windowless_focus_with_a_failed_walk_is_found_from_its_nearest_window() {
    let title = common::unique_title("mockapp-failed-walk-path");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let (mut focus, _) = outpost.listener_focus(&button);
    cut_route(&mut focus, hwnd);
    let (outcome, calls) = outpost.complete_focus(focus);
    let reported = Focused::of(outcome, calls);
    assert_eq!(reported.name(), Some("Original Name"));
    assert_followed(&mut app, &outpost, "btn1", &button);

    drop(outpost);
    app.quit();
}

/// A UIA focus with no route to its element, as Windows Terminal's tab
/// focus elements, which are in no tree, have none, is answered from the
/// listener's read, and nothing is followed: a change of the element is
/// not told. Once the focus has been elsewhere and comes back, its focus
/// is completed with its route and finds the element: the focus is
/// answered under its node, and its changes are followed from then on.
fn a_focus_without_a_path_is_followed_from_its_next_focus_event() {
    let title = common::unique_title("mockapp-focus-not-found");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let node = focus_without_a_path(&outpost, &button).node();

    app.send("set-focus slider1");
    let _ = outpost.uia_focus_answer(&client.focused());
    app.send("set-focus btn1");
    let reported = outpost.uia_focus_answer(&button);
    assert_eq!(
        (reported.node(), reported.name()),
        (node, Some("Original Name"))
    );
    assert_followed(&mut app, &outpost, "btn1", &button);

    drop(outpost);
    app.quit();
}

/// A text field with no window of its own whose walk up the listener's
/// step failed, so that the route it sends holds no window, is found by
/// the outpost as its application's focused element, read with the focus
/// read's process-id check and taken because its runtime id is the
/// field's (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's
/// own element"). The field, "Notes", is four levels below its window,
/// where the walk down from its nearest window by the route's runtime ids
/// does not reach it. Found, the field has no text pattern, so its text
/// is answered as none, `NoText`; not found, it would be `NotRead`.
fn a_windowless_text_focus_whose_walk_failed_is_found_as_the_focused_element() {
    let title = common::unique_title("mockapp-cut-walk");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let notes = client.focused();
    outpost.read_focus_as(notes.live().element());
    let (mut focus, _) = outpost.listener_focus(&notes);
    cut_route(&mut focus, hwnd);
    let (outcome, calls) = outpost.complete_focus(focus);
    let reported = Focused::of(outcome, calls);
    assert_eq!(
        (
            reported.name(),
            reported.read.ancestors.len(),
            &reported.read.caret
        ),
        (Some("Notes"), 0, &FocusCaret::NoText),
        "no ancestor: the chain goes on from the parent of the field's nearest window"
    );
    // The route from the field's nearest window, its element read with
    // one connection call and the walk down the route's runtime ids, which
    // does not reach the field four levels below, two UIA calls; the
    // focused-element read, one UIA call, its stand-in counting no
    // connection; the field's registration and its states and caret; and
    // the foreground object of the window the focus enters, one connection,
    // one UIA call and one message. Without the read the field is not
    // found.
    assert_eq!(
        reported.calls,
        CallCounts {
            uia: 7,
            msaa: 0,
            window_messages: 1,
            connections: 2,
        }
    );

    drop(outpost);
    app.quit();
}

/// Cuts the route of the listener's read `focus`, a windowless element of
/// window `hwnd`, to the element alone, going on from the window's parent,
/// as a walk up that failed below the element's nearest window sends it.
fn cut_route(focus: &mut common::outpost::ListenerFocus, hwnd: HWND) {
    let route = focus
        .object
        .route
        .as_mut()
        .expect("a focus read answers its route");
    assert_eq!(
        route.steps.first().and_then(|step| step.window),
        None,
        "the element has no window of its own"
    );
    route.steps.truncate(1);
    route.end = RouteEnd::ContinueFrom(WindowHandle(
        u64::try_from(verbatim_uia::parent_window(hwnd.0 as isize)).expect("a window handle"),
    ));
    focus.object.ancestors.clear();
}

/// Completes `focus`, the listener's read of "Notes" whose element no route
/// leads to, and asserts the outpost answers the field with its text
/// `caret`, after the calls `calls`.
fn assert_notes_reported(
    outpost: &OutpostUnderTest,
    focus: common::outpost::ListenerFocus,
    caret: &FocusCaret,
    calls: CallCounts,
) {
    let (outcome, made) = outpost.complete_focus(focus);
    let reported = Focused::of(outcome, made);
    assert_eq!(
        (reported.name(), &reported.read.caret, reported.calls),
        (Some("Notes"), caret, calls)
    );
}

/// After a cut walk, the application's focused element is taken as the
/// focus's only when its runtime id is the focus's: here it is the button
/// "Deep button", the field's sibling, so the field is not found and its
/// text is not read, `NotRead`; taken, the button would give `NoText`.
/// The route from the field's nearest window, one connection call and two
/// UIA calls, which does not reach the field four levels below; the
/// focused-element read, one UIA call; and the foreground object of the
/// window the focus enters, one connection, one UIA call and one message.
fn a_cut_walks_focused_element_with_another_runtime_id_is_not_taken() {
    let title = common::unique_title("mockapp-cut-walk-other-id");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let (mut focus, _) = outpost.listener_focus(&client.focused());
    cut_route(&mut focus, hwnd);
    outpost.read_focus_as(client.named("Deep button").live().element());
    assert_notes_reported(
        &outpost,
        focus,
        &FocusCaret::NotRead,
        CallCounts {
            uia: 4,
            msaa: 0,
            window_messages: 1,
            connections: 2,
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
/// `NoText`. The route from the nearest window the route carries, this
/// application's, one connection call and two UIA calls, which does not
/// find the other's field; the focused-element read, one UIA call; and the
/// foreground object of the window the focus enters, one connection, one
/// UIA call and one message.
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
    let (mut focus, _) = outpost.listener_focus(&notes);
    cut_route(&mut focus, other_hwnd);
    focus
        .object
        .route
        .as_mut()
        .expect("a focus read answers its route")
        .window = WindowHandle(hwnd.0 as u64);
    outpost.read_focus_as(notes.live().element());
    assert_notes_reported(
        &outpost,
        focus,
        &FocusCaret::NotRead,
        CallCounts {
            uia: 4,
            msaa: 0,
            window_messages: 1,
            connections: 2,
        },
    );

    drop(outpost);
    other.quit();
    app.quit();
}

/// A focus with no window of its own whose route holds its ancestors, none
/// with a window either, has no route to its element, but its walk up was
/// not cut, so the application's focused element is not read for it,
/// though that element is the field: the field is not found and its text
/// is not read, `NotRead`, with no UIA call; read, it would give `NoText`.
fn a_focus_whose_walk_was_not_cut_is_not_read_as_the_focused_element() {
    let title = common::unique_title("mockapp-uncut-walk");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus notes");
    let notes = client.focused();
    let (mut focus, _) = outpost.listener_focus(&notes);
    let route = focus
        .object
        .route
        .as_mut()
        .expect("a focus read answers its route");
    for step in &mut route.steps {
        step.window = None;
    }
    route.end = RouteEnd::Root;
    outpost.read_focus_as(notes.live().element());
    assert_notes_reported(
        &outpost,
        focus,
        &FocusCaret::NotRead,
        CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        },
    );

    drop(outpost);
    app.quit();
}

/// An ancestor of the focus, whose element the outpost does not hold, is
/// answered not there when the walk down the focus's route shows it gone:
/// once mockapp has disconnected "Inner group", four levels below its
/// window, a call on the element fails as on one that is gone. The walk is
/// the classic one.
fn a_route_walk_that_finds_its_element_gone_answers_gone() {
    let title = common::unique_title("mockapp-route-gone");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );

    app.send("set-focus notes");
    let reported = outpost.uia_focus_answer(&client.focused());
    let group = reported
        .read
        .ancestors
        .iter()
        .find(|ancestor| common::outpost::name_of(&ancestor.node) == Some("Inner group"))
        .and_then(|ancestor| ancestor.node.read.node)
        .expect("the focus's chain has the inner group");
    app.send("disconnect group2");
    let answer = outpost.requested(
        hwnd,
        verbatim_model::request::Request::Navigate {
            node: group,
            direction: verbatim_model::QueryKind::FirstChild,
            set: vec![PropertyId::Name],
        },
    );
    assert_eq!(answer.outcome, verbatim_model::request::Outcome::NotThere);

    drop(outpost);
    app.quit();
}

/// Focuses `id` in mockapp's caret watch fixture and returns the focus the
/// outpost answers, with its caret for a document and with no text read
/// for anything else.
fn focus_in_caret_fixture(
    app: &mut common::MockApp,
    client: &Client,
    outpost: &OutpostUnderTest,
    id: &str,
) -> Focused {
    app.send(&format!("set-focus {id}"));
    let focused = outpost.uia_focus_answer(&client.focused());
    assert_eq!(
        matches!(focused.read.caret, FocusCaret::Read(_)),
        focused.role() == Role::EditableText,
        "the caret with a document's focus alone: {:?}",
        focused.read.caret
    );
    focused
}

/// The focus-following registration moves with the focus as NVDA's local
/// group does, added to the new focus as its completion reads it
/// (`docs/design/readers-tell-core-asks-build1.md`, "Changes on objects"):
/// after the focus leaves the document for a button and returns, a text
/// change the document raises is heard and reported for its node.
fn a_text_change_after_the_focus_returns_to_a_document_is_heard() {
    let title = common::unique_title("mockapp-focus-returns");
    let mut app = common::spawn("caret_watch.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    let notes = focus_in_caret_fixture(&mut app, &client, &outpost, "doc");
    assert_eq!(notes.name(), Some("Notes"));
    let ok = focus_in_caret_fixture(&mut app, &client, &outpost, "ok");
    assert_eq!(ok.name(), Some("OK"));
    let returned = focus_in_caret_fixture(&mut app, &client, &outpost, "doc");
    assert_eq!(returned.node(), notes.node());

    app.send("text-changed doc");
    match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::TextChanged { node_id },
            ..
        } => assert_eq!(node_id, notes.node()),
        other => panic!("the outpost said {other:?}, not the document's text change"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A windowless UIA focus is tested by NVDA's acceptance test against its
/// nearest window, which Core judges from the tell's facts before it asks
/// anything (`docs/parity/focus-events.md`, "Event acceptance"): with
/// another application's window in front, mockapp's button's focus is
/// dropped and nothing is said. A dropped focus never becomes the readers'
/// last focus, so when the same element's focus comes again once its
/// window is in front, as a console's text area's does, focused before its
/// window comes in front and again after, it is no repeat: it is read and
/// spoken.
fn a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is() {
    let title = common::unique_title("mockapp-focus-in-front");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    core.outpost
        .set_foreground(common::another_applications_window());
    let step = core.uia_focus(&client.focused());
    assert_eq!((step.said, step.calls.uia), (Vec::new(), 0));

    core.outpost.set_foreground(hwnd);
    assert_eq!(
        core.uia_focus(&client.focused()).spoken(),
        window_and_button()
    );

    drop(core);
    app.quit();
}

/// A foreground event for a window read through UI Automation is dropped,
/// even with the window in front, as NVDA drops every MSAA event for such
/// a window (`books/dev/src/reference/adr/0001-focus.md`, "The foreground
/// and the window title"): Core says nothing for it, and the focus inside
/// the window that follows is spoken alone, the window first.
fn a_foreground_event_for_a_window_read_through_uia_is_dropped() {
    let title = common::unique_title("mockapp-uia-foreground");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    assert_eq!(core.foreground(hwnd).spoken(), NOTHING);

    app.send("set-focus btn1");
    assert_eq!(
        core.uia_focus(&client.focused()).spoken(),
        window_and_button()
    );

    drop(core);
    app.quit();
}

/// A fake focus's MSAA answer whose object neither has the focused state
/// nor is inside one that has, its state cleared by `set-states`, is
/// refused as NVDA refuses it before it is queued: the answer carries no
/// window and no control, which the app turns into no input. The same
/// object asked for by a plain query, which has no such test, is its
/// focused control.
fn a_fake_focus_on_an_msaa_object_without_the_focused_state_answers_nothing() {
    use verbatim_model::protocol::FocusPurpose;
    let title = common::unique_title("mockapp-fake-focus-unfocused");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let mut outpost = OutpostUnderTest::new(&app);
    app.send("set-focus first");
    app.send("set-states first focusable");
    let (fake, _) = outpost.focus_now_for(FocusPurpose::Fake);
    assert_eq!((fake.window, fake.focus), (None, None));
    let (plain, _) = outpost.focus_now_for(FocusPurpose::Plain);
    assert_eq!(
        plain.focus.map(|control| control.node.name),
        Some(Some("First".to_owned()))
    );
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
/// here by its class on NVDA's MSAA list, which the test has the
/// listener's reader read anyway, is dropped: the outpost's own verdict
/// decides which backend's focus counts in each of its windows, and in an
/// MSAA window its MSAA focus speaks for it.
fn a_listener_report_in_a_window_the_outpost_judges_msaa_is_dropped() {
    let title = common::unique_title("mockapp-report-in-msaa-window");
    let mut app = common::spawn("msaa_class_window.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    outpost.listener_forces_uia(true);
    let read = outpost.listener_fact(&client.focused());
    outpost.deliver(read);
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A UIA focus whose window's class is on NVDA's MSAA class list is skipped
/// by the listener, its nearest window the one call made, as the outpost
/// judges that window MSAA by the same table.
fn the_listener_skips_a_focus_in_a_class_listed_window() {
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
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A focus event repeated on the element the listener last read, which
/// still has the keyboard focus, is read in full and reported, and Core
/// judges the repeat (`books/dev/src/explanation/design/ownership.md`):
/// the listener makes five calls, the windowless sender's nearest window
/// for NVDA's native check, the sender's keyboard focus, and the read,
/// whose runtime id is its last read's: whether that element still has
/// the keyboard focus, the sender's cache, and one step up to its known
/// parent. The outpost reports the same node again.
fn a_repeated_focus_on_the_still_focused_element_is_read_in_full() {
    let title = common::unique_title("mockapp-repeated-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = outpost.uia_focus(&client.focused());
    let (judged, calls) = outpost.listener_read(&client.focused());
    let Judged::Read(_) = &judged else {
        panic!("the listener did not read a repeated focus: {judged:?}");
    };
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (5, 0, 0),
        "the nearest window, the keyboard focus, and the read"
    );
    let again = outpost.focus(judged.into_fact(0, FocusEvent::Focus));
    assert_eq!(
        (again.node.id, again.node.name.as_deref()),
        (button.node.id, Some("Original Name"))
    );

    drop(outpost);
    app.quit();
}

/// Windows Terminal raises its terminal's focus twice as it launches and
/// then minimizes itself, so the second focus can be read as the focus is
/// being lost. The listener reads, as NVDA does, the keyboard focus of the
/// element it read before under the runtime id first, and the sender's
/// after it, in its walk: here the focus leaves between the two reads, so
/// the earlier element reads focused and the sender not, and the repeat is
/// dropped, the focus announced once
/// (`books/dev/src/reference/adr/0001-focus.md`, "The listener's checks, ported from NVDA's focus
/// handler").
fn a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once() {
    let title = common::unique_title("mockapp-focus-leaves-between");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let first = outpost.uia_focus(&client.focused());
    assert_eq!(first.node.name.as_deref(), Some("Original Name"));
    let repeated = client.focused();
    let app = Arc::new(Mutex::new(app));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (moves, seen) = (Arc::clone(&app), Arc::clone(&calls));
    outpost.listener_observes_calls(Arc::new(move |call| {
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
        if call == "the focus's keyboard focus" {
            moves
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send("set-focus slider1");
        }
    }));
    let (judged, _) = outpost.listener_read(&repeated);
    assert_eq!(
        calls.lock().unwrap_or_else(PoisonError::into_inner)[..],
        [
            "the previous focus's keyboard focus",
            "the focus's keyboard focus"
        ],
        "the earlier element's keyboard focus first, then the sender's"
    );
    let Judged::Dropped(reason) = judged else {
        panic!("the listener read a focus that had left: {judged:?}");
    };
    assert_eq!(reason, NOT_FOCUSED);
    outpost.deliver(Judged::Dropped(reason).into_fact(0, FocusEvent::Focus));
    outpost.settled();

    drop(outpost);
    let app = Arc::try_unwrap(app)
        .unwrap_or_else(|_| panic!("the observer was dropped with the outpost"))
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    app.quit();
}

/// A UIA menu opening is judged and read as a focus on the menu, as NVDA's
/// automation event handler maps it to one, by NVDA's checks but the
/// duplicate test, which is its focus handler's alone: an opening on the
/// element that is the committed focus, and still has the keyboard focus,
/// is that focus again, spoken again as NVDA's redirect to its focus
/// object speaks it; one whose element has lost the keyboard focus is
/// dropped, as NVDA's `shouldAllowUIAFocusEvent` drops it, asking nothing.
fn a_menu_opening_is_judged_as_a_focus_on_the_menu() {
    let title = common::unique_title("mockapp-menu-opening");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let button = client.focused();
    assert_eq!(core.uia_focus(&button).spoken(), window_and_button());
    assert_eq!(
        core.uia_event(EventKind::UiaMenuOpened, &button, None, Payload::None)
            .spoken(),
        vec![vec![
            SegmentContent::Label("Original Name".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );

    app.send("set-focus slider1");
    let step = core.uia_event(EventKind::UiaMenuOpened, &button, None, Payload::None);
    assert_eq!((step.said, step.calls.uia), (Vec::new(), 0));

    drop(core);
    app.quit();
}

/// A UIA menu opening whose sender is the committed focus is judged as
/// that focus, as NVDA's automation event handler redirects it to its
/// focus object (`docs/parity/focus-events.md`): Core speaks the focus
/// again and reads nothing for it.
fn a_menu_opening_on_the_current_focus_is_judged_as_that_focus() {
    let title = common::unique_title("mockapp-menu-opening-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let button = client.focused();
    let _ = core.uia_focus(&button);
    let step = core.uia_event(EventKind::UiaMenuOpened, &button, None, Payload::None);
    assert_eq!(
        (step.spoken(), step.calls),
        (
            vec![vec![
                SegmentContent::Label("Original Name".to_owned()),
                SegmentContent::Role(Role::Button)
            ]],
            CallCounts::default()
        )
    );

    drop(core);
    app.quit();
}

/// A UIA menu opening whose sender is not the committed focus is read as
/// a focus on it, through NVDA's native check of its windowless sender's
/// nearest window, and spoken as a focus.
fn a_menu_opening_on_another_element_is_judged_by_its_own_window() {
    let title = common::unique_title("mockapp-menu-opening-other");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let _ = core.uia_focus(&client.focused());
    app.send("set-focus slider1");
    let slider = client.focused();
    assert_eq!(
        core.uia_event(EventKind::UiaMenuOpened, &slider, None, Payload::None)
            .spoken(),
        [vec![
            SegmentContent::Label("Level".to_owned()),
            SegmentContent::Role(Role::Slider),
            SegmentContent::Value("1".to_owned())
        ]]
    );

    drop(core);
    app.quit();
}

/// Sets the process's UIA transaction timeout to `milliseconds`, through a
/// client of its own, on the calling thread: a setting of the whole
/// process (`books/dev/src/reference/uia-bugs.md`, "The connection timeout
/// applies to the whole process"), which the listener's reader's calls
/// then wait by.
fn set_transaction_timeout(milliseconds: u32) {
    let uia = Uia::new().expect("a UIA client");
    let timeouts: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
    // SAFETY: setting a timeout takes a plain integer.
    unsafe { timeouts.SetTransactionTimeout(milliseconds) }.expect("the timeout set");
}

/// The process's UIA transaction timeout, in milliseconds.
fn transaction_timeout() -> u32 {
    let uia = Uia::new().expect("a UIA client");
    let timeouts: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
    // SAFETY: reading a timeout returns a plain integer.
    unsafe { timeouts.TransactionTimeout() }.expect("the transaction timeout")
}

/// How long a call mockapp holds is waited for before UIA's transaction
/// timeout ends it, in the tests that hold one. UIA answers the read's
/// later calls into the held window by the same timeout, so each of them
/// fails too.
const HELD_TIMEOUT_MS: u32 = 300;

/// The listener's read of `element`, made with the process's transaction
/// timeout at [`HELD_TIMEOUT_MS`] and mockapp holding its next provider
/// call from the moment the read's call named `holding` begins, or from
/// the read's start with none: the read, and the provider method mockapp
/// held, once the usual timeout is restored and the call released.
fn read_held(
    app: common::MockApp,
    outpost: &OutpostUnderTest,
    element: &Cached<ListenerEvent>,
    holding: Option<&'static str>,
) -> (common::MockApp, Judged, String) {
    let usual = transaction_timeout();
    set_transaction_timeout(HELD_TIMEOUT_MS);
    let app = Arc::new(Mutex::new(app));
    match holding {
        None => app.lock().unwrap_or_else(PoisonError::into_inner).hold(),
        Some(holding) => {
            let held = Arc::clone(&app);
            outpost.listener_observes_calls(Arc::new(move |call| {
                if call == holding {
                    held.lock().unwrap_or_else(PoisonError::into_inner).hold();
                }
            }));
        }
    }
    let (judged, _) = outpost.listener_read(element);
    set_transaction_timeout(usual);
    // The observer, which shares mockapp, is replaced by one that shares
    // nothing, so mockapp is the test's alone again.
    outpost.listener_observes_calls(Arc::new(|_| {}));
    let mut app = Arc::try_unwrap(app)
        .unwrap_or_else(|_| panic!("the observer let mockapp go"))
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    let held = app.held();
    app.release();
    (app, judged, held)
}

/// A UIA focus whose live keyboard focus read fails by UIA's transaction
/// timeout is let through to its walk, as NVDA lets any COM error of that
/// read through (`shouldAllowUIAFocusEvent` answers true,
/// `NVDAObjects/UIA/__init__.py` 1632 to 1637), though its event's cache
/// says it lacked the keyboard focus. The walk's own read of it, into the
/// same held window, times out too, so it is reported from its event's
/// cache, its chain going on from its window's parent, read live by the
/// outpost, as NVDA's object walks its parent by `Window._get_parent`. The
/// sender is mockapp's window element, whose window's verdict an earlier
/// focus kept, so that the live read is the read's first provider call,
/// which mockapp holds past the timeout.
fn a_focus_whose_keyboard_focus_read_times_out_is_reported() {
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
    let (app, judged, held) = read_held(app, &outpost, &window, None);
    assert_eq!(held, "GetPropertyValue", "the keyboard focus read was held");
    let Judged::Read(read) = judged else {
        panic!(
            "the listener did not report a focus whose keyboard focus read timed out: {judged:?}"
        );
    };
    // Reported whole as its event's cache gives it, its own class and
    // window included, with the Focused state, in its own window, its chain
    // going on from its parent window, the desktop.
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
            ancestors: Vec::new(),
            ancestors_end: verbatim_model::protocol::AncestorsEnd::ContinueFrom(
                verbatim_uia::parent_window(hwnd.0 as isize)
            ),
            selected_child: None,
            value_from_selection: false,
            window: Some(hwnd.0 as isize),
            same_as_last: None,
            answers: None,
            cached_from: None,
        }
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus whose element is gone once its nearest window has been
/// looked up, so that its live keyboard focus read fails, is let through to
/// its walk, as NVDA's COM error rule lets a gone element through
/// (`NVDAObjects/UIA/__init__.py` 1632 to 1637): its own read then fails
/// too, and it is reported from its event's cache, its chain going on from
/// the parent window of its nearest window, mockapp's, as NVDA's object
/// walks its parent by `Window._get_parent`. mockapp's button is
/// disconnected as the keyboard focus read begins.
fn a_focus_whose_keyboard_focus_read_fails_is_walked() {
    let title = common::unique_title("mockapp-focus-read-gone");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let app = Arc::new(Mutex::new(app));
    let gone = Arc::clone(&app);
    outpost.listener_observes_calls(Arc::new(move |call| {
        if call == "the focus's keyboard focus" {
            gone.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send("disconnect btn1");
        }
    }));
    let (judged, _) = outpost.listener_read(&button);
    let Judged::Read(read) = judged else {
        panic!("the listener did not report a gone focus let through: {judged:?}");
    };
    assert_eq!(
        (
            read.hwnd,
            read.snapshot.name.as_deref(),
            read.snapshot.states.contains(State::Focused),
            read.ancestors.len(),
            read.ancestors_end,
            read.window,
        ),
        (
            0,
            Some("Original Name"),
            true,
            0,
            verbatim_model::protocol::AncestorsEnd::ContinueFrom(verbatim_uia::parent_window(
                hwnd.0 as isize
            )),
            Some(hwnd.0 as isize),
        ),
        "reported from its event, going on from its nearest window's parent"
    );

    drop(outpost);
    Arc::try_unwrap(app)
        .unwrap_or_else(|_| panic!("the observer was dropped with the outpost"))
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .quit();
}

/// A UIA focus under the runtime id of the focus read before, whose
/// earlier element's keyboard focus read fails by UIA's transaction
/// timeout, is no repeat: the earlier element reads as gone, as NVDA
/// catches its duplicate check's error and handles the event
/// (`UIAHandler/__init__.py` 902 to 917), and the read goes on. Its later
/// calls, into the same held window, time out too: the sender's keyboard
/// focus lets it through, and it is reported from its event's cache, its
/// chain going on from its nearest window's parent. mockapp holds the
/// earlier element's read, the walk's first call, past the timeout, its
/// first provider call for it being `FragmentRoot`.
fn a_focus_whose_earlier_elements_read_times_out_is_read_as_a_new_element() {
    let title = common::unique_title("mockapp-earlier-read-timed-out");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    let _ = outpost.uia_focus(&button);
    let (app, judged, held) = read_held(
        app,
        &outpost,
        &button,
        Some("the previous focus's keyboard focus"),
    );
    assert_eq!(
        held, "FragmentRoot",
        "the earlier element's keyboard focus read was held"
    );
    let Judged::Read(read) = judged else {
        panic!("the listener did not read the focus: {judged:?}");
    };
    assert_eq!(
        (
            read.snapshot.name.as_deref(),
            read.same_as_last,
            read.ancestors.len(),
            read.ancestors_end,
        ),
        (
            Some("Original Name"),
            Some(EarlierElement::Gone),
            0,
            verbatim_model::protocol::AncestorsEnd::ContinueFrom(verbatim_uia::parent_window(
                hwnd.0 as isize
            )),
        ),
        "read as a new element"
    );

    drop(outpost);
    app.quit();
}

/// A focus on a menu item that no pattern makes checkable reads its legacy
/// MSAA state, where a Windows Forms menu item says it is checked, with
/// one live read after the focus's classic read, made for an element the
/// read maps to a menu item: six calls, the nearest window, the keyboard
/// focus, the read's three (the item's cache, and steps up to the menu and
/// to the window the focus before had), and the state.
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
        (6, 0, 0),
        "the nearest window, the keyboard focus, the read, and the legacy state"
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
/// outpost's (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact"), and
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
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A console's text area's focus is read with no check of the console's
/// provider, which is its outpost's (`books/dev/src/reference/adr/0001-focus.md`, "One judge per
/// fact"): five calls, the sender's nearest window, the keyboard
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
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
    let (node, ancestors) = match outpost.next() {
        OutpostToCore::Event {
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
/// (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact": the listener's
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
    outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The focus's help text and `ControllerFor` relation are followed, as
/// NVDA's local group follows them, and told to Core, which asks one read
/// for each (`docs/design/readers-tell-core-asks-build1.md`, "Changes on
/// objects"): a help text change is spoken as the focus's new description,
/// read when Core judges it, and a search box's relation naming its list
/// and then nothing plays the suggestions opening and closing, each read
/// in one call, the refresh of the box that reads the relation live.
fn a_focus_s_help_text_and_suggestions_are_followed() {
    let title = common::unique_title("mockapp-suggestions");
    let mut app = common::spawn("controller.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus search");
    let _ = core.uia_focus(&client.focused());

    app.send("set-description search Type to search");
    assert_eq!(
        core.told(1).spoken(),
        [vec![SegmentContent::Description(
            "Type to search".to_owned()
        )]]
    );

    for (command, earcon) in [
        (
            "set-controller-for search results",
            verbatim_model::Earcon::SuggestionsOpened,
        ),
        (
            "set-controller-for search",
            verbatim_model::Earcon::SuggestionsClosed,
        ),
    ] {
        app.send(command);
        let step = core.told(1);
        assert_eq!(
            (step.said, step.calls.uia),
            (vec![common::core_driver::Said::Earcon(earcon)], 1),
            "{command}"
        );
    }

    drop(core);
    app.quit();
}

/// A followed node's change raised while another application's window is
/// in front is told with its window's facts, which the outpost's callback
/// gathers from Windows, and acceptance refuses it, as NVDA's
/// property-change handler applies `shouldAcceptEvent`: Core says nothing
/// and asks nothing; once mockapp is in front again the next change is
/// spoken.
fn a_followed_change_while_another_window_is_in_front_is_refused() {
    let title = common::unique_title("mockapp-followed-background");
    let mut app = common::spawn("controller.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let window = core.outpost.app_window();

    app.send("set-focus search");
    let _ = core.uia_focus(&client.focused());

    core.outpost
        .set_foreground(common::another_applications_window());
    app.send("set-description search Type to search");
    let step = core.told(1);
    assert_eq!((step.said, step.calls.uia), (Vec::new(), 0));

    core.outpost.set_foreground(window);
    app.send("set-description search Search the list");
    assert_eq!(
        core.told(1).spoken(),
        [vec![SegmentContent::Description(
            "Search the list".to_owned()
        )]]
    );

    drop(core);
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
    let snapshot = || verbatim_model::protocol::UiaSnapshotFact {
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
                    verbatim_listener::listener::capture_selection(element, &element.read())
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
        OutpostToCore::Event {
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
/// setting (`books/dev/src/explanation/design/uia-shapes.md`, decision 4): with a theme that
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
        .handle_command(&CoreToOutpost::Fetches(Fetches {
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
        ..
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

/// Where the listener's walk for `focus` ended.
fn route_end(focus: &common::outpost::ListenerFocus) -> RouteEnd {
    focus
        .object
        .route
        .as_ref()
        .expect("a focus read answers its route")
        .end
        .clone()
}

/// The names of the ancestors the listener read for `focus`, nearest
/// first.
fn listener_names(focus: &common::outpost::ListenerFocus) -> Vec<Option<&str>> {
    focus
        .object
        .ancestors
        .iter()
        .map(
            |ancestor| match common::outpost::value(&ancestor.values, PropertyId::Name) {
                Some(PropertyValue::Text(name)) => Some(name.as_str()),
                _ => None,
            },
        )
        .collect()
}

/// The outpost's completion of the listener's read `focus`, which must
/// answer a focus.
fn completed(outpost: &OutpostUnderTest, focus: common::outpost::ListenerFocus) -> Focused {
    let (outcome, calls) = outpost.complete_focus(focus);
    Focused::of(outcome, calls)
}

/// The nodes of a focus answer's chain, outermost first, ending with the
/// focus.
fn chain_nodes(focused: &Focused) -> impl Iterator<Item = &verbatim_model::request::FocusNode> {
    focused
        .read
        .ancestors
        .iter()
        .rev()
        .map(|ancestor| &ancestor.node)
        .chain(std::iter::once(&focused.read.focus))
}

/// The roles and names of a focus answer's chain, outermost first.
fn roles_and_names(focused: &Focused) -> Vec<(Role, Option<&str>)> {
    chain_nodes(focused)
        .map(|node| {
            (
                common::outpost::role_of(node),
                common::outpost::name_of(node),
            )
        })
        .collect()
}

/// The windows of a focus answer's chain, outermost first, as its nodes
/// carry them: each node's own window, none for a node inside a window.
fn windows_of(focused: &Focused) -> Vec<Option<WindowHandle>> {
    chain_nodes(focused)
        .map(
            |node| match common::outpost::value(&node.read.values, PropertyId::NativeWindow) {
                Some(PropertyValue::Window(window)) => Some(*window),
                _ => None,
            },
        )
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
    let reported = outpost.uia_focus_answer(&client.focused());
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
    let reported = outpost.msaa_focus_answer(common::outpost::node_ids(hwnd, 1));
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

/// An MSAA object a walk reaches from a child window's window object, in
/// its parent window, is that window's client object, as oleacc's window
/// object answers `accParent`, so it carries the window even when it names
/// no address, as a Windows Forms panel's client offers no identity string
/// (`experiments/focus-pipeline-rework-measurements/captures/two-same-named-dialogs.txt`):
/// the "Save" button in a child window of mockapp's "Save As" dialog, whose
/// client area names its address (`child-client-identity`) while the
/// dialog's does not, has both dialogs carry their windows, so two windows
/// of one role and name are told apart (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
fn an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client() {
    let title = common::unique_title("mockapp-msaa-window-client");
    let mut app = common::spawn("save_as.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    app.send("child-window");
    app.send("child-client-identity");
    // SAFETY: a local search of the dialog's children.
    let child = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(Some(hwnd), None, None, None)
    }
    .expect("mockapp made the child window");
    let outpost = OutpostUnderTest::new(&app);
    // The button, index 1, at its object id in the child window.
    let reported = outpost.msaa_focus_answer(common::outpost::node_ids(child, 1));
    assert_eq!(
        (roles_and_names(&reported), windows_of(&reported)),
        (
            vec![
                (Role::Dialog, Some("Save As")),
                (Role::Dialog, Some("Save As")),
                (Role::Button, Some("Save"))
            ],
            vec![Some(handle(hwnd)), Some(handle(child)), None]
        ),
        "the outpost's chain and its windows"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
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
    let reported = outpost.msaa_focus_answer(common::outpost::node_ids(child, 1));
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
    let (run, _) = outpost.msaa_run(&[(
        common::outpost::node_ids(child, 1),
        verbatim_model::request::CandidateKind::Focus,
    )]);
    assert_eq!(
        run.tried,
        [verbatim_model::request::Tried::Refused(
            verbatim_model::request::Refusal::NotFocused
        )]
    );
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
/// its outermost element's nearest window, included, never read, a
/// deliberate difference from NVDA's fallback
/// (`books/dev/src/reference/crates/verbatim-outpost.md`, "Console windows and frames of another
/// process"): the button "Far",
/// under 66 groups of its own in mockapp's window (`nested.json`), is read
/// by the listener up to its 64th ancestor, which names the window to
/// complete from, and reported with the window, a plain window named by its
/// title, above the 64 groups read. Started from that window's parent, as a
/// failure's chain is, the window would be lost; without the completion,
/// the outermost group read is the chain's top.
fn a_depth_limited_chain_starts_at_its_unread_nearest_window() {
    let title = common::unique_title("mockapp-depth-limit");
    let app = common::spawn("nested.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    let far = client.focused();
    let (focus, _) = outpost.listener_focus(&far);
    assert_eq!(
        (focus.object.ancestors.len(), route_end(&focus)),
        (64, RouteEnd::PlainFrom(handle(hwnd))),
        "the listener's walk, cut at its depth limit"
    );
    let reported = completed(&outpost, focus);
    let chain = roles_and_names(&reported);
    assert_eq!(
        (chain.len(), chain[0], chain[1], chain[64], chain[65]),
        (
            66,
            (Role::Pane, Some(title.as_str())),
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
    let reported = outpost.msaa_focus_answer(common::outpost::node_ids(child, 1));
    assert_eq!(
        roles_and_names(&reported),
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
/// outpost reports as a plain window, named by its title. The listener's
/// read makes three calls: the button's fresh cache, and the two steps, to
/// "Embedded" and to the host's element. Without the rule, it steps on
/// from the host's element, a fourth call, and the host's element, named
/// by its fixture, is the outermost ancestor.
fn a_hung_window_above_a_uia_focus_ends_the_listeners_walk() {
    let (app, host, island) = uia_island("mockapp-hung-uia-listener");
    let outpost = OutpostUnderTest::new(&app);
    outpost.judge_hung(host);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    let (focus, listener_calls) = outpost.listener_focus(&ok);
    assert_eq!(
        (
            listener_names(&focus),
            route_end(&focus),
            listener_calls.uia
        ),
        (
            vec![Some("Embedded")],
            RouteEnd::ContinueFrom(handle(host)),
            3
        ),
        "the listener's walk and its calls"
    );
    let reported = completed(&outpost, focus);
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
        focus
            .ancestors
            .iter()
            .chain(std::iter::once(&focus.node))
            .map(|node| (node.role, node.name.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (Role::Window, Some(app.title.as_str())),
            (Role::Group, Some("Embedded")),
            (Role::Button, Some("OK"))
        ],
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
/// with no name, in its place. The listener reads the focus before the
/// window is judged hung, since its own test of its read's window would
/// drop it after.
fn a_focus_in_a_hung_nearest_window_keeps_the_listeners_ancestors() {
    let (app, _host, island) = uia_island("mockapp-hung-nearest-listener");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    let (focus, _) = outpost.listener_focus(&ok);
    outpost.judge_hung(island);
    let reported = completed(&outpost, focus);
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

/// A UIA focus whose walk's first step fails goes on from the parent
/// window of its nearest window, by that window's own object read live, as
/// NVDA's object, which carries its nearest window, walks its parent by
/// `Window._get_parent`: the button "OK" in mockapp's UIA child window
/// (`uia_island.json`), its step to "Embedded" failing (`island
/// answer-parent`, past the listener's lookup of its nearest window), has
/// the listener name the host window, and the outpost reports the host
/// window's element, read then, above it. The child window, its nearest,
/// is not in the chain, as NVDA's line does not hold it.
fn a_failed_uia_read_continues_from_its_nearest_windows_parent() {
    let (mut app, host, island) = uia_island("mockapp-failed-read-parent");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    app.send("island answer-parent ok fail 1");
    let (focus, _) = outpost.listener_focus(&ok);
    assert_eq!(
        (focus.object.ancestors.len(), route_end(&focus)),
        (0, RouteEnd::ContinueFrom(handle(host))),
        "the listener's walk, going on from the host window"
    );
    let reported = completed(&outpost, focus);
    assert_eq!(
        (roles_and_names(&reported), windows_of(&reported)),
        (
            vec![
                (Role::Window, Some("Mockapp Island Fixture")),
                (Role::Button, Some("OK"))
            ],
            vec![Some(handle(host)), None]
        ),
        "the outpost's chain and its windows"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A chain that goes on after a failure carries one entry per window: the
/// button "OK" in mockapp's UIA child window (`uia_island.json`), whose
/// step from the child window's own element, "Embedded", to the host
/// window's fails (`island answer-parent embedded fail`), goes on from the
/// parent window of the outermost element's window, the host's, never from
/// that window itself, so the chain holds the host's element and the child
/// window's, each once. Gone on from its nearest window, the child window
/// would be in it twice, its element and a plain window for its handle.
fn a_failed_read_s_chain_carries_one_entry_per_window() {
    let (mut app, host, island) = uia_island("mockapp-one-entry-per-window");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    app.send("island answer-parent embedded fail");
    let reported = outpost.uia_focus_answer(&ok);
    assert_eq!(
        (roles_and_names(&reported), windows_of(&reported)),
        (
            vec![
                (Role::Window, Some("Mockapp Island Fixture")),
                (Role::Group, Some("Embedded")),
                (Role::Button, Some("OK"))
            ],
            vec![Some(handle(host)), Some(handle(island)), None]
        ),
        "the outpost's chain and its windows"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A UIA window whose element cannot be read, met on a focus's way up,
/// loses the focus, as NVDA's unguarded `ElementFromHandleBuildCache` ends
/// its event (`NVDAObjects/UIA/__init__.py` 1553 to 1557): the button "OK"
/// in mockapp's UIA child window (`uia_island.json`), its step to
/// "Embedded" failing, has its chain go on from the host window, whose root
/// provider fails (`fail-window-element`), so that the outpost cannot make
/// its element, and the outpost reports nothing. Read as a plain window, as
/// before, the host would have been reported above the focus.
fn a_uia_window_whose_element_cannot_be_read_loses_the_focus() {
    let (mut app, host, island) = uia_island("mockapp-unreadable-window-element");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    app.send("island answer-parent ok fail 1");
    app.send("fail-window-element");
    let request = uia
        .request::<verbatim_uia::Ancestor>(common::ancestor_gates())
        .expect("a cache request");
    assert!(
        uia.window_element(host.0 as isize, &request).is_err(),
        "the host window's element cannot be made"
    );
    let (focus, _) = outpost.listener_focus(&ok);
    assert_eq!(
        route_end(&focus),
        RouteEnd::ContinueFrom(handle(host)),
        "the listener's walk goes on from the host window"
    );
    let (outcome, _) = outpost.complete_focus(focus);
    assert_eq!(outcome, verbatim_model::request::Outcome::NotThere);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A UIA dialog the focus listener read in a focus's chain, whose element
/// the outpost does not hold, has its own text on the focus's first report:
/// the outpost finds it by the path of the report being built, not by the
/// one it committed before. The "Yes" button of mockapp's dialog window
/// (`dialog_class_window.json`), the outpost's first report, carries the
/// dialog's text.
fn a_dialog_the_listener_read_has_its_text_on_its_first_report() {
    const QUESTION: &str = "Remove the theme Mine? This cannot be undone. Its sounds go with it.";
    let title = common::unique_title("mockapp-dialog-first-report");
    let mut app = common::spawn("dialog_class_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::new(&app);
    app.send("set-focus yes");
    let reported = outpost.uia_focus_answer(&common::listener_focus(&uia, hwnd.0 as isize));
    assert_eq!(
        reported
            .read
            .ancestors
            .iter()
            .map(|ancestor| (
                common::outpost::role_of(&ancestor.node),
                ancestor.dialog_text.as_deref()
            ))
            .collect::<Vec<_>>(),
        [(Role::Dialog, Some(QUESTION))],
        "the chain's dialog text"
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// What Core does with `message` from outpost `outpost`, watching
/// application `application`, as the app applies it, and what it says.
fn core_takes(
    state: &mut SrState,
    message: OutpostToCore,
    (outpost, application): (OutpostId, Pid),
) -> (Vec<Effect>, Vec<Vec<SegmentContent>>) {
    let input = match message {
        OutpostToCore::LiveRereadGone { request } => Input::LiveRereadGone { outpost, request },
        mut message => {
            if let OutpostToCore::Event { event, .. } = &mut message {
                event.assign_outpost(outpost);
            }
            verbatim_model::protocol::event_input(application, message)
                .unwrap_or_else(|| panic!("not a message Core takes as an input"))
        }
    };
    let effects = verbatim_core::reduce(state, &mut verbatim_core::Runtime::default(), &input);
    let spoken = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(
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

/// The live re-reads `effects` ask for.
fn live_rereads(effects: &[Effect]) -> Vec<&verbatim_model::LiveReread> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LiveReread(reread) => Some(reread),
            _ => None,
        })
        .collect()
}

/// A return to an application whose window was retitled while the user was
/// in another reads the window's whole chain live and says the new title,
/// with no live re-read: the listener's reading thread for mockapp still
/// holds its last focus, but the window text saved with its ancestry cache
/// reads otherwise now, so the title check makes the walk read live to the
/// top (`books/dev/src/reference/crates/verbatim-core.md`, "The ancestry
/// cache"). The user goes from mockapp's UIA window to a second mockapp's
/// MSAA button and back, each window made the foreground as the outposts
/// read it, with no keystroke.
fn a_return_to_an_application_reads_its_whole_chain() {
    let title = common::unique_title("mockapp-return-retitled");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    let other_title = common::unique_title("mockapp-return-other");
    let mut other = common::spawn("small.json", "msaa", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let other_outpost = OutpostUnderTest::new(&other);
    let mut state = SrState::new();
    let from = (OutpostId(1), Pid(app.pid()));

    app.send("set-focus inner");
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let first = outpost.next();
    outpost.settled();
    let _ = core_takes(&mut state, first, from);

    outpost.set_foreground(other_hwnd);
    common::apply(&mut other, other_hwnd, "set-focus btn1");
    // The button is node 1, after the root, so its object id is 2.
    other_outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: other_hwnd.0 as isize,
        id_object: 2,
        id_child: 0,
    });
    let away = other_outpost.next();
    other_outpost.settled();
    let _ = core_takes(&mut state, away, (OutpostId(2), Pid(other.pid())));

    let retitled = format!("{title} retitled");
    app.send(&format!("set-title {retitled}"));
    outpost.set_foreground(hwnd);
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let back = outpost.next();
    outpost.settled();
    let OutpostToCore::Event {
        event:
            NormalizedEvent::FocusChanged {
                first_cached_ancestor,
                ..
            },
        ..
    } = &back
    else {
        panic!("the outpost said {back:?}, not the return's focus");
    };
    assert_eq!(
        *first_cached_ancestor, None,
        "the walk read live to the top"
    );
    let (effects, spoken) = core_takes(&mut state, back, from);
    assert_eq!(
        spoken,
        [
            vec![
                SegmentContent::Label(retitled.clone()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Items View".to_owned()),
                SegmentContent::Role(Role::List),
            ],
            vec![
                SegmentContent::Label("Inner".to_owned()),
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(2),
                },
            ],
        ],
        "the new title, then the list and the item"
    );
    assert_eq!(
        live_rereads(&effects),
        Vec::<&verbatim_model::LiveReread>::new()
    );
    assert_eq!(state.live_re_reads(), 0, "no live re-read");
    drop(other_outpost);
    other.quit();
    drop(outpost);
    app.quit();
}

/// A return to mockapp's UIA window whose application raises two focus
/// events for the same element is spoken once
/// (`books/dev/src/reference/crates/verbatim-core.md`, "The ancestry
/// cache"): both reads take the window entry from the listener's ancestry
/// cache. Core asks a live re-read for the first, and the second waits
/// behind it; the listener answers the first gone, since the read it holds
/// last is the second's, and with that answer Core decides the second and
/// asks its live re-read; the listener reads it live, and Core speaks the
/// window, the list and the item. The user goes
/// to a second mockapp's MSAA button and back, each window made the
/// foreground as the outposts read it, with no keystroke.
fn a_return_with_two_focus_events_of_one_element_is_spoken_once() {
    let title = common::unique_title("mockapp-return-twice");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    let other_title = common::unique_title("mockapp-return-twice-other");
    let mut other = common::spawn("small.json", "msaa", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let other_outpost = OutpostUnderTest::new(&other);
    let mut state = SrState::new();
    let from = (OutpostId(1), Pid(app.pid()));

    app.send("set-focus inner");
    outpost.deliver(outpost.listener_fact(&client.focused()));
    let first = outpost.next();
    outpost.settled();
    let _ = core_takes(&mut state, first, from);

    outpost.set_foreground(other_hwnd);
    common::apply(&mut other, other_hwnd, "set-focus btn1");
    // The button is node 1, after the root, so its object id is 2.
    other_outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: other_hwnd.0 as isize,
        id_object: 2,
        id_child: 0,
    });
    let away = other_outpost.next();
    other_outpost.settled();
    let _ = core_takes(&mut state, away, (OutpostId(2), Pid(other.pid())));

    outpost.set_foreground(hwnd);
    let [first, second] = ["first", "second"].map(|event| {
        outpost.deliver(outpost.listener_fact(&client.focused()));
        let back = outpost.next();
        outpost.settled();
        let (effects, spoken) = core_takes(&mut state, back, from);
        assert!(spoken.is_empty(), "nothing said for the {event} event");
        let asked: Vec<_> = live_rereads(&effects).into_iter().cloned().collect();
        asked
    });
    let [first] = first.as_slice() else {
        panic!("one live re-read asked for the first event: {first:?}");
    };
    assert!(second.is_empty(), "the second waits: {second:?}");
    let (first_request, first_trace) = (first.request, first.trace_id);
    let (judged, calls) = outpost.listener_live_reread_traced(first_request, first_trace);
    assert!(
        matches!(judged, Judged::Dropped(NOT_THE_LAST_READ)),
        "the first request: {judged:?}"
    );
    assert_eq!(calls.uia, 0, "no call for the first request");
    outpost.deliver(judged.into_reread_fact(98, first_request));
    let gone = outpost.next();
    match gone {
        OutpostToCore::LiveRereadGone { request } if request == first_request => {}
        ref other => panic!("the outpost said {other:?}, not the answer of gone"),
    }
    let (effects, spoken) = core_takes(&mut state, gone, from);
    assert_eq!(
        spoken,
        Vec::<Vec<SegmentContent>>::new(),
        "the first request's answer of gone says nothing"
    );
    let asked = live_rereads(&effects);
    let [second] = asked.as_slice() else {
        panic!("one live re-read asked for the second event: {effects:?}");
    };
    let (second_request, second_trace) = (second.request, second.trace_id);
    assert_eq!(state.live_re_reads(), 2, "one for each event");

    let (judged, _) = outpost.listener_live_reread_traced(second_request, second_trace);
    outpost.deliver(judged.into_reread_fact(99, second_request));
    let answer = outpost.next();
    outpost.settled();
    assert_eq!(
        core_takes(&mut state, answer, from).1,
        [
            vec![
                SegmentContent::Label("Mockapp Reuse Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Items View".to_owned()),
                SegmentContent::Role(Role::List),
            ],
            vec![
                SegmentContent::Label("Inner".to_owned()),
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(2),
                },
            ],
        ],
        "the second request's live read is spoken"
    );
    assert_eq!(state.live_re_reads(), 2, "its answer asks for no other");
    drop(other_outpost);
    other.quit();
    drop(outpost);
    app.quit();
}

/// The fact the listener's callback sends for the toast element of mockapp's
/// window, with the element, which has no window of its own.
fn toast_candidate(client: &Client) -> (Cached<ListenerEvent>, DeliveredFact) {
    let element = client.named("New notification");
    let fields = element.read();
    assert_eq!(fields.role.hwnd, 0, "the element has no window of its own");
    assert!(verbatim_uia::map::is_toast_candidate(
        fields.role.control_type,
        fields.automation_id.as_deref(),
    ));
    let (hwnd, snapshot) =
        verbatim_listener::listener::capture(&element, &fields).expect("a process");
    (element, DeliveredFact::UiaToast { hwnd, snapshot })
}

/// A toast whose element has no window of its own, as a XAML toast's has
/// none, is told by its nearest window's class on the reading thread, and
/// passed on with that window and what its read found.
fn a_toast_without_a_window_of_its_own_is_passed_on_with_its_nearest() {
    let title = common::unique_title("mockapp-toast-element");
    let app = common::spawn("toast_element.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    let (element, fact) = toast_candidate(&client);

    let Judged::Passed(passed) = outpost.listener_check(&element, fact) else {
        panic!("the toast was not passed on");
    };
    let DeliveredFact::UiaToast {
        hwnd: window,
        snapshot,
    } = *passed
    else {
        panic!("a toast, not {passed:?}");
    };
    assert_eq!(
        (
            window,
            snapshot.name.as_deref(),
            snapshot.details.position_in_set,
            snapshot.details.set_size
        ),
        (hwnd.0 as isize, Some("New notification"), Some(1), Some(1))
    );
    drop(outpost);
    app.quit();
}

/// A toast candidate whose nearest window is of another class is dropped.
fn a_toast_candidate_under_another_window_class_is_dropped() {
    let title = common::unique_title("mockapp-toast-other-class");
    let app = common::spawn("toast_element_other_class.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);
    let (element, fact) = toast_candidate(&client);

    assert!(matches!(
        outpost.listener_check(&element, fact),
        Judged::Dropped(verbatim_listener_logic::focus::TOAST_NOT_CORE_WINDOW)
    ));
    drop(outpost);
    app.quit();
}

#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    harness::run_isolated(&[
        (
            "a_toast_without_a_window_of_its_own_is_passed_on_with_its_nearest",
            a_toast_without_a_window_of_its_own_is_passed_on_with_its_nearest,
        ),
        (
            "a_toast_candidate_under_another_window_class_is_dropped",
            a_toast_candidate_under_another_window_class_is_dropped,
        ),
        (
            "a_fake_focus_on_an_msaa_object_without_the_focused_state_answers_nothing",
            a_fake_focus_on_an_msaa_object_without_the_focused_state_answers_nothing,
        ),
        (
            "a_return_to_an_application_reads_its_whole_chain",
            a_return_to_an_application_reads_its_whole_chain,
        ),
        (
            "a_return_with_two_focus_events_of_one_element_is_spoken_once",
            a_return_with_two_focus_events_of_one_element_is_spoken_once,
        ),
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
            "an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client",
            an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client,
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
            "a_depth_limited_chain_starts_at_its_unread_nearest_window",
            a_depth_limited_chain_starts_at_its_unread_nearest_window,
        ),
        (
            "a_failed_uia_read_continues_from_its_nearest_windows_parent",
            a_failed_uia_read_continues_from_its_nearest_windows_parent,
        ),
        (
            "a_failed_read_s_chain_carries_one_entry_per_window",
            a_failed_read_s_chain_carries_one_entry_per_window,
        ),
        (
            "a_uia_window_whose_element_cannot_be_read_loses_the_focus",
            a_uia_window_whose_element_cannot_be_read_loses_the_focus,
        ),
        (
            "a_dialog_the_listener_read_has_its_text_on_its_first_report",
            a_dialog_the_listener_read_has_its_text_on_its_first_report,
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
            "the_listener_skips_a_focus_in_a_class_listed_window",
            the_listener_skips_a_focus_in_a_class_listed_window,
        ),
        (
            "an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off",
            an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off,
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
            "a_focus_whose_keyboard_focus_read_times_out_is_reported",
            a_focus_whose_keyboard_focus_read_times_out_is_reported,
        ),
        (
            "a_focus_whose_keyboard_focus_read_fails_is_walked",
            a_focus_whose_keyboard_focus_read_fails_is_walked,
        ),
        (
            "a_focus_whose_earlier_elements_read_times_out_is_read_as_a_new_element",
            a_focus_whose_earlier_elements_read_times_out_is_read_as_a_new_element,
        ),
        (
            "a_menu_items_legacy_state_is_read_with_its_focus",
            a_menu_items_legacy_state_is_read_with_its_focus,
        ),
        (
            "a_repeated_msaa_focus_is_read_in_full",
            a_repeated_msaa_focus_is_read_in_full,
        ),
        (
            "a_focus_s_help_text_and_suggestions_are_followed",
            a_focus_s_help_text_and_suggestions_are_followed,
        ),
        (
            "a_followed_change_while_another_window_is_in_front_is_refused",
            a_followed_change_while_another_window_is_in_front_is_refused,
        ),
        (
            "a_reused_runtime_id_keeps_its_node_with_its_element_gone_remote",
            a_reused_runtime_id_keeps_its_node_with_its_element_gone_remote,
        ),
        (
            "a_reused_runtime_id_keeps_its_node_with_its_element_gone_classic",
            a_reused_runtime_id_keeps_its_node_with_its_element_gone_classic,
        ),
        (
            "a_text_focus_under_the_runtime_id_of_a_field_that_lost_the_focus_reads_its_own_line",
            a_text_focus_under_the_runtime_id_of_a_field_that_lost_the_focus_reads_its_own_line,
        ),
        (
            "a_text_focus_under_the_runtime_id_of_a_field_gone_reads_its_own_line",
            a_text_focus_under_the_runtime_id_of_a_field_gone_reads_its_own_line,
        ),
        (
            "a_text_focus_whose_element_is_not_found_under_a_reused_runtime_id_is_not_read",
            a_text_focus_whose_element_is_not_found_under_a_reused_runtime_id_is_not_read,
        ),
        (
            "a_terminal_under_the_runtime_id_of_one_that_lost_the_focus_is_read_afresh",
            a_terminal_under_the_runtime_id_of_one_that_lost_the_focus_is_read_afresh,
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
            "a_windowless_focus_with_a_failed_walk_is_found_from_its_nearest_window",
            a_windowless_focus_with_a_failed_walk_is_found_from_its_nearest_window,
        ),
        (
            "a_focus_without_a_path_is_followed_from_its_next_focus_event",
            a_focus_without_a_path_is_followed_from_its_next_focus_event,
        ),
        (
            "a_windowless_text_focus_whose_walk_failed_is_found_as_the_focused_element",
            a_windowless_text_focus_whose_walk_failed_is_found_as_the_focused_element,
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
            "a_route_walk_that_finds_its_element_gone_answers_gone",
            a_route_walk_that_finds_its_element_gone_answers_gone,
        ),
        (
            "a_text_change_after_the_focus_returns_to_a_document_is_heard",
            a_text_change_after_the_focus_returns_to_a_document_is_heard,
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
            "a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once",
            a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once,
        ),
        (
            "a_repeated_focus_on_the_still_focused_element_is_read_in_full",
            a_repeated_focus_on_the_still_focused_element_is_read_in_full,
        ),
        (
            "a_foreground_event_for_a_window_read_through_uia_is_dropped",
            a_foreground_event_for_a_window_read_through_uia_is_dropped,
        ),
    ]);
}
