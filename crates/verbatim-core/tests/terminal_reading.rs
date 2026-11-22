//! The focused terminal read through the public reducer as the shell drives
//! it (`terminal_support`): the simulated terminal answering each step of
//! Core's window reads, review reads and searches as its provider does, and
//! what Core says of it (`books/dev/src/explanation/design/terminal-model.md`):
//! new output line by line, the review cursor on its row as output moves
//! it, the keys' answers, "Report status bar", and the command marks.

mod desk;
#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;

use terminal_support::{Asked, Session, plain, said};
use verbatim_model::{
    Effect, Input, Message, ReaderSettings, ReviewCommand, SegmentContent, TraceId,
};
use verbatim_term_sim::Flavor;

/// Lines `range`, each ended.
fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "line {line}\r\n");
        text
    })
}

/// A console host `width` by `height` with `history` rows, showing `text`.
fn showing(width: usize, height: usize, history: usize, text: &str) -> Session {
    Session::new(Flavor::ConsoleHost, (width, height, history), text)
}

#[test]
fn new_output_is_spoken_line_by_line() {
    let mut session = showing(20, 4, 100, "");
    session.write("alpha\r\nbeta\r\n", 100);
    assert_eq!(session.heard(), plain(&["alpha", "beta"]));
    assert_eq!(session.requests(), [Asked::Window]);
}

/// Every window read Core takes is given to the control plane's event
/// stream, which the end-to-end suite's waits on a prompt read: the
/// output's read and a review command's own.
#[test]
fn every_terminal_read_core_takes_is_the_control_planes_terminal_read() {
    let mut session = showing(20, 4, 100, "");
    session.write("alpha\r\n", 100);
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    let _ = session.heard();
    let windows = session
        .requests()
        .iter()
        .filter(|asked| matches!(asked, Asked::Window))
        .count();
    assert_eq!((windows, session.observed), (2, 2));
}

// The review cursor.

/// The review position moves with its row as output scrolls it up the
/// screen and into the history, and each command reads the window first.
#[test]
fn the_review_position_moves_with_its_row_as_output_scrolls_it() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\nthree\r\n");
    // Up from the caret's line, the blank row below the output, to "three"
    // and "two".
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["three"]));
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["two"]));
    session.write("four\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n", 200);
    assert_eq!(
        session.heard(),
        plain(&["four", "five", "six", "seven", "eight", "nine"])
    );
    let _ = session.requests();
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["two"]));
    // The window, then the rows around the place.
    assert_eq!(session.requests(), [Asked::Window, Asked::Lines]);
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["three"]));
    // The window, the rows around the place, and the move from it.
    assert_eq!(
        session.requests(),
        [Asked::Window, Asked::Lines, Asked::Lines]
    );
}

/// A four-row console host whose history holds four rows, full: the review
/// position on "line 11", then two lines of output rotate every row up two
/// places under the buffer's fixed positions. The review position moves
/// with its row (`books/dev/src/explanation/design/terminal-model.md`, "The
/// review cursor"; build 9's design, 5.6, F11), so the next command reads
/// "line 11" again, not the row now at its old position.
#[test]
fn the_review_position_moves_with_its_row_as_a_full_history_rotates() {
    let mut session = showing(20, 4, 4, &numbered(1..=12));
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["line 12", "line 11"]));
    session.write(&numbered(13..=14), 100);
    assert_eq!(session.heard(), plain(&["line 13", "line 14"]));
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["line 11"]));
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["line 12"]));
}

/// A full history on a three-row screen: the review position on "line 4",
/// then a flood that carries every row further up than the document holds.
/// Its place leaves the document with its line, so the review cursor keeps
/// the buffer's position it read last and reads the row there now
/// (`books/dev/src/explanation/design/terminal-model.md`, "Limits").
#[test]
fn a_review_place_whose_line_left_the_document_keeps_its_position() {
    let mut session = showing(20, 3, 9, &numbered(1..=12));
    session.command(ReviewCommand::ReviewTop, 0);
    session.command(ReviewCommand::ReviewNextLine, 0);
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["line 2", "line 3", "line 4"]));
    session.write(&numbered(13..=24), 100);
    let _ = session.heard();
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["line 16"]));
}

