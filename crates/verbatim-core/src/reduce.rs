//! The reducer (architecture section 2): pure state-and-effect transitions
//! from normalized accessibility events and fetch completions.
//!
//! Nothing in this module performs I/O, reads a clock, or consults
//! randomness; the only way the outside world enters is through the `Input`
//! passed to [`reduce`]. That purity is what lets the flight recorder turn a
//! captured sequence of inputs into a deterministic regression test (see
//! [`crate::replay`]).
//!
//! The behavior is specified in the focus and announcement files of
//! `docs/parity/` (listed in `docs/parity/readme.md`), written from the NVDA behavior documented in
//! `docs/nvda/events.md`.

use std::sync::Arc;

use verbatim_model::{
    ActionName, Earcon, EarlierElement, Effect, Input, Message, NodeId, NodeSnapshot, Percent,
    Phrase, Pid, PropertyChange, QueryKind, ReviewCommand, Role, SegmentContent, SpeechPriority,
    State, StateSet, TraceId, Utterance, UtteranceSegment, UtteranceSource, WindowFacts,
};
use verbatim_model::{
    Backend, FocusNow, FocusText, FocusValidity, ObjectKey, WindowHandle, WindowIdentity,
};

use crate::focus::execute::{Commit, Report};
use crate::focus::versions::{Focus, LineEntry};
use crate::focus_rules::{
    ChangeKind, ChangeTarget, change_target, is_sensitive_slider, lowest_common_ancestor,
};
use crate::foreground::ForegroundFact;
use crate::state::{CellMemory, FocusContext, Navigator, SpokenRecord, SrState};
use crate::{editing, say_all, shell, terminal, text};

/// The activity id of the shell's window-snap results notification, the one
/// UIA notification spoken from any application (`docs/parity/focus-events.md`, "Event
/// acceptance"); the supervisor starts an outpost for no other notification.
pub const SNAP_RESULTS_ACTIVITY: &str = "Windows.Shell.SnapComponent.SnapHotKeyResults";

/// Advances `state` by one `input`, changing it in place, and returns the
/// effects the imperative shell must execute. `runtime` is the event
/// queue's paused function kept beside `state` by whoever runs the
/// reducer; the queue's inputs (`Input::Queue`) run with it, rebuilding it
/// from the state's checkpoint when it has none, beside today's inputs
/// until each moves to the queue in its step of build 1.
///
/// Deterministic: no I/O, no clocks, and no randomness, so the same state
/// and input always produce the same new state and effects. The state is
/// changed in place rather than copied, so a step costs nothing for the
/// parts of the state it does not touch (architecture section 2).
#[must_use]
pub fn reduce(state: &mut SrState, runtime: &mut crate::Runtime, input: &Input) -> Vec<Effect> {
    // Once Verbatim is exiting nothing more is said or asked.
    if state.exiting {
        return Vec::new();
    }
    // The speech side's reports and Core's timers reach Core as queue
    // inputs and are handled at once, as the inputs they stand for, from
    // here on (`crate::entries::Routed::AtOnce`).
    let at_once;
    // The input's time: a queue input's own, else the last input's.
    let now_us = match input {
        Input::Queue { at_us, .. } => *at_us,
        _ => state.queue.now_us(),
    };
    let (input, mut effects) = match input {
        Input::Queue {
            trace_id,
            at_us,
            input: queued,
        } => match crate::entries::route(state, runtime, (*trace_id, *at_us), queued) {
            crate::entries::Routed::Said(said) => (input, said),
            crate::entries::Routed::AtOnce(standing_for) => {
                at_once = *standing_for;
                (&at_once, reduce_input(state, &at_once, now_us))
            }
        },
        _ => (input, reduce_input(state, input, now_us)),
    };
    // A press whose read ended with no answer, its terminal's model gone,
    // says nothing and ends.
    // Whatever cuts speech off drops the terminal output handed to it, and
    // with it the output still waiting, as a key press does in NVDA.
    if effects.iter().any(terminal::cuts_speech) {
        terminal::cut(state, now_us);
    }
    // A say-all in the event queue changed by an input outside it, such as
    // the speech side's cut: the queue settles, its entries stamping their
    // own speech.
    if !matches!(input, Input::Queue { .. }) && crate::entries::unsettled(state) {
        let trace_id = input.trace_id().unwrap_or(state.queue_trace.0);
        effects.extend(crate::entries::settle(state, runtime, trace_id));
    }
    effects
}

/// [`reduce`]'s dispatch, by input, at `now_us`.
fn reduce_input(state: &mut SrState, input: &Input, now_us: u64) -> Vec<Effect> {
    match input {
        Input::Settings(settings) => {
            if state.settings.report_terminal_output && !settings.report_terminal_output {
                crate::terminal::drop_waiting(state, now_us);
            }
            state.settings = *settings;
            Vec::new()
        }
        Input::Reported(reported) => {
            state.reported = *reported;
            Vec::new()
        }
        // Playback will reach none of a cut utterance's marks it had not
        // reached: say-all stops, as on any cut, and terminal output drops
        // what waits, as anything that cuts speech off does.
        Input::UtteranceCancelled { marks, .. } => {
            terminal::utterance_cancelled(state, marks, now_us);
            say_all::utterance_cancelled(state, marks)
        }
        // A failed utterance counts as heard: its marks are taken as
        // reached, though say-all moves no cursor to what was not heard.
        Input::UtteranceFailed { marks } => {
            let mut effects = say_all::utterance_failed(state, marks);
            for mark in marks {
                let waits = terminal::output_waits(state, state.queue.waiting_items());
                effects.extend(terminal::mark_reached(state, *mark, (now_us, waits)));
            }
            effects
        }
        // What the shell's threads learn; `Tick` is reserved vocabulary with
        // no policy yet, which `shell::reduce` leaves alone, as it does any
        // other input.
        _ => shell::reduce(state, input),
    }
}

/// Speaks what an activation did: the action's name, NVDA's "Activate" for
/// an action with none, or "No action" when nothing was `activated`.
pub(crate) fn activation_completed(
    trace_id: TraceId,
    activated: bool,
    action: Option<&ActionName>,
) -> Vec<Effect> {
    let segment = match (activated, action) {
        (false, _) => UtteranceSegment::new(SegmentContent::Message(Message::NoAction)),
        (true, Some(ActionName::Named(name))) if !name.trim().is_empty() => {
            UtteranceSegment::text(name.clone())
        }
        (true, Some(ActionName::Invoke)) => {
            UtteranceSegment::new(SegmentContent::Message(Message::Invoke))
        }
        (true, _) => UtteranceSegment::new(SegmentContent::Message(Message::Activate)),
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![segment],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Keeps the navigator's copy of the focus current: a name, value, or state
/// change, or a fresher report of the same focus, applies to the navigator
/// too while it rests on the focus, so reporting the current object or
/// reviewing it reads what the object is now, as NVDA reads it live.
fn sync_navigator_with_focus(state: &mut SrState) {
    let (Some(focus), Some(navigator)) = (
        state.versions.focus().map(|focus| &focus.context),
        state.navigator.as_mut(),
    ) else {
        return;
    };
    if navigator.object.id != focus.snapshot.id || navigator.object == focus.snapshot {
        return;
    }
    navigator.object = focus.snapshot.clone();
}

/// The parts of a `FocusChanged` event the focus handling reads.
struct FocusReport<'a> {
    node: &'a NodeSnapshot,
    /// When its outpost observed it, in microseconds; 0 when not known.
    observed_at_us: u64,
    /// Whether it is a whole-window focus.
    whole_window: bool,
    /// Its ancestors, outermost first, complete to the desktop.
    ancestors: &'a [LineEntry],
    /// What the outpost's caret read found, for a focus that may have text.
    text: Option<&'a FocusText>,
    /// What its reader read of the element it held before under its node.
    earlier: Option<EarlierElement>,
}

impl FocusReport<'_> {
    /// Whether it is a new element under the node of the previous focus,
    /// `previous`: its reader read that the element it held before under
    /// that node is gone. One that only lost the focus is still the same
    /// object, so the lines meet at it (`docs/parity/focus-events.md`,
    /// "Duplicate focus suppression").
    fn replaces(&self, previous: &NodeSnapshot) -> bool {
        self.node.id == previous.id && self.earlier == Some(EarlierElement::Gone)
    }
}

impl Report {
    /// The report as the focus handling reads it.
    fn view(&self) -> FocusReport<'_> {
        FocusReport {
            node: &self.node,
            observed_at_us: self.observed_at_us,
            whole_window: self.whole_window,
            ancestors: &self.ancestors,
            text: self.text.as_ref(),
            earlier: self.earlier,
        }
    }
}

