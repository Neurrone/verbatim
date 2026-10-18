//! UIA remote operations against mockapp's real, out-of-process UIA
//! provider (`verbatim-uia-rops`): the remote focus ancestry must return
//! exactly what the classic walk returns, the same ancestors with the same
//! cached properties, and the two must fail the same way when the provider
//! is stalled or gone. A walk that hits UIA's transaction timeout is a
//! failure either way, which the outpost reports as a focus whose
//! containers are unknown. mockapp's providers are server-side, so
//! programs run in its process.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use common::outpost::OutpostUnderTest;
use verbatim_model::NormalizedEvent;
use verbatim_model::{Role, State};
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::focus_read::Judged;
use verbatim_outpost::protocol::OutpostToSupervisor;
use verbatim_uia::map::{add_legacy_checked_state, wants_legacy_checked_state};
use verbatim_uia::{
    Ancestor, Cached, ElementExt, Fill, FocusInput, Kept, ListenerEvent, Live, Node, Shape, Uia,
};
use verbatim_uia_rops::{
    Ancestry, Error, FocusAncestry, FocusAncestryFn, FocusQuery, NavigationDirection, Status,
    StepQuery, focus_ancestry, focus_ancestry_classic, focus_ancestry_remote,
    navigation_step_classic, navigation_step_remote,
};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomationElement, TreeScope_Descendants, UIA_E_ELEMENTNOTAVAILABLE,
    UIA_E_TIMEOUT, UIA_LegacyIAccessibleStatePropertyId, UIA_NamePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
use windows::core::{BSTR, Interface};

const BOTH: [(&str, FocusAncestryFn); 2] = [
    ("remote", focus_ancestry_remote),
    ("classic", focus_ancestry_classic),
];

/// UIA's transaction timeout is process-wide: the test that shortens it
/// holds this for writing, so it runs alone, and every other test holds it
/// for reading while it runs.
static TRANSACTION_TIMEOUT: RwLock<()> = RwLock::new(());

/// What a fixture holds of [`TRANSACTION_TIMEOUT`].
enum Timeout {
    Shared(#[expect(dead_code, reason = "held, not read")] RwLockReadGuard<'static, ()>),
    Alone(#[expect(dead_code, reason = "held, not read")] RwLockWriteGuard<'static, ()>),
}

/// A running ancestry fixture and a client reading it.
struct Fixture {
    app: common::MockApp,
    uia: Uia,
    root: IUIAutomationElement,
    hwnd: isize,
    /// How a focus query fills the elements it returns, with every detail,
    /// as the focus listener reads.
    fill: Fill<Ancestor>,
    _timeout: Timeout,
}

impl Fixture {
    /// A fixture for a test that leaves the transaction timeout alone.
    fn start(prefix: &str) -> Self {
        let timeout = TRANSACTION_TIMEOUT
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        Self::with(prefix, "ancestry.json", Timeout::Shared(timeout))
    }

    /// A fixture of `file` for a test that changes the transaction
    /// timeout, run while no other test in this process runs.
    fn start_alone(prefix: &str, file: &str) -> Self {
        let timeout = TRANSACTION_TIMEOUT
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        Self::with(prefix, file, Timeout::Alone(timeout))
    }

    fn with(prefix: &str, file: &str, timeout: Timeout) -> Self {
        let title = common::unique_title(prefix);
        let app = common::spawn(file, "uia", &title);
        let hwnd = common::find_window(&title).0 as isize;
        let uia = Uia::new().expect("Uia::new");
        let root = uia
            .window_live(hwnd)
            .expect("mockapp's window element")
            .element()
            .clone();
        let ancestors = Fill::new(
            &uia.request::<Ancestor>(Ancestor::all_on())
                .expect("the ancestors' cache request"),
        );
        Self {
            app,
            uia,
            root,
            hwnd,
            fill: ancestors,
            _timeout: timeout,
        }
    }

    /// The element named `name`, read as a node, as the outpost reads the
    /// focus it hands the focus query.
    fn find_node(&self, name: &str) -> Cached<Node> {
        let value = VARIANT::from(BSTR::from(name));
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &value)
            .expect("condition");
        let request = self
            .uia
            .request::<Node>(Node::all_on())
            .expect("the node's cache request");
        Live::new(self.root.clone())
            .find_first(TreeScope_Descendants, &condition, &request)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
            .unwrap_or_else(|| panic!("no element named {name:?}"))
    }

    /// The element named `name`, for its runtime id or a call into its
    /// application: nothing of its cache is read.
    fn find(&self, name: &str) -> IUIAutomationElement {
        self.find_node(name).live().element().clone()
    }

    /// Focuses fixture node `id`, named `name`, and returns its element,
    /// which the provider then reports focused, as the focus query reads
    /// it: mockapp acknowledges the command once it has taken effect.
    fn focus(&mut self, id: &str, name: &str) -> Cached<FocusInput> {
        self.app.send(&format!("focus {id}"));
        let element = self.find_node(name);
        assert_eq!(
            element.live().element().has_keyboard_focus().ok(),
            Some(true),
            "{name} has the keyboard focus"
        );
        element.narrow()
    }

    /// [`focus`](Self::focus), the element filled as the focus listener's
    /// registration fills a focus event's sender.
    fn focus_delivered(&mut self, id: &str, name: &str) -> Cached<ListenerEvent> {
        self.app.send(&format!("focus {id}"));
        let value = VARIANT::from(BSTR::from(name));
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &value)
            .expect("condition");
        let request = self
            .uia
            .request_fixed::<ListenerEvent>()
            .expect("the listener's cache request");
        let element = Live::new(self.root.clone())
            .find_first(TreeScope_Descendants, &condition, &request)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
            .unwrap_or_else(|| panic!("no element named {name:?}"));
        assert_eq!(
            element.live().element().has_keyboard_focus().ok(),
            Some(true),
            "{name} has the keyboard focus"
        );
        element
    }
}

