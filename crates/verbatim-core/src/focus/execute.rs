//! A focus answer executed (design, "Focus and foreground"): its nodes
//! become the snapshots the announcement code speaks, joined above the
//! place the walk met to the committed focus's line, the focus is
//! executed through that code against the committed focus, and the
//! committed focus becomes it, one record
//! ([`super::versions::Focus`]).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{FocusCaret, FocusNode, FocusRead, KnownAncestor};
use verbatim_model::tell::{AcceptanceFacts, Event, Relation};
use verbatim_model::{
    Backend, EarlierElement, Effect, FocusText, NodeSnapshot, ObjectKey, Pid, Role, TraceId,
    WindowFacts, WindowHandle,
};

use super::versions::{Kept, LineEntry, Place, ReadBy};
use crate::entries::Entries;
use crate::entries::window_facts;
use crate::event_queue::Cx;
use crate::event_queue::RequestWindow;
use crate::foreground::ForegroundFact;
use crate::keys::entry::Fence;
use crate::state::SrState;

/// A focus as Core executes it: the parts of a focus the focus handling
/// reads, owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) trace_id: TraceId,
    pub(crate) source: Pid,
    /// When its reader observed it, in microseconds; 0 when not known.
    pub(crate) observed_at_us: u64,
    pub(crate) window: Option<WindowFacts>,
    pub(crate) node: NodeSnapshot,
    pub(crate) whole_window: bool,
    /// Its ancestors as Core takes them, outermost first.
    pub(crate) ancestors: Arc<[LineEntry]>,
    pub(crate) text: Option<FocusText>,
    pub(crate) earlier: Option<EarlierElement>,
    /// The top-level window its read named, which stands for the entry
    /// just below the desktop when it carries no window facts, as a focus
    /// Core read itself does.
    pub(crate) top_level: Option<WindowHandle>,
}

/// How executing a focus writes the committed focus: the one parameter by
/// which [`crate::reduce::execute_report`] and
/// [`crate::reduce::take_silently`] write the record.
#[derive(Clone, Debug)]
pub(crate) enum Commit {
    /// A reader's focus answer: the committed focus becomes it, with what
    /// Core keeps of the answer, `kept`, and the readers' last focus with
    /// the speech fence `fence` ([`super::versions::Versions::execute`]).
    New {
        /// What Core keeps of the answer.
        kept: Kept,
        /// The speech fence of the entry that read it.
        fence: Fence,
    },
    /// The committed focus, a live reader's, spoken again, as a UI
    /// Automation menu opening on it does: it is written back with only
    /// what Core spoke of it replaced; its variant, what Core keeps of its
    /// answer, its line, the readers' last focus and the postponed dialog
    /// are left as they are ([`super::versions::Versions::respoken`]).
    InPlace(Box<super::versions::Focus>),
}

/// The ancestors Core already knows, as a focus request sent them: each
/// with the place, in the committed focus's line, of the ancestor it
/// names, so that the answer's meeting point is found again.
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

/// The snapshot today's announcement code speaks of `node`, read through
/// `backend`; `None` when the reader minted no node for it, a reader's
/// fault.
pub(crate) fn snapshot(node: &FocusNode, backend: Backend) -> Option<NodeSnapshot> {
    node.read.snapshot(backend)
}

/// The backend that read `node` in an answer read through `backend`: an
/// MSAA answer reads every node through MSAA; in a UI Automation answer, a
/// node with no runtime id was read through MSAA, as the plain windows
/// completing a walk cut at its depth limit are, and is another object
/// than a UI Automation element of the same window
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
fn node_backend(node: &FocusNode, backend: Backend) -> Backend {
    match backend {
        Backend::Uia if node.runtime_id.is_none() => Backend::Msaa,
        other => other,
    }
}

