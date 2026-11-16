//! Core's requests, answered on the worker
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
//! build", "The focus answers" and Table 14): each request is one program,
//! answered once with one of the five outcomes. The reads are the ones the
//! outpost has always made; what they read is answered by the request's
//! property set, each node with its nearest window's facts, read from
//! Windows with no call into the application.
//!
//! - The shared list: a node's properties, one navigation step, a find, a
//!   node's children, a text request and an activation; and, from outside
//!   the event queue, the control plane's tree dump and the taskbar's two
//!   reads for the notification area list.
//! - The generic read of the object an event names, for an MSAA object at
//!   an event's address or a node the outpost holds: its properties, its
//!   relation to nodes Core names, and its ancestors up to one Core
//!   already knows.
//! - An MSAA run, newest first, its candidates tried as NVDA's pump tries
//!   them: a window read through UI Automation skipped, a focus event
//!   without the focused state refused, and the first valid focus read.
//! - The application's current focus.
//! - The completion of a UI Automation focus from the listener's route:
//!   the outpost finds its own copy of the element, adds the node's event
//!   registration on the registering thread and waits for it, then reads
//!   the node's states, as NVDA adds a focus's event handler group before
//!   it reads the focus's states.
//!
//! A request whose window has no verdict in force has the window probed
//! first, and the answer carries the probe.

#![forbid(unsafe_code)]

use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    Answer, Ask, FindScope, FocusAncestor, FocusCaret, FocusNode, FocusRead, HeldIds,
    KnownAncestor, MsaaIds, MsaaRun, NodeRead, NodeWindow, ObjectClass, ObjectRead, ObjectRef,
    Outcome, Probe, Purpose, ReadObject, Refusal, RelationFound, RelationQuery, Reply, Request,
    RouteEnd, RunRead, Tried, WindowVerdict,
};
use verbatim_model::{
    Backend, NodeDetails, NodeId, NodeSnapshot, Pid, QueryKind, Role, State, StateSet, TextAnchor,
    TextOp, TextReply,
};

use super::read::{self, Client, ReadError};
use super::text_reads;
use super::window::{hwnd_of, window_handle};
use super::{Context, RequestContext};

/// How many children a request reads of one node at most, and how many
/// nodes a find of descendants visits: a bound on a request into a list
/// with no end, as an application can give.
const MAX_NODES: usize = 1000;

/// The answer to `ask`, read on the worker.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per request of the shared list"
)]
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
    // A request that carries "probe first" and whose handling consults no
    // verdict of its own still answers its window's probe, so that Core
    // records the verdict rather than count the probe timed out.
    if !consults_verdict(&ask.request) {
        let _ = verdict(context, hwnd_of(ask.window), ask.verdict, &mut probes);
    }
    let outcome = match &ask.request {
        Request::ReadObject(read) => read_object(context, client, ask, read, &mut probes),
        Request::Properties { node, set } => match properties(context, client, *node, set) {
            Ok(values) => Outcome::Result(Reply::Properties(values)),
            Err(error) => failed(&error),
        },
        Request::Navigate {
            node,
            direction,
            set,
        } => match read::navigate(context, client, *node, *direction) {
            Ok(reached) => Outcome::Result(Reply::Navigated(
                reached.map(|reached| node_read(context, &reached, None, set)),
            )),
            Err(error) => failed(&error),
        },
        Request::Find {
            node,
            scope,
            property,
            value,
            set,
        } => match find(context, client, *node, *scope, (*property, value)) {
            Ok(found) => Outcome::Result(Reply::Found(
                found.map(|found| node_read(context, &found, None, set)),
            )),
            Err(error) => failed(&error),
        },
        Request::Children { node, set } => match children(context, client, *node) {
            Ok(children) => Outcome::Result(Reply::Children(
                children
                    .iter()
                    .map(|child| node_read(context, child, None, set))
                    .collect(),
            )),
            Err(error) => failed(&error),
        },
        Request::Text { node, op } => match op {
            TextOp::ReadTerminal(_)
            | TextOp::TerminalHistory { .. }
            | TextOp::TerminalFind { .. } => terminal(context, client, *node, op),
            _ => Outcome::Result(Reply::Text(text_reads::answer_asked(context, *node, op))),
        },
        Request::Activate { node } => match read::activate(context, client, *node) {
            Ok(action) => Outcome::Result(Reply::Activated(action)),
            Err(error) => failed(&error),
        },
        Request::MsaaRun(run) => msaa_run(context, client, run, &mut probes),
        Request::CurrentFocus {
            set,
            known,
            baseline,
        } => current_focus(context, client, set, (known, *baseline)),
        Request::CompleteUia(complete) => {
            match verdict(
                context,
                hwnd_of(complete.route.window),
                ask.verdict,
                &mut probes,
            ) {
                Some(true) => super::completion::complete(context, client, complete),
                Some(false) => Outcome::Refused(Refusal::ReadThroughMsaa),
                None => Outcome::NotThere,
            }
        }
        Request::Follow(follow) => super::followed::follow(context, client, follow),
        Request::DumpTree { locations } => match read::dump_tree(context, client, *locations) {
            Ok(dumped) => Outcome::Result(Reply::Tree(Box::new(dumped))),
            Err(error) => failed(&error),
        },
        Request::ShellItems { kind } => {
            match super::shell_items::shell_items(context, client, *kind) {
                Ok(items) => Outcome::Result(Reply::ShellItems(items)),
                Err(error) => failed(&error),
            }
        }
        Request::ShellItemPosition { kind, item } => {
            match super::shell_items::shell_item_position(context, client, (*kind, item)) {
                Ok(found) => Outcome::Result(Reply::ShellItemPosition(found)),
                Err(error) => failed(&error),
            }
        }
    };
    Answer {
        number: ask.number,
        outcome,
        probes,
        detail: String::new(),
    }
}

