//! The caret key as an event-queue entry (Table 7) and the stroke line
//! it sends from (Table 13): the starting read at its turn, the pass, the
//! watch and the key's words; a later key, a pending focus, the 150 ms
//! release, the foreground and the sentence move. Ported from the key
//! line's tests where their speech holds (`tests/text.rs`), with the
//! sequences "What users hear" changes.

use verbatim_model::request::{
    CancelReason, CaretBinding, Outcome as WireOutcome, Press, PressAction, QueueEffect,
    QueueInput, Reply, Request as Wire,
};
use verbatim_model::stroke::{HeldStroke, SentTransition, Stroke};
use verbatim_model::{
    CaretKey, CaretMotion, CaretReport, Effect as Said, KeySeq, OutpostId, Role, SentenceMove,
    SentenceMoved, Started, StartingRead, TextAnchor, TextChunk, TextDetails, TextOp, TextProvider,
    TextReply, TextUnit, TraceId, Watch, WatchFinalRead, Watched, WatchedRead, WindowHandle,
};

use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, entry, feed_at, focus_read, node, ready, uia_tell,
    values, win,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Effect, Foreground, Input, Timed};
use crate::keys::caret_key;
use crate::keys::entry::PressEntry;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
};

/// The startup read answered with the edit field "Body" committed, node 2
/// in window 5, read through UI Automation; its `Follow` is request 2, so
/// the next request is 3.
fn editing() -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let mut read = focus_read(None);
    read.focus.read.values = values("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        600,
        answer(OUTPOST, 1, Reply::Focus(Box::new(read))),
    );
    state
}

/// The committed field's record, as the hook's view publishes it.
fn bound(held: &Held) -> CaretBinding {
    caret_key::binding(&held.state).expect("the committed field has text")
}

/// The same field read through an edit control's messages, whose text has
/// a sentence unit.
fn bound_to_edit(held: &Held) -> CaretBinding {
    CaretBinding {
        provider: TextProvider::EditMessages,
        ..bound(held)
    }
}

/// The stroke the hook held for key `key`.
fn stroke(key: u64) -> HeldStroke {
    HeldStroke {
        stroke: Stroke {
            vk: 0x27,
            scan_code: 0x4D,
            extended: true,
            sequence: u32::try_from(key).unwrap(),
        },
        modifiers: Vec::new(),
    }
}

fn caret(motion: CaretMotion) -> CaretKey {
    CaretKey {
        motion,
        select: false,
    }
}

/// Key `key`, `motion`, pressed at `at_us`, held by the hook and bound to
/// `binding`.
fn held_key(key: u64, motion: CaretMotion, at_us: u64, binding: &CaretBinding) -> Input<Entry> {
    pressed(
        key,
        at_us,
        PressAction::CaretKey {
            key: caret(motion),
            held: Some(stroke(key)),
            bound: Some(Box::new(binding.clone())),
        },
    )
}

/// Key `key` pressed at `at_us` with `action`.
fn pressed(key: u64, at_us: u64, action: PressAction) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: at_us,
            action,
        },
    ))))
}

/// What one step did, as these tests read it.
#[derive(Debug, Default, PartialEq)]
struct Step {
    /// Each text request sent: its number, its node's number and what it
    /// asks.
    text: Vec<(u64, u64, TextOp)>,
    /// Each other request sent, by its kind.
    other: Vec<&'static str>,
    /// Each utterance, by its segments.
    said: Vec<Vec<String>>,
    /// Each send of strokes, by the sequence of each transition.
    strokes: Vec<Vec<u32>>,
    /// Each cancel: its request's number and its reason.
    cancelled: Vec<(u64, CancelReason)>,
}

/// Feeds `input` at `at_us` and reads the step.
fn step(held: &mut Held, at_us: u64, input: Input<Entry>) -> Step {
    let effects = feed(&mut held.state, &mut held.runtime, Timed { at_us, input }).unwrap();
    let mut step = Step::default();
    for effect in effects {
        read_effect(&mut step, effect);
    }
    step
}

