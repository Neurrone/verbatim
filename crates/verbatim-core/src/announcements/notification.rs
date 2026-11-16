//! A UI Automation notification at its turn (Table 12): spoken with no
//! request when it is from the committed focus's application, as NVDA's
//! default drops a background application's (`NVDAObjects/UIA/__init__.py`
//! 2541 to 2543); the shell's window-snap results are spoken from File
//! Explorer whatever the focus, queued and never cutting, as its module
//! speaks them (`appModules/explorer.py` 527 to 528, 548 to 550). A
//! notification whose check found no window is dropped, as NVDA's default
//! `shouldProcessUIANotificationEvent` drops it (`appModuleHandler.py` 842
//! to 845), but the snap results, which take the desktop window.
//!
//! Speech, as NVDA's (`NVDAObjects/UIA/__init__.py` 2545 to 2556): a
//! notification processed "most recent" or "important most recent" cuts
//! speech, or during say-all is spoken without cutting; every other
//! processing is queued.

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
