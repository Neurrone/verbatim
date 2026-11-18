//! The identity check in the duplicate test (`docs/parity/focus-events.md`,
//! "Duplicate focus suppression"): the readers' last focus's fence
//! (`LastFocus::since`), each path that sets it, clears it or leaves it,
//! the depth it gives a later MSAA focus, and the identity row at that
//! focus's answer; then a run walked strictly newest first across
//! applications, and the desktop window's candidate. Each is fed through
//! the reducer in the listener's order. The pairs come from recordings
//! where one gives them: Verbatim's settings dialog raising its list's own
//! focus twice after the menu check read its item
//! (`tests/fixtures/settings_dialog_keys_run02.jsonl`), and mockapp's list
//! raising its own focus and then its item's on a mouse return. The
//! refusals between a pair, the key between, the desktop switch while a
//! read is out and the outposts' answers are code-level.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, Depth, FocusRead, HeldIds, Identity, MsaaIds,
    Outcome as WireOutcome, QueueInput, Reply, Request as Wire,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{KeySeq, Pid, Role, StateSet, TraceId, WindowHandle};

use crate::entries::tests::{
    Held, LISTENER, NOTHING, OUTPOST, answer, answer_outcome, entry, feed_at, focus_read, held,
    listener_answer, msaa_tell, not_focused, ready, ready_at, uia_tell, values,
};
use crate::event_queue::{Effect, Instance, Reader, Timed};
use crate::keys::entry::Fence;

/// The list's item, child 2 of the list's client.
const ITEM: MsaaIds = MsaaIds {
    window: WindowHandle(5),
    object: -4,
    child: 2,
};

/// The window's title, spoken with the first focus in it.
const TITLE: &str = "Label(\"Editor\") Role(Window)";

/// The list's own focus, on child 0, raised at `at_us`.
fn list_tell(at_us: u64) -> Event {
    let mut tell = msaa_tell(-4);
    tell.at_us = at_us;
    tell
}

/// The item's own focus, raised at `at_us`.
fn item_tell(at_us: u64) -> Event {
    let mut tell = msaa_tell(-4);
    tell.at_us = at_us;
    tell.object = EventObject::Msaa {
        window: ITEM.window,
        object: ITEM.object,
        child: ITEM.child,
    };
    tell
}

/// The item read by its ids, named `name`, item `position` of `size`.
fn item_read(name: &str, (position, size): (u32, u32)) -> FocusRead {
    let mut read = focus_read(Some(HeldIds {
        event: ITEM,
        held_child: 2,
    }));
    let mut item = values(name, Role::ListItem);
    item.0.push(PropertyAnswer {
        id: PropertyId::PositionInSet,
        value: Some(PropertyValue::Count(position)),
    });
    item.0.push(PropertyAnswer {
        id: PropertyId::SetSize,
        value: Some(PropertyValue::Count(size)),
    });
    read.focus.read.values = item;
    read
}

/// [`item_read`] as the focused candidate's answer.
fn item_answer(name: &str, place: (u32, u32)) -> Reply {
    Reply::Candidate(CandidateAnswer::focused(item_read(name, place)))
}

/// How the item named `name`, item `position` of `size`, is spoken.
fn item_said(name: &str, (position, size): (u32, u32)) -> String {
    format!("Label({name:?}) Position {{ position: {position}, set_size: Some({size}) }}")
}

/// The identity of the item "Copy", 2 of 3, as Core names it.
fn copy_identity() -> Identity {
    Identity {
        states: StateSet::default(),
        role: Role::ListItem,
        position_in_set: Some(2),
        set_size: Some(3),
        name: Some("Copy".to_owned()),
        value: None,
    }
}

/// The depth of the one MSAA candidate out.
fn depth_out(held: &Held) -> Depth {
    let depths: Vec<Depth> = held
        .state
        .queue
        .table_for_tests()
        .into_iter()
        .filter_map(|request| match request {
            Wire::MsaaRun(run) => Some(run.depth),
            _ => None,
        })
        .collect();
    let [depth] = &depths[..] else {
        panic!("one candidate out: {depths:?}");
    };
    depth.clone()
}

