//! The check after a menu or switcher ends, and the UI Automation menu
//! opening.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{CandidateAnswer, Outcome, Reply};
use verbatim_model::tell::{ElementToken, Read};
use verbatim_model::{Role, State, StateSet, WindowHandle};

use crate::entries::tests::{
    APP, Held, LISTENER, NOTHING, OUTPOST, SPOKEN, SPOKEN_FOLLOWED, answer, asked_completion,
    entry, feed_at, focus_read, held, listener_answer, msaa_tell, node, ready, uia_tell, values,
};
use crate::event_queue::{Foreground, Input};

const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
    shell: false,
    ..Foreground::BLANK
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

/// The focus answer of the check, an MSAA focus whose object or an
/// ancestor has the focused state, read in full; or without it, its
/// facts alone.
fn focused(focused: bool) -> Reply {
    let mut read = focus_read(Some(held(-4)));
    let mut states = StateSet::default();
    states.insert(State::Focused);
    read.focus.read.values.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(states)),
    });
    Reply::Candidate(CandidateAnswer {
        facts: verbatim_model::request::CandidateFacts::Found {
            focused: Some(focused),
            role: None,
        },
        read: focused.then(|| Box::new(read)),
    })
}

#[test]
fn a_menu_end_checks_the_focus_50_ms_later_and_speaks_a_focused_answer() {
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(feed_at(&mut state, 1_049, Input::Tick), NOTHING);
    assert_eq!(
        feed_at(&mut state, 51_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(true))),
        SPOKEN
    );
}

/// A popup menu opened from the edit "Field" closes with no focus event
/// while the application's focus has moved to "OK", and a value change of
/// the edit sets its text-changed flag within the 50 ms: the check compares
/// the committed node, which is unchanged, so it reads and the new focus is
/// spoken (`docs/parity/focus-events.md`, "The focus after a menu or the
/// switcher closes"). Derived from NVDA's pinned source, as that document
/// gives it, under Dickson's ruling of 2026-11-20 for captures that cannot
/// be made: no known application closes a menu so.
#[test]
fn a_value_change_of_the_committed_edit_before_the_check_leaves_it_to_speak_the_moved_focus() {
    let mut state = ready();
    feed_at(&mut state, 100, Input::Foreground(IN_FRONT));
    let mut edit = focus_read(Some(held(-6)));
    edit.focus.read.node = Some(node(3));
    edit.focus.read.values = values("Field", Role::EditableText);
    feed_at(
        &mut state,
        200,
        answer(OUTPOST, 1, Reply::Candidate(CandidateAnswer::focused(edit))),
    );
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    let mut value_change = msaa_tell(-4);
    value_change.kind = EventKind::ValueChange;
    assert_eq!(feed_at(&mut state, 2_000, entry(value_change)), NOTHING);
    assert!(state.versions.live().unwrap().kept.selection.text_changed);
    feed_at(&mut state, 51_000, Input::Tick);
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(true))),
        ["Label(\"OK\") Role(Button)"]
    );
}

/// The outpost answers the check's MSAA focus not focused when neither it
/// nor any ancestor up to the desktop has the focused state, and Core
/// refuses it, as NVDA's fake focus is tested before it is queued:
/// nothing is said, and the readers' last focus and the caret baseline
/// stay as they were, since nothing was queued.
#[test]
fn a_checked_msaa_focus_answered_unfocused_is_refused_and_not_spoken() {
    let mut state = in_front();
    feed_at(&mut state, 1_000, menu_end());
    feed_at(&mut state, 51_000, Input::Tick);
    assert_eq!(
        feed_at(&mut state, 52_000, answer(OUTPOST, 2, focused(false))),
        NOTHING
    );
    assert!(state.versions.last.is_none());
    assert!(state.queue.front_for_tests().is_none());
}

