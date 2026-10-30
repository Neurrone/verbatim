//! Core's terminal model (`docs/design/terminal-model.md`, "Where it
//! runs"), driven through `reduce` with snapshots of the simulated
//! terminal, read as the outpost reads it for Core, with Core's requests
//! for reads, history, colours and the screen's shape answered from it
//! (`terminal_support`): what the reading rules say is Core's speech; the
//! keys Core takes in are the rules' keys, given before the reads that
//! began after them; a read's speech goes out for the newest key whose
//! window holds it; reads owed are waited for; the timer gives the keys
//! first; the review cursor's requests are answered from the memory, a
//! place moving with its row, rows above the memory read from the history;
//! a snapshot rebuilds its read exactly; a terminal session survives a
//! flight recorder's dump and replay; and the colours of a key's window
//! reach the echo's read whatever reads the outpost leaves unsent or takes
//! at once; and "Report status bar" reads the footer after a fresh read,
//! spelled and copied on later presses, the review cursor left alone.

mod terminal_support;

use std::fmt::Write as _;
use std::io::BufReader;

use terminal_support::{Outpost, Said, Session, said};
use verbatim_core::{RecordedInput, SrState, dump, read_of, replay, snapshot_of};
use verbatim_model::{
    CaretKey, CaretMotion, Effect, Input, Message, ReaderSettings, ReviewCommand, SegmentContent,
    TerminalScope, TextOp, TraceId, TypingEcho,
};
use verbatim_term_sim::conformance::{history, streams};
use verbatim_term_sim::{Flavor, Query};
use verbatim_text::terminal_reading::{Settings, TerminalReader};
use verbatim_text::terminal_rows::{Read, ReadText};

/// Heard with no key.
fn plain(words: &[&str]) -> Vec<Said> {
    words.iter().map(|words| said(None, words)).collect()
}

/// A review command, its first press.
fn command(command: ReviewCommand) -> Input {
    pressed(command, 0)
}

fn pressed(command: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        pressed_at_ms: 0,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        command,
        repeat,
    }
}

const FRESH: TextOp = TextOp::TerminalReadNow(TerminalScope::Fresh);

/// A session whose terminal shows `text`, written before its first read.
fn showing(width: usize, height: usize, history: usize, text: &str) -> Session {
    let mut outpost = Outpost::new(Flavor::ConsoleHost, width, height, history);
    outpost.sim.write(text.as_bytes());
    Session::with(outpost)
}

/// Lines `range`, each ended.
fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "line {line}\r\n");
        text
    })
}

#[test]
fn new_output_is_spoken_line_by_line() {
    let mut session = Session::new(Flavor::ConsoleHost, 20, 4, 100);
    session.write("alpha\r\nbeta\r\n", 100);
    assert_eq!(session.heard(), plain(&["alpha", "beta"]));
    assert_eq!(session.requests(), []);
}

#[test]
fn the_review_position_moves_with_its_row_as_output_scrolls_it() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\nthree\r\n");
    // Up from the caret's line, the blank row below the output, to "three"
    // and "two", each after a fresh read.
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["three"]));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["two"]));
    assert_eq!(session.requests(), [FRESH, FRESH]);
    // Output scrolls "two" up the screen and into the history above the
    // rows the memory keeps.
    session.write("four\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n", 200);
    assert_eq!(
        session.heard(),
        plain(&["four", "five", "six", "seven", "eight", "nine"])
    );
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["two"]));
    // Down from it, through the history, read again for each command.
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["three"]));
    // Each history read asks for a screen more than the command needs.
    let history = TextOp::TerminalHistory {
        above: 13,
        whole: false,
    };
    assert_eq!(session.requests(), [FRESH, history.clone(), FRESH, history]);
}

/// A terminal of four rows whose review position is on "two", in the
/// history above the rows the memory keeps, with nine lines printed below
/// it and seven above it, so that no history read below reaches the
/// document's start.
fn reviewing_two() -> Session {
    let text = format!("{}one\r\ntwo\r\nthree\r\n", numbered(1..=6));
    let mut session = showing(20, 4, 100, &text);
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    session.write("four\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n", 200);
    let _ = session.heard();
    let _ = session.requests();
    session
}

#[test]
fn output_arriving_while_a_history_read_is_out_is_covered_by_the_screen_more_it_asks_for() {
    // Before the outpost reads the history, the program prints two more
    // lines, read and taken first. The history is counted from the newest
    // read's start, and the screen more than the command needed still holds
    // "two": the command reads it with one history read.
    let mut session = reviewing_two();
    session.output_before_histories = [("ten\r\neleven\r\n".to_owned(), 300)].into();
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["ten", "eleven", "two"]));
    assert_eq!(
        session.requests(),
        [
            FRESH,
            TextOp::TerminalHistory {
                above: 13,
                whole: false,
            }
        ]
    );
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["three"]));
}

#[test]
fn more_than_a_screen_arriving_while_a_history_read_is_out_ends_the_command() {
    // Five more lines, more than the screen more the command asked for: the
    // history read does not reach "two", so the command says nothing and
    // reads no more history. The review position stays on "two", which the
    // next command reads.
    let mut session = reviewing_two();
    session.output_before_histories = [(
        "ten\r\neleven\r\ntwelve\r\nthirteen\r\nfourteen\r\n".to_owned(),
        300,
    )]
    .into();
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(
        session.heard(),
        plain(&["ten", "eleven", "twelve", "thirteen", "fourteen"])
    );
    assert_eq!(
        session.requests(),
        [
            FRESH,
            TextOp::TerminalHistory {
                above: 13,
                whole: false,
            }
        ]
    );
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["two"]));
}