fn read_effect(step: &mut Step, effect: Effect<Said>) {
    match effect {
        Effect::Send { ask, .. } => match ask.request {
            Wire::Text { node, op } => step.text.push((ask.number.0, node.number(), op)),
            other => step.other.push(match other {
                Wire::ReadObject(_) => "ReadObject",
                Wire::Follow(_) => "Follow",
                _ => "other",
            }),
        },
        Effect::Cancel { cancel, .. } => step.cancelled.push((cancel.number.0, cancel.reason)),
        Effect::Said(Said::Speak(utterance, _)) => step.said.push(
            utterance
                .segments
                .iter()
                .map(|segment| format!("{:?}", segment.content))
                .collect(),
        ),
        Effect::Said(Said::Queue(QueueEffect::SendStrokes(sent))) => step.strokes.push(
            sent.iter()
                .map(|transition| match transition {
                    SentTransition::Held(held) => held.stroke.sequence,
                    SentTransition::Deferred { stroke, .. } => stroke.sequence,
                })
                .collect(),
        ),
        Effect::AnswerDropped { .. } => {}
        other => panic!("an effect these tests read: {other:?}"),
    }
}

/// The line `text` starting at anchor `anchor`, read with the caret at byte
/// `offset`.
fn line(text: &str, anchor: u64, offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: text.to_owned(),
        start: TextAnchor(anchor),
        offset,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

/// `text` read as `unit` at anchor `anchor`.
fn unit(unit: TextUnit, text: &str, anchor: u64) -> TextChunk {
    TextChunk {
        unit,
        ..line(text, anchor, 0)
    }
}

fn caret_on(line: TextChunk) -> CaretReport {
    CaretReport {
        line,
        selection: None,
        read_at_us: 0,
    }
}

/// The starting read's answer: point 50, the caret on `line`, `unit` at it.
fn started(number: u64, line: TextChunk, unit: TextChunk) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        Reply::Text(TextReply::Started(Box::new(Started {
            point: TextAnchor(50),
            caret: caret_on(line),
            unit,
            word: None,
        }))),
    )
}

fn watched(number: u64, watched: Watched) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        Reply::Text(TextReply::Watched(Box::new(watched))),
    )
}

fn read(line: TextChunk, unit: TextChunk) -> WatchedRead {
    WatchedRead {
        caret: caret_on(line),
        unit,
    }
}

fn starting_read(motion: CaretMotion, provider: TextProvider) -> TextOp {
    TextOp::StartingRead(StartingRead {
        provider,
        details: TextDetails::ALL,
        unit: motion.unit(),
        word: matches!(motion, CaretMotion::Delete | CaretMotion::DeleteWord),
    })
}

fn watch(motion: CaretMotion, passed_at_us: u64, events: bool) -> TextOp {
    TextOp::Watch(Watch {
        provider: TextProvider::Uia,
        point: TextAnchor(50),
        motion,
        passed_at_us,
        poll_us: 10_000,
        events,
        bound_us: 100_000,
        final_read: WatchFinalRead::Unit {
            unit: motion.unit(),
            details: TextDetails::ALL,
        },
    })
}

/// `motion` pressed as key 1 at 1 ms on the field, its starting read
/// (request 3) answered at 5 ms with `line` and `unit`: the key is passed
/// and its watch is request 4.
fn watching(motion: CaretMotion, line: TextChunk, unit: TextChunk) -> Held {
    let mut held = editing();
    let binding = bound(&held);
    let asked = step(&mut held, 1_000, held_key(1, motion, 1_000, &binding));
    assert_eq!(
        asked.text,
        [(3, 2, starting_read(motion, TextProvider::Uia))]
    );
    assert_eq!(asked.strokes, NO_STROKES, "held until its starting read");
    let passed = step(&mut held, 5_000, started(3, line, unit));
    assert_eq!(passed.strokes, [vec![1]]);
    assert_eq!(passed.text, [(4, 2, watch(motion, 5_000, true))]);
    assert_eq!(passed.said, NO_WORDS);
    held
}

fn character(text: &str) -> Vec<String> {
    vec![format!("Character({text:?})")]
}

fn spoken(text: &str) -> Vec<String> {
    vec![format!("Text({text:?})")]
}

const BLANK: &str = "Message(Blank)";

/// No utterance.
const NO_WORDS: [Vec<String>; 0] = [];
/// No text request.
const NO_REQUESTS: [(u64, u64, TextOp); 0] = [];
/// No send of strokes.
const NO_STROKES: [Vec<u32>; 0] = [];

