//! Scripted-input tests for what the reducer says of what the shell's
//! threads learn (architecture section 2, "Only the reducer speaks"): the
//! start, exit and error sounds, an application not responding, a lock
//! key's new state, the time and date, the clipboard's result, a shell
//! item gone, the theme page's previews, and the fake focus after a menu
//! or the Alt+Tab switcher closes. Each asserts every effect its input
//! produces.

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, Earcon, Effect, Input, LockKey, Message, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, Role, SegmentContent, SpeechPriority, StateSet,
    TraceId, Utterance, UtteranceSegment,
};

/// An utterance of `content` alone, with no source node.
fn said(trace_id: TraceId, priority: SpeechPriority, content: SegmentContent) -> Effect {
    Effect::Speak(Utterance {
        trace_id,
        priority,
        segments: vec![UtteranceSegment::new(content)],
        source: None,
        say_all: false,
        validity: None,
    })
}

fn effects_of(input: &Input) -> Vec<Effect> {
    reduce(&mut SrState::new(), input)
}

#[test]
fn the_start_and_error_sounds_and_not_responding_play_their_earcons() {
    assert_eq!(
        effects_of(&Input::Started),
        [Effect::PlayEarcon(Earcon::Start)]
    );
    assert_eq!(
        effects_of(&Input::ErrorLogged),
        [Effect::PlayEarcon(Earcon::Error)]
    );
    assert_eq!(
        effects_of(&Input::ApplicationNotResponding {
            outpost: OutpostId(3)
        }),
        [Effect::PlayEarcon(Earcon::AppNotResponding)]
    );
}

#[test]
fn the_exit_sound_plays_unless_a_new_instance_replaces_this_one() {
    assert_eq!(
        effects_of(&Input::Exiting { replaced: false }),
        [Effect::PlayEarcon(Earcon::Exit)]
    );
    assert_eq!(effects_of(&Input::Exiting { replaced: true }), []);
}

#[test]
fn a_lock_keys_new_state_is_said_interrupting() {
    let trace_id = TraceId::mint();
    assert_eq!(
        effects_of(&Input::LockKeyToggled {
            trace_id,
            key: LockKey::ScrollLock,
            on: true,
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Phrase(Phrase::LockKeyState {
                key: LockKey::ScrollLock,
                on: true
            }),
        )]
    );
}

#[test]
fn the_time_is_said_on_a_first_press_and_the_date_on_a_second() {
    let trace_id = TraceId::mint();
    let clock = |repeat| Input::ClockRead {
        trace_id,
        repeat,
        time: Some("10:30 AM".to_owned()),
        date: Some("Saturday, October 24, 2026".to_owned()),
    };
    assert_eq!(
        effects_of(&clock(0)),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Text("10:30 AM".to_owned()),
        )]
    );
    assert_eq!(
        effects_of(&clock(1)),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Text("Saturday, October 24, 2026".to_owned()),
        )]
    );
    // One the system could not format says nothing.
    assert_eq!(
        effects_of(&Input::ClockRead {
            trace_id,
            repeat: 0,
            time: None,
            date: Some("Saturday".to_owned()),
        }),
        []
    );
}

#[test]
fn a_copy_is_confirmed_with_its_text_or_said_to_have_failed() {
    let trace_id = TraceId::mint();
    assert_eq!(
        effects_of(&Input::ClipboardCopied {
            trace_id,
            text: "Name".to_owned(),
            copied: true,
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Phrase(Phrase::CopiedToClipboard("Name".to_owned())),
        )]
    );
    assert_eq!(
        effects_of(&Input::ClipboardCopied {
            trace_id,
            text: "Name".to_owned(),
            copied: false,
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Message(Message::UnableToCopy),
        )]
    );
}

#[test]
fn a_shell_item_gone_is_said_queued() {
    let trace_id = TraceId::mint();
    assert_eq!(
        effects_of(&Input::ShellItemGone {
            trace_id,
            name: "Clock".to_owned(),
        }),
        [said(
            trace_id,
            SpeechPriority::Queued,
            SegmentContent::Phrase(Phrase::ShellItemGone("Clock".to_owned())),
        )]
    );
}

#[test]
fn the_theme_pages_previews_are_spoken_interrupting_or_played() {
    let trace_id = TraceId::mint();
    assert_eq!(
        effects_of(&Input::PreviewSpeech {
            trace_id,
            segments: vec![UtteranceSegment::new(SegmentContent::Label(
                "Sample".to_owned()
            ))],
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Label("Sample".to_owned()),
        )]
    );
    assert_eq!(
        effects_of(&Input::PreviewEarcon(Earcon::Exit)),
        [Effect::PlayEarcon(Earcon::Exit)]
    );
}

#[test]
fn a_menus_end_asks_for_the_foreground_focus_unless_a_later_focus_was_applied() {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 1_000,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: NodeId::in_outpost(OutpostId(1), 5),
                    backend: Backend::Uia,
                    role: Role::Button,
                    name: Some("OK".to_owned()),
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
                text: None,
            },
        },
    );
    assert_eq!(state.latest_focus_observed_at(), Some(1_000));
    // A focus observed after the close was applied: nothing more is asked.
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_ms: 900 }),
        []
    );
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_ms: 1_000 }),
        []
    );
    // None since: the application in front is asked for its focus.
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_ms: 1_001 }),
        [Effect::ReadForegroundFocus]
    );
}