fn query<'a>(
    element: &'a Cached<FocusInput>,
    known: &'a [Vec<i32>],
    fill: &'a Fill<Ancestor>,
) -> FocusQuery<'a> {
    FocusQuery {
        element,
        known,
        previous: None,
        depth_limit: 50,
        fill,
        out_of_time: None,
        require_focus: true,
        refresh: false,
        lock_up_from: None,
    }
}

fn focused(answer: Result<FocusAncestry, Error>, label: &str) -> Ancestry {
    match answer {
        Ok(FocusAncestry::Focused(ancestry)) => ancestry,
        Ok(FocusAncestry::NotFocused) => panic!("{label}: reported not focused"),
        Err(error) => panic!("{label}: {error}"),
    }
}

/// Everything a caller reads from a returned element without another
/// cross-process call: its runtime id, every field of its shape, each the
/// read of a cached property, and what the shape's read maps to, `mapped`.
///
/// Two fields are compared only where their pattern is available, as the
/// shape's mapping reads them (`unset_where_meaningless`): read ignoring
/// defaults, a remotely filled cache gives a property's default where a
/// locally built one gives UIA's "not supported" value, so `ValueIsReadOnly`
/// and `RangeValueValue` read as read-only and zero remotely, and as absent
/// locally, for an element without the `Value` or `RangeValue` pattern. The
/// remote program leaves `IsDataValidForForm` out when unsupported
/// (`LEFT_OUT_WHEN_UNSUPPORTED`), recording it as absent, so it reads as
/// absent either way.
fn cached_view<S: Shape + std::fmt::Debug>(
    element: &Cached<S>,
    unset_where_meaningless: impl FnOnce(&mut S),
    mapped: impl FnOnce(&S) -> String,
) -> String {
    let read = element.read();
    let mut fields = element.read();
    unset_where_meaningless(&mut fields);
    format!("{:?}\n{fields:?}\n{}", element.runtime_id(), mapped(&read))
}

/// A node's value and state fields with the two read ignoring defaults
/// unset where their pattern is not available ([`cached_view`]).
fn unset_node_fields(node: &mut Node) {
    if !node.value.range_available {
        node.value.range = None;
    }
    if !node.states.value_available {
        node.states.value_read_only = None;
    }
}

/// [`cached_view`] of an ancestor: its node's parts, whether it is a
/// control and content, and its description for the layout judgment.
fn ancestor_view(element: &Cached<Ancestor>) -> String {
    cached_view(
        element,
        |ancestor| unset_node_fields(&mut ancestor.node),
        |ancestor| {
            format!(
                "{:?}\n{}\n{:?}",
                ancestor.node.parts(Vec::new()),
                ancestor.is_control_and_content(),
                ancestor.layout_description()
            )
        },
    )
}

/// [`cached_view`] of a node: its parts.
fn node_view(element: &Cached<Node>) -> String {
    cached_view(element, unset_node_fields, |node| {
        format!("{:?}", node.parts(Vec::new()))
    })
}

fn views(elements: &[Cached<Ancestor>]) -> Vec<String> {
    elements.iter().map(ancestor_view).collect()
}

/// Runs both implementations and asserts they agree, returning the
/// remote answer.
fn both_agree(uia: &Uia, query: &FocusQuery<'_>) -> Ancestry {
    let remote = focused(focus_ancestry_remote(uia, query), "remote");
    let classic = focus_ancestry_classic(uia, query);
    let classic = focused(classic, "classic");
    let (remote_views, classic_views) = (views(&remote.ancestors), views(&classic.ancestors));
    assert_eq!(
        remote_views.len(),
        classic_views.len(),
        "the number of ancestors"
    );
    for (remote_view, classic_view) in remote_views.iter().zip(&classic_views) {
        let differences: Vec<(&str, &str)> = remote_view
            .lines()
            .zip(classic_view.lines())
            .filter(|(remote_line, classic_line)| remote_line != classic_line)
            .collect();
        assert!(
            differences.is_empty(),
            "an ancestor's cached properties differ, remote then classic: {differences:#?}"
        );
    }
    assert_eq!(
        remote.met_known, classic.met_known,
        "the known ancestor met"
    );
    assert_eq!(
        remote.depth_limited, classic.depth_limited,
        "the depth limit"
    );
    assert_eq!(remote.window, classic.window, "the nearest window");
    assert_eq!(
        remote.previous_focused, classic.previous_focused,
        "the previous element's focus"
    );
    assert!(
        remote.window.is_some(),
        "every fixture element has a window"
    );
    assert_eq!(
        remote.selected_child.as_ref().map(ancestor_view),
        classic.selected_child.as_ref().map(ancestor_view),
        "the selected child and its cached properties"
    );
    remote
}

fn names(elements: &[Cached<Ancestor>]) -> Vec<String> {
    elements
        .iter()
        .map(|element| element.read().node.name.unwrap_or_default())
        .collect()
}