/// A terminal's read, history read or search Core asks
/// (`docs/design/readers-tell-core-asks-build3.md`): a terminal that is
/// gone answers "not there", which ends Core's model of it, and any other
/// read that brings nothing answers its reply, which Core takes as failed.
fn terminal(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
    op: &TextOp,
) -> Outcome {
    let Some(uia) = client.uia() else {
        return Outcome::Result(Reply::Text(TextReply::Unanswered));
    };
    let reply = match op {
        TextOp::ReadTerminal(read) => text_reads::terminal_read(context, uia, node, read),
        TextOp::TerminalHistory {
            above,
            whole,
            start,
            height,
        } => text_reads::terminal_history(context, uia, node, (*start, *above, *whole, *height)),
        TextOp::TerminalFind {
            needle,
            from,
            rows,
            start,
        } => text_reads::terminal_find(context, uia, node, *start, (needle, *from, *rows)),
        _ => TextReply::Unsupported,
    };
    match reply {
        TextReply::Gone => Outcome::NotThere,
        reply => Outcome::Result(Reply::Text(reply)),
    }
}

/// Whether `request`'s handling consults its window's verdict itself,
/// probing when it carries "probe first": a completion, an MSAA run by
/// each candidate's, and a read of an MSAA object.
fn consults_verdict(request: &Request) -> bool {
    matches!(
        request,
        Request::CompleteUia(_)
            | Request::MsaaRun(_)
            | Request::ReadObject(ReadObject {
                object: ObjectRef::Msaa { .. },
                ..
            })
    )
}

/// The outcome of a read that failed: not there for a node gone, else
/// ended, its reason logged.
fn failed(error: &ReadError) -> Outcome {
    match error {
        ReadError::Gone => Outcome::NotThere,
        ReadError::Failed(reason) => {
            tracing::debug!(reason, "a request's read failed");
            Outcome::Ended
        }
    }
}

