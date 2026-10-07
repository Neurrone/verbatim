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
//! same `verbatim-uia` and `verbatim-uia-rops` calls in the same order as
//! the outpost's worker does once it has the element in hand
//! ([`uia_focus`] and [`uia_navigate`] say which code each follows), and
//! read this thread's count. The outpost's one read of the focused element
//! is counted where the outpost makes it; its provider hits are the only
//! cost they leave out. A UIA focus is measured both ways the outpost reads
//! it: with remote operations, as it does by default, and with the classic
//! walk, as it does with `uia.remote_operations` off.
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
    CallCounts, CallKind, Fetches, NodeId, NodeSnapshot, NormalizedEvent, QueryKind, Role, TraceId,
    TreeNode,
};
use verbatim_model::{
    CaretWait, CaretWatch, PreviousSelection, TextMovement, TextOp, TextPoint, TextPosition,
    TextRead, TextReadAhead, TextReply, TextUnit, Theme,
};
use verbatim_outpost::Outpost;
use verbatim_outpost::dialog_text::{UiaObject, dialog_text};
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
    SupervisorToOutpost, read_message,
};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{Anchors, CaretSignal, TextSource, caret_report, perform};
use verbatim_uia::map::{snapshot_from_cached_element, with_legacy_checked_state};
use verbatim_uia::{
    AncestorStops, AncestorWalk, CACHED_PROPERTIES, ElementExt, FOCUS_PROPERTIES, NodeIdRegistry,
    Registration, Scope, Subscription, Uia,
};
use verbatim_uia_rops::{
    FocusAncestry, FocusQuery, NavigationDirection, Path, StepQuery, focus_ancestry,
    navigation_step,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, UIA_MenuOpenedEventId,
    UIA_SelectionItem_ElementSelectedEventId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
};
use windows::core::AgileReference;

/// The fixture's nodes, by their index in mockapp's tree (depth first, the
/// root at 0): mockapp answers `WM_GETOBJECT` for index `i` at object id
/// `i + 1`, the address an MSAA focus event names.
const FIRST: usize = 2;
const SECOND: usize = 3;
const LIST: usize = 4;
const ITEM_ONE: usize = 5;
const ITEM_TWO: usize = 6;

/// The dialog fixture's first button, by its index in mockapp's tree.
const YES: usize = 3;

/// The question the dialog fixture's message box asks.
const QUESTION: &str = "Remove the theme Mine? This cannot be undone.";

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

    /// [`msaa_focus`](Self::msaa_focus), returning the focus's ancestors
    /// too.
    fn msaa_focus_in_context(
        &self,
        hwnd: HWND,
        index: usize,
    ) -> (NodeSnapshot, Vec<NodeSnapshot>, CallCounts) {
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
                event:
                    NormalizedEvent::FocusChanged {
                        node, ancestors, ..
                    },
                timing,
                ..
            } => Some((node, ancestors, timing.calls)),
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
        calls(0, 30, 1),
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

/// The description the dialog among `ancestors` was reported with.
fn dialog_description(ancestors: &[NodeSnapshot]) -> Option<&str> {
    ancestors
        .iter()
        .find(|ancestor| ancestor.role == Role::Dialog)
        .expect("the dialog is an ancestor")
        .details
        .description
        .as_deref()
}

/// A focus entering a message box gathers its text
/// (`verbatim_outpost::dialog_text`): the dialog's children, each child's
/// role and states, and the question's name, value, and description, on top
/// of a cold focus's calls. A focus moving within it is not measured:
/// mockapp answers every `accParent` with a new COM object, so the dialog
/// reached from the next button is a new node to the outpost, where a real
/// dialog is the node it reported before and is not read again.
fn msaa_dialog_text_costs_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-counts-msaa-dialog");
    let mut app = common::spawn("dialog.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut ratchet = Ratchet::default();

    common::apply(&mut app, hwnd, "set-focus yes");
    let (node, ancestors, made) = outpost.msaa_focus_in_context(hwnd, YES);
    assert_eq!(node.name.as_deref(), Some("Yes"));
    assert_eq!(dialog_description(&ancestors), Some(QUESTION));
    let cost = Cost {
        calls: made,
        hits: common::read_hits(hwnd),
    };
    ratchet.check(
        "MSAA focus into a message box",
        &cost,
        calls(0, 44, 1),
        &[
            ("WM_GETOBJECT", 2),
            ("accParent", 17),
            ("accChildCount", 2),
            ("get_accChild", 4),
            ("get_accName", 4),
            ("get_accValue", 4),
            ("get_accDescription", 4),
            ("get_accRole", 7),
            ("get_accState", 6),
            ("get_accKeyboardShortcut", 3),
            ("accLocation", 3),
        ],
    );

    ratchet.finish();
    app.send("quit");
}

