//! Scripted-input tests for milestone M4's text handling in the reducer:
//! caret keys and the selection, typed character and word echo, the
//! review cursor over text, say-all, and formatting spoken as it changes,
//! each driven through the text protocol with scripted outpost replies.

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretRead, CaretReport, CaretSelection, Change, Compared,
    Effect, Input, Message, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase,
    Pid, QueryId, ReadFacts, ReadPurpose, ReaderSettings, ReviewCommand, Role, SegmentContent,
    Selection, SelectionChange, SelectionText, SpeechMark, SpeechPriority, State, StateSet,
    TextAnchor, TextChunk, TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReadAhead,
    TextReply, TextRequest, TextThrough, TextUnit, TraceId, TypingEcho, Utterance,
    UtteranceSegment,
};
use verbatim_model::{BulletStyle, FocusText, FormatRun, LineStyle, TextAttributes, TextFormat};

const OUTPOST: OutpostId = OutpostId(1);

fn id(number: u64) -> NodeId {
    NodeId::in_outpost(OUTPOST, number)
}

fn node(number: u64, role: Role, states: StateSet) -> NodeSnapshot {
    NodeSnapshot {
        id: id(number),
        backend: Backend::Uia,
        role,
        name: Some("Body".to_owned()),
        value: None,
        states,
        details: NodeDetails::default(),
    }
}

fn event(event: NormalizedEvent) -> Input {
    Input::Event {
        trace_id: TraceId::from_raw(1),
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
        purpose: None,
    }
}

fn focus(state: &mut SrState, snapshot: NodeSnapshot) {
    let _ = reduce(
        state,
        &event(NormalizedEvent::FocusChanged {
            node: snapshot,
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        }),
    );
}

/// A line chunk starting at `anchor`, with the point of interest at
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

fn caret_event(state: &mut SrState, number: u64, line: TextChunk) {
    let caret = CaretReport {
        line,
        selection: None,
        read_at_us: stamp(),
    };
    set_baseline(&caret);
    let _ = reduce(
        state,
        &event(NormalizedEvent::CaretMoved {
            node_id: id(number),
            caret,
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
            facts: None,
        }),
    );
}

/// An edit field with focus and its caret on `text` at `offset`.
fn editing(text: &str, offset: u32) -> SrState {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    caret_event(&mut state, 5, line(text, 100, offset));
    state
}

/// Answers the read of the line at the caret that a review command makes
/// while the review cursor follows the caret, with `line`.
fn read_at_caret(state: &mut SrState, effects: &[Effect], line: TextChunk) -> Vec<Effect> {
    let asked = request(effects);
    assert_eq!(
        asked.op,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        })
    );
    reduce(
        state,
        &completed(
            asked.query_id,
            TextReply::Read {
                moved: 0,
                chunk: line,
            },
        ),
    )
}

/// Settles the review cursor, following the caret, on the caret's `line`,
/// as the first review command after the caret moved does.
fn settle(state: &mut SrState, line: TextChunk) {
    let effects = reduce(state, &command(ReviewCommand::ReviewCurrentCharacter, 0));
    let _ = read_at_caret(state, &effects, line);
}

/// An edit field as [`editing`] makes it, with the review cursor settled at
/// the caret.
fn reviewing(text: &str, offset: u32) -> SrState {
    let mut state = editing(text, offset);
    settle(&mut state, line(text, 100, offset));
    state
}

/// When [`key`] says each key was pressed.
const KEY_PRESSED_AT: u64 = 1_700_000_000_000;

fn key(motion: CaretMotion, select: bool) -> Input {
    Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(2),
        key: CaretKey { motion, select },
        pressed_at_ms: KEY_PRESSED_AT,
    }
}

fn command(command: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        pressed_at_ms: 0,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(3),
        command,
        repeat,
    }
}

fn completed(query_id: QueryId, reply: impl Into<Reply>) -> Input {
    completed_on(query_id, id(5), reply)
}

/// [`completed`], a report of `node_id`.
fn completed_on(query_id: QueryId, node_id: NodeId, reply: impl Into<Reply>) -> Input {
    match reply.into() {
        Reply::Text(reply) => completed_text(query_id, reply),
        Reply::Key(reply) => {
            let CaretReply {
                moved,
                caret,
                read_at_ms,
                unit,
                selection_changes,
                content_changed,
                new_selection,
                same_line,
                ..
            } = *reply;
            let (before_line, before_selection) = baseline_taken();
            let mut caret = caret;
            if read_at_ms != 0 {
                caret.read_at_us = read_at_ms * 1_000;
            }
            set_baseline(&caret);
            let facts = key_facts(
                unit.unwrap_or_else(|| caret.line.clone()),
                moved,
                (before_line, before_selection),
                same_line.unwrap_or(true),
            );
            event(NormalizedEvent::CaretMoved {
                node_id,
                caret,
                selection_changes,
                content_changed,
                new_selection,
                facts: Some(Box::new(facts)),
            })
        }
    }
}

/// When a report's read ended: after every key's pass in these tests.
const READ_ENDED_AT_US: u64 = u64::MAX / 2;

/// A key's read facts: `moved` says whether the caret left where it was,
/// in the last read compared with.
fn key_facts(
    unit: TextChunk,
    moved: bool,
    (previous_line, previous_selection): (TextChunk, Option<CaretSelection>),
    on_previous_line: bool,
) -> ReadFacts {
    ReadFacts {
        unit: Some(unit),
        change: Change::Compared {
            compared: Compared {
                caret_moved: moved,
                ..Compared::default()
            },
            previous_line,
            previous_selection,
            on_previous_line,
        },
        read_ended_at_us: READ_ENDED_AT_US,
    }
}

/// The answer of a read for a key, not an evidence, finding `line`.
fn caret_read_reply(caret: CaretReport, unit: TextChunk) -> TextReply {
    let facts = key_facts(unit, false, baseline_taken(), true);
    TextReply::Caret(Box::new(verbatim_model::CaretReply {
        caret,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts,
    }))
}

/// The key's bound passes and its final read, asked then, answers `line`
/// showing no effect.
fn bound_answer(state: &mut SrState, line: TextChunk) -> Vec<Effect> {
    let due = reduce(
        state,
        &Input::HeldKeyDue {
            trace_id: TraceId::from_raw(2),
            at_us: READ_ENDED_AT_US,
        },
    );
    let unit = line.clone();
    let caret = CaretReport {
        line,
        selection: None,
        read_at_us: stamp(),
    };
    reduce(
        state,
        &completed_text(request_of(&due), caret_read_reply(caret, unit)),
    )
}

/// The key's bound passes and its final read goes unanswered: it ends.
fn bound_unanswered(state: &mut SrState) -> Vec<Effect> {
    let due = reduce(
        state,
        &Input::HeldKeyDue {
            trace_id: TraceId::from_raw(2),
            at_us: READ_ENDED_AT_US,
        },
    );
    reduce(
        state,
        &completed_text(request_of(&due), TextReply::Unanswered),
    )
}

/// A reply as a test gives it: a text reply, or a key's result, which the
/// protocol sends as a report with its facts.
enum Reply {
    Text(TextReply),
    Key(Box<CaretReply>),
}

impl From<TextReply> for Reply {
    fn from(reply: TextReply) -> Self {
        Self::Text(reply)
    }
}

/// A key's result as these tests describe it.
struct CaretReply {
    moved: bool,
    caret: CaretReport,
    read_at_ms: u64,
    unit: Option<TextChunk>,
    selection_changes: Vec<SelectionChange>,
    content_changed: bool,
    new_selection: Option<SelectionChange>,
    same_line: Option<bool>,
}

fn key_reply(reply: Box<CaretReply>) -> Reply {
    Reply::Key(reply)
}

std::thread_local! {
    /// The read the next key is judged from.
    static BASELINE: std::cell::RefCell<Option<(TextChunk, Option<CaretSelection>)>> =
        const { std::cell::RefCell::new(None) };
    /// The next stamp a report takes.
    static STAMP: std::cell::Cell<u64> = const { std::cell::Cell::new(1_000) };
}

/// A new, later stamp.
fn stamp() -> u64 {
    STAMP.with(|stamp| {
        let next = stamp.get() + 1;
        stamp.set(next);
        next
    })
}

fn set_baseline(caret: &CaretReport) {
    BASELINE.with(|baseline| {
        *baseline.borrow_mut() = Some((caret.line.clone(), caret.selection.clone()));
    });
}

/// The baseline, as the outpost keeps it from the last caret it reported.
fn baseline_taken() -> (TextChunk, Option<CaretSelection>) {
    BASELINE.with(|baseline| {
        baseline
            .borrow()
            .clone()
            .unwrap_or_else(|| (line("", 100, 0), None))
    })
}

fn selection_starting_on(selection: Selection, line: Option<Box<TextChunk>>) -> CaretSelection {
    CaretSelection {
        range: selection,
        start_line: line,
    }
}

fn caret_selection(selection: Selection) -> CaretSelection {
    CaretSelection {
        range: selection,
        start_line: None,
    }
}

/// `completed` for a reply to a text request.
fn completed_text(query_id: QueryId, reply: TextReply) -> Input {
    text_answer(TraceId::from_raw(4), query_id, reply)
}

/// The outpost's answer `reply` to the text request `query_id`, traced
/// `trace_id`.
fn text_answer(trace_id: TraceId, query_id: QueryId, reply: TextReply) -> Input {
    Input::Answered {
        trace_id,
        query_id,
        outpost: OUTPOST,
        outcome: verbatim_model::protocol::QueryOutcome::Done(
            verbatim_model::protocol::QueryResult::Text(reply),
        ),
    }
}

/// `effect`, or the effect it carries on a key's behalf.
fn own(effect: &Effect) -> &Effect {
    match effect {
        Effect::ForKey(_, effect) => effect,
        effect => effect,
    }
}

/// The one text request among `effects`.
fn request(effects: &[Effect]) -> TextRequest {
    if let Some(note) = effects.iter().find_map(|effect| match own(effect) {
        Effect::KeyNote(note) => Some(*note),
        _ => None,
    }) {
        // A key the hook passed sends a note and asks no query: its reports
        // are matched by their node.
        return TextRequest {
            query_id: QueryId(0),
            node_id: note.node(),
            op: TextOp::ReadCaret(CaretRead {
                fetch: note.fetch(),
                purpose: ReadPurpose::BeforeFieldKey,
            }),
        };
    }
    let requests: Vec<&TextRequest> = effects
        .iter()
        .filter_map(|effect| match own(effect) {
            Effect::Text(request) => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 1, "one text request in {effects:?}");
    requests[0].clone()
}

/// Every segment spoken by `effects`, in order.
fn spoken(effects: &[Effect]) -> Vec<UtteranceSegment> {
    effects
        .iter()
        .filter_map(|effect| match own(effect) {
            Effect::Speak(utterance, _) => Some(utterance.segments.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn character(text: &str) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Character(text.to_owned()))
}

fn message(message: Message) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Message(message))
}

fn caret_reply(moved: bool, line: TextChunk, unit: Option<TextChunk>) -> Reply {
    key_reply(Box::new(CaretReply {
        same_line: None,
        moved,
        read_at_ms: 0,
        caret: CaretReport {
            line,
            selection: None,
            read_at_us: stamp(),
        },
        unit,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }))
}

#[test]
fn an_arrow_key_watches_for_evidence_then_speaks_the_character_at_the_caret() {
    let mut state = editing("Hello\r\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, false));
    let request = request(&effects);
    assert_eq!(request.node_id, id(5));

    let effects = reduce(
        &mut state,
        &completed(
            request.query_id,
            caret_reply(true, line("Hello\r\n", 100, 1), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("e")]);
    // At the end of the line, the caret is on the line break, named by its
    // first character, as NVDA names it.
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("Hello\r\n", 100, 5), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("\r")]);
    // At the end of the text there is no character: blank.
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("Bye", 101, 3), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
}

#[test]
fn a_backspace_in_a_run_of_one_character_watches_its_line() {
    // Deleting one star of a password field's mask changes neither the
    // characters beside the caret nor, where positions follow edits, the
    // caret: the watch says the key deletes, so the outpost takes the line
    // changing as evidence, and the star removed is spoken.
    let mut state = editing("******", 5);
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let request = request(&effects);
    let effects = reduce(
        &mut state,
        &completed(
            request.query_id,
            caret_reply(true, line("*****", 100, 4), None),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Character(
            "*".to_owned()
        ))]
    );
}

#[test]
fn a_caret_key_that_moves_nothing_says_its_unit_at_its_bound() {
    // Control+Home at the start of the text: the application moves nothing,
    // so the final read finds no evidence, and the line is said as NVDA's
    // script says it at its 100 ms.
    let mut state = editing("one two\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::Top, false));
    let _ = request(&effects);
    let effects = bound_answer(&mut state, line("one two\n", 100, 0));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("one two")]);
}

#[test]
fn a_caret_keys_watch_ends_with_its_answer_and_with_typing() {
    // Once a watch is answered, ended or not, the key waits for nothing: a
    // second answer to the same request says nothing.
    let mut state = editing("abc\n", 0);
    let query = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let effects = bound_unanswered(&mut state);
    assert_eq!(effects, [Effect::BoundHeldKey(None)]);
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply(true, line("abc\n", 100, 1), None)),
    );
    assert_eq!(effects, [], "the watch had ended");

    // A key pressed while another is watched (here a character typed, its
    // key cutting speech off first, as every key does): an answer read
    // from then on may show the later key's effect, so it is not spoken
    // for the earlier key, and only keeps the caret current.
    let mut state = editing("abc\n", 0);
    let query = request_of(&reduce(&mut state, &key(CaretMotion::StartOfLine, false)));
    let _ = reduce(
        &mut state,
        &Input::SpeechCancelled {
            at_us: (50) * 1_000,
        },
    );
    let effects = reduce(&mut state, &typed("x"));
    assert_eq!(spoken(&effects), vec![character("x")]);
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply_at(line("xabc\n", 100, 1), 60)),
    );
    assert_eq!(
        effects,
        [Effect::BoundHeldKey(None)],
        "the later key's effect is not the key's answer"
    );
    // Its caret is now the newest, so a Backspace deletes what it shows.
    let query = request_of(&reduce(
        &mut state,
        &Input::CaretKey {
            key_seq: verbatim_model::KeySeq::NONE_YET,
            trace_id: TraceId::from_raw(5),
            key: CaretKey {
                motion: CaretMotion::Backspace,
                select: false,
            },
            pressed_at_ms: 70,
        },
    ));
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply_at(line("abc\n", 100, 0), 80)),
    );
    assert_eq!(spoken(&effects), vec![character("x")]);

    // An answer read before the later key was pressed is the key's own.
    let mut state = editing("abc\n", 0);
    let query = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let _ = reduce(
        &mut state,
        &Input::SpeechCancelled {
            at_us: (50) * 1_000,
        },
    );
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply_at(line("abc\n", 100, 1), 40)),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);
}

#[test]
fn a_line_break_the_caret_reaches_is_named_with_its_formatting_read() {
    // The outpost cuts the character from the line, a carriage return and
    // line feed as one cluster; it is named by its carriage return.
    let mut state = editing("Hello\r\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, false));
    let unit = TextChunk {
        unit: TextUnit::Character,
        formats: vec![FormatRun {
            start: 0,
            end: 2,
            attributes: TextAttributes::default(),
        }],
        ..line("\r\n", 102, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("Hello\r\n", 100, 5), Some(unit)),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("\r")]);
}

fn request_of(effects: &[Effect]) -> QueryId {
    request(effects).query_id
}

#[test]
fn line_word_and_paragraph_keys_speak_their_unit() {
    let mut state = editing("one two\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::NextLine, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("second line\n", 101, 0), None),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::text("second line")]
    );

    // The provider's word, as the application moved the caret by it.
    let effects = reduce(&mut state, &key(CaretMotion::NextWord, false));
    let word = TextChunk {
        unit: TextUnit::Word,
        ..line("line ", 102, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("second line\n", 101, 7), Some(word)),
        ),
    );
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("line")]);

    // A provider without paragraphs sends no unit: the line is spoken.
    let effects = reduce(&mut state, &key(CaretMotion::NextParagraph, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("third\n", 103, 0), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("third")]);
}

#[test]
fn a_word_of_one_character_is_spoken_as_that_character() {
    // Notepad's word unit makes a sentence's full stop a word of its own;
    // spoken as text, it would say nothing.
    let mut state = editing("the river bank.\r\n", 11);
    let effects = reduce(&mut state, &key(CaretMotion::NextWord, false));
    let word = TextChunk {
        unit: TextUnit::Word,
        ..line(".\r\n", 102, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("the river bank.\r\n", 101, 14), Some(word)),
        ),
    );
    assert_eq!(spoken(&effects), vec![character(".")]);
}

#[test]
fn the_review_cursor_speaks_a_word_of_one_character_by_its_name() {
    let mut state = reviewing("the river bank.\r\n", 11);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));
    assert_eq!(spoken(&effects), vec![character(".")]);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewCurrentWord, 0));
    assert_eq!(spoken(&effects), vec![character(".")]);
}

#[test]
fn shift_movement_speaks_what_was_selected_and_unselected() {
    let mut state = editing("hello, world\n", 5);
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, true));
    let request = request(&effects);
    assert!(matches!(request.op, TextOp::ReadCaret(_)));
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello, world\n", 100, 6),
            selection: None,
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: vec![
            SelectionChange {
                selected: true,
                text: ",".to_owned(),
                characters: 1,
            },
            SelectionChange {
                selected: false,
                text: "hello".to_owned(),
                characters: 5,
            },
            SelectionChange {
                selected: true,
                text: "x".repeat(600),
                characters: 600,
            },
        ],
        content_changed: false,
        new_selection: None,
    }));
    let effects = reduce(&mut state, &completed(request.query_id, reply));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Selected(
                SelectionText::Character(",".to_owned())
            ))),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Unselected(
                SelectionText::Text("hello".to_owned())
            ))),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Selected(
                SelectionText::Characters(600)
            ))),
        ]
    );
}

