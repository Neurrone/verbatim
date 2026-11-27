//! Scripted-input tests for what the reducer says of what the shell's
//! threads learn (architecture section 2, "Only the reducer speaks"): the
//! start, exit and error sounds, an application not responding, a lock
//! key's new state, the time and date, the clipboard's result, a shell
//! item gone, the theme page's previews, the fake focus after a menu
//! or the Alt+Tab switcher closes, and the keyboard hook's view of the
//! foreground window. Each asserts every effect its input produces.

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{
    App, Desk, FRAME, Heard, LISTENER, ONE, TWO, focus_heard, focus_now, focus_now_in,
    focus_now_with, focus_read_of, focus_tell, listener_read,
};
use readers::Found;
use verbatim_core::{SrState, reduce};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusRead, ForegroundReport, HeldIds, MsaaIds, NodeRead, Outcome, Press, PressAction,
    QueueInput, Reader, RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, Earcon, Effect, HookForeground, Input, KeySeq, LockKey, Message, NodeDetails,
    NodeSnapshot, Phrase, Pid, Role, SegmentContent, SpeechPriority, State, StateSet, TraceId,
    Utterance, UtteranceSegment, WindowHandle,
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

fn effects_of(arrival: &verbatim_model::Arrival) -> Vec<Effect> {
    reduce(
        &mut SrState::new(),
        &mut verbatim_core::Runtime::default(),
        arrival,
    )
}

#[test]
fn the_start_and_error_sounds_play_their_earcons() {
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::Started,
        }),
        [Effect::PlayEarcon(Earcon::Start)]
    );
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ErrorLogged,
        }),
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
        reduce(
            &mut state,
            &mut verbatim_core::Runtime::default(),
            &verbatim_model::Arrival {
                at_us: 0,
                input: Input::Exiting { replaced: false },
            }
        ),
        [Effect::StopSpeech, Effect::PlayEarcon(Earcon::Exit)]
    );
    assert_eq!(
        reduce(
            &mut state,
            &mut verbatim_core::Runtime::default(),
            &verbatim_model::Arrival {
                at_us: 0,
                input: Input::ErrorLogged,
            }
        ),
        []
    );
    let mut state = SrState::new();
    assert_eq!(
        reduce(
            &mut state,
            &mut verbatim_core::Runtime::default(),
            &verbatim_model::Arrival {
                at_us: 0,
                input: Input::Exiting { replaced: true },
            }
        ),
        []
    );
    assert_eq!(
        reduce(
            &mut state,
            &mut verbatim_core::Runtime::default(),
            &verbatim_model::Arrival {
                at_us: 0,
                input: Input::ErrorLogged,
            }
        ),
        []
    );
}

#[test]
fn a_lock_keys_new_state_is_said_interrupting() {
    let trace_id = TraceId::from_raw(1);
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::Queue {
                trace_id,
                input: verbatim_model::request::QueueInput::LockKeyRead {
                    key: LockKey::ScrollLock,
                    on: true
                }
            },
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
    let key = KeySeq::new(4);
    let clock = |repeat| verbatim_model::Arrival {
        at_us: 0,
        input: Input::ClockRead {
            trace_id,
            repeat,
            key,
            time: Some("10:30 AM".to_owned()),
            date: Some("Saturday, October 24, 2026".to_owned()),
        },
    };
    // Each carries the gesture's key number, handed back with the read.
    let for_key = |effect| match effect {
        Effect::Speak(utterance, None) => Effect::Speak(utterance, Some(key)),
        other => other,
    };
    assert_eq!(
        effects_of(&clock(0)),
        [for_key(said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Text("10:30 AM".to_owned()),
        ))]
    );
    assert_eq!(
        effects_of(&clock(1)),
        [for_key(said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Text("Saturday, October 24, 2026".to_owned()),
        ))]
    );
    // One the system could not format says nothing.
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ClockRead {
                trace_id,
                repeat: 0,
                key,
                time: None,
                date: Some("Saturday".to_owned()),
            },
        }),
        []
    );
}