/// The window facts today's announcement code reads, from the facts the
/// tell carries. A window owned by Verbatim's hidden frame, as Verbatim's
/// menu is, takes the frame, the top of its owner chain, as its top-level
/// window, so that the focus enters the frame and says its title,
/// "Verbatim" (Verbatim-only, design "Focus and foreground").
///
/// The window in front at the tell's check is Program Manager's or the
/// taskbar's when the tell's own top-level window or root owner is that
/// window and has a shell class: the tell's facts alone decide, read
/// together by its reader, so the title rule does not depend on whether
/// the listener's foreground report reached Core before the tell. A
/// window in front that is neither is of no class the tell names, and no
/// rule reads the answer for it: the title rule judges a focus whose own
/// top-level window is in front.
fn window_of(tell: &Event, facts: &AcceptanceFacts) -> WindowFacts {
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
        in_foreground: facts.in_foreground(),
        foreground_window: facts.foreground,
        class: facts.window.class.clone(),
        application: tell.application.clone(),
        alt_held: false,
        foreground_is_shell: [&facts.root, &facts.root_owner].into_iter().any(|window| {
            window.handle == facts.foreground
                && crate::applications::is_shell_foreground_class(&window.class)
        }),
    }
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
    /// The automation id of the focus's parent, which a UI Automation tell
    /// carries for the task switcher's item alone.
    pub(crate) parent_automation_id: Option<String>,
    /// The speech fence of the entry that read it: a run's or a menu
    /// start's executed candidate's own, never the run's, and the entry's
    /// for any other, which the readers' last focus keeps when it executes
    /// ([`super::versions::LastFocus::since`]).
    pub(crate) fence: Fence,
}