#[test]
fn a_movement_that_leaves_a_selection_speaks_the_unit_then_what_it_unselected() {
    let mut state = editing(
        "hello, world
",
        5,
    );
    let at = |offset| TextPosition {
        anchor: TextAnchor(100),
        offset,
    };
    let selected = Selection {
        start: at(0),
        end: at(5),
    };
    // Shift+Home selects "hello".
    let effects = reduce(&mut state, &key(CaretMotion::StartOfLine, true));
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(
                "hello, world
",
                100,
                0,
            ),
            selection: Some(caret_selection(selected)),
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));

    // Right Arrow sends no selection of its own, its answer comparing with
    // the selection the outpost kept, and speaks the character, then the
    // text unselected.
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, false));
    let asked = request(&effects);
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(
                "hello, world
",
                100,
                5,
            ),
            selection: None,
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: vec![SelectionChange {
            selected: false,
            text: "hello".to_owned(),
            characters: 5,
        }],
        content_changed: false,
        new_selection: None,
    }));
    let effects = reduce(&mut state, &completed(asked.query_id, reply));
    assert_eq!(
        spoken(&effects),
        vec![
            character(","),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Unselected(
                SelectionText::Text("hello".to_owned())
            ))),
        ]
    );

    // Nor does a deletion.
    let mut state = editing(
        "hello, world
",
        0,
    );
    let effects = reduce(&mut state, &key(CaretMotion::NextWord, true));
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(
                "hello, world
",
                100,
                5,
            ),
            selection: Some(caret_selection(selected)),
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Delete, false));
    assert!(matches!(request(&effects).op, TextOp::ReadCaret(_)));
}

/// A caret event for node 5 observed at `observed_at_ms`, in milliseconds
/// to compare with a key's `pressed_at_ms`.
fn caret_event_at(state: &mut SrState, line: TextChunk, observed_at_ms: u64) -> Vec<Effect> {
    reduce(
        state,
        &Input::Event {
            trace_id: TraceId::from_raw(6),
            observed_at_us: observed_at_ms * 1_000,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::CaretMoved {
                node_id: id(5),
                caret: CaretReport {
                    line,
                    selection: None,
                    read_at_us: stamp(),
                },
                selection_changes: Vec::new(),
                content_changed: false,
                new_selection: None,
                facts: None,
            },
            purpose: None,
        },
    )
}

/// A caret key pressed at `pressed_at_ms`.
fn key_at(motion: CaretMotion, pressed_at_ms: u64) -> Input {
    Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(7),
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms,
    }
}

/// A caret reply the outpost read at `read_at_ms`.
fn caret_reply_at(line: TextChunk, read_at_ms: u64) -> Reply {
    key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        caret: CaretReport {
            line,
            selection: None,
            read_at_us: stamp(),
        },
        read_at_ms,
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }))
}

#[test]
fn a_late_reply_read_before_a_newer_caret_event_leaves_the_caret_newest() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("abc", 100, 0), 300);
    let effects = reduce(&mut state, &key_at(CaretMotion::NextCharacter, 400));
    // A caret event observed after the reply was read reaches Core first.
    let _ = caret_event_at(&mut state, line("abc", 100, 2), 500);
    let _ = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("abc", 100, 1), 450),
        ),
    );
}

#[test]
fn a_review_cursor_moved_off_the_caret_reviews_from_where_it_is() {
    let mut state = reviewing("one two\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("two")]);
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Text(_)))
    );
    assert_eq!(spoken(&effects), vec![character("t")]);
}

#[test]
fn backspace_speaks_what_it_deleted_once_the_caret_moved() {
    let mut state = editing("abc\n", 2);
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("ac\n", 100, 1), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);

    // At the start of the text nothing is deleted, and nothing is said.
    let mut state = editing("abc\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let _ = request_of(&effects);
    let effects = bound_answer(&mut state, line("abc\n", 100, 0));
    assert_eq!(spoken(&effects), []);
}

#[test]
fn backspace_at_a_lines_start_names_the_line_break_it_deleted() {
    // Windows 11 Notepad's text breaks lines with a carriage return.
    let mut state = editing("def\r", 0);
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("abcdef\r", 99, 3), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("\r")]);

    // With Control too, as Notepad deletes the break before a line's start.
    let mut state = editing("def\r", 0);
    let effects = reduce(&mut state, &key(CaretMotion::BackspaceWord, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("abcdef\r", 99, 3), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("\r")]);

    // A standard edit control's carriage return and line feed are spoken
    // as the line feed. On the text's last line, which has no break, the
    // break is the one the text was last seen to use.
    let mut state = editing("abc\r\n", 1);
    caret_event(&mut state, 5, line("def", 101, 0));
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("abcdef", 100, 3), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("\n")]);

    // On the text's first line there is no break to delete.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    caret_event(
        &mut state,
        5,
        TextChunk {
            first: true,
            ..line("abc\r\n", 100, 0)
        },
    );
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("abc\r\n", 100, 0), None),
        ),
    );
    assert_eq!(spoken(&effects), []);
}

#[test]
fn delete_compares_the_character_and_speaks_the_new_one() {
    let mut state = editing("abc\n", 1);
    let effects = reduce(&mut state, &key(CaretMotion::Delete, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("ac\n", 100, 1), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("c")]);
}

/// A caret report of node 5 with `line` and `selection`, carrying
/// `changes`, said in general terms when `content_changed`.
fn selection_event(
    state: &mut SrState,
    (line, selection): (TextChunk, Option<Selection>),
    (changes, content_changed): (Vec<SelectionChange>, bool),
) -> Vec<Effect> {
    let caret = CaretReport {
        line,
        selection: selection.map(caret_selection),
        read_at_us: stamp(),
    };
    set_baseline(&caret);
    reduce(
        state,
        &event(NormalizedEvent::CaretMoved {
            node_id: id(5),
            caret,
            selection_changes: changes,
            content_changed,
            new_selection: None,
            facts: None,
        }),
    )
}

/// The answer to a caret key: `line`, nothing selected, the selection
/// removed in general terms.
fn selection_removed_reply(line: TextChunk) -> Reply {
    key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line,
            selection: None,
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: vec![SelectionChange {
            selected: false,
            text: String::new(),
            characters: 0,
        }],
        content_changed: true,
        new_selection: None,
    }))
}

/// A selection from byte `start` of the line whose anchor is 100 (or of
/// another chunk, `on_line` false) to an end elsewhere.
fn selection_from(start: u32, on_line: bool) -> Selection {
    Selection {
        start: TextPosition {
            anchor: TextAnchor(if on_line { 100 } else { 200 }),
            offset: start,
        },
        end: TextPosition {
            anchor: TextAnchor(201),
            offset: 0,
        },
    }
}

#[test]
fn backspace_over_a_selection_says_what_was_before_its_start_then_the_selection_change() {
    // What comes before the selection's start, where NVDA's caret is,
    // whichever end the caret is
    // (`target/e2e-evidence/m42-selection-keys-2026-10-22/`): a classic
    // edit control's caret is the start of "beta " (`textbox-second-word-backspace.txt`,
    // `textbox-second-word-control-backspace.txt`); Notepad's is the
    // end of "beta " (`notepad-second-word-control-backspace.txt`) and of
    // "gamma" up to the line's end
    // (`notepad-third-word-to-end-backspace.txt`,
    // `notepad-third-word-to-end-control-backspace.txt`).
    let text = "alpha beta gamma\r\n";
    let space = character(" ");
    let alpha = UtteranceSegment::text("alpha".to_owned());
    let beta = UtteranceSegment::text("beta".to_owned());
    for (caret, start, motion, said) in [
        (6, 6, CaretMotion::Backspace, &space),
        (6, 6, CaretMotion::BackspaceWord, &alpha),
        (11, 6, CaretMotion::BackspaceWord, &alpha),
        (16, 11, CaretMotion::Backspace, &space),
        (16, 11, CaretMotion::BackspaceWord, &beta),
    ] {
        let mut state = editing(text, 0);
        let _ = selection_event(
            &mut state,
            (line(text, 100, caret), Some(selection_from(start, true))),
            (Vec::new(), false),
        );
        let effects = reduce(&mut state, &key(motion, false));
        let effects = reduce(
            &mut state,
            &completed(
                request_of(&effects),
                selection_removed_reply(line("alpha gamma\r\n", 100, 6)),
            ),
        );
        assert_eq!(
            spoken(&effects),
            vec![said.clone(), message(Message::SelectionRemoved)],
            "{motion:?} with the caret at {caret} and the selection from {start}"
        );
    }

    // With nothing before the start, "blank" (`textbox-word-backspace.txt`).
    let mut state = editing(text, 0);
    let first = TextChunk {
        first: true,
        ..line(text, 100, 0)
    };
    let _ = selection_event(
        &mut state,
        (first, Some(selection_from(0, true))),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            selection_removed_reply(line("beta gamma\r\n", 100, 0)),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![message(Message::Blank), message(Message::SelectionRemoved)]
    );
}

#[test]
fn backspace_over_a_selection_from_an_earlier_line_says_what_was_before_its_start() {
    let text = "alpha beta gamma\r\n";
    let space = character(" ");
    let beta = UtteranceSegment::text("beta".to_owned());
    // A selection that starts on an earlier line than the caret's, as
    // Notepad's Shift+End over "gamma" leaves it, the caret at the next
    // line's start: what comes before its start, in the line the outpost
    // reported with it (`notepad-third-word-to-end-backspace.txt`,
    // `notepad-third-word-to-end-control-backspace.txt`).
    for (motion, said) in [
        (CaretMotion::Backspace, &space),
        (CaretMotion::BackspaceWord, &beta),
    ] {
        let mut state = editing(text, 0);
        let caret = CaretReport {
            line: line(text, 100, 0),
            selection: Some(selection_starting_on(
                Selection {
                    start: TextPosition {
                        anchor: TextAnchor(300),
                        offset: 11,
                    },
                    end: TextPosition {
                        anchor: TextAnchor(201),
                        offset: 0,
                    },
                },
                Some(Box::new(line(text, 300, 11))),
            )),
            read_at_us: stamp(),
        };
        set_baseline(&caret);
        let _ = reduce(
            &mut state,
            &event(NormalizedEvent::CaretMoved {
                node_id: id(5),
                caret,
                selection_changes: Vec::new(),
                content_changed: false,
                new_selection: None,
                facts: None,
            }),
        );
        let effects = reduce(&mut state, &key(motion, false));
        let effects = reduce(
            &mut state,
            &completed(
                request_of(&effects),
                selection_removed_reply(line(text, 100, 0)),
            ),
        );
        assert_eq!(
            spoken(&effects),
            vec![said.clone(), message(Message::SelectionRemoved)],
            "{motion:?} over a selection from an earlier line"
        );
    }

    // A start in no line reported with it has nothing known before it:
    // only the selection's change is said.
    let mut state = editing(text, 0);
    let _ = selection_event(
        &mut state,
        (line(text, 100, 6), Some(selection_from(3, false))),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            selection_removed_reply(line("gamma\r\n", 100, 0)),
        ),
    );
    assert_eq!(spoken(&effects), vec![message(Message::SelectionRemoved)]);
}

#[test]
fn a_caret_reports_selection_change_while_a_key_waits_is_said_after_the_keys_words() {
    let text = "alpha beta gamma\r\n";
    let removed = vec![SelectionChange {
        selected: false,
        text: String::new(),
        characters: 0,
    }];
    let selected = Some(Selection {
        start: TextPosition {
            anchor: TextAnchor(200),
            offset: 0,
        },
        end: TextPosition {
            anchor: TextAnchor(201),
            offset: 0,
        },
    });
    // The key's caret event reached the outpost before its watch, so its
    // report compared the selection and the answer finds nothing more.
    let mut state = editing(text, 0);
    let _ = selection_event(
        &mut state,
        (line(text, 100, 6), selected),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::Delete, false));
    let asked = request_of(&effects);
    let heard = selection_event(
        &mut state,
        (line("alpha gamma\r\n", 100, 6), None),
        (removed.clone(), true),
    );
    assert_eq!(spoken(&heard), []);
    let effects = reduce(
        &mut state,
        &completed(
            asked,
            caret_reply(true, line("alpha gamma\r\n", 100, 6), None),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![character("g"), message(Message::SelectionRemoved)]
    );

    // A watch that ends with no evidence still says the report's change.
    let mut state = editing(text, 0);
    let _ = selection_event(
        &mut state,
        (line(text, 100, 6), selected),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let asked = request_of(&effects);
    let _ = selection_event(
        &mut state,
        (line("alpha gamma\r\n", 100, 6), None),
        (removed.clone(), true),
    );
    let _ = asked;
    let effects = bound_unanswered(&mut state);
    assert_eq!(spoken(&effects), vec![message(Message::SelectionRemoved)]);
}

/// A caret report's selection change held while a key waited is said
/// when the key's answer says the caret did not move, and when the answer
/// was read once a later key was pressed, which is not this key's to
/// speak.
#[test]
fn a_caret_reports_held_change_is_said_with_an_answer_not_spoken() {
    let text = "alpha beta gamma\r\n";
    let after = "alpha gamma\r\n";
    let removed = vec![SelectionChange {
        selected: false,
        text: String::new(),
        characters: 0,
    }];
    let selected = Some(Selection {
        start: TextPosition {
            anchor: TextAnchor(200),
            offset: 0,
        },
        end: TextPosition {
            anchor: TextAnchor(201),
            offset: 0,
        },
    });
    for later in [false, true] {
        let mut state = editing(text, 0);
        let _ = selection_event(
            &mut state,
            (line(text, 100, 6), selected),
            (Vec::new(), false),
        );
        let effects = reduce(&mut state, &key(CaretMotion::Other, false));
        let asked = request_of(&effects);
        let _ = selection_event(
            &mut state,
            (line(after, 100, 6), None),
            (removed.clone(), true),
        );
        if later {
            let _ = reduce(
                &mut state,
                &Input::SpeechCancelled {
                    at_us: (50) * 1_000,
                },
            );
        }
        let effects = if later {
            reduce(
                &mut state,
                &completed(asked, caret_reply_at(line(after, 100, 6), 60)),
            )
        } else {
            bound_answer(&mut state, line(after, 100, 6))
        };
        assert_eq!(
            spoken(&effects),
            vec![message(Message::SelectionRemoved)],
            "{}",
            if later {
                "an answer read after a later key"
            } else {
                "an answer that did not move"
            }
        );
    }
}

#[test]
fn a_newer_key_or_a_focus_change_supersedes_a_waiting_key() {
    let mut state = editing("abc\n", 0);
    let first = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let second = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    // A read that ended after the later key was passed on and shows the first
    // key's move is the later key's evidence: its landing is said, and the
    // later key's own move goes unsaid (Dickson, 2026-11-09).
    let effects = reduce(
        &mut state,
        &completed(first, caret_reply(true, line("abc\n", 100, 1), None)),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let effects = reduce(
        &mut state,
        &completed(second, caret_reply(true, line("abc\n", 100, 2), None)),
    );
    assert!(effects.is_empty(), "the focus moved on: {effects:?}");
}

#[test]
fn caret_keys_are_watched_in_a_terminal_and_not_at_all_without_text() {
    let mut state = SrState::new();
    focus(&mut state, node(7, Role::Terminal, StateSet::new()));

    let mut state = SrState::new();
    focus(&mut state, node(8, Role::Button, StateSet::new()));
    assert_eq!(
        reduce(&mut state, &key(CaretMotion::PreviousLine, false)),
        []
    );
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(8),
        text: text.to_owned(),
        at_ms: 0,
    }
}

#[test]
fn typed_characters_are_echoed_and_a_protected_field_echoes_stars() {
    let mut state = editing("", 0);
    assert_eq!(
        spoken(&reduce(&mut state, &typed("a"))),
        vec![character("a")]
    );
    // Word echo is off by default.
    assert_eq!(
        spoken(&reduce(&mut state, &typed(" "))),
        vec![character(" ")]
    );

    let mut state = SrState::new();
    focus(
        &mut state,
        node(
            9,
            Role::EditableText,
            StateSet::new().with(State::Protected),
        ),
    );
    assert_eq!(
        spoken(&reduce(&mut state, &typed("p"))),
        vec![character("*")]
    );
}

#[test]
fn word_echo_speaks_a_word_when_it_ends() {
    let mut state = editing("", 0);
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_typed_characters: TypingEcho::Off,
            speak_typed_words: TypingEcho::Always,
            ..ReaderSettings::default()
        }),
    );
    for letter in ["h", "i"] {
        assert_eq!(spoken(&reduce(&mut state, &typed(letter))), []);
    }
    assert_eq!(
        spoken(&reduce(&mut state, &typed(" "))),
        vec![UtteranceSegment::text("hi")]
    );
}

/// Combining marks and the zero-width non-joiner, each typed as a key of
/// its own, continue the word: a Devanagari conjunct (ka, virama, ssa), a
/// Thai syllable with a tone mark (ko kai, mai ek, sara aa), and Persian
/// with a zero-width non-joiner (mim, yeh, the non-joiner, khah) are each
/// spoken as one word when a space ends it.
#[test]
fn word_echo_keeps_combining_marks_and_joiners_in_the_word() {
    let mut state = editing("", 0);
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_typed_characters: TypingEcho::Off,
            speak_typed_words: TypingEcho::Always,
            ..ReaderSettings::default()
        }),
    );
    for (keys, word) in [
        (&["क", "\u{94D}", "ष"][..], "क्ष"),
        (&["ก", "\u{E48}", "า"][..], "ก่า"),
        (&["م", "ی", "\u{200C}", "خ"][..], "می\u{200C}خ"),
    ] {
        for key in keys {
            assert_eq!(spoken(&reduce(&mut state, &typed(key))), [], "{word}");
        }
        assert_eq!(
            spoken(&reduce(&mut state, &typed(" "))),
            vec![UtteranceSegment::text(word)]
        );
    }
}

/// The word being typed goes on past a key that selects and a key outside
/// the caret table, and starts again after a caret movement without Shift
/// (`docs/parity/input.md`, "Typed-character echo").
#[test]
fn word_echo_forgets_the_word_only_when_the_caret_moves_without_shift() {
    let mut state = editing("", 0);
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_typed_characters: TypingEcho::Off,
            speak_typed_words: TypingEcho::Always,
            ..ReaderSettings::default()
        }),
    );
    let type_word = |state: &mut SrState, between: Input| {
        for letter in ["h", "e", "l"] {
            let _ = reduce(state, &typed(letter));
        }
        let _ = reduce(state, &between);
        for letter in ["l", "o"] {
            let _ = reduce(state, &typed(letter));
        }
        spoken(&reduce(state, &typed(" ")))
    };
    let other = key(CaretMotion::Other, false);
    assert_eq!(
        type_word(&mut state, other),
        vec![UtteranceSegment::text("hello")]
    );
    let selecting = key(CaretMotion::PreviousCharacter, true);
    assert_eq!(
        type_word(&mut state, selecting),
        vec![UtteranceSegment::text("hello")]
    );
    let moving = key(CaretMotion::PreviousCharacter, false);
    assert_eq!(
        type_word(&mut state, moving),
        vec![UtteranceSegment::text("lo")]
    );
}

