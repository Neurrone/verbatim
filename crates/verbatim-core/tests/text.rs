//! Scripted-input tests for milestone M4's text handling in the reducer:
//! caret keys and the selection, typed character and word echo, the
//! review cursor over text, and say-all, each driven through the text
//! protocol with scripted outpost replies.

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReply, CaretReport, CaretWait, Effect, Input, Message,
    NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, PreviousSelection,
    QueryId, ReaderSettings, ReviewCommand, Role, SegmentContent, Selection, SelectionChange,
    SelectionText, SpeechMark, State, StateSet, TextAnchor, TextChunk, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReply, TextRequest, TextUnit, TraceId, TypingEcho,
    UtteranceSegment,
};

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

fn key(motion: CaretMotion, select: bool) -> Input {
    Input::CaretKey {
        trace_id: TraceId::mint(),
        key: CaretKey { motion, select },
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
    // At the end of the line, the line break is blank.
    let effects = reduce(&mut state, &key(CaretMotion::EndOfLine, false));
    let effects = reduce(
        &mut state,
        &completed(
            request_of(&effects),
            caret_reply(true, line("Hello\r\n", 100, 5), None),
        ),
    );
    assert_eq!(spoken(&effects), vec![message(Message::Blank)]);
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
    assert_eq!(
        spoken(&effects),
        vec![UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::SpeakTypedCharacters(TypingEcho::Off)
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

#[test]
fn say_all_reads_by_line_where_there_are_no_sentences_and_moves_the_caret() {
    let mut state = editing("first\n", 0);
    let effects = reduce(&mut state, &command(ReviewCommand::SayAllFromCaret, 0));
    assert!(effects.contains(&Effect::KeepDisplayOn(true)));
    let first = request(&effects);
    assert_eq!(
        first.op,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Sentence,
        })
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
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        })
    );
    let effects = reduce(
        &mut state,
        &completed(
            by_line.query_id,
            TextReply::Read {
                moved: 0,
                chunk: line("first\n", 100, 0),
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].1, "first");
    // Little is queued, so the next line is read at once.
    let next = request(&effects);
    assert_eq!(
        next.op,
        TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition::at(TextAnchor(100))),
            movement: Some(TextMovement {
                unit: TextUnit::Line,
                count: 1
            }),
            unit: TextUnit::Line,
        })
    );
    // Playback reaches the first line: the caret moves there.
    let effects = reduce(&mut state, &Input::MarkReached { mark: pieces[0].0 });
    assert_eq!(
        request(&effects).op,
        TextOp::MoveCaret(TextPoint::At(TextPosition::at(TextAnchor(100))))
    );
    // The document ends: say-all is over, and the display may sleep.
    let effects = reduce(
        &mut state,
        &completed(
            next.query_id,
            TextReply::Read {
                moved: 0,
                chunk: line("first\n", 100, 0),
            },
        ),
    );
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
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
            TextReply::Read {
                moved: 0,
                chunk: paragraph,
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    let texts: Vec<&str> = pieces.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts, ["One. ", "Two? ", "Three!"]);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Text(_))),
        "three sentences are queued, so nothing more is read yet"
    );
    // Reaching the second sentence leaves one queued: the next paragraph is
    // read, a paragraph on.
    let effects = reduce(&mut state, &Input::MarkReached { mark: pieces[1].0 });
    let reads: Vec<TextOp> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Text(request) => Some(request.op.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reads,
        vec![
            TextOp::MoveCaret(TextPoint::At(TextPosition {
                anchor: TextAnchor(400),
                offset: 5
            })),
            TextOp::Read(TextRead {
                at: TextPoint::At(TextPosition::at(TextAnchor(400))),
                movement: Some(TextMovement {
                    unit: TextUnit::Paragraph,
                    count: 1
                }),
                unit: TextUnit::Sentence,
            }),
        ]
    );
    // A key stops it; a late answer is dropped.
    let effects = reduce(&mut state, &Input::SpeechCancelled);
    assert_eq!(effects, vec![Effect::KeepDisplayOn(false)]);
    let late = reads_query(&reduce(
        &mut state,
        &Input::MarkReached { mark: pieces[2].0 },
    ));
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
        TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition::at(TextAnchor(100))),
            movement: None,
            unit: TextUnit::Sentence,
        })
    );
    let effects = reduce(
        &mut state,
        &completed(
            first.query_id,
            TextReply::Read {
                moved: 0,
                chunk: TextChunk {
                    unit: TextUnit::Paragraph,
                    ..line("A b. C d.\n", 500, 0)
                },
            },
        ),
    );
    let pieces = say_all_pieces(&effects);
    let _ = reduce(&mut state, &Input::MarkReached { mark: pieces[1].0 });
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
            Phrase::Selected(SelectionText::Text("delta epsilon".to_owned()))
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