#[test]
fn moving_up_reads_the_history_to_its_start() {
    let mut session = showing(20, 3, 100, &numbered(1..=12));
    session.command(ReviewCommand::ReviewTop, 0);
    assert_eq!(session.heard(), plain(&["line 1"]));
    // Up from the document's first row goes nowhere.
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["Message(Top) line 1"]));
    for line in ["line 2", "line 3", "line 4"] {
        session.command(ReviewCommand::ReviewNextLine, 0);
        assert_eq!(session.heard(), plain(&[line]));
    }
}

/// A review line ends at its text: no padding is reviewed.
#[test]
fn a_row_ends_at_its_text() {
    let mut session = showing(20, 4, 100, "ab\r\ncd\r\n");
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["cd", "ab"]));
    session.command(ReviewCommand::ReviewNextCharacter, 0);
    session.command(ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(session.heard(), plain(&["b", "Message(Right) b"]));
}

/// A program erases the screen and the history and prints them again with
/// two lines added at the top, as Pi's regular mode does on Control+O: the
/// review position's line is two rows lower, and is found there.
#[test]
fn a_review_place_is_found_lower_after_its_history_was_printed_again_longer() {
    let lines = numbered(1..=12);
    let mut session = showing(20, 3, 100, &lines);
    session.command(ReviewCommand::ReviewTop, 0);
    session.command(ReviewCommand::ReviewNextLine, 0);
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["line 1", "line 2", "line 3"]));
    session.write(
        &format!("\x1b[2J\x1b[3J\x1b[Hnew a\r\nnew b\r\n{lines}"),
        100,
    );
    let _ = session.heard();
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["line 3"]));
}

#[test]
fn the_review_reads_where_the_caret_is() {
    let mut session = showing(20, 4, 100, "ready> abc");
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    session.command(ReviewCommand::ReviewPreviousCharacter, 0);
    assert_eq!(session.heard(), plain(&["ready> abc", "c"]));
}

// The keys' answers.

#[test]
fn a_typed_character_is_echoed_for_its_key_by_the_read_that_shows_it() {
    let mut session = showing(20, 4, 100, "$ ");
    let key = TraceId::from_raw(2);
    session.typed("l", 1000, key);
    assert_eq!(session.heard(), []);
    session.write("l", 1050);
    assert_eq!(session.heard(), [said(Some(key), "l")]);
}

#[test]
fn a_read_after_every_window_carries_no_key() {
    let mut session = showing(20, 4, 100, "$ ");
    session.typed("l", 1000, TraceId::from_raw(2));
    // A line of output after the window: no key.
    session.write("\r\nlate\r\n", 1400);
    assert_eq!(session.heard(), plain(&["late"]));
}

#[test]
fn with_passwords_spoken_a_character_is_heard_as_typed_and_its_asterisk_as_shown() {
    // A password prompt with "speak passwords" on: the character is echoed
    // as it is typed, and the asterisk the prompt shows for it is output
    // in the key's window.
    let mut session = showing(30, 4, 10, "Password: ");
    session.settings(ReaderSettings {
        speak_terminal_passwords: true,
        ..ReaderSettings::default()
    });
    let key = TraceId::from_raw(2);
    session.typed("s", 1000, key);
    assert_eq!(session.heard(), [said(None, "s")]);
    session.write("*", 1050);
    assert_eq!(session.heard(), [said(Some(key), "*")]);
}

#[test]
fn lines_of_one_and_two_characters_after_enter_are_output_not_echo() {
    // Enter, then a program printing "y" and "ok": short as they are, both
    // are output, never taken for the echo of typing.
    let mut session = showing(30, 6, 10, "ready> ");
    let key = TraceId::from_raw(2);
    session.typed("\r", 1000, key);
    session.write("\r\ny\r\nok\r\nready> ", 1050);
    assert_eq!(
        session.heard(),
        [
            said(Some(key), "y"),
            said(Some(key), "ok"),
            said(None, "ready>")
        ]
    );
}

// "Report status bar".

const STATUS_BAR: ReviewCommand = ReviewCommand::ReportStatusBar;

#[test]
fn the_status_bar_is_the_lines_below_the_caret() {
    // Pi's editor: the transcript, the editor's line with the caret, and its
    // footer of two lines below it, one segment each.
    let mut session = showing(
        20,
        6,
        100,
        "transcript\r\n> fix it\r\nmodel: x\r\ntokens: 12\x1b[2;9H",
    );
    session.command(STATUS_BAR, 0);
    assert_eq!(session.heard(), plain(&["model: x tokens: 12"]));
    assert_eq!(session.requests(), [Asked::Window]);
}