#[test]
fn control_characters_end_a_word_but_are_never_spelled() {
    let mut state = editing("", 0);
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_typed_words: TypingEcho::Always,
            ..ReaderSettings::default()
        }),
    );
    assert_eq!(
        spoken(&reduce(&mut state, &typed("a"))),
        vec![character("a")]
    );
    // Tab ends the word, which is spoken, and is not spelled itself.
    assert_eq!(
        spoken(&reduce(&mut state, &typed("\t"))),
        vec![UtteranceSegment::text("a")]
    );
    assert_eq!(spoken(&reduce(&mut state, &typed("\r"))), []);
}

#[test]
fn echo_only_in_edit_controls_stays_quiet_elsewhere() {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_typed_characters: TypingEcho::EditControls,
            ..ReaderSettings::default()
        }),
    );
    focus(&mut state, node(10, Role::Button, StateSet::new()));
    assert_eq!(spoken(&reduce(&mut state, &typed("a"))), []);
    focus(&mut state, node(11, Role::EditableText, StateSet::new()));
    assert_eq!(
        spoken(&reduce(&mut state, &typed("a"))),
        vec![character("a")]
    );
}

#[test]
fn a_terminals_typing_waits_for_its_reading_rules_to_show_it() {
    let mut state = SrState::new();
    focus(&mut state, node(12, Role::Terminal, StateSet::new()));
    // Its echo comes from the reading rules, once the terminal shows it
    // (`tests/terminal.rs`); a text change alone says nothing.
    assert_eq!(spoken(&reduce(&mut state, &typed("l"))), []);
    let changed = event(NormalizedEvent::TextChanged { node_id: id(12) });
    assert_eq!(spoken(&reduce(&mut state, &changed)), []);

    // With terminal passwords spoken, typing is echoed at once.
    let _ = reduce(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_terminal_passwords: true,
            ..ReaderSettings::default()
        }),
    );
    assert_eq!(
        spoken(&reduce(&mut state, &typed("x"))),
        vec![character("x")]
    );
}

#[test]
fn the_review_cursor_follows_the_caret_and_keeps_its_column_across_lines() {
    let mut state = editing("abcdef\n", 4);
    // Following the caret: the line at the caret is read afresh.
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    let effects = read_at_caret(&mut state, &effects, line("abcdef\n", 100, 4));
    assert_eq!(spoken(&effects), vec![character("e")]);

    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextLine, 0));
    let request = request(&effects);
    assert_eq!(
        request.op,
        TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition {
                anchor: TextAnchor(100),
                offset: 0
            }),
            movement: Some(TextMovement {
                unit: TextUnit::Line,
                count: 1
            }),
            unit: TextUnit::Line,
        })
    );
    // A shorter line puts the cursor on its last character.
    let effects = reduce(
        &mut state,
        &completed(
            request.query_id,
            TextReply::Read {
                moved: 1,
                chunk: line("xy\n", 101, 0),
            },
        ),
    );
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("xy")]);
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("y")]);

    // The next longer line returns to the remembered column.
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextLine, 0));
    let _ = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Read {
                moved: 1,
                chunk: line("012345678\n", 102, 0),
            },
        ),
    );
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("4")]);
}

#[test]
fn the_review_cursor_meets_each_character_of_a_line_break() {
    // A standard edit control's line: the end of the line is its line
    // feed, and moving by character crosses the carriage return first.
    let mut state = reviewing("ab\r\n", 1);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewEndOfLine, 0));
    assert_eq!(spoken(&effects), vec![character("\n")]);
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewPreviousCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("\r")]);
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewPreviousCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(spoken(&effects), vec![character("\r")]);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(spoken(&effects), vec![character("\n")]);
    // The line feed is the line's last character.
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(
        spoken(&effects),
        vec![message(Message::Right), character("\n")]
    );

    // Windows 11 Notepad's line ends with a carriage return alone, and the
    // text's last line, with no break, ends on its last character.
    let mut state = reviewing("ab\r", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewEndOfLine, 0));
    assert_eq!(spoken(&effects), vec![character("\r")]);
    let mut state = reviewing("ab", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewEndOfLine, 0));
    assert_eq!(spoken(&effects), vec![character("b")]);
}

/// A terminal row's line ends at its last character: padding is never
/// reviewed (`books/dev/src/explanation/design/terminal-model.md`, "Features and settings"), so moving on
/// from the last character is at the edge, padded or not.
#[test]
fn a_terminal_rows_line_ends_at_its_last_character() {
    for row in [
        "ab",
        "ab        
",
    ] {
        let mut state = SrState::new();
        focus(&mut state, node(15, Role::Terminal, StateSet::new()));
        caret_event(&mut state, 15, line(row, 400, 1));
        settle(&mut state, line(row, 400, 1));
        let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
        let read = request(&effects);
        let effects = reduce(
            &mut state,
            &completed(
                read.query_id,
                TextReply::Read {
                    moved: 0,
                    chunk: line(row, 400, 1),
                },
            ),
        );
        assert_eq!(
            spoken(&effects),
            vec![message(Message::Right), character("b")],
            "{row:?}"
        );
    }
}

#[test]
fn a_terminal_keeps_a_cell_column_and_reads_past_the_text_as_blank() {
    let mut state = SrState::new();
    focus(&mut state, node(13, Role::Terminal, StateSet::new()));
    // The caret on 中 at cells 2 and 3 of a padded row.
    caret_event(&mut state, 13, line("ab中文cd    \r\n", 200, 2));
    settle(&mut state, line("ab中文cd    \r\n", 200, 2));
    // Every review command in a terminal reads the review cursor's line
    // again first, where the cursor is (`books/dev/src/explanation/design/terminal-model.md`,
    // "The review cursor"); a line read the same keeps its cell column.
    let seeded = |state: &mut SrState, command: ReviewCommand, chunk: TextChunk| {
        let effects = reduce(state, &self::command(command, 0));
        let read = request(&effects);
        let TextOp::Read(TextRead {
            at, movement: None, ..
        }) = read.op
        else {
            panic!("a read of the review cursor's line, not {:?}", read.op);
        };
        assert_eq!(
            at,
            TextPoint::At(TextPosition {
                anchor: chunk.start,
                offset: chunk.offset,
            })
        );
        reduce(
            state,
            &completed(read.query_id, TextReply::Read { moved: 0, chunk }),
        )
    };
    let effects = seeded(
        &mut state,
        ReviewCommand::ReviewNextCharacter,
        line("ab中文cd    \r\n", 200, 2),
    );
    assert_eq!(spoken(&effects), vec![character("文")]);
    let effects = seeded(
        &mut state,
        ReviewCommand::ReviewNextLine,
        line("ab中文cd    \r\n", 200, 5),
    );
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Read {
                moved: 1,
                chunk: line("xy\r\n", 201, 0),
            },
        ),
    );
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("xy")]);
    // Cell 4 is past "xy": a blank cell, and the column is kept exactly.
    let effects = seeded(
        &mut state,
        ReviewCommand::ReviewCurrentCharacter,
        line("xy\r\n", 201, 2),
    );
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
    let effects = seeded(
        &mut state,
        ReviewCommand::ReviewPreviousLine,
        line("xy\r\n", 201, 2),
    );
    let _ = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Read {
                moved: -1,
                chunk: line("ab中文cd    \r\n", 200, 0),
            },
        ),
    );
    let effects = seeded(
        &mut state,
        ReviewCommand::ReviewCurrentCharacter,
        line("ab中文cd    \r\n", 200, 5),
    );
    assert_eq!(spoken(&effects), vec![character("文")]);
}

#[test]
fn review_edges_are_known_from_the_line_or_from_a_movement_that_did_not_move() {
    let mut state = SrState::new();
    focus(&mut state, node(14, Role::EditableText, StateSet::new()));
    caret_event(
        &mut state,
        14,
        TextChunk {
            first: true,
            ..line("only\n", 300, 0)
        },
    );
    settle(
        &mut state,
        TextChunk {
            first: true,
            ..line("only\n", 300, 0)
        },
    );
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousLine, 0));
    assert_eq!(
        spoken(&effects),
        vec![message(Message::Top), UtteranceSegment::text("only")]
    );
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextLine, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Read {
                moved: 0,
                chunk: line("only\n", 300, 0),
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![message(Message::Bottom), UtteranceSegment::text("only")]
    );
}

#[test]
fn repeated_presses_describe_and_spell_with_descriptions() {
    let mut state = reviewing("ab\n", 0);
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 1),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::CharacterDescription(
            "a".to_owned()
        ))]
    );
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewCurrentLine, 2));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::CharacterDescription("a".to_owned())),
            UtteranceSegment::new(SegmentContent::CharacterDescription("b".to_owned())),
        ]
    );
}

#[test]
fn a_navigator_without_a_caret_reads_its_line_first_and_falls_back_to_flat_text() {
    let mut state = SrState::new();
    let mut edit = node(15, Role::EditableText, StateSet::new());
    edit.value = Some("flat value".to_owned());
    focus(&mut state, edit);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewCurrentLine, 0));
    let request = request(&effects);
    assert_eq!(
        request.op,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        })
    );
    let effects = reduce(&mut state, &completed(request.query_id, TextReply::NoText));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("flat value")]);
}

#[test]
fn select_then_copy_selects_from_the_start_marker_says_so_copies_and_clears_the_marker() {
    let mut state = reviewing("hello world\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
    let effects = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    assert_eq!(spoken(&effects), vec![message(Message::StartMarked)]);
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));

    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    // The marker on "h" and the review cursor on "w", each with the point
    // just past its character, for the range from the earlier through the
    // character at the later.
    let start = at(0);
    let end = at(6);
    let through = Some(TextThrough {
        start: at(1),
        end: at(7),
    });
    let select = request(&effects);
    assert_eq!(
        select.op,
        TextOp::Select {
            start,
            end,
            through
        }
    );
    // The selection made is spoken as a selecting key's is.
    let effects = reduce(
        &mut state,
        &completed(
            select.query_id,
            TextReply::Selected(vec![SelectionChange {
                selected: true,
                text: "hello w".to_owned(),
                characters: 7,
            }]),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("hello w".to_owned()))
        ))]
    );
    // The caret the selection moved takes the review cursor to the range's
    // start; the second press still copies the range selected.
    caret_event(
        &mut state,
        5,
        line(
            "hello world
",
            101,
            0,
        ),
    );
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    let effects = read_at_caret(
        &mut state,
        &effects,
        line(
            "hello world
",
            101,
            0,
        ),
    );
    let request = request(&effects);
    assert_eq!(
        request.op,
        TextOp::ReadRange {
            start,
            end,
            through
        }
    );
    let effects = reduce(
        &mut state,
        &completed(
            request.query_id,
            TextReply::Range {
                text: "hello w".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("hello w".to_owned())]);
    // The copy cleared the start marker: a further press has none.
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
}

/// Whether `effects` ask the outpost for anything.
fn requests_text(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Text(_)))
}

/// A start marker on "h" of "hello world", the review cursor moved to
/// "w", and select then copy's first press made, its selection of "hello
/// w" answered; returns the range it selected.
fn selected_from_the_marker(state: &mut SrState) -> TextOp {
    let _ = reduce(state, &command(ReviewCommand::SetStartMarker, 0));
    let _ = reduce(state, &command(ReviewCommand::ReviewNextWord, 0));
    let effects = reduce(state, &command(ReviewCommand::SelectThenCopy, 0));
    let select = request(&effects);
    let _ = reduce(
        state,
        &completed(
            select.query_id,
            TextReply::Selected(vec![SelectionChange {
                selected: true,
                text: "hello w".to_owned(),
                characters: 7,
            }]),
        ),
    );
    select.op
}

#[test]
fn a_first_press_after_a_selection_not_yet_copied_asks_for_the_second() {
    // NVDA keeps the range a first press selected: a further first press
    // says so and selects nothing, and the second press copies that range
    // (`docs/parity/review.md`, "Select then copy from the review cursor").
    let mut state = reviewing("hello world\n", 0);
    let TextOp::Select {
        start,
        end,
        through,
    } = selected_from_the_marker(&mut state)
    else {
        panic!("the first press selects");
    };
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert!(!requests_text(&effects));
    assert_eq!(spoken(&effects), vec![message(Message::PressTwiceToCopy)]);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    assert_eq!(
        request(&effects).op,
        TextOp::ReadRange {
            start,
            end,
            through
        }
    );
}

#[test]
fn a_copy_keeps_a_start_marker_set_while_its_read_was_in_flight() {
    // The copy clears the marker it copied from, not one set after the
    // second press and before its read was answered
    // (`docs/parity/review.md`, "Select then copy from the review cursor").
    let mut state = reviewing(
        "hello world
",
        0,
    );
    let _ = selected_from_the_marker(&mut state);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    let copy = request(&effects);
    let effects = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    assert_eq!(spoken(&effects), vec![message(Message::StartMarked)]);
    let effects = reduce(
        &mut state,
        &completed(
            copy.query_id,
            TextReply::Range {
                text: "hello w".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("hello w".to_owned())]);
    // The new marker, on "w", is kept: a first press selects from it.
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(
        request(&effects).op,
        TextOp::Select {
            start: at(6),
            end: at(6),
            through: Some(TextThrough {
                start: at(7),
                end: at(7),
            }),
        }
    );
}

#[test]
fn a_copy_of_nothing_clears_the_start_marker_and_says_nothing() {
    let mut state = reviewing("hello world\n", 0);
    let _ = selected_from_the_marker(&mut state);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    let effects = reduce(
        &mut state,
        &completed(
            request(&effects).query_id,
            TextReply::Range {
                text: String::new(),
                truncated: false,
            },
        ),
    );
    assert_eq!(effects, Vec::new());
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
}

#[test]
fn a_copy_the_text_cannot_answer_clears_the_start_marker() {
    let mut state = reviewing("hello world\n", 0);
    let _ = selected_from_the_marker(&mut state);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    let effects = reduce(
        &mut state,
        &completed(request(&effects).query_id, TextReply::NoText),
    );
    assert_eq!(spoken(&effects), vec![message(Message::NotSupported)]);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
}

#[test]
fn the_start_marker_in_another_object_is_no_start_marker_and_is_kept() {
    // NVDA says "No start marker set" with the navigator elsewhere, and
    // the marker is there again when it returns.
    let mut state = reviewing("hello world\n", 0);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert!(!requests_text(&effects));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    let effects = read_at_caret(&mut state, &effects, line("hello world\n", 100, 0));
    assert_eq!(
        request(&effects).op,
        TextOp::Select {
            start: at(0),
            end: at(0),
            through: Some(TextThrough {
                start: at(1),
                end: at(1),
            }),
        }
    );
}

#[test]
fn moving_to_the_start_marker_in_another_object_is_no_start_marker_and_keeps_it() {
    // Verbatim+Shift+F9 with the navigator elsewhere says "No start marker
    // set" and asks for no text; back in the text, the marker on "w" is
    // there to move to, though the caret is now on "h".
    let mut state = reviewing("hello world\n", 6);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::MoveToStartMarker, 0));
    assert!(!requests_text(&effects));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::MoveToStartMarker, 0));
    let effects = read_at_caret(&mut state, &effects, line("hello world\n", 100, 0));
    assert_eq!(
        request(&effects).op,
        TextOp::Read(TextRead {
            at: at(6),
            movement: None,
            unit: TextUnit::Line,
        })
    );
}

/// The point `offset` bytes into the line [`reviewing`] reads, anchor 100.
fn at(offset: u32) -> TextPoint {
    TextPoint::At(TextPosition {
        anchor: TextAnchor(100),
        offset,
    })
}

#[test]
fn select_then_copy_back_from_the_marker_runs_from_the_cursor_through_the_marker() {
    // The marker on "w", the review cursor moved back to "h": the range
    // runs from the cursor through the character at the marker, both
    // ends' characters included (`docs/parity/review.md`, "Select then
    // copy from the review cursor"). The outpost, which alone can tell
    // which end comes first, is sent both ends and the points just past
    // their characters.
    let mut state = reviewing("hello world\n", 6);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    let through = Some(TextThrough {
        start: at(7),
        end: at(1),
    });
    let select = request(&effects);
    assert_eq!(
        select.op,
        TextOp::Select {
            start: at(6),
            end: at(0),
            through
        }
    );
    // The text cannot be selected: said as NVDA says it, and the second
    // press copies the same range.
    let effects = reduce(
        &mut state,
        &completed(select.query_id, TextReply::Unsupported),
    );
    assert_eq!(spoken(&effects), vec![message(Message::CannotSelect)]);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    assert_eq!(
        request(&effects).op,
        TextOp::ReadRange {
            start: at(6),
            end: at(0),
            through
        }
    );
}

#[test]
fn the_follow_caret_toggle_is_spoken_saved_and_obeyed() {
    let mut state = reviewing("abc\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ToggleFollowCaret, 0));
    assert_eq!(
        spoken(&effects),
        vec![message(Message::CaretDoesNotMoveReview)]
    );
    assert!(effects.contains(&Effect::SettingsChanged(ReaderSettings {
        follow_caret: false,
        ..ReaderSettings::default()
    })));
    caret_event(&mut state, 5, line("xyz\n", 101, 2));
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(
        spoken(&effects),
        vec![character("a")],
        "the review stayed put"
    );

    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ToggleTypedCharacters, 0),
    );
    // From the default, only in edit controls, the next setting is always.
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::SpeakTypedCharacters(TypingEcho::Always)
        ))]
    );
}

#[test]
fn a_unit_the_text_does_not_have_is_not_supported() {
    let mut state = editing("abc\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextPage, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::UnsupportedUnit(TextUnit::Page),
        ),
    );
    assert_eq!(spoken(&effects), vec![message(Message::NotSupported)]);
}

#[test]
fn the_next_word_past_the_line_moves_to_the_next_line_first_word() {
    let mut state = reviewing("last\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Read {
                moved: 1,
                chunk: line("  next one\n", 101, 0),
            },
        ),
    );
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("next")]);
}

/// The one effect of `effects`, a text request.
fn only_request(effects: &[Effect]) -> TextRequest {
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
    request(effects)
}

