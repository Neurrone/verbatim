//! A focus answer executed (design, "Focus and foreground"): its nodes
//! become the snapshots today's announcement code speaks, the focus is
//! executed through that code against the committed focus, and what Core
//! keeps of the answer is kept with the committed focus
//! ([`super::versions::Kept`]). The announcement code and the reducer's
//! focus context are shared with the old focus path until the old types
//! are deleted.

use std::sync::Arc;

use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusAncestor, FocusNode, FocusRead, KnownAncestor};
use verbatim_model::tell::{AcceptanceFacts, Event, Relation};
use verbatim_model::{
    Backend, EarlierElement, Effect, NodeDetails, NodeSnapshot, Role, StateSet, TraceId,
    WindowFacts, WindowHandle,
};

use super::versions::{Kept, KeptAncestor, Place, ReadBy};
use crate::entries::window_facts;
use crate::event_queue::RequestWindow;
use crate::focus_queue::Report;
use crate::state::SrState;

/// The ancestors Core already knows, as a focus request sent them: each
/// with the place, in the committed focus's kept ancestors, of the
/// ancestor it names, so that the answer's meeting point is found again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Known {
    /// The list as sent.
    pub(crate) sent: Vec<KnownAncestor>,
    /// For each, the place of its ancestor, nearest first.
    pub(crate) places: Vec<usize>,
}

impl Known {
    /// The ancestors Core already knows for a request to the outpost
    /// generation `asked` (`None` for the listener).
    pub(crate) fn of(state: &SrState, asked: Option<verbatim_model::OutpostId>) -> Self {
        let (places, sent) = state.versions.known_with_places(asked).into_iter().unzip();
        Self { sent, places }
    }
}

fn value(values: &PropertyValues, id: PropertyId) -> Option<&PropertyValue> {
    values.answer(id).and_then(|answer| answer.value.as_ref())
}

