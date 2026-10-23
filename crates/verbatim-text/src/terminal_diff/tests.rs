use super::*;

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|line| (*line).to_owned()).collect()
}

/// The normal read of `new` after `old`, which said what it read.
fn read(old: &[String], new: &[String], height: usize, caret: Option<usize>) -> Found {
    diff_read(&Read {
        old,
        said: old,
        lines: new,
        height,
        caret,
    })
    .0
}

/// A line that grew at its end by `text`.
fn grown(text: &str, line: &str) -> LineChange {
    LineChange {
        text: text.to_owned(),
        line: line.to_owned(),
        appended: true,
        uncertain: text.len() - text.trim_start().len(),
        inserted: text.to_owned(),
        since_read: None,
    }
}

/// A line rewritten, spoken as `text`, having gained `inserted`.
fn rewritten(text: &str, line: &str, inserted: &str) -> LineChange {
    LineChange {
        text: text.to_owned(),
        line: line.to_owned(),
        appended: false,
        uncertain: 0,
        inserted: inserted.to_owned(),
        since_read: None,
    }
}

#[test]
#[expect(clippy::too_many_lines, reason = "one table of cases")]
fn insertions_are_spoken_and_deletions_are_not() {
    type Case<'a> = (&'a [&'a str], &'a [&'a str], usize, Option<usize>, Found);
    let cases: &[Case<'_>] = &[
        // Output at the end, after the prompt line that grew.
        (
            &["ready>"],
            &["ready> echo hi", "hi", "ready>"],
            5,
            Some(2),
            Found {
                changed: Some(grown(" echo hi", "ready> echo hi")),
                appended: lines(&["hi", "ready>"]),
                ..Found::default()
            },
        ),
        // Two lines written below a full screen.
        (
            &["a", "b", "c"],
            &["a", "b", "c", "d", "e"],
            3,
            Some(4),
            Found {
                appended: lines(&["d", "e"]),
                ..Found::default()
            },
        ),
        // Inserted in the middle, above a fixed footer.
        (
            &["one", "two", "-- footer --"],
            &["one", "two", "three", "-- footer --"],
            4,
            None,
            Found {
                above: lines(&["three"]),
                ..Found::default()
            },
        ),
        // Lines written above a footer that was redrawn: the footer is
        // rewritten, not the first of the new lines.
        (
            &["one", "two", "status: busy"],
            &["one", "two", "three", "four", "status: done"],
            3,
            None,
            Found {
                above: lines(&["three", "four"]),
                changed: Some(rewritten("done", "status: done", "done")),
                ..Found::default()
            },
        ),
        // A message printed above the prompt while typing.
        (
            &["out", "ready> ech"],
            &["out", "Message", "ready> ech"],
            3,
            Some(2),
            Found {
                above: lines(&["Message"]),
                ..Found::default()
            },
        ),
        // Deletion only.
        (&["a", "b", "c"], &["a", "c"], 3, None, Found::default()),
        // A full-screen program scrolling up: a line appears at the top,
        // one leaves at the bottom.
        (
            &["2", "3", "4"],
            &["1", "2", "3"],
            3,
            None,
            Found {
                above: lines(&["1"]),
                ..Found::default()
            },
        ),
        // Scrolling down a line.
        (
            &["1", "2", "3"],
            &["2", "3", "4"],
            3,
            None,
            Found {
                appended: lines(&["4"]),
                ..Found::default()
            },
        ),
        // Identical lines kept at their position: one more is new.
        (
            &["x", "x", "x"],
            &["x", "x", "x", "x"],
            3,
            None,
            Found {
                appended: lines(&["x"]),
                ..Found::default()
            },
        ),
        // A redraw with the same text.
        (&["a", "b"], &["a", "b"], 3, None, Found::default()),
        // A selection list's marker moved by two characters.
        (
            &["> one", "  two", "  three"],
            &["  one", "> two", "  three"],
            3,
            None,
            Found {
                above: lines(&["> two"]),
                ..Found::default()
            },
        ),
        // Several changes in one status block.
        (
            &["cpu 10%", "mem 20%", "ready>"],
            &["cpu 11%", "mem 25%", "ready>"],
            3,
            Some(2),
            Found {
                above: lines(&["11%", "25%"]),
                ..Found::default()
            },
        ),
        // Blank lines above the caret count as lines.
        (
            &["ready>"],
            &["ready>", "a", "", "b"],
            4,
            Some(3),
            Found {
                appended: lines(&["a", "", "b"]),
                ..Found::default()
            },
        ),
        // Lines of symbols alone are spoken.
        (
            &["a"],
            &["a", "~", "────"],
            3,
            None,
            Found {
                appended: lines(&["~", "────"]),
                ..Found::default()
            },
        ),
        // A character typed in the middle of the prompt.
        (
            &["out", "ready> abd"],
            &["out", "ready> abcd"],
            3,
            Some(1),
            Found {
                changed: Some(rewritten("abcd", "ready> abcd", "c")),
                ..Found::default()
            },
        ),
    ];
    for (old, new, height, caret, expected) in cases {
        assert_eq!(
            read(&lines(old), &lines(new), *height, *caret),
            *expected,
            "{old:?} to {new:?}"
        );
    }
}

#[test]
fn a_changed_line_speaks_from_the_word_that_changed() {
    assert_eq!(
        line_change("Loading 10%", "Loading 20%"),
        Some(rewritten("20%", "Loading 20%", "2"))
    );
    assert_eq!(
        line_change("progress step one", "progress step two"),
        Some(rewritten("two", "progress step two", "two"))
    );
    assert_eq!(
        line_change("ready>", "ready> ls"),
        Some(grown(" ls", "ready> ls"))
    );
    // A change at a space speaks from the next word.
    assert_eq!(
        line_change("a b", "a  c"),
        Some(rewritten("c", "a  c", " c"))
    );
    // Chinese has no spaces: the word the dictionary finds.
    assert_eq!(
        line_change("我们今天去学校", "我们明天去学校"),
        Some(rewritten("明天去学校", "我们明天去学校", "明"))
    );
    // Thai likewise.
    assert_eq!(
        line_change("สวัสดีครับ", "สวัสดีค่ะ"),
        Some(rewritten("ค่ะ", "สวัสดีค่ะ", "ค่ะ"))
    );
    // A combining mark changes its whole grapheme and word.
    assert_eq!(
        line_change("cafe noir", "cafe\u{301} noir"),
        Some(rewritten(
            "cafe\u{301} noir",
            "cafe\u{301} noir",
            "e\u{301}"
        ))
    );
    // Wide characters and tabs are characters like any other.
    assert_eq!(
        line_change("名前\tA", "名前\tB"),
        Some(rewritten("B", "名前\tB", "B"))
    );
}

#[test]
fn a_line_that_only_lost_text_or_gained_white_space_is_silent() {
    assert_eq!(line_change("ready> ls", "ready>"), None);
    assert_eq!(line_change("abc", "abc"), None);
    assert_eq!(line_change("echo hello", "echo hllo"), None);
    assert_eq!(line_change("> one", "  one"), None);
    assert_eq!(line_change("a-b", "a b"), None);
}

#[test]
fn a_spinner_is_silent_until_the_line_changes_to_text() {
    assert_eq!(line_change("Working |", "Working /"), None);
    assert_eq!(line_change("⠋ build", "⠙ build"), None);
    assert_eq!(
        line_change("Working |", "Working done"),
        Some(rewritten("done", "Working done", "done"))
    );
    // A letter replaced is not a spinner.
    assert_eq!(
        line_change("step a", "step b"),
        Some(rewritten("b", "step b", "b"))
    );
}

#[test]
fn typing_above_a_status_line_is_the_changed_line() {
    let old = lines(&["out", "ready> ech", "-- 12:00 --"]);
    // The status line below the prompt ticks as the user types.
    assert_eq!(
        read(
            &old,
            &lines(&["out", "ready> echo", "-- 12:01 --"]),
            3,
            Some(1)
        ),
        Found {
            above: lines(&["01 --"]),
            changed: Some(grown("o", "ready> echo")),
            ..Found::default()
        }
    );
    assert_eq!(
        read(
            &old,
            &lines(&["out", "ready> echo", "-- 12:00 --"]),
            3,
            Some(1)
        ),
        Found {
            changed: Some(grown("o", "ready> echo")),
            ..Found::default()
        }
    );
}

#[test]
fn a_footer_redrawn_unchanged_below_output_says_nothing() {
    assert_eq!(
        read(
            &lines(&["ready>", "one", "status: busy"]),
            &lines(&["ready>", "one", "a", "b", "c", "status: busy"]),
            3,
            Some(4),
        ),
        Found {
            above: lines(&["a", "b", "c"]),
            ..Found::default()
        }
    );
}

#[test]
fn a_footer_whose_view_moves_is_found_in_the_snapshot() {
    // A line written into the scroll region above the footer: the view
    // moves down a row, the footer is drawn on the new last row, and the
    // line is written over the row the footer left. The snapshot runs to
    // the document's end, so the footer is in it.
    assert_eq!(
        read(
            &lines(&["ready>", "one", "status: busy"]),
            &lines(&["ready>", "one", "a", "status: busy"]),
            3,
            Some(2),
        ),
        Found {
            above: lines(&["a"]),
            ..Found::default()
        }
    );
    // The next read starts at the screen's new top and finds nothing new.
    let screen = lines(&["one", "a", "status: busy"]);
    assert_eq!(read(&screen, &screen, 3, Some(1)), Found::default());
}

#[test]
fn a_full_history_shifting_beneath_the_kept_row_still_finds_the_old_screen() {
    // Two lines written with the history full: the kept row now holds the
    // old screen's last line.
    assert_eq!(
        read(&lines(&["d", "e", "f"]), &lines(&["f", "g", "h"]), 3, None),
        Found {
            appended: lines(&["g", "h"]),
            ..Found::default()
        }
    );
}

#[test]
fn rows_not_yet_written_below_the_caret_are_not_lines() {
    // A screen cleared and given a footer on its last row, the caret at
    // the top: the rows between were passed over, not printed.
    let new = lines(&["", "", "", "", "", "status: busy"]);
    assert_eq!(written(&new, Some(0)), [5]);
    let old = lines(&["ready> footer"]);
    let (found, _) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &new,
        height: 6,
        caret: Some(0),
    });
    assert_eq!(
        found,
        Found {
            appended: lines(&["status: busy"]),
            ..Found::default()
        }
    );
    // With no caret, the blank rows at the end are the ones not written.
    assert_eq!(written(&lines(&["a", "", "b", "", ""]), None), [0, 1, 2]);
}

