//! The reading rules, one rule of `docs/design/terminal-model.md`, section
//! 5, or one case of sections 7 and 8, each: a stream written into the
//! simulated terminal, read as the outpost reads it at the times given,
//! with keys as they reach the program, and the speech, the lines withheld
//! and the timer asserted whole.

use verbatim_term_sim::{Flavor, Query, Terminal};

use super::{
    ColourRun, Counted, Key, KeyKind, Reason, Settings, Speech, Step, TerminalReader, Unit,
    Withheld,
};
use crate::terminal_rows::{Anchor, Read, hash};

/// A terminal read as the outpost reads it: each read starts where the last
/// said, as many rows above it as the memory asks, and the whole document,
/// or a read from further above, is read when the reader asks.
struct Term {
    terminal: Terminal,
    reader: TerminalReader,
    query: Query,
}

impl Term {
    /// A console host terminal of `width` by `height` with `history` rows,
    /// read once at time 0 before anything is written.
    fn new(width: usize, height: usize, history: usize) -> Self {
        Self::with(width, height, history, Settings::default())
    }

    fn with(width: usize, height: usize, history: usize, settings: Settings) -> Self {
        Self::of(Flavor::ConsoleHost, width, height, history, settings)
    }

    /// A terminal of `flavor`, read once at time 0 as `new` reads it.
    fn of(flavor: Flavor, width: usize, height: usize, history: usize, settings: Settings) -> Self {
        let mut term = Self {
            terminal: Terminal::new(flavor, width, height, history),
            reader: TerminalReader::new(settings),
            query: Query::default(),
        };
        term.query.above = u32::try_from(term.reader.memory().above()).expect("a few rows");
        let first = term.read(0);
        assert_eq!(first, quiet(None), "the first read speaks nothing");
        term
    }

    /// Writes `bytes` and reads at `at_ms`.
    fn write(&mut self, bytes: &str, at_ms: u64) -> Step {
        self.terminal.write(bytes.as_bytes());
        self.read(at_ms)
    }

    /// Writes `bytes` and reads at `at_ms` with the screen's colours, as the
    /// first read after a key does.
    fn write_coloured(&mut self, bytes: &str, at_ms: u64) -> Step {
        self.terminal.write(bytes.as_bytes());
        let colours = self.colours();
        self.take(at_ms, false, Some(&colours))
    }

    /// The screen's colours as the read program gives them.
    fn colours(&self) -> Vec<Vec<ColourRun>> {
        self.terminal
            .colours()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|run| ColourRun {
                        column: run.column,
                        length: run.length,
                        foreground: run.foreground,
                        background: run.background,
                    })
                    .collect()
            })
            .collect()
    }

    fn read(&mut self, at_ms: u64) -> Step {
        self.take(at_ms, false, None)
    }

    /// One read at `at_ms`, the whole document read again when asked.
    fn take(&mut self, at_ms: u64, waited: bool, colours: Option<&[Vec<ColourRun>]>) -> Step {
        let mut query = self.query;
        loop {
            let snapshot = self.terminal.snapshot(&query).expect("a simulated read");
            let step = self.reader.read(
                &Read {
                    text: snapshot.text,
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
                    above: usize::try_from(snapshot.above).expect("a few rows"),
                },
                waited,
                colours,
            );
            let screen_rows = u32::try_from(
                step.outcome
                    .as_ref()
                    .expect("every read has an outcome")
                    .height,
            )
            .expect("a small screen");
            if step.whole_document {
                query = Query {
                    start: None,
                    screen_rows,
                    whole: true,
                    above: 0,
                };
                continue;
            }
            if let Some(above) = step.search_above {
                query.above = u32::try_from(above).expect("a small buffer");
                continue;
            }
            self.query = Query {
                start: Some(snapshot.next_start),
                screen_rows,
                whole: false,
                above: u32::try_from(self.reader.memory().above()).expect("a small screen"),
            };
            return Step {
                outcome: None,
                ..step
            };
        }
    }

    fn key(&mut self, kind: KeyKind, at_ms: u64) -> Step {
        self.reader.key(Key { kind, at_ms })
    }

    fn typed(&mut self, text: &str, at_ms: u64) -> Step {
        self.key(KeyKind::Typed(text.to_owned()), at_ms)
    }

    /// The row the review position is on.
    fn review(&self) -> Option<isize> {
        self.reader.memory().review().map(|anchor| anchor.row)
    }
}