/// The readers' last focus's fence, `None` with no last focus.
#[expect(
    clippy::option_option,
    reason = "no last focus, its fence cleared, or its fence"
)]
fn since(held: &Held) -> Option<Option<Fence>> {
    held.state.versions.last.as_ref().map(|last| last.since)
}

/// The item "Copy", 2 of 3, read for its own focus and spoken, after the
/// key `key` joined: the readers' last focus, whose fence is that key's.
fn spoken_copy(key: u64) -> Held {
    let mut held = ready();
    held.state.arrivals.key_joined(KeySeq::new(key));
    assert_eq!(
        feed_at(&mut held, 1_000, entry(item_tell(1_000))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(depth_out(&held), Depth::FullIfFocused);
    assert_eq!(
        feed_at(
            &mut held,
            2_000,
            answer(OUTPOST, 1, item_answer("Copy", (2, 3)))
        ),
        [TITLE.to_owned(), item_said("Copy", (2, 3))]
    );
    assert_eq!(since(&held), Some(Some(held.state.arrivals.fence())));
    held
}

/// The item's own focus again with no key between is read unless it is the
/// item by its ids and its reading; its outpost answers that it is, and
/// nothing is said.
#[test]
fn the_items_own_focus_again_is_read_unless_it_is_the_same() {
    let mut held = spoken_copy(3);
    assert_eq!(
        feed_at(&mut held, 3_000, entry(item_tell(3_000))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        depth_out(&held),
        Depth::FullUnlessSame {
            ids: ITEM,
            identity: Box::new(copy_identity()),
        }
    );
    assert_eq!(
        feed_at(&mut held, 4_000, answer(OUTPOST, 2, Reply::CandidateSame)),
        NOTHING
    );
    assert!(held.state.queue.front_for_tests().is_none());
    assert_eq!(since(&held), Some(Some(held.state.arrivals.fence())));
}

/// The recording's pair: the list's own focus behind its item, with no key
/// between, its outpost's redirect reaching the item: nothing is said.
#[test]
fn the_lists_own_focus_reaching_the_same_item_says_nothing() {
    let mut held = spoken_copy(3);
    feed_at(&mut held, 3_000, entry(list_tell(3_000)));
    assert_eq!(
        depth_out(&held),
        Depth::FullUnlessSame {
            ids: ITEM,
            identity: Box::new(copy_identity()),
        }
    );
    assert_eq!(
        feed_at(&mut held, 4_000, answer(OUTPOST, 2, Reply::CandidateSame)),
        NOTHING
    );
}

/// Outlook's case: the application put another item at the same ids, with
/// the same position and count, so its reading differs, and its outpost
/// reads it in full: it is spoken.
#[test]
fn another_item_at_the_same_ids_is_spoken() {
    let mut held = spoken_copy(3);
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(
        feed_at(
            &mut held,
            4_000,
            answer(OUTPOST, 2, item_answer("Paste", (2, 3)))
        ),
        [item_said("Paste", (2, 3))]
    );
}

/// The twin deletion: the item deleted and its twin now at its ids, the set
/// one smaller, read in full: it is spoken.
#[test]
fn the_same_reading_in_a_set_one_smaller_is_spoken() {
    let mut held = spoken_copy(3);
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(
        feed_at(
            &mut held,
            4_000,
            answer(OUTPOST, 2, item_answer("Copy", (2, 2)))
        ),
        [item_said("Copy", (2, 2))]
    );
}

/// A key since the item was spoken: its own focus is read to its focused
/// state, as any focus is, and spoken.
#[test]
fn after_a_key_the_items_own_focus_is_read_and_spoken() {
    let mut held = spoken_copy(3);
    held.state.arrivals.key_joined(KeySeq::new(4));
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(depth_out(&held), Depth::FullIfFocused);
    assert_eq!(
        feed_at(
            &mut held,
            4_000,
            answer(OUTPOST, 2, item_answer("Copy", (2, 3)))
        ),
        [item_said("Copy", (2, 3))]
    );
}

/// A last focus held by child id 0 keeps NVDA's test: equal ids are a
/// duplicate with no read, and other ids are read to their focused state,
/// never by identity.
#[test]
fn a_last_focus_held_by_child_0_has_no_identity_check() {
    let mut state = ready();
    feed_at(&mut state, 1_000, entry(msaa_tell(-4)));
    let read = focus_read(Some(held(-4)));
    feed_at(
        &mut state,
        2_000,
        answer(OUTPOST, 1, Reply::Candidate(CandidateAnswer::focused(read))),
    );
    assert_eq!(feed_at(&mut state, 3_000, entry(msaa_tell(-4))), NOTHING);
    feed_at(&mut state, 3_100, entry(item_tell(3_100)));
    assert_eq!(depth_out(&state), Depth::FullIfFocused);
}

/// Core's own focus committed over the item (a desktop switch's): the
/// last focus and its fence stay, but the committed focus is not it, so no
/// identity is named.
#[test]
fn an_own_focus_committed_leaves_the_fence_and_names_no_identity() {
    let mut held = spoken_copy(3);
    let fence = since(&held);
    held.state.versions.commit_own(false);
    assert_eq!(since(&held), fence);
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(depth_out(&held), Depth::FullIfFocused);
}

/// A run's executed candidate keeps its own fence, never the run's newest:
/// the older item focus, raised before a key, executes after the newer
/// stale focus, raised after it, is refused.
#[test]
fn a_run_keeps_its_executed_candidates_own_fence() {
    let mut held = ready();
    // An unrelated read holds the front while the run forms behind it.
    feed_at(&mut held, 1_000, entry(msaa_tell(-9)));
    held.state.arrivals.key_joined(KeySeq::new(5));
    let older = held.state.arrivals.fence();
    feed_at(&mut held, 1_100, entry(item_tell(1_100)));
    held.state.arrivals.key_joined(KeySeq::new(6));
    let mut stale = msaa_tell(-7);
    stale.at_us = 1_200;
    feed_at(&mut held, 1_200, entry(stale));
    assert_eq!(
        feed_at(
            &mut held,
            2_000,
            answer_outcome(OUTPOST, 1, WireOutcome::NotThere)
        ),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        feed_at(
            &mut held,
            3_000,
            answer(OUTPOST, 2, Reply::Candidate(not_focused()))
        ),
        ["send Outpost(Pid(40)) MsaaRun"],
        "the newest, refused, then the item"
    );
    assert_eq!(
        feed_at(
            &mut held,
            4_000,
            answer(OUTPOST, 3, item_answer("Copy", (2, 3)))
        ),
        [TITLE.to_owned(), item_said("Copy", (2, 3))]
    );
    assert_ne!(older, held.state.arrivals.fence());
    assert_eq!(since(&held), Some(Some(older)));
}

/// The outcomes of another candidate's read between the pair: what each
/// leaves of the fence, and the depth the item's own focus then gets.
#[expect(
    clippy::option_option,
    reason = "no last focus, its fence cleared, or its fence"
)]
fn between(outcome: WireOutcome) -> (Option<Option<Fence>>, Depth) {
    let mut held = spoken_copy(3);
    assert_eq!(
        feed_at(&mut held, 3_000, entry(msaa_tell(-9))),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    feed_at(&mut held, 4_000, answer_outcome(OUTPOST, 2, outcome));
    let left = since(&held);
    feed_at(&mut held, 5_000, entry(item_tell(5_000)));
    (left, depth_out(&held))
}

/// A candidate tried and refused, not there, or whose reader ended clears
/// the fence (Dickson, 2026-11-18): the item's own focus after it is read
/// in full, as the mouse return needs.
#[test]
fn a_try_refused_or_gone_clears_the_fence() {
    for outcome in [
        WireOutcome::Result(Reply::Candidate(not_focused())),
        WireOutcome::Result(Reply::Candidate(CandidateAnswer {
            facts: CandidateFacts::NotThere,
            read: None,
        })),
        WireOutcome::NotThere,
        WireOutcome::Ended,
    ] {
        assert_eq!(
            between(outcome.clone()),
            (Some(None), Depth::FullIfFocused),
            "{outcome:?}"
        );
    }
}

/// A candidate skipped, or whose read did not answer, was never tried
/// through MSAA: the fence stays.
#[test]
fn a_skip_or_no_answer_keeps_the_fence() {
    let fence = Some(Some(spoken_copy(3).state.arrivals.fence()));
    for outcome in [
        WireOutcome::Result(Reply::Candidate(CandidateAnswer {
            facts: CandidateFacts::Skipped,
            read: None,
        })),
        WireOutcome::NotResponding,
    ] {
        assert_eq!(
            between(outcome.clone()),
            (
                fence,
                Depth::FullUnlessSame {
                    ids: ITEM,
                    identity: Box::new(copy_identity()),
                }
            ),
            "{outcome:?}"
        );
    }
}

/// A foreground marker of an unrelated top-level window empties the last
/// focus and its fence.
#[test]
fn an_unrelated_foreground_empties_the_last_focus_and_its_fence() {
    let mut held = spoken_copy(3);
    feed_at(
        &mut held,
        3_000,
        crate::event_queue::Input::Foreground(crate::event_queue::Foreground {
            window: WindowHandle(90),
            root_owner: WindowHandle(90),
            process: Pid(41),
            thread: 42,
            by_class: None,
            shell: false,
        }),
    );
    // The marker waits behind the startup read the first report appends,
    // until Core stops waiting for it.
    feed_at(&mut held, 13_004_000, crate::event_queue::Input::Tick);
    assert_eq!(since(&held), None);
}

/// A UI Automation focus executed sets its own fence.
#[test]
fn a_uia_focus_sets_its_fence() {
    let mut held = ready();
    held.state.arrivals.key_joined(KeySeq::new(7));
    crate::entries::tests::uia_focus_read(
        &mut held,
        1_000,
        uia_tell(1, Vec::new()),
        focus_read(None),
    );
    assert_eq!(since(&held), Some(Some(held.state.arrivals.fence())));
}

/// A UI Automation focus whose element is gone at its completion clears
/// the fence.
#[test]
fn a_uia_focus_gone_at_its_completion_clears_the_fence() {
    let mut held = spoken_copy(3);
    let (asked, _) =
        crate::entries::tests::sent_at(&mut held, 3_000, entry(uia_tell(1, Vec::new())));
    let [object] = asked[..] else {
        panic!("one listener read: {asked:?}");
    };
    let (asked, _) = crate::entries::tests::sent_at(
        &mut held,
        3_001,
        answer(LISTENER, object, Reply::Object(Box::new(listener_answer()))),
    );
    let [completion] = asked[..] else {
        panic!("one completion: {asked:?}");
    };
    feed_at(
        &mut held,
        3_002,
        answer_outcome(OUTPOST, completion, WireOutcome::NotThere),
    );
    assert_eq!(since(&held), Some(None));
}

/// The listener's tell of an input desktop switch, `open` when the new one
/// can be opened.
fn switch(open: bool) -> QueueInput {
    QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::DesktopSwitch,
            process: crate::entries::tests::APP,
            application: "app".to_owned(),
            thread: 41,
            at_us: 3_100,
            object: EventObject::Msaa {
                window: WindowHandle(0),
                object: 0,
                child: 0,
            },
            cached: PropertyValues::default(),
            payload: Payload::DesktopSwitch {
                input_desktop_open: open,
            },
            window: TellWindow::NoWindow,
            checked: None,
        }),
    }
}

