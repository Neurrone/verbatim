//! Scripted-input tests for milestone M4's text handling in the reducer:
//! caret keys and the selection, typed character and word echo, the
//! review cursor over text, say-all, and formatting spoken as it changes,
//! each driven through the text protocol with scripted outpost replies.

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReply, CaretReport, CaretWait, Effect, Input, Message,
    NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, PreviousSelection,
    QueryId, ReaderSettings, ReviewCommand, Role, SegmentContent, Selection, SelectionChange,
    SelectionText, SpeechMark, State, StateSet, TextAnchor, TextChunk, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReadAhead, TextReply, TextRequest, TextUnit, TraceId,
    TypingEcho, UtteranceSegment,
};
use verbatim_model::{FormatRun, TextAttributes, TextFormat};

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
            },
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
        pressed_at_ms: 0,
    }
}

fn completed(query_id: QueryId, reply: TextReply) -> Input {
    Input::TextCompleted {
        trace_id: TraceId::mint(),
        query_id,
        reply,
    }
}

/// The one text request among `effects`.
fn request(effects: &[Effect]) -> TextRequest {
    let requests: Vec<&TextRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
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
        .filter_map(|effect| match effect {
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
        moved,
        read_at_ms: 0,
        caret: CaretReport {
            line,
            selection: None,
        },
        unit,
        selection_changes: Vec::new(),
    }))
}

#[test]
fn an_arrow_key_waits_for_evidence_then_speaks_the_character_at_the_caret() {
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
    assert_eq!(watch.wait, CaretWait::Standard);
    assert!(watch.previous_selection.is_none());

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
    let mut state = editing("the river bank.\r\n", 11);
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
    let TextOp::AwaitCaret(watch) = &request.op else {
        panic!("expected a caret wait");
    };
    let caret = TextPosition {
        anchor: TextAnchor(100),
        offset: 5,
    };
    assert_eq!(
        watch.previous_selection,
        Some(PreviousSelection {
            start: caret,
            end: caret
        })
    );
    let reply = TextReply::Caret(Box::new(CaretReply {
        moved: true,
        read_at_ms: 0,
        caret: CaretReport {
            line: line("hello, world\n", 100, 6),
            selection: None,
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
        },
        unit: None,
        selection_changes: Vec::new(),
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));

    // Right Arrow asks how the selection changed from it, and speaks the
    // character, then the text unselected.
    let effects = reduce(&mut state, &key(CaretMotion::NextCharacter, false));
    let asked = request(&effects);
    let TextOp::AwaitCaret(watch) = &asked.op else {
        panic!("expected a caret wait");
    };
    assert_eq!(
        watch.previous_selection,
        Some(PreviousSelection {
            start: at(0),
            end: at(5)
        })
    );
    let reply = TextReply::Caret(Box::new(CaretReply {
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
        },
        unit: None,
        selection_changes: vec![SelectionChange {
            selected: false,
            text: "hello".to_owned(),
            characters: 5,
        }],
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

    // A deletion replaces a selection rather than unselecting it.
    let mut state = editing(
        "hello, world
",
        0,
    );
    let effects = reduce(&mut state, &key(CaretMotion::NextWord, true));
    let reply = TextReply::Caret(Box::new(CaretReply {
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
        },
        unit: None,
        selection_changes: Vec::new(),
    }));
    let _ = reduce(&mut state, &completed(request_of(&effects), reply));
    let effects = reduce(&mut state, &key(CaretMotion::Delete, false));
    let TextOp::AwaitCaret(watch) = &request(&effects).op else {
        panic!("expected a caret wait");
    };
    assert_eq!(watch.previous_selection, None);
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
                },
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
        moved: true,
        caret: CaretReport {
            line,
            selection: None,
        },
        read_at_ms,
        unit: None,
        selection_changes: Vec::new(),
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
    // The review cursor, following the caret, stays on the newest caret.
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("c")]);
    // A key after both finds the newest caret too.
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

/// A command whose key the hook saw at `pressed_at_ms`.
fn command_at(command: ReviewCommand, pressed_at_ms: u64) -> Input {
    Input::Command {
        trace_id: TraceId::mint(),
        command,
        repeat: 0,
        pressed_at_ms,
    }
}