#[test]
fn a_history_answer_counted_from_a_read_core_did_not_take_ends_the_command() {
    // The review position on "line 4", in the history above the memory's
    // rows; before the outpost reads the history, a flood carries the last
    // read's rows out of the buffer. Core does not take the read that shows
    // it, asking for the whole document instead, which comes after the
    // history; the history is counted from the start of the read Core did
    // not take, so it cannot be placed: the command says nothing, and the
    // next one reads where the whole document left the review position.
    let mut session = showing(20, 3, 9, &numbered(1..=12));
    session.input(&command(ReviewCommand::ReviewTop));
    session.input(&command(ReviewCommand::ReviewNextLine));
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 2", "line 3", "line 4"]));
    let _ = session.requests();
    session.output_before_histories = [(numbered(13..=40), 100)].into();
    session.hold_whole = true;
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    session.answer_held();
    let mut flood = vec!["Phrase(SkippedUncountedLines)".to_owned()];
    flood.extend((30..=40).map(|line| format!("line {line}")));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, flood);
    assert_eq!(
        session.requests(),
        [
            FRESH,
            TextOp::TerminalHistory {
                above: 13,
                whole: false,
            },
            TextOp::TerminalReadNow(TerminalScope::Whole)
        ]
    );
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["line 32"]));
}

#[test]
fn a_review_place_whose_line_left_its_search_keeps_its_place() {
    // A full history on a three-row screen: the review position on a line
    // in the history above the memory's rows, and then a flood that carries
    // every row further up than a screen. Its line is nowhere from its
    // place to a screen above it, so it keeps its place and reads the row
    // there now ("The review anchor").
    let mut session = showing(20, 3, 9, &numbered(1..=12));
    session.input(&command(ReviewCommand::ReviewTop));
    session.input(&command(ReviewCommand::ReviewNextLine));
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 2", "line 3", "line 4"]));
    session.write(&numbered(13..=24), 100);
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["line 16"]));
}

#[test]
fn a_review_place_above_the_memory_is_found_lower_after_its_history_was_printed_again_longer() {
    // A program erases the screen and the history and prints them again with
    // two lines added at the top, as Pi's regular mode does on Control+O:
    // the review position's line, in the history above the memory's rows,
    // is now two rows lower than its place, and is found there, a screen
    // below the place being searched as well as a screen above it ("The
    // review anchor").
    let lines = numbered(1..=12);
    let mut session = showing(20, 3, 100, &lines);
    session.input(&command(ReviewCommand::ReviewTop));
    session.input(&command(ReviewCommand::ReviewNextLine));
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 1", "line 2", "line 3"]));
    session.write(
        &format!("\x1b[2J\x1b[3J\x1b[Hnew a\r\nnew b\r\n{lines}"),
        100,
    );
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["line 3"]));
}

#[test]
fn moving_above_the_memorys_rows_reads_the_history_up_to_its_start() {
    let mut session = showing(20, 3, 100, &numbered(1..=12));
    session.input(&command(ReviewCommand::ReviewTop));
    assert_eq!(session.heard(), plain(&["line 1"]));
    assert_eq!(
        session.requests(),
        [
            FRESH,
            TextOp::TerminalHistory {
                above: 0,
                whole: true
            }
        ]
    );
    // Up from the document's first row goes nowhere.
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["Message(Top) line 1"]));
    for line in ["line 2", "line 3", "line 4"] {
        session.input(&command(ReviewCommand::ReviewNextLine));
        assert_eq!(session.heard(), plain(&[line]));
    }
}