fn text(values: &PropertyValues, id: PropertyId) -> Option<String> {
    match value(values, id) {
        Some(PropertyValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

fn count(values: &PropertyValues, id: PropertyId) -> Option<u32> {
    match value(values, id) {
        Some(PropertyValue::Count(count)) => Some(*count),
        _ => None,
    }
}

/// A node's own window, for a window object.
fn own_window(values: &PropertyValues) -> Option<WindowHandle> {
    match value(values, PropertyId::NativeWindow) {
        Some(PropertyValue::Window(window)) if window.0 != 0 => Some(*window),
        _ => None,
    }
}

/// The snapshot today's announcement code speaks of `node`, read through
/// `backend`; `None` when the reader minted no node for it, a reader's
/// fault.
fn snapshot(node: &FocusNode, backend: Backend) -> Option<NodeSnapshot> {
    let values = &node.read.values;
    Some(NodeSnapshot {
        id: node.read.node?,
        backend,
        role: match value(values, PropertyId::Role) {
            Some(PropertyValue::Role(role)) => *role,
            _ => Role::Unknown,
        },
        name: text(values, PropertyId::Name),
        value: text(values, PropertyId::Value),
        states: match value(values, PropertyId::States) {
            Some(PropertyValue::States(states)) => *states,
            _ => StateSet::default(),
        },
        details: NodeDetails {
            description: text(values, PropertyId::Description),
            keyboard_shortcut: text(values, PropertyId::KeyboardShortcut),
            position_in_set: count(values, PropertyId::PositionInSet),
            set_size: count(values, PropertyId::SetSize),
            level: count(values, PropertyId::Level),
            rect: match value(values, PropertyId::Rect) {
                Some(PropertyValue::Rect(rect)) => Some(*rect),
                _ => None,
            },
            class_name: text(values, PropertyId::ClassName),
            cell: match value(values, PropertyId::Cell) {
                Some(PropertyValue::Cell(cell)) => Some(*cell),
                _ => None,
            },
            table_counts: match value(values, PropertyId::TableCounts) {
                Some(PropertyValue::TableCounts(counts)) => Some(*counts),
                _ => None,
            },
            window: own_window(values),
            ..NodeDetails::default()
        },
    })
}

/// The window facts today's announcement code reads, from the facts the
/// tell carries.
fn window_of(tell: &Event, facts: &AcceptanceFacts) -> WindowFacts {
    let in_foreground = facts.window_to_foreground != Relation::Outside
        || facts.root_owner_to_foreground != Relation::Outside
        || facts.root_owner_is_foregrounds;
    WindowFacts {
        top_level: facts.root.handle,
        root_owner: facts.root_owner.handle,
        topmost: facts.window_topmost || facts.root_topmost,
        under_active_window: facts
            .window
            .class
            .starts_with("Windows.UI.Core")
            .then_some(facts.window_to_active != Relation::Outside),
        in_foreground,
        foreground_window: facts.foreground,
        class: facts.window.class.clone(),
        application: tell.application.clone(),
        alt_held: false,
        foreground_is_shell: false,
    }
}

/// The answer's ancestors, nearest first, joined with the committed
/// focus's above the one the walk met: each snapshot, and what Core keeps
/// of it. `None` when a node cannot be found, a reader's fault.
fn chain(
    state: &SrState,
    ancestors: &[FocusAncestor],
    met: Option<u32>,
    known: &Known,
    backend: Backend,
) -> Option<(Vec<NodeSnapshot>, Vec<KeptAncestor>)> {
    let mut snapshots = Vec::new();
    let mut kept = Vec::new();
    for ancestor in ancestors {
        snapshots.push(snapshot(&ancestor.node, backend)?);
        kept.push(KeptAncestor {
            node: ancestor.node.read.node?,
            runtime_id: ancestor.node.runtime_id.clone(),
            window: own_window(&ancestor.node.read.values),
            simple_child: ancestor.simple_child,
            dialog_text: ancestor.dialog_text.clone(),
        });
    }
    if let Some(met) = met {
        let place = *known.places.get(usize::try_from(met).ok()?)?;
        let committed = state.versions.committed_kept()?;
        let context = state.focus.object()?;
        for above in committed.ancestors.get(place..)? {
            let node = context
                .ancestors
                .iter()
                .find(|snapshot| snapshot.id == above.node)?;
            snapshots.push(node.clone());
            kept.push(above.clone());
        }
    }
    Some((snapshots, kept))
}

/// Executes the focus `read` answered for `tell`, judged by `in_window`
/// and asked with `known`: the
/// focus is spoken by today's announcement code against the committed
/// focus, and both versions of the focus become it. A focus whose nodes
/// the reader did not mint, or whose meeting point Core does not hold,
/// ends unspoken: a reader's fault.
pub(crate) fn execute(
    state: &mut SrState,
    (tell, in_window): (&Event, &RequestWindow),
    read: &FocusRead,
    known: &Known,
    read_by: ReadBy,
) -> Vec<Effect> {
    let backend = match read_by {
        ReadBy::Uia { .. } => Backend::Uia,
        ReadBy::Msaa { .. } => Backend::Msaa,
    };
    let Some(node) = snapshot(&read.focus, backend) else {
        return Vec::new();
    };
    let Some((nearest_first, kept_ancestors)) =
        chain(state, &read.ancestors, read.met, known, backend)
    else {
        return Vec::new();
    };
    let window = window_facts(tell).map(|facts| window_of(tell, facts));
    let mut ancestors = nearest_first;
    ancestors.reverse();
    let ancestors = crate::reduce::focus_ancestors(window.as_ref(), &node, &ancestors).to_vec();
    let report = Report {
        trace_id: TraceId::from_raw(0),
        source: tell.process,
        observed_at_us: tell.at_us,
        window,
        whole_window: tell.kind == verbatim_model::event_kind::EventKind::Foreground,
        node: node.clone(),
        ancestors: Arc::from(ancestors),
        selected_child: None,
        text: None,
        earlier: read.runtime_id_reused.then_some(EarlierElement::Gone),
        group: None,
        alive: true,
    };
    let effects = crate::reduce::execute_report(state, &report, false, 0);
    let place = Place {
        window: read.focus.read.window,
        top_level: in_window.top_level,
        root_owner: in_window.root_owner,
        ancestor_windows: kept_ancestors
            .iter()
            .filter_map(|ancestor| ancestor.window)
            .collect(),
    };
    state.versions.execute(Kept {
        source: tell.process,
        outpost: node.id.outpost(),
        node: node.id,
        read_by,
        place,
        ancestors: kept_ancestors,
        redirected_to: read.redirected_to,
        foreground: read.foreground.as_ref().and_then(|node| node.read.node),
        value_from_selection: read.value_from_selection,
        class: read.class,
    });
    effects
}