/// Whether window `hwnd` is read through UI Automation, by `verdict`, or,
/// with none in force, by a probe, which `probes` records, begun at now on
/// the clock the outpost stamps its observations with, the run's shared
/// clock but in a test that sets its own: `None` for a window the
/// outpost's application does not serve, which is neither probed nor
/// read.
fn verdict(
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
            let began_us = context.observed_now();
            let uia = read::window_uses_uia(context, hwnd);
            probes.push(Probe {
                window: window_handle(hwnd),
                thread: verbatim_win32::window::window_owner(hwnd).0,
                provider: uia,
                began_us,
            });
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
        .or_else(|| context.store.key_of(node.id).map(|(hwnd, _, _)| hwnd))
        .or_else(|| context.store.nearest_window_of(node.id))
        .or_else(|| match context.store.runtime_id_of(node.id).as_deref() {
            Some([42, hwnd]) => isize::try_from(*hwnd).ok(),
            _ => None,
        })
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
                    // requests, or apart ([`read_apart`]).
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
                    | PropertyId::ChildCount => None,
                },
            })
            .collect(),
    )
}

/// Whether property `id` is read apart from a node's snapshot, by
/// [`read_apart`].
fn is_read_apart(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::Suggestions
            | PropertyId::HasValuePattern
            | PropertyId::WindowClass
            | PropertyId::ComboBoxList
            | PropertyId::ChildCount
    )
}

/// The properties of `set` of node `node_id`: its snapshot, read only when
/// the set names a property the snapshot holds, and each property read
/// apart from it ([`read_apart`]).
fn properties(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    set: &[PropertyId],
) -> Result<PropertyValues, ReadError> {
    let mut answers = if set.iter().any(|&id| !is_read_apart(id)) {
        values(context, &read::read_node(context, client, node_id)?, set)
    } else {
        PropertyValues(
            set.iter()
                .map(|&id| PropertyAnswer { id, value: None })
                .collect(),
        )
    };
    let apart: Vec<PropertyId> = set
        .iter()
        .copied()
        .filter(|&id| is_read_apart(id))
        .collect();
    if !apart.is_empty() {
        let read = read_apart(context, client, node_id, &apart)?;
        for answer in &mut answers.0 {
            if let Some(value) = read.answer(answer.id) {
                answer.value.clone_from(&value.value);
            }
        }
    }
    Ok(answers)
}

/// The properties of `set` that no snapshot holds, of node `node_id`, each
/// by the one read its table entry names: whether its suggestions are
/// open, a refresh reading its controller-for relation live; whether it
/// has the `Value` pattern; its nearest window's class, from Windows; and,
/// for a classic .NET Framework Windows Forms combo box, its list's window
/// by `GetComboBoxInfo`; and how many children a `SysTreeView32` item
/// holds, by the control's messages, as NVDA's tree view item counts them
/// (`NVDAObjects/IAccessible/sysTreeView32.py`), with no read of a child.
fn read_apart(
    context: &RequestContext<'_>,
    client: &mut Client,
    node_id: NodeId,
    set: &[PropertyId],
) -> Result<PropertyValues, ReadError> {
    let wants = |id| set.contains(&id);
    let suggestions = if wants(PropertyId::Suggestions) {
        read::refreshed::<verbatim_uia::SuggestionsRefresh>(context, client, node_id)?
    } else {
        None
    };
    let combo_box = if wants(PropertyId::HasValuePattern) || wants(PropertyId::ComboBoxList) {
        read::refreshed::<verbatim_uia::ComboBoxRefresh>(context, client, node_id)?
    } else {
        None
    };
    Ok(PropertyValues(
        set.iter()
            .map(|&id| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::Suggestions => suggestions.as_ref().and_then(|element| {
                        let read = element.read();
                        read.reports_suggestions()
                            .then_some(PropertyValue::Flag(read.controls_any))
                    }),
                    PropertyId::HasValuePattern => combo_box
                        .as_ref()
                        .map(|element| PropertyValue::Flag(element.read().value_available)),
                    PropertyId::WindowClass => read::kept_own_window(context, node_id)
                        .or_else(|| known_nearest_of(context, node_id))
                        .map(|hwnd| {
                            PropertyValue::Text(verbatim_win32::window::window_class_name(hwnd))
                        }),
                    PropertyId::ComboBoxList => combo_box.as_ref().and_then(|element| {
                        let read = element.read();
                        let class = verbatim_reader_rules::class::normalize_class_name(
                            &verbatim_win32::window::window_class_name(read.hwnd),
                        );
                        if !read.is_net_framework_win_forms(&class) {
                            return None;
                        }
                        verbatim_win32::window::combo_box_list(read.hwnd)
                            .map(|list| PropertyValue::Window(window_handle(list)))
                    }),
                    PropertyId::ChildCount => context
                        .store
                        .key_of(node_id)
                        .and_then(|(hwnd, _, child)| {
                            verbatim_ia2::acquire::tree_view_child_count(hwnd, child)
                        })
                        .map(PropertyValue::Count),
                    _ => None,
                },
            })
            .collect(),
    ))
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