#[test]
fn a_row_ends_at_its_text_and_a_range_joins_lines() {
    let mut session = showing(20, 4, 100, "ab\r\ncd\r\n");
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    // No padding: the row's text is all of it.
    assert_eq!(session.heard(), plain(&["cd", "ab"]));
    session.input(&command(ReviewCommand::ReviewNextCharacter));
    session.input(&command(ReviewCommand::SetStartMarker));
    session.input(&command(ReviewCommand::ReviewNextLine));
    session.input(&pressed(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(
        session.heard(),
        plain(&["b", "Message(StartMarked)", "cd", "Message(CannotSelect)"])
    );
    session.input(&pressed(ReviewCommand::SelectThenCopy, 1));
    assert_eq!(session.copied, ["b\r\ncd"]);
}

#[test]
fn the_review_reads_where_the_caret_is() {
    let mut session = showing(20, 4, 100, "ready> abc");
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    session.input(&command(ReviewCommand::ReviewPreviousCharacter));
    assert_eq!(session.heard(), plain(&["ready> abc", "c"]));
}

// Keys against reads.

fn typed(text: &str, at_ms: u64) -> (TraceId, Input) {
    let trace_id = TraceId::mint();
    (
        trace_id,
        Input::CharacterTyped {
            key_seq: verbatim_model::KeySeq::NONE_YET,
            trace_id,
            text: text.to_owned(),
            at_ms,
        },
    )
}

#[test]
fn a_key_whose_time_is_after_a_reads_start_is_given_to_the_rules_after_that_read() {
    let mut session = showing(20, 4, 100, "$ ");
    let (key, input) = typed("l", 1000);
    session.input(&input);
    assert_eq!(
        session.requests(),
        [TextOp::TerminalColours {
            from_ms: 1000,
            until_ms: 1300
        }]
    );
    // The program's own output, in a read that began before the key: no
    // key's window holds it.
    session.write("\r\nbuild done\r\n$ ", 990);
    assert_eq!(session.heard(), plain(&["build done", "$"]));
    // The key's echo, in a read that began after it, goes out for it.
    session.write("l", 1050);
    assert_eq!(session.heard(), [said(Some(key), "l")]);
}

#[test]
fn a_reads_speech_goes_out_for_the_newest_key_whose_window_holds_it() {
    // "echo onex": Up Arrow held, "x" typed while it waits and deferred
    // behind it. Both reach the program before the read that shows the
    // recalled line with the "x", one window, and its speech goes out for
    // "x", whose number the speech manager's fence keeps.
    let mut session = showing(20, 3, 10, "$ ");
    let up = TraceId::mint();
    session.input(&Input::HeldKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: up,
        key: CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        pressed_at_ms: 200,
    });
    let (x, input) = typed("x", 210);
    session.input(&input);
    // The held key's caret read, answered as failed: Up is passed on with
    // the hook's time, and "x" after it.
    session.release_held();
    session.write("echo onex", 250);
    assert_eq!(session.heard(), [said(Some(x), "echo onex")]);
}

#[test]
fn a_character_shown_after_the_recalled_line_is_heard_with_typing_echo_off() {
    // Up Arrow, then "x" typed before the recalled line is read: the line
    // is spoken, and the "x", shown by a later read, is output, heard with
    // the typing echo off (a limit of the terminal design).
    let mut session = showing(20, 3, 10, "$ ");
    session.input(&Input::Settings(ReaderSettings {
        speak_typed_characters: TypingEcho::Off,
        ..ReaderSettings::default()
    }));
    session.input(&Input::HeldKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        pressed_at_ms: 200,
    });
    let (x, input) = typed("x", 210);
    session.input(&input);
    session.release_held();
    session.write("echo one", 220);
    assert_eq!(session.heard(), [said(Some(x), "echo one")]);
    session.write("x", 250);
    assert_eq!(session.heard(), [said(Some(x), "x")]);
}

#[test]
fn a_read_after_every_window_carries_no_key() {
    let mut session = showing(20, 4, 100, "$ ");
    let (_, input) = typed("l", 1000);
    session.input(&input);
    // A line of output after the window: no key.
    session.write("\r\nlate\r\n", 1400);
    assert_eq!(session.heard(), plain(&["late"]));
}

#[test]
fn live_snapshots_that_arrive_while_a_whole_read_is_owed_are_dropped() {
    let mut session = showing(20, 3, 9, &numbered(1..=3));
    session.hold_whole = true;
    // A flood that carries the last read's rows out of the buffer: the read
    // keeps nothing, and the rules ask for the whole document.
    session.write(&numbered(4..=40), 100);
    assert_eq!(session.heard(), []);
    assert_eq!(
        session.requests(),
        [TextOp::TerminalReadNow(TerminalScope::Whole)]
    );
    // A live read begun before the whole read: dropped.
    session.write(&numbered(41..=41), 110);
    assert_eq!(session.heard(), []);
    assert_eq!(session.requests(), []);
    session.answer_held();
    assert_eq!(
        session.heard(),
        plain(&[
            "Phrase(SkippedUncountedLines)",
            "line 31",
            "line 32",
            "line 33",
            "line 34",
            "line 35",
            "line 36",
            "line 37",
            "line 38",
            "line 39",
            "line 40",
            "line 41"
        ])
    );
}

#[test]
fn the_timer_gives_the_keys_that_reached_the_program_by_then_first() {
    // A ruler a key changed is held until the keys pause; a second key
    // before the timer, with no read since, moves the pause later, so the
    // timer says nothing and comes again.
    let mut session = showing(20, 5, 10, "line one\r\nline two\r\n\r\n1,1 Top\x1b[1;1H");
    let up = TraceId::mint();
    session.input(&Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: up,
        key: CaretKey {
            motion: CaretMotion::NextLine,
            select: false,
        },
        pressed_at_ms: 1000,
    });
    session.write("\x1b[4;1H2,1 Top\x1b[2;1H", 1050);
    assert_eq!(session.heard(), [said(Some(up), "line two")]);
    assert_eq!(session.timers, [Some(2000)]);
    session.input(&Input::ClearingKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        at_ms: 1900,
    });
    session.timers.clear();
    session.input(&Input::TerminalTimer {
        trace_id: TraceId::mint(),
        at_ms: 2000,
    });
    assert_eq!(session.heard(), []);
    assert_eq!(session.timers, [Some(3250)]);
}

// The wire form of a read.

