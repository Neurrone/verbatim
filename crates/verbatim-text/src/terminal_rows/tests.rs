//! The memory and the alignment, one case of `docs/design/terminal-model.md`,
//! "The read", "The alignment", "Lingering", "Screens" and "Starting
//! again", each: a stream written into the simulated terminal, read as the
//! outpost reads it, and what the memory made of the read asserted whole.

use verbatim_term_sim::{Flavor, Query, Terminal};

use super::{Anchor, NewRow, OldRow, Outcome, Read, Switch, TerminalMemory, hash};

/// A terminal watched as the outpost watches it: each read starts where
/// the last said, a screen above it, with the height the memory found, and
/// the whole document is read at once when the memory asks.
struct Watch {
    terminal: Terminal,
    memory: TerminalMemory,
    query: Query,
    at_ms: u64,
    /// How many snapshots the last read took.
    taken: usize,
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
            taken: 0,
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
        self.taken = 0;
        loop {
            let snapshot = self.terminal.snapshot(&query).expect("a simulated read");
            self.taken += 1;
            let read = Read {
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
                above: usize::try_from(snapshot.above).expect("a few rows"),
            };
            let outcome = self.memory.read(&read);
            let screen_rows = u32::try_from(outcome.height).expect("a small screen");
            if outcome.whole_document {
                assert_eq!(self.taken, 1, "the whole document is read at most once");
                query = Query {
                    start: None,
                    screen_rows,
                    whole: true,
                    above: 0,
                };
                continue;
            }
            self.query = Query {
                start: Some(snapshot.next_start),
                screen_rows,
                whole: false,
                above: u32::try_from(self.memory.above()).expect("a small screen"),
            };
            return outcome;
        }
    }

    /// The texts of the memory's rows.
    fn texts(&self) -> Vec<&str> {
        self.memory
            .rows()
            .kept()
            .iter()
            .map(|row| &*row.text)
            .collect()
    }
}

fn kept_row(to: usize) -> OldRow {
    OldRow::Kept(to)
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
    // less the block of the two rows above it, which after two more rows
    // holds the third's text: the block left the buffer.
    let mut watch = Watch::console(10, 3, 2);
    watch.write("1\r\n2\r\n3\r\n4\r\n5");
    let outcome = watch.write("\r\n6\r\n7");
    assert_eq!(watch.texts(), ["3", "4", "5", "6", "7"]);
    assert_eq!(outcome.rows, [kept(2), kept(3), kept(4), NEW, NEW]);
    assert_eq!(
        outcome.old,
        [
            OldRow::Left,
            OldRow::Left,
            OldRow::Kept(0),
            OldRow::Kept(1),
            OldRow::Kept(2)
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
    assert_eq!(watch.texts(), ["b", "c", "d", "footer"]);
    assert_eq!(outcome.rows, [kept(1), kept(2), NEW, moved(3)]);
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
                moved: false,
                alone: false,
            }
        ]
    );
}

#[test]
fn a_status_row_moved_down_a_row_pairs_with_its_old_self() {
    // Pi's regular mode: a streamed line written where the status row was,
    // and the rows below drawn again a row lower, the spinner changed: the
    // status row is paired with its old self between the kept rows
    // ("The alignment").
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
                moved: true,
                alone: false,
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
    // The region below a line of finished output, caught erased by a read
    // and drawn again by the next: its rows, at and below the caret, linger
    // and come back.
    let mut watch = Watch::console(20, 6, 10);
    watch.write("done\r\n------\r\n> input\r\ncost $1\r\n");
    let erased = watch.write("\x1b[3A\x1b[J");
    assert_eq!(
        erased.old,
        [
            kept_row(0),
            OldRow::Gone { lingering: true },
            OldRow::Gone { lingering: true },
            OldRow::Gone { lingering: true },
            GONE,
        ]
    );
    let back = watch.write("------\r\n> input\r\ncost $2\r\n");
    assert_eq!(watch.texts(), ["done", "------", "> input", "cost $2", ""]);
    assert_eq!(
        back.rows,
        [
            kept(0),
            NewRow::Returned { moved: false },
            NewRow::Returned { moved: false },
            NEW,
            NEW,
        ]
    );
}

