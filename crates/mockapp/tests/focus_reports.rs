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
//! Each test either drives Core's reducer against the real outpost
//! (`common::core_driver`), the listener's check and reads made on this
//! thread, and asserts what Core says, or asks the listener's read and the
//! outpost's answers as Core asks them (`common::outpost`'s request driver)
//! and asserts what they answer; mockapp's focus moves with `set-focus`,
//! which raises no event.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex, PoisonError};

use verbatim_model::protocol::OutpostToCore;
use verbatim_model::{
    CallCounts, EarlierElement, Role, SegmentContent, State, StateSet, WindowHandle,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomation2, UIA_NamePropertyId};
use windows::core::Interface;

use common::core_driver::{CheckMoment, CoreDriver};
use common::outpost::OutpostUnderTest;
use verbatim_listener_logic::check::Call;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{FocusCaret, KnownAncestor, MsaaIds, RouteEnd};
use verbatim_model::tell::ElementToken;
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

/// The object id mockapp's MSAA tree answers counts.json's "Settings"
/// group at: its index in the fixture, after the root, plus one.
const GROUP_OBJECT: i32 = 2;

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
/// answered under the first's node, a terminal, its runtime id told as
/// reused, so that Core forgets what it kept of the first's text.
fn a_terminal_under_the_runtime_id_of_one_that_lost_the_focus_is_read_afresh() {
    let title = common::unique_title("mockapp-reuse-terminal");
    let mut app = common::spawn("reuse_terminal.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus first");
    let first = outpost.uia_focus_answer(&client.focused());
    assert_eq!(
        (first.role(), first.read.runtime_id_reused),
        (Role::Terminal, false)
    );

    app.send("share-runtime-id second first");
    app.send("set-focus second");
    let second = outpost.uia_focus_answer(&client.focused());
    assert_eq!(
        (second.role(), second.node(), second.read.runtime_id_reused),
        (Role::Terminal, first.node(), true)
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
    let reached = MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: tree,
        child: 1,
    };
    let redirect = match &reported.facts {
        Some(verbatim_model::request::CandidateFacts::Found { redirect, .. }) => *redirect,
        other => panic!("the candidate's facts are {other:?}"),
    };
    assert_eq!(
        (redirect, reported.read.msaa.map(|held| held.event)),
        (
            Some((verbatim_model::request::Redirect::FocusedChild, reached)),
            Some(reached)
        ),
        "the item the redirect reached, named by the ids its own event names"
    );

    drop(outpost);
    app.quit();
}

/// The outpost reads a candidate to the depth Core asks and judges none of
/// its facts: at `Facts` it stops after the focused state, at
/// `FullIfFocused` a focused candidate is read in full, and a menu start's
/// role is read as a fact.
fn a_candidate_is_read_to_the_depth_core_asks() {
    use verbatim_model::request::{CandidateFacts, CandidateKind, Depth};
    let title = common::unique_title("mockapp-candidate-depth");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "set-focus root");
    let outpost = OutpostUnderTest::new(&app);
    let ids = MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        child: 0,
    };
    let (facts_only, _) = outpost.msaa_run((ids, CandidateKind::Focus), Depth::Facts);
    assert!(
        matches!(
            facts_only.facts,
            CandidateFacts::Found {
                focused: Some(true),
                role: None,
                redirect: None,
                ..
            }
        ),
        "{:?}",
        facts_only.facts
    );
    assert!(facts_only.read.is_none(), "no full read at Facts");
    let (read, _) = outpost.msaa_run((ids, CandidateKind::Focus), Depth::FullIfFocused);
    assert!(read.read.is_some(), "a focused candidate is read in full");
    let (start, _) = outpost.msaa_run((ids, CandidateKind::MenuStart), Depth::Facts);
    assert!(
        matches!(
            start.facts,
            CandidateFacts::Found {
                focused: None,
                role: Some(Role::Window),
                ..
            }
        ),
        "{:?}",
        start.facts
    );
    drop(outpost);
    app.quit();
}

/// A focus that was not redirected says so, whatever address its node is
/// kept at: a focus event on mockapp's window client, whose role is
/// window, is read as the window's own object, kept under `OBJID_WINDOW`,
/// and answered with no redirect, as NVDA redirects only a focus on a
/// list's own object, or on an object's own child id 0, to the child its
/// `accFocus` names (`IAccessibleHandler/__init__.py` 734 to 759).
fn a_focus_that_was_not_redirected_reports_no_redirect() {
    let title = common::unique_title("mockapp-no-redirect");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "set-focus root");
    let outpost = OutpostUnderTest::new(&app);

    let reported = outpost.msaa_focus_answer(MsaaIds {
        window: WindowHandle(hwnd.0 as u64),
        object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        child: 0,
    });
    let redirect = match &reported.facts {
        Some(verbatim_model::request::CandidateFacts::Found { redirect, .. }) => *redirect,
        other => panic!("the candidate's facts are {other:?}"),
    };
    assert_eq!((reported.role(), redirect), (Role::Window, None));

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

/// A windowless UI Automation focus whose listener walk met an ancestor
/// that a current-focus read minted is found by the outpost, by the route
/// that read recorded for the ancestor, and followed: Core read "Deep
/// button" as the current focus, as it does at startup, and the focus then
/// moves to its sibling "Remember", whose walk stops at their group, which
/// has no window of its own. The route runs down the raw view, through
/// the unnamed pane the chain does not report. Without the recorded route
/// the completion finds no element, so nothing of the check box is
/// followed. `remote` reads the current focus's ancestors by the remote
/// program, else by the classic walk.
fn focus_below_an_ancestor_a_current_focus_read_minted(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-focus-now-route-{path}"));
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );

    app.send("set-focus deep");
    outpost.read_focus_as(client.focused().live().element());
    let (outcome, calls) = outpost.current_focus();
    let now = Focused::of(outcome, calls);
    let group = &now.read.ancestors.first().expect("the button's group").node;
    assert_eq!(
        common::outpost::name_of(group),
        Some("Inner group"),
        "the current focus's nearest ancestor"
    );
    let group_id = group.runtime_id.clone().expect("the group's runtime id");

    app.send("set-focus check");
    let check = client.focused();
    let (focus, _) = outpost.listener_focus(&check);
    assert_eq!(
        route_end(&focus),
        RouteEnd::Known(KnownAncestor::Uia(group_id)),
        "the listener's walk meets the group"
    );
    let reported = completed(&outpost, focus);
    assert_eq!(reported.name(), Some("Remember"));
    assert_followed(&mut app, &outpost, "check", &check);

    drop(outpost);
    app.quit();
}