fn a_deep_chain_reads_the_same_both_ways() {
    let mut fixture = Fixture::start("mockapp-rops-deep");
    let deep = fixture.focus("deep", "Deep button");
    let ancestry = both_agree(&fixture.uia, &query(&deep, &[], &fixture.fill));
    assert_eq!(
        names(&ancestry.ancestors),
        [
            "Inner group",
            "Tools",
            "Outer group",
            "",
            "Mockapp Ancestry Fixture"
        ],
        "nearest first, up to the top-level window and not the desktop"
    );
    assert_eq!(ancestry.met_known, None);
    assert!(!ancestry.depth_limited);
    assert!(ancestry.selected_child.is_none());

    // Other properties on the way: a value, a checked box.
    for (id, name) in [("notes", "Notes"), ("check", "Remember")] {
        let element = fixture.focus(id, name);
        both_agree(&fixture.uia, &query(&element, &[], &fixture.fill));
    }
    fixture.app.quit();
}

/// The classic walk asks for the lock of each element with a window of its
/// own, and holds the lock it is answered for that element's step up and
/// every later one (`docs/design/focus-pipeline.md`, section 20.3): here
/// the step from mockapp's window to the desktop root stands for the step
/// from the Settings page's content up into its frame host.
fn the_classic_walk_holds_the_lock_for_a_step_up_into_another_application() {
    let mut fixture = Fixture::start("mockapp-rops-lock-up");
    let deep = fixture.focus("deep", "Deep button");
    let lock = verbatim_uia::lock::AppLock::new(
        0,
        0,
        std::sync::Arc::new(verbatim_uia::lock::new_mutex().expect("a mutex")),
    );
    let asked = std::cell::RefCell::new(Vec::new());
    let lock_up_from = |window: isize| {
        asked.borrow_mut().push(window);
        (window == fixture.hwnd).then(|| lock.clone())
    };
    let query = FocusQuery {
        lock_up_from: Some(&lock_up_from),
        ..query(&deep, &[], &fixture.fill)
    };
    let _ = verbatim_uia::calls::take_locks();
    let ancestry = focused(focus_ancestry_classic(&fixture.uia, &query), "classic");
    assert_eq!(
        verbatim_uia::calls::take_locks(),
        1,
        "the lock is taken for the one step up from mockapp's window"
    );
    assert_eq!(*asked.borrow(), [fixture.hwnd], "the windows asked about");
    assert_eq!(
        names(&ancestry.ancestors),
        [
            "Inner group",
            "Tools",
            "Outer group",
            "",
            "Mockapp Ancestry Fixture"
        ],
        "the walk went on to the root after the step it held the lock for"
    );
    fixture.app.quit();
}

/// A remotely read element that does not support `IsDataValidForForm`,
/// which the program leaves out of its cache, reads as a valid entry, as
/// UI Automation reads "not supported", and its read makes no failed
/// cached read: the program's absent record answers it
/// (`docs/design/uia-shapes.md`, section 7, decision 3). The focus and
/// every ancestor, none of which mockapp gives the property.
fn a_remotely_read_element_without_validity_reads_valid_with_no_failed_read() {
    let mut fixture = Fixture::start("mockapp-rops-absent");
    let deep = fixture.focus("deep", "Deep button");
    let refreshed = FocusQuery {
        refresh: true,
        ..query(&deep, &[], &fixture.fill)
    };
    let ancestry = focused(focus_ancestry_remote(&fixture.uia, &refreshed), "remote");
    let element = ancestry
        .element
        .expect("the focus's own cache, filled afresh");
    let _ = verbatim_uia::calls::take_failed_reads();
    let reads: Vec<Ancestor> = std::iter::once(&element)
        .chain(&ancestry.ancestors)
        .map(Cached::read)
        .collect();
    assert_eq!(verbatim_uia::calls::take_failed_reads(), 0);
    assert_eq!(reads.len(), 6, "the focus and its five ancestors");
    for read in &reads {
        let node = &read.node;
        assert_eq!(node.states.data_valid, None, "{:?}", node.name);
        assert!(
            !node
                .states
                .states(node.role.role())
                .contains(State::InvalidEntry),
            "{:?}",
            node.name
        );
    }
    fixture.app.quit();
}

fn a_list_and_a_tab_control_report_their_selected_child() {
    let mut fixture = Fixture::start("mockapp-rops-selection");
    for (id, name, selected) in [
        ("fruits", "Fruits", "Banana"),
        ("pages", "Pages", "General"),
    ] {
        let container = fixture.focus(id, name);
        let ancestry = both_agree(&fixture.uia, &query(&container, &[], &fixture.fill));
        let child = ancestry.selected_child.expect("a selected child");
        assert_eq!(names(&[child]), [selected]);
        assert_eq!(names(&ancestry.ancestors), ["Mockapp Ancestry Fixture"]);
    }
    // A selection made after the fixture loaded.
    fixture.app.send("select apple");
    let fruits = fixture.focus("fruits", "Fruits");
    let ancestry = both_agree(&fixture.uia, &query(&fruits, &[], &fixture.fill));
    assert_eq!(
        names(&[ancestry.selected_child.expect("selected")]),
        ["Apple"]
    );
    fixture.app.quit();
}