/// Takes `owned`'s focus silently in place of a focus that reads the same:
/// its new ids are taken, the navigator moved only if there is none, and
/// the caret baseline kept (the silent re-take, `crate::focus::retake`).
/// The committed focus is written as `commit` says.
pub(crate) fn take_silently(state: &mut SrState, owned: &Report, commit: Commit) {
    let report = &owned.view();
    let window = owned.window.as_ref();
    let found = state
        .versions
        .focus()
        .cloned()
        .map(|focus| Line { focus, left: false });
    let line = focus_ancestry(
        found.as_ref(),
        (window, owned.top_level, committed_top(state)),
        report,
    );
    let previous = found.as_ref().map(|found| &found.focus);
    let new_focus = focus_context(
        (owned.trace_id, owned.source),
        window,
        report,
        (previous, &line.ancestors),
    );
    write(state, commit, new_focus, line.ancestors);
    if state.navigator.is_none() {
        state.navigator = Some(Navigator::on(report.node.clone(), None));
    }
    editing::keep_focus_caret(state, report.node.id, report.text);
}

/// Executes `report`, judged `left` or not, against the focus object, as
/// NVDA executes a focus against the one before it
/// (`docs/parity/focus-announcement.md`, "Focus announcement content and
/// property order"):
///
/// - A whole-window focus makes its window the foreground object.
/// - A whole-window focus with no name becomes the focus silently: a bare
///   "window" says nothing, and the window is not announced later.
/// - While the focus is dead (its outpost ended) and the report is not
///   judged left, a report of the same focus from a replacement outpost is
///   taken silently.
/// - Otherwise the new chain is joined to the focus line where they meet
///   ([`focus_ancestry`]), and the containers below the meeting point, the
///   newly entered ones, the node, and a selection container's selected
///   item are spoken, queued behind current speech (which the key press
///   that moved focus has already cut off), and the navigator follows
///   focus. When the two lines differ just below the desktop, speech is cut
///   off first, and the new top-level window, its outermost ancestor, is
///   one of the entered containers, spoken by the same rule as the others
///   (`docs/parity/focus-context.md`, "Window announcement on switching
///   applications"). Speech queued for an earlier focus expires as NVDA's
///   speech manager expires it (`Effect::DropExpiredSpeech`).
/// - The title rule: a focus that enters the window in front, one that is
///   not the entered window or that is judged left, says the window's
///   title when its line does not already speak it ([`window_title`]).
/// - A report judged left is executed as a focus from another top-level
///   window is (the left mark, `crate::focus::versions`).
///
/// The committed focus is written as `commit` says before anything after
/// the join reads it, so the rest sees the focus it executed.
pub(crate) fn execute_report(
    state: &mut SrState,
    owned: &Report,
    left: bool,
    commit: Commit,
) -> Vec<Effect> {
    let report = &owned.view();
    let window = owned.window.as_ref();
    let (trace_id, source) = (owned.trace_id, owned.source);
    let found = state
        .versions
        .focus()
        .cloned()
        .map(|focus| Line { focus, left });
    let line = focus_ancestry(
        found.as_ref(),
        (window, owned.top_level, committed_top(state)),
        report,
    );
    if report.whole_window {
        state
            .foreground
            .apply(ForegroundFact::Object(ObjectKey::of(report.node)));
    }
    let previous_left = found.as_ref().is_some_and(|found| found.left);
    let title = enter_window(state, window, report, previous_left);

    // A left focus is followed as a focus from another top-level window is:
    // nothing of the line left behind carries over.
    let previous = found
        .as_ref()
        .filter(|found| !found.left)
        .map(|found| &found.focus);
    let new_focus = focus_context(
        (trace_id, source),
        window,
        report,
        (previous, &line.ancestors),
    );
    if let Some(focus) = previous
        && state.focus_ended()
        && reads_the_same(focus, report.node, report.ancestors)
    {
        // The silent re-read: the replacement outpost reports the focus the
        // user already heard, so take its ids without speaking.
        write(state, commit, new_focus, line.ancestors);
        if state.navigator.is_none() {
            state.navigator = Some(Navigator::on(report.node.clone(), None));
        }
        editing::keep_focus_caret(state, report.node.id, report.text);
        return Vec::new();
    }

    // The review cursor follows focus (roadmap M3): every focus change snaps
    // the navigator object to the new focus and resets the review cursor to
    // its start.
    let navigator = Navigator::on(report.node.clone(), None);
    // When speech is cut off (`docs/nvda/speech.md`, "Cancellation", and
    // `docs/nvda/events.md`): focus speech is queued, not interrupting. A
    // focus line that differs from the previous one just below the desktop
    // brings a new foreground window, which cancels speech, nameless or
    // not, and so does entering a menu; speech for a focus the user has
    // since left is dropped by the speech manager, which is told where the
    // focus now is.
    let foreground_changed = line.cut;
    let line_met = line.met.is_some();
    let entered: Vec<&NodeSnapshot> = line.entered.iter().map(|entry| &entry.snapshot).collect();
    let mut effects = vec![Effect::DropExpiredSpeech(FocusNow {
        focus: ObjectKey::of(report.node),
        ancestors: line
            .ancestors
            .iter()
            .map(|ancestor| ObjectKey::of(&ancestor.snapshot))
            .collect(),
        foreground: state.foreground.object(),
        menu_item_of_focus: menu_item_of_focus(previous, report.node),
    })];
    // Entering a menu cuts speech too, at its place among the entered
    // containers (`focus_speech`).
    end_text_activity(state, foreground_changed, &mut effects);
    write(state, commit, new_focus, line.ancestors);
    state.navigator = Some(navigator);
    // A nameless whole-window focus, and any focus in the shell's staging
    // windows, say nothing; speech was cut above if the foreground window
    // changed.
    // The virtual desktop's name kept is said by the focus taken, as NVDA
    // says it before any focus (`crate::desktop`).
    let desktop_name = crate::desktop::name_for_focus(state, trace_id);
    if report.whole_window && !has_text(report.node.name.as_deref())
        || crate::applications::is_shell_staging_window(window)
    {
        effects.extend(desktop_name);
        return effects;
    }

    // NVDA says the window in front only when the focus line changed just
    // below the desktop; lines that meet at an object they share, such as a
    // window and the one it owns across processes, add no title.
    let title = title
        .filter(|_| !line_met)
        .and_then(|top_level| window_title(top_level, report.ancestors, &entered));
    let speech = focus_speech(state, trace_id, report, (title, entered, desktop_name));
    effects.extend(speech);
    effects.extend(editing::focus_text(
        state,
        trace_id,
        report.node,
        report.text,
    ));
    effects
}

/// What Core speaks of `report`'s focus when it is taken: its node under
/// `ancestors`, its line joined with `previous`, the line it is planned
/// against, not left.
fn focus_context(
    (trace_id, source): (TraceId, Pid),
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    (previous, ancestors): (Option<&Focus>, &[LineEntry]),
) -> FocusContext {
    FocusContext {
        source,
        window: window.cloned(),
        snapshot: report.node.clone(),
        spoken: spoken_record(previous, report, ancestors),
        reported_focused: report.node.states.contains(State::Focused),
        trace_id: Some(trace_id),
        observed_at_us: report.observed_at_us,
        reported_format: verbatim_model::TextAttributes::default(),
        caret: previous
            .map(|previous| &previous.context)
            .filter(|previous| previous.snapshot.id == report.node.id)
            .and_then(|previous| previous.caret.clone()),
        // The pacing of the same node, not left, is carried over; the
        // commit's terminal takes it (`crate::terminal::committed`).
        terminal: previous
            .map(|previous| &previous.context)
            .filter(|previous| previous.snapshot.id == report.node.id)
            .and_then(|previous| previous.terminal.as_deref())
            .map(|terminal| Box::new(terminal.carried())),
    }
}

/// Writes the committed focus as `commit` says, with `context` what Core
/// spoke of it and `line` its line: a reader's new focus becomes the
/// committed focus, or the committed focus spoken again takes `context`
/// alone ([`Commit`]).
fn write(state: &mut SrState, commit: Commit, context: FocusContext, line: Arc<[LineEntry]>) {
    match commit {
        Commit::New { kept, fence } => {
            let focus = Focus {
                context,
                kept,
                line,
            };
            state.versions.execute(focus, fence);
        }
        Commit::InPlace(mut focus) => {
            focus.context = context;
            state.versions.respoken(focus);
        }
    }
}

/// The focus line a report is planned or executed against, owned so the
/// plan can write the state: a focus, and the report's verdict, whether a
/// foreground change left the last queued focus for a window other than
/// the report's before it (`crate::focus::versions`). A report from the window
/// the focus was left for is not judged left, so its line is compared with
/// the focus's as it is, as NVDA compares its focus line with the one
/// before, and a window owned across processes meets its owner both ways.
#[derive(Clone, Debug)]
struct Line {
    focus: Focus,
    left: bool,
}