#[test]
fn a_command_reads_the_caret_from_before_its_key_though_a_later_caret_event_came_first() {
    // The review cursor does not follow the caret, so the first review
    // command starts it at the caret.
    let mut state = SrState::new();
    focus(&mut state, node(5, Role::EditableText, StateSet::new()));
    let _ = reduce(&mut state, &command(ReviewCommand::ToggleFollowCaret, 0));
    let _ = caret_event_at(&mut state, line("abc", 100, 0), 100);
    // Observed after the command's key was pressed, but reaching Core first.
    let _ = caret_event_at(&mut state, line("abc", 100, 1), 210);
    let mut review = state.clone();
    let effects = reduce(
        &mut review,
        &command_at(ReviewCommand::ReviewCurrentCharacter, 200),
    );
    assert_eq!(spoken(&effects), vec![character("a")]);
    // A gesture the control plane injected has no press time: the current
    // caret.
    let effects = reduce(
        &mut state.clone(),
        &command_at(ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(spoken(&effects), vec![character("b")]);

    let before = TextPoint::At(TextPosition {
        anchor: TextAnchor(100),
        offset: 0,
    });
    let effects = reduce(
        &mut state.clone(),
        &command_at(ReviewCommand::SayAllFromCaret, 200),
    );
    assert_eq!(
        request(&effects).op,
        read_ahead(before, None, TextUnit::Sentence)
    );
    let effects = reduce(
        &mut state.clone(),
        &command_at(ReviewCommand::ReportCaretLocation, 200),
    );
    assert_eq!(request(&effects).op, TextOp::Location(before));
    // With no report since the key, the outpost reads the caret itself.
    let effects = reduce(
        &mut state,
        &command_at(ReviewCommand::ReportCaretLocation, 300),
    );
    assert_eq!(request(&effects).op, TextOp::Location(TextPoint::Caret));
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
fn caret_keys_wait_longer_in_a_terminal_and_not_at_all_without_text() {
    let mut state = SrState::new();
    focus(&mut state, node(7, Role::Terminal, StateSet::new()));
    let TextOp::AwaitCaret(watch) =
        request(&reduce(&mut state, &key(CaretMotion::PreviousLine, false))).op
    else {
        panic!("expected a caret wait");
    };
    assert_eq!(watch.wait, CaretWait::Extended);
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
    // Following the caret: no read is needed for the current character.
    let effects = reduce(
        &mut state,
        &command(ReviewCommand::ReviewCurrentCharacter, 0),
    );
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
    let mut state = editing("ab\r\n", 1);
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
    let mut state = editing("ab\r", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewEndOfLine, 0));
    assert_eq!(spoken(&effects), vec![character("\r")]);
    let mut state = editing("ab", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::ReviewEndOfLine, 0));
    assert_eq!(spoken(&effects), vec![character("b")]);
}

#[test]
fn a_terminal_keeps_a_cell_column_and_reads_past_the_text_as_blank() {
    let mut state = SrState::new();
    focus(&mut state, node(13, Role::Terminal, StateSet::new()));
    // The caret on 中 at cells 2 and 3 of a padded row.
    caret_event(&mut state, 13, line("ab中文cd    \r\n", 200, 2));
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
    let mut state = editing("ab\n", 0);
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
fn select_then_copy_uses_the_start_marker() {
    let mut state = editing("hello world\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(spoken(&effects), vec![message(Message::NoStartMarker)]);
    let effects = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    assert_eq!(spoken(&effects), vec![message(Message::StartMarked)]);
    let _ = reduce(&mut state, &command(ReviewCommand::ReviewNextWord, 0));

    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 0));
    let start = TextPoint::At(TextPosition {
        anchor: TextAnchor(100),
        offset: 0,
    });
    let end = TextPoint::At(TextPosition {
        anchor: TextAnchor(100),
        offset: 7,
    });
    assert_eq!(request(&effects).op, TextOp::Select { start, end });
    let effects = reduce(&mut state, &command(ReviewCommand::SelectThenCopy, 1));
    let request = request(&effects);
    assert_eq!(request.op, TextOp::ReadRange { start, end });
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
}

#[test]
fn the_follow_caret_toggle_is_spoken_saved_and_obeyed() {
    let mut state = editing("abc\n", 0);
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
    let mut state = editing("last\n", 0);
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
/// during say all" setting applies to it.
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
                    _ => None,
                },
                _ => None,
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
        count: 16,
    })
}

/// A mark reached at `at_ms` milliseconds since the Unix epoch.
fn reached(mark: SpeechMark, at_ms: u64) -> Input {
    Input::MarkReached { mark, at_ms }
}