fn the_walk_stops_at_a_known_ancestor_or_the_depth_limit() {
    let mut fixture = Fixture::start("mockapp-rops-known");
    let outer = fixture.find("Outer group");
    let tools = fixture.find("Tools");
    let deep = fixture.focus("deep", "Deep button");
    // An unrelated id first, so the index reported is the one met.
    let known = vec![
        vec![1, 2, 3],
        verbatim_uia::runtime_id(&outer),
        verbatim_uia::runtime_id(&tools),
    ];
    let ancestry = both_agree(&fixture.uia, &query(&deep, &known, &fixture.fill));
    assert_eq!(names(&ancestry.ancestors), ["Inner group", "Tools"]);
    assert_eq!(ancestry.met_known, Some(2));

    let limited = FocusQuery {
        depth_limit: 2,
        ..query(&deep, &[], &fixture.fill)
    };
    let ancestry = both_agree(&fixture.uia, &limited);
    assert_eq!(names(&ancestry.ancestors), ["Inner group", "Tools"]);
    assert!(ancestry.depth_limited);
    fixture.app.quit();
}

fn an_element_that_lost_the_focus_returns_early() {
    let mut fixture = Fixture::start("mockapp-rops-stale");
    let deep = fixture.focus("deep", "Deep button");
    fixture.focus("notes", "Notes");
    for (label, ancestry) in BOTH {
        assert!(
            matches!(
                ancestry(&fixture.uia, &query(&deep, &[], &fixture.fill)),
                Ok(FocusAncestry::NotFocused)
            ),
            "{label}: the stale focus is reported as not focused"
        );
    }
    fixture.app.quit();
}

/// An object-navigation step reads the same neighbor, with the same cached
/// properties and snapshot, and the same nearest window, both ways: parent,
/// siblings, first child, and an edge with no neighbor.
fn a_navigation_step_reads_the_same_both_ways() {
    let fixture = Fixture::start("mockapp-rops-navigation");
    let nodes = Fill::new(
        &fixture
            .uia
            .request::<Node>(Node::all_on())
            .expect("the node's cache request"),
    );
    for (from, direction, expected) in [
        (
            "Deep button",
            NavigationDirection::Parent,
            Some("Inner group"),
        ),
        (
            "Deep button",
            NavigationDirection::NextSibling,
            Some("Notes"),
        ),
        (
            "Notes",
            NavigationDirection::PreviousSibling,
            Some("Deep button"),
        ),
        (
            "Inner group",
            NavigationDirection::FirstChild,
            Some("Deep button"),
        ),
        ("Remember", NavigationDirection::NextSibling, None),
    ] {
        let element = fixture.find_node(from).narrow::<Kept>();
        let query = StepQuery {
            element: &element,
            direction,
            fill: &nodes,
        };
        let remote = navigation_step_remote(&fixture.uia, &query).expect("the remote program runs");
        let classic = navigation_step_classic(&fixture.uia, &query).expect("the classic step runs");
        let label = format!("{direction:?} from {from}");
        assert_eq!(remote.window, Some(fixture.hwnd), "{label}");
        assert_eq!(classic.window, Some(fixture.hwnd), "{label}");
        let view = |step: &verbatim_uia_rops::Step| step.neighbor.as_ref().map(node_view);
        assert_eq!(view(&remote), view(&classic), "{label}");
        assert_eq!(
            remote
                .neighbor
                .as_ref()
                .and_then(|neighbor| neighbor.read().name),
            expected.map(str::to_owned),
            "{label}"
        );
    }
    fixture.app.quit();
}

/// A UIA error constant as the HRESULT it is.
fn hresult(code: u32) -> windows::core::HRESULT {
    windows::core::HRESULT(i32::from_ne_bytes(code.to_ne_bytes()))
}

/// A failed run's extended error, when the run failed to execute.
fn execution_failure(error: &Error) -> Option<windows::core::HRESULT> {
    match error {
        Error::Failed(failure) if failure.status == Status::ExecutionFailure => {
            Some(failure.extended_error)
        }
        _ => None,
    }
}

/// The finding that decides where the outpost may run a program: against a
/// stalled provider, `Execute` waits exactly as a classic call does. UIA's
/// connection timeout does not bound it; its transaction timeout does, and
/// the run then ends with an execution failure carrying `UIA_E_TIMEOUT`.
/// Both are process-wide.
fn a_stalled_provider_holds_execute_until_the_transaction_timeout() {
    const STALL: Duration = Duration::from_millis(4000);
    // The transaction timeout is process-wide, so no other test in this
    // binary runs meanwhile ([`TRANSACTION_TIMEOUT`]).
    const TIMEOUT: Duration = Duration::from_millis(1000);
    let mut fixture = Fixture::start_alone("mockapp-rops-stall", "ancestry.json");
    let deep = fixture.focus("deep", "Deep button");

    // Each run starts once mockapp has acknowledged that its window thread
    // is stalled, and is judged by when it returned against when mockapp
    // says the stall ended.
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let connection = ProcessTimeout::set(&client, Kind::Connection, TIMEOUT);
    fixture.app.stall(STALL);
    let answer = focus_ancestry_remote(&fixture.uia, &query(&deep, &[], &fixture.fill));
    let returned = common::now_us();
    assert!(
        matches!(answer, Ok(FocusAncestry::Focused(_))),
        "the connection timeout did not end the run"
    );
    let ended = fixture.app.stall_ended(STALL);
    assert!(
        returned >= ended,
        "the run waited for the stalled provider: it returned at {returned} us, before the stall ended at {ended} us"
    );
    drop(connection);

    let _restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);
    for (label, ancestry) in BOTH {
        fixture.app.stall(STALL);
        let answer = ancestry(&fixture.uia, &query(&deep, &[], &fixture.fill));
        let returned = common::now_us();
        // Waited for, too, so the next run starts against a window thread
        // that is answering again.
        let ended = fixture.app.stall_ended(STALL);
        assert!(
            returned < ended,
            "{label}: the transaction timeout ended the call: it returned at {returned} us, after the stall ended at {ended} us"
        );
        assert_eq!(
            answer.as_ref().err().and_then(Error::hresult),
            Some(hresult(UIA_E_TIMEOUT)),
            "{label}: {answer:?}"
        );
        if label == "remote" {
            assert!(
                answer.as_ref().err().and_then(execution_failure).is_some(),
                "{answer:?}"
            );
        }
    }
    fixture.app.quit();
}

