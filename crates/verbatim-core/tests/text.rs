//! Scripted-input tests for milestone M4's text handling in the reducer:
//! caret keys and the selection, typed character and word echo, the
//! review cursor over text, say-all, and formatting spoken as it changes,
//! each driven through the text protocol with scripted outpost replies.

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReply, CaretReport, CaretWatch, Effect, Input, Message,
    NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, QueryId,
    ReaderSettings, ReviewCommand, Role, SegmentContent, Selection, SelectionChange, SelectionText,
    SpeechMark, State, StateSet, TextAnchor, TextChunk, TextMovement, TextOp, TextPoint,
    TextPosition, TextRead, TextReadAhead, TextReply, TextRequest, TextThrough, TextUnit, TraceId,
    TypingEcho, UtteranceSegment,
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
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
    }
}

fn focus(state: &mut SrState, snapshot: NodeSnapshot) {
    let _ = reduce(
        state,
        &event(NormalizedEvent::FocusChanged {
            node: snapshot,
            foreground: false,
            ancestors: Vec::new(),
            ancestors_unknown: false,
            selected_child: None,
            text: None,
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
    let _ = reduce(
        state,
        &event(NormalizedEvent::CaretMoved {
            node_id: id(number),
            caret: CaretReport {
                line,
                selection: None,
                selection_line: None,
            },
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
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
        trace_id: TraceId::mint(),
        key: CaretKey { motion, select },
        pressed_at_ms: KEY_PRESSED_AT,
    }
}

fn command(command: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        trace_id: TraceId::mint(),
        command,
        repeat,
    }
}

fn completed(query_id: QueryId, reply: TextReply) -> Input {
    Input::TextCompleted {
        trace_id: TraceId::mint(),
        query_id,
        reply,
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
            Effect::Speak(utterance) => Some(utterance.segments.clone()),
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

fn caret_reply(moved: bool, line: TextChunk, unit: Option<TextChunk>) -> TextReply {
    TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved,
        read_at_ms: 0,
        caret: CaretReport {
            line,
            selection: None,
            selection_line: None,
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
    let TextOp::AwaitCaret(watch) = &request.op else {
        panic!("expected a caret wait, got {:?}", request.op);
    };
    assert_eq!(
        watch.since,
        Some(TextPosition {
            anchor: TextAnchor(100),
            offset: 0
        })
    );
    // When the key was pressed, for the outpost to tell which of its caret
    // reports came before it.
    assert_eq!(watch.pressed_at_ms, KEY_PRESSED_AT);
    assert_eq!(watch.unit, TextUnit::Character);

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
    assert_eq!(
        request.op,
        TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(TextPosition {
                anchor: TextAnchor(100),
                offset: 5
            }),
            pressed_at_ms: KEY_PRESSED_AT,
            unit: TextUnit::Character,
            compare: None,
            deletes: true,
        })
    );
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
fn a_caret_key_that_moves_nothing_is_silent() {
    // Control+Home at the start of the text: the application moves nothing
    // and raises nothing, so the outpost's watch ends without evidence,
    // when the next key replaces it, the focus moves, or its bound passes,
    // and is answered `WatchEnded`. Nothing is said, unlike NVDA, which
    // reads the line after its 100 ms wait.
    let mut state = editing("one two\n", 0);
    let effects = reduce(&mut state, &key(CaretMotion::Top, false));
    let request = request(&effects);
    assert_eq!(
        request.op,
        TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(TextPosition {
                anchor: TextAnchor(100),
                offset: 0
            }),
            pressed_at_ms: KEY_PRESSED_AT,
            unit: TextUnit::Line,
            compare: None,
            deletes: false,
        })
    );
    let effects = reduce(
        &mut state,
        &completed(request.query_id, TextReply::WatchEnded),
    );
    assert_eq!(effects, []);
    // A reply without evidence says nothing either.
    let effects = reduce(&mut state, &key(CaretMotion::Top, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(false, line("one two\n", 100, 0), None),
        ),
    );
    assert_eq!(effects, []);
}

#[test]
fn a_caret_keys_watch_ends_with_its_answer_and_with_typing() {
    // Once a watch is answered, ended or not, the key waits for nothing: a
    // second answer to the same request says nothing.
    let mut state = editing("abc\n", 0);
    let query = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let effects = reduce(&mut state, &completed(query, TextReply::WatchEnded));
    assert_eq!(effects, []);
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
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 50 });
    let effects = reduce(&mut state, &typed("x"));
    assert_eq!(spoken(&effects), vec![character("x")]);
    let effects = reduce(
        &mut state,
        &completed(query, caret_reply_at(line("xabc\n", 100, 1), 60)),
    );
    assert_eq!(
        effects,
        [],
        "the later key's effect is not the key's answer"
    );
    // Its caret is now the newest, so a Backspace deletes what it shows.
    let query = request_of(&reduce(
        &mut state,
        &Input::CaretKey {
            trace_id: TraceId::mint(),
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
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 50 });
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
    let TextOp::AwaitCaret(watch) = request(&effects).op else {
        panic!("expected a caret wait");
    };
    assert_eq!(watch.unit, TextUnit::Paragraph);
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
    assert!(matches!(request.op, TextOp::AwaitCaret(_)));
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello, world\n", 100, 6),
            selection: None,
            selection_line: None,
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(
                "hello, world
",
                100,
                0,
            ),
            selection: Some(selected),
            selection_line: None,
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
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
            selection_line: None,
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(
                "hello, world
",
                100,
                5,
            ),
            selection: Some(selected),
            selection_line: None,
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Delete, false));
    assert!(matches!(request(&effects).op, TextOp::AwaitCaret(_)));
}

/// A caret event for node 5 observed at `observed_at_ms`.
fn caret_event_at(state: &mut SrState, line: TextChunk, observed_at_ms: u64) -> Vec<Effect> {
    reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::CaretMoved {
                node_id: id(5),
                caret: CaretReport {
                    line,
                    selection: None,
                    selection_line: None,
                },
                selection_changes: Vec::new(),
                content_changed: false,
                new_selection: None,
            },
        },
    )
}

