//! Review-cursor text: the flat text of a navigator object and the pure
//! line, word, and character walks over it (roadmap M3).
//!
//! An object without a text pattern (a button, a list item, a label) is
//! reviewed as its flat presentation: its value when it has one, otherwise
//! its name. Lines split on any line break (`verbatim_text::lines`);
//! characters are grapheme clusters and words come from Unicode's word
//! rules with dictionaries, the same units the review cursor walks in text
//! with a text pattern (`text::words`, `text::grapheme_at`), so a Hindi
//! vowel sign stays with its letter, an emoji with its skin tone is one
//! character, and a Thai or Chinese name has words.

use std::ops::Range;

use verbatim_model::NodeSnapshot;

use crate::text;

/// The review text of a node: its value if it has a non-empty one, else its
/// name, else the empty string. This is what the review cursor walks.
#[must_use]
pub(crate) fn text_of(node: &NodeSnapshot) -> String {
    if let Some(value) = node.value.as_ref().filter(|value| !value.is_empty()) {
        return value.clone();
    }
    node.name.clone().unwrap_or_default()
}

/// The `[start, end)` byte span of the line containing `offset`, without
/// its line break. Any line break ends a line (`verbatim_text::lines`): a
/// carriage return and line feed together, either alone, or Unicode's line
/// and paragraph separators. An offset in a break belongs to the line the
/// break ends, and one at or past the end to the last line. Empty text is
/// one empty line, `[0, 0)`.
#[must_use]
pub(crate) fn line_span(text: &str, offset: usize) -> (usize, usize) {
    let line = verbatim_text::line_at(text, offset);
    (line.start, line.end)
}

/// The span of the line after the one containing `offset`, or `None` on the
/// last line.
#[must_use]
pub(crate) fn next_line_span(text: &str, offset: usize) -> Option<(usize, usize)> {
    verbatim_text::lines(text)
        .into_iter()
        .find(|line| line.start > offset)
        .map(|line| (line.start, line.end))
}

/// The words of flat `text`, each as its byte range, in order: the words
/// the review cursor walks in text with a text pattern (`text::words`),
/// with punctuation a word of its own and white space no word. Flat text
/// carries no language (a name or value has none), so the rules are chosen
/// from the text itself (`verbatim_text::WordRules::for_text`).
#[must_use]
pub(crate) fn words(text: &str) -> Vec<Range<usize>> {
    text::words(text, None)
}

/// The word of `words` after the one the cursor at `offset` is on
/// (`text::word_at`), or `None` past the last.
#[must_use]
pub(crate) fn next_word(words: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    let current = text::word_at(words, offset);
    words
        .iter()
        .find(|range| {
            current
                .as_ref()
                .is_none_or(|current| range.start > current.start)
        })
        .cloned()
}

/// The word of `words` before the one the cursor at `offset` is on, or
/// `None` at the first.
#[must_use]
pub(crate) fn previous_word(words: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    let current = text::word_at(words, offset)?;
    words
        .iter()
        .rev()
        .find(|range| range.start < current.start)
        .cloned()
}

/// The character span at `offset`: the grapheme cluster starting at or
/// containing it, or `None` at the end of the text.
#[must_use]
pub(crate) fn char_span(text: &str, offset: usize) -> Option<(usize, usize)> {
    text::grapheme_at(text, offset.min(text.len())).map(|range| (range.start, range.end))
}

/// The start of the character (grapheme cluster) before the one at
/// `offset`, or `None` at the start.
#[must_use]
pub(crate) fn previous_char(text: &str, offset: usize) -> Option<usize> {
    text::previous_grapheme(text, offset.min(text.len())).map(|range| range.start)
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
    fn a_carriage_return_alone_or_with_a_line_feed_ends_a_line() {
        // Windows 11 Notepad's text ends its lines with a bare carriage
        // return; gamma and delta are two lines, not one.
        let text = "gamma\rdelta\r\nepsilon";
        assert_eq!(line_span(text, 0), (0, 5));
        assert_eq!(next_line_span(text, 0), Some((6, 11)));
        assert_eq!(next_line_span(text, 6), Some((13, 20)));
        assert_eq!(next_line_span(text, 13), None);
        // The line feed of a carriage return and line feed is still the
        // line it ends, so the previous line is found from it.
        assert_eq!(line_span(text, 12), (6, 11));
    }

    #[test]
    fn words_follow_unicode_rules_and_skip_white_space() {
        let text = "the quick  fox, ok";
        let words = words(text);
        let spans: Vec<&str> = words.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(spans, ["the", "quick", "fox", ",", "ok"]);
        // From white space, the next word is the one after the word the
        // white space follows.
        assert_eq!(next_word(&words, 0), Some(4..9));
        assert_eq!(next_word(&words, 9), Some(11..14));
        assert_eq!(next_word(&words, 16), None);
        assert_eq!(previous_word(&words, 11), Some(4..9));
        assert_eq!(previous_word(&words, 1), None);
    }

    #[test]
    fn characters_are_grapheme_clusters() {
        let text = "aé中";
        assert_eq!(char_span(text, 0), Some((0, 1))); // 'a'
        assert_eq!(char_span(text, 1), Some((1, 3))); // 'é' is two bytes
        assert_eq!(char_span(text, 3), Some((3, 6))); // '中' is three bytes
        assert_eq!(char_span(text, 6), None);
        assert_eq!(previous_char(text, 6), Some(3));
        assert_eq!(previous_char(text, 3), Some(1));
        assert_eq!(previous_char(text, 1), Some(0));
        assert_eq!(previous_char(text, 0), None);
        // A letter and its combining accent are one character.
        let text = "e\u{301}x";
        assert_eq!(char_span(text, 0), Some((0, 3)));
        assert_eq!(previous_char(text, 3), Some(0));
    }
}