/// The entered window as a focus Core takes changes it, and the window
/// whose title the focus is to say, if any (`docs/parity/focus-context.md`,
/// "Window announcement on switching applications"): a whole-window focus
/// enters its window, saying no title but its own; a focus whose own
/// top-level window is the window in front when it was published
/// (`WindowFacts::foreground_window`) enters it when it is not the entered
/// window or the line planned against was left, making its chain's outermost entry the
/// foreground object, as NVDA's `doPreGainFocus` moves its foreground
/// object from the focus's ancestry, and says its title. A focus in a
/// popup, whose top-level window is not the foreground window, changes
/// nothing. A foreground change never sets the entered window: a report
/// read in a window as another came in front carries the window in front
/// at its publication, and setting the entered window from it would lose
/// that window's title.
fn enter_window(
    state: &mut SrState,
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    previous_left: bool,
) -> Option<WindowHandle> {
    let window = window?;
    if report.whole_window {
        state
            .foreground
            .apply(ForegroundFact::Entered(window.top_level));
        return None;
    }
    entered_window(state, Some(window), report, previous_left)?;
    state.foreground.apply(ForegroundFact::EnteredBy(
        window.top_level,
        report
            .ancestors
            .first()
            .map(|top| ObjectKey::of(&top.snapshot)),
    ));
    // Program Manager and the taskbar are entered with no title, as NVDA
    // takes no foreground event for them.
    (!window.foreground_is_shell).then_some(window.top_level)
}

/// The top-level window a focus that is not a whole-window focus enters,
/// with no write: its own, when it is the window in front at its
/// publication and is not the entered window or the line planned against was left
/// ([`enter_window`]).
fn entered_window(
    state: &SrState,
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    previous_left: bool,
) -> Option<WindowHandle> {
    let window = window?;
    let enters = !report.whole_window
        && window.top_level == window.foreground_window
        && (state.foreground.entered() != Some(window.top_level) || previous_left);
    enters.then_some(window.top_level)
}

/// The title a focus entering the top-level window `top_level` says: the
/// window's own entry in its chain, `ancestors`, the outermost entry with
/// that window handle, which stands for the window (its UIA element, MSAA
/// window or client object, or plain window), unless its line already
/// speaks it among the `entered` containers. An entry UI Automation does
/// not count a control and content does not stand for the window
/// ([`verbatim_model::NodeDetails::uia_layout`]), as it said nothing before
/// the reader reported such entries (Claude, 2026-11-21). An entry with no
/// name, and a chain with no entry for the window, add nothing.
fn window_title<'a>(
    top_level: WindowHandle,
    ancestors: &'a [LineEntry],
    entered: &[&NodeSnapshot],
) -> Option<&'a NodeSnapshot> {
    let entry = ancestors
        .iter()
        .map(|ancestor| &ancestor.snapshot)
        .filter(|ancestor| !ancestor.details.uia_layout)
        .find(|ancestor| ancestor.details.window == Some(top_level))?;
    let spoken = entered.iter().any(|container| {
        container.id == entry.id
            && !is_menu(container)
            && (is_sensitive_slider(container) || is_presentable_container(container))
            && !container_segments(container, true).is_empty()
    });
    (!spoken && has_text(entry.name.as_deref())).then_some(entry)
}

/// The spoken record of a new focus, `report`'s node under `ancestors`
/// ([`SpokenRecord`]): the node as it is about to be spoken, and for each
/// ancestor the states last spoken of it in the previous focus line, the
/// previous focus included, or else its reading, from which an entered
/// container is spoken.
fn spoken_record(
    previous: Option<&Focus>,
    report: &FocusReport<'_>,
    ancestors: &[LineEntry],
) -> SpokenRecord {
    let entries = ancestors
        .iter()
        .map(|ancestor| &ancestor.snapshot)
        .map(|ancestor| {
            let spoken = previous.and_then(|previous| previous.spoken_states(ancestor.id));
            (ancestor.id, spoken.unwrap_or(ancestor.states))
        })
        .collect();
    SpokenRecord::of(report.node, entries)
}

/// Ends what the previous focus's text was doing when the focus moves: a
/// say-all reading it (its speech is cut off too, since say-all's speech
/// is not focus speech the speech manager would drop), a caret key still
/// waiting for evidence (the focus announcement wins, as NVDA's wait gives
/// way to a pending focus event), and the typing being echoed. Speech is
/// cut off when `cut` says so or say-all was reading. A terminal's pacing
/// goes with a focus left or another node, as the new record carries only
/// the pacing of the same node not left ([`focus_context`]).
fn end_text_activity(state: &mut SrState, cut: bool, effects: &mut Vec<Effect>) {
    let reading = state.say_all.is_some();
    effects.extend(say_all::stop(state));
    if cut || reading {
        effects.push(Effect::StopSpeech);
    }
    state.typed_word.clear();
}

/// The speech for a new focus: the window's title the title rule adds, if
/// any, spoken with the words of a whole-window focus of the window
/// ([`window_title`]), then each entered container as its own utterance,
/// outermost first, valid while the focus is inside it, an entered menu
/// cutting speech in its place instead ([`is_menu`]), then the focus, valid
/// while it is the focus. When the focus has entered another top-level
/// window, the window is the outermost entered container, spoken first
/// whenever it is presentable, as NVDA speaks a window it took from the
/// focus's ancestry ([`is_presentable_container`]). A focus that may have
/// text leaves its value out, since its text follows
/// (`editing::focus_text`).
fn focus_speech(
    state: &mut SrState,
    trace_id: TraceId,
    report: &FocusReport<'_>,
    (title, entered, desktop_name): (Option<&NodeSnapshot>, Vec<&NodeSnapshot>, Option<Effect>),
) -> Vec<Effect> {
    let positions = speaks_positions(state);
    let last_level = &mut state.last_tree_level;
    let cell_memory = &mut state.cell_memory;
    let reads_text = editing::may_have_text(report.node.role);
    let mut effects = Vec::new();
    if let Some(title) = title {
        effects.push(Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments: unremembered_segments(title, Reason::Focus, positions),
                source: Some(source_of(title)),
                say_all: false,
                validity: None,
            },
            None,
        ));
    }
    // The virtual desktop's name, after the foreground's title and before
    // the entered containers, in NVDA's order
    // (`books/dev/src/reference/adr/0001-focus.md`, "The desktop's return and name").
    effects.extend(desktop_name);
    for container in entered {
        // Entering a menu bar, menu, or menu item cuts speech and says
        // nothing, in its place among the entered containers, outermost
        // first: speech for a container entered outside the menu is cut
        // too (`docs/parity/changes-and-selection.md`, "Entered menus").
        let sensitive_slider = is_sensitive_slider(container);
        if is_menu(container) && !sensitive_slider {
            effects.push(Effect::StopSpeech);
            continue;
        }
        if !sensitive_slider && !is_presentable_container(container) {
            continue;
        }
        let segments = container_segments(container, positions);
        if segments.is_empty() {
            continue;
        }
        effects.push(Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(source_of(container)),
                say_all: false,
                validity: None,
            },
            None,
        ));
    }
    let segments = if reads_text && report.node.value.is_some() {
        let mut node = report.node.clone();
        node.value = None;
        node_segments(&node, Reason::Focus, (last_level, cell_memory), positions)
    } else {
        node_segments(
            report.node,
            Reason::Focus,
            (last_level, cell_memory),
            positions,
        )
    };
    effects.push(Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(source_of(report.node)),
            say_all: false,
            validity: Some(FocusValidity {
                node: report.node.id,
                window: WindowIdentity::of(report.node),
            }),
        },
        None,
    ));
    effects
}

/// The focus the user is leaving, `previous`, when its speech stays valid
/// although the focus moves to `node`: an MSAA menu item whose ancestors
/// hold `node`, an MSAA popup menu, as when an application focuses a
/// submenu's first item and then the submenu itself (`docs/nvda/speech.md`,
/// "Expired focus speech").
fn menu_item_of_focus(previous: Option<&Focus>, node: &NodeSnapshot) -> Option<NodeId> {
    let previous = previous?;
    let item = &previous.context.snapshot;
    (item.backend == Backend::Msaa
        && item.role == Role::MenuItem
        && node.backend == Backend::Msaa
        && node.role == Role::Menu
        && previous
            .line
            .iter()
            .any(|ancestor| ancestor.snapshot.id == node.id))
    .then_some(item.id)
}

/// Whether `node` is a menu bar, a menu, or a menu item, whose entering
/// cuts speech and says nothing.
fn is_menu(node: &NodeSnapshot) -> bool {
    matches!(node.role, Role::MenuBar | Role::Menu | Role::MenuItem)
}

