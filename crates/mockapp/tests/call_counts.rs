//! The operation ledger's ratchet (`docs/performance.md`): for each operation
//! on each backend, exactly how many cross-process calls the client side
//! makes, by kind, and exactly how many calls mockapp's providers answer, by
//! method.
//!
//! An exact count is a ratchet: a change that adds a call fails here, and a
//! change that removes one must lower the number here, deliberately, in the
//! same commit as the ledger. On a mismatch the test prints every measured
//! count, so the numbers that moved are all in one run's output.
//!
//! The MSAA operations run through a real outpost in this process, as
//! `slow_application.rs` does: the test hands it a focus fact or a query,
//! as the listener and Core would, and reads the calls from the event's or
//! reply's timing. The UIA operations cannot: the outpost finds a UIA
//! focus's element by reading the system's keyboard focus
//! (`GetFocusedElement`), and a test must not take the keyboard focus from
//! the desktop it runs on. They run on this thread instead, making the
//! same `verbatim-uia` calls in the same order as the outpost's worker does
//! once it has the element in hand ([`uia_focus`] and [`uia_navigate`] say
//! which code each follows), and read this thread's count. The outpost's
//! one read of the focused element is then the only call they leave out.
//!
//! mockapp's focus moves with `set-focus`, which raises no event, so no
//! other client on the machine (a running screen reader, say) calls into
//! mockapp while an operation is measured.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::collections::HashMap;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use verbatim_model::{
    CallCounts, CallKind, NodeId, NodeSnapshot, NormalizedEvent, QueryKind, Role, TraceId, TreeNode,
};
use verbatim_outpost::Outpost;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
    SupervisorToOutpost, read_message,
};
use verbatim_uia::map::snapshot_from_cached_element;
use verbatim_uia::{AncestorStops, AncestorWalk, NodeIdRegistry, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationCacheRequest, IUIAutomationElement};

/// The fixture's nodes, by their index in mockapp's tree (depth first, the
/// root at 0): mockapp answers `WM_GETOBJECT` for index `i` at object id
/// `i + 1`, the address an MSAA focus event names.
const FIRST: usize = 2;
const SECOND: usize = 3;
const LIST: usize = 4;
const ITEM_ONE: usize = 5;
const ITEM_TWO: usize = 6;

/// Calls by kind, in the order `CallCounts` lists them.
fn calls(uia: u32, msaa: u32, window_messages: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa,
        window_messages,
    }
}

/// What one operation cost: the client's calls and the providers' hits.
struct Cost {
    calls: CallCounts,
    hits: Vec<(&'static str, u32)>,
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
        expected_hits: &[(&str, u32)],
    ) {
        if cost.calls != expected_calls || cost.hits != expected_hits {
            self.mismatches.push(format!(
                "{operation}: measured {:?} and hits {:?}",
                cost.calls, cost.hits
            ));
        }
    }

    fn finish(self) {
        assert!(
            self.mismatches.is_empty(),
            "an operation's cost moved; when that is deliberate, update this test and \
             docs/performance.md together.\n{}",
            self.mismatches.join("\n")
        );
    }
}

/// A real outpost in this process, watching mockapp, with its messages.
struct OutpostUnderTest {
    outpost: Outpost,
    messages: Receiver<OutpostToSupervisor>,
    next_request: u64,
}

impl OutpostUnderTest {
    fn new(pid: u32) -> Self {
        let (pipe_in, pipe_out) = std::io::pipe().expect("an anonymous pipe");
        let outpost = Outpost::new(Box::new(pipe_out), pid);
        let (messages_tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe_in);
            while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
                if messages_tx.send(message).is_err() {
                    return;
                }
            }
        });
        Self {
            outpost,
            messages,
            next_request: 1,
        }
    }

    /// The next message `wanted` picks out, skipping the rest.
    fn wait_for<T>(&self, mut wanted: impl FnMut(OutpostToSupervisor) -> Option<T>) -> T {
        let deadline = Instant::now() + common::WAIT_TIMEOUT;
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(wait)
                .expect("the outpost answers before the wait times out");
            if let Some(found) = wanted(message) {
                return found;
            }
        }
    }

    /// Hands the outpost an MSAA focus on mockapp's node `index`, as the
    /// listener would, and returns the focus it reports and the calls it
    /// made.
    fn msaa_focus(&self, hwnd: HWND, index: usize) -> (NodeSnapshot, CallCounts) {
        self.outpost
            .handle_command(&SupervisorToOutpost::DeliverFact {
                trace_id: TraceId::mint(),
                observed_at_ms: 0,
                timing: EventTiming::default(),
                fact: DeliveredFact::MsaaFocus {
                    hwnd: hwnd.0 as isize,
                    id_object: i32::try_from(index + 1).expect("a small index"),
                    id_child: 0,
                },
            });
        self.wait_for(|message| match message {
            OutpostToSupervisor::Event {
                event: NormalizedEvent::FocusChanged { node, .. },
                timing,
                ..
            } => Some((node, timing.calls)),
            _ => None,
        })
    }

    /// Asks the outpost for one object-navigation step, as Core would, and
    /// returns the neighbor and the calls it made.
    fn navigate(&mut self, node_id: NodeId, kind: QueryKind) -> (NodeSnapshot, CallCounts) {
        let request_id = self.next_request;
        self.next_request += 1;
        self.outpost.handle_command(&SupervisorToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id,
            query: Query::Navigate { node_id, kind },
        });
        self.wait_for(|message| match message {
            OutpostToSupervisor::Reply {
                request_id: answered,
                outcome,
                timing,
                ..
            } if answered == request_id => match outcome {
                QueryOutcome::Done(QueryResult::Navigated(Some(neighbor))) => {
                    Some((neighbor, timing.calls))
                }
                other => panic!("navigation answered {other:?}"),
            },
            _ => None,
        })
    }
}