/// A step that says and withholds `speech` and `withheld`, with `timer`.
fn step(speech: Vec<Speech>, withheld: Vec<Withheld>, timer: Option<u64>) -> Step {
    Step {
        speech,
        withheld,
        timer,
        whole_document: false,
        search_above: None,
        outcome: None,
    }
}

fn says(speech: Vec<Speech>) -> Step {
    step(speech, Vec::new(), None)
}

fn quiet(timer: Option<u64>) -> Step {
    step(Vec::new(), Vec::new(), timer)
}

fn line(row: usize, text: &str) -> Speech {
    Speech::Line {
        row,
        text: text.to_owned(),
    }
}

fn growth(row: usize, text: &str) -> Speech {
    Speech::Growth {
        row,
        text: text.to_owned(),
    }
}

fn changed(row: usize, text: &str) -> Speech {
    Speech::Changed {
        row,
        text: text.to_owned(),
    }
}

fn answer(row: usize, text: &str) -> Speech {
    Speech::Answer {
        row,
        text: text.to_owned(),
    }
}

fn echo(row: usize, text: &str) -> Speech {
    Speech::Echo {
        row,
        text: text.to_owned(),
    }
}

fn caret(row: usize, unit: Unit, text: &str) -> Speech {
    Speech::Caret {
        row,
        unit,
        text: text.to_owned(),
    }
}

fn withheld(row: usize, reason: Reason) -> Withheld {
    Withheld { row, reason }
}

// Section 5.3, new output.

#[test]
fn new_output_is_spoken_line_by_line() {
    let mut term = Term::new(20, 5, 10);
    assert_eq!(
        term.write("one\r\ntwo\r\n", 100),
        says(vec![line(0, "one"), line(1, "two")])
    );
}

#[test]
fn a_line_seen_for_the_first_time_is_spoken_whole_at_once() {
    let mut term = Term::new(20, 5, 10);
    assert_eq!(term.write("ready>", 100), says(vec![line(0, "ready>")]));
}

#[test]
fn a_growing_line_speaks_its_gain_and_holds_its_unfinished_word() {
    let mut term = Term::new(30, 5, 10);
    term.write("abc def", 100);
    assert_eq!(
        term.write(" ghi jk", 200),
        step(
            vec![growth(0, " ghi ")],
            vec![withheld(0, Reason::UnfinishedWord)],
            Some(1200)
        )
    );
    // The line grows past the word and the cursor leaves it.
    assert_eq!(term.write("l\r\n", 300), says(vec![growth(0, "jkl")]));
}

#[test]
fn an_unfinished_word_is_spoken_after_the_quiet_period() {
    let mut term = Term::new(30, 5, 10);
    term.write("abc", 100);
    assert_eq!(
        term.write("de", 200),
        step(
            Vec::new(),
            vec![withheld(0, Reason::UnfinishedWord)],
            Some(1200)
        )
    );
    assert_eq!(term.reader.timer(1200), says(vec![growth(0, "de")]));
}

#[test]
fn kept_rows_are_silent_as_output_scrolls() {
    let mut term = Term::new(20, 3, 10);
    term.write("a\r\nb\r\nc", 100);
    assert_eq!(term.write("\r\nd", 200), says(vec![line(3, "d")]));
}

#[test]
fn a_flood_speaks_its_first_and_last_lines_and_counts_the_rest() {
    let settings = Settings {
        lines_in_full: 2,
        last_lines: 2,
        ..Settings::default()
    };
    let mut term = Term::with(20, 10, 20, settings);
    assert_eq!(
        term.write("1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n", 100),
        says(vec![
            line(0, "1"),
            line(1, "2"),
            Speech::Skipped {
                rows: vec![2, 3, 4],
                count: Counted::Lines(3)
            },
            line(5, "6"),
            line(6, "7"),
        ])
    );
}

#[test]
fn a_flood_past_the_history_is_counted_as_more_than() {
    let settings = Settings {
        lines_in_full: 1,
        last_lines: 1,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 4, settings);
    term.write("a\r\nb\r\nc\r\nd", 100);
    // Ten lines through a buffer of seven: the last screen is gone, the
    // whole document is read and does not hold it.
    assert_eq!(
        term.write("\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10", 200),
        says(vec![
            line(0, "4"),
            Speech::Skipped {
                rows: vec![1, 2, 3, 4, 5],
                count: Counted::MoreThan(5)
            },
            line(6, "10"),
        ])
    );
}