#[test]
fn a_snapshot_rebuilds_its_read_and_the_rules_take_it_alike() {
    for (name, stream) in streams() {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let mut terminal = verbatim_term_sim::Terminal::new(flavor, 120, 30, history(flavor));
            let mut original = TerminalReader::new(Settings::default());
            let mut rebuilt = TerminalReader::new(Settings::default());
            let mut query = Query::default();
            for (chunk, bytes) in stream.chunks.iter().enumerate() {
                terminal.write(bytes.as_bytes());
                let at_ms = u64::try_from(chunk).expect("a few chunks") * 10;
                loop {
                    let snapshot = terminal.snapshot(&query).expect("a simulated read");
                    let read = Read {
                        text: ReadText::unpadded(&snapshot.text),
                        screen: snapshot.screen,
                        from_start: snapshot.from_start,
                        shrank: snapshot.shrank,
                        at_document_start: snapshot.at_document_start,
                        alternate: snapshot.alternate,
                        first_row: snapshot.first_row,
                        caret_rows_to_end: snapshot.caret_rows_to_end,
                        caret_line: snapshot.caret_line,
                        caret_column: snapshot.caret_column,
                        at_ms,
                        above: usize::try_from(snapshot.above).expect("rows"),
                    };
                    let wire = serde_json::to_string(&snapshot_of(&read, false, None))
                        .expect("a snapshot serializes");
                    let again = read_of(&serde_json::from_str(&wire).expect("a snapshot parses"));
                    assert_eq!(again, read, "{name} in {flavor:?}, chunk {chunk}");
                    let step = original.read(&read, false, None);
                    assert_eq!(
                        rebuilt.read(&again, false, None),
                        step,
                        "{name} in {flavor:?}, chunk {chunk}"
                    );
                    let height = step.outcome.as_ref().map_or(0, |outcome| outcome.height);
                    let screen_rows = u32::try_from(height).expect("a small screen");
                    if step.whole_document || step.history_block {
                        query = Query {
                            start: None,
                            screen_rows,
                            whole: step.whole_document,
                            above: if step.whole_document { 0 } else { screen_rows },
                        };
                        continue;
                    }
                    let blank = step.outcome.as_ref().is_some_and(|outcome| outcome.blank);
                    query = Query {
                        start: if blank {
                            query.start
                        } else {
                            Some(snapshot.next_start)
                        },
                        screen_rows,
                        whole: false,
                        above: u32::try_from(original.memory().above()).expect("rows"),
                    };
                    break;
                }
            }
        }
    }
}

// "Report status bar".

const STATUS_BAR: ReviewCommand = ReviewCommand::ReportStatusBar;

/// The segments of the last utterance Core spoke.
fn last_spoken(session: &Session) -> Vec<SegmentContent> {
    session
        .log
        .iter()
        .flat_map(|(_, effects)| effects)
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(
                utterance
                    .segments
                    .iter()
                    .map(|segment| segment.content.clone())
                    .filter(|content| !matches!(content, SegmentContent::Mark(_)))
                    .collect(),
            ),
            _ => None,
        })
        .next_back()
        .expect("Core spoke")
}

#[test]
fn the_status_bar_is_the_lines_below_the_caret_one_segment_each() {
    // Pi's editor: the transcript, the editor's line with the caret, and its
    // footer of two lines below it.
    let mut session = showing(
        20,
        6,
        100,
        "transcript\r\n> fix it\r\nmodel: x\r\ntokens: 12\x1b[2;9H",
    );
    session.log.clear();
    session.input(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["model: x tokens: 12"]));
    assert_eq!(
        last_spoken(&session),
        [
            SegmentContent::Text("model: x".to_owned()),
            SegmentContent::Text("tokens: 12".to_owned()),
        ]
    );
    assert_eq!(session.requests(), [FRESH]);
}

#[test]
fn the_status_bar_is_the_rewritten_rows_at_the_bottom_with_the_caret_on_the_last() {
    let mut session = showing(30, 5, 100, "output\r\nprogress 1\r\nstatus 1");
    // The program rewrites its last two rows, leaving the caret on the last.
    session.write("\x1b[1A\rprogress 2\r\nstatus 2", 100);
    let _ = session.heard();
    session.input(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["progress 2 status 2"]));
}

#[test]
fn the_status_bar_is_the_last_line_with_text_with_no_footer() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\nready>");
    session.input(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["ready>"]));
}

#[test]
fn a_blank_screen_has_no_status_bar_information() {
    let mut session = showing(20, 4, 100, "");
    session.input(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["Message(NoStatusBarInformation)"]));
}

#[test]
fn the_status_bar_is_spelled_on_the_second_press_and_copied_on_the_third() {
    let mut session = showing(20, 4, 100, "one\r\nab\r\ncd\x1b[1;4H");
    // The lines spelled as one text, the space between them spelled.
    session.input(&pressed(STATUS_BAR, 1));
    assert_eq!(session.heard(), plain(&["a b Message(Space) c d"]));
    session.input(&pressed(STATUS_BAR, 2));
    assert_eq!(session.heard(), []);
    assert_eq!(session.copied, ["ab\r\ncd"]);
}

#[test]
fn the_status_bar_leaves_the_review_cursor_where_it_was() {
    let mut session = showing(20, 5, 100, "one\r\ntwo\r\nready>\r\nfooter");
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["ready>", "two"]));
    session.input(&command(STATUS_BAR));
    session.input(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["footer", "two"]));
}

