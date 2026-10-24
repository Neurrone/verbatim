//! What the shell's threads learn, and what the reducer says of it
//! (architecture section 2, "Only the reducer speaks"): the lifecycle
//! sounds, the error sound, an application that stopped responding, a lock
//! key's new state, the time and date, the clipboard's result, a shell
//! item that has gone, the theme page's previews, and the fake focus after
//! a menu or the Alt+Tab switcher closes. The shell reads each fact (a
//! key's state, the clock, the clipboard) and the reducer decides what is
//! said and how, so all speech is ordered, cancellable and recorded in one
//! place.

use verbatim_model::{
    Earcon, Effect, Input, LockKey, Message, Phrase, SegmentContent, SpeechPriority, TraceId,
    Utterance, UtteranceSegment,
};

use crate::state::SrState;

/// Handles one input the shell's threads produce; any other input does
/// nothing here.
pub(crate) fn reduce(state: &SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::Started => started(),
        Input::Exiting { replaced } => exiting(*replaced),
        Input::ErrorLogged => error_logged(),
        Input::ApplicationNotResponding { .. } => not_responding(),
        Input::LockKeyToggled { trace_id, key, on } => lock_key(*trace_id, *key, *on),
        Input::ClockRead {
            trace_id,
            repeat,
            time,
            date,
        } => clock(*trace_id, *repeat, time.as_ref(), date.as_ref()),
        Input::ClipboardCopied {
            trace_id,
            text,
            copied,
        } => clipboard(*trace_id, text, *copied),
        Input::ShellItemGone { trace_id, name } => shell_item_gone(*trace_id, name),
        Input::PreviewSpeech { trace_id, segments } => preview_speech(*trace_id, segments),
        Input::PreviewEarcon(earcon) => vec![Effect::PlayEarcon(*earcon)],
        Input::MenuOrSwitchEnded { ended_at_ms } => menu_or_switch_ended(state, *ended_at_ms),
        _ => Vec::new(),
    }
}

/// An utterance of `segments` with no source node, as the reducer speaks
/// what is not about any object.
fn utterance(
    trace_id: TraceId,
    priority: SpeechPriority,
    segments: Vec<UtteranceSegment>,
) -> Effect {
    Effect::Speak(Utterance {
        trace_id,
        priority,
        segments,
        source: None,
        say_all: false,
        validity: None,
    })
}

/// Verbatim has started: the start sound, as NVDA plays it. No start
/// message is spoken, as NVDA speaks none (`docs/parity/system.md`,
/// "Starting"): the first speech is the foreground's announcement.
fn started() -> Vec<Effect> {
    vec![Effect::PlayEarcon(Earcon::Start)]
}

/// Verbatim is exiting: the exit sound, unless a new instance is replacing
/// this one, which stops at once with no sound (Dickson, 2026-10-10;
/// `docs/parity/system.md`, "Replacing a running instance").
fn exiting(replaced: bool) -> Vec<Effect> {
    if replaced {
        Vec::new()
    } else {
        vec![Effect::PlayEarcon(Earcon::Exit)]
    }
}

/// An error was logged: the error sound, as NVDA plays one for a logged
/// error.
fn error_logged() -> Vec<Effect> {
    vec![Effect::PlayEarcon(Earcon::Error)]
}

/// An application stopped responding: reported once per stall, which the
/// shell has already judged.
fn not_responding() -> Vec<Effect> {
    vec![Effect::PlayEarcon(Earcon::AppNotResponding)]
}

/// A lock key's new state ("caps lock on"), interrupting, as NVDA
/// announces it once the key has changed it.
fn lock_key(trace_id: TraceId, key: LockKey, on: bool) -> Vec<Effect> {
    vec![utterance(
        trace_id,
        SpeechPriority::Interrupt,
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::LockKeyState { key, on },
        ))],
    )]
}

/// The report-time gesture: the time on its first press, the date on a
/// quick second, interrupting, as NVDA's script does. The operating system
/// formats both for the user's locale, so they are spoken as the text they
/// are. One that could not be formatted says nothing.
fn clock(
    trace_id: TraceId,
    repeat: u8,
    time: Option<&String>,
    date: Option<&String>,
) -> Vec<Effect> {
    let text = if repeat == 0 { time } else { date };
    text.map(|text| {
        utterance(
            trace_id,
            SpeechPriority::Interrupt,
            vec![UtteranceSegment::text(text.clone())],
        )
    })
    .into_iter()
    .collect()
}

/// A copy to the clipboard was made, confirmed by reading the clipboard
/// back, or failed: NVDA's "Copied to clipboard:" with the text, or
/// "Unable to copy", interrupting.
fn clipboard(trace_id: TraceId, text: &str, copied: bool) -> Vec<Effect> {
    let content = if copied {
        SegmentContent::Phrase(Phrase::CopiedToClipboard(text.to_owned()))
    } else {
        SegmentContent::Message(Message::UnableToCopy)
    };
    vec![utterance(
        trace_id,
        SpeechPriority::Interrupt,
        vec![UtteranceSegment::new(content)],
    )]
}

/// The shell item list dialog's chosen item was gone: said queued.
fn shell_item_gone(trace_id: TraceId, name: &str) -> Vec<Effect> {
    vec![utterance(
        trace_id,
        SpeechPriority::Queued,
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::ShellItemGone(name.to_owned()),
        ))],
    )]
}

/// The theme page's preview of an indication spoken, interrupting.
fn preview_speech(trace_id: TraceId, segments: &[UtteranceSegment]) -> Vec<Effect> {
    vec![utterance(
        trace_id,
        SpeechPriority::Interrupt,
        segments.to_vec(),
    )]
}

/// A menu or the Alt+Tab switcher closed at `ended_at_ms`: unless a focus
/// observed since then has been applied, the application in front is asked
/// for its focus, as NVDA's fake focus is queued when no focus event was
/// validly processed with the end; a focus fact dropped or unreadable does
/// not skip it (`docs/parity/backends.md`).
fn menu_or_switch_ended(state: &SrState, ended_at_ms: u64) -> Vec<Effect> {
    if state
        .latest_focus_observed_at()
        .is_some_and(|observed| observed >= ended_at_ms)
    {
        Vec::new()
    } else {
        vec![Effect::ReadForegroundFocus]
    }
}