/// Whether a name or description has real, non-whitespace text.
fn has_text(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

/// Whether a reported focus, from another outpost generation, reads the
/// same as the dead focus's kept copy: role, name, value, and states, and
/// the ancestors' names and roles. Node ids are not compared, since a
/// replacement outpost issues new ones (`docs/parity/focus-events.md`,
/// "Recovery after an outpost is replaced"); a report of the ended
/// generation itself, waiting when it ended, is no replacement's.
fn reads_the_same(focus: &Focus, node: &NodeSnapshot, ancestors: &[LineEntry]) -> bool {
    focus.context.snapshot.id.outpost() != node.id.outpost() && same_reading(focus, node, ancestors)
}

/// Whether `node`, under `ancestors`, reads the same as the focus `focus`
/// Core holds: role, name, value, and states, and the ancestors' names and
/// roles, whatever their ids.
pub(crate) fn same_reading(focus: &Focus, node: &NodeSnapshot, ancestors: &[LineEntry]) -> bool {
    let kept = &focus.context.snapshot;
    kept.role == node.role
        && kept.name == node.name
        && kept.value == node.value
        && kept.states == node.states
        && focus.line.len() == ancestors.len()
        && focus.line.iter().zip(ancestors).all(|(old, new)| {
            old.snapshot.role == new.snapshot.role && old.snapshot.name == new.snapshot.name
        })
}

/// A state change of `node_id`, spoken on the focus or an ancestor
/// ([`change_on`]), and on nothing else.
/// A change of `node_id`'s property, judged by today's rules against the
/// committed focus and its ancestors, the navigator's copy of the focus
/// kept current: the event queue's change entries
/// (`crate::changes`) speak through this until the announcement code is
/// rewritten.
pub(crate) fn property_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    change: &PropertyChange,
    child_count: Option<u32>,
) -> Vec<Effect> {
    let effects = match change {
        PropertyChange::Name(name) => reduce_name_changed(state, trace_id, node_id, name.as_ref()),
        PropertyChange::Value(value) => {
            reduce_value_changed(state, trace_id, node_id, value.clone())
        }
        PropertyChange::States(new_states) => states_event(
            state,
            trace_id,
            node_id,
            (*new_states, StatesFrom::Other),
            child_count,
        ),
        PropertyChange::Description(description) => {
            reduce_description_changed(state, trace_id, node_id, description.as_ref())
        }
        PropertyChange::Suggestions(open) => reduce_suggestions(state, node_id, *open),
        // `PropertyChange` is `#[non_exhaustive]`.
        _ => Vec::new(),
    };
    sync_navigator_with_focus(state);
    effects
}

/// A state change of `node_id` to the states an outpost's UI Automation
/// property change carries from its event's cache, judged as
/// [`property_changed`] judges one, but that on the focus the focused
/// state's loss drops it ([`StatesFrom::OutpostEvent`]).
pub(crate) fn outpost_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    let effects = states_event(
        state,
        trace_id,
        node_id,
        (new_states, StatesFrom::OutpostEvent),
        None,
    );
    sync_navigator_with_focus(state);
    effects
}

/// Where a state change's states come from, which decides whether the
/// focused state's loss on the focus drops it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatesFrom {
    /// An outpost's UI Automation property change, its states from the
    /// event's cache, taken as the application raised it: a focus that
    /// reported the focused state and lacks it here had lost it before
    /// the change was raised, so the focus event came first and the change
    /// is not spoken (`docs/parity/changes-and-selection.md`, "State
    /// changes after the focus has left").
    OutpostEvent,
    /// An MSAA change's read, or a selection's tell: spoken whatever its
    /// focused state, the change and the focus sharing one pipe in the
    /// order they were raised.
    Other,
}

fn states_event(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    (new_states, from): (StateSet, StatesFrom),
    child_count: Option<u32>,
) -> Vec<Effect> {
    match change_on(state, ChangeKind::States, node_id) {
        Some(ChangeTarget::Focus) => {
            reduce_states_changed(state, trace_id, node_id, (new_states, from), child_count)
        }
        Some(ChangeTarget::Ancestor) => {
            reduce_ancestor_states_changed(state, trace_id, node_id, new_states)
        }
        None => Vec::new(),
    }
}

/// What a change of `kind` on `node_id` concerns, when it is spoken, judged
/// against the live focus Core holds by the rule the outposts judge it by
/// before they read it ([`change_target`]). The foreground object is never
/// a speaking ancestor: NVDA's event on the foreground window resolves to
/// its foreground object, not to the ancestor instance
/// (`docs/parity/focus-announcement.md`, "A top-level window's state
/// change"), which an outpost reads for when the window is the focus of a
/// report Core refused.
fn change_on(state: &SrState, kind: ChangeKind, node_id: NodeId) -> Option<ChangeTarget> {
    let focus = state.versions.live()?;
    let foreground = state.foreground.object();
    change_target(
        kind,
        node_id,
        focus.context.snapshot.id,
        focus.line.iter().map(|ancestor| ancestor.snapshot.id),
        |id| {
            !foreground.is_some_and(|object| {
                focus.line.iter().any(|ancestor| {
                    ancestor.snapshot.id == id
                        && ObjectKey::of(&ancestor.snapshot).is_same_object(&object)
                })
            })
        },
    )
}

/// A value change of `node_id` that is not the focus's: spoken, the value
/// alone, when the node is the focus's parent, its innermost ancestor, and
/// a slider that focuses its thumb ([`is_sensitive_slider`]), and the value
/// differs from the one it was last read with in the focus line, as NVDA's
/// slider speaks its value while its thumb is the focus
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
fn slider_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    let Some(focus) = state.versions.live_mut() else {
        return Vec::new();
    };
    let Some(parent) = focus
        .line
        .last()
        .map(|parent| &parent.snapshot)
        .filter(|parent| parent.id == node_id && is_sensitive_slider(parent))
    else {
        return Vec::new();
    };
    if parent.value == value {
        return Vec::new();
    }
    let source = source_of(parent);
    let mut line = focus.line.to_vec();
    if let Some(last) = line.last_mut() {
        last.snapshot.value.clone_from(&value);
    }
    focus.line = Arc::from(line);
    let Some(text) = value else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::value(text)],
            source: Some(source),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Handles a name change: when the focused node's name differs from the one
/// last spoken of it ([`SpokenRecord`]), the new name alone is spoken,
/// queued behind current speech, as NVDA does. A name change on any other
/// node, including an ancestor of the focus, is silent.
fn reduce_name_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    name: Option<&String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Name, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.versions.focus_mut().map(|focus| &mut focus.context) else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.name.as_ref() == name;
    spoken.name = name.cloned();
    focus.snapshot.name = name.cloned();
    if unchanged {
        return Vec::new();
    }
    let Some(text) = name.filter(|name| !name.trim().is_empty()) else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::label(text.clone())],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Announces `node`, an object an announcement read, with reason focus at
/// `priority`, as NVDA speaks an alert's or a toast's object: it never
/// moves the focus or the navigator, and the level spoken before plays no
/// part.
pub(crate) fn announce_object(
    trace_id: TraceId,
    node: &NodeSnapshot,
    priority: SpeechPriority,
) -> Effect {
    Effect::Speak(
        announce_node(
            trace_id,
            priority,
            node,
            // Its read names the position only as the settings report it.
            (Reason::Focus, &mut None, &mut CellMemory::default(), true),
        ),
        None,
    )
}

/// Toggles a reader setting from its key, says its new value as NVDA does,
/// and reports the settings for the shell to save.
pub(crate) fn toggle_setting(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
) -> Vec<Effect> {
    let settings = &mut state.settings;
    let segment = match command {
        ReviewCommand::ToggleFollowCaret => {
            settings.follow_caret = !settings.follow_caret;
            SegmentContent::Message(if settings.follow_caret {
                Message::CaretMovesReview
            } else {
                Message::CaretDoesNotMoveReview
            })
        }
        ReviewCommand::ToggleTypedCharacters => {
            settings.speak_typed_characters = settings.speak_typed_characters.next();
            SegmentContent::Phrase(verbatim_model::Phrase::SpeakTypedCharacters(
                settings.speak_typed_characters,
            ))
        }
        _ => {
            settings.speak_typed_words = settings.speak_typed_words.next();
            SegmentContent::Phrase(verbatim_model::Phrase::SpeakTypedWords(
                settings.speak_typed_words,
            ))
        }
    };
    vec![
        editing::speak(trace_id, vec![UtteranceSegment::new(segment)]),
        Effect::SettingsChanged(state.settings),
    ]
}

/// The navigator object `object` announced, without its text: an object
/// that may have text leaves its value out, its text said after the rest.
pub(crate) fn navigator_utterance(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
    reason: Reason,
) -> Effect {
    let positions = speaks_positions(state);
    let levels = (
        reason,
        &mut state.last_tree_level,
        &mut state.cell_memory,
        positions,
    );
    let utterance = if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, levels)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, levels)
    };
    Effect::Speak(utterance, None)
}