#[test]
fn say_all_reads_by_line_where_there_are_no_sentences_and_moves_the_caret() {
    let mut state = editing("first\n", 0);
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
                chunks: vec![line("first\n", 100, 0)],
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].1, "first");
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
    let effects = reduce(&mut state, &reached(pieces[0].0, 0));
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
                chunks: vec![line("first\n", 100, 0)],
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
        ..line("three", 102, 0)
    };
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::Chunks {
                moved: 0,
                chunks: vec![line("one\n", 100, 0), line("two\n", 101, 0), last],
            },
        ),
    );
    // Two pieces with speech, the third waiting; the text's end was read,
    // so nothing more is asked for.
    let pieces = say_all_pieces(&effects);
    let texts: Vec<&str> = pieces.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts, ["one", "two"]);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Text(_)))
    );
    // Reaching the first hands on the third, with its own mark.
    let effects = reduce(&mut state, &reached(pieces[0].0, 0));
    let third = say_all_pieces(&effects);
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].1, "three");
    let _ = reduce(&mut state, &reached(pieces[1].0, 0));
    // The last piece reached: say-all ends.
    let effects = reduce(&mut state, &reached(third[0].0, 0));
    assert!(effects.contains(&Effect::KeepDisplayOn(false)));
}

#[test]
fn say_all_reads_the_next_batch_when_little_is_left_to_speak() {
    let mut state = editing("x\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            TextReply::UnsupportedUnit(TextUnit::Sentence),
        ),
    );
    // Six lines of 59 characters to speak: twelve seconds at the assumed
    // pace of 30 a second, above the low-water mark.
    let text = format!(
        "{}
",
        "a".repeat(59)
    );
    let chunks: Vec<TextChunk> = (0..6).map(|index| line(&text, 100 + index, 0)).collect();
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
    let mut reach = |state: &mut SrState, index: usize, at_ms: u64| {
        let effects = reduce(state, &reached(marks[index], at_ms));
        marks.extend(say_all_pieces(&effects).into_iter().map(|(mark, _)| mark));
        reads(&effects)
    };
    assert_eq!(reach(&mut state, 0, 1_000_000), 0);
    // The first line took ten seconds: speech is slow, so the four lines
    // left last long.
    assert_eq!(reach(&mut state, 1, 1_010_000), 0);
    // Speech speeds up to 118 characters a second. The pace measured
    // rises a quarter of the way each time, so after one fast line the
    // three lines left still last over the mark, and after two the two
    // left (118 characters, about four seconds at the assumed pace) do not.
    assert_eq!(reach(&mut state, 2, 1_010_500), 0);
    assert_eq!(reach(&mut state, 3, 1_011_000), 1);
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
    let effects = reduce(&mut state, &reached(pieces[1].0, 0));
    assert!(effects.contains(&Effect::Text(TextRequest {
        query_id: request(&effects).query_id,
        node_id: request(&effects).node_id,
        op: TextOp::MoveCaret(TextPoint::At(TextPosition {
            anchor: TextAnchor(400),
            offset: 5
        })),
    })));
    let third = say_all_pieces(&effects);
    assert_eq!(third[0].1, "Three!");
    // A key stops it, dropping what was waiting; a late mark is ignored.
    let effects = reduce(&mut state, &Input::SpeechCancelled);
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    let late = reads_query(&reduce(&mut state, &reached(third[0].0, 0)));
    assert!(late.is_none());
}

fn reads_query(effects: &[Effect]) -> Option<QueryId> {
    effects.iter().find_map(|effect| match effect {
        Effect::Text(request) => Some(request.query_id),
        _ => None,
    })
}

