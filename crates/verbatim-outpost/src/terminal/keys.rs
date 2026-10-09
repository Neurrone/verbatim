//! What a key judged by where the caret landed (Escape, Control+U) did to
//! a typed line that had wrapped onto more rows, judged from the screen
//! before the key and the screen as read now (`phase6-design.md`,
//! "Selection lists in a terminal", the wrapped Escape).

use super::Memory;

/// Row `row` of `screen`, empty past its end.
fn line(screen: &[String], row: usize) -> &str {
    screen.get(row).map_or("", String::as_str)
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

    fn screen<S: AsRef<str>>(lines: &[S], caret: Option<usize>) -> Memory {
        Memory {
            screen: lines.iter().map(|line| line.as_ref().to_owned()).collect(),
            caret,
            scrolled: Some(0),
            ..Memory::default()
        }
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
}
