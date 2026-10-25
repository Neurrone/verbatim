//! The reading rules, one rule of `docs/design/terminal-model.md`, "Reading
//! rules", or one case of "Evidence", each: a stream written into the
//! simulated terminal, read as the outpost reads it at the times given,
//! with keys as they reach the program, and the speech, the lines withheld
//! and the timer asserted whole.

use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::{Flavor, Query, Terminal};

use super::{
    ColourRun, Key, KeyKind, Reason, Settings, Speech, Step, TerminalReader, Unit, Withheld,
};
use crate::terminal_rows::{Anchor, Lost, Read, ReadText, Text, hash};

/// A terminal read as the outpost reads it: each read starts where the last
/// said, a screen above it, the whole document is read when the reader
/// asks, and the first read after a key takes the screen's colours.
struct Term {
    terminal: Terminal,
    reader: TerminalReader,
    query: Query,
    /// Whether the last read moved the review position to the caret.
    followed: bool,
    /// Whether a key came since the last read, so the next read takes the
    /// screen's colours.
    colours_due: bool,
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
            followed: false,
            colours_due: false,
        };
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

    /// One read at `at_ms` without the screen's colours, even after a key.
    fn read_colourless(&mut self, at_ms: u64) -> Step {
        self.colours_due = false;
        self.take(at_ms, false, None)
    }

    /// One read at `at_ms`, the whole document read again when asked, with
    /// `colours`, or the screen's own when a key came since the last read.
    fn take(&mut self, at_ms: u64, waited: bool, colours: Option<&[Vec<ColourRun>]>) -> Step {
        let due = (colours.is_none() && self.colours_due).then(|| self.colours());
        let colours = colours.or(due.as_deref());
        self.colours_due = false;
        let mut query = self.query;
        let mut taken = 0;
        loop {
            let snapshot = self.terminal.snapshot(&query).expect("a simulated read");
            taken += 1;
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
                above: usize::try_from(snapshot.above).expect("a few rows"),
            };
            let step = self.reader.read(&read, waited, colours);
            let screen_rows = u32::try_from(
                step.outcome
                    .as_ref()
                    .expect("every read has an outcome")
                    .height,
            )
            .expect("a small screen");
            if step.whole_document {
                assert_eq!(taken, 1, "the whole document is read at most once");
                query = Query {
                    start: None,
                    screen_rows,
                    whole: true,
                    above: 0,
                };
                continue;
            }
            if step.history_block {
                assert_eq!(taken, 1, "a first read is read again at most once");
                query = Query {
                    start: None,
                    screen_rows,
                    whole: false,
                    above: screen_rows,
                };
                continue;
            }
            self.query = Query {
                start: Some(snapshot.next_start),
                screen_rows,
                whole: false,
                above: u32::try_from(self.reader.memory().above()).expect("a small screen"),
            };
            // Whether the review position followed is asserted beside where
            // it is ([`Term::review`]).
            self.followed = step.followed;
            return Step {
                outcome: None,
                followed: false,
                ..step
            };
        }
    }

    fn key(&mut self, kind: KeyKind, at_ms: u64) -> Step {
        self.colours_due = true;
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
        history_block: false,
        outcome: None,
        followed: false,
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
        text: Text::from(text),
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

// "New text": new output.

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
fn a_whole_document_read_leaves_only_the_screen_and_the_screen_above_held() {
    // Once a read of the whole document is handled, the memory and the
    // rules keep its screen and a screen above it, and the rows above only
    // as hashes in the remembered history ("The memory").
    let mut term = Term::new(20, 3, 10);
    term.write("a\r\nb\r\nc\r\nd", 100);
    let flood: String = (1..=20)
        .flat_map(|number| ["\r\n".to_owned(), number.to_string()])
        .collect();
    // Twenty lines through a buffer of thirteen: the whole document is read,
    // the rows "8" to "20", which keep no row of the last read: the history
    // overflowed, and every line is new, for Core's flood policy.
    let mut expected = vec![Speech::LostRead(Lost::MoreThan)];
    expected.extend((8..=20).map(|number| line(number - 8, &number.to_string())));
    assert_eq!(term.write(&flood, 200), says(expected));
    let memory = term.reader.memory();
    let rows = memory.rows();
    assert_eq!(rows.len(), 13);
    assert_eq!(rows.first_kept(), 7);
    let kept: Vec<&str> = rows.kept().iter().map(|row| &*row.text).collect();
    assert_eq!(kept, ["15", "16", "17", "18", "19", "20"]);
    assert_eq!(term.reader.screen.rows.len(), 6);
    assert_eq!(term.reader.screen.skipped, 7);
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

// "Changed rows": redrawn rows.

#[test]
fn a_redrawn_row_speaks_the_word_that_changed() {
    let mut term = Term::new(20, 4, 10);
    // The blank lines between count in Core's flood policy, and say
    // nothing.
    assert_eq!(
        term.write("output\r\n\x1b[4;1Hstatus: busy", 100),
        says(vec![
            line(0, "output"),
            line(1, ""),
            line(2, ""),
            line(3, "status: busy")
        ])
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

// "Keys and their window": keys.

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
fn delete_speaks_what_it_leaves_at_the_caret() {
    // Delete removes the character at the caret: what is said is the
    // character it leaves there, not what it removed ("The caret's line").
    let mut term = Term::new(20, 3, 10);
    term.write("ok\r\n$ echo hello\x1b[2;8H", 100);
    term.key(KeyKind::Delete, 200);
    assert_eq!(
        term.write("\x1b[P", 250),
        says(vec![caret(1, Unit::Character, "e")])
    );
}

#[test]
fn up_arrow_and_a_character_at_once_speak_the_recalled_line_with_it_once() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ ", 100);
    term.key(KeyKind::Caret(Unit::Line), 200);
    term.typed("x", 210);
    // "Keys and their window": the recalled line's change, with the character in it,
    // once, and no echo of the character.
    assert_eq!(
        term.write("echo onex", 250),
        says(vec![Speech::Gained {
            row: 0,
            text: "echo onex".to_owned()
        }])
    );
}

#[test]
fn up_arrow_alone_speaks_the_recalled_line_whole() {
    // A command typed and entered just before, inside the Up Arrow's
    // window, is not a character typed after it.
    let mut term = Term::new(20, 3, 10);
    term.write("$ ", 100);
    term.typed("l", 120);
    term.key(KeyKind::Forget, 150);
    term.key(KeyKind::Caret(Unit::Line), 200);
    assert_eq!(
        term.write("echo one", 250),
        says(vec![caret(0, Unit::Line, "$ echo one")])
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

// "The review anchor and following": the review cursor.

#[test]
fn the_review_cursor_follows_the_caret_a_key_moved() {
    let mut term = Term::new(20, 5, 10);
    term.write("a\r\nb\r\nc", 100);
    assert_eq!(
        (term.review(), term.followed),
        (Some(0), false),
        "the program's cursor is not followed"
    );
    term.key(KeyKind::Caret(Unit::Line), 1000);
    assert_eq!(
        term.write("\x1b[A", 1050),
        says(vec![caret(1, Unit::Line, "b")])
    );
    assert_eq!((term.review(), term.followed), (Some(1), true));
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

// "A key's answer": rows away from the caret that keys change.

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
fn a_key_moving_the_caret_to_another_line_that_also_gained_text_speaks_the_line() {
    // A menu whose cursor follows its marker: a key moves the marker and
    // the caret to the next entry, which gained the marker; the caret's new
    // line is spoken whatever changed on it, and the old entry only lost
    // text ("The caret's line").
    let mut term = Term::new(20, 5, 10);
    term.write("fruits:\r\n> apple\r\n  banana\x1b[2;1H", 100);
    term.key(KeyKind::Other, 1000);
    assert_eq!(
        term.write("\x1b[2;1H  apple\x1b[3;1H> banana\x1b[3;1H", 1050),
        step(
            vec![caret(2, Unit::Line, "> banana")],
            vec![withheld(1, Reason::LostText)],
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
    // kept row that moved, silent ("A key's answer"), with no history above the
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

#[test]
fn a_suggestion_followed_by_text_in_the_typings_colours_is_spoken() {
    // A suggestion need not reach the row's end: a clock in the typing's
    // colours after it ends the suggestion ("Suggestions").
    let mut term = Term::new(30, 3, 10);
    term.write("ok\r\n$ e\x1b[2;20H12:00\x1b[2;4H", 100);
    term.typed("c", 1000);
    assert_eq!(
        term.write_coloured("c\x1b[90mho hi\x1b[0m\x1b[2;5H", 1050),
        says(vec![
            echo(1, "c"),
            Speech::Suggestion {
                row: 1,
                text: "ho hi".to_owned()
            }
        ])
    );
}

// "Screens": the alternate screen.

#[test]
fn the_alternate_screen_speaks_what_it_shows_and_closing_it_speaks_the_shells_screen() {
    // A pager fills the alternate screen: a new screen, spoken. Closing it
    // brings back the main screen, a new screen too: its visible rows are
    // spoken, the row above them history, unspoken, as NVDA speaks the
    // shell's screen when a pager closes (`nv-con-pager.txt`; "Screens").
    let mut term = Term::new(20, 3, 10);
    term.write("m1\r\nm2\r\nm3\r\n$ ", 100);
    assert_eq!(
        term.write("\x1b[?1049h\x1b[Hfile one\r\nfile two\r\n:", 200),
        says(vec![line(0, "file one"), line(1, "file two"), line(2, ":")])
    );
    assert_eq!(
        term.write("\x1b[?1049l", 300),
        says(vec![line(1, "m2"), line(2, "m3"), line(3, "$")])
    );
}

#[test]
fn windows_terminals_clear_leaving_a_shortened_copy_of_the_line_before_says_the_prompt() {
    // Windows Terminal's clear erases the screen and the history and leaves
    // the prompt, a shortened copy of the line the clear was typed on: the
    // document shrank, so the read's start is refused, no row of the last
    // read is kept, and a new screen pairs nothing, so the prompt is new
    // ("Screens"; NVDA's capture in `*_typing`).
    let mut term = Term::of(Flavor::WindowsTerminal, 20, 3, 10, Settings::default());
    term.write("a\r\nb\r\nready> one\r\nout\r\nready> cls", 100);
    assert_eq!(
        term.write("\x1b[H\x1b[2J\x1b[3Jready> ", 200),
        says(vec![line(0, "ready>")])
    );
}

// "Starting again": resizing.

/// A read of a screen of `lines`, `width` cells wide, with nothing above
/// it, the caret on `caret` at `column`.
/// A read's text set to `text`, as the screen and from the document's
/// start, its caret on the last row.
trait WithText {
    fn with_text(self, text: &str) -> Self;
}

impl WithText for Read {
    fn with_text(self, text: &str) -> Self {
        Read {
            text: ReadText::unpadded(text),
            screen: text.to_owned(),
            first_row: text.split("\r\n").next().unwrap_or_default().to_owned(),
            caret_rows_to_end: 1,
            ..self
        }
    }
}

fn screen_read(lines: &[&str], width: usize, caret: usize, column: usize, at_ms: u64) -> Read {
    let padded: Vec<String> = lines.iter().map(|line| format!("{line:width$}")).collect();
    let text = padded.join("\r\n");
    Read {
        text: ReadText::unpadded(&text),
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

#[test]
fn a_resize_keeps_the_review_cursors_character_within_its_line() {
    // The review cursor on "m", on a wrapped line's second row: the line
    // rewrapped onto one row keeps it on "m" ("Starting again").
    let read = |rows: [&str; 3], width: usize, at_ms: u64| {
        let lines: Vec<String> = rows
            .iter()
            .map(|line| {
                let cells = line.len().div_ceil(width).max(1) * width;
                format!("{line:cells$}")
            })
            .collect();
        Read {
            caret_line: lines[2].clone(),
            ..screen_read(&["x"], width, 0, 3, at_ms)
        }
        .with_text(&lines.join("\r\n"))
    };
    let mut reader = TerminalReader::new(Settings::default());
    reader.read(&read(["abcdefghijklmn", "zz", "$ x"], 10, 0), false, None);
    reader.memory_mut().set_review(1, 2);
    reader.read(&read(["abcdefghijklmn", "zz", "$ x"], 20, 100), false, None);
    assert_eq!(
        reader.memory().review(),
        Some(Anchor {
            row: 0,
            column: 12,
            line_hash: hash("abcdefghijklmn")
        })
    );
}

// "The focus": focus.

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
fn with_output_off_a_flood_through_a_full_history_drops_marks_whose_lines_are_not_found() {
    // Ten lines rotate a full history of twenty past the last screen, which
    // would have the whole document read; with nothing reported the memory
    // starts again from the read, and each command mark is looked for by its
    // line's text, and dropped when the read does not show it ("Starting
    // again").
    let settings = Settings {
        report_new_output: false,
        ..Settings::default()
    };
    let mut term = Term::with(20, 3, 20, settings);
    // The memory keeps only the screen and a screen above it, so each mark
    // is set on its line while the line is on the screen.
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
    // The read starts a screen above the last screen's top, and neither
    // "f5" nor "$ two" is in it.
    assert_eq!(term.reader.memory().marks(), []);
}

// "Evidence": programs.

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
    // docker pull's shape: progress rows below the command that started
    // them, which is kept, so the rows beside it pair as rewritten ("The
    // alignment").
    let mut term = Term::new(20, 4, 10);
    term.write("$ pull\r\na: 10%\r\nb: 10%", 100);
    term.write("\x1b[2;1Ha: 20%\x1b[3;1Hb: 20%", 1100);
    assert_eq!(
        term.write("\x1b[2;1Ha: 30%\x1b[3;1Hb: 30%", 2100),
        step(
            Vec::new(),
            vec![withheld(1, Reason::Volatile), withheld(2, Reason::Volatile)],
            Some(3600)
        )
    );
    assert_eq!(
        term.reader.timer(3600),
        says(vec![changed(1, "30%"), changed(2, "30%")])
    );
}

#[test]
fn with_output_unreported_the_review_position_stays_on_a_kept_row_through_a_flood() {
    // A flood under a footer, with "report new output" off, starts the
    // memory again from the read; the review position on the footer is
    // found again by its line, the read's last row ("Starting again").
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
            row: 5,
            column: 2,
            line_hash: hash("footer")
        })
    );
}

#[test]
fn with_output_unreported_the_review_position_is_found_by_its_line_when_a_clear_refuses_the_start()
{
    // Windows Terminal's clear moves the screen into the history, so the
    // read's start is refused; with "report new output" off the memory
    // starts again from the read, and the review position is found again
    // by its line, which the read still shows ("Starting again").
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
            row: 3,
            column: 2,
            line_hash: hash("line 21")
        })
    );
}

#[test]
fn a_volatile_rows_last_value_is_heard_once_it_settles_after_the_line_ends() {
    // "Volatile rows" and "Every time limit": steps under 2 seconds apart make the row
    // volatile; its last step, written with the line's end and the prompt
    // in one read, still waits for the quiet period and is then heard.
    let mut term = Term::new(30, 5, 10);
    term.write("Downloading 10%", 100);
    assert_eq!(
        term.write("\rDownloading 20%", 600),
        says(vec![changed(0, "20%")])
    );
    assert_eq!(
        term.write("\rDownloading 30%", 1100),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(2100))
    );
    assert_eq!(term.reader.timer(2100), says(vec![changed(0, "30%")]));
    assert_eq!(
        term.write("\rDownloading 100%", 2600),
        step(Vec::new(), vec![withheld(0, Reason::Volatile)], Some(4850))
    );
    // The prompt, in the next read, is new output and is spoken at once;
    // the row still waits for its quiet period, 1.5 times the 1.5 seconds
    // between its last two changes.
    assert_eq!(
        term.write("\r\nready>", 2610),
        step(vec![line(1, "ready>")], Vec::new(), Some(4850))
    );
    assert_eq!(term.reader.timer(4850), says(vec![changed(0, "100%")]));
}

#[test]
fn a_cursor_the_program_moves_while_it_draws_and_parks_back_says_nothing() {
    // "Keys and their window": a key the program reads unshown scrolls rows down a
    // line; the program's cursor goes to the top as it resets the scroll
    // region, to the end of the row it writes, and back to where the key
    // found it. Read at each of those, only the new row is spoken. No read
    // can tell the jump to the top from a key's move, so the review cursor
    // follows it, and comes back with the caret.
    let mut term = Term::new(20, 5, 10);
    term.write(
        "row 1\r\nrow 2\r\nrow 3\r\n\r\nstatus: ready\x1b[5;14H",
        100,
    );
    term.typed("n", 1000);
    assert_eq!(term.write("\x1b[1;4r\x1b[1T\x1b[r", 1010), quiet(None));
    assert_eq!((term.review(), term.followed), (Some(0), true));
    assert_eq!(term.write("row 0", 1020), says(vec![line(0, "row 0")]));
    assert_eq!(term.write("\x1b[5;14H", 1030), quiet(None));
    assert_eq!((term.review(), term.followed), (Some(4), true));
}

#[test]
fn typing_onto_a_second_row_and_escape_on_it() {
    // "Keys and their window": typing that wraps past the right margin is echoed as
    // typed, and Escape erasing the whole input says all it removed.
    let mut term = Term::new(20, 5, 10);
    term.write("$ ", 100);
    let mut at = 1000;
    for c in "echo aaaaaaaaaaaaa".chars() {
        term.typed(&c.to_string(), at);
        assert_eq!(
            term.write(&c.to_string(), at + 10),
            says(vec![echo(0, &c.to_string())]),
            "{c} at {at}"
        );
        at += 80;
    }
    for c in ["x", "y", "z"] {
        term.typed(c, at);
        assert_eq!(term.write(c, at + 10), says(vec![echo(0, c)]), "{c}");
        at += 80;
    }
    term.key(KeyKind::Forget, at);
    assert_eq!(
        term.write(&format!("\x1b[1;3H{}\x1b[1;3H", " ".repeat(21)), at + 10),
        says(vec![Speech::Lost {
            row: 0,
            text: "echo aaaaaaaaaaaaaxyz".to_owned()
        }])
    );
}

#[test]
fn a_read_inside_a_programs_redraw_speaks_the_line_its_cursor_passes() {
    // A key the program reads unshown rewrites row 2, and a read lands
    // before the program parks its cursor back on the status row: within
    // the key's window the caret ended on another line that was already
    // there, which is spoken whatever changed on it; the cursor parked back
    // where the key found it did not move ("The caret's line").
    let mut term = Term::new(20, 4, 10);
    term.write("row 1\r\nrow 2\r\n\r\nstatus: ready\x1b[4;14H", 100);
    term.typed("n", 5000);
    assert_eq!(
        term.write("\x1b[2;1H\x1b[2Krow 2 changed", 5010),
        says(vec![caret(1, Unit::Line, "row 2 changed")])
    );
    assert_eq!(term.write("\x1b[4;14H", 5020), quiet(None));
}

#[test]
fn a_scroll_up_in_a_region_with_the_cursor_parked_back_says_only_the_new_row() {
    // "Keys and their window": as the cursor parked back on the status row after the
    // program scrolled the rows above it up a line and wrote the new last
    // one, as conhost_full_screen's last key does.
    let mut term = Term::new(20, 5, 10);
    term.write(
        "row 1\r\nrow 2\r\nrow 3\r\nrow 4\r\nstatus: ready\x1b[5;14H",
        100,
    );
    // The key before, 270 milliseconds earlier and so in the same window,
    // scrolled them down a line and wrote the new first one.
    term.typed("n", 4730);
    assert_eq!(
        term.write("\x1b[1;4r\x1b[1T\x1b[r\x1b[1;1H\x1b[2Krow 0", 4740),
        says(vec![line(0, "row 0")])
    );
    assert_eq!(term.write("\x1b[5;14H", 4750), quiet(None));
    term.typed("n", 5000);
    assert_eq!(
        term.write("\x1b[1;4r\x1b[1S\x1b[r\x1b[4;1H\x1b[2Krow 5", 5010),
        says(vec![line(3, "row 5")])
    );
    assert_eq!(term.write("\x1b[5;14H", 5020), quiet(None));
}

#[test]
fn a_lost_read_taken_as_the_focus_arrives_says_nothing() {
    // Output written while the focus was elsewhere moves a full history
    // further than a read covers, so the focus's read is lost and read
    // whole; it speaks nothing, not even the lost read's mark ("The
    // focus"), and the reads after it find nothing new.
    let mut term = Term::new(20, 5, 10);
    let first = (1..=30)
        .map(|line| format!("f{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let _ = term.write(&first, 100);
    let _ = term.reader.blur();
    let away = (31..=300)
        .map(|line| format!("f{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    term.terminal.write(away.as_bytes());
    let _ = term.reader.focus(5000);
    assert_eq!(term.read(5100), quiet(None));
    assert_eq!(term.read(5200), quiet(None));
}

#[test]
fn the_rows_above_a_first_reads_screen_are_history_at_the_next_read() {
    // A terminal first read with output already in its history is read
    // again with the screen above its screen, its history block; the next
    // read starts a screen above it, finds those rows as they were, and
    // they are history, not output ("The read", "Positions").
    let mut term = Term::new(20, 5, 100);
    let lines = (1..=40)
        .map(|line| format!("line {line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    term.terminal.write(lines.as_bytes());
    term.reader = TerminalReader::new(Settings::default());
    term.query = Query::default();
    assert_eq!(term.read(1000), quiet(None));
    assert_eq!(term.read(1100), quiet(None));
}

#[test]
fn a_flood_past_a_first_reads_screen_and_its_history_is_a_lost_read() {
    // A first read of a terminal whose history is full, as when the focus
    // returns to a Windows Terminal tab while a build streams, holds the
    // screen above its screen. More than two screens of output before the
    // next read move the buffer under those rows, so positions are not
    // trusted, nothing of the first read is found, and the whole document
    // is read: its rows went by past a full history ("A lost read").
    let mut term = Term::of(Flavor::WindowsTerminal, 12, 5, 6, Settings::default());
    let lines = (1..=20)
        .map(|line| format!("p{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    term.terminal.write(lines.as_bytes());
    term.reader = TerminalReader::new(Settings::default());
    term.query = Query::default();
    assert_eq!(term.read(1000), quiet(None));
    let mut flood = (1..=22)
        .map(|line| format!("n{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    flood.push_str("PS C:> ");
    let mut expected = vec![Speech::LostRead(Lost::MoreThan)];
    expected.extend((13..=22).map(|number| line(number - 13, &format!("n{number}"))));
    expected.push(line(10, "PS C:>"));
    assert_eq!(term.write(&flood, 2000), says(expected));
}

#[test]
fn a_flood_larger_than_the_buffer_from_a_fresh_screen_says_more_than_went_by() {
    // A fresh console host, then one write of more lines than its buffer
    // holds before the next read: nothing of the screen is kept and no
    // screen is aside, so the read is lost, and every line of the whole
    // document is new after the lost read's mark, more than went by ("A
    // lost read", "Screens").
    let mut term = Term::new(20, 5, 10);
    assert_eq!(term.write("$ run\r\n", 100), says(vec![line(0, "$ run")]));
    let flood = (1..=20)
        .map(|line| format!("n{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let mut expected = vec![Speech::LostRead(Lost::MoreThan)];
    expected.extend((7..=20).map(|number| line(number - 7, &format!("n{number}"))));
    expected.push(line(14, "$"));
    assert_eq!(term.write(&(flood + "$ "), 200), says(expected));
}

#[test]
fn a_pager_closing_over_its_history_speaks_the_shells_visible_screen() {
    // The pager's reads keep its rows as it scrolls, so the shell's screen
    // stays aside; closing it finds that screen back at its places, a new
    // screen, so only its visible rows are spoken, with no skip ("The
    // aside screen", "Screens"; NVDA speaks the shell's screen when a
    // pager closes, `nv-con-pager.txt`).
    let mut term = Term::new(20, 3, 20);
    let history = (1..=8)
        .map(|line| format!("h{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let _ = term.write(&(history + "$ "), 100);
    assert_eq!(
        term.write("\x1b[?1049h\x1b[Hp1\r\np2\r\n:", 200),
        says(vec![line(0, "p1"), line(1, "p2"), line(2, ":")])
    );
    assert_eq!(
        term.write("\r\x1b[Kp3\r\n:", 300),
        says(vec![line(1, "p3")])
    );
    assert_eq!(
        term.write("\x1b[?1049l", 400),
        says(vec![line(6, "h7"), line(7, "h8"), line(8, "$")])
    );
}

#[test]
fn enter_onto_a_line_already_there_says_nothing_so_a_clear_homing_the_caret_first_is_silent() {
    // "The caret's line": Enter's answer is only new output, never the
    // caret's line, as NVDA turns off its new-line announcement in
    // terminals (Dickson, 2026-10-25). Windows Terminal's `cls` from Windows
    // PowerShell homes the caret onto the screen's first row in one write
    // and erases in the next: a read between them finds the caret on a line
    // that was already there, with no text changed, and says nothing. The
    // erase then leaves the prompt on that row, which only lost text while
    // the caret moved forward over the prompt both reads show: it is not
    // that line, so the read keeps nothing, a new screen, and the prompt is
    // said ("The alignment"; Dickson, 2026-10-25).
    let mut term = Term::of(Flavor::WindowsTerminal, 30, 5, 20, Settings::default());
    assert_eq!(
        term.write("ready> .\\secret.ps1\r\nsecret\r\nready> cls", 100),
        says(vec![
            line(0, "ready> .\\secret.ps1"),
            line(1, "secret"),
            line(2, "ready> cls")
        ])
    );
    term.key(KeyKind::Enter, 1000);
    assert_eq!(term.write("\x1b[H", 1010), quiet(None));
    assert_eq!(
        term.write("\x1b[2J\x1b[3Jready> ", 1020),
        says(vec![line(0, "ready>")])
    );
}

#[test]
fn escape_restoring_a_menu_completed_word_says_what_it_removed() {
    // PSReadLine's MenuComplete: Control+Space after "Get-Ch" completes the
    // word and draws a menu below; Escape restores "Get-Ch" and erases the
    // menu. The caret moved back, so the line is that line, and what it
    // lost is said ("The caret's line").
    let mut term = Term::new(40, 5, 10);
    term.write("PS> Get-Ch", 100);
    term.key(KeyKind::Other, 1000);
    let _ = term.write_coloured("ildItem\r\nGet-ChildItem  Get-Choice\x1b[1;18H", 1050);
    term.key(KeyKind::Forget, 3000);
    assert_eq!(
        term.write_coloured("\x1b[2;1H\x1b[K\x1b[1;11H\x1b[K", 3050),
        says(vec![Speech::Lost {
            row: 0,
            text: "ildItem".to_owned()
        }])
    );
}

// "Colours": the keys' colours are those of the first read with colours in
// their window.

#[test]
fn a_colourless_read_in_a_keys_window_leaves_the_suggestion_to_the_next_read_with_colours() {
    let mut term = Term::new(20, 3, 10);
    term.write("$ e", 100);
    term.typed("c", 1000);
    term.terminal.write(b"c\x1b[90mho hi\x1b[0m\x1b[5D");
    assert_eq!(term.read_colourless(1050), quiet(None));
    let colours = term.colours();
    assert_eq!(
        term.take(1100, false, Some(&colours)),
        says(vec![
            echo(0, "c"),
            Speech::Suggestion {
                row: 0,
                text: "ho hi".to_owned()
            }
        ])
    );
}

#[test]
fn a_colourless_read_in_a_keys_window_leaves_the_emphasis_to_the_next_read_with_colours() {
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
    term.key(KeyKind::Other, 1000);
    assert_eq!(term.read_colourless(1050), quiet(None));
    let colours = vec![
        vec![plain(20)],
        vec![lit(5), plain(15)],
        vec![plain(20)],
        vec![plain(20)],
    ];
    assert_eq!(
        term.take(1100, false, Some(&colours)),
        says(vec![answer(1, "apple")])
    );
}

// The reader restored from its serialized form, as Core's flight recorder
// dumps and replays it ("Where it runs").

/// The steps of a session of typing at a prompt, output, a redrawn row and
/// a recalled line, in which after `restore_after` inputs the reader is
/// serialized and restored and the rest are given to the restored reader.
fn session(restore_after: Option<usize>) -> Vec<Step> {
    let mut term = Term::new(20, 5, 10);
    let mut steps = Vec::new();
    let inputs: [&dyn Fn(&mut Term) -> Step; 10] = [
        &|term| term.write("$ ", 100),
        &|term| term.typed("l", 1000),
        &|term| term.write_coloured("l", 1050),
        &|term| term.typed("s", 1200),
        &|term| term.write_coloured("s", 1250),
        &|term| term.key(KeyKind::Enter, 2000),
        &|term| term.write("\r\none\r\nprogress 10%\r\n", 2050),
        &|term| term.write("\x1b[Aprogress 20%\r\n$ ", 5000),
        &|term| term.key(KeyKind::Caret(Unit::Line), 6000),
        &|term| term.write_coloured("ls", 6050),
    ];
    for (taken, input) in inputs.iter().enumerate() {
        steps.push(input(&mut term));
        if restore_after == Some(taken + 1) {
            let json = serde_json::to_string(&term.reader).expect("the reader serializes");
            let restored: TerminalReader =
                serde_json::from_str(&json).expect("the reader deserializes");
            assert_eq!(restored, term.reader, "the restored reader is the reader");
            term.reader = restored;
        }
    }
    steps
}

#[test]
fn a_reader_serialized_mid_session_and_restored_gives_the_same_steps() {
    let whole = session(None);
    for restore_after in 1..whole.len() {
        assert_eq!(
            session(Some(restore_after)),
            whole,
            "restored after {restore_after} inputs"
        );
    }
}

// A read identical to the last, which the outpost could leave unsent
// (decision 1 of the plan for moving the reader into Core).

/// One read of `terminal` at `at_ms` as the outpost takes it, given to
/// `reader`, the whole document or the history block read again at once
/// when the rules ask, and the query of the next read.
fn corpus_read(
    terminal: &Terminal,
    reader: &mut TerminalReader,
    query: &mut Query,
    at_ms: u64,
) -> Step {
    let mut taken = 0;
    loop {
        let snapshot = terminal.snapshot(query).expect("a simulated read");
        taken += 1;
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
            above: usize::try_from(snapshot.above).expect("a few rows"),
        };
        let step = reader.read(&read, false, None);
        let height = step.outcome.as_ref().map_or(0, |outcome| outcome.height);
        let screen_rows = u32::try_from(height).expect("a small screen");
        if step.whole_document || step.history_block {
            assert_eq!(taken, 1, "a read is taken again at most once");
            *query = Query {
                start: None,
                screen_rows,
                whole: step.whole_document,
                above: if step.whole_document { 0 } else { screen_rows },
            };
            continue;
        }
        let blank = step.outcome.as_ref().is_some_and(|outcome| outcome.blank);
        *query = Query {
            start: if blank {
                query.start
            } else {
                Some(snapshot.next_start)
            },
            screen_rows,
            whole: false,
            above: u32::try_from(reader.memory().above()).expect("a small screen"),
        };
        return step;
    }
}

/// What the rules say over a corpus stream in `flavor`, read as the oracle
/// reads it, a read after each chunk and one after the quiet period, with
/// the quiet-period timer run when it comes due, as Core runs it; and, with
/// `twice`, each read followed by another with nothing written between, at
/// the time of the read after it: the read the outpost would take next,
/// identical to the last but for its time. A read taken again says and
/// withholds nothing.
fn corpus_speech((name, stream): (&str, &Stream), flavor: Flavor, twice: bool) -> Vec<Speech> {
    const QUIET_MS: u64 = 10_000;
    let mut terminal = Terminal::new(flavor, 120, 30, history(flavor));
    let mut reader = TerminalReader::new(Settings::default());
    let mut query = Query::default();
    let mut speech = Vec::new();
    let mut due: Option<u64> = None;
    // The timer, run at each time it came due before `at_ms`.
    let timers = |reader: &mut TerminalReader,
                  due: &mut Option<u64>,
                  speech: &mut Vec<Speech>,
                  at_ms: u64| {
        while let Some(at) = due.filter(|at| *at <= at_ms) {
            let step = reader.timer(at);
            speech.extend(step.speech);
            *due = step.timer;
        }
    };
    let time = |chunk: usize| {
        stream
            .at_ms
            .get(chunk)
            .copied()
            .unwrap_or(u64::try_from(chunk).expect("a few chunks") * 10)
    };
    let last = time(stream.chunks.len().saturating_sub(1)) + QUIET_MS;
    for chunk in 0..=stream.chunks.len() {
        if let Some(bytes) = stream.chunks.get(chunk) {
            terminal.write(bytes.as_bytes());
        }
        let at_ms = if chunk < stream.chunks.len() {
            time(chunk)
        } else {
            last
        };
        timers(&mut reader, &mut due, &mut speech, at_ms);
        let step = corpus_read(&terminal, &mut reader, &mut query, at_ms);
        speech.extend(step.speech);
        due = step.timer;
        if twice {
            let next_ms = if chunk + 1 < stream.chunks.len() {
                time(chunk + 1)
            } else {
                last
            };
            timers(&mut reader, &mut due, &mut speech, next_ms);
            let again = corpus_read(&terminal, &mut reader, &mut query, next_ms);
            due = again.timer;
            assert_eq!(
                (again.speech, again.withheld),
                (Vec::new(), Vec::new()),
                "{name} in {flavor:?}: the read after chunk {chunk} taken again at {next_ms} says nothing"
            );
        }
    }
    speech
}

#[test]
fn every_corpus_stream_with_each_read_given_twice_says_what_it_says_with_each_read_once() {
    let corpus = streams();
    assert!(!corpus.is_empty(), "the corpus has no streams");
    for (name, stream) in &corpus {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            assert_eq!(
                corpus_speech((name, stream), flavor, true),
                corpus_speech((name, stream), flavor, false),
                "{name} in {flavor:?}"
            );
        }
    }
}