#[test]
fn an_arrow_key_reads_its_start_is_passed_watched_and_says_the_character_at_the_caret() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let ended = step(
        &mut held,
        20_000,
        watched(
            4,
            Watched::Effect(read(
                line("line 01", 9, 1),
                unit(TextUnit::Character, "i", 10),
            )),
        ),
    );
    assert_eq!(ended.said, [character("i")]);
    assert_eq!(ended.text, NO_REQUESTS);
    assert_eq!(ended.strokes, NO_STROKES);
    assert_eq!(
        held.state.focus_caret(node(2)).unwrap().line.offset,
        1,
        "the final read is Core's caret"
    );
}

#[test]
fn line_word_and_paragraph_keys_say_their_unit() {
    for (motion, said_unit, said) in [
        (
            CaretMotion::NextLine,
            unit(TextUnit::Line, "line 02", 11),
            spoken("line 02"),
        ),
        (
            CaretMotion::NextWord,
            unit(TextUnit::Word, "two ", 11),
            spoken("two"),
        ),
        (
            CaretMotion::NextParagraph,
            unit(TextUnit::Paragraph, "Second paragraph.", 11),
            spoken("Second paragraph."),
        ),
        (
            CaretMotion::NextWord,
            unit(TextUnit::Word, "a ", 11),
            character("a"),
        ),
    ] {
        let mut held = watching(
            motion,
            line("one two", 7, 0),
            unit(TextUnit::Word, "one ", 8),
        );
        let after = if motion == CaretMotion::NextLine {
            line("line 02", 11, 0)
        } else {
            line("one two", 11, 4)
        };
        let ended = step(
            &mut held,
            20_000,
            watched(4, Watched::Effect(read(after, said_unit))),
        );
        assert_eq!(ended.said, [said], "{motion:?}");
    }
}

#[test]
fn a_key_that_moves_nothing_says_its_unit_at_its_bound_and_backspace_nothing() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("end", 7, 3),
        unit(TextUnit::Character, "", 8),
    );
    let ended = step(
        &mut held,
        105_000,
        watched(
            4,
            Watched::Bound(read(line("end", 9, 3), unit(TextUnit::Character, "", 10))),
        ),
    );
    assert_eq!(ended.said, [vec![BLANK.to_owned()]]);

    let mut held = watching(
        CaretMotion::Backspace,
        line("x", 7, 0),
        unit(TextUnit::Character, "x", 8),
    );
    let ended = step(
        &mut held,
        105_000,
        watched(
            4,
            Watched::Bound(read(line("x", 9, 0), unit(TextUnit::Character, "x", 10))),
        ),
    );
    assert_eq!(ended, Step::default());
}

#[test]
fn backspace_says_what_it_deleted_from_its_starting_read() {
    for (motion, text, offset, said) in [
        (CaretMotion::Backspace, "abc", 3, character("c")),
        (CaretMotion::BackspaceWord, "one two", 7, spoken("two")),
    ] {
        let mut held = watching(motion, line(text, 7, offset), unit(motion.unit(), "", 8));
        let ended = step(
            &mut held,
            20_000,
            watched(
                4,
                Watched::Effect(read(line("ab", 9, 2), unit(TextUnit::Character, "", 10))),
            ),
        );
        assert_eq!(ended.said, [said], "{motion:?}");
    }
}

/// Windows 11 Notepad's text breaks lines with a carriage return, which
/// Backspace at a line's start deletes, with or without Control; a
/// standard edit control's carriage return and line feed are spoken as the
/// line feed.
#[test]
fn backspace_at_a_lines_start_names_the_line_break_it_deleted() {
    for (motion, text, said) in [
        (CaretMotion::Backspace, "def\r", character("\r")),
        (CaretMotion::BackspaceWord, "def\r", character("\r")),
        (CaretMotion::Backspace, "def\r\n", character("\n")),
    ] {
        let mut held = watching(motion, line(text, 7, 0), unit(motion.unit(), "d", 8));
        let ended = step(
            &mut held,
            20_000,
            watched(
                4,
                Watched::Effect(read(
                    line("abcdef\r", 9, 3),
                    unit(TextUnit::Character, "d", 10),
                )),
            ),
        );
        assert_eq!(ended.said, [said], "{motion:?} {text:?}");
    }
}

#[test]
fn delete_reads_the_word_at_its_start_and_says_the_character_now_at_the_caret() {
    let mut held = watching(
        CaretMotion::Delete,
        line("abc", 7, 0),
        unit(TextUnit::Character, "a", 8),
    );
    let ended = step(
        &mut held,
        40_000,
        watched(
            4,
            Watched::Effect(read(line("bc", 9, 0), unit(TextUnit::Character, "b", 10))),
        ),
    );
    assert_eq!(ended.said, [character("b")]);
}