#[test]
fn a_copy_is_confirmed_with_its_text_or_said_to_have_failed() {
    let trace_id = TraceId::from_raw(3);
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ClipboardCopied {
                trace_id,
                text: "Name".to_owned(),
                copied: true,
            },
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Phrase(Phrase::CopiedToClipboard("Name".to_owned())),
        )]
    );
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ClipboardCopied {
                trace_id,
                text: "Name".to_owned(),
                copied: false,
            },
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
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ShellItemGone {
                trace_id,
                name: "Clock".to_owned(),
            },
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
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::ShellItemUnclickable {
                trace_id,
                name: "Clock".to_owned(),
            },
        }),
        [said(
            trace_id,
            SpeechPriority::Queued,
            SegmentContent::Phrase(Phrase::ShellItemUnclickable("Clock".to_owned())),
        )]
    );
}

/// Source-derived (`nvda/source/gui/blockAction.py` 55-59 and 104,
/// the message, at `ui.delayedMessage`'s priority NOW, `ui.py` 183);
/// see the GUI lifecycle's test of the block itself.
#[test]
fn an_action_blocked_by_a_message_box_is_said_next() {
    let trace_id = TraceId::from_raw(7);
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::BlockedByMessageBox { trace_id },
        }),
        [said(
            trace_id,
            SpeechPriority::Next,
            SegmentContent::Message(Message::DialogRequiresResponse),
        )]
    );
}

#[test]
fn the_theme_pages_previews_are_spoken_interrupting_or_played() {
    let trace_id = TraceId::from_raw(6);
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::PreviewSpeech {
                trace_id,
                segments: vec![UtteranceSegment::new(SegmentContent::Label(
                    "Sample".to_owned()
                ))],
            },
        }),
        [said(
            trace_id,
            SpeechPriority::Interrupt,
            SegmentContent::Label("Sample".to_owned()),
        )]
    );
    assert_eq!(
        effects_of(&verbatim_model::Arrival {
            at_us: 0,
            input: Input::PreviewEarcon(Earcon::Exit),
        }),
        [Effect::PlayEarcon(Earcon::Exit)]
    );
}
/// A foreground report for window `window` of `app`, raised by `thread`,
/// of a process at `integrity`, Verbatim being at `own`.
fn foreground(
    app: App,
    window: u64,
    (console, thread): (bool, u32),
    integrity: Option<u32>,
    own: Option<u32>,
) -> ForegroundReport {
    ForegroundReport {
        window: WindowHandle(window),
        ghost: None,
        root_owner: WindowHandle(window),
        process: Some(app.pid),
        thread,
        class: "Frame".to_owned(),
        console,
        integrity,
        own_integrity: own,
    }
}

/// The hook's view of the foreground once `report`, of [`ONE`], is
/// reduced, the first report, which says nothing and asks for the focus.
fn hook_view_after(report: ForegroundReport) -> Option<HookForeground> {
    let mut desk = Desk::new();
    assert_eq!(
        desk.step(QueueInput::Foreground(report)),
        [Heard::Sent(Reader::Outpost(ONE.pid), "CurrentFocus")]
    );
    desk.state.hook_foreground()
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
        hook_view_after(foreground(ONE, 7, (false, 40), integrity, own)).map(|view| view.holds_keys)
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
        hook_view_after(foreground(ONE, 7, (true, 40), Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_text: verbatim_model::FocusTextKind::None,
            keyboard_thread: Some(40),
            desktop_switches: 0,
        })
    );
    assert_eq!(
        hook_view_after(foreground(ONE, 7, (true, 0), Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_text: verbatim_model::FocusTextKind::None,
            keyboard_thread: None,
            desktop_switches: 0,
        }),
        "a console whose thread no event has named yet"
    );
    assert_eq!(
        hook_view_after(foreground(ONE, 7, (false, 40), Some(MEDIUM), Some(MEDIUM))),
        Some(HookForeground {
            window: WindowHandle(7),
            holds_keys: true,
            focus_text: verbatim_model::FocusTextKind::None,
            keyboard_thread: None,
            desktop_switches: 0,
        })
    );
}

