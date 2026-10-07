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

use verbatim_model::{
    BulletStyle, FormatRun, LineStyle, Message, SegmentContent, TextAttributes, TextChunk,
    TextFormat, UtteranceSegment,
};
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

/// The line break that ends a line's `text`, when it has one.
pub(crate) fn line_break(text: &str) -> Option<&str> {
    let content = text.trim_end_matches(is_line_break);
    (content.len() < text.len()).then(|| &text[content.len()..])
}

/// The characters of a line's `text` as the caret and the review cursor
/// meet them, each a byte range, in order: the grapheme clusters of its
/// content, then each character of the line break that ends it on its own,
/// as NVDA's character unit has them (`docs/nvda/editable-text-and-terminals.md`,
/// "A line break as a character"), so a carriage return and line feed are
/// two characters. In a terminal (`grid`) the row's padding and line break
/// are not characters: past its text is a blank cell.
pub(crate) fn characters(text: &str, grid: bool) -> Vec<Range<usize>> {
    let content = line_content(text, grid);
    let mut characters = verbatim_text::graphemes(content);
    if !grid {
        let end = content.len();
        characters.extend(
            text[end..]
                .char_indices()
                .map(|(index, c)| end + index..end + index + c.len_utf8()),
        );
    }
    characters
}

/// The character ([`characters`]) of a line's `text` starting at or
/// containing byte `offset`: a line break when the offset is on one, and
/// `None` past the last (the end of the text, or a blank cell past a
/// terminal row's text).
pub(crate) fn character_at(text: &str, offset: usize, grid: bool) -> Option<&str> {
    characters(text, grid)
        .into_iter()
        .find(|range| range.start <= offset && offset < range.end)
        .map(|range| &text[range])
}

/// The character ([`characters`]) of a line's `text` before the one at
/// byte `offset`, or `None` at the line's start.
pub(crate) fn previous_character(text: &str, offset: usize, grid: bool) -> Option<Range<usize>> {
    characters(text, grid)
        .into_iter()
        .take_while(|range| range.start < offset)
        .last()
}

/// The character ([`characters`]) of a line's `text` after the one at byte
/// `offset`, or `None` at the line's last.
pub(crate) fn next_character(text: &str, offset: usize, grid: bool) -> Option<Range<usize>> {
    characters(text, grid)
        .into_iter()
        .find(|range| range.start > offset)
}

