//! The rules of `terminal_window`, one test each, on windows written by
//! hand: twenty cells wide, every row on the screen.

use super::*;

/// The screen's width in the tests.
const WIDTH: u32 = 20;

/// A window of `lines`, each one row unless longer than the width, the
/// caret `rows_to_end` rows from the end at `column`, read at `at_ms`.
fn window(lines: &[&str], caret: Option<(u32, usize)>, at_ms: u64) -> Window {
    coloured(lines, caret, at_ms, None)
}

/// [`window`] with every screen row's colours.
fn coloured(
    lines: &[&str],
    caret: Option<(u32, usize)>,
    at_ms: u64,
    colours: Option<Vec<Vec<ColourRun>>>,
) -> Window {
    let text: String = lines.iter().flat_map(|line| [*line, "\r\n"]).collect();
    let lengths: Vec<(u32, u32)> = lines
        .iter()
        .map(|line| (1, u32::try_from(line.chars().count()).unwrap().max(WIDTH)))
        .collect();
    let rows: u32 = lengths
        .iter()
        .map(|(_, length)| length.div_ceil(WIDTH))
        .sum();
    Window::new(Read {
        text: &text,
        lengths: &lengths,
        width: WIDTH,
        screen_top: rows,
        screen_rows: rows,
        caret: caret.map(|(rows_to_end, column)| Caret {
            rows_to_end,
            column,
        }),
        colours,
        ended_us: at_ms * 1000,
    })
}

/// An answer with only `output`, keeping a line of the last.
fn output(lines: &[&str]) -> Answer {
    Answer {
        output: lines.iter().map(|line| (*line).to_owned()).collect(),
        shares_line: true,
        ..Answer::default()
    }
}

/// An answer keeping a line of the last and saying nothing.
fn quiet() -> Answer {
    output(&[])
}

/// An answer with only `output`, keeping no line of the last: a window
/// of one line, changed.
fn changed(lines: &[&str]) -> Answer {
    Answer {
        shares_line: false,
        ..output(lines)
    }
}

/// A key's answer of a line.
fn said(text: &str) -> Said {
    Said {
        unit: Unit::Line,
        text: text.to_owned(),
    }
}

#[test]
fn new_lines_are_output_and_kept_lines_say_nothing() {
    let old = window(&["$ ls", "a"], None, 0);
    let new = window(&["$ ls", "a", "b", "c"], None, 10);
    assert_eq!(Reading::new().answer(&old, &new), output(&["b", "c"]));
}

#[test]
fn blank_lines_are_passed_on_to_be_counted_once_text_is_below_them() {
    // Q1: the blank line between two new lines is counted; the blank rows
    // after the last line with text are not lines yet (TM 12).
    let old = window(&["$ run"], None, 0);
    let new = window(&["$ run", "one", "", "two", "", ""], None, 10);
    assert_eq!(
        Reading::new().answer(&old, &new),
        output(&["one", "", "two"])
    );
}

#[test]
fn a_line_that_only_lost_text_or_gained_white_space_says_nothing() {
    let old = window(&["$ x", "status: done"], None, 0);
    let lost = window(&["$ x", "status:"], None, 10);
    assert_eq!(Reading::new().answer(&old, &lost), quiet());
    let spaced = window(&["$ x", "status:  done"], None, 10);
    assert_eq!(Reading::new().answer(&old, &spaced), quiet());
}

#[test]
fn a_read_with_no_text_where_the_last_had_text_is_dropped() {
    // TM 10: a read landing between a program's clear and its reprint.
    let old = window(&["header", "body"], None, 0);
    let new = window(&["", ""], None, 10);
    let answer = Reading::new().answer(&old, &new);
    assert_eq!(
        answer,
        Answer {
            dropped: true,
            ..Answer::default()
        }
    );
}

#[test]
fn a_read_sharing_no_line_with_the_last_says_so() {
    // F14's input: Core judges a new screen from this and the place count.
    let old = window(&["$ less file"], None, 0);
    let new = window(&["first page", "(END)"], None, 10);
    let answer = Reading::new().answer(&old, &new);
    assert_eq!(
        answer,
        Answer {
            output: vec!["first page".to_owned(), "(END)".to_owned()],
            ..Answer::default()
        }
    );
    assert_eq!(new.screen_lines(), ["first page", "(END)"]);
}