/// A later report replaces the earlier.
#[test]
fn the_latest_foreground_report_decides() {
    let mut desk = Desk::new();
    assert_eq!(
        desk.step(QueueInput::Foreground(foreground(
            ONE,
            7,
            (true, 40),
            Some(HIGH),
            Some(MEDIUM)
        ))),
        [Heard::Sent(Reader::Outpost(ONE.pid), "CurrentFocus")]
    );
    let next = foreground(TWO, 9, (false, 50), Some(MEDIUM), Some(MEDIUM));
    assert_eq!(desk.step(QueueInput::Foreground(next)), []);
    assert_eq!(
        desk.state.hook_foreground(),
        Some(HookForeground {
            window: WindowHandle(9),
            holds_keys: true,
            focus_text: verbatim_model::FocusTextKind::None,
            keyboard_thread: None,
            desktop_switches: 0,
        })
    );
}

// The scenarios below drive Core through its event queue on a desk of
// applications (`desk`), as the shell drives it.

/// `app`'s top-level window came to the foreground, as the focus listener
/// reports it, the first report: Core asks `app`'s outpost for its focus at
/// startup.
fn bring_forward(desk: &mut Desk, app: App) -> Vec<Heard> {
    desk.app(app);
    desk.step(QueueInput::Foreground(ForegroundReport {
        window: app.top_level(),
        root_owner: app.top_level(),
        process: Some(app.pid),
        thread: 41,
        class: "Frame".to_owned(),
        ghost: None,
        console: false,
        integrity: None,
        own_integrity: None,
    }))
}