#[test]
fn a_read_whose_screen_shows_no_text_changes_nothing_and_the_screen_comes_back() {
    // Ink's region erased whole, so the screen shows no text: the read
    // changes nothing, and the region drawn again is aligned with the
    // memory as it was, the cost rewritten ("A read with no text").
    let mut watch = Watch::console(20, 6, 10);
    watch.write("------\r\n> input\r\ncost $1\r\n");
    let erased = watch.write("\x1b[3A\x1b[J");
    assert!(erased.blank, "the screen shows no text");
    assert_eq!(watch.texts(), ["------", "> input", "cost $1", ""]);
    let back = watch.write("------\r\n> input\r\ncost $2\r\n");
    assert_eq!(watch.texts(), ["------", "> input", "cost $2", ""]);
    assert_eq!(
        back.rows,
        [
            kept(0),
            kept(1),
            NewRow::Rewritten {
                from: 2,
                moved: false,
                alone: false,
            },
            kept(3),
        ]
    );
}

#[test]
fn blank_lines_printed_to_clear_the_screen_change_nothing_and_output_after_them_is_new() {
    // A script that clears by printing a screen of blank lines: that read
    // shows no text and changes nothing, so the output after it is aligned
    // with the memory before the blank lines ("A read with no text").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("a\r\nb\r\nc\r\nd");
    let cleared = watch.write("\r\n\r\n\r\n");
    assert!(cleared.blank, "the screen shows no text");
    let outcome = watch.write("e");
    assert_eq!(watch.texts(), ["b", "c", "d", "", "", "e"]);
    assert_eq!(outcome.rows, [kept(1), kept(2), kept(3), NEW, NEW, NEW]);
}

#[test]
fn a_repeated_row_is_kept_in_place_rather_than_paired_with_a_copy() {
    // The rule could be kept as the new last row too; that moves it, in a
    // second block, so the copy in place is kept ("The alignment").
    let mut watch = Watch::console(10, 5, 10);
    watch.write("x\r\n----");
    let outcome = watch.write("\r\n----");
    assert_eq!(watch.texts(), ["x", "----", "----"]);
    assert_eq!(outcome.rows, [kept(0), kept(1), NEW]);
}

#[test]
fn a_row_moved_past_others_is_kept_where_its_text_went() {
    // A list drawn again with its first entry last: the longest common
    // subsequence keeps the entries that did not cross it, and the entry
    // whose text is on no other row is kept where that text went.
    let mut watch = Watch::console(10, 5, 10);
    watch.write("x\r\na\r\nb\r\nc");
    let outcome = watch.write("\x1b[2;1Hb\x1b[K\r\nc\x1b[K\r\na\x1b[K");
    assert_eq!(watch.texts(), ["x", "b", "c", "a"]);
    assert_eq!(outcome.rows, [kept(0), moved(2), moved(3), moved(1)]);
    assert_eq!(
        outcome.old,
        [
            OldRow::Kept(0),
            OldRow::Kept(3),
            OldRow::Kept(1),
            OldRow::Kept(2)
        ]
    );
}

#[test]
fn a_wrapped_line_split_across_the_reads_top_grows() {
    // A line of seven rows on a screen of two: the read starts at its
    // sixth row, less a screen above it, so at its fourth row, which
    // continues it from above.
    let mut watch = Watch::console(4, 2, 10);
    watch.write("abcdefghijklmnopqrstuvwxyz12");
    let outcome = watch.write("34");
    assert_eq!(watch.texts(), ["mnop", "qrst", "uvwx", "yz12", "34"]);
    assert!(
        outcome.continued,
        "the first row continues a line from above"
    );
    assert_eq!(outcome.rows, [kept(3), kept(4), kept(5), kept(6), NEW]);
    assert_eq!(outcome.lines.len(), 1);
    assert_eq!(&*outcome.lines[0].text, "mnopqrstuvwxyz1234");
    assert_eq!(outcome.lines[0].grew_from, Some(16));
}