/// Moves mockapp's focus to `id`, hands the outpost the focus on `index`,
/// and measures it.
fn measure_msaa_focus(
    app: &mut common::MockApp,
    hwnd: HWND,
    outpost: &OutpostUnderTest,
    (id, index): (&str, usize),
) -> (NodeSnapshot, Cost) {
    common::apply(app, hwnd, &format!("set-focus {id}"));
    let (node, calls) = outpost.msaa_focus(hwnd, index);
    (
        node,
        Cost {
            calls,
            hits: common::read_hits(hwnd),
        },
    )
}

fn msaa_focus_changes_cost_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-focus");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut ratchet = Ratchet::default();

    let (node, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("first", FIRST));
    assert_eq!(node.name.as_deref(), Some("First"));
    ratchet.check(
        "MSAA focus, cold",
        &cost,
        calls(0, 30, 2),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accLocation", 3),
        ],
    );

    let (node, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("second", SECOND));
    assert_eq!(node.name.as_deref(), Some("Second"));
    ratchet.check(
        "MSAA focus, steady state",
        &cost,
        calls(0, 30, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accLocation", 3),
        ],
    );

    let (node, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("list", LIST));
    assert_eq!(node.role, Role::List);
    ratchet.check(
        "MSAA focus into a list",
        &cost,
        calls(0, 31, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 7),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accFocus", 1),
            ("accSelection", 1),
            ("accLocation", 3),
        ],
    );

    let (node, _) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item1", ITEM_ONE));
    assert_eq!(node.name.as_deref(), Some("One"));
    let (node, cost) = measure_msaa_focus(&mut app, hwnd, &outpost, ("item2", ITEM_TWO));
    assert_eq!(node.name.as_deref(), Some("Two"));
    ratchet.check(
        "MSAA arrow to the next list item",
        &cost,
        calls(0, 30, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("accParent", 14),
            ("get_accChild", 1),
            ("get_accName", 3),
            ("get_accValue", 3),
            ("get_accDescription", 3),
            ("get_accRole", 4),
            ("get_accState", 3),
            ("get_accKeyboardShortcut", 3),
            ("accLocation", 3),
        ],
    );

    ratchet.finish();
    app.send("quit");
}

fn msaa_navigation_steps_cost_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-navigation");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::new(app.pid());
    let mut ratchet = Ratchet::default();
    let (first, _) = measure_msaa_focus(&mut app, hwnd, &outpost, ("first", FIRST));

    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.id, QueryKind::NextSibling);
    assert_eq!(second.name.as_deref(), Some("Second"));
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
    };
    ratchet.check(
        "MSAA next sibling",
        &cost,
        calls(0, 10, 0),
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

    common::reset_hits(hwnd);
    let (group, calls_made) = outpost.navigate(second.id, QueryKind::Parent);
    assert_eq!(group.name.as_deref(), Some("Settings"));
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
    };
    ratchet.check(
        "MSAA parent",
        &cost,
        calls(0, 10, 0),
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

    ratchet.finish();
    app.send("quit");
}

/// A UIA client on this thread over mockapp's tree, with every element the
/// tree holds, by name, for the operations to start from.
struct UiaUnderTest {
    uia: Uia,
    cache: IUIAutomationCacheRequest,
    registry: NodeIdRegistry,
    elements: HashMap<String, IUIAutomationElement>,
}

