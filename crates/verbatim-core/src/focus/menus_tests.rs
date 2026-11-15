//! The check after a menu or switcher ends, and the UI Automation menu
//! opening.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{Reply, RunRead, Tried};
use verbatim_model::tell::{ElementToken, Read};
use verbatim_model::{State, StateSet, WindowHandle};

use crate::entries::tests::{
    APP, Held, NOTHING, OUTPOST, SPOKEN, answer, asked_completion, entry, feed_at, focus_read,
    held, msaa_tell, ready, uia_tell,
};
use crate::event_queue::{Foreground, Input};

const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
};

/// A state with the foreground record set, its startup read answered
/// with nothing there.
fn in_front() -> Held {
    let mut state = ready();
    feed_at(&mut state, 100, Input::Foreground(IN_FRONT));
    feed_at(
        &mut state,
        200,
        crate::entries::tests::answer_outcome(
            OUTPOST,
            1,
            verbatim_model::request::Outcome::NotThere,
        ),
    );
    state
}

/// An MSAA menu end tell on window 5.
fn menu_end() -> Input<crate::entries::Entry> {
    let mut tell = msaa_tell(-4);
    tell.kind = EventKind::MenuEnd;
    entry(tell)
}

/// The focus answer of the check, an MSAA focus whose object has the
/// focused state, or not.
fn focused(focused: bool) -> Reply {
    let mut read = focus_read(Some(held(-4)));
    let mut states = StateSet::default();
    if focused {
        states.insert(State::Focused);
    }
    read.focus.read.values.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(states)),
    });
    Reply::Focus(Box::new(read))
}

#[test]
fn a_menu_end_checks_the_focus_50_ms_later_and_speaks_a_focused_answer() {
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(feed_at(&mut state, 1_049, Input::Tick), NOTHING);
    assert_eq!(
        feed_at(&mut state, 51_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(true))),
        SPOKEN
    );
}

#[test]
fn a_checked_msaa_focus_without_the_focused_state_is_not_spoken() {
    let mut state = in_front();
    feed_at(&mut state, 1_000, menu_end());
    feed_at(&mut state, 51_000, Input::Tick);
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(false))),
        NOTHING
    );
    assert!(state.versions.last.is_none());
}

#[test]
fn a_focus_committed_before_the_check_ends_it_with_no_read() {
    let mut state = in_front();
    feed_at(&mut state, 1_000, menu_end());
    // A focus is executed within the 50 ms.
    feed_at(&mut state, 2_000, entry(msaa_tell(-5)));
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(focus_read(Some(held(-5)))))],
    };
    assert_eq!(
        feed_at(&mut state, 3_000, answer(OUTPOST, 2, Reply::Run(tried))),
        SPOKEN
    );
    assert_eq!(feed_at(&mut state, 51_000, Input::Tick), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_menu_end_after_a_run_that_executed_a_focus_makes_no_check() {
    let mut state = in_front();
    // The run asks; the menu end waits behind it.
    assert_eq!(
        feed_at(&mut state, 1_000, entry(msaa_tell(-5))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    feed_at(&mut state, 1_100, menu_end());
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(focus_read(Some(held(-5)))))],
    };
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 2, Reply::Run(tried))),
        SPOKEN
    );
    assert_eq!(feed_at(&mut state, 60_000, Input::Tick), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// A UI Automation menu opening on runtime id `[42, runtime]`, raised at
/// `at_us`.
fn opening(runtime: i32, at_us: u64) -> Input<crate::entries::Entry> {
    let mut tell = uia_tell(runtime, vec![(ElementToken(3), Read::Answered(true))]);
    tell.kind = EventKind::UiaMenuOpened;
    tell.at_us = at_us;
    entry(tell)
}

#[test]
fn a_menu_opening_behind_a_pending_focus_raised_before_it_is_dropped() {
    let mut state = ready();
    // The focus at 1 000 is asked; the opening raised at 1 500 is dropped.
    assert_eq!(
        feed_at(&mut state, 1_000, entry(uia_tell(1, Vec::new()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(feed_at(&mut state, 1_600, opening(2, 1_500)), NOTHING);
    // An opening raised before the focus is kept.
    assert_eq!(feed_at(&mut state, 1_700, opening(2, 900)), NOTHING);
    let waiting = format!("{:?}", state.queue.front_for_tests());
    assert!(waiting.contains("UiaFocus"), "{waiting}");
    assert_eq!(state.queue.waiting_for_tests(), 1);
}

#[test]
fn a_menu_opening_on_the_committed_focus_executes_it_again() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    let again = feed_at(&mut state, 4_000, opening(1, 4_000));
    assert!(state.queue.front_for_tests().is_none());
    assert_eq!(again, ["Label(\"OK\") Role(Button)"]);
}