#[test]
fn windows_terminals_last_row_cut_a_cell_short_keeps_the_width() {
    // With no history the buffer's last row is the screen's, and Windows
    // Terminal gives it one cell short ("The terminals").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 6, 3, 0);
    let outcome = watch.write("abc\r\ndef\r\nghi");
    assert_eq!((outcome.width, outcome.height), (6, 3));
    assert_eq!(watch.texts(), ["abc", "def", "ghi"]);
}

#[test]
fn a_clear_and_a_reprint_repeat_the_known_history() {
    // Pi's regular mode after a change above the screen: the screen and
    // the history cleared and everything printed again in one frame, a
    // line above the screen taken out, so the start is refused and the
    // whole document is read. The rows the remembered history held are
    // shown, not new, and the last read's rows are kept where most of them
    // went ("The alignment").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("h1\r\nh2\r\nh3\r\nh4\r\nh5\r\nh6\r\nh7\r\nh8\r\nh9");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jh1\r\nh3\r\nh4\r\nh5\r\nh6\r\nh7\r\nh8\r\nh9");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(
        watch.texts(),
        ["h1", "h3", "h4", "h5", "h6", "h7", "h8", "h9"]
    );
    assert_eq!(
        outcome.rows,
        [
            NewRow::Shown,
            NewRow::Shown,
            kept(3),
            kept(4),
            kept(5),
            kept(6),
            kept(7),
            kept(8)
        ]
    );
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
fn the_alternate_screen_opened_and_closed_is_a_new_screen_each_way() {
    // A full-screen program draws the alternate screen: nothing of the
    // shell's screen survives, so it is a new screen. Closing it brings the
    // main screen back, which keeps nothing of the program's screen: a new
    // screen too, its visible rows new and the row above them history,
    // unspoken ("Screens").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("m1\r\nm2\r\nm3\r\nm4");
    let opened = watch.write("\x1b[?1049h\x1b[Hvim\x1b[3;1H\"f\" 4L\x1b[1;1H");
    assert_eq!(opened.switch, Switch::New);
    assert_eq!(watch.texts(), ["vim", "", "\"f\" 4L"]);
    assert_eq!(opened.rows, [NEW, NEW, NEW]);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(watch.texts(), ["m1", "m2", "m3", "m4"]);
    assert_eq!(closed.rows, [NewRow::Shown, NEW, NEW, NEW]);
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
    assert_eq!(watch.texts(), ["b", "c", "d", "e", "f"]);
    assert_eq!(
        review(&watch),
        Some(0),
        "rotated: b is the first row of the screen above the screen"
    );
    watch.memory.set_review(2, 0);
    watch.write("\x1b[3;1Hg");
    assert_eq!(watch.texts(), ["b", "c", "d", "e", "g"]);
    assert_eq!(review(&watch), Some(2), "a redraw elsewhere: still on d");
}

#[test]
fn a_rotation_between_one_and_two_screens_is_settled_in_one_read() {
    // A full history of 1 to 9 on a screen of three rotates by four, more
    // than a screen: the read, from a screen above the last read's screen,
    // still holds the last two rows of that screen, and the rows of the
    // history above the screen allow no smaller rotation ("The read").
    let mut watch = Watch::console(10, 3, 6);
    watch.write("1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9");
    let outcome = watch.write("\r\na\r\nb\r\nc\r\nd");
    assert_eq!(watch.taken, 1, "one read");
    assert_eq!(outcome.rotation, 4);
    assert!(!outcome.overflowed, "the history did not overflow");
    assert_eq!(watch.texts(), ["8", "9", "a", "b", "c", "d"]);
    assert_eq!(outcome.rows, [kept(7), kept(8), NEW, NEW, NEW, NEW]);
    let mut old = vec![OldRow::Above; 7];
    old.extend([OldRow::Kept(0), OldRow::Kept(1)]);
    assert_eq!(outcome.old, old);
}