impl UiaUnderTest {
    fn new(hwnd: HWND) -> Self {
        let uia = Uia::new().expect("a UIA client");
        let cache = uia.base_cache_request().expect("a cache request");
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let root = uia
            .element_from_handle(hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        // SAFETY: `root` was just built with `cache`.
        let (tree, _) =
            unsafe { uia.walk_tree(&root, &cache, &registry, 8, 64) }.expect("mockapp's tree");
        let mut elements = HashMap::new();
        collect(&tree, &registry, &mut elements);
        Self {
            uia,
            cache,
            registry,
            elements,
        }
    }

    fn element(&self, name: &str) -> &IUIAutomationElement {
        &self.elements[name]
    }

    /// Runs `operation` on this thread and measures it.
    fn measure<T>(&self, hwnd: HWND, operation: impl FnOnce(&Self) -> T) -> (T, Cost) {
        let _ = verbatim_uia::calls::take();
        common::reset_hits(hwnd);
        let result = operation(self);
        let cost = Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
        };
        (result, cost)
    }
}

/// Every named node's live element, from the registry the walk filled.
fn collect(
    node: &TreeNode,
    registry: &NodeIdRegistry,
    elements: &mut HashMap<String, IUIAutomationElement>,
) {
    if let Some(name) = &node.snapshot.name
        && let Some(element) = registry
            .element_of(node.snapshot.id)
            .and_then(|agile| agile.resolve().ok())
    {
        elements.insert(name.clone(), element);
    }
    for child in &node.children {
        collect(child, registry, elements);
    }
}

/// The calls the outpost's worker makes for a UIA focus once it has the
/// focused element in hand, in the same order: `Worker::uia_focus` in
/// `verbatim-outpost`'s `worker.rs` finds the nearest window of an element
/// with none of its own and, the first time, probes that window's provider
/// for arbitration; `uia_enrichment` and `uia_ancestors` in its `read.rs`
/// walk the ancestors, stopping at one the previous focus's chain holds,
/// and read a list's selected child. Returns the new focus's chain,
/// outermost first, ending with the focus, as the worker keeps it.
fn uia_focus(
    under_test: &UiaUnderTest,
    hwnd: HWND,
    element: &IUIAutomationElement,
    previous: &[NodeSnapshot],
    first_in_window: bool,
) -> (Vec<NodeSnapshot>, Option<NodeSnapshot>) {
    let UiaUnderTest {
        uia,
        cache,
        registry,
        ..
    } = under_test;
    assert_eq!(
        verbatim_uia::nearest_window_handle(element),
        Some(hwnd.0 as isize)
    );
    if first_in_window {
        assert_eq!(
            verbatim_uia::probe_server_side_provider(hwnd.0 as isize),
            Some(true)
        );
    }
    // SAFETY: every element here was built with `cache`.
    let node = unsafe { snapshot_from_cached_element(element, registry) };
    let known = |id: NodeId| previous.iter().any(|known| known.id == id);
    // mockapp's window is read through UIA, so the walk crosses into no
    // other API.
    let read_by_other_api = |_: isize| false;
    // SAFETY: as above.
    let (chain, crossed, walked) = unsafe {
        uia.ancestor_chain(
            element,
            cache,
            registry,
            64,
            &AncestorStops {
                read_by_other_api: &read_by_other_api,
                known: &known,
                deadline: None,
            },
        )
    }
    .expect("the ancestor walk");
    assert_eq!(crossed, None);
    // The walk met the previous chain: reuse what lies above, as the
    // outpost's `splice` does.
    let ancestors = match walked {
        AncestorWalk::MetKnown(met) => {
            let index = previous
                .iter()
                .position(|known| known.id == met)
                .expect("the met ancestor is in the previous chain");
            previous[..index].iter().cloned().chain(chain).collect()
        }
        AncestorWalk::Complete => chain,
        AncestorWalk::OutOfTime => panic!("the walk has no deadline"),
    };
    let selected = (node.role == Role::List)
        // SAFETY: as above.
        .then(|| unsafe { uia.selected_child(element, cache, registry) }.expect("no error"))
        .flatten();
    (
        ancestors.into_iter().chain(std::iter::once(node)).collect(),
        selected,
    )
}