#[test]
fn a_rewrite_read_half_done_speaks_the_word_that_changed_once_done() {
    let old = lines(&["two", "ready>", "Loading 50%"]);
    // Read while the line was cleared and only its first letter written:
    // nothing is said, and the line keeps what it said.
    let half = lines(&["two", "ready>", "L"]);
    let (found, _) = diff_read(&Read {
        old: &old,
        said: &old,
        lines: &half,
        height: 3,
        caret: Some(2),
    });
    assert_eq!(
        found,
        Found {
            said: vec![(2, "Loading 50%".to_owned())],
            ..Found::default()
        }
    );
    let said = lines(&["two", "ready>", "Loading 50%"]);
    let (found, _) = diff_read(&Read {
        old: &half,
        said: &said,
        lines: &lines(&["two", "ready>", "Loading 51%"]),
        height: 3,
        caret: Some(2),
    });
    assert_eq!(
        found,
        Found {
            changed: Some(LineChange {
                since_read: Some(Box::new(grown("oading 51%", "Loading 51%"))),
                ..rewritten("51%", "Loading 51%", "1")
            }),
            ..Found::default()
        }
    );
}

#[test]
fn a_line_typed_back_to_what_it_said_says_nothing_but_shows_the_typing() {
    // Backspace, then the same letter typed again.
    let (found, _) = diff_read(&Read {
        old: &lines(&["ready>", "ready> l"]),
        said: &lines(&["ready>", "ready> ls"]),
        lines: &lines(&["ready>", "ready> ls"]),
        height: 3,
        caret: Some(1),
    });
    assert_eq!(
        found,
        Found {
            changed: Some(LineChange {
                text: String::new(),
                line: "ready> ls".to_owned(),
                appended: false,
                uncertain: 0,
                inserted: String::new(),
                since_read: Some(Box::new(grown("s", "ready> ls"))),
            }),
            ..Found::default()
        }
    );
}