/// A caret key pressed at `pressed_at_ms`.
fn key_at(motion: CaretMotion, pressed_at_ms: u64) -> Input {
    Input::CaretKey {
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms,
    }
}

/// A caret reply the outpost read at `read_at_ms`.
fn caret_reply_at(line: TextChunk, read_at_ms: u64) -> TextReply {
    TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        caret: CaretReport {
            line,
            selection: None,
            selection_line: None,
        },
        read_at_ms,
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }))
}

#[test]
fn backspace_deletes_from_the_caret_it_found_though_its_own_caret_event_came_first() {
    // From a flight recorder, Notepad under load: "xy" typed after "delta
    // epsilon", then two Backspaces. The second key's path to Core was
    // slower than Notepad's caret event for what it did.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("delta epsilonxy", 100, 15), 300);
    let effects = reduce(&mut state, &key_at(CaretMotion::Backspace, 400));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("delta epsilonx", 100, 14), 420),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("y")]);
    let _ = caret_event_at(&mut state, line("delta epsilonx", 100, 14), 491);
    // The second Backspace's own caret event reaches Core before the key.
    let _ = caret_event_at(&mut state, line("delta epsilon", 100, 13), 611);
    let effects = reduce(&mut state, &key_at(CaretMotion::Backspace, 603));
    let asked = request(&effects);
    let TextOp::AwaitCaret(watch) = &asked.op else {
        panic!("expected a caret wait, got {:?}", asked.op);
    };
    // The caret the key found, not the one it left.
    assert_eq!(
        watch.since,
        Some(TextPosition {
            anchor: TextAnchor(100),
            offset: 14
        })
    );
    let effects = reduce(
        &mut state,
        &completed(
            asked.query_id,
            caret_reply_at(line("delta epsilon", 100, 13), 615),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("x")]);

    // Delete compares the text the key found at the caret: "a", not the
    // "b" its own caret event already shows.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("abc", 100, 0), 100);
    let _ = caret_event_at(&mut state, line("bc", 100, 0), 210);
    let effects = reduce(&mut state, &key_at(CaretMotion::Delete, 200));
    let TextOp::AwaitCaret(watch) = request(&effects).op else {
        panic!("expected a caret wait");
    };
    assert_eq!(watch.compare.as_deref(), Some("a"));
}