#[test]
fn a_flood_past_the_history_with_every_line_spoken_is_counted_as_unknown() {
    // Ten lines through a buffer of seven rows, which holds five of them,
    // two wrapped: every line the history holds is spoken, so how many
    // went by is not known, said after the last of those lines.
    let mut term = Term::with(20, 3, 4, Settings::default());
    term.write("a\r\nb\r\nc\r\nd", 100);
    assert_eq!(
        term.write(
            "\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\nseven is a wrapped line\r\n8\r\nnine is a wrapped line\r\n10",
            200
        ),
        says(vec![
            line(0, "6"),
            line(1, "seven is a wrapped line"),
            line(3, "8"),
            line(4, "nine is a wrapped line"),
            line(6, "10"),
            Speech::Skipped {
                rows: vec![],
                count: Counted::Unknown
            },
        ])
    );
}

#[test]
fn a_flood_past_the_history_speaking_more_lines_than_the_first_is_counted_after_the_last() {
    // Fifty lines through a buffer of forty-three rows: the forty-three
    // lines the history holds are fewer than the first thirty and the last
    // thirty together, so each is spoken, and how many went by, not known,
    // is said after the last of them.
    let mut term = Term::with(20, 3, 40, Settings::default());
    term.write("a\r\nb\r\nc", 100);
    let flood: String = (1..=50)
        .flat_map(|number| ["\r\n".to_owned(), number.to_string()])
        .collect();
    let mut expected: Vec<Speech> = (0..43)
        .map(|row| line(row, &(row + 8).to_string()))
        .collect();
    expected.push(Speech::Skipped {
        rows: vec![],
        count: Counted::Unknown,
    });
    assert_eq!(term.write(&flood, 200), says(expected));
}

#[test]
fn a_whole_document_read_leaves_only_the_screen_and_the_block_held() {
    // Section 4.1: once a read of the whole document is handled, the memory
    // and the rules keep its screen and the block of four rows above it,
    // and the rows above the block only as hashes in the known history.
    let settings = Settings {
        lines_in_full: 1,
        last_lines: 1,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 10, settings);
    term.write("a\r\nb\r\nc\r\nd", 100);
    let flood: String = (1..=20)
        .flat_map(|number| ["\r\n".to_owned(), number.to_string()])
        .collect();
    // Twenty lines through a buffer of thirteen: the whole document is read,
    // the rows "8" to "20".
    assert_eq!(
        term.write(&flood, 200),
        says(vec![
            line(0, "8"),
            Speech::Skipped {
                rows: (1..=11).collect(),
                count: Counted::MoreThan(11)
            },
            line(12, "20"),
        ])
    );
    let memory = term.reader.memory();
    let rows = memory.rows();
    assert_eq!(rows.len(), 13);
    assert_eq!(rows.first_kept(), 6);
    let kept: Vec<&str> = rows.kept().iter().map(|row| &*row.text).collect();
    assert_eq!(kept, ["14", "15", "16", "17", "18", "19", "20"]);
    let above: Vec<u64> = (8..=13)
        .map(|number| crate::terminal_rows::hash(&number.to_string()))
        .collect();
    assert_eq!(memory.known_history(), above);
    assert_eq!(term.reader.screen.rows.len(), 7);
    assert_eq!(term.reader.screen.skipped, 6);
}

#[test]
fn an_unfinished_word_pushed_out_of_a_full_history_is_spoken_before_the_count() {
    let settings = Settings {
        lines_in_full: 1,
        last_lines: 1,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 4, settings);
    term.write("abc", 100);
    assert_eq!(
        term.write("de", 200),
        step(
            Vec::new(),
            vec![withheld(0, Reason::UnfinishedWord)],
            Some(1200)
        )
    );
    // Ten lines through a buffer of seven before the quiet period: the
    // held word is spoken as it was last read, then the flood.
    assert_eq!(
        term.write("\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10", 300),
        says(vec![
            Speech::Held {
                text: "de".to_owned()
            },
            line(0, "4"),
            Speech::Skipped {
                rows: vec![1, 2, 3, 4, 5],
                count: Counted::MoreThan(5)
            },
            line(6, "10"),
        ])
    );
}

#[test]
fn a_volatile_row_pushed_out_of_a_full_history_is_spoken_before_the_count() {
    let settings = Settings {
        lines_in_full: 1,
        last_lines: 1,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 4, settings);
    term.write("Working 0s", 100);
    term.write("\rWorking 1s", 1100);
    assert_eq!(
        term.write("\rWorking 2s", 2100),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(3600))
    );
    // Ten lines through a buffer of seven before it settles: its latest
    // value is spoken as it was last read, then the flood.
    assert_eq!(
        term.write("\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10", 2200),
        says(vec![
            Speech::Held {
                text: "2s".to_owned()
            },
            line(0, "4"),
            Speech::Skipped {
                rows: vec![1, 2, 3, 4, 5],
                count: Counted::MoreThan(5)
            },
            line(6, "10"),
        ])
    );
}