/// One queued utterance of `segments`, traced `trace`: 3 for a command's
/// own speech, 4 for speech an outpost's answer brought.
fn said(trace: u64, segments: Vec<UtteranceSegment>) -> Vec<Effect> {
    vec![Effect::Speak(
        Utterance {
            trace_id: TraceId::from_raw(trace),
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// A read of the line at `at`, after moving by `movement` when given.
fn line_read(at: TextPoint, movement: Option<(TextUnit, i32)>) -> TextOp {
    TextOp::Read(TextRead {
        at,
        movement: movement.map(|(unit, count)| TextMovement { unit, count }),
        unit: TextUnit::Line,
    })
}

/// Checks that `effects` are one request for `op` and answers it with a
/// read of `chunk` that moved `moved` units.
fn answer_read(
    state: &mut SrState,
    effects: &[Effect],
    op: &TextOp,
    (moved, chunk): (i32, TextChunk),
) -> Vec<Effect> {
    let asked = only_request(effects);
    assert_eq!(&asked.op, op);
    reduce(
        state,
        &completed(asked.query_id, TextReply::Read { moved, chunk }),
    )
}

/// The start of the line anchored at `anchor`.
fn line_start(anchor: u64) -> TextPoint {
    TextPoint::At(TextPosition::at(TextAnchor(anchor)))
}

/// A terminal focused with its caret, and the review cursor settled, on
/// `row` at `offset`, anchored at 400.
fn reviewing_terminal(row: &str, offset: u32) -> SrState {
    let mut state = SrState::new();
    focus(&mut state, node(15, Role::Terminal, StateSet::new()));
    caret_event(&mut state, 15, line(row, 400, offset));
    settle(&mut state, line(row, 400, offset));
    state
}

#[test]
fn review_bottom_reads_the_last_line_from_its_start() {
    let mut state = reviewing("first\n", 3);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewBottom, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(TextPoint::End, None),
        (0, line("last line", 102, 9)),
    );
    assert_eq!(effects, said(4, vec![UtteranceSegment::text("last line")]));
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(effects, said(3, vec![character("l")]));
}

/// In a terminal, bottom first reads the review cursor's row again, as
/// every review command there does (`books/dev/src/explanation/design/terminal-model.md`, "The
/// review cursor"), then the text's last row.
#[test]
fn review_bottom_in_a_terminal_reads_the_last_row() {
    let mut state = reviewing_terminal("$ ls", 2);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewBottom, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(
            TextPoint::At(TextPosition {
                anchor: TextAnchor(400),
                offset: 2,
            }),
            None,
        ),
        (0, line("$ ls", 400, 2)),
    );
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(TextPoint::End, None),
        (0, line("done", 405, 4)),
    );
    assert_eq!(effects, said(4, vec![UtteranceSegment::text("done")]));
}

/// Page movement keeps the review cursor's column, as moving by line does
/// (`docs/parity/review.md`, "Review cursor columns"), and a page movement
/// that did not move says "Top" or "Bottom", as NVDA does, and leaves the
/// review cursor where it was. Each page here is one line, so the speech
/// is the whole page's, as NVDA's is; a page of several lines is read
/// differently from NVDA, which `docs/parity/review.md` records, and this
/// test asserts nothing about it.
#[test]
fn review_pages_keep_the_column_and_name_the_edges() {
    let mut state = reviewing("abcdef\n", 2);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextPage, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(line_start(100), Some((TextUnit::Page, 1))),
        (1, line("012345\n", 140, 0)),
    );
    assert_eq!(effects, said(4, vec![UtteranceSegment::text("012345")]));
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(effects, said(3, vec![character("2")]));

    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextPage, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(line_start(140), Some((TextUnit::Page, 1))),
        (0, line("012345\n", 140, 0)),
    );
    assert_eq!(
        effects,
        said(
            4,
            vec![message(Message::Bottom), UtteranceSegment::text("012345")]
        )
    );

    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousPage, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(line_start(140), Some((TextUnit::Page, -1))),
        (-1, line("abcdef\n", 100, 0)),
    );
    assert_eq!(effects, said(4, vec![UtteranceSegment::text("abcdef")]));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousPage, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(line_start(100), Some((TextUnit::Page, -1))),
        (0, line("abcdef\n", 100, 0)),
    );
    assert_eq!(
        effects,
        said(
            4,
            vec![message(Message::Top), UtteranceSegment::text("abcdef")]
        )
    );
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(effects, said(3, vec![character("c")]));
}

/// The selection's start is its first character, and its end the
/// character before the point past the selection, its last.
#[test]
fn the_review_cursor_moves_to_the_selections_first_and_last_characters() {
    let mut state = reviewing("hello world\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewSelectionStart, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(TextPoint::SelectionStart, None),
        (0, line("hello world\n", 100, 6)),
    );
    assert_eq!(effects, said(4, vec![character("w")]));

    let effects = reduce(&mut state, &command(ReviewCommand::ReviewSelectionEnd, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(TextPoint::SelectionEnd, None),
        (0, line("hello world\n", 100, 11)),
    );
    assert_eq!(effects, said(4, vec![character("d")]));
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(effects, said(3, vec![character("d")]));
}

/// Start of line moves to the line's first character and says it, with no
/// read: the review cursor holds its line.
#[test]
fn start_of_line_says_the_lines_first_character() {
    let mut state = reviewing("hello world\n", 8);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewStartOfLine, 0));
    assert_eq!(effects, said(3, vec![character("h")]));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(effects, said(3, vec![character("e")]));
}

/// In a terminal, start of line reads the review cursor's row again first
/// (`books/dev/src/explanation/design/terminal-model.md`, "The review cursor"), then says the
/// row's first cell.
#[test]
fn start_of_line_in_a_terminal_says_the_rows_first_cell() {
    let mut state = reviewing_terminal("ab cd", 3);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewStartOfLine, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(
            TextPoint::At(TextPosition {
                anchor: TextAnchor(400),
                offset: 3,
            }),
            None,
        ),
        (0, line("ab cd", 400, 3)),
    );
    assert_eq!(effects, said(4, vec![character("a")]));
}

/// Previous word reads the word before on the line; from the line's first
/// word it reads the line above and lands on its last word; on the first
/// line it says "Top" and the word again.
#[test]
fn previous_word_crosses_a_line_start_and_stops_at_the_top() {
    let mut state = reviewing("big world\n", 4);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    assert_eq!(effects, said(3, vec![UtteranceSegment::text("big")]));

    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(line_start(100), Some((TextUnit::Line, -1))),
        (
            -1,
            TextChunk {
                first: true,
                ..line("one two\n", 90, 0)
            },
        ),
    );
    assert_eq!(effects, said(4, vec![UtteranceSegment::text("two")]));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    assert_eq!(effects, said(3, vec![UtteranceSegment::text("one")]));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    assert_eq!(
        effects,
        said(
            3,
            vec![message(Message::Top), UtteranceSegment::text("one")]
        )
    );
}

/// The caret's location is asked of the focus's caret and said as NVDA
/// says it (`docs/parity/review.md`, "Review cursor line/word/character
/// over object text").
#[test]
fn caret_location_asks_for_the_carets_place_and_says_it() {
    let mut state = editing("abc\n", 1);
    let effects = reduce(&mut state, &command(ReviewCommand::ReportCaretLocation, 0));
    let asked = only_request(&effects);
    assert_eq!(asked.op, TextOp::Location(TextPoint::Caret));
    let effects = reduce(
        &mut state,
        &completed(asked.query_id, TextReply::Location { x: 108, y: 200 }),
    );
    assert_eq!(
        effects,
        said(
            4,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Positioned { x: 108, y: 200 }
            ))]
        )
    );
}

/// In text measured by character offsets the caret's location is said as
/// how far through the text it is, then its place on the screen.
#[test]
fn an_offset_measured_location_says_the_percentage_and_the_place() {
    let mut state = editing("abc\n", 1);
    let effects = reduce(&mut state, &command(ReviewCommand::ReportCaretLocation, 0));
    let asked = only_request(&effects);
    assert_eq!(asked.op, TextOp::Location(TextPoint::Caret));
    let reply = TextReply::OffsetLocation {
        offset: 1,
        length: 4,
        point: Some((108, 200)),
    };
    let effects = reduce(&mut state, &completed(asked.query_id, reply));
    assert_eq!(
        effects,
        said(
            4,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::OffsetPositioned {
                    offset: 1,
                    length: 4,
                    point: Some((108, 200)),
                }
            ))]
        )
    );
}

/// The review cursor's location is asked at the review cursor, not the
/// caret, and said the same way.
#[test]
fn review_location_asks_for_the_review_cursors_place_and_says_it() {
    let mut state = reviewing("hello world\n", 0);
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));
    let effects = reduce(&mut state, &command(ReviewCommand::ReportReviewLocation, 0));
    let asked = only_request(&effects);
    assert_eq!(asked.op, TextOp::Location(at(6)));
    let effects = reduce(
        &mut state,
        &completed(asked.query_id, TextReply::Location { x: 148, y: 200 }),
    );
    assert_eq!(
        effects,
        said(
            4,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Positioned { x: 148, y: 200 }
            ))]
        )
    );
}

/// Moving to a start marker in the same text reads the marker's line and
/// says the character at the marker, where the review cursor then is.
#[test]
fn moving_to_the_start_marker_in_the_same_text_says_its_character() {
    let mut state = reviewing("hello world\n", 6);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewPreviousWord, 0));
    let effects = reduce(&mut state, &command(ReviewCommand::MoveToStartMarker, 0));
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(at(6), None),
        (0, line("hello world\n", 100, 6)),
    );
    assert_eq!(effects, said(4, vec![character("w")]));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(effects, said(3, vec![character("o")]));
}

/// Verbatim+3 cycles word echo from its default, off, through only in edit
/// controls and always back to off, saying each and reporting the settings
/// to save.
#[test]
fn the_typed_words_toggle_cycles_says_and_saves_each_setting() {
    let mut state = editing("abc\n", 0);
    for echo in [
        TypingEcho::EditControls,
        TypingEcho::Always,
        TypingEcho::Off,
    ] {
        let effects = reduce(&mut state, &command(ReviewCommand::ToggleTypedWords, 0));
        let mut expected = said(
            3,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::SpeakTypedWords(echo),
            ))],
        );
        expected.push(Effect::SettingsChanged(ReaderSettings {
            speak_typed_words: echo,
            ..ReaderSettings::default()
        }));
        assert_eq!(effects, expected, "{echo:?}");
    }
}

/// Verbatim+6 pressed twice turns following the caret off and on again,
/// saying and saving each, and once on the review cursor follows the
/// caret again.
#[test]
fn the_follow_caret_toggle_turns_following_off_and_back_on() {
    let mut state = reviewing("abc\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ToggleFollowCaret, 0));
    let mut expected = said(3, vec![message(Message::CaretDoesNotMoveReview)]);
    expected.push(Effect::SettingsChanged(ReaderSettings {
        follow_caret: false,
        ..ReaderSettings::default()
    }));
    assert_eq!(effects, expected);
    let effects = reduce(&mut state, &command(ReviewCommand::ToggleFollowCaret, 0));
    let mut expected = said(3, vec![message(Message::CaretMovesReview)]);
    expected.push(Effect::SettingsChanged(ReaderSettings::default()));
    assert_eq!(effects, expected);

    caret_event(&mut state, 5, line("xyz\n", 101, 2));
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    let effects = answer_read(
        &mut state,
        &effects,
        &line_read(TextPoint::Caret, None),
        (0, line("xyz\n", 101, 2)),
    );
    assert_eq!(effects, said(4, vec![character("z")]));
}

/// The index mark and text of each say-all utterance among `effects`, each
/// checked to be marked as read by say-all, so the theme's "play sounds
/// during say all" setting applies to it. Any utterance of another shape
/// fails the test, so no speech is skipped unchecked.
fn say_all_pieces(effects: &[Effect]) -> Vec<(SpeechMark, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => match utterance.segments.as_slice() {
                [mark, text] => match (&mark.content, &text.content) {
                    (SegmentContent::Mark(mark), SegmentContent::Text(text)) => {
                        assert!(utterance.say_all, "{utterance:?} is read by say-all");
                        Some((*mark, text.clone()))
                    }
                    _ => panic!("say-all speech is a mark and its text: {utterance:?}"),
                },
                _ => panic!("say-all speech is a mark and its text: {utterance:?}"),
            },
            _ => None,
        })
        .collect()
}

/// Say-all's read ahead from `at`, after `movement`, by `unit`.
fn read_ahead(at: TextPoint, movement: Option<TextMovement>, unit: TextUnit) -> TextOp {
    TextOp::ReadAhead(TextReadAhead {
        at,
        movement,
        unit,
        count: 20,
    })
}

/// Playback reaching `mark`.
fn reached(mark: SpeechMark) -> Input {
    Input::MarkReached { mark }
}

#[test]
fn say_all_reads_by_line_where_there_are_no_sentences_and_moves_the_caret() {
    let mut state = editing("First.\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    assert!(effects.contains(&Effect::KeepDisplayOn(true)));
    let first = request(&effects);
    assert_eq!(
        first.op,
        read_ahead(TextPoint::Caret, None, TextUnit::Sentence)
    );
    // UIA has no sentences: reading goes by line.
    let effects = reduce(
        &mut state,
        &completed(
            first.query_id,
            TextReply::UnsupportedUnit(TextUnit::Sentence),
        ),
    );
    let by_line = request(&effects);
    assert_eq!(
        by_line.op,
        read_ahead(TextPoint::Caret, None, TextUnit::Line)
    );
    let effects = reduce(
        &mut state,
        &completed(
            by_line.query_id,
            TextReply::Chunks {
                moved: 0,
                chunks: vec![line("First.\n", 100, 0)],
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].1, "First.");
    // Little is left to speak, so the next batch is read at once, a line on
    // from the last line read.
    let next = request(&effects);
    assert_eq!(
        next.op,
        read_ahead(
            TextPoint::At(TextPosition::at(TextAnchor(100))),
            Some(TextMovement {
                unit: TextUnit::Line,
                count: 1
            }),
            TextUnit::Line,
        )
    );
    // Playback reaches the first line: the caret moves there.
    let effects = reduce(&mut state, &reached(pieces[0].0));
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(100))))
    );
    // The document ends: say-all is over, and the display may sleep.
    let effects = reduce(
        &mut state,
        &completed(
            next.query_id,
            TextReply::Chunks {
                moved: 0,
                chunks: vec![line("First.\n", 100, 0)],
            },
        ),
    );
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
}

#[test]
fn say_all_hands_out_a_batch_a_piece_at_a_time_and_ends_after_the_last() {
    let mut state = editing("x\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::UnsupportedUnit(TextUnit::Sentence),
        ),
    );
    let last = TextChunk {
        last: true,
        ..line("Three.", 102, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Chunks {
                moved: 0,
                chunks: vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last],
            },
        ),
    );
    // Two pieces with speech, the third waiting; the text's end was read,
    // so nothing more is asked for.
    let pieces = say_all_pieces(&effects);
    let texts: Vec<&str> = pieces.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts, ["One.", "Two."]);
    assert_eq!(effects.len(), 2, "nothing but the two pieces: {effects:?}");
    // Reaching the first moves the caret there and hands on the third,
    // with its own mark.
    let effects = reduce(&mut state, &reached(pieces[0].0));
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(100))))
    );
    let third = say_all_pieces(&effects);
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].1, "Three.");
    assert_eq!(effects.len(), 2, "{effects:?}");
    // Reaching the second moves the caret on.
    let effects = reduce(&mut state, &reached(pieces[1].0));
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(101))))
    );
    assert_eq!(effects.len(), 1, "{effects:?}");
    // The last piece reached: the caret moves to it and say-all ends.
    let effects = reduce(&mut state, &reached(third[0].0));
    assert_eq!(
        effects,
        vec![
            Effect::Text(TextRequest {
                op: TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(102)))),
                ..request(&effects)
            }),
            Effect::KeepDisplayOn(false),
        ]
    );
}

#[test]
fn say_all_reads_the_next_batch_when_fewer_than_ten_pieces_are_left() {
    let mut state = editing("x\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::UnsupportedUnit(TextUnit::Sentence),
        ),
    );
    // Twelve lines to speak: two with speech, ten buffered, which is not
    // yet fewer than ten.
    let chunks: Vec<TextChunk> = (0..12)
        .map(|index| line("A b.\n", 100 + index, 0))
        .collect();
    let effects = reduce(
        &mut state,
        &completed(request_of(&effects), TextReply::Chunks { moved: 0, chunks }),
    );
    let reads = |effects: &[Effect]| {
        effects
            .iter()
            .filter(|effect| {
                matches!(effect, Effect::Text(request) if matches!(request.op, TextOp::ReadAhead(_)))
            })
            .count()
    };
    assert_eq!(reads(&effects), 0);
    let mut marks: Vec<SpeechMark> = say_all_pieces(&effects)
        .into_iter()
        .map(|(mark, _)| mark)
        .collect();
    let mut reach = |state: &mut SrState, index: usize| {
        let effects = reduce(state, &reached(marks[index]));
        marks.extend(say_all_pieces(&effects).into_iter().map(|(mark, _)| mark));
        reads(&effects)
    };
    // Each line reached leaves one fewer: eleven, then ten, then nine, when
    // the next batch is read, once.
    assert_eq!(reach(&mut state, 0), 0);
    assert_eq!(reach(&mut state, 1), 0);
    assert_eq!(reach(&mut state, 2), 1);
    assert_eq!(reach(&mut state, 3), 0);
}

#[test]
fn say_all_splits_a_paragraph_into_sentences_and_stops_on_a_key() {
    let mut state = editing("x\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let paragraph = TextChunk {
        unit: TextUnit::Paragraph,
        ..line("One. Two? Three!\r\n", 400, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Chunks {
                moved: 0,
                chunks: vec![paragraph],
            },
        ),
    );
    // Two sentences with speech, the third waiting; so little is left that
    // the next paragraph is read at once, a paragraph on.
    let pieces = say_all_pieces(&effects);
    let texts: Vec<&str> = pieces.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts, ["One. ", "Two? "]);
    assert_eq!(
        request(&effects).op,
        read_ahead(
            TextPoint::At(TextPosition::at(TextAnchor(400))),
            Some(TextMovement {
                unit: TextUnit::Paragraph,
                count: 1
            }),
            TextUnit::Sentence,
        )
    );
    // Reaching the second sentence moves the caret there and hands on the
    // third.
    let effects = reduce(&mut state, &reached(pieces[1].0));
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition {
            anchor: TextAnchor(400),
            offset: 5
        }))
    );
    let third = say_all_pieces(&effects);
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].1, "Three!");
    assert_eq!(effects.len(), 2, "{effects:?}");
    // A key stops it, dropping what was waiting; a late mark is ignored.
    let effects = reduce(&mut state, &Input::SpeechCancelled { at_us: 0 });
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    assert_eq!(reduce(&mut state, &reached(third[0].0)), vec![]);
}

