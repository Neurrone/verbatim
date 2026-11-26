//! Verbatim's menu closed over a hung window, in the order the shell
//! feeds Core across the process boundary, from Verbatim's own log of the
//! capture series (`cap-keys/v3/verbatim.out`) and its recordings
//! (`c3-g6-escape-*`): the menu's popup end and menu end, then the
//! listener's report of the ghost window now in front, which reached Core
//! 3.7 to 13.5 ms after the menu closed, the shell giving the reducer the
//! ghost and the event queue the hung window it stands for; the check 50
//! ms after the end, NVDA's fake focus, its wake carrying the window
//! Windows names in front then, read live by the shell, asked of hungwin's
//! outpost; and the
//! shell's refusal of it as hung with the ghost read in front. NVDA says
//! "hungwin (Not Responding)  window" there in 20 of 20 runs, by its plain
//! window object of the ghost (`IAccessibleHandler/__init__.py` 976 to
//! 1002; `NVDAObjects/window/__init__.py` 83 to 98), and Verbatim says the
//! same, with no sound (Dickson, 2026-11-19).

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{App, Desk, Heard, focus_read_of, follow, queued};
use readers::Found;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{
    ForegroundReport, Front, FrontWindow, GhostFront, LiveFront, QueueInput, Reader, RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    NodeDetails, NodeSnapshot, Pid, Role, SegmentContent, StateSet, TraceId, UtteranceSegment,
    WindowHandle,
};

/// Verbatim, whose menu is open.
const VERBATIM: App = App {
    pid: Pid(3368),
    generation: 5,
    top: 5_000,
};

/// hungwin, the application that stops responding behind the menu.
const HUNGWIN: App = App {
    pid: Pid(2892),
    generation: 6,
    top: 2_000,
};

/// The ghost window the desktop window manager shows for hungwin.
const GHOST: WindowHandle = WindowHandle(7_000);

/// `app`'s top-level window as the listener reports it in front.
fn report(app: App) -> QueueInput {
    QueueInput::Foreground(ForegroundReport {
        window: app.top_level(),
        root_owner: app.top_level(),
        process: app.pid,
        thread: 41,
        class: "Frame".to_owned(),
        ghost: None,
        console: false,
        integrity: None,
        own_integrity: None,
    })
}

/// Core's wake as the shell makes it with a check waiting, Windows naming
/// `app`'s top-level window in front: for hungwin, the hung window its
/// ghost stands for, as the shell maps it.
fn due_over(app: App) -> QueueInput {
    QueueInput::Due {
        in_front: Some(LiveFront::Window(FrontWindow {
            window: app.top_level(),
            root_owner: app.top_level(),
            process: app.pid,
            thread: 41,
            class: "Frame".to_owned(),
        })),
    }
}

/// The menu's item "Settings...", focused.
fn item() -> NodeSnapshot {
    NodeSnapshot {
        id: VERBATIM.node(2),
        backend: verbatim_model::Backend::Uia,
        role: Role::MenuItem,
        name: Some("Settings...".to_owned()),
        value: None,
        states: StateSet::default(),
        details: NodeDetails::default(),
    }
}

/// Verbatim's menu in front, its item focused: Core's startup read of
/// it, answered.
fn menu_open() -> Desk {
    let mut desk = Desk::new();
    desk.app(VERBATIM);
    desk.app(HUNGWIN);
    let asked = desk.step(report(VERBATIM));
    assert_eq!(
        asked,
        [Heard::Sent(Reader::Outpost(VERBATIM.pid), "CurrentFocus")]
    );
    let item = item();
    let spoken = desk.reply_found(&Found::Read(focus_read_of(VERBATIM, &item, &[])));
    assert_eq!(spoken.last(), Some(&follow(VERBATIM)));
    desk
}

