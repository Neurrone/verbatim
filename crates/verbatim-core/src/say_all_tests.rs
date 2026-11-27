//! Both say-alls as event-queue entries: the press starts it at its turn,
//! each read is an entry of its own asked of the node's window, each caret
//! move a request of the index mark's entry, its speech carries its
//! starting key's number, and stopping it withdraws its read.
use crate::entries::tests::found_at;
use crate::readers::Found;

use verbatim_model::batch::{Batch, Op, StepRefusal};
use verbatim_model::request::{
    Cancel, CancelReason, Outcome as WireOutcome, Press, PressAction, Question, Reply,
    RequestNumber,
};
use verbatim_model::{
    Effect as Said, KeySeq, ReviewCommand, Role, SegmentContent, SpeechMark, TextAnchor, TextChunk,
    TextMovement, TextPoint, TextPosition, TextUnit, TraceId, WindowHandle,
};

use crate::text_batches;

use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready, values,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Foreground, Input, Timed};
use crate::keys::entry::{Fence, PressEntry};
use verbatim_model::Effect;
use verbatim_model::request::QueueEffect;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: Some(APP),
    thread: 41,
    by_class: None,
    ..Foreground::BLANK
};

/// The startup read answered with `name` of `role` committed, node 2 in
/// window 5, the navigator on it; its `Follow` is request 2, so the next
/// request is 3.
fn committed(name: &str, role: Role) -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let mut read = focus_read(None);
    read.focus.read.values = values(name, role);
    let _ = found_at(&mut state, 600, (OUTPOST, 1), Found::Read(read));
    state
}

/// The edit field "Body" committed.
fn editing() -> Held {
    committed("Body", Role::EditableText)
}

/// `command` pressed as key `key`.
fn press(key: u64, command: ReviewCommand) -> Input<Entry> {
    pressed(key, PressAction::Command { command, repeat: 0 })
}

/// Key `key` pressed with `action`.
fn pressed(key: u64, action: PressAction) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action,
        },
    ))))
}

/// A key passed to the application, which stops say-all: a caret key of
/// motion `Other` passed at once, as the app posts it.
fn passed_key(key: u64) -> Input<Entry> {
    pressed(
        key,
        PressAction::CaretKey {
            key: verbatim_model::CaretKey {
                motion: verbatim_model::CaretMotion::Other,
                select: false,
            },
            held: None,
            bound: None,
        },
    )
}

/// Playback reaching `mark`, joining the queue.
fn at_mark(mark: SpeechMark) -> Input<Entry> {
    Input::Entry(Entry::Mark(mark, Fence::default()))
}

/// What one step did, as these tests read it.
#[derive(Debug, Default)]
struct Step {
    /// Each request sent: its number and what it asks of node 2's text.
    sent: Vec<(u64, Batch)>,
    /// Each say-all utterance: its marks, its texts and the key number it
    /// carries.
    said: Vec<Utterance>,
    /// Each cancel told to a reader.
    cancelled: Vec<Cancel>,
    /// Each change to keeping the display on.
    display: Vec<bool>,
    /// Each answer dropped, by its request's number.
    dropped: Vec<u64>,
    /// Each application whose outpost Core starts.
    started: Vec<verbatim_model::Pid>,
}

/// One say-all utterance.
#[derive(Debug)]
struct Utterance {
    marks: Vec<SpeechMark>,
    texts: Vec<String>,
    key: Option<KeySeq>,
}

impl Step {
    /// The one request sent.
    fn one_sent(&self) -> (u64, Batch) {
        assert_eq!(self.sent.len(), 1, "one request: {self:?}");
        self.sent[0].clone()
    }

    /// The texts of every utterance, in order.
    fn texts(&self) -> Vec<Vec<&str>> {
        self.said
            .iter()
            .map(|utterance| utterance.texts.iter().map(String::as_str).collect())
            .collect()
    }
}