#[test]
fn a_later_key_withdraws_the_watch_and_the_key_says_nothing() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let binding = bound(&held);
    let later = step(
        &mut held,
        30_000,
        held_key(2, CaretMotion::NextCharacter, 30_000, &binding),
    );
    assert_eq!(later.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(later.said, NO_WORDS);
    // The next key's starting read is taken at its turn, with no wait.
    assert_eq!(
        later.text,
        [(
            5,
            2,
            starting_read(CaretMotion::NextCharacter, TextProvider::Uia)
        )]
    );
    // The watch's answer, already on its way, is dropped by number.
    let late = step(
        &mut held,
        31_000,
        watched(
            4,
            Watched::Effect(read(
                line("line 01", 9, 1),
                unit(TextUnit::Character, "i", 10),
            )),
        ),
    );
    assert_eq!(late, Step::default());
}

#[test]
fn a_later_key_lets_the_starting_read_finish_and_the_key_is_passed_silently() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let later = step(
        &mut held,
        3_000,
        held_key(2, CaretMotion::PreviousCharacter, 3_000, &binding),
    );
    assert_eq!(later, Step::default(), "the starting read is not withdrawn");
    let answered = step(
        &mut held,
        5_000,
        started(3, line("line 01", 7, 0), unit(TextUnit::Character, "l", 8)),
    );
    assert_eq!(answered.strokes, [vec![1]]);
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(
        answered.text,
        [(
            4,
            2,
            starting_read(CaretMotion::PreviousCharacter, TextProvider::Uia)
        )]
    );
}

#[test]
fn a_marked_key_at_its_turn_is_sent_with_no_read() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let _ = step(
        &mut held,
        2_000,
        held_key(2, CaretMotion::NextCharacter, 2_000, &binding),
    );
    let _ = step(
        &mut held,
        3_000,
        held_key(3, CaretMotion::NextCharacter, 3_000, &binding),
    );
    let answered = step(
        &mut held,
        5_000,
        started(3, line("line 01", 7, 0), unit(TextUnit::Character, "l", 8)),
    );
    assert_eq!(answered.strokes, [vec![1], vec![2]], "key 2 is sent unread");
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(
        answered.text,
        [(
            4,
            2,
            starting_read(CaretMotion::NextCharacter, TextProvider::Uia)
        )]
    );
}

#[test]
fn a_focus_pending_withdraws_the_watch_and_the_key_says_nothing() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let focus = step(&mut held, 30_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(focus.said, NO_WORDS);
    assert_eq!(focus.other, ["ReadObject"], "the focus is taken");
}

/// Ruled 2026-11-16. Needs capture: written to the design's derived
/// speech ("What users hear"), awaiting NVDA's capture.
#[test]
fn backspace_with_a_focus_pending_says_its_deleted_text_then_the_focus_is_taken() {
    let mut held = watching(
        CaretMotion::Backspace,
        line("x", 7, 1),
        unit(TextUnit::Character, "", 8),
    );
    let focus = step(&mut held, 40_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(focus.said, [character("x")]);
    assert_eq!(focus.other, ["ReadObject"]);
}

/// Ruled 2026-11-16. Needs capture: written to the design's derived
/// speech, awaiting NVDA's capture. An empty message box, the caret at
/// its start: "blank", then the focus.
#[test]
fn backspace_with_a_focus_pending_at_its_starting_read_reads_then_says_its_deleted_text() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::Backspace, 1_000, &binding),
    );
    let focus = step(&mut held, 2_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus, Step::default(), "Backspace's starting read goes on");
    let empty = TextChunk {
        first: true,
        ..line("", 7, 0)
    };
    let answered = step(
        &mut held,
        5_000,
        started(3, empty, unit(TextUnit::Character, "", 8)),
    );
    assert_eq!(answered.strokes, [vec![1]]);
    assert_eq!(answered.said, [vec![BLANK.to_owned()]]);
    assert_eq!(answered.text, NO_REQUESTS, "no watch");
    assert_eq!(answered.other, ["ReadObject"]);
}