/// When `object` is the focus's reading, what was last spoken of the
/// focus becomes what is spoken now ([`announce_focus_object`]).
/// Moves what was last spoken of the focus to `object`, a live read of
/// the focus's own node just spoken, as NVDA's property cache of its one
/// focus object takes whatever is spoken of it
/// (`docs/parity/changes-and-selection.md`, "Name change on the focus").
pub(crate) fn focus_read_spoken(state: &mut SrState, object: &NodeSnapshot) {
    if let Some(focus) = state
        .versions
        .focus_mut()
        .map(|focus| &mut focus.context)
        .filter(|focus| focus.snapshot.id == object.id)
    {
        let ancestors = Arc::clone(&focus.spoken().ancestors);
        focus.spoken = SpokenRecord::of(object, ancestors);
    }
}

pub(crate) fn focus_spoken_now(state: &mut SrState, object: &NodeSnapshot) {
    if let Some(focus) = state
        .versions
        .focus_mut()
        .map(|focus| &mut focus.context)
        .filter(|focus| focus.snapshot == *object)
    {
        let ancestors = Arc::clone(&focus.spoken().ancestors);
        focus.spoken = SpokenRecord::of(object, ancestors);
    }
}

/// The utterance that announces `object` for a query, as
/// [`announce_navigator`] announces it, without its text: an object that
/// may have text leaves its value out, its text said after the rest.
/// Verbatim+Tab speaks its answer so (`crate::report_focus`). An answer of
/// the focus Core holds moves what was last spoken of it to what is spoken
/// now, as [`announce_focus_object`] does, so a change the answer spoke is
/// not spoken again when its event comes; nothing else of the focus
/// changes.
pub(crate) fn announce_query(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
) -> Utterance {
    focus_read_spoken(state, object);
    let positions = speaks_positions(state);
    let levels = (
        Reason::Query,
        &mut state.last_tree_level,
        &mut state.cell_memory,
        positions,
    );
    if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, levels)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, levels)
    }
}

/// The text the report-object copy press puts on the clipboard: the
/// object's name and value joined by a space, each included only when
/// present, matching what a user reading the object would expect to paste.
pub(crate) fn clipboard_text(node: &NodeSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(name) = node.name.as_ref().filter(|name| !name.is_empty()) {
        parts.push(name.clone());
    }
    if let Some(value) = node.value.as_ref().filter(|value| !value.is_empty()) {
        parts.push(value.clone());
    }
    parts.join(" ")
}

/// `text` spelled character by character, a space spoken as "space".
pub(crate) fn spelled(text: &str) -> Vec<UtteranceSegment> {
    text::spelled(text, false, None)
}

/// A node selected inside an element the focus controls ("Selection in a
/// list the focus controls" in `docs/nvda/events.md`), read in `window`:
/// spoken as a focus, interrupting, with the navigator moved to it, while
/// `controller` is still the live focus. The focus itself does not move.
pub(crate) fn reduce_controlled_selection(
    state: &mut SrState,
    trace_id: TraceId,
    controller: NodeId,
    (node, window): (&NodeSnapshot, crate::event_queue::RequestWindow),
) -> Vec<Effect> {
    if state
        .versions
        .live()
        .is_none_or(|focus| focus.context.snapshot.id != controller)
    {
        return Vec::new();
    }
    state.navigator = Some(Navigator::on(node.clone(), Some(window)));
    let positions = speaks_positions(state);
    let effects = vec![Effect::Speak(
        announce_node(
            trace_id,
            SpeechPriority::Interrupt,
            node,
            (
                Reason::Focus,
                &mut state.last_tree_level,
                &mut state.cell_memory,
                positions,
            ),
        ),
        None,
    )];
    effects
}

/// Shared handling for `ValueChanged` and `PropertyChanged(Value(..))`: both
/// speak the bare new value, only when the changed node is the focused one,
/// the value differs from the one last spoken of it ([`SpokenRecord`]), and
/// the role speaks its
/// value at all. An edit field or document never speaks its changes of
/// value ("When values and descriptions are spoken" in
/// `docs/nvda/speech.md`). A slider that focuses its thumb speaks its own
/// change of value while it is the focus's parent
/// ([`slider_value_changed`]).
fn reduce_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Value, node_id) != Some(ChangeTarget::Focus) {
        return slider_value_changed(state, trace_id, node_id, value);
    }
    let Some(focus) = state.versions.focus_mut().map(|focus| &mut focus.context) else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.value == value;
    spoken.value.clone_from(&value);
    focus.snapshot.value.clone_from(&value);
    // A focused progress bar with a number, on screen, is heard only as a
    // bar, by its read at its value change (`crate::progress`); off
    // screen, or with no number, it is an ordinary value change, as NVDA's
    // behaviour falls through.
    if focus.snapshot.role == Role::ProgressBar
        && !focus.snapshot.states.contains(State::Offscreen)
        && value.as_deref().and_then(Percent::parse).is_some()
    {
        return Vec::new();
    }
    if unchanged {
        return Vec::new();
    }
    let role = focus.snapshot.role;
    let Some(text) = value.filter(|_| speaks_value(role) && !reports_text_itself(role)) else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::value(text)],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// The theme's progress indication for the bar at `place` at `percent`,
/// once the percentage has moved by at least a percent from the last one
/// indicated at that place (`docs/nvda/object-model.md`, "How a progress
/// bar reports its value").
pub(crate) fn indicate_progress(
    state: &mut SrState,
    place: (i32, i32),
    percent: Percent,
) -> Vec<Effect> {
    let hundredths = percent.hundredths();
    let last = state
        .progress_reported
        .iter()
        .position(|(at, _)| *at == place);
    if let Some(index) = last {
        let (_, reported) = state.progress_reported.remove(index);
        if reported.abs_diff(hundredths) < 100 {
            state.progress_reported.push((place, reported));
            return Vec::new();
        }
    }
    state.progress_reported.push((place, hundredths));
    if state.progress_reported.len() > crate::state::PROGRESS_BARS_KEPT {
        state.progress_reported.remove(0);
    }
    vec![Effect::PlayEarcon(Earcon::Progress(percent.whole()))]
}

/// Handles a complete state-set replacement on the focused node (MSAA
/// `EVENT_OBJECT_STATECHANGE` and equivalent UIA property changes carry the
/// whole new set, not a delta). Diffs against the stored snapshot and
/// announces, queued, the gained states and the lost states
/// spoken by their absence, by the rules and in the order of "Which states
/// are spoken, and in what order" in `docs/nvda/speech.md`. Ignored for any
/// node other than the focused one; a no-op if nothing speakable changed,
/// and for states `from` an outpost's event that lack the focused state on
/// a focus that reported it, its states still kept ([`StatesFrom`]).
/// When the change makes the focus expanded and carries the node's number
/// of children, which the outpost reads only for a Win32 tree view item,
/// the number follows as an utterance of its own ("How many items an
/// expanded tree view item holds" in `docs/nvda/speech.md`).
fn reduce_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    (new_states, from): (StateSet, StatesFrom),
    child_count: Option<u32>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::States, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.versions.focus_mut().map(|focus| &mut focus.context) else {
        return Vec::new();
    };
    let role = focus.snapshot.role;
    let utterance_source = source_of(&focus.snapshot);
    let old_states = focus.spoken().states;
    focus.snapshot.states = new_states;
    if old_states == new_states {
        return Vec::new();
    }
    // An outpost's change can reach Core before the listener's focus event
    // for the control the focus went to, raised before it; its states,
    // from the event's cache, tell that the focus had moved when the
    // change was raised, so the focus came first, as NVDA hears it.
    if from == StatesFrom::OutpostEvent
        && focus.reported_focused
        && !new_states.contains(State::Focused)
    {
        return Vec::new();
    }
    focus.spoken().states = new_states;

    let gained = StateSet::from_iter(
        new_states
            .iter()
            .filter(|state| !old_states.contains(*state)),
    );
    let segments = state_change_segments(role, old_states, new_states);
    let items = child_count
        .filter(|_| gained.contains(State::Expanded))
        .map(|count| {
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Items(count),
            ))]
        });
    [
        Some(segments).filter(|segments| !segments.is_empty()),
        items,
    ]
    .into_iter()
    .flatten()
    .map(|segments| {
        Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(utterance_source),
                say_all: false,
                validity: None,
            },
            None,
        )
    })
    .collect()
}

/// What a change of a node's states from `old_states` to `new_states` speaks:
/// the gained states and the lost states spoken by their absence, by the
/// rules and in the order of "Which states are spoken, and in what order" in
/// `docs/nvda/speech.md`.
fn state_change_segments(
    role: Role,
    old_states: StateSet,
    new_states: StateSet,
) -> Vec<UtteranceSegment> {
    let gained = StateSet::from_iter(
        new_states
            .iter()
            .filter(|state| !old_states.contains(*state)),
    );
    let lost = StateSet::from_iter(
        old_states
            .iter()
            .filter(|state| !new_states.contains(*state)),
    );
    let positive = intersect(spoken_states(role, new_states, StateReason::Change), gained);
    let mut negative = intersect(negated_states(role, new_states, StateReason::Change), lost);
    // Losing half checked without becoming checked is a change to "not
    // checked".
    if lost.contains(State::Mixed) && !new_states.contains(State::Checked) {
        negative.insert(State::Checked);
    }
    ordered_state_segments(positive, negative)
}

