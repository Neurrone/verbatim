//! Review commands over the navigator's text as event-queue entries: each
//! resolves the navigator at its turn and makes its reads as the queue's
//! requests on the navigator's window, holding the queue until its last
//! answer.
use crate::entries::tests::found_at;
use crate::readers::Found;

use verbatim_model::batch::{Batch, StepRefusal};
use verbatim_model::request::{Outcome as WireOutcome, Press, PressAction, Reply, Request as Wire};
use verbatim_model::{
    Effect as Said, KeySeq, ReviewCommand, Role, TextAnchor, TextChunk, TextMovement, TextPoint,
    TextPosition, TextUnit, TraceId, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready, values,
};
use crate::event_queue::{Foreground, Input, Timed};
use crate::keys::entry::PressEntry;
use crate::text_batches;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
    shell: false,
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
    assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
    state
}

/// `command` pressed as key `key`, `repeat` extra times.
fn press(key: u64, command: ReviewCommand, repeat: u8) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action: PressAction::Command { command, repeat },
        },
    ))))
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

/// The answer to request `number`, a line read that moved `moved` lines.
fn read(number: u64, moved: i32, chunk: TextChunk) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        text_batches::one(text_batches::chunks(moved, vec![chunk])),
    )
}

/// The request out last, of the kinds not yet batches.
fn asked(state: &Held) -> Wire {
    state
        .queue
        .table_for_tests()
        .last()
        .cloned()
        .expect("a request out")
}

/// The batch out last.
fn batch(state: &Held) -> Batch {
    state
        .queue
        .batches_for_tests()
        .last()
        .map(|(_, batch)| batch.clone())
        .expect("a batch out")
}

/// The line of node 2's text read at `at`, moved by `movement`.
fn line_at(at: TextPoint, movement: Option<TextMovement>) -> Batch {
    text_batches::line(node(2), at, movement)
}

/// The edit field "Body" committed, the review cursor placed on the line
/// "one two" at its start by a review of the current line, answered as
/// request 3.
fn reviewing() -> Held {
    let mut state = committed("Body", Role::EditableText);
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReviewCurrentLine, 0)
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(batch(&state), (line_at(TextPoint::Caret, None)));
    assert_eq!(
        feed_at(&mut state, 1_100, read(3, 0, line("one two\r\n", 100, 0))),
        ["Text(\"one two\")"]
    );
    state
}

#[test]
fn the_current_line_is_read_at_the_caret_and_spoken() {
    let _ = reviewing();
}

/// A move reads the line it moves to from the line the review cursor is
/// on, and speaks it.
#[test]
fn the_next_line_is_read_from_the_review_cursors_line_and_spoken() {
    let mut state = reviewing();
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            press(2, ReviewCommand::ReviewNextLine, 0)
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        batch(&state),
        (line_at(
            TextPoint::At(TextPosition::at(TextAnchor(100))),
            Some(TextMovement {
                unit: TextUnit::Line,
                count: 1
            })
        ))
    );
    assert_eq!(
        feed_at(&mut state, 2_100, read(4, 1, line("three\r\n", 200, 0))),
        ["Text(\"three\")"]
    );
}

/// At the last line, the move says the edge and the line the cursor stays
/// on.
#[test]
fn the_next_line_at_the_bottom_says_the_edge_and_the_line() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(
        feed_at(&mut state, 2_100, read(4, 0, line("one two\r\n", 100, 0))),
        ["Message(Bottom) Text(\"one two\")"]
    );
}

/// A command that answers from the review cursor's line reads it again
/// first, then speaks from it.
#[test]
fn the_current_character_reads_the_review_cursors_line_again_and_says_it() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(
        batch(&state),
        (line_at(TextPoint::At(TextPosition::at(TextAnchor(100))), None))
    );
    assert_eq!(
        feed_at(&mut state, 2_100, read(4, 0, line("one two\r\n", 100, 0))),
        ["Character(\"o\")"]
    );
}