/// Whether `node`'s property `property` has `value`.
fn has_value(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    (property, value): (PropertyId, &PropertyValue),
) -> bool {
    values(context, node, &[property])
        .answer(property)
        .and_then(|answer| answer.value.as_ref())
        .is_some_and(|found| found == value)
}

/// `node`'s children, in order, at most [`MAX_NODES`].
fn children(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    let mut children = Vec::new();
    let mut next = read::navigate(context, client, node, QueryKind::FirstChild)?;
    while let Some(child) = next {
        if children.len() >= MAX_NODES {
            break;
        }
        next = match read::navigate(context, client, child.id, QueryKind::NextSibling) {
            Ok(next) => next,
            // A child gone as it was read ends the list there.
            Err(ReadError::Gone) => None,
            Err(error) => return Err(error),
        };
        children.push(child);
    }
    Ok(children)
}

/// The first of `node`'s children, or descendants depth first, whose
/// property has the value, visiting at most [`MAX_NODES`].
fn find(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
    scope: FindScope,
    wanted: (PropertyId, &PropertyValue),
) -> Result<Option<NodeSnapshot>, ReadError> {
    let mut visited = 0usize;
    let mut stack = vec![children(context, client, node)?.into_iter()];
    while let Some(level) = stack.last_mut() {
        let Some(child) = level.next() else {
            stack.pop();
            continue;
        };
        visited = visited.saturating_add(1);
        if has_value(context, &child, wanted) {
            return Ok(Some(child));
        }
        if visited >= MAX_NODES {
            break;
        }
        if scope == FindScope::Descendants {
            let below = match children(context, client, child.id) {
                Ok(below) => below,
                Err(ReadError::Gone) => Vec::new(),
                Err(error) => return Err(error),
            };
            stack.push(below.into_iter());
        }
    }
    Ok(None)
}