#[test]
fn the_input_reprinted_below_a_completion_list_is_silent() {
    let mut term = Term::new(30, 6, 10);
    term.write("$ ", 100);
    term.typed("l", 200);
    term.typed("s", 210);
    assert_eq!(term.write("ls", 250), says(vec![echo(0, "ls")]));
    term.key(KeyKind::Forget, 1000);
    assert_eq!(
        term.write("\r\nlsof  lsblk\r\n$ ls", 1050),
        says(vec![line(1, "lsof  lsblk")])
    );
}

#[test]
fn trailing_spaces_are_padding_and_never_spoken() {
    let mut term = Term::new(20, 5, 10);
    assert_eq!(term.write("a b   \r\n", 100), says(vec![line(0, "a b")]));
}

// Section 5.4, redrawn rows.

#[test]
fn a_redrawn_row_speaks_the_word_that_changed() {
    let mut term = Term::new(20, 4, 10);
    assert_eq!(
        term.write("output\r\n\x1b[4;1Hstatus: busy", 100),
        says(vec![line(0, "output"), line(3, "status: busy")])
    );
    assert_eq!(
        term.write("\x1b[4;1H\x1b[2Kstatus: done", 5000),
        says(vec![changed(3, "done")])
    );
}

#[test]
fn a_spinner_symbol_replaced_alone_is_silent() {
    let mut term = Term::new(20, 4, 10);
    term.write("\u{280b} Working...", 100);
    assert_eq!(
        term.write("\r\u{2819} Working...", 5000),
        step(Vec::new(), vec![withheld(0, Reason::Spinner)], None)
    );
}

#[test]
fn a_volatile_row_is_heard_at_its_first_change_and_once_it_settles() {
    let mut term = Term::new(20, 4, 10);
    term.write("Working 0s", 100);
    assert_eq!(
        term.write("\rWorking 1s", 1100),
        says(vec![changed(0, "1s")])
    );
    // Two changes within 2 seconds: volatile, held until quiet for 1.5
    // times the interval between its last two changes.
    assert_eq!(
        term.write("\rWorking 2s", 2100),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(3600))
    );
    assert_eq!(
        term.write("\rWorking 3s", 3100),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(4600))
    );
    assert_eq!(term.reader.timer(4600), says(vec![changed(0, "3s")]));
}

#[test]
fn a_row_changed_at_two_reads_in_a_row_after_waiting_for_speech_is_volatile() {
    let mut term = Term::new(20, 4, 10);
    term.write("50%", 100);
    assert_eq!(term.write("\r60%", 10_000), says(vec![changed(0, "60%")]));
    term.terminal.write(b"\r70%");
    assert_eq!(
        term.take(20_000, true, None),
        step(
            Vec::new(),
            vec![withheld(0, Reason::Volatile)],
            Some(35_000)
        )
    );
}

#[test]
fn a_row_that_only_lost_text_is_silent_until_it_is_drawn_again() {
    let mut term = Term::new(20, 4, 10);
    term.write("Loading 50%", 100);
    assert_eq!(
        term.write("\r\x1b[2KLoading", 5000),
        step(Vec::new(), vec![withheld(0, Reason::LostText)], None)
    );
    assert_eq!(term.write(" 51%", 10_000), says(vec![changed(0, " 51%")]));
}

#[test]
fn a_held_row_scrolled_above_the_screen_is_spoken_before_the_next_read_loses_it() {
    let mut term = Term::new(20, 3, 10);
    term.write("Working 0s", 100);
    term.write("\rWorking 1s", 1100);
    term.write("\rWorking 2s", 2100);
    // Output pushes it into the history, where no program redraws it.
    assert_eq!(
        term.write("\r\na\r\nb\r\nc", 2200),
        says(vec![
            line(1, "a"),
            line(2, "b"),
            line(3, "c"),
            changed(0, "2s")
        ])
    );
}

// Section 5.5, keys.

#[test]
fn typing_the_terminal_shows_is_echoed() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ ", 100);
    term.typed("l", 200);
    assert_eq!(term.write("l", 250), says(vec![echo(0, "l")]));
}

