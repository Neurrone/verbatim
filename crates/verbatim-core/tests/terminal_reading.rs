//! Core's terminal model (`books/dev/src/explanation/design/terminal-model.md`, "Where it
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
//! at once; with "speak passwords" on, a typed character is heard as typed
//! and its asterisk as shown; lines of one or two characters after Enter
//! are output, not echo; and "Report status bar" reads the footer after a fresh read,
//! spelled and copied on later presses, the review cursor left alone.

#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;
use std::io::BufReader;

use terminal_support::{Asked, Key, Outpost, Said, Session, caret, said};
use verbatim_core::{RecordedInput, SrState, dump, read_of, replay, snapshot_of};
use verbatim_model::request::{PressAction, QueueInput};
use verbatim_model::{
    CaretKey, CaretMotion, Earcon, Effect, Input, KeySeq, Message, ReaderSettings, ReviewCommand,
    SegmentContent, SpeechMark, TraceId, TypingEcho,
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
fn command(command: ReviewCommand) -> Key {
    pressed(command, 0)
}

fn pressed(command: ReviewCommand, repeat: u8) -> Key {
    Key {
        trace_id: TraceId::from_raw(1),
        at_ms: 0,
        action: PressAction::Command { command, repeat },
    }
}

/// A read of the screen and the rows above it.
const FRESH: Asked = Asked::Read;

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
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["three"]));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["two"]));
    assert_eq!(session.requests(), [FRESH, FRESH]);
    // Output scrolls "two" up the screen and into the history above the
    // rows the memory keeps.
    session.write("four\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n", 200);
    assert_eq!(
        session.heard(),
        plain(&["four", "five", "six", "seven", "eight", "nine"])
    );
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["two"]));
    // Down from it, through the history, read again for each command.
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["three"]));
    // Each command reads the screen, then the history above Core's rows.
    assert_eq!(
        session.requests(),
        [FRESH, Asked::History, FRESH, Asked::History]
    );
}

/// A terminal of four rows whose review position is on "two", in the
/// history above the rows the memory keeps, with nine lines printed below
/// it and seven above it, so that no history read below reaches the
/// document's start.
fn reviewing_two() -> Session {
    let text = format!("{}one\r\ntwo\r\nthree\r\n", numbered(1..=6));
    let mut session = showing(20, 4, 100, &text);
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
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
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["ten", "eleven", "two"]));
    assert_eq!(session.requests(), [FRESH, Asked::History]);
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["three"]));
}

#[test]
fn more_than_a_screen_arriving_while_a_history_read_is_out_is_placed_from_the_anchor() {
    // Five more lines, more than the screen more the command asked for,
    // read while the history read is out: the history is counted from the
    // start the command named, and placed from the anchor Core put at that
    // start's row, which the read moved with its row
    // (`docs/design/readers-tell-core-asks-build3.md`, Table 5), so the
    // command reads "two" after the output read before it; the next
    // command reads it again.
    let mut session = reviewing_two();
    session.output_before_histories = [(
        "ten\r\neleven\r\ntwelve\r\nthirteen\r\nfourteen\r\n".to_owned(),
        300,
    )]
    .into();
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    // The output's first two lines are handed to speech ahead of the
    // command's words, the rest as playback reaches them.
    assert_eq!(
        session.heard(),
        plain(&["ten", "eleven", "two", "twelve", "thirteen", "fourteen"])
    );
    assert_eq!(session.requests(), [FRESH, Asked::History]);
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["two"]));
}