#[test]
fn the_status_bar_is_the_rewritten_rows_at_the_bottom_with_the_caret_on_the_last() {
    let mut session = showing(30, 5, 100, "output\r\nprogress 1\r\nstatus 1");
    // The program rewrites its last two rows, leaving the caret on the last.
    session.write("\x1b[1A\rprogress 2\r\nstatus 2", 100);
    let _ = session.heard();
    session.command(STATUS_BAR, 0);
    assert_eq!(session.heard(), plain(&["progress 2 status 2"]));
}

#[test]
fn the_status_bar_is_the_last_line_with_text_with_no_footer() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\nready>");
    session.command(STATUS_BAR, 0);
    assert_eq!(session.heard(), plain(&["ready>"]));
}

#[test]
fn a_blank_screen_has_no_status_bar_information() {
    let mut session = showing(20, 4, 100, "");
    session.command(STATUS_BAR, 0);
    assert_eq!(session.heard(), plain(&["Message(NoStatusBarInformation)"]));
}

#[test]
fn the_status_bar_is_spelled_on_the_second_press_and_copied_on_the_third() {
    let mut session = showing(20, 4, 100, "one\r\nab\r\ncd\x1b[1;4H");
    // The lines spelled as one text, the space between them spelled.
    session.command(STATUS_BAR, 1);
    assert_eq!(session.heard(), plain(&["a b Message(Space) c d"]));
    session.command(STATUS_BAR, 2);
    assert_eq!(session.heard(), []);
    assert_eq!(session.copied, ["ab\r\ncd"]);
}

#[test]
fn the_status_bar_leaves_the_review_cursor_where_it_was() {
    let mut session = showing(20, 5, 100, "one\r\ntwo\r\nready>\r\nfooter");
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["ready>", "two"]));
    session.command(STATUS_BAR, 0);
    session.command(ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(session.heard(), plain(&["footer", "two"]));
}

#[test]
fn a_footer_written_just_before_the_command_is_the_one_read() {
    let mut session = showing(20, 4, 100, "ready>\r\n\r\n\r\nstatus: busy\x1b[1;7H");
    // The footer changes with no read before the command: the command's
    // own read takes it, says its change, and reads it.
    session
        .sim
        .terminal
        .write(b"\x1b[s\x1b[4;1Hstatus: done\x1b[u");
    session.command(STATUS_BAR, 0);
    assert_eq!(session.heard(), plain(&["done", "status: done"]));
}

#[test]
fn outside_a_terminal_no_status_line_is_found() {
    let mut state = verbatim_core::SrState::new();
    let effects = verbatim_core::reduce(
        &mut state,
        &mut verbatim_core::Runtime::default(),
        &Input::Queue {
            trace_id: TraceId::from_raw(1),
            at_us: 0,
            input: verbatim_model::request::QueueInput::Press(Box::new(
                verbatim_model::request::Press {
                    key_seq: verbatim_model::KeySeq::new(1),
                    pressed_at_us: 0,
                    action: verbatim_model::request::PressAction::Command {
                        command: STATUS_BAR,
                        repeat: 0,
                    },
                },
            )),
        },
    );
    let spoken: Vec<Vec<SegmentContent>> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .map(|segment| segment.content.clone())
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(
        spoken,
        [vec![SegmentContent::Message(Message::NoStatusLineFound)]]
    );
}

// Moving by command.

const UP: ReviewCommand = ReviewCommand::PreviousCommandMark;
const DOWN: ReviewCommand = ReviewCommand::NextCommandMark;

