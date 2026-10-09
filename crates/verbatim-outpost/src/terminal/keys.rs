//! What a line key (Up or Down Arrow) did in a terminal, judged from the
//! screen before the key and the screen as read now (`phase6-design.md`,
//! "Selection lists in a terminal").
//!
//! A program answers a line key by redrawing: a shell writes the line
//! recalled from its history, an editor moves the caret, and a selection
//! list moves its marker, by rewriting two lines or its whole region. The
//! terminal shows each write as it is made, so a read can find a redraw
//! half done: the caret moved to the line the marker is leaving, or the
//! marker erased and not yet drawn again. There is no event that says the
//! program has finished; the screen itself is the evidence. A read is the
//! key's effect when it shows one of these, compared with the screen
//! before the key, and otherwise the key waits for the next read:
//!
//! - A marker moved: a line gained text that another line lost. Every
//!   such line, top to bottom, is the answer, so a key that moves two
//!   markers says both lines.
//! - The caret's line gained text (a line recalled from history).
//! - The caret's line was cut short, its end removed, and nothing else
//!   changed (a shorter line recalled).
//! - The caret moved to the line next to the one it was on, and no line
//!   only lost text (an editor's caret, its status line perhaps redrawn).
//!
//! A read that shows only text removed, a marker erased from more lines
//! than it has been drawn on, or the caret somewhere else over unchanged
//! lines, is a redraw under way. A screen that scrolled, or was replaced,
//! is not compared by line: the caret's line is the answer.

use super::Memory;

/// What a line key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyEffect {
    /// The caret's line shows it.
    CaretLine,
    /// These lines show it, top to bottom: each line that gained a marker
    /// another line lost, the caret's line among them when it is one.
    Redrawn(Vec<String>),
}

/// What changed on one line: the text removed and the text inserted in
/// its place, between their common start and end.
struct Edit<'a> {
    removed: &'a str,
    inserted: &'a str,
}

impl Edit<'_> {
    fn gained(&self) -> bool {
        !self.inserted.trim().is_empty()
    }

    fn lost(&self) -> bool {
        !self.removed.trim().is_empty()
    }
}

/// How `old` became `new`, or `None` when they are the same.
fn edit<'a>(old: &'a str, new: &'a str) -> Option<Edit<'a>> {
    if old == new {
        return None;
    }
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    Some(Edit {
        removed: &old[prefix..old.len() - suffix],
        inserted: &new[prefix..new.len() - suffix],
    })
}

/// Row `row` of `screen`, empty past its end.
fn line(screen: &[String], row: usize) -> &str {
    screen.get(row).map_or("", String::as_str)
}

/// What a line key did, from the screen `before` it and the screen read
/// `now`, with the caret's line on each; `None` while a redraw is under way
/// or nothing has changed yet (the module's rules).
#[must_use]
pub fn line_key_effect(before: &Memory, now: &Memory) -> Option<KeyEffect> {
    if now.scrolled != Some(0) || now.alternate != before.alternate {
        return Some(KeyEffect::CaretLine);
    }
    let rows = before.screen.len().max(now.screen.len());
    let edits: Vec<(usize, Edit<'_>)> = (0..rows)
        .filter_map(|row| {
            edit(line(&before.screen, row), line(&now.screen, row)).map(|edit| (row, edit))
        })
        .collect();
    // How many lines lost `text`, and how many gained it.
    let losing = |text: &str| {
        edits
            .iter()
            .filter(|(_, edit)| edit.lost() && edit.removed.trim() == text)
            .count()
    };
    let gaining = |text: &str| {
        edits
            .iter()
            .filter(|(_, edit)| edit.gained() && edit.inserted.trim() == text)
            .count()
    };
    let marked: Vec<(usize, &str)> = edits
        .iter()
        .filter(|(row, edit)| {
            edit.gained()
                && edits.iter().any(|(other, lost)| {
                    other != row && lost.lost() && lost.removed.trim() == edit.inserted.trim()
                })
        })
        .map(|(row, edit)| (*row, edit.inserted.trim()))
        .collect();
    if !marked.is_empty() {
        // A marker erased from more lines than it is drawn on yet.
        if marked.iter().any(|(_, text)| losing(text) > gaining(text)) {
            return None;
        }
        if let [(row, _)] = marked.as_slice()
            && now.caret == Some(*row)
        {
            return Some(KeyEffect::CaretLine);
        }
        return Some(KeyEffect::Redrawn(
            marked
                .iter()
                .map(|(row, _)| line(&now.screen, *row).to_owned())
                .collect(),
        ));
    }
    let caret_edit = now
        .caret
        .and_then(|caret| edits.iter().find(|(row, _)| *row == caret));
    if caret_edit.is_some_and(|(_, edit)| edit.gained()) {
        return Some(KeyEffect::CaretLine);
    }
    // A line cut short: what is left is the start of what was there.
    if let (Some((row, edit)), [_]) = (caret_edit, edits.as_slice())
        && edit.inserted.is_empty()
        && line(&before.screen, *row).starts_with(line(&now.screen, *row))
    {
        return Some(KeyEffect::CaretLine);
    }
    let only_lost = edits.iter().any(|(_, edit)| edit.lost() && !edit.gained());
    let next_line =
        matches!((before.caret, now.caret), (Some(was), Some(is)) if was.abs_diff(is) == 1);
    (next_line && !only_lost).then_some(KeyEffect::CaretLine)
}

/// What a key judged by where the caret landed (Escape, Control+U) did to
/// a typed line that had wrapped onto more rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WrappedRemoval {
    /// It removed this text from the end of the line.
    Removed(String),
    /// The line was cut short, but the row after it still shows the rest
    /// of it: the redraw is under way.
    UnderWay,
}

