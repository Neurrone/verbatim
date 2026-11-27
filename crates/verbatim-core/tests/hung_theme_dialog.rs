//! The incident of `theme_panel` on GitHub (run 44883440325), where
//! Verbatim's GUI thread (thread 672, process 9052) stalled from 08:20:25.507
//! to 30.675 behind the Remove Theme confirmation, a ghost (262832) stood
//! for the Theme dialog (2359904) from 30.689 to 31.507, and the task
//! dialog (131776) raised its foreground and focus events while still
//! flagged hung: the listener dropped them all, as NVDA drops them
//! (`IAccessibleHandler/__init__.py` 556 to 575), and nothing was said for
//! 75 s.
//!
//! Seeded from Core's recorded state before input 464 of the run's flight
//! recorder, and fed its recorded inputs 464 to 476, with the listener's
//! three reports of a hung window placed where the listener places them
//! (`listener.log` 823 to 861): the Theme dialog's state change at 30.675;
//! the ghost's own drop, mapped to the Theme dialog, behind the ghost's
//! foreground report (input 464); and the task dialog's foreground drop
//! behind its foreground report (input 467). The hang round's checks are
//! answered hung before the ghost ended at 31.507 and answering after, and
//! the recovery reads with the confirmation and its No button as a passing
//! run read them. Verbatim plays "not responding" once and speaks the
//! confirmation once the window answers; NVDA says nothing, which the
//! parity ledger records as a difference (Dickson, 2026-11-21).

mod desk;
#[path = "support/readers.rs"]
mod readers;

use desk::{App, Desk, Heard, focus_read_of, queued};
use readers::Found;
use verbatim_core::read_dump;
use verbatim_model::request::{Instance, QueueEffect, QueueInput};
use verbatim_model::tell::{Hung, WindowFacet};
use verbatim_model::{
    Backend, Earcon, Effect, Input, NodeDetails, NodeSnapshot, Pid, Role, SegmentContent, StateSet,
    UtteranceSegment, WindowHandle,
};

/// The incident's recorded state before input 464, and inputs 464 to 476.
const FIXTURE: &str = include_str!("fixtures/hung_theme_dialog/inputs.jsonl");

/// Verbatim's application: its process, its outpost of generation 2, and
/// the task dialog's window.
const VERBATIM: App = App {
    pid: Pid(9052),
    generation: 2,
    top: 131_776,
};

/// The Theme dialog.
const THEME_DIALOG: u64 = 2_359_904;

/// The task dialog, the Remove Theme confirmation.
const TASK_DIALOG: u64 = 131_776;

/// When the ghost ended and both dialogs answered again.
const ANSWERING_US: u64 = 1_795_249_231_507_067;

/// A window of Verbatim's GUI thread.
fn facet(handle: u64, class: &str) -> WindowFacet {
    WindowFacet {
        handle: WindowHandle(handle),
        class: class.to_owned(),
        process: Some(VERBATIM.pid),
        thread: 672,
    }
}

/// The listener's report of top-level dialog `dialog` hung at `at_us`,
/// its facets as the recorded tells of the same windows carry them
/// (input 472 for the task dialog).
fn reported(dialog: u64, at_us: u64) -> QueueInput {
    QueueInput::WindowHung {
        from: desk::LISTENER,
        hung: Hung {
            window: facet(dialog, "#32770"),
            root: facet(dialog, "#32770"),
            root_owner: facet(524_872, "wxWindowNR"),
            at_us,
        },
    }
}

