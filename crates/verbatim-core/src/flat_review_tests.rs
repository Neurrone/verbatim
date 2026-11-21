//! Flat review: an object with no text interface reviewed through the text
//! path over its flat text, NVDA's basic text, read live at each press,
//! every request of the press answered by Core from that text
//! (`docs/parity/review.md`, "Review cursor line/word/character over
//! object text"). Each press goes through the event queue
//! ([`crate::desk_tests`]); its one read, the object's properties, is
//! answered as the object still is.

use verbatim_model::{
    Message, NodeDetails, NodeSnapshot, ReviewCommand, Role, SegmentContent, SpeechPriority,
    StateSet, UtteranceSegment,
};

use crate::desk_tests::{Desk, Heard, ONE};

/// Node 140 of [`ONE`], of `role`, named `name` with `value`, focused, so
/// the navigator is on it; it has no text interface, so the review cursor
/// walks its flat text.
fn reviewing_object(role: Role, name: Option<&str>, value: Option<&str>) -> (Desk, NodeSnapshot) {
    let object = NodeSnapshot {
        id: ONE.node(140),
        backend: verbatim_model::Backend::Uia,
        role,
        name: name.map(str::to_owned),
        value: value.map(str::to_owned),
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &object, &[]);
    (desk, object)
}

/// Static text with no name and `value`, whose flat text is the value.
fn reviewing(value: &str) -> (Desk, NodeSnapshot) {
    reviewing_object(Role::StaticText, None, Some(value))
}

/// Review command `command` pressed on `desk`, its navigator `object`: the
/// segments of the one queued utterance that is all it said.
fn review(
    desk: &mut Desk,
    object: &NodeSnapshot,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<UtteranceSegment> {
    match desk.review(command, repeat, object).as_slice() {
        [Heard::Say(SpeechPriority::Queued, segments)] => segments.clone(),
        other => panic!("expected one queued utterance and nothing else, got {other:?}"),
    }
}

fn message(message: Message) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Message(message))
}

/// A character as the text path speaks it.
fn character(character: &str) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Character(character.to_owned()))
}

/// NVDA's capture `tests/fixtures/b9-flat-review.jsonl`: the navigator on
/// File Explorer's window, which has no text interface, review start of
/// line says "v" (step 3), review end of line "r" (step 4), and review
/// current line the whole line (step 5), which leaves the review cursor
/// where it was, on the "r" (`globalCommands.py` 1975 to 1990).
#[test]
fn flat_review_says_the_character_at_the_lines_start_and_end_as_nvdas_capture() {
    let (mut desk, window) =
        reviewing_object(Role::Window, Some("verbatim-e2e-b9 - File Explorer"), None);
    assert_eq!(
        review(&mut desk, &window, ReviewCommand::ReviewStartOfLine, 0),
        [character("v")]
    );
    assert_eq!(
        review(&mut desk, &window, ReviewCommand::ReviewEndOfLine, 0),
        [character("r")]
    );
    assert_eq!(
        review(&mut desk, &window, ReviewCommand::ReviewCurrentLine, 0),
        [UtteranceSegment::text("verbatim-e2e-b9 - File Explorer")]
    );
    assert_eq!(
        review(&mut desk, &window, ReviewCommand::ReviewCurrentCharacter, 0),
        [character("r")]
    );
}

/// Flat text is NVDA's basic text: the name, the value and the description,
/// each one that is not blank, joined by spaces (`NVDAObjects/__init__.py`
/// 1450 to 1463). Source-derived: no capture.
#[test]
fn flat_text_is_the_name_value_and_description_joined() {
    let mut object = reviewing_object(Role::ListItem, Some("Inbox"), Some(" ")).1;
    object.details.description = Some("3 unread".to_owned());
    assert_eq!(crate::review_text::flat_text(&object), "Inbox 3 unread");
    object.value = Some("12".to_owned());
    assert_eq!(crate::review_text::flat_text(&object), "Inbox 12 3 unread");
    object.name = None;
    object.details.description = None;
    assert_eq!(crate::review_text::flat_text(&object), "12");
}