/// A desktop switch while the item's identity read is out commits Core's
/// own focus, which leaves the readers' last focus: the answer that it is
/// the item is still a duplicate, and the read after the return speaks the
/// focus.
#[test]
fn a_desktop_switch_while_the_identity_read_is_out_leaves_its_answer_a_duplicate() {
    let mut held = spoken_copy(3);
    // The window in front, its first report's startup read answered with
    // the item.
    let in_front = crate::event_queue::Foreground {
        window: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: crate::entries::tests::APP,
        thread: 41,
        by_class: None,
        shell: false,
    };
    assert_eq!(
        feed_at(
            &mut held,
            2_500,
            crate::event_queue::Input::Foreground(in_front)
        ),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        feed_at(
            &mut held,
            2_600,
            answer(OUTPOST, 2, item_answer("Copy", (2, 3)))
        ),
        [item_said("Copy", (2, 3))]
    );
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(
        depth_out(&held),
        Depth::FullUnlessSame {
            ids: ITEM,
            identity: Box::new(copy_identity()),
        }
    );
    let _ = crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), 3_100),
        &switch(false),
    );
    assert_eq!(
        feed_at(&mut held, 3_200, answer(OUTPOST, 3, Reply::CandidateSame)),
        NOTHING
    );
    let _ = crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), 4_000),
        &switch(true),
    );
    let due = 4_000 + crate::input_desktop::RETURN_DELAY_US;
    let _ = crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), due),
        &QueueInput::Due,
    );
    assert_eq!(
        held.state.queue.table_for_tests().len(),
        1,
        "the read after the return"
    );
    assert_eq!(
        feed_at(
            &mut held,
            due + 1_000,
            answer(OUTPOST, 4, item_answer("Copy", (2, 3)))
        ),
        [TITLE.to_owned(), item_said("Copy", (2, 3))]
    );
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
                Wire::MsaaRun(run) => Some(run.candidate.ids.object),
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

