//! Core's requests, answered on the worker
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
//! build", "The focus answers" and Table 14), each answered once with one
//! of the five outcomes: a batch (`super::batch`), or one of the
//! terminal's three text operations. What a read reads is answered by its
//! property set, each node with its nearest window's facts, read from
//! Windows with no call into the application. The reads a batch's steps
//! share are here: a node's properties as its snapshot holds them, its
//! children, the ancestors' nearest windows, and the answer a focus read
//! is made into.
//!
//! A request whose window has no verdict in force has the window probed
//! first, and the answer carries the probe.

#![forbid(unsafe_code)]

use verbatim_model::property::{
    NodeDetailSet, PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, FocusAncestor, FocusCaret, FocusNode, FocusRead, HeldIds, KnownAncestor, MsaaIds,
    NodeRead, NodeWindow, Probe, Question, RouteEnd, WindowVerdict,
};
use verbatim_model::{
    Backend, NodeDetails, NodeId, NodeSnapshot, Pid, QueryKind, Role, State, StateSet,
};

use super::RequestContext;
use super::read::{self, Client, ReadError};
use super::window::{hwnd_of, window_handle};

/// How many children a read reads of one node at most: a bound on a read
/// into a list with no end, as an application can give.
const MAX_NODES: usize = 1000;

/// The answer to `ask`, read on the worker.
pub(super) fn answer(context: &RequestContext<'_>, client: &mut Client, ask: &Ask) -> Answer {
    let mut probes = Vec::new();
    // Core's verdict for the request's window is the one its reads judge
    // that window by: the outpost probes it again for none of them.
    let given = match ask.verdict {
        WindowVerdict::Uia => Some(true),
        WindowVerdict::Msaa => Some(false),
        WindowVerdict::ProbeFirst => None,
    };
    context
        .given_verdict
        .set(given.map(|uia| (hwnd_of(ask.window), uia)));
    let outcome = match &ask.request {
        Question::Batch(batch) => super::batch::answer(context, client, ask, batch, &mut probes),
    };
    Answer {
        number: ask.number,
        outcome,
        probes,
        detail: String::new(),
    }
}

/// The node details `request`'s reads fetch: those its batch's property
/// sets name.
pub(super) fn node_details(request: &Question) -> NodeDetailSet {
    match request {
        Question::Batch(batch) => super::batch::node_details(batch),
    }
}

/// Whether window `hwnd` is read through UI Automation, by `verdict`, or,
/// with none in force, by a probe, which `probes` records, begun at now on
/// the clock the outpost stamps its observations with, the run's shared
/// clock but in a test that sets its own: `None` for a window the
/// outpost's application does not serve, which is neither probed nor
/// read.
pub(super) fn verdict(
    context: &RequestContext<'_>,
    hwnd: isize,
    verdict: WindowVerdict,
    probes: &mut Vec<Probe>,
) -> Option<bool> {
    match verdict {
        WindowVerdict::Uia => Some(true),
        WindowVerdict::Msaa => Some(false),
        WindowVerdict::ProbeFirst => {
            if !read::serves(context, hwnd) {
                return None;
            }
            // Core asks for a probe: the window is probed whatever the
            // class lists say, and the probe is reported, Core's cache
            // being the one that keeps a verdict past its request.
            let uia = crate::arbitration::probed_uses_uia(
                &context.arbitrator,
                (&context.verdicts, context.observed_now()),
                hwnd,
                || verbatim_uia::console_reports_formatting(hwnd),
            );
            if let Some(probed) = context.verdicts.probed(hwnd)
                && !probes
                    .iter()
                    .any(|probe| probe.window == window_handle(hwnd))
            {
                probes.push(Probe {
                    window: window_handle(hwnd),
                    thread: verbatim_win32::window::window_owner(hwnd).0,
                    provider: uia,
                    began_us: probed.began_us,
                });
            }
            Some(uia)
        }
    }
}

/// The facts of `window` a node read in it carries, read from Windows with
/// no call into the application.
pub(super) fn node_window(window: isize) -> NodeWindow {
    let (thread, process) = verbatim_win32::window::window_owner(window);
    NodeWindow {
        handle: window_handle(window),
        top_level: window_handle(verbatim_win32::window::top_level_of(window)),
        root_owner: window_handle(verbatim_win32::window::root_owner_of(window)),
        process: Pid(process),
        thread,
        desktop: window == verbatim_win32::window::desktop_window(),
    }
}