/// Moving to another line of flat text keeps the review cursor's column,
/// as in any text (`docs/parity/review.md`, "Review cursor columns";
/// Dickson, 2026-10-06).
#[test]
fn flat_review_keeps_the_column_across_lines() {
    let (mut desk, body) = reviewing("abcdef\nuvwxyz");
    for _ in 0..4 {
        let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    }
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0),
        [UtteranceSegment::text("uvwxyz")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        [character("y")]
    );
}

/// Flat review's current line leaves the position where it was
/// (`docs/parity/review.md`, "Dead objects"; `globalCommands.py` 1975 to
/// 1990).
#[test]
fn flat_reviews_current_line_leaves_the_position_where_it_was() {
    let (mut desk, body) = reviewing("hello\nworld");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [character("e")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        [UtteranceSegment::text("hello")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [character("l")]
    );
}

/// The edges are named and the unit read again; moving by character meets
/// the line break that ends a line before the edge, as in any text.
#[test]
fn review_edges_are_named_and_the_unit_is_read_again() {
    let (mut desk, body) = reviewing("ab\ncd");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousLine, 0),
        [message(Message::Top), UtteranceSegment::text("ab")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousCharacter, 0),
        [message(Message::Left), character("a")]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [character("\n")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [message(Message::Right), character("\n")]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0),
        [message(Message::Bottom), UtteranceSegment::text("cd")]
    );
}

/// Flat review's bottom moves to the last line's start and reads the line.
#[test]
fn flat_review_bottom_reads_the_last_line_from_its_start() {
    let (mut desk, body) = reviewing("ab\ncd\nef");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewBottom, 0),
        [UtteranceSegment::text("ef")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        [character("e")]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewTop, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        [UtteranceSegment::text("ab")]
    );
}

/// Flat review's start of line says the character there, as NVDA's script
/// does (`globalCommands.py` 2238 to 2250).
#[test]
fn flat_review_start_of_line_says_the_character_there() {
    let (mut desk, body) = reviewing("ab\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewStartOfLine, 0),
        [character("c")]
    );
}

/// Flat review's previous word crosses a line start, and at the first word
/// says "Top" and the word again.
#[test]
fn flat_review_previous_word_crosses_a_line_start_and_stops_at_the_top() {
    let (mut desk, body) = reviewing("ab cd\nef");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        [UtteranceSegment::text("cd")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        [UtteranceSegment::text("ab")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        [message(Message::Top), UtteranceSegment::text("ab")]
    );
}

/// Flat review walks grapheme clusters: in "हिन्दी" the consonant ha with
/// its vowel sign is one character, and the conjunct after it, na, virama,
/// da, and the long vowel sign, is the next, starting at byte 6. A start
/// marked there, copied with the review cursor at the line's end, is that
/// character exactly.
#[test]
fn flat_review_moves_by_grapheme_cluster() {
    let (mut desk, body) = reviewing("हिन्दी");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [character("न्दी")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SetStartMarker, 0),
        [message(Message::StartMarked)]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewEndOfLine, 0),
        [character("न्दी")]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        [Heard::Copied("न्दी".to_owned())]
    );
}

/// Flat text cannot be selected, so select then copy's first press says
/// so as NVDA does, and the second copies; with the review cursor moved
/// back before the marker, from the cursor through the character at the
/// marker (`docs/parity/review.md`, "Select then copy from the review
/// cursor").
#[test]
fn flat_review_copies_back_from_the_marker_through_the_marker_s_character() {
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewPreviousCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::CannotSelect)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        [Heard::Copied("bc".to_owned())]
    );
}

/// Flat text's first press is NVDA's attempt to select, so a further first
/// press asks for the second, which copies and clears the start marker
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
#[test]
fn flat_review_asks_for_the_second_press_after_a_first() {
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::CannotSelect)]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::PressTwiceToCopy)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        [Heard::Copied("ab".to_owned())]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::NoStartMarker)]
    );
}

