//! How a real outpost reports a UIA focus from mockapp's provider: under
//! which node, when the provider reuses a dead element's runtime id, and
//! with which states, when they changed after the focus event, how soon,
//! when the application is slow to answer the reads queued before it, when
//! the focused element read answers a stand-in for a windowed focus, and
//! when a windowless focus's element no longer has the keyboard focus.
//!
//! The outpost runs in this process and reads the focused element from the
//! test (`common::outpost`), as `call_counts.rs` describes; mockapp's focus
//! moves with `set-focus`, which raises no event.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::time::Duration;

use verbatim_model::{NormalizedEvent, Role, State, StateSet};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
};
use verbatim_uia::{ElementExt as _, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId, UIA_NamePropertyId,
};
use windows::core::BSTR;

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

    /// The element named `name`, built with the listener's cache request.
    fn named(&self, name: &str) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("a cache request");
        let root = self
            .uia
            .element_from_handle(self.hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
            .expect("a condition");
        root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
            .expect("the search")
            .unwrap_or_else(|| panic!("no element is named {name:?}"))
    }
}

/// File Explorer, going back from a subfolder, gave the parent folder's
/// item the runtime id of the subfolder's item it had just destroyed. The
/// outpost reports the new focus under a new node, since the element it
/// held under that id is gone; a focus repeated on the new element keeps
/// its node, since that element still has the focus. Both ways the outpost
/// reads a focus: with remote operations, where the held element's focus
/// is read in the same round trip, and without.
///
/// The calls each focus makes are pinned. Remotely, the held element's focus
/// is read in the focus's one round trip, so a repeated focus costs the two
/// calls any focus does (the focused element and the program); a held
/// element that is gone fails the whole run before it starts, which is run
/// again without it, one call more. Classically, the held element's focus
/// is one call of its own.
fn reused_runtime_id(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let (gone_cost, kept_cost) = if remote { (3, 2) } else { (5, 4) };
    let title = common::unique_title(&format!("mockapp-reuse-{path}"));
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::with_options(
        app.pid(),
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

    let reported = outpost.uia_focus(&inner);
    assert_eq!(
        reported.node.id, new_node,
        "{path}: the element that has the focus keeps its node"
    );
    assert_eq!(
        reported.calls.uia, kept_cost,
        "{path}: the calls the repeated focus made"
    );

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
/// event, and said "Inner 1 of 4"; the outpost reports the states it reads
/// with the focused element, and its name and role from the event.
fn a_focus_reports_the_states_read_when_it_is_handled() {
    let title = common::unique_title("mockapp-live-states");
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());

    app.send("set-focus inner");
    let event = client.focused();
    app.send("select inner");
    let read = client.focused();
    let reported = outpost.uia_focus_read_later(&event, &read);
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
fn a_controls_own_focus_reports_its_focused_child() {
    let title = common::unique_title("mockapp-focus-child");
    let mut app = common::spawn("focus_child.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    // The tree is node 1, after the root, so its object id is 2; General
    // is its first child.
    let tree = 2;

    common::apply(&mut app, hwnd, "focus-child categories general");
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

/// How long each of mockapp's provider calls waits in the busy test: an
/// application building a window, as File Explorer was when the first focus
/// in a new window waited 2.6 seconds behind reads queued before it.
const SLOW_CALL: Duration = Duration::from_millis(20);

/// How long mockapp's window thread is stalled while the test queues
/// everything, so the outpost handles none of it before all of it is queued.
const STALL: Duration = Duration::from_secs(1);

/// The timing of the one message in `said` that `pick` picks.
fn timing_of(
    said: &[OutpostToSupervisor],
    pick: impl Fn(&OutpostToSupervisor) -> bool,
) -> EventTiming {
    match said.iter().find(|message| pick(message)) {
        Some(
            OutpostToSupervisor::Event { timing, .. } | OutpostToSupervisor::Reply { timing, .. },
        ) => *timing,
        other => panic!("no such message: {other:?}"),
    }
}

/// A focus is never kept waiting behind reads of other objects queued
/// before it. Ten selections in a list are queued, each a read mockapp
/// answers slowly, and then a focus on another item; the outpost's worker
/// is busy with a query for an item's ancestors when they arrive (mockapp
/// is stalled under it), so all of them wait together. The focus is
/// handled first, after the query, and the selections after it, in their
/// own order; between the query's answer and the focus, the worker reads
/// nothing, so the focus waits less than one of the slow application's
/// calls takes.
fn a_focus_is_handled_before_slow_reads_queued_ahead_of_it() {
    let title = common::unique_title("mockapp-busy");
    let mut app = common::spawn("busy.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let mut outpost = OutpostUnderTest::new(app.pid());

    app.send("set-focus item1");
    let first = outpost.uia_focus(&client.focused()).node;
    let selections: Vec<DeliveredFact> = (2..=11)
        .map(|n| {
            let element = client.named(&format!("File {n}"));
            match uia_focus_fact(&element).expect("mockapp's element has its process") {
                ListenerFact {
                    fact: DeliveredFact::UiaFocus { hwnd, snapshot, .. },
                    ..
                } => DeliveredFact::UiaSelection { hwnd, snapshot },
                other => panic!("the listener's fact for a list item was {other:?}"),
            }
        })
        .collect();
    app.send("set-focus item12");
    let focused = client.focused();
    let ListenerFact { fact: focus, .. } =
        uia_focus_fact(&focused).expect("mockapp's element has its process");
    outpost.read_focus_as(&focused);

    app.send(&format!("slow {}", SLOW_CALL.as_millis()));
    app.stall(STALL);
    let request = outpost.ask(Query::Ancestors { node_id: first.id });
    for selection in selections {
        outpost.deliver(selection);
    }
    outpost.deliver(focus);
    app.stall_ended(STALL);

    let said: Vec<OutpostToSupervisor> = (0..12).map(|_| outpost.next()).collect();
    app.send("slow 0");
    outpost.settled();
    let answered = timing_of(&said, |message| {
        matches!(message, OutpostToSupervisor::Reply { .. })
    });
    let focus = timing_of(&said, |message| {
        matches!(
            message,
            OutpostToSupervisor::Event {
                event: NormalizedEvent::FocusChanged { .. },
                ..
            }
        )
    });
    let waited = Duration::from_micros(
        focus
            .dequeued_at_us
            .saturating_sub(answered.published_at_us),
    );
    eprintln!("the focus waited {waited:?} after the query ahead of it was answered");

    let heard: Vec<String> = said
        .iter()
        .map(|message| match message {
            OutpostToSupervisor::Reply {
                request_id,
                outcome: QueryOutcome::Done(QueryResult::Ancestors(_)),
                ..
            } => format!("ancestors {request_id}"),
            OutpostToSupervisor::Event {
                event: NormalizedEvent::FocusChanged { node, .. },
                ..
            } => format!("focus {}", node.name.as_deref().unwrap_or_default()),
            OutpostToSupervisor::Event {
                event: NormalizedEvent::SelectionChanged { node },
                ..
            } => format!("selection {}", node.name.as_deref().unwrap_or_default()),
            other => panic!("the outpost said {other:?}"),
        })
        .collect();
    let mut expected = vec![format!("ancestors {request}"), "focus File 12".to_owned()];
    expected.extend((2..=11).map(|n| format!("selection File {n}")));
    assert_eq!(heard, expected);
    assert!(
        waited < SLOW_CALL,
        "the focus waited {waited:?} after the query, for nothing else"
    );

    drop(outpost);
    app.quit();
}

/// An application can raise a UIA focus event on an element that is a
/// window of its own, as Windows 11 Notepad's text area is, while the
/// focused element read answers another of its elements: the outpost held
/// such a focus back and read again. NVDA accepts a UIA focus whose own
/// element has the keyboard focus, read live; so the outpost reads the
/// event's own window's element and reports the focus at once, though the
/// focused element read answers another element of the application.
fn a_windowed_focus_is_reported_though_the_focused_element_read_answers_a_stand_in() {
    let title = common::unique_title("mockapp-stand-in");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(app.pid());

    let stand_in = client.named("Original Name");
    app.send("set-focus root");
    let cache = client.uia.base_cache_request().expect("a cache request");
    let window = client
        .uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let reported = outpost.uia_focus_read_later(&window, &stand_in);
    assert_eq!(
        (reported.node.role, reported.node.name),
        (Role::Window, window.cached_string(UIA_NamePropertyId))
    );

    drop(outpost);
    app.quit();
}

/// A UIA focus event whose element has no window of its own and no longer
/// has the keyboard focus when the outpost reads it is dropped, as NVDA
/// ignores a focus event whose element no longer has the keyboard focus
/// (`shouldAllowUIAFocusEvent`). Nothing reads it again: the application's
/// next focus event is the evidence of where the focus is, and reports it.
/// The event repeats the focus the outpost reported last, the one case in
/// which the outpost held such a focus back and read the focused element
/// up to three times more.
fn a_windowless_focus_that_lost_the_keyboard_focus_waits_for_the_next_focus_event() {
    let title = common::unique_title("mockapp-lost-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());

    app.send("set-focus btn1");
    let button = client.focused();
    let reported = outpost.uia_focus(&button);
    assert_eq!(reported.node.name.as_deref(), Some("Original Name"));

    app.send("set-focus slider1");
    let slider = client.focused();
    outpost.read_focus_as(&slider);
    let reads = outpost.focus_reads();
    let ListenerFact { fact, .. } =
        uia_focus_fact(&button).expect("mockapp's element has its process");
    outpost.deliver(fact);
    outpost.settled();
    assert_eq!(
        outpost.focus_reads() - reads,
        1,
        "the focused element was read once, for the event"
    );

    let reported = outpost.uia_focus(&slider);
    assert_eq!(reported.node.name.as_deref(), Some("Level"));

    drop(outpost);
    app.quit();
}

/// The console host's window is the parent of its text area, reports the
/// keyboard focus whenever the text area has it, and raises focus events
/// around the text area's; those are never the focus (NVDA refuses them,
/// `consoleUIAWindow` in `NVDAObjects/UIA/winConsoleUIA.py`). So a UIA
/// focus on a console window's own element, while the focused element read
/// answers its text area, is not reported, though the window's element has
/// the keyboard focus: the text area's own focus is the one reported.
fn a_console_windows_own_focus_is_not_reported_when_its_text_area_is_focused() {
    let title = common::unique_title("mockapp-console-window");
    let mut app = common::spawn("console_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let client = Client::new(hwnd);
    let outpost = OutpostUnderTest::new(app.pid());

    let text_area = client.named("Text Area");
    app.send("set-focus root");
    let cache = client.uia.base_cache_request().expect("a cache request");
    let window = client
        .uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let ListenerFact { pid: _, fact } =
        uia_focus_fact(&window).expect("mockapp's element has its process");
    outpost.read_focus_as(&text_area);
    outpost.deliver(fact);
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
            "a_focus_is_handled_before_slow_reads_queued_ahead_of_it",
            a_focus_is_handled_before_slow_reads_queued_ahead_of_it,
        ),
        (
            "a_controls_own_focus_reports_its_focused_child",
            a_controls_own_focus_reports_its_focused_child,
        ),
        (
            "a_windowed_focus_is_reported_though_the_focused_element_read_answers_a_stand_in",
            a_windowed_focus_is_reported_though_the_focused_element_read_answers_a_stand_in,
        ),
        (
            "a_windowless_focus_that_lost_the_keyboard_focus_waits_for_the_next_focus_event",
            a_windowless_focus_that_lost_the_keyboard_focus_waits_for_the_next_focus_event,
        ),
        (
            "a_console_windows_own_focus_is_not_reported_when_its_text_area_is_focused",
            a_console_windows_own_focus_is_not_reported_when_its_text_area_is_focused,
        ),
    ]);
}