/// The generic read of the object an event names.
fn read_object(
    context: &RequestContext<'_>,
    client: &mut Client,
    ask: &Ask,
    read: &ReadObject,
    probes: &mut Vec<Probe>,
) -> Outcome {
    let (snapshot, relation, nearest) = match &read.object {
        // The listener's.
        ObjectRef::Kept(_) => return Outcome::Refused(Refusal::Unsupported),
        ObjectRef::Node(node) => {
            let relation = match &read.relation {
                None => None,
                Some(RelationQuery::OneOf(nodes)) => Some(one_of(*node, nodes)),
                Some(RelationQuery::ControlledBy(focus)) => {
                    Some(controlled_by(context, client, *focus, *node))
                }
                Some(RelationQuery::Selection { focus, nodes }) => {
                    Some(match controlled_by(context, client, *focus, *node) {
                        RelationFound::Unrelated => one_of(*node, nodes),
                        found => found,
                    })
                }
            };
            match read::read_node(context, client, *node) {
                Ok(snapshot) => (snapshot, relation, None),
                Err(error) => return failed(&error),
            }
        }
        ObjectRef::Msaa {
            window,
            object,
            child,
        } => {
            let hwnd = hwnd_of(*window);
            match verdict(context, hwnd, ask.verdict, probes) {
                Some(false) => {}
                Some(true) => return Outcome::Refused(Refusal::ReadThroughUia),
                None => return Outcome::NotThere,
            }
            let Some(mut object) = verbatim_ia2::acquire::event_object(hwnd, *object, *child)
            else {
                return Outcome::NotThere;
            };
            // An MSAA object controls nothing: a selection's relation is
            // its place among the nodes, as NVDA compares an MSAA selection
            // with the focus's ancestors. The first node, the committed
            // focus, is met by NVDA's equality, as NVDA directs an event to
            // the focus object it equals; every other only at the address
            // it was reported at, as NVDA meets an ancestor again only in
            // its table of live objects by address and its gate asks
            // whether the object is that very ancestor
            // (`docs/parity/focus-announcement.md`, "An ancestor is
            // recognized by the address it was reported at").
            let relation = match &read.relation {
                None => None,
                Some(RelationQuery::ControlledBy(_)) => Some(RelationFound::Unrelated),
                Some(RelationQuery::OneOf(nodes) | RelationQuery::Selection { nodes, .. }) => Some(
                    nodes
                        .iter()
                        .enumerate()
                        .position(|(place, &node)| {
                            if place == 0 {
                                object.is_equal_to(node, context.nodes())
                            } else {
                                object.which_of(&[node], context.nodes()) == Some(node)
                            }
                        })
                        .and_then(|place| u32::try_from(place).ok())
                        .map_or(RelationFound::Unrelated, RelationFound::Is),
                ),
            };
            if read.purpose == Purpose::Change {
                return changed(context, &mut object, hwnd, read, relation);
            }
            let snapshot = object.read(context.nodes(), verbatim_ia2::acquire::Purpose::Announce);
            (snapshot, relation, Some(hwnd))
        }
    };
    let (ancestors, met) = match &read.walk {
        None => (Vec::new(), None),
        Some(known) => match walk(context, client, &snapshot, known) {
            Ok(walked) => walked,
            Err(read::FocusLost) => return Outcome::NotThere,
        },
    };
    let descendants = if read.descendants {
        match descendants(context, client, snapshot.id) {
            Ok(descendants) => descendants,
            Err(error) => return failed(&error),
        }
    } else {
        Vec::new()
    };
    let nearest = nearest.or_else(|| known_nearest(context, &snapshot));
    let ancestor_reads = ancestor_reads(context, &ancestors, nearest, &read.set);
    Outcome::Result(Reply::Object(Box::new(ObjectRead {
        object: node_read(context, &snapshot, nearest, &read.set),
        relation,
        descendants: descendants
            .iter()
            .map(|descendant| node_read(context, descendant, nearest, &read.set))
            .collect(),
        ancestors: ancestor_reads,
        met,
        route: None,
    })))
}

/// The answer to a change read of the MSAA object `object` in `hwnd`,
/// whose relation to the nodes Core named is `relation`: not there when
/// it is none of them, read no further; else the properties of its set
/// alone, each read as a full read gives it, as NVDA reads only the
/// property its event names (`docs/design/readers-tell-core-asks-build1.md`,
/// "Changes on objects").
fn changed(
    context: &RequestContext<'_>,
    object: &mut verbatim_ia2::acquire::EventObject,
    hwnd: isize,
    read: &ReadObject,
    relation: Option<RelationFound>,
) -> Outcome {
    let Some(place) = (match (&read.relation, relation) {
        (
            Some(RelationQuery::OneOf(nodes) | RelationQuery::Selection { nodes, .. }),
            Some(RelationFound::Is(place)),
        ) => usize::try_from(place)
            .ok()
            .and_then(|place| nodes.get(place).copied()),
        _ => None,
    }) else {
        return Outcome::NotThere;
    };
    let nodes = context.nodes();
    let role = object.role();
    let text = |text: Option<String>| text.map(PropertyValue::Text);
    let values = read
        .set
        .iter()
        .map(|&id| PropertyAnswer {
            id,
            value: match id {
                PropertyId::Name => text(object.name(role, nodes)),
                PropertyId::Value => text(object.value(role)),
                PropertyId::Description => text(object.description(role, nodes)),
                PropertyId::Role => Some(PropertyValue::Role(role)),
                PropertyId::States => Some(PropertyValue::States(object.states(role))),
                _ => None,
            },
        })
        .collect();
    Outcome::Result(Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: Some(place),
            window: node_window(hwnd),
            values: PropertyValues(values),
        },
        relation,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: None,
    })))
}