/// Word press `number` (also its request's number) of `command` with the
/// review cursor at anchor `from`'s byte `offset`: it asks the word row's
/// one batch, and with `steps` answered says `said`.
fn word_press(
    state: &mut Held,
    number: u64,
    (command, from, offset): (ReviewCommand, u64, u32),
    steps: Vec<verbatim_model::batch::StepOutcome>,
    said: &str,
) {
    let _ = feed_at(state, 1_000 * number, press(number, command, 0));
    let here = text_batches::at(from, offset);
    let expected = match command {
        ReviewCommand::ReviewPreviousWord => text_batches::previous_word(node(2), here),
        ReviewCommand::ReviewNextWord => text_batches::next_word(node(2), here),
        _ => text_batches::current_word(node(2), here),
    };
    assert_eq!(batch(state), expected, "{said}");
    let reply = Reply::Steps(steps);
    assert_eq!(
        feed_at(state, 1_000 * number + 100, answer(OUTPOST, number, reply)),
        [said]
    );
}

/// Review current word reads the provider's word at the review cursor,
/// one batch, and spells it on a second press.
#[test]
fn the_current_word_is_the_texts_own_word_at_the_review_cursor() {
    let mut state = reviewing();
    let word = || vec![text_batches::word(0, "one ", 100)];
    word_press(
        &mut state,
        4,
        (ReviewCommand::ReviewCurrentWord, 100, 0),
        word(),
        "Text(\"one\")",
    );
    let _ = feed_at(
        &mut state,
        5_000,
        press(5, ReviewCommand::ReviewCurrentWord, 1),
    );
    assert_eq!(
        feed_at(&mut state, 5_100, answer(OUTPOST, 5, Reply::Steps(word()))),
        ["Text(\"o\") Text(\"n\") Text(\"e\")"]
    );
}

/// A word press: the command and where the review cursor is (anchor and
/// offset), the batch's answer, and what it says.
type WordPress<'a> = (
    (ReviewCommand, u64, u32),
    Vec<verbatim_model::batch::StepOutcome>,
    &'a str,
);

/// Word presses one after another, from request `first` on: each the
/// command, where the review cursor is (anchor and offset), the batch's
/// answer and what it says ([`word_press`]).
fn word_presses(state: &mut Held, first: u64, presses: Vec<WordPress<'_>>) {
    for (number, (at, steps, said)) in (first..).zip(presses) {
        word_press(state, number, at, steps, said);
    }
}

/// The review word rows by the provider's word, against NVDA's capture
/// `tests/fixtures/p1-word-break.jsonl` (Notepad holding "alpha beta", a
/// line break, "gamma"; the request list's design, section 6, "The word
/// rows' speech"): from "gamma" previous word says the line break by its
/// name (step 4), then "beta" and "alpha" (steps 5 and 6); next word from
/// "alpha" says "beta", the line break and "gamma" (steps 7 to 9), and at
/// the end "Bottom" and "gamma" (step 11). Each press is one batch from
/// where the last one left the review cursor, the start of the word it
/// said on the line read there; the provider's words are Notepad's, as the
/// capture shows them. A move that went no word, or a next word that does
/// not start after the review cursor, says the edge and the word there,
/// and the cursor stays.
#[test]
fn review_words_move_by_the_providers_word_as_nvdas_capture() {
    use text_batches::{line_at as on_line, range_text, word};
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    let _ = feed_at(&mut state, 1_100, read(3, 0, line("gamma", 200, 0)));
    let (previous, next) = (
        ReviewCommand::ReviewPreviousWord,
        ReviewCommand::ReviewNextWord,
    );
    let first = "alpha beta\r";
    let alpha = || word(0, "alpha ", 100);
    let gamma = || word(0, "gamma", 200);
    word_presses(
        &mut state,
        4,
        vec![
            (
                (previous, 200, 0),
                vec![gamma(), word(-1, "\r", 110), on_line(first, 100, 10)],
                "Character(\"\\r\")",
            ),
            (
                (previous, 100, 10),
                vec![
                    word(0, "\r", 110),
                    word(-1, "beta", 106),
                    on_line(first, 100, 6),
                ],
                "Text(\"beta\")",
            ),
            (
                (previous, 100, 6),
                vec![
                    word(0, "beta", 106),
                    word(-1, "alpha ", 100),
                    on_line(first, 100, 0),
                ],
                "Text(\"alpha\")",
            ),
            (
                (previous, 100, 0),
                vec![alpha(), alpha(), on_line(first, 100, 0)],
                "Message(Top) Text(\"alpha\")",
            ),
            (
                (next, 100, 0),
                vec![
                    word(1, "beta", 106),
                    range_text("alpha "),
                    on_line(first, 100, 6),
                ],
                "Text(\"beta\")",
            ),
            (
                (next, 100, 6),
                vec![
                    word(1, "\r", 110),
                    range_text("beta"),
                    on_line(first, 100, 10),
                ],
                "Character(\"\\r\")",
            ),
            (
                (next, 100, 10),
                vec![
                    word(1, "gamma", 200),
                    range_text("\r"),
                    on_line("gamma", 200, 0),
                ],
                "Text(\"gamma\")",
            ),
            // The end: the provider does not move, or moves onto the text's
            // end, landing on no word after the cursor (NVDA's issue 12808
            // check).
            (
                (next, 200, 0),
                vec![gamma(), range_text(""), on_line("gamma", 200, 0)],
                "Message(Bottom) Text(\"gamma\")",
            ),
            (
                (next, 200, 0),
                vec![
                    word(1, "gamma", 200),
                    range_text(""),
                    on_line("gamma", 200, 0),
                ],
                "Message(Bottom) Text(\"gamma\")",
            ),
        ],
    );
}