#[test]
fn a_focus_pending_withdraws_another_keys_starting_read_and_the_key_is_sent_silently() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let focus = step(&mut held, 2_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus.cancelled, [(3, CancelReason::Withdrawn)]);
    assert_eq!(focus.strokes, [vec![1]]);
    assert_eq!(focus.said, NO_WORDS);
    assert_eq!(focus.other, ["ReadObject"]);
}

#[test]
fn a_focus_pending_at_a_keys_turn_sends_it_with_no_read() {
    let mut held = editing();
    let binding = bound(&held);
    // Key 1 holds the front while key 2 and a focus wait behind it.
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::Backspace, 1_000, &binding),
    );
    let _ = step(
        &mut held,
        2_000,
        held_key(2, CaretMotion::NextCharacter, 2_000, &binding),
    );
    let _ = step(&mut held, 3_000, entry(uia_tell(7, Vec::new())));
    let answered = step(
        &mut held,
        5_000,
        started(3, line("ab", 7, 2), unit(TextUnit::Character, "", 8)),
    );
    // Key 1 is marked by key 2: silent. Key 2 is marked by nothing but a
    // focus is pending: sent with no read.
    assert_eq!(answered.strokes, [vec![1], vec![2]]);
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(answered.text, NO_REQUESTS);
    assert_eq!(answered.other, ["ReadObject"]);
}

#[test]
fn a_held_key_is_released_150_ms_after_its_press_its_read_withdrawn_and_it_says_nothing() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let _ = step(
        &mut held,
        2_000,
        held_key(2, CaretMotion::NextCharacter, 2_000, &binding),
    );
    assert_eq!(held.state.strokes.next_release(), Some(151_000));
    let released = step(&mut held, 152_000, Input::Tick);
    assert_eq!(released.strokes, [vec![1, 2]]);
    assert_eq!(released.cancelled, [(3, CancelReason::Withdrawn)]);
    // Key 2, released before its turn, says nothing and asks nothing.
    assert_eq!(released.said, NO_WORDS);
    assert_eq!(released.text, NO_REQUESTS);
    assert_eq!(held.state.queue.waiting_for_tests(), 0);
}

#[test]
fn an_unheld_key_says_nothing_asks_nothing_and_forgets_the_typed_word() {
    let mut held = editing();
    let binding = bound(&held);
    held.state.typed_word = "wor".to_owned();
    let unheld = step(
        &mut held,
        1_000,
        pressed(
            1,
            1_000,
            PressAction::CaretKey {
                key: caret(CaretMotion::NextCharacter),
                held: None,
                bound: Some(Box::new(binding)),
            },
        ),
    );
    assert_eq!(unheld, Step::default());
    assert_eq!(held.state.typed_word, "");
}

#[test]
fn a_key_whose_node_is_no_longer_held_or_whose_read_fails_is_sent_at_once_silently() {
    let mut held = editing();
    let dead = CaretBinding {
        node: verbatim_model::NodeId::in_outpost(OutpostId(2), 2),
        ..bound(&held)
    };
    let sent = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &dead),
    );
    assert_eq!(sent.strokes, [vec![1]]);
    assert_eq!(sent.text, NO_REQUESTS);
    assert_eq!(sent.said, NO_WORDS);

    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let failed = step(
        &mut held,
        5_000,
        answer_outcome(OUTPOST, 3, WireOutcome::NotThere),
    );
    assert_eq!(failed.strokes, [vec![1]]);
    assert_eq!(failed.text, NO_REQUESTS);
    assert_eq!(failed.said, NO_WORDS);
}

#[test]
fn a_waiting_key_whose_window_leaves_the_foreground_ends_and_is_sent_in_press_order() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let _ = step(
        &mut held,
        2_000,
        held_key(2, CaretMotion::NextCharacter, 2_000, &binding),
    );
    let elsewhere = Foreground {
        window: WindowHandle(90),
        root_owner: WindowHandle(90),
        ..IN_FRONT
    };
    let moved = step(&mut held, 3_000, Input::Foreground(elsewhere));
    assert_eq!(moved.cancelled, [(3, CancelReason::Foreground)]);
    assert_eq!(moved.strokes, [vec![1], vec![2]]);
    assert_eq!(moved.said, NO_WORDS);
    assert_eq!(moved.text, NO_REQUESTS);
}