/// Feeds `input` at `at_us` and reads the step. Any effect but a text
/// request, say-all speech, a cancel or the display fails the test, so
/// nothing is skipped unchecked.
fn step(held: &mut Held, at_us: u64, input: Input<Entry>) -> Step {
    let effects = feed(&mut held.state, &mut held.runtime, Timed { at_us, input }).unwrap();
    let mut step = Step::default();
    for effect in effects {
        read_effect(&mut step, effect);
    }
    step
}

/// Feeds the reducer's own `input`, outside the queue, and reads the step.
fn reduce(held: &mut Held, input: &verbatim_model::Input) -> Step {
    let effects = crate::reduce(&mut held.state, &mut held.runtime, input);
    let mut step = Step::default();
    for effect in effects {
        match effect {
            Said::Queue(verbatim_model::request::QueueEffect::Send { ask, .. }) => {
                read_effect(
                    &mut step,
                    Effect::Queue(QueueEffect::Send { to: OUTPOST, ask }),
                );
            }
            Said::Queue(verbatim_model::request::QueueEffect::Cancel { cancel, .. }) => {
                step.cancelled.push(cancel);
            }
            Said::Queue(verbatim_model::request::QueueEffect::WakeAt(_)) => {}
            other => read_effect(&mut step, other),
        }
    }
    step
}

fn read_effect(step: &mut Step, effect: Effect) {
    match effect {
        Effect::Queue(QueueEffect::Send { ask, .. }) => match ask.request {
            Question::Batch(batch) => step.sent.push((ask.number.0, batch)),
        },
        Effect::Queue(QueueEffect::Cancel { cancel, .. }) => step.cancelled.push(cancel),
        Effect::Queue(QueueEffect::AnswerDropped { number, .. }) => step.dropped.push(number.0),
        Effect::Queue(QueueEffect::StartOutpost(process)) => step.started.push(process),
        Said::KeepDisplayOn(on) => step.display.push(on),
        Said::Speak(utterance, key) => {
            assert!(utterance.say_all, "{utterance:?} is read by say-all");
            let mut marks = Vec::new();
            let mut texts = Vec::new();
            for segment in &utterance.segments {
                match &segment.content {
                    SegmentContent::Mark(mark) => marks.push(*mark),
                    SegmentContent::Text(text) => texts.push(text.clone()),
                    other => panic!("say-all speech is marks and text: {other:?}"),
                }
            }
            assert!(
                matches!(
                    utterance.segments.first().map(|first| &first.content),
                    Some(SegmentContent::Mark(_))
                ),
                "an utterance opens with a mark: {utterance:?}"
            );
            step.said.push(Utterance { marks, texts, key });
        }
        other => panic!("an effect these tests read: {other:?}"),
    }
}

/// The line `text`, starting at anchor `anchor`, read with the point at
/// byte `offset`.
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

/// Say-all's read ahead from `at`, after `movement`, by `unit`.
fn read_ahead(at: TextPoint, movement: Option<TextMovement>, unit: TextUnit) -> Batch {
    text_batches::read(node(2), at, movement, unit, 20)
}

/// The read a line on from the line at `anchor`.
fn a_line_on(anchor: u64) -> Batch {
    read_ahead(
        TextPoint::At(TextPosition::at(TextAnchor(anchor))),
        Some(TextMovement {
            unit: TextUnit::Line,
            count: 1,
        }),
        TextUnit::Line,
    )
}

/// The caret moved to the start of the line at `anchor`.
fn caret_to(anchor: u64) -> Batch {
    text_batches::move_caret(node(2), TextPoint::At(TextPosition::at(TextAnchor(anchor))))
}

/// A caret move made, minting baseline 60.
fn placed() -> Reply {
    text_batches::moved(60)
}

/// The answer to request `number`.
fn text_answer(number: u64, reply: Reply) -> Input<Entry> {
    answer(OUTPOST, number, reply)
}