/// The nearest window of `node` known without a call: its own window, the
/// window its MSAA object was read at, the one recorded when it was read,
/// or a window's element's runtime id.
fn known_nearest(context: &RequestContext<'_>, node: &NodeSnapshot) -> Option<isize> {
    node.details
        .window
        .map(hwnd_of)
        .or_else(|| known_nearest_of(context, node.id))
        .filter(|&hwnd| hwnd != 0)
}

/// `node` as an answer carries it: its id, its nearest window's facts, the
/// window known without a call or else `nearest`, and its properties of
/// `set`.
pub(super) fn node_read(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    nearest: Option<isize>,
    set: &[PropertyId],
) -> NodeRead {
    let window = known_nearest(context, node).or(nearest).unwrap_or(0);
    NodeRead {
        node: Some(node.id),
        window: node_window(window),
        values: values(context, node, set),
    }
}

/// `node`'s properties of `set`, as its snapshot holds them: a property
/// the snapshot has no value for answers none.
pub(super) fn values(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    set: &[PropertyId],
) -> PropertyValues {
    let details = &node.details;
    let text = |text: &Option<String>| text.clone().map(PropertyValue::Text);
    PropertyValues(
        set.iter()
            .map(|&id| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::Name => text(&node.name),
                    PropertyId::Role => Some(PropertyValue::Role(node.role)),
                    PropertyId::Value => text(&node.value),
                    PropertyId::States => Some(PropertyValue::States(node.states)),
                    PropertyId::Description => text(&details.description),
                    PropertyId::KeyboardShortcut => text(&details.keyboard_shortcut),
                    PropertyId::PositionInSet => details.position_in_set.map(PropertyValue::Count),
                    PropertyId::SetSize => details.set_size.map(PropertyValue::Count),
                    PropertyId::Level => details.level.map(PropertyValue::Count),
                    PropertyId::Rect => details.rect.map(PropertyValue::Rect),
                    PropertyId::ClassName => text(&details.class_name),
                    PropertyId::NativeWindow => details.window.map(PropertyValue::Window),
                    PropertyId::Process => Some(PropertyValue::Process(Pid(context.target_pid()))),
                    PropertyId::RuntimeId => context
                        .store
                        .runtime_id_of(node.id)
                        .map(|runtime_id| PropertyValue::RuntimeId(RuntimeId(runtime_id))),
                    PropertyId::HasKeyboardFocus => {
                        Some(PropertyValue::Flag(node.states.contains(State::Focused)))
                    }
                    PropertyId::Cell => details.cell.map(PropertyValue::Cell),
                    PropertyId::TableCounts => details.table_counts.map(PropertyValue::TableCounts),
                    // Not in a snapshot: read by the listener, by the text
                    // requests, or apart ([`read_apart`]); the four a
                    // batch's operations read, which the outpost runs from
                    // the readers' step of the request list's build, and no
                    // kind of request names.
                    PropertyId::AutomationId
                    | PropertyId::HasTextPattern
                    | PropertyId::RangeValue
                    | PropertyId::TextSummary
                    | PropertyId::ValueFromSelection
                    | PropertyId::Suggestions
                    | PropertyId::HasValuePattern
                    | PropertyId::WindowClass
                    | PropertyId::ComboBoxList
                    | PropertyId::ParentAutomationId
                    | PropertyId::ChildCount
                    | PropertyId::FocusedWithin
                    | PropertyId::MsaaAddress
                    | PropertyId::WindowShown
                    | PropertyId::IsContent => None,
                },
            })
            .collect(),
    )
}

/// The snapshot `values` describe of node `id`, read through `backend`:
/// what the listener read of an element, for the completion to keep.
pub(super) fn snapshot_of(id: NodeId, backend: Backend, values: &PropertyValues) -> NodeSnapshot {
    let value = |id| values.answer(id).and_then(|answer| answer.value.as_ref());
    let text = |id| match value(id) {
        Some(PropertyValue::Text(text)) => Some(text.clone()),
        _ => None,
    };
    let count = |id| match value(id) {
        Some(PropertyValue::Count(count)) => Some(*count),
        _ => None,
    };
    NodeSnapshot {
        id,
        backend,
        role: match value(PropertyId::Role) {
            Some(PropertyValue::Role(role)) => *role,
            _ => Role::Unknown,
        },
        name: text(PropertyId::Name),
        value: text(PropertyId::Value),
        states: match value(PropertyId::States) {
            Some(PropertyValue::States(states)) => *states,
            _ => StateSet::default(),
        },
        details: NodeDetails {
            description: text(PropertyId::Description),
            keyboard_shortcut: text(PropertyId::KeyboardShortcut),
            position_in_set: count(PropertyId::PositionInSet),
            set_size: count(PropertyId::SetSize),
            level: count(PropertyId::Level),
            rect: match value(PropertyId::Rect) {
                Some(PropertyValue::Rect(rect)) => Some(*rect),
                _ => None,
            },
            class_name: text(PropertyId::ClassName),
            cell: match value(PropertyId::Cell) {
                Some(PropertyValue::Cell(cell)) => Some(*cell),
                _ => None,
            },
            table_counts: match value(PropertyId::TableCounts) {
                Some(PropertyValue::TableCounts(counts)) => Some(*counts),
                _ => None,
            },
            window: match value(PropertyId::NativeWindow) {
                Some(PropertyValue::Window(window)) if window.0 != 0 => Some(*window),
                _ => None,
            },
            ..NodeDetails::default()
        },
    }
}