#[test]
fn a_growing_line_holds_its_unfinished_word_until_it_grows_past_it() {
    // F5 (TM 13): the caret at the line's end, no space after it.
    let mut reading = Reading::new();
    let first = window(&["> The qu"], Some((1, 8)), 0);
    let grown = window(&["> The quick brown f"], Some((1, 19)), 100);
    let done = window(&["> The quick brown fox", "$"], Some((1, 1)), 200);
    let base = window(&["> "], Some((1, 2)), 0);
    assert_eq!(reading.answer(&base, &first), changed(&["The"]));
    assert_eq!(reading.due(), Some(1_000_000));
    assert_eq!(reading.answer(&first, &grown), changed(&["quick brown"]));
    assert_eq!(reading.due(), Some(1_100_000));
    // The caret left the line: the rest is spoken, the prompt with it.
    assert_eq!(reading.answer(&grown, &done), changed(&["fox", "$"]));
    assert_eq!(reading.due(), None);
}

#[test]
fn a_held_word_is_spoken_once_the_quiet_period_passes() {
    let mut reading = Reading::new();
    let base = window(&["> "], Some((1, 2)), 0);
    let first = window(&["> Hello wor"], Some((1, 11)), 0);
    assert_eq!(reading.answer(&base, &first), changed(&["Hello"]));
    assert_eq!(reading.timer(999_999), Answer::default());
    assert_eq!(
        reading.timer(1_000_000),
        Answer {
            output: vec!["wor".to_owned()],
            ..Answer::default()
        }
    );
    assert_eq!(reading.due(), None);
}

#[test]
fn a_rewritten_line_is_spoken_from_its_first_changed_word() {
    // F6 (TM 16): vim's ruler.
    let old = window(&["text", "1,1           Top"], None, 0);
    let new = window(&["text", "2,1           Top"], None, 10);
    assert_eq!(
        Reading::new().answer(&old, &new),
        output(&["2,1           Top"])
    );
    let old = window(&["text", "Copying file one"], None, 0);
    let new = window(&["text", "Copying file two"], None, 10);
    assert_eq!(Reading::new().answer(&old, &new), output(&["two"]));
}

#[test]
fn a_spinner_glyph_changed_alone_is_silent() {
    // Q6: one character, neither a letter nor a digit, replaced in place.
    let old = window(&["| Working"], None, 0);
    let new = window(&["/ Working"], None, 10);
    assert_eq!(Reading::new().answer(&old, &new), changed(&[]));
}

#[test]
fn a_row_rewritten_at_the_read_before_a_read_that_waited_for_speech_is_volatile() {
    // TM 17: two reads in a row of which the second waited for speech,
    // however far apart, count as changes within the limit; without the
    // wait the second change, five seconds on, is spoken.
    let zero = window(&["build", "Working 0s"], None, 0);
    let one = window(&["build", "Working 1s"], None, 1000);
    let six = window(&["build", "Working 6s"], None, 6000);
    let mut waited = Reading::new();
    assert_eq!(waited.answer(&zero, &one), output(&["1s"]));
    assert_eq!(waited.answer_waited(&one, &six), quiet());
    assert_eq!(waited.due(), Some(13_500_000));
    let mut live = Reading::new();
    assert_eq!(live.answer(&zero, &one), output(&["1s"]));
    assert_eq!(live.answer(&one, &six), output(&["6s"]));
}

#[test]
fn a_row_changing_twice_within_two_seconds_is_held_until_it_settles() {
    // F7 (TM 17): its first change heard, then silent, then spoken whole
    // once still for the quiet period, 1.5 times the interval when longer.
    let mut reading = Reading::new();
    let zero = window(&["build", "Working 0s"], None, 0);
    let one = window(&["build", "Working 1s"], None, 1000);
    let two = window(&["build", "Working 2s"], None, 2000);
    let three = window(&["build", "Working 3s"], None, 3000);
    assert_eq!(reading.answer(&zero, &one), output(&["1s"]));
    assert_eq!(reading.answer(&one, &two), quiet());
    assert_eq!(reading.due(), Some(3_500_000));
    assert_eq!(reading.answer(&two, &three), quiet());
    assert_eq!(reading.due(), Some(4_500_000));
    assert_eq!(
        reading.timer(4_500_000),
        Answer {
            output: vec!["Working 3s".to_owned()],
            ..Answer::default()
        }
    );
    assert_eq!(reading.due(), None);
}