impl Origin {
    /// The origin of a focus a tell named, read on `asked`, for an entry
    /// of speech fence `fence`.
    pub(crate) fn of_tell(tell: &Event, asked: RequestWindow, fence: Fence) -> Self {
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
            parent_automation_id: match tell
                .cached
                .answer(PropertyId::ParentAutomationId)
                .and_then(|answer| answer.value.as_ref())
            {
                Some(PropertyValue::Text(id)) => Some(id.clone()),
                _ => None,
            },
            fence,
        }
    }

    /// The origin of a focus Core read itself on `asked`, under `trace_id`,
    /// for an entry of speech fence `fence`.
    pub(crate) fn of_read(trace_id: TraceId, asked: RequestWindow, fence: Fence) -> Self {
        Self {
            trace_id,
            source: asked.process,
            observed_at_us: 0,
            window: None,
            whole_window: false,
            top_process: asked.process,
            asked,
            frame_owned: false,
            parent_automation_id: None,
            fence,
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
/// or its meeting point is not one Core sent, a reader's fault. Its
/// entries are its ancestors, joined where the walk met a place Core sent
/// to the committed focus's own line above that place, the committed focus
/// itself at place 0: one line, nothing looked up in another record.
fn ready(
    state: &SrState,
    origin: &Origin,
    read: &FocusRead,
    known: &Known,
    read_by: ReadBy,
) -> Option<Ready> {
    let backend = read_by.backend();
    let mut node = snapshot(&read.focus, backend)?;
    node.details
        .parent_automation_id
        .clone_from(&origin.parent_automation_id);
    // Nearest first, as the walk read them.
    let mut ancestors = Vec::new();
    for ancestor in &read.ancestors {
        let mut entry = snapshot(&ancestor.node, node_backend(&ancestor.node, backend))?;
        entry.details.uia_layout = ancestor.uia_layout;
        entry.details.described = ancestor.described;
        ancestors.push(LineEntry {
            snapshot: entry,
            runtime_id: ancestor.node.runtime_id.clone(),
            simple_child: ancestor.simple_child,
        });
    }
    if let Some(met) = read.met {
        let place = *known.places.get(usize::try_from(met).ok()?)?;
        let committed = state.versions.focus()?;
        let line =
            std::iter::once(committed.as_entry()).chain(committed.line.iter().rev().cloned());
        ancestors.extend(line.skip(place));
    }
    let read_foreground = match &read.foreground {
        Some(foreground) => Some(snapshot(foreground, node_backend(foreground, backend))?),
        None => None,
    };
    // A focus in a window Verbatim's hidden frame owns takes the frame,
    // which the answer read as its foreground, as its outermost ancestor,
    // so that it enters the frame and says its title. The frame is a
    // top-level window, read through MSAA as its client pane, which stands
    // for it; the answer's values do not carry that mark, so it is set
    // here, or the pane would be structure and say nothing. Core keeps it
    // as the focus's outermost ancestor too, so the next focus in the menu,
    // which meets the menu and reads no foreground, keeps the frame above
    // it and enters nothing, and a focus that reads it again finds it
    // already entered: the title is said once, entering the menu. A
    // window the frame owns that is in front itself, such as a dialog, is
    // the foreground read; when it is the focus too it is not taken, since
    // the focus is never among its own ancestors
    // (`docs/parity/focus-context.md`).
    if origin.frame_owned
        && let Some(frame) = &read_foreground
        && frame.id != node.id
        && !ancestors
            .iter()
            .any(|ancestor| ancestor.snapshot.id == frame.id)
    {
        let mut frame = frame.clone();
        frame.details.top_level_client = frame.role == Role::Pane;
        ancestors.push(LineEntry {
            snapshot: frame,
            runtime_id: None,
            simple_child: false,
        });
    }
    ancestors.reverse();
    // An item whose containers an application's rule drops has none, in
    // what is spoken and in what Core keeps alike, one representation, so
    // that the next focus is sent none as ancestors Core knows and meets
    // none of them ([`crate::applications::drops_ancestry`]).
    if crate::applications::drops_ancestry(origin.window.as_ref(), &node) {
        ancestors.clear();
    }
    // The top ancestor stands for the foreground object only when the
    // answer could not read it.
    let foreground = read_foreground.or_else(|| ancestors.first().map(|top| top.snapshot.clone()));
    let place = Place {
        window: read.focus.read.window.handle,
        top_level: origin.asked.top_level,
        root_owner: origin.asked.root_owner,
        ancestor_windows: ancestors
            .iter()
            .rev()
            .filter_map(|ancestor| ancestor.snapshot.details.window)
            .collect(),
    };
    let console_terminal = node.role == Role::Terminal && in_console(origin, &ancestors);
    let mut selection = crate::text_facts::SelectionFacts::default();
    crate::text_facts::focus_answered(&mut selection, &read.caret);
    let kept = Kept {
        read_by,
        place,
        asked: origin.asked,
        foreground: read.foreground.as_ref().and_then(|node| node.read.node),
        value_from_selection: read.value_from_selection,
        selection,
        console_terminal,
    };
    let report = Report {
        trace_id: origin.trace_id,
        source: origin.source,
        observed_at_us: origin.observed_at_us,
        window: origin.window.clone(),
        whole_window: origin.whole_window,
        node,
        ancestors: Arc::from(ancestors),
        text: Some(focus_text(&read.caret)),
        earlier: read.runtime_id_reused.then_some(EarlierElement::Gone),
        top_level: Some(origin.asked.top_level),
    };
    Some(Ready {
        report,
        kept,
        foreground,
    })
}

/// Whether a focus of `origin` with `ancestors`, outermost first, is in a
/// console window: its tell's window is one, or, for a focus Core read
/// itself, its nearest ancestor with a window of its own is.
fn in_console(origin: &Origin, ancestors: &[LineEntry]) -> bool {
    let console = Some(super::follow::CONSOLE_WINDOW_CLASS);
    if let Some(window) = &origin.window {
        return Some(window.class.as_str()) == console;
    }
    ancestors
        .iter()
        .rev()
        .map(|ancestor| &ancestor.snapshot)
        .find(|ancestor| ancestor.details.window.is_some())
        .is_some_and(|ancestor| ancestor.details.class_name.as_deref() == console)
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
/// baseline it minted, silently, a repeated focus's too, as NVDA's
/// `initAutoSelectDetection` sets its baseline (`editableText.py` 417 to
/// 427; Dickson, 2026-11-17) (Table 17,
/// columns A to C). A focus whose nodes the reader did not mint, or whose
/// meeting point is not one Core sent, ends unspoken: a reader's fault.
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
    let Some(ready) = ready(state, origin, read, known, read_by) else {
        return Executed::default();
    };
    // A reused runtime id: the element the node held before is gone, so
    // Core forgets the node's caret and its terminal's model, and the
    // focus's text is taken afresh, as a new object's is (build 1's
    // design, "The outpost in this build").
    if read.runtime_id_reused {
        let node = ready.report.node.id;
        if state.focus_caret(node).is_some() {
            state.forget_caret();
        }
        state.drop_models(|model| model.node == node);
    }
    // With the readers' last focus emptied by a foreground change to an
    // unrelated window, the committed focus was left: the new focus is
    // compared with no ancestors, so the window's title and the focus are
    // said (design, "The ancestors spoken at execution").
    let left = state.versions.last.is_none();
    let mut said = vec![observed(&ready.report)];
    let (terminal, follows) = commit(state, &ready);
    let new = Commit::New {
        kept: ready.kept,
        fence: origin.fence,
    };
    said.extend(crate::reduce::execute_report(
        state,
        &ready.report,
        left,
        new,
    ));
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
    crate::terminal::reading::committed(state, terminal, follows, origin.observed_at_us);
    Executed {
        said,
        committed: true,
    }
}

/// The focus `report` as the control plane's event stream carries it, as
/// its reader reported it before Core judged its reports itself.
fn observed(report: &Report) -> Effect {
    verbatim_model::Observed::effect(
        report.trace_id,
        (report.source, report.node.backend),
        report.window.clone(),
        verbatim_model::NormalizedEvent::FocusChanged {
            node: report.node.clone(),
            whole_window: report.whole_window,
            ancestors: report
                .ancestors
                .iter()
                .map(|ancestor| ancestor.snapshot.clone())
                .collect(),
            text: report.text.clone().map(Box::new),
            earlier: report.earlier,
        },
    )
}

/// What executing a focus answer did: what it said, and whether it
/// committed the focus.
#[derive(Debug, Default)]
pub(crate) struct Executed {
    /// What it said.
    pub(crate) said: Vec<Effect>,
    /// Whether the focus was committed.
    pub(crate) committed: bool,
}

impl Executed {
    /// Emits what it said, each utterance stamped with `fence`, and sends
    /// the requests the commit wanted, a terminal's baseline after its
    /// `Follow`s (`docs/design/readers-tell-core-asks-build3.md`, "The
    /// focused terminal's life"). Every caller passes its entry's fence
    /// ([`crate::entries::front_fence`]) but the read at a press while a
    /// dialog is postponed, which passes the postponing entry's
    /// (`super::shown`).
    pub(crate) fn deliver(self, cx: &mut Cx<Entries>, fence: crate::keys::entry::Fence) {
        for mut said in self.said {
            crate::keys::entry::stamp(fence, &mut said);
            cx.emit_stamped(said);
        }
        cx.send_wanted();
    }
}

/// What committing `ready` starts (Table 10, columns A and B): the
/// focused terminal, when it is one, and every `Follow` of the commit, which
/// [`crate::terminal::reading::committed`] wants in order once the
/// committed focus is written, a terminal's baseline after its `Follow`.
fn commit(
    state: &mut SrState,
    ready: &Ready,
) -> (
    Option<crate::terminal::reading::Terminal>,
    Vec<(RequestWindow, Option<verbatim_model::request::Followed>)>,
) {
    let (kept, node) = (&ready.kept, &ready.report.node);
    let parent = ready.report.ancestors.last().map(|parent| &parent.snapshot);
    let follows = state
        .follows
        .committed(Some((kept, node, parent)), kept.console_terminal);
    let terminal = matches!(kept.read_by, ReadBy::Uia { .. })
        .then(|| {
            crate::terminal::reading::terminal_of(
                node.role,
                node.id,
                kept.asked,
                kept.console_terminal,
            )
        })
        .flatten();
    let follows = follows
        .into_iter()
        .map(|follow| (follow.window, follow.follow))
        .collect();
    (terminal, follows)
}

/// Executes the committed focus again, as a UI Automation menu opening on
/// it does (design, "Focus and foreground"; NVDA:
/// `UIAHandler/__init__.py` 856 to 868), under `trace_id`: only what Core
/// spoke of it is written ([`Commit::InPlace`]). Nothing unless a live
/// reader's focus is committed.
pub(crate) fn again(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let Some(focus) = state.versions.live().cloned() else {
        return Vec::new();
    };
    let report = Report {
        trace_id,
        source: focus.context.source,
        observed_at_us: 0,
        window: focus.context.window.clone(),
        whole_window: false,
        node: focus.context.snapshot.clone(),
        ancestors: Arc::clone(&focus.line),
        text: None,
        earlier: None,
        top_level: Some(focus.kept.place.top_level),
    };
    crate::reduce::execute_report(state, &report, false, Commit::InPlace(Box::new(focus)))
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
) -> Executed {
    if ignored(state, origin) {
        return Executed::default();
    }
    let known = Known::default();
    let Some(mut ready) = ready(state, origin, read, &known, read_by.clone()) else {
        return Executed::default();
    };
    let Some(old) = state.versions.focus().filter(|focus| {
        state.versions.last.is_some()
            && crate::reduce::same_reading(focus, &ready.report.node, &ready.report.ancestors)
    }) else {
        return execute(state, origin, read, &known, read_by);
    };
    // The text-changed flag is the focus's and survives its reader's
    // replacement, so it is taken from the old record before the new one
    // is written; the old baseline's anchor ended with its generation, and
    // the answer minted one in the new generation (Table 17, column J).
    ready.kept.selection.text_changed = old.kept.selection.text_changed;
    let (terminal, follows) = commit(state, &ready);
    let new = Commit::New {
        kept: ready.kept,
        fence: origin.fence,
    };
    crate::reduce::take_silently(state, &ready.report, new);
    crate::terminal::reading::committed(state, terminal, follows, origin.observed_at_us);
    Executed {
        said: Vec::new(),
        committed: true,
    }
}