/// The children of `node`, a node whose children MSAA does not list (a
/// window's own object, a tree view's item, or a node read through UI
/// Automation), in order, at most [`MAX_NODES`], and whether they are all
/// of them, for a walk down the tree from an MSAA object: reached by
/// navigation, the step to the first child and each step on to the next
/// sibling taken on the backend of the node it goes from, where a child
/// gone as the step on from it is read ends the list there, incomplete.
pub(super) fn children(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
) -> Result<(Vec<NodeSnapshot>, bool), ReadError> {
    let mut children = Vec::new();
    let mut next = read::navigate(context, client, node, QueryKind::FirstChild)?;
    while let Some(child) = next {
        if children.len() >= MAX_NODES {
            break;
        }
        next = match read::navigate(context, client, child.id, QueryKind::NextSibling) {
            Ok(next) => next,
            Err(ReadError::Gone) => {
                children.push(child);
                return Ok((children, false));
            }
            Err(error) => return Err(error),
        };
        children.push(child);
    }
    Ok((children, true))
}

/// The nearest window of node `node` known without a call, by its id.
pub(super) fn known_nearest_of(context: &RequestContext<'_>, node: NodeId) -> Option<isize> {
    context
        .store
        .key_of(node)
        .map(|(hwnd, _, _)| hwnd)
        .or_else(|| context.store.nearest_window_of(node))
        .or_else(|| match context.store.runtime_id_of(node).as_deref() {
            Some([42, hwnd]) => isize::try_from(*hwnd).ok(),
            _ => None,
        })
        .filter(|&hwnd| hwnd != 0)
}

/// `ancestors`, nearest first, as an answer carries them, each with its
/// nearest window: its own, or the next one above it, or above the
/// outermost that is a window, `nearest`.
pub(super) fn ancestor_reads(
    context: &RequestContext<'_>,
    ancestors: &[NodeSnapshot],
    nearest: Option<isize>,
    set: &[PropertyId],
) -> Vec<NodeRead> {
    let windows = nearest_windows(context, ancestors, nearest);
    ancestors
        .iter()
        .zip(windows)
        .map(|(ancestor, window)| node_read(context, ancestor, window, set))
        .collect()
}

/// The nearest window of each of `ancestors`, nearest first: its own, or
/// that of the next ancestor above it with one, or the parent window of
/// the outermost ancestor that is a window, else `nearest`.
fn nearest_windows(
    context: &RequestContext<'_>,
    ancestors: &[NodeSnapshot],
    nearest: Option<isize>,
) -> Vec<Option<isize>> {
    let mut windows = vec![None; ancestors.len()];
    let mut above = nearest;
    for (place, ancestor) in ancestors.iter().enumerate().rev() {
        let own = known_nearest(context, ancestor);
        if let Some(slot) = windows.get_mut(place) {
            *slot = own.or(above);
        }
        if own.is_some() {
            above = own;
        }
    }
    windows
}

/// Whether `node` is the known ancestor `known`.
pub(super) fn is_known(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    known: &KnownAncestor,
) -> bool {
    match known {
        KnownAncestor::Node(id) => node.id == *id,
        KnownAncestor::Window(window) => node.details.window == Some(*window),
        KnownAncestor::Uia(runtime_id) => context
            .store
            .runtime_id_of(node.id)
            .is_some_and(|own| own == runtime_id.0),
    }
}