#[test]
fn typing_is_echoed_at_the_read_that_shows_it_and_never_spoken_as_output() {
    // F9 (TM 20): "s" typed, then "*" shown at a password prompt (Q2).
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ ech"], Some((1, 5)), 0));
    reading.hold_typed("o", 10_000);
    reading.key(KeyKind::Typed, 10_000, &old);
    let shown = window(&["$ echo"], Some((1, 6)), 20);
    assert_eq!(
        reading.answer(&old, &shown),
        Answer {
            echo: vec!["o".to_owned()],
            shares_line: false,
            ..Answer::default()
        }
    );
}

#[test]
fn a_line_changed_some_other_way_drops_the_typing_and_says_what_it_shows() {
    // TM 20 and Q2: a password prompt shows "*" for the "s" typed.
    let mut reading = Reading::new();
    let old = Arc::new(window(&["Password: "], Some((1, 10)), 0));
    reading.hold_typed("s", 10_000);
    reading.key(KeyKind::Typed, 10_000, &old);
    let shown = window(&["Password: *"], Some((1, 11)), 20);
    assert_eq!(
        reading.answer(&old, &shown),
        Answer {
            keys: vec![said("*")],
            ..Answer::default()
        }
    );
}

#[test]
fn typing_not_shown_within_300_milliseconds_is_dropped_unspoken() {
    let mut reading = Reading::new();
    let old = window(&["$ "], Some((1, 2)), 0);
    reading.hold_typed("x", 0);
    let late = window(&["$ "], Some((1, 2)), 400);
    assert_eq!(reading.answer(&old, &late), quiet());
    // The "x" written later is output, not an echo: a word at the caret,
    // held until the quiet period passes (F5).
    let shown = window(&["$ x"], Some((1, 3)), 500);
    assert_eq!(reading.answer(&late, &shown), changed(&[]));
    assert_eq!(
        reading.timer(1_500_000),
        Answer {
            output: vec!["x".to_owned()],
            ..Answer::default()
        }
    );
}

#[test]
fn a_completion_after_tab_is_output_however_short() {
    // Q8: Tab forgets the typing, and the completion is spoken: whole
    // once it ends in a space, a word at the caret once the quiet period
    // passes (F5).
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ cd w"], Some((1, 6)), 0));
    reading.hold_typed("w", 0);
    reading.key(KeyKind::Forget, 0, &old);
    let spaced = window(&["$ cd web "], Some((1, 9)), 1000);
    assert_eq!(reading.answer(&old, &spaced), changed(&["eb"]));
    let old = Arc::new(spaced);
    reading.key(KeyKind::Forget, 2000, &old);
    let slashed = window(&["$ cd web s/"], Some((1, 11)), 3000);
    assert_eq!(reading.answer(&old, &slashed), changed(&[]));
    assert_eq!(
        reading.timer(4_000_000),
        Answer {
            output: vec!["s/".to_owned()],
            ..Answer::default()
        }
    );
}

#[test]
fn a_line_gone_at_the_caret_that_comes_back_is_not_new() {
    // Q7 (TM 9): a read inside Ink's erase and redraw of its input box.
    let mut reading = Reading::new();
    let full = window(&["reply", "> box", "footer"], Some((2, 2)), 0);
    let erased = window(&["reply", "x"], Some((1, 0)), 100);
    let drawn = window(&["reply", "> box", "footer"], Some((2, 2)), 200);
    assert_eq!(reading.answer(&full, &erased), output(&["x"]));
    assert_eq!(
        reading.answer(&erased, &drawn),
        Answer {
            shares_line: true,
            ..Answer::default()
        }
    );
}

#[test]
fn a_line_key_to_another_line_speaks_that_row() {
    // F8, TM 19: Up Arrow in a list the program draws.
    let mut reading = Reading::new();
    let old = Arc::new(window(&["apple", "banana"], Some((1, 0)), 0));
    reading.key(KeyKind::Caret(Unit::Line), 0, &old);
    let moved = window(&["apple", "banana"], Some((2, 0)), 50);
    assert_eq!(
        reading.answer(&old, &moved),
        Answer {
            keys: vec![said("apple")],
            shares_line: true,
            ..Answer::default()
        }
    );
}

