//! UIA remote operations against mockapp's real, out-of-process UIA
//! provider (`verbatim-uia-rops`): the remote focus ancestry must return
//! exactly what the classic walk returns, the same ancestors with the same
//! cached properties, and the two must fail the same way when the provider
//! is stalled or gone. mockapp's providers are server-side, so programs run
//! in its process.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use verbatim_uia::{CACHED_PROPERTIES, NodeIdRegistry, Uia, map};
use verbatim_uia_rops::{
    Ancestry, Error, FocusAncestry, FocusAncestryFn, FocusQuery, LEFT_OUT_WHEN_UNSUPPORTED, Status,
    focus_ancestry_classic, focus_ancestry_remote,
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

/// A running ancestry fixture and a client reading it.
struct Fixture {
    app: common::MockApp,
    uia: Uia,
    root: IUIAutomationElement,
}

impl Fixture {
    fn start(prefix: &str) -> Self {
        let title = common::unique_title(prefix);
        let app = common::spawn("ancestry.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("Uia::new");
        let cache = uia.base_cache_request().expect("base cache request");
        let root = uia
            .element_from_handle(hwnd.0 as isize, &cache)
            .expect("element_from_handle");
        Self { app, uia, root }
    }

    /// The element named `name`, built with the base cache request as a
    /// focus event's sender is.
    fn find(&self, name: &str) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("base cache request");
        let value = VARIANT::from(BSTR::from(name));
        // SAFETY: a live client and root element; a search by name.
        unsafe {
            let condition = self
                .uia
                .client()
                .CreatePropertyCondition(UIA_NamePropertyId, &value)
                .expect("condition");
            self.root
                .FindFirstBuildCache(TreeScope_Descendants, &condition, &cache)
                .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
        }
    }

    /// Focuses fixture node `id`, named `name`, and returns its element
    /// once the provider reports it focused.
    fn focus(&mut self, id: &str, name: &str) -> IUIAutomationElement {
        self.app.send(&format!("focus {id}"));
        let element = self.find(name);
        common::wait_until(&format!("{name} has the keyboard focus"), || {
            // SAFETY: a live element; a live read.
            unsafe { element.CurrentHasKeyboardFocus() }.is_ok_and(windows::core::BOOL::as_bool)
        });
        element
    }
}

fn query<'a>(element: &'a IUIAutomationElement, known: &'a [Vec<i32>]) -> FocusQuery<'a> {
    FocusQuery {
        element,
        known,
        depth_limit: 50,
        properties: CACHED_PROPERTIES,
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
        // SAFETY: a live element built with the base properties.
        let value = unsafe { element.GetCachedPropertyValue(property) };
        let _ = write!(
            view,
            "\n{}: {:?}",
            property.0,
            value.map(|value| format!("{value:?}"))
        );
    }
    // SAFETY: as above.
    let snapshot = unsafe { map::snapshot_from_cached_element(element, registry) };
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
        // SAFETY: live elements with the name cached.
        .map(|element| {
            unsafe { element.CachedName() }.map_or_else(|_| "?".into(), |n| n.to_string())
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
    fixture.app.send("quit");
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
    let fruits = fixture.find("Fruits");
    common::wait_until("Apple is selected", || {
        // SAFETY: a live element; the cached properties stay readable.
        unsafe {
            verbatim_uia::selected_element(
                &fruits,
                &fixture.uia.base_cache_request().expect("cache"),
            )
        }
        .is_some_and(|child| names(&[child]) == ["Apple"])
    });
    fixture.app.send("focus fruits");
    let ancestry = both_agree(&fixture.uia, &query(&fruits, &[]));
    assert_eq!(
        names(&[ancestry.selected_child.expect("selected")]),
        ["Apple"]
    );
    fixture.app.send("quit");
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
    fixture.app.send("quit");
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
    fixture.app.send("quit");
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
/// stalled provider, `Execute` waits exactly as a classic call does. The
/// connection timeout (`Uia::within`) does not bound it; UIA's transaction
/// timeout, which is process-wide, does, and the run then ends with an
/// execution failure carrying `UIA_E_TIMEOUT`.
fn a_stalled_provider_holds_execute_until_the_transaction_timeout() {
    const STALL: Duration = Duration::from_millis(4000);
    // The transaction timeout is process-wide, so the other tests in this
    // binary run under it meanwhile; long enough not to fail their calls.
    const TIMEOUT: Duration = Duration::from_millis(1000);
    let mut fixture = Fixture::start("mockapp-rops-stall");
    let deep = fixture.focus("deep", "Deep button");
    let stall = |fixture: &mut Fixture| {
        fixture.app.send(&format!("stall {}", STALL.as_millis()));
        // Let mockapp's window thread take the stall.
        std::thread::sleep(Duration::from_millis(200));
    };

    stall(&mut fixture);
    let started = Instant::now();
    let answer = fixture
        .uia
        .within(TIMEOUT, |uia| {
            focus_ancestry_remote(uia, &query(&deep, &[]))
        })
        .expect("within");
    assert!(
        matches!(answer, Ok(FocusAncestry::Focused(_))),
        "the connection timeout did not end the run"
    );
    assert!(
        started.elapsed() >= STALL.saturating_sub(TIMEOUT),
        "the run waited for the stalled provider ({:?})",
        started.elapsed()
    );

    let client: IUIAutomation2 = fixture.uia.client().cast().expect("IUIAutomation2");
    // SAFETY: reading and setting a timeout take plain integers.
    let usual = unsafe { client.TransactionTimeout() }.expect("transaction timeout");
    let timeout = u32::try_from(TIMEOUT.as_millis()).expect("milliseconds");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(timeout) }.expect("set transaction timeout");
    std::thread::sleep(STALL);
    for (label, ancestry) in BOTH {
        stall(&mut fixture);
        let started = Instant::now();
        let answer = ancestry(&fixture.uia, &query(&deep, &[]));
        let elapsed = started.elapsed();
        assert!(
            elapsed < STALL.saturating_sub(TIMEOUT),
            "{label}: the transaction timeout ended the call ({elapsed:?})"
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
        std::thread::sleep(STALL);
    }
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(usual) }.expect("restore transaction timeout");
    fixture.app.send("quit");
}

fn a_provider_that_has_exited_fails_at_once() {
    let mut fixture = Fixture::start("mockapp-rops-exited");
    let deep = fixture.focus("deep", "Deep button");
    let Fixture { app, uia, .. } = fixture;
    drop(app);
    std::thread::sleep(Duration::from_millis(500));
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
        matches!(&answer, Err(Error::Uia(error)) if verbatim_uia::element_is_gone(error)),
        "{answer:?}"
    );
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
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
    ]);
}