#[test]
fn a_footer_over_more_output_than_the_history_holds_overflows() {
    // The whole document keeps no row of the last read at or above the
    // caret, and nothing of the remembered history, in a full buffer:
    // output may have been lost ("Screens"). The footer below the caret
    // stays kept and silent, and the caret's line, its first row where it
    // was, counts as rewritten; the rest is new.
    let mut watch = Watch::console(10, 3, 6);
    watch.write("\x1b[3;1Hfooter\x1b[1;2r\x1b[1;1Ha\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh");
    let outcome = watch.write("\r\nn\r\no\r\np\r\nq\r\nr\r\ns\r\nt\r\nu\r\nv\r\nw\r\nx\r\ny");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(
        watch.texts(),
        ["r", "s", "t", "u", "v", "w", "x", "y", "footer"]
    );
    assert!(outcome.overflowed, "more went by than the history holds");
    assert_eq!(
        outcome.rows,
        [
            NEW,
            NEW,
            NEW,
            NEW,
            NEW,
            NEW,
            NEW,
            NewRow::Rewritten {
                from: 7,
                moved: false,
                alone: false,
            },
            kept(8)
        ]
    );
}

#[test]
fn a_history_erase_keeps_the_screens_rows() {
    // Erasing the history refuses the read's start, so the whole document
    // is read, and it keeps the screen's rows: no screen changed, and the
    // output after it is new ("Screens").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("m1\r\nm2\r\nm3\r\nm4");
    let erased = watch.write("\x1b[3J");
    assert_eq!(erased.switch, Switch::None);
    assert_eq!(watch.texts(), ["m2", "m3", "m4"]);
    assert_eq!(erased.rows, [kept(1), kept(2), kept(3)]);
    let scrolled = watch.write("\r\nn1");
    assert_eq!(scrolled.switch, Switch::None);
    assert_eq!(watch.texts(), ["m2", "m3", "m4", "n1"]);
    assert_eq!(scrolled.rows, [kept(0), kept(1), kept(2), NEW]);
}

#[test]
fn a_pagers_first_page_is_a_new_screen_and_closing_it_speaks_the_shell() {
    // `conhost_pager`'s shape: the pager's first page keeps no row of the
    // shell's screen, so it is a new screen; its second page pairs each row
    // with the first page's by likeness, so it is no new screen; and the
    // main screen back leaves the caret's line where the pager's status row
    // was, so that row counts as rewritten and the rest of the shell's
    // screen is new, spoken as NVDA speaks it (`nv-con-pager.txt`;
    // "Screens").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("PS>pager\r\n");
    let opened = watch.write("\x1b[?1049h\x1b[H\x1b[2Jp1\r\np2\r\n:");
    assert_eq!(opened.switch, Switch::New);
    assert_eq!(opened.rows, [NEW, NEW, NEW]);
    let paged = watch.write("\x1b[H\x1b[2Jq1\r\nq2\r\n(END)");
    assert_eq!(paged.switch, Switch::None);
    let rewritten = |from| NewRow::Rewritten {
        from,
        moved: false,
        alone: false,
    };
    assert_eq!(paged.rows, [rewritten(0), rewritten(1), rewritten(2)]);
    let closed = watch.write("\x1b[?1049lclosed\r\nPS>");
    assert_eq!(closed.switch, Switch::None);
    assert_eq!(watch.texts(), ["PS>pager", "closed", "PS>"]);
    assert_eq!(closed.rows, [NEW, NEW, rewritten(2)]);
}

