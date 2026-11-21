//! A UI Automation notification at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::tell::TellWindow;
use verbatim_model::{
    Effect, NotificationProcessing, Pid, SpeechPriority, Utterance, UtteranceSegment,
};

use crate::entries::Entries;
use crate::event_queue::Cx;
use crate::reduce::SNAP_RESULTS_ACTIVITY;
use crate::state::SrState;
use verbatim_model::tell::Event;

/// The application File Explorer's module speaks the snap results from.
const EXPLORER: &str = "explorer";

/// The function of a notification entry, which asks nothing: `tell`
/// carrying `notification`, its text, how it asks to be processed and its
/// activity id.
pub(crate) fn run(
    cx: &mut Cx<Entries>,
    tell: &Event,
    notification: (&str, NotificationProcessing, &str),
) {
    let focus_process = focus_process(cx);
    let said = judged(&cx.facts(), tell, notification, focus_process);
    if let Some(said) = said {
        cx.emit(said);
    }
}

/// The application of the committed focus; with none, the application in
/// front, as NVDA's focus object always names one.
fn focus_process(cx: &Cx<Entries>) -> Option<Pid> {
    cx.facts()
        .versions
        .live()
        .map(|focus| focus.kept.asked.process)
        .or_else(|| cx.foreground().map(|front| front.process))
}

/// What `tell`, carrying `notification` ([`run`]), says, judged against
/// the committed focus's application `focus_process`.
pub(crate) fn judged(
    state: &SrState,
    tell: &Event,
    (text, processing, activity): (&str, NotificationProcessing, &str),
    focus_process: Option<Pid>,
) -> Option<Effect> {
    let snap = activity == SNAP_RESULTS_ACTIVITY && tell.application == EXPLORER;
    let windowed = matches!(tell.window, TellWindow::Found(_));
    if !snap && (!windowed || Some(tell.process) != focus_process) {
        return None;
    }
    if text.is_empty() {
        return None;
    }
    let priority = if snap {
        SpeechPriority::Queued
    } else {
        match processing {
            NotificationProcessing::MostRecent | NotificationProcessing::ImportantMostRecent => {
                if state.say_all.is_some() {
                    SpeechPriority::Next
                } else {
                    SpeechPriority::Interrupt
                }
            }
            _ => SpeechPriority::Queued,
        }
    };
    Some(Effect::Speak(
        Utterance {
            trace_id: tell.trace_id,
            priority,
            segments: vec![UtteranceSegment::text(text.to_owned())],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    ))
}
