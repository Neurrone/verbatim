//! Scripted-input tests for what the reducer says of what the shell's
//! threads learn (architecture section 2, "Only the reducer speaks"): the
//! start, exit and error sounds, an application not responding, a lock
//! key's new state, the time and date, the clipboard's result, a shell
//! item gone, the theme page's previews, the fake focus after a menu
//! or the Alt+Tab switcher closes, and the keyboard hook's view of the
//! foreground window. Each asserts every effect its input produces.

use verbatim_core::{SrState, reduce};
use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{
    Backend, DesktopNews, Earcon, Effect, ForegroundWindow, HookForeground, Input, KeySeq, LockKey,
    Message, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, Role,
    SegmentContent, SpeechPriority, StateSet, TraceId, Utterance, UtteranceSegment, WindowFacts,
    WindowHandle,
};

/// An utterance of `content` alone, with no source node.
fn said(trace_id: TraceId, priority: SpeechPriority, content: SegmentContent) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority,
            segments: vec![UtteranceSegment::new(content)],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )
}

fn effects_of(input: &Input) -> Vec<Effect> {
    reduce(&mut SrState::new(), input)
}

#[test]
fn the_start_and_error_sounds_play_their_earcons() {
    assert_eq!(
        effects_of(&Input::Started),
        [Effect::PlayEarcon(Earcon::Start)]
    );
    assert_eq!(
        effects_of(&Input::ErrorLogged),
        [Effect::PlayEarcon(Earcon::Error)]
    );
}

/// Exiting cancels speech, then plays the exit sound, as NVDA does,
/// unless a new instance replaces this one; after it no input does
/// anything, so nothing is said after the exit sound.
#[test]
fn exiting_cancels_speech_then_plays_the_exit_sound_and_nothing_follows() {
    let mut state = SrState::new();
    assert_eq!(
        reduce(&mut state, &Input::Exiting { replaced: false }),
        [Effect::StopSpeech, Effect::PlayEarcon(Earcon::Exit)]
    );
    assert_eq!(reduce(&mut state, &Input::ErrorLogged), []);
    let mut state = SrState::new();
    assert_eq!(reduce(&mut state, &Input::Exiting { replaced: true }), []);
    assert_eq!(reduce(&mut state, &Input::ErrorLogged), []);
}

#[test]
fn a_lock_keys_new_state_is_said_interrupting() {
    let trace_id = TraceId::from_raw(1);
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
    let trace_id = TraceId::from_raw(2);
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
    let trace_id = TraceId::from_raw(3);
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
    let trace_id = TraceId::from_raw(4);
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
fn a_shell_item_unclickable_is_said_queued() {
    let trace_id = TraceId::from_raw(5);
    assert_eq!(
        effects_of(&Input::ShellItemUnclickable {
            trace_id,
            name: "Clock".to_owned(),
        }),
        [said(
            trace_id,
            SpeechPriority::Queued,
            SegmentContent::Phrase(Phrase::ShellItemUnclickable("Clock".to_owned())),
        )]
    );
}

#[test]
fn the_theme_pages_previews_are_spoken_interrupting_or_played() {
    let trace_id = TraceId::from_raw(6);
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
            trace_id: TraceId::from_raw(7),
            observed_at_us: 1_000,
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
                whole_window: false,
                ancestors: Vec::new(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                answers: None,
                first_cached_ancestor: None,
            },
            purpose: None,
        },
    );
    assert_eq!(state.latest_focus_observed_at(), Some(1_000));
    // A focus observed after the close was applied: nothing more is asked.
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_us: 900 }),
        []
    );
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_us: 1_000 }),
        []
    );
    // None since: the application in front is asked for its focus.
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_us: 1_001 }),
        [Effect::ReadFocus(
            verbatim_model::protocol::FocusPurpose::Fake
        )]
    );
}