/// A node of `app`, `number`, of `role`, named `name`.
fn node(app: App, number: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(number),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// The "OK" button, node 5 of `app`.
fn ok(app: App) -> NodeSnapshot {
    node(app, 5, Role::Button, "OK")
}

/// The "OK" button's announcement.
fn ok_button() -> Vec<UtteranceSegment> {
    vec![
        UtteranceSegment::label("OK"),
        UtteranceSegment::new(SegmentContent::Role(Role::Button)),
    ]
}

/// The ids an MSAA read of the "OK" button holds it by.
fn held_ok(app: App) -> HeldIds {
    HeldIds {
        event: MsaaIds {
            window: app.window(),
            object: -4,
            child: 0,
        },
        held_child: 0,
    }
}

/// The outpost's read of the "OK" button of `app` through MSAA.
fn msaa_ok(app: App) -> FocusRead {
    let mut read = focus_read_of(app, &ok(app), &[]);
    read.msaa = Some(held_ok(app));
    read.focus.runtime_id = None;
    read
}

/// The listener's MSAA tell of `kind` on `app`'s window, object `object`.
fn msaa_tell(app: App, kind: EventKind, object: i32) -> Event {
    Event {
        trace_id: TraceId::from_raw(12),
        kind,
        process: app.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us: 0,
        object: EventObject::Msaa {
            window: app.window(),
            object,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(app.facts())),
        checked: None,
    }
}

/// `app` in front with the "OK" button its focus at startup, read as
/// `read`: what the read's answer did, the announcement of the first
/// focus on the desk.
fn started_on(desk: &mut Desk, app: App, read: FocusRead) -> Vec<Heard> {
    assert_eq!(
        bring_forward(desk, app),
        [Heard::Sent(Reader::Outpost(app.pid), "CurrentFocus")]
    );
    desk.reply_found(&Found::Read(read))
}

/// What a UI Automation focus on the "OK" button of `app` does, as the
/// first focus on the desk.
fn uia_ok_heard(app: App) -> Vec<Heard> {
    focus_heard(app, focus_now(app, 5, &[]), vec![ok_button()])
}

/// What an MSAA focus on the "OK" button of `app` does, with
/// `foreground` the foreground object: an MSAA node is followed by no
/// registration.
fn msaa_ok_heard(app: App, foreground: Option<(App, u64)>) -> Vec<Heard> {
    vec![
        Heard::Expire(focus_now_with(app, 5, &[], foreground)),
        Heard::Say(SpeechPriority::Queued, ok_button()),
    ]
}

/// A menu's end told on `app`'s window and its check 50 ms later: what the
/// end and the check's wake did, `app`'s window the one Windows names in
/// front then, the check's read of its application's focus asked.
fn menu_ended(desk: &mut Desk, app: App) {
    assert_eq!(
        desk.tell(LISTENER, msaa_tell(app, EventKind::MenuEnd, -4)),
        []
    );
    assert_eq!(
        desk.due_over(app),
        [Heard::Sent(Reader::Outpost(app.pid), "CurrentFocus")]
    );
}

/// The listener's tell of a switch of the input desktop to one Verbatim
/// can open, the user's.
fn desktop_returned(desk: &mut Desk) -> Vec<Heard> {
    let mut tell = msaa_tell(ONE, EventKind::DesktopSwitch, 0);
    tell.payload = Payload::DesktopSwitch {
        input_desktop_open: true,
    };
    tell.window = TellWindow::NoWindow;
    desk.tell(LISTENER, tell)
}

#[test]
fn a_lock_keys_announcement_is_heard_after_a_key_pressed_within_its_30_ms() {
    let mut desk = Desk::new();
    // A focus whose typing is echoed.
    let _ = reduce(
        &mut desk.state,
        &mut desk.runtime,
        &verbatim_model::Arrival {
            at_us: 0,
            input: Input::Settings(verbatim_model::ReaderSettings {
                speak_typed_characters: verbatim_model::TypingEcho::Always,
                ..verbatim_model::ReaderSettings::default()
            }),
        },
    );
    let _ = desk.focus(ONE, &ok(ONE), &[]);
    let effects = reduce(
        &mut desk.state,
        &mut desk.runtime,
        &verbatim_model::Arrival {
            at_us: 1_000_000,
            input: Input::ToggleKeyPressed {
                trace_id: TraceId::from_raw(9),
                toggle: LockKey::ScrollLock,
            },
        },
    );
    assert_eq!(effects, []);
    desk.keys = 1;
    assert!(desk.state.announces_toggle_key());
    assert_eq!(
        desk.state.toggle_key_read_due(),
        Some((LockKey::ScrollLock, 1_030))
    );
    // "a", pressed as key 2 within the 30 ms, is spoken with its number.
    let effects = desk.feed(QueueInput::Press(Box::new(Press {
        key_seq: KeySeq::new(2),
        pressed_at_us: 1_010_000,
        action: PressAction::Typed {
            text: "a".to_owned(),
        },
    })));
    let [
        Effect::Speak(utterance, key),
        Effect::Queue(verbatim_model::request::QueueEffect::WakeAt(Some(_))),
    ] = effects.as_slice()
    else {
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
    assert!(desk.state.announces_toggle_key());
    // The announcement carries no number, so the letter's cancel does not
    // fence it off.
    let toggled = TraceId::from_raw(11);
    assert_eq!(
        reduce(
            &mut desk.state,
            &mut desk.runtime,
            &verbatim_model::Arrival {
                at_us: 1_030_000,
                input: Input::Queue {
                    trace_id: toggled,
                    input: QueueInput::LockKeyRead {
                        key: LockKey::ScrollLock,
                        on: true
                    }
                },
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
    assert!(!desk.state.announces_toggle_key());
}

/// The frame host whose top-level window, 10, holds a Settings page.
const FRAME_HOST: App = App {
    pid: Pid(70),
    generation: 7,
    top: 10,
};

/// The Settings page's process, whose controls are in the frame host's
/// top-level window.
const SETTINGS: App = App {
    pid: Pid(1),
    generation: 8,
    top: 10,
};

/// A menu's end told from a hung window holding attention: the listener
/// tells it, as NVDA tests no menu end for a hang
/// (`verbatim_listener_logic::tell::hang_tested`), and Core's check of the
/// focus 50 ms later is refused by the shell's live hang check, which
/// finds the hung window itself in front, with no ghost: the
/// not-responding sound plays (Dickson, 2026-11-19). A ghost in front is
/// spoken as NVDA's plain window instead (`focus::plain`; the
/// `hung_menu_end` tests).
#[test]
fn a_menu_end_from_a_hung_window_holding_attention_plays_the_sound() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, msaa_ok(ONE)),
        msaa_ok_heard(ONE, None)
    );
    menu_ended(&mut desk, ONE);
    let [(check, _, _)] = desk.sent[..] else {
        panic!("one check asked: {:?}", desk.sent);
    };
    assert_eq!(
        desk.step(QueueInput::InFront {
            number: RequestNumber(check),
            front: verbatim_model::request::Front::Hung,
        }),
        [Heard::Earcon(Earcon::AppNotResponding)]
    );
}

/// `app`'s window reported hung by `from`.
fn reported_hung(from: verbatim_model::request::Instance, app: App) -> QueueInput {
    let facet = app.facet(app.window(), "AppPane");
    QueueInput::WindowHung {
        from,
        hung: verbatim_model::tell::Hung {
            window: facet.clone(),
            root: app.facet(app.top_level(), "Frame"),
            root_owner: app.facet(app.top_level(), "Frame"),
            at_us: 1,
        },
    }
}

/// A reader's report of a hung window (ADR 0003, "A frozen window"): an
/// outpost's is taken as the listener's, the sound playing for the first
/// window of the application in front to join the recovery set, and a
/// report from an instance that is not its reader's current one is dropped
/// as its tells are.
#[test]
fn an_outposts_report_of_a_hung_window_is_taken_as_the_listeners() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, msaa_ok(ONE)),
        msaa_ok_heard(ONE, None)
    );
    assert_eq!(desk.step(reported_hung(ONE.replaced().instance(), ONE)), []);
    assert_eq!(
        desk.step(reported_hung(ONE.instance(), ONE)),
        [Heard::Earcon(Earcon::AppNotResponding)]
    );
    assert_eq!(desk.step(reported_hung(LISTENER, ONE)), []);
}

