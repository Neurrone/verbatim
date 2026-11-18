//! The redirect guard's cells (design change A), a run walked strictly
//! newest first across applications, and the desktop window's candidate,
//! fed through the reducer in the listener's order. The guard's orders
//! come from recordings where one gives them: Verbatim's settings dialog
//! raising its list's own focus twice, 1.0 to 1.5 ms apart (rec passes 311
//! and 313), and mockapp's list raising its own focus and then its item's,
//! 0.1 to 0.3 ms apart, on a mouse return (`recordings-validity-design`,
//! NVDA saying the item once). The click away whose read fails has no
//! recording: that order is code-level.

use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, HeldIds, MsaaIds, Outcome as WireOutcome, Redirect, Reply,
};
use verbatim_model::tell::{EventObject, TellWindow};
use verbatim_model::{KeySeq, Pid, WindowHandle};

use crate::entries::tests::{
    Held, NOTHING, OUTPOST, SPOKEN, answer, answer_outcome, entry, feed_at, focus_read, msaa_tell,
    not_focused, ready, ready_at, redirected,
};
use crate::event_queue::{Effect, Input, Instance, Reader, Timed};

/// The list's item, child 2 of the list's client.
const ITEM: MsaaIds = MsaaIds {
    window: WindowHandle(5),
    object: -4,
    child: 2,
};

/// The list's own focus, on child 0, raised at `at_us`.
fn list_focus(held: &mut Held, at_us: u64) -> Vec<String> {
    let mut tell = msaa_tell(-4);
    tell.at_us = at_us;
    feed_at(held, at_us, entry(tell))
}

/// The item's own focus, raised at `at_us`.
fn item_focus(held: &mut Held, at_us: u64) -> Vec<String> {
    let mut tell = msaa_tell(-4);
    tell.at_us = at_us;
    tell.object = EventObject::Msaa {
        window: ITEM.window,
        object: ITEM.object,
        child: ITEM.child,
    };
    feed_at(held, at_us, entry(tell))
}

/// The list's focus answered redirected to the item by `redirect`.
fn to_item(number: u64, redirect: Redirect) -> Input<crate::entries::Entry> {
    let read = focus_read(Some(HeldIds {
        event: ITEM,
        held_child: 2,
    }));
    answer(
        OUTPOST,
        number,
        Reply::Candidate(redirected(read, redirect, ITEM)),
    )
}

/// The list's first focus read and spoken, redirected to its item: the
/// readers' last focus carries the guard with the list focus's fence.
fn guarded() -> Held {
    let mut held = ready();
    assert_eq!(
        list_focus(&mut held, 1_000),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        feed_at(&mut held, 2_000, to_item(1, Redirect::List)),
        SPOKEN
    );
    assert!(
        held.state
            .versions
            .last
            .as_ref()
            .is_some_and(|last| last.redirect.is_some())
    );
    held
}

/// Rec passes 311 and 313: the dialog's list raises its own focus twice;
/// the second, redirected to the same item with the same fence, ends
/// unspoken at its answer: the item is said once.
#[test]
fn a_lists_second_own_focus_redirected_to_the_guarded_item_ends_unspoken() {
    let mut held = guarded();
    assert_eq!(
        list_focus(&mut held, 3_000),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        feed_at(&mut held, 4_000, to_item(2, Redirect::List)),
        NOTHING
    );
}

/// The wider redirect is not NVDA's list rule: the same second focus is
/// read and spoken.
#[test]
fn a_second_focus_by_the_wider_redirect_is_spoken() {
    let mut held = guarded();
    list_focus(&mut held, 3_000);
    assert_eq!(
        feed_at(&mut held, 4_000, to_item(2, Redirect::FocusedChild)),
        ["Label(\"OK\") Role(Button)"]
    );
}