#[test]
fn a_footer_written_just_before_the_command_is_the_one_read() {
    let mut session = showing(20, 4, 100, "ready>\r\n\r\n\r\nstatus: busy\x1b[1;7H");
    // The footer changes with no live read before the command: the
    // command's fresh read takes it, and reads it, after speaking its change.
    session
        .outpost
        .sim
        .write(b"\x1b[s\x1b[4;1Hstatus: done\x1b[u");
    session.log.clear();
    session.input(&command(STATUS_BAR));
    // In the order Core speaks them; the support hears an utterance with no
    // index marks at once, ahead of output still queued.
    let spoken: Vec<String> = session
        .log
        .iter()
        .flat_map(|(_, effects)| effects)
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(terminal_support::words(&utterance.segments)),
            _ => None,
        })
        .collect();
    assert_eq!(spoken, ["done", "status: done"]);
}

#[test]
fn outside_a_terminal_no_status_line_is_found() {
    let mut state = SrState::new();
    let effects = verbatim_core::reduce(&mut state, &command(STATUS_BAR));
    let [Effect::Speak(utterance, _)] = effects.as_slice() else {
        panic!("one utterance, not {effects:?}");
    };
    assert_eq!(
        utterance
            .segments
            .iter()
            .map(|segment| segment.content.clone())
            .collect::<Vec<_>>(),
        [SegmentContent::Message(Message::NoStatusLineFound)]
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
        let (_, input) = typed(&character.to_string(), at_ms);
        session.input(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.input(&input);
    session.write(&format!("\r\n{output}$ "), at_ms + 5);
    let _ = session.heard();
    let _ = session.requests();
    at_ms + 2000
}

/// The search for a mark's line Core asks for.
fn find(needle: &str, from: i32) -> TextOp {
    TextOp::TerminalFind {
        needle: needle.to_owned(),
        from,
        rows: 1,
    }
}

#[test]
fn enter_on_a_typed_line_marks_it_and_a_bare_enter_does_not() {
    let mut session = showing(30, 6, 100, "$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    // A bare Enter: a new prompt, no mark.
    let (_, input) = typed("\r", at_ms);
    session.input(&input);
    session.write("\r\n$ ", at_ms + 5);
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    // Past the oldest mark nothing is said.
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    assert_eq!(session.requests(), [FRESH, FRESH]);
}

#[test]
fn a_line_recalled_with_up_arrow_and_entered_is_marked() {
    let mut session = showing(30, 8, 100, "$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    session.input(&Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        pressed_at_ms: at_ms,
    });
    session.write("echo one", at_ms + 5);
    let (_, input) = typed("\r", at_ms + 1000);
    session.input(&input);
    session.write("\r\none\r\n$ ", at_ms + 1005);
    let _ = session.heard();
    for _ in 0..2 {
        session.input(&command(UP));
        assert_eq!(session.heard(), plain(&["$ echo one"]));
    }
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn up_and_down_move_between_the_marks_and_say_nothing_past_them() {
    let mut session = showing(30, 10, 100, "$ ");
    let mut at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    at_ms = enter(&mut session, "echo two", "two\r\n", at_ms);
    enter(&mut session, "echo three", "three\r\n", at_ms);
    for line in ["$ echo three", "$ echo two", "$ echo one"] {
        session.input(&command(UP));
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    for line in ["$ echo two", "$ echo three"] {
        session.input(&command(DOWN));
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.input(&command(DOWN));
    assert_eq!(session.heard(), []);
    // The review cursor is on the mark's line, as a line move leaves it.
    session.input(&command(ReviewCommand::ReviewPreviousLine));
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
        let (_, input) = typed(&character.to_string(), at_ms);
        session.input(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.input(&input);
    session.write("\x1b[2K\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 200);
    let _ = session.heard();
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["hi there"]));
}

#[test]
fn input_printed_lower_again_is_marked_again() {
    // Pi's regular mode, the same input entered twice, the editor cleared
    // and the input printed in separate reads each time: the older mark
    // with the same text does not stand for the newer one.
    let mut session = showing(
        30,
        12,
        100,
        "transcript\r\n---\r\n\r\n---\r\nfooter\x1b[3;1H",
    );
    let mut at_ms = 1000;
    for _ in 0..2 {
        for character in "hi there".chars() {
            let (_, input) = typed(&character.to_string(), at_ms);
            session.input(&input);
            session.write(&character.to_string(), at_ms + 5);
            at_ms += 10;
        }
        let (_, input) = typed("\r", at_ms);
        session.input(&input);
        session.write("\x1b[2K\r", at_ms + 100);
        session.write("\x1b[1A\x1b[Lhi there\x1b[2B\r", at_ms + 500);
        at_ms += 2000;
    }
    let _ = session.heard();
    for _ in 0..2 {
        session.input(&command(UP));
        assert_eq!(session.heard(), plain(&["hi there"]));
    }
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
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
        let (_, input) = typed(&character.to_string(), at_ms);
        session.input(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.input(&input);
    // The editor clears at once; the input is printed only after the quiet
    // period.
    session.write("\x1b[2K\r", at_ms + 100);
    session.write("\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 1500);
    let _ = session.heard();
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn a_mark_above_the_memory_is_found_by_one_search() {
    let mut session = showing(30, 4, 100, "$ ");
    enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), 5000);
    let _ = session.heard();
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    // The document's 23 rows: the next read starts at row 19, and the
    // search works outward from the mark's own row, 19 rows above it.
    assert_eq!(session.requests(), [FRESH, find("$ echo one", 19)]);
}

#[test]
fn a_command_entered_twice_above_the_memory_is_found_at_each_mark() {
    // The search for each mark finds the other line with the same text too:
    // the match nearest the mark's place is its line.
    let mut session = showing(30, 4, 100, "$ ");
    let at_ms = enter(&mut session, "ls", "a\r\n", 1000);
    enter(&mut session, "ls", "a\r\n", at_ms);
    session.write(&numbered(1..=20), 6000);
    let _ = session.heard();
    for _ in 0..2 {
        session.input(&command(UP));
        assert_eq!(session.heard(), plain(&["$ ls"]));
    }
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    session.input(&command(DOWN));
    assert_eq!(session.heard(), plain(&["$ ls"]));
    session.input(&command(DOWN));
    assert_eq!(session.heard(), []);
}

#[test]
fn every_copy_of_a_command_entered_many_times_is_reached() {
    // More rows start with the command's text than one search takes: each
    // press searches outward from the next mark's own place, so each is
    // found, the oldest included.
    let mut session = showing(30, 30, 1000, "$ ");
    let mut at_ms = 1000;
    for _ in 0..22 {
        at_ms = enter(&mut session, "x", "", at_ms);
    }
    session.write(&numbered(1..=100), at_ms);
    let _ = session.heard();
    let _ = session.requests();
    for press in 0..22 {
        session.input(&command(UP));
        assert_eq!(session.heard(), plain(&["$ x"]), "press {press}");
        // Each mark lies above the rows Core keeps, found by one search.
        let finds = session
            .requests()
            .iter()
            .filter(|op| matches!(op, TextOp::TerminalFind { .. }))
            .count();
        assert_eq!(finds, 1, "press {press}");
        // The line below is the next copy's, or the output's for the newest.
        session.input(&command(ReviewCommand::ReviewNextLine));
        let below = if press == 0 { "$ line 1" } else { "$ x" };
        assert_eq!(session.heard(), plain(&[below]), "press {press}");
        session.input(&command(ReviewCommand::ReviewPreviousLine));
        let _ = session.heard();
        let _ = session.requests();
    }
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn a_marks_line_printed_again_nearer_the_review_position_is_passed_over() {
    // A program prints the command's line again below it, unmarked: Up goes
    // to the mark's own line, whose output follows it, not to the copy.
    let mut session = showing(30, 4, 100, "$ ");
    let at_ms = enter(&mut session, "echo hi", "hi\r\n", 1000);
    session.write(&format!("$ echo hi\r\ncopy\r\n{}", numbered(1..=20)), at_ms);
    let _ = session.heard();
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo hi"]));
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["hi"]));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    let _ = session.heard();
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn output_arriving_while_a_search_is_out_moves_to_the_mark_with_one_search() {
    // The program keeps printing: before the outpost searches, more output
    // has scrolled the rows and been read, so the answer is counted from
    // that newer read. Each press still lands on its mark's own line, with
    // one search.
    let mut session = showing(30, 4, 100, "$ ");
    let mut at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    at_ms = enter(&mut session, "echo two", "two\r\n", at_ms);
    session.write(&numbered(1..=20), at_ms);
    let _ = session.heard();
    let _ = session.requests();
    session.output_before_finds = [
        (numbered(21..=25), at_ms + 1000),
        (numbered(26..=30), at_ms + 2000),
    ]
    .into();
    // The output read before the search is spoken as any output is.
    let spoken = |range: std::ops::RangeInclusive<u32>, mark: &str| {
        let mut spoken: Vec<String> = range.map(|line| format!("line {line}")).collect();
        spoken.push(mark.to_owned());
        spoken
    };
    session.input(&command(UP));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, spoken(21..=25, "$ echo two"));
    let searched = |requests: &[TextOp]| {
        requests
            .iter()
            .filter(|op| matches!(op, TextOp::TerminalFind { .. }))
            .count()
    };
    assert_eq!(searched(&session.requests()), 1);
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["two"]));
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(UP));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, spoken(26..=30, "$ echo one"));
    assert_eq!(searched(&session.requests()), 1);
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["one"]));
}