#[test]
fn a_wrapped_line_losing_its_tail_says_nothing_and_keeps_what_it_said() {
    // A command wider than the screen, one logical line, cut back to its
    // first row's text.
    let command = format!("ready> echo {}xyz", "a".repeat(108));
    let cut = format!("ready> echo {}", "a".repeat(108));
    assert_eq!(
        read(
            &lines(&["ready> ls", &command]),
            &lines(&["ready> ls", &cut]),
            3,
            Some(1),
        ),
        Found {
            said: vec![(1, command.clone())],
            ..Found::default()
        }
    );
}

#[test]
fn what_a_row_said_goes_with_its_text_when_the_screen_scrolls() {
    // A pager's rows, the first one cut short as the screen scrolls.
    let old = lines(&["abc def", "abc", "xyz", ":"]);
    let (found, _) = diff_read(&Read {
        old: &old,
        said: &old,
        lines: &lines(&["abc", "xyz", "uvw", ":"]),
        height: 4,
        caret: Some(3),
    });
    assert_eq!(
        found,
        Found {
            above: lines(&["uvw"]),
            ..Found::default()
        }
    );
    // Had the row kept the "abc def" it said before the scroll, only "x"
    // would be new.
    let old = lines(&["abc", "xyz", "uvw", ":"]);
    assert_eq!(
        read(&old, &lines(&["abc defx", "xyz", "uvw", ":"]), 4, Some(3)),
        Found {
            above: lines(&["defx"]),
            ..Found::default()
        }
    );
}

