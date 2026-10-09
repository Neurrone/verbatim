//! Reading a terminal by diffing its screen, on simulated text: a buffer of
//! padded rows whose last `height` rows are the screen, a history that
//! keeps at most `capacity` rows, and the anchor found again by its text as
//! the remote program finds it, run through [`read_new`].

use super::screen::line_change;
use super::*;

/// A screen of `lines` with the caret on line `caret`.
fn remembered(lines: &[&str], caret: usize) -> Memory {
    Memory {
        screen: lines.iter().map(|line| (*line).to_owned()).collect(),
        caret: Some(caret),
        ..Memory::default()
    }
}

#[test]
fn the_screen_before_a_key_is_found_after_its_change_was_read() {
    let typed = remembered(&["ready> echo aaaxyz"], 0);
    let cleared = remembered(&["ready>"], 0);
    let mut terminal = Terminal::default();
    terminal.remember_at(typed.clone(), None, 100);
    // The key, pressed at 150, cleared the line; two reads, from the
    // console's update and from UIA's text change, found it cleared
    // before the key's request came.
    terminal.remember_at(cleared.clone(), None, 160);
    terminal.remember_at(cleared.clone(), None, 170);
    assert_eq!(terminal.screen_at(150), Some(&typed));
    // A key pressed after the change was first read finds the screen as
    // it is now.
    assert_eq!(terminal.screen_at(165), Some(&cleared));
    // A key pressed before the screen before the change was first read
    // finds nothing.
    assert_eq!(terminal.screen_at(100), None);
    // Two changes after the key: the screen before it is no longer kept.
    terminal.remember_at(remembered(&["ready> x"], 0), None, 180);
    assert_eq!(terminal.screen_at(150), None);
}

/// The columns every simulated row is padded to.
const WIDTH: usize = 20;

/// How many of the unread lines a read takes from their first.
const HEAD: u32 = 3;

/// How many matches the simulated search checks.
const MATCHES: usize = 20;

struct Sim {
    rows: Vec<String>,
    height: usize,
    capacity: usize,
    /// Whether the screen is an alternate screen, with no history.
    alternate: bool,
    /// Whether the next read is disturbed.
    disturbed: bool,
    /// Whether the terminal's view moves under the next read.
    view_moved: bool,
}

fn padded(text: &str) -> String {
    format!("{text:<WIDTH$}\r\n")
}

impl Sim {
    fn new(height: usize, capacity: usize, rows: &[&str]) -> Self {
        let mut sim = Self {
            rows: Vec::new(),
            height,
            capacity,
            alternate: false,
            disturbed: false,
            view_moved: false,
        };
        sim.push(rows);
        sim
    }

    /// Writes rows at the end, discarding the oldest past the capacity.
    fn push(&mut self, rows: &[&str]) {
        self.rows.extend(rows.iter().map(|row| padded(row)));
        let excess = self.rows.len().saturating_sub(self.capacity);
        self.rows.drain(..excess);
    }

    /// Writes a line wider than the screen at the end: rows of `WIDTH`
    /// cells, joined without a line break, as a terminal gives a line that
    /// wrapped onto more rows.
    fn push_wrapped(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let rows: Vec<String> = chars
            .chunks(WIDTH)
            .map(|row| row.iter().collect())
            .collect();
        let last = rows.len() - 1;
        for (index, row) in rows.into_iter().enumerate() {
            self.rows
                .push(if index == last { padded(&row) } else { row });
        }
        let excess = self.rows.len().saturating_sub(self.capacity);
        self.rows.drain(..excess);
    }

    /// Rewrites row `index` from the end (0 is the last).
    fn rewrite(&mut self, from_end: usize, text: &str) {
        let index = self.rows.len() - 1 - from_end;
        self.rows[index] = padded(text);
    }

    /// Replaces every row, as clearing the screen or switching screens does.
    fn replace(&mut self, rows: &[&str], alternate: bool) {
        self.rows = rows.iter().map(|row| padded(row)).collect();
        self.alternate = alternate;
    }