#[test]
fn typing_shown_after_300_milliseconds_is_output_not_echo() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ ", 100);
    term.typed("l", 200);
    assert_eq!(
        term.write("l", 600),
        step(
            Vec::new(),
            vec![withheld(0, Reason::UnfinishedWord)],
            Some(1600)
        )
    );
    assert_eq!(term.reader.timer(1600), says(vec![growth(0, " l")]));
}

#[test]
fn typing_the_terminal_never_shows_is_never_spoken() {
    let mut term = Term::new(20, 3, 10);
    term.write("Password: ", 100);
    term.typed("s", 200);
    assert_eq!(term.read(250), quiet(None));
    term.key(KeyKind::Forget, 300);
    assert_eq!(term.write("\r\nok\r\n", 350), says(vec![line(1, "ok")]));
}

#[test]
fn a_line_changed_other_than_by_the_typing_speaks_what_it_shows() {
    let mut term = Term::new(20, 3, 10);
    term.write("Password: ", 100);
    term.typed("s", 200);
    assert_eq!(
        term.write("*", 250),
        says(vec![Speech::Gained {
            row: 0,
            text: "*".to_owned()
        }])
    );
}

#[test]
fn a_key_landing_at_a_word_start_says_the_word() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ echo hello", 100);
    term.key(KeyKind::Other, 200);
    assert_eq!(
        term.write("\x1b[8G", 250),
        says(vec![caret(0, Unit::Word, "hello")])
    );
}

#[test]
fn a_key_moving_one_character_says_the_character() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ echo hello", 100);
    term.key(KeyKind::Other, 200);
    assert_eq!(
        term.write("\x1b[D", 250),
        says(vec![caret(0, Unit::Character, "o")])
    );
}

#[test]
fn a_key_landing_at_the_line_start_says_the_character_there() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ echo hello", 100);
    term.key(KeyKind::Other, 200);
    assert_eq!(
        term.write("\r", 250),
        says(vec![caret(0, Unit::Character, "$")])
    );
}

#[test]
fn a_caret_key_verbatim_knows_says_its_unit() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ echo hello", 100);
    term.key(KeyKind::Caret(Unit::Word), 200);
    assert_eq!(
        term.write("\x1b[5D", 250),
        says(vec![caret(0, Unit::Word, "hello")])
    );
}

#[test]
fn text_the_caret_line_gained_that_was_not_typed_is_spoken() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ ec", 100);
    term.key(KeyKind::Forget, 200);
    assert_eq!(
        term.write("ho ", 250),
        says(vec![Speech::Gained {
            row: 0,
            text: "ho".to_owned()
        }])
    );
}

#[test]
fn text_the_caret_line_lost_next_to_the_caret_is_spoken() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ echo hello", 100);
    term.key(KeyKind::Other, 200);
    assert_eq!(
        term.write("\x1b[5D\x1b[K", 250),
        says(vec![Speech::Lost {
            row: 0,
            text: "hello".to_owned()
        }])
    );
}

#[test]
fn up_arrow_and_a_character_at_once_speak_the_recalled_line_with_it_once() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ ", 100);
    term.key(KeyKind::Caret(Unit::Line), 200);
    term.typed("x", 210);
    assert_eq!(
        term.write("echo onex", 250),
        says(vec![caret(0, Unit::Line, "$ echo onex")])
    );
}

#[test]
fn a_change_found_after_the_keys_window_is_output() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ abcdef", 100);
    term.key(KeyKind::Other, 200);
    assert_eq!(
        term.write("\r$ abcxyz", 600),
        says(vec![changed(0, "abcxyz")])
    );
}

// Section 5.6, the review cursor.

#[test]
fn the_review_cursor_follows_the_caret_a_key_moved() {
    let mut term = Term::new(20, 5, 10);
    term.write("a\r\nb\r\nc", 100);
    assert_eq!(
        term.review(),
        Some(0),
        "the program's cursor is not followed"
    );
    term.key(KeyKind::Caret(Unit::Line), 1000);
    assert_eq!(
        term.write("\x1b[A", 1050),
        says(vec![caret(1, Unit::Line, "b")])
    );
    assert_eq!(term.review(), Some(1));
}

#[test]
fn with_review_following_off_only_the_user_moves_the_review_cursor() {
    let settings = Settings {
        review_follows_caret: false,
        ..Settings::default()
    };
    let mut term = Term::with(20, 5, 10, settings);
    term.write("a\r\nb\r\nc", 100);
    term.key(KeyKind::Caret(Unit::Line), 1000);
    term.write("\x1b[A", 1050);
    assert_eq!(term.review(), Some(0));
}

