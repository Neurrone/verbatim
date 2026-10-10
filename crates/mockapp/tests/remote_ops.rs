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

use std::fmt::Write as _;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use common::outpost::OutpostUnderTest;
use verbatim_model::NormalizedEvent;
use verbatim_outpost::OutpostOptions;
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{ListenerFact, OutpostToSupervisor};
use verbatim_uia::{CACHED_PROPERTIES, ElementExt, NodeIdRegistry, Uia, map};
use verbatim_uia_rops::{
    Ancestry, Error, FocusAncestry, FocusAncestryFn, FocusQuery, LEFT_OUT_WHEN_UNSUPPORTED,
    NavigationDirection, Status, StepQuery, focus_ancestry, focus_ancestry_classic,
    focus_ancestry_remote, navigation_step_classic, navigation_step_remote,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomationElement, TreeScope_Descendants, UIA_E_ELEMENTNOTAVAILABLE,
    UIA_E_TIMEOUT, UIA_NamePropertyId,
};
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
    _timeout: Timeout,
}

impl Fixture {
    /// A fixture for a test that leaves the transaction timeout alone.
    fn start(prefix: &str) -> Self {
        let timeout = TRANSACTION_TIMEOUT
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        Self::with(prefix, Timeout::Shared(timeout))
    }

    /// A fixture for the test that changes the transaction timeout, run
    /// while no other test in this process runs.
    fn start_alone(prefix: &str) -> Self {
        let timeout = TRANSACTION_TIMEOUT
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        Self::with(prefix, Timeout::Alone(timeout))
    }

    fn with(prefix: &str, timeout: Timeout) -> Self {
        let title = common::unique_title(prefix);
        let app = common::spawn("ancestry.json", "uia", &title);
        let hwnd = common::find_window(&title).0 as isize;
        let uia = Uia::new().expect("Uia::new");
        let cache = uia.base_cache_request().expect("base cache request");
        let root = uia
            .element_from_handle(hwnd, &cache)
            .expect("element_from_handle");
        Self {
            app,
            uia,
            root,
            hwnd,
            _timeout: timeout,
        }
    }

    /// The element named `name`, built with the base cache request as a
    /// focus event's sender is.
    fn find(&self, name: &str) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("base cache request");
        let value = VARIANT::from(BSTR::from(name));
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &value)
            .expect("condition");
        self.root
            .find_first_build_cache(TreeScope_Descendants, &condition, &cache)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
            .unwrap_or_else(|| panic!("no element named {name:?}"))
    }

    /// Focuses fixture node `id`, named `name`, and returns its element,
    /// which the provider then reports focused: mockapp acknowledges the
    /// command once it has taken effect.
    fn focus(&mut self, id: &str, name: &str) -> IUIAutomationElement {
        self.app.send(&format!("focus {id}"));
        let element = self.find(name);
        assert_eq!(
            element.has_keyboard_focus().ok(),
            Some(true),
            "{name} has the keyboard focus"
        );
        element
    }
}