#[test]
fn a_scroll_on_the_alternate_screen_is_no_rotation() {
    // The alternate screen has no history ("Screens"), so a scroll
    // rotates nothing: the rows it moved up are kept, moved, and the row
    // it scrolled off the top is gone. The review position on that row
    // goes to the caret, since no row above it survived ("The review anchor and following").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("\x1b[?1049h\x1b[Ha\r\nb\r\nc");
    watch.memory.set_review(0, 0);
    let outcome = watch.write("\r\nd");
    assert_eq!(watch.texts(), ["b", "c", "d"]);
    assert_eq!(outcome.rows, [moved(1), moved(2), NEW]);
    assert_eq!(outcome.old, [GONE, OldRow::Kept(0), OldRow::Kept(1)]);
    assert_eq!(outcome.rotation, 0);
    assert_eq!(
        watch.memory.review(),
        Some(Anchor {
            row: 2,
            column: 1,
            line_hash: hash("d")
        })
    );
}

#[test]
fn the_review_position_on_a_gone_row_goes_to_the_nearest_surviving_row_above() {
    // The row it was on erased ("The review anchor and following"), it moves to the row above.
    let mut watch = Watch::console(10, 5, 10);
    watch.write("x\r\na\r\nb");
    watch.memory.set_review(1, 0);
    watch.write("\x1b[2;1H\x1b[2K");
    assert_eq!(watch.texts(), ["x", "", "b"]);
    assert_eq!(
        watch.memory.review(),
        Some(Anchor {
            row: 0,
            column: 0,
            line_hash: hash("x")
        })
    );
}

#[test]
fn the_known_history_moves_up_with_the_rotation() {
    // The remembered history is kept by coordinate and moved up as the
    // history rotates ("The memory"). In a buffer of nine rows, read after
    // each line, the memory keeps the screen and a screen above it, six
    // rows, so the remembered history holds the three rows above them and
    // no row that left.
    let mut watch = Watch::console(10, 3, 6);
    watch.write("1");
    for number in 2..=30 {
        watch.write(&format!("\r\n{number}"));
    }
    assert_eq!(watch.texts(), ["25", "26", "27", "28", "29", "30"]);
    assert_eq!(
        watch.memory.known_history(),
        [hash("22"), hash("23"), hash("24")]
    );
}

#[test]
fn a_whole_document_read_takes_the_known_history_from_the_document() {
    // Thirty lines read one at a time through a buffer of thirteen rows,
    // then a flood of twenty, read whole: the remembered history is the
    // rows the document holds above the memory's, 38 to 44, and none from
    // before them.
    let mut watch = Watch::console(10, 3, 10);
    watch.write("1");
    for number in 2..=30 {
        watch.write(&format!("\r\n{number}"));
    }
    let flood: String = (31..=50)
        .flat_map(|number| ["\r\n".to_owned(), number.to_string()])
        .collect();
    watch.write(&flood);
    assert_eq!(watch.taken, 2, "the flood read whole");
    watch.memory.trim();
    assert_eq!(watch.texts(), ["45", "46", "47", "48", "49", "50"]);
    let above: Vec<u64> = (38..=44).map(|number| hash(&number.to_string())).collect();
    assert_eq!(watch.memory.known_history(), above);
}

#[test]
fn output_repeated_after_a_clear_that_lost_the_old_screen_is_new() {
    // Windows Terminal: `type f.txt`, then `cls` and other output within
    // one read. The clear takes the history too, so the whole document is
    // read, and it keeps no row of the last read: a new screen. The clear
    // erased the history, so the remembered history is gone too, and every
    // row is new, though some repeat what it held ("Screens").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("alpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    watch.write("1111\r\n2222\r\n3333\r\n4444");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jalpha\r\nbravo\r\nzulu\r\n");
    assert_eq!(watch.texts(), ["alpha", "bravo", "zulu", ""]);
    assert_eq!(outcome.switch, Switch::New);
    assert_eq!(outcome.rows, [NEW, NEW, NEW, NEW]);
}