/// The menu's end of `kind`, told by the listener at `at_us`.
fn end(kind: EventKind, at_us: u64) -> QueueInput {
    QueueInput::Tell {
        from: desk::LISTENER,
        tell: Box::new(Event {
            trace_id: TraceId::from_raw(21),
            kind,
            process: VERBATIM.pid,
            application: "verbatim".to_owned(),
            thread: 41,
            at_us,
            object: EventObject::Msaa {
                window: VERBATIM.window(),
                object: -4,
                child: 0,
            },
            cached: PropertyValues::default(),
            payload: Payload::None,
            window: TellWindow::Found(Box::new(VERBATIM.facts())),
            checked: None,
        }),
    }
}

/// The listener's report of the ghost in front at `at_us`, as the shell
/// feeds it: the hung window it stands for, with the ghost as the
/// keyboard hook's window.
fn ghost_in_front(desk: &mut Desk, at_us: u64) -> Vec<Heard> {
    let QueueInput::Foreground(mut hung) = report(HUNGWIN) else {
        unreachable!("a foreground report")
    };
    hung.ghost = Some(GHOST);
    desk.step_at(at_us, QueueInput::Foreground(hung))
}

/// The ghost as the shell reads it in front.
fn ghost() -> Front {
    Front::Ghost(GhostFront {
        ghost: GHOST,
        stands_for: HUNGWIN.top_level(),
        process: HUNGWIN.pid,
        thread: 41,
        name: "hungwin (Not Responding)".to_owned(),
        unavailable: false,
    })
}

/// The menu closed at 100 ms, the ghost's report reaching Core
/// `report_after_us` later, then the check 50 ms after the end: what the
/// shell's refusal of the check as hung, with the ghost in front, said.
fn closed_over_the_ghost(report_after_us: u64) -> Vec<Heard> {
    let mut desk = menu_open();
    assert_eq!(desk.step_at(100_000, end(EventKind::PopupEnd, 100_000)), []);
    assert_eq!(desk.step_at(100_400, end(EventKind::MenuEnd, 100_400)), []);
    assert_eq!(ghost_in_front(&mut desk, 100_400 + report_after_us), []);
    assert_eq!(
        desk.step_at(150_000, due_over(HUNGWIN)),
        [Heard::Sent(Reader::Outpost(HUNGWIN.pid), "CurrentFocus")]
    );
    let [(number, _, _)] = desk.sent[..] else {
        panic!("one check asked: {:?}", desk.sent);
    };
    desk.step_at(
        151_000,
        QueueInput::InFront {
            number: RequestNumber(number),
            front: ghost(),
        },
    )
}

/// What the plain window of the ghost says, after speech is stopped, the
/// whole line having changed below the desktop; the speech of any focus
/// left behind is expired, and the menu's `Follow` stops.
fn plain_spoken() -> Vec<Heard> {
    vec![
        Heard::Expire(verbatim_model::FocusNow {
            focus: verbatim_model::ObjectKey::of_id(verbatim_model::NodeId::plain(1, 1)),
            ancestors: Vec::new(),
            foreground: None,
            menu_item_of_focus: None,
        }),
        Heard::Stop,
        queued(vec![
            UtteranceSegment::label("hungwin (Not Responding)".to_owned()),
            UtteranceSegment::new(SegmentContent::Role(Role::Window)),
        ]),
        Heard::Sent(Reader::Outpost(VERBATIM.pid), "Follow"),
    ]
}

/// The ghost's report 3.7 ms after the menu closed, the fastest of the
/// log's six runs: the plain window is spoken, with no sound.
#[test]
fn the_ghosts_report_3_7_ms_after_the_menu_closed_speaks_the_plain_window() {
    assert_eq!(closed_over_the_ghost(3_700), plain_spoken());
}

/// The ghost's report 13.5 ms after the menu closed, the slowest of the
/// log's six runs: the same.
#[test]
fn the_ghosts_report_13_5_ms_after_the_menu_closed_speaks_the_plain_window() {
    assert_eq!(closed_over_the_ghost(13_500), plain_spoken());
}