#[test]
fn a_press_makes_at_most_three_searches() {
    // Four marks above the memory, the newest three's lines gone: the first
    // press drops those three with a search each and says nothing; the
    // next finds the oldest with one more.
    let mut session = showing(30, 4, 100, "$ ");
    let mut at_ms = 1000;
    for name in ["a", "b", "c", "d"] {
        at_ms = enter(&mut session, &format!("echo {name}"), "\r\n", at_ms);
    }
    session.write(&numbered(1..=20), at_ms);
    session.write(
        &format!("\x1b[2J\x1b[3J\x1b[H$ echo a\r\n\r\n{}$ ", numbered(1..=20)),
        at_ms + 1000,
    );
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    let needles: Vec<String> = session
        .requests()
        .into_iter()
        .filter_map(|op| match op {
            TextOp::TerminalFind { needle, .. } => Some(needle),
            _ => None,
        })
        .collect();
    assert_eq!(needles, ["$ echo d", "$ echo c", "$ echo b"]);
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo a"]));
}

#[test]
fn a_line_move_after_a_mark_move_while_output_scrolls_reads_its_own_line() {
    // The review position on "$ line 1", above the rows the memory keeps,
    // and six lines printed that no read has shown yet. Three commands are
    // pressed faster than the outpost answers: a line move down, which reads
    // the history after its fresh read; a move to the previous command mark,
    // whose search goes out while two more lines are read; and a line move
    // up, which reads from where the review position was when it was
    // pressed. The read taken while the search was out counts its rows from
    // a start six rows lower, so the history the first command read is not
    // among its rows: the last command reads the history again and says the
    // line above "$ line 1". Each command's speech cuts off the one before,
    // so only the last is heard after the output.
    let mut session = showing(30, 4, 100, "intro\r\n$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), at_ms);
    let _ = session.heard();
    session.input(&command(UP));
    session.input(&command(ReviewCommand::ReviewNextLine));
    session.input(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["$ echo one", "one", "$ line 1"]));
    let _ = session.requests();
    session.outpost.sim.write(numbered(21..=26).as_bytes());
    session.output_before_finds = [(numbered(27..=28), at_ms + 1000)].into();
    session.inputs(&[
        command(ReviewCommand::ReviewNextLine),
        command(UP),
        command(ReviewCommand::ReviewPreviousLine),
    ]);
    assert_eq!(
        session.heard(),
        plain(&[
            "line 21", "line 22", "line 23", "line 24", "line 25", "line 26", "line 27", "line 28",
            "one"
        ])
    );
    let history = |above| TextOp::TerminalHistory {
        above,
        whole: false,
    };
    assert_eq!(
        session.requests(),
        [FRESH, history(31), find("$ echo one", 25), history(33)]
    );
}