/// The answer to request `number`: `chunks`, moved by none.
fn chunks(number: u64, chunks: Vec<TextChunk>) -> Input<Entry> {
    text_answer(number, text_batches::one(text_batches::chunks(0, chunks)))
}

/// Say-all from the caret pressed as key 1 on "Body", its first read
/// (request 3) answered that there are no sentences and its read by line
/// (request 4) answered with `lines`; returns that answer's step.
fn by_line(state: &mut Held, lines: Vec<TextChunk>) -> Step {
    let started = step(state, 1_000, press(1, ReviewCommand::SayAllFromCaret));
    assert_eq!(started.display, [true]);
    assert_eq!(
        started.one_sent(),
        (3, read_ahead(TextPoint::Caret, None, TextUnit::Sentence))
    );
    let again = step(
        state,
        1_100,
        text_answer(
            3,
            text_batches::one(text_batches::refused(StepRefusal::NoSuchUnit)),
        ),
    );
    assert_eq!(
        again.one_sent(),
        (4, read_ahead(TextPoint::Caret, None, TextUnit::Line))
    );
    step(state, 1_200, chunks(4, lines))
}

#[test]
fn say_all_reads_by_line_where_there_are_no_sentences_and_moves_the_caret() {
    let mut state = editing();
    let read = by_line(&mut state, vec![line("First.\n", 100, 0)]);
    assert_eq!(read.texts(), [["First."]]);
    // Little is left to speak, so the next batch is read at once, a line on
    // from the last line read, as its own entry.
    assert_eq!(read.one_sent(), (5, a_line_on(100)));
    // Playback reaches the first line while that read is out: its entry
    // waits its turn.
    let mark = read.said[0].marks[0];
    let waiting = step(&mut state, 1_300, at_mark(mark));
    assert!(
        waiting.sent.is_empty() && waiting.said.is_empty(),
        "{waiting:?}"
    );
    // The document ends; at the mark's turn the caret moves to the line,
    // say-all is over and the display may sleep.
    let ended = step(&mut state, 1_400, chunks(5, vec![line("First.\n", 100, 0)]));
    assert_eq!(ended.one_sent(), (6, caret_to(100)));
    assert_eq!(ended.display, [false]);
    assert!(ended.said.is_empty(), "{ended:?}");
    assert!(state.say_all.is_none());
}

#[test]
fn say_all_hands_out_a_batch_a_piece_at_a_time_and_ends_after_the_last() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("Three.", 102, 0)
    };
    let read = by_line(
        &mut state,
        vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last],
    );
    // Two pieces with speech, the third waiting; the text's end was read,
    // so nothing more is asked for.
    assert_eq!(read.texts(), [["One."], ["Two."]]);
    assert!(read.sent.is_empty(), "{read:?}");
    let (first, second) = (read.said[0].marks[0], read.said[1].marks[0]);
    // Reaching the first hands on the third, with its own mark, and moves
    // the caret there.
    let reached_first = step(&mut state, 1_300, at_mark(first));
    assert_eq!(reached_first.texts(), [["Three."]]);
    assert_eq!(reached_first.one_sent(), (5, caret_to(100)));
    let third = reached_first.said[0].marks[0];
    let moved = step(&mut state, 1_310, text_answer(5, placed()));
    assert!(moved.sent.is_empty() && moved.said.is_empty(), "{moved:?}");
    // Reaching the second moves the caret on.
    let reached_second = step(&mut state, 1_400, at_mark(second));
    assert_eq!(reached_second.one_sent(), (6, caret_to(101)));
    assert!(reached_second.said.is_empty(), "{reached_second:?}");
    let _ = step(&mut state, 1_410, text_answer(6, placed()));
    // The last piece reached: the caret moves to it and say-all ends.
    let reached_last = step(&mut state, 1_500, at_mark(third));
    assert_eq!(reached_last.one_sent(), (7, caret_to(102)));
    assert_eq!(reached_last.display, [false]);
}

