//! Pure helpers over text Core received (milestone M4): a line's content
//! without its line break, characters as grapheme clusters, columns in
//! characters or terminal cells, words by the text's language, and the
//! spoken segments for characters, words, lines, and spelling.
//!
//! Offsets are byte offsets into the UTF-8 text an outpost sent; Core never
//! does arithmetic on provider positions (`phase6-design.md`,
//! "Internationalization in the text model"). Segmentation is
//! `verbatim-text`'s.

use std::ops::Range;

use verbatim_model::{Message, SegmentContent, TextChunk, UtteranceSegment};
use verbatim_text::{Segmenter, WordRules, is_line_break};

/// The text of a line without the line break that ends it. In a terminal
/// (`grid`) the trailing padding goes too, so the line reads as its text.
pub(crate) fn line_content(text: &str, grid: bool) -> &str {
    let content = text.trim_end_matches(is_line_break);
    if grid {
        verbatim_text::trim_padding(content)
    } else {
        content
    }
}

/// The width of a terminal row in cells: its text without the line break,
/// padding included, so the review cursor can move across the blank cells
/// at its end.
pub(crate) fn row_width(text: &str) -> usize {
    verbatim_text::cell_width(text.trim_end_matches(is_line_break))
}

/// `offset` as a character boundary of `text`: at most its length, and
/// moved back to the start of a character it falls inside. An outpost
/// promises boundaries; this keeps a broken promise from panicking Core.
pub(crate) fn boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Whether `text` has nothing to read: empty, or only whitespace.
pub(crate) fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// The grapheme cluster of `content` starting at or containing `offset`,
/// or `None` at or past the end.
pub(crate) fn grapheme_at(content: &str, offset: usize) -> Option<Range<usize>> {
    verbatim_text::grapheme_at(content, offset)
}

/// The start of the grapheme cluster before the one at `offset`, or `None`
/// at the start.
pub(crate) fn previous_grapheme(content: &str, offset: usize) -> Option<Range<usize>> {
    verbatim_text::graphemes(content)
        .into_iter()
        .take_while(|range| range.start < offset)
        .last()
}

/// The grapheme cluster after the one at `offset`, or `None` at the last.
pub(crate) fn next_grapheme(content: &str, offset: usize) -> Option<Range<usize>> {
    verbatim_text::graphemes(content)
        .into_iter()
        .find(|range| range.start > offset)
}

/// The column of `offset` in `content`: terminal cells before it in a
/// terminal (`grid`), grapheme clusters before it elsewhere.
pub(crate) fn column_of(content: &str, offset: usize, grid: bool) -> usize {
    let before = &content[..boundary(content, offset)];
    if grid {
        verbatim_text::cell_width(before)
    } else {
        verbatim_text::graphemes(before).len()
    }
}

/// Where `column` falls on a line with `content`: the byte offset of the
/// character there. In a terminal (`grid`) a column inside a wide
/// character lands on that character, and a column past the text lands at
/// the text's end, a blank cell. Elsewhere a column past the last character
/// lands on the last character, so a shorter line puts the cursor at its
/// end while the column is remembered.
pub(crate) fn offset_at_column(content: &str, column: usize, grid: bool) -> usize {
    let clusters = verbatim_text::graphemes(content);
    if grid {
        let mut cells = 0;
        for range in &clusters {
            let width = verbatim_text::cell_width(&content[range.clone()]).max(1);
            if column < cells + width {
                return range.start;
            }
            cells += width;
        }
        content.len()
    } else {
        clusters
            .get(column)
            .or(clusters.last())
            .map_or(0, |range| range.start)
    }
}

/// Words: the segments of `content` that are not whitespace, by the rules
/// for its language.
pub(crate) fn words(content: &str, language: Option<&str>) -> Vec<Range<usize>> {
    let rules = WordRules::for_text(content, language);
    Segmenter::new()
        .words(content, rules)
        .into_iter()
        .filter(|range| !is_blank(&content[range.clone()]))
        .collect()
}

/// The word the cursor at `offset` is on: the word containing it, or, on
/// whitespace, the word the whitespace follows (a word includes its
/// trailing whitespace, as NVDA's do), or the next one at the line's
/// start; `None` on a line with no words.
pub(crate) fn word_at(words: &[Range<usize>], offset: usize) -> Option<Range<usize>> {
    words
        .iter()
        .rev()
        .find(|range| range.start <= offset)
        .or_else(|| words.first())
        .cloned()
}

/// The text of a chunk's unit as it is spoken: without its line break, and
/// in a terminal without padding.
pub(crate) fn chunk_content(chunk: &TextChunk, grid: bool) -> &str {
    line_content(&chunk.text, grid)
}

/// A segment in `language`, when there is one.
fn in_language(content: SegmentContent, language: Option<&str>) -> UtteranceSegment {
    UtteranceSegment {
        content,
        language: language.map(str::to_owned),
    }
}

/// The segments for a unit of text read aloud: its text, or "blank" when
/// it has nothing to read.
pub(crate) fn text_segments(text: &str, language: Option<&str>) -> Vec<UtteranceSegment> {
    if is_blank(text) {
        vec![UtteranceSegment::new(SegmentContent::Message(
            Message::Blank,
        ))]
    } else {
        vec![in_language(SegmentContent::Text(text.to_owned()), language)]
    }
}