#[test]
fn the_focus_arriving_moves_the_review_cursor_to_the_caret() {
    let mut term = Term::new(20, 5, 10);
    term.write("a\r\nb\r\nc", 100);
    term.reader.focus(200);
    assert_eq!(term.read(300), quiet(None));
    assert_eq!(term.review(), Some(2));
}

// Section 5.7, rows away from the caret that keys change.

#[test]
fn the_row_that_gained_the_marker_is_the_keys_answer() {
    let mut term = Term::new(20, 5, 10);
    term.write("> apple\r\n  banana\r\n$ ", 100);
    term.key(KeyKind::Other, 1000);
    assert_eq!(
        term.write("\x1b[1;1H  apple\x1b[2;1H> banana\x1b[3;3H", 1050),
        step(
            vec![answer(1, "> banana")],
            vec![withheld(0, Reason::LostText)],
            None
        )
    );
}

#[test]
fn a_row_that_changed_on_its_own_just_before_the_key_is_not_its_answer() {
    let mut term = Term::new(20, 5, 10);
    term.write("cpu 10%\r\n$ ", 100);
    term.write("\x1b[1;1Hcpu 20%\x1b[2;3H", 4000);
    term.key(KeyKind::Other, 4500);
    assert_eq!(
        term.write("\x1b[1;1Hcpu 30%\x1b[2;3H", 4550),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(5550))
    );
}

#[test]
fn other_rows_a_key_changed_are_spoken_once_the_keys_pause() {
    let mut term = Term::new(20, 5, 10);
    term.write("line one\r\nline two\r\n\r\n1,1 Top\x1b[1;1H", 100);
    term.key(KeyKind::Caret(Unit::Line), 1000);
    assert_eq!(
        term.write("\x1b[4;1H2,1 Top\x1b[2;1H", 1050),
        step(
            vec![caret(1, Unit::Line, "line two")],
            vec![withheld(3, Reason::HeldForKeys)],
            Some(2000)
        )
    );
    assert_eq!(term.reader.timer(2000), says(vec![changed(3, "2,1 Top")]));
}

#[test]
fn filtering_a_list_says_the_echo_then_the_current_entry_and_the_count_when_typing_pauses() {
    let mut term = Term::new(20, 6, 10);
    // "e" keeps apple and cherry: cherry moves up into banana's row, a
    // kept row that moved, silent (section 5.7), with no history above the
    // screen to rotate.
    term.write("  apple\r\n> banana\r\n  cherry\r\n  3/3\r\n> ", 100);
    term.typed("e", 1000);
    assert_eq!(
        term.write(
            "\x1b[1;1H> apple\x1b[2;1H  cherry\x1b[K\x1b[3;1H\x1b[K\x1b[4;1H  2/3\x1b[5;3He",
            1050
        ),
        step(
            vec![echo(4, "e"), answer(0, "> apple")],
            vec![withheld(3, Reason::HeldForKeys)],
            Some(2000)
        )
    );
    assert_eq!(term.reader.timer(2000), says(vec![changed(3, "2/3")]));
}

#[test]
fn the_row_that_took_the_highlight_is_the_keys_answer() {
    let mut term = Term::new(20, 5, 10);
    term.write("fruits:\r\napple\r\nbanana\r\n$ ", 100);
    let plain = |length| ColourRun {
        column: 0,
        length,
        foreground: 0x00cc_cccc,
        background: 0,
    };
    let lit = |length| ColourRun {
        column: 0,
        length,
        foreground: 0,
        background: 0x00cc_cccc,
    };
    // The first key in the list: no earlier colours, so the row whose
    // colours differ from both its neighbours'.
    term.key(KeyKind::Other, 1000);
    let first = vec![
        vec![plain(20)],
        vec![lit(5), plain(15)],
        vec![plain(20)],
        vec![plain(20)],
    ];
    assert_eq!(
        term.take(1050, false, Some(&first)),
        says(vec![answer(1, "apple")])
    );
    // The highlight moves: the row that took it.
    term.key(KeyKind::Other, 2000);
    let second = vec![
        vec![plain(20)],
        vec![plain(20)],
        vec![lit(6), plain(14)],
        vec![plain(20)],
    ];
    assert_eq!(
        term.take(2050, false, Some(&second)),
        says(vec![answer(2, "banana")])
    );
}

