//! The emulator's own behaviour, one test per operation the model names.
//! What the real terminals answer is checked by the conformance tests.

use super::*;

fn terminal(height: usize, history: usize) -> Terminal {
    Terminal::new(Flavor::ConsoleHost, 10, height, history)
}

#[test]
fn windows_terminals_rows_end_with_their_line_break() {
    let mut terminal = Terminal::new(Flavor::WindowsTerminal, 10, 3, 5);
    terminal.write(b"ab\r\n");
    let read = first_read(&terminal);
    assert_eq!(
        (read.text.as_str(), read.caret_line.as_str()),
        ("ab        \r\n          \r\n", "          \r\n")
    );
}

fn first_read(terminal: &Terminal) -> Snapshot {
    terminal
        .snapshot(&Query::default())
        .expect("the read succeeds")
}

#[test]
fn a_line_is_padded_to_the_width_and_the_document_ends_with_the_cursors_row() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\n");
    let read = first_read(&terminal);
    assert_eq!(read.text, "ab        \r\n          ");
}

#[test]
fn a_line_past_the_last_column_is_one_line_over_two_rows() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"0123456789abc\r\n");
    let read = first_read(&terminal);
    assert_eq!(read.text, "0123456789abc       \r\n          ");
    assert!(terminal.grid().main.rows[0].wraps);
}

#[test]
fn a_full_history_rotates_under_fixed_coordinates() {
    let mut terminal = terminal(2, 1);
    terminal.write(b"one\r\ntwo\r\nthree\r\nfour");
    let rows = &terminal.grid().main.rows;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        [
            rows[0].text().trim_end(),
            rows[1].text().trim_end(),
            rows[2].text().trim_end()
        ],
        ["two", "three", "four"]
    );
}

#[test]
fn a_row_keeps_its_identity_as_the_screen_scrolls() {
    let mut terminal = terminal(2, 5);
    terminal.write(b"one\r\n");
    let id = terminal.grid().main.rows[0].id;
    terminal.write(b"two\r\nthree\r\n");
    let row = &terminal.grid().main.rows[0];
    assert_eq!((row.id, row.text().trim_end()), (id, "one"));
}

#[test]
fn a_main_range_inside_the_alternate_screen_reads_it_and_one_beyond_is_refused() {
    let mut terminal = terminal(2, 5);
    terminal.write(b"1\r\n2\r\n3\r\n4\r\n");
    let inside = Range::at(Position { row: 1, column: 0 });
    let beyond = Range::at(Position { row: 4, column: 0 });
    terminal.write(b"\x1b[?1049hALT");
    assert_eq!(
        terminal.text(Range {
            start: inside.start,
            end: Position { row: 1, column: 10 }
        }),
        Ok("ALT       ".to_owned())
    );
    assert_eq!(terminal.text(beyond), Err(Refused));
}

#[test]
fn a_kept_start_reads_from_where_it_was() {
    let mut terminal = terminal(3, 10);
    terminal.write(b"a\r\nb\r\n");
    let read = first_read(&terminal);
    terminal.write(b"c\r\n");
    let next = terminal
        .snapshot(&Query {
            start: Some(read.next_start),
            screen_rows: screen_rows(&read.screen),
            whole: false,
        })
        .expect("the read succeeds");
    assert!(next.from_start);
    assert_eq!(
        next.text,
        "a         \r\nb         \r\nc         \r\n          "
    );
}

#[test]
fn reverse_video_swaps_the_colours_shown() {
    let mut terminal = terminal(1, 0);
    terminal.write(b"\x1b[7mx\x1b[0my");
    let runs = &terminal.colours()[0];
    assert_eq!(
        (runs[0].foreground, runs[0].background, runs[1].foreground),
        (DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, DEFAULT_FOREGROUND)
    );
}

fn line(text: &str, rows: &[RowId], finished: u64) -> PrintedLine {
    PrintedLine {
        text: text.to_owned(),
        rows: rows.to_vec(),
        finished,
        changed: None,
        left: None,
        reprint: false,
    }
}

fn changed(write: u64, line: PrintedLine) -> PrintedLine {
    PrintedLine {
        changed: Some(write),
        ..line
    }
}

fn left(write: u64, line: PrintedLine) -> PrintedLine {
    PrintedLine {
        left: Some(write),
        ..line
    }
}

#[test]
fn a_line_ended_by_a_line_feed_is_recorded_and_one_still_written_is_not() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\ncd");
    let id = terminal.grid().main.rows[0].id;
    assert_eq!(terminal.printed(), [line("ab", &[id], 1)]);
}

#[test]
fn a_wrapped_line_is_one_line_over_its_rows() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"0123456789abc\r\n");
    let rows = &terminal.grid().main.rows;
    assert_eq!(
        terminal.printed(),
        [line("0123456789abc", &[rows[0].id, rows[1].id], 1)]
    );
}

#[test]
fn a_line_redrawn_after_a_carriage_return_is_recorded_as_the_line_feed_finds_it() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"10%\r50%\r\n");
    assert_eq!(terminal.printed(), [line("50%", &[0], 1)]);
}