/// The calls the outpost's worker makes for one object-navigation step
/// from a UIA node it holds, in the same order: `resolve_uia_element` in
/// `verbatim-outpost`'s `read.rs` refreshes the kept element's cache, which
/// also proves it still answers, and `navigate` finds the node's nearest
/// window, for correcting the neighbor's backend, then takes the step. The
/// correction makes no call for a neighbor with no window of its own.
fn uia_navigate(
    under_test: &UiaUnderTest,
    element: &IUIAutomationElement,
    kind: QueryKind,
) -> NodeSnapshot {
    let UiaUnderTest {
        uia,
        cache,
        registry,
        ..
    } = under_test;
    // The outpost counts this read where it makes it.
    verbatim_uia::calls::count(CallKind::Uia);
    // SAFETY: `element` is live.
    let fresh = unsafe { element.BuildUpdatedCache(cache) }.expect("the element answers");
    assert!(verbatim_uia::nearest_window_handle(&fresh).is_some());
    // SAFETY: `fresh` was just built with `cache`.
    unsafe { uia.navigate(&fresh, cache, registry, kind) }
        .expect("the step")
        .expect("a neighbor")
}

#[expect(
    clippy::too_many_lines,
    reason = "one focus change after another, each with its expected cost"
)]
fn uia_focus_changes_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-focus");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let ((chain, _), cost) = under_test.measure(hwnd, |under_test| {
        uia_focus(under_test, hwnd, under_test.element("First"), &[], true)
    });
    let names: Vec<_> = chain.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(
        names,
        [
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First")
        ]
    );
    ratchet.check(
        "UIA focus, cold",
        &cost,
        calls(5, 0, 1),
        &[
            ("WM_GETOBJECT", 4),
            ("ProviderOptions", 44),
            ("GetPatternProvider", 18),
            ("GetPropertyValue", 51),
            ("HostRawElementProvider", 15),
            ("Navigate", 12),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 7),
        ],
    );

    let ((chain, _), cost) = under_test.measure(hwnd, |under_test| {
        uia_focus(
            under_test,
            hwnd,
            under_test.element("Second"),
            &chain,
            false,
        )
    });
    ratchet.check(
        "UIA focus, steady state",
        &cost,
        calls(2, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 9),
            ("GetPropertyValue", 29),
            ("HostRawElementProvider", 10),
            ("Navigate", 7),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 6),
        ],
    );

    let ((chain, selected), cost) = under_test.measure(hwnd, |under_test| {
        uia_focus(
            under_test,
            hwnd,
            under_test.element("Options"),
            &chain,
            false,
        )
    });
    assert_eq!(
        selected.and_then(|node| node.name).as_deref(),
        Some("One"),
        "the list's selected item"
    );
    ratchet.check(
        "UIA focus into a list",
        &cost,
        calls(5, 0, 0),
        &[
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 30),
            ("GetPatternProvider", 20),
            ("GetPropertyValue", 49),
            ("HostRawElementProvider", 14),
            ("Navigate", 7),
            ("GetRuntimeId", 4),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
            ("IsSelected", 1),
            ("GetSelection", 1),
        ],
    );

    let ((chain, _), _) = under_test.measure(hwnd, |under_test| {
        uia_focus(under_test, hwnd, under_test.element("One"), &chain, false)
    });
    let (_, cost) = under_test.measure(hwnd, |under_test| {
        uia_focus(under_test, hwnd, under_test.element("Two"), &chain, false)
    });
    ratchet.check(
        "UIA arrow to the next list item",
        &cost,
        calls(2, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 9),
            ("GetPropertyValue", 29),
            ("HostRawElementProvider", 10),
            ("Navigate", 7),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 6),
        ],
    );

    ratchet.finish();
    drop(app);
}

fn uia_navigation_steps_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-navigation");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let (second, cost) = under_test.measure(hwnd, |under_test| {
        uia_navigate(
            under_test,
            under_test.element("First"),
            QueryKind::NextSibling,
        )
    });
    assert_eq!(second.name.as_deref(), Some("Second"));
    ratchet.check(
        "UIA next sibling",
        &cost,
        calls(3, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 31),
            ("GetPatternProvider", 18),
            ("GetPropertyValue", 51),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );

    let (group, cost) = under_test.measure(hwnd, |under_test| {
        uia_navigate(under_test, under_test.element("Second"), QueryKind::Parent)
    });
    assert_eq!(group.name.as_deref(), Some("Settings"));
    ratchet.check(
        "UIA parent",
        &cost,
        calls(3, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 18),
            ("GetPropertyValue", 51),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );

    ratchet.finish();
    drop(app);
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
        (
            "msaa_focus_changes_cost_exactly",
            msaa_focus_changes_cost_exactly,
        ),
        (
            "msaa_navigation_steps_cost_exactly",
            msaa_navigation_steps_cost_exactly,
        ),
        (
            "uia_focus_changes_cost_exactly",
            uia_focus_changes_cost_exactly,
        ),
        (
            "uia_navigation_steps_cost_exactly",
            uia_navigation_steps_cost_exactly,
        ),
    ]);
}