#[test]
fn a_history_answer_after_a_flood_is_placed_from_the_anchor() {
    // The review position on "line 4", in the history above the memory's
    // rows; before the outpost reads the history, a flood carries the last
    // read's rows out of the buffer. The read that shows it is lost, and
    // Core owes the whole document, which comes after the history; the
    // history is counted from the start the command named and placed from
    // the anchor at its row (`docs/design/readers-tell-core-asks-build3.md`,
    // Table 5): the review position's line is gone, so it keeps its place
    // and reads the row there now, "line 32" ("Limits"), and the next
    // command reads it again.
    let mut session = showing(20, 3, 9, &numbered(1..=12));
    session.press(&command(ReviewCommand::ReviewTop));
    session.press(&command(ReviewCommand::ReviewNextLine));
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 2", "line 3", "line 4"]));
    let _ = session.requests();
    session.output_before_histories = [(numbered(13..=40), 100)].into();
    session.hold_whole = true;
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    session.answer_held();
    let mut flood = vec![
        "line 32".to_owned(),
        "Phrase(SkippedUncountedLines)".to_owned(),
    ];
    flood.extend((30..=40).map(|line| format!("line {line}")));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, flood);
    assert_eq!(session.requests(), [FRESH, Asked::History, Asked::Whole]);
    session.press(&command(ReviewCommand::ReviewCurrentLine));
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
    session.press(&command(ReviewCommand::ReviewTop));
    session.press(&command(ReviewCommand::ReviewNextLine));
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 2", "line 3", "line 4"]));
    session.write(&numbered(13..=24), 100);
    let _ = session.heard();
    let _ = session.requests();
    session.press(&command(ReviewCommand::ReviewCurrentLine));
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
    session.press(&command(ReviewCommand::ReviewTop));
    session.press(&command(ReviewCommand::ReviewNextLine));
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["line 1", "line 2", "line 3"]));
    session.write(
        &format!("\x1b[2J\x1b[3J\x1b[Hnew a\r\nnew b\r\n{lines}"),
        100,
    );
    let _ = session.heard();
    let _ = session.requests();
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    assert_eq!(session.heard(), plain(&["line 3"]));
}

#[test]
fn moving_above_the_memorys_rows_reads_the_history_up_to_its_start() {
    let mut session = showing(20, 3, 100, &numbered(1..=12));
    session.press(&command(ReviewCommand::ReviewTop));
    assert_eq!(session.heard(), plain(&["line 1"]));
    assert_eq!(session.requests(), [FRESH, Asked::History]);
    // Up from the document's first row goes nowhere.
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["Message(Top) line 1"]));
    for line in ["line 2", "line 3", "line 4"] {
        session.press(&command(ReviewCommand::ReviewNextLine));
        assert_eq!(session.heard(), plain(&[line]));
    }
}

#[test]
fn a_row_ends_at_its_text_and_a_range_joins_lines() {
    let mut session = showing(20, 4, 100, "ab\r\ncd\r\n");
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    // No padding: the row's text is all of it.
    assert_eq!(session.heard(), plain(&["cd", "ab"]));
    session.press(&command(ReviewCommand::ReviewNextCharacter));
    session.press(&command(ReviewCommand::SetStartMarker));
    session.press(&command(ReviewCommand::ReviewNextLine));
    session.press(&pressed(ReviewCommand::SelectThenCopy, 0));
    assert_eq!(
        session.heard(),
        plain(&["b", "Message(StartMarked)", "cd", "Message(CannotSelect)"])
    );
    session.press(&pressed(ReviewCommand::SelectThenCopy, 1));
    assert_eq!(session.copied, ["b\r\ncd"]);
}

#[test]
fn the_review_reads_where_the_caret_is() {
    let mut session = showing(20, 4, 100, "ready> abc");
    session.press(&command(ReviewCommand::ReviewCurrentLine));
    session.press(&command(ReviewCommand::ReviewPreviousCharacter));
    assert_eq!(session.heard(), plain(&["ready> abc", "c"]));
}

/// Review words in a terminal Core reads are Core's own words over its
/// rows (the request list's design, section 6, the local answerers), so
/// word review keeps the words it had: previous word from a row's first
/// word lands on the last word of the row above, next word from a row's
/// last word on the first word of the row below, and previous word on
/// the text's first word says "Top" and that word, and stays. Each press
/// answers from memory after its fresh read.
#[test]
fn terminal_review_words_are_cores_own_words_over_its_rows() {
    let mut session = showing(20, 4, 100, "zero\r\none two\r\nthree\r\n");
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["three"]));
    for (word, said) in [
        (ReviewCommand::ReviewPreviousWord, "two"),
        (ReviewCommand::ReviewPreviousWord, "one"),
        (ReviewCommand::ReviewPreviousWord, "zero"),
        (ReviewCommand::ReviewNextWord, "one"),
        (ReviewCommand::ReviewNextWord, "two"),
        (ReviewCommand::ReviewNextWord, "three"),
        (ReviewCommand::ReviewCurrentWord, "three"),
    ] {
        session.press(&command(word));
        assert_eq!(session.heard(), plain(&[said]), "{word:?}");
    }
    session.press(&command(ReviewCommand::ReviewTop));
    let _ = session.heard();
    session.press(&command(ReviewCommand::ReviewPreviousWord));
    assert_eq!(session.heard(), plain(&["Message(Top) zero"]));
    assert!(session.requests().iter().all(|asked| *asked == FRESH));
}