fn query<'a>(element: &'a IUIAutomationElement, known: &'a [Vec<i32>]) -> FocusQuery<'a> {
    FocusQuery {
        element,
        known,
        previous: None,
        depth_limit: 50,
        properties: CACHED_PROPERTIES,
        deadline: None,
        require_focus: true,
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
/// cross-process call: its runtime id, every cached property, and the
/// snapshot the outpost maps it to.
///
/// The cached values are compared as read with defaults: read while
/// ignoring defaults, a remotely filled cache returns a property's default
/// where a locally built one returns UIA's "not supported" value, which is
/// why the snapshot gates such properties on their pattern's availability,
/// and the remote program leaves the rest out when unsupported
/// (`LEFT_OUT_WHEN_UNSUPPORTED`); the snapshot covers those.
fn cached_view(element: &IUIAutomationElement, registry: &NodeIdRegistry) -> String {
    let mut view = format!("{:?}", verbatim_uia::runtime_id(element));
    for &property in CACHED_PROPERTIES {
        if LEFT_OUT_WHEN_UNSUPPORTED.contains(&property) {
            continue;
        }
        let value = element.cached_value(property);
        let _ = write!(
            view,
            "\n{}: {:?}",
            property.0,
            value.map(|value| format!("{value:?}"))
        );
    }
    let snapshot = map::snapshot_from_cached_element(element, registry);
    let _ = write!(view, "\n{snapshot:?}");
    view
}

fn views(elements: &[IUIAutomationElement], registry: &NodeIdRegistry) -> Vec<String> {
    elements
        .iter()
        .map(|element| cached_view(element, registry))
        .collect()
}

/// Runs both implementations and asserts they agree, returning the
/// remote answer.
fn both_agree(uia: &Uia, query: &FocusQuery<'_>) -> Ancestry {
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let remote = focused(focus_ancestry_remote(uia, query), "remote");
    let classic = focus_ancestry_classic(uia, query);
    let classic = focused(classic, "classic");
    let (remote_views, classic_views) = (
        views(&remote.ancestors, &registry),
        views(&classic.ancestors, &registry),
    );
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
        remote
            .selected_child
            .as_ref()
            .map(|child| cached_view(child, &registry)),
        classic
            .selected_child
            .as_ref()
            .map(|child| cached_view(child, &registry)),
        "the selected child and its cached properties"
    );
    remote
}

fn names(elements: &[IUIAutomationElement]) -> Vec<String> {
    elements
        .iter()
        .map(|element| {
            element
                .cached_string(UIA_NamePropertyId)
                .unwrap_or_default()
        })
        .collect()
}

fn a_deep_chain_reads_the_same_both_ways() {
    let mut fixture = Fixture::start("mockapp-rops-deep");
    let deep = fixture.focus("deep", "Deep button");
    let ancestry = both_agree(&fixture.uia, &query(&deep, &[]));
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
        both_agree(&fixture.uia, &query(&element, &[]));
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
        let ancestry = both_agree(&fixture.uia, &query(&container, &[]));
        let child = ancestry.selected_child.expect("a selected child");
        assert_eq!(names(&[child]), [selected]);
        assert_eq!(names(&ancestry.ancestors), ["Mockapp Ancestry Fixture"]);
    }
    // A selection made after the fixture loaded.
    fixture.app.send("select apple");
    let fruits = fixture.focus("fruits", "Fruits");
    let ancestry = both_agree(&fixture.uia, &query(&fruits, &[]));
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
    let ancestry = both_agree(&fixture.uia, &query(&deep, &known));
    assert_eq!(names(&ancestry.ancestors), ["Inner group", "Tools"]);
    assert_eq!(ancestry.met_known, Some(2));

    let limited = FocusQuery {
        depth_limit: 2,
        ..query(&deep, &[])
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
                ancestry(&fixture.uia, &query(&deep, &[])),
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
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
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
        let element = fixture.find(from);
        let query = StepQuery {
            element: &element,
            direction,
            properties: CACHED_PROPERTIES,
        };
        let remote = navigation_step_remote(&fixture.uia, &query).expect("the remote program runs");
        let classic = navigation_step_classic(&fixture.uia, &query).expect("the classic step runs");
        let label = format!("{direction:?} from {from}");
        assert_eq!(remote.window, Some(fixture.hwnd), "{label}");
        assert_eq!(classic.window, Some(fixture.hwnd), "{label}");
        let view = |step: &verbatim_uia_rops::Step| {
            step.neighbor
                .as_ref()
                .map(|neighbor| cached_view(neighbor, &registry))
        };
        assert_eq!(view(&remote), view(&classic), "{label}");
        assert_eq!(
            remote
                .neighbor
                .as_ref()
                .map(|neighbor| names(std::slice::from_ref(neighbor)).remove(0)),
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
    let mut fixture = Fixture::start_alone("mockapp-rops-stall");
    let deep = fixture.focus("deep", "Deep button");

    // Each run starts once mockapp has acknowledged that its window thread
    // is stalled, and is judged by when it returned against when mockapp
    // says the stall ended.
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let connection = ProcessTimeout::set(&client, Kind::Connection, TIMEOUT);
    fixture.app.stall(STALL);
    let answer = focus_ancestry_remote(&fixture.uia, &query(&deep, &[]));
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
        let answer = ancestry(&fixture.uia, &query(&deep, &[]));
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
    let mut fixture = Fixture::start_alone("mockapp-rops-timed-out");
    let deep = fixture.focus("deep", "Deep button");
    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    let _restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);

    fixture.app.send(&format!("slow {}", SLOW_CALL.as_millis()));
    let classic = focus_ancestry_classic(&fixture.uia, &query(&deep, &[]));
    assert_eq!(
        failure(&classic),
        Some(hresult(UIA_E_TIMEOUT)),
        "classic: {classic:?}"
    );
    let picked = focus_ancestry(&fixture.uia, &query(&deep, &[]), true);
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
    let picked = focus_ancestry(&fixture.uia, &query(&deep, &[]), true);
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

/// The outpost, the focus walk's caller, handles a walk that timed out:
/// with every provider call slow (`slow`), so that a hop filling an
/// ancestor's cache outlasts UIA's transaction timeout while each read of
/// the focus itself does not, the focus is reported with its containers
/// unknown, never as having none, whether remote operations are on or off.
/// The timeout is short, and the calls only a little slow, since mockapp
/// goes on answering a timed-out program's calls, each that much late,
/// before it takes the next command.
fn a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown() {
    const TIMEOUT: Duration = Duration::from_millis(300);
    const SLOW_CALL: Duration = Duration::from_millis(15);
    for remote in [true, false] {
        let path = if remote { "remote" } else { "classic" };
        let mut fixture = Fixture::start_alone(&format!("mockapp-rops-unknown-{path}"));
        let deep = fixture.focus("deep", "Deep button");
        let outpost = OutpostUnderTest::with_options(
            &fixture.app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        let ListenerFact { fact, .. } =
            uia_focus_fact(&deep).expect("mockapp's element has its process");
        outpost.read_focus_as(&deep);
        let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
        let restored = ProcessTimeout::set(&client, Kind::Transaction, TIMEOUT);

        fixture.app.send(&format!("slow {}", SLOW_CALL.as_millis()));
        outpost.deliver(fact);
        let said = outpost.next();
        // Acknowledged once mockapp has answered the calls still queued,
        // among them the rest of a program UIA stopped waiting for.
        fixture.app.send("slow 0");
        drop(restored);
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
/// focus walk makes for a selection container (`Uia::selected_element`),
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
    let mut fixture = Fixture::start_alone("mockapp-rops-selection-timeout");
    let cache = fixture
        .uia
        .cache_request(CACHED_PROPERTIES)
        .expect("cache request");
    let read = |element: &IUIAutomationElement| fixture.uia.selected_element(element, &cache);
    let containers = [fixture.find("Fruits"), fixture.find("Pages")];
    let button = fixture.find("Deep button");
    assert_eq!(
        names(&[read(&containers[0]).expect("read").expect("selected")]),
        ["Banana"]
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
        let answer = uia.selected_element(container, &cache);
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
        app, uia, _timeout, ..
    } = fixture;
    // Returns once the process has exited.
    drop(app);
    let started = Instant::now();
    let answer = focus_ancestry_remote(&uia, &query(&deep, &[]));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        answer.as_ref().err().and_then(execution_failure),
        Some(hresult(UIA_E_ELEMENTNOTAVAILABLE)),
        "{answer:?}"
    );
    let answer = focus_ancestry_classic(&uia, &query(&deep, &[]));
    assert!(
        matches!(&answer, Err(Error::Uia(error)) if error.code() == hresult(UIA_E_ELEMENTNOTAVAILABLE)),
        "{answer:?}"
    );
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_navigation_step_reads_the_same_both_ways",
            a_navigation_step_reads_the_same_both_ways,
        ),
        (
            "a_deep_chain_reads_the_same_both_ways",
            a_deep_chain_reads_the_same_both_ways,
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
            "a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown",
            a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown,
        ),
        (
            "a_selection_read_that_times_out_or_finds_the_list_gone_fails",
            a_selection_read_that_times_out_or_finds_the_list_gone_fails,
        ),
    ]);
}