/// Ruled 2026-11-16. Needs capture: written to the design's derived
/// speech, awaiting NVDA's capture. The key runs on the field it was
/// bound to at its press, which is no longer the committed focus, so its
/// watch names no event shortcut.
#[test]
fn a_key_whose_field_is_no_longer_the_focus_runs_on_that_field() {
    let mut held = editing();
    let earlier = CaretBinding {
        node: node(6),
        window: win(6),
        ..bound(&held)
    };
    let asked = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::EndOfLine, 1_000, &earlier),
    );
    assert_eq!(
        asked.text,
        [(
            3,
            6,
            starting_read(CaretMotion::EndOfLine, TextProvider::Uia)
        )]
    );
    let passed = step(
        &mut held,
        5_000,
        started(3, line("abc", 7, 0), unit(TextUnit::Character, "a", 8)),
    );
    assert_eq!(
        passed.text,
        [(4, 6, watch(CaretMotion::EndOfLine, 5_000, false))]
    );
    let ended = step(
        &mut held,
        105_000,
        watched(
            4,
            Watched::Bound(read(line("abc", 9, 3), unit(TextUnit::Character, "", 10))),
        ),
    );
    assert_eq!(ended.said, [vec![BLANK.to_owned()]]);
}

/// Alt+Down in text with a sentence unit: its watch reads only at its
/// effect, and at its bound Core moves by sentence from its starting
/// point.
fn sentence_watching() -> (Held, CaretBinding) {
    let mut held = editing();
    let binding = bound_to_edit(&held);
    let asked = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextSentence, 1_000, &binding),
    );
    assert_eq!(
        asked.text,
        [(
            3,
            2,
            starting_read(CaretMotion::NextSentence, TextProvider::EditMessages)
        )]
    );
    let passed = step(
        &mut held,
        5_000,
        started(
            3,
            line("One. Two.\r\n", 7, 0),
            unit(TextUnit::Line, "One. Two.\r\n", 8),
        ),
    );
    assert_eq!(
        passed.text,
        [(
            4,
            2,
            TextOp::Watch(Watch {
                provider: TextProvider::EditMessages,
                point: TextAnchor(50),
                motion: CaretMotion::NextSentence,
                passed_at_us: 5_000,
                poll_us: 10_000,
                events: false,
                bound_us: 100_000,
                final_read: WatchFinalRead::UnitAtEffect {
                    unit: TextUnit::Line,
                    details: TextDetails::ALL,
                },
            })
        )]
    );
    (held, binding)
}

fn sentence_move() -> TextOp {
    TextOp::MoveBySentence(SentenceMove {
        provider: TextProvider::EditMessages,
        point: TextAnchor(50),
        forward: true,
        details: TextDetails::ALL,
    })
}

fn moved(number: u64, moved: SentenceMoved) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        Reply::Text(TextReply::Sentence(Box::new(moved))),
    )
}

#[test]
fn alt_down_with_no_effect_moves_by_sentence_and_says_it() {
    let (mut held, _) = sentence_watching();
    let bound = step(&mut held, 105_000, watched(4, Watched::BoundUnread));
    assert_eq!(bound.text, [(5, 2, sentence_move())]);
    let said = step(
        &mut held,
        106_000,
        moved(
            5,
            SentenceMoved::Moved {
                sentence: "Two.".to_owned(),
                line: line("One. Two.\r\n", 12, 5),
                baseline: TextAnchor(13),
            },
        ),
    );
    assert_eq!(said.said, [spoken("Two.")]);
}

#[test]
fn alt_down_at_the_texts_end_says_blank_and_at_its_start_the_first_sentence() {
    let (mut held, _) = sentence_watching();
    let _ = step(&mut held, 105_000, watched(4, Watched::BoundUnread));
    let end = step(
        &mut held,
        106_000,
        moved(
            5,
            SentenceMoved::End {
                line: line("Three.", 12, 6),
                baseline: TextAnchor(13),
            },
        ),
    );
    assert_eq!(end.said, [vec![BLANK.to_owned()]]);
    // Core's caret is at the text's end, and the move minted the baseline.
    let caret = held
        .state
        .focus_caret(node(2))
        .expect("the caret is Core's");
    assert_eq!((caret.line.text.as_str(), caret.line.offset), ("Three.", 6));
    assert_eq!(
        crate::text_facts::held(&held.state),
        Some((node(2), TextAnchor(13)))
    );

    let (mut held, _) = sentence_watching();
    let _ = step(&mut held, 105_000, watched(4, Watched::BoundUnread));
    let start = step(
        &mut held,
        106_000,
        moved(
            5,
            SentenceMoved::StayedAtStart {
                sentence: "One. ".to_owned(),
                line: line("One. Two.\r\n", 12, 0),
                baseline: TextAnchor(13),
            },
        ),
    );
    assert_eq!(start.said, [spoken("One.")]);
}