#[test]
fn output_repeated_after_a_clear_that_erases_rows_of_the_last_read_speaks_the_rows_above_the_screen()
 {
    // Windows Terminal: `type f.txt`, more output, then `cls` and `type
    // f.txt` within one read. The clear erased the history, so the rows
    // above the screen match no remembered history and are new; the
    // visible screen's rows are aligned with the last read, which had them
    // ("Screens").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("alpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    watch.write("1111\r\n2222\r\n3333\r\n4444");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jalpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    assert_eq!(watch.texts(), ["alpha", "bravo", "charlie", "delta", ""]);
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(outcome.rows, [NEW, NEW, kept(2), kept(3), NEW]);
}

#[test]
fn a_footer_below_a_cleared_screen_pushed_into_the_history_does_not_overflow() {
    // `conhost_footer_overflow`'s shape: the console host pushes the
    // cleared screen into the history, a footer is drawn on the last row
    // under a scroll region, and the first line of output above it scrolls
    // the region into the history, its rows staying where they were, and
    // moves the footer down a row. The rows before the clear are kept, so
    // nothing went by unseen ("The read").
    let mut watch = Watch::console(20, 5, 50);
    watch.write("PS> one\r\nout\r\nPS> run\r\n");
    watch.write("\x1b[2J\x1b[1;4r\x1b[5;1Hstatus: busy\x1b[4;1H");
    let outcome = watch.write("\r\nflood line 1");
    assert_eq!(
        watch.texts(),
        [
            "PS> one",
            "out",
            "PS> run",
            "",
            "",
            "",
            "",
            "flood line 1",
            "status: busy"
        ]
    );
    assert_eq!(
        outcome.rows,
        [
            kept(0),
            kept(1),
            kept(2),
            kept(3),
            kept(4),
            kept(5),
            kept(6),
            NEW,
            moved(7)
        ]
    );
    assert_eq!(outcome.rotation, 0);
    assert!(!outcome.overflowed, "the history is far from full");
}

#[test]
fn a_row_paired_alone_by_likeness_elsewhere_starts_its_change_history_afresh() {
    // A status row below a kept row ticks twice, then is written over in
    // place with a line of output and the next tick is printed a row
    // lower: the alignment pairs the tick with the old status row, which
    // moved alone, with no neighbour kept in the same move, so its change
    // history starts afresh ("The alignment").
    let mut watch = Watch::console(12, 5, 10);
    watch.write("x\r\n1s");
    watch.write("\r2s");
    watch.write("\r3s");
    let outcome = watch.write("\rok\r\n4s");
    assert_eq!(watch.texts(), ["x", "ok", "4s"]);
    assert_eq!(
        outcome.rows,
        [
            kept(0),
            NEW,
            NewRow::Rewritten {
                from: 1,
                moved: true,
                alone: true,
            }
        ]
    );
}

#[test]
fn a_lingering_row_is_taken_back_in_another_line() {
    // A wrapped line's last row is erased, so it lingers, and its text is
    // printed again as a line of its own within the lingering time: the
    // nearest row with its text is the row taken back, with no condition on
    // its line (Dickson, 2026-10-24; "Lingering").
    let mut watch = Watch::console(10, 5, 10);
    watch.write("abcdefghijxyz");
    let erased = watch.write("\x1b[2K");
    assert_eq!(erased.old, [kept_row(0), OldRow::Gone { lingering: true }]);
    let outcome = watch.write("\r\nxyz");
    assert_eq!(watch.texts(), ["abcdefghij", "", "xyz"]);
    assert_eq!(outcome.rows[2], NewRow::Returned { moved: true });
}

