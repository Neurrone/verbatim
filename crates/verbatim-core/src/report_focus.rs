//! Verbatim+Tab, a live focus query (`books/dev/src/reference/crates/verbatim-core.md`,
//! "Verbatim+Tab"): each press asks the application with the keyboard focus
//! for its focus now, a `ReadFocus` query with purpose `Report`, and the
//! answer is spoken as NVDA+Tab speaks its focus object, never taken as
//! Core's focus. Nothing is said until it answers.
//!
//! The press waiting for its answer is Core's ([`WaitingPress`]). A press
//! within the repeat interval while one waits is kept with it, so the answer
//! is spoken, then spelled, as NVDA runs its scripts in order; a press after
//! the interval asks again and replaces it, the older query's answer then
//! being late. Its endings are its answer, its query's end with no answer
//! ([`Input::ReadFocusUnanswered`]: abandoned at its deadline, or nothing to
//! ask, its outpost's end or no focus), a focus Core applies that was
//! observed after the press, and a replacing press. A focus observed before
//! the press and applied while it waits leaves it waiting, as NVDA runs
//! NVDA+Tab after the focus event queued before it and speaks both.

use serde::{Deserialize, Serialize};
use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{
    Effect, Input, Message, NodeSnapshot, NormalizedEvent, SpeechPriority, TraceId, Utterance,
};

use crate::state::SrState;
use crate::{editing, review_text};

/// Verbatim+Tab's press waiting for its query's answer: the press's trace,
/// which the query is asked and answered under, and the first and last
/// repeat counts kept with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WaitingPress {
    /// The trace of the press that asked.
    pub(crate) trace_id: TraceId,
    /// The repeat count of the press that asked.
    pub(crate) first: u8,
    /// The repeat count of the last press kept with it.
    pub(crate) last: u8,
    /// When the press that asked was made, in milliseconds on the run's
    /// shared clock; 0 when unknown.
    pub(crate) pressed_at_ms: u64,
}

/// Verbatim+Tab pressed at `pressed_at_ms`, `repeat` extra times in quick
/// succession, under `trace_id`: kept with a press already waiting when it
/// repeats it, else a new query, which replaces any press waiting.
pub(crate) fn pressed(
    state: &mut SrState,
    (trace_id, pressed_at_ms): (TraceId, u64),
    repeat: u8,
) -> Vec<Effect> {
    if repeat > 0
        && let Some(waiting) = state.report_press.as_mut()
    {
        waiting.last = waiting.last.max(repeat);
        return Vec::new();
    }
    state.report_press = Some(WaitingPress {
        trace_id,
        first: repeat,
        last: repeat,
        pressed_at_ms,
    });
    vec![Effect::ReadFocus(FocusPurpose::Report)]
}

/// The answer to Verbatim+Tab's query asked under `trace_id`, `event`: when
/// its press still waits, the focus it names is spoken for each press kept,
/// in order: the object alone, then its text, for the first press; its name
/// spelled for the second; spelled with character descriptions for a
/// third. An answer for no waiting press is late, and says nothing.
pub(crate) fn answered(
    state: &mut SrState,
    trace_id: TraceId,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    let NormalizedEvent::FocusChanged { node, .. } = event else {
        return Vec::new();
    };
    let Some(press) = state
        .report_press
        .filter(|press| press.trace_id == trace_id)
    else {
        return Vec::new();
    };
    state.report_press = None;
    let mut effects = Vec::new();
    for repeat in press.first..=press.last {
        effects.extend(speak(state, trace_id, node, repeat));
    }
    effects
}

/// Core applied a focus observed at `observed_at_us`: a waiting press ends,
/// its answer then late, when the focus was observed after it, in a later
/// millisecond, or when either time is unknown (0), as nothing orders them.
pub(crate) fn focus_applied(state: &mut SrState, observed_at_us: u64) {
    if state.report_press.is_some_and(|press| {
        press.pressed_at_ms == 0
            || observed_at_us == 0
            || observed_at_us / 1_000 > press.pressed_at_ms
    }) {
        state.report_press = None;
    }
}

/// What one press says of `node`, for its `repeat` count.
fn speak(state: &mut SrState, trace_id: TraceId, node: &NodeSnapshot, repeat: u8) -> Vec<Effect> {
    if repeat == 0 {
        let mut effects = vec![Effect::Speak(
            crate::reduce::announce_query(state, trace_id, node),
            None,
        )];
        effects.extend(editing::reported_text(state, trace_id, node));
        return effects;
    }
    let segments =
        review_text::spell_or_blank(node.name.as_deref().unwrap_or_default(), repeat > 1, None);
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(node)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Verbatim+Tab's query asked under `trace_id` ended with no answer
/// ([`Input::ReadFocusUnanswered`]): when its press still waits, it ends,
/// saying "No focus" unless the query was abandoned at its deadline.
pub(crate) fn unanswered(state: &mut SrState, trace_id: TraceId, abandoned: bool) -> Vec<Effect> {
    if state
        .report_press
        .is_none_or(|press| press.trace_id != trace_id)
    {
        return Vec::new();
    }
    state.report_press = None;
    if abandoned {
        return Vec::new();
    }
    vec![editing::speak(
        trace_id,
        review_text::message(Message::NoFocus),
    )]
}

/// The input that ends a waiting press with no answer, for the reducer's
/// dispatch.
pub(crate) fn ended(state: &mut SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::ReadFocusUnanswered {
            trace_id,
            abandoned,
        } => unanswered(state, *trace_id, *abandoned),
        _ => Vec::new(),
    }
}