/// A UIA message box's text, gathered as the outpost's worker gathers it
/// for a dialog the focus newly entered (`describe_dialogs` in
/// `verbatim-outpost`'s `read.rs`): one call reads the dialog's children
/// with their properties cached.
fn uia_dialog_text_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-dialog");
    let mut app = common::spawn("dialog.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let dialog = under_test.element("Remove Theme").clone();
    let (text, cost) = under_test.measure(hwnd, |under_test| {
        dialog_text(&UiaObject::new(&under_test.uia, CACHED_PROPERTIES, dialog))
    });
    assert_eq!(text.as_deref(), Some(QUESTION));
    ratchet.check(
        "UIA message box text",
        &cost,
        calls(1, 0, 0),
        &[
            ("ProviderOptions", 29),
            ("GetPatternProvider", 44),
            ("GetPropertyValue", 92),
            ("HostRawElementProvider", 13),
            ("Navigate", 9),
            ("GetRuntimeId", 8),
            ("BoundingRectangle", 4),
            ("FragmentRoot", 9),
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

    // A theme reporting descriptions and shortcuts as off: the outpost no
    // longer asks for them, and saves their calls.
    outpost
        .outpost
        .handle_command(&SupervisorToOutpost::Fetches(Fetches {
            description: false,
            shortcut: false,
            ..Fetches::default()
        }));
    common::reset_hits(hwnd);
    let (second, calls_made) = outpost.navigate(first.id, QueryKind::NextSibling);
    assert_eq!(second.name.as_deref(), Some("Second"));
    assert_eq!(second.details.description, None);
    let cost = Cost {
        calls: calls_made,
        hits: common::read_hits(hwnd),
    };
    ratchet.check(
        "MSAA next sibling, descriptions and shortcuts off",
        &cost,
        calls(0, 8, 0),
        &[
            ("accParent", 1),
            ("get_accName", 1),
            ("get_accValue", 1),
            ("get_accRole", 1),
            ("get_accState", 1),
            ("accLocation", 1),
            ("accNavigate", 1),
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
        let (tree, _) = uia
            .walk_tree(&root, &cache, &registry, 8, 64)
            .expect("mockapp's tree");
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

/// The calls the outpost's worker makes for a UIA focus, in the same order:
/// `Worker::uia_focus` in `verbatim-outpost`'s `worker.rs` reads the
/// focused element (`GetFocusedElement`, which a test cannot make, so it is
/// counted here where the outpost makes it), then, with remote operations
/// on, `uia_remote_enrichment` in its `read.rs` reads the ancestors, the
/// selected child, and the nearest window in one `Execute`
/// (`verbatim_uia_rops::focus_ancestry`) and turns them into the chain
/// (`Uia::ancestor_chain_from`), reusing the previous chain from where it
/// meets it; with them off, the worker finds the nearest window of an
/// element with none of its own, and `uia_enrichment` and `uia_ancestors`
/// walk the ancestors hop by hop, stopping at one the previous focus's
/// chain holds, and read a list's selected child. The first time, the
/// window's provider is probed for arbitration. Returns the new focus's
/// chain, outermost first, ending with the focus, as the worker keeps it.
fn uia_focus(
    under_test: &UiaUnderTest,
    hwnd: HWND,
    element: &IUIAutomationElement,
    previous: &[NodeSnapshot],
    (first_in_window, remote): (bool, bool),
) -> (Vec<NodeSnapshot>, Option<NodeSnapshot>) {
    let UiaUnderTest {
        uia,
        cache,
        registry,
        ..
    } = under_test;
    // The outpost's read of the focused element.
    verbatim_uia::calls::count(CallKind::Uia);
    // A menu item without the Toggle pattern would read its legacy state
    // here; the fixture has none, so no call is made.
    let node = with_legacy_checked_state(element, snapshot_from_cached_element(element, registry));
    let known = |id: NodeId| previous.iter().any(|known| known.id == id);
    // mockapp's window is read through UIA, so the walk crosses into no
    // other API.
    let read_by_other_api = |_: isize| false;
    let stops = AncestorStops {
        read_by_other_api: &read_by_other_api,
        known: &known,
        deadline: None,
    };
    let (chain, crossed, walked, selected) = if remote {
        let (known_ids, known_runtime_ids): (Vec<NodeId>, Vec<Vec<i32>>) = previous
            .iter()
            .filter_map(|node| Some((node.id, registry.runtime_id_of(node.id)?)))
            .unzip();
        let query = FocusQuery {
            element,
            known: &known_runtime_ids,
            depth_limit: 64,
            properties: CACHED_PROPERTIES,
            deadline: None,
        };
        let (answer, path) = focus_ancestry(uia, &query, true).expect("the focus ancestry");
        assert!(matches!(path, Path::Remote), "answered by {path:?}");
        let FocusAncestry::Focused(ancestry) = answer else {
            panic!("the element has the keyboard focus");
        };
        assert_eq!(ancestry.window, Some(hwnd.0 as isize), "the nearest window");
        let (chain, crossed, mut walked) =
            Uia::ancestor_chain_from(&ancestry.ancestors, registry, &stops);
        if let (AncestorWalk::Complete, Some(index)) = (walked, ancestry.met_known) {
            walked = AncestorWalk::MetKnown(known_ids[index]);
        }
        let selected = ancestry
            .selected_child
            .map(|child| snapshot_from_cached_element(&child, registry));
        (chain, crossed, walked, selected)
    } else {
        assert_eq!(
            verbatim_uia::nearest_window_handle(element),
            Some(hwnd.0 as isize)
        );
        let (chain, crossed, walked) = uia
            .ancestor_chain(element, cache, registry, 64, &stops)
            .expect("the ancestor walk");
        let selected = (node.role == Role::List)
            .then(|| {
                uia.selected_child(element, cache, registry)
                    .expect("no error")
            })
            .flatten();
        (chain, crossed, walked, selected)
    };
    if first_in_window {
        assert_eq!(
            verbatim_uia::probe_server_side_provider(hwnd.0 as isize),
            Some(true)
        );
    }
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
    (
        ancestors.into_iter().chain(std::iter::once(node)).collect(),
        selected,
    )
}

/// The calls the outpost's worker makes for one object-navigation step
/// from a UIA node it holds, in the same order. With remote operations,
/// `uia_remote_step` in `verbatim-outpost`'s `read.rs` takes the step from
/// the kept element and finds its nearest window in one program
/// (`verbatim_uia_rops::navigation_step`). Classically, `resolve_uia_element`
/// refreshes the kept element's cache, which also proves it still answers,
/// and `navigate` finds the node's nearest window, for correcting the
/// neighbor's backend, then takes the step. The correction makes no call
/// for a neighbor with no window of its own.
fn uia_navigate(
    under_test: &UiaUnderTest,
    element: &IUIAutomationElement,
    kind: QueryKind,
    remote: bool,
) -> NodeSnapshot {
    let UiaUnderTest {
        uia,
        cache,
        registry,
        ..
    } = under_test;
    if remote {
        let direction = match kind {
            QueryKind::Parent => NavigationDirection::Parent,
            _ => NavigationDirection::NextSibling,
        };
        let properties = verbatim_uia::cached_properties(Theme::builtin_default().fetches());
        let (step, path) = navigation_step(
            uia,
            &StepQuery {
                element,
                direction,
                properties: &properties,
            },
            true,
        )
        .expect("the step");
        assert!(matches!(path, Path::Remote), "{path:?}");
        assert!(step.window.is_some());
        return snapshot_from_cached_element(&step.neighbor.expect("a neighbor"), registry);
    }
    let fresh = element
        .build_updated_cache(cache)
        .expect("the element answers");
    assert!(verbatim_uia::nearest_window_handle(&fresh).is_some());
    uia.navigate(&fresh, cache, registry, kind)
        .expect("the step")
        .expect("a neighbor")
}

/// What each UIA focus change in [`uia_focus_changes`] is expected to cost.
struct FocusCosts<'a> {
    cold: (CallCounts, &'a [(&'a str, u32)]),
    steady: (CallCounts, &'a [(&'a str, u32)]),
    into_list: (CallCounts, &'a [(&'a str, u32)]),
    next_item: (CallCounts, &'a [(&'a str, u32)]),
}