/// Speech the reducer did not cut off itself cuts say-all's utterances
/// off: once the pipeline reports them, say-all stops, as on a key, rather
/// than waiting for marks playback will never reach.
#[test]
fn say_all_stops_when_its_utterance_is_cut_off_elsewhere() {
    let mut state = editing(
        "x
", 0,
    );
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Chunks {
                moved: 0,
                chunks: vec![
                    line(
                        "One.
", 100, 0,
                    ),
                    line(
                        "Two.
", 101, 0,
                    ),
                ],
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    // An utterance with no mark of say-all's changes nothing.
    assert_eq!(
        reduce(
            &mut state,
            &Input::UtteranceCancelled {
                marks: vec![SpeechMark(9_999)],
                at_ms: 0
            }
        ),
        vec![]
    );
    let effects = reduce(
        &mut state,
        &Input::UtteranceCancelled {
            marks: vec![pieces[0].0],
            at_ms: 0,
        },
    );
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    // Its other utterance's report, and a late mark, find nothing waiting.
    assert_eq!(
        reduce(
            &mut state,
            &Input::UtteranceCancelled {
                marks: vec![pieces[1].0],
                at_ms: 0
            }
        ),
        vec![]
    );
    assert_eq!(reduce(&mut state, &reached(pieces[1].0)), vec![]);
}

/// An utterance the synthesizer failed on counts as heard: say-all carries
/// on past it, as NVDA's speech manager does when a later index arrives,
/// leaving the caret where the last heard unit put it, and moves it again
/// when playback reaches the next utterance.
#[test]
fn say_all_carries_on_past_an_utterance_that_failed() {
    let mut state = editing(
        "x
", 0,
    );
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Chunks {
                moved: 0,
                chunks: vec![
                    line(
                        "One.
", 100, 0,
                    ),
                    line(
                        "Two.
", 101, 0,
                    ),
                    line(
                        "Three.
", 102, 0,
                    ),
                ],
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    // The first fails: nothing moves, and the third is handed on.
    let effects = reduce(
        &mut state,
        &Input::UtteranceFailed {
            marks: vec![pieces[0].0],
        },
    );
    let third = say_all_pieces(&effects);
    assert_eq!(third.len(), 1, "{effects:?}");
    assert_eq!(third[0].1, "Three.");
    assert_eq!(effects.len(), 1, "{effects:?}");
    // Playback reaches the second: the caret moves there.
    let effects = reduce(&mut state, &reached(pieces[1].0));
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(101))))
    );
}

#[test]
fn a_caret_key_after_say_all_starts_from_where_say_all_moved_the_caret() {
    // From `notepad_say_all`: Notepad last reported the caret on the first
    // line, say-all moves it to the second as playback reaches it, and a
    // key stops say-all; Notepad's report of the move was observed in the
    // key's millisecond, so it may show the key's effect. The key's watch
    // starts from the second line's start, where say-all put the caret, as
    // NVDA's say-all sets the caret before a key reads it.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("One.\n", 100, 0), KEY_PRESSED_AT - 1_000);
    let effects = say_all_by_line(
        &mut state,
        vec![line("One.\n", 100, 0), line("Two.\n", 101, 0)],
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 2);
    let _ = reduce(&mut state, &reached(pieces[0].0));
    let effects = reduce(&mut state, &reached(pieces[1].0));
    assert_eq!(
        caret_move(&effects),
        Some(TextPoint::At(TextPosition::at(TextAnchor(101))))
    );
    let _ = reduce(
        &mut state,
        &Input::SpeechCancelled {
            at_us: (KEY_PRESSED_AT - 2) * 1_000,
        },
    );
    let _ = caret_event_at(&mut state, line("Two.\n", 102, 0), KEY_PRESSED_AT);
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, false));
    assert!(matches!(request(&effects).op, TextOp::ReadCaret(_)));
}

#[test]
fn say_all_from_the_review_cursor_leaves_the_review_cursor_where_it_stopped() {
    let mut state = reviewing("one\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromReview, 0));
    let first = request(&effects);
    assert_eq!(
        first.op,
        read_ahead(
            TextPoint::At(TextPosition::at(TextAnchor(100))),
            None,
            TextUnit::Sentence,
        )
    );
    let effects = reduce(
        &mut state,
        &completed(
            first.query_id,
            TextReply::Chunks {
                moved: 0,
                chunks: vec![TextChunk {
                    unit: TextUnit::Paragraph,
                    ..line("A b. C d.\n", 500, 0)
                }],
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    let _ = reduce(&mut state, &reached(pieces[1].0));
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_us: 0 });
    // The next review command reads the line where reading stopped.
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewCurrentLine, 0));
    assert_eq!(
        request(&effects).op,
        TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition {
                anchor: TextAnchor(500),
                offset: 5
            }),
            movement: None,
            unit: TextUnit::Line,
        })
    );
}

/// One part of a say-all utterance: an index mark, or text.
#[derive(Debug, PartialEq)]
enum Said {
    Mark(SpeechMark),
    Text(String),
}

/// Every say-all utterance among `effects`, each as its marks and texts in
/// order, checked to be marked as read by say-all and to open with a mark.
/// Any other speech fails the test, so none is skipped unchecked.
fn say_all_utterances(effects: &[Effect]) -> Vec<Vec<Said>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => {
                assert!(utterance.say_all, "{utterance:?} is read by say-all");
                let said: Vec<Said> = utterance
                    .segments
                    .iter()
                    .map(|segment| match &segment.content {
                        SegmentContent::Mark(mark) => Said::Mark(*mark),
                        SegmentContent::Text(text) => Said::Text(text.clone()),
                        _ => panic!("say-all speech is marks and text: {utterance:?}"),
                    })
                    .collect();
                assert!(
                    matches!(said.first(), Some(Said::Mark(_))),
                    "an utterance opens with a mark: {said:?}"
                );
                Some(said)
            }
            _ => None,
        })
        .collect()
}

/// The texts of an utterance, without its marks.
fn texts_of(said: &[Said]) -> Vec<&str> {
    said.iter()
        .filter_map(|part| match part {
            Said::Text(text) => Some(text.as_str()),
            Said::Mark(_) => None,
        })
        .collect()
}

/// The marks of an utterance, in order.
fn marks_of(said: &[Said]) -> Vec<SpeechMark> {
    said.iter()
        .filter_map(|part| match part {
            Said::Mark(mark) => Some(*mark),
            Said::Text(_) => None,
        })
        .collect()
}

/// The caret move among `effects`, if there is one.
fn caret_move(effects: &[Effect]) -> Option<TextPoint> {
    let moves: Vec<TextPoint> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Text(TextRequest {
                op: TextOp::MoveCaret(point),
                ..
            }) => Some(*point),
            _ => None,
        })
        .collect();
    assert!(moves.len() <= 1, "one caret move at most: {effects:?}");
    moves.first().copied()
}

/// Starts say-all from the caret of a document with no sentence unit,
/// answering its first read with `chunks`, and returns the effects.
fn say_all_by_line(state: &mut SrState, chunks: Vec<TextChunk>) -> Vec<Effect> {
    let effects = reduce(state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        state,
        &completed(
            request_of(&effects),
            TextReply::UnsupportedUnit(TextUnit::Sentence),
        ),
    );
    reduce(
        state,
        &completed(request_of(&effects), TextReply::Chunks { moved: 0, chunks }),
    )
}

#[test]
fn say_all_splits_lines_at_their_last_sentence_end_and_joins_a_sentence_across_lines() {
    let mut state = editing("x\n", 0);
    let last = TextChunk {
        last: true,
        ..line("line.\n", 102, 0)
    };
    let effects = say_all_by_line(
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
    let utterances = say_all_utterances(&effects);
    assert_eq!(utterances.len(), 2, "{effects:?}");
    assert_eq!(effects.len(), 2, "{effects:?}");
    let [first, second] = [&utterances[0], &utterances[1]];
    assert!(matches!(first.as_slice(), [Said::Mark(_), Said::Text(_)]));
    assert_eq!(texts_of(first), ["Verbatim is written in Rust. "]);
    assert!(matches!(
        second.as_slice(),
        [Said::Mark(_), Said::Text(_), Said::Mark(_), Said::Text(_)]
    ));
    assert_eq!(
        texts_of(second),
        ["It is informed by NVDA ", "but not constrained by it. "]
    );

    // The first line reached: the caret moves to its start, and the last
    // utterance is handed on, the text held back at the end spoken with
    // the last line.
    let effects = reduce(&mut state, &reached(marks_of(first)[0]));
    assert_eq!(
        caret_move(&effects),
        Some(TextPoint::At(TextPosition::at(TextAnchor(100))))
    );
    let third = say_all_utterances(&effects);
    assert_eq!(third.len(), 1);
    assert_eq!(effects.len(), 2, "{effects:?}");
    assert!(matches!(
        third[0].as_slice(),
        [Said::Mark(_), Said::Text(_), Said::Mark(_), Said::Text(_)]
    ));
    assert_eq!(texts_of(&third[0]), ["A last ", "line."]);
    // The rest of the first line: the caret stays on the first line.
    let effects = reduce(&mut state, &reached(marks_of(second)[0]));
    assert_eq!(effects, vec![]);
    // The second line's text starts: the caret moves to its start.
    let effects = reduce(&mut state, &reached(marks_of(second)[1]));
    assert_eq!(
        caret_move(&effects),
        Some(TextPoint::At(TextPosition::at(TextAnchor(101))))
    );
    assert_eq!(effects.len(), 1, "{effects:?}");
    // The last utterance starts with the rest of the second line: the
    // caret stays there, and a key stops say-all with it there.
    let effects = reduce(&mut state, &reached(marks_of(&third[0])[0]));
    assert_eq!(effects, vec![]);
    let effects = reduce(&mut state, &Input::SpeechCancelled { at_us: 0 });
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    assert_eq!(reduce(&mut state, &reached(marks_of(&third[0])[1])), vec![]);
}

#[test]
fn say_all_speaks_ten_lines_with_no_sentence_end_together() {
    let mut state = editing("x\n", 0);
    // Twelve lines with no sentence end, the fifth blank, which says
    // nothing but counts among the ten.
    let mut chunks: Vec<TextChunk> = (0..12)
        .map(|index| {
            let text = if index == 4 {
                "\n".to_owned()
            } else {
                format!("word {index}\n")
            };
            line(&text, 100 + index, 0)
        })
        .collect();
    chunks[11].last = true;
    let effects = say_all_by_line(&mut state, chunks);
    let utterances = say_all_utterances(&effects);
    assert_eq!(utterances.len(), 2, "{effects:?}");
    assert_eq!(effects.len(), 2, "{effects:?}");
    // The first ten lines, the blank one silent, each with its mark.
    assert_eq!(
        texts_of(&utterances[0]),
        [
            "word 0 ", "word 1 ", "word 2 ", "word 3 ", "word 5 ", "word 6 ", "word 7 ", "word 8 ",
            "word 9",
        ]
    );
    assert_eq!(marks_of(&utterances[0]).len(), 9);
    // The rest, spoken when the text ends.
    assert_eq!(texts_of(&utterances[1]), ["word 10 ", "word 11"]);
    assert_eq!(marks_of(&utterances[1]).len(), 2);
    // Each line's mark moves the caret to that line's start.
    let effects = reduce(&mut state, &reached(marks_of(&utterances[0])[4]));
    assert_eq!(
        caret_move(&effects),
        Some(TextPoint::At(TextPosition::at(TextAnchor(105))))
    );
}

#[test]
fn held_anchors_name_every_position_the_state_keeps() {
    let mut state = editing("abc\n", 1);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    // The caret and the start marker are both on the line at anchor 100,
    // in the one edit field.
    let anchors: Vec<(OutpostId, Vec<TextAnchor>)> = state
        .held_anchors()
        .into_iter()
        .map(|(outpost, anchors)| (outpost, anchors.into_iter().collect()))
        .collect();
    assert_eq!(anchors, [(OUTPOST, vec![TextAnchor(100)])]);
    let nodes: Vec<(OutpostId, Vec<NodeId>)> = state
        .held_nodes()
        .into_iter()
        .map(|(outpost, nodes)| (outpost, nodes.into_iter().collect()))
        .collect();
    assert_eq!(nodes, [(OUTPOST, vec![id(5)])]);
}

/// The caret read of a focus whose caret is at the start of a line minted
/// at `anchor`, with nothing selected.
fn caret_at_anchor(anchor: u64) -> FocusText {
    FocusText::Caret {
        caret: CaretReport {
            line: line("gamma\n", anchor, 0),
            selection: None,
            read_at_us: stamp(),
        },
        selected_text: None,
        changes: verbatim_model::CaretChanges::default(),
        facts: None,
    }
}

/// The text anchors `state` holds, by outpost.
fn anchors_held(state: &SrState) -> Vec<(OutpostId, Vec<TextAnchor>)> {
    state
        .held_anchors()
        .into_iter()
        .map(|(outpost, anchors)| (outpost, anchors.into_iter().collect()))
        .collect()
}

#[test]
fn a_duplicate_focus_takes_its_caret_without_speaking() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "gamma\n", caret_at_anchor(100));
    // The same node reported again is a duplicate: nothing is said, and
    // its caret, read afresh, is Core's.
    let announced = focus_with_value(&mut state, StateSet::new(), "gamma\n", caret_at_anchor(200));
    assert_eq!(
        (announced, anchors_held(&state)),
        (Vec::new(), vec![(OUTPOST, vec![TextAnchor(200)])])
    );
}

#[test]
fn a_replacement_outposts_silent_re_read_takes_its_caret_without_speaking() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "gamma\n", caret_at_anchor(100));
    let _ = reduce(
        &mut state,
        &Input::OutpostEnded {
            trace_id: TraceId::from_raw(90),
            outpost: OUTPOST,
        },
    );
    // The replacement numbers its nodes and anchors afresh and reports the
    // focus the user already heard: nothing is said, and its caret is
    // Core's.
    let replacement = OutpostId(2);
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::FocusChanged {
            node: NodeSnapshot {
                id: NodeId::in_outpost(replacement, 2),
                value: Some("gamma\n".to_owned()),
                ..node(5, Role::Document, StateSet::new())
            },
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: Some(Box::new(caret_at_anchor(200))),
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        }),
    );
    assert_eq!(
        (effects, anchors_held(&state)),
        (Vec::new(), vec![(replacement, vec![TextAnchor(200)])])
    );
}

/// Focuses a document holding `value`, with what the outpost's caret read
/// found, `text`, returning the segments of each utterance the focus
/// announcement spoke, in order.
fn focus_with_value(
    state: &mut SrState,
    states: StateSet,
    value: &str,
    text: FocusText,
) -> Vec<Vec<UtteranceSegment>> {
    let snapshot = NodeSnapshot {
        value: Some(value.to_owned()),
        ..node(5, Role::Document, states)
    };
    let effects = reduce(
        state,
        &event(NormalizedEvent::FocusChanged {
            node: snapshot,
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: Some(Box::new(text)),
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        }),
    );
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(utterance.segments.clone()),
            _ => None,
        })
        .collect()
}

/// The caret read of a focus whose caret is on `text`, at its start, with
/// `selection` and its `selected_text`.
fn caret_read(selection: Option<Selection>, text: &str, selected_text: Option<&str>) -> FocusText {
    FocusText::Caret {
        caret: CaretReport {
            line: line(text, 100, 0),
            selection: selection.map(caret_selection),
            read_at_us: stamp(),
        },
        selected_text: selected_text.map(str::to_owned),
        changes: verbatim_model::CaretChanges::default(),
        facts: None,
    }
}

/// The focus announcement of the document `focus_with_value` focuses.
fn document() -> Vec<UtteranceSegment> {
    vec![
        UtteranceSegment::label("Body"),
        UtteranceSegment::new(SegmentContent::Role(Role::Document)),
    ]
}

fn caret_moved(selection: Option<Selection>, text: &str) -> Input {
    event(NormalizedEvent::CaretMoved {
        node_id: id(5),
        caret: CaretReport {
            line: line(text, 100, 0),
            selection: selection.map(caret_selection),
            read_at_us: stamp(),
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: None,
    })
}

#[test]
fn a_focused_document_says_its_caret_line_and_not_its_value() {
    let mut state = SrState::new();
    // The line the focus came with ends its announcement, from the one
    // message.
    let announced = focus_with_value(
        &mut state,
        StateSet::new(),
        "gamma\rdelta\r",
        caret_read(None, "gamma\r", None),
    );
    assert_eq!(
        announced,
        vec![document(), vec![UtteranceSegment::text("gamma")]]
    );
    // A later caret report says nothing by itself.
    assert_eq!(
        spoken(&reduce(&mut state, &caret_moved(None, "delta\r"))),
        []
    );
}

#[test]
fn an_empty_focused_field_is_blank() {
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(&mut state, StateSet::new(), "", caret_read(None, "", None)),
        vec![document(), vec![message(Message::Blank)]]
    );
}

#[test]
fn a_focused_field_with_text_selected_says_the_selection() {
    let mut state = SrState::new();
    let selection = Selection {
        start: TextPosition::at(TextAnchor(101)),
        end: TextPosition::at(TextAnchor(102)),
    };
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::FocusChanged {
            node: NodeSnapshot {
                value: Some("alpha\rdelta epsilon\r".to_owned()),
                ..node(5, Role::Document, StateSet::new())
            },
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: Some(Box::new(caret_read(
                Some(selection),
                "delta epsilon\r",
                Some("delta epsilon"),
            ))),
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        }),
    );
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Text(_))),
        "nothing is asked of the outpost: {effects:?}"
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Document)),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Preselected(
                SelectionText::Text("delta epsilon".to_owned())
            ))),
        ]
    );
}

/// A caret report of the focused field, observed at `observed_at_ms`, with
/// `selection` and the selection changes `changes` (each selected or not,
/// and its text, of as many characters as it has bytes).
fn selection_report(
    observed_at_ms: u64,
    selection: Option<Selection>,
    changes: &[(bool, &str)],
) -> Input {
    Input::Event {
        trace_id: TraceId::from_raw(9),
        observed_at_us: observed_at_ms * 1_000,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::CaretMoved {
            node_id: id(5),
            caret: CaretReport {
                line: line("alpha beta\r", 100, 5),
                selection: selection.map(caret_selection),
                read_at_us: stamp(),
            },
            selection_changes: changes
                .iter()
                .map(|&(selected, text)| SelectionChange {
                    selected,
                    text: text.to_owned(),
                    characters: u32::try_from(text.len()).expect("a short text"),
                })
                .collect(),
            content_changed: false,
            new_selection: None,
            facts: None,
        },
        purpose: None,
    }
}