/// The Remove Theme confirmation, as a passing run of `theme_panel` read
/// it.
fn confirmation() -> NodeSnapshot {
    NodeSnapshot {
        id: VERBATIM.node(200),
        backend: Backend::Uia,
        role: Role::Dialog,
        name: Some("Remove Theme".to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails {
            description: Some("Remove the theme Default copy? This cannot be undone.".to_owned()),
            ..NodeDetails::default()
        },
    }
}

/// Its No button, focused.
fn no_button() -> NodeSnapshot {
    NodeSnapshot {
        id: VERBATIM.node(201),
        backend: Backend::Uia,
        role: Role::Button,
        name: Some("No".to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails {
            keyboard_shortcut: Some("Alt+N".to_owned()),
            ..NodeDetails::default()
        },
    }
}

/// The outpost's answer to a recovery read: the No button focused in the
/// confirmation.
fn the_confirmation_read() -> Found {
    Found::Read(focus_read_of(VERBATIM, &no_button(), &[confirmation()]))
}

/// The hang rounds due by `until`, each check answered as Windows
/// answered it: hung before the ghost ended, answering after. What they
/// made heard.
fn rounds_until(desk: &mut Desk, until: u64) -> Vec<Heard> {
    let mut heard = Vec::new();
    while let Some(wake) = desk.wake.filter(|wake| *wake <= until) {
        desk.at_us = wake;
        desk.wake = None;
        let mut checks = Vec::new();
        for effect in desk.feed(QueueInput::Due { in_front: None }) {
            match effect {
                Effect::Queue(QueueEffect::WakeAt(next)) => desk.wake = next,
                Effect::Queue(QueueEffect::CheckHung(windows)) => checks.extend(windows),
                other => panic!("a round's effect: {other:?}"),
            }
        }
        for window in checks {
            heard.extend(desk.step_at(
                wake + 100,
                QueueInput::HangCheck {
                    window,
                    hung: wake < ANSWERING_US,
                },
            ));
        }
    }
    heard
}

/// The current-focus read last asked, by number and the instance it went
/// to.
fn last_current_focus(desk: &Desk) -> (u64, Instance) {
    desk.asked
        .iter()
        .rev()
        .find_map(|(number, to, request)| {
            (desk::kind(request) == "CurrentFocus").then_some((*number, *to))
        })
        .expect("a current-focus read")
}

/// The utterances and sounds in `heard`, in order.
fn spoken(heard: Vec<Heard>) -> Vec<Heard> {
    heard
        .into_iter()
        .filter(|heard| matches!(heard, Heard::Say(..) | Heard::Earcon(_)))
        .collect()
}

#[test]
fn a_dialog_whose_events_arrive_while_its_window_is_hung_is_spoken_once_it_answers() {
    let dump = read_dump(&mut FIXTURE.as_bytes()).expect("the fixture reads");
    let mut desk = Desk::new();
    desk.state = dump.base;
    let mut inputs = vec![(
        1_795_249_230_675_200,
        reported(THEME_DIALOG, 1_795_249_230_675_135),
    )];
    for (index, recorded) in dump.inputs.into_iter().enumerate() {
        let Input::Queue { at_us, input, .. } = recorded.input else {
            panic!("a queue input: {:?}", recorded.input);
        };
        inputs.push((at_us, input));
        match index {
            // Behind the ghost's foreground report, input 464.
            0 => inputs.push((
                1_795_249_230_690_200,
                reported(THEME_DIALOG, 1_795_249_230_690_065),
            )),
            // Behind the task dialog's foreground report, input 467.
            3 => inputs.push((
                1_795_249_231_501_700,
                reported(TASK_DIALOG, 1_795_249_231_486_101),
            )),
            _ => {}
        }
    }
    let mut heard = Vec::new();
    for (at_us, input) in inputs {
        heard.extend(rounds_until(&mut desk, at_us));
        heard.extend(desk.step_at(at_us, input));
    }
    // The first round after the ghost's end finds both dialogs answering:
    // the task dialog, first by handle, is read again, then the Theme
    // dialog, whose read finds the same focus.
    heard.extend(rounds_until(&mut desk, 1_795_249_231_700_000));
    let mut answered = Vec::new();
    for _ in [TASK_DIALOG, THEME_DIALOG] {
        let (number, to) = last_current_focus(&desk);
        answered.push(number);
        heard.extend(desk.found(to, number, &the_confirmation_read()));
    }
    assert_ne!(
        answered[0], answered[1],
        "each dialog's recovery read is its own"
    );
    heard.extend(rounds_until(&mut desk, 1_795_249_232_700_000));
    let segment = |content| UtteranceSegment {
        content,
        language: None,
    };
    assert_eq!(
        spoken(heard),
        [
            Heard::Earcon(Earcon::AppNotResponding),
            queued(vec![
                segment(SegmentContent::Label("Remove Theme".to_owned())),
                segment(SegmentContent::Role(Role::Dialog)),
                segment(SegmentContent::Description(
                    "Remove the theme Default copy? This cannot be undone.".to_owned()
                )),
            ]),
            queued(vec![
                segment(SegmentContent::Label("No".to_owned())),
                segment(SegmentContent::Role(Role::Button)),
                segment(SegmentContent::Shortcut("Alt+N".to_owned())),
            ]),
        ]
    );
}