#[test]
fn backspace_deletes_from_the_caret_event_of_the_key_before_it() {
    // The first Backspace's caret event, observed before the second key was
    // pressed, is where the second key found the caret, though the first
    // key's reply, read earlier, still showed "xy".
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("delta epsilonxy", 100, 15), 300);
    let effects = reduce(&mut state, &key_at(CaretMotion::Backspace, 400));
    let _ = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("delta epsilonxy", 100, 15), 401),
        ),
    );
    let _ = caret_event_at(&mut state, line("delta epsilonx", 100, 14), 491);
    let effects = reduce(&mut state, &key_at(CaretMotion::Backspace, 603));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("delta epsilon", 100, 13), 615),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("x")]);

    // A report observed in the same millisecond as the key may already show
    // what it did, so it is not the caret the key found.
    let _ = caret_event_at(&mut state, line("delta epsilo", 100, 12), 700);
    let effects = reduce(&mut state, &key_at(CaretMotion::Backspace, 700));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("delta epsilo", 100, 12), 705),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("n")]);
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
    // A key after both finds the newest caret.
    let effects = reduce(&mut state, &key_at(CaretMotion::NextCharacter, 600));
    let TextOp::AwaitCaret(watch) = request(&effects).op else {
        panic!("expected a caret wait");
    };
    assert_eq!(
        watch.since,
        Some(TextPosition {
            anchor: TextAnchor(100),
            offset: 2
        })
    );
}

#[test]
fn a_review_command_reads_the_live_caret_though_a_caret_keys_event_comes_late() {
    // Down Arrow, then the current line read before the application's
    // caret event for Down Arrow reaches Core.
    let mut state = reviewing("one\n", 0);
    let down = reduce(&mut state, &key(CaretMotion::NextLine, false));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewCurrentLine, 0));
    // Core still holds the caret on "one"; the outpost reads where it is.
    let effects = read_at_caret(&mut state, &effects, line("two\n", 101, 0));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("two")]);
    // The next command starts from the line that read found.
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
    // Down Arrow's own event and answer arrive late, and the review
    // cursor follows the caret again.
    caret_event(&mut state, 5, line("two\n", 101, 0));
    let _ = reduce(
        &mut state,
        &completed(
            request_of(&down),
            caret_reply(true, line("two\n", 101, 0), None),
        ),
    );
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    let effects = read_at_caret(&mut state, &effects, line("two\n", 101, 0));
    assert_eq!(spoken(&effects), vec![character("t")]);
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
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(false, line("abc\n", 100, 0), None),
        ),
    );
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
    let TextOp::AwaitCaret(watch) = request(&effects).op else {
        panic!("expected a caret wait");
    };
    assert_eq!(watch.compare.as_deref(), Some("b"));
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
    reduce(
        state,
        &event(NormalizedEvent::CaretMoved {
            node_id: id(5),
            caret: CaretReport {
                line,
                selection,
                selection_line: None,
            },
            selection_changes: changes,
            content_changed,
            new_selection: None,
        }),
    )
}