/// The segments for a word read on its own: its text, or, for a word that
/// is a single character (a full stop a provider counts as a word of its
/// own), that character by its name, as NVDA speaks it
/// (`docs/nvda/speech.md`, "A word of one character"): spoken as text, a
/// punctuation mark would say nothing.
pub(crate) fn word_segments(word: &str, language: Option<&str>) -> Vec<UtteranceSegment> {
    let word = word.trim();
    if verbatim_text::graphemes(word).len() == 1 {
        character_segments(Some(word), language)
    } else {
        text_segments(word, language)
    }
}

/// The segments for one character spoken on its own: by its name, raised
/// in pitch when a capital (the presentation stage decides both), or
/// "blank" for none or a line break.
pub(crate) fn character_segments(
    character: Option<&str>,
    language: Option<&str>,
) -> Vec<UtteranceSegment> {
    match character {
        Some(character) if !character.is_empty() && !character.chars().all(is_line_break) => {
            vec![in_language(
                SegmentContent::Character(character.to_owned()),
                language,
            )]
        }
        _ => vec![UtteranceSegment::new(SegmentContent::Message(
            Message::Blank,
        ))],
    }
}

/// `text` spelled one character (grapheme cluster) at a time: a space as
/// "space", a capital raised in pitch, a letter or digit as itself, any
/// other character by its name; with `descriptions`, each character by its
/// description where it has one ("Alpha"), as NVDA spells on a third press.
pub(crate) fn spelled(
    text: &str,
    descriptions: bool,
    language: Option<&str>,
) -> Vec<UtteranceSegment> {
    verbatim_text::graphemes(text)
        .into_iter()
        .map(|range| {
            let character = &text[range];
            let content = if descriptions {
                SegmentContent::CharacterDescription(character.to_owned())
            } else if character == " " {
                SegmentContent::Message(Message::Space)
            } else if is_single_uppercase(character) {
                SegmentContent::SpelledCapital(character.to_owned())
            } else if character.chars().all(char::is_alphanumeric) {
                SegmentContent::Text(character.to_owned())
            } else {
                SegmentContent::Character(character.to_owned())
            };
            in_language(content, language)
        })
        .collect()
}

/// Whether `text` is one uppercase letter.
fn is_single_uppercase(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_uppercase() && chars.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_loses_its_break_and_a_terminal_row_its_padding() {
        assert_eq!(line_content("abc\r\n", false), "abc");
        assert_eq!(line_content("abc\r", false), "abc");
        assert_eq!(line_content("abc\u{2029}", false), "abc");
        assert_eq!(line_content("abc  \n", false), "abc  ");
        assert_eq!(line_content("abc  \n", true), "abc");
        assert_eq!(row_width("abc  \r\n"), 5);
    }

    #[test]
    fn an_offset_inside_a_character_moves_back_to_its_start() {
        assert_eq!(boundary("a中b", 2), 1);
        assert_eq!(boundary("a中b", 4), 4);
        assert_eq!(boundary("ab", 9), 2);
    }

    #[test]
    fn columns_count_characters_or_cells() {
        let text = "中文ab";
        // Characters: 中 is column 0, 文 1, a 2.
        assert_eq!(column_of(text, 6, false), 2);
        // Cells: 中 and 文 take two each, so a is at cell 4.
        assert_eq!(column_of(text, 6, true), 4);
        // Cell 1 is the second half of 中, which is where the cursor lands.
        assert_eq!(offset_at_column(text, 1, true), 0);
        assert_eq!(offset_at_column(text, 4, true), 6);
        // Past the end of a terminal row's text is a blank cell.
        assert_eq!(offset_at_column(text, 10, true), text.len());
        // Elsewhere, a column past the end lands on the last character.
        assert_eq!(offset_at_column(text, 10, false), 7);
        assert_eq!(offset_at_column("", 3, false), 0);
    }

    #[test]
    fn the_word_at_whitespace_is_the_word_before_it() {
        let text = "the quick  fox";
        let found = words(text, None);
        assert_eq!(word_at(&found, 0), Some(0..3));
        assert_eq!(word_at(&found, 3), Some(0..3));
        assert_eq!(word_at(&found, 10), Some(4..9));
        assert_eq!(word_at(&found, 11), Some(11..14));
        assert_eq!(word_at(&words(" lead", None), 0), Some(1..5));
        assert_eq!(word_at(&words("   ", None), 1), None);
    }

    #[test]
    fn spelling_uses_names_capitals_and_descriptions() {
        assert_eq!(
            spelled("a B,", false, None),
            vec![
                UtteranceSegment::text("a"),
                UtteranceSegment::new(SegmentContent::Message(Message::Space)),
                UtteranceSegment::new(SegmentContent::SpelledCapital("B".into())),
                UtteranceSegment::new(SegmentContent::Character(",".into())),
            ]
        );
        assert_eq!(
            spelled("ab", true, None),
            vec![
                UtteranceSegment::new(SegmentContent::CharacterDescription("a".into())),
                UtteranceSegment::new(SegmentContent::CharacterDescription("b".into())),
            ]
        );
        // An emoji sequence is one character.
        assert_eq!(spelled("\u{1F469}\u{200D}\u{1F4BB}", false, None).len(), 1);
    }

    #[test]
    fn a_line_break_or_nothing_is_a_blank_character() {
        assert_eq!(
            character_segments(Some("\r\n"), None),
            vec![UtteranceSegment::new(SegmentContent::Message(
                Message::Blank
            ))]
        );
        assert_eq!(
            character_segments(Some(","), Some("en")),
            vec![UtteranceSegment {
                content: SegmentContent::Character(",".into()),
                language: Some("en".into()),
            }]
        );
    }
}