// Keys against reads.

fn typed(text: &str, at_ms: u64) -> (TraceId, Key) {
    let trace_id = TraceId::from_raw(2);
    (
        trace_id,
        Key {
            trace_id,
            at_ms,
            action: PressAction::Typed {
                text: text.to_owned(),
            },
        },
    )
}

#[test]
fn a_key_whose_time_is_after_a_reads_start_is_given_to_the_rules_after_that_read() {
    let mut session = showing(20, 4, 100, "$ ");
    let (key, input) = typed("l", 1000);
    session.press(&input);
    // The colours travel in the reads themselves: nothing else is asked.
    assert_eq!(session.requests(), Vec::<Asked>::new());
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
    let up = TraceId::from_raw(3);
    let held = caret(
        up,
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        200,
        true,
    );
    let (x, input) = typed("x", 210);
    // Up's starting read is answered at 205: Up is passed on then, and
    // "x" after it.
    session.at_ms = 205;
    session.presses(&[held, input]);
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
    let held = caret(
        TraceId::from_raw(4),
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        200,
        true,
    );
    let (x, input) = typed("x", 210);
    session.at_ms = 205;
    session.presses(&[held, input]);
    session.write("echo one", 220);
    assert_eq!(session.heard(), [said(Some(x), "echo one")]);
    session.write("x", 250);
    assert_eq!(session.heard(), [said(Some(x), "x")]);
}

#[test]
fn a_read_after_every_window_carries_no_key() {
    let mut session = showing(20, 4, 100, "$ ");
    let (_, input) = typed("l", 1000);
    session.press(&input);
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
    session.input(&Input::Settings(ReaderSettings {
        speak_terminal_passwords: true,
        ..ReaderSettings::default()
    }));
    let (key, input) = typed("s", 1000);
    session.press(&input);
    // Echoed by the key's own entry, whose speech carries its number.
    assert_eq!(session.heard(), [said(None, "s")]);
    session.write("*", 1050);
    assert_eq!(session.heard(), [said(Some(key), "*")]);
    // The colours travel in the reads themselves: nothing else is asked.
    assert_eq!(session.requests(), Vec::<Asked>::new());
}

#[test]
fn lines_of_one_and_two_characters_after_enter_are_output_not_echo() {
    // Enter, then a program printing "y" and "ok": short as they are, both
    // are output, never taken for the echo of typing.
    let mut session = showing(30, 6, 10, "ready> ");
    let (key, input) = typed("\r", 1000);
    session.press(&input);
    session.write("\r\ny\r\nok\r\nready> ", 1050);
    assert_eq!(
        session.heard(),
        [
            said(Some(key), "y"),
            said(Some(key), "ok"),
            said(None, "ready>")
        ]
    );
    // The colours travel in the reads themselves: nothing else is asked.
    assert_eq!(session.requests(), Vec::<Asked>::new());
}

#[test]
fn live_snapshots_that_arrive_while_a_whole_read_is_owed_are_dropped() {
    let mut session = showing(20, 3, 9, &numbered(1..=3));
    session.hold_whole = true;
    // A flood that carries the last read's rows out of the buffer: the read
    // keeps nothing, and the rules ask for the whole document.
    session.write(&numbered(4..=40), 100);
    assert_eq!(session.heard(), []);
    assert_eq!(session.requests(), [Asked::Whole]);
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
    let up = TraceId::from_raw(5);
    session.press(&caret(
        up,
        CaretKey {
            motion: CaretMotion::NextLine,
            select: false,
        },
        1000,
        false,
    ));
    session.write("\x1b[4;1H2,1 Top\x1b[2;1H", 1050);
    assert_eq!(session.heard(), [said(Some(up), "line two")]);
    assert_eq!(session.timers, [Some(2000)]);
    session.press(&Key {
        trace_id: TraceId::from_raw(6),
        at_ms: 1900,
        action: PressAction::Clearing,
    });
    session.timers.clear();
    session.input(&Input::Queue {
        trace_id: TraceId::from_raw(7),
        at_us: 0,
        input: verbatim_model::request::QueueInput::TerminalDue { at_ms: 2000 },
    });
    assert_eq!(session.heard(), []);
    assert_eq!(session.timers, [Some(3250)]);
}