/// Moves mockapp's focus to `id` and measures the outpost's handling of a
/// focus on the element named `name`, the way `remote` says.
fn measure_uia_focus(
    (app, under_test, hwnd): (&mut common::MockApp, &UiaUnderTest, HWND),
    (id, name): (&str, &str),
    previous: &[NodeSnapshot],
    (first_in_window, remote): (bool, bool),
) -> ((Vec<NodeSnapshot>, Option<NodeSnapshot>), Cost) {
    common::apply(app, hwnd, &format!("set-focus {id}"));
    under_test.measure(hwnd, |under_test| {
        uia_focus(
            under_test,
            hwnd,
            under_test.element(name),
            previous,
            (first_in_window, remote),
        )
    })
}

/// The same focus changes, through the remote operation or the classic
/// walk as `remote` says, each checked against `expected`.
fn uia_focus_changes(remote: bool, expected: &FocusCosts<'_>) {
    let path = if remote { "remote" } else { "classic" };
    let title = common::unique_title(&format!("mockapp-counts-uia-focus-{path}"));
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();
    let check = |ratchet: &mut Ratchet,
                 label: &str,
                 cost: &Cost,
                 expected: &(CallCounts, &[(&str, u32)])| {
        ratchet.check(
            &format!("UIA focus ({path}), {label}"),
            cost,
            expected.0,
            expected.1,
        );
    };

    let ((chain, _), cost) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        ("first", "First"),
        &[],
        (true, remote),
    );
    let names: Vec<_> = chain.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(
        names,
        [
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("First")
        ]
    );
    check(&mut ratchet, "cold", &cost, &expected.cold);

    let ((chain, _), cost) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        ("second", "Second"),
        &chain,
        (false, remote),
    );
    let names: Vec<_> = chain.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(
        names,
        [
            Some("Mockapp Counts Fixture"),
            Some("Settings"),
            Some("Second")
        ]
    );
    check(&mut ratchet, "steady state", &cost, &expected.steady);

    let ((chain, selected), cost) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        ("list", "Options"),
        &chain,
        (false, remote),
    );
    assert_eq!(
        selected.and_then(|node| node.name).as_deref(),
        Some("One"),
        "the list's selected item"
    );
    check(&mut ratchet, "into a list", &cost, &expected.into_list);

    let ((chain, _), _) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        ("item1", "One"),
        &chain,
        (false, remote),
    );
    let ((chain, _), cost) = measure_uia_focus(
        (&mut app, &under_test, hwnd),
        ("item2", "Two"),
        &chain,
        (false, remote),
    );
    let names: Vec<_> = chain.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(
        names,
        [Some("Mockapp Counts Fixture"), Some("Options"), Some("Two")]
    );
    check(
        &mut ratchet,
        "arrow to the next list item",
        &cost,
        &expected.next_item,
    );

    ratchet.finish();
    drop(app);
}

