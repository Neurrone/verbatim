//! The memory and the alignment, one case of `docs/design/terminal-model.md`,
//! "The read", "The alignment", "Lingering", "Screens" and "Starting
//! again", each: a stream written into the simulated terminal, read as the
//! outpost reads it, and what the memory made of the read asserted whole.

use verbatim_term_sim::{Flavor, Query, Terminal};

use super::{Anchor, Lost, NewRow, OldRow, Outcome, Read, ReadText, Switch, TerminalMemory, hash};

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
fn a_box_of_two_equal_borders_redrawn_lower_past_a_row_keeps_both_borders() {
    // An input box sent: its line printed above it as it was, and the box
    // drawn again lower. The subsequence keeps one border, after the line;
    // the other, which crossed the line, is kept at the nearest new row
    // with its text, though the text is on two rows ("The alignment").
    let mut watch = Watch::console(10, 8, 10);
    watch.write("x\r\n====\r\n> a\r\n  b\r\n====\x1b[3;4H");
    let outcome = watch.write("\x1b[2;1H\x1b[J> a\r\n  b\r\n* y\r\n====\r\n>\r\n====\x1b[6;2H");
    assert_eq!(
        watch.texts(),
        ["x", "> a", "  b", "* y", "====", ">", "===="]
    );
    assert_eq!(
        outcome.rows,
        [kept(0), moved(2), moved(3), NEW, kept(4), NEW, moved(1)]
    );
    assert_eq!(
        outcome.old,
        [
            kept_row(0),
            kept_row(6),
            kept_row(1),
            kept_row(2),
            kept_row(4)
        ]
    );
}