/// The session of the timer's test, its timer armed for 2000 by a key
/// whose answer a read showed.
fn timed() -> Session {
    let mut session = showing(20, 5, 10, "line one\r\nline two\r\n\r\n1,1 Top\x1b[1;1H");
    session.press(&caret(
        TraceId::from_raw(5),
        CaretKey {
            motion: CaretMotion::NextLine,
            select: false,
        },
        1000,
        false,
    ));
    session.write("\x1b[4;1H2,1 Top\x1b[2;1H", 1050);
    assert_eq!(session.armed(), Some(2000));
    session
}

#[test]
fn the_timer_goes_with_the_model_when_the_outpost_ends() {
    let mut session = timed();
    session.queue(verbatim_model::request::QueueInput::ReaderEnded(
        terminal_support::OUTPOST,
    ));
    assert_eq!(session.armed(), None);
}

#[test]
fn the_timer_goes_with_the_model_when_the_terminals_text_is_forgotten() {
    // The focus read again says the runtime id the terminal's node held
    // now names another element while the focus stays: its model goes,
    // and the next read makes a new one, whose rules asked for no timer.
    let mut session = timed();
    session.focus_after(true);
    assert_eq!(session.armed(), None);
    session.write("more\r\n", 1100);
    assert_eq!(session.armed(), None);
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
    session.press(&command(STATUS_BAR));
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
    session.press(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["progress 2 status 2"]));
}

#[test]
fn the_status_bar_is_the_last_line_with_text_with_no_footer() {
    let mut session = showing(20, 4, 100, "one\r\ntwo\r\nready>");
    session.press(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["ready>"]));
}

#[test]
fn a_blank_screen_has_no_status_bar_information() {
    let mut session = showing(20, 4, 100, "");
    session.press(&command(STATUS_BAR));
    assert_eq!(session.heard(), plain(&["Message(NoStatusBarInformation)"]));
}

#[test]
fn the_status_bar_is_spelled_on_the_second_press_and_copied_on_the_third() {
    let mut session = showing(20, 4, 100, "one\r\nab\r\ncd\x1b[1;4H");
    // The lines spelled as one text, the space between them spelled.
    session.press(&pressed(STATUS_BAR, 1));
    assert_eq!(session.heard(), plain(&["a b Message(Space) c d"]));
    session.press(&pressed(STATUS_BAR, 2));
    assert_eq!(session.heard(), []);
    assert_eq!(session.copied, ["ab\r\ncd"]);
}

#[test]
fn the_status_bar_leaves_the_review_cursor_where_it_was() {
    let mut session = showing(20, 5, 100, "one\r\ntwo\r\nready>\r\nfooter");
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), plain(&["ready>", "two"]));
    session.press(&command(STATUS_BAR));
    session.press(&command(ReviewCommand::ReviewCurrentLine));
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
    session.press(&command(STATUS_BAR));
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
        let (_, input) = typed(&character.to_string(), at_ms);
        session.press(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.press(&input);
    session.write(&format!("\r\n{output}$ "), at_ms + 5);
    let _ = session.heard();
    let _ = session.requests();
    at_ms + 2000
}

/// The search for a mark's line Core asks for.
fn find(needle: &str) -> Asked {
    Asked::Search(needle.to_owned())
}