/// The selection of "alpha" in [`selection_report`]'s line.
fn alpha_selected() -> Selection {
    Selection {
        start: TextPosition::at(TextAnchor(100)),
        end: TextPosition {
            anchor: TextAnchor(100),
            offset: 5,
        },
    }
}

/// A focused field whose announcement has ended with its caret's line.
fn announced_field() -> SrState {
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(
            &mut state,
            StateSet::new(),
            "alpha beta\r",
            caret_read(None, "alpha beta\r", None),
        ),
        vec![document(), vec![UtteranceSegment::text("alpha beta")]]
    );
    state
}

#[test]
fn a_selection_no_key_made_says_what_was_selected() {
    let mut state = announced_field();
    let effects = reduce(
        &mut state,
        &selection_report(0, Some(alpha_selected()), &[(true, "alpha")]),
    );
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("alpha".to_owned()))
        ))]
    );
}

#[test]
fn a_caret_report_with_no_selection_change_says_nothing() {
    let mut state = announced_field();
    let effects = reduce(&mut state, &selection_report(0, None, &[]));
    assert_eq!(effects, []);
}

/// A change with no text: "x" typed over a selection, its old range in a
/// UIA text holding nothing once the text it held was cut.
#[test]
fn a_selection_change_with_no_text_says_nothing() {
    let mut state = announced_field();
    let effects = reduce(&mut state, &selection_report(0, None, &[(false, "")]));
    assert_eq!(effects, []);
}

/// A caret report of the focused field whose selection changes are said in
/// general terms, the text having changed since the selection compared
/// with was reported: with `selection`, the changes `changes` (each
/// selected or not, with no text), and the new selection's text `new`.
fn general_report(selection: Option<Selection>, changes: &[bool], new: Option<&str>) -> Input {
    let Input::Event {
        event: NormalizedEvent::CaretMoved { node_id, caret, .. },
        ..
    } = selection_report(0, selection, &[])
    else {
        unreachable!("selection_report makes a caret report");
    };
    Input::Event {
        trace_id: TraceId::from_raw(10),
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Msaa,
        window: None,
        event: NormalizedEvent::CaretMoved {
            node_id,
            caret,
            selection_changes: changes
                .iter()
                .map(|&selected| SelectionChange {
                    selected,
                    text: String::new(),
                    characters: 0,
                })
                .collect(),
            content_changed: true,
            new_selection: new.map(|text| SelectionChange {
                selected: true,
                text: text.to_owned(),
                characters: u32::try_from(text.len()).expect("a short text"),
            }),
            facts: None,
        },
        purpose: None,
    }
}

#[test]
fn a_selection_unselected_after_a_text_change_with_nothing_selected_says_selection_removed() {
    let mut state = announced_field();
    let effects = reduce(&mut state, &general_report(None, &[false], None));
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
    assert_eq!(spoken(&effects), vec![message(Message::SelectionRemoved)]);
}

#[test]
fn a_selection_unselected_after_a_text_change_with_another_selected_says_selected_instead() {
    let mut state = announced_field();
    let effects = reduce(
        &mut state,
        &general_report(Some(alpha_selected()), &[false], Some("alpha")),
    );
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::SelectedInstead(SelectionText::Text("alpha".to_owned()))
        ))]
    );
}

#[test]
fn a_selection_extended_after_a_text_change_says_the_whole_new_selection_selected() {
    let mut state = announced_field();
    let effects = reduce(
        &mut state,
        &general_report(Some(alpha_selected()), &[true], Some("alpha")),
    );
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("alpha".to_owned()))
        ))]
    );
}

/// A selection replaced by one apart from it after a text change: both
/// general words, each with the whole new selection, each its own
/// utterance.
#[test]
fn a_selection_replaced_after_a_text_change_says_selected_then_selected_instead() {
    let mut state = announced_field();
    let effects = reduce(
        &mut state,
        &general_report(Some(alpha_selected()), &[false, true], Some("alpha")),
    );
    assert_eq!(effects.len(), 2, "two utterances in {effects:?}");
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Selected(
                SelectionText::Text("alpha".to_owned())
            ))),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::SelectedInstead(
                SelectionText::Text("alpha".to_owned())
            ))),
        ]
    );
}

/// A selecting key's answer after a text change: the whole new selection,
/// one character by its name.
#[test]
fn a_selecting_keys_answer_after_a_text_change_says_the_whole_new_selection() {
    let mut state = editing("hello, world\n", 5);
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, true));
    let request = request(&effects);
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello, world\n", 100, 6),
            selection: Some(caret_selection(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 5,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 6,
                },
            })),
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: vec![SelectionChange {
            selected: true,
            text: String::new(),
            characters: 0,
        }],
        content_changed: true,
        new_selection: Some(SelectionChange {
            selected: true,
            text: ",".to_owned(),
            characters: 1,
        }),
    }));
    let effects = reduce(&mut state, &completed(request.query_id, reply));
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Character(",".to_owned()))
        ))]
    );
}

/// Control+X on a selected line in a classic edit control: the answer
/// says the selection's change in general terms, and the text the key
/// removed, the selection's, is not said as well, as NVDA says only
/// "selection removed".
#[test]
fn a_keys_answer_removing_the_selection_with_its_text_says_only_selection_removed() {
    let mut state = editing("hello world\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, true));
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello world\n", 100, 0),
            selection: Some(caret_selection(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 0,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 11,
                },
            })),
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let reply = key_reply(Box::new(CaretReply {
        same_line: Some(true),
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("\n", 101, 0),
            selection: None,
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: vec![SelectionChange {
            selected: false,
            text: String::new(),
            characters: 0,
        }],
        content_changed: true,
        new_selection: None,
    }));
    let effects = reduce(&mut state, &completed(request_of(&effects), reply));
    assert_eq!(effects.len(), 2, "the speech and the deadline: {effects:?}");
    assert_eq!(spoken(&effects), vec![message(Message::SelectionRemoved)]);
}

/// Control+X on a selected line in Windows 11 Notepad: the answer, from
/// UIA, shows the selection removed with no change in general terms, and
/// the text the key removed, the selection's, is not said, as NVDA says
/// nothing for it.
#[test]
fn a_keys_answer_removing_the_selection_in_specific_terms_does_not_say_the_removed_text() {
    let text = "xbeta gammaelta epsilon\n";
    let mut state = editing(text, 0);
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, true));
    let reply = key_reply(Box::new(CaretReply {
        same_line: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(text, 100, 0),
            selection: Some(caret_selection(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 0,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 24,
                },
            })),
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let reply = key_reply(Box::new(CaretReply {
        same_line: Some(true),
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("\n", 100, 0),
            selection: None,
            read_at_us: stamp(),
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let effects = reduce(&mut state, &completed(request_of(&effects), reply));
    assert_eq!(effects, [Effect::BoundHeldKey(None)]);
}

#[test]
fn a_selecting_keys_change_reported_while_it_waits_is_said_with_its_answer() {
    // The key's caret event reaches Core while its watch waits: the
    // outpost compared the report with the selection it kept and replaced
    // it, so the answer has no change left, and the report's words are
    // said with it.
    let mut state = announced_field();
    let effects = reduce(&mut state, &key(CaretMotion::StartOfLine, true));
    let asked = request(&effects);
    let effects = reduce(
        &mut state,
        &selection_report(
            KEY_PRESSED_AT + 5,
            Some(alpha_selected()),
            &[(true, "alpha")],
        ),
    );
    assert_eq!(effects, []);
    let answer = caret_reply(true, line("alpha beta\r", 100, 0), None);
    let effects = reduce(&mut state, &completed(asked.query_id, answer));
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("alpha".to_owned()))
        ))]
    );
}

#[test]
fn a_focus_without_text_says_its_value_instead() {
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(
            &mut state,
            StateSet::new(),
            "plain value",
            FocusText::NoText
        ),
        vec![document(), vec![UtteranceSegment::value("plain value")]]
    );
}

#[test]
fn a_focus_whose_text_was_not_read_says_neither_its_line_nor_its_value() {
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(
            &mut state,
            StateSet::new(),
            "plain value",
            FocusText::NotRead
        ),
        vec![document()]
    );
}

#[test]
fn a_protected_field_says_its_masked_line_selection_or_value_as_any_field() {
    // The masked text the control gives is read as any field's text
    // (`docs/parity/focus-announcement.md`, "Text on focus").
    let protected = StateSet::new().with(State::Protected);
    let announcement = vec![
        UtteranceSegment::label("Body"),
        UtteranceSegment::new(SegmentContent::Role(Role::Document)),
        UtteranceSegment::new(SegmentContent::State(State::Protected)),
    ];
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(
            &mut state,
            protected,
            "******",
            caret_read(None, "******", None)
        ),
        vec![announcement.clone(), vec![UtteranceSegment::text("******")]]
    );
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(
            &mut state,
            protected,
            "******",
            caret_read(Some(selection_from(0, true)), "******", Some("******"))
        ),
        vec![
            announcement.clone(),
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Preselected(SelectionText::Text("******".to_owned()))
            ))]
        ]
    );
    // With no text to read, its value is spoken as any field's.
    let mut state = SrState::new();
    assert_eq!(
        focus_with_value(&mut state, protected, "******", FocusText::NoText),
        vec![announcement, vec![UtteranceSegment::value("******")]]
    );
}

/// `chunk` with formatting: each stretch a byte range, a spelling error or
/// not.
fn with_errors(mut chunk: TextChunk, stretches: &[(u32, u32, bool)]) -> TextChunk {
    chunk.formats = stretches
        .iter()
        .map(|&(start, end, spelling_error)| FormatRun {
            start,
            end,
            attributes: TextAttributes {
                spelling_error,
                ..TextAttributes::default()
            },
        })
        .collect();
    chunk
}

fn format(format: TextFormat) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Format(format))
}

/// Answers a caret key with `line`, and with `unit` at the caret.
fn answered(
    state: &mut SrState,
    motion: CaretMotion,
    line: TextChunk,
    unit: Option<TextChunk>,
) -> Vec<UtteranceSegment> {
    let effects = reduce(state, &key(motion, false));
    spoken(&reduce(
        state,
        &completed(request_of(&effects), caret_reply(true, line, unit)),
    ))
}

#[test]
fn a_line_speaks_a_spelling_error_where_it_starts() {
    let mut state = editing("first\n", 0);
    let text = "hello wrold there\n";
    let line = with_errors(
        line(text, 101, 0),
        &[(0, 6, false), (6, 11, true), (11, 18, false)],
    );
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, line.clone(), None),
        vec![
            UtteranceSegment::text("hello"),
            format(TextFormat::SpellingError),
            UtteranceSegment::text("wrold there"),
        ]
    );
    // Leaving the error inside a line says nothing, and reading the same
    // line again starts out of the error, as the last stretch left it.
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, line, None),
        vec![
            UtteranceSegment::text("hello"),
            format(TextFormat::SpellingError),
            UtteranceSegment::text("wrold there"),
        ]
    );
}

#[test]
fn a_line_joins_its_stretches_as_the_text_has_them() {
    let mut state = editing(
        "first
", 0,
    );
    let line = with_errors(
        line(
            "Ths line has a tset.
",
            101,
            0,
        ),
        &[
            (0, 3, true),
            (3, 15, false),
            (15, 19, true),
            (19, 21, false),
        ],
    );
    // Only a spoken change parts the text; the dot after the error follows
    // it with no space, as NVDA says the line.
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, line, None),
        vec![
            format(TextFormat::SpellingError),
            UtteranceSegment::text("Ths line has a"),
            format(TextFormat::SpellingError),
            UtteranceSegment::text("tset."),
        ]
    );
}

#[test]
fn characters_report_entering_and_leaving_an_error() {
    let text = "ab cd\n";
    let mut state = editing(text, 0);
    let character = |text: &str, error: bool| {
        let length = u32::try_from(text.len()).expect("short");
        with_errors(
            TextChunk {
                unit: TextUnit::Character,
                ..line(text, 200, 0)
            },
            &[(0, length, error)],
        )
    };
    let at = |offset| line(text, 100, offset);
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextCharacter,
            at(1),
            Some(character("b", true))
        ),
        vec![format(TextFormat::SpellingError), self::character("b")]
    );
    // Within the error, the character alone.
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::PreviousCharacter,
            at(0),
            Some(character("a", true))
        ),
        vec![self::character("a")]
    );
    // Out of it, said, as a character is extra detail.
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextCharacter,
            at(2),
            Some(character(" ", false))
        ),
        vec![format(TextFormat::NotSpellingError), self::character(" ")]
    );
}

#[test]
fn a_word_carries_the_change_its_trailing_space_makes() {
    let mut state = editing("tset of it\n", 0);
    let word = |text: &str, stretches: &[(u32, u32, bool)]| {
        with_errors(
            TextChunk {
                unit: TextUnit::Word,
                ..line(text, 200, 0)
            },
            stretches,
        )
    };
    let at = |offset| line("tset of it\n", 100, offset);
    // The misspelt word, then its space, out of the error.
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextWord,
            at(0),
            Some(word("tset ", &[(0, 4, true), (4, 5, false)]))
        ),
        vec![
            format(TextFormat::SpellingError),
            UtteranceSegment::text("tset"),
            format(TextFormat::NotSpellingError),
        ]
    );
    // The next word, already out of it, says only itself.
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextWord,
            at(5),
            Some(word("of ", &[(0, 3, false)]))
        ),
        vec![UtteranceSegment::text("of")]
    );
}

#[test]
fn a_focus_reports_the_formatting_at_its_line_start_afresh() {
    let mut state = SrState::new();
    let line = with_errors(line("Ths is\r", 100, 0), &[(0, 3, true), (3, 7, false)]);
    let announced = focus_with_value(
        &mut state,
        StateSet::new(),
        "Ths is\r",
        FocusText::Caret {
            caret: CaretReport {
                line,
                selection: None,
                read_at_us: stamp(),
            },
            selected_text: None,
            changes: verbatim_model::CaretChanges::default(),
            facts: None,
        },
    );
    assert_eq!(
        announced,
        vec![
            document(),
            vec![
                format(TextFormat::SpellingError),
                UtteranceSegment::text("Ths is"),
            ]
        ]
    );
}

#[test]
fn bold_starts_and_ends_and_a_font_change_is_named() {
    let mut state = editing("first\n", 0);
    let mut line = line("plain bold\n", 101, 0);
    line.formats = vec![
        FormatRun {
            start: 0,
            end: 6,
            attributes: TextAttributes {
                bold: Some(false),
                font_name: Some("Calibri".to_owned()),
                ..TextAttributes::default()
            },
        },
        FormatRun {
            start: 6,
            end: 11,
            attributes: TextAttributes {
                bold: Some(true),
                font_name: Some("Calibri".to_owned()),
                ..TextAttributes::default()
            },
        },
    ];
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, line, None),
        vec![
            format(TextFormat::FontName("Calibri".to_owned())),
            UtteranceSegment::text("plain"),
            format(TextFormat::Bold),
            UtteranceSegment::text("bold"),
        ]
    );
    let mut next = self::line("next\n", 102, 0);
    next.formats = vec![FormatRun {
        start: 0,
        end: 5,
        attributes: TextAttributes {
            bold: Some(false),
            font_name: Some("Calibri".to_owned()),
            ..TextAttributes::default()
        },
    }];
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, next, None),
        vec![format(TextFormat::NotBold), UtteranceSegment::text("next")]
    );
}

/// `text` as a line all in bold.
fn bold_line(text: &str, anchor: u64) -> TextChunk {
    let end = u32::try_from(text.len()).expect("short");
    let bold = TextAttributes {
        bold: Some(true),
        ..TextAttributes::default()
    };
    formatted(line(text, anchor, 0), vec![(0, end, bold)])
}

/// A focus event for `node`, whole-window or not, with `text`.
fn focus_event(node: NodeSnapshot, whole_window: bool, text: Option<FocusText>) -> Input {
    event(NormalizedEvent::FocusChanged {
        node,
        whole_window,
        ancestors: Vec::new(),
        selected_child: None,
        text: text.map(Box::new),
        earlier: None,
        group: None,
        answers: None,
        first_cached_ancestor: None,
    })
}

/// The formatting is kept on the focus object, so the silent re-take
/// after an outpost is replaced, a new focus object, starts afresh and the
/// next line says its bold again (Dickson, 2026-11-08).
#[test]
fn the_silent_re_take_after_a_replacement_reports_formatting_afresh() {
    let mut state = editing("first\n", 0);
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextLine,
            bold_line("first bold\n", 101),
            None
        ),
        vec![
            format(TextFormat::Bold),
            UtteranceSegment::text("first bold")
        ]
    );
    let _ = reduce(
        &mut state,
        &Input::OutpostEnded {
            trace_id: TraceId::from_raw(90),
            outpost: OUTPOST,
        },
    );
    let effects = reduce(
        &mut state,
        &focus_event(
            NodeSnapshot {
                id: NodeId::in_outpost(OutpostId(2), 2),
                ..node(5, Role::EditableText, StateSet::new())
            },
            false,
            Some(FocusText::Caret {
                caret: CaretReport {
                    line: bold_line("first bold\n", 200),
                    selection: None,
                    read_at_us: stamp(),
                },
                selected_text: None,
                changes: verbatim_model::CaretChanges::default(),
                facts: None,
            }),
        ),
    );
    assert_eq!(effects, Vec::new());
    // The replacement answers the key.
    let effects = reduce(&mut state, &key(CaretMotion::NextLine, false));
    let answer = completed_on(
        request_of(&effects),
        NodeId::in_outpost(OutpostId(2), 2),
        caret_reply(true, bold_line("second line\n", 201), None),
    );
    assert_eq!(
        spoken(&reduce(&mut state, &answer)),
        vec![
            format(TextFormat::Bold),
            UtteranceSegment::text("second line")
        ]
    );
}