#[test]
fn say_all_reads_the_next_batch_when_fewer_than_ten_pieces_are_left() {
    let mut state = editing();
    // Twelve lines to speak: two with speech, ten buffered, which is not
    // yet fewer than ten.
    let lines: Vec<TextChunk> = (0..12)
        .map(|index| line("A b.\n", 100 + index, 0))
        .collect();
    let read = by_line(&mut state, lines);
    let reads = |step: &Step| {
        step.sent
            .iter()
            .filter(|(_, batch)| matches!(batch.steps[0].op, Op::ReadText { count: 20, .. }))
            .count()
    };
    assert_eq!(reads(&read), 0);
    let mut marks: Vec<SpeechMark> = read.said.iter().map(|said| said.marks[0]).collect();
    // Each mark reached moves the caret, holding the queue until the move
    // is answered; a read it wants joins behind it.
    let mut reach = |state: &mut Held, index: u64| {
        let at_us = 2_000 + index * 100;
        let reached = step(
            state,
            at_us,
            at_mark(marks[usize::try_from(index).unwrap()]),
        );
        marks.extend(reached.said.iter().map(|said| said.marks[0]));
        let (number, batch) = reached.one_sent();
        assert!(
            matches!(batch.steps[0].op, Op::Select { .. }),
            "{reached:?}"
        );
        let moved = step(state, at_us + 10, text_answer(number, placed()));
        reads(&moved)
    };
    // Each line reached leaves one fewer: eleven, then ten, then nine, when
    // the next batch is read, once.
    assert_eq!(reach(&mut state, 0), 0);
    assert_eq!(reach(&mut state, 1), 0);
    assert_eq!(reach(&mut state, 2), 1);
}

/// Reads a paragraph by sentence from the caret: say-all pressed as key 1,
/// its first read answered with "One. Two? Three!"; returns that step.
fn by_sentence(state: &mut Held) -> Step {
    let started = step(state, 1_000, press(1, ReviewCommand::SayAllFromCaret));
    assert_eq!(started.sent.len(), 1, "{started:?}");
    let paragraph = TextChunk {
        unit: TextUnit::Paragraph,
        ..line("One. Two? Three!\r\n", 400, 0)
    };
    step(state, 1_100, chunks(3, vec![paragraph]))
}

#[test]
fn say_all_splits_a_paragraph_into_sentences_and_a_key_withdraws_its_read() {
    let mut state = editing();
    let read = by_sentence(&mut state);
    // Two sentences with speech, the third waiting; so little is left that
    // the next paragraph is read at once, a paragraph on.
    assert_eq!(read.texts(), [["One. "], ["Two? "]]);
    assert_eq!(
        read.one_sent(),
        (
            4,
            read_ahead(
                TextPoint::At(TextPosition::at(TextAnchor(400))),
                Some(TextMovement {
                    unit: TextUnit::Paragraph,
                    count: 1
                }),
                TextUnit::Sentence,
            )
        )
    );
    // A key stops it while that read is out: the read is withdrawn and the
    // display may sleep; a late mark and the late answer are ignored.
    let key = step(&mut state, 1_200, passed_key(2));
    assert_eq!(
        key.cancelled,
        [Cancel {
            number: RequestNumber(4),
            reason: CancelReason::Withdrawn
        }]
    );
    assert_eq!(key.display, [false]);
    assert!(key.said.is_empty() && key.sent.is_empty(), "{key:?}");
    let late = step(&mut state, 1_300, at_mark(read.said[1].marks[0]));
    assert!(late.said.is_empty() && late.sent.is_empty(), "{late:?}");
    let answer = step(&mut state, 1_400, chunks(4, vec![line("x", 500, 0)]));
    assert_eq!(answer.dropped, [4]);
    assert!(
        answer.said.is_empty() && answer.sent.is_empty(),
        "{answer:?}"
    );
}