#[test]
fn enter_on_a_typed_line_marks_it_and_a_bare_enter_does_not() {
    let mut session = showing(30, 6, 100, "$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    // A bare Enter: a new prompt, no mark.
    let (_, input) = typed("\r", at_ms);
    session.press(&input);
    session.write("\r\n$ ", at_ms + 5);
    let _ = session.heard();
    let _ = session.requests();
    session.press(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    // Past the oldest mark nothing is said.
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
    assert_eq!(session.requests(), [FRESH, FRESH]);
}

#[test]
fn a_line_recalled_with_up_arrow_and_entered_is_marked() {
    let mut session = showing(30, 8, 100, "$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    session.press(&caret(
        TraceId::from_raw(8),
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        at_ms,
        false,
    ));
    session.write("echo one", at_ms + 5);
    let (_, input) = typed("\r", at_ms + 1000);
    session.press(&input);
    session.write("\r\none\r\n$ ", at_ms + 1005);
    let _ = session.heard();
    for _ in 0..2 {
        session.press(&command(UP));
        assert_eq!(session.heard(), plain(&["$ echo one"]));
    }
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn up_and_down_move_between_the_marks_and_say_nothing_past_them() {
    let mut session = showing(30, 10, 100, "$ ");
    let mut at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    at_ms = enter(&mut session, "echo two", "two\r\n", at_ms);
    enter(&mut session, "echo three", "three\r\n", at_ms);
    for line in ["$ echo three", "$ echo two", "$ echo one"] {
        session.press(&command(UP));
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
    for line in ["$ echo two", "$ echo three"] {
        session.press(&command(DOWN));
        assert_eq!(session.heard(), plain(&[line]));
    }
    session.press(&command(DOWN));
    assert_eq!(session.heard(), []);
    // The review cursor is on the mark's line, as a line move leaves it.
    session.press(&command(ReviewCommand::ReviewPreviousLine));
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
        session.press(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.press(&input);
    session.write("\x1b[2K\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 200);
    let _ = session.heard();
    session.press(&command(UP));
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
            session.press(&input);
            session.write(&character.to_string(), at_ms + 5);
            at_ms += 10;
        }
        let (_, input) = typed("\r", at_ms);
        session.press(&input);
        session.write("\x1b[2K\r", at_ms + 100);
        session.write("\x1b[1A\x1b[Lhi there\x1b[2B\r", at_ms + 500);
        at_ms += 2000;
    }
    let _ = session.heard();
    for _ in 0..2 {
        session.press(&command(UP));
        assert_eq!(session.heard(), plain(&["hi there"]));
    }
    session.press(&command(UP));
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
        session.press(&input);
        session.write(&character.to_string(), at_ms + 5);
        at_ms += 10;
    }
    let (_, input) = typed("\r", at_ms);
    session.press(&input);
    // The editor clears at once; the input is printed only after the quiet
    // period.
    session.write("\x1b[2K\r", at_ms + 100);
    session.write("\x1b[2;1H\x1b[Lhi there\x1b[4;1H", at_ms + 1500);
    let _ = session.heard();
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
}

#[test]
fn a_mark_above_the_memory_is_found_by_one_search() {
    let mut session = showing(30, 4, 100, "$ ");
    enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), 5000);
    let _ = session.heard();
    session.press(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    // The mark lies above the rows Core keeps: one search finds it.
    assert_eq!(session.requests(), [FRESH, find("$ echo one")]);
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
        session.press(&command(UP));
        assert_eq!(session.heard(), plain(&["$ ls"]));
    }
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
    session.press(&command(DOWN));
    assert_eq!(session.heard(), plain(&["$ ls"]));
    session.press(&command(DOWN));
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
        session.press(&command(UP));
        assert_eq!(session.heard(), plain(&["$ x"]), "press {press}");
        // Each mark lies above the rows Core keeps, found by one search.
        let finds = session
            .requests()
            .iter()
            .filter(|op| matches!(op, Asked::Search(_)))
            .count();
        assert_eq!(finds, 1, "press {press}");
        // The line below is the next copy's, or the output's for the newest.
        session.press(&command(ReviewCommand::ReviewNextLine));
        let below = if press == 0 { "$ line 1" } else { "$ x" };
        assert_eq!(session.heard(), plain(&[below]), "press {press}");
        session.press(&command(ReviewCommand::ReviewPreviousLine));
        let _ = session.heard();
        let _ = session.requests();
    }
    session.press(&command(UP));
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
    session.press(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo hi"]));
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["hi"]));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    let _ = session.heard();
    session.press(&command(UP));
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
    // The output read before the search is spoken as any output is, before
    // the move's words: its first two lines handed to speech ahead of them,
    // the rest as playback reaches them.
    let spoken = |range: std::ops::RangeInclusive<u32>, mark: &str| {
        let lines: Vec<String> = range.map(|line| format!("line {line}")).collect();
        let mut spoken = lines[..2].to_vec();
        spoken.push(mark.to_owned());
        spoken.extend_from_slice(&lines[2..]);
        spoken
    };
    session.press(&command(UP));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, spoken(21..=25, "$ echo two"));
    let searched = |requests: &[Asked]| {
        requests
            .iter()
            .filter(|op| matches!(op, Asked::Search(_)))
            .count()
    };
    assert_eq!(searched(&session.requests()), 1);
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["two"]));
    session.press(&command(ReviewCommand::ReviewPreviousLine));
    let _ = session.heard();
    let _ = session.requests();
    session.press(&command(UP));
    let heard: Vec<String> = session.heard().into_iter().map(|said| said.words).collect();
    assert_eq!(heard, spoken(26..=30, "$ echo one"));
    assert_eq!(searched(&session.requests()), 1);
    session.press(&command(ReviewCommand::ReviewNextLine));
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
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
    let needles: Vec<String> = session
        .requests()
        .into_iter()
        .filter_map(|op| match op {
            Asked::Search(needle) => Some(needle),
            _ => None,
        })
        .collect();
    assert_eq!(needles, ["$ echo d", "$ echo c", "$ echo b"]);
    session.press(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo a"]));
}