/// The word rows in a Windows Forms text box, whose words are the edit
/// control's (the outpost's `text/edit.rs`, a line break character a word
/// of its own), against NVDA's capture
/// `tests/fixtures/s10-review-words-text-box.jsonl`: from "Apple", the
/// first word of the second row, previous word stops on the line feed and
/// the carriage return before "Price" (steps 20 to 22), and next word at
/// the text's end says "Bottom" and "blank" (step 26).
#[test]
fn review_words_in_a_text_box_stop_on_each_line_break_character() {
    use text_batches::{line_at as on_line, range_text, word};
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    let _ = feed_at(
        &mut state,
        1_100,
        read(3, 0, line("Apple   red 1.20\r\n", 200, 0)),
    );
    let previous = ReviewCommand::ReviewPreviousWord;
    let first = "Fruit   Color   Price\r\n";
    word_presses(
        &mut state,
        4,
        vec![
            (
                (previous, 200, 0),
                vec![
                    word(0, "Apple   ", 200),
                    word(-1, "\n", 122),
                    on_line(first, 100, 22),
                ],
                "Character(\"\\n\")",
            ),
            (
                (previous, 100, 22),
                vec![
                    word(0, "\n", 122),
                    word(-1, "\r", 121),
                    on_line(first, 100, 21),
                ],
                "Character(\"\\r\")",
            ),
            (
                (previous, 100, 21),
                vec![
                    word(0, "\r", 121),
                    word(-1, "Price\r\n", 116),
                    on_line(first, 100, 16),
                ],
                "Text(\"Price\")",
            ),
        ],
    );
    let _ = feed_at(&mut state, 7_000, press(7, ReviewCommand::ReviewBottom, 0));
    let _ = feed_at(&mut state, 7_100, read(7, 0, line("", 300, 0)));
    word_press(
        &mut state,
        8,
        (ReviewCommand::ReviewNextWord, 300, 0),
        vec![word(0, "", 300), range_text(""), on_line("", 300, 0)],
        "Message(Bottom) Message(Blank)",
    );
}

/// A press behind a review press waits for its last answer.
#[test]
fn a_press_behind_a_review_press_waits_for_its_answer() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_050,
            press(2, ReviewCommand::ReviewCurrentLine, 0)
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        feed_at(&mut state, 1_100, read(3, 0, line("one two\r\n", 100, 0))),
        ["Text(\"one two\")", "send Outpost(Pid(40)) other"]
    );
}