/// The answer to a caret key: `line`, nothing selected, the selection
/// removed in general terms.
fn selection_removed_reply(line: TextChunk) -> TextReply {
    TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line,
            selection: None,
            selection_line: None,
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
        let _ = reduce(
            &mut state,
            &event(NormalizedEvent::CaretMoved {
                node_id: id(5),
                caret: CaretReport {
                    line: line(text, 100, 0),
                    selection: Some(Selection {
                        start: TextPosition {
                            anchor: TextAnchor(300),
                            offset: 11,
                        },
                        end: TextPosition {
                            anchor: TextAnchor(201),
                            offset: 0,
                        },
                    }),
                    selection_line: Some(Box::new(line(text, 300, 11))),
                },
                selection_changes: Vec::new(),
                content_changed: false,
                new_selection: None,
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
    let effects = reduce(&mut state, &completed(asked, TextReply::WatchEnded));
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
            let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 50 });
        }
        let effects = reduce(
            &mut state,
            &completed(asked, caret_reply(later, line(after, 100, 6), None)),
        );
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

/// A key's answer that compared the selection with the one the outpost
/// kept is the only one to carry that change, which no later answer
/// carries again: Control+Backspace over the first word, which Notepad
/// answers with nothing, then Backspace over it, whose effect the first
/// key's watch may be answered with
/// (`target/e2e-evidence/m42-selection-fixes-2026-10-22/`). The first
/// key's answer, superseded by the second key, says nothing itself, and
/// its change is said after the second key's words. An answer read once a
/// later key that is not a caret key was pressed says its change too.
#[test]
fn a_superseded_answers_selection_change_is_said_after_the_next_keys_words() {
    let text = "alpha beta gamma\r\n";
    let first = TextChunk {
        first: true,
        ..line(text, 100, 6)
    };
    let mut state = editing(text, 0);
    let _ = selection_event(
        &mut state,
        (first, Some(selection_from(0, true))),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::BackspaceWord, false));
    let superseded = request_of(&effects);
    let effects = reduce(&mut state, &key(CaretMotion::Backspace, false));
    let answered = request_of(&effects);
    let effects = reduce(
        &mut state,
        &completed(
            superseded,
            selection_removed_reply(line("beta gamma\r\n", 100, 0)),
        ),
    );
    assert_eq!(spoken(&effects), []);
    let effects = reduce(
        &mut state,
        &completed(
            answered,
            caret_reply(true, line("beta gamma\r\n", 100, 0), None),
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![message(Message::Blank), message(Message::SelectionRemoved)]
    );

    let mut state = editing(text, 0);
    let _ = selection_event(
        &mut state,
        (line(text, 100, 6), Some(selection_from(6, true))),
        (Vec::new(), false),
    );
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 50 });
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            selection_removed_reply(line("alpha gamma\r\n", 100, 6)),
        ),
    );
    assert_eq!(spoken(&effects), vec![message(Message::SelectionRemoved)]);
}

#[test]
fn a_newer_key_or_a_focus_change_supersedes_a_waiting_key() {
    let mut state = editing("abc\n", 0);
    let first = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let second = request_of(&reduce(&mut state, &key(CaretMotion::NextCharacter, false)));
    let effects = reduce(
        &mut state,
        &completed(first, caret_reply(true, line("abc\n", 100, 1), None)),
    );
    assert!(effects.is_empty(), "superseded: {effects:?}");
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
    let TextOp::AwaitCaret(watch) =
        request(&reduce(&mut state, &key(CaretMotion::PreviousLine, false))).op
    else {
        panic!("expected a caret watch");
    };
    assert!(watch.since.is_none(), "no caret known yet");

    let mut state = SrState::new();
    focus(&mut state, node(8, Role::Button, StateSet::new()));
    assert_eq!(
        reduce(&mut state, &key(CaretMotion::PreviousLine, false)),
        []
    );
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        trace_id: TraceId::mint(),
        text: text.to_owned(),
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
fn a_terminal_holds_typing_until_its_text_changes() {
    let mut state = SrState::new();
    focus(&mut state, node(12, Role::Terminal, StateSet::new()));
    assert_eq!(spoken(&reduce(&mut state, &typed("l"))), []);
    let changed = event(NormalizedEvent::TextChanged { node_id: id(12) });
    assert_eq!(spoken(&reduce(&mut state, &changed)), vec![character("l")]);

    // A password prompt shows nothing: Enter drops what was held.
    assert_eq!(spoken(&reduce(&mut state, &typed("s"))), []);
    assert_eq!(spoken(&reduce(&mut state, &typed("\r"))), []);
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

#[test]
fn a_terminal_keeps_a_cell_column_and_reads_past_the_text_as_blank() {
    let mut state = SrState::new();
    focus(&mut state, node(13, Role::Terminal, StateSet::new()));
    // The caret on 中 at cells 2 and 3 of a padded row.
    caret_event(&mut state, 13, line("ab中文cd    \r\n", 200, 2));
    settle(&mut state, line("ab中文cd    \r\n", 200, 2));
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextCharacter, 0));
    assert_eq!(spoken(&effects), vec![character("文")]);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewNextLine, 0));
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
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewPreviousLine, 0));
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
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
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