/// Reaching the second sentence moves the caret into the paragraph, at the
/// sentence's start, and hands on the third.
#[test]
fn say_all_moves_the_caret_to_a_sentence_inside_a_paragraph() {
    let mut state = editing();
    let read = by_sentence(&mut state);
    let _ = step(&mut state, 1_150, chunks(4, Vec::new()));
    let reached = step(&mut state, 1_200, at_mark(read.said[1].marks[0]));
    assert_eq!(reached.texts(), [["Three!"]]);
    assert_eq!(
        reached.one_sent(),
        (
            5,
            text_batches::move_caret(
                node(2),
                TextPoint::At(TextPosition {
                    anchor: TextAnchor(400),
                    offset: 5
                })
            )
        )
    );
}

/// The key's cut of speech reaches Core outside the queue: say-all stops
/// at once and its read out is withdrawn.
#[test]
fn a_keys_cut_of_speech_stops_say_all_and_withdraws_its_read() {
    let mut state = editing();
    let read = by_sentence(&mut state);
    assert_eq!(read.sent.len(), 1, "{read:?}");
    let cut = reduce(
        &mut state,
        &verbatim_model::Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::SpeechCut {
                key: verbatim_model::KeySeq::new(2),
            },
        },
    );
    assert_eq!(cut.display, [false]);
    assert_eq!(
        cut.cancelled,
        [Cancel {
            number: RequestNumber(4),
            reason: CancelReason::Withdrawn
        }]
    );
    assert!(cut.said.is_empty() && cut.sent.is_empty(), "{cut:?}");
}

/// Speech the reducer did not cut off itself cuts say-all's utterances
/// off: once the pipeline reports them, say-all stops, as on a key, rather
/// than waiting for marks playback will never reach.
#[test]
fn say_all_stops_when_its_utterance_is_cut_off_elsewhere() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("Two.\n", 101, 0)
    };
    let read = by_line(&mut state, vec![line("One.\n", 100, 0), last]);
    assert_eq!(read.texts(), [["One."], ["Two."]]);
    let cut = |marks: Vec<SpeechMark>| verbatim_model::Input::Queue {
        trace_id: verbatim_model::TraceId::from_raw(0),
        at_us: 0,
        input: verbatim_model::request::QueueInput::UtteranceCancelled { marks, at_ms: 0 },
    };
    // An utterance with no mark of say-all's changes nothing.
    let other = reduce(&mut state, &cut(vec![SpeechMark(9_999)]));
    assert!(
        other.display.is_empty() && other.said.is_empty(),
        "{other:?}"
    );
    let stopped = reduce(&mut state, &cut(vec![read.said[0].marks[0]]));
    assert_eq!(stopped.display, [false]);
    assert!(stopped.cancelled.is_empty(), "no read was out: {stopped:?}");
    // Its other utterance's report, and a late mark, find nothing waiting.
    let again = reduce(&mut state, &cut(vec![read.said[1].marks[0]]));
    assert!(
        again.display.is_empty() && again.said.is_empty(),
        "{again:?}"
    );
    let late = step(&mut state, 1_300, at_mark(read.said[1].marks[0]));
    assert!(late.said.is_empty() && late.sent.is_empty(), "{late:?}");
}

/// An utterance the synthesizer failed on counts as heard: say-all carries
/// on past it, leaving the caret where the last heard unit put it, and
/// moves it again when playback reaches the next utterance.
#[test]
fn say_all_carries_on_past_an_utterance_that_failed() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("Three.\n", 102, 0)
    };
    let read = by_line(
        &mut state,
        vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last],
    );
    assert_eq!(read.texts(), [["One."], ["Two."]]);
    // The first fails: nothing moves, and the third is handed on.
    let failed = reduce(
        &mut state,
        &verbatim_model::Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::UtteranceFailed {
                marks: vec![read.said[0].marks[0]],
            },
        },
    );
    assert_eq!(failed.texts(), [["Three."]]);
    assert!(failed.sent.is_empty(), "{failed:?}");
    // Playback reaches the second: the caret moves there.
    let reached = step(&mut state, 1_300, at_mark(read.said[1].marks[0]));
    assert_eq!(reached.one_sent(), (5, caret_to(101)));
}