#[test]
fn a_sentence_keys_effect_says_the_line() {
    let (mut held, _) = sentence_watching();
    let ended = step(
        &mut held,
        20_000,
        watched(
            4,
            Watched::Effect(read(
                line("Three.", 12, 0),
                unit(TextUnit::Line, "Three.", 13),
            )),
        ),
    );
    assert_eq!(ended.said, [spoken("Three.")]);
    assert_eq!(ended.text, NO_REQUESTS);
}

/// Needs capture: written to the design's derived speech, awaiting NVDA's
/// capture. Alt+Down, then Down before its effect is seen: the caret moves
/// to "Two." silently, and Down waits behind the move.
#[test]
fn a_later_key_during_a_sentence_keys_watch_still_moves_it_silently() {
    let (mut held, binding) = sentence_watching();
    let later = step(
        &mut held,
        40_000,
        held_key(2, CaretMotion::NextLine, 40_000, &binding),
    );
    assert_eq!(later.cancelled, [(4, CancelReason::Withdrawn)]);
    assert_eq!(later.text, [(5, 2, sentence_move())]);
    let answered = step(
        &mut held,
        45_000,
        moved(
            5,
            SentenceMoved::Moved {
                sentence: "Two.".to_owned(),
                line: line("One. Two.\r\n", 12, 5),
                baseline: TextAnchor(13),
            },
        ),
    );
    assert_eq!(answered.said, NO_WORDS);
    assert_eq!(
        answered.text,
        [(
            6,
            2,
            starting_read(CaretMotion::NextLine, TextProvider::EditMessages)
        )]
    );
}

#[test]
fn a_focus_pending_drops_a_sentence_move() {
    let (mut held, _) = sentence_watching();
    let _ = step(&mut held, 105_000, watched(4, Watched::BoundUnread));
    let focus = step(&mut held, 106_000, entry(uia_tell(7, Vec::new())));
    assert_eq!(focus.cancelled, [(5, CancelReason::Withdrawn)]);
    assert_eq!(focus.said, NO_WORDS);
    assert_eq!(focus.other, ["ReadObject"]);
}

/// Routes a queue input from the shell at `at_us`.
fn route(held: &mut Held, at_us: u64, input: &QueueInput) -> Vec<Said> {
    crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), at_us),
        input,
    )
    .said()
    .into_iter()
    .filter(|said| !matches!(said, Said::Queue(QueueEffect::WakeAt(_))))
    .collect()
}

fn deferred(sequence: u32, pressed: bool) -> QueueInput {
    QueueInput::Deferred {
        stroke: Stroke {
            vk: 0x58,
            scan_code: 0x2D,
            extended: false,
            sequence,
        },
        pressed,
    }
}

#[test]
fn a_deferred_transition_goes_with_the_held_key_ahead_of_it_and_at_once_with_none() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    assert_eq!(route(&mut held, 2_000, &deferred(2, true)), []);
    let passed = step(
        &mut held,
        5_000,
        started(3, line("line 01", 7, 0), unit(TextUnit::Character, "l", 8)),
    );
    assert_eq!(passed.strokes, [vec![1, 2]]);
    let alone = route(&mut held, 6_000, &deferred(3, false));
    assert!(matches!(
        alone.as_slice(),
        [Said::Queue(QueueEffect::SendStrokes(sent))] if sent.len() == 1
    ));
}

#[test]
fn a_reset_of_the_hooks_line_drops_the_keys_stroke_and_withdraws_its_read() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let reset = route(&mut held, 2_000, &QueueInput::StrokesReset);
    assert!(
        matches!(
            reset.as_slice(),
            [Said::Queue(QueueEffect::Cancel { cancel, .. })]
                if cancel.reason == CancelReason::Withdrawn
        ),
        "{reset:?}"
    );
    assert_eq!(held.state.queue.front_for_tests(), None);
}