/// The index mark and text of each say-all utterance among `effects`, each
/// checked to be marked as read by say-all, so the theme's "play sounds
/// during say all" setting applies to it. Any utterance of another shape
/// fails the test, so no speech is skipped unchecked.
fn say_all_pieces(effects: &[Effect]) -> Vec<(SpeechMark, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance) => match utterance.segments.as_slice() {
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
    let effects = reduce(&mut state, &Input::SpeechCancelled { at_ms: 0 });
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    assert_eq!(reduce(&mut state, &reached(third[0].0)), vec![]);
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
            at_ms: KEY_PRESSED_AT - 2,
        },
    );
    let _ = caret_event_at(&mut state, line("Two.\n", 102, 0), KEY_PRESSED_AT);
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, false));
    assert_eq!(
        request(&effects).op,
        TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(TextPosition::at(TextAnchor(101))),
            pressed_at_ms: KEY_PRESSED_AT,
            unit: TextUnit::Character,
            compare: None,
            deletes: false,
        })
    );
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
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 0 });
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
            Effect::Speak(utterance) => {
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
    let effects = reduce(&mut state, &Input::SpeechCancelled { at_ms: 0 });
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
            selection_line: None,
        },
        selected_text: None,
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
    let _ = reduce(&mut state, &Input::OutpostEnded { outpost: OUTPOST });
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
            foreground: false,
            ancestors: Vec::new(),
            ancestors_unknown: false,
            selected_child: None,
            text: Some(Box::new(caret_at_anchor(200))),
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
            foreground: false,
            ancestors: Vec::new(),
            ancestors_unknown: false,
            selected_child: None,
            text: Some(Box::new(text)),
        }),
    );
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance) => Some(utterance.segments.clone()),
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
            selection,
            selection_line: None,
        },
        selected_text: selected_text.map(str::to_owned),
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
            selection,
            selection_line: None,
        },
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
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
            foreground: false,
            ancestors: Vec::new(),
            ancestors_unknown: false,
            selected_child: None,
            text: Some(Box::new(caret_read(
                Some(selection),
                "delta epsilon\r",
                Some("delta epsilon"),
            ))),
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
        trace_id: TraceId::mint(),
        observed_at_ms,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::CaretMoved {
            node_id: id(5),
            caret: CaretReport {
                line: line("alpha beta\r", 100, 5),
                selection,
                selection_line: None,
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
        },
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
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
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
        },
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
/// general words, each with the whole new selection.
#[test]
fn a_selection_replaced_after_a_text_change_says_selected_then_selected_instead() {
    let mut state = announced_field();
    let effects = reduce(
        &mut state,
        &general_report(Some(alpha_selected()), &[false, true], Some("alpha")),
    );
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello, world\n", 100, 6),
            selection: Some(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 5,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 6,
                },
            }),
            selection_line: None,
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello world\n", 100, 0),
            selection: Some(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 0,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 11,
                },
            }),
            selection_line: None,
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: Some(true),
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("\n", 101, 0),
            selection: None,
            selection_line: None,
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
    assert_eq!(effects.len(), 1, "one effect in {effects:?}");
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
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line(text, 100, 0),
            selection: Some(Selection {
                start: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 0,
                },
                end: TextPosition {
                    anchor: TextAnchor(100),
                    offset: 24,
                },
            }),
            selection_line: None,
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Other, false));
    let reply = TextReply::Caret(Box::new(CaretReply {
        same_line: Some(true),
        removed: None,
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("\n", 100, 0),
            selection: None,
            selection_line: None,
        },
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }));
    let effects = reduce(&mut state, &completed(request_of(&effects), reply));
    assert_eq!(effects, []);
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
                selection_line: None,
            },
            selected_text: None,
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
        matches!(effects.as_slice(), [Effect::Speak(_)]),
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
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
            result: verbatim_model::FetchResult::Node(object),
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

#[test]
fn reporting_the_focus_reads_the_focus_wherever_the_navigator_is() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.value = Some("one two".to_owned());
    focus(&mut state, edit);
    caret_event(&mut state, 5, line("one two\n", 100, 4));
    let _ = navigate_to(&mut state, node(31, Role::Button, StateSet::new()));

    // The focus is announced as a query, its known caret line in place of
    // its value, and the navigator stays where it was.
    let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, 0));
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

    // A focus whose caret Core does not know is asked for its text, and the
    // answer is spoken although the navigator is elsewhere.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = navigate_to(&mut state, node(31, Role::Button, StateSet::new()));
    let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, 0));
    let selection = request(&effects);
    assert_eq!(selection.node_id, id(5));
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