/// A focus report of node `number` in window 10, observed at `us`: a
/// foreground change of the window when `foreground` holds.
fn focus_in_window(number: u64, foreground: bool, us: u64) -> Input {
    let window = WindowFacts {
        top_level: WindowHandle(10),
        root_owner: WindowHandle(10),
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
        foreground_window: verbatim_model::WindowHandle(0),
    };
    Input::Event {
        trace_id: TraceId::from_raw(8),
        observed_at_us: us,
        source: Pid(1),
        backend: Backend::Msaa,
        window: Some(window),
        event: NormalizedEvent::FocusChanged {
            node: NodeSnapshot {
                id: NodeId::in_outpost(OutpostId(1), number),
                backend: Backend::Msaa,
                role: if foreground {
                    Role::Window
                } else {
                    Role::Button
                },
                name: Some("OK".to_owned()),
                value: None,
                states: StateSet::new(),
                details: NodeDetails::default(),
            },
            whole_window: foreground,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        },
        purpose: None,
    }
}

/// A menu's end compares with the newest focus taken, not with a
/// foreground report the reducer refused: the refused report, observed
/// after the end, does not stand for a focus processed with it.
#[test]
fn a_menus_end_is_compared_with_the_focus_taken_only() {
    let mut state = SrState::new();
    let _ = reduce(&mut state, &focus_in_window(5, false, 1_000));
    assert_eq!(
        reduce(&mut state, &focus_in_window(6, true, 2_000)),
        [],
        "the focus is already in the window"
    );
    assert_eq!(state.latest_focus_observed_at(), Some(1_000));
    assert_eq!(
        reduce(&mut state, &Input::MenuOrSwitchEnded { ended_at_us: 1_500 }),
        [Effect::ReadFocus(
            verbatim_model::protocol::FocusPurpose::Fake
        )]
    );
}

/// A foreground report for window 7, raised by thread 40, of a process at
/// `integrity`, Verbatim being at `own`.
fn foreground(console: bool, thread: u32, integrity: Option<u32>, own: Option<u32>) -> Input {
    Input::Foreground(ForegroundWindow {
        window: WindowHandle(7),
        root_owner: WindowHandle(7),
        process: Pid(70),
        observed_at_us: 1,
        own: false,
        thread,
        console,
        integrity,
        own_integrity: own,
    })
}

/// The hook's view of the foreground once `input` is reduced, which must
/// say nothing.
fn hook_view_after(input: &Input) -> Option<HookForeground> {
    let mut state = SrState::new();
    assert_eq!(reduce(&mut state, input), []);
    state.hook_foreground()
}

const MEDIUM: u32 = 0x2000;
const HIGH: u32 = 0x3000;

/// Keys are held back for a window only when its process's integrity is
/// known and no higher than Verbatim's (`books/dev/src/explanation/design/key-holding.md`,
/// "Elevated windows"); nothing is known before the listener reports.
#[test]
fn keys_are_held_only_for_a_window_of_no_higher_integrity() {
    assert_eq!(SrState::new().hook_foreground(), None);
    let view = |integrity, own| {
        hook_view_after(&foreground(false, 40, integrity, own)).map(|view| view.holds_keys)
    };
    assert_eq!(view(Some(MEDIUM), Some(MEDIUM)), Some(true));
    assert_eq!(view(Some(0x1000), Some(MEDIUM)), Some(true));
    assert_eq!(view(Some(HIGH), Some(HIGH)), Some(true));
    assert_eq!(view(Some(HIGH), Some(MEDIUM)), Some(false));
    assert_eq!(view(None, Some(MEDIUM)), Some(false));
    assert_eq!(view(Some(MEDIUM), None), Some(false));
}