#[test]
fn a_line_move_after_a_mark_move_while_output_scrolls_reads_its_own_line() {
    // The review position on "$ line 1", above the rows the memory keeps,
    // and six lines printed that no read has shown yet. Three commands are
    // pressed faster than the outpost answers: they run one after another in
    // the key line, each answered in press order with what its own read
    // finds: the line after "$ line 1", the previous command mark, and the
    // line above it, each from where the one before left the review cursor.
    let mut session = showing(30, 4, 100, "intro\r\n$ ");
    let at_ms = enter(&mut session, "echo one", "one\r\n", 1000);
    session.write(&numbered(1..=20), at_ms);
    let _ = session.heard();
    session.press(&command(UP));
    session.press(&command(ReviewCommand::ReviewNextLine));
    session.press(&command(ReviewCommand::ReviewNextLine));
    assert_eq!(session.heard(), plain(&["$ echo one", "one", "$ line 1"]));
    let _ = session.requests();
    session.outpost.sim.write(numbered(21..=26).as_bytes());
    session.output_before_finds = [(numbered(27..=28), at_ms + 1000)].into();
    session.presses(&[
        command(ReviewCommand::ReviewNextLine),
        command(UP),
        command(ReviewCommand::ReviewPreviousLine),
    ]);
    // The first command's fresh read shows lines 21 to 26, spoken before
    // its words, the first two handed to speech ahead of them and the
    // rest, with what the search's output read shows, as playback reaches
    // them.
    assert_eq!(
        session.heard(),
        plain(&[
            "line 21",
            "line 22",
            "line 2",
            "$ echo one",
            "intro",
            "line 23",
            "line 24",
            "line 25",
            "line 26",
            "line 27",
            "line 28"
        ])
    );
    assert_eq!(
        session.requests(),
        // Each press reads afresh, presses waiting together too.
        [
            FRESH,
            Asked::History,
            FRESH,
            find("$ echo one"),
            FRESH,
            Asked::History
        ]
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
    session.press(&command(UP));
    assert_eq!(session.heard(), []);
    session.unanswered_finds = false;
    session.press(&command(UP));
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
    session.press(&command(UP));
    assert_eq!(session.heard(), plain(&["$ echo one"]));
    let requests = session.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(matches!(&requests[1], Asked::Search(needle) if needle == "$ echo two"));
    assert!(matches!(&requests[2], Asked::Search(needle) if needle == "$ echo one"));
    // The dropped mark is not looked for again.
    session.press(&command(UP));
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
    session.press(&command(UP));
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
    session.press(&input);
    session.write("l", 1050);
    session.press(&command(ReviewCommand::ReviewPreviousLine));
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
    dump::write_dump(
        &mut file,
        "test",
        dump::UNKNOWN_COMMIT,
        false,
        "now",
        &base,
        &recorded,
    )
    .expect("a dump is written");
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
fn a_held_keys_starting_read_is_aligned_and_its_output_spoken() {
    // Output the program wrote before Up's starting read: the read is
    // aligned as any read, and its output spoken; then the key passes, its
    // watch looks for its effect, and its final read is taken.
    let mut session = showing(20, 4, 10, "$ ls");
    session.outpost.sim.write(b"\r\nfile\r\n$ ");
    session.at_ms = 300;
    session.press(&caret(
        TraceId::from_raw(9),
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        200,
        true,
    ));
    assert_eq!(session.heard(), plain(&["file", "$"]));
    assert_eq!(session.requests(), [Asked::Read, Asked::Watch, Asked::Read]);
}

/// How a terminal key's wait polls: from its starting point, its pass, its
/// poll, its bound, whether events count, and its effect test.
type Polled = (
    verbatim_model::TextAnchor,
    u64,
    u32,
    u32,
    bool,
    verbatim_model::batch::EffectTest,
);

/// Up held in `session` at 200 ms, its starting read answered at 300 ms:
/// how the wait Core sent polls.
fn held_up_watch(session: &mut Session) -> Vec<Polled> {
    session.at_ms = 300;
    session.press(&caret(
        TraceId::from_raw(9),
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        200,
        true,
    ));
    assert_eq!(session.requests(), [Asked::Read, Asked::Watch, Asked::Read]);
    std::mem::take(&mut session.watches)
        .into_iter()
        .map(|wait| {
            (
                wait.from,
                wait.passed_at_us,
                wait.poll_us,
                wait.bound_us,
                wait.events,
                wait.effect,
            )
        })
        .collect()
}

/// Up's effect test: neither the unit at the caret nor the line counts.
const MOVES: verbatim_model::batch::EffectTest = verbatim_model::batch::EffectTest {
    unit: false,
    line: false,
};

/// A terminal caret key's watch in the console host
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Terminals"): from the starting point its
/// starting read minted (anchor 3, after
/// the baseline's start, 1, and the starting read's own, 2), from its pass
/// at 300 ms, a check every 5 ms (Verbatim-only, ruled 2026-10-26) to
/// NVDA's 100 ms bound, its multiplier 1 on consoles (`winConsoleUIA.py`
/// 470), no event taken as its effect, and no unit read at its end, the
/// terminal's full read following. Converted from the console poll's
/// timing (`TerminalReads::Poll`), which goes into the watch.
#[test]
fn a_console_hosts_caret_key_is_watched_every_5_ms_to_100_ms_with_no_event() {
    let mut session = Session::in_console(Outpost::new(Flavor::ConsoleHost, 20, 4, 10));
    assert_eq!(
        held_up_watch(&mut session),
        [(
            verbatim_model::TextAnchor(3),
            300_000,
            5_000,
            100_000,
            false,
            MOVES
        )]
    );
}

/// A terminal caret key's watch outside the console host: a check every
/// 10 ms, as NVDA's poll, to NVDA's bound multiplied by its terminal's 3
/// (`NVDAObjects/UIA/winConsoleUIA.py` 470, 486), 300 ms, with no event
/// taken as its effect. Converted from the read at the pass in Windows
/// Terminal (`CoreToOutpost::ReadTerminal`), which goes into the watch.
#[test]
fn a_windows_terminal_caret_key_is_watched_every_10_ms_to_300_ms_with_no_event() {
    let mut session = Session::new(Flavor::WindowsTerminal, 20, 4, 10);
    assert_eq!(
        held_up_watch(&mut session),
        [(
            verbatim_model::TextAnchor(3),
            300_000,
            10_000,
            300_000,
            false,
            MOVES
        )]
    );
}

/// Every read of the terminal Core takes is the control plane's terminal
/// read, as the outpost reported each read it sent before Core asked its
/// reads itself, which the harness waits on for the caret's line
/// (`terminal.rs`, `wait_for_caret_on`): the output reads, for a change
/// told and for the key's pass, under the trace of their answers, and a
/// held key's starting and final reads, under the key's, in the order they
/// answered.
#[test]
fn every_terminal_read_core_takes_is_the_control_planes_terminal_read() {
    let mut session = showing(20, 4, 10, "$ ls");
    session.write(
        "
file
$ ", 100,
    );
    let _ = held_up_watch(&mut session);
    let expected: Vec<verbatim_model::Observed> = session
        .answered_reads
        .iter()
        .zip([2, 9, 2, 9])
        .map(|(snapshot, trace)| verbatim_model::Observed {
            trace_id: TraceId::from_raw(trace),
            source: verbatim_model::Pid(1),
            backend: verbatim_model::Backend::Uia,
            window: None,
            event: verbatim_model::NormalizedEvent::TerminalSnapshot {
                node_id: terminal_support::node(),
                snapshot: Box::new(snapshot.clone()),
            },
        })
        .collect();
    assert_eq!(
        session.answered_reads.len(),
        4,
        "two output reads and the key's two reads"
    );
    assert_eq!(session.observed, expected);
}

#[test]
fn a_key_deferred_behind_a_held_key_reaches_the_rules_at_the_held_keys_pass() {
    // "x" typed at 210 while Up, pressed at 200, waits for its starting
    // read, answered at 300: Up is passed on at 300, and "x", sent with
    // it, reached the program then too, the newer of the two keys, so the
    // read that shows them is spoken for "x".
    let mut session = showing(20, 4, 10, "$ ");
    let held = caret(
        TraceId::from_raw(10),
        CaretKey {
            motion: CaretMotion::PreviousLine,
            select: false,
        },
        200,
        true,
    );
    let (x, input) = typed("x", 210);
    session.at_ms = 300;
    session.presses(&[held, input]);
    assert_eq!(session.requests(), [Asked::Read]);
    session.write("echo onex", 320);
    assert_eq!(session.heard(), [said(Some(x), "echo onex")]);
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
    let right = TraceId::from_raw(11);
    session.press(&caret(
        right,
        CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        200,
        false,
    ));
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
    session.press(&input);
    let _ = session.requests();
    session.outpost.sim.write(b"l");
    session.next_read_began = Some(1000);
    session.live(1002);
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
    session.press(&input);
    session.write("a", 1010);
    let _ = session.heard();
    let (b, input) = typed("b", 2000);
    session.press(&input);
    session.live(2001);
    assert_eq!(session.heard(), [], "the read is the last one again");
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
    session.hold_output = true;
    session.live(999);
    let (c, input) = typed("c", 1000);
    session.press(&input);
    session.hold_output = false;
    session.release_output(1001);
    assert_eq!(session.heard(), []);
    session.write("c\x1b[90mho hi\x1b[0m\x1b[5D", 1010);
    assert_eq!(
        session.heard(),
        [said(Some(c), "c"), said(Some(c), "ho hi")]
    );
}

/// A terminal whose window hangs while output is written
/// (`docs/design/readers-tell-core-asks-build3.md`, "Exact expected speech",
/// sequence 14, and Table 2, columns E and K): the change told during the
/// hang has its read refused unsent, and the not-responding sound plays,
/// the terminal holding attention; a change told while the reading is
/// stalled asks nothing and sounds nothing; once the window answers Core's
/// recovery check, `Follow` is sent again and the output written meanwhile
/// is read and spoken as one read.
#[test]
fn a_hung_terminal_sounds_then_speaks_its_output_once_the_window_answers() {
    let mut session = Session::new(Flavor::ConsoleHost, 20, 4, 100);
    session.hung = true;
    session.write("alpha\r\n", 100);
    assert_eq!(session.heard(), []);
    assert_eq!(session.earcons(), [Earcon::AppNotResponding]);
    session.write("beta\r\n", 200);
    assert_eq!(session.heard(), []);
    assert_eq!(session.earcons(), []);
    assert_eq!(session.output_reads, 1, "the refused read alone");
    session.hung = false;
    session.due();
    assert_eq!(session.heard(), plain(&["alpha", "beta"]));
    assert_eq!(session.earcons(), []);
    assert_eq!(session.output_reads, 2, "one read after the hang");
}

/// The words and key number of each utterance `session`'s steps from the
/// `from`th on handed to speech.
fn handed_on(session: &Session, from: usize) -> Vec<(String, Option<KeySeq>)> {
    let effects = session.log[from..].iter().flat_map(|(_, effects)| effects);
    effects
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, key) => {
                Some((terminal_support::words(&utterance.segments), *key))
            }
            _ => None,
        })
        .collect()
}

/// Output handed to speech outside its read's entry keeps that read's key
/// number (the build 3 design's review, S4 and R2-1): a typed key's output,
/// handed on when the utterance before it fails, carries the key's
/// number, so a later key's cut fences it off; output the quiet-period
/// timer reads carries none, so no earlier key's cut fences it off.
#[test]
fn output_handed_on_outside_its_entry_carries_its_reads_key_number() {
    let mut session = showing(20, 8, 100, "$ ");
    session.manual_play = true;
    let (_, input) = typed("l", 1000);
    session.press(&input);
    session.write("l\r\none\r\ntwo\r\nthree\r\n", 1050);
    let from = session.log.len();
    let marks = vec![SpeechMark(0), SpeechMark(1)];
    session.queue(QueueInput::UtteranceFailed { marks });
    let two = ("two".to_owned(), Some(KeySeq::new(1)));
    assert_eq!(handed_on(&session, from), [two]);
    let mut session = timed();
    let from = session.log.len();
    session.queue(QueueInput::TerminalDue { at_ms: 2000 });
    assert_eq!(handed_on(&session, from), [("2,1 Top".to_owned(), None)]);
}
