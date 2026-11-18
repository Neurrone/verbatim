//! The caret key's confirmation of an unconfirmed MSAA focus (design
//! "Confirming, and its bound"): every cell of the two paths' stage
//! tables, then the sequences through the reducer in the listener's order:
//! the watch withdrawn by an unconfirmed focus, the confirmation's answers
//! and endings, the bound, staggered arrivals and the held starting point.
//! The orders are code-level: a stray focus inside a 100 ms watch has no
//! trigger a capture can produce on demand, and NVDA's speech is from
//! source (`editableText.py` 93 to 99; `IAccessibleHandler/__init__.py`
//! 762 to 782).

use verbatim_model::request::{CandidateAnswer, CandidateFacts, Depth, MsaaRun, RunCandidate};
use verbatim_model::request::{CandidateKind, MsaaIds, WindowVerdict};

use super::*;
use crate::entries::tests::msaa_tell;
use crate::event_queue::Reader;
use crate::focus::validity::Pending;
use crate::keys::caret_key::{Arrival, CaretPress, CaretStage, TerminalStage};

const KEY: Arrival = Arrival::Key;
const OTHER: Arrival = Arrival::Other;

fn focus_arrival(pending: Pending, at_us: u64) -> Arrival {
    Arrival::Focus { pending, at_us }
}

fn counts() -> Arrival {
    focus_arrival(Pending::Counts, 1_000)
}

fn unconfirmed(at_us: u64) -> Arrival {
    focus_arrival(Pending::Unconfirmed(TraceId::from_raw(100)), at_us)
}

fn not_pending() -> Arrival {
    focus_arrival(Pending::No, 1_000)
}

/// The edit path's table: each stage against each arrival, the watch's
/// bound at 100 ms.
#[test]
fn every_cell_of_the_edit_paths_stage_table() {
    let starting = CaretStage::Starting {
        deletes_back: false,
    };
    let backspace = CaretStage::Starting { deletes_back: true };
    let watching = CaretStage::Watching {
        bound_at_us: 100_000,
    };
    for (stage, arrival, withdrawn) in [
        (starting, KEY, false),
        (starting, counts(), true),
        (backspace, counts(), false),
        (starting, unconfirmed(50_000), false),
        (starting, not_pending(), false),
        (starting, OTHER, false),
        (watching, KEY, true),
        (watching, counts(), true),
        (watching, unconfirmed(99_999), true),
        (watching, unconfirmed(100_000), false),
        (watching, not_pending(), false),
        (watching, OTHER, false),
        (CaretStage::Confirming, KEY, true),
        (CaretStage::Confirming, counts(), true),
        (CaretStage::Confirming, unconfirmed(50_000), false),
        (CaretStage::Confirming, not_pending(), false),
        (CaretStage::Sentence, KEY, false),
        (CaretStage::Sentence, counts(), true),
        (CaretStage::Sentence, unconfirmed(50_000), false),
        (CaretStage::Sentence, not_pending(), false),
    ] {
        assert_eq!(
            stage.withdrawn_by(arrival),
            withdrawn,
            "{stage:?} {arrival:?}"
        );
    }
}

/// The terminal path's table: its final read comes after the bound, so an
/// unconfirmed focus never withdraws it.
#[test]
fn every_cell_of_the_terminal_paths_stage_table() {
    let watching = TerminalStage::Watching {
        bound_at_us: 300_000,
    };
    for (stage, arrival, withdrawn) in [
        (TerminalStage::Starting, KEY, false),
        (TerminalStage::Starting, counts(), true),
        (TerminalStage::Starting, unconfirmed(50_000), false),
        (TerminalStage::Starting, not_pending(), false),
        (watching, KEY, true),
        (watching, counts(), true),
        (watching, unconfirmed(299_999), true),
        (watching, unconfirmed(300_000), false),
        (watching, not_pending(), false),
        (TerminalStage::Confirming, KEY, true),
        (TerminalStage::Confirming, counts(), true),
        (TerminalStage::Confirming, unconfirmed(50_000), false),
        (TerminalStage::Final, KEY, true),
        (TerminalStage::Final, counts(), true),
        (TerminalStage::Final, unconfirmed(50_000), false),
        (TerminalStage::Final, not_pending(), false),
    ] {
        assert_eq!(
            stage.withdrawn_by(arrival),
            withdrawn,
            "{stage:?} {arrival:?}"
        );
    }
}