/// The read afresh of a UI Automation focus, after its walk met an
/// ancestor Core no longer knows, keeps the focus entry's fence.
#[test]
fn the_read_afresh_keeps_its_entrys_fence() {
    use verbatim_model::property::RuntimeId;
    use verbatim_model::request::{KnownAncestor, RouteEnd};
    let mut held = ready();
    held.state.arrivals.key_joined(KeySeq::new(8));
    feed_at(&mut held, 1_000, entry(uia_tell(1, Vec::new())));
    let mut object = listener_answer();
    if let Some(route) = object.route.as_mut() {
        route.end = RouteEnd::Known(KnownAncestor::Uia(RuntimeId(vec![42, 4])));
    }
    assert_eq!(
        feed_at(
            &mut held,
            2_000,
            answer(LISTENER, 1, Reply::Object(Box::new(object)))
        ),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    feed_at(
        &mut held,
        3_000,
        answer(
            OUTPOST,
            2,
            Reply::Candidate(CandidateAnswer::focused(focus_read(None))),
        ),
    );
    assert_eq!(since(&held), Some(Some(held.state.arrivals.fence())));
}

/// One step of the recording's pair in some order.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// The menu's end, which makes the check 50 ms later.
    MenuEnd,
    /// The time passes the check's due time.
    Due,
    /// One of the list's two own focuses arrives.
    ListFocus,
    /// The request out is answered as the settings dialog's outpost
    /// answered it: the item "Speech", 1 of 3, read in full, or, asked
    /// unless it is that item, that it is.
    Answer,
}