    /// Where the screen starts: the last `height` rows.
    fn screen_start(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    fn row(&self, index: usize) -> String {
        self.rows.get(index).cloned().unwrap_or_default()
    }

    /// Where the anchor's top row is, searched as the program does: the
    /// more distinctive row, nearest the screen first, with its partner
    /// beside it.
    fn find(&self, memory: &Memory) -> Option<usize> {
        let top = memory.top_row.trim_end();
        let next = memory.next_row.trim_end();
        if top.is_empty() && next.is_empty() {
            return None;
        }
        let by_next = top.is_empty() || (!next.is_empty() && next.len() > top.len());
        let last = (self.screen_start() + 1).min(self.rows.len().saturating_sub(1));
        (0..=last)
            .rev()
            .filter(|&index| {
                let sought = if by_next {
                    &memory.next_row
                } else {
                    &memory.top_row
                };
                self.row(index) == *sought
            })
            .take(MATCHES)
            .find_map(|index| {
                if by_next {
                    (index > 0 && self.row(index - 1) == memory.top_row).then(|| index - 1)
                } else {
                    let pairs =
                        memory.next_row.is_empty() || self.row(index + 1) == memory.next_row;
                    pairs.then_some(index)
                }
            })
    }
}

impl ScreenSource for Sim {
    type Error = std::convert::Infallible;

