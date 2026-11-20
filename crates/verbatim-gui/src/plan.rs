//! Widget-free helpers for the settings GUI: accessible names, category
//! cycling and the list dialog's first selection, which are easy to get
//! wrong and worth testing without a display.

/// Strips wxWidgets accelerator markers from a label so it can serve as an
/// accessible name.
///
/// A single `&` marks the next character as the mnemonic and is not part of the
/// visible text; `&&` is a literal ampersand. Controls whose accessible name we
/// set explicitly need this stripped form, or the marker is spoken aloud.
#[must_use]
pub fn accessible_name(label: &str) -> String {
    let mut name = String::with_capacity(label.len());
    let mut characters = label.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '&' {
            if characters.peek() == Some(&'&') {
                characters.next();
                name.push('&');
            }
            // A lone `&` is the mnemonic marker; drop it.
        } else {
            name.push(character);
        }
    }
    name
}

/// The next index when cycling categories with wraparound.
///
/// `forward` moves toward the end (Ctrl+Tab), `!forward` toward the start
/// (Ctrl+Shift+Tab). Returns `current` unchanged for an empty list.
#[must_use]
pub fn cycle_index(current: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return current;
    }
    if forward {
        (current + 1) % len
    } else {
        (current + len - 1) % len
    }
}

/// The list selection a freshly opened list dialog starts with: the first
/// item when the list is non-empty, so keyboard users land on a selected
/// item rather than an unselected list (mirroring NVDA's systrayList).
#[must_use]
pub fn initial_list_selection(item_count: usize) -> Option<usize> {
    (item_count > 0).then_some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_name_drops_mnemonic_markers() {
        // The live GUI exposed this: a check box whose label kept its `&` marker
        // reported the wrong accessible name, so the name we set must be clean.
        assert_eq!(accessible_name("Rate boos&t"), "Rate boost");
        assert_eq!(accessible_name("&Voice"), "Voice");
        assert_eq!(accessible_name("V&olume"), "Volume");
        assert_eq!(accessible_name("Synthesizer"), "Synthesizer");
        assert_eq!(
            accessible_name("Salt && Pepper"),
            "Salt & Pepper",
            "a doubled ampersand is a literal one"
        );
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        assert_eq!(cycle_index(0, 3, true), 1);
        assert_eq!(cycle_index(2, 3, true), 0, "forward wraps past the end");
        assert_eq!(cycle_index(0, 3, false), 2, "backward wraps past the start");
        assert_eq!(cycle_index(1, 3, false), 0);
        assert_eq!(cycle_index(0, 0, true), 0, "an empty list stays put");
        assert_eq!(
            cycle_index(5, 1, true),
            0,
            "a single category maps to itself"
        );
    }

    #[test]
    fn list_selection_starts_on_the_first_item() {
        assert_eq!(initial_list_selection(3), Some(0));
        assert_eq!(initial_list_selection(1), Some(0));
        assert_eq!(
            initial_list_selection(0),
            None,
            "an empty list has nothing to select"
        );
    }
}