/// A console host window's keys are translated with the layout of the
/// thread its event named, any other window's with its owner's, which the
/// hook reads itself (`books/dev/src/reference/crates/verbatim-input-windows.md`, "Typed
/// text").
#[test]
fn a_console_window_takes_the_thread_its_event_named() {
    assert_eq!(
        hook_view_after(&foreground(true, 40, Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_has_text: false,
            keyboard_thread: Some(40),
        })
    );
    assert_eq!(
        hook_view_after(&foreground(true, 0, Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_has_text: false,
            keyboard_thread: None,
        }),
        "a console whose thread no event has named yet"
    );
    assert_eq!(
        hook_view_after(&foreground(false, 40, Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_has_text: false,
            keyboard_thread: None,
        })
    );
}

/// A later report replaces the earlier.
#[test]
fn the_latest_foreground_report_decides() {
    let mut state = SrState::new();
    assert_eq!(
        reduce(&mut state, &foreground(true, 40, Some(HIGH), Some(MEDIUM))),
        []
    );
    let next = Input::Foreground(ForegroundWindow {
        window: WindowHandle(9),
        root_owner: WindowHandle(9),
        process: Pid(90),
        observed_at_us: 2,
        own: false,
        thread: 50,
        console: false,
        integrity: Some(MEDIUM),
        own_integrity: Some(MEDIUM),
    });
    assert_eq!(reduce(&mut state, &next), []);
    assert_eq!(
        state.hook_foreground(),
        Some(HookForeground {
            window: WindowHandle(9),
            holds_keys: true,
            focus_has_text: false,
            keyboard_thread: None,
        })
    );
}

#[test]
fn a_lock_keys_announcement_is_heard_after_a_key_pressed_within_its_30_ms() {
    let mut state = SrState::new();
    // A focus whose typing is echoed.
    let _ = reduce(
        &mut state,
        &Input::Settings(verbatim_model::ReaderSettings {
            speak_typed_characters: verbatim_model::TypingEcho::Always,
            ..verbatim_model::ReaderSettings::default()
        }),
    );
    let _ = reduce(&mut state, &focus_in_window(5, false, 500_000));
    let effects = reduce(
        &mut state,
        &Input::ToggleKeyPressed {
            trace_id: TraceId::from_raw(9),
            key_seq: KeySeq::new(1),
            toggle: LockKey::ScrollLock,
            pressed_at_ms: 1_000,
        },
    );
    assert_eq!(effects, []);
    assert!(state.announces_toggle_key());
    assert_eq!(
        state.toggle_key_read_due(),
        Some((LockKey::ScrollLock, 1_030))
    );
    // "a", pressed as key 2 within the 30 ms, is spoken with its number.
    let letter = TraceId::from_raw(10);
    let effects = reduce(
        &mut state,
        &Input::CharacterTyped {
            trace_id: letter,
            key_seq: KeySeq::new(2),
            text: "a".to_owned(),
            at_ms: 1_010,
        },
    );
    let [Effect::Speak(utterance, key)] = effects.as_slice() else {
        panic!("expected the letter echoed: {effects:?}");
    };
    assert_eq!(
        (utterance.segments.as_slice(), *key),
        (
            [UtteranceSegment::new(SegmentContent::Character(
                "a".to_owned()
            ))]
            .as_slice(),
            Some(KeySeq::new(2))
        )
    );
    assert!(state.announces_toggle_key());
    // The announcement carries no number, so the letter's cancel does not
    // fence it off.
    let toggled = TraceId::from_raw(11);
    assert_eq!(
        reduce(
            &mut state,
            &Input::LockKeyToggled {
                trace_id: toggled,
                key: LockKey::ScrollLock,
                on: true,
            }
        ),
        [said(
            toggled,
            SpeechPriority::Interrupt,
            SegmentContent::Phrase(Phrase::LockKeyState {
                key: LockKey::ScrollLock,
                on: true
            }),
        )]
    );
    assert!(!state.announces_toggle_key());
}