/// A state change on an ancestor of the focus, spoken as NVDA's base state
/// change handler speaks one on the focus's ancestors ("The focus gate" in
/// `docs/nvda/events.md`): pressing a focused button can change the state of
/// a container above it, such as the sort state of a column header. The
/// change is diffed against the states last spoken of the ancestor
/// ([`SpokenRecord`]), which then become the new ones, as does its reading;
/// queued, as on the focus.
/// Ignored for a node that is neither the focus nor one of its ancestors.
fn reduce_ancestor_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    let Some(focus) = state.versions.live_mut() else {
        return Vec::new();
    };
    let Some(index) = focus
        .line
        .iter()
        .position(|ancestor| ancestor.snapshot.id == node_id)
    else {
        return Vec::new();
    };
    let Some(ancestor) = focus.line.get(index).map(|ancestor| &ancestor.snapshot) else {
        return Vec::new();
    };
    let (role, source) = (ancestor.role, source_of(ancestor));
    let old_states = focus.spoken_states(node_id).unwrap_or(ancestor.states);
    if ancestor.states != new_states {
        let mut line = focus.line.to_vec();
        if let Some(ancestor) = line.get_mut(index) {
            ancestor.snapshot.states = new_states;
        }
        focus.line = Arc::from(line);
    }
    if old_states == new_states {
        return Vec::new();
    }
    let spoken = focus.context.spoken();
    if let Some(entry) = spoken.ancestors.iter().position(|(id, _)| *id == node_id) {
        let mut entries = spoken.ancestors.to_vec();
        if let Some((_, states)) = entries.get_mut(entry) {
            *states = new_states;
        }
        spoken.ancestors = Arc::from(entries);
    }
    let segments = state_change_segments(role, old_states, new_states);
    if segments.is_empty() {
        return Vec::new();
    }
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(source),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// A description change on the focus, spoken as NVDA's base handler speaks
/// it ("The focus gate" in `docs/nvda/events.md`): the new description
/// alone, queued, when it differs from the one last spoken of it
/// ([`SpokenRecord`]) and has text, even when it is the focus's name over
/// again, a change's speech having no name to compare it with
/// (`docs/parity/changes-and-selection.md`, "A description change").
/// Ignored for any other node.
/// An editable field's suggestions appeared (`open`) or went away: the
/// focus's opening plays the suggestions-opened indication, and any other
/// change the suggestions-closed one, the focus test applying only to an
/// opening (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away").
fn reduce_suggestions(state: &SrState, node_id: NodeId, open: bool) -> Vec<Effect> {
    if open
        && state
            .versions
            .live()
            .is_some_and(|focus| focus.context.snapshot.id == node_id)
    {
        vec![Effect::PlayEarcon(Earcon::SuggestionsOpened)]
    } else {
        vec![Effect::PlayEarcon(Earcon::SuggestionsClosed)]
    }
}

fn reduce_description_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    description: Option<&String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Description, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.versions.focus_mut().map(|focus| &mut focus.context) else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.description.as_ref() == description;
    spoken.description = description.cloned();
    focus.snapshot.details.description = description.cloned();
    if unchanged {
        return Vec::new();
    }
    let Some(text) = description.filter(|text| !text.trim().is_empty()) else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::new(SegmentContent::Description(
                text.clone(),
            ))],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// What a navigation in direction `kind` says when there is no neighbor
/// that way, or the object it started from is not there: the direction's
/// edge message, NVDA's wording, "No next", "No previous", "No containing
/// object", "No objects inside" (`docs/parity/review.md`, "Dead objects");
/// the navigator stays put.
pub(crate) fn navigation_edge(trace_id: TraceId, kind: QueryKind) -> Vec<Effect> {
    let message = edge_message_of(kind);
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::new(SegmentContent::Message(message))],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// The edge message a navigation `QueryKind` speaks when there is no
/// neighbor in its direction — NVDA's messages, one per command.
pub(crate) fn edge_message_of(kind: QueryKind) -> verbatim_model::Message {
    use verbatim_model::Message;
    match kind {
        QueryKind::Parent => Message::NoContainingObject,
        QueryKind::NextSibling => Message::NoNextObject,
        QueryKind::PreviousSibling => Message::NoPreviousObject,
        QueryKind::FirstChild => Message::NoObjectsInside,
    }
}

/// The utterance-source metadata describing `node`, for presentation
/// themes (decision D12).
pub(crate) fn source_of(node: &NodeSnapshot) -> UtteranceSource {
    UtteranceSource {
        role: node.role,
        rect: node.details.rect,
    }
}

/// A new focus's place in Core's focus ancestry ([`focus_ancestry`]).
struct JoinedAncestry<'a> {
    /// The focus's ancestors as Core keeps them, outermost first: the
    /// previous focus ancestry up to the lowest common ancestor, then the
    /// new chain's entries below it.
    ancestors: Arc<[LineEntry]>,
    /// The entered ancestors: the new chain's entries below the lowest
    /// common ancestor, outermost first, the containers the focus has newly
    /// entered.
    entered: &'a [LineEntry],
    /// The two ancestries differ just below the desktop, sharing no entry:
    /// the focus is in a new foreground window, and speech is cut.
    cut: bool,
    /// The lowest common ancestor's index in the new chain, its ancestors
    /// outermost first and then the focus; `None` when the two share none.
    /// When they meet at an object they share, the focus has not moved to
    /// a new foreground window as NVDA judges it, by its focus difference
    /// level, and no window title is added for it ([`window_title`]).
    met: Option<usize>,
}

/// Joins a new focus's chain, `report`'s ancestors and node, to the
/// previous focus ancestry, `previous`'s ancestors and focus, at their
/// lowest common ancestor ([`lowest_common_ancestor`]), as NVDA builds its
/// focus ancestry when the focus moves
/// (`docs/parity/focus-context.md`, "Focus-ancestry context"). The new
/// chain is walked from the focus outward and stops at the first entry that
/// is the same object as one in the previous line: by node id, or by window
/// handle when both are windows, so that one window two outposts read is
/// one entry, such as the Settings frame its own process reports as its
/// focus and the page's process as a pane above the page. The previous
/// line's entries are kept to that point, as NVDA keeps its old objects,
/// so a window matched by its handle keeps the name it was entered with;
/// the new entries below it are the containers newly entered, an element
/// replaced by a new one with the same role and name, such as File
/// Explorer's items view list after a folder is opened, among them; the
/// outpost gives an MSAA object reached by another path the id NVDA's
/// equality finds it to have. The chains are whole, layout containers
/// included, as NVDA's focus ancestry is, so the meeting point is NVDA's
/// focus difference level and only speech filters what is not presentable
/// ([`is_presentable_container`]); the MSAA reader alone leaves out a
/// window object, as NVDA's own walk does, so the focus's top-level
/// window, which its window facts name, stands for the entry just below
/// the desktop ([`ancestry_top`]): with no object of the
/// chains shared, two lines in one top-level window meet there, empty
/// chains or not, the whole chain is entered and speech is not cut, and
/// lines in two top-level windows differ just below the desktop: the whole
/// chain is entered and speech is cut. With no previous focus, the first
/// focus Core takes, the whole chain is entered and speech is not cut, as
/// NVDA, starting, makes the focus its focus object before it handles the
/// focus, so the two lines meet at the focus itself.
fn focus_ancestry<'a>(
    previous: Option<&Line>,
    (window, top_level, previous_top): (
        Option<&WindowFacts>,
        Option<WindowHandle>,
        Option<WindowHandle>,
    ),
    report: &FocusReport<'a>,
) -> JoinedAncestry<'a> {
    let Some(line) = previous else {
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut: false,
            met: None,
        };
    };
    // A left focus is followed as a focus from another top-level window is:
    // the lines meet only at the desktop, and speech is cut.
    if line.left {
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut: true,
            met: None,
        };
    }
    let previous = &line.focus;
    let itself = previous.as_entry();
    let new_focus = LineEntry {
        snapshot: report.node.clone(),
        runtime_id: None,
        simple_child: false,
    };
    // A new element under the previous focus's node, the one it stood for
    // gone, is not that focus: the lines meet above it, if anywhere.
    let met = if report.replaces(&previous.context.snapshot) {
        previous.line.split_last().and_then(|(last, above)| {
            lowest_common_ancestor((above, last), (report.ancestors, &new_focus))
        })
    } else {
        lowest_common_ancestor((&previous.line, &itself), (report.ancestors, &new_focus))
    };
    let Some((at_old, at_new)) = met else {
        let old_top = ancestry_top(
            &previous.context.snapshot,
            previous.context.window.as_ref(),
            previous_top,
        );
        let new_top = ancestry_top(report.node, window, top_level);
        let cut = old_top.is_none() || old_top != new_top;
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut,
            met: None,
        };
    };
    // The previous line is kept through the meeting entry, the previous
    // focus among them when they meet there, unless the meeting entry
    // stands for the new focus itself.
    let kept = if at_new == report.ancestors.len() {
        at_old
    } else {
        at_old.saturating_add(1)
    };
    let entered = report
        .ancestors
        .get(at_new.saturating_add(1)..)
        .unwrap_or_default();
    JoinedAncestry {
        ancestors: previous
            .line
            .iter()
            .chain(std::iter::once(&itself))
            .take(kept)
            .chain(entered)
            .cloned()
            .collect(),
        entered,
        cut: false,
        met: Some(at_new),
    }
}