/// The HRESULT a run failed with, `None` when it answered.
fn failure(answer: &Result<impl std::fmt::Debug, Error>) -> Option<windows::core::HRESULT> {
    answer.as_ref().err().and_then(Error::hresult)
}

/// A focus walk that hits UIA's transaction timeout is a failure, never a
/// success with what it read before: the classic walk's hop that times out
/// is not the root, and a remote program that times out is not answered
/// by the classic walk, which would wait on the application a second time
/// (`docs/crates/verbatim-uia-rops.md`, "Fallback rules"). With every
/// provider call slow (`slow`), the live focus read answers in time but a
/// hop that fills an ancestor's cache, many calls, does not; with the
/// window thread stalled (`stall`) for less than two timeouts, the classic
/// walk after the program's timeout would have waited the stall out and
/// read the ancestors.
fn a_focus_walk_that_times_out_fails() {
    const TIMEOUT: Duration = Duration::from_millis(1000);
    const SLOW_CALL: Duration = Duration::from_millis(50);
    const STALL: Duration = Duration::from_millis(1500);
    let mut fixture = Fixture::start_alone("mockapp-rops-timed-out", "ancestry.json");
    let deep = fixture.focus("deep", "Deep button");
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let _restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);

    fixture.app.send(&format!("slow {}", SLOW_CALL.as_millis()));
    let classic = focus_ancestry_classic(&fixture.uia, &query(&deep, &[], &fixture.fill));
    assert_eq!(
        failure(&classic),
        Some(hresult(UIA_E_TIMEOUT)),
        "classic: {classic:?}"
    );
    let picked = focus_ancestry(&fixture.uia, &query(&deep, &[], &fixture.fill), true);
    assert_eq!(
        failure(&picked),
        Some(hresult(UIA_E_TIMEOUT)),
        "remote: {picked:?}"
    );
    assert!(
        picked.as_ref().err().and_then(execution_failure).is_some(),
        "the program's own failure: {picked:?}"
    );
    fixture.app.send("slow 0");

    fixture.app.stall(STALL);
    let picked = focus_ancestry(&fixture.uia, &query(&deep, &[], &fixture.fill), true);
    let returned = common::now_us();
    let ended = fixture.app.stall_ended(STALL);
    assert_eq!(
        failure(&picked),
        Some(hresult(UIA_E_TIMEOUT)),
        "stalled: {picked:?}"
    );
    assert!(
        returned < ended,
        "the program's timeout was answered without waiting again: it returned at {returned} us, after the stall ended at {ended} us"
    );
    fixture.app.quit();
}

/// The focus listener drops a focus whose walk to its nearest window does
/// not answer within UIA's transaction timeout, every provider call slow
/// (`slow`): NVDA's walk (`getNearestWindowHandle`) answers no window when
/// it fails, and NVDA makes no object for an element with none, so it
/// drops the event (`docs/design/focus-pipeline.md`, section 16.8).
fn a_focus_whose_nearest_window_walk_times_out_is_dropped() {
    const TIMEOUT: Duration = Duration::from_millis(300);
    const SLOW_CALL: Duration = Duration::from_millis(15);
    let mut fixture = Fixture::start_alone("mockapp-rops-no-window", "ancestry.json");
    let deep = fixture.focus_delivered("deep", "Deep button");
    let outpost = OutpostUnderTest::new(&fixture.app);
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);
    fixture.app.send(&format!("slow {}", SLOW_CALL.as_millis()));
    let (judged, _) = outpost.listener_read(&deep);
    // Acknowledged once mockapp has answered the calls still queued.
    fixture.app.send("slow 0");
    drop(restored);
    let Judged::Dropped(reason) = judged else {
        panic!("the listener took a focus whose nearest window it could not find: {judged:?}");
    };
    assert_eq!(
        reason,
        "it has no window above it, so NVDA makes no object for it"
    );
    drop(outpost);
    fixture.app.quit();
}