/// A hang reported for a process whose focus is in the attention window
/// plays the sound, as a Settings page (`SystemSettings`) hangs inside
/// `ApplicationFrameHost`'s frame, which attention names; a hang in a
/// process with no focus there plays nothing.
#[test]
fn a_hang_in_the_process_focused_inside_the_attention_window_plays_the_sound() {
    let mut state = SrState::new();
    let frame = Input::Foreground(ForegroundWindow {
        window: WindowHandle(10),
        root_owner: WindowHandle(10),
        process: Pid(70),
        observed_at_us: 1,
        own: false,
        thread: 40,
        console: false,
        integrity: None,
        own_integrity: None,
    });
    assert_eq!(reduce(&mut state, &frame), []);
    let _ = reduce(&mut state, &focus_in_window(5, false, 1_000));
    assert_eq!(
        reduce(
            &mut state,
            &Input::ApplicationNotResponding {
                outpost: OutpostId(1),
                process: Pid(1),
            }
        ),
        [Effect::PlayEarcon(Earcon::AppNotResponding)]
    );
    assert_eq!(
        reduce(
            &mut state,
            &Input::ApplicationNotResponding {
                outpost: OutpostId(2),
                process: Pid(2),
            }
        ),
        []
    );
}

/// Every segment `effects` speak, in order.
fn spoken(effects: &[Effect]) -> Vec<UtteranceSegment> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(utterance.segments.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// A focus of node `number`, an "OK" button read through `backend`, with
/// `ancestors`, in window 10, observed at `us`, answering a focus query for
/// `purpose`, or a report for `None`.
fn button_focus(
    (number, backend): (u64, Backend),
    ancestors: Vec<NodeSnapshot>,
    us: u64,
    purpose: Option<FocusPurpose>,
) -> Input {
    let Input::Event {
        trace_id,
        source,
        window,
        event: NormalizedEvent::FocusChanged { mut node, .. },
        ..
    } = focus_in_window(number, false, us)
    else {
        unreachable!("a focus");
    };
    node.backend = backend;
    Input::Event {
        trace_id,
        observed_at_us: us,
        source,
        backend,
        window,
        event: NormalizedEvent::FocusChanged {
            node,
            whole_window: false,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        },
        purpose,
    }
}

/// The "OK" button's announcement.
fn ok_button() -> Vec<UtteranceSegment> {
    vec![
        UtteranceSegment::label("OK"),
        UtteranceSegment::new(SegmentContent::Role(Role::Button)),
    ]
}

/// A fake focus's MSAA answer of the MSAA focus Core holds is a duplicate
/// by NVDA's rule for MSAA objects, and says nothing.
#[test]
fn a_fake_focus_on_the_msaa_focus_core_holds_says_nothing() {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &button_focus((5, Backend::Msaa), Vec::new(), 1_000, None),
    );
    let effects = reduce(
        &mut state,
        &button_focus(
            (5, Backend::Msaa),
            Vec::new(),
            2_000,
            Some(FocusPurpose::Fake),
        ),
    );
    assert_eq!(effects, []);
}

/// A fake focus's UIA answer of the UIA focus Core holds is queued with no
/// duplicate test, as NVDA queues a UIA object, and is spoken again.
#[test]
fn a_fake_focus_on_the_uia_focus_core_holds_is_spoken_again() {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &button_focus((5, Backend::Uia), Vec::new(), 1_000, None),
    );
    let effects = reduce(
        &mut state,
        &button_focus(
            (5, Backend::Uia),
            Vec::new(),
            2_000,
            Some(FocusPurpose::Fake),
        ),
    );
    assert_eq!(spoken(&effects), ok_button());
}

/// A return to the user's desktop asks the application in front for its
/// focus again, as NVDA queues it again.
#[test]
fn the_focus_is_asked_for_after_a_return_to_the_users_desktop() {
    assert_eq!(
        effects_of(&Input::Desktop {
            trace_id: TraceId::from_raw(9),
            news: DesktopNews::Returned,
        }),
        [Effect::ReadFocus(FocusPurpose::Again)]
    );
}

