//! A UI Automation notification at its turn (Table 12;
//! `docs/parity/alerts.md`, "UIA notification events"): spoken with no
//! request when it is from the committed focus's application; the shell's
//! window-snap results are spoken from File Explorer whatever the focus,
//! queued and never cutting. A notification whose check found no window is
//! dropped, but the snap results, which take the desktop window.
//!
//! Speech: a notification processed "most recent" or "important most
//! recent" cuts speech, or during say-all is spoken at Next, after the
//! line being read, until M6.3 gives speech an interrupt-and-resume
//! priority; every other processing is queued.

use verbatim_model::tell::TellWindow;
use verbatim_model::{
    Effect, NotificationProcessing, Pid, SpeechPriority, Utterance, UtteranceSegment,
};

use super::{Announcement, Kind};
use crate::entries::Entries;
use crate::event_queue::Cx;
use crate::reduce::SNAP_RESULTS_ACTIVITY;
use crate::state::SrState;

/// The application File Explorer's module speaks the snap results from.
const EXPLORER: &str = "explorer";

/// The function of a notification entry, which asks nothing.
pub(crate) fn run(cx: &mut Cx<Entries>, announcement: &Announcement) {
    let focus_process = focus_process(cx);
    let said = judged(&cx.facts(), announcement, focus_process);
    if let Some(said) = said {
        cx.emit(said);
    }
}

/// The application of the committed focus; with none, the application in
/// front, as NVDA's focus object always names one.
fn focus_process(cx: &Cx<Entries>) -> Option<Pid> {
    cx.facts()
        .versions
        .committed_read()
        .map(|kept| kept.window.process)
        .or_else(|| cx.foreground().map(|front| front.process))
}

/// What `announcement`, a notification, says, judged against the
/// committed focus's application `focus_process`.
pub(crate) fn judged(
    state: &SrState,
    announcement: &Announcement,
    focus_process: Option<Pid>,
) -> Option<Effect> {
    let Kind::Notification {
        text,
        processing,
        activity,
        ..
    } = &announcement.kind
    else {
        return None;
    };
    let tell = &announcement.tell;
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
            segments: vec![UtteranceSegment::text(text.clone())],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    ))
}