#[test]
fn say_all_from_the_review_cursor_leaves_the_review_cursor_where_it_stopped() {
    let mut state = editing("one\n", 0);
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
    let _ = reduce(&mut state, &reached(pieces[1].0, 0));
    let _ = reduce(&mut state, &Input::SpeechCancelled);
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

#[test]
fn held_anchors_name_every_position_the_state_keeps() {
    let mut state = editing("abc\n", 1);
    let _ = reduce(&mut state, &command(ReviewCommand::SetStartMarker, 0));
    let held = state.held_anchors();
    let anchors = held.get(&OUTPOST).expect("the outpost's anchors");
    assert!(anchors.contains(&TextAnchor(100)));
    assert_eq!(anchors.len(), 1);
    assert!(state.held_nodes()[&OUTPOST].contains(&id(5)));
}

/// Focuses a document holding `value`, returning what the focus
/// announcement spoke.
fn focus_with_value(state: &mut SrState, states: StateSet, value: &str) -> Vec<UtteranceSegment> {
    let snapshot = NodeSnapshot {
        value: Some(value.to_owned()),
        ..node(5, Role::Document, states)
    };
    spoken(&reduce(
        state,
        &event(NormalizedEvent::FocusChanged {
            node: snapshot,
            foreground: false,
            ancestors: Vec::new(),
            ancestors_unknown: false,
            selected_child: None,
        }),
    ))
}

fn caret_moved(selection: Option<Selection>, text: &str) -> Input {
    event(NormalizedEvent::CaretMoved {
        node_id: id(5),
        caret: CaretReport {
            line: line(text, 100, 0),
            selection,
        },
    })
}

#[test]
fn a_focused_document_says_its_caret_line_and_not_its_value() {
    let mut state = SrState::new();
    let announced = focus_with_value(&mut state, StateSet::new(), "gamma\rdelta\r");
    assert_eq!(
        announced,
        vec![
            UtteranceSegment::label("Body"),
            UtteranceSegment::new(SegmentContent::Role(Role::Document)),
        ]
    );
    // The first caret report ends the announcement with the caret's line.
    let effects = reduce(&mut state, &caret_moved(None, "gamma\r"));
    assert_eq!(spoken(&effects), vec![UtteranceSegment::text("gamma")]);
    // A later one says nothing by itself.
    assert_eq!(
        spoken(&reduce(&mut state, &caret_moved(None, "delta\r"))),
        []
    );
}

#[test]
fn an_empty_focused_field_is_blank() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "");
    assert_eq!(
        spoken(&reduce(&mut state, &caret_moved(None, ""))),
        vec![message(Message::Blank)]
    );
}

#[test]
fn a_focused_field_with_text_selected_says_the_selection() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "alpha\rdelta epsilon\r");
    let selection = Selection {
        start: TextPosition::at(TextAnchor(101)),
        end: TextPosition::at(TextAnchor(102)),
    };
    let effects = reduce(&mut state, &caret_moved(Some(selection), "delta epsilon\r"));
    assert_eq!(spoken(&effects), []);
    let asked = request(&effects);
    assert_eq!(
        asked.op,
        TextOp::ReadRange {
            start: TextPoint::At(selection.start),
            end: TextPoint::At(selection.end),
        }
    );
    let effects = reduce(
        &mut state,
        &completed(
            asked.query_id,
            TextReply::Range {
                text: "delta epsilon".to_owned(),
                truncated: false,
            },
        ),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Preselected(SelectionText::Text("delta epsilon".to_owned()))
        ))]
    );
}

#[test]
fn a_focus_without_text_says_its_value_instead() {
    let mut state = SrState::new();
    let _ = focus_with_value(&mut state, StateSet::new(), "plain value");
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::NoText { node_id: id(5) }),
    );
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::value("plain value")]
    );
}

#[test]
fn a_protected_field_never_says_its_text() {
    let mut state = SrState::new();
    let protected = StateSet::new().with(State::Protected);
    let announced = focus_with_value(&mut state, protected, "secret");
    assert!(
        !announced
            .iter()
            .any(|segment| matches!(segment.content, SegmentContent::Value(_))),
        "{announced:?}"
    );
    assert_eq!(
        spoken(&reduce(&mut state, &caret_moved(None, "secret"))),
        []
    );
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::NoText { node_id: id(5) }),
    );
    assert_eq!(spoken(&effects), []);
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
            UtteranceSegment::text("wrold"),
            UtteranceSegment::text("there"),
        ]
    );
    // Leaving the error inside a line says nothing, and reading the same
    // line again starts out of the error, as the last stretch left it.
    assert_eq!(
        answered(&mut state, CaretMotion::NextLine, line, None),
        vec![
            UtteranceSegment::text("hello"),
            format(TextFormat::SpellingError),
            UtteranceSegment::text("wrold"),
            UtteranceSegment::text("there"),
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
    let _ = focus_with_value(&mut state, StateSet::new(), "Ths is\r");
    let line = with_errors(line("Ths is\r", 100, 0), &[(0, 3, true), (3, 7, false)]);
    let effects = reduce(
        &mut state,
        &event(NormalizedEvent::CaretMoved {
            node_id: id(5),
            caret: CaretReport {
                line,
                selection: None,
            },
        }),
    );
    assert_eq!(
        spoken(&effects),
        vec![
            format(TextFormat::SpellingError),
            UtteranceSegment::text("Ths"),
            UtteranceSegment::text("is"),
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
