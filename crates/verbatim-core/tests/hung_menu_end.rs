//! Verbatim's menu closed over a hung window, in the order the shell
//! feeds Core across the process boundary, from Verbatim's own log of the
//! capture series (`cap-keys/v3/verbatim.out`) and its recordings
//! (`c3-g6-escape-*`): the menu's popup end and menu end, then the
//! listener's report of the ghost window now in front, which reached Core
//! 3.7 to 13.5 ms after the menu closed, the shell giving the reducer the
//! ghost and the event queue the hung window it stands for; the check 50
//! ms after the end, NVDA's fake focus, asked of hungwin's outpost; and the
//! shell's refusal of it as hung with the ghost read in front. NVDA says
//! "hungwin (Not Responding)  window" there in 20 of 20 runs, by its plain
//! window object of the ghost (`IAccessibleHandler/__init__.py` 976 to
//! 1002; `NVDAObjects/window/__init__.py` 83 to 98), and Verbatim says the
//! same, with no sound (Dickson, 2026-11-19).

mod desk;

use desk::{App, Desk, Heard, focus_read_of, follow, queued};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, ForegroundReport, Front, GhostFront, QueueInput, Reader,
    Reply, RequestNumber,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    ForegroundWindow, Input, NodeDetails, NodeSnapshot, Pid, Role, SegmentContent, StateSet,
    TraceId, UtteranceSegment, WindowHandle,
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

/// The desktop window manager, the ghost's own process.
const DWM: Pid = Pid(6732);

/// `app`'s top-level window as the listener reports it in front.
fn report(app: App) -> QueueInput {
    QueueInput::Foreground(ForegroundReport {
        window: app.top_level(),
        root_owner: app.top_level(),
        process: app.pid,
        thread: 41,
        class: "Frame".to_owned(),
    })
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
    let spoken = desk.reply(Reply::Candidate(CandidateAnswer::focused(focus_read_of(
        VERBATIM,
        &item,
        &[],
    ))));
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
/// feeds it: the reducer takes the ghost itself, then the event queue the
/// hung window it stands for.
fn ghost_in_front(desk: &mut Desk, at_us: u64) -> Vec<Heard> {
    let raw = verbatim_core::reduce(
        &mut desk.state,
        &mut desk.runtime,
        &Input::Foreground(ForegroundWindow {
            window: GHOST,
            root_owner: GHOST,
            process: DWM,
            observed_at_us: at_us,
            own: false,
            thread: 77,
            console: false,
            integrity: None,
            own_integrity: None,
        }),
    );
    assert!(
        raw.iter().all(|effect| !matches!(
            effect,
            verbatim_model::Effect::Speak(..) | verbatim_model::Effect::PlayEarcon(_)
        )),
        "{raw:?}"
    );
    desk.step_at(at_us, report(HUNGWIN))
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
        desk.step_at(150_000, QueueInput::Due),
        [Heard::Sent(Reader::Outpost(HUNGWIN.pid), "CurrentFocus")]
    );
    let [(number, _, _)] = desk.sent[..] else {
        panic!("one check asked: {:?}", desk.sent);
    };
    desk.step_at(
        151_000,
        QueueInput::HungFocus {
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
/// show (a recorded limit): the check asks the window still in the
/// record, Verbatim's own, whose focus is not focused once the menu has
/// gone, so nothing is said, and the report that follows says nothing
/// either.
#[test]
fn the_ghosts_report_after_the_check_says_nothing() {
    let mut desk = menu_open();
    assert_eq!(desk.step_at(100_000, end(EventKind::PopupEnd, 100_000)), []);
    assert_eq!(desk.step_at(100_400, end(EventKind::MenuEnd, 100_400)), []);
    assert_eq!(
        desk.step_at(150_000, QueueInput::Due),
        [Heard::Sent(Reader::Outpost(VERBATIM.pid), "CurrentFocus")]
    );
    assert_eq!(
        desk.reply(Reply::Candidate(CandidateAnswer {
            facts: CandidateFacts::Found {
                focused: Some(false),
                role: None,
            },
            read: None,
        })),
        []
    );
    assert_eq!(ghost_in_front(&mut desk, 160_000), []);
}
