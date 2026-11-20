//! Review commands over the navigator's text as event-queue entries: each
//! resolves the navigator at its turn and makes its reads as the queue's
//! requests on the navigator's window, holding the queue until its last
//! answer.
use verbatim_model::request::CandidateAnswer;

use verbatim_model::request::{Outcome as WireOutcome, Press, PressAction, Reply, Request as Wire};
use verbatim_model::{
    Effect as Said, KeySeq, ReviewCommand, Role, TextAnchor, TextChunk, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReply, TextUnit, TraceId, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready, values,
};
use crate::event_queue::{Foreground, Input, Timed};
use crate::keys::entry::PressEntry;

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
    let _ = feed_at(
        &mut state,
        600,
        answer(OUTPOST, 1, Reply::Candidate(CandidateAnswer::focused(read))),
    );
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
        Reply::Text(TextReply::Read { moved, chunk }),
    )
}

/// The request out last.
fn asked(state: &Held) -> Wire {
    state
        .queue
        .table_for_tests()
        .last()
        .cloned()
        .expect("a request out")
}

/// The read of node 2's text `op`.
fn text(op: TextOp) -> Wire {
    Wire::Text { node: node(2), op }
}

/// The line read at `at`, moved by `movement`.
fn line_at(at: TextPoint, movement: Option<TextMovement>) -> TextOp {
    TextOp::Read(TextRead {
        at,
        movement,
        unit: TextUnit::Line,
    })
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
    assert_eq!(asked(&state), text(line_at(TextPoint::Caret, None)));
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
        asked(&state),
        text(line_at(
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
fn the_current_word_reads_the_review_cursors_line_again_and_says_the_word() {
    let mut state = reviewing();
    let _ = feed_at(
        &mut state,
        2_000,
        press(2, ReviewCommand::ReviewCurrentWord, 0),
    );
    assert_eq!(
        asked(&state),
        text(line_at(
            TextPoint::At(TextPosition::at(TextAnchor(100))),
            None
        ))
    );
    assert_eq!(
        feed_at(&mut state, 2_100, read(4, 0, line("one two\r\n", 100, 0))),
        ["Text(\"one\")"]
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
            answer(OUTPOST, 4, Reply::Text(TextReply::AnchorLost))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(asked(&state), text(line_at(TextPoint::Caret, None)));
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
            answer(OUTPOST, 3, Reply::Text(TextReply::NoText))
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
        asked(&state),
        Wire::Text {
            op: TextOp::ReadRange { .. },
            ..
        }
    ));
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 3_200,
            input: answer(
                OUTPOST,
                6,
                Reply::Text(TextReply::Range {
                    text: "o".to_owned(),
                    truncated: false,
                }),
            ),
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
        asked(&state),
        Wire::Text {
            op: TextOp::Select { .. },
            ..
        }
    ));
    let said = feed_at(
        &mut state,
        3_200,
        answer(
            OUTPOST,
            6,
            Reply::Text(TextReply::SelectionPlaced {
                changes: vec![verbatim_model::SelectionChange {
                    selected: true,
                    text: "o".to_owned(),
                    characters: 1,
                }],
                baseline: TextAnchor(80),
            }),
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
        asked(&state),
        text(TextOp::Location(TextPoint::At(TextPosition::at(
            TextAnchor(100)
        ))))
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_100,
            answer(
                OUTPOST,
                4,
                Reply::Text(TextReply::Location { x: 10, y: 20 })
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