/// The top-level window just below the desktop in the focus line of
/// `focus`, as its window facts `window` name it, the entry a reader can
/// leave out of the chain as not presentable ([`focus_ancestry`]): `None`
/// for a focus whose containers an application's rule drops, whose
/// container is the desktop, as the task switcher's item's is
/// ([`crate::applications::drops_ancestry`]). A focus with no facts, one
/// Core read itself, is in the top-level window its read named,
/// `top_level`.
fn ancestry_top(
    focus: &NodeSnapshot,
    window: Option<&WindowFacts>,
    top_level: Option<WindowHandle>,
) -> Option<WindowHandle> {
    let Some(window) = window else {
        return top_level;
    };
    (!crate::applications::drops_ancestry(Some(window), focus)).then_some(window.top_level)
}

/// The top-level window of the committed focus's read, as Core keeps it.
fn committed_top(state: &SrState) -> Option<WindowHandle> {
    state
        .versions
        .focus()
        .map(|focus| focus.kept.place.top_level)
}

/// Whether a focus ancestor adds context worth speaking when focus first
/// enters it. The rule is exclusion-based: an ancestor speaks unless it is
/// one of the following.
///
/// - An item-level or text-entry role (`TreeItem`, `ListItem`,
///   `EditableText`). Focus lands on these; they are never context.
/// - A structural node with no semantics of its own (`Unknown`, `Pane`,
///   `Border`, `Whitespace`), which NVDA counts as layout whatever its
///   name (`docs/nvda/object-model.md`, "Presentation classification"),
///   except a pane that is the client area of a top-level window with a
///   system menu ([`verbatim_model::NodeDetails::top_level_client`]): it
///   stands for its window, which is presented as any entered window is,
///   and is spoken by its title alone, since a pane's role is silent on
///   focus.
/// - A menu bar, menu, or menu item: entering one is silent, as in NVDA.
/// - A `Window`, `Group`, or `PropertyPage` with neither a name nor a
///   description, which would speak as a bare role. NVDA treats these as
///   layout; a named window is announced on entry like any container. A
///   UI Automation ancestor's description counts whatever the theme
///   reports ([`verbatim_model::NodeDetails::described`]), as NVDA's
///   judgment reads it.
/// - `StaticText` with no text, which is nothing.
/// - Any node that is invisible ([`State::Invisible`]), which NVDA counts
///   unavailable, never presented in the focus ancestry.
/// - A UI Automation node that UI Automation does not count both a control
///   and a content element ([`verbatim_model::NodeDetails::uia_layout`]),
///   which NVDA takes for layout whatever its role
///   (`NVDAObjects/UIA/__init__.py` 2121 to 2128).
///
/// Whitespace-only names and descriptions count as absent. Everything else
/// speaks on entry whether named or not: dialogs, toolbars, and an unnamed
/// tree, which announces as a bare "tree view", the same as NVDA. The
/// behavior is recorded in `docs/parity/focus-context.md`, "Focus-ancestry
/// context".
fn is_presentable_container(node: &NodeSnapshot) -> bool {
    if node.states.contains(State::Invisible) || node.details.uia_layout {
        return false;
    }
    let named = has_text(node.name.as_deref());
    let described = node.details.described || has_text(node.details.description.as_deref());
    match node.role {
        // Never context: item-level and text-entry roles, structural roles
        // with no semantics, and menus. See this function's doc for the rule.
        Role::TreeItem
        | Role::ListItem
        | Role::EditableText
        | Role::ProgressBar
        | Role::BusyIndicator
        | Role::TitleBar
        | Role::Unknown
        | Role::Border
        | Role::Whitespace
        | Role::MenuBar
        | Role::Menu
        | Role::MenuItem => false,
        // A pane is structure, unless it is a top-level window's client
        // area read through MSAA, which stands for its window.
        Role::Pane => node.details.top_level_client,
        // Layout-when-unlabeled roles: content only when named or described.
        Role::Window | Role::Group | Role::PropertyPage => named || described,
        // Static text: content only when it has real, non-whitespace text.
        Role::StaticText => named,
        // Every other role is context, named or not.
        _ => true,
    }
}

/// The spoken introduction for one entered container: spoken as a focus
/// is, without its value, its level, or (except for a list) its keyboard
/// shortcut, which NVDA leaves out for an entered container ("When the role
/// is spoken" and "When values and descriptions are spoken" in
/// `docs/nvda/speech.md`). A slider that focuses its thumb is spoken as a
/// focus, whole ([`is_sensitive_slider`]).
fn container_segments(node: &NodeSnapshot, positions: bool) -> Vec<UtteranceSegment> {
    if is_sensitive_slider(node) {
        return unremembered_segments(node, Reason::Focus, positions);
    }
    let mut entered = node.clone();
    entered.value = None;
    entered.details.level = None;
    if entered.role != Role::List {
        entered.details.keyboard_shortcut = None;
    }
    // An entered container speaks no level and is read without a cell, so
    // nothing is remembered.
    unremembered_segments(&entered, Reason::Focus, positions)
}

/// [`node_segments`] for what remembers nothing it says: a window's title or
/// an entered container.
fn unremembered_segments(
    node: &NodeSnapshot,
    reason: Reason,
    positions: bool,
) -> Vec<UtteranceSegment> {
    node_segments(
        node,
        reason,
        (&mut None, &mut CellMemory::default()),
        positions,
    )
}

/// Whether a node's position in its set is spoken, as the settings report
/// it.
fn speaks_positions(state: &SrState) -> bool {
    state
        .reported
        .node
        .contains(verbatim_model::property::NodeDetail::Position)
}

/// Why a node is being spoken, which decides whether its role is
/// ("When the role is spoken" in `docs/nvda/speech.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Focus moved to it, it was selected in a list the focus controls, it
    /// is a toast, or object navigation moved the navigator to it.
    Focus,
    /// The user asked for it, as with reporting the current object.
    Query,
}

/// Roles left unspoken on focus when the node has a name or a value: an
/// item says its name, not "list item" after it. The rule and the role set
/// are described under "When the role is spoken" in `docs/nvda/speech.md`.
fn is_silent_on_focus(role: Role) -> bool {
    matches!(
        role,
        Role::Pane
            | Role::Unknown
            | Role::Application
            | Role::Cell
            | Role::ListItem
            | Role::MenuItem
            | Role::TreeItem
            | Role::StaticText
    )
}

/// Whether a role speaks its value: a check box, radio button, link, menu
/// item, or application does not ("When values and descriptions are
/// spoken" in `docs/nvda/speech.md`).
fn speaks_value(role: Role) -> bool {
    !matches!(
        role,
        Role::CheckBox
            | Role::RadioButton
            | Role::Link
            | Role::MenuItem
            | Role::Application
            | Role::BusyIndicator
    )
}

/// Whether a role reports its own text as it changes, so a change of value
/// is not spoken: an edit field or a document.
fn reports_text_itself(role: Role) -> bool {
    matches!(role, Role::EditableText | Role::Document)
}

/// Whether a node announced for `reason` speaks its role: always, unless
/// the reason is focus, the node has a name, a value, or a row or column in
/// a table to hear instead, and its role is one left silent on focus.
fn speaks_role(node: &NodeSnapshot, reason: Reason) -> bool {
    let something_else = node.name.as_deref().is_some_and(|name| !name.is_empty())
        || node.value.as_deref().is_some_and(|value| !value.is_empty())
        || node.details.cell.is_some();
    !(reason == Reason::Focus && something_else && is_silent_on_focus(node.role))
}