/// An utterance failing with little left to speak wants the next read,
/// which joins the queue as its own entry.
#[test]
fn a_failed_utterance_with_little_left_reads_on_as_an_entry() {
    let mut state = editing();
    // Eleven lines: two with speech and nine buffered, so nothing more is
    // read yet.
    let lines: Vec<TextChunk> = (0..11)
        .map(|index| {
            line(
                "A b.
",
                100 + index,
                0,
            )
        })
        .collect();
    let read = by_line(&mut state, lines);
    assert!(read.sent.is_empty(), "{read:?}");
    // Both fail: two more are handed on and seven are left buffered, so
    // the next read joins the queue as its own entry and is asked at once.
    let failed = reduce(
        &mut state,
        &verbatim_model::Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::UtteranceFailed {
                marks: vec![read.said[1].marks[0]],
            },
        },
    );
    assert_eq!(failed.said.len(), 2, "{failed:?}");
    assert_eq!(failed.one_sent(), (5, a_line_on(110)));
}

/// Say-all sets the caret before a key reads it: each move made is taken
/// as Core's caret, the unit read with the caret at its start.
#[test]
fn a_caret_move_say_all_made_is_cores_caret() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("Two.\n", 101, 0)
    };
    let read = by_line(&mut state, vec![line("One.\n", 100, 0), last]);
    let first = step(&mut state, 1_300, at_mark(read.said[0].marks[0]));
    let (number, op) = first.one_sent();
    assert_eq!(op, caret_to(100));
    let _ = step(&mut state, 1_310, text_answer(number, placed()));
    let caret = state.focus_caret(node(2)).expect("Core holds the caret");
    assert_eq!(caret.line.start, TextAnchor(100));
    // The move minted the next selection baseline.
    assert_eq!(
        crate::text_facts::held(&state.state),
        Some((node(2), TextAnchor(60)))
    );
}

/// Every utterance say-all hands to speech carries its starting key's
/// number, those handed on by a later index mark's entry too.
#[test]
fn say_alls_speech_carries_its_starting_keys_number() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("Three.", 102, 0)
    };
    let read = by_line(
        &mut state,
        vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last],
    );
    let reached = step(&mut state, 1_300, at_mark(read.said[0].marks[0]));
    let keys: Vec<Option<KeySeq>> = read
        .said
        .iter()
        .chain(&reached.said)
        .map(|said| said.key)
        .collect();
    assert_eq!(keys, [Some(KeySeq::new(1)); 3]);
}

/// The outpost generation holding say-all's positions ends: say-all stops,
/// its read answered "reader ended", and nothing more is read.
#[test]
fn say_all_stops_when_its_outpost_generation_ends() {
    let mut state = editing();
    let read = by_sentence(&mut state);
    assert_eq!(read.sent.len(), 1, "{read:?}");
    let ended = step(&mut state, 1_200, Input::ReaderEnded(OUTPOST));
    assert_eq!(ended.display, [false]);
    // Its first failure is replaced at once.
    assert_eq!(ended.started, [crate::entries::tests::APP]);
    assert!(ended.said.is_empty() && ended.sent.is_empty(), "{ended:?}");
    assert!(state.say_all.is_none());
    let late = step(&mut state, 1_300, at_mark(read.said[1].marks[0]));
    assert!(late.said.is_empty() && late.sent.is_empty(), "{late:?}");
}

