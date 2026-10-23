//! The memory and the alignment, one case of `docs/design/terminal-model.md`,
//! sections 4 and 5.8, each: a stream written into the simulated terminal,
//! read as the outpost reads it, and what the memory made of the read
//! asserted whole.

use verbatim_term_sim::{Flavor, Query, Terminal};

use super::{NewRow, OldRow, Outcome, Read, Switch, TerminalMemory};

/// A terminal watched as the outpost watches it: each read starts where
/// the last said, with the height the memory found, and a read the memory
/// asks for is taken at once.
struct Watch {
    terminal: Terminal,
    memory: TerminalMemory,
    query: Query,
    at_ms: u64,
}

/// Milliseconds between two reads.
const STEP_MS: u64 = 100;

impl Watch {
    /// A terminal of `flavor`, read once before anything is written.
    fn new(flavor: Flavor, width: usize, height: usize, history: usize) -> Self {
        let mut watch = Self {
            terminal: Terminal::new(flavor, width, height, history),
            memory: TerminalMemory::new(),
            query: Query::default(),
            at_ms: 0,
        };
        let first = watch.read();
        assert!(first.first, "the first read is the memory's first");
        watch
    }

    fn console(width: usize, height: usize, history: usize) -> Self {
        Self::new(Flavor::ConsoleHost, width, height, history)
    }

    /// Writes `bytes` and reads.
    fn write(&mut self, bytes: &str) -> Outcome {
        self.terminal.write(bytes.as_bytes());
        self.read()
    }

    /// Reads, reading the whole document when the memory asks.
    fn read(&mut self) -> Outcome {
        self.at_ms += STEP_MS;
        let mut query = self.query;
        loop {
            let snapshot = self.terminal.snapshot(&query).expect("a simulated read");
            let outcome = self.memory.read(&Read {
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
                at_ms: self.at_ms,
            });
            let screen_rows = u32::try_from(outcome.height).expect("a small screen");
            if outcome.whole_document {
                query = Query {
                    start: None,
                    screen_rows,
                    whole: true,
                };
                continue;
            }
            self.query = Query {
                start: Some(snapshot.next_start),
                screen_rows,
                whole: false,
            };
            return outcome;
        }
    }

    /// The texts of the memory's rows.
    fn texts(&self) -> Vec<&str> {
        self.memory
            .rows()
            .iter()
            .map(|row| row.text.as_str())
            .collect()
    }
}

fn kept(from: usize) -> NewRow {
    NewRow::Kept { from, moved: false }
}

fn moved(from: usize) -> NewRow {
    NewRow::Kept { from, moved: true }
}

const NEW: NewRow = NewRow::New;
const GONE: OldRow = OldRow::Gone { lingering: false };

#[test]
fn plain_output_is_new() {
    let mut watch = Watch::console(10, 5, 10);
    let outcome = watch.write("one\r\ntwo\r\n");
    assert_eq!(watch.texts(), ["one", "two", ""]);
    assert_eq!(outcome.rows, [NEW, NEW, NEW]);
    assert_eq!(outcome.old, [GONE]);
    assert_eq!(outcome.rotation, 0);
}

#[test]
fn a_scroll_keeps_the_rows_where_they_are() {
    let mut watch = Watch::console(10, 3, 10);
    watch.write("a\r\nb\r\nc");
    let outcome = watch.write("\r\nd\r\ne");
    assert_eq!(watch.texts(), ["a", "b", "c", "d", "e"]);
    assert_eq!(outcome.rows, [kept(0), kept(1), kept(2), NEW, NEW]);
    assert_eq!(outcome.rotation, 0);
}

#[test]
fn rotation_is_how_far_the_rows_moved_up() {
    // Five rows fill a buffer of five; the read after starts at the third,
    // which after two more rows holds the fifth's text.
    let mut watch = Watch::console(10, 3, 2);
    watch.write("1\r\n2\r\n3\r\n4\r\n5");
    let outcome = watch.write("\r\n6\r\n7");
    assert_eq!(watch.texts(), ["5", "6", "7"]);
    assert_eq!(outcome.rows, [kept(4), NEW, NEW]);
    assert_eq!(
        outcome.old,
        [
            OldRow::Above,
            OldRow::Above,
            OldRow::Above,
            OldRow::Above,
            OldRow::Kept(0)
        ]
    );
    assert_eq!(outcome.rotation, 2);
}

