//! The check after a menu or switcher ends, and the UI Automation menu
//! opening.

use crate::entries::tests::found_at;
use crate::readers::Found;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{FrontWindow, Instance, LiveFront, Outcome, Reader};
use verbatim_model::tell::{ElementToken, Read};
use verbatim_model::{Pid, Role, State, StateSet, WindowHandle};

use crate::entries::tests::{
    APP, Held, LISTENER, NOTHING, OUTPOST, SPOKEN, SPOKEN_FOLLOWED, asked_completion, due_at,
    entry, feed_at, focus_read, held, listener_answer, msaa_tell, node, ready, ready_at, uia_tell,
    values,
};
use crate::event_queue::{Foreground, Input};

const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: Some(APP),
    thread: 41,
    by_class: None,
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

/// Windows naming the foreground record's window in front, as the shell
/// reads it for a wake that finds a check waiting.
fn the_records() -> LiveFront {
    LiveFront::Window(FrontWindow {
        window: IN_FRONT.window,
        root_owner: IN_FRONT.root_owner,
        process: IN_FRONT.process.expect("the test front names its process"),
        thread: IN_FRONT.thread,
        class: String::new(),
    })
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
fn focused(focused: bool) -> Found {
    let mut read = focus_read(Some(held(-4)));
    let mut states = StateSet::default();
    states.insert(State::Focused);
    read.focus.read.values.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(states)),
    });
    if focused {
        Found::Read(read)
    } else {
        Found::Unfocused
    }
}