#[test]
fn a_suggestion_is_spoken_when_it_appears_and_not_again_as_typing_follows_it() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ e", 100);
    term.typed("c", 1000);
    assert_eq!(
        term.write_coloured("c\x1b[90mho hi\x1b[0m\x1b[5D", 1050),
        says(vec![
            echo(0, "c"),
            Speech::Suggestion {
                row: 0,
                text: "ho hi".to_owned()
            }
        ])
    );
    term.typed("h", 2000);
    assert_eq!(term.write_coloured("h", 2050), says(vec![echo(0, "h")]));
}

// Section 5.8, the alternate screen.

#[test]
fn the_alternate_screen_speaks_what_it_shows_and_closing_it_speaks_nothing_kept() {
    let mut term = Term::new(20, 3, 10);
    term.write("m1\r\nm2\r\nm3\r\n$ ", 100);
    assert_eq!(
        term.write("\x1b[?1049h\x1b[Hfile one\r\nfile two", 200),
        says(vec![line(0, "file one"), line(1, "file two")])
    );
    assert_eq!(term.write("\x1b[?1049l", 300), quiet(None));
}

// Section 5.9, resizing.

/// A read of a screen of `lines`, `width` cells wide, with nothing above
/// it, the caret on `caret` at `column`.
fn screen_read(lines: &[&str], width: usize, caret: usize, column: usize, at_ms: u64) -> Read {
    let padded: Vec<String> = lines.iter().map(|line| format!("{line:width$}")).collect();
    let text = padded.join("\r\n");
    Read {
        text: text.clone(),
        screen: text,
        from_start: true,
        shrank: false,
        at_document_start: true,
        alternate: true,
        first_row: padded[0].clone(),
        caret_rows_to_end: u32::try_from(lines.len() - caret).expect("a small screen"),
        caret_line: padded[caret].clone(),
        caret_column: column,
        at_ms,
        above: 0,
    }
}

#[test]
fn a_resize_speaks_nothing_but_the_caret_line_when_it_changed() {
    let mut reader = TerminalReader::new(Settings::default());
    reader.read(&screen_read(&["abc", "$ x"], 10, 1, 3, 0), false, None);
    let mut quiet_step = reader.read(&screen_read(&["abc", "$ x"], 20, 1, 3, 100), false, None);
    quiet_step.outcome = None;
    assert_eq!(quiet_step, quiet(None));
    let mut spoken = reader.read(&screen_read(&["abc", "$ y"], 30, 1, 3, 200), false, None);
    spoken.outcome = None;
    assert_eq!(spoken, says(vec![caret(1, Unit::Line, "$ y")]));
}

// Section 5.10, focus.

#[test]
fn output_written_in_the_background_is_not_heard_on_return() {
    let mut term = Term::new(20, 6, 10);
    term.write("a\r\n", 100);
    term.reader.blur();
    assert_eq!(term.write("b\r\n", 200), quiet(None));
    term.reader.focus(300);
    assert_eq!(term.write("c\r\n", 400), quiet(None));
    assert_eq!(term.write("d\r\n", 500), says(vec![line(3, "d")]));
}

// "Report new output".

#[test]
fn with_report_new_output_off_output_is_silent_and_keys_are_answered() {
    let settings = Settings {
        report_new_output: false,
        ..Settings::default()
    };
    let mut term = Term::with(20, 4, 10, settings);
    assert_eq!(term.write("output\r\n$ ", 100), quiet(None));
    term.typed("l", 200);
    assert_eq!(term.write("l", 250), says(vec![echo(1, "l")]));
}

#[test]
fn with_report_new_output_off_a_flood_through_a_full_history_keeps_marks_it_cannot_refute() {
    // Ten lines rotate a full history of twenty past the last screen, which
    // would have the whole document read; with nothing reported the memory
    // is taken from the read, keeping each command mark whose line is where
    // it was, or that is above the read, where it cannot be checked
    // (section 4.2).
    let settings = Settings {
        report_new_output: false,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 20, settings);
    // The memory keeps only the screen and the block above it, so each
    // mark is set on its line while the line is on the screen.
    let row_of = |term: &Term, text: &str| {
        let rows = term.reader.memory().rows();
        rows.first_kept()
            + rows
                .kept()
                .iter()
                .position(|row| &*row.text == text)
                .expect("the line is kept")
    };
    assert_eq!(term.write("f1\r\nf2\r\nf3\r\nf4\r\nf5", 50), quiet(None));
    let f5 = row_of(&term, "f5");
    term.reader.memory_mut().add_mark(f5);
    let filler: String = (6..=20)
        .flat_map(|number| ["\r\nf".to_owned(), number.to_string()])
        .collect();
    assert_eq!(
        term.write(&format!("{filler}\r\n$ one\r\n$ two\r\n$ three"), 100),
        quiet(None)
    );
    let two = row_of(&term, "$ two");
    term.reader.memory_mut().add_mark(two);
    let flood: String = (1..=10)
        .flat_map(|number| ["\r\n".to_owned(), number.to_string()])
        .collect();
    assert_eq!(term.write(&flood, 200), quiet(None));
    // The read starts four rows above the last screen's top, where "f5"
    // was sixteen rows above; "$ two"'s place holds other text now.
    let marks: Vec<isize> = term
        .reader
        .memory()
        .marks()
        .iter()
        .map(|mark| mark.row)
        .collect();
    assert_eq!(marks, [-12]);
}