/// A hang met by a request for a process whose focus is in the window in
/// front plays the sound, as a Settings page (`SystemSettings`) hangs
/// inside `ApplicationFrameHost`'s frame, which is in front; a hang in a
/// process with no focus there plays nothing.
#[test]
fn a_hang_in_the_process_focused_inside_the_attention_window_plays_the_sound() {
    let mut desk = Desk::new();
    desk.app(SETTINGS);
    assert_eq!(
        bring_forward(&mut desk, FRAME_HOST),
        [Heard::Sent(Reader::Outpost(FRAME_HOST.pid), "CurrentFocus")]
    );
    assert_eq!(desk.reply_outcome(Outcome::NotThere), []);
    // The page's focus, its window in the frame's window, which the frame
    // host owns.
    let mut tell = focus_tell(SETTINGS, 5, desk.at_us + 1_000);
    let mut facts = SETTINGS.facts();
    facts.root = FRAME_HOST.facet(FRAME_HOST.top_level(), "ApplicationFrameWindow");
    facts.root_owner = facts.root.clone();
    tell.window = TellWindow::Found(Box::new(facts));
    assert_eq!(
        desk.tell(LISTENER, tell),
        [Heard::Sent(Reader::Listener, "ReadObject")]
    );
    assert_eq!(
        desk.reply_found(&listener_read(SETTINGS, 5)),
        [Heard::Sent(Reader::Outpost(SETTINGS.pid), "Completion")]
    );
    let [(completion, _, _)] = desk.sent[..] else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    assert_eq!(
        desk.step(QueueInput::RefusedHung(RequestNumber(completion))),
        [
            Heard::Earcon(Earcon::AppNotResponding),
            Heard::Sent(Reader::Outpost(SETTINGS.pid), "Follow")
        ]
    );
    // A focus in another application, not in front, meets a hang too.
    assert_eq!(
        desk.tell(LISTENER, focus_tell(TWO, 5, desk.at_us + 1_000)),
        [Heard::Sent(Reader::Listener, "ReadObject")]
    );
    assert_eq!(
        desk.reply_found(&listener_read(TWO, 5)),
        [Heard::Sent(Reader::Outpost(TWO.pid), "Completion")]
    );
    let [(completion, _, _)] = desk.sent[..] else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    assert_eq!(
        desk.step(QueueInput::RefusedHung(RequestNumber(completion))),
        [Heard::Sent(Reader::Outpost(TWO.pid), "Follow")]
    );
}

/// The check after a menu's end reads the MSAA focus Core holds: a
/// duplicate by NVDA's rule for MSAA objects, which says nothing.
#[test]
fn a_fake_focus_on_the_msaa_focus_core_holds_says_nothing() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, msaa_ok(ONE)),
        msaa_ok_heard(ONE, None)
    );
    menu_ended(&mut desk, ONE);
    let mut read = msaa_ok(ONE);
    read.focus
        .read
        .values
        .0
        .push(verbatim_model::property::PropertyAnswer {
            id: PropertyId::States,
            value: Some(PropertyValue::States(StateSet::new().with(State::Focused))),
        });
    assert_eq!(desk.reply_found(&Found::Read(read)), []);
}