#[test]
fn reporting_the_focus_again_spells_its_name_alone() {
    let mut state = SrState::new();
    let mut edit = node(5, Role::EditableText, StateSet::new());
    edit.name = Some("Ab".to_owned());
    focus(&mut state, edit);
    caret_event(&mut state, 5, line("cd\n", 100, 1));

    // The second press spells the name, not the caret's line.
    let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, 1));
    assert_eq!(
        spoken(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::SpelledCapital("A".to_owned())),
            UtteranceSegment::text("b"),
        ]
    );

    // The third and later spell it with descriptions, and copy nothing.
    for repeat in [2, 3] {
        let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, repeat));
        assert_eq!(
            spoken(&effects),
            vec![
                UtteranceSegment::new(SegmentContent::CharacterDescription("A".to_owned())),
                UtteranceSegment::new(SegmentContent::CharacterDescription("b".to_owned())),
            ]
        );
    }

    // A focus with no name is "blank".
    let mut state = SrState::new();
    let mut unnamed = node(5, Role::EditableText, StateSet::new());
    unnamed.name = None;
    focus(&mut state, unnamed);
    let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, 1));
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
}

#[test]
fn reporting_the_focus_with_none_says_no_focus() {
    let mut state = SrState::new();
    let effects = reduce(&mut state, &command(ReviewCommand::ReportFocus, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoFocus)]);
}

/// A caret key the keyboard hook held back, pressed at `pressed_at_ms`.
fn held_at(trace_id: TraceId, motion: CaretMotion, pressed_at_ms: u64) -> Input {
    Input::HeldKey {
        trace_id,
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms,
    }
}

/// The caret read before a held key, read at `read_at_ms`.
fn read_before(line: TextChunk, read_at_ms: u64) -> TextReply {
    TextReply::Caret(Box::new(CaretReply {
        same_line: None,
        removed: None,
        moved: false,
        caret: CaretReport {
            line,
            selection: None,
            selection_line: None,
        },
        read_at_ms,
        unit: None,
        selection_changes: Vec::new(),
        content_changed: false,
        new_selection: None,
    }))
}

#[test]
fn a_held_key_is_passed_on_once_its_caret_is_read_and_judged_from_that_caret() {
    // Core last heard of the caret at the line's start; the application
    // has since moved it to "l" without reporting it.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = caret_event_at(&mut state, line("Hello", 100, 0), 300);
    let trace = TraceId::mint();
    let effects = reduce(&mut state, &held_at(trace, CaretMotion::NextCharacter, 400));
    // Nothing is passed on before the caret is read.
    let asked = request(&effects);
    assert_eq!(effects.len(), 1);
    assert_eq!(asked.op, TextOp::ReadCaret);
    let effects = reduce(
        &mut state,
        &completed(asked.query_id, read_before(line("Hello", 101, 3), 410)),
    );
    // The key is watched from the caret read, taken as pressed just after
    // the read, and then passed on, so the watch reaches the outpost
    // before the key reaches the application.
    assert_eq!(effects.last(), Some(&Effect::PassHeldKey(trace)));
    let TextOp::AwaitCaret(watch) = request(&effects).op else {
        panic!("expected a caret wait, got {effects:?}");
    };
    assert_eq!(
        watch.since,
        Some(TextPosition {
            anchor: TextAnchor(101),
            offset: 3
        })
    );
    assert_eq!(watch.pressed_at_ms, 411);
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply_at(line("Hello", 101, 4), 420),
        ),
    );
    assert_eq!(spoken(&effects), vec![character("o")]);
}