#[test]
fn a_search_that_never_ran_drops_no_mark() {
    let mut session = showing(30, 4, 100, "$ ");
    enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), 5000);
    let _ = session.heard();
    let _ = session.requests();
    // The outpost answers the search unanswered, as when the focus moved:
    // the move says nothing, and the mark is kept for the next.
    session.unanswered_finds = true;
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    session.unanswered_finds = false;
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
}

#[test]
fn a_mark_whose_line_is_gone_is_dropped_and_the_next_one_tried() {
    // The program erases the screen and its history and prints the history
    // again without the second command, as Pi's Control+O prints its
    // transcript again: the second mark's line is nowhere, and the first's
    // is found.
    let mut session = showing(30, 4, 100, "$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    enter(&mut session, "echo two", "two\r\n", at_ms);
    session.write(&numbered(1..=20), 6000);
    session.write(
        &format!(
            "\x1b[2J\x1b[3J\x1b[H$ echo one\r\none\r\n{}$ ",
            numbered(1..=20)
        ),
        7000,
    );
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    let requests = session.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(matches!(&requests[1], TextOp::TerminalFind { needle, .. } if needle == "$ echo two"));
    assert!(matches!(&requests[2], TextOp::TerminalFind { needle, .. } if needle == "$ echo one"));
    // The dropped mark is not looked for again.
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn a_mark_above_windows_terminals_clear_is_dropped() {
    // Windows Terminal's clear, followed by more than a screen of output
    // before the next read: the read's start is refused, and the whole
    // document read after it starts at the document's start, which the
    // mark's place lies above, so the mark is dropped, and the command
    // says nothing and needs no search.
    let mut outpost = Outpost::new(Flavor::WindowsTerminal, 30, 4, 100);
    outpost.sim.write(b"$ ");
    let mut session = Session::with(outpost);
    enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&format!("\x1b[H\x1b[2J\x1b[3J{}$ ", numbered(1..=10)), 5000);
    let _ = session.heard();
    let _ = session.requests();
    session.input(&command(UP));
    assert_eq!(session.heard(), []);
    assert_eq!(session.requests(), [FRESH]);
}

// The flight recorder.

#[test]
fn a_terminal_session_survives_a_flight_recorders_dump_and_replay() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\n");
    session.write("three\r\n", 100);
    let base: SrState = session.state.clone();
    session.log.clear();
    let (_, input) = typed("l", 1000);
    session.input(&input);
    session.write("l", 1050);
    session.input(&command(ReviewCommand::ReviewPreviousLine));
    session.write("s\r\nfour\r\n", 3000);
    let (inputs, effects): (Vec<Input>, Vec<Vec<Effect>>) = session.log.drain(..).unzip();
    let recorded: Vec<RecordedInput> = inputs
        .iter()
        .zip(&effects)
        .map(|(input, effects)| RecordedInput {
            input: input.clone(),
            effect_count: effects.len(),
        })
        .collect();
    let mut file = Vec::new();
    dump::write_dump(&mut file, "test", "now", &base, &recorded).expect("a dump is written");
    let contents = dump::read_dump(&mut BufReader::new(file.as_slice())).expect("a dump is read");
    assert_eq!(contents.base, base);
    let inputs: Vec<Input> = contents
        .inputs
        .into_iter()
        .map(|recorded| recorded.input)
        .collect();
    assert_eq!(replay(&contents.base, &inputs), effects);
}

#[test]
fn the_read_before_a_held_key_is_aligned_and_its_caret_answers_it() {
    let mut session = showing(20, 4, 10, "$ ls");
    session.input(&Input::HeldKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        pressed_at_ms: 200,
    });
    assert_eq!(session.requests(), [TextOp::ReadCaret]);
    // Output the program wrote before the read: the read is aligned as any
    // read, and its output spoken.
    session.outpost.sim.write(b"\r\nfile\r\n$ ");
    let line = verbatim_model::TextChunk {
        unit: verbatim_model::TextUnit::Line,
        text: "$ ".to_owned(),
        start: verbatim_model::TextAnchor(7),
        offset: 2,
        languages: Vec::new(),
        first: false,
        last: true,
        truncated: false,
        formats: Vec::new(),
    };
    session.answer_caret_read(
        300,
        verbatim_model::CaretReport {
            line,
            selection: None,
            selection_line: None,
        },
    );
    assert_eq!(session.heard(), plain(&["file", "$"]));
    // The key is passed on as pressed just after the read began, its
    // colours asked for from then and its watch opened from then.
    let requests = session.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(
        requests[0],
        TextOp::TerminalColours {
            from_ms: 301,
            until_ms: 601
        }
    );
    assert!(
        matches!(&requests[1], TextOp::AwaitCaret(watch) if watch.pressed_at_ms == 301),
        "{requests:?}"
    );
}