#[test]
fn the_caret_back_where_the_keys_started_says_nothing() {
    let mut reading = Reading::new();
    let old = Arc::new(window(&["one", "two"], Some((1, 0)), 0));
    reading.key(KeyKind::Caret(Unit::Line), 0, &old);
    let back = window(&["one", "two"], Some((1, 0)), 50);
    assert_eq!(reading.answer(&old, &back), quiet());
}

#[test]
fn enter_is_answered_only_by_new_output() {
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ ls"], Some((1, 4)), 0));
    reading.key(KeyKind::Enter, 0, &old);
    let ran = window(&["$ ls", "a.txt", "$"], Some((1, 1)), 50);
    assert_eq!(reading.answer(&old, &ran), output(&["a.txt", "$"]));
}

#[test]
fn one_character_lost_before_the_caret_is_spelled() {
    // Backspace, as NVDA's script spells a one-character deletion
    // (`editableText.py` 317 to 320).
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ helo"], Some((1, 6)), 0));
    reading.key(KeyKind::Other, 0, &old);
    let deleted = window(&["$ hel"], Some((1, 5)), 50);
    assert_eq!(
        reading.answer(&old, &deleted),
        Answer {
            keys: vec![Said {
                unit: Unit::Character,
                text: "o".to_owned(),
            }],
            ..Answer::default()
        }
    );
}

#[test]
fn a_word_lost_before_the_caret_is_said_as_text() {
    // Control+Backspace: NVDA speaks a longer deletion as a message.
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ echo hello"], Some((1, 12)), 0));
    reading.key(KeyKind::Other, 0, &old);
    let deleted = window(&["$ echo "], Some((1, 7)), 50);
    assert_eq!(
        reading.answer(&old, &deleted),
        Answer {
            keys: vec![said("hello")],
            ..Answer::default()
        }
    );
}

#[test]
fn delete_speaks_what_it_leaves_at_the_caret() {
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ echo"], Some((1, 2)), 0));
    reading.key(KeyKind::Delete, 0, &old);
    let deleted = window(&["$ cho"], Some((1, 2)), 50);
    assert_eq!(
        reading.answer(&old, &deleted),
        Answer {
            keys: vec![Said {
                unit: Unit::Character,
                text: "c".to_owned(),
            }],
            ..Answer::default()
        }
    );
}

#[test]
fn a_recalled_line_with_a_character_typed_after_it_is_the_line() {
    // f4fb5828: Up Arrow and "t" at once answer once, without the echo.
    let mut reading = Reading::new();
    let old = Arc::new(window(&["$ "], Some((1, 2)), 0));
    reading.key(KeyKind::Caret(Unit::Line), 0, &old);
    reading.hold_typed("t", 10_000);
    reading.key(KeyKind::Typed, 10_000, &old);
    let recalled = window(&["$ cat a.txt"], Some((1, 11)), 60);
    assert_eq!(
        reading.answer(&old, &recalled),
        Answer {
            keys: vec![said("$ cat a.txt")],
            ..Answer::default()
        }
    );
}

/// One row's colours, a highlight or not.
fn row(highlight: bool) -> Vec<ColourRun> {
    let (foreground, background) = if highlight { (0, 7) } else { (7, 0) };
    vec![ColourRun {
        column: 0,
        length: WIDTH,
        foreground,
        background,
    }]
}

#[test]
fn the_row_a_key_moved_the_highlight_to_is_spoken() {
    // F8's larger form, TM 21: the highlight moved by colour alone, the
    // caret left on the input line (fzf).
    let mut reading = Reading::new();
    let lines = ["top", "apple", "banana", "cherry", "> an"];
    let colours = |at: usize| (0..5).map(|row| self::row(row == at)).collect();
    let old = Arc::new(coloured(&lines, Some((1, 4)), 0, Some(colours(2))));
    reading.key(KeyKind::Other, 0, &old);
    let moved = coloured(&lines, Some((1, 4)), 50, Some(colours(1)));
    assert_eq!(
        reading.answer(&old, &moved),
        Answer {
            keys: vec![said("apple")],
            shares_line: true,
            ..Answer::default()
        }
    );
}

#[test]
fn a_row_away_from_the_caret_that_gained_text_is_spoken_whole_at_once() {
    let mut reading = Reading::new();
    let old = Arc::new(window(&["  banana", "$ x"], Some((1, 3)), 0));
    reading.key(KeyKind::Other, 0, &old);
    let marked = window(&["> banana", "$ x"], Some((1, 3)), 50);
    assert_eq!(
        reading.answer(&old, &marked),
        Answer {
            keys: vec![said("> banana")],
            shares_line: true,
            ..Answer::default()
        }
    );
}

