//! Review-cursor text: the flat text of a navigator object and the pure
//! line, word, and character walks over it (roadmap M3).
//!
//! M3's review cursor reviews one object at a time — the navigator object —
//! with no text-pattern support yet (that arrives in M4). An object's review
//! text is therefore its flat presentation: its value when it has one (an
//! edit control's content), otherwise its name (a button's or list item's
//! label). Navigation is ordinary Unicode string work: lines split on `\n`,
//! words on whitespace runs, characters by `char`. Grapheme-cluster
//! characters and word-boundary segmentation are deliberately left for M4,
//! where the character-description table and text model land; M3 walks
//! `char`s and whitespace, which reads correctly for the plain labels and
//! single-line values it faces.

use verbatim_model::NodeSnapshot;

/// The review text of a node: its value if it has a non-empty one, else its
/// name, else the empty string. This is what the review cursor walks.
#[must_use]
pub(crate) fn text_of(node: &NodeSnapshot) -> String {
    if let Some(value) = node.value.as_ref().filter(|value| !value.is_empty()) {
        return value.clone();
    }
    node.name.clone().unwrap_or_default()
}

/// The `[start, end)` character-offset span of the line containing `offset`.
/// Lines are separated by `\n`; the separator itself belongs to no line. An
/// offset at or past the end clamps to the last line. Empty text is one
/// empty line, `[0, 0)`.
#[must_use]
pub(crate) fn line_span(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    (start, end)
}

/// The span of the word containing `offset`: a run of non-whitespace
/// characters. If `offset` sits on whitespace, the next word is returned;
/// past the last word, an empty span at the end. This is "current word" —
/// the word the cursor is within, or the next one it would reach.
#[must_use]
pub(crate) fn word_span(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let on_word = text[offset..]
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_whitespace());
    let start = if on_word {
        word_start(text, offset)
    } else {
        // On or past whitespace: advance to the next word's start.
        text[offset..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map_or(text.len(), |(index, _)| offset + index)
    };
    let end = text[start..]
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(text.len(), |(index, _)| start + index);
    (start, end)
}

/// The start of the non-whitespace run `offset` is inside: scan back while
/// the preceding character is non-whitespace.
fn word_start(text: &str, offset: usize) -> usize {
    let mut start = offset.min(text.len());
    for (index, ch) in text[..start].char_indices().rev() {
        if ch.is_whitespace() {
            break;
        }
        start = index;
    }
    start
}

/// The offset of the previous word start before `offset`, or `None` at the
/// first word.
#[must_use]
pub(crate) fn previous_word_start(text: &str, offset: usize) -> Option<usize> {
    let offset = offset.min(text.len());
    // Step back one char, then to that word's start; repeat until we land
    // strictly before the current word's start.
    let current = word_span(text, offset).0;
    let mut probe = current;
    while probe > 0 {
        let prev = text[..probe]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        let start = word_span(text, prev).0;
        if start < current {
            return Some(start);
        }
        probe = prev;
    }
    None
}

/// The offset of the next word start after the word containing `offset`, or
/// `None` if there is no further word.
#[must_use]
pub(crate) fn next_word_start(text: &str, offset: usize) -> Option<usize> {
    let (_, end) = word_span(text, offset);
    let next = text[end..]
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(index, _)| end + index)?;
    Some(next)
}

/// The character span `[offset, next_char)` at `offset`, or `None` at the
/// end of the text.
#[must_use]
pub(crate) fn char_span(text: &str, offset: usize) -> Option<(usize, usize)> {
    let offset = offset.min(text.len());
    let ch = text[offset..].chars().next()?;
    Some((offset, offset + ch.len_utf8()))
}

/// The offset one character before `offset`, or `None` at the start.
#[must_use]
pub(crate) fn previous_char(text: &str, offset: usize) -> Option<usize> {
    let offset = offset.min(text.len());
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{Backend, NodeDetails, NodeId, Role, StateSet};

    fn node(name: Option<&str>, value: Option<&str>) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId::new(1),
            backend: Backend::Uia,
            role: Role::EditableText,
            name: name.map(str::to_owned),
            value: value.map(str::to_owned),
            states: StateSet::new(),
            details: NodeDetails::default(),
        }
    }

    #[test]
    fn text_prefers_value_then_name() {
        assert_eq!(text_of(&node(Some("Label"), Some("content"))), "content");
        assert_eq!(text_of(&node(Some("Label"), Some(""))), "Label");
        assert_eq!(text_of(&node(Some("Label"), None)), "Label");
        assert_eq!(text_of(&node(None, None)), "");
    }

    #[test]
    fn lines_split_on_newline() {
        let text = "first\nsecond\nthird";
        assert_eq!(line_span(text, 0), (0, 5));
        assert_eq!(line_span(text, 6), (6, 12));
        assert_eq!(line_span(text, 13), (13, 18));
        // Past the end clamps to the last line.
        assert_eq!(line_span(text, 99), (13, 18));
    }

    #[test]
    fn words_are_non_whitespace_runs() {
        let text = "the quick  fox";
        assert_eq!(word_span(text, 0), (0, 3)); // "the"
        assert_eq!(word_span(text, 1), (0, 3)); // inside "the"
        assert_eq!(word_span(text, 3), (4, 9)); // on the space -> next word "quick"
        assert_eq!(word_span(text, 4), (4, 9)); // "quick"
        assert_eq!(&text[word_span(text, 11).0..word_span(text, 11).1], "fox");
    }

    #[test]
    fn word_motion_walks_forward_and_back() {
        let text = "the quick fox";
        assert_eq!(next_word_start(text, 0), Some(4));
        assert_eq!(next_word_start(text, 4), Some(10));
        assert_eq!(next_word_start(text, 10), None);
        assert_eq!(previous_word_start(text, 10), Some(4));
        assert_eq!(previous_word_start(text, 4), Some(0));
        assert_eq!(previous_word_start(text, 0), None);
    }

    #[test]
    fn character_motion_respects_utf8() {
        let text = "aé中";
        assert_eq!(char_span(text, 0), Some((0, 1))); // 'a'
        assert_eq!(char_span(text, 1), Some((1, 3))); // 'é' is two bytes
        assert_eq!(char_span(text, 3), Some((3, 6))); // '中' is three bytes
        assert_eq!(char_span(text, 6), None);
        assert_eq!(previous_char(text, 6), Some(3));
        assert_eq!(previous_char(text, 3), Some(1));
        assert_eq!(previous_char(text, 1), Some(0));
        assert_eq!(previous_char(text, 0), None);
    }
}