/// The focus listener, the focus walk's caller, and the outpost handle a
/// walk that timed out: with every pattern request slow (`slow-patterns`),
/// so that filling an element's cache outlasts UIA's transaction timeout
/// while the walk to its nearest window and the live read of its keyboard
/// focus, which request no pattern, do not, the focus is reported from its
/// event, as NVDA lets a focus through whose read fails, with its
/// containers unknown, never as having none, whether remote operations are
/// on or off. (With every provider call slow, the walk to the nearest
/// window times out first, and the focus is dropped, as
/// `a_focus_whose_nearest_window_walk_times_out_is_dropped` shows.)
/// The timeout is short, and the requests only a little slow, since
/// mockapp goes on answering a timed-out program's calls, each that much
/// late, before it takes the next command.
fn a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown() {
    const TIMEOUT: Duration = Duration::from_millis(300);
    const SLOW_CALL: Duration = Duration::from_millis(40);
    for remote in [true, false] {
        let path = if remote { "remote" } else { "classic" };
        let mut fixture =
            Fixture::start_alone(&format!("mockapp-rops-unknown-{path}"), "ancestry.json");
        let outpost = OutpostUnderTest::with_options(
            &fixture.app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        // A focus before, in the same window, read at mockapp's own pace:
        // the outpost probes the window for arbitration and keeps the
        // answer, as it has for a window it already reads. (Here the
        // listener's read shares the outpost's process, whose UI Automation
        // answers a probe of the window that it has no provider once a
        // transaction with it has timed out; a listener of its own does not
        // touch the outpost's.)
        let check = fixture.focus_delivered("check", "Remember");
        outpost.deliver(outpost.listener_fact(&check));
        assert!(
            matches!(
                outpost.next(),
                OutpostToSupervisor::Event {
                    event: NormalizedEvent::FocusChanged { .. },
                    ..
                }
            ),
            "{path}: the focus before is reported"
        );
        outpost.settled();
        let deep = fixture.focus_delivered("deep", "Deep button");
        let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
        let restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);

        fixture
            .app
            .send(&format!("slow-patterns {}", SLOW_CALL.as_millis()));
        // The listener's read runs in this process, under the timeout set
        // for it.
        let fact = outpost.listener_fact(&deep);
        // Acknowledged once mockapp has answered the calls still queued,
        // among them the rest of a program UIA stopped waiting for: until
        // then its window, busy, answers the outpost's probe that it has no
        // UIA provider.
        fixture.app.send("slow-patterns 0");
        drop(restored);
        outpost.deliver(fact);
        let said = outpost.next();
        match said {
            OutpostToSupervisor::Event {
                event:
                    NormalizedEvent::FocusChanged {
                        node,
                        ancestors,
                        ancestors_unknown,
                        ..
                    },
                ..
            } => {
                assert_eq!(node.name.as_deref(), Some("Deep button"), "{path}");
                assert!(ancestors_unknown, "{path}: the containers are unknown");
                assert_eq!(ancestors, [], "{path}");
            }
            other => panic!("{path}: the outpost said {other:?}, not the focus"),
        }
        outpost.settled();
        drop(outpost);
        fixture.app.quit();
    }
}

/// Reading a list's or a tab control's selected item, the read the classic
/// focus walk makes for a selection container (`Uia::selected`),
/// fails when the application does not answer within UIA's transaction
/// timeout, never answering that nothing is selected: with every provider
/// call slow (`slow`), and with the window thread stalled (`stall`) for
/// longer than the timeout, returning before the stall ended. It fails as
/// well once the list is gone, with its provider's process. An element with
/// no `Selection` pattern still answers that nothing is selected.
fn a_selection_read_that_times_out_or_finds_the_list_gone_fails() {
    const TIMEOUT: Duration = Duration::from_millis(1000);
    const SLOW_CALL: Duration = Duration::from_millis(50);
    const STALL: Duration = Duration::from_millis(1500);
    let mut fixture = Fixture::start_alone("mockapp-rops-selection-timeout", "ancestry.json");
    let request = fixture
        .uia
        .request::<Node>(Node::all_on())
        .expect("cache request");
    let read = |element: &Live| fixture.uia.selected(element, &request);
    let containers = [
        Live::new(fixture.find("Fruits")),
        Live::new(fixture.find("Pages")),
    ];
    let button = Live::new(fixture.find("Deep button"));
    assert_eq!(
        read(&containers[0])
            .expect("read")
            .expect("selected")
            .read()
            .name
            .as_deref(),
        Some("Banana")
    );
    assert!(
        read(&button).expect("read").is_none(),
        "no Selection pattern"
    );
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);

    fixture.app.send(&format!("slow {}", SLOW_CALL.as_millis()));
    for container in &containers {
        let answer = read(container);
        assert_eq!(
            answer.as_ref().err().map(windows::core::Error::code),
            Some(hresult(UIA_E_TIMEOUT)),
            "slow: {answer:?}"
        );
    }
    fixture.app.send("slow 0");

    for container in &containers {
        fixture.app.stall(STALL);
        let answer = read(container);
        let returned = common::now_us();
        let ended = fixture.app.stall_ended(STALL);
        assert_eq!(
            answer.as_ref().err().map(windows::core::Error::code),
            Some(hresult(UIA_E_TIMEOUT)),
            "stalled: {answer:?}"
        );
        assert!(
            returned < ended,
            "the transaction timeout ended the read: it returned at {returned} us, after the stall ended at {ended} us"
        );
    }
    drop(restored);

    let Fixture {
        app, uia, _timeout, ..
    } = fixture;
    // Returns once the process has exited.
    drop(app);
    for container in &containers {
        let answer = uia.selected(container, &request);
        assert_eq!(
            answer.as_ref().err().map(windows::core::Error::code),
            Some(hresult(UIA_E_ELEMENTNOTAVAILABLE)),
            "gone: {answer:?}"
        );
    }
}