#[test]
fn another_row_the_keys_changed_is_spoken_once_the_keys_pause() {
    // TM 21: fzf's count, held while keys keep coming.
    let mut reading = Reading::new();
    let old = Arc::new(window(&["  40/40", "> "], Some((1, 2)), 0));
    reading.key(KeyKind::Other, 0, &old);
    let counted = window(&["  24/40", "> "], Some((1, 2)), 50);
    assert_eq!(reading.answer(&old, &counted), quiet());
    assert_eq!(reading.due(), Some(1_000_000));
    assert_eq!(
        reading.timer(1_000_000),
        Answer {
            keys: vec![said("24/40")],
            ..Answer::default()
        }
    );
}

#[test]
fn a_row_the_program_was_changing_before_the_keys_stays_the_programs() {
    // TM 18: a clock on the prompt's row away from the caret.
    let mut reading = Reading::new();
    let first = window(&["12:00:01", "$ "], Some((1, 2)), 0);
    let second = window(&["12:00:02", "$ "], Some((1, 2)), 1000);
    assert_eq!(reading.answer(&first, &second), output(&["02"]));
    let old = Arc::new(second);
    reading.key(KeyKind::Other, 1_500_000, &old);
    let third = window(&["12:00:03", "$ "], Some((1, 2)), 1600);
    // Its second change within 2 seconds makes it volatile, held.
    assert_eq!(reading.answer(&old, &third), quiet());
    assert_eq!(reading.due(), Some(2_600_000));
}

#[test]
fn a_wrapped_line_takes_its_rows_and_the_caret_its_row() {
    // A line longer than the width wraps; a wide character takes two cells.
    let line = "abcdefghijklmnopqrstuvwxyz";
    let old = window(&[line], Some((1, 2)), 0);
    assert_eq!(old.at_caret(), Some((0, "abcdefghijklmnopqrstuv", "wxyz")));
    assert_eq!(old.row_text(0, 0), "abcdefghijklmnopqrst");
    assert_eq!(old.row_text(0, 1), "uvwxyz");
    assert_eq!(old.line_place(line), Some(1));
    let wide = window(&["中文中文中文中文中文中"], None, 0);
    assert_eq!(wide.row_text(0, 0), "中文中文中文中文中文");
    assert_eq!(wide.row_text(0, 1), "中");
    assert_eq!(
        wide.last_text_line(),
        Some(("中文中文中文中文中文中".to_owned(), 1))
    );
}

#[test]
fn the_empty_baseline_makes_every_line_new() {
    // NVDA's empty text for a first read that failed.
    let new = window(&["alpha", "beta"], None, 10);
    assert_eq!(
        Reading::new().answer(&Window::empty(), &new),
        Answer {
            output: vec!["alpha".to_owned(), "beta".to_owned()],
            ..Answer::default()
        }
    );
}

#[test]
fn the_row_a_highlight_leaves_beside_the_count_gained_nothing() {
    // fzf-inline's read 4: the highlight moves up from the list's last row,
    // which still differs from the count row below it.
    let mut reading = Reading::new();
    let lines = ["top", "apple", "banana", "  2/2", "> an"];
    let colours = |at: usize| -> Vec<Vec<ColourRun>> {
        (0..5)
            .map(|line| {
                let mut runs = row(line == at);
                if line == 3 {
                    runs[0].foreground = 3;
                }
                runs
            })
            .collect()
    };
    let old = Arc::new(coloured(&lines, Some((1, 4)), 0, Some(colours(2))));
    reading.key(KeyKind::Other, 0, &old);
    let moved = coloured(&lines, Some((1, 4)), 50, Some(colours(1)));
    assert_eq!(
        reading.answer(&old, &moved),
        Answer {
            keys: vec![said("apple")],
            shares_line: true,
            ..Answer::default()
        }
    );
}

#[test]
fn an_inserted_line_keeps_its_leading_spaces() {
    // Only a row's padding, after its last character, is not its text
    // (TM 3): a list's unmarked entries are said as written.
    let old = window(&["$ pick"], None, 0);
    let new = window(&["$ pick", "> apple", "  banana", "$"], None, 10);
    assert_eq!(
        Reading::new().answer(&old, &new),
        output(&["> apple", "  banana", "$"])
    );
}