#[test]
fn held_keys_are_passed_on_in_order_whatever_their_reads_answer() {
    let mut state = editing("Hello", 0);
    let (first, second) = (TraceId::mint(), TraceId::mint());
    let effects = reduce(&mut state, &held_at(first, CaretMotion::NextCharacter, 400));
    let reading = request(&effects).query_id;
    // A key pressed while the first one's caret is read waits behind it.
    let effects = reduce(
        &mut state,
        &held_at(second, CaretMotion::NextCharacter, 405),
    );
    assert_eq!(effects, []);
    // A read that failed, or passed its bound, still passes the key on,
    // watched as a key the hook passed itself, the watch asked for first.
    // The second key waits for the first key's watch, no longer than 100 ms
    // after the first key was passed on.
    let effects = reduce(&mut state, &completed(reading, TextReply::Unanswered));
    let [
        Effect::Text(watched),
        Effect::PassHeldKey(passed),
        Effect::BoundHeldKeyWait { key, within_ms },
    ] = effects.as_slice()
    else {
        panic!(
            "expected the first key's watch, then the key passed on, then the second key's wait: {effects:?}"
        );
    };
    assert_eq!((*passed, *key, *within_ms), (first, second, 95));
    let TextOp::AwaitCaret(watch) = &watched.op else {
        panic!("expected the first key's watch: {watched:?}");
    };
    assert_eq!(watch.pressed_at_ms, 400);
    // The next key's caret is read once the first key's watch is answered,
    // here having ended without evidence.
    let effects = reduce(
        &mut state,
        &completed(watched.query_id, TextReply::WatchEnded),
    );
    // The second key's read is made on its own key's behalf, though the
    // first key's answer asked for it, so its answer and speech are its own.
    let [Effect::ForKey(key, read)] = effects.as_slice() else {
        panic!("expected the second key's read alone: {effects:?}");
    };
    let Effect::Text(read) = &**read else {
        panic!("expected the second key's read: {read:?}");
    };
    assert_eq!(read.op, TextOp::ReadCaret);
    assert_eq!(*key, second);
    let effects = reduce(
        &mut state,
        &completed(read.query_id, read_before(line("Hello", 101, 1), 430)),
    );
    assert_eq!(effects.last(), Some(&Effect::PassHeldKey(second)));

    // With no focus whose caret is read, a held key is passed on at once.
    let mut state = SrState::new();
    focus(&mut state, node(6, Role::Button, StateSet::new()));
    let third = TraceId::mint();
    let effects = reduce(&mut state, &held_at(third, CaretMotion::NextCharacter, 500));
    assert_eq!(
        effects,
        [Effect::ForKey(third, Box::new(Effect::PassHeldKey(third)))]
    );
}