/// The second press copies the range the first press built, not the one
/// the review cursor has moved to since (`docs/parity/review.md`, "Select
/// then copy from the review cursor").
#[test]
fn flat_review_copies_the_range_the_first_press_built() {
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::CannotSelect)]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        [message(Message::PressTwiceToCopy)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        [Heard::Copied("ab".to_owned())]
    );
}

/// A copy of empty flat text copies nothing, says nothing, and still
/// clears the start marker.
#[test]
fn flat_review_copy_of_nothing_clears_the_start_marker() {
    let (mut desk, empty) = reviewing_object(Role::StaticText, None, None);
    let _ = review(&mut desk, &empty, ReviewCommand::SetStartMarker, 0);
    assert_eq!(desk.review(ReviewCommand::SelectThenCopy, 1, &empty), []);
    assert_eq!(
        review(&mut desk, &empty, ReviewCommand::SelectThenCopy, 0),
        [message(Message::NoStartMarker)]
    );
}

/// Moving to the start marker lands on the marker's character.
#[test]
fn flat_review_moves_to_the_start_marker() {
    let (mut desk, body) = reviewing("ab\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewTop, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::MoveToStartMarker, 0),
        [character("d")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        [UtteranceSegment::text("cd")]
    );
}

/// A start marker in flat text, set and then selected through, is a place
/// in Core's own text, not in the provider's, so Core asks the outpost to
/// keep no text anchor for it.
#[test]
fn a_flat_start_marker_holds_no_anchor_in_the_outpost() {
    let (mut desk, body) = reviewing("ab\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(
        desk.held.state.held_anchors(),
        verbatim_model::HeldAnchors::new()
    );
}

/// Flat text has no pages, selection or screen place: those commands say
/// "Not supported in this document", and say-all from the review cursor
/// speaks the rest of the text at once.
#[test]
fn flat_review_has_no_pages_selection_or_location_and_says_all_at_once() {
    let (mut desk, body) = reviewing("ab\ncd");
    for command in [
        ReviewCommand::ReviewNextPage,
        ReviewCommand::ReviewSelectionStart,
        ReviewCommand::ReviewSelectionEnd,
        ReviewCommand::ReportReviewLocation,
    ] {
        assert_eq!(
            review(&mut desk, &body, command, 0),
            [message(Message::NotSupported)],
            "{command:?}"
        );
    }
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SayAllFromReview, 0),
        [UtteranceSegment::text("b\ncd")]
    );
}

/// An emoji with a skin tone is one character in flat review, so the next
/// character after it is the space.
#[test]
fn flat_review_keeps_an_emoji_sequence_whole() {
    let (mut desk, body) = reviewing("👍🏽 ok");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        [character(" ")]
    );
}

/// Flat review finds the words of Thai, written without spaces, by
/// dictionary: "สวัสดีชาวโลก" is "สวัสดี", "ชาว", and "โลก".
#[test]
fn flat_review_finds_words_written_without_spaces() {
    let (mut desk, body) = reviewing("สวัสดีชาวโลก");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextWord, 0),
        [UtteranceSegment::text("ชาว")]
    );
}

#[test]
fn an_empty_unit_is_blank() {
    let (mut desk, body) = reviewing("ab\n\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        [message(Message::Blank)]
    );
}

#[test]
fn the_current_line_pressed_twice_is_spelled_and_a_character_thrice_gives_its_code() {
    let (mut desk, body) = reviewing("a b");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 1),
        [
            UtteranceSegment::text("a"),
            message(Message::Space),
            UtteranceSegment::text("b"),
        ]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 2),
        [
            UtteranceSegment::text("97,"),
            UtteranceSegment::text("0"),
            UtteranceSegment::text("x"),
            UtteranceSegment::text("6"),
            UtteranceSegment::text("1"),
        ]
    );
}

#[test]
fn review_cursor_walks_lines_words_and_characters() {
    let (mut desk, body) = reviewing("first line\nsecond line");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        [UtteranceSegment::text("first line")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0),
        [UtteranceSegment::text("second line")]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewTop, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentWord, 0),
        [UtteranceSegment::text("first")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextWord, 0),
        [UtteranceSegment::text("line")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        [character("l")]
    );
}