/// What a key judged by where the caret landed did, from the screen
/// `before` it and the screen read `now`, when its caret left the row it
/// was on: a terminal's text gives a line that wrapped onto more rows
/// whole, so when the caret is on the same line of the screen and that
/// line was cut short, the key removed the rest of it, unless the next
/// line still shows that rest, a redraw under way. `None` otherwise, when
/// the key is judged by the caret's row alone.
#[must_use]
pub fn wrapped_removal(before: &Memory, now: &Memory) -> Option<WrappedRemoval> {
    let (Some(was), Some(is)) = (before.caret, now.caret) else {
        return None;
    };
    if is != was || now.scrolled != Some(0) {
        return None;
    }
    let typed = line(&before.screen, was);
    let left = line(&now.screen, is);
    if left.len() >= typed.len() || !typed.starts_with(left) {
        return None;
    }
    let rest = line(&now.screen, is + 1);
    if !rest.trim().is_empty() && typed.ends_with(rest) {
        return Some(WrappedRemoval::UnderWay);
    }
    Some(WrappedRemoval::Removed(typed[left.len()..].to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: [&str; 6] = [
        "Pick a fruit:",
        "> apple",
        "  banana",
        "  cherry",
        "  date",
        "",
    ];

    fn screen<S: AsRef<str>>(lines: &[S], caret: Option<usize>) -> Memory {
        Memory {
            screen: lines.iter().map(|line| line.as_ref().to_owned()).collect(),
            caret,
            scrolled: Some(0),
            ..Memory::default()
        }
    }

    /// `lines` with each row given rewritten.
    fn with(lines: &[&str], rows: &[(usize, &str)]) -> Vec<String> {
        let mut lines: Vec<String> = lines.iter().map(|&line| line.to_owned()).collect();
        for &(row, text) in rows {
            text.clone_into(&mut lines[row]);
        }
        lines
    }

    #[test]
    fn a_marker_moved_on_another_line_is_that_line() {
        let before = screen(&LIST, Some(5));
        let lines = with(&LIST, &[(1, "  apple"), (2, "> banana")]);
        assert_eq!(
            line_key_effect(&before, &screen(&lines, Some(5))),
            Some(KeyEffect::Redrawn(vec!["> banana".to_owned()]))
        );
    }

    /// Two lists, each with a marker on its first item.
    const TWO_LISTS: [&str; 7] = [
        "Fruit:", "> apple", "  banana", "Colour:", "> red", "  green", "",
    ];

    #[test]
    fn every_line_that_gained_a_marker_is_said_top_to_bottom() {
        let moved = with(
            &TWO_LISTS,
            &[
                (1, "  apple"),
                (2, "> banana"),
                (4, "  red"),
                (5, "> green"),
            ],
        );
        // With the caret on neither, and on the second, which is said in
        // its place.
        for caret in [6, 5] {
            assert_eq!(
                line_key_effect(&screen(&TWO_LISTS, Some(6)), &screen(&moved, Some(caret))),
                Some(KeyEffect::Redrawn(vec![
                    "> banana".to_owned(),
                    "> green".to_owned()
                ]))
            );
        }
    }

    #[test]
    fn a_marker_erased_from_more_lines_than_it_is_drawn_on_is_under_way() {
        let before = screen(&TWO_LISTS, Some(6));
        // Both markers erased, and one drawn again.
        let half = with(&TWO_LISTS, &[(1, "  apple"), (2, "> banana"), (4, "  red")]);
        assert_eq!(line_key_effect(&before, &screen(&half, Some(2))), None);
    }

    #[test]
    fn a_marker_moved_to_the_caret_is_the_caret_line() {
        let before = screen(&LIST, Some(1));
        let lines = with(&LIST, &[(1, "  apple"), (2, "> banana")]);
        assert_eq!(
            line_key_effect(&before, &screen(&lines, Some(2))),
            Some(KeyEffect::CaretLine)
        );
    }

    #[test]
    fn a_marker_drawn_before_the_old_one_is_erased_is_the_caret_line() {
        let before = screen(&LIST, Some(1));
        let lines = with(&LIST, &[(2, "> banana")]);
        assert_eq!(
            line_key_effect(&before, &screen(&lines, Some(2))),
            Some(KeyEffect::CaretLine)
        );
    }

    #[test]
    fn a_redraw_under_way_is_not_the_effect() {
        let before = screen(&LIST, Some(1));
        // The caret moved to the line the marker leaves, far from where it was.
        assert_eq!(line_key_effect(&before, &screen(&LIST, Some(4))), None);
        // The caret moved within its line.
        assert_eq!(line_key_effect(&before, &screen(&LIST, Some(1))), None);
        // The marker erased and the caret on the next line.
        let erased = with(&LIST, &[(1, "  apple")]);
        assert_eq!(line_key_effect(&before, &screen(&erased, Some(1))), None);
        assert_eq!(line_key_effect(&before, &screen(&erased, Some(2))), None);
    }

    #[test]
    fn a_line_recalled_on_the_caret_line_is_the_caret_line() {
        let before = screen(&["PS> ", ""], Some(0));
        let now = screen(&["PS> git status", ""], Some(0));
        assert_eq!(line_key_effect(&before, &now), Some(KeyEffect::CaretLine));
    }

    #[test]
    fn a_shorter_line_recalled_is_the_caret_line() {
        let before = screen(&["PS> git status -s", ""], Some(0));
        let now = screen(&["PS> git status", ""], Some(0));
        assert_eq!(line_key_effect(&before, &now), Some(KeyEffect::CaretLine));
    }

    #[test]
    fn a_caret_moved_a_line_over_unchanged_text_is_the_caret_line() {
        let lines = ["one", "two", "three", "status 1,1"];
        let before = screen(&lines, Some(0));
        assert_eq!(
            line_key_effect(&before, &screen(&lines, Some(1))),
            Some(KeyEffect::CaretLine)
        );
        // With a status line redrawn, which gains as much as it loses.
        let moved = with(&lines, &[(3, "status 2,1")]);
        assert_eq!(
            line_key_effect(&before, &screen(&moved, Some(1))),
            Some(KeyEffect::CaretLine)
        );
    }

    #[test]
    fn a_wrapped_line_removed_says_all_it_removed() {
        // The terminal gives the line that wrapped whole.
        let before = screen(&["ready> echo aaaaaaxyz"], Some(0));
        let cut = screen(&["ready>"], Some(0));
        assert_eq!(
            wrapped_removal(&before, &cut),
            Some(WrappedRemoval::Removed(" echo aaaaaaxyz".to_owned()))
        );
        // The first row cut, the second not yet erased.
        let under_way = screen(&["ready>", "aaxyz"], Some(0));
        assert_eq!(
            wrapped_removal(&before, &under_way),
            Some(WrappedRemoval::UnderWay)
        );
        // The caret moved over the line left as it was.
        assert_eq!(wrapped_removal(&before, &before), None);
    }

    #[test]
    fn a_screen_that_scrolled_is_the_caret_line() {
        let before = screen(&LIST, Some(1));
        let mut now = screen(&LIST, Some(1));
        now.scrolled = Some(1);
        assert_eq!(line_key_effect(&before, &now), Some(KeyEffect::CaretLine));
    }
}