/// Core's read of the current focus reads the foreground window after the
/// focus, and only for a focus whose walk met no ancestor Core already
/// knows, as NVDA reads `objectInForeground` only when the new focus's
/// ancestry shares nothing below the desktop with the old one's
/// (`eventHandler.py` 402 to 413): the first read, of "Deep button", enters
/// mockapp's window and carries it as its foreground, taken from the
/// focus's own chain; the second, of its sibling "Remember", meets their
/// group and reads no foreground window at all.
fn a_focus_now_reads_the_foreground_only_for_a_focus_entering_a_new_window() {
    let title = common::unique_title("mockapp-focus-now-foreground");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus deep");
    outpost.read_focus_as(client.focused().live().element());
    let (outcome, calls) = outpost.current_focus();
    let first = Focused::of(outcome, calls);
    assert_eq!(
        (
            first.read.met,
            first.read.foreground.as_ref().map(common::outpost::name_of)
        ),
        (None, Some(Some("Mockapp Ancestry Fixture"))),
        "the first read enters the window"
    );

    app.send("set-focus check");
    outpost.read_focus_as(client.focused().live().element());
    let reads = outpost.foreground_reads();
    let (outcome, calls) = outpost.current_focus();
    let second = Focused::of(outcome, calls);
    assert_eq!(
        (
            second.name(),
            second.read.met,
            second.read.foreground.is_none(),
            outpost.foreground_reads() - reads
        ),
        (Some("Remember"), Some(2), true, 0),
        "the second read meets the group and reads no foreground"
    );

    drop(outpost);
    app.quit();
}

/// The routes a focus-now read records below the ancestor Core already
/// knows that its walk met go on from that ancestor's route: Core read
/// "Apple" as the current focus, then reads "Deep button", whose walk
/// meets mockapp's window, which Core knows, and mints the groups and the
/// toolbar between, with the unnamed pane the chain does not report. A
/// listener focus on "Remember" then meets the inner group, and its
/// completion finds the check box by the inner group's route and follows
/// it. `remote` reads the current focus's ancestors by the remote
/// program, else by the classic walk.
fn routes_below_a_met_ancestor(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-focus-now-met-route-{path}"));
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );

    app.send("set-focus apple");
    outpost.read_focus_as(client.focused().live().element());
    let (outcome, calls) = outpost.current_focus();
    let _ = Focused::of(outcome, calls);
    app.send("set-focus deep");
    outpost.read_focus_as(client.focused().live().element());
    let (outcome, calls) = outpost.current_focus();
    let deep = Focused::of(outcome, calls);
    assert_eq!(
        (deep.read.met, deep.ancestor_names()),
        (
            Some(4),
            vec![Some("Outer group"), Some("Tools"), Some("Inner group")]
        ),
        "{path}: the walk meets the window, the fifth ancestor Core names, and mints the containers below it"
    );

    app.send("set-focus check");
    let check = client.focused();
    let reported = outpost.uia_focus_answer(&check);
    assert_eq!(reported.name(), Some("Remember"), "{path}");
    assert_followed(&mut app, &outpost, "check", &check);

    drop(outpost);
    app.quit();
}

fn routes_below_an_ancestor_a_focus_now_walk_met_are_found_remote() {
    routes_below_a_met_ancestor(true);
}

fn routes_below_an_ancestor_a_focus_now_walk_met_are_found_classic() {
    routes_below_a_met_ancestor(false);
}

fn a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_remote() {
    focus_below_an_ancestor_a_current_focus_read_minted(true);
}

fn a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_classic() {
    focus_below_an_ancestor_a_current_focus_read_minted(false);
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
    // connection; the field's registration and its states and caret, its
    // text pattern asked for once, which it has none of; and
    // the foreground object of the window the focus enters, one connection
    // and one UIA call, the window judged by Core's verdict, which the
    // request carries, with no probe. Without the read the field is not
    // found.
    assert_eq!(
        reported.calls,
        CallCounts {
            uia: 6,
            msaa: 0,
            window_messages: 0,
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
/// window the focus enters, one connection and one UIA call, the window
/// judged by Core's verdict, which the request carries, with no probe.
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
            window_messages: 0,
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
/// foreground object of the window the focus enters, one connection and
/// one UIA call, the window judged by Core's verdict, which the request
/// carries, with no probe.
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
            window_messages: 0,
            connections: 2,
        },
    );

    drop(outpost);
    other.quit();
    app.quit();
}

/// A focus with no window of its own whose route holds its ancestors, none
/// with a window either, has no route to its element, and though its walk
/// up was not cut, the application's focused element is read for it and
/// taken, its runtime id being the field's (Dickson's ruling of
/// 2026-11-17, `books/dev/src/reference/adr/0001-focus.md`, "The outpost's
/// own element"): the field is found, and having no text pattern, its text
/// is answered as none, `NoText`; not found, it would be `NotRead`. The
/// focused-element read and the field's states and text read, three UIA
/// calls in all, with no connection.
fn a_focus_no_route_finds_is_read_as_the_focused_element() {
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
        &FocusCaret::NoText,
        CallCounts {
            uia: 3,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        },
    );

    drop(outpost);
    app.quit();
}