/// A read whose text is not there stops say-all.
#[test]
fn a_read_of_text_not_there_stops_say_all() {
    let mut state = editing();
    let _ = step(&mut state, 1_000, press(1, ReviewCommand::SayAllFromCaret));
    let gone = step(
        &mut state,
        1_100,
        answer_outcome(OUTPOST, 3, WireOutcome::NotThere),
    );
    assert_eq!(gone.display, [false]);
    assert!(state.say_all.is_none());
}

#[test]
fn say_all_from_the_caret_with_no_focus_says_no_caret() {
    let mut state = ready();
    let effects = feed_at(&mut state, 1_000, press(1, ReviewCommand::SayAllFromCaret));
    assert_eq!(effects, ["Message(NoCaret)"]);
}

/// Say-all on a focus that cannot have text, a button, says nothing and
/// starts nothing, as NVDA's says nothing where no caret position can be
/// made (`speech/sayAll.py` 419 to 423, 137 to 139; Dickson, 2026-11-19).
#[test]
fn say_all_from_the_caret_on_a_focus_without_text_says_nothing() {
    let mut state = committed("OK", Role::Button);
    let effects = feed_at(&mut state, 1_000, press(1, ReviewCommand::SayAllFromCaret));
    assert_eq!(effects, Vec::<String>::new());
    assert!(state.say_all.is_none());
}

#[test]
fn say_all_from_the_review_cursor_leaves_the_review_cursor_where_it_stopped() {
    let mut state = editing();
    // The review cursor placed on the line at anchor 100.
    let _ = step(&mut state, 900, press(1, ReviewCommand::ReviewCurrentLine));
    let _ = feed_at(
        &mut state,
        950,
        text_answer(
            3,
            text_batches::one(text_batches::chunks(0, vec![line("one\n", 100, 0)])),
        ),
    );
    let started = step(&mut state, 1_000, press(2, ReviewCommand::SayAllFromReview));
    assert_eq!(
        started.one_sent(),
        (
            4,
            read_ahead(
                TextPoint::At(TextPosition::at(TextAnchor(100))),
                None,
                TextUnit::Sentence,
            )
        )
    );
    let paragraph = TextChunk {
        unit: TextUnit::Paragraph,
        last: true,
        ..line("A b. C d.\n", 500, 0)
    };
    let read = step(&mut state, 1_100, chunks(4, vec![paragraph]));
    assert_eq!(read.texts(), [["A b. "], ["C d."]]);
    // Reaching the second sentence moves the review cursor, asking nothing.
    let reached = step(&mut state, 1_200, at_mark(read.said[1].marks[0]));
    assert!(reached.sent.is_empty(), "{reached:?}");
    let _ = step(&mut state, 1_300, passed_key(3));
    // The next review command reads the line where reading stopped.
    let next = step(
        &mut state,
        1_400,
        press(4, ReviewCommand::ReviewCurrentLine),
    );
    assert_eq!(
        next.one_sent(),
        (
            5,
            text_batches::line(
                node(2),
                TextPoint::At(TextPosition {
                    anchor: TextAnchor(500),
                    offset: 5
                }),
                None
            )
        )
    );
}