/// `offset` as a position on a line's `text` a cursor can rest at: a
/// character boundary, on the line's content or its line break, but never
/// past the line's last character ([`characters`]) when the line has a
/// break, and in a terminal never past the end of the row's text.
pub(crate) fn position_in(text: &str, offset: usize, grid: bool) -> usize {
    let content = line_content(text, grid);
    let offset = boundary(text, offset);
    if offset <= content.len() {
        offset
    } else if grid {
        content.len()
    } else {
        characters(text, grid)
            .last()
            .map_or(content.len(), |last| offset.min(last.start))
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

/// Whether `text` has nothing to read: empty, or only white space, the
/// zero-width space among it (`verbatim_text::is_space`).
pub(crate) fn is_blank(text: &str) -> bool {
    text.chars().all(verbatim_text::is_space)
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
/// "blank" for none. A line break is a character with a name, "carriage
/// return" or "line feed" (`docs/nvda/editable-text-and-terminals.md`, "A
/// line break as a character"); a carriage return and line feed met as one
/// grapheme cluster are named by the carriage return, the character the
/// cluster starts with.
pub(crate) fn character_segments(
    character: Option<&str>,
    language: Option<&str>,
) -> Vec<UtteranceSegment> {
    match character {
        Some(character) if !character.is_empty() => {
            let character = if character == "\r\n" { "\r" } else { character };
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

/// The formatting changes from `old` to `new` that are spoken, in NVDA's
/// order (`docs/nvda/document-formatting.md`, "The cache, attribute by
/// attribute"): font name, size, and color when present and different,
/// the color and the background together as "dark red on light grey"
/// when both change; bold, italic, strikethrough, and underline starting,
/// or ending after having been on, the underline by its kind when the kind
/// was read; a link starting or ending; a spelling or grammar error
/// starting, and, with `extra_detail` (a character or a word), ending. A
/// bullet is not a change: [`formatted_segments`] speaks it at the start
/// of every line that has one.
pub(crate) fn format_changes(
    old: &TextAttributes,
    new: &TextAttributes,
    extra_detail: bool,
) -> Vec<TextFormat> {
    let mut changes = Vec::new();
    let differs = |old: &Option<String>, new: &Option<String>| {
        new.as_ref()
            .filter(|value| Some(*value) != old.as_ref())
            .cloned()
    };
    if let Some(name) = differs(&old.font_name, &new.font_name) {
        changes.push(TextFormat::FontName(name));
    }
    if let Some(size) = differs(&old.font_size, &new.font_size) {
        changes.push(TextFormat::FontSize(size));
    }
    let background = differs(&old.background_color, &new.background_color);
    if let Some(color) = differs(&old.color, &new.color) {
        changes.push(TextFormat::Color(color));
        if let Some(background) = background {
            changes.push(TextFormat::OnBackgroundColor(background));
        }
    } else if let Some(background) = background {
        changes.push(TextFormat::BackgroundColor(background));
    }
    let switched = |old: Option<bool>, new: Option<bool>, on, off| match (old, new) {
        (Some(true), Some(false)) => Some(off),
        (None | Some(false), Some(true)) => Some(on),
        _ => None,
    };
    changes.extend(switched(
        old.bold,
        new.bold,
        TextFormat::Bold,
        TextFormat::NotBold,
    ));
    changes.extend(switched(
        old.italic,
        new.italic,
        TextFormat::Italic,
        TextFormat::NotItalic,
    ));
    changes
        .extend(line_change(old.strikethrough, new.strikethrough).map(TextFormat::Strikethrough));
    if new.underline_style.is_some() {
        changes.extend(
            line_change(old.underline_style, new.underline_style).map(TextFormat::UnderlineStyle),
        );
    } else {
        changes.extend(switched(
            old.underline,
            new.underline,
            TextFormat::Underline,
            TextFormat::NotUnderline,
        ));
    }
    if new.link != old.link {
        changes.push(if new.link {
            TextFormat::Link
        } else {
            TextFormat::NotLink
        });
    }
    for (old, new, on, off) in [
        (
            old.spelling_error,
            new.spelling_error,
            TextFormat::SpellingError,
            TextFormat::NotSpellingError,
        ),
        (
            old.grammar_error,
            new.grammar_error,
            TextFormat::GrammarError,
            TextFormat::NotGrammarError,
        ),
    ] {
        if new && !old {
            changes.push(on);
        } else if old && !new && extra_detail {
            changes.push(off);
        }
    }
    changes
}

/// The change of a line under or through text, as NVDA reports
/// strikethrough and underline: a line, or a different line, from where
/// there was none or another; and no line after having had one. A line
/// never read says nothing.
fn line_change(old: Option<LineStyle>, new: Option<LineStyle>) -> Option<LineStyle> {
    match new? {
        LineStyle::None => old
            .is_some_and(LineStyle::is_drawn)
            .then_some(LineStyle::None),
        drawn => (old != Some(drawn)).then_some(drawn),
    }
}

/// How a unit of text with formatting is spoken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spoken {
    /// As text, with each change of formatting where it happens.
    Text,
    /// As a word: text, or a single character by its name.
    Word,
    /// As one character, after the formatting at its start.
    Character,
}

/// The stretches of `formats` within the byte range `span`, cut to it, in
/// order.
fn runs_within(formats: &[FormatRun], span: &Range<usize>) -> Vec<(Range<usize>, TextAttributes)> {
    formats
        .iter()
        .filter_map(|run| {
            let start = (run.start as usize).max(span.start);
            let end = (run.end as usize).min(span.end);
            (start < end).then(|| (start..end, run.attributes.clone()))
        })
        .collect()
}

/// The speech for `chunk`'s text from `span` (its content: the text without
/// its line break), whose formatting the outpost read, as NVDA speaks a
/// unit with formatting (`docs/nvda/document-formatting.md`): the changes
/// from `reported` at the unit's start, then the text with each later
/// change placed where it happens. `spoken` is the part of `span` read
/// aloud (without surrounding white space for a word); white space outside
/// it still changes the formatting, which is spoken. `reported` becomes
/// the formatting at the end. `None` when the chunk carries no formatting,
/// and `reported` is left as it was.
pub(crate) fn formatted_segments(
    chunk: &TextChunk,
    (span, spoken): (Range<usize>, Range<usize>),
    how: Spoken,
    reported: &mut TextAttributes,
    language: Option<&str>,
) -> Option<Vec<UtteranceSegment>> {
    let mut runs = runs_within(&chunk.formats, &span);
    if runs.is_empty() {
        // A character, or an empty unit, has its formatting at its start.
        let at = chunk
            .formats
            .iter()
            .find(|run| (run.start as usize) <= span.start && span.start <= run.end as usize)?;
        runs.push((span.clone(), at.attributes.clone()));
    }
    let extra_detail = matches!(how, Spoken::Word | Spoken::Character);
    let format = |change| UtteranceSegment::new(SegmentContent::Format(change));
    let text = &chunk.text[spoken.clone()];
    let mut segments: Vec<UtteranceSegment> = format_changes(reported, &runs[0].1, extra_detail)
        .into_iter()
        .map(format)
        .collect();
    // A list item's bullet, which is not in the text, after the changes at
    // the start of a line read as text, as NVDA speaks its line prefix
    // (`docs/nvda/document-formatting.md`, "Line prefixes"); never for a
    // word or a character.
    if how == Spoken::Text
        && let Some(bullet) = runs[0]
            .1
            .bullet
            .filter(|bullet| *bullet != BulletStyle::None)
    {
        segments.push(format(TextFormat::Bullet(bullet)));
    }
    *reported = runs[0].1.clone();
    let single = how == Spoken::Character
        || (how == Spoken::Word && verbatim_text::graphemes(text.trim()).len() == 1);
    if single {
        let character = if how == Spoken::Character {
            text
        } else {
            text.trim()
        };
        segments.extend(character_segments(
            (!character.is_empty()).then_some(character),
            language,
        ));
        return Some(segments);
    }
    if is_blank(text) {
        for (_, attributes) in &runs[1..] {
            *reported = attributes.clone();
        }
        segments.extend(text_segments("", None));
        return Some(segments);
    }
    for (index, (range, attributes)) in runs.iter().enumerate() {
        if index > 0 {
            segments.extend(
                format_changes(reported, attributes, extra_detail)
                    .into_iter()
                    .map(format),
            );
            *reported = attributes.clone();
        }
        let start = range.start.max(spoken.start);
        let end = range.end.min(spoken.end);
        // Each stretch is its own piece of speech, without the white space
        // around it, which the pieces are spoken apart by anyway.
        let piece = chunk.text[start.min(end)..end].trim();
        if !piece.is_empty() {
            segments.push(in_language(
                SegmentContent::Text(piece.to_owned()),
                language,
            ));
        }
    }
    Some(segments)
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
    fn each_character_of_a_line_break_is_a_character_of_its_own() {
        // A standard edit control's break: two characters.
        assert_eq!(characters("ab\r\n", false), vec![0..1, 1..2, 2..3, 3..4]);
        assert_eq!(character_at("ab\r\n", 2, false), Some("\r"));
        assert_eq!(character_at("ab\r\n", 3, false), Some("\n"));
        // Windows 11 Notepad's: one.
        assert_eq!(character_at("ab\r", 2, false), Some("\r"));
        // The text's last line has no break, so its end has no character.
        assert_eq!(character_at("ab", 2, false), None);
        // A terminal row's break and padding are not characters.
        assert_eq!(characters("ab  \r\n", true), vec![0..1, 1..2]);
        assert_eq!(character_at("ab  \r\n", 2, true), None);
        assert_eq!(line_break("ab\r\n"), Some("\r\n"));
        assert_eq!(line_break("ab"), None);
    }

    #[test]
    fn a_position_rests_on_a_line_break_but_not_past_it() {
        assert_eq!(position_in("ab\r\n", 3, false), 3);
        assert_eq!(position_in("ab\r\n", 4, false), 3);
        assert_eq!(position_in("ab\r", 9, false), 2);
        assert_eq!(position_in("ab", 9, false), 2);
        assert_eq!(position_in("ab  \r\n", 5, true), 2);
        assert_eq!(previous_character("ab\r\n", 3, false), Some(2..3));
        assert_eq!(next_character("ab\r\n", 1, false), Some(2..3));
        assert_eq!(next_character("ab\r\n", 3, false), None);
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
        // An emoji sequence is one character, spoken by its name.
        assert_eq!(
            spelled("\u{1F469}\u{200D}\u{1F4BB}", false, None),
            vec![UtteranceSegment::new(SegmentContent::Character(
                "\u{1F469}\u{200D}\u{1F4BB}".into()
            ))]
        );
    }

    #[test]
    fn a_zero_width_space_separates_words_and_reads_as_blank() {
        // Khmer "ka" and "kha" with a zero-width space between them, as
        // Khmer marks its word breaks.
        assert_eq!(words("ក\u{200B}ខ", None), vec![0..3, 6..9]);
        assert!(is_blank("\u{200B}"));
        assert!(is_blank(" \u{FEFF}\t"));
        assert!(!is_blank("\u{200B}a"));
    }

    #[test]
    fn a_line_break_is_a_named_character_and_nothing_is_blank() {
        let character = |text: &str| {
            vec![UtteranceSegment::new(SegmentContent::Character(
                text.into(),
            ))]
        };
        assert_eq!(character_segments(Some("\r"), None), character("\r"));
        assert_eq!(character_segments(Some("\n"), None), character("\n"));
        // A carriage return and line feed met as one cluster.
        assert_eq!(character_segments(Some("\r\n"), None), character("\r"));
        assert_eq!(
            character_segments(None, None),
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