/// The stage is read from the request out: a confirmation is an MSAA
/// candidate asked for its facts alone.
#[test]
fn the_stage_is_derived_from_the_request_out() {
    let held = editing();
    let binding = bound(&held);
    let press = CaretPress {
        trace_id: TraceId::from_raw(71),
        key_seq: KeySeq::new(1),
        pressed_at_us: 1_000,
        key: caret(CaretMotion::NextCharacter),
        held: true,
        bound: Some(binding),
    };
    let run = |depth| {
        Wire::MsaaRun(MsaaRun {
            candidate: RunCandidate {
                ids: MsaaIds {
                    window: WindowHandle(5),
                    object: -5,
                    child: 0,
                },
                kind: CandidateKind::Focus,
                verdict: WindowVerdict::ProbeFirst,
            },
            depth,
            spare: None,
            set: Vec::new(),
            known: Vec::new(),
            details: TextDetails::NONE,
        })
    };
    assert_eq!(
        CaretStage::of(&press, &run(Depth::Facts)),
        Some(CaretStage::Confirming)
    );
    assert_eq!(CaretStage::of(&press, &run(Depth::Full)), None);
    let watch = Wire::Text {
        node: node(2),
        op: watch(CaretMotion::NextCharacter, 5_000, true),
    };
    assert_eq!(
        CaretStage::of(&press, &watch),
        Some(CaretStage::Watching {
            bound_at_us: 105_000
        })
    );
}

/// An MSAA focus on object `object` of window 5, raised at `at_us`, of
/// trace `trace`.
fn msaa_focus(object: i32, at_us: u64, trace: u64) -> Input<Entry> {
    let mut tell = msaa_tell(object);
    tell.at_us = at_us;
    tell.trace_id = TraceId::from_raw(trace);
    entry(tell)
}

/// A confirmation's answer: found, with `focused`.
fn confirmed(number: u64, focused: bool) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        Reply::Candidate(CandidateAnswer {
            facts: CandidateFacts::Found {
                focused: Some(focused),
                role: None,
                window_class: "Button".to_owned(),
                redirect: None,
            },
            read: None,
        }),
    )
}

/// The confirmation of the candidate on `object`, request `number`, the
/// key's starting point held.
fn confirming(number: u64, object: i32) -> (u64, i32, Reader, Depth, Option<TextAnchor>) {
    (
        number,
        object,
        Reader::Outpost(APP),
        Depth::Facts,
        Some(TextAnchor(50)),
    )
}

/// The watch's effect: the caret moved onto "i".
fn moved_onto_i(number: u64) -> Input<Entry> {
    watched(
        number,
        Watched::Effect(read(
            line("line 01", 9, 1),
            unit(TextUnit::Character, "i", 10),
        )),
    )
}