#[test]
fn a_screen_cleared_into_the_history_over_repeated_text_keeps_its_rows_in_order() {
    // Windows Terminal clears a screen holding "hello" twice into a full
    // history whose block above the screen is blank rows, which tell no
    // rotation: the old screen's rows, paired in order as well as by text
    // ("The alignment"), give it, and each "hello" is kept as the copy in its
    // own place in the order, none of them new.
    let mut watch = Watch::new(Flavor::WindowsTerminal, 12, 5, 6);
    watch.write("\r\n");
    watch.write("\r\n\r\n\r\nhello\r\n~hello\r\n\r\nhello");
    assert_eq!(
        watch.texts(),
        ["", "", "", "", "hello", "~hello", "", "hello"]
    );
    let outcome = watch.write("\x1b[H\x1b[2Ja b\r\ncat\r\n");
    assert_eq!(
        watch.texts(),
        ["", "", "hello", "~hello", "", "hello", "a b", "cat", ""]
    );
    assert_eq!(outcome.rotation, 2);
    assert_eq!(
        outcome.rows,
        [
            kept(2),
            kept(3),
            kept(4),
            kept(5),
            kept(6),
            kept(7),
            NEW,
            NEW,
            NEW
        ]
    );
}

#[test]
fn a_tie_between_rotations_is_settled_by_the_documents_first_row() {
    // A full history of one repeated line under a first line "a": the rows
    // the read shows allow every rotation, and the document's first row,
    // "x" where the remembered history held "a" at the document's start and
    // "x" a row below, settles the rotation of one ("The alignment").
    let mut watch = Watch::console(10, 3, 6);
    watch.write("a\r\nx\r\nx\r\nx\r\nx\r\nx\r\nx\r\nx\r\nx");
    let outcome = watch.write("\r\nx");
    assert_eq!(watch.taken, 1, "one read");
    assert_eq!(outcome.rotation, 1);
    assert_eq!(watch.texts(), ["x", "x", "x", "x", "x", "x"]);
    assert_eq!(
        outcome.rows,
        [kept(4), kept(5), kept(6), kept(7), kept(8), NEW]
    );
}

#[test]
fn windows_terminals_clear_and_a_full_screen_program_are_each_a_new_screen() {
    // The clear refuses the read's start and keeps nothing: a new screen,
    // the prompt new. The full-screen program's screen and the screen after
    // the clear coming back are new screens too ("Screens").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("a\r\nb\r\nc\r\nd\r\ne");
    let cleared = watch.write("\x1b[H\x1b[2J\x1b[3Jp> ");
    assert_eq!(cleared.switch, Switch::New);
    assert_eq!(cleared.rows, [NEW]);
    let opened = watch.write("\x1b[?1049h\x1b[Hvim\r\n~\r\n:q");
    assert_eq!(opened.switch, Switch::New);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(watch.texts(), ["p>"]);
    assert_eq!(closed.rows, [NEW]);
}

#[test]
fn the_alternate_screen_from_a_shell_as_long_as_it_is_a_new_screen_not_an_overflow() {
    // A fresh shell's document fills the screen, with no history, and the
    // alternate screen is as long, its cursor homed as a full-screen
    // program's is: nothing of the shell's screen survives, so it is a new
    // screen, and the document did not move, so it is no overflow; the
    // shell's screen coming back is a new screen again ("Screens").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("$ ls\r\nfile\r\n$ vim");
    let opened = watch.write("\x1b[?1049h\x1b[Hhello\r\n~\r\n:q\x1b[H");
    assert_eq!(opened.switch, Switch::New);
    assert!(!opened.overflowed, "no overflow");
    assert_eq!(opened.rows, [NEW, NEW, NEW]);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(closed.rows, [NEW, NEW, NEW]);
}

#[test]
fn a_command_mark_survives_a_full_screen_program() {
    // A mark is kept by its line's hash: lost while a full-screen program
    // shows no row of its line, it is found again when the shell's screen
    // comes back ("The review cursor").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("$ ls\r\nfile\r\n$ vim");
    watch.memory.add_mark(0);
    watch.write("\x1b[?1049h\x1b[Hhello\r\n~\r\n:q\x1b[H");
    assert_eq!(watch.memory.marks(), []);
    watch.write("\x1b[?1049l");
    assert_eq!(
        watch.memory.marks(),
        [Anchor {
            row: 0,
            column: 0,
            line_hash: hash("$ ls")
        }]
    );
}