/// The popup end and the menu end that closing Verbatim's menu raises
/// 0.4 ms apart, in the order of `theme_panel`'s recording, make one check:
/// NVDA's limiter keeps one menu event of a pump
/// (`orderedWinEventLimiter.py` 76 to 78, 93 to 95), so it calls
/// `_fakeFocus` once (`IAccessibleHandler/__init__.py` 976 to 991). The
/// check finds nothing there, which ends it: no second check reads the
/// dialog that has opened meanwhile, whose own focus speaks it.
#[test]
fn a_popup_end_and_a_menu_end_of_one_pump_make_one_check() {
    let mut state = in_front();
    let mut popup_end = msaa_tell(-4);
    popup_end.kind = EventKind::PopupEnd;
    assert_eq!(feed_at(&mut state, 1_000, entry(popup_end)), NOTHING);
    assert_eq!(feed_at(&mut state, 1_400, menu_end()), NOTHING);
    assert_eq!(
        feed_at(&mut state, 51_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(feed_at(&mut state, 51_400, Input::Tick), NOTHING);
    assert_eq!(
        feed_at(
            &mut state,
            60_000,
            crate::entries::tests::answer_outcome(OUTPOST, 2, Outcome::NotThere)
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// A UI Automation focus committed between the two ends of a closing
/// menu gives the later end a check of its own: the popup end at 0 notes
/// the committed focus and its check comes at 50; the UI Automation focus
/// "OK" commits at 3, NVDA's immediate pump, so the menu end at 8 falls
/// in a later pump with no valid focus, and NVDA schedules its own
/// `_fakeFocus` (`IAccessibleHandler/__init__.py` 976 to 991, 1093 to
/// 1103). The first check finds the committed focus changed and ends; the
/// second reads the UI Automation focus and speaks it again, as NVDA
/// queues a UI Automation fake focus with no duplicate test (762 to 782).
#[test]
fn a_menu_end_after_a_focus_committed_since_the_first_end_makes_its_own_check() {
    let mut state = in_front();
    let mut popup_end = msaa_tell(-4);
    popup_end.kind = EventKind::PopupEnd;
    assert_eq!(feed_at(&mut state, 1_000, entry(popup_end)), NOTHING);
    assert_eq!(
        feed_at(&mut state, 3_000, entry(uia_tell(1, Vec::new()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            3_200,
            answer(LISTENER, 2, Reply::Object(Box::new(listener_answer())))
        ),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            3_400,
            answer(OUTPOST, 3, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    assert_eq!(feed_at(&mut state, 9_000, menu_end()), NOTHING);
    assert_eq!(feed_at(&mut state, 51_000, Input::Tick), NOTHING);
    assert_eq!(
        feed_at(&mut state, 59_000, Input::Tick),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            60_000,
            answer(
                OUTPOST,
                5,
                Reply::Candidate(CandidateAnswer {
                    facts: verbatim_model::request::CandidateFacts::Found {
                        focused: None,
                        role: None,
                    },
                    read: Some(Box::new(focus_read(None))),
                })
            )
        ),
        ["Label(\"OK\") Role(Button)", "send Outpost(Pid(40)) Follow"]
    );
}

#[test]
fn a_focus_committed_before_the_check_ends_it_with_no_read() {
    let mut state = in_front();
    feed_at(&mut state, 1_000, menu_end());
    // A focus is executed within the 50 ms.
    feed_at(&mut state, 2_000, entry(msaa_tell(-5)));
    let tried = CandidateAnswer::focused(focus_read(Some(held(-5))));
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 2, Reply::Candidate(tried))
        ),
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
    let tried = CandidateAnswer::focused(focus_read(Some(held(-5))));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(OUTPOST, 2, Reply::Candidate(tried))
        ),
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
    Reply::Candidate(CandidateAnswer {
        facts: verbatim_model::request::CandidateFacts::Found {
            focused: None,
            role: Some(role),
        },
        read: Some(Box::new(read)),
    })
}

/// The run candidates' kinds of the request out.
fn asked_kinds(state: &Held) -> Vec<verbatim_model::request::CandidateKind> {
    state
        .queue
        .table_for_tests()
        .into_iter()
        .flat_map(|request| match request {
            verbatim_model::request::Request::MsaaRun(run) => vec![run.candidate.kind],
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
    assert!(state.versions.focus().is_none());
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
        answer(OUTPOST, 1, Reply::Candidate(CandidateAnswer::focused(read))),
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

/// A run waiting ahead of a UI Automation menu opening is walked as a run
/// (review, M4): a newest duplicate of the readers' last focus hides the
/// older candidate that would count, as NVDA's pump stops at the
/// duplicate and queues nothing, so the opening is kept; with the order
/// reversed the newer candidate counts and the opening is dropped.
#[test]
fn a_menu_opening_behind_a_run_whose_newest_is_a_duplicate_is_kept() {
    use std::collections::VecDeque;

    use crate::entries::Entry;
    use crate::event_queue::Item;
    use crate::focus::validity::Confirmation;

    let mut state = crate::state::SrState::default();
    state.versions.last = Some(crate::focus::versions::LastFocus {
        read_by: crate::focus::versions::ReadBy::Msaa {
            event: Some(held(-4).event),
            held_child: 0,
        },
        place: crate::focus::versions::Place {
            window: WindowHandle(5),
            top_level: WindowHandle(4),
            root_owner: WindowHandle(4),
            ancestor_windows: Vec::new().into(),
        },
        role: verbatim_model::Role::EditableText,
        since: None,
    });
    let candidate = |object: i32, at_us: u64, focused: bool| {
        let mut tell = msaa_tell(object);
        tell.at_us = at_us;
        let Some(Entry::Candidate(mut candidate)) = Entry::of_tell(LISTENER, tell) else {
            unreachable!("an MSAA focus is a candidate");
        };
        if focused {
            candidate.confirmation =
                Confirmation::Read(verbatim_model::request::CandidateFacts::Found {
                    focused: Some(true),
                    role: None,
                });
        }
        Entry::Candidate(candidate)
    };
    let Input::Entry(arriving) = opening(2, 5_000) else {
        unreachable!("an entry");
    };
    let backend = crate::focus::backend::BackendCache::default();
    let dropped = |run: Vec<Entry>| {
        let waiting = VecDeque::from([Item::Run(run)]);
        crate::focus::menus::opening_dropped(
            &state,
            (None, &waiting),
            &arriving,
            &backend,
            (6_000, Some(IN_FRONT)),
        )
    };
    // The focus confirmed focused, then the duplicate, newer.
    assert!(!dropped(vec![
        candidate(-9, 1_000, true),
        candidate(-4, 1_002, false)
    ]));
    // The duplicate, then the focus confirmed focused, newer.
    assert!(dropped(vec![
        candidate(-4, 1_000, false),
        candidate(-9, 1_002, true)
    ]));
}