#[test]
fn a_key_deferred_behind_a_held_key_reaches_the_rules_at_the_held_keys_pass() {
    // "x" typed at 210 while Up, pressed at 200, waits for the read before
    // it; the read began at 300, so Up is passed on at 301, and "x", sent
    // with it, reached the program then too.
    let mut session = showing(20, 4, 10, "$ ");
    session.input(&Input::HeldKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        pressed_at_ms: 200,
    });
    let (_, x) = typed("x", 210);
    session.input(&x);
    assert_eq!(session.requests(), [TextOp::ReadCaret]);
    let line = verbatim_model::TextChunk {
        unit: verbatim_model::TextUnit::Line,
        text: "$ ".to_owned(),
        start: verbatim_model::TextAnchor(7),
        offset: 2,
        languages: Vec::new(),
        first: false,
        last: true,
        truncated: false,
        formats: Vec::new(),
    };
    session.answer_caret_read(
        300,
        verbatim_model::CaretReport {
            line,
            selection: None,
            selection_line: None,
        },
    );
    let requests = session.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(
        requests[0],
        TextOp::TerminalColours {
            from_ms: 301,
            until_ms: 601
        }
    );
    assert!(
        matches!(&requests[1], TextOp::AwaitCaret(watch) if watch.pressed_at_ms == 301),
        "{requests:?}"
    );
    assert_eq!(
        requests[2],
        TextOp::TerminalColours {
            from_ms: 301,
            until_ms: 601
        }
    );
}

#[test]
fn a_caret_move_with_no_key_says_nothing_and_the_next_key_is_judged_from_it() {
    let mut session = showing(20, 4, 10, "$ echo one");
    // The program moves the caret back over "one" with no key: nothing is
    // said ("The caret's line").
    session.write("\x1b[3D", 100);
    assert_eq!(session.heard(), []);
    // Right Arrow then moves it one along, from where the program put it,
    // to the "n".
    let right = TraceId::mint();
    session.input(&Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: right,
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 200,
    });
    session.write("\x1b[C", 250);
    assert_eq!(session.heard(), [said(Some(right), "n")]);
}

#[test]
fn a_key_that_reached_the_program_while_a_read_was_taken_is_given_before_it() {
    // The read began at 1000, the key reached the program at 1001, and the
    // terminal answered the read after it, showing the key's echo; the
    // read ended at 1002. The echo goes out for the key.
    let mut session = showing(20, 4, 100, "$ ");
    let (key, input) = typed("l", 1001);
    session.input(&input);
    let _ = session.requests();
    session.outpost.sim.write(b"l");
    session.at_ms = 1002;
    let (_, mut snapshot) = session.outpost.read(None, 1002);
    snapshot.started_ms = 1000;
    session.input(&terminal_support::event(
        verbatim_model::NormalizedEvent::TerminalSnapshot {
            node_id: terminal_support::node(),
            snapshot: Box::new(snapshot),
        },
    ));
    assert_eq!(session.heard(), [said(Some(key), "l")]);
}

// "Colours": every read that begins in a key's window takes the colours, so
// neither a read the outpost does not send nor one it takes at once uses
// them up.

#[test]
fn a_read_not_sent_as_the_last_one_again_leaves_the_colours_to_the_echos_read() {
    // A caret event's read in the key's window, with the colours but
    // identical to the last read sent, is not sent; the echo's read after
    // it has the colours too, and the echo is spoken.
    let mut session = showing(20, 4, 100, "$ ");
    let (_, input) = typed("a", 1000);
    session.input(&input);
    session.write("a", 1010);
    let _ = session.heard();
    let (b, input) = typed("b", 2000);
    session.input(&input);
    let inputs = session.log.len();
    session.live(2001);
    assert_eq!(session.log.len(), inputs, "the read is the last one again");
    session.write("b", 2010);
    assert_eq!(session.heard(), [said(Some(b), "b")]);
}

#[test]
fn a_read_taken_at_once_before_the_program_draws_the_key_leaves_the_echo_to_the_next() {
    // The key's colours request reaches the outpost after a read without
    // colours ended in the key's window, so a read is taken at once with
    // them, before the program has drawn the key; the read that shows the
    // echo and the suggestion after it has the colours too, and tells them
    // apart.
    let mut session = showing(20, 3, 10, "$ e");
    session.hold_colours = true;
    let (c, input) = typed("c", 1000);
    session.input(&input);
    session.live(1001);
    session.deliver_colours(1002);
    assert_eq!(session.heard(), []);
    session.write("c\x1b[90mho hi\x1b[0m\x1b[5D", 1010);
    assert_eq!(
        session.heard(),
        [said(Some(c), "c"), said(Some(c), "ho hi")]
    );
}