#[test]
fn keys_pressed_behind_a_held_key_are_handled_after_it_in_order() {
    // A held Backspace, then, while its caret is read, a typed "c" and a
    // held Right Arrow: the "c" is echoed only once Backspace was passed
    // on, on its own trace, and Right Arrow's caret is read once
    // Backspace's watch has been answered.
    let mut state = editing("ab", 2);
    let (backspace, letter, right) = (TraceId::mint(), TraceId::mint(), TraceId::mint());
    let effects = reduce(&mut state, &held_at(backspace, CaretMotion::Backspace, 400));
    let reading = request(&effects).query_id;
    let typed_c = Input::CharacterTyped {
        trace_id: letter,
        text: "c".to_owned(),
    };
    assert_eq!(reduce(&mut state, &typed_c), []);
    assert_eq!(
        reduce(&mut state, &held_at(right, CaretMotion::NextCharacter, 403)),
        []
    );
    let effects = reduce(
        &mut state,
        &completed(reading, read_before(line("ab", 101, 2), 410)),
    );
    let order: Vec<String> = effects
        .iter()
        .map(|effect| match effect {
            Effect::PassHeldKey(key) if *key == backspace => "pass Backspace".to_owned(),
            Effect::Text(request) if matches!(request.op, TextOp::AwaitCaret(_)) => {
                "watch Backspace".to_owned()
            }
            Effect::ForKey(key, effect) if *key == letter => match &**effect {
                Effect::Speak(utterance) => format!("echo {:?}", utterance.segments),
                other => format!("letter's {other:?}"),
            },
            Effect::ForKey(key, effect) if *key == right => match &**effect {
                Effect::Text(request) if request.op == TextOp::ReadCaret => {
                    "read before Right".to_owned()
                }
                other => format!("Right's {other:?}"),
            },
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        order,
        [
            "watch Backspace".to_owned(),
            "pass Backspace".to_owned(),
            format!("echo {:?}", vec![character("c")]),
            format!(
                "{:?}",
                Effect::BoundHeldKeyWait {
                    key: right,
                    within_ms: 100
                }
            ),
        ]
    );
    let effects = reduce(
        &mut state,
        &completed(request_of(&effects), TextReply::WatchEnded),
    );
    let [Effect::ForKey(key, read)] = effects.as_slice() else {
        panic!("expected Right Arrow's read alone: {effects:?}");
    };
    assert_eq!(*key, right);
    assert!(matches!(&**read, Effect::Text(request) if request.op == TextOp::ReadCaret));
}

#[test]
fn a_held_key_waits_for_the_key_aheads_answer_no_longer_than_the_bound() {
    // Right Arrow, held, read and passed on at 411: its watch has no
    // answer yet, as for a key that moved nothing.
    let mut state = editing("Hello", 3);
    let (first, second, third) = (TraceId::mint(), TraceId::mint(), TraceId::mint());
    let effects = reduce(&mut state, &held_at(first, CaretMotion::NextCharacter, 400));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            read_before(line("Hello", 101, 3), 410),
        ),
    );
    assert_eq!(effects.last(), Some(&Effect::PassHeldKey(first)));
    let first_watch = request_of(&effects);
    // A second Right Arrow pressed 20 ms later waits for that answer, and
    // the shell is asked to end the wait 100 ms after the first key was
    // passed on; a third pressed behind it asks nothing more.
    let effects = reduce(
        &mut state,
        &held_at(second, CaretMotion::NextCharacter, 431),
    );
    assert_eq!(
        effects,
        [Effect::BoundHeldKeyWait {
            key: second,
            within_ms: 80
        }]
    );
    assert_eq!(
        reduce(&mut state, &held_at(third, CaretMotion::NextCharacter, 441)),
        []
    );
    // The end of a wait for a key that is not waiting changes nothing.
    assert_eq!(
        reduce(&mut state, &Input::HeldKeyWaitEnded { trace_id: third }),
        []
    );
    // At the bound, the second key's caret is read, on its own trace.
    let effects = reduce(&mut state, &Input::HeldKeyWaitEnded { trace_id: second });
    let [Effect::ForKey(key, read)] = effects.as_slice() else {
        panic!("expected the second key's read alone: {effects:?}");
    };
    assert_eq!(*key, second);
    let Effect::Text(read) = &**read else {
        panic!("expected the second key's read: {read:?}");
    };
    assert_eq!(read.op, TextOp::ReadCaret);
    // The first key's watch was marked as having a later key, so its late
    // answer keeps the caret current and says nothing.
    let effects = reduce(
        &mut state,
        &completed(first_watch, caret_reply_at(line("Hello", 101, 4), 520)),
    );
    assert_eq!(effects, []);
    // The second key is passed on, and the third waits for its watch, the
    // wait bounded 100 ms from the second's pass-on.
    let effects = reduce(
        &mut state,
        &completed(read.query_id, read_before(line("Hello", 101, 4), 530)),
    );
    let [
        Effect::Text(watched),
        Effect::PassHeldKey(passed),
        Effect::BoundHeldKeyWait { key, within_ms },
    ] = effects.as_slice()
    else {
        panic!("expected the second key's watch and pass-on, then the third's wait: {effects:?}");
    };
    assert!(matches!(watched.op, TextOp::AwaitCaret(_)));
    assert_eq!((*passed, *key, *within_ms), (second, third, 100));

    // A held key pressed once the bound has run out is read at once.
    let mut state = editing("Hello", 3);
    let effects = reduce(&mut state, &held_at(first, CaretMotion::NextCharacter, 400));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            read_before(line("Hello", 101, 3), 410),
        ),
    );
    assert_eq!(effects.last(), Some(&Effect::PassHeldKey(first)));
    let effects = reduce(
        &mut state,
        &held_at(second, CaretMotion::NextCharacter, 511),
    );
    let [Effect::ForKey(key, read)] = effects.as_slice() else {
        panic!("expected the second key's read alone: {effects:?}");
    };
    assert_eq!(*key, second);
    assert!(matches!(&**read, Effect::Text(request) if request.op == TextOp::ReadCaret));
}