#[test]
fn a_footer_at_its_coordinate_is_kept_while_the_history_rotates() {
    // Output scrolls in a region above a footer on the last row, into a
    // full history: the transcript moves up with the rotation, and the
    // footer, which stays at its coordinate, is kept as moved against it.
    let mut watch = Watch::console(10, 3, 1);
    watch.write("\x1b[3;1Hfooter\x1b[1;2r\x1b[1;1Ha\r\nb\r\nc");
    let outcome = watch.write("\r\nd");
    assert_eq!(watch.texts(), ["c", "d", "footer"]);
    assert_eq!(outcome.rows, [kept(2), NEW, moved(3)]);
    assert_eq!(outcome.rotation, 1);
}

#[test]
fn a_footer_redrawn_in_place_is_rewritten() {
    let mut watch = Watch::console(20, 4, 10);
    watch.write("output\r\n\x1b[4;1Hstatus: busy\x1b[2;1H");
    let outcome = watch.write("\x1b[4;1H\x1b[2Kstatus: done\x1b[2;1H");
    assert_eq!(watch.texts(), ["output", "", "", "status: done"]);
    assert_eq!(
        outcome.rows,
        [
            kept(0),
            kept(1),
            kept(2),
            NewRow::Rewritten {
                from: 3,
                moved: false
            }
        ]
    );
}

#[test]
fn a_status_row_moved_down_a_row_pairs_with_its_old_self() {
    // Pi's regular mode: a streamed line written where the status row was,
    // and the rows below drawn again a row lower, the spinner changed
    // (section 4.3, item 4).
    let mut watch = Watch::console(20, 6, 10);
    watch.write("transcript\r\n\u{280b} Working...\r\n> editor\r\nfooter");
    let outcome =
        watch.write("\x1b[2;1H\x1b[Jstreamed\r\n\u{2819} Working...\r\n> editor\r\nfooter");
    assert_eq!(
        watch.texts(),
        [
            "transcript",
            "streamed",
            "\u{2819} Working...",
            "> editor",
            "footer"
        ]
    );
    assert_eq!(
        outcome.rows,
        [
            kept(0),
            NEW,
            NewRow::Rewritten {
                from: 1,
                moved: true
            },
            moved(2),
            moved(3)
        ]
    );
}

#[test]
fn an_ink_region_erased_and_drawn_again_at_once_is_kept() {
    let mut watch = Watch::console(20, 6, 10);
    watch.write("------\r\n> input\r\ncost $1\r\n");
    let outcome = watch.write("\x1b[3A\x1b[J------\r\n> input\r\ncost $1\r\n");
    assert_eq!(outcome.rows, [kept(0), kept(1), kept(2), kept(3)]);
}

#[test]
fn an_ink_region_caught_erased_lingers_and_is_taken_back() {
    let mut watch = Watch::console(20, 6, 10);
    watch.write("------\r\n> input\r\ncost $1\r\n");
    let erased = watch.write("\x1b[3A\x1b[J");
    assert_eq!(
        erased.old,
        [
            OldRow::Gone { lingering: true },
            OldRow::Gone { lingering: true },
            OldRow::Gone { lingering: true },
            GONE,
        ]
    );
    let back = watch.write("------\r\n> input\r\ncost $2\r\n");
    assert_eq!(watch.texts(), ["------", "> input", "cost $2", ""]);
    assert_eq!(
        back.rows,
        [
            NewRow::Returned { moved: false },
            NewRow::Returned { moved: false },
            NEW,
            NEW,
        ]
    );
}

#[test]
fn a_repeated_row_is_kept_in_place_rather_than_paired_with_a_copy() {
    // The rule could be kept as the new last row too; that moves it, in a
    // second block, so the copy in place is kept (section 4.3, item 3).
    let mut watch = Watch::console(10, 5, 10);
    watch.write("x\r\n----");
    let outcome = watch.write("\r\n----");
    assert_eq!(watch.texts(), ["x", "----", "----"]);
    assert_eq!(outcome.rows, [kept(0), kept(1), NEW]);
}

