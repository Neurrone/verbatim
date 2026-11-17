//! The check after a menu or switcher ends, and the UI Automation menu
//! opening.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{Outcome, Refusal, Reply, RunRead, Tried};
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
    shell: false,
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
        ["send Outpost(Pid(40)) CurrentFocus with the focused-state test"]
    );
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(true))),
        SPOKEN
    );
}

/// The outpost refuses the check's MSAA focus "not focused" when neither
/// it nor any ancestor up to the desktop has the focused state, as NVDA's
/// fake focus is tested before it is queued: nothing is said, and the
/// readers' last focus and the caret baseline stay as they were, since
/// nothing was queued.
#[test]
fn a_checked_msaa_focus_refused_as_unfocused_is_not_spoken() {
    let mut state = in_front();
    feed_at(&mut state, 1_000, menu_end());
    feed_at(&mut state, 51_000, Input::Tick);
    assert_eq!(
        feed_at(
            &mut state,
            52_000,
            crate::entries::tests::answer_outcome(
                OUTPOST,
                2,
                Outcome::Refused(Refusal::NotFocused)
            )
        ),
        NOTHING
    );
    assert!(state.versions.last.is_none());
    assert!(state.queue.front_for_tests().is_none());
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

#[test]
fn a_menu_opening_on_the_committed_focus_that_lost_the_keyboard_focus_is_dropped() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    assert_eq!(feed_at(&mut state, 4_000, lost_opening(1, 4_000)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_menu_opening_on_another_element_that_lost_the_keyboard_focus_asks_nothing() {
    let mut state = ready();
    assert_eq!(feed_at(&mut state, 1_000, lost_opening(2, 1_000)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// [`opening`], whose element the check thread found without the keyboard
/// focus.
fn lost_opening(runtime: i32, at_us: u64) -> Input<crate::entries::Entry> {
    let mut tell = uia_tell(runtime, vec![(ElementToken(3), Read::Answered(true))]);
    tell.kind = EventKind::UiaMenuOpened;
    tell.at_us = at_us;
    if let Some(checked) = tell.checked.as_mut() {
        checked.keyboard_focus = Some(Read::Answered(false));
    }
    entry(tell)
}

/// An MSAA menu start tell on window 5.
fn menu_start() -> Input<crate::entries::Entry> {
    let mut tell = msaa_tell(-4);
    tell.kind = EventKind::MenuStart;
    entry(tell)
}

/// A focus answer of `role`, named "Context".
fn read_as(role: verbatim_model::Role) -> Reply {
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = crate::entries::tests::values("Context", role);
    Reply::Run(RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    })
}

/// The run candidates' kinds of the request out.
fn asked_kinds(state: &Held) -> Vec<verbatim_model::request::CandidateKind> {
    state
        .queue
        .table_for_tests()
        .into_iter()
        .flat_map(|request| match request {
            verbatim_model::request::Request::MsaaRun(run) => run
                .candidates
                .into_iter()
                .map(|candidate| candidate.kind)
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn a_menu_start_reads_its_object_and_a_menu_popup_executes_as_a_focus() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, menu_start()),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        asked_kinds(&state),
        [verbatim_model::request::CandidateKind::MenuStart]
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(OUTPOST, 1, read_as(verbatim_model::Role::Menu))
        ),
        [
            "Label(\"Editor\") Role(Window)",
            "Label(\"Context\") Role(Menu)"
        ]
    );
}

#[test]
fn a_menu_start_on_anything_but_a_menu_popup_says_nothing() {
    let mut state = ready();
    feed_at(&mut state, 1_000, menu_start());
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(OUTPOST, 1, read_as(verbatim_model::Role::Button))
        ),
        NOTHING
    );
    assert!(state.focus.object().is_none());
}

/// A focus run asked, a menu start waiting behind it, and the run's
/// answer executing a focus of `role`: what was sent and said.
fn start_after_a_run(role: verbatim_model::Role) -> Vec<String> {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, entry(msaa_tell(-4))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(feed_at(&mut state, 1_100, menu_start()), NOTHING);
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = crate::entries::tests::values("File", role);
    feed_at(
        &mut state,
        2_000,
        answer(
            OUTPOST,
            1,
            Reply::Run(RunRead {
                tried: vec![Tried::Focus(Box::new(read))],
            }),
        ),
    )
}

#[test]
fn a_menu_start_after_a_run_that_executed_an_msaa_menu_item_is_dropped() {
    assert_eq!(
        start_after_a_run(verbatim_model::Role::MenuItem),
        ["Label(\"Editor\") Role(Window)", "Label(\"File\")"]
    );
}

#[test]
fn a_menu_start_after_a_run_that_executed_another_focus_reads_its_object() {
    assert_eq!(
        start_after_a_run(verbatim_model::Role::Button),
        [
            "Label(\"Editor\") Role(Window)",
            "Label(\"File\") Role(Button)",
            "send Outpost(Pid(40)) MsaaRun"
        ]
    );
}