/// `node`'s descendants, depth first in document order, NVDA's
/// `recursiveDescendants`, at most [`MAX_NODES`]; a child gone as it was
/// read is left out with what is below it.
fn descendants(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: NodeId,
) -> Result<Vec<NodeSnapshot>, ReadError> {
    let mut found = Vec::new();
    let mut stack = vec![children(context, client, node)?.into_iter()];
    while let Some(level) = stack.last_mut() {
        let Some(child) = level.next() else {
            stack.pop();
            continue;
        };
        if found.len() >= MAX_NODES {
            break;
        }
        let below = match children(context, client, child.id) {
            Ok(below) => below,
            Err(ReadError::Gone) => Vec::new(),
            Err(error) => return Err(error),
        };
        found.push(child);
        stack.push(below.into_iter());
    }
    Ok(found)
}

/// Whether `object` is, or is inside, an element the committed focus
/// `focus` names in its `ControllerFor` relation, read live, as NVDA's
/// `event_selection` reads `focus.controllerFor` at the selection
/// (`NVDAObjects/__init__.py` 1314 to 1330). A focus or an object that is
/// not a UI Automation node, or a focus whose element the outpost no
/// longer holds, controls nothing, as NVDA's object of either backend
/// equals no object of the other and a dead focus names no relation.
pub(super) fn controlled_by(
    context: &RequestContext<'_>,
    client: &mut Client,
    focus: NodeId,
    object: NodeId,
) -> RelationFound {
    let (Some(runtime_id), Some(focused)) = (
        context.store.runtime_id_of(object),
        context
            .store
            .element_of(focus)
            .and_then(|agile| agile.resolve().ok()),
    ) else {
        return RelationFound::Unrelated;
    };
    let Ok((uia, nodes)) = client.uia_with(context, Context::node_request) else {
        return RelationFound::Unrelated;
    };
    let fill = verbatim_uia::Fill::new(&nodes);
    let query = verbatim_uia_rops::ControlledQuery {
        focused: focused.live().element(),
        selected: &runtime_id,
        fill: &fill,
    };
    let window = known_nearest_of(context, focus);
    let remote = context.tries_remote(window);
    let (answer, path) = match verbatim_uia_rops::controlled_selection(uia, &query, remote) {
        Ok(found) => found,
        Err(error) => {
            tracing::debug!(%error, "the focus's controller-for relation could not be read");
            return RelationFound::Unrelated;
        }
    };
    if let (verbatim_uia_rops::Path::Fallback(verbatim_uia_rops::Error::Import(_)), Some(hwnd)) =
        (&path, window)
    {
        context.read_classically(verbatim_win32::window::top_level_of(hwnd));
    }
    match answer.selected {
        None => RelationFound::Unrelated,
        // The relation's elements are listed up to the one it was found
        // in: the last is the object itself when it is one of them.
        Some(_) if answer.controlled.last() == Some(&verbatim_uia::runtime_id_key(&runtime_id)) => {
            RelationFound::Controlled
        }
        Some(_) => RelationFound::InsideControlled,
    }
}