// Sections 7 and 8: programs.

#[test]
fn pi_streaming_above_its_status_row_speaks_the_streamed_line_only() {
    let mut term = Term::new(20, 6, 10);
    term.write(
        "transcript\r\n\u{280b} Working...\r\n> editor\r\nfooter",
        100,
    );
    assert_eq!(
        term.write(
            "\x1b[2;1H\x1b[Jstreamed\r\n\u{2819} Working...\r\n> editor\r\nfooter",
            200
        ),
        step(
            vec![line(1, "streamed")],
            vec![withheld(2, Reason::Spinner)],
            None
        )
    );
}

#[test]
fn an_ink_region_drawn_again_speaks_only_its_changed_cost() {
    let mut term = Term::new(20, 6, 10);
    term.write("------\r\n> input\r\ncost $1\r\n", 100);
    assert_eq!(
        term.write("\x1b[3A\x1b[J------\r\n> input\r\ncost $2\r\n", 5000),
        says(vec![changed(2, "2")])
    );
}

#[test]
fn several_progress_lines_are_each_heard_when_they_settle() {
    let mut term = Term::new(20, 4, 10);
    term.write("a: 10%\r\nb: 10%", 100);
    term.write("\x1b[1;1Ha: 20%\x1b[2;1Hb: 20%", 1100);
    assert_eq!(
        term.write("\x1b[1;1Ha: 30%\x1b[2;1Hb: 30%", 2100),
        step(
            Vec::new(),
            vec![withheld(0, Reason::Volatile), withheld(1, Reason::Volatile)],
            Some(3600)
        )
    );
    assert_eq!(
        term.reader.timer(3600),
        says(vec![changed(0, "30%"), changed(1, "30%")])
    );
}

#[test]
fn with_output_unreported_the_review_position_stays_on_a_kept_row_through_a_flood() {
    // Section 4.3, item 5, and 5.6: a flood under a footer, with "report
    // new output" off, takes the memory from the read; the review position
    // on the footer, which the read still shows where it was, stays on it.
    let settings = Settings {
        report_new_output: false,
        ..Settings::default()
    };
    let mut term = Term::with(10, 3, 6, settings);
    term.write(
        "\x1b[3;1Hfooter\x1b[1;2r\x1b[1;1Ha\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh",
        100,
    );
    let footer = term.reader.memory().rows().len() - 1;
    term.reader.memory_mut().set_review(footer, 2);
    term.write(
        "\r\nn\r\no\r\np\r\nq\r\nr\r\ns\r\nt\r\nu\r\nv\r\nw\r\nx\r\ny",
        200,
    );
    assert_eq!(footer, 8);
    assert_eq!(
        term.reader.memory().review(),
        Some(Anchor {
            row: 6,
            column: 2,
            line_hash: hash("footer")
        })
    );
}

#[test]
fn with_output_unreported_the_review_position_goes_to_the_caret_when_a_clear_refuses_the_start() {
    // Section 5.6: Windows Terminal's clear moves the screen into the
    // history, so the read's start is refused and the memory's rows cannot
    // be placed in the read; with "report new output" off the memory is
    // taken from the read (section 4.2), and the review position, whose
    // line cannot be found, goes to the caret.
    let settings = Settings {
        report_new_output: false,
        ..Settings::default()
    };
    let mut term = Term::of(Flavor::WindowsTerminal, 10, 3, 20, settings);
    let lines = (1..=22)
        .map(|number| format!("line {number}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    term.write(&lines, 100);
    let screen_row = term.reader.memory().rows().len() - 2;
    term.reader.memory_mut().set_review(screen_row, 2);
    assert_eq!(term.write("\x1b[H\x1b[2Jx", 200), quiet(None));
    assert_eq!(
        term.reader.memory().review(),
        Some(Anchor {
            row: 6,
            column: 1,
            line_hash: hash("x")
        })
    );
}
