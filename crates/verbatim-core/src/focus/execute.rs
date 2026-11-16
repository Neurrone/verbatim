//! A focus answer executed (design, "Focus and foreground"): its nodes
//! become the snapshots today's announcement code speaks, the focus is
//! executed through that code against the committed focus, and what Core
//! keeps of the answer is kept with the committed focus
//! ([`super::versions::Kept`]). The announcement code and the reducer's
//! focus context are shared with the old focus path until the old types
//! are deleted.

use std::sync::Arc;

use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusAncestor, FocusCaret, FocusNode, FocusRead, KnownAncestor};
use verbatim_model::tell::{AcceptanceFacts, Event, Relation};
use verbatim_model::{
    Backend, EarlierElement, Effect, FocusText, NodeDetails, NodeSnapshot, ObjectKey, Pid, Role,
    StateSet, TextAnchor, TraceId, WindowFacts, WindowHandle,
};

use super::follow::FollowRequest;
use super::versions::{Kept, KeptAncestor, Place, ReadBy};
use crate::entries::Entries;
use crate::entries::window_facts;
use crate::event_queue::Cx;
use crate::event_queue::RequestWindow;
use crate::focus_queue::Report;
use crate::foreground::ForegroundFact;
use crate::state::SrState;

/// The ancestors Core already knows, as a focus request sent them: each
/// with the place, in the committed focus's kept ancestors, of the
/// ancestor it names, so that the answer's meeting point is found again;
/// and the committed focus's selection baseline, which a repeated focus's
/// caret is compared with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Known {
    /// The list as sent.
    pub(crate) sent: Vec<KnownAncestor>,
    /// For each, the place of its ancestor, nearest first.
    pub(crate) places: Vec<usize>,
    /// The selection baseline the request names.
    pub(crate) baseline: Option<TextAnchor>,
}

impl Known {
    /// The ancestors Core already knows for a request to the outpost
    /// generation `asked` (`None` for the listener), with the committed
    /// focus's baseline when that generation holds it.
    pub(crate) fn of(state: &SrState, asked: Option<verbatim_model::OutpostId>) -> Self {
        let (places, sent) = state.versions.known_with_places(asked).into_iter().unzip();
        Self {
            sent,
            places,
            baseline: crate::text_facts::named_in_focus_read(state, asked),
        }
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
pub(crate) fn snapshot(node: &FocusNode, backend: Backend) -> Option<NodeSnapshot> {
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
            desktop_window: node.read.window.desktop,
            ..NodeDetails::default()
        },
    })
}