/// The nearest window of node `node` known without a call, by its id.
fn known_nearest_of(context: &RequestContext<'_>, node: NodeId) -> Option<isize> {
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

/// `node`'s place among `nodes`, by its id.
fn one_of(node: NodeId, nodes: &[NodeId]) -> RelationFound {
    nodes
        .iter()
        .position(|&each| each == node)
        .and_then(|place| u32::try_from(place).ok())
        .map_or(RelationFound::Unrelated, RelationFound::Is)
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

/// Snapshots standing for the known nodes, by id alone, which a walk stops
/// at when it reaches one of them.
pub(super) fn known_stops(known: &[KnownAncestor]) -> Vec<NodeSnapshot> {
    known
        .iter()
        .filter_map(|known| match known {
            KnownAncestor::Node(id) => Some(NodeSnapshot {
                id: *id,
                backend: Backend::Msaa,
                role: Role::Unknown,
                name: None,
                value: None,
                states: StateSet::default(),
                details: NodeDetails::default(),
            }),
            KnownAncestor::Uia(_) | KnownAncestor::Window(_) => None,
        })
        .collect()
}

/// `node`'s ancestors, nearest first, up to the first Core already knows,
/// and its place in `known`; the walk stops at a known node of this
/// outpost.
fn walk(
    context: &RequestContext<'_>,
    client: &mut Client,
    node: &NodeSnapshot,
    known: &[KnownAncestor],
) -> Result<(Vec<NodeSnapshot>, Option<u32>), read::FocusLost> {
    let stops = known_stops(known);
    let candidates = candidates_of(&stops);
    let chain = if node.backend == Backend::Msaa {
        read::msaa_enrichment(context, client, (node, &stops, Some(&candidates)))?.0
    } else {
        read::ancestors(context, client, node.id).map_err(|_| read::FocusLost)?
    };
    Ok(cut_at_known(context, chain, known))
}

/// The kept nodes of `stops`, for NVDA's equality to compare what a walk
/// reads with.
fn candidates_of(stops: &[NodeSnapshot]) -> verbatim_ia2::equality::Candidates {
    verbatim_ia2::equality::Candidates::new(vec![stops.iter().map(|node| node.id).collect()])
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
    /// Core's selection baseline for the committed focus, which a repeated
    /// focus's caret is compared with.
    pub(super) baseline: Option<TextAnchor>,
}

/// The focus answer `focus` describes, by `set`.
pub(super) fn focus_read(
    context: &RequestContext<'_>,
    focus: &Focus<'_>,
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
            dialog_text: (ancestor.role == Role::Dialog)
                .then(|| ancestor.details.description.clone())
                .flatten(),
        })
        .collect();
    FocusRead {
        focus: focus_node(focus.node, nearest),
        msaa: focus.msaa,
        redirected_to: None,
        ancestors,
        met: focus.met,
        foreground: focus
            .foreground
            .map(|window| focus_node(window, window.details.window.map(hwnd_of))),
        value_from_selection: focus.value_from_selection,
        class: ObjectClass::Plain,
        runtime_id_reused: focus.runtime_id_reused,
        caret: caret_of(context, focus.node, focus.baseline),
    }
}

/// The focus's caret, read after its states (Table 17): for a node that
/// may have text, its caret and selection, compared with `baseline` when
/// the node holds it, the next baseline held; none for a node with no
/// text. A terminal's first read is its baseline read, so its caret is not
/// read here.
fn caret_of(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    baseline: Option<TextAnchor>,
) -> FocusCaret {
    if node.role == Role::Terminal {
        FocusCaret::NotRead
    } else if text_reads::may_have_text(context, node.id, node.role) {
        text_reads::focus_caret(context, node.id, baseline)
    } else {
        FocusCaret::NoText
    }
}