/// Which of UIA's process-wide timeouts a [`ProcessTimeout`] sets.
#[derive(Clone, Copy)]
enum Kind {
    Connection,
    Transaction,
}

/// One of UIA's timeouts, set for a while and restored when dropped,
/// whether the test passed or not.
struct ProcessTimeout {
    client: IUIAutomation2,
    kind: Kind,
    usual: u32,
}

impl ProcessTimeout {
    fn set(client: &IUIAutomation2, kind: Kind, timeout: Duration) -> Self {
        let usual = Self::read(client, kind);
        let timeout = u32::try_from(timeout.as_millis()).expect("milliseconds");
        Self::write(client, kind, timeout).expect("set the timeout");
        Self {
            client: client.clone(),
            kind,
            usual,
        }
    }

    fn read(client: &IUIAutomation2, kind: Kind) -> u32 {
        match kind {
            // SAFETY: reading a timeout returns a plain integer.
            Kind::Connection => unsafe { client.ConnectionTimeout() },
            // SAFETY: as above.
            Kind::Transaction => unsafe { client.TransactionTimeout() },
        }
        .expect("the timeout")
    }

    fn write(client: &IUIAutomation2, kind: Kind, timeout: u32) -> windows::core::Result<()> {
        match kind {
            // SAFETY: setting a timeout takes a plain integer.
            Kind::Connection => unsafe { client.SetConnectionTimeout(timeout) },
            // SAFETY: as above.
            Kind::Transaction => unsafe { client.SetTransactionTimeout(timeout) },
        }
    }
}

impl Drop for ProcessTimeout {
    fn drop(&mut self) {
        let restored = Self::write(&self.client, self.kind, self.usual);
        if !std::thread::panicking() {
            restored.expect("restore the timeout");
        }
    }
}

fn a_provider_that_has_exited_fails_at_once() {
    let mut fixture = Fixture::start("mockapp-rops-exited");
    let deep = fixture.focus("deep", "Deep button");
    let Fixture {
        app,
        uia,
        fill,
        _timeout,
        ..
    } = fixture;
    // Returns once the process has exited.
    drop(app);
    let started = Instant::now();
    let answer = focus_ancestry_remote(&uia, &query(&deep, &[], &fill));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        answer.as_ref().err().and_then(execution_failure),
        Some(hresult(UIA_E_ELEMENTNOTAVAILABLE)),
        "{answer:?}"
    );
    let answer = focus_ancestry_classic(&uia, &query(&deep, &[], &fill));
    assert!(
        matches!(&answer, Err(Error::Uia(error)) if error.code() == hresult(UIA_E_ELEMENTNOTAVAILABLE)),
        "{answer:?}"
    );
}

/// Windows PowerShell showing a Windows Forms window whose menu bar holds
/// one menu item, "View", checked: Windows Forms says a menu item is
/// checked only in its legacy MSAA state, which NVDA reads for menu items
/// (`verbatim_uia::map::wants_legacy_checked_state`). Every Windows has
/// Windows PowerShell and Windows Forms, as the end-to-end suite's text box
/// relies on. Closed through its window, the process exiting, by
/// [`close`](Self::close); ended on drop otherwise.
struct FormsMenu {
    child: Child,
    hwnd: HWND,
    script: PathBuf,
}

impl FormsMenu {
    /// Opens the window, titled `title`, and waits for the script's word
    /// that it is shown.
    fn open(title: &str) -> Self {
        common::contain_children();
        let script = std::env::temp_dir().join(format!("{title}.ps1"));
        std::fs::write(
            &script,
            format!(
                "Add-Type -AssemblyName System.Windows.Forms\r\n\
                 $form = New-Object System.Windows.Forms.Form\r\n\
                 $form.Text = '{title}'\r\n\
                 $menu = New-Object System.Windows.Forms.MenuStrip\r\n\
                 $view = New-Object System.Windows.Forms.ToolStripMenuItem('View')\r\n\
                 $view.Checked = $true\r\n\
                 [void]$menu.Items.Add($view)\r\n\
                 $form.MainMenuStrip = $menu\r\n\
                 $form.Controls.Add($menu)\r\n\
                 $form.Add_Shown({{ [Console]::Out.WriteLine('shown'); [Console]::Out.Flush() }})\r\n\
                 [void]$form.ShowDialog()\r\n"
            ),
        )
        .expect("the script is written");
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Windows PowerShell starts");
        let stdout = child.stdout.take().expect("its output");
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let line = received
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("the window is shown");
        assert_eq!(line, "shown", "the script's word");
        let hwnd = common::find_window(title);
        Self {
            child,
            hwnd,
            script,
        }
    }