#[test]
fn a_second_line_feed_on_an_unchanged_line_records_nothing_more() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\n\x1b[A\n");
    assert_eq!(terminal.printed(), [line("ab", &[0], 1)]);
}

#[test]
fn a_character_written_over_a_line_changes_it() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\ncd\r\n");
    terminal.write(b"\x1b[1;1Hx");
    assert_eq!(
        terminal.printed(),
        [changed(2, line("ab", &[0], 1)), line("cd", &[1], 1)]
    );
}

#[test]
fn erasing_a_lines_text_changes_it() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\ncd\r\n");
    terminal.write(b"\x1b[1;1H\x1b[K");
    assert_eq!(
        terminal.printed(),
        [changed(2, line("ab", &[0], 1)), line("cd", &[1], 1)]
    );
}

#[test]
fn a_character_inserted_before_a_lines_text_changes_it() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\n");
    terminal.write(b"\x1b[1;1H\x1b[@");
    assert_eq!(terminal.printed(), [changed(2, line("ab", &[0], 1))]);
}

#[test]
fn a_character_deleted_before_a_lines_text_changes_it() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\n");
    terminal.write(b"\x1b[1;1H\x1b[P");
    assert_eq!(terminal.printed(), [changed(2, line("ab", &[0], 1))]);
}

#[test]
fn a_row_starting_to_wrap_changes_its_line_and_the_line_it_joins() {
    // The console host marks a row as wrapping when its last column is
    // written, here with the character already there, so no character
    // changes.
    let mut terminal = terminal(3, 5);
    terminal.write(b"0123456789\r\ncd\r\n");
    terminal.write(b"\x1b[1;10H9");
    assert_eq!(
        terminal.printed(),
        [
            changed(2, line("0123456789", &[0], 1)),
            changed(2, line("cd", &[1], 1)),
        ]
    );
}

#[test]
fn a_row_scrolled_under_a_wrapping_row_changes_its_line() {
    // Scrolling the region of the second and third rows takes the wrapped
    // line's second row out and moves "cd" up under its first row, which
    // still wraps.
    let mut terminal = terminal(4, 5);
    terminal.write(b"0123456789ab\r\ncd\r\n");
    terminal.write(b"\x1b[2;3r\x1b[S");
    assert_eq!(
        terminal.printed(),
        [
            changed(2, left(2, line("0123456789ab", &[0, 1], 1))),
            changed(2, line("cd", &[2], 1)),
        ]
    );
}

#[test]
fn a_line_whose_first_row_continued_a_row_that_left_is_not_recorded() {
    // "0123" wraps onto "45"; scrolling the screen up takes "0123" out of
    // the buffer, and the line feed then ends "45" alone.
    let mut terminal = Terminal::new(Flavor::WindowsTerminal, 4, 3, 5);
    terminal.write(b"012345\x1b[S\x1b[A\r\n");
    assert_eq!(terminal.printed(), []);
}

#[test]
fn a_line_records_the_write_it_left_a_full_history_in() {
    let mut terminal = terminal(2, 1);
    terminal.write(b"one\r\ntwo\r\n");
    terminal.write(b"three\r\nfour\r\n");
    assert_eq!(
        terminal.printed(),
        [
            left(2, line("one", &[0], 1)),
            left(2, line("two", &[1], 1)),
            line("three", &[2], 2),
            line("four", &[3], 2),
        ]
    );
}

#[test]
fn a_line_records_the_write_that_erased_the_history_it_was_in() {
    let mut terminal = terminal(2, 5);
    terminal.write(b"ab\r\ncd\r\nef\r\n");
    terminal.write(b"\x1b[3J");
    assert_eq!(
        terminal.printed(),
        [
            left(2, line("ab", &[0], 1)),
            left(2, line("cd", &[1], 1)),
            line("ef", &[2], 1),
        ]
    );
}

#[test]
fn a_line_records_the_write_that_scrolled_it_out_of_a_region() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"ab\r\ncd\r\n");
    terminal.write(b"\x1b[1;2r\x1b[T");
    assert_eq!(
        terminal.printed(),
        [line("ab", &[0], 1), left(2, line("cd", &[1], 1))]
    );
}

#[test]
fn lines_after_a_clear_that_repeat_the_screen_before_it_are_a_reprint_until_one_differs() {
    let mut terminal = terminal(4, 5);
    terminal.write(b"ab\r\ncd\r\n");
    terminal.write(b"\x1b[H\x1b[2Jab\r\nxx\r\ncd\r\n");
    assert_eq!(
        terminal.printed(),
        [
            line("ab", &[0], 1),
            line("cd", &[1], 1),
            PrintedLine {
                reprint: true,
                ..line("ab", &[2], 2)
            },
            line("xx", &[3], 2),
            line("cd", &[4], 2),
        ]
    );
}

#[test]
fn a_line_on_the_alternate_screen_is_not_recorded() {
    let mut terminal = terminal(3, 5);
    terminal.write(b"\x1b[?1049hab\r\n");
    assert_eq!(terminal.printed(), []);
}