/// The mouse return's order: the list's own focus, then the item's own
/// 0.1 ms later; the item's ends with no read, so the item is said once.
#[test]
fn the_reached_items_own_focus_ends_with_no_read() {
    let mut held = guarded();
    assert_eq!(item_focus(&mut held, 2_100), NOTHING);
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A key since the list's focus moves the item's fence: its own focus is
/// read and spoken.
#[test]
fn after_a_key_the_items_own_focus_is_read_and_spoken() {
    let mut held = guarded();
    held.state.arrivals.key_joined(KeySeq::new(9));
    assert_eq!(
        item_focus(&mut held, 3_000),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    let read = focus_read(Some(HeldIds {
        event: ITEM,
        held_child: 2,
    }));
    assert_eq!(
        feed_at(
            &mut held,
            4_000,
            answer(OUTPOST, 2, Reply::Candidate(CandidateAnswer::focused(read)))
        ),
        ["Label(\"OK\") Role(Button)"]
    );
}

/// Code-level: a click on another control whose read answers not there
/// ends the guard (Dickson, 2026-11-18), so the click back on the list,
/// its own focus redirected to the item, is spoken.
#[test]
fn a_candidate_tried_and_not_there_ends_the_guard() {
    let mut held = guarded();
    assert_eq!(
        feed_at(&mut held, 3_000, entry(msaa_tell(-9))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    let gone = CandidateAnswer {
        facts: CandidateFacts::NotThere,
        read: None,
    };
    assert_eq!(
        feed_at(&mut held, 4_000, answer(OUTPOST, 2, Reply::Candidate(gone))),
        NOTHING
    );
    assert!(
        held.state
            .versions
            .last
            .as_ref()
            .is_some_and(|last| last.redirect.is_none())
    );
    list_focus(&mut held, 5_000);
    assert_eq!(
        feed_at(&mut held, 6_000, to_item(3, Redirect::List)),
        ["Label(\"OK\") Role(Button)"]
    );
}

/// A refusal without a read, a candidate never tried, leaves the guard.
#[test]
fn a_candidate_refused_with_no_read_keeps_the_guard() {
    let mut held = guarded();
    let mut unrelated = msaa_tell(-9);
    if let TellWindow::Found(facts) = &mut unrelated.window {
        facts.window_to_foreground = verbatim_model::tell::Relation::Outside;
        facts.root_owner_to_foreground = verbatim_model::tell::Relation::Outside;
        facts.root_owner_is_foregrounds = false;
    }
    assert_eq!(feed_at(&mut held, 3_000, entry(unrelated)), NOTHING);
    assert_eq!(item_focus(&mut held, 3_100), NOTHING);
}

/// Within one run, the newer candidate refused after its read comes before
/// the older valid one executes, so the guard the valid list read sets
/// stands: the refusal cannot clear it.
#[test]
fn a_refusal_in_a_run_comes_before_the_valid_read_that_sets_the_guard() {
    let mut held = ready();
    // An unrelated read holds the front while the list's focus and a newer
    // stale focus join a run behind it.
    feed_at(&mut held, 1_000, entry(msaa_tell(-9)));
    list_focus(&mut held, 1_100);
    let mut stale = msaa_tell(-7);
    stale.at_us = 1_200;
    feed_at(&mut held, 1_200, entry(stale));
    let gone = CandidateAnswer {
        facts: CandidateFacts::NotThere,
        read: None,
    };
    assert_eq!(
        feed_at(&mut held, 2_000, answer(OUTPOST, 1, Reply::Candidate(gone))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        feed_at(
            &mut held,
            3_000,
            answer(OUTPOST, 2, Reply::Candidate(not_focused()))
        ),
        ["send Outpost(Pid(40)) MsaaRun"],
        "the newest, refused, then the list"
    );
    assert_eq!(
        feed_at(&mut held, 4_000, to_item(3, Redirect::List)),
        SPOKEN
    );
    assert!(
        held.state
            .versions
            .last
            .as_ref()
            .is_some_and(|last| last.redirect.is_some())
    );
    assert_eq!(item_focus(&mut held, 4_100), NOTHING);
}

/// The other application, with its outpost ready.
const OTHER: Pid = Pid(41);

/// A run spanning two applications is tried strictly newest first, one
/// request per candidate, each of its own application's outpost, as NVDA's
/// pump has no grouping by application.
#[test]
fn a_run_spanning_two_applications_is_tried_strictly_newest_first() {
    let mut held = ready();
    let other = Instance {
        reader: Reader::Outpost(OTHER),
        generation: 5,
    };
    ready_at(&mut held, 0, other);
    feed_at(&mut held, 1_000, entry(msaa_tell(-9)));
    // Oldest first: this application's, the other's, this one's again.
    let mut first = msaa_tell(-5);
    first.at_us = 1_100;
    feed_at(&mut held, 1_100, entry(first));
    let mut second = msaa_tell(-6);
    second.at_us = 1_200;
    second.process = OTHER;
    if let TellWindow::Found(facts) = &mut second.window {
        facts.window.process = OTHER;
    }
    feed_at(&mut held, 1_200, entry(second));
    let mut third = msaa_tell(-7);
    third.at_us = 1_300;
    feed_at(&mut held, 1_300, entry(third));
    let asked = |held: &Held| {
        held.state
            .queue
            .table_for_tests()
            .into_iter()
            .find_map(|request| match request {
                verbatim_model::request::Request::MsaaRun(run) => Some(run.candidate.ids.object),
                _ => None,
            })
    };
    let gone = || {
        Reply::Candidate(CandidateAnswer {
            facts: CandidateFacts::NotThere,
            read: None,
        })
    };
    feed_at(&mut held, 2_000, answer(OUTPOST, 1, gone()));
    assert_eq!(asked(&held), Some(-7));
    assert_eq!(
        feed_at(&mut held, 3_000, answer(OUTPOST, 2, gone())),
        ["send Outpost(Pid(41)) MsaaRun"]
    );
    assert_eq!(asked(&held), Some(-6));
    assert_eq!(
        feed_at(&mut held, 4_000, answer(other, 3, gone())),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(asked(&held), Some(-5));
}

/// A focus on the desktop window is refused with no request, so Core
/// starts no outpost for the desktop window's process: no reader reads
/// it (a limit; `docs/parity/focus-events.md`).
#[test]
fn a_focus_on_the_desktop_window_starts_no_outpost_and_asks_nothing() {
    let mut held = ready();
    let csrss = Pid(6520);
    let mut tell = msaa_tell(-4);
    tell.process = csrss;
    tell.object = EventObject::Msaa {
        window: WindowHandle(65_552),
        object: -4,
        child: 0,
    };
    if let TellWindow::Found(facts) = &mut tell.window {
        facts.desktop = true;
        facts.window.process = csrss;
        facts.window.class = "#32769".to_owned();
    }
    let effects = crate::entries::feed(
        &mut held.state,
        &mut held.runtime,
        Timed {
            at_us: 1_000,
            input: entry(tell),
        },
    )
    .unwrap();
    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, Effect::StartOutpost(_) | Effect::Send { .. })),
        "{effects:?}"
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A read answered not responding says nothing about the focus: the
/// guard stays.
#[test]
fn a_read_not_responding_keeps_the_guard() {
    let mut held = guarded();
    feed_at(&mut held, 3_000, entry(msaa_tell(-9)));
    feed_at(
        &mut held,
        4_000,
        answer_outcome(OUTPOST, 2, WireOutcome::NotResponding),
    );
    assert!(
        held.state
            .versions
            .last
            .as_ref()
            .is_some_and(|last| last.redirect.is_some())
    );
}