/// The ghost's report delayed past the check, which the recordings never
/// show: the foreground record still names Verbatim's window, and the
/// check asks hungwin all the same, by the window Windows names in front
/// when the check comes due, as NVDA's fake focus asks Windows
/// (`IAccessibleHandler/__init__.py` 991 to 995;
/// `NVDAObjects/window/__init__.py` 162 to 181; Dickson, 2026-11-26). The
/// plain window is spoken as in the recorded order, and the report that
/// follows says nothing more.
#[test]
fn the_ghosts_report_after_the_check_still_asks_hungwin() {
    let mut desk = menu_open();
    assert_eq!(desk.step_at(100_000, end(EventKind::PopupEnd, 100_000)), []);
    assert_eq!(desk.step_at(100_400, end(EventKind::MenuEnd, 100_400)), []);
    assert_eq!(
        desk.step_at(150_000, due_over(HUNGWIN)),
        [Heard::Sent(Reader::Outpost(HUNGWIN.pid), "CurrentFocus")]
    );
    let [(number, _, _)] = desk.sent[..] else {
        panic!("one check asked: {:?}", desk.sent);
    };
    assert_eq!(
        desk.step_at(
            151_000,
            QueueInput::InFront {
                number: RequestNumber(number),
                front: ghost(),
            },
        ),
        plain_spoken()
    );
    assert_eq!(ghost_in_front(&mut desk, 160_000), []);
}

/// The menu's window `kind` event on its own window object, as the
/// listener tells a destroy or a hide.
fn window_event(kind: EventKind, at_us: u64) -> QueueInput {
    let QueueInput::Tell { from, mut tell } = end(kind, at_us) else {
        unreachable!("end makes a tell");
    };
    tell.object = EventObject::Msaa {
        window: VERBATIM.window(),
        object: 0,
        child: 0,
    };
    QueueInput::Tell { from, tell }
}

/// Escape closes the menu over the hung window with no ghost, in the order
/// of `menu_end_over_a_hung_window`'s first run (its flight recorder's
/// lines 60 to 68): the popup end, the menu window's destroy, the menu end
/// and its hide, then the check 60 ms later, with no foreground report,
/// since Windows activated nothing. The check's wake carries that no
/// window is in front, so it names Verbatim's window, the
/// one still in the record, and the shell, finding no window in front,
/// answers it with the desktop: NVDA's fake focus then focuses its
/// `Desktop`, "Desktop  window" (1 of 18 Escapes in the captures; Dickson,
/// 2026-11-19).
#[test]
fn the_menu_closed_with_no_window_in_front_speaks_the_desktop() {
    let mut desk = menu_open();
    assert_eq!(desk.step_at(100_000, end(EventKind::PopupEnd, 100_000)), []);
    assert_eq!(
        desk.step_at(100_400, window_event(EventKind::Destroy, 100_400)),
        []
    );
    assert_eq!(desk.step_at(100_900, end(EventKind::MenuEnd, 100_900)), []);
    assert_eq!(
        desk.step_at(101_100, window_event(EventKind::Hide, 101_100)),
        []
    );
    assert_eq!(
        desk.step_at(
            161_000,
            QueueInput::Due {
                in_front: Some(LiveFront::Nothing)
            }
        ),
        [Heard::Sent(Reader::Outpost(VERBATIM.pid), "CurrentFocus")]
    );
    let [(number, _, _)] = desk.sent[..] else {
        panic!("one check asked: {:?}", desk.sent);
    };
    assert_eq!(
        desk.step_at(
            161_100,
            QueueInput::InFront {
                number: RequestNumber(number),
                front: Front::Desktop(verbatim_model::request::DesktopFront {
                    desktop: WindowHandle(65_552),
                    process: Pid(600),
                    thread: 601,
                }),
            },
        ),
        [
            Heard::Expire(verbatim_model::FocusNow {
                focus: verbatim_model::ObjectKey::of_id(verbatim_model::NodeId::plain(1, 1)),
                ancestors: Vec::new(),
                foreground: None,
                menu_item_of_focus: None,
            }),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::new(SegmentContent::Message(verbatim_model::Message::Desktop)),
                UtteranceSegment::new(SegmentContent::Role(Role::Window)),
            ]),
            Heard::Sent(Reader::Outpost(VERBATIM.pid), "Follow"),
        ]
    );
}
