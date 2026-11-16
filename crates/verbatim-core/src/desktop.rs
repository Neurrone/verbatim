//! The desktop's news from the focus listener
//! (`books/dev/src/reference/adr/0001-focus.md`, "The desktop's return and name"): the user's
//! desktop returned, when the
//! application in front is asked for its focus again, as NVDA queues it
//! again; and the virtual desktop's name, kept until the next focus Core
//! takes speaks it, as NVDA speaks it before any focus, or spoken as a
//! message at its due time.

use serde::{Deserialize, Serialize};
use verbatim_model::{Effect, SpeechPriority, TraceId, Utterance, UtteranceSegment};

use crate::state::SrState;

/// The virtual desktop's name, kept until the next focus or its due time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DesktopName {
    /// The name.
    name: String,
    /// The trace of the tell that changed it.
    trace_id: TraceId,
    /// When it is said alone, if no focus said it first.
    due_us: u64,
}

/// The virtual desktop's name changed to `name` under `trace_id`: the
/// newest name is kept, as NVDA keeps one, said alone at `due_us` unless a
/// focus says it first.
pub(crate) fn named(state: &mut SrState, trace_id: TraceId, name: String, due_us: u64) {
    state.desktop_name = Some(DesktopName {
        name,
        trace_id,
        due_us,
    });
}

/// When the kept name is said alone.
pub(crate) fn due(state: &SrState) -> Option<u64> {
    state.desktop_name.as_ref().map(|name| name.due_us)
}

/// The time `now_us` came: a kept name whose due time has passed is said
/// alone, queued, and no longer kept.
pub(crate) fn tick(state: &mut SrState, now_us: u64) -> Option<Effect> {
    if due(state).is_none_or(|due_us| due_us > now_us) {
        return None;
    }
    state
        .desktop_name
        .take()
        .map(|name| name_utterance(name.trace_id, name.name))
}

/// The virtual desktop's name kept, for the focus Core is taking, which
/// speaks it after the foreground's cancel and its title, and before the
/// entered containers; the name is no longer kept.
pub(crate) fn name_for_focus(state: &mut SrState, trace_id: TraceId) -> Option<Effect> {
    state
        .desktop_name
        .take()
        .map(|name| name_utterance(trace_id, name.name))
}

/// The name spoken as NVDA speaks a message: queued, about no object.
fn name_utterance(trace_id: TraceId, name: String) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::text(name)],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )
}