/// A nameless whole-window focus is a new focus object too: when the focus
/// returns to the text as one, its next line says its bold again, as
/// NVDA's per-object cache starts empty with each new object (Dickson,
/// 2026-11-08).
#[test]
fn a_nameless_whole_window_focus_reports_formatting_afresh() {
    let mut state = editing("first\n", 0);
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextLine,
            bold_line("first bold\n", 101),
            None
        ),
        vec![
            format(TextFormat::Bold),
            UtteranceSegment::text("first bold")
        ]
    );
    let nameless = |number| NodeSnapshot {
        name: None,
        ..node(number, Role::EditableText, StateSet::new())
    };
    for number in [7, 5] {
        let effects = reduce(&mut state, &focus_event(nameless(number), true, None));
        assert_eq!(spoken(&effects), Vec::new());
    }
    assert_eq!(
        answered(
            &mut state,
            CaretMotion::NextLine,
            bold_line("second line\n", 102),
            None
        ),
        vec![
            format(TextFormat::Bold),
            UtteranceSegment::text("second line")
        ]
    );
}

/// `chunk` with formatting: each stretch a byte range and its attributes.
fn formatted(mut chunk: TextChunk, stretches: Vec<(u32, u32, TextAttributes)>) -> TextChunk {
    chunk.formats = stretches
        .into_iter()
        .map(|(start, end, attributes)| FormatRun {
            start,
            end,
            attributes,
        })
        .collect();
    chunk
}

/// Answers a caret key with `line`, and with `unit` at the caret, checking
/// that speech is its one effect, and returns what it says.
fn answered_alone(
    state: &mut SrState,
    motion: CaretMotion,
    line: TextChunk,
    unit: Option<TextChunk>,
) -> Vec<UtteranceSegment> {
    let effects = reduce(state, &key(motion, false));
    let effects = reduce(
        state,
        &completed(request_of(&effects), caret_reply(true, line, unit)),
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Speak(..), Effect::BoundHeldKey(None)]
        ),
        "speech alone: {effects:?}"
    );
    spoken(&effects)
}

#[test]
fn a_heading_says_its_size_and_a_list_item_its_bullet_before_its_text() {
    let mut state = editing("first\n", 0);
    let size = |points: &str| TextAttributes {
        font_size: Some(points.to_owned()),
        ..TextAttributes::default()
    };
    let heading = formatted(
        line("Title\n", 101, 0),
        vec![(0, 5, size("28.0 pt")), (5, 6, size("11.0 pt"))],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, heading, None),
        vec![
            format(TextFormat::FontSize("28.0 pt".to_owned())),
            UtteranceSegment::text("Title"),
        ],
        "the line break, not part of the line's content, changes nothing"
    );
    let bulleted = TextAttributes {
        bullet: Some(BulletStyle::FilledRound),
        ..size("11.0 pt")
    };
    let item = || formatted(line("item\n", 102, 0), vec![(0, 5, bulleted.clone())]);
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, item(), None),
        vec![
            format(TextFormat::FontSize("11.0 pt".to_owned())),
            format(TextFormat::Bullet(BulletStyle::FilledRound)),
            UtteranceSegment::text("item"),
        ],
        "the bullet after the changes"
    );
    // Every line read says its bullet, which is no change of formatting.
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, item(), None),
        vec![
            format(TextFormat::Bullet(BulletStyle::FilledRound)),
            UtteranceSegment::text("item"),
        ]
    );
    // A word says none.
    let word = formatted(
        TextChunk {
            unit: TextUnit::Word,
            ..line("item", 201, 0)
        },
        vec![(0, 4, bulleted.clone())],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextWord, item(), Some(word)),
        vec![UtteranceSegment::text("item")]
    );
    // A line out of the list says nothing of it.
    let plain = formatted(
        line("after\n", 103, 0),
        vec![(
            0,
            6,
            TextAttributes {
                bullet: Some(BulletStyle::None),
                ..size("11.0 pt")
            },
        )],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, plain, None),
        vec![UtteranceSegment::text("after")]
    );
}

#[test]
fn a_background_color_is_named_alone_or_after_the_color() {
    let mut state = editing("first\n", 0);
    let colored = |color: &str, background: &str| TextAttributes {
        color: Some(color.to_owned()),
        background_color: Some(background.to_owned()),
        ..TextAttributes::default()
    };
    let line = formatted(
        line("one two three four\n", 101, 0),
        vec![
            (0, 4, colored("black", "white")),
            (4, 8, colored("dark red", "light grey")),
            (8, 14, colored("dark red", "white")),
            (14, 19, colored("black", "white")),
        ],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, line, None),
        vec![
            format(TextFormat::Color("black".to_owned())),
            format(TextFormat::OnBackgroundColor("white".to_owned())),
            UtteranceSegment::text("one"),
            format(TextFormat::Color("dark red".to_owned())),
            format(TextFormat::OnBackgroundColor("light grey".to_owned())),
            UtteranceSegment::text("two"),
            format(TextFormat::BackgroundColor("white".to_owned())),
            UtteranceSegment::text("three"),
            format(TextFormat::Color("black".to_owned())),
            UtteranceSegment::text("four"),
        ]
    );
}

#[test]
fn strikethrough_and_the_kind_of_underline_start_change_and_end() {
    let mut state = editing("first\n", 0);
    let drawn = |strikethrough, underline_style| TextAttributes {
        strikethrough: Some(strikethrough),
        underline_style: Some(underline_style),
        // Read for the font attributes too; the kind is reported instead.
        underline: Some(underline_style != LineStyle::None),
        ..TextAttributes::default()
    };
    let line = formatted(
        line("a b c d e\n", 101, 0),
        vec![
            (0, 2, drawn(LineStyle::None, LineStyle::None)),
            (2, 4, drawn(LineStyle::Single, LineStyle::Double)),
            (4, 6, drawn(LineStyle::Double, LineStyle::Wavy)),
            (6, 8, drawn(LineStyle::Double, LineStyle::Single)),
            (8, 10, drawn(LineStyle::None, LineStyle::None)),
        ],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, line, None),
        vec![
            UtteranceSegment::text("a"),
            format(TextFormat::Strikethrough(LineStyle::Single)),
            format(TextFormat::UnderlineStyle(LineStyle::Double)),
            UtteranceSegment::text("b"),
            format(TextFormat::Strikethrough(LineStyle::Double)),
            format(TextFormat::UnderlineStyle(LineStyle::Wavy)),
            UtteranceSegment::text("c"),
            format(TextFormat::UnderlineStyle(LineStyle::Single)),
            UtteranceSegment::text("d"),
            format(TextFormat::Strikethrough(LineStyle::None)),
            format(TextFormat::UnderlineStyle(LineStyle::None)),
            UtteranceSegment::text("e"),
        ]
    );
}

#[test]
fn a_link_is_said_where_it_starts_and_where_it_ends() {
    let mut state = editing("first\n", 0);
    let linked = |link| TextAttributes {
        link,
        ..TextAttributes::default()
    };
    let line = formatted(
        line("see the page here\n", 101, 0),
        vec![
            (0, 8, linked(false)),
            (8, 12, linked(true)),
            (12, 18, linked(false)),
        ],
    );
    assert_eq!(
        answered_alone(&mut state, CaretMotion::NextLine, line, None),
        vec![
            UtteranceSegment::text("see the"),
            format(TextFormat::Link),
            UtteranceSegment::text("page"),
            format(TextFormat::NotLink),
            UtteranceSegment::text("here"),
        ]
    );
}

/// Moves the navigator from the focus to its next sibling, `object`, and
/// returns the effects of landing there.
fn navigate_to(state: &mut SrState, object: NodeSnapshot) -> Vec<Effect> {
    let effects = reduce(state, &command(ReviewCommand::NextSibling, 0));
    let Some(Effect::Fetch(query)) = effects.first() else {
        panic!("expected a navigation fetch, got {effects:?}");
    };
    reduce(
        state,
        &Input::Answered {
            trace_id: TraceId::from_raw(11),
            query_id: query.query_id,
            outpost: query.node_id.outpost(),
            outcome: verbatim_model::protocol::QueryOutcome::Done(
                verbatim_model::protocol::QueryResult::Navigated(Some(object)),
            ),
        },
    )
}

/// An edit field without the focus, with a value, as navigation finds it.
fn unfocused_edit() -> NodeSnapshot {
    let mut edit = node(30, Role::EditableText, StateSet::new());
    edit.name = Some("Notes".to_owned());
    edit.value = Some("first line second line".to_owned());
    edit
}

#[test]
fn navigating_to_an_edit_field_reads_the_caret_line_in_place_of_its_value() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::Button, StateSet::new()));
    let effects = navigate_to(&mut state, unfocused_edit());
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Notes"),
            UtteranceSegment::new(SegmentContent::Role(Role::EditableText)),
        ],
        "the value is left out"
    );
    let selection = request(&effects);
    assert_eq!(selection.node_id, id(30));
    assert_eq!(
        selection.op,
        TextOp::ReadRange {
            start: TextPoint::SelectionStart,
            end: TextPoint::SelectionEnd,
            through: None,
        }
    );
    // Nothing selected: the caret's line is read next.
    let effects = reduce(
        &mut state,
        &completed(
            selection.query_id,
            TextReply::Range {
                text: String::new(),
                truncated: false,
            },
        ),
    );
    assert_eq!(spoken(&effects), Vec::new());
    let line_request = request(&effects);
    assert_eq!(
        line_request.op,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        })
    );
    let effects = reduce(
        &mut state,
        &completed(
            line_request.query_id,
            TextReply::Read {
                moved: 0,
                chunk: line("second line\r\n", 200, 3),
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::text("second line")]
    );
}

#[test]
fn navigating_to_an_edit_field_with_a_selection_says_it_is_selected() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::Button, StateSet::new()));
    let effects = navigate_to(&mut state, unfocused_edit());
    let selection = request(&effects);
    let effects = reduce(
        &mut state,
        &completed(
            selection.query_id,
            TextReply::Range {
                text: "second".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Preselected(SelectionText::Text("second".to_owned()))
        ))]
    );
}

#[test]
fn navigating_to_a_protected_edit_field_reads_its_masked_selection() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::Button, StateSet::new()));
    let mut edit = unfocused_edit();
    edit.states = StateSet::new().with(State::Protected);
    let effects = navigate_to(&mut state, edit);
    let selection = request(&effects);
    assert_eq!(
        (selection.node_id, &selection.op),
        (
            id(30),
            &TextOp::ReadRange {
                start: TextPoint::SelectionStart,
                end: TextPoint::SelectionEnd,
                through: None,
            }
        )
    );
    let effects = reduce(
        &mut state,
        &completed(
            selection.query_id,
            TextReply::Range {
                text: "******".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Preselected(SelectionText::Text("******".to_owned()))
        ))]
    );
}

#[test]
fn a_navigator_edit_field_without_text_says_its_value_and_a_late_answer_is_dropped() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::Button, StateSet::new()));
    let effects = navigate_to(&mut state, unfocused_edit());
    let selection = request(&effects);
    let effects = reduce(
        &mut state,
        &completed(selection.query_id, TextReply::NoText),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::value("first line second line")]
    );

    // An answer for an object the navigator has left says nothing.
    let effects = navigate_to(&mut state, unfocused_edit());
    let selection = request(&effects);
    let _ = navigate_to(&mut state, node(31, Role::Button, StateSet::new()));
    let effects = reduce(
        &mut state,
        &completed(
            selection.query_id,
            TextReply::Range {
                text: "second".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(effects, Vec::new());
}

#[test]
fn reporting_the_focused_edit_field_reads_its_known_caret_line() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.value = Some("one two".to_owned());
    focus(&mut state, edit);
    caret_event(&mut state, 5, line("one two\n", 100, 4));
    let effects = reduce(&mut state, &command(ReviewCommand::ReportObject, 0));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Text(_))),
        "the known caret needs no request: {effects:?}"
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::EditableText)),
            UtteranceSegment::text("one two"),
        ]
    );
}

#[test]
fn reporting_an_edit_field_again_spells_then_copies_its_name_and_caret_line() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.value = Some("first\nab cd".to_owned());
    focus(&mut state, edit);
    caret_event(&mut state, 5, line("ab cd\n", 100, 1));

    // The second press spells the name and the caret's line, not the value.
    let effects = reduce(&mut state, &command(ReviewCommand::ReportObject, 1));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::SpelledCapital("B".to_owned())),
            UtteranceSegment::text("o"),
            UtteranceSegment::text("d"),
            UtteranceSegment::text("y"),
            UtteranceSegment::new(SegmentContent::Message(Message::Space)),
            UtteranceSegment::text("a"),
            UtteranceSegment::text("b"),
            UtteranceSegment::new(SegmentContent::Message(Message::Space)),
            UtteranceSegment::text("c"),
            UtteranceSegment::text("d"),
        ]
    );

    // The third copies them.
    let effects = reduce(&mut state, &command(ReviewCommand::ReportObject, 2));
    assert_eq!(
        effects,
        vec![Effect::CopyToClipboard("Body ab cd".to_owned())]
    );

    // An object whose caret Core does not know is asked for its selection,
    // and the selected text is used.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::ReportObject, 2));
    let asked = request(&effects);
    assert_eq!(
        asked.op,
        TextOp::ReadRange {
            start: TextPoint::SelectionStart,
            end: TextPoint::SelectionEnd,
            through: None,
        }
    );
    let effects = reduce(
        &mut state,
        &completed(
            asked.query_id,
            TextReply::Range {
                text: "chosen".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(
        effects,
        vec![Effect::CopyToClipboard("Body chosen".to_owned())]
    );
}

/// Verbatim+Tab's press: [`command`]'s, traced 3.
fn verbatim_tab(repeat: u8) -> Input {
    command(ReviewCommand::ReportFocus, repeat)
}

/// The answer to the Verbatim+Tab query asked under `trace_id`: `node`,
/// the focus its application has now.
fn report_answer(trace_id: TraceId, node: NodeSnapshot) -> Input {
    Input::Event {
        trace_id,
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            node,
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        },
        purpose: Some(verbatim_model::protocol::FocusPurpose::Report),
    }
}

/// The live query Verbatim+Tab asks.
const REPORT_QUERY: Effect = Effect::ReadFocus(verbatim_model::protocol::FocusPurpose::Report);

/// Verbatim+Tab says nothing until its query answers, then the answer as
/// a query announces it, the focus's known caret line in place of its
/// value, the navigator staying where it was; an answer that is not the
/// focus Core holds is asked for its own text, which is spoken.
#[test]
fn verbatim_tab_speaks_the_answer_alone_then_its_line() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.value = Some("one two".to_owned());
    focus(&mut state, edit.clone());
    caret_event(&mut state, 5, line("one two\n", 100, 4));
    let _ = navigate_to(&mut state, node(31, Role::Button, StateSet::new()));

    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    let effects = reduce(&mut state, &report_answer(TraceId::from_raw(3), edit));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::EditableText)),
            UtteranceSegment::text("one two"),
        ]
    );
    let effects = reduce(&mut state, &command(ReviewCommand::ReportObject, 0));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );

    // An answer of another element than Core's focus is asked for its
    // selected text, which is spoken.
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    let effects = reduce(
        &mut state,
        &report_answer(
            TraceId::from_raw(3),
            node(7, Role::EditableText, StateSet::new()),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::EditableText)),
        ]
    );
    let selection = request(&effects);
    assert_eq!(selection.node_id, id(7));
    let effects = reduce(
        &mut state,
        &completed(
            selection.query_id,
            TextReply::Range {
                text: "two".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Preselected(SelectionText::Text("two".to_owned()))
        ))]
    );
}

/// Verbatim+Tab's answer is spoken only: Core's focus and its navigator
/// stay as they were, so the focus's own repeat is still a duplicate.
#[test]
fn verbatim_tab_never_moves_cores_focus() {
    let mut state = SrState::new();
    let held = node(5, Role::Button, StateSet::new());
    focus(&mut state, held.clone());
    let before = state.clone();
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    let mut other = node(9, Role::Link, StateSet::new());
    other.name = Some("Other".to_owned());
    let effects = reduce(&mut state, &report_answer(TraceId::from_raw(3), other));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Other"),
            UtteranceSegment::new(SegmentContent::Role(Role::Link)),
        ]
    );
    assert_eq!(state.focused(), before.focused());
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::FocusChanged {
            node: held,
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        }),
    );
    assert_eq!(effects, [], "the focus Core holds, repeated");
}

/// A second press within the repeat interval while the first waits is
/// kept with it, and a third too: the answer is spoken, then its name
/// spelled, then spelled with character descriptions. A press repeated
/// with none waiting asks again and spells its answer's name, "blank" for
/// none.
#[test]
fn a_second_verbatim_tab_while_waiting_speaks_the_object_then_spells() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.name = Some("Ab".to_owned());
    focus(&mut state, edit.clone());
    caret_event(&mut state, 5, line("cd\n", 100, 1));
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    assert_eq!(reduce(&mut state, &verbatim_tab(1)), []);
    assert_eq!(reduce(&mut state, &verbatim_tab(2)), []);
    let effects = reduce(&mut state, &report_answer(TraceId::from_raw(3), edit));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Ab"),
            UtteranceSegment::new(SegmentContent::Role(Role::EditableText)),
            UtteranceSegment::text("cd"),
            UtteranceSegment::new(SegmentContent::SpelledCapital("A".to_owned())),
            UtteranceSegment::text("b"),
            UtteranceSegment::new(SegmentContent::CharacterDescription("A".to_owned())),
            UtteranceSegment::new(SegmentContent::CharacterDescription("b".to_owned())),
        ]
    );

    let mut unnamed = node(6, Role::EditableText, StateSet::new());
    unnamed.name = None;
    assert_eq!(reduce(&mut state, &verbatim_tab(1)), [REPORT_QUERY]);
    let effects = reduce(&mut state, &report_answer(TraceId::from_raw(3), unnamed));
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
}

/// An answer that comes after a newer focus was applied is late, and says
/// nothing; so is a replaced press's, whose newer press's answer is
/// spoken.
#[test]
fn a_verbatim_tab_answer_after_a_newer_focus_is_dropped() {
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::Button, StateSet::new()));
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let effects = reduce(
        &mut state,
        &report_answer(TraceId::from_raw(3), node(5, Role::Button, StateSet::new())),
    );
    assert_eq!(effects, []);

    let press = |trace| Input::Command {
        pressed_at_ms: 0,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(trace),
        command: ReviewCommand::ReportFocus,
        repeat: 0,
    };
    assert_eq!(reduce(&mut state, &press(40)), [REPORT_QUERY]);
    assert_eq!(reduce(&mut state, &press(41)), [REPORT_QUERY]);
    let late = reduce(
        &mut state,
        &report_answer(
            TraceId::from_raw(40),
            node(6, Role::Button, StateSet::new()),
        ),
    );
    assert_eq!(late, []);
    let effects = reduce(
        &mut state,
        &report_answer(
            TraceId::from_raw(41),
            node(6, Role::Button, StateSet::new()),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );
}