#[test]
fn a_row_erased_and_written_again_is_new() {
    let old = lines(&["two", "three", "row 28", "status"]);
    let erased = lines(&["two", "three", "", "status"]);
    // Erased whole, the row keeps nothing of what it said.
    assert_eq!(read(&old, &erased, 4, None), Found::default());
    assert_eq!(
        read(
            &erased,
            &lines(&["two", "three", "row 30", "status"]),
            4,
            None
        ),
        Found {
            above: lines(&["row 30"]),
            ..Found::default()
        }
    );
}

#[test]
fn an_old_screen_not_in_the_snapshot_is_missing() {
    // The kept row now holds newer text.
    assert_eq!(
        read(&lines(&["a", "b"]), &lines(&["x", "y"]), 2, Some(1)),
        Found {
            old_missing: true,
            ..Found::default()
        }
    );
}

#[test]
fn the_document_read_finds_the_old_screen_by_the_lines_above_identical_ones() {
    // Lines written past the kept row, the old screen still in the
    // history: found by content, then diffed from there.
    let (found, _) = diff_document(&Read {
        old: &lines(&["ready>", "y", "y"]),
        said: &lines(&["ready>", "y", "y"]),
        lines: &lines(&["x", "ready>", "y", "y", "y", "y"]),
        height: 3,
        caret: None,
    });
    assert_eq!(
        found,
        Found {
            appended: lines(&["y", "y"]),
            ..Found::default()
        }
    );
}

#[test]
fn the_document_read_takes_identical_lines_nearest_the_end() {
    // No position to go by: the old screen of identical lines matches
    // nearest the end, so nothing is new.
    let old = lines(&["y", "y", "y"]);
    let (found, _) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &lines(&["ready>", "y", "y", "y", "y", "y"]),
        height: 3,
        caret: None,
    });
    assert_eq!(found, Found::default());
}

#[test]
fn the_document_read_of_a_cleared_screen_speaks_what_it_shows() {
    // `cls`: the old screen gives way to the prompt alone, which starts as
    // the old last line did; it is new, not that line shortened.
    let old = lines(&["ready> echo hi", "hi", "ready> cls"]);
    let (found, _) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &lines(&["ready>"]),
        height: 3,
        caret: Some(0),
    });
    assert_eq!(
        found,
        Found {
            appended: lines(&["ready>"]),
            ..Found::default()
        }
    );
}

#[test]
fn the_document_read_of_an_alternate_screen_rewrites_it_in_place() {
    let old = lines(&["two", "three", "ready> vim"]);
    let drawn = lines(&["~", "file.txt", "~"]);
    let (found, _) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &drawn,
        height: 3,
        caret: None,
    });
    assert_eq!(
        found,
        Found {
            above: lines(&["~", "file.txt"]),
            changed: Some(rewritten("~", "~", "~")),
            ..Found::default()
        }
    );
    // A row changed near the top of the alternate screen.
    assert_eq!(
        read(&drawn, &lines(&["~", "file.txt [+]", "~"]), 3, Some(0)),
        Found {
            above: lines(&["[+]"]),
            ..Found::default()
        }
    );
}