/// The MSAA ids `node` was read by, by the key it is kept under.
fn held_ids(context: &RequestContext<'_>, node: &NodeSnapshot) -> Option<HeldIds> {
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

/// An MSAA run, its candidates tried newest first until one is a valid
/// focus.
fn msaa_run(
    context: &RequestContext<'_>,
    client: &mut Client,
    run: &MsaaRun,
    probes: &mut Vec<Probe>,
) -> Outcome {
    let mut tried = Vec::new();
    for candidate in &run.candidates {
        let ids = candidate.ids;
        let hwnd = hwnd_of(ids.window);
        let outcome = match verdict(context, hwnd, candidate.verdict, probes) {
            // Read through UI Automation, whose own focus speaks for it,
            // as NVDA drops a native UI Automation window's `WinEvent`.
            Some(true) => Tried::Skipped,
            None => Tried::NotThere,
            Some(false) => try_candidate(context, client, (ids, candidate.kind), run),
        };
        let found = matches!(outcome, Tried::Focus(_));
        tried.push(outcome);
        if found {
            break;
        }
    }
    Outcome::Result(Reply::Run(RunRead { tried }))
}

/// One MSAA run candidate read as NVDA's pump reads it: the object at its
/// event's address, a focus event's object or an ancestor having the
/// focused state, then its ancestors up to one Core already knows.
fn try_candidate(
    context: &RequestContext<'_>,
    client: &mut Client,
    (ids, kind): (MsaaIds, verbatim_model::request::CandidateKind),
    run: &MsaaRun,
) -> Tried {
    let hwnd = hwnd_of(ids.window);
    let Some(mut candidate) = verbatim_ia2::acquire::focus_candidate(hwnd, ids.object, ids.child)
    else {
        return Tried::NotThere;
    };
    if kind == verbatim_model::request::CandidateKind::Focus {
        let plain = |window| read::reads_as_plain(context, window);
        let mut parent = |window| read::parent_window_for_focused_state(context, client, window);
        if !candidate.has_focused_state(read::MAX_ANCESTOR_HOPS, &plain, &mut parent) {
            return Tried::Refused(Refusal::NotFocused);
        }
    }
    let held_child = candidate.resolved_child();
    let stops = known_stops(&run.known);
    let candidates = candidates_of(&stops);
    let node = candidate.read(context.nodes(), Some(&candidates));
    let Ok((chain, _)) = read::msaa_enrichment(context, client, (&node, &stops, Some(&candidates)))
    else {
        return Tried::NotThere;
    };
    let (mut ancestors, met) = cut_at_known(context, chain, &run.known);
    let mut node = node;
    read::describe_dialogs(
        context,
        client,
        (ancestors.iter_mut().chain(std::iter::once(&mut node)), None),
        &[],
    );
    let msaa = HeldIds {
        event: ids,
        held_child,
    };
    let key = context.store.key_of(node.id);
    let redirected_to = key
        .filter(|&(window, object, child)| (window, object, child) != (hwnd, ids.object, ids.child))
        .map(|(window, object, child)| MsaaIds {
            window: window_handle(window),
            object,
            child,
        });
    // A focus that met none of the ancestors Core knows enters a new
    // window: its foreground object is read with it, as NVDA reads it then.
    let foreground = met
        .is_none()
        .then(|| read::foreground_object(context, client))
        .flatten();
    let mut read = focus_read(
        context,
        &Focus {
            node: &node,
            ancestors: &ancestors,
            met,
            nearest: Some(hwnd),
            msaa: Some(msaa),
            foreground: foreground.as_ref(),
            value_from_selection: false,
            runtime_id_reused: false,
            baseline: run.baseline,
        },
        &run.set,
    );
    read.redirected_to = redirected_to;
    Tried::Focus(Box::new(read))
}

/// The application's current focus, as a focus-now read finds it, with its
/// ancestors up to one Core already knows.
fn current_focus(
    context: &RequestContext<'_>,
    client: &mut Client,
    set: &[PropertyId],
    (known, baseline): (&[KnownAncestor], Option<TextAnchor>),
) -> Outcome {
    let (now, control_window) = read::focus_now(context, client, false, &known_stops(known));
    let Some(mut control) = now.focus else {
        return Outcome::NotThere;
    };
    let (mut ancestors, met) = cut_at_known(context, control.ancestors, known);
    read::describe_dialogs(
        context,
        client,
        (
            ancestors
                .iter_mut()
                .chain(std::iter::once(&mut control.node)),
            None,
        ),
        &[],
    );
    let msaa = (control.node.backend == Backend::Msaa)
        .then(|| held_ids(context, &control.node))
        .flatten();
    let foreground = now.window.as_ref().map(|(window, _)| window);
    Outcome::Result(Reply::Focus(Box::new(focus_read(
        context,
        &Focus {
            node: &control.node,
            ancestors: &ancestors,
            met,
            nearest: control_window,
            msaa,
            foreground,
            value_from_selection: false,
            runtime_id_reused: false,
            baseline,
        },
        set,
    ))))
}

/// The route's end as the outpost's continuation above it takes it.
pub(super) fn ancestors_end(end: &RouteEnd) -> verbatim_model::protocol::AncestorsEnd {
    use verbatim_model::protocol::AncestorsEnd;
    match end {
        RouteEnd::Known(_) | RouteEnd::ProcessBoundary | RouteEnd::Root => AncestorsEnd::Top,
        RouteEnd::ContinueFrom(window) => AncestorsEnd::ContinueFrom(hwnd_of(*window)),
        RouteEnd::PlainFrom(window) => AncestorsEnd::PlainFrom(hwnd_of(*window)),
    }
}