fn uia_focus_changes_cost_exactly_remote() {
    uia_focus_changes(
        true,
        &FocusCosts {
            cold: (
                calls(2, 0, 1),
                &[
                    ("WM_GETOBJECT", 4),
                    ("ProviderOptions", 36),
                    ("GetPatternProvider", 22),
                    ("GetPropertyValue", 50),
                    ("HostRawElementProvider", 13),
                    ("Navigate", 11),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 4),
                ],
            ),
            steady: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 22),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 9),
                    ("Navigate", 7),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 4),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`.
            into_list: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 2),
                    ("ProviderOptions", 24),
                    ("GetPatternProvider", 23),
                    ("GetPropertyValue", 49),
                    ("HostRawElementProvider", 10),
                    ("Navigate", 8),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 3),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(2, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 22),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 9),
                    ("Navigate", 7),
                    ("GetRuntimeId", 4),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 4),
                ],
            ),
        },
    );
}

fn uia_focus_changes_cost_exactly_classic() {
    uia_focus_changes(
        false,
        &FocusCosts {
            cold: (
                calls(6, 0, 1),
                &[
                    ("WM_GETOBJECT", 4),
                    ("ProviderOptions", 44),
                    ("GetPatternProvider", 22),
                    ("GetPropertyValue", 51),
                    ("HostRawElementProvider", 15),
                    ("Navigate", 12),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 7),
                ],
            ),
            steady: (
                calls(3, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 27),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 10),
                    ("Navigate", 7),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 6),
                ],
            ),
            // The list's first selected item through `SelectionPattern2`'s
            // `FirstSelectedItem`, fetched with the `Selection` pattern in
            // one call, and its cache, one more (6 calls and 143 provider
            // calls through the `Selection` pattern alone, before).
            into_list: (
                calls(5, 0, 0),
                &[
                    ("WM_GETOBJECT", 2),
                    ("ProviderOptions", 34),
                    ("GetPatternProvider", 24),
                    ("GetPropertyValue", 49),
                    ("HostRawElementProvider", 14),
                    ("Navigate", 7),
                    ("GetRuntimeId", 5),
                    ("BoundingRectangle", 2),
                    ("FragmentRoot", 9),
                    ("IsSelected", 1),
                    ("FirstSelectedItem", 1),
                ],
            ),
            next_item: (
                calls(3, 0, 0),
                &[
                    ("WM_GETOBJECT", 1),
                    ("ProviderOptions", 27),
                    ("GetPatternProvider", 11),
                    ("GetPropertyValue", 29),
                    ("HostRawElementProvider", 10),
                    ("Navigate", 7),
                    ("GetRuntimeId", 3),
                    ("BoundingRectangle", 1),
                    ("FragmentRoot", 6),
                ],
            ),
        },
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "both ways' pinned provider hits, listed in full"
)]
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
            true,
        )
    });
    assert_eq!(second.name.as_deref(), Some("Second"));
    // One program: the step and the nearest window, the neighbor's cache
    // filled inside the provider; the kept element is not refreshed first.
    ratchet.check(
        "UIA next sibling, remotely",
        &cost,
        calls(1, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 26),
            ("GetPatternProvider", 11),
            ("GetPropertyValue", 28),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );
    let (group, cost) = under_test.measure(hwnd, |under_test| {
        uia_navigate(
            under_test,
            under_test.element("Second"),
            QueryKind::Parent,
            true,
        )
    });
    assert_eq!(group.name.as_deref(), Some("Settings"));
    ratchet.check(
        "UIA parent, remotely",
        &cost,
        calls(1, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 27),
            ("GetPatternProvider", 11),
            ("GetPropertyValue", 28),
            ("HostRawElementProvider", 9),
            ("Navigate", 9),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 1),
            ("FragmentRoot", 3),
        ],
    );

    let (second, cost) = under_test.measure(hwnd, |under_test| {
        uia_navigate(
            under_test,
            under_test.element("First"),
            QueryKind::NextSibling,
            false,
        )
    });
    assert_eq!(second.name.as_deref(), Some("Second"));
    ratchet.check(
        "UIA next sibling, classically",
        &cost,
        calls(3, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 31),
            ("GetPatternProvider", 22),
            ("GetPropertyValue", 51),
            ("HostRawElementProvider", 13),
            ("Navigate", 7),
            ("GetRuntimeId", 5),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 9),
        ],
    );

    let (group, cost) = under_test.measure(hwnd, |under_test| {
        uia_navigate(
            under_test,
            under_test.element("Second"),
            QueryKind::Parent,
            false,
        )
    });
    assert_eq!(group.name.as_deref(), Some("Settings"));
    ratchet.check(
        "UIA parent, classically",
        &cost,
        calls(3, 0, 0),
        &[
            ("WM_GETOBJECT", 1),
            ("ProviderOptions", 32),
            ("GetPatternProvider", 22),
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

/// What registering for events costs mockapp. A registration is made on its
/// own thread and is the same with remote operations on or off, so it
/// makes no counted call on this thread either way; what it costs is the
/// provider calls UIA makes while registering, pinned here: the focus
/// listener's desktop-wide group (an element selected, a menu opened, and
/// notifications), and an outpost's focus-following property subscription
/// moved to a focus, `First` inside the group `Settings`, registered on the
/// focus alone.
fn uia_event_registrations_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-registrations");
    let app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::new(
            vec![
                Subscription::Event {
                    event: UIA_SelectionItem_ElementSelectedEventId,
                    callback: Arc::new(|_| {}),
                },
                Subscription::Event {
                    event: UIA_MenuOpenedEventId,
                    callback: Arc::new(|_| {}),
                },
                Subscription::Notifications {
                    callback: Arc::new(|_, _, _, _, _| {}),
                },
            ],
            Scope::Desktop,
        )
        .expect("the listener's registration")
    });
    drop(registration);
    ratchet.check(
        "UIA desktop-wide registration, one group",
        &cost,
        calls(0, 0, 0),
        &[],
    );

    let focus = AgileReference::new(under_test.element("First")).expect("an agile reference");
    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::new(
            vec![Subscription::Properties {
                properties: FOCUS_PROPERTIES.to_vec(),
                callback: Arc::new(|_, _| {}),
            }],
            Scope::Elements(vec![focus]),
        )
        .expect("the focus-following registration")
    });
    drop(registration);
    ratchet.check(
        "UIA focus-following registration",
        &cost,
        calls(0, 0, 0),
        &[("HostRawElementProvider", 1), ("FragmentRoot", 2)],
    );

    ratchet.finish();
    drop(app);
}

/// A container's selected child, through `SelectionPattern2` (mockapp's
/// list has it) and, for a provider without it (mockapp's tab control),
/// through the `Selection` pattern, both ways the outpost reads it: inside
/// the focus's remote program, and classically. The calls of the focus
/// ancestry around it are those of the focus ledger above; these are the
/// selected child's alone classically, and the whole program remotely.
#[expect(
    clippy::too_many_lines,
    reason = "both containers' pinned provider hits, listed in full"
)]
fn uia_selected_children_cost_exactly() {
    let title = common::unique_title("mockapp-counts-uia-selection");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let cache = under_test
        .uia
        .cache_request(CACHED_PROPERTIES)
        .expect("a cache request");
    let mut ratchet = Ratchet::default();
    let selected_name = |selected: Option<IUIAutomationElement>| {
        let selected = selected.expect("a selected child");
        snapshot_from_cached_element(&selected, &under_test.registry).name
    };

    // The program's hits beyond the selected child's, the same for both.
    let program = |extra_pattern: u32, selection: (&'static str, u32)| {
        vec![
            ("WM_GETOBJECT", 2),
            ("ProviderOptions", 24),
            ("GetPatternProvider", 23 + extra_pattern),
            ("GetPropertyValue", 49),
            ("HostRawElementProvider", 10),
            ("Navigate", 8),
            ("GetRuntimeId", 3),
            ("BoundingRectangle", 2),
            ("FragmentRoot", 3),
            ("IsSelected", 1),
            selection,
        ]
    };
    for (id, name, selected, classic_calls, classic_hits, remote_hits) in [
        // `FirstSelectedItem`, cached in the call that also caches the
        // `Selection` pattern, and the item's cache.
        (
            "fruits",
            "Fruits",
            "Banana",
            2,
            vec![
                ("ProviderOptions", 14),
                ("GetPatternProvider", 13),
                ("GetPropertyValue", 23),
                ("HostRawElementProvider", 8),
                ("Navigate", 1),
                ("GetRuntimeId", 5),
                ("BoundingRectangle", 1),
                ("FragmentRoot", 7),
                ("IsSelected", 1),
                ("FirstSelectedItem", 1),
            ],
            program(0, ("FirstSelectedItem", 1)),
        ),
        // `FirstSelectedItem` answered "not supported" in the same call
        // that cached the `Selection` pattern, then its selection and the
        // item's cache: 3, as the `Selection` pattern alone took. The
        // program asks for `SelectionPattern2` once more.
        (
            "pages",
            "Pages",
            "General",
            3,
            vec![
                ("ProviderOptions", 14),
                ("GetPatternProvider", 14),
                ("GetPropertyValue", 23),
                ("HostRawElementProvider", 9),
                ("Navigate", 1),
                ("GetRuntimeId", 5),
                ("BoundingRectangle", 1),
                ("FragmentRoot", 8),
                ("IsSelected", 1),
                ("GetSelection", 1),
            ],
            program(1, ("GetSelection", 1)),
        ),
    ] {
        common::apply(&mut app, hwnd, &format!("set-focus {id}"));
        let container = under_test.element(name);
        let (child, cost) = under_test.measure(hwnd, |under_test| {
            under_test.uia.selected_element(container, &cache)
        });
        assert_eq!(selected_name(child).as_deref(), Some(selected));
        ratchet.check(
            &format!("UIA selected child of {name}, classically"),
            &cost,
            calls(classic_calls, 0, 0),
            &classic_hits,
        );
        let (ancestry, cost) = under_test.measure(hwnd, |under_test| {
            focus_ancestry(
                &under_test.uia,
                &FocusQuery {
                    element: container,
                    known: &[],
                    depth_limit: 64,
                    properties: CACHED_PROPERTIES,
                    deadline: None,
                },
                true,
            )
            .expect("the focus ancestry")
        });
        let FocusAncestry::Focused(ancestry) = ancestry.0 else {
            panic!("{name} has the focus");
        };
        assert_eq!(
            selected_name(ancestry.selected_child).as_deref(),
            Some(selected)
        );
        ratchet.check(
            &format!("UIA focus on {name} with its selected child, remotely"),
            &cost,
            calls(1, 0, 0),
            &remote_hits,
        );
    }

    ratchet.finish();
    app.send("quit");
}

/// The active text position changed event: registering for it, with a text
/// focus's caret and text changes, as one group on the focus, and handling
/// one, which keeps the event's range as a position in the focus's text and
/// reads nothing, the same with remote operations on or off.
fn uia_active_text_position_costs_exactly() {
    let title = common::unique_title("mockapp-counts-uia-active-position");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let under_test = UiaUnderTest::new(hwnd);
    let mut ratchet = Ratchet::default();

    let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&ranges);
    let notes = AgileReference::new(under_test.element("Notes")).expect("an agile reference");
    let (registration, cost) = under_test.measure(hwnd, |_| {
        Registration::new(
            vec![
                Subscription::Events {
                    events: vec![
                        UIA_Text_TextSelectionChangedEventId,
                        UIA_Text_TextChangedEventId,
                    ],
                    callback: Arc::new(|_, _| {}),
                },
                Subscription::ActiveTextPosition {
                    callback: Arc::new(move |_, range| {
                        if let Some(range) = range.and_then(|range| AgileReference::new(range).ok())
                        {
                            seen.lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(range);
                        }
                    }),
                },
            ],
            Scope::Elements(vec![notes]),
        )
        .expect("the text focus's registration")
    });
    ratchet.check(
        "UIA text focus registration",
        &cost,
        calls(0, 0, 0),
        &[("HostRawElementProvider", 1), ("FragmentRoot", 2)],
    );

    app.send("active-text-position doc 6 10");
    common::wait_until("the active text position changed event", || {
        !ranges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    });
    let range = ranges
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(0)
        .resolve()
        .expect("the event's range");
    let mut store = Anchors::new(Arc::default());
    let (_, cost) = under_test.measure(hwnd, |_| {
        let start = UiaPos::start_of(&range).expect("the range's start");
        store.node(1).position_at(start)
    });
    ratchet.check(
        "UIA active text position change",
        &cost,
        calls(0, 0, 0),
        &[],
    );

    drop(registration);
    ratchet.finish();
    app.send("quit");
}

/// A caret key's wait that never waits: the caret has already moved.
struct AlreadyMoved;

impl CaretSignal for AlreadyMoved {
    fn caret_event(&mut self) -> bool {
        false
    }

    fn wait(&mut self, _timeout: std::time::Duration) {
        panic!("the caret had already moved, so nothing should wait");
    }

    fn now(&mut self) -> Instant {
        Instant::now()
    }

    fn now_ms(&mut self) -> u64 {
        0
    }
}

/// One caret move answered, as the outpost's worker answers a caret key
/// with `text_reads::answer` once it has the node's text: the caret is
/// reported (Core's knowledge), mockapp's caret moves one character on, as
/// Right Arrow moves it, and the wait for evidence finds it at once and
/// reports the line and the character there. Then the caret moves on again
/// and is reported alone, as the worker reports it for a caret event, after
/// a typed character above all, with `text_reads::report_caret`. Returns
/// the calls and hits of the answer, and of the report.
fn measure_caret_move<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    take: fn() -> CallCounts,
) -> (Cost, Cost) {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 0");
    let (before, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 1");
    let _ = take();
    let reply = perform(
        source,
        &mut anchors,
        &TextOp::AwaitCaret(CaretWatch {
            pressed_at_ms: 0,
            since: Some(TextPosition {
                anchor: before.line.start,
                offset: before.line.offset,
            }),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: None,
            wait: CaretWait::Standard,
        }),
        &mut AlreadyMoved,
    );
    let calls = take();
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    assert!(reply.moved);
    assert_eq!(reply.unit.expect("the character").text, "l");
    let answer = Cost {
        calls,
        hits: common::read_hits(hwnd),
    };

    common::apply(app, hwnd, "caret doc 2");
    let _ = take();
    let (report, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    let calls = take();
    assert_eq!(report.line.offset, 2);
    let report = Cost {
        calls,
        hits: common::read_hits(hwnd),
    };
    (answer, report)
}

/// What a remote caret read costs the provider beyond its text calls: the
/// element's import and its text pattern.
const IMPORT_HITS: [(&str, u32); 5] = [
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
];

/// The hits of `times` remote caret reads that each made `text` calls.
fn remote_hits(times: u32, text: &[(&'static str, u32)]) -> Vec<(&'static str, u32)> {
    IMPORT_HITS
        .iter()
        .chain(text)
        .map(|&(name, count)| (name, count * times))
        .collect()
}

/// The report after a focus, whose line is spoken, with the line's
/// formatting: mockapp's first line has four stretches (bold "alpha", a
/// space, the misspelt "beta", the line feed).
fn measure_focus_report(hwnd: HWND, source: &mut UiaText) -> Cost {
    let mut store = Anchors::new(Arc::default());
    let _ = verbatim_uia::calls::take();
    common::reset_hits(hwnd);
    let (report, _) = caret_report(source, &mut store.node(1), &mut || 0, true).expect("the caret");
    assert_eq!(report.line.formats.len(), 4);
    Cost {
        calls: verbatim_uia::calls::take(),
        hits: common::read_hits(hwnd),
    }
}

/// A caret move, a caret report, the report after a focus with its
/// line's formatting, and a wait that finds nothing, through UIA, remotely
/// or classically, with the default theme's formatting (spelling and
/// grammar errors).
#[expect(
    clippy::too_many_lines,
    reason = "both ways' pinned provider hits, listed in full"
)]
fn check_uia_caret_costs(ratchet: &mut Ratchet, remote: bool) {
    let title = common::unique_title("mockapp-counts-uia-caret");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd).remote(remote);
    let (answer, report) =
        measure_caret_move(&mut app, hwnd, &mut source, verbatim_uia::calls::take);
    let (polls, waited) = measure_fruitless_wait(&mut app, hwnd, &mut source);
    assert_eq!(polls, 11, "every 10 milliseconds for 100");
    let focus_report = measure_focus_report(hwnd, &mut source);
    // The same report with every formatting indication on: seven attributes
    // per stretch, the annotation types, font name and size, weight,
    // italic, underline style, and color.
    let mut every_attribute = uia_notes(hwnd).remote(remote).fetches(Fetches::default());
    let formatted_report = measure_focus_report(hwnd, &mut every_attribute);
    // The caret's read, the evidence, the line and the caret's offset
    // in it, and the character's spelling error.
    let move_hits = [
        ("ITextProvider::GetSelection", 1),
        ("Clone", 3),
        ("CompareEndpoints", 2),
        ("ExpandToEnclosingUnit", 2),
        ("GetAttributeValue", 1),
        ("GetText", 2),
        ("MoveEndpointByRange", 1),
    ];
    let report_hits = [
        ("ITextProvider::GetSelection", 1),
        ("Clone", 2),
        ("CompareEndpoints", 1),
        ("ExpandToEnclosingUnit", 1),
        ("GetText", 2),
        ("MoveEndpointByRange", 1),
    ];
    // The report's, and the line walked by the format unit: four
    // stretches, each cut at the line's end, read, and its annotations
    // read.
    let focus_hits = [
        ("ITextProvider::GetSelection", 1),
        ("Clone", 7),
        ("CompareEndpoints", 9),
        ("ExpandToEnclosingUnit", 1),
        ("GetAttributeValue", 4),
        ("GetText", 6),
        ("MoveEndpointByUnit", 4),
        ("MoveEndpointByRange", 7),
    ];
    // The same, with seven attributes read for each of the four stretches.
    let formatted_hits: Vec<(&'static str, u32)> = focus_hits
        .iter()
        .map(|&(name, count)| match name {
            "GetAttributeValue" => (name, 28),
            _ => (name, count),
        })
        .collect();
    if remote {
        // One round trip each. Inside the provider the program also
        // copies the collapsed caret before using it, one clone and one
        // endpoint move more than the classic reads.
        let plus_copy = |hits: &[(&'static str, u32)]| -> Vec<(&'static str, u32)> {
            hits.iter()
                .map(|&(name, count)| match name {
                    "Clone" | "MoveEndpointByRange" => (name, count + 1),
                    _ => (name, count),
                })
                .collect()
        };
        ratchet.check(
            "UIA caret move, remotely",
            &answer,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&move_hits)),
        );
        ratchet.check(
            "UIA caret report, remotely",
            &report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&report_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus, remotely",
            &focus_report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&focus_hits)),
        );
        ratchet.check(
            "UIA caret report after a focus with every attribute, remotely",
            &formatted_report,
            calls(1, 0, 0),
            &remote_hits(1, &plus_copy(&formatted_hits)),
        );
        // One round trip per read, each the whole read: the read that
        // finds the evidence is the answer.
        ratchet.check(
            "UIA caret wait finding nothing, remotely",
            &waited,
            calls(11, 0, 0),
            &remote_hits(11, &plus_copy(&move_hits)),
        );
    } else {
        ratchet.check(
            "UIA caret move, classically",
            &answer,
            calls(12, 0, 0),
            &move_hits,
        );
        ratchet.check(
            "UIA caret report, classically",
            &report,
            calls(8, 0, 0),
            &report_hits,
        );
        ratchet.check(
            "UIA caret report after a focus, classically",
            &focus_report,
            calls(39, 0, 0),
            &focus_hits,
        );
        // One `GetAttributeValues` call per stretch for all seven
        // attributes (`IUIAutomationTextRange3`), which the provider answers
        // one attribute at a time; 63 calls when each attribute was a call.
        ratchet.check(
            "UIA caret report after a focus with every attribute, classically",
            &formatted_report,
            calls(39, 0, 0),
            &formatted_hits,
        );
        ratchet.check(
            "UIA caret wait finding nothing, classically",
            &waited,
            calls(132, 0, 0),
            &move_hits
                .iter()
                .map(|&(name, count)| (name, count * 11))
                .collect::<Vec<_>>(),
        );
    }
    app.send("quit");
}

/// One text request answered as the outpost's worker answers it
/// (`text_reads::answer`), with its calls and hits.
fn measure_text(
    hwnd: HWND,
    source: &mut UiaText,
    anchors: &mut verbatim_outpost::text::NodeText<'_, verbatim_outpost::text::uia::UiaPos>,
    op: &TextOp,
) -> (TextReply, Cost) {
    let _ = verbatim_uia::calls::take();
    common::reset_hits(hwnd);
    let reply = perform(source, anchors, op, &mut AlreadyMoved);
    (
        reply,
        Cost {
            calls: verbatim_uia::calls::take(),
            hits: common::read_hits(hwnd),
        },
    )
}

/// The text protocol's other requests through UIA, remotely or
/// classically: the review cursor's next line, a word from a position
/// inside the line (found by the text before it), say-all's read ahead of
/// every line, the selected text read for a copy, the caret moved as
/// say-all moves it, the caret's location, and a caret key's answer that
/// reports a selection's change.
#[expect(
    clippy::too_many_lines,
    reason = "each request measured in turn, as one session makes them"
)]
fn check_uia_text_costs(ratchet: &mut Ratchet, remote: bool) {
    let way = if remote { "remotely" } else { "classically" };
    let title = common::unique_title("mockapp-counts-uia-text");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd).remote(remote);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(&mut app, hwnd, "caret doc 0");
    let (report, _) = caret_report(&mut source, &mut anchors, &mut || 0, false).expect("the caret");
    let caret = TextPosition {
        anchor: report.line.start,
        offset: report.line.offset,
    };

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Read(TextRead {
            at: TextPoint::At(caret),
            movement: Some(TextMovement {
                unit: TextUnit::Line,
                count: 1,
            }),
            unit: TextUnit::Line,
        }),
    );
    let TextReply::Read { chunk: line, .. } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(line.text, "gamma\n");
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 3),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 1),
                ("Move", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(9, 0, 0),
            &[
                ("Clone", 2),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 1),
                ("Move", 1),
                ("MoveEndpointByRange", 2),
            ],
        )
    };
    ratchet.check(
        &format!("UIA review next line, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition {
                anchor: report.line.start,
                offset: 6,
            }),
            movement: None,
            unit: TextUnit::Word,
        }),
    );
    let TextReply::Read { chunk: word, .. } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(word.text, "beta");
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 4),
                ("ExpandToEnclosingUnit", 1),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByUnit", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(12, 0, 0),
            &[
                ("Clone", 3),
                ("ExpandToEnclosingUnit", 1),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByUnit", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    };
    ratchet.check(
        &format!("UIA review word inside the line, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::ReadAhead(TextReadAhead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
            count: 16,
        }),
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    assert_eq!(chunks.len(), 3);
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 8),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 3),
                ("GetText", 4),
                ("Move", 3),
                ("MoveEndpointByRange", 8),
            ],
        )
    } else {
        (
            calls(29, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("Clone", 7),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 3),
                ("GetText", 4),
                ("Move", 3),
                ("MoveEndpointByRange", 7),
            ],
        )
    };
    ratchet.check(
        &format!("UIA say-all read ahead, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::MoveCaret(TextPoint::At(TextPosition::at(chunks[1].start))),
    );
    assert_eq!(reply, TextReply::Done);
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("Clone", 2),
                ("CompareEndpoints", 1),
                ("MoveEndpointByRange", 2),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    } else {
        (
            calls(4, 0, 0),
            &[
                ("Clone", 1),
                ("CompareEndpoints", 1),
                ("MoveEndpointByRange", 1),
                ("ITextRangeProvider::Select", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA say-all caret move, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::Location(TextPoint::Caret),
    );
    assert_eq!(reply, TextReply::Location { x: 100, y: 216 });
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 3),
                ("GetPatternProvider", 1),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 2),
                ("Navigate", 1),
                ("FragmentRoot", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 2),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 1),
                ("GetBoundingRectangles", 1),
                ("MoveEndpointByRange", 1),
            ],
        )
    } else {
        (
            calls(5, 0, 0),
            &[
                ("ProviderOptions", 1),
                ("HostRawElementProvider", 1),
                ("FragmentRoot", 1),
                ("ITextProvider::GetSelection", 1),
                ("Clone", 1),
                ("CompareEndpoints", 1),
                ("ExpandToEnclosingUnit", 1),
                ("GetBoundingRectangles", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA caret location, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    common::apply(&mut app, hwnd, "caret doc 0 5");
    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::ReadRange {
            start: TextPoint::SelectionStart,
            end: TextPoint::SelectionEnd,
        },
    );
    assert_eq!(
        reply,
        TextReply::Range {
            text: "alpha".to_owned(),
            truncated: false
        }
    );
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 2),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 3),
                ("CompareEndpoints", 2),
                ("GetText", 1),
                ("MoveEndpointByRange", 3),
            ],
        )
    } else {
        (
            calls(7, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 1),
                ("CompareEndpoints", 2),
                ("GetText", 1),
                ("MoveEndpointByRange", 1),
            ],
        )
    };
    ratchet.check(
        &format!("UIA selected text, {way}"),
        &cost,
        expected.0,
        expected.1,
    );

    common::apply(&mut app, hwnd, "caret doc 0");
    let (collapsed, _) =
        caret_report(&mut source, &mut anchors, &mut || 0, false).expect("the caret");
    let at = TextPosition {
        anchor: collapsed.line.start,
        offset: collapsed.line.offset,
    };
    common::apply(&mut app, hwnd, "caret doc 0 5");
    let (reply, cost) = measure_text(
        hwnd,
        &mut source,
        &mut anchors,
        &TextOp::AwaitCaret(CaretWatch {
            pressed_at_ms: 0,
            since: Some(at),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: Some(PreviousSelection { start: at, end: at }),
            wait: CaretWait::Standard,
        }),
    );
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    assert_eq!(reply.selection_changes.len(), 1);
    let expected: (CallCounts, &[(&str, u32)]) = if remote {
        (
            calls(1, 0, 0),
            &[
                ("ProviderOptions", 2),
                ("GetPatternProvider", 2),
                ("GetPropertyValue", 1),
                ("HostRawElementProvider", 1),
                ("Navigate", 1),
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 5),
                ("CompareEndpoints", 8),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByRange", 4),
            ],
        )
    } else {
        (
            calls(23, 0, 0),
            &[
                ("ITextProvider::GetSelection", 1),
                ("GetCaretRange", 1),
                ("Clone", 4),
                ("CompareEndpoints", 8),
                ("ExpandToEnclosingUnit", 2),
                ("GetAttributeValue", 1),
                ("GetText", 3),
                ("MoveEndpointByRange", 3),
            ],
        )
    };
    ratchet.check(
        &format!("UIA caret key selecting, {way}"),
        &cost,
        expected.0,
        expected.1,
    );
    app.send("quit");
}

fn uia_text_requests_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for remote in [true, false] {
        check_uia_text_costs(&mut ratchet, remote);
    }
    ratchet.finish();
}

fn caret_moves_cost_exactly() {
    common::init_com();
    let mut ratchet = Ratchet::default();
    for remote in [true, false] {
        check_uia_caret_costs(&mut ratchet, remote);
    }

    let title = common::unique_title("mockapp-counts-edit-caret");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    let mut source = EditText::new(edit.0 as isize, 0);
    let (answer, report) =
        measure_caret_move(&mut app, hwnd, &mut source, verbatim_ia2::calls::take);
    ratchet.check("Edit control caret move", &answer, calls(0, 0, 5), &[]);
    ratchet.check("Edit control caret report", &report, calls(0, 0, 5), &[]);
    app.send("quit");

    ratchet.finish();
}

/// A caret key's wait whose caret never moves, on a clock that moves only
/// when the wait waits, counting the caret's reads.
struct NeverMoves {
    start: Instant,
    waited: std::time::Duration,
    reads: u32,
}

impl CaretSignal for NeverMoves {
    fn caret_event(&mut self) -> bool {
        false
    }

    fn wait(&mut self, timeout: std::time::Duration) {
        self.waited += timeout;
    }

    fn now(&mut self) -> Instant {
        self.start + self.waited
    }

    fn now_ms(&mut self) -> u64 {
        0
    }

    fn reading(&mut self) {
        self.reads += 1;
    }
}

/// A caret key's wait that finds no evidence: the caret stays where it was
/// reported, so the wait reads it every 10 milliseconds until its 100 run
/// out, then answers. Returns how many reads it made, and the calls and
/// hits of the whole wait and answer: one round trip per read remotely.
fn measure_fruitless_wait(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut UiaText,
) -> (u32, Cost) {
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    common::apply(app, hwnd, "caret doc 1");
    let (before, _) = caret_report(source, &mut anchors, &mut || 0, false).expect("the caret");
    let _ = verbatim_uia::calls::take();
    common::reset_hits(hwnd);
    let mut signal = NeverMoves {
        start: Instant::now(),
        waited: std::time::Duration::ZERO,
        reads: 0,
    };
    let reply = perform(
        source,
        &mut anchors,
        &TextOp::AwaitCaret(CaretWatch {
            pressed_at_ms: 0,
            since: Some(TextPosition {
                anchor: before.line.start,
                offset: before.line.offset,
            }),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: None,
            wait: CaretWait::Standard,
        }),
        &mut signal,
    );
    let calls = verbatim_uia::calls::take();
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    assert!(!reply.moved);
    (
        signal.reads,
        Cost {
            calls,
            hits: common::read_hits(hwnd),
        },
    )
}

/// The text of mockapp's "Notes" document through UIA, as the outpost's
/// worker builds it from the node's element and its text patterns, with
/// the default theme's formatting: spelling and grammar errors.
fn uia_notes(hwnd: HWND) -> UiaText {
    let under_test = UiaUnderTest::new(hwnd);
    let notes = under_test.element("Notes").clone();
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, pattern2, false).fetches(Theme::builtin_default().fetches())
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
        ("caret_moves_cost_exactly", caret_moves_cost_exactly),
        (
            "uia_text_requests_cost_exactly",
            uia_text_requests_cost_exactly,
        ),
        (
            "msaa_focus_changes_cost_exactly",
            msaa_focus_changes_cost_exactly,
        ),
        (
            "msaa_navigation_steps_cost_exactly",
            msaa_navigation_steps_cost_exactly,
        ),
        (
            "msaa_dialog_text_costs_exactly",
            msaa_dialog_text_costs_exactly,
        ),
        (
            "uia_dialog_text_costs_exactly",
            uia_dialog_text_costs_exactly,
        ),
        (
            "uia_focus_changes_cost_exactly_remote",
            uia_focus_changes_cost_exactly_remote,
        ),
        (
            "uia_focus_changes_cost_exactly_classic",
            uia_focus_changes_cost_exactly_classic,
        ),
        (
            "uia_navigation_steps_cost_exactly",
            uia_navigation_steps_cost_exactly,
        ),
        (
            "uia_event_registrations_cost_exactly",
            uia_event_registrations_cost_exactly,
        ),
        (
            "uia_selected_children_cost_exactly",
            uia_selected_children_cost_exactly,
        ),
        (
            "uia_active_text_position_costs_exactly",
            uia_active_text_position_costs_exactly,
        ),
    ]);
}