/// Right Arrow watching, its watch request 4 passed at 5 ms, bound at
/// 105 ms; an MSAA focus raised at 29 ms withdraws it and is confirmed as
/// request 5.
fn confirming_after_the_watch() -> Held {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let arrived = step(&mut held, 30_000, msaa_focus(-5, 29_000, 100));
    assert_eq!(arrived.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(arrived.said, NO_WORDS);
    assert_eq!(arrived.runs, [confirming(5, -5)]);
    held
}

/// Disagreement 5's shape: a focus not focused leaves the key to watch the
/// rest of its bound, and it speaks; at the focus's turn the stored
/// refusal asks nothing.
#[test]
fn an_unconfirmed_focus_found_not_focused_lets_the_key_watch_on_and_speak() {
    let mut held = confirming_after_the_watch();
    let answered = step(&mut held, 31_000, confirmed(5, false));
    assert_eq!(
        answered.text,
        [(6, 2, watch(CaretMotion::NextCharacter, 5_000, true))]
    );
    let ended = step(&mut held, 40_000, moved_onto_i(6));
    assert_eq!(ended.said, [character("i")]);
    assert_eq!(
        ended.runs,
        [],
        "the stored refusal asks nothing at its turn"
    );
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A focus confirmed focused counts: the key is silent, and the focus is
/// read in full at its own turn.
#[test]
fn an_unconfirmed_focus_found_focused_silences_the_key_and_is_read_at_its_turn() {
    let mut held = confirming_after_the_watch();
    let answered = step(&mut held, 31_000, confirmed(5, true));
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(answered.text, NO_REQUESTS);
    assert_eq!(
        answered.runs,
        [(6, -5, Reader::Outpost(APP), Depth::Full, None)]
    );
}

/// Backspace silenced by a confirmed focus still says what it deleted.
#[test]
fn backspace_silenced_by_a_confirmed_focus_says_what_it_deleted() {
    let mut held = watching(
        CaretMotion::Backspace,
        line("x", 7, 1),
        unit(TextUnit::Character, "x", 8),
    );
    let arrived = step(&mut held, 30_000, msaa_focus(-5, 29_000, 100));
    assert_eq!(arrived.runs, [confirming(5, -5)]);
    let answered = step(&mut held, 31_000, confirmed(5, true));
    assert_eq!(answered.said, [character("x")]);
}

/// A key arriving while the confirmation is out withdraws it: the key is
/// marked and silent, and the late answer is dropped unstored, so the
/// focus is asked again at its own turn.
#[test]
fn a_key_withdraws_the_confirmation_and_the_focus_is_asked_at_its_turn() {
    let mut held = confirming_after_the_watch();
    let binding = bound(&held);
    let key = step(
        &mut held,
        31_000,
        held_key(2, CaretMotion::NextCharacter, 31_000, &binding),
    );
    assert_eq!(key.cancelled, [(5, CancelReason::Withdrawn)]);
    assert_eq!(key.said, NO_WORDS);
    assert_eq!(
        key.runs,
        [(6, -5, Reader::Outpost(APP), Depth::FullIfFocused, None)],
        "the focus, ahead of key 2, at its own turn with nothing stored"
    );
}

/// A counting focus arriving while the confirmation is out withdraws it.
#[test]
fn a_counting_focus_withdraws_the_confirmation_and_the_key_is_silent() {
    let mut held = confirming_after_the_watch();
    let focus = step(&mut held, 31_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus.cancelled, [(5, CancelReason::Withdrawn)]);
    assert_eq!(focus.said, NO_WORDS);
}

/// Three unconfirmed focuses arriving one after another, each while the
/// previous one's confirmation is out, are read once each, newest first,
/// and none again at its turn.
#[test]
fn three_staggered_unconfirmed_focuses_are_read_once_each_and_not_at_their_turns() {
    let mut held = confirming_after_the_watch();
    let second = step(&mut held, 32_000, msaa_focus(-6, 31_000, 101));
    assert_eq!(second, Step::default(), "kept while the first is confirmed");
    let answered = step(&mut held, 33_000, confirmed(5, false));
    assert_eq!(answered.runs, [confirming(6, -6)]);
    let third = step(&mut held, 34_000, msaa_focus(-7, 33_500, 102));
    assert_eq!(third, Step::default());
    assert_eq!(
        step(&mut held, 35_000, confirmed(6, false)).runs,
        [confirming(7, -7)]
    );
    let last = step(&mut held, 36_000, confirmed(7, false));
    assert_eq!(
        last.text,
        [(8, 2, watch(CaretMotion::NextCharacter, 5_000, true))]
    );
    let ended = step(&mut held, 40_000, moved_onto_i(8));
    assert_eq!(ended.said, [character("i")]);
    assert_eq!(ended.runs, []);
    assert!(held.state.queue.front_for_tests().is_none());
}

/// A focus raised before the bound and confirmed after it still counts,
/// however long the confirmation took.
#[test]
fn a_focus_raised_before_the_bound_and_confirmed_after_it_counts() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let arrived = step(&mut held, 104_500, msaa_focus(-5, 104_000, 100));
    assert_eq!(arrived.runs, [confirming(5, -5)]);
    let answered = step(&mut held, 160_000, confirmed(5, true));
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(
        answered.runs,
        [(6, -5, Reader::Outpost(APP), Depth::Full, None)]
    );
}

/// A focus raised at or after the bound neither withdraws the watch nor is
/// confirmed, as NVDA's wait ends at its timeout.
#[test]
fn a_focus_raised_after_the_bound_does_not_withdraw_the_watch() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let arrived = step(&mut held, 106_000, msaa_focus(-5, 105_000, 100));
    assert_eq!(arrived, Step::default());
    let ended = step(&mut held, 107_000, moved_onto_i(4));
    assert_eq!(ended.said, [character("i")]);
    assert_eq!(
        ended.runs,
        [(5, -5, Reader::Outpost(APP), Depth::FullIfFocused, None)]
    );
}