/// The desktop's return's answer has no duplicate test, so the MSAA focus
/// Core holds is spoken again.
#[test]
fn a_desktop_return_s_focus_is_spoken_again_on_the_focus_core_holds() {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &button_focus((5, Backend::Msaa), Vec::new(), 1_000, None),
    );
    let effects = reduce(
        &mut state,
        &button_focus(
            (5, Backend::Msaa),
            Vec::new(),
            2_000,
            Some(FocusPurpose::Again),
        ),
    );
    assert_eq!(spoken(&effects), ok_button());
}

/// A list, outpost 1's node 3, named `name`.
fn list(name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::in_outpost(OutpostId(1), 3),
        backend: Backend::Uia,
        role: Role::List,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// The virtual desktop's name is kept until the next focus, which says it
/// before the containers it enters, as NVDA says it; its
/// due time then says nothing.
#[test]
fn the_virtual_desktops_name_is_spoken_before_the_next_focuss_containers() {
    let mut state = SrState::new();
    let name = Input::Desktop {
        trace_id: TraceId::from_raw(9),
        news: DesktopNews::Name("Desktop 2".to_owned()),
    };
    assert_eq!(reduce(&mut state, &name), []);
    let effects = reduce(
        &mut state,
        &button_focus((5, Backend::Uia), vec![list("Desktop")], 1_000, None),
    );
    let mut expected = vec![
        UtteranceSegment::text("Desktop 2"),
        UtteranceSegment::label("Desktop"),
        UtteranceSegment::new(SegmentContent::Role(Role::List)),
    ];
    expected.extend(ok_button());
    assert_eq!(spoken(&effects), expected);
    let due = Input::Desktop {
        trace_id: TraceId::from_raw(10),
        news: DesktopNews::NameDue,
    };
    assert_eq!(reduce(&mut state, &due), []);
}

/// A name no focus follows is spoken as a message at its due time,
/// queued, and only once.
#[test]
fn the_virtual_desktops_name_is_spoken_when_no_focus_follows() {
    let mut state = SrState::new();
    let name = Input::Desktop {
        trace_id: TraceId::from_raw(9),
        news: DesktopNews::Name("Desktop 2".to_owned()),
    };
    let due = Input::Desktop {
        trace_id: TraceId::from_raw(10),
        news: DesktopNews::NameDue,
    };
    assert_eq!(reduce(&mut state, &name), []);
    assert_eq!(
        reduce(&mut state, &due),
        [Effect::Speak(
            Utterance {
                trace_id: TraceId::from_raw(10),
                priority: SpeechPriority::Queued,
                segments: vec![UtteranceSegment::text("Desktop 2")],
                source: None,
                say_all: false,
                validity: None,
            },
            None,
        )]
    );
    assert_eq!(reduce(&mut state, &due), []);
}

/// A fake focus's answer carries the application's name and the Alt key's
/// state as a reported focus does, so the Alt+Tab switcher's item, read
/// with Alt held, drops its containers, none of them spoken.
#[test]
fn the_alt_tab_rule_applies_to_a_fake_focus_answer() {
    let mut state = SrState::new();
    let mut item = button_focus(
        (6, Backend::Uia),
        vec![list("Running applications")],
        1_000,
        Some(FocusPurpose::Fake),
    );
    let Input::Event {
        window: Some(window),
        event: NormalizedEvent::FocusChanged { node, .. },
        ..
    } = &mut item
    else {
        unreachable!("a focus in a window");
    };
    window.application = "explorer".to_owned();
    window.alt_held = true;
    node.role = Role::ListItem;
    node.name = Some("Notepad".to_owned());
    node.details.parent_automation_id = Some("SwitchItemListControl".to_owned());
    let effects = reduce(&mut state, &item);
    assert_eq!(
        spoken(&effects),
        [UtteranceSegment::label("Notepad")],
        "the item alone, its list not said"
    );
}