#[test]
fn a_flood_past_a_small_history_is_more_than_the_history_holds() {
    // A history of 8 lines, 3 of them on the screen.
    let history: Vec<String> = (13..=20).map(|line| format!("line {line}")).collect();
    let old = lines(&["b", "c", "d"]);
    let (found, _) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &history,
        height: 3,
        caret: None,
    });
    assert_eq!(
        found,
        Found {
            skipped: Some(Skipped::MoreThan(5)),
            appended: lines(&["line 18", "line 19", "line 20"]),
            ..Found::default()
        }
    );
}

/// A screen of 30 distinct lines ending with a prompt.
fn thirty_lines() -> Vec<String> {
    let mut screen: Vec<String> = (1..30).map(|line| format!("old {line}")).collect();
    screen.push("ready>".to_owned());
    screen
}

#[test]
fn a_flood_read_in_one_snapshot_aligns_one_window() {
    let old = thirty_lines();
    let flood: Vec<String> = (1..=2000).map(|line| format!("line {line}")).collect();
    let mut snapshot = old[..29].to_vec();
    snapshot.push("ready> cmd".to_owned());
    snapshot.extend(flood.iter().cloned());
    snapshot.push("ready>".to_owned());
    let (found, work) = diff_read(&Read {
        old: &old,
        said: &old,
        lines: &snapshot,
        height: 30,
        caret: Some(2030),
    });
    let mut appended = flood;
    appended.push("ready>".to_owned());
    assert_eq!(
        found,
        Found {
            changed: Some(grown(" cmd", "ready> cmd")),
            appended,
            ..Found::default()
        }
    );
    // The window is the old screen's 30 lines, one screen of 30, and one
    // line more. The common start compares the 29 lines above the prompt
    // for equality and the prompt once for having grown, 30 comparisons,
    // and takes the whole old screen, so nothing else is compared.
    assert_eq!(
        work,
        Work {
            aligned_new: 61,
            aligned_old: 30,
            compared: 30,
        }
    );
}

#[test]
fn a_flood_past_the_whole_history_is_more_than_the_history_holds() {
    let old = thirty_lines();
    // The kept row holds 30 lines of newer text, none of the old screen's.
    let newer: Vec<String> = (8972..=9001).map(|line| format!("line {line}")).collect();
    let (found, work) = diff_read(&Read {
        old: &old,
        said: &old,
        lines: &newer,
        height: 30,
        caret: Some(29),
    });
    assert_eq!(
        found,
        Found {
            old_missing: true,
            ..Found::default()
        }
    );
    // One comparison for the common start, one for the common end, and the
    // 30 by 30 lines between them for the common subsequence: 902.
    assert_eq!(
        work,
        Work {
            aligned_new: 30,
            aligned_old: 30,
            compared: 902,
        }
    );
    let document: Vec<String> = (1..=9001).map(|line| format!("line {line}")).collect();
    let (found, work) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &document,
        height: 30,
        caret: Some(9000),
    });
    assert_eq!(
        found,
        Found {
            skipped: Some(Skipped::MoreThan(8971)),
            appended: newer,
            ..Found::default()
        }
    );
    // Each of the 8,972 lines that could end the old screen (every line
    // from the 30th to the last) is compared once with the prompt, and
    // none is it; then the history's first line is compared with its
    // second, which differs: 8,973, within 9,001 times 30.
    assert_eq!(
        work,
        Work {
            aligned_new: 0,
            aligned_old: 0,
            compared: 8973,
        }
    );
}

#[test]
fn a_flood_of_identical_lines_past_the_history_is_uncounted() {
    let old = thirty_lines();
    let document = vec!["y".to_owned(); 9001];
    let (found, work) = diff_document(&Read {
        old: &old,
        said: &old,
        lines: &document,
        height: 30,
        caret: Some(9000),
    });
    assert_eq!(
        found,
        Found {
            skipped: Some(Skipped::Uncounted),
            appended: vec!["y".to_owned(); 30],
            ..Found::default()
        }
    );
    // The 8,972 candidates as before, then each of the history's 8,971
    // lines after its first compared with the first: 8,972 and 8,970.
    assert_eq!(
        work,
        Work {
            aligned_new: 0,
            aligned_old: 0,
            compared: 17_942,
        }
    );
}