/// While the starting read is out an unconfirmed focus waits; at the pass
/// it is confirmed, with no watch asked.
#[test]
fn an_unconfirmed_focus_during_the_starting_read_is_confirmed_at_the_pass() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let arrived = step(&mut held, 2_000, msaa_focus(-5, 1_500, 100));
    assert_eq!(arrived, Step::default(), "the starting read goes on");
    let passed = step(
        &mut held,
        5_000,
        started(3, line("line 01", 7, 0), unit(TextUnit::Character, "l", 8)),
    );
    assert_eq!(passed.strokes, [vec![1]]);
    assert_eq!(passed.text, NO_REQUESTS);
    assert_eq!(passed.runs, [confirming(4, -5)]);
}

/// The candidate's outpost ending while it is confirmed stores it as not
/// there, and the key watches on.
#[test]
fn a_confirmation_whose_outpost_ended_stores_not_there_and_the_key_watches_on() {
    let mut held = confirming_after_the_watch();
    let ended = step(
        &mut held,
        31_000,
        answer_outcome(OUTPOST, 5, WireOutcome::Ended),
    );
    assert_eq!(
        ended.text,
        [(6, 2, watch(CaretMotion::NextCharacter, 5_000, true))]
    );
}

/// A confirmation whose window does not respond leaves the key silent;
/// Backspace still says what it deleted.
#[test]
fn a_confirmation_not_responding_leaves_the_key_silent() {
    let mut held = watching(
        CaretMotion::Backspace,
        line("x", 7, 1),
        unit(TextUnit::Character, "x", 8),
    );
    let _ = step(&mut held, 30_000, msaa_focus(-5, 29_000, 100));
    let hung = step(
        &mut held,
        31_000,
        answer_outcome(OUTPOST, 5, WireOutcome::NotResponding),
    );
    assert_eq!(hung.said, [character("x")]);
    assert_eq!(hung.text, NO_REQUESTS);
}

/// A confirmation that finds the window read through UI Automation stores
/// it skipped: the key watches on, and at its turn the candidate is
/// refused with no request, its window judged as UI Automation.
#[test]
fn a_skipped_confirmation_leaves_the_window_judged_as_uia() {
    let mut held = confirming_after_the_watch();
    let skipped = step(
        &mut held,
        31_000,
        answer(
            OUTPOST,
            5,
            Reply::Candidate(CandidateAnswer {
                facts: CandidateFacts::Skipped,
                read: None,
            }),
        ),
    );
    assert_eq!(
        skipped.text,
        [(6, 2, watch(CaretMotion::NextCharacter, 5_000, true))]
    );
    let ended = step(&mut held, 40_000, moved_onto_i(6));
    assert_eq!(ended.said, [character("i")]);
    assert_eq!(ended.runs, []);
}

/// The key's starting point stays held while its confirmation is out,
/// though the confirmation reads nothing of it.
#[test]
fn the_starting_point_is_held_while_a_confirmation_is_out() {
    let held = confirming_after_the_watch();
    let anchors = held.state.held_anchors();
    assert_eq!(
        anchors
            .get(&OutpostId(crate::entries::tests::GENERATION))
            .map(|anchors| anchors.iter().copied().collect::<Vec<_>>()),
        Some(vec![TextAnchor(50)])
    );
}

/// The foreground leaving the confirmation's window stops it: the key is
/// silent.
#[test]
fn the_foreground_leaving_the_candidates_window_stops_the_confirmation() {
    let mut held = confirming_after_the_watch();
    let elsewhere = step(
        &mut held,
        31_000,
        Input::Foreground(Foreground {
            window: WindowHandle(70),
            root_owner: WindowHandle(70),
            process: verbatim_model::Pid(41),
            thread: 0,
            by_class: None,
            shell: false,
        }),
    );
    assert_eq!(elsewhere.cancelled, [(5, CancelReason::Foreground)]);
    assert_eq!(elsewhere.said, NO_WORDS);
    assert_eq!(elsewhere.text, NO_REQUESTS);
}