/// A read its application does not answer says nothing.
#[test]
fn a_review_read_not_answered_says_nothing() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_100,
            answer_outcome(OUTPOST, 3, WireOutcome::NotResponding)
        ),
        Vec::<String>::new()
    );
}

/// A position gone from the text: the review starts again at the caret.
#[test]
fn a_lost_review_position_starts_again_at_the_caret() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_100,
            answer(OUTPOST, 4, text_batches::one(text_batches::lost(100)))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(batch(&state), (line_at(TextPoint::Caret, None)));
}

/// An object with no text interface reviews its live read as flat text.
#[test]
fn an_object_without_text_reviews_its_live_read_as_flat_text() {
    let mut state = committed("OK", Role::Button);
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReviewCurrentLine, 0)
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        asked(&state),
        Wire::Properties {
            node: node(2),
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        }
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_100,
            answer(OUTPOST, 3, Reply::Properties(values("Okay", Role::Button)))
        ),
        ["Text(\"Okay\")"]
    );
}

/// Flat text holds the description whatever the theme reports, as NVDA's
/// basic text does (`NVDAObjects/__init__.py` 1450 to 1463; source-derived,
/// no capture): with descriptions off, the live read still asks for it,
/// review current line says it, and the navigator's object keeps none.
#[test]
fn flat_text_reads_the_description_with_descriptions_off() {
    use verbatim_model::property::{
        NodeDetail, NodeDetailSet, PropertyAnswer, PropertyId, PropertyValue,
    };
    let mut state = committed("Inbox", Role::ListItem);
    state.state.reported.node = NodeDetailSet::ALL.without(NodeDetail::Description);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    let Wire::Properties { set, .. } = asked(&state) else {
        panic!("a live read of the object");
    };
    assert!(set.contains(&PropertyId::Description), "{set:?}");
    let mut read = values("Inbox", Role::ListItem);
    read.0.push(PropertyAnswer {
        id: PropertyId::Description,
        value: Some(PropertyValue::Text("3 unread".to_owned())),
    });
    assert_eq!(
        feed_at(
            &mut state,
            1_100,
            answer(OUTPOST, 3, Reply::Properties(read))
        ),
        ["Text(\"Inbox 3 unread\")"]
    );
    let object = &state.navigator.as_ref().unwrap().object;
    assert_eq!(object.details.description, None);
}

/// A field that answers it has no text is reviewed flat from then on.
#[test]
fn a_field_with_no_text_is_reviewed_as_flat_text() {
    let mut state = committed("Body", Role::EditableText);
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_100,
            answer(
                OUTPOST,
                3,
                text_batches::one(text_batches::refused(StepRefusal::NoText))
            )
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        asked(&state),
        Wire::Properties {
            node: node(2),
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        }
    );
}

/// A dead object reviews as an object with empty text.
#[test]
fn a_dead_object_reviews_as_empty_flat_text() {
    for (command, said) in [
        (ReviewCommand::ReviewCurrentLine, "Message(Blank)"),
        (ReviewCommand::ReviewCurrentWord, "Message(Blank)"),
        (ReviewCommand::ReviewCurrentCharacter, "Message(Blank)"),
        (
            ReviewCommand::ReviewNextLine,
            "Message(Bottom) Message(Blank)",
        ),
    ] {
        let mut state = committed("OK", Role::Button);
        let _ = feed_at(&mut state, 1_000, press(1, command, 0));
        assert_eq!(
            feed_at(
                &mut state,
                1_100,
                answer_outcome(OUTPOST, 3, WireOutcome::NotThere)
            ),
            [said],
            "{command:?}"
        );
    }
}