#[test]
fn at_exit_core_names_the_held_strokes_for_the_shell_to_release() {
    let mut held = editing();
    let binding = bound(&held);
    let _ = step(
        &mut held,
        1_000,
        held_key(1, CaretMotion::NextCharacter, 1_000, &binding),
    );
    let exit = route(&mut held, 2_000, &QueueInput::Exit);
    assert_eq!(
        exit,
        [Said::Queue(QueueEffect::ReleaseAtExit(vec![
            stroke(1).stroke
        ]))]
    );
}

#[test]
fn a_caret_key_binds_to_a_committed_field_and_not_to_a_button() {
    let held = editing();
    assert_eq!(
        bound(&held),
        CaretBinding {
            node: node(2),
            window: win(4),
            role: Role::EditableText,
            provider: TextProvider::Uia,
            record: Some(TraceId::from_raw(0)),
            formatting: verbatim_model::TextAttributes::default(),
        }
    );
    let mut button = ready();
    let _ = feed_at(&mut button, 500, Input::Foreground(IN_FRONT));
    let _ = feed_at(
        &mut button,
        600,
        answer(OUTPOST, 1, Reply::Focus(Box::new(focus_read(None)))),
    );
    assert_eq!(caret_key::binding(&button.state), None);
}

/// The starting point is an anchor the outpost holds for Core: while the
/// watch naming it is out, `NodesHeld` names it (design, "Held anchors").
/// The key's reads compare no selection and leave the baseline alone.
#[test]
fn the_starting_point_is_held_while_its_watch_is_out_and_no_baseline_is_minted() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let anchors = held.state.held_anchors();
    assert_eq!(
        anchors
            .get(&OutpostId(crate::entries::tests::GENERATION))
            .map(|anchors| anchors.iter().copied().collect::<Vec<_>>()),
        Some(vec![TextAnchor(50)])
    );
    let _ = step(
        &mut held,
        20_000,
        watched(
            4,
            Watched::Effect(read(
                line("line 01", 7, 1),
                unit(TextUnit::Character, "i", 8),
            )),
        ),
    );
    assert_eq!(crate::text_facts::held(&held.state), None);
}

/// Italic text, as a key's unit carries it and a memory keeps it.
fn italic() -> verbatim_model::TextAttributes {
    verbatim_model::TextAttributes {
        italic: Some(true),
        ..verbatim_model::TextAttributes::default()
    }
}

/// The character "i" in italic.
fn italic_i() -> TextChunk {
    TextChunk {
        formats: vec![verbatim_model::FormatRun {
            start: 0,
            end: 1,
            attributes: italic(),
        }],
        ..unit(TextUnit::Character, "i", 10)
    }
}

/// The formatting memory by value (build 2's design, "The formatting
/// memory", after ADR 0001's decision that it goes with the focus object):
/// a key's binding carries its focus record's memory from the press; the
/// key compares its unit's formatting with it, so "italic" is said, and
/// writes what it spoke back to the record, still the committed focus.
#[test]
fn a_keys_formatting_memory_goes_back_to_its_record() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    let binding = bound(&held);
    assert_eq!(
        binding.formatting,
        verbatim_model::TextAttributes::default()
    );
    let ended = step(
        &mut held,
        20_000,
        watched(4, Watched::Effect(read(line("line 01", 9, 1), italic_i()))),
    );
    assert_eq!(
        ended.said,
        [vec![
            "Format(Italic)".to_owned(),
            "Character(\"i\")".to_owned()
        ]]
    );
    assert_eq!(held.state.focus.object().unwrap().reported_format, italic());
}

/// A key whose focus record is no longer the committed focus, a later
/// focus taken on the same field being a new record, speaks from its own
/// record's memory and leaves the new record's untouched (ADR 0001, "The
/// formatting last reported goes with the focus object").
#[test]
fn a_keys_formatting_memory_is_not_written_to_a_later_focus_record() {
    let mut held = watching(
        CaretMotion::NextCharacter,
        line("line 01", 7, 0),
        unit(TextUnit::Character, "l", 8),
    );
    held.state.focus.object_mut().unwrap().trace_id = Some(TraceId::from_raw(99));
    let ended = step(
        &mut held,
        20_000,
        watched(4, Watched::Effect(read(line("line 01", 9, 1), italic_i()))),
    );
    assert_eq!(
        ended.said,
        [vec![
            "Format(Italic)".to_owned(),
            "Character(\"i\")".to_owned()
        ]]
    );
    assert_eq!(
        held.state.focus.object().unwrap().reported_format,
        verbatim_model::TextAttributes::default()
    );
}