/// The window facts today's announcement code reads, from the facts the
/// tell carries. A window owned by Verbatim's hidden frame, as Verbatim's
/// menu is, takes the frame, the top of its owner chain, as its top-level
/// window, so that the focus enters the frame and says its title,
/// "Verbatim" (Verbatim-only, design "Focus and foreground").
fn window_of(tell: &Event, facts: &AcceptanceFacts) -> WindowFacts {
    let in_foreground = facts.window_to_foreground != Relation::Outside
        || facts.root_owner_to_foreground != Relation::Outside
        || facts.root_owner_is_foregrounds;
    let top_level = if facts.hidden_frame_owned {
        facts.root_owner.handle
    } else {
        facts.root.handle
    };
    WindowFacts {
        top_level,
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
        // Place 0 is the committed focus itself, which the walk met as an
        // ancestor of the focus inside it.
        let above_from = if let Some(above_from) = place.checked_sub(1) {
            above_from
        } else {
            let itself = &context.snapshot;
            if itself.id != committed.node {
                return None;
            }
            let dialog_text = (itself.role == Role::Dialog)
                .then(|| itself.details.description.clone())
                .flatten();
            snapshots.push(itself.clone());
            kept.push(committed.as_ancestor(dialog_text));
            0
        };
        for above in committed.ancestors.get(above_from..)? {
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

/// Where a focus answer came from: the trace it runs under, the
/// application it belongs to, when the event was heard, the window facts
/// today's announcement code reads, and the window its read named.
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    /// The trace: the tell's, or the input's that made Core's own entry.
    pub(crate) trace_id: TraceId,
    /// The application.
    pub(crate) source: Pid,
    /// When the reader heard the event; 0 for a read Core made itself.
    pub(crate) observed_at_us: u64,
    /// The window facts the tell carried, `None` for a read Core made
    /// itself.
    pub(crate) window: Option<WindowFacts>,
    /// Whether it is a foreground tell's.
    pub(crate) whole_window: bool,
    /// The process of the focus's top-level window.
    pub(crate) top_process: Pid,
    /// The window its read named.
    pub(crate) asked: RequestWindow,
    /// Whether the tell's window is owned by Verbatim's hidden frame, whose
    /// focus takes the frame as its foreground.
    pub(crate) frame_owned: bool,
}

impl Origin {
    /// The origin of a focus a tell named, read on `asked`.
    pub(crate) fn of_tell(tell: &Event, asked: RequestWindow) -> Self {
        let facts = window_facts(tell);
        Self {
            trace_id: tell.trace_id,
            source: tell.process,
            observed_at_us: tell.at_us,
            window: facts.map(|facts| window_of(tell, facts)),
            whole_window: tell.kind == verbatim_model::event_kind::EventKind::Foreground,
            top_process: facts.map_or(asked.process, |facts| facts.root.process),
            asked,
            frame_owned: facts.is_some_and(|facts| facts.hidden_frame_owned),
        }
    }

    /// The origin of a focus Core read itself on `asked`, under `trace_id`.
    pub(crate) fn of_read(trace_id: TraceId, asked: RequestWindow) -> Self {
        Self {
            trace_id,
            source: asked.process,
            observed_at_us: 0,
            window: None,
            whole_window: false,
            top_process: asked.process,
            asked,
            frame_owned: false,
        }
    }
}

/// A focus answer made ready to execute: the report today's announcement
/// code speaks, what Core keeps of it, and the snapshot of the foreground
/// object it read, or of its top ancestor when it could not read one.
struct Ready {
    report: Report,
    kept: Kept,
    foreground: Option<NodeSnapshot>,
}

/// `read` made ready to execute, or `None` when its nodes were not minted
/// or its meeting point is not held, a reader's fault.
fn ready(
    state: &SrState,
    origin: &Origin,
    read: &FocusRead,
    known: &Known,
    read_by: ReadBy,
) -> Option<Ready> {
    let backend = match read_by {
        ReadBy::Uia { .. } => Backend::Uia,
        ReadBy::Msaa { .. } => Backend::Msaa,
    };
    let node = snapshot(&read.focus, backend)?;
    let (nearest_first, kept_ancestors) = chain(state, &read.ancestors, read.met, known, backend)?;
    let read_foreground = match &read.foreground {
        Some(foreground) => Some(snapshot(foreground, backend)?),
        None => None,
    };
    let mut ancestors = nearest_first;
    // A focus in a window Verbatim's hidden frame owns takes the frame,
    // which the answer read as its foreground, as its outermost ancestor,
    // so that it enters the frame and says its title.
    if origin.frame_owned
        && let Some(frame) = &read_foreground
        && !ancestors.iter().any(|ancestor| ancestor.id == frame.id)
    {
        ancestors.push(frame.clone());
    }
    ancestors.reverse();
    let ancestors =
        crate::reduce::focus_ancestors(origin.window.as_ref(), &node, &ancestors).to_vec();
    // The top ancestor stands for the foreground object only when the
    // answer could not read it.
    let foreground = read_foreground.or_else(|| ancestors.first().cloned());
    let report = Report {
        trace_id: origin.trace_id,
        source: origin.source,
        observed_at_us: origin.observed_at_us,
        window: origin.window.clone(),
        whole_window: origin.whole_window,
        node: node.clone(),
        ancestors: Arc::from(ancestors),
        selected_child: None,
        text: Some(focus_text(&read.caret)),
        earlier: read.runtime_id_reused.then_some(EarlierElement::Gone),
        group: None,
        alive: true,
    };
    let place = Place {
        window: read.focus.read.window.handle,
        top_level: origin.asked.top_level,
        root_owner: origin.asked.root_owner,
        ancestor_windows: kept_ancestors
            .iter()
            .filter_map(|ancestor| ancestor.window)
            .collect(),
    };
    let kept = Kept {
        source: origin.source,
        outpost: node.id.outpost(),
        node: node.id,
        read_by,
        place,
        window: origin.asked,
        ancestors: kept_ancestors,
        redirected_to: read.redirected_to,
        foreground: read.foreground.as_ref().and_then(|node| node.read.node),
        value_from_selection: read.value_from_selection,
        class: read.class,
        role: node.role,
        own_window: own_window(&read.focus.read.values),
        selection: crate::text_facts::SelectionFacts::default(),
    };
    Some(Ready {
        report,
        kept,
        foreground,
    })
}

/// The text today's announcement code speaks of a focus whose answer
/// carried `caret`: its line or selected text, its value when it has no
/// text, or neither when its text could not be read.
fn focus_text(caret: &FocusCaret) -> FocusText {
    match caret {
        FocusCaret::NoText => FocusText::NoText,
        FocusCaret::NotRead => FocusText::NotRead,
        FocusCaret::Read(read) => FocusText::Caret {
            caret: read.caret.clone(),
            selected_text: read.selected_text.clone(),
            changes: verbatim_model::CaretChanges::default(),
            facts: None,
        },
    }
}

/// Whether a focus of `origin` is dropped at its answer: its top-level
/// window is an ignored process's (Verbatim-only: the test harness's rule,
/// design "Focus and foreground").
fn ignored(state: &SrState, origin: &Origin) -> bool {
    state.ignored.contains(&origin.top_process)
}

/// Executes the focus `read` answered for `origin`, asked with `known`:
/// the focus is spoken by today's announcement code against the committed
/// focus, and both versions of the focus become it. When the walk met
/// none of the ancestors Core knows, the focus difference level is 1 or
/// less, and the foreground object becomes the one the answer read for
/// it, the top ancestor only if it could not be read (design, "Focus and
/// foreground"). The answer's caret becomes Core's, with the selection
/// baseline it minted; a repeated focus, on the committed focus's node,
/// then says the change from the baseline its request named (Table 17,
/// columns A to C). A focus whose nodes the reader did not mint, or whose
/// meeting point Core does not hold, ends unspoken: a reader's fault.
pub(crate) fn execute(
    state: &mut SrState,
    origin: &Origin,
    read: &FocusRead,
    known: &Known,
    read_by: ReadBy,
) -> Executed {
    if ignored(state, origin) {
        return Executed::default();
    }
    let Some(mut ready) = ready(state, origin, read, known, read_by) else {
        return Executed::default();
    };
    let repeated = state
        .versions
        .committed_read()
        .filter(|committed| committed.node == ready.kept.node)
        .map(|committed| committed.selection);
    let mut said = crate::reduce::execute_report(state, &ready.report, false, 0);
    if read.met.is_none()
        && let Some(foreground) = &ready.foreground
    {
        state
            .foreground
            .apply(ForegroundFact::Object(ObjectKey::of(foreground)));
    }
    if !matches!(read.caret, FocusCaret::Read(_)) {
        state.forget_caret();
    }
    let change =
        crate::text_facts::focus_answered(&mut ready.kept.selection, &read.caret, repeated);
    if !change.is_empty() {
        said.push(crate::editing::speak(origin.trace_id, change));
    }
    let follows = commit(state, origin, ready);
    Executed {
        said,
        follows,
        committed: true,
    }
}

/// What executing a focus answer did: what it said, the `Follow`s its
/// commit sends, and whether it committed the focus.
#[derive(Debug, Default)]
pub(crate) struct Executed {
    /// What it said.
    pub(crate) said: Vec<Effect>,
    /// The `Follow`s the commit sends.
    pub(crate) follows: Vec<FollowRequest>,
    /// Whether the focus was committed.
    pub(crate) committed: bool,
}

impl Executed {
    /// Emits what it said and sends its `Follow`s.
    pub(crate) fn deliver(self, cx: &mut Cx<Entries>) {
        for said in self.said {
            cx.emit(said);
        }
        super::follow::send(cx, self.follows);
    }
}

/// Commits `ready`: both versions of the focus become it, and the
/// `Follow`s the commit sends are returned (Table 10, columns A and B).
fn commit(state: &mut SrState, origin: &Origin, ready: Ready) -> Vec<FollowRequest> {
    let class = origin.window.as_ref().map(|window| window.class.as_str());
    let follows = state.follows.committed(
        Some((
            &ready.kept,
            &ready.report.node,
            ready.report.ancestors.last(),
        )),
        class,
    );
    state.versions.execute(ready.kept);
    follows
}

/// Executes the committed focus again, as a UI Automation menu opening on
/// it does (design, "Focus and foreground"; NVDA:
/// `UIAHandler/__init__.py` 856 to 868), under `trace_id`. Nothing with
/// nothing committed.
pub(crate) fn again(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let Some(focus) = state.focus.object() else {
        return Vec::new();
    };
    let report = Report {
        trace_id,
        source: focus.source,
        observed_at_us: 0,
        window: focus.window.clone(),
        whole_window: false,
        node: focus.snapshot.clone(),
        ancestors: Arc::clone(&focus.ancestors),
        selected_child: None,
        text: None,
        earlier: None,
        group: None,
        alive: focus.alive,
    };
    crate::reduce::execute_report(state, &report, false, 0)
}

/// How a re-take's answer ended (design, "Focus and foreground", the
/// re-take after a reader's replacement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retaken {
    /// It read the same as the focus Core holds, and was taken silently.
    Silently,
    /// It differed, and was executed.
    Executed,
    /// It was dropped: an ignored process's, or a reader's fault.
    Dropped,
}

/// Judges a re-take's `read` against the focus Core holds: one that reads
/// the same is taken silently with its new ids, the navigator moved only
/// if there is none and the caret baseline kept; a different one is
/// executed. With the readers' last focus empty it is executed and spoken
/// (an accepted corner, Table 3).
pub(crate) fn retake(
    state: &mut SrState,
    origin: &Origin,
    read: &FocusRead,
    read_by: ReadBy,
) -> (Retaken, Executed) {
    if ignored(state, origin) {
        return (Retaken::Dropped, Executed::default());
    }
    let known = Known::default();
    let Some(ready) = ready(state, origin, read, &known, read_by.clone()) else {
        return (Retaken::Dropped, Executed::default());
    };
    let same = state.versions.last.is_some()
        && state.focus.object().is_some_and(|focus| {
            crate::reduce::same_reading(focus, &ready.report.node, &ready.report.ancestors)
        });
    if same {
        crate::reduce::take_silently(state, &ready.report);
        // The text-changed flag is the focus's and survives its reader's
        // replacement; the old baseline's anchor ended with its generation,
        // and the answer minted one in the new generation (Table 17,
        // column J).
        let mut ready = ready;
        ready.kept.selection = crate::text_facts::SelectionFacts {
            baseline: match &read.caret {
                FocusCaret::Read(caret) => Some(caret.baseline),
                FocusCaret::NoText | FocusCaret::NotRead => None,
            },
            text_changed: state
                .versions
                .committed_kept()
                .is_some_and(|kept| kept.selection.text_changed),
        };
        let follows = commit(state, origin, ready);
        return (
            Retaken::Silently,
            Executed {
                said: Vec::new(),
                follows,
                committed: true,
            },
        );
    }
    (
        Retaken::Executed,
        execute(state, origin, read, &known, read_by),
    )
}
