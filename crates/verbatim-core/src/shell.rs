//! What the shell's threads learn, and what the reducer says of it
//! (architecture section 2, "Only the reducer speaks"): the lifecycle
//! sounds, the error sound, a lock key's new state, the time and date, the
//! clipboard's result, a shell item that has gone, and the theme page's
//! previews. The shell
//! reads each fact (a key's state, the clock, the clipboard) and the
//! reducer decides what is said and how, so all speech is ordered,
//! cancellable and recorded in one place. The scripts of the gestures that
//! are not review commands, the menu, the time and the shell's lists, are
//! here too: each asks the shell to read or show something
//! (`books/dev/src/explanation/design/ownership.md`).

use verbatim_model::{
    Earcon, Effect, GuiRequest, Input, KeySeq, LockKey, Message, Phrase, Script, SegmentContent,
    ShellItemKind, SpeechPriority, TraceId, Utterance, UtteranceSegment,
};

use crate::state::SrState;

/// Handles one input the shell's threads produce; any other input does
/// nothing here.
pub(crate) fn reduce(state: &mut SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::Started => started(),
        Input::Exiting { replaced } => {
            state.exiting = true;
            exiting(*replaced)
        }
        Input::ErrorLogged => error_logged(),
        Input::LockKeyToggled { trace_id, key, on } => {
            // The announcement at the front has come; one from a dump made
            // before announcements were kept has nothing to take.
            state.toggle_keys.pop_front();
            lock_key(*trace_id, *key, *on)
        }
        Input::ClockRead {
            trace_id,
            repeat,
            key,
            time,
            date,
        } => clock(*trace_id, *repeat, *key, time.as_ref(), date.as_ref()),
        Input::ClipboardCopied {
            trace_id,
            text,
            copied,
        } => clipboard(*trace_id, text, *copied),
        Input::ShellItemGone { trace_id, name } => {
            shell_item_not_clicked(*trace_id, Phrase::ShellItemGone(name.to_owned()))
        }
        Input::ShellItemUnclickable { trace_id, name } => {
            shell_item_not_clicked(*trace_id, Phrase::ShellItemUnclickable(name.to_owned()))
        }
        Input::ShellItemClickFailed { trace_id, name } => {
            shell_item_not_clicked(*trace_id, Phrase::ShellItemClickFailed(name.to_owned()))
        }
        Input::BlockedByMessageBox { trace_id } => blocked_by_message_box(*trace_id),
        Input::PreviewSpeech { trace_id, segments } => preview_speech(*trace_id, segments),
        Input::PreviewEarcon(earcon) => vec![Effect::PlayEarcon(*earcon)],
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
    Effect::Speak(
        Utterance {
            trace_id,
            priority,
            segments,
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )
}

/// Runs a gesture's script, pressed `repeat` extra times in quick
/// succession as key `key`: the time is read, or the menu or a shell list
/// shown.
pub(crate) fn script(script: Script, repeat: u8, key: KeySeq) -> Vec<Effect> {
    vec![match script {
        Script::ReportTime => Effect::ReadClock { repeat, key },
        Script::ShowMenu => Effect::Show(GuiRequest::Menu),
        Script::ShellList => Effect::Show(GuiRequest::ShellList(shell_list_kind(repeat))),
    }]
}

/// The shell surface the tray-list gesture lists: the system tray on a
/// single press (`repeat` 0), the taskbar on a double press.
pub(crate) fn shell_list_kind(repeat: u8) -> ShellItemKind {
    if repeat == 0 {
        ShellItemKind::SystemTray
    } else {
        ShellItemKind::Taskbar
    }
}

/// How long after a lock key reached the operating system its new state is
/// read and announced, as NVDA does: Windows has changed the state by then.
pub(crate) const TOGGLE_KEY_DELAY_MS: u64 = 30;

/// A lock key reached the operating system at `pressed_at_ms`, its input's
/// arrival: its
/// announcement is to come, in press order, once the shell has read the
/// key's state [`TOGGLE_KEY_DELAY_MS`] later, when the front read's due
/// time ([`SrState::toggle_key_read_due`]) is past. It never waits behind
/// a held key, as NVDA starts the announcement's timer when the key is
/// pressed, outside its script queue.
pub(crate) fn toggle_key_pressed(
    state: &mut SrState,
    toggle: LockKey,
    pressed_at_ms: u64,
) -> Vec<Effect> {
    state
        .toggle_keys
        .push_back((toggle, pressed_at_ms.saturating_add(TOGGLE_KEY_DELAY_MS)));
    Vec::new()
}

/// Verbatim has started: the start sound, as NVDA plays it. No start
/// message is spoken, as NVDA speaks none (`docs/parity/system.md`,
/// "Starting"): the first speech is the foreground's announcement.
fn started() -> Vec<Effect> {
    vec![Effect::PlayEarcon(Earcon::Start)]
}

/// Verbatim is exiting: speech is cancelled, then the exit sound plays, as
/// NVDA exits (Dickson, 2026-11-03; `docs/parity/system.md`, "Exiting"),
/// unless a new instance is replacing this one, which stops at once with
/// no sound (Dickson, 2026-10-10; "Replacing a running instance").
fn exiting(replaced: bool) -> Vec<Effect> {
    if replaced {
        Vec::new()
    } else {
        vec![Effect::StopSpeech, Effect::PlayEarcon(Earcon::Exit)]
    }
}

/// An error was logged: the error sound, as NVDA plays one for a logged
/// error.
fn error_logged() -> Vec<Effect> {
    vec![Effect::PlayEarcon(Earcon::Error)]
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
/// are, carrying the gesture's key number `key`. One that could not be
/// formatted says nothing.
fn clock(
    trace_id: TraceId,
    repeat: u8,
    key: KeySeq,
    time: Option<&String>,
    date: Option<&String>,
) -> Vec<Effect> {
    let text = if repeat == 0 { time } else { date };
    text.map(|text| {
        Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Interrupt,
                segments: vec![UtteranceSegment::text(text.clone())],
                source: None,
                say_all: false,
                validity: None,
            },
            Some(key),
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

/// The shell item list dialog's chosen item was not clicked, for the
/// reason `phrase` says: said queued.
fn shell_item_not_clicked(trace_id: TraceId, phrase: Phrase) -> Vec<Effect> {
    vec![utterance(
        trace_id,
        SpeechPriority::Queued,
        vec![UtteranceSegment::new(SegmentContent::Phrase(phrase))],
    )]
}

/// The settings dialog or Exit was refused while a modal message box is
/// up: NVDA's message, which it speaks at its priority NOW
/// (`docs/parity/system.md`, "A settings file that fails to load"); Next
/// until M6.3 brings the interrupting priority that resumes.
fn blocked_by_message_box(trace_id: TraceId) -> Vec<Effect> {
    vec![utterance(
        trace_id,
        SpeechPriority::Next,
        vec![UtteranceSegment::new(SegmentContent::Message(
            Message::DialogRequiresResponse,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_press_lists_the_system_tray() {
        assert_eq!(shell_list_kind(0), ShellItemKind::SystemTray);
    }

    #[test]
    fn a_double_press_lists_the_taskbar() {
        assert_eq!(shell_list_kind(1), ShellItemKind::Taskbar);
    }
}