#[test]
fn a_row_kept_by_its_text_goes_to_the_nearest_copy() {
    // The rule moved past the two rows below it, and a copy of it printed
    // further down: the subsequence keeps the rows it crossed, and the rule
    // is kept at the copy nearer its place.
    let mut watch = Watch::console(10, 8, 10);
    watch.write("x\r\n-\r\na\r\nb");
    let outcome = watch.write("\x1b[2;1H\x1b[Ja\r\nb\r\n-\r\ny\r\n-");
    assert_eq!(watch.texts(), ["x", "a", "b", "-", "y", "-"]);
    assert_eq!(
        outcome.rows,
        [kept(0), moved(2), moved(3), moved(1), NEW, NEW]
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
fn a_clear_and_a_reprint_keep_the_last_reads_rows() {
    // Pi's regular mode after a change above the screen: the screen and
    // the history cleared and everything printed again in one frame, a
    // line above the screen taken out. The document shrank below the
    // read's start, which is refused, so the read starts a screen above the
    // screen, and the last read's rows are found by their text, all kept
    // and silent ("The read", "Positions").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("h1\r\nh2\r\nh3\r\nh4\r\nh5\r\nh6\r\nh7\r\nh8\r\nh9");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jh1\r\nh3\r\nh4\r\nh5\r\nh6\r\nh7\r\nh8\r\nh9");
    assert_eq!(watch.taken, 1, "one read");
    assert_eq!(watch.texts(), ["h4", "h5", "h6", "h7", "h8", "h9"]);
    assert_eq!(
        outcome.rows,
        [kept(3), kept(4), kept(5), kept(6), kept(7), kept(8)]
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
    assert!(outcome.lost.is_none(), "the history did not overflow");
    assert_eq!(watch.texts(), ["8", "9", "a", "b", "c", "d"]);
    assert_eq!(outcome.rows, [kept(7), kept(8), NEW, NEW, NEW, NEW]);
    let mut old = vec![OldRow::Above; 7];
    old.extend([OldRow::Kept(0), OldRow::Kept(1)]);
    assert_eq!(outcome.old, old);
}

#[test]
fn a_footer_over_more_output_than_the_history_holds_overflows() {
    // The whole document keeps no row of the last read at or above the
    // caret, so the read is lost and the whole document is read; nothing
    // the last read had at or above the caret is in it, so every row above
    // the visible screen is new, counted as more than ("A lost read"); the
    // footer below the caret stays kept and silent.
    let mut watch = Watch::console(10, 3, 6);
    watch.write("\x1b[3;1Hfooter\x1b[1;2r\x1b[1;1Ha\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh");
    let outcome = watch.write("\r\nn\r\no\r\np\r\nq\r\nr\r\ns\r\nt\r\nu\r\nv\r\nw\r\nx\r\ny");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(
        watch.texts(),
        ["r", "s", "t", "u", "v", "w", "x", "y", "footer"]
    );
    assert!(
        outcome.lost == Some(Lost::MoreThan),
        "more went by than the history holds"
    );
    assert_eq!(
        outcome.rows,
        [NEW, NEW, NEW, NEW, NEW, NEW, NEW, NEW, kept(8)]
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
    // shell's screen, so it is a new screen; its second page keeps its
    // caret's line, the status row, where it was, so that row counts as
    // rewritten and the page is no new screen; and the main screen back
    // leaves the caret's line where the pager's status row was, so that row
    // counts as rewritten and the rest of the shell's screen is new, spoken
    // as NVDA speaks it (`nv-con-pager.txt`; "Screens").
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
    assert_eq!(paged.rows, [NEW, NEW, rewritten(2)]);
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
fn output_repeated_after_a_clear_that_lost_the_old_screen_is_new() {
    // Windows Terminal: `type f.txt`, then `cls` and other output within
    // one read. The clear takes the history too and shrinks the document,
    // refusing the read's start; the read keeps no row of the last read,
    // its screen does not start at the document's start and the aside
    // screen is not in it, so it is lost and the whole document read, which
    // holds nothing of the last read: every row is new, counted as about
    // ("Screens", "A lost read").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("alpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    watch.write("1111\r\n2222\r\n3333\r\n4444");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jalpha\r\nbravo\r\nzulu\r\n");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(watch.texts(), ["alpha", "bravo", "zulu", ""]);
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(outcome.lost, Some(Lost::About));
    assert_eq!(outcome.rows, [NEW; 4]);
}

#[test]
fn output_repeated_after_a_clear_that_erases_rows_of_the_last_read_is_silent() {
    // Windows Terminal: `type f.txt`, more output, then `cls` and `type
    // f.txt` within one read. The rows above the screen repeat the
    // remembered history from the document's first row, and the visible
    // rows are the last read's, so nothing is new: a limit ("Limits").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("alpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    watch.write("1111\r\n2222\r\n3333\r\n4444");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jalpha\r\nbravo\r\ncharlie\r\ndelta\r\n");
    assert_eq!(watch.texts(), ["alpha", "bravo", "charlie", "delta", ""]);
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(
        outcome.rows,
        [NewRow::Shown, NewRow::Shown, kept(2), kept(3), NEW]
    );
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
    assert!(outcome.lost.is_none(), "the history is far from full");
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
fn a_caret_line_that_only_lost_text_while_the_caret_moved_forward_over_it_is_not_that_line() {
    // Windows Terminal's `cls` homes the caret onto the first row, then
    // erases and writes the prompt there: the row only lost text, and the
    // caret moved forward over "ready> ", which both reads show in the same
    // cells, so it is not the caret's line rewritten. Nothing is kept or
    // paired: a new screen ("The alignment"; Dickson, 2026-10-25).
    let mut watch = Watch::new(Flavor::WindowsTerminal, 30, 5, 20);
    watch.write("ready> .\\secret.ps1\r\nsecret\r\nready> cls");
    watch.write("\x1b[H");
    let cleared = watch.write("\x1b[2J\x1b[3Jready> ");
    assert_eq!(cleared.switch, Switch::New);
    assert_eq!(cleared.rows, [NEW]);
}

#[test]
fn a_space_dismissing_a_suggestion_leaves_the_caret_line_that_line() {
    // A space typed after "$ git sta" dismisses the grey suggestion after
    // it: the line only lost text and the caret moved forward, but over a
    // cell whose text changed, the suggestion's "t" now a space, so the row
    // is still the caret's line, rewritten ("The alignment"; Dickson,
    // 2026-10-25).
    let mut watch = Watch::console(40, 3, 10);
    watch.write("ok\r\n$ git sta\x1b[90mtus --porcelain --branch\x1b[0m\x1b[2;10H");
    let typed = watch.write("\x1b[K ");
    assert_eq!(
        typed.rows,
        [
            kept(0),
            NewRow::Rewritten {
                from: 1,
                moved: false,
                alone: false
            }
        ]
    );
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
    assert!(opened.lost.is_none(), "no overflow");
    assert_eq!(opened.rows, [NEW, NEW, NEW]);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(closed.rows, [NEW, NEW, NEW]);
}

#[test]
fn a_command_mark_above_the_read_keeps_its_place() {
    // Before the history is full a place is exact: the mark set on the
    // document's first row stays on that row however far output carries it
    // above the read ("Features and settings").
    let mut watch = Watch::console(10, 3, 100);
    watch.write("$ one");
    assert_eq!(
        watch.memory.add_mark(0),
        Some(Anchor {
            row: 0,
            column: 0,
            line_hash: hash("$ one")
        })
    );
    for line in 1..=20 {
        watch.write(&format!("\r\nl{line}"));
    }
    // The document's 21 rows end where the read does.
    let first = 21 - isize::try_from(watch.memory.rows().len()).expect("a few rows");
    assert_eq!(
        watch.memory.marks(),
        [Anchor {
            row: -first,
            column: 0,
            line_hash: hash("$ one")
        }]
    );
}

#[test]
fn a_command_mark_above_the_documents_start_is_dropped() {
    // A history of two rows: a read of the screen and a screen above it
    // starts at the document's start, which shows that the marked row left
    // the buffer.
    let mut watch = Watch::console(10, 3, 2);
    watch.write("$ one");
    watch.memory.add_mark(0);
    watch.write("\r\nl1");
    assert_eq!(watch.memory.marks().len(), 1);
    for line in 2..=8 {
        watch.write(&format!("\r\nl{line}"));
    }
    assert_eq!(watch.memory.marks(), []);
}

#[test]
fn command_marks_are_capped_the_oldest_dropped_first() {
    let mut watch = Watch::console(10, 3, 100);
    watch.write("a\r\nb");
    // A mark is set once on its line.
    watch.memory.add_mark(0);
    watch.memory.add_mark(0);
    assert_eq!(watch.memory.marks().len(), 1);
    // As many marks as are kept, each moved to a place of its own above.
    let above = |number: usize| Anchor {
        row: -isize::try_from(number).expect("a count"),
        column: 0,
        line_hash: hash("a"),
    };
    watch
        .memory
        .replace_mark(watch.memory.marks()[0], Some(above(1)));
    for number in 2..=super::MARKS {
        let mark = watch.memory.add_mark(0).expect("the row is kept");
        watch.memory.replace_mark(mark, Some(above(number)));
    }
    assert_eq!(watch.memory.marks().len(), super::MARKS);
    watch.memory.add_mark(1);
    let marks = watch.memory.marks();
    assert_eq!(marks.len(), super::MARKS);
    assert_eq!(marks[0], above(2));
    assert_eq!(
        marks[super::MARKS - 1],
        Anchor {
            row: 1,
            column: 0,
            line_hash: hash("b")
        }
    );
}

#[test]
fn a_command_mark_survives_a_full_screen_program() {
    // A mark is set aside with the shell's screen while a full-screen
    // program shows its own, and comes back at its place, counted from the
    // read's end, when the shell's screen does ("The aside screen").
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

#[test]
fn blank_rows_above_the_screen_that_read_the_same_leave_the_rows_found_by_text() {
    // A full history whose rows above the last read's screen are blank:
    // after the buffer moved a row they read the same, but with no text
    // among them positions are not trusted, and the rows are found by
    // their text, moved up a row ("Positions").
    let mut watch = Watch::console(10, 3, 6);
    watch.write("1\r\n2\r\n3\r\n4\r\n\r\n\r\n\r\n\r\np\r\nq\r\nr");
    let outcome = watch.write("\r\ns");
    assert_eq!(watch.taken, 1, "one read");
    assert!(outcome.by_text, "found by text");
    assert_eq!(outcome.rotation, 1);
    assert_eq!(outcome.lost, None);
    assert_eq!(watch.texts(), ["", "", "p", "q", "r", "s"]);
    assert_eq!(
        outcome.rows,
        [kept(4), kept(5), kept(6), kept(7), kept(8), NEW]
    );
}

#[test]
fn a_lost_read_whose_rows_are_found_once_counts_about() {
    // Ten lines into a full history move it further than the read covers,
    // so the read is lost and the whole document read; the last read's
    // rows are found in it at one place, the rows above them history and
    // the rows below them new, counted as about ("A lost read").
    let mut watch = Watch::console(10, 3, 20);
    let lines = (1..=30)
        .map(|line| format!("{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    watch.write(&lines);
    let more = (31..=40)
        .map(|line| format!("{line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let outcome = watch.write(&more);
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(outcome.lost, Some(Lost::About));
    let mut rows = vec![NewRow::Shown; 7];
    rows.extend([kept(17), kept(18), kept(19), kept(20), kept(21)]);
    rows.extend([NEW; 11]);
    assert_eq!(outcome.rows, rows);
}

#[test]
fn a_lost_read_whose_rows_fit_twice_is_uncounted() {
    // Alternating lines moved by an odd number of rows: the rows above the
    // last read's screen no longer read the same, and the last read's rows
    // fit at more than one place, so the read is lost; in the whole
    // document they fit at more than one place too, so only the visible
    // screen is aligned and what went by has no count ("A lost read").
    let mut watch = Watch::console(10, 3, 6);
    watch.write("a\r\nb\r\na\r\nb\r\na\r\nb\r\na\r\nb\r\na\r\nb\r\n");
    let outcome = watch.write("a\r\nb\r\na\r\n");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(outcome.lost, Some(Lost::Uncounted));
    let mut rows = vec![NewRow::Shown; 6];
    rows.extend([kept(3), kept(4), NEW]);
    assert_eq!(outcome.rows, rows);
}

/// A read of `rows`, each padded to 10 cells, its screen the last
/// `screen` of them and its caret at the end of the last.
fn read_of(rows: &[&str], screen: usize, at_document_start: bool, at_ms: u64) -> Read {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:10}")).collect();
    let last = padded.last().cloned().unwrap_or_default();
    Read {
        text: ReadText::unpadded(&padded.join("\r\n")),
        screen: padded[padded.len() - screen..].join("\r\n"),
        at_document_start,
        alternate: rows.len() == screen && at_document_start,
        first_row: if at_document_start {
            padded[0].clone()
        } else {
            String::new()
        },
        caret_rows_to_end: 1,
        caret_line: last,
        caret_column: rows.last().map_or(0, |row| row.len()),
        at_ms,
        ..Read::default()
    }
}

/// A memory that read a shell's screen at the document's start, "h1",
/// "h2" and "$ pi", and then a new screen whose start was refused, so that
/// the shell's screen is aside ("The aside screen").
fn with_the_shell_aside() -> TerminalMemory {
    let mut memory = TerminalMemory::new();
    assert!(
        memory
            .read(&read_of(&["h1", "h2", "$ pi"], 3, true, 0))
            .first
    );
    let opened = memory.read(&Read {
        shrank: true,
        ..read_of(&["q1", "q2", "q3"], 3, true, 100)
    });
    assert_eq!(opened.switch, Switch::New);
    memory
}

/// `count` numbered lines of `word`, each ended with a line break.
fn numbered(word: &str, count: usize) -> String {
    (1..=count)
        .map(|line| format!("{word}{line}\r\n"))
        .collect::<Vec<_>>()
        .concat()
}

#[test]
fn a_flood_larger_than_the_buffer_from_a_fresh_screen_is_a_lost_read() {
    // A fresh console host's screen starts at the document's start; one
    // write of more lines than the buffer holds before the next read keeps
    // nothing of it, and with no screen set aside the read is lost, the
    // last read's rows nowhere in the whole document: more than the buffer
    // went by ("Screens", "A lost read").
    let mut watch = Watch::console(10, 3, 6);
    watch.write("$ run\r\n");
    let outcome = watch.write(&(numbered("n", 20) + "$ "));
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(outcome.lost, Some(Lost::MoreThan));
    assert_eq!(outcome.rows, [NEW; 9]);
}

#[test]
fn a_flood_larger_than_the_history_after_windows_terminals_clear_is_a_lost_read() {
    // Windows Terminal's clear empties the history and is a new screen,
    // setting the screen before it aside; the typing on the cleared screen
    // keeps it there, its screen at the document's start. A write of more
    // than the history then keeps nothing of the prompt's screen, and the
    // aside screen's rows left with the clear, so the read is lost ("The
    // aside screen", "A lost read"). The flood's last line repeats a row of
    // the aside screen, but nearer the document's end than that row was, so
    // it is not found there.
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 6);
    watch.write("a\r\nb\r\nn20\r\nc\r\n$ clear");
    let cleared = watch.write("\x1b[H\x1b[2J\x1b[3J$ ");
    assert_eq!(cleared.switch, Switch::New);
    watch.write("run\r\n");
    let outcome = watch.write(&(numbered("n", 20) + "$ "));
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(outcome.lost, Some(Lost::MoreThan));
}

#[test]
fn a_pager_closing_over_more_than_two_screens_of_history_is_a_new_screen() {
    // The pager's own reads keep its rows as it scrolls, its screen at the
    // document's start, so the shell's screen stays aside; closing it keeps
    // nothing of the pager's screen, and the shell's rows are found at
    // their places, so it is a new screen: its visible rows new, the rows
    // above them history ("The aside screen", "Screens").
    let mut watch = Watch::console(10, 3, 20);
    watch.write(&(numbered("h", 8) + "$ less"));
    let opened = watch.write("\x1b[?1049h\x1b[Hp1\r\np2\r\n:");
    assert_eq!(opened.switch, Switch::New);
    let scrolled = watch.write("\r\x1b[Kp3\r\n:");
    assert_eq!(scrolled.switch, Switch::None);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(watch.taken, 1, "one read");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(closed.lost, None);
    let mut rows = vec![NewRow::Shown; 6];
    rows.extend([NEW; 3]);
    assert_eq!(closed.rows, rows);
}

#[test]
fn a_full_screen_program_closing_over_a_full_buffer_and_two_new_lines_is_a_new_screen() {
    // With the buffer full, the shell's two lines after the program closed
    // move its rows up two places: the aside screen's rows are found higher
    // than their places, so it is a new screen ("The aside screen").
    let mut watch = Watch::console(10, 3, 6);
    watch.write(&(numbered("h", 12) + "$ vim"));
    watch.write("\x1b[?1049h\x1b[Hv1\r\n~\r\n:");
    watch.write("\x1b[1;1Hv2");
    let closed = watch.write("\x1b[?1049l\r\nout\r\n$ ");
    assert_eq!(closed.switch, Switch::New);
    assert_eq!(closed.lost, None);
    let texts = watch.texts();
    assert_eq!(texts[texts.len() - 3..], ["$ vim", "out", "$"]);
}

#[test]
fn the_aside_screen_is_looked_for_in_a_lost_reads_whole_document() {
    // A read that keeps nothing and does not hold the aside screen is lost;
    // in the whole document the aside screen's rows are found higher than
    // their places, so it is a new screen after all ("The aside screen").
    let mut memory = with_the_shell_aside();
    let far = memory.read(&Read {
        from_start: true,
        above: 3,
        ..read_of(&["o1", "o2", "o3", "o4", "o5", "o6", "o7"], 3, false, 200)
    });
    assert!(far.whole_document, "a lost read");
    let rows = [
        "h1", "h2", "$ pi", "x1", "o1", "o2", "o3", "o4", "o5", "o6", "o7",
    ];
    let whole = memory.read(&read_of(&rows, 3, true, 300));
    assert_eq!(whole.switch, Switch::New);
    assert_eq!(whole.lost, None);
}

#[test]
fn a_read_keeping_something_with_its_screen_below_the_documents_start_drops_the_aside_screen() {
    // Back on a screen with history, a read that keeps something of the
    // last read drops the aside screen, so a later lost read whose whole
    // document holds the aside screen's rows is counted, not taken for that
    // screen back ("The aside screen").
    let mut memory = with_the_shell_aside();
    let kept_some = memory.read(&Read {
        from_start: true,
        ..read_of(&["q1", "q2", "q3", "r1"], 3, true, 200)
    });
    assert_eq!(kept_some.switch, Switch::None);
    let far = memory.read(&Read {
        from_start: true,
        above: 3,
        ..read_of(&["s1", "s2", "s3", "s4", "s5", "s6"], 3, false, 300)
    });
    assert!(far.whole_document, "a lost read");
    let rows = [
        "h1", "h2", "$ pi", "q1", "q2", "q3", "r1", "s1", "s2", "s3", "s4", "s5", "s6",
    ];
    let whole = memory.read(&read_of(&rows, 3, true, 400));
    assert_eq!(whole.switch, Switch::None);
    assert_eq!(whole.lost, Some(Lost::About));
}

#[test]
fn a_page_that_keeps_no_row_sets_the_page_before_it_aside() {
    // A limit ("Limits"): a page that keeps no row of the page before it,
    // its status row included, with the caret off its line so that pairs
    // nothing either, is a new screen and sets that page aside in place of
    // the shell's screen, so the shell's screen coming back over its
    // history is not recognised: the read is lost, and counted as more than
    // went by.
    let mut watch = Watch::console(10, 3, 20);
    watch.write(&(numbered("h", 8) + "$ less"));
    watch.write("\x1b[?1049h\x1b[Hp1\r\np2\r\n:");
    let paged = watch.write("\x1b[H\x1b[2Jq1\r\nq2\r\n=\x1b[H");
    assert_eq!(paged.switch, Switch::New);
    let closed = watch.write("\x1b[?1049l");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(closed.switch, Switch::None);
    assert_eq!(closed.lost, Some(Lost::MoreThan));
}

#[test]
fn a_history_printed_again_longer_is_found_lower_with_its_screen() {
    // A program erases the screen and the history and prints them again
    // with two lines added at the top, as Pi's regular mode does on
    // Control+O. The kept start still lies above the new screen, so it is
    // accepted, but the rows there are other text: found by text, the
    // screen's rows went two rows lower, and the history block's rows are
    // found at that one shift too, kept and silent; the rows above where
    // the last read's first row went are history ("The alignment").
    let mut watch = Watch::console(10, 3, 20);
    watch.write("a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng");
    let outcome = watch.write("\x1b[2J\x1b[3J\x1b[Hx\r\ny\r\na\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng");
    assert_eq!(watch.taken, 1, "one read");
    assert!(outcome.by_text, "found by text");
    assert!(outcome.rewritten, "rows found lower than their places");
    assert_eq!(watch.texts(), ["y", "a", "b", "c", "d", "e", "f", "g"]);
    assert_eq!(
        outcome.rows,
        [
            NewRow::Shown,
            NewRow::Shown,
            kept(1),
            kept(2),
            kept(3),
            kept(4),
            kept(5),
            kept(6)
        ]
    );
}

#[test]
fn the_history_blocks_text_printed_lower_while_the_screen_went_up_is_new() {
    // The history block's lines printed again in a full history: the
    // screen's row went up two rows with the buffer, and the history block's
    // text is printed again lower, which is new output, not the history
    // block moved down, though it would keep more rows ("The alignment").
    let mut watch = Watch::console(10, 3, 3);
    watch.write("h1\r\nh2\r\nh3\r\ns1\r\n\r\n");
    let outcome = watch.write("h1\r\nh2\r\nh3");
    assert_eq!(watch.taken, 1, "one read");
    assert!(outcome.by_text, "found by text");
    assert!(!outcome.rewritten, "no row found lower");
    assert_eq!(watch.texts(), ["h3", "s1", "", "h1", "h2", "h3"]);
    assert_eq!(outcome.rows, [kept(2), kept(3), kept(4), NEW, NEW, NEW]);
}

#[test]
fn a_stream_through_a_full_history_takes_no_vote_on_the_screens_shift() {
    // A stream through a full history moves every row up, so each read is
    // found by text and no row of the history block has its text lower
    // than its place: the vote on the screen rows' shift is never taken
    // ("The alignment"; `docs/performance.md`, "The terminal memory's
    // cost, estimated").
    let mut watch = Watch::console(10, 3, 3);
    watch.write("s1\r\ns2\r\ns3\r\ns4\r\ns5\r\ns6\r\ns7");
    super::VOTES.with(|votes| votes.set(0));
    for line in 8..=13 {
        let outcome = watch.write(&format!("\r\ns{line}"));
        assert!(outcome.by_text, "found by text");
    }
    assert_eq!(super::VOTES.with(std::cell::Cell::get), 0);
}

#[test]
fn more_than_a_screen_of_output_after_windows_terminals_clear_is_a_lost_read_all_new() {
    // `clear; dir` typed as one line: the clear erases the screen and the
    // history and the output that follows fills more than a screen before
    // the next read, the document shorter than before. The read's start is
    // refused and it keeps nothing, its screen does not start at the
    // document's start and no screen is aside, so it is lost; the whole
    // document holds nothing of the last read, and after a refused start
    // every row of it is new, counted as about ("Screens", "A lost read").
    let mut watch = Watch::new(Flavor::WindowsTerminal, 10, 3, 20);
    watch.write("p1\r\np2\r\np3\r\np4\r\np5\r\np6\r\n$ clear");
    let outcome = watch.write("\x1b[H\x1b[2J\x1b[3Jo1\r\no2\r\no3\r\no4\r\no5\r\n$ ");
    assert_eq!(watch.taken, 2, "the whole document read");
    assert_eq!(outcome.switch, Switch::None);
    assert_eq!(outcome.lost, Some(Lost::About));
    assert_eq!(watch.texts(), ["o1", "o2", "o3", "o4", "o5", "$"]);
    assert_eq!(outcome.rows, [NEW; 6]);
}

#[test]
fn the_review_position_above_a_refused_start_is_found_by_its_text_or_goes_to_the_caret() {
    // A program erases the screen and the history and prints them again
    // shorter, refusing the read's start, as Pi's regular mode does on its
    // second Control+O. The rows above the read are gone, the document
    // having shrunk beneath them: the review position there is found again
    // by its line's text, or, with no row of that text, goes to the caret
    // ("The review cursor").
    for (line, row, column, found) in [("a2", 1, 0, "a2"), ("a3", 5, 2, "a9")] {
        let mut watch = Watch::console(10, 3, 20);
        watch.write("a1\r\na2\r\na3\r\na4\r\na5\r\na6\r\na7\r\na8\r\na9");
        let first = isize::try_from(watch.memory.first_kept()).expect("a few rows");
        watch.memory.set_review_anchor(Anchor {
            row: first - 2,
            column: 0,
            line_hash: hash(line),
        });
        let outcome = watch.write("\x1b[2J\x1b[3J\x1b[Ha1\r\na2\r\nnew\r\na7\r\na8\r\na9");
        assert_eq!(watch.taken, 1, "one read");
        assert_eq!(watch.texts(), ["a1", "a2", "new", "a7", "a8", "a9"]);
        assert_eq!(outcome.rows[3..], [kept(6), kept(7), kept(8)]);
        assert_eq!(
            watch.memory.review(),
            Some(Anchor {
                row,
                column,
                line_hash: hash(found)
            }),
            "{line}"
        );
    }
}

#[test]
fn the_review_position_on_a_gone_row_after_a_refused_start_never_moves_above_the_read() {
    // A program prints its history again shorter, below the document's
    // start, and the review position's row, in the last read's history
    // block, is gone with every row above it in the read: the rows above
    // the read are gone too, the document having shrunk beneath them, so
    // it goes to the caret ("The review cursor").
    let mut watch = Watch::console(10, 3, 20);
    watch.write(numbered("a", 20).trim_end());
    let first = watch.memory.first_kept();
    let kept_first = watch
        .memory
        .rows()
        .get(first)
        .map(|row| row.text.to_string());
    assert_eq!(kept_first.as_deref(), Some("a15"));
    watch.memory.set_review(first + 2, 0);
    let outcome = watch.write("\x1b[2J\x1b[3J\x1b[Hb1\r\nb2\r\nb3\r\nb4\r\na18\r\na19\r\na20");
    assert_eq!(watch.taken, 1, "one read");
    assert_eq!(watch.texts(), ["b2", "b3", "b4", "a18", "a19", "a20"]);
    assert_eq!(
        outcome.rows[3..],
        [kept(first + 3), kept(first + 4), kept(first + 5)]
    );
    assert_eq!(
        watch.memory.review(),
        Some(Anchor {
            row: 5,
            column: 3,
            line_hash: hash("a20")
        })
    );
}

#[test]
fn no_character_takes_more_cells_than_its_bytes_and_none_below_u0800_more_than_one() {
    // A read's rows are split without measuring each character when these
    // hold ("Rows and the width"): a line of no more bytes than the width,
    // or of no more characters below U+0800 than the width, is one row.
    let wider: Vec<char> = (0..=u32::from(char::MAX))
        .filter_map(char::from_u32)
        .filter(|&c| {
            let cells = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            cells > c.len_utf8() || (u32::from(c) < 0x800 && cells > 1)
        })
        .collect();
    assert_eq!(wider, Vec::<char>::new());
}