/// The pair played in `order` from the window in front, every utterance it
/// said, and how many requests were asked.
fn pair_in(order: &[Step]) -> (Vec<String>, usize) {
    let mut held = ready();
    let in_front = crate::event_queue::Foreground {
        window: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: crate::entries::tests::APP,
        thread: 41,
        by_class: None,
        shell: false,
    };
    feed_at(
        &mut held,
        100,
        crate::event_queue::Input::Foreground(in_front),
    );
    feed_at(
        &mut held,
        200,
        answer_outcome(OUTPOST, 1, WireOutcome::NotThere),
    );
    let mut said = Vec::new();
    let mut out: Vec<(u64, Wire)> = Vec::new();
    let mut asked = 0;
    let mut at_us = 1_000;
    for step in order {
        at_us += 1_000;
        let input = match step {
            Step::MenuEnd => {
                let mut tell = msaa_tell(-4);
                tell.kind = EventKind::MenuEnd;
                tell.at_us = at_us;
                entry(tell)
            }
            Step::Due => {
                at_us += 60_000;
                crate::event_queue::Input::Tick
            }
            Step::ListFocus => entry(list_tell(at_us)),
            Step::Answer => {
                let (number, request) = out.remove(0);
                let reply = match request {
                    Wire::MsaaRun(run) if matches!(run.depth, Depth::FullUnlessSame { .. }) => {
                        Reply::CandidateSame
                    }
                    _ => item_answer("Speech", (1, 3)),
                };
                answer(OUTPOST, number, reply)
            }
        };
        let effects =
            crate::entries::feed(&mut held.state, &mut held.runtime, Timed { at_us, input })
                .unwrap();
        for effect in effects {
            match effect {
                Effect::Send { ask, .. } => {
                    asked += 1;
                    out.push((ask.number.0, ask.request));
                }
                Effect::Said(verbatim_model::Effect::Speak(utterance, _)) => said.push(
                    utterance
                        .segments
                        .iter()
                        .filter(|segment| {
                            !matches!(segment.content, verbatim_model::SegmentContent::Mark(_))
                        })
                        .map(|segment| format!("{:?}", segment.content))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => {}
            }
        }
    }
    assert!(out.is_empty(), "every request answered: {out:?}");
    (said, asked)
}

/// Every order of the menu check's answer and the list's two own focuses,
/// with the recording's readings, says the item exactly once: the check's
/// read first, each focus then answered that it is the item; or the run
/// first, the check then ending with no read as the committed focus
/// changed.
#[test]
fn every_order_of_the_check_and_the_pair_says_the_item_once() {
    use Step::{Answer, Due, ListFocus, MenuEnd};
    let once = vec![TITLE.to_owned(), item_said("Speech", (1, 3))];
    for (order, asked) in [
        (
            vec![MenuEnd, Due, Answer, ListFocus, Answer, ListFocus, Answer],
            3,
        ),
        (
            vec![MenuEnd, Due, ListFocus, Answer, ListFocus, Answer, Answer],
            3,
        ),
        (vec![MenuEnd, Due, ListFocus, ListFocus, Answer, Answer], 2),
        (vec![MenuEnd, ListFocus, ListFocus, Answer, Answer, Due], 2),
        (vec![MenuEnd, ListFocus, Answer, Due, ListFocus, Answer], 2),
    ] {
        assert_eq!(pair_in(&order), (once.clone(), asked), "{order:?}");
    }
}

/// With position reporting off, the focus read still asks the position and
/// the set's size, which the identity names, and the focus is spoken
/// without them: the setting decides only what is spoken (Dickson,
/// 2026-11-18).
#[test]
fn with_position_reporting_off_the_read_asks_the_position_and_speech_leaves_it_out() {
    use verbatim_model::property::{NodeDetail, NodeDetailSet};
    let mut held = ready();
    held.state.reported.node = NodeDetailSet::ALL.without(NodeDetail::Position);
    held.state.arrivals.key_joined(KeySeq::new(3));
    feed_at(&mut held, 1_000, entry(item_tell(1_000)));
    let set: Vec<PropertyId> = held
        .state
        .queue
        .table_for_tests()
        .into_iter()
        .find_map(|request| match request {
            Wire::MsaaRun(run) => Some(run.set),
            _ => None,
        })
        .expect("the item asked");
    assert!(
        set.contains(&PropertyId::PositionInSet) && set.contains(&PropertyId::SetSize),
        "{set:?}"
    );
    assert_eq!(
        feed_at(
            &mut held,
            2_000,
            answer(OUTPOST, 1, item_answer("Copy", (2, 3)))
        ),
        [TITLE, "Label(\"Copy\")"]
    );
    feed_at(&mut held, 3_000, entry(item_tell(3_000)));
    assert_eq!(
        depth_out(&held),
        Depth::FullUnlessSame {
            ids: ITEM,
            identity: Box::new(copy_identity()),
        }
    );
}