/// A focus that may have no text, a button, with no route to its element,
/// is read as the application's focused element as a text field is, and
/// taken, its runtime id being the button's (Dickson's ruling of
/// 2026-11-17): its changes are followed from this focus on.
fn a_button_no_route_finds_is_read_as_the_focused_element_and_followed() {
    let title = common::unique_title("mockapp-no-route-button");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus btn1");
    let button = client.focused();
    outpost.read_focus_as(button.live().element());
    let reported = focus_without_a_path(&outpost, &button);
    assert_eq!(reported.name(), Some("Original Name"));
    assert_followed(&mut app, &outpost, "btn1", &button);

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
/// after the focus leaves the document for a button and returns, a caret
/// event the document raises is heard and told for its element.
fn a_caret_event_after_the_focus_returns_to_a_document_is_heard() {
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

    app.send("caret-event doc");
    match outpost.next() {
        OutpostToCore::Tell(tell) => assert_eq!(
            (
                tell.kind,
                match tell.object {
                    EventObject::Uia { runtime_id, .. } | EventObject::Followed { runtime_id } => {
                        Some(runtime_id)
                    }
                    _ => None,
                }
            ),
            (
                EventKind::UiaTextSelectionChanged,
                notes.read.focus.runtime_id.clone()
            )
        ),
        other => panic!("the outpost said {other:?}, not the document's caret event"),
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

/// The check after a menu ends with no focus in its run asks the
/// application's current focus 50 ms later, and Core refuses an MSAA answer
/// whose object neither has the focused state nor is inside one that has,
/// its state cleared by `set-states`, as NVDA refuses its fake focus before
/// it is queued (`docs/parity/focus-events.md`): nothing is said. The
/// outpost runs the test, asked by Core's check, before it reads the
/// object, so its calls are the focused object's lookup, `accFocus` asked
/// again of the object it named, which names itself, and the test's steps
/// up to the desktop alone. With the focused state, the same answer is spoken.
fn a_fake_focus_on_an_msaa_object_without_the_focused_state_answers_nothing() {
    let title = common::unique_title("mockapp-fake-focus-unfocused");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let client = (windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0, 0);

    common::apply(&mut app, hwnd, "set-focus first");
    common::apply(&mut app, hwnd, "set-states first focusable");
    assert_eq!(
        core.msaa_event(EventKind::MenuEnd, hwnd, client).said,
        Vec::new()
    );
    let checked = core.due();
    assert_eq!(
        (checked.said, checked.calls),
        (
            Vec::new(),
            CallCounts {
                msaa: 15,
                ..CallCounts::default()
            }
        ),
        "the current focus's focused-state test, which refuses it"
    );

    common::apply(&mut app, hwnd, "set-states first focusable focused");
    let _ = core.msaa_event(EventKind::MenuEnd, hwnd, client);
    assert_eq!(
        core.due().spoken(),
        [
            vec![
                SegmentContent::Label("Settings".to_owned()),
                SegmentContent::Role(Role::Group)
            ],
            vec![
                SegmentContent::Label("First".to_owned()),
                SegmentContent::Role(Role::Button)
            ]
        ]
    );

    drop(core);
    app.quit();
}

/// The check after a menu end tests the focused state of the current
/// focus and of every ancestor up to the desktop, as NVDA's
/// `shouldAllowIAccessibleFocusEvent` walks every parent
/// (`NVDAObjects/IAccessible/__init__.py` 874 to 892), not only those below
/// the ancestor Core already knows: Core's focus is mockapp's "Settings"
/// group, by its focus event, and a menu opens and closes over it; the
/// group keeps the focused state, and the current focus is its button
/// "First", without it, whose walk meets the group. The fake focus is
/// allowed and "First" spoken. Tested on the cut chain alone, it was
/// refused and nothing was said.
fn a_fake_focus_whose_focused_ancestor_core_knows_is_spoken() {
    let title = common::unique_title("mockapp-fake-focus-known-ancestor");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    common::apply(&mut app, hwnd, "set-focus group");
    let _ = core.msaa_event(EventKind::MsaaFocus, hwnd, (GROUP_OBJECT, 0));
    let client = (windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0, 0);
    assert_eq!(
        core.msaa_event(EventKind::MenuStart, hwnd, client).said,
        Vec::new()
    );
    common::apply(&mut app, hwnd, "set-focus first");
    common::apply(&mut app, hwnd, "set-states first focusable");
    common::apply(&mut app, hwnd, "set-states group focused");
    assert_eq!(
        core.msaa_event(EventKind::MenuEnd, hwnd, client).said,
        Vec::new()
    );
    assert_eq!(
        core.due().spoken(),
        [vec![
            SegmentContent::Label("First".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );

    drop(core);
    app.quit();
}

/// Core's read of the current focus takes the window with the keyboard
/// focus at once, before any call into an application, as NVDA's fake
/// focus takes the foreground window and its thread's focus window when
/// its timer fires (`NVDAObjects/window/__init__.py` 165 to 173): the
/// request names a window with no verdict in force, whose probe mockapp
/// holds; while the probe waits, the keyboard focus moves to a window of
/// another application, the desktop's. The answer is the focus in mockapp's
/// window, which had the keyboard focus when the read began. Taken after
/// the probe, the focus window would be the desktop, which mockapp does
/// not serve, and nothing would be answered.
fn a_focus_now_takes_the_focus_window_before_its_probe() {
    let title = common::unique_title("mockapp-focus-now-window-first");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    common::apply(&mut app, hwnd, "set-focus first");

    app.hold();
    let number = outpost.request(
        hwnd,
        verbatim_model::request::Request::CurrentFocus {
            set: common::outpost::focus_set(),
            known: Vec::new(),
            details: verbatim_model::TextDetails::NONE,
            depth: verbatim_model::request::Depth::Full,
        },
    );
    assert_eq!(app.held(), "GetObject", "the probe of the request's window");
    // SAFETY: GetDesktopWindow has no preconditions.
    outpost
        .set_focus_window(unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() });
    app.release();
    let focused = Focused::of(outpost.answer(number).outcome, CallCounts::default());
    assert_eq!(focused.name(), Some("First"));

    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The MSAA focus-now read follows `accFocus` down until it stops naming
/// another object, and turns a child id it names into the child's own
/// object with `accChild`, as NVDA's focus object is found
/// (`docs/parity/focus-events.md`, "The focus after a menu or the switcher
/// closes"): mockapp
/// answers each node's `accFocus` with only its own child on the way to
/// the focus, by child id (`acc-focus-stepwise`), so the read reaches
/// "Deep button", five levels below the client, through four objects
/// `accChild` gives. Followed one level, the focus would be the client's
/// first child, the unnamed pane.
fn an_msaa_focus_now_follows_acc_focus_down_to_the_focus() {
    let title = common::unique_title("mockapp-acc-focus-stepwise");
    let mut app = common::spawn("ancestry.json", "msaa", &title);
    app.send("set-focus deep");
    app.send("acc-focus-stepwise");
    let outpost = OutpostUnderTest::new(&app);
    let (outcome, calls) = outpost.current_focus();
    let focus = Focused::of(outcome, calls);
    assert_eq!(
        (focus.role(), focus.name()),
        (Role::Button, Some("Deep button"))
    );
    drop(outpost);
    app.quit();
}

/// The focused element a current-focus read finds, when it belongs to
/// another process than the one serving the focus window, as
/// `GetFocusedElementBuildCache` returns once the keyboard focus has moved
/// to another application (`experiments/getfocusedelement-after-the-focus-moves/`):
/// the outpost checks its cached process before anything else is done with
/// it, answers that nothing is there, and sends the other application
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
    let outpost = OutpostUnderTest::new(&app);

    other.send("set-focus btn1");
    outpost.read_focus_as(other_client.focused().live().element());
    common::reset_hits(other_hwnd);
    let (outcome, _) = outpost.current_focus();
    assert_eq!(
        outcome,
        verbatim_model::request::Outcome::NotThere,
        "no focused control is answered"
    );
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
/// `NotifyWinEvent`: Core asks that application's outpost, never this
/// one's, so mockapp's outpost neither probes nor reads that window, the
/// other mockapp answers nothing, no `WM_GETOBJECT` and no provider call,
/// and nothing is said, the other application's outpost not starting here.
fn a_focus_event_naming_another_applications_window_is_not_read() {
    let title = common::unique_title("mockapp-event-here");
    let app = common::spawn("small.json", "uia", &title);
    let other_title = common::unique_title("mockapp-event-elsewhere");
    let other = common::spawn("small.json", "uia", &other_title);
    let other_hwnd = common::find_window(&other_title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    core.outpost.set_foreground(other_hwnd);

    common::reset_hits(other_hwnd);
    let step = core.msaa_event(EventKind::MsaaFocus, other_hwnd, (-4, 0));
    assert_eq!(
        (step.said, step.calls, common::read_hits(other_hwnd)),
        (Vec::new(), CallCounts::default(), Vec::new()),
        "nothing said, and the other application's calls"
    );

    drop(core);
    other.quit();
    app.quit();
}

/// A UIA focus whose window's class is on NVDA's MSAA class list ends the
/// check thread's check at the window's class, after its walk, the one
/// call into the application, as NVDA's native-element test fails, and
/// Core drops it, asking nothing: the window's MSAA events speak for it.
fn the_listener_skips_a_focus_in_a_class_listed_window() {
    let title = common::unique_title("mockapp-class-listed");
    let mut app = common::spawn("msaa_class_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    core.observe_check(Some(Box::new(move |moment| {
        if let CheckMoment::Calling(call) = moment {
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }
    })));
    let step = core.uia_focus(&client.focused());
    core.observe_check(None);
    assert_eq!(
        (
            step.said,
            step.calls,
            calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
        ),
        (
            Vec::new(),
            CallCounts::default(),
            vec![Call::NearestWindow, Call::Classes(hwnd.0 as isize)]
        )
    );

    drop(core);
    app.quit();
}

/// A focus event repeated on the element that is the readers' last focus,
/// which still has the keyboard focus, is NVDA's duplicate
/// (`docs/parity/focus-events.md`): the check thread reads, after its walk
/// and probe, the kept element's keyboard focus and the sender's, both
/// focused, and Core ends the repeat with no read and nothing said.
fn a_repeated_focus_on_the_still_focused_element_is_a_duplicate() {
    let title = common::unique_title("mockapp-repeated-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    assert_eq!(
        core.uia_focus(&client.focused()).spoken(),
        window_and_button()
    );
    let step = core.uia_focus(&client.focused());
    assert_eq!((step.said, step.calls), (Vec::new(), CallCounts::default()));

    drop(core);
    app.quit();
}

/// Windows Terminal raises its terminal's focus twice as it launches and
/// then minimizes itself, so the second focus can be read as the focus is
/// being lost. The check thread reads, as NVDA does, the keyboard focus of
/// the element kept under the runtime id first, and the sender's after it:
/// here the focus leaves between the two reads, so the earlier element
/// reads focused and the sender not, and Core drops the repeat by the
/// sender's answer, asking nothing, the focus announced once
/// (`docs/parity/focus-events.md`).
fn a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once() {
    let title = common::unique_title("mockapp-focus-leaves-between");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    assert_eq!(
        core.uia_focus(&client.focused()).spoken(),
        window_and_button()
    );
    let repeated = client.focused();
    let app = Arc::new(Mutex::new(app));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (moves, seen) = (Arc::clone(&app), Arc::clone(&calls));
    core.observe_check(Some(Box::new(move |moment| {
        let CheckMoment::Calling(call) = moment else {
            return;
        };
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
        if call == Call::KeyboardFocus {
            moves
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send("set-focus slider1");
        }
    })));
    let step = core.uia_focus(&repeated);
    // After the walk and the window's verdict, whose probe depends on how
    // long ago the first focus's probe was made.
    let focus_reads: Vec<Call> = calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .copied()
        .filter(|call| {
            !matches!(
                call,
                Call::NearestWindow | Call::Classes(_) | Call::Probe(_)
            )
        })
        .collect();
    assert_eq!(
        focus_reads,
        [Call::KeptFocus(ElementToken(1)), Call::KeyboardFocus],
        "the earlier element's keyboard focus first, then the sender's"
    );
    assert_eq!((step.said, step.calls), (Vec::new(), CallCounts::default()));
    core.observe_check(None);

    drop(core);
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

/// Core told of a UI Automation focus on `element`, its check made with
/// the process's transaction timeout at [`HELD_TIMEOUT_MS`] and mockapp
/// holding its next provider call from the moment the check's call
/// `holding` begins, to the check's end, when the usual timeout is
/// restored and the call released, so Core's reads after it are answered
/// as usual: mockapp, what Core did, and the provider method mockapp held.
fn checked_held(
    app: common::MockApp,
    core: &mut CoreDriver,
    element: &Cached<ListenerEvent>,
    holding: impl Fn(Call) -> bool + 'static,
) -> (common::MockApp, common::core_driver::Step, String) {
    let usual = transaction_timeout();
    let app = Arc::new(Mutex::new(app));
    let held = Arc::new(Mutex::new(None));
    let (holder, method) = (Arc::clone(&app), Arc::clone(&held));
    core.observe_check(Some(Box::new(move |moment| {
        let mut app = holder.lock().unwrap_or_else(PoisonError::into_inner);
        match moment {
            CheckMoment::Calling(call) if holding(call) => {
                let mut method = method.lock().unwrap_or_else(PoisonError::into_inner);
                if method.is_none() {
                    set_transaction_timeout(HELD_TIMEOUT_MS);
                    app.hold();
                    *method = Some(String::new());
                }
            }
            CheckMoment::Calling(_) => {}
            CheckMoment::Checked => {
                let mut method = method.lock().unwrap_or_else(PoisonError::into_inner);
                if method.is_some() {
                    set_transaction_timeout(usual);
                    *method = Some(app.held());
                    app.release();
                }
            }
        }
    })));
    let step = core.uia_focus(element);
    // The observer, which shares mockapp, is dropped, so mockapp is the
    // test's alone again.
    core.observe_check(None);
    let app = Arc::try_unwrap(app)
        .unwrap_or_else(|_| panic!("the observer let mockapp go"))
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    let held = held
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("the check made the call held");
    (app, step, held)
}

/// A UIA focus whose live keyboard focus read fails by UIA's transaction
/// timeout is let through, as NVDA lets any COM error of that read through
/// (`shouldAllowUIAFocusEvent` answers true, `docs/parity/focus-events.md`),
/// though its event's cache says it lacked the keyboard focus: the check
/// thread's read of it is held past the timeout, and Core reads the focus
/// and speaks it. The sender is mockapp's window element, whose window's
/// probe an earlier focus made, so that the keyboard focus read is the
/// check's first provider call after its walk.
fn a_focus_whose_keyboard_focus_read_times_out_is_reported() {
    let title = common::unique_title("mockapp-focus-read-timed-out");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus slider1");
    let _ = core.uia_focus(&client.focused());
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
    let (app, step, held) =
        checked_held(app, &mut core, &window, |call| call == Call::KeyboardFocus);
    assert_eq!(held, "GetPropertyValue", "the keyboard focus read was held");
    assert_eq!(
        step.spoken(),
        [vec![
            SegmentContent::Label("Mockapp Events Fixture".to_owned()),
            SegmentContent::Role(Role::Window)
        ]]
    );

    drop(core);
    app.quit();
}

/// A UIA focus whose element is gone once its nearest window has been
/// looked up, so that the check's live keyboard focus read fails, is let
/// through, as NVDA's COM error rule lets a gone element through
/// (`docs/parity/focus-events.md`): the listener's read of it then fails
/// too, and it is answered from its event's cache, its chain going on from
/// the parent window of its nearest window, mockapp's, as NVDA's object
/// walks its parent by `Window._get_parent`, so Core speaks the button
/// alone, mockapp's window, which the startup read put in front, being in
/// no chain. mockapp's button is disconnected as the check's keyboard
/// focus read begins.
fn a_focus_whose_keyboard_focus_read_fails_is_walked() {
    let title = common::unique_title("mockapp-focus-read-gone");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let button = client.focused();
    let app = Arc::new(Mutex::new(app));
    let gone = Arc::clone(&app);
    core.observe_check(Some(Box::new(move |moment| {
        if moment == CheckMoment::Calling(Call::KeyboardFocus) {
            gone.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send("disconnect btn1");
        }
    })));
    assert_eq!(
        core.uia_focus(&button).spoken(),
        [vec![
            SegmentContent::Label("Original Name".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );
    core.observe_check(None);

    drop(core);
    Arc::try_unwrap(app)
        .unwrap_or_else(|_| panic!("the observer let mockapp go"))
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .quit();
}

/// A UIA focus under the runtime id of the focus read before, whose
/// earlier element's keyboard focus read fails by UIA's transaction
/// timeout, is no repeat: a failed read means not a duplicate, as NVDA
/// catches its duplicate check's error and handles the event
/// (`docs/parity/focus-events.md`), and the sender's keyboard focus read,
/// into the same held window, times out too and lets it through. Core reads
/// it and speaks it again. mockapp holds the check's read of the earlier
/// element, its first provider call for it being `FragmentRoot`, to the
/// check's end.
fn a_focus_whose_earlier_elements_read_times_out_is_read_as_a_new_element() {
    let title = common::unique_title("mockapp-earlier-read-timed-out");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus btn1");
    let button = client.focused();
    assert_eq!(core.uia_focus(&button).spoken(), window_and_button());
    let (app, step, held) = checked_held(app, &mut core, &button, |call| {
        matches!(call, Call::KeptFocus(_))
    });
    assert_eq!(
        held, "FragmentRoot",
        "the earlier element's keyboard focus read was held"
    );
    assert_eq!(
        step.spoken(),
        [vec![
            SegmentContent::Label("Original Name".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );

    drop(core);
    app.quit();
}

/// A focus on a menu item that no pattern makes checkable reads its legacy
/// MSAA state, where a Windows Forms menu item says it is checked, with
/// one live read after the focus's classic read, made for an element the
/// read maps to a menu item: the listener's read makes four calls, its
/// three (the item's cache, and steps up to the menu and to the window the
/// focus before had), and the state.
fn a_menu_items_legacy_state_is_read_with_its_focus() {
    let title = common::unique_title("mockapp-menu-item-focus");
    let mut app = common::spawn("modal_owner.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus close");
    let _ = outpost.uia_focus_answer(&client.focused());
    app.send("set-focus copy");
    let (focus, calls) = outpost.listener_focus(&client.focused());
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (4, 0, 0),
        "the read's three and the legacy state"
    );
    let reported = completed(&outpost, focus);
    assert_eq!(
        (reported.name(), reported.role()),
        (Some("Copy"), Role::MenuItem)
    );

    drop(outpost);
    app.quit();
}

/// A state change on a followed menu item that no pattern makes checkable
/// carries its legacy MSAA checked state, as NVDA's menu item reads its
/// states with `LegacyIAccessibleState` whatever raised the change
/// (`NVDAObjects/UIA/__init__.py`, `MenuItem`): mockapp's "Copy", focused
/// and followed, is checked, which its UIA states say nothing of and its
/// legacy state does, and raises a state change.
fn a_menu_items_state_change_carries_its_legacy_checked_state() {
    let title = common::unique_title("mockapp-menu-item-legacy-change");
    let mut app = common::spawn("modal_owner.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus copy");
    let copy = client.focused();
    let _ = outpost.uia_focus_answer(&copy);
    app.send("set-states copy focusable checked");
    match outpost.next() {
        OutpostToCore::Tell(tell) => assert_eq!(
            (tell.object, tell.payload),
            (
                EventObject::Followed {
                    runtime_id: RuntimeId(copy.runtime_id())
                },
                Payload::PropertyChanged {
                    property: PropertyId::States,
                    value: Some(PropertyValue::States(StateSet::from_iter([
                        State::Focusable,
                        State::Checkable,
                        State::Checked
                    ]))),
                }
            )
        ),
        other => panic!("the outpost said {other:?}, not the item's states"),
    }

    drop(outpost);
    app.quit();
}

/// The console host's window is the parent of its text area, reports the
/// keyboard focus whenever the text area has it, and raises focus events
/// around the text area's; those are never the focus (NVDA refuses them
/// whatever the element reports, `docs/parity/focus-events.md`): Core
/// refuses a UIA focus on the element of automation id "Console Window"
/// whose window's class is `ConsoleWindowClass` at its tell, even while
/// it reports the keyboard focus, asking nothing and saying nothing.
fn a_console_windows_own_focus_is_never_reported() {
    let title = common::unique_title("mockapp-console-window");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus root");
    let listener = client
        .uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let window = client
        .uia
        .window_element(hwnd.0 as isize, &listener)
        .expect("mockapp's root element");
    let step = core.uia_focus(&window);
    assert_eq!((step.said, step.calls), (Vec::new(), CallCounts::default()));

    drop(core);
    app.quit();
}

/// A console's text area's focus is read with no check of the console's
/// provider, which is its outpost's: the listener's read makes three
/// calls, the text area's cache and the steps up to the console window and
/// to the desktop's root, and no window message. The outpost, which
/// judges the console by its window's class, completes it as the
/// console's terminal, nameless, with the line at its caret, which the
/// focus says as NVDA's announcement of a focused terminal does.
fn a_console_text_areas_focus_is_checked_from_the_text_area() {
    let title = common::unique_title("mockapp-console-text-area");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus text");
    let (focus, calls) = outpost.listener_focus(&client.focused());
    assert_eq!(
        (calls.uia, calls.msaa, calls.window_messages),
        (3, 0, 0),
        "the read's three"
    );
    let reported = completed(&outpost, focus);
    assert_eq!(
        (
            reported.role(),
            reported.name(),
            reported.ancestor_names(),
            caret_line(&reported)
        ),
        (
            Role::Terminal,
            None,
            vec![Some("Mockapp Console Window Fixture")],
            Some("ready>")
        )
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A UIA focus in a window with no UIA provider, on no class list, through
/// UIA's MSAA proxy: the check thread's probe of its window finds no
/// provider, which ends the check, as NVDA's native-element test fails,
/// and Core drops the focus by the probe's verdict, asking nothing, the
/// window's MSAA focus speaking for it. The application sees the probe's
/// one `WM_GETOBJECT` and nothing of its `IAccessible` is asked.
fn a_focus_in_a_window_with_no_uia_provider_is_dropped_by_its_probe() {
    let title = common::unique_title("mockapp-msaa-window-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

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
    let step = core.uia_focus(&window);
    assert_eq!(
        (step.said, step.calls, common::read_hits(hwnd)),
        (Vec::new(), CallCounts::default(), vec![("WM_GETOBJECT", 1)])
    );

    drop(core);
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

/// A selection, or a notification, from a windowless element is told with
/// its nearest window, which the check thread's walk finds, as NVDA walks up
/// to it (`getNearestWindowHandle`), for Core's backend and acceptance
/// tests.
fn a_windowless_selection_is_given_its_nearest_window() {
    let title = common::unique_title("mockapp-nearest-window");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus item1");
    let item = client.focused();
    assert_eq!(
        item.read().role.hwnd,
        0,
        "the item has no window of its own"
    );
    for kind in [EventKind::UiaElementSelected, EventKind::UiaNotification] {
        let tell = core.uia_tell(kind, &item, None, Payload::None);
        let verbatim_model::tell::TellWindow::Found(facts) = &tell.window else {
            panic!("{kind:?} told with no window: {:?}", tell.window);
        };
        assert_eq!(facts.window.handle, handle(hwnd), "{kind:?}");
    }

    drop(core);
    app.quit();
}

/// An unnamed grouping with a description is focus context whatever the
/// theme, as NVDA's layout judgment reads the description whatever its
/// setting (`books/dev/src/explanation/design/uia-shapes.md`, decision 4): with a theme that
/// reports descriptions as off, Core's walked read keeps the description
/// for the layout judgment and its completion reports no description, so
/// the outpost's completion of the focus's ancestors reports the grouping,
/// without the description.
fn an_unnamed_grouping_with_a_description_is_focus_context_with_descriptions_off() {
    let title = common::unique_title("mockapp-described-group");
    let mut app = common::spawn("described_group.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(&app);
    outpost.read_with(
        common::outpost::focus_set()
            .into_iter()
            .filter(|id| *id != PropertyId::Description)
            .collect(),
    );

    app.send("set-focus louder");
    let focus = outpost.uia_focus_answer(&client.focused());
    assert_eq!(focus.name(), Some("Louder"));
    let reported: Vec<(Role, Option<&str>, Option<&PropertyValue>)> = focus
        .read
        .ancestors
        .iter()
        .rev()
        .map(|ancestor| {
            (
                common::outpost::role_of(&ancestor.node),
                common::outpost::name_of(&ancestor.node),
                common::outpost::value(&ancestor.node.read.values, PropertyId::Description),
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
    let reported = outpost.uia_focus_answer(&client.focused());
    let FocusCaret::Read(read) = &reported.read.caret else {
        panic!(
            "the focus came without its caret: {:?}",
            reported.read.caret
        );
    };
    assert_eq!(
        (
            reported.states().contains(State::Protected),
            read.caret.line.text.as_str(),
            read.caret.selection.is_some(),
            read.selected_text.as_deref(),
        ),
        (true, "alpha beta\n", true, Some("alpha"))
    );
    outpost.settled();

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
    let (run, _) = outpost.msaa_run(
        (
            common::outpost::node_ids(child, 1),
            verbatim_model::request::CandidateKind::Focus,
        ),
        verbatim_model::request::Depth::FullIfFocused,
    );
    assert!(
        matches!(
            run,
            verbatim_model::request::CandidateAnswer {
                facts: verbatim_model::request::CandidateFacts::Found {
                    focused: Some(false),
                    ..
                },
                read: None,
            }
        ),
        "found not focused, and not read further: {run:?}"
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
/// current-focus read, which the outpost makes itself, with a remote operation
/// when `remote`: a remote program cannot judge a window between its steps,
/// so with the host window above the focus judged hung the outpost reads
/// the ancestors by the classic walk, which stops at its step into the
/// host's element, and reports the host window as a plain window; without
/// remote operations its own walk stops there too, and the host window is
/// not probed for its verdict. The outpost makes, after the focused
/// element's read, two steps, and with `remote` the focus's keyboard focus
/// read first, which the classic walk of a current-focus read makes, and one
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
    let outpost = OutpostUnderTest::with_options(
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
    let (outcome, calls) = outpost.current_focus();
    let focus = Focused::of(outcome, calls);
    assert_eq!(
        roles_and_names(&focus),
        vec![
            (Role::Window, Some(app.title.as_str())),
            (Role::Group, Some("Embedded")),
            (Role::Button, Some("OK"))
        ],
        "{path}: the current focus's chain"
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

/// The nodes a focus-now read mints where its chain goes on from a window
/// keep their routes, as those of its first walk do: the button "OK" in
/// mockapp's UIA child window (`uia_island_beside.json`), whose first step
/// fails, so that the chain goes on from the parent window of its nearest
/// window, the host window, its element read then. The focus
/// then moves to "Apply", beside the child window in the host window,
/// whose listener walk meets the host window's element, which Core knows,
/// and its completion finds the button by the host element's route and
/// follows it. Without that route the completion finds no element and
/// nothing is followed. `remote` reads the current focus's ancestors by
/// the remote program, else by the classic walk.
fn routes_where_a_focus_now_chain_goes_on_from_a_window(remote: bool) {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-focus-now-onward-route-{path}"));
    let mut app = common::spawn("uia_island_beside.json", "uia", &title);
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
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    let uia = Uia::new().expect("a UIA client");
    outpost.set_focus_window(island);
    outpost.read_focus_as(
        common::listener_focus(&uia, island.0 as isize)
            .live()
            .element(),
    );
    app.send("island answer-parent ok fail");
    let (outcome, calls) = outpost.current_focus();
    let now = Focused::of(outcome, calls);
    assert_eq!(
        roles_and_names(&now),
        vec![
            (Role::Window, Some("Mockapp Island Beside Fixture")),
            (Role::Button, Some("OK"))
        ],
        "{path}: the chain goes on from the host window"
    );
    app.send("island answer-parent ok usual");

    outpost.set_focus_window(host);
    app.send("set-focus apply");
    let client = Client::new(host);
    let apply = client.focused();
    let (focus, _) = outpost.listener_focus(&apply);
    assert_eq!(
        route_end(&focus),
        RouteEnd::Known(KnownAncestor::Uia(RuntimeId(vec![
            42,
            u32::try_from(host.0 as usize & 0xFFFF_FFFF)
                .expect("the handle's low half")
                .cast_signed(),
        ]))),
        "{path}: the listener's walk meets the host window's element"
    );
    let reported = completed(&outpost, focus);
    assert_eq!(reported.name(), Some("Apply"), "{path}");
    assert_followed(&mut app, &outpost, "apply", &apply);

    drop(outpost);
    app.quit();
}

fn routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_remote() {
    routes_where_a_focus_now_chain_goes_on_from_a_window(true);
}

fn routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_classic() {
    routes_where_a_focus_now_chain_goes_on_from_a_window(false);
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

/// The toast element of mockapp's window, which has no window of its own,
/// as a XAML toast's has none, and is a toast candidate.
fn toast_element(client: &Client) -> Cached<ListenerEvent> {
    let element = client.named("New notification");
    let fields = element.read();
    assert_eq!(fields.role.hwnd, 0, "the element has no window of its own");
    assert!(verbatim_uia::map::is_toast_candidate(
        fields.role.control_type,
        fields.automation_id.as_deref(),
    ));
    element
}

/// A toast whose element has no window of its own is told with its
/// nearest window, a Windows Runtime core window, which the check thread's
/// walk finds; Core reads it from the listener and speaks it as NVDA's
/// toast speaks it (`docs/parity/focus-events.md`).
fn a_toast_without_a_window_of_its_own_is_passed_on_with_its_nearest() {
    let title = common::unique_title("mockapp-toast-element");
    let app = common::spawn("toast_element.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    let element = toast_element(&client);
    let step = core.uia_event(EventKind::UiaWindowOpened, &element, None, Payload::None);
    assert_eq!(
        step.spoken(),
        [vec![
            SegmentContent::Label("New notification".to_owned()),
            SegmentContent::Role(Role::Window),
            SegmentContent::Position {
                position: 1,
                set_size: Some(1)
            }
        ]]
    );

    drop(core);
    app.quit();
}

/// A toast candidate whose nearest window is of another class is dropped
/// by Core, which asks nothing for it.
fn a_toast_candidate_under_another_window_class_is_dropped() {
    let title = common::unique_title("mockapp-toast-other-class");
    let app = common::spawn("toast_element_other_class.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    let element = toast_element(&client);
    let step = core.uia_event(EventKind::UiaWindowOpened, &element, None, Payload::None);
    assert_eq!((step.said, step.calls), (Vec::new(), CallCounts::default()));

    drop(core);
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
            "a_protected_fields_focus_comes_with_its_caret_and_selected_text",
            a_protected_fields_focus_comes_with_its_caret_and_selected_text,
        ),
        (
            "an_msaa_focus_now_follows_acc_focus_down_to_the_focus",
            an_msaa_focus_now_follows_acc_focus_down_to_the_focus,
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
            "a_candidate_is_read_to_the_depth_core_asks",
            a_candidate_is_read_to_the_depth_core_asks,
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
            "a_focus_that_was_not_redirected_reports_no_redirect",
            a_focus_that_was_not_redirected_reports_no_redirect,
        ),
        (
            "a_fake_focus_whose_focused_ancestor_core_knows_is_spoken",
            a_fake_focus_whose_focused_ancestor_core_knows_is_spoken,
        ),
        (
            "a_focus_now_takes_the_focus_window_before_its_probe",
            a_focus_now_takes_the_focus_window_before_its_probe,
        ),
        (
            "a_focus_now_reads_the_foreground_only_for_a_focus_entering_a_new_window",
            a_focus_now_reads_the_foreground_only_for_a_focus_entering_a_new_window,
        ),
        (
            "routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_remote",
            routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_remote,
        ),
        (
            "routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_classic",
            routes_where_a_focus_now_chain_goes_on_from_a_window_are_kept_classic,
        ),
        (
            "routes_below_an_ancestor_a_focus_now_walk_met_are_found_remote",
            routes_below_an_ancestor_a_focus_now_walk_met_are_found_remote,
        ),
        (
            "routes_below_an_ancestor_a_focus_now_walk_met_are_found_classic",
            routes_below_an_ancestor_a_focus_now_walk_met_are_found_classic,
        ),
        (
            "a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_remote",
            a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_remote,
        ),
        (
            "a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_classic",
            a_focus_below_an_ancestor_a_current_focus_read_minted_is_found_and_followed_classic,
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
            "a_focus_no_route_finds_is_read_as_the_focused_element",
            a_focus_no_route_finds_is_read_as_the_focused_element,
        ),
        (
            "a_button_no_route_finds_is_read_as_the_focused_element_and_followed",
            a_button_no_route_finds_is_read_as_the_focused_element_and_followed,
        ),
        (
            "a_route_walk_that_finds_its_element_gone_answers_gone",
            a_route_walk_that_finds_its_element_gone_answers_gone,
        ),
        (
            "a_caret_event_after_the_focus_returns_to_a_document_is_heard",
            a_caret_event_after_the_focus_returns_to_a_document_is_heard,
        ),
        (
            "a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is",
            a_focus_dropped_before_its_window_is_in_front_is_reported_once_it_is,
        ),
        (
            "a_menu_items_state_change_carries_its_legacy_checked_state",
            a_menu_items_state_change_carries_its_legacy_checked_state,
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
            "a_focus_in_a_window_with_no_uia_provider_is_dropped_by_its_probe",
            a_focus_in_a_window_with_no_uia_provider_is_dropped_by_its_probe,
        ),
        (
            "a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once",
            a_focus_that_leaves_between_the_two_keyboard_focus_reads_is_announced_once,
        ),
        (
            "a_repeated_focus_on_the_still_focused_element_is_a_duplicate",
            a_repeated_focus_on_the_still_focused_element_is_a_duplicate,
        ),
        (
            "a_foreground_event_for_a_window_read_through_uia_is_dropped",
            a_foreground_event_for_a_window_read_through_uia_is_dropped,
        ),
    ]);
}