#[test]
fn say_all_splits_lines_at_their_last_sentence_end_and_joins_a_sentence_across_lines() {
    let mut state = editing();
    let last = TextChunk {
        last: true,
        ..line("line.\n", 102, 0)
    };
    let read = by_line(
        &mut state,
        vec![
            line(
                "Verbatim is written in Rust. It is informed by NVDA\n",
                100,
                0,
            ),
            line("but not constrained by it. A last\n", 101, 0),
            last,
        ],
    );
    // The first line up to its sentence end, then the sentence that runs
    // on to the second line spoken whole, with the second line's mark
    // where its text starts; the text's end was read, so nothing more is
    // asked for.
    assert_eq!(
        read.texts(),
        [
            vec!["Verbatim is written in Rust. "],
            vec!["It is informed by NVDA ", "but not constrained by it. "]
        ]
    );
    assert!(read.sent.is_empty(), "{read:?}");
    let (first, second) = (&read.said[0], &read.said[1]);
    assert_eq!((first.marks.len(), second.marks.len()), (1, 2));
    // The first line reached: the caret moves to its start, and the last
    // utterance is handed on, the text held back at the end spoken with
    // the last line.
    let reached = step(&mut state, 1_300, at_mark(first.marks[0]));
    assert_eq!(reached.one_sent(), (5, caret_to(100)));
    assert_eq!(reached.texts(), [["A last ", "line."]]);
    let third = reached.said[0].marks.clone();
    assert_eq!(third.len(), 2);
    let _ = step(&mut state, 1_310, text_answer(5, placed()));
    // The rest of the first line: the caret stays on the first line.
    let rest = step(&mut state, 1_400, at_mark(second.marks[0]));
    assert!(rest.sent.is_empty() && rest.said.is_empty(), "{rest:?}");
    // The second line's text starts: the caret moves to its start.
    let next = step(&mut state, 1_500, at_mark(second.marks[1]));
    assert_eq!(next.one_sent(), (6, caret_to(101)));
    let _ = step(&mut state, 1_510, text_answer(6, placed()));
    // The last utterance starts with the rest of the second line: the
    // caret stays there, and a key stops say-all with it there.
    let held = step(&mut state, 1_600, at_mark(third[0]));
    assert!(held.sent.is_empty() && held.said.is_empty(), "{held:?}");
    let key = step(&mut state, 1_700, passed_key(2));
    assert_eq!(key.display, [false]);
    let late = step(&mut state, 1_800, at_mark(third[1]));
    assert!(late.sent.is_empty() && late.said.is_empty(), "{late:?}");
}

#[test]
fn say_all_speaks_ten_lines_with_no_sentence_end_together() {
    let mut state = editing();
    // Twelve lines with no sentence end, the fifth blank, which says
    // nothing but counts among the ten.
    let mut lines: Vec<TextChunk> = (0..12)
        .map(|index| {
            let text = if index == 4 {
                "\n".to_owned()
            } else {
                format!("word {index}\n")
            };
            line(&text, 100 + index, 0)
        })
        .collect();
    lines[11].last = true;
    let read = by_line(&mut state, lines);
    assert!(read.sent.is_empty(), "{read:?}");
    // The first ten lines, the blank one silent, each with its mark; the
    // rest, spoken when the text ends.
    assert_eq!(
        read.texts(),
        [
            vec![
                "word 0 ", "word 1 ", "word 2 ", "word 3 ", "word 5 ", "word 6 ", "word 7 ",
                "word 8 ", "word 9",
            ],
            vec!["word 10 ", "word 11"],
        ]
    );
    assert_eq!(read.said[0].marks.len(), 9);
    assert_eq!(read.said[1].marks.len(), 2);
    // Each line's mark moves the caret to that line's start.
    let reached = step(&mut state, 1_300, at_mark(read.said[0].marks[4]));
    assert_eq!(reached.one_sent(), (5, caret_to(105)));
}

/// A move between command marks, on text no terminal model of Core's
/// answers, has no marks to move between: it says nothing and stops
/// say-all, as any command does.
#[test]
fn a_command_mark_outside_a_modelled_terminal_says_nothing_and_stops_say_all() {
    let mut state = editing();
    let read = by_sentence(&mut state);
    assert_eq!(read.sent.len(), 1, "{read:?}");
    let next = step(&mut state, 1_200, press(2, ReviewCommand::NextCommandMark));
    assert_eq!(next.display, [false]);
    assert_eq!(next.cancelled.len(), 1, "{next:?}");
    assert!(next.said.is_empty() && next.sent.is_empty(), "{next:?}");
    let previous = step(
        &mut state,
        1_300,
        press(3, ReviewCommand::PreviousCommandMark),
    );
    assert!(
        previous.said.is_empty() && previous.sent.is_empty() && previous.display.is_empty(),
        "{previous:?}"
    );
}