/// The focus it speaks keeps the check's own fence, the key before the
/// menu ended, not a key that joined while the check was out.
#[test]
fn a_menu_end_checks_the_focus_50_ms_later_and_speaks_a_focused_answer() {
    let mut state = in_front();
    state
        .state
        .arrivals
        .key_joined(verbatim_model::KeySeq::new(3));
    let fence = state.state.arrivals.fence();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(due_at(&mut state, 1_049, Some(the_records())), NOTHING);
    assert_eq!(
        due_at(&mut state, 51_000, Some(the_records())),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    state
        .state
        .arrivals
        .key_joined(verbatim_model::KeySeq::new(4));
    assert_eq!(
        found_at(&mut state, 52_000, (OUTPOST, 2), focused(true)),
        SPOKEN
    );
    assert_eq!(
        state.state.versions.last.as_ref().map(|last| last.since),
        Some(Some(fence))
    );
}

/// A check whose focus the outpost reached through `accFocus`, an object
/// with no address of its own, names no event ids, so a later focus event
/// on the window's client is no duplicate of it and is read, as NVDA reads
/// it (Dickson, 2026-11-27): before, the outpost sent the object's key in
/// its store, the client's ids, and the client's own focus was dropped as
/// a duplicate.
#[test]
fn a_focus_after_a_check_that_read_an_object_with_no_address_is_read() {
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(
        due_at(&mut state, 51_000, Some(the_records())),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    let no_address = verbatim_model::request::HeldIds {
        event: None,
        held_child: 0,
    };
    let Found::Read(mut read) = focused(true) else {
        panic!("a focused answer");
    };
    read.msaa = Some(no_address);
    assert_eq!(
        found_at(&mut state, 52_000, (OUTPOST, 2), Found::Read(read)),
        SPOKEN
    );
    assert_eq!(
        state.versions.last.as_ref().map(|last| &last.read_by),
        Some(&crate::focus::versions::ReadBy::Msaa {
            event: None,
            held_child: 0
        })
    );
    let mut client = msaa_tell(-4);
    client.at_us = 60_000;
    assert_eq!(
        feed_at(&mut state, 60_000, entry(client)),
        ["send Outpost(Pid(40)) Candidate"]
    );
}

/// A check whose focus the outpost reached through `accFocus` and named by
/// the address its identity string gives is the duplicate of that object's
/// own later focus event at the address, which says nothing, as NVDA's
/// object with ids from `IAccIdentity` is (Dickson, 2026-11-27).
#[test]
fn a_focus_at_the_address_a_check_named_its_object_by_is_a_duplicate() {
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(
        due_at(&mut state, 51_000, Some(the_records())),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(
        found_at(&mut state, 52_000, (OUTPOST, 2), focused(true)),
        SPOKEN
    );
    let mut own = msaa_tell(-4);
    own.at_us = 60_000;
    assert_eq!(feed_at(&mut state, 60_000, entry(own)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

/// A menu ends while the foreground record still names its application,
/// the listener's report of the window that came in front being late, and
/// Windows names another application's window in front when the check
/// comes due: the check asks that application, as NVDA's fake focus asks
/// Windows for the foreground window when its timer runs
/// (`IAccessibleHandler/__init__.py` 991 to 995; `NVDAObjects/window/__init__.py`
/// 162 to 181; Dickson, 2026-11-26).
#[test]
fn a_check_asks_the_application_windows_names_in_front_not_the_records() {
    const OTHER: Instance = Instance {
        reader: Reader::Outpost(Pid(70)),
        generation: 9,
    };
    let mut state = in_front();
    assert_eq!(ready_at(&mut state, 300, OTHER), NOTHING);
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    let other = FrontWindow {
        window: WindowHandle(8),
        root_owner: WindowHandle(8),
        process: Pid(70),
        thread: 71,
        class: String::new(),
    };
    assert_eq!(
        due_at(&mut state, 51_000, Some(LiveFront::Window(other))),
        ["send Outpost(Pid(70)) CurrentFocus with the focused state as a fact"]
    );
}

/// A wake that finds no window in front names the record's window, whose
/// request the shell answers unsent with the desktop
/// (`tests/hung_menu_end.rs`); one whose window in front went before it was
/// read asks nothing, as NVDA's fake focus does nothing with no focus
/// object (`IAccessibleHandler/__init__.py` 995 to 997).
#[test]
fn a_check_with_no_window_in_front_names_the_records_and_a_gone_one_asks_nothing() {
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(
        due_at(&mut state, 51_000, Some(LiveFront::Nothing)),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    let mut state = in_front();
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    assert_eq!(due_at(&mut state, 51_000, Some(LiveFront::Gone)), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
    assert!(state.state.queue_idle());
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
    found_at(&mut state, 200, (OUTPOST, 1), Found::Read(edit));
    assert_eq!(feed_at(&mut state, 1_000, menu_end()), NOTHING);
    let mut value_change = msaa_tell(-4);
    value_change.kind = EventKind::ValueChange;
    assert_eq!(feed_at(&mut state, 2_000, entry(value_change)), NOTHING);
    assert!(state.versions.live().unwrap().kept.selection.text_changed);
    due_at(&mut state, 51_000, Some(the_records()));
    assert_eq!(
        found_at(&mut state, 52_000, (OUTPOST, 2), focused(true)),
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
    due_at(&mut state, 51_000, Some(the_records()));
    assert_eq!(
        found_at(&mut state, 52_000, (OUTPOST, 2), focused(false)),
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
        due_at(&mut state, 51_000, Some(the_records())),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(due_at(&mut state, 51_400, Some(the_records())), NOTHING);
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
        found_at(&mut state, 3_200, (LISTENER, 2), listener_answer()),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        found_at(
            &mut state,
            3_400,
            (OUTPOST, 3),
            Found::Read(focus_read(None))
        ),
        SPOKEN_FOLLOWED
    );
    assert_eq!(feed_at(&mut state, 9_000, menu_end()), NOTHING);
    assert_eq!(due_at(&mut state, 51_000, Some(the_records())), NOTHING);
    assert_eq!(
        due_at(&mut state, 59_000, Some(the_records())),
        ["send Outpost(Pid(40)) CurrentFocus with the focused state as a fact"]
    );
    assert_eq!(
        found_at(
            &mut state,
            60_000,
            (OUTPOST, 5),
            Found::Read(focus_read(None))
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
    let tried = Found::Read(focus_read(Some(held(-5))));
    assert_eq!(found_at(&mut state, 3_000, (OUTPOST, 2), tried), SPOKEN);
    assert_eq!(due_at(&mut state, 51_000, Some(the_records())), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_menu_end_after_a_run_that_executed_a_focus_makes_no_check() {
    let mut state = in_front();
    // The run asks; the menu end waits behind it.
    assert_eq!(
        feed_at(&mut state, 1_000, entry(msaa_tell(-5))),
        ["send Outpost(Pid(40)) Candidate"]
    );
    feed_at(&mut state, 1_100, menu_end());
    let tried = Found::Read(focus_read(Some(held(-5))));
    assert_eq!(found_at(&mut state, 2_000, (OUTPOST, 2), tried), SPOKEN);
    assert_eq!(due_at(&mut state, 60_000, Some(the_records())), NOTHING);
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
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    let again = feed_at(&mut state, 4_000, opening(1, 4_000));
    assert!(state.queue.front_for_tests().is_none());
    assert_eq!(again, ["Label(\"OK\") Role(Button)"]);
}

#[test]
fn a_menu_opening_on_the_committed_focus_that_lost_the_keyboard_focus_is_dropped() {
    let mut state = asked_completion();
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
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
fn read_as(role: verbatim_model::Role) -> Found {
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = crate::entries::tests::values("Context", role);
    Found::Role(Some(role), Some(read))
}

/// Whether each candidate out is read as a menu start: its object at its
/// event's ids with no redirect, its role read.
fn asked_kinds(state: &Held) -> Vec<bool> {
    state
        .queue
        .batches_for_tests()
        .into_iter()
        .filter_map(|(_, batch)| {
            batch.steps.iter().find_map(|step| match step.op.subject() {
                Some(verbatim_model::batch::Subject::Msaa { redirect, .. }) => Some(!redirect),
                _ => None,
            })
        })
        .collect()
}

#[test]
fn a_menu_start_reads_its_object_and_a_menu_popup_executes_as_a_focus() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, menu_start()),
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(asked_kinds(&state), [true]);
    assert_eq!(
        found_at(
            &mut state,
            2_000,
            (OUTPOST, 1),
            read_as(verbatim_model::Role::Menu)
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
        found_at(
            &mut state,
            2_000,
            (OUTPOST, 1),
            read_as(verbatim_model::Role::Button)
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
        ["send Outpost(Pid(40)) Candidate"]
    );
    assert_eq!(feed_at(&mut state, 1_100, menu_start()), NOTHING);
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = crate::entries::tests::values("File", role);
    found_at(&mut state, 2_000, (OUTPOST, 1), Found::Read(read))
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
            "send Outpost(Pid(40)) Candidate"
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
            event: held(-4).event,
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
        let Some(Entry::Candidate(mut candidate)) = Entry::of_tell(LISTENER, tell, false) else {
            unreachable!("an MSAA focus is a candidate");
        };
        if focused {
            candidate.confirmation =
                Confirmation::Read(crate::focus::validity::CandidateFacts::Found {
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