#[test]
fn a_wrapped_line_split_across_the_reads_top_grows() {
    // A line of three rows on a screen of two: the read starts at its
    // second row, which continues it from above.
    let mut watch = Watch::console(4, 2, 10);
    watch.write("abcdefghijkl");
    let outcome = watch.write("mn");
    assert_eq!(watch.texts(), ["efgh", "ijkl", "mn"]);
    assert!(
        outcome.continued,
        "the first row continues a line from above"
    );
    assert_eq!(outcome.rows, [kept(1), kept(2), NEW]);
    assert_eq!(outcome.lines.len(), 1);
    assert_eq!(outcome.lines[0].text, "efghijklmn");
    assert_eq!(outcome.lines[0].grew_from, Some(8));
}

#[test]
fn windows_terminals_last_row_cut_a_cell_short_keeps_the_width() {
    // With no history the buffer's last row is the screen's, and Windows
    // Terminal gives it one cell short (section 2.1).
    let mut watch = Watch::new(Flavor::WindowsTerminal, 6, 3, 0);
    let outcome = watch.write("abc\r\ndef\r\nghi");
    assert_eq!((outcome.width, outcome.height), (6, 3));
    assert_eq!(watch.texts(), ["abc", "def", "ghi"]);
}

#[test]
fn a_clear_and_a_reprint_repeat_the_known_history() {
    // Pi's regular mode after a change above the screen: the screen and
    // the history cleared and everything printed again in one frame, a
    // line above the screen taken out, so the text shrank and the whole
    // document is read; the line above the old screen is a reprint.
    let mut watch = Watch::console(10, 3, 20);
    watch.write("h1\r\nh2\r\nh3\r\nh4\r\nh5");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jh1\r\nh3\r\nh4\r\nh5");
    assert_eq!(watch.texts(), ["h1", "h3", "h4", "h5"]);
    assert_eq!(outcome.rows, [NewRow::Reprint, kept(2), kept(3), kept(4)]);
}

#[test]
fn output_repeated_after_cls_is_not_a_reprint() {
    // The console host's cls pushes the screen into the history, so the
    // output printed again is below the old screen, and new.
    let mut watch = Watch::console(10, 3, 20);
    watch.write("ls1\r\nls2\r\n");
    watch.write("\x1b[H\x1b[2J");
    let outcome = watch.write("ls1\r\nls2\r\n");
    assert_eq!(watch.texts(), ["ls1", "ls2", "ls1", "ls2", ""]);
    assert_eq!(outcome.rows, [kept(0), kept(1), NEW, NEW, NEW]);
}

#[test]
fn the_alternate_screen_opened_and_closed_restores_the_main_memory() {
    let mut watch = Watch::console(10, 3, 20);
    watch.write("m1\r\nm2\r\nm3\r\nm4");
    let opened = watch.write("\x1b[?1049h\x1b[Hvim");
    assert_eq!(opened.switch, Switch::Opened);
    assert_eq!(watch.texts(), ["vim"]);
    assert_eq!(opened.rows, [NEW]);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(closed.switch, Switch::Closed);
    // The read starts where the alternate screen's read said, its first
    // row, which on the main screen is the document's first.
    assert_eq!(watch.texts(), ["m1", "m2", "m3", "m4"]);
    assert_eq!(closed.rows, [kept(0), kept(1), kept(2), kept(3)]);
}

#[test]
fn the_review_position_stays_on_its_text_through_a_scroll_a_rotation_and_a_redraw() {
    let mut watch = Watch::console(10, 3, 2);
    watch.write("a\r\nb\r\nc");
    watch.memory.set_review(1, 0);
    watch.write("\r\nd");
    let review = |watch: &Watch| watch.memory.review().map(|anchor| anchor.row);
    assert_eq!(review(&watch), Some(1), "scrolled: still on b");
    watch.write("\r\ne\r\nf");
    assert_eq!(watch.texts(), ["c", "d", "e", "f"]);
    assert_eq!(
        review(&watch),
        Some(-1),
        "rotated: b is a row above the read"
    );
    watch.memory.set_review(1, 0);
    watch.write("\x1b[3;1Hg");
    assert_eq!(watch.texts(), ["d", "e", "g"]);
    assert_eq!(review(&watch), Some(0), "a redraw elsewhere: still on d");
}