/// Types `command` at the prompt from `at_ms`, a character every 10
/// milliseconds, each shown by a read, then presses Enter and shows
/// `output` and a new prompt; returns when the next key may come.
fn enter(session: &mut Session, command: &str, output: &str, at_ms: u64) -> u64 {
    let mut at_ms = at_ms;
    for character in command.chars() {
        session.typed(&character.to_string(), at_ms, TraceId::from_raw(2));
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    session.typed("\r", at_ms, TraceId::from_raw(2));
    session.write(&format!("\r\n{output}$ "), at_ms + 5);
    let _ = session.heard();
    let _ = session.requests();
    at_ms + 2000
}

/// How many command mark searches `requests` holds.
fn searches(requests: &[Asked]) -> usize {
    requests
        .iter()
        .filter(|asked| matches!(asked, Asked::Search))
        .count()
}

#[test]
fn up_and_down_move_between_the_marks_and_say_nothing_past_them() {
    let mut session = showing(30, 10, 100, "$ ");
    let mut at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    at_ms = enter(&mut session, "echo two", "two\r\n", at_ms);
    enter(&mut session, "echo three", "three\r\n", at_ms);
    for line in ["$ echo three", "$ echo two", "$ echo one"] {
        session.command(UP, 0);
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.command(UP, 0);
    assert_eq!(session.heard(), []);
    for line in ["$ echo two", "$ echo three"] {
        session.command(DOWN, 0);
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.command(DOWN, 0);
    assert_eq!(session.heard(), []);
    // The review cursor is on the mark's line, as a line move leaves it.
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(session.heard(), plain(&["two"]));
}

#[test]
fn input_printed_lower_is_marked_on_its_new_row() {
    // Pi's regular mode: the editor between two rules, the input typed on
    // its own row; Enter clears the editor and prints the input into the
    // transcript above it.
    let mut session = showing(
        30,
        8,
        100,
        "transcript\r\n---\r\n\r\n---\r\nfooter\x1b[3;1H",
    );
    let mut at_ms = 1000;
    for character in "hi there".chars() {
        session.typed(&character.to_string(), at_ms, TraceId::from_raw(2));
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    session.typed("\r", at_ms, TraceId::from_raw(2));
    session.write("\x1b[2K\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 200);
    let _ = session.heard();
    session.command(UP, 0);
    assert_eq!(session.heard(), plain(&["hi there"]));
}

#[test]
fn input_printed_after_the_quiet_period_is_not_marked() {
    let mut session = showing(
        30,
        8,
        100,
        "transcript\r\n---\r\n\r\n---\r\nfooter\x1b[3;1H",
    );
    let mut at_ms = 1000;
    for character in "hi there".chars() {
        session.typed(&character.to_string(), at_ms, TraceId::from_raw(2));
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    session.typed("\r", at_ms, TraceId::from_raw(2));
    // The editor clears at once; the input is printed only after the quiet
    // period.
    session.write("\x1b[2K\r", at_ms + 100);
    session.write("\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 1500);
    let _ = session.heard();
    session.command(UP, 0);
    assert_eq!(session.heard(), []);
}

/// A mark carried above the screen by output is found at its place, which
/// every read moved by the rows written, with no search.
#[test]
fn a_mark_above_the_screen_is_found_at_its_place() {
    let mut session = showing(30, 4, 100, "$ ");
    enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), 5000);
    let _ = session.heard();
    session.command(UP, 0);
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    assert_eq!(searches(&session.requests()), 0);
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["one"]));
}

/// A program prints the command's line again below it, unmarked: Up goes
/// to the mark's own line, whose output follows it, not to the copy.
#[test]
fn a_marks_line_printed_again_nearer_the_review_position_is_passed_over() {
    let mut session = showing(30, 4, 100, "$ ");
    let at_ms = enter(&mut session, "echo hi", "hi\r\n", 1000);
    session.write(&format!("$ echo hi\r\ncopy\r\n{}", numbered(1..=20)), at_ms);
    let _ = session.heard();
    session.command(UP, 0);
    assert_eq!(session.heard(), plain(&["$ echo hi"]));
    session.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(session.heard(), plain(&["hi"]));
    session.command(ReviewCommand::ReviewPreviousLine, 0);
    let _ = session.heard();
    session.command(UP, 0);
    assert_eq!(session.heard(), []);
}

#[test]
fn a_command_entered_twice_is_reached_at_each_mark() {
    let mut session = showing(30, 4, 100, "$ ");
    let at_ms = enter(&mut session, "ls", "a\r\n", 1000);
    enter(&mut session, "ls", "a\r\n", at_ms);
    session.write(&numbered(1..=20), 6000);
    let _ = session.heard();
    for _ in 0..2 {
        session.command(UP, 0);
        assert_eq!(session.heard(), plain(&["$ ls"]));
    }
    session.command(UP, 0);
    assert_eq!(session.heard(), []);
    session.command(DOWN, 0);
    assert_eq!(session.heard(), plain(&["$ ls"]));
    session.command(DOWN, 0);
    assert_eq!(session.heard(), []);
}