/// The check after a menu's end reads the UI Automation focus Core holds:
/// judged by neither of NVDA's tests, as NVDA queues a UIA object, it is
/// spoken again.
#[test]
fn a_fake_focus_on_the_uia_focus_core_holds_is_spoken_again() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, focus_read_of(ONE, &ok(ONE), &[])),
        uia_ok_heard(ONE)
    );
    menu_ended(&mut desk, ONE);
    assert_eq!(
        desk.reply_found(&Found::Read(focus_read_of(ONE, &ok(ONE), &[]))),
        focus_heard(ONE, focus_now_in(ONE, 5, &[]), vec![ok_button()])
    );
}

/// A return to the user's desktop asks the application in front for its
/// focus again, 200 ms later, as NVDA queues it again.
#[test]
fn the_focus_is_asked_for_after_a_return_to_the_users_desktop() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, focus_read_of(ONE, &ok(ONE), &[])),
        uia_ok_heard(ONE)
    );
    assert_eq!(desktop_returned(&mut desk), []);
    assert_eq!(
        desk.due(),
        [Heard::Sent(Reader::Outpost(ONE.pid), "CurrentFocus")]
    );
}

/// The read after the desktop's return has no duplicate test, so the MSAA
/// focus Core holds is spoken again.
#[test]
fn a_desktop_return_s_focus_is_spoken_again_on_the_focus_core_holds() {
    let mut desk = Desk::new();
    assert_eq!(
        started_on(&mut desk, ONE, msaa_ok(ONE)),
        msaa_ok_heard(ONE, None)
    );
    assert_eq!(desktop_returned(&mut desk), []);
    assert_eq!(
        desk.due(),
        [Heard::Sent(Reader::Outpost(ONE.pid), "CurrentFocus")]
    );
    assert_eq!(
        desk.reply_found(&Found::Read(msaa_ok(ONE))),
        msaa_ok_heard(ONE, Some((ONE, FRAME)))
    );
}

/// The listener's tell of the virtual desktop's name change, on the
/// desktop window, and the listener's read of the name it asks: what each
/// did.
fn desktop_named(desk: &mut Desk, name: &str) {
    let mut tell = msaa_tell(ONE, EventKind::NameChange, 0);
    let mut facts = ONE.facts();
    facts.desktop = true;
    tell.window = TellWindow::Found(Box::new(facts));
    assert_eq!(
        desk.tell(LISTENER, tell),
        [Heard::Sent(Reader::Listener, "ReadObject")]
    );
    let read = NodeRead {
        node: None,
        window: ONE.node_window(),
        values: PropertyValues(vec![verbatim_model::property::PropertyAnswer {
            id: PropertyId::Name,
            value: Some(PropertyValue::Text(name.to_owned())),
        }]),
    };
    assert_eq!(desk.reply_object(read), []);
}

/// The virtual desktop's name is kept until the next focus, which says it
/// before the containers it enters, as NVDA says it; its due time then
/// says nothing.
#[test]
fn the_virtual_desktops_name_is_spoken_before_the_next_focuss_containers() {
    let mut desk = Desk::new();
    desktop_named(&mut desk, "Desktop 2");
    let list = node(ONE, 3, Role::List, "Desktop");
    assert_eq!(
        desk.focus(ONE, &ok(ONE), &[list]),
        focus_heard(
            ONE,
            focus_now(ONE, 5, &[3]),
            vec![
                vec![UtteranceSegment::text("Desktop 2")],
                vec![
                    UtteranceSegment::label("Desktop"),
                    UtteranceSegment::new(SegmentContent::Role(Role::List)),
                ],
                ok_button(),
            ]
        )
    );
    assert_eq!(desk.due(), []);
}

/// A name no focus follows is spoken as a message at its due time, 250 ms
/// after its change, queued, and only once.
#[test]
fn the_virtual_desktops_name_is_spoken_when_no_focus_follows() {
    let mut desk = Desk::new();
    desktop_named(&mut desk, "Desktop 2");
    assert_eq!(
        desk.due(),
        [Heard::Say(
            SpeechPriority::Queued,
            vec![UtteranceSegment::text("Desktop 2")]
        )]
    );
    assert_eq!(desk.wake, None);
}