/// A dead object's flat position is the start, its column 0 too: with the
/// review cursor at the start of an empty first line remembering column 2,
/// a press that finds the object gone leaves the position at offset 0 and
/// column 0.
#[test]
fn a_dead_object_resets_the_flat_column() {
    use crate::state::{FlatPosition, ReviewText};
    let mut state = committed(
        "
abcd",
        Role::Button,
    );
    state.navigator.as_mut().unwrap().text = ReviewText::Flat(FlatPosition {
        offset: 0,
        column: 2,
    });
    let _ = feed_at(
        &mut state,
        1_000,
        press(1, ReviewCommand::ReviewCurrentLine, 0),
    );
    let _ = feed_at(
        &mut state,
        1_100,
        answer_outcome(OUTPOST, 3, WireOutcome::NotThere),
    );
    assert_eq!(
        state.navigator.as_ref().unwrap().text,
        ReviewText::Flat(FlatPosition::default())
    );
}

/// The start marker is set at the review cursor; select then copy's second
/// press copies from it through the character at the review cursor.
#[test]
fn a_start_marker_set_then_copied_copies_the_range_read() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::SetStartMarker, 0),
    );
    assert_eq!(
        feed_at(&mut state, 2_100, read(4, 0, line("one two\r\n", 100, 0))),
        ["Message(StartMarked)"]
    );
    let _ = feed_at(
        &mut state,
        3_000,
        press(3, ReviewCommand::SelectThenCopy, 1),
    );
    let _ = feed_at(&mut state, 3_100, read(5, 0, line("one two\r\n", 100, 0)));
    assert!(matches!(
        batch(&state).steps[0].op,
        verbatim_model::batch::Op::ReadRange { .. }
    ));
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 3_200,
            input: answer(OUTPOST, 6, text_batches::one(text_batches::range_text("o"))),
        },
    )
    .unwrap();
    assert!(
        effects.contains(&Said::CopyToClipboard("o".to_owned())),
        "{effects:?}"
    );
    assert_eq!(state.start_marker, None);
}

/// Select then copy's first press selects from the start marker on the
/// focused text: the selection it made is said, and the answer mints the
/// next selection baseline, Verbatim's own selection not being spoken as a
/// change (Table 17, column F).
#[test]
fn a_selection_made_on_the_focused_text_is_said_and_mints_the_baseline() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::SetStartMarker, 0),
    );
    let _ = feed_at(
        &mut state,
        2_100,
        read(
            4,
            0,
            line(
                "one two
", 100, 0,
            ),
        ),
    );
    let _ = feed_at(
        &mut state,
        3_000,
        press(3, ReviewCommand::SelectThenCopy, 0),
    );
    let _ = feed_at(
        &mut state,
        3_100,
        read(
            5,
            0,
            line(
                "one two
", 100, 0,
            ),
        ),
    );
    assert!(matches!(
        batch(&state).steps[1].op,
        verbatim_model::batch::Op::Select { .. }
    ));
    let said = feed_at(
        &mut state,
        3_200,
        answer(
            OUTPOST,
            6,
            text_batches::selected(
                vec![verbatim_model::SelectionChange {
                    selected: true,
                    text: "o".to_owned(),
                    characters: 1,
                }],
                80,
            ),
        ),
    );
    assert_eq!(said, ["Phrase(Selected(Character(\"o\")))"]);
    assert_eq!(
        crate::text_facts::held(&state.state),
        Some((node(2), TextAnchor(80)))
    );
}

/// The review cursor's location is asked of the application and said.
#[test]
fn the_review_cursors_location_is_asked_and_said() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::ReportReviewLocation, 0),
    );
    assert_eq!(
        batch(&state),
        text_batches::locate(node(2), TextPoint::At(TextPosition::at(TextAnchor(100))))
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_100,
            answer(
                OUTPOST,
                4,
                text_batches::located(verbatim_model::batch::Located {
                    screen: Some((10, 20)),
                    offset: None,
                })
            )
        ),
        ["Phrase(Positioned { x: 10, y: 20 })"]
    );
}

#[test]
fn with_no_navigator_a_review_command_says_so() {
    let mut state = ready();
    assert_eq!(
        feed_at(
            &mut state,
            1_000,
            press(1, ReviewCommand::ReviewCurrentLine, 0)
        ),
        ["Message(NoNavigatorObject)"]
    );
}