/// `chain`, outermost first, as an answer carries it: nearest first, up to
/// the first ancestor among `known`, which is left out, with its place in
/// `known`; the whole chain when it meets none.
pub(super) fn cut_at_known(
    context: &RequestContext<'_>,
    chain: Vec<NodeSnapshot>,
    known: &[KnownAncestor],
) -> (Vec<NodeSnapshot>, Option<u32>) {
    let mut nearest_first: Vec<NodeSnapshot> = chain.into_iter().rev().collect();
    let met = nearest_first.iter().enumerate().find_map(|(at, node)| {
        known
            .iter()
            .position(|known| is_known(context, node, known))
            .map(|place| (at, place))
    });
    match met {
        Some((at, place)) => {
            nearest_first.truncate(at);
            (nearest_first, u32::try_from(place).ok())
        }
        None => (nearest_first, None),
    }
}

/// The kept nodes of `stops`, for NVDA's equality to compare what a walk
/// reads with.
pub(super) fn candidates_of(known: &[KnownAncestor]) -> verbatim_ia2::equality::Candidates {
    verbatim_ia2::equality::Candidates::new(vec![
        known
            .iter()
            .filter_map(|known| match known {
                KnownAncestor::Node(id) => Some(*id),
                KnownAncestor::Uia(_) | KnownAncestor::Window(_) => None,
            })
            .collect(),
    ])
}

/// A focus answer for `node`, read through `backend`, with `ancestors`
/// nearest first and the known ancestor `met`, its nearest window
/// `nearest`, its MSAA ids `msaa` and the foreground window it entered
/// under.
pub(super) struct Focus<'a> {
    pub(super) node: &'a NodeSnapshot,
    pub(super) ancestors: &'a [NodeSnapshot],
    pub(super) met: Option<u32>,
    pub(super) nearest: Option<isize>,
    pub(super) msaa: Option<HeldIds>,
    pub(super) foreground: Option<&'a NodeSnapshot>,
    pub(super) value_from_selection: bool,
    pub(super) runtime_id_reused: bool,
    /// The focus's caret, read by its executor's arm.
    pub(super) caret: FocusCaret,
}

/// The focus answer `focus` describes, by `set`.
pub(super) fn focus_read(
    context: &RequestContext<'_>,
    focus: Focus<'_>,
    set: &[PropertyId],
) -> FocusRead {
    let runtime_id = |node: &NodeSnapshot| context.store.runtime_id_of(node.id).map(RuntimeId);
    let focus_node = |node: &NodeSnapshot, nearest: Option<isize>| FocusNode {
        read: node_read(context, node, nearest, set),
        runtime_id: runtime_id(node),
    };
    let nearest = focus.nearest.or_else(|| known_nearest(context, focus.node));
    let windows = nearest_windows(context, focus.ancestors, nearest);
    let ancestors = focus
        .ancestors
        .iter()
        .zip(windows)
        .map(|(ancestor, window)| FocusAncestor {
            node: focus_node(ancestor, window),
            simple_child: context
                .store
                .key_of(ancestor.id)
                .is_some_and(|(_, _, child)| child != verbatim_ia2::CHILDID_SELF),
            uia_layout: ancestor.details.uia_layout,
            described: ancestor.details.described,
        })
        .collect();
    FocusRead {
        focus: focus_node(focus.node, nearest),
        msaa: focus.msaa,
        ancestors,
        met: focus.met,
        foreground: focus
            .foreground
            .map(|window| focus_node(window, window.details.window.map(hwnd_of))),
        value_from_selection: focus.value_from_selection,
        runtime_id_reused: focus.runtime_id_reused,
        caret: focus.caret,
    }
}

/// The MSAA ids `node` was read by, by the key it is kept under.
pub(super) fn held_ids(context: &RequestContext<'_>, node: &NodeSnapshot) -> Option<HeldIds> {
    let (window, object, child) = context.store.key_of(node.id)?;
    Some(HeldIds {
        event: MsaaIds {
            window: window_handle(window),
            object,
            child,
        },
        held_child: child,
    })
}

/// The route's end as the outpost's continuation above it takes it.
pub(super) fn ancestors_end(end: &RouteEnd) -> super::read::AncestorsEnd {
    use super::read::AncestorsEnd;
    match end {
        RouteEnd::Known(_) | RouteEnd::ProcessBoundary | RouteEnd::Root => AncestorsEnd::Top,
        RouteEnd::ContinueFrom(window) => AncestorsEnd::ContinueFrom(hwnd_of(*window)),
        RouteEnd::PlainFrom(window) => AncestorsEnd::PlainFrom(hwnd_of(*window)),
    }
}