/// A focus observed before the press, applied while its query waits, is
/// spoken and the press's answer after it, as NVDA runs NVDA+Tab after the
/// focus event queued before it; a focus observed after the press makes
/// its answer late.
#[test]
fn a_focus_observed_before_a_waiting_verbatim_tab_keeps_its_press() {
    let at = |observed_at_us, number| {
        let mut input = event(NormalizedEvent::FocusChanged {
            node: node(number, Role::Button, StateSet::new()),
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        });
        if let Input::Event {
            observed_at_us: observed,
            ..
        } = &mut input
        {
            *observed = observed_at_us;
        }
        input
    };
    let press = Input::Command {
        pressed_at_ms: 1_000,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(3),
        command: ReviewCommand::ReportFocus,
        repeat: 0,
    };
    let mut state = SrState::new();
    let _ = reduce(&mut state, &at(500_000, 5));
    assert_eq!(reduce(&mut state, &press), [REPORT_QUERY]);
    let _ = reduce(&mut state, &at(999_000, 6));
    let effects = reduce(
        &mut state,
        &report_answer(TraceId::from_raw(3), node(6, Role::Button, StateSet::new())),
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );

    assert_eq!(reduce(&mut state, &press), [REPORT_QUERY]);
    let _ = reduce(&mut state, &at(1_001_000, 7));
    let late = reduce(
        &mut state,
        &report_answer(TraceId::from_raw(3), node(7, Role::Button, StateSet::new())),
    );
    assert_eq!(late, []);
}

/// A plain focus query's answer applied while Verbatim+Tab's press waits
/// does not end the press, though it is stamped with its read's start,
/// after the press: an outpost asked both at its start answers the plain
/// query first, and the press's own answer is then spoken, as NVDA speaks
/// the focus and then NVDA+Tab's answer.
#[test]
fn a_plain_query_answer_read_after_a_waiting_press_keeps_the_press() {
    let plain_answer = |observed_at_us, number| Input::Event {
        trace_id: TraceId::from_raw(9),
        observed_at_us,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            node: node(number, Role::Button, StateSet::new()),
            whole_window: false,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
            answers: None,
            first_cached_ancestor: None,
        },
        purpose: Some(verbatim_model::protocol::FocusPurpose::Plain),
    };
    let press = Input::Command {
        pressed_at_ms: 1_000,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(3),
        command: ReviewCommand::ReportFocus,
        repeat: 0,
    };
    let mut state = SrState::new();
    assert_eq!(reduce(&mut state, &press), [REPORT_QUERY]);
    let _ = reduce(&mut state, &plain_answer(1_200_000, 6));
    let effects = reduce(
        &mut state,
        &report_answer(TraceId::from_raw(3), node(6, Role::Button, StateSet::new())),
    );
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );
}

/// A press with nothing to ask, or whose query's outpost ended or answered
/// no focus, says "No focus", as with no focus at all.
#[test]
fn verbatim_tab_with_nothing_to_ask_says_no_focus() {
    let mut state = SrState::new();
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    let effects = reduce(
        &mut state,
        &Input::ReadFocusUnanswered {
            trace_id: TraceId::from_raw(3),
            abandoned: false,
        },
    );
    assert_eq!(spoken(&effects), vec![message(Message::NoFocus)]);
}

/// A query abandoned at its deadline, its application hung, says nothing,
/// and its press waits no more: an answer under its trace is late.
#[test]
fn verbatim_tab_abandoned_says_nothing() {
    let mut state = SrState::new();
    assert_eq!(reduce(&mut state, &verbatim_tab(0)), [REPORT_QUERY]);
    let effects = reduce(
        &mut state,
        &Input::ReadFocusUnanswered {
            trace_id: TraceId::from_raw(3),
            abandoned: true,
        },
    );
    assert_eq!(effects, []);
    let late = reduce(
        &mut state,
        &report_answer(TraceId::from_raw(3), node(5, Role::Button, StateSet::new())),
    );
    assert_eq!(late, []);
}

/// The key numbers each utterance of `effects` carries, in order.
fn stamps(effects: &[Effect]) -> Vec<Option<verbatim_model::KeySeq>> {
    effects
        .iter()
        .filter_map(|effect| match own(effect) {
            Effect::Speak(_, key) => Some(*key),
            _ => None,
        })
        .collect()
}

/// A held key, its trace `trace_id`.
fn held_at(trace_id: TraceId, motion: CaretMotion, pressed_at_ms: u64) -> Input {
    Input::HeldKey {
        trace_id,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms,
    }
}

/// The answer of a held key's before read, finding `line`.
fn read_before(line: TextChunk, read_at_ms: u64) -> TextReply {
    let micros = read_at_ms * 1_000;
    TextReply::Caret(Box::new(verbatim_model::CaretReply {
        caret: CaretReport {
            line: line.clone(),
            selection: None,
            read_at_us: micros,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: ReadFacts {
            unit: Some(line),
            change: Change::NoPrevious,
            read_ended_at_us: micros,
        },
    }))
}

/// How many final reads `effects` ask.
fn final_reads(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            matches!(own(effect), Effect::Text(request) if matches!(
                request.op,
                TextOp::ReadCaret(CaretRead { purpose: ReadPurpose::Final, .. })
            ))
        })
        .count()
}

/// A field on `text` at `offset`, and a held key of `motion` passed on after
/// its before read, begun at 500 ms, found the same. Returns the state, and
/// the key's request.
fn key_passed_on(text: &str, offset: u32, motion: CaretMotion) -> (SrState, QueryId) {
    let mut state = editing(text, offset);
    let query = request_of(&reduce(
        &mut state,
        &held_at(TraceId::from_raw(2), motion, 499),
    ));
    let effects = reduce(
        &mut state,
        &completed(query, read_before(line(text, 100, offset), 500)),
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(own(effect), Effect::PassHeldKey(_))),
        "passed on: {effects:?}"
    );
    (state, query)
}

/// The key's 100 ms passing, the input the deadline brings.
fn key_bound_passed() -> Input {
    Input::HeldKeyDue {
        trace_id: TraceId::from_raw(2),
        at_us: 600_001,
    }
}

#[test]
fn a_keys_bound_with_no_evidence_orders_the_final_read() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(&mut state, &key_bound_passed());
    assert_eq!(final_reads(&effects), 1);
}

#[test]
fn evidence_before_a_keys_bound_leaves_no_final_read() {
    let (mut state, query) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply(true, line("abc\n", 100, 1), None)),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);
    assert_eq!(final_reads(&reduce(&mut state, &key_bound_passed())), 0);
}

#[test]
fn a_focus_moving_within_the_bound_ends_the_watch_after_the_units_words() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(
        &mut state,
        &focus_event(node(6, Role::Button, StateSet::new()), false, None),
    );
    assert_eq!(spoken(&effects)[0], character("a"));
}

#[test]
fn a_focus_moving_within_a_backspaces_bound_says_the_removed_text() {
    let (mut state, _) = key_passed_on("abc\n", 2, CaretMotion::Backspace);
    let effects = reduce(
        &mut state,
        &focus_event(node(6, Role::Button, StateSet::new()), false, None),
    );
    assert_eq!(spoken(&effects)[0], character("b"));
}

#[test]
fn a_focus_moving_first_says_what_the_watch_heard_before_the_focus() {
    let (mut state, _) = key_passed_on("\n", 0, CaretMotion::PreviousLine);
    let heard = reduce(&mut state, &selection_report(0, None, &[(true, "x")]));
    assert_eq!(spoken(&heard), []);
    let effects = reduce(
        &mut state,
        &focus_event(node(6, Role::Button, StateSet::new()), false, None),
    );
    assert_eq!(
        spoken(&effects)[..2],
        [
            message(Message::Blank),
            UtteranceSegment::new(SegmentContent::Phrase(Phrase::Selected(
                SelectionText::Character("x".to_owned())
            )))
        ]
    );
}

#[test]
fn a_keys_final_read_follows_what_waits_and_a_key_taking_the_line_has_none() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    assert_eq!(
        reduce(
            &mut state,
            &held_at(TraceId::from_raw(3), CaretMotion::NextCharacter, 510),
        ),
        []
    );
    assert_eq!(final_reads(&reduce(&mut state, &key_bound_passed())), 0);
}

#[test]
fn a_new_window_taking_the_focus_cuts_the_keys_words() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(
        &mut state,
        &focus_event(node(6, Role::Button, StateSet::new()), true, None),
    );
    let words = effects
        .iter()
        .position(|effect| spoken(std::slice::from_ref(effect)).first() == Some(&character("a")))
        .expect("the unit is said");
    let cut = effects
        .iter()
        .position(|effect| matches!(own(effect), Effect::StopSpeech))
        .expect("the new window cuts speech");
    assert!(words < cut, "{effects:?}");
}

#[test]
fn a_focus_moving_while_the_before_read_runs_leaves_the_key_silent() {
    let mut state = editing("abc\n", 0);
    let query = request_of(&reduce(
        &mut state,
        &held_at(TraceId::from_raw(2), CaretMotion::NextCharacter, 499),
    ));
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let effects = reduce(
        &mut state,
        &completed(query, read_before(line("abc\n", 100, 0), 500)),
    );
    assert_eq!(spoken(&effects), []);
}

#[test]
fn a_selection_changed_before_a_keys_read_is_said_as_it_is_passed_on() {
    let mut state = editing("alpha\n", 0);
    let query = request_of(&reduce(
        &mut state,
        &held_at(TraceId::from_raw(2), CaretMotion::NextCharacter, 499),
    ));
    let TextReply::Caret(mut read) = read_before(line("alpha\n", 100, 0), 500) else {
        unreachable!("a key's answer");
    };
    read.selection_changes = vec![SelectionChange {
        selected: true,
        text: "alpha".to_owned(),
        characters: 5,
    }];
    let effects = reduce(&mut state, &completed(query, TextReply::Caret(read)));
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("alpha".to_owned()))
        ))]
    );
}

#[test]
fn a_repeated_focus_says_the_selection_it_found_once() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "gamma\n", caret_at_anchor(100));
    let selected = |anchor| {
        let FocusText::Caret {
            caret,
            selected_text,
            ..
        } = caret_at_anchor(anchor)
        else {
            unreachable!("a caret");
        };
        FocusText::Caret {
            caret,
            selected_text,
            changes: verbatim_model::CaretChanges {
                selection_changes: vec![SelectionChange {
                    selected: true,
                    text: "gamma".to_owned(),
                    characters: 5,
                }],
                ..verbatim_model::CaretChanges::default()
            },
            facts: None,
        }
    };
    assert_eq!(
        focus_with_value(&mut state, StateSet::new(), "gamma\n", selected(200)),
        vec![vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(SelectionText::Text("gamma".to_owned()))
        ))]]
    );
    assert_eq!(
        focus_with_value(&mut state, StateSet::new(), "gamma\n", caret_at_anchor(300)),
        Vec::<Vec<UtteranceSegment>>::new()
    );
}

#[test]
fn speech_is_stamped_with_the_number_of_the_key_on_whose_behalf_it_is_spoken() {
    // A held Backspace, key 1, and "c" typed as key 2 behind it: the echo
    // is reduced while Core handles Backspace's read, answered under
    // Backspace's trace, and carries the letter's number, its `ForKey`
    // trace's, not the input's.
    let mut state = editing("ab", 2);
    let (backspace, letter) = (TraceId::from_raw(26), TraceId::from_raw(27));
    let mut held = held_at(backspace, CaretMotion::Backspace, 400);
    if let Input::HeldKey { key_seq, .. } = &mut held {
        *key_seq = verbatim_model::KeySeq::new(1);
    }
    let reading = request(&reduce(&mut state, &held)).query_id;
    let typed_c = Input::CharacterTyped {
        key_seq: verbatim_model::KeySeq::new(2),
        trace_id: letter,
        text: "c".to_owned(),
        at_ms: 0,
    };
    assert_eq!(reduce(&mut state, &typed_c), []);
    let effects = reduce(
        &mut state,
        &text_answer(backspace, reading, read_before(line("ab", 101, 2), 410)),
    );
    assert_eq!(stamps(&effects), [Some(verbatim_model::KeySeq::new(2))]);
}

#[test]
fn speech_of_an_unkeyed_input_or_a_forgotten_key_carries_no_number() {
    let mut state = editing("ab", 2);
    let copied = |trace_id| Input::ClipboardCopied {
        trace_id,
        text: "ab".to_owned(),
        copied: true,
    };
    let first = TraceId::from_raw(28);
    let _ = reduce(
        &mut state,
        &Input::CharacterTyped {
            key_seq: verbatim_model::KeySeq::new(1),
            trace_id: first,
            text: "c".to_owned(),
            at_ms: 0,
        },
    );
    assert_eq!(
        stamps(&reduce(&mut state, &copied(first))),
        [Some(verbatim_model::KeySeq::new(1))]
    );
    assert_eq!(
        stamps(&reduce(&mut state, &copied(TraceId::from_raw(29)))),
        [None]
    );
    // 1,024 newer keys push the first one's number out.
    for number in 2..=1_025 {
        let _ = reduce(
            &mut state,
            &Input::CharacterTyped {
                key_seq: verbatim_model::KeySeq::new(number),
                trace_id: TraceId::from_raw(30),
                text: "c".to_owned(),
                at_ms: 0,
            },
        );
    }
    assert_eq!(stamps(&reduce(&mut state, &copied(first))), [None]);
}

/// A report of node 5 with facts showing `moved`, read ending at
/// `ended_at_us`.
fn report_ended(line: TextChunk, moved: bool, ended_at_us: u64) -> Input {
    event(NormalizedEvent::CaretMoved {
        node_id: id(5),
        caret: CaretReport {
            line: line.clone(),
            selection: None,
            read_at_us: ended_at_us,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: Some(Box::new(ReadFacts {
            read_ended_at_us: ended_at_us,
            ..key_facts(line.clone(), moved, (line, None), true)
        })),
    })
}

#[test]
fn a_report_read_before_the_key_was_passed_on_is_not_its_evidence() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    // The before read ended at 500 ms; this read ended at 400 ms.
    let effects = reduce(
        &mut state,
        &report_ended(line("abc\n", 100, 1), true, 400_000),
    );
    assert_eq!(spoken(&effects), []);
    assert!(state.holds_inputs_back(), "the key still waits");
}

#[test]
fn a_report_read_after_the_key_was_passed_on_with_no_effect_is_not_its_evidence() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(
        &mut state,
        &report_ended(line("abc\n", 100, 0), false, 900_000),
    );
    assert_eq!(spoken(&effects), []);
    assert!(state.holds_inputs_back(), "the key still waits");
}

#[test]
fn a_report_of_another_node_is_not_the_keys_evidence() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let moved = event(NormalizedEvent::CaretMoved {
        node_id: id(6),
        caret: CaretReport {
            line: line("x\n", 100, 1),
            selection: None,
            read_at_us: 900_000,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: Some(Box::new(key_facts(
            line("x\n", 100, 1),
            true,
            (line("x\n", 100, 0), None),
            true,
        ))),
    });
    assert_eq!(spoken(&reduce(&mut state, &moved)), []);
    assert!(state.holds_inputs_back(), "the key still waits");
}

#[test]
fn a_report_read_after_the_pass_showing_a_change_is_the_keys_evidence() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let effects = reduce(
        &mut state,
        &report_ended(line("abc\n", 100, 1), true, 900_000),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);
    assert!(!state.holds_inputs_back(), "the key ended");
}

/// How many one-way terminal reads `effects` ask.
fn terminal_reads(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(own(effect), Effect::ReadTerminal(_)))
        .count()
}

/// A held key passed on over a field of `role`, in a window whose console
/// flag is `console`; what the pass did.
fn passed_on_over(role: Role, console: bool) -> Vec<Effect> {
    let mut state = SrState::new();
    focus(&mut state, node(5, role, StateSet::new()));
    let window = verbatim_model::WindowHandle(0x100);
    let _ = reduce(
        &mut state,
        &Input::Foreground(verbatim_model::ForegroundWindow {
            window,
            root_owner: window,
            process: Pid(7),
            observed_at_us: 1,
            own: false,
            thread: 1,
            console,
            integrity: None,
            own_integrity: None,
        }),
    );
    let query = request_of(&reduce(
        &mut state,
        &held_at(TraceId::from_raw(2), CaretMotion::NextCharacter, 499),
    ));
    reduce(
        &mut state,
        &completed(query, read_before(line("abc\n", 100, 0), 500)),
    )
}

#[test]
fn a_key_passed_on_in_a_terminal_asks_one_read_and_only_there() {
    assert_eq!(terminal_reads(&passed_on_over(Role::Terminal, false)), 1);
    assert_eq!(terminal_reads(&passed_on_over(Role::Terminal, true)), 0);
    assert_eq!(
        terminal_reads(&passed_on_over(Role::EditableText, false)),
        0
    );
}

#[test]
fn evidence_with_no_unit_waits_for_the_final_reads_unit() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextLine);
    let moved = event(NormalizedEvent::CaretMoved {
        node_id: id(5),
        caret: CaretReport {
            line: line("next\n", 101, 0),
            selection: None,
            read_at_us: 900_000,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: Some(Box::new(ReadFacts {
            unit: None,
            ..key_facts(
                line("next\n", 101, 0),
                true,
                (line("abc\n", 100, 0), None),
                true,
            )
        })),
    });
    assert_eq!(spoken(&reduce(&mut state, &moved)), []);
    let effects = bound_answer(&mut state, line("next\n", 101, 0));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("next")]);
}

#[test]
fn a_final_read_showing_an_effect_is_the_keys_evidence() {
    let (mut state, _) = key_passed_on("abc\n", 0, CaretMotion::NextCharacter);
    let due = reduce(&mut state, &key_bound_passed());
    let reply = TextReply::Caret(Box::new(verbatim_model::CaretReply {
        caret: CaretReport {
            line: line("abc\n", 100, 2),
            selection: None,
            read_at_us: 700_000,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
        facts: ReadFacts {
            read_ended_at_us: 700_000,
            ..key_facts(
                line("abc\n", 100, 2),
                true,
                (line("abc\n", 100, 0), None),
                true,
            )
        },
    }));
    let effects = reduce(&mut state, &completed_text(request_of(&due), reply));
    assert_eq!(spoken(&effects), vec![character("c")]);
}
