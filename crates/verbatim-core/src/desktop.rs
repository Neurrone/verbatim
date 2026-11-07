//! The desktop's news from the focus listener
//! (`docs/design/focus-pipeline-rework-remaining.md`, "The desktop switch and
//! the virtual desktop's name"): the user's desktop returned, when the
//! application in front is asked for its focus again, as NVDA queues it
//! again; and the virtual desktop's name, kept until the next focus Core
//! takes speaks it, as NVDA speaks it before any focus, or spoken as a
//! message at its due time.

use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{DesktopNews, Effect, SpeechPriority, TraceId, Utterance, UtteranceSegment};

use crate::state::SrState;

/// Takes `news` of the desktop, under `trace_id`.
pub(crate) fn news(state: &mut SrState, trace_id: TraceId, news: &DesktopNews) -> Vec<Effect> {
    match news {
        DesktopNews::Returned => vec![Effect::ReadFocus(FocusPurpose::Again)],
        // The newest name is kept, as NVDA keeps one.
        DesktopNews::Name(name) => {
            state.desktop_name = Some(name.clone());
            Vec::new()
        }
        DesktopNews::NameDue => state
            .desktop_name
            .take()
            .map(|name| name_utterance(trace_id, name))
            .into_iter()
            .collect(),
    }
}

/// The virtual desktop's name kept, for the focus Core is taking, which
/// speaks it after the foreground's cancel and its title, and before the
/// entered containers; the name is no longer kept.
pub(crate) fn name_for_focus(state: &mut SrState, trace_id: TraceId) -> Option<Effect> {
    state
        .desktop_name
        .take()
        .map(|name| name_utterance(trace_id, name))
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