    fn read(
        &mut self,
        anchor: Option<&Memory>,
        seen_rows: u32,
        head_wanted: u32,
    ) -> Result<ScreenText, Self::Error> {
        // A view that moves a row down while the screen is read: the range
        // was taken where the screen started before the move.
        let moved = std::mem::take(&mut self.view_moved);
        let start = self.screen_start().saturating_sub(usize::from(moved));
        let end = (start + self.height).min(self.rows.len());
        let text: String = self.rows[start..end].concat();
        let mut screen = ScreenText {
            text,
            top_row: self.row(start),
            next_row: if start + 1 < self.rows.len() {
                self.row(start + 1)
            } else {
                String::new()
            },
            alternate: self.alternate || start == 0,
            settled: !std::mem::take(&mut self.disturbed),
            view_moved: moved,
            ..ScreenText::default()
        };
        if let Some(memory) = anchor {
            // With no history, before or now, the range kept at the top row
            // finds it at once: the rows stay put, whatever text moved
            // through them.
            if memory.alternate && screen.alternate {
                screen.shift = Some(0);
                return Ok(screen);
            }
            match self.find(memory) {
                Some(found) if found <= start => {
                    let shift = start - found;
                    screen.shift = Some(u32::try_from(shift).unwrap());
                    let seen = seen_rows as usize;
                    if seen > 0 {
                        screen.old_last_row = self.row(found + seen - 1);
                    }
                    let unread = shift.saturating_sub(seen).min(head_wanted as usize);
                    if unread > 0 {
                        screen.head = self.rows[found + seen..found + seen + unread].concat();
                        screen.head_rows = u32::try_from(unread).unwrap();
                    }
                }
                _ if memory.top_row.trim().is_empty() && memory.next_row.trim().is_empty() => {}
                _ => screen.document_rows = Some(u32::try_from(self.rows.len()).unwrap()),
            }
        }
        Ok(screen)
    }
}

/// Reads `sim` after `memory`, expecting a settled read.
fn read(sim: &mut Sim, memory: Option<&Memory>) -> (TerminalOutput, Memory) {
    match read_new(sim, memory, ReadMode::Change, HEAD).unwrap() {
        Found::Output(output, memory) => (output, memory),
        Found::Unsettled => panic!("the read settles"),
    }
}

fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

#[test]
fn a_first_read_and_a_baseline_find_nothing_new() {
    let mut sim = Sim::new(4, 100, &["ready>"]);
    let (output, memory) = read(&mut sim, None);
    assert!(output.is_empty());
    assert_eq!(memory.screen, ["ready>"]);
    sim.push(&["more"]);
    let Found::Output(output, _) =
        read_new(&mut sim, Some(&memory), ReadMode::Baseline, HEAD).unwrap()
    else {
        panic!("a baseline settles");
    };
    assert!(output.is_empty());
}

#[test]
fn output_on_a_screen_not_yet_full_is_new() {
    let mut sim = Sim::new(5, 100, &["ready>"]);
    let (_, memory) = read(&mut sim, None);
    sim.rewrite(0, "ready> echo hi");
    sim.push(&["hi", "ready>"]);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(
        output.changed.as_ref().map(|change| change.text.as_str()),
        Some(" echo hi")
    );
    assert_eq!(output.lines, ["hi", "ready>"]);
    assert_eq!(output.skipped, None);
    assert_eq!(memory.screen, ["ready> echo hi", "hi", "ready>"]);
}

#[test]
fn a_flood_within_the_history_is_counted_exactly() {
    let mut sim = Sim::new(3, 100, &["a", "b", "c"]);
    let (_, memory) = read(&mut sim, None);
    let lines: Vec<String> = (1..=10).map(|line| format!("line {line}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    sim.push(&lines);
    let (output, memory) = read(&mut sim, Some(&memory));
    // Ten new lines: the first three read, four counted, the last three on
    // the screen.
    assert_eq!(output.head, ["line 1", "line 2", "line 3"]);
    assert_eq!(output.skipped, Some(Skipped::Count(4)));
    assert_eq!(output.lines, ["line 8", "line 9", "line 10"]);
    assert_eq!(output.changed, None);
    assert_eq!(output.above, Vec::<String>::new());
    assert_eq!(memory.screen, ["line 8", "line 9", "line 10"]);
}

#[test]
fn a_screen_a_flood_scrolled_away_whole_is_not_taken_for_the_main_screen() {
    // A screen with no history yet, as a new console host's, read as the
    // flood's first line is written.
    let mut sim = Sim::new(3, 100, &["a", "b", "line 1"]);
    let (_, memory) = read(&mut sim, None);
    // Read after the flood scrolled that screen away whole: a known shift
    // in the same text, no alternate screen, so it is no main screen.
    let first: Vec<String> = (2..=9).map(|line| format!("line {line}")).collect();
    sim.push(&first.iter().map(String::as_str).collect::<Vec<_>>());
    let (_, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.main, None);
    // A later screen whose first line starts with the line that ended the
    // first screen ("line 1"), "line 17", is still the flood, counted.
    let rest: Vec<String> = (10..=19).map(|line| format!("line {line}")).collect();
    sim.push(&rest.iter().map(String::as_str).collect::<Vec<_>>());
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.changed, None);
    assert_eq!(output.head, ["line 10", "line 11", "line 12"]);
    assert_eq!(output.skipped, Some(Skipped::Count(4)));
    assert_eq!(output.lines, ["line 17", "line 18", "line 19"]);
}

#[test]
fn a_flood_after_typing_speaks_the_command_line_that_scrolled_away() {
    let mut sim = Sim::new(3, 100, &["a", "b", "ready>"]);
    let (_, memory) = read(&mut sim, None);
    sim.rewrite(0, "ready> run");
    let lines: Vec<String> = (1..=6).map(|line| format!("line {line}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    sim.push(&lines);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(
        output.changed.as_ref().map(|change| change.text.as_str()),
        Some(" run")
    );
    assert_eq!(output.head, ["line 1", "line 2", "line 3"]);
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, ["line 4", "line 5", "line 6"]);
}

#[test]
fn a_flood_past_the_history_is_more_than_the_history_holds() {
    let mut sim = Sim::new(3, 8, &["a", "b", "c", "d"]);
    let (_, memory) = read(&mut sim, None);
    let lines: Vec<String> = (1..=20).map(|line| format!("line {line}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    sim.push(&lines);
    let (output, _) = read(&mut sim, Some(&memory));
    // The history holds 8 rows, 3 of them on the screen and spoken.
    assert_eq!(output.skipped, Some(Skipped::MoreThan(5)));
    assert_eq!(output.lines, ["line 18", "line 19", "line 20"]);
    assert_eq!(output.head, Vec::<String>::new());
}

#[test]
fn a_full_history_shifting_beneath_the_screen_is_still_counted() {
    let mut sim = Sim::new(3, 6, &["a", "b", "c", "d", "e", "f"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["g", "h"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["g", "h"]);
    assert_eq!(output.skipped, None);
}

#[test]
fn a_line_rewritten_in_place_speaks_the_word_that_changed() {
    let mut sim = Sim::new(4, 100, &["ready>", "Loading 10%"]);
    let (_, memory) = read(&mut sim, None);
    sim.rewrite(0, "Loading 20%");
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(
        output.changed.as_ref().map(|change| change.text.as_str()),
        Some("20%")
    );
    assert_eq!(output.lines, Vec::<String>::new());
}

#[test]
fn a_redraw_with_the_same_text_finds_nothing() {
    let mut sim = Sim::new(4, 100, &["ready>", "x"]);
    let (_, memory) = read(&mut sim, None);
    let (output, _) = read(&mut sim, Some(&memory));
    assert!(output.is_empty());
}

#[test]
fn a_cleared_screen_speaks_what_it_shows() {
    let mut sim = Sim::new(4, 100, &["a", "b", "ready> cls"]);
    let (_, memory) = read(&mut sim, None);
    sim.replace(&["ready>"], false);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.skipped, None);
    assert_eq!([output.above, output.lines].concat(), strings(&["ready>"]));
}

#[test]
fn an_alternate_screen_is_diffed_without_a_count() {
    let mut sim = Sim::new(3, 100, &["one", "two", "three", "ready> vim"]);
    let (_, memory) = read(&mut sim, None);
    sim.replace(&["~", "file.txt", "~"], true);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(output.skipped, None);
    assert_eq!(output.above, ["~", "file.txt"]);
    assert_eq!(
        output.changed.map(|change| change.text),
        Some("~".to_owned())
    );
    // A row changed near the top of the alternate screen.
    sim.replace(&["~", "file.txt [+]", "~"], true);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.above, ["[+]"]);
}

#[test]
fn a_disturbed_read_is_set_aside() {
    let mut sim = Sim::new(3, 100, &["a"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["b"]);
    sim.disturbed = true;
    assert_eq!(
        read_new(&mut sim, Some(&memory), ReadMode::Change, HEAD).unwrap(),
        Found::Unsettled
    );
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["b"]);
}

#[test]
fn a_read_whose_view_output_scrolled_is_trusted() {
    let mut sim = Sim::new(3, 100, &["a", "b", "c"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["d", "e"]);
    sim.view_moved = true;
    // The range was taken a row before the view's end: the rows below it
    // are read next time.
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["d"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["e"]);
}

#[test]
fn a_read_whose_view_moved_over_a_footer_redrawn_lower_keeps_the_footer() {
    let mut sim = Sim::new(3, 100, &["ready>", "one", "status: busy"]);
    let (_, memory) = read(&mut sim, None);
    // A line written into the scroll region above the footer: the view
    // moves down a row, the footer is drawn on the new last row, and the
    // line is written over the row the footer left. The read's range was
    // taken before the move, so its rows end where the footer was.
    let footer = sim.rows.pop().expect("the footer");
    sim.push(&["a"]);
    sim.rows.push(footer);
    sim.view_moved = true;
    let (output, memory) = read(&mut sim, Some(&memory));
    let spoken: Vec<String> = output
        .above
        .into_iter()
        .chain(output.changed.map(|change| change.text))
        .chain(output.head)
        .chain(output.lines)
        .collect();
    assert_eq!(spoken, strings(&["a"]));
    // The footer is still on the screen, below the rows read.
    assert_eq!(
        memory.screen,
        strings(&["ready>", "one", "a", "status: busy"])
    );
    // The next read, with the view still, finds nothing new.
    let (output, _) = read(&mut sim, Some(&memory));
    assert!(output.is_empty(), "{output:?}");
}

#[test]
fn white_space_padding_of_any_kind_is_not_text() {
    let mut sim = Sim::new(3, 100, &["ready>"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["x\u{3000}\u{a0}\t"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["x"]);
}

#[test]
fn a_scrollback_of_identical_lines_is_counted_by_the_shift() {
    let mut sim = Sim::new(3, 100, &["ready>", "y", "y"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["y", "y"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, ["y", "y"]);
}

fn output(
    changed: Option<LineChange>,
    head: &[&str],
    skipped: Option<Skipped>,
    new: &[&str],
) -> TerminalOutput {
    TerminalOutput {
        above: Vec::new(),
        changed,
        head: strings(head),
        skipped,
        lines: strings(new),
    }
}

#[test]
fn combined_output_speaks_what_was_found_above_the_last_line_first() {
    let older = TerminalOutput {
        above: strings(&["message"]),
        changed: line_change("ready> ec", "ready> echo"),
        ..TerminalOutput::default()
    };
    let newer = output(None, &[], None, &["out"]);
    assert_eq!(
        combine(older, newer, 5),
        output(None, &[], None, &["message", "ho", "out"])
    );
}

#[test]
fn combined_output_keeps_every_line_in_order_within_the_limit() {
    let older = output(None, &[], None, &["one", "two"]);
    let newer = output(None, &[], None, &["three", "ready>"]);
    assert_eq!(
        combine(older, newer, 5),
        output(None, &[], None, &["one", "two", "three", "ready>"])
    );
}

#[test]
fn combined_output_past_the_limit_keeps_its_first_and_last_lines_and_counts_the_rest() {
    let older = output(None, &["a", "b"], Some(Skipped::Count(4)), &["c", "d"]);
    let newer = output(None, &[], Some(Skipped::Count(2)), &["e", "f", "g"]);
    assert_eq!(
        combine(older, newer, 2),
        output(None, &["a", "b"], Some(Skipped::Count(9)), &["f", "g"]),
        "c, d and e are counted with the six skipped"
    );
    let older = output(None, &[], Some(Skipped::Uncounted), &["a"]);
    let newer = output(None, &[], None, &["b"]);
    assert_eq!(
        combine(older, newer, 2),
        output(None, &[], Some(Skipped::Uncounted), &["a", "b"])
    );
}

#[test]
fn a_change_to_a_line_still_waiting_puts_the_whole_line_in_its_place() {
    let grew = line_change("ready> l", "ready> ls").expect("a change");
    let older = output(None, &[], None, &["done", "ready> l"]);
    let newer = output(Some(grew.clone()), &[], None, &["file"]);
    assert_eq!(
        combine(older, newer, 5),
        output(None, &[], None, &["done", "ready> ls", "file"])
    );
    let after_blank = output(None, &[], None, &["done", ""]);
    let newer = output(Some(grew), &[], None, &[]);
    assert_eq!(
        combine(after_blank, newer, 5),
        output(None, &[], None, &["done", "", "s"]),
        "after a blank line the change is spoken as Core speaks it"
    );
}

#[test]
fn two_changes_of_the_same_line_become_one() {
    let first = line_change("ready>", "ready> l").expect("a change");
    let second = line_change("ready> l", "ready> ls").expect("a change");
    let combined = combine(
        output(Some(first.clone()), &[], None, &[]),
        output(Some(second), &[], None, &["out"]),
        5,
    );
    assert_eq!(
        combined,
        output(
            Some(LineChange {
                text: " ls".to_owned(),
                line: "ready> ls".to_owned(),
                appended: true,
                uncertain: first.uncertain,
                inserted: " ls".to_owned(),
                since_read: None,
            }),
            &[],
            None,
            &["out"]
        )
    );
    let rewritten = line_change("[###   ] 30%", "[####  ] 40%").expect("a change");
    let grew = line_change("[####  ] 40%", "[####  ] 40% done").expect("a change");
    assert_eq!(
        combine(
            output(Some(rewritten), &[], None, &[]),
            output(Some(grew), &[], None, &[]),
            5
        )
        .changed,
        Some(LineChange {
            text: "#  ] 40% done".to_owned(),
            line: "[####  ] 40% done".to_owned(),
            appended: false,
            uncertain: 0,
            inserted: "#  ] 4 done".to_owned(),
            since_read: None,
        }),
        "a rewrite then growth says the line from the word where it first differed"
    );
}

#[test]
fn output_too_large_for_one_message_is_split_in_order() {
    let small = output(None, &[], None, &["a", "b"]);
    assert_eq!(split(small.clone(), 100), [small]);
    let large = TerminalOutput {
        above: strings(&["message"]),
        changed: line_change("ready>", "ready> ls"),
        head: strings(&["h1", "h2", "h3"]),
        skipped: Some(Skipped::Count(5)),
        lines: strings(&["l1", "l2", "l3"]),
    };
    let parts = split(large, 30);
    assert_eq!(
        parts,
        [
            TerminalOutput {
                above: strings(&["message"]),
                changed: line_change("ready>", "ready> ls"),
                ..TerminalOutput::default()
            },
            // Ten bytes a line here, so three lines fill a part.
            output(None, &[], None, &["h1", "h2", "h3"]),
            output(None, &[], Some(Skipped::Count(5)), &["l1"]),
            output(None, &[], None, &["l2", "l3"]),
        ]
    );
}

#[test]
fn typing_above_a_status_line_is_the_changed_line() {
    use super::screen::{Shift, diff};
    let old = strings(&["ready> ech", "-- 12:00 --"]);
    // Typing on the prompt, with the caret there, while the status line
    // below it ticks.
    let new = strings(&["ready> echo", "-- 12:01 --"]);
    let found = diff(&old, &new, Shift::Known(0), Some(0));
    assert_eq!(
        found.changed.map(|change| (change.text, change.appended)),
        Some(("o".to_owned(), true))
    );
    assert_eq!(found.above, ["01 --"]);
    // With the status line unchanged, the prompt's change is still the
    // changed line, not the last line's.
    let new = strings(&["ready> echo", "-- 12:00 --"]);
    let found = diff(&old, &new, Shift::Known(0), Some(0));
    assert_eq!(
        found.changed.map(|change| (change.text, change.appended)),
        Some(("o".to_owned(), true))
    );
    assert_eq!(found.above, Vec::<String>::new());
    assert_eq!(found.below, Vec::<String>::new());
}

#[test]
fn a_rewrite_read_half_done_speaks_the_word_that_changed_once_done() {
    let mut sim = Sim::new(3, 100, &["one", "two", "ready>", "Loading 50%"]);
    let (_, memory) = read(&mut sim, None);
    // Read while the line was cleared and only its first letter written.
    sim.rewrite(0, "L");
    let (output, memory) = read(&mut sim, Some(&memory));
    assert!(output.is_empty());
    sim.rewrite(0, "Loading 51%");
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(
        output.changed.map(|change| change.text),
        Some("51%".to_owned())
    );
}

#[test]
fn a_line_typed_back_to_what_it_said_says_nothing_but_shows_the_typing() {
    let mut sim = Sim::new(3, 100, &["one", "two", "ready>", "ready> ls"]);
    let (_, memory) = read(&mut sim, None);
    // Backspace, then the same letter typed again.
    sim.rewrite(0, "ready> l");
    let (output, memory) = read(&mut sim, Some(&memory));
    assert!(output.is_empty());
    sim.rewrite(0, "ready> ls");
    let (output, _) = read(&mut sim, Some(&memory));
    let change = output.changed.expect("a change");
    assert_eq!(change.text, "");
    assert_eq!(
        change.since_read.map(|since| (since.text, since.appended)),
        Some(("s".to_owned(), true))
    );
}

#[test]
fn a_line_cleared_and_typed_again_shows_the_typing_since_it_was_read() {
    // "echo hi" cleared with Escape, then " o" typed after "echo": from
    // what the line said, a rewrite of its last word; since it was read,
    // " o" added.
    let mut sim = Sim::new(3, 100, &["one", "two", "ready>", "ready> echo hi"]);
    let (_, memory) = read(&mut sim, None);
    sim.rewrite(0, "ready> echo");
    let (output, memory) = read(&mut sim, Some(&memory));
    assert!(output.is_empty());
    sim.rewrite(0, "ready> echo o");
    let (output, _) = read(&mut sim, Some(&memory));
    let change = output.changed.expect("a change");
    assert_eq!((change.text.as_str(), change.appended), ("o", false));
    assert_eq!(
        change.since_read.map(|since| (since.text, since.appended)),
        Some((" o".to_owned(), true))
    );
}

#[test]
fn a_character_typed_mid_line_after_a_cleared_screen_is_the_insertion() {
    let mut sim = Sim::new(3, 100, &["one", "two", "three", "ready> abd"]);
    let (_, memory) = read(&mut sim, None);
    // `cls`: the screen holds only the prompt, nothing above it.
    sim.replace(&["ready> abd"], false);
    let (_, memory) = read(&mut sim, Some(&memory));
    sim.rewrite(0, "ready> abcd");
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(
        output
            .changed
            .map(|change| (change.text, change.appended, change.inserted)),
        Some(("abcd".to_owned(), false, "c".to_owned()))
    );
    assert!(output.above.is_empty() && output.lines.is_empty());
}

#[test]
fn a_screen_cleared_down_to_its_prompt_speaks_the_prompt() {
    use super::screen::{Shift, diff};
    // `cls`: the old screen's lines give way to the prompt alone, which
    // starts as the old last line did; it is new, not that line shortened.
    let old = strings(&["ready> echo hi", "hi", "ready> cls"]);
    let new = strings(&["ready>"]);
    let found = diff(&old, &new, Shift::Unknown, None);
    assert_eq!(found.changed, None);
    assert_eq!([found.above, found.below].concat(), strings(&["ready>"]));
}

#[test]
fn closing_an_alternate_screen_speaks_only_what_followed_on_the_main_screen() {
    let main = ["one", "two", "ready>", "ready> .\\alt.ps1"];
    let mut sim = Sim::new(3, 100, &main);
    let (_, memory) = read(&mut sim, None);
    // The program opens its alternate screen, which is read cleared before
    // it is drawn.
    sim.replace(&[""], true);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert!(output.is_empty());
    sim.replace(&["row 1", "row 2", "status: ready"], true);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(output.changed, None);
    assert_eq!(
        [output.above, output.lines].concat(),
        strings(&["row 1", "row 2", "status: ready"])
    );
    // And closes it: the main screen is back, scrolled by the program's
    // last line and the prompt.
    sim.replace(&main, false);
    sim.push(&["closed", "ready>"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(
        [output.above, output.lines].concat(),
        strings(&["closed", "ready>"])
    );
    assert_eq!(output.changed, None);
}

#[test]
fn a_row_erased_and_written_again_is_new() {
    let mut sim = Sim::new(4, 100, &["one", "two", "three", "row 28", "status"]);
    let (_, memory) = read(&mut sim, None);
    sim.rewrite(1, "");
    let (_, memory) = read(&mut sim, Some(&memory));
    sim.rewrite(1, "row 30");
    let (output, _) = read(&mut sim, Some(&memory));
    let spoken: Vec<String> = output
        .above
        .into_iter()
        .chain(output.changed.map(|change| change.text))
        .chain(output.lines)
        .collect();
    assert_eq!(spoken, strings(&["row 30"]));
}

#[test]
fn a_footer_kept_below_a_scroll_region_is_said_only_as_it_changed() {
    let mut sim = Sim::new(3, 100, &["ready>", "one", "status: busy"]);
    let (_, memory) = read(&mut sim, None);
    // Lines written into a scroll region above the footer, which stays on
    // the last row as the rows above it scroll into the history, then the
    // footer redrawn.
    let footer = sim.rows.pop().expect("the footer");
    sim.push(&["a", "b", "c", "d", "e"]);
    sim.rows.push(footer);
    sim.rewrite(0, "status: done");
    let (output, _) = read(&mut sim, Some(&memory));
    let spoken: Vec<String> = output
        .changed
        .map(|change| change.text)
        .into_iter()
        .chain(output.head)
        .chain(output.lines)
        .collect();
    assert_eq!(spoken, strings(&["a", "b", "c", "d", "e", "done"]));
}

/// A pager's alternate screen, `rows` content lines from `first`, and its
/// prompt on the last row.
fn pager(first: usize, rows: usize) -> Vec<String> {
    let mut screen: Vec<String> = (first..first + rows).map(|n| format!("p{n}")).collect();
    screen.push(":".to_owned());
    screen
}

#[test]
fn a_pager_moving_down_and_up_a_line_speaks_only_the_new_line() {
    let mut sim = Sim::new(6, 100, &["ready> less"]);
    let (_, memory) = read(&mut sim, None);
    let page = |first| pager(first, 5);
    sim.replace(
        &page(1).iter().map(String::as_str).collect::<Vec<_>>(),
        true,
    );
    let (_, memory) = read(&mut sim, Some(&memory));
    // Down: the text scrolls up through the rows, which stay put.
    sim.replace(
        &page(2).iter().map(String::as_str).collect::<Vec<_>>(),
        true,
    );
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.scrolled, Some(1));
    assert_eq!(output.changed, None);
    assert_eq!([output.above, output.lines].concat(), strings(&["p6"]));
    // Up: the text scrolls down a row again.
    sim.replace(
        &page(1).iter().map(String::as_str).collect::<Vec<_>>(),
        true,
    );
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.scrolled, Some(0));
    assert_eq!(output.changed, None);
    assert_eq!([output.above, output.lines].concat(), strings(&["p1"]));
}

#[test]
fn a_line_cut_short_keeps_what_it_said_on_its_own_row_after_a_scroll() {
    // A pager's rows, the first one's end being rewritten as the screen
    // scrolls: what a row said goes with the text, not with the row.
    let mut sim = Sim::new(4, 100, &["ready> less"]);
    let (_, memory) = read(&mut sim, None);
    sim.replace(&["abc def", "abc", "xyz", ":"], true);
    let (_, memory) = read(&mut sim, Some(&memory));
    sim.replace(&["abc", "xyz", "uvw", ":"], true);
    let (_, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.said, strings(&["abc", "xyz", "uvw", ":"]));
    // Had the row kept the "abc def" it said before the scroll, only "x"
    // would be new.
    sim.replace(&["abc defx", "xyz", "uvw", ":"], true);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.above, strings(&["defx"]));
}

#[test]
fn rows_not_yet_written_and_still_blank_above_a_footer_are_not_lines() {
    // A screen with rows not yet written to below the prompt, cleared and
    // given a footer on its last row: the rows between were blank before
    // and are blank now, and are not inserted lines.
    let mut sim = Sim::new(6, 100, &["ready> footer", "", "", "", "", ""]);
    let (_, memory) = read(&mut sim, None);
    assert_eq!(memory.screen, strings(&["ready> footer"]));
    assert_eq!(memory.unwritten, 5);
    sim.replace(&["", "", "", "", "", "status: busy"], false);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.above, Vec::<String>::new());
    assert_eq!(output.changed, None);
    assert_eq!(output.lines, strings(&["status: busy"]));
}

#[test]
fn a_blank_line_on_a_row_that_scrolled_into_view_counts() {
    // The screen full, so every row the output writes scrolls in: a blank
    // line among them was never a row of the old screen.
    let mut sim = Sim::new(3, 100, &["one", "two", "ready>"]);
    let (_, memory) = read(&mut sim, None);
    assert_eq!(memory.unwritten, 0);
    sim.push(&["a", "", "b"]);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.lines, strings(&["a", "", "b"]));
}

#[test]
fn a_screen_cleared_into_the_history_keeps_its_unwritten_rows_out() {
    // The console host clears its screen by scrolling it into the history:
    // the prompt goes up a row, and the rows not yet written to, still
    // blank above the footer drawn on the last row, are not lines.
    let mut sim = Sim::new(6, 100, &["ready> footer", "", "", "", "", ""]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["status: busy"]);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.scrolled, Some(1));
    assert_eq!(output.above, Vec::<String>::new());
    assert_eq!(output.lines, strings(&["status: busy"]));
}

#[test]
fn a_flood_after_a_wrapped_line_counts_the_rows_it_took() {
    // A line that wrapped onto two rows is on the screen as a flood starts:
    // the rows the old screen held are four, not its three lines, so only
    // the rows past them went by unread.
    let mut sim = Sim::new(4, 100, &["one"]);
    sim.push_wrapped("a line of thirty characters!!!");
    sim.push(&["ready>"]);
    let (_, memory) = read(&mut sim, None);
    assert_eq!(memory.screen.len(), 3);
    assert_eq!(memory.rows, [1, 2, 1]);
    assert_eq!(memory.rows_held(), 4);
    let lines: Vec<String> = (1..=6).map(|line| format!("l{line}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    sim.push(&lines);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.head, strings(&["l1", "l2"]));
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, strings(&["l3", "l4", "l5", "l6"]));
    assert_eq!(output.changed, None);
}

#[test]
fn a_wrapped_line_scrolled_part_way_off_the_top_is_not_new() {
    // Two rows scrolled away: the first line and the first row of the
    // wrapped line, whose second row is now the screen's top.
    let mut sim = Sim::new(4, 100, &["one"]);
    sim.push_wrapped("a line of thirty characters!!!");
    sim.push(&["ready>"]);
    let (_, memory) = read(&mut sim, None);
    sim.push(&["x", "y"]);
    let (output, memory) = read(&mut sim, Some(&memory));
    assert_eq!(memory.scrolled, Some(2));
    assert_eq!(output.above, Vec::<String>::new());
    assert_eq!(output.lines, strings(&["x", "y"]));
}

#[test]
fn a_footer_row_a_flood_wrote_over_is_a_new_line_not_the_footer_changed() {
    // A flood above a footer kept on the last row, read twice: by the
    // second read the row the footer was on holds a line of the flood,
    // which is new, the first after the old screen, and not the footer
    // rewritten (which Core would put in place of the newest line waiting).
    let mut sim = Sim::new(3, 100, &["ready>", "one", "status: busy"]);
    let (_, memory) = read(&mut sim, None);
    let footer = sim.rows.pop().expect("the footer");
    sim.push(&["a", "b", "c", "d", "e"]);
    sim.rows.push(footer.clone());
    let (_, memory) = read(&mut sim, Some(&memory));
    sim.rows.pop();
    sim.push(&["f", "g", "h", "i", "j"]);
    sim.rows.push(footer);
    let (output, _) = read(&mut sim, Some(&memory));
    assert_eq!(output.changed, None);
    assert_eq!(output.head, strings(&["f", "g", "h"]));
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, strings(&["i", "j"]));
}