/// The full announcement for a node, in NVDA's property order: name, role,
/// value, states, description, keyboard shortcut, row and column in a table
/// (`docs/parity/focus-announcement.md`, "Table cell coordinates"),
/// position in set, level —
/// each as its semantic span kind, never anonymous text (decision D12).
/// Detail spans simply do not appear when the backend reported nothing; the
/// role does not appear when `reason` leaves it silent ([`speaks_role`]).
///
/// A tree or list item's level goes first instead when it differs from
/// `last_level`, the last level put first, which it then becomes ("Where
/// the level goes" in `docs/nvda/speech.md`). A cell's row or column is left
/// out when unchanged within one table ([`CellMemory::said`]).
fn node_segments(
    node: &NodeSnapshot,
    reason: Reason,
    (last_level, cell_memory): (&mut Option<u32>, &mut CellMemory),
    positions: bool,
) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    if node.details.desktop_window {
        segments.push(UtteranceSegment::new(SegmentContent::Message(
            verbatim_model::Message::Desktop,
        )));
    } else if let Some(name) = &node.name {
        segments.push(UtteranceSegment::label(name.clone()));
    }
    if speaks_role(node, reason) {
        segments.push(UtteranceSegment::new(SegmentContent::Role(node.role)));
    }
    if let Some(value) = node.value.as_ref().filter(|_| speaks_value(node.role)) {
        segments.push(UtteranceSegment::value(value.clone()));
    }
    segments.extend(state_segments(node.role, node.states, reason));
    // A description that only repeats the name is dropped.
    if let Some(description) = node
        .details
        .description
        .as_ref()
        .filter(|description| node.name.as_ref() != Some(*description))
    {
        segments.push(UtteranceSegment::new(SegmentContent::Description(
            description.clone(),
        )));
    }
    if let Some(shortcut) = &node.details.keyboard_shortcut {
        segments.push(UtteranceSegment::new(SegmentContent::Shortcut(
            shortcut.clone(),
        )));
    }
    if let Some(cell) = &node.details.cell {
        let (row, column) = cell_memory.said(cell);
        if let Some(row) = row {
            segments.push(UtteranceSegment::new(SegmentContent::Row(row)));
        }
        if let Some(column) = column {
            segments.push(UtteranceSegment::new(SegmentContent::Column(column)));
        }
    }
    if let Some(counts) = node.details.table_counts {
        // NVDA's counts are said for a query only, and leave the table
        // forgotten, so the next cell says both its row and its column
        // (`docs/parity/focus-announcement.md`, "Table cell coordinates").
        segments.push(UtteranceSegment::new(SegmentContent::Phrase(
            verbatim_model::Phrase::TableCounts {
                rows: counts.rows,
                columns: counts.columns,
            },
        )));
        cell_memory.forget_table();
    }
    // A focus read carries its position whatever the settings, for the
    // identity check (`crate::details::focus_set`): the setting decides
    // only whether it is spoken.
    if let Some(position) = node.details.position_in_set.filter(|_| positions) {
        segments.push(UtteranceSegment::new(SegmentContent::Position {
            position,
            set_size: node.details.set_size,
        }));
    }
    if let Some(level) = node.details.level {
        let segment = UtteranceSegment::new(SegmentContent::Level(level));
        if matches!(node.role, Role::TreeItem | Role::ListItem) && *last_level != Some(level) {
            *last_level = Some(level);
            segments.insert(0, segment);
        } else {
            segments.push(segment);
        }
    }
    segments
}

/// Builds a complete announcement utterance for a node (no entered-context
/// prefix): navigation, toast, and report-object announcements.
fn announce_node(
    trace_id: TraceId,
    priority: SpeechPriority,
    node: &NodeSnapshot,
    (reason, last_level, cell_memory, positions): (Reason, &mut Option<u32>, &mut CellMemory, bool),
) -> Utterance {
    Utterance {
        trace_id,
        priority,
        segments: node_segments(node, reason, (last_level, cell_memory), positions),
        source: Some(source_of(node)),
        say_all: false,
        validity: None,
    }
}

/// Why states are being spoken, which decides which of them are ("Which
/// states are spoken, and in what order" in `docs/nvda/speech.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum StateReason {
    /// Focus moved to the node, or the navigator did.
    Focus,
    /// The user asked for the node.
    Query,
    /// The focused node's states changed.
    Change,
}

impl From<Reason> for StateReason {
    fn from(reason: Reason) -> Self {
        match reason {
            Reason::Focus => Self::Focus,
            Reason::Query => Self::Query,
        }
    }
}

/// The order states are spoken in, positive or negated alike.
const STATE_ORDER: [State; 19] = [
    State::Disabled,
    State::Focused,
    State::Selected,
    State::Busy,
    State::Pressed,
    State::Checked,
    State::Mixed,
    State::ReadOnly,
    State::Multiline,
    State::Expanded,
    State::Collapsed,
    State::Invisible,
    State::Visited,
    State::Linked,
    State::HasPopup,
    State::Protected,
    State::Required,
    State::InvalidEntry,
    State::Offscreen,
];

/// The states in `a` that are also in `b`.
fn intersect(a: StateSet, b: StateSet) -> StateSet {
    StateSet::from_iter(a.iter().filter(|state| b.contains(*state)))
}

/// Of a node's `states`, the ones spoken as present for `reason`.
fn spoken_states(role: Role, states: StateSet, reason: StateReason) -> StateSet {
    let mut spoken = states;
    // Never worth hearing.
    spoken.remove(State::Selectable);
    spoken.remove(State::Focusable);
    spoken.remove(State::Checkable);
    // A combo box always has a popup.
    if role == Role::ComboBox {
        spoken.remove(State::HasPopup);
    }
    // Only a link says it was visited.
    if role != Role::Link {
        spoken.remove(State::Visited);
    }
    if reason == StateReason::Query {
        return spoken;
    }
    spoken.remove(State::Focused);
    // Linked is said only when it changes.
    if reason != StateReason::Change {
        spoken.remove(State::Linked);
    }
    spoken.remove(State::Offscreen);
    // Invisible is said only for a query (`docs/parity/focus-context.md`,
    // "Focus-ancestry context").
    spoken.remove(State::Invisible);
    // Selection is the expected state of a focused item.
    if reason != StateReason::Change
        && matches!(
            role,
            Role::ListItem | Role::TreeItem | Role::MenuItem | Role::Row | Role::CheckBox
        )
        && states.contains(State::Selectable)
    {
        spoken.remove(State::Selected);
    }
    if !matches!(role, Role::EditableText | Role::CheckBox) {
        spoken.remove(State::ReadOnly);
    }
    if role != Role::EditableText {
        spoken.remove(State::Multiline);
    }
    if role == Role::CheckBox {
        spoken.remove(State::Pressed);
    }
    // Whether a submenu is open is not worth hearing.
    if role == Role::MenuItem && spoken.contains(State::HasPopup) {
        spoken.remove(State::Expanded);
        spoken.remove(State::Collapsed);
    }
    spoken
}

/// The states whose absence from a node is spoken for `reason`. A change
/// speaks "not selected" and "not checked" only while the node still
/// reports itself focused: the item the focus is leaving loses its
/// selection before the focus event for the next one arrives, and that
/// loss is not worth hearing.
fn negated_states(role: Role, states: StateSet, reason: StateReason) -> StateSet {
    let mut negated = StateSet::new();
    let focused = reason != StateReason::Change || states.contains(State::Focused);
    if states.contains(State::Selectable)
        && states.contains(State::Focusable)
        && reason != StateReason::Query
        && focused
        && matches!(
            role,
            Role::ListItem
                | Role::TreeItem
                | Role::Row
                | Role::Cell
                | Role::ColumnHeader
                | Role::RowHeader
                | Role::CheckBox
        )
    {
        negated.insert(State::Selected);
    }
    if (matches!(role, Role::CheckBox | Role::RadioButton) || states.contains(State::Checkable))
        && !states.contains(State::Mixed)
        && focused
    {
        negated.insert(State::Checked);
    }
    if role == Role::ToggleButton {
        negated.insert(State::Pressed);
    }
    if reason == StateReason::Change {
        return negated;
    }
    StateSet::from_iter(negated.iter().filter(|state| !states.contains(*state)))
}

/// `positive` and `negative` as segments, in [`STATE_ORDER`].
fn ordered_state_segments(positive: StateSet, negative: StateSet) -> Vec<UtteranceSegment> {
    STATE_ORDER
        .into_iter()
        .filter_map(|state| {
            if positive.contains(state) {
                Some(UtteranceSegment::new(SegmentContent::State(state)))
            } else if negative.contains(state) {
                Some(UtteranceSegment::new(SegmentContent::NegatedState(state)))
            } else {
                None
            }
        })
        .collect()
}

/// The spoken states of a node announced for `reason`, by the rules and in
/// the order of "Which states are spoken, and in what order" in
/// `docs/nvda/speech.md`.
fn state_segments(role: Role, states: StateSet, reason: Reason) -> Vec<UtteranceSegment> {
    let reason = StateReason::from(reason);
    ordered_state_segments(
        spoken_states(role, states, reason),
        negated_states(role, states, reason),
    )
}
