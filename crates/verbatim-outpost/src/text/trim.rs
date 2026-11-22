//! A trimmed read's text (`trim` on `ReadText` and `ReadRange`, and
//! `Visible`'s): each line without the spaces that pad it, with each
//! line's padded length, as a terminal's rows are read.

use verbatim_model::TerminalText;

/// `units`, UTF-16 as a terminal's provider gives its text, as UTF-8
/// without the spaces that pad its lines, with each line's padded length,
/// exactly as `TerminalText::unpadded(&String::from_utf16_lossy(units))`
/// makes it, an unpaired surrogate becoming U+FFFD, but in one pass and
/// with no copy of the padded text between. The text starts with room for
/// one byte a code unit, which ASCII text never outgrows, since padding is
/// only removed.
#[must_use]
pub fn unpadded_utf16(units: &[u16]) -> TerminalText {
    let mut into = TerminalText {
        text: String::with_capacity(units.len()),
        lengths: Vec::new(),
    };
    let TerminalText { text, lengths } = &mut into;
    // The line's length so far in characters, padding included; the spaces
    // after its last other character, written only once another follows;
    // and a carriage return that may begin a line break.
    let mut length: u32 = 0;
    let mut spaces: u32 = 0;
    let mut carriage_return = false;
    let push = |text: &mut String, spaces: &mut u32, length: &mut u32, character: char| {
        for _ in 0..*spaces {
            text.push(' ');
        }
        *spaces = 0;
        text.push(character);
        *length += 1;
    };
    let end_line = |lengths: &mut Vec<(u32, u32)>, length: u32| match lengths.last_mut() {
        Some((count, last)) if *last == length => *count += 1,
        _ => lengths.push((1, length)),
    };
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
        if carriage_return {
            carriage_return = false;
            if character == '\n' {
                end_line(lengths, length);
                text.push_str("\r\n");
                length = 0;
                spaces = 0;
                continue;
            }
            push(text, &mut spaces, &mut length, '\r');
        }
        match character {
            '\r' => carriage_return = true,
            ' ' => {
                spaces += 1;
                length += 1;
            }
            character => push(text, &mut spaces, &mut length, character),
        }
    }
    if carriage_return {
        push(text, &mut spaces, &mut length, '\r');
    }
    end_line(lengths, length);
    into
}

/// The byte offset in `trimmed`, `units` trimmed ([`unpadded_utf16`]), of
/// the point `offset` code units into `units`: the same line, and the same
/// column, no further than the line's end without its padding.
#[must_use]
pub fn trimmed_offset(units: &[u16], offset: usize, trimmed: &str) -> u32 {
    const BREAK: &str = "\r\n";
    let before = String::from_utf16_lossy(&units[..offset.min(units.len())]);
    let line = before.matches(BREAK).count();
    let column = before.rsplit(BREAK).next().map_or(0, str::len);
    let start: usize = trimmed
        .split(BREAK)
        .take(line)
        .map(|text| text.len() + BREAK.len())
        .sum();
    let length = trimmed.split(BREAK).nth(line).map_or(0, str::len);
    let within =
        verbatim_text::floor_boundary(trimmed.get(start..).unwrap_or(""), column.min(length));
    u32::try_from(start + within).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// A point keeps its line and column, and one in a line's padding
    /// comes back to the line's end without it.
    #[test]
    fn a_point_keeps_its_line_and_column_within_the_text() {
        let units = utf16("ab   \r\nc d  \r\n");
        let trimmed = unpadded_utf16(&units);
        assert_eq!(trimmed.text, "ab\r\nc d\r\n");
        assert_eq!(trimmed.lengths, vec![(2, 5), (1, 0)]);
        assert_eq!(trimmed_offset(&units, 0, &trimmed.text), 0);
        assert_eq!(trimmed_offset(&units, 1, &trimmed.text), 1);
        assert_eq!(trimmed_offset(&units, 4, &trimmed.text), 2);
        assert_eq!(trimmed_offset(&units, 7, &trimmed.text), 4);
        assert_eq!(trimmed_offset(&units, 9, &trimmed.text), 6);
        assert_eq!(trimmed_offset(&units, 11, &trimmed.text), 7);
        assert_eq!(trimmed_offset(&units, 14, &trimmed.text), 9);
    }
}