/// A menu start behind the key is confirmed for its role: a menu popup
/// counts and silences the key; anything else lets it watch on.
#[test]
fn a_menu_start_behind_the_key_is_confirmed_for_its_role() {
    for (role, counts) in [(Role::Menu, true), (Role::Window, false)] {
        let mut held = watching(
            CaretMotion::NextCharacter,
            line("line 01", 7, 0),
            unit(TextUnit::Character, "l", 8),
        );
        let mut tell = msaa_tell(-4);
        tell.kind = verbatim_model::event_kind::EventKind::PopupStart;
        tell.at_us = 29_000;
        tell.trace_id = TraceId::from_raw(100);
        let arrived = step(&mut held, 30_000, entry(tell));
        assert_eq!(arrived.cancelled, [(4, CancelReason::Withdrawn)]);
        assert_eq!(arrived.runs, [confirming(5, -4)]);
        let answered = step(
            &mut held,
            31_000,
            answer(
                OUTPOST,
                5,
                Reply::Candidate(CandidateAnswer {
                    facts: CandidateFacts::Found {
                        focused: None,
                        role: Some(role),
                        window_class: "#32768".to_owned(),
                        redirect: None,
                    },
                    read: None,
                }),
            ),
        );
        assert_eq!(answered.said, NO_WORDS, "{role:?}");
        if counts {
            assert_eq!(answered.text, NO_REQUESTS, "{role:?}");
        } else {
            assert_eq!(
                answered.text,
                [(6, 2, watch(CaretMotion::NextCharacter, 5_000, true))],
                "{role:?}"
            );
        }
    }
}

// Disagreements 1, 3 and 4 of the design, hand-written cross-process
// sequences, NVDA's speech from source: a stray event inside a 100 ms watch
// has no trigger a capture can produce on demand.

/// Disagreement 1, from source (`IAccessibleHandler/__init__.py` 845 to
/// 854): a foreground event on the field's own frame while the watch is
/// out is refused for pending too, so the key speaks.
#[test]
fn a_foreground_on_the_fields_own_frame_does_not_silence_the_key() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    // The field's frame, window 4, is its ancestor's window, as a real
    // field's walk reads it.
    if let Some(last) = held.state.versions.last.as_mut() {
        last.place.ancestor_windows = vec![WindowHandle(4)].into();
    }
    let mut tell = msaa_tell(0);
    tell.kind = verbatim_model::event_kind::EventKind::Foreground;
    tell.object = verbatim_model::tell::EventObject::Msaa {
        window: WindowHandle(4),
        object: 0,
        child: 0,
    };
    tell.at_us = 29_000;
    if let verbatim_model::tell::TellWindow::Found(facts) = &mut tell.window {
        facts.window = facts.root.clone();
        facts.window_to_foreground = verbatim_model::tell::Relation::Equal;
    }
    let arrived = step(&mut held, 30_000, entry(tell));
    assert_eq!(arrived, Step::default(), "the watch goes on");
    let ended = step(&mut held, 40_000, moved_onto_i(4));
    assert_eq!(ended.said, [character("i")]);
}

/// Disagreement 3, Verbatim-only: the hidden frame's own MSAA focus behind
/// the key is refused with no read, and the key speaks.
#[test]
fn the_hidden_frames_own_focus_does_not_silence_the_key() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let mut tell = msaa_tell(-4);
    tell.at_us = 29_000;
    if let verbatim_model::tell::TellWindow::Found(facts) = &mut tell.window {
        facts.hidden_frame = true;
    }
    let arrived = step(&mut held, 30_000, entry(tell));
    assert_eq!(arrived, Step::default());
    let ended = step(&mut held, 40_000, moved_onto_i(4));
    assert_eq!(ended.said, [character("i")]);
}

/// Disagreement 4, from source (`UIAHandler/__init__.py` 809 to 818, 853 to
/// 868): a UI Automation menu opening on the committed element counts as
/// pending, executing it again: the key is silent.
#[test]
fn a_menu_opening_on_the_committed_element_silences_the_key() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let mut tell = uia_tell(1, Vec::new());
    tell.kind = verbatim_model::event_kind::EventKind::UiaMenuOpened;
    tell.at_us = 29_000;
    let arrived = step(&mut held, 30_000, entry(tell));
    assert_eq!(arrived.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(arrived.text, NO_REQUESTS);
}