    /// Closes the window and waits for Windows PowerShell to exit, which it
    /// must do cleanly.
    fn close(mut self) {
        // SAFETY: posting a message to a window handle is sound whatever
        // the handle; the window is the script's, live until it closes.
        unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .expect("the close message is posted");
        let handle = HANDLE(self.child.as_raw_handle());
        let timeout = u32::try_from(common::WAIT_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the child's process handle, live while `self` owns it.
        let waited = unsafe { WaitForSingleObject(handle, timeout) };
        assert_eq!(waited, WAIT_OBJECT_0, "Windows PowerShell did not exit");
        let status = self.child.wait().expect("its exit is read");
        assert!(status.success(), "Windows PowerShell exited with {status}");
    }
}

impl Drop for FormsMenu {
    fn drop(&mut self) {
        let killed = self.child.kill();
        let ended = self.child.wait();
        let removed = std::fs::remove_file(&self.script);
        if !std::thread::panicking() {
            if let Err(error) = &killed {
                assert!(
                    matches!(self.child.try_wait(), Ok(Some(_))),
                    "Windows PowerShell {} could not be ended: {error}",
                    self.child.id()
                );
            }
            ended.expect("Windows PowerShell is reaped");
            removed.expect("the script is removed");
        }
    }
}

/// A Windows Forms menu item, which no pattern makes checkable, says it is
/// checked only in its legacy MSAA state. Its element can be imported into
/// a remote operation (found on 2026-10-17, Windows PowerShell 5.1's
/// Windows Forms on Windows 11 26200), so the focus read takes the remote
/// program, which reads that state in its one round trip: the checked and
/// focusable bits (`STATE_SYSTEM_CHECKED`, `STATE_SYSTEM_FOCUSABLE`), the
/// same bits the live read the listener made before answers. The classic
/// walk reads none, leaving the live read to its caller.
fn a_windows_forms_menu_item_is_checked_from_the_remote_read() {
    let title = common::unique_title("mockapp-rops-forms-menu");
    let forms = FormsMenu::open(&title);
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(forms.hwnd.0 as isize)
        .expect("the form's window element");
    let value = VARIANT::from(BSTR::from("View"));
    let condition = uia
        .property_condition(UIA_NamePropertyId, &value)
        .expect("condition");
    let request = uia
        .request::<Node>(Node::all_on())
        .expect("the node's cache request");
    let view = root
        .find_first(TreeScope_Descendants, &condition, &request)
        .expect("the search")
        .expect("the menu item");
    let parts = view.read().parts(view.runtime_id());
    assert_eq!(parts.role, Role::MenuItem);
    assert!(
        wants_legacy_checked_state(parts.role, parts.states),
        "no pattern makes it checkable: {:?}",
        parts.states
    );

    let input: Cached<FocusInput> = view.narrow();
    let fill = Fill::new(
        &uia.request::<Ancestor>(Ancestor::all_on())
            .expect("the ancestors' cache request"),
    );
    let query = FocusQuery {
        require_focus: false,
        ..query(&input, &[], &fill)
    };
    let (answer, path) = focus_ancestry(&uia, &query, true).expect("the focus ancestry");
    assert!(matches!(path, verbatim_uia_rops::Path::Remote), "{path:?}");
    let ancestry = focused(Ok(answer), "remote");
    let legacy = ancestry.legacy_state.expect("read for a menu item");
    assert_eq!(legacy, Some(0x0010_0010), "checked and focusable");
    let classic = focused(focus_ancestry_classic(&uia, &query), "classic");
    assert_eq!(classic.legacy_state, None, "the classic walk reads none");

    let live = view
        .live()
        .element()
        .current_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId)
        .expect("the legacy state's read");
    assert_eq!(live, legacy, "the live read answers the same");
    let mut states = parts.states;
    add_legacy_checked_state(&mut states, legacy);
    assert_eq!(
        states,
        parts.states.with(State::Checkable).with(State::Checked),
        "checked, from its legacy state {legacy:?}"
    );

    forms.close();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_windows_forms_menu_item_is_checked_from_the_remote_read",
            a_windows_forms_menu_item_is_checked_from_the_remote_read,
        ),
        (
            "a_navigation_step_reads_the_same_both_ways",
            a_navigation_step_reads_the_same_both_ways,
        ),
        (
            "a_deep_chain_reads_the_same_both_ways",
            a_deep_chain_reads_the_same_both_ways,
        ),
        (
            "the_classic_walk_holds_the_lock_for_a_step_up_into_another_application",
            the_classic_walk_holds_the_lock_for_a_step_up_into_another_application,
        ),
        (
            "a_remotely_read_element_without_validity_reads_valid_with_no_failed_read",
            a_remotely_read_element_without_validity_reads_valid_with_no_failed_read,
        ),
        (
            "a_list_and_a_tab_control_report_their_selected_child",
            a_list_and_a_tab_control_report_their_selected_child,
        ),
        (
            "the_walk_stops_at_a_known_ancestor_or_the_depth_limit",
            the_walk_stops_at_a_known_ancestor_or_the_depth_limit,
        ),
        (
            "an_element_that_lost_the_focus_returns_early",
            an_element_that_lost_the_focus_returns_early,
        ),
        (
            "a_stalled_provider_holds_execute_until_the_transaction_timeout",
            a_stalled_provider_holds_execute_until_the_transaction_timeout,
        ),
        (
            "a_provider_that_has_exited_fails_at_once",
            a_provider_that_has_exited_fails_at_once,
        ),
        (
            "a_focus_walk_that_times_out_fails",
            a_focus_walk_that_times_out_fails,
        ),
        (
            "a_focus_whose_nearest_window_walk_times_out_is_dropped",
            a_focus_whose_nearest_window_walk_times_out_is_dropped,
        ),
        (
            "a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown",
            a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown,
        ),
        (
            "a_selection_read_that_times_out_or_finds_the_list_gone_fails",
            a_selection_read_that_times_out_or_finds_the_list_gone_fails,
        ),
    ]);
}
