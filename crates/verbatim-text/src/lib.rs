//! Text segmentation for Verbatim's text model (milestone M4;
//! `phase6-design.md`, "Internationalization in the text model").
//!
//! Platform-neutral and original: the rules are Unicode's (UAX 29 for
//! characters, words, and sentences; UAX 11 for terminal cell widths), as
//! ICU4X implements them, and the behaviors chosen on top follow the
//! prose in `docs/nvda/text-infos.md` rather than NVDA's source.
//!
//! - A character is a grapheme cluster ([`graphemes`]): an emoji sequence,
//!   a letter with combining accents, a Hangul syllable, or an Indic
//!   conjunct is one character, and a surrogate pair is never split.
//! - Words ([`Segmenter::words`]) follow Unicode's word rules with ICU's
//!   dictionaries for scripts that do not separate words with spaces
//!   (Chinese, Japanese, Thai, Lao, Khmer, Burmese); Chinese text uses the
//!   jieba dictionary segmenter, as NVDA uses cppjieba. A run of spaces and
//!   tabs is one segment, so a position anywhere in it belongs to the same
//!   word, as in NVDA.
//! - Sentences ([`Segmenter::sentences`]) follow Unicode's sentence rules.
//! - Speech without pauses ([`last_pause`]) splits text after its last
//!   sentence end, as say-all speaks it.
//! - Lines ([`lines`], [`line_at`]) end at any line break
//!   ([`is_line_break`]): a carriage return and line feed together, either
//!   alone, or Unicode's line and paragraph separators.
//! - [`cell_width`] is how many terminal cells text takes; [`trim_padding`]
//!   removes a terminal line's trailing padding, whatever whitespace it is.
//!
//! Offsets are byte offsets into the UTF-8 string given. Positions in a
//! provider's own units (UTF-16 code units, text ranges) are converted at
//! the outpost, never here.

#![forbid(unsafe_code)]

use std::ops::Range;
use std::sync::OnceLock;

use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use icu_segmenter::options::{SentenceBreakInvariantOptions, WordBreakInvariantOptions};
use icu_segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};
use jieba_rs::Jieba;

/// The grapheme clusters of `text`, each as its byte range, in order.
#[must_use]
pub fn graphemes(text: &str) -> Vec<Range<usize>> {
    ranges(GraphemeClusterSegmenter::new().segment_str(text))
}

/// The byte range of the grapheme cluster containing `offset`, or `None`
/// when `offset` is at or past the end.
#[must_use]
pub fn grapheme_at(text: &str, offset: usize) -> Option<Range<usize>> {
    graphemes(text)
        .into_iter()
        .find(|range| range.contains(&offset))
}

/// How many terminal cells `text` takes: two for each wide character (East
/// Asian Width wide or fullwidth, such as Chinese, Japanese, and Korean
/// characters), none for combining marks, one otherwise.
#[must_use]
pub fn cell_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

/// `line` without its trailing padding: every trailing character with
/// Unicode's `White_Space` property, whatever the language, so a terminal
/// line padded to the window's width reads as its text. Spaces inside the
/// line are kept.
#[must_use]
pub fn trim_padding(line: &str) -> &str {
    line.trim_end_matches(char::is_whitespace)
}

/// Whether the grapheme cluster `grapheme` belongs in a word: every code
/// point in it is a letter, a mark, or a number by its general category,
/// or the zero-width non-joiner or joiner, which shape the letters around
/// them. So a virama, a Thai tone mark, or a Persian zero-width non-joiner
/// typed on its own continues the word it is typed into.
#[must_use]
pub fn is_word_grapheme(grapheme: &str) -> bool {
    let categories = CodePointMapData::<GeneralCategory>::new();
    let word = GeneralCategoryGroup::Letter
        .union(GeneralCategoryGroup::Mark)
        .union(GeneralCategoryGroup::Number);
    !grapheme.is_empty()
        && grapheme
            .chars()
            .all(|c| matches!(c, '\u{200C}' | '\u{200D}') || word.contains(categories.get(c)))
}

/// Whether `c` is a bidirectional formatting character: the left-to-right
/// and right-to-left marks, the embeddings and overrides with the pop that
/// ends them, and the isolates with theirs. They order text on screen and
/// have nothing to say.
#[must_use]
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Removes every bidirectional formatting character ([`is_bidi_control`])
/// from `text`, as an application's names and values are read: File
/// Explorer's dates and the clock put left-to-right marks between their
/// numbers.
pub fn strip_bidi_controls(text: &mut String) {
    if text.contains(is_bidi_control) {
        text.retain(|c| !is_bidi_control(c));
    }
}

/// Whether `c` separates words as white space does: a character with
/// Unicode's `White_Space` property, or the zero-width space or the
/// zero-width no-break space (the byte order mark), which have no width but
/// mark a word break, as Khmer, Thai, and Burmese text uses them.
#[must_use]
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200B}' | '\u{FEFF}')
}

/// Whether `c` ends a line: a carriage return, a line feed, the vertical
/// tab, the form feed, the next-line control, or Unicode's line or
/// paragraph separator. A carriage return on its own is a line break, as
/// Windows 11 Notepad's text gives it; a carriage return followed by a line
/// feed is one break ([`lines`]).
#[must_use]
pub fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\r' | '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// The lines of `text`, each as the byte range of its content without the
/// break that ends it, in order. A carriage return followed by a line feed
/// is one break; any other [`is_line_break`] character is a break of its
/// own, so two in a row end an empty line. Text that ends with a break has
/// an empty last line after it, and empty text is one empty line.
#[must_use]
pub fn lines(text: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if !is_line_break(c) {
            continue;
        }
        lines.push(start..index);
        start = index + c.len_utf8();
        if c == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some() {
            start += 1;
        }
    }
    lines.push(start..text.len());
    lines
}

/// The content of the line containing byte `offset`, without its break.
/// An offset in a line's break belongs to the line the break ends, and an
/// offset past the end to the last line.
#[must_use]
pub fn line_at(text: &str, offset: usize) -> Range<usize> {
    lines(text)
        .into_iter()
        .take_while(|line| line.start <= offset)
        .last()
        .unwrap_or(0..0)
}

/// Which word segmentation to use for a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordRules {
    /// Unicode's word rules with ICU's dictionaries for scripts written
    /// without spaces.
    Unicode,
    /// The jieba dictionary segmenter, for Chinese.
    Chinese,
}

impl WordRules {
    /// The rules for text in `language` (a BCP 47 tag such as `zh-CN`, or
    /// `None` when unknown): Chinese for Chinese, and for unknown text that
    /// contains Chinese characters and no Japanese kana; Unicode otherwise.
    /// Japanese written with kanji has kana too, so it stays with Unicode's
    /// rules and ICU's Japanese dictionary.
    #[must_use]
    pub fn for_text(text: &str, language: Option<&str>) -> Self {
        let chinese = if let Some(tag) = language {
            let primary = tag.split(['-', '_']).next().unwrap_or("");
            primary.eq_ignore_ascii_case("zh")
        } else {
            text.chars().any(is_han) && !text.chars().any(is_kana)
        };
        if chinese {
            Self::Chinese
        } else {
            Self::Unicode
        }
    }
}

/// Word and sentence segmentation. Building it is cheap; the jieba
/// dictionary is loaded the first time Chinese text is segmented, and kept
/// for the life of the process.
#[derive(Debug, Default)]
pub struct Segmenter {
    _private: (),
}

impl Segmenter {
    /// A segmenter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The segments of `text` by `rules`, each as its byte range, in order,
    /// covering the whole text: words, the punctuation between them, and
    /// whitespace, with each run of whitespace merged into one segment.
    #[must_use]
    pub fn words(&self, text: &str, rules: WordRules) -> Vec<Range<usize>> {
        let segments = match rules {
            WordRules::Unicode => ranges(
                WordSegmenter::new_auto(WordBreakInvariantOptions::default()).segment_str(text),
            ),
            WordRules::Chinese => jieba()
                .cut(text, true)
                .into_iter()
                .map(|token| token.byte_start..token.byte_end)
                .collect(),
        };
        merge_whitespace_runs(text, segments)
    }

    /// The byte range of the word segment containing `offset` ([`words`](Self::words)),
    /// or `None` when `offset` is at or past the end.
    #[must_use]
    pub fn word_at(&self, text: &str, offset: usize, rules: WordRules) -> Option<Range<usize>> {
        self.words(text, rules)
            .into_iter()
            .find(|range| range.contains(&offset))
    }

    /// The sentences of `text`, each as its byte range, in order.
    #[must_use]
    pub fn sentences(&self, text: &str) -> Vec<Range<usize>> {
        ranges(SentenceSegmenter::new(SentenceBreakInvariantOptions::default()).segment_str(text))
    }
}

/// Where `text` splits for speech without pauses: the byte offset just
/// past its last sentence end and the whitespace after it, or `None` when
/// it has no sentence end (`docs/nvda/speech.md`, "Say-all speaks without
/// pauses"). Say-all speaks the text before the offset now and holds back
/// the rest to speak with what follows it.
///
/// A sentence end is a sentence-ending mark ([`is_sentence_end`]) that
/// directly follows a character that is neither whitespace nor such a
/// mark, with at most one closing quotation mark, guillemet, corner
/// bracket, or parenthesis ([`is_closing`]) after it, and then whitespace
/// or the end of the text. Chinese and Japanese leave no space after their
/// full-width marks, so after one of those ([`is_full_width_end`]) the
/// next sentence may follow at once. There is no list of abbreviations:
/// "Dr. Smith" splits after "Dr. ". A decimal point is followed by a
/// numeral, and the last full stop of an ellipsis follows another, so
/// neither is a sentence end.
#[must_use]
pub fn last_pause(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    (1..chars.len()).rev().find_map(|index| {
        let (_, mark) = chars[index];
        let (_, before) = chars[index - 1];
        if !is_sentence_end(mark) || before.is_whitespace() || is_sentence_end(before) {
            return None;
        }
        let mut next = index + 1;
        if chars.get(next).is_some_and(|&(_, c)| is_closing(c)) {
            next += 1;
        }
        if !is_full_width_end(mark) && chars.get(next).is_some_and(|&(_, c)| !c.is_whitespace()) {
            return None;
        }
        while chars.get(next).is_some_and(|&(_, c)| c.is_whitespace()) {
            next += 1;
        }
        Some(chars.get(next).map_or(text.len(), |&(offset, _)| offset))
    })
}

/// Whether `c` ends a sentence: a full stop, exclamation mark, or question
/// mark; their Chinese and Japanese full-width forms; the Devanagari danda
/// and double danda; the Arabic question mark and the Urdu full stop; the
/// Armenian full stop; and the Ethiopic full stop and question mark.
fn is_sentence_end(c: char) -> bool {
    matches!(
        c,
        '.' | '!'
            | '?'
            | '\u{3002}'
            | '\u{FF01}'
            | '\u{FF1F}'
            | '\u{0964}'
            | '\u{0965}'
            | '\u{061F}'
            | '\u{06D4}'
            | '\u{0589}'
            | '\u{1362}'
            | '\u{1367}'
    )
}

/// Whether `c` is a full-width sentence end of Chinese and Japanese (the
/// ideographic full stop and the full-width exclamation and question
/// marks), after which the next sentence follows without a space.
fn is_full_width_end(c: char) -> bool {
    matches!(c, '\u{3002}' | '\u{FF01}' | '\u{FF1F}')
}

/// Whether `c` may close a sentence after its end: a quotation mark or
/// apostrophe, a guillemet either way round (French closes with », German
/// with «), the low and high double quotation marks German closes with
/// („ and “), a corner bracket or white corner bracket, or a parenthesis,
/// full-width or not.
fn is_closing(c: char) -> bool {
    matches!(
        c,
        '"' | '\''
            | '\u{201D}'
            | '\u{2019}'
            | ')'
            | '\u{00BB}'
            | '\u{00AB}'
            | '\u{201E}'
            | '\u{201C}'
            | '\u{300D}'
            | '\u{300F}'
            | '\u{FF09}'
    )
}

/// The shared jieba segmenter, with its built-in dictionary.
fn jieba() -> &'static Jieba {
    static JIEBA: OnceLock<Jieba> = OnceLock::new();
    JIEBA.get_or_init(Jieba::new)
}

/// Consecutive boundaries as ranges: a segmenter's boundaries start at 0
/// and end at the text's length.
fn ranges(boundaries: impl Iterator<Item = usize>) -> Vec<Range<usize>> {
    let boundaries: Vec<usize> = boundaries.collect();
    boundaries
        .windows(2)
        .map(|pair| pair[0]..pair[1])
        .filter(|range| !range.is_empty())
        .collect()
}

/// `segments` with each run of adjacent white-space-only segments
/// ([`is_space`]) merged into one, so spaces and tabs together are a
/// single segment.
fn merge_whitespace_runs(text: &str, segments: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let is_space = |range: &Range<usize>| text[range.clone()].chars().all(is_space);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(segments.len());
    for segment in segments {
        match merged.last_mut() {
            Some(last) if last.end == segment.start && is_space(last) && is_space(&segment) => {
                last.end = segment.end;
            }
            _ => merged.push(segment),
        }
    }
    merged
}

/// Whether `c` is a Han character (CJK Unified Ideographs and their
/// extensions in the Basic Multilingual Plane).
fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// Whether `c` is Japanese kana (Hiragana or Katakana).
fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }

    /// `text` split where speech without pauses splits it.
    fn split(text: &str) -> Option<(&str, &str)> {
        last_pause(text).map(|offset| text.split_at(offset))
    }

    #[test]
    fn a_line_splits_after_its_last_sentence_end() {
        assert_eq!(
            split("written in Rust. It is informed by NVDA"),
            Some(("written in Rust. ", "It is informed by NVDA"))
        );
        // Only the last sentence end splits; the whitespace after it,
        // however long, goes before the split.
        assert_eq!(split("One. Two!  Three"), Some(("One. Two!  ", "Three")));
        assert_eq!(split("Is it?\tYes"), Some(("Is it?\t", "Yes")));
        // A sentence end at the end of the text leaves nothing after it.
        assert_eq!(split("It ends here."), Some(("It ends here.", "")));
        assert_eq!(split("It ends here.  "), Some(("It ends here.  ", "")));
        assert_eq!(split("no sentence end"), None);
        assert_eq!(split(""), None);
    }

    #[test]
    fn abbreviations_split_and_decimals_do_not() {
        // No list of abbreviations: each is a sentence end.
        assert_eq!(split("Ask Dr. Smith"), Some(("Ask Dr. ", "Smith")));
        assert_eq!(split("say e.g. this"), Some(("say e.g. ", "this")));
        // A decimal point is followed by a numeral.
        assert_eq!(split("pi is 3.14 or so"), None);
        assert_eq!(split("It is 3.5. Then"), Some(("It is 3.5. ", "Then")));
    }

    #[test]
    fn a_mark_needs_a_word_before_it() {
        // An ellipsis's last full stop follows another.
        assert_eq!(split("wait... and see"), None);
        assert_eq!(split("Really?! Yes"), None);
        // A mark after whitespace, or opening the text, is no sentence end.
        assert_eq!(split("a . b"), None);
        assert_eq!(split(". b"), None);
    }

    #[test]
    fn one_closing_character_may_follow_the_mark() {
        assert_eq!(
            split("He said \"stop.\" Then"),
            Some(("He said \"stop.\" ", "Then"))
        );
        assert_eq!(split("(see above.) Then"), Some(("(see above.) ", "Then")));
        assert_eq!(
            split("It\u{2019}s \u{2018}done.\u{2019} Then"),
            Some(("It\u{2019}s \u{2018}done.\u{2019} ", "Then"))
        );
        // Two closing characters are not a sentence end.
        assert_eq!(split("(he said \"stop.\") Then"), None);
        // Guillemets either way round, and German quotation marks.
        assert_eq!(split("«Oui.» Puis"), Some(("«Oui.» ", "Puis")));
        assert_eq!(split("»Ja.« Dann"), Some(("»Ja.« ", "Dann")));
        assert_eq!(split("„Ja.“ Dann"), Some(("„Ja.“ ", "Dann")));
        assert_eq!(split("Ja.„ Dann"), Some(("Ja.„ ", "Dann")));
    }

    #[test]
    fn chinese_and_japanese_marks_need_no_space_after_them() {
        assert_eq!(
            split("\u{4F60}\u{597D}\u{3002}\u{518D}"),
            Some(("\u{4F60}\u{597D}\u{3002}", "\u{518D}"))
        );
        assert_eq!(split("好吗？好的！谢谢"), Some(("好吗？好的！", "谢谢")));
        // A corner bracket or a full-width parenthesis may close it.
        assert_eq!(split("「はい。」次"), Some(("「はい。」", "次")));
        assert_eq!(split("『終。』次"), Some(("『終。』", "次")));
        assert_eq!(split("（注意。）再"), Some(("（注意。）", "再")));
        // A space after one goes before the split.
        assert_eq!(split("好。 再"), Some(("好。 ", "再")));
        // An ideographic full stop opening the text is no sentence end.
        assert_eq!(split("\u{3002}再"), None);
    }

    #[test]
    fn other_scripts_end_sentences_with_their_own_marks() {
        // Devanagari danda and double danda.
        assert_eq!(split("यह ठीक है। अब"), Some(("यह ठीक है। ", "अब")));
        assert_eq!(split("श्लोक॥ अब"), Some(("श्लोक॥ ", "अब")));
        // Arabic question mark and Urdu full stop.
        assert_eq!(split("هل أنت؟ نعم"), Some(("هل أنت؟ ", "نعم")));
        assert_eq!(split("یہ ہے۔ اب"), Some(("یہ ہے۔ ", "اب")));
        // Armenian full stop.
        assert_eq!(split("Բարեւ։ Ինչ"), Some(("Բարեւ։ ", "Ինչ")));
        // Ethiopic full stop and question mark.
        assert_eq!(split("ሰላም። እንዴት"), Some(("ሰላም። ", "እንዴት")));
        assert_eq!(split("ደህና፧ አዎ"), Some(("ደህና፧ ", "አዎ")));
        // These still need whitespace or the end after them.
        assert_eq!(split("है।अब"), None);
    }

    #[test]
    fn an_emoji_sequence_is_one_character() {
        // Woman technologist with a skin tone: four code points joined.
        let text = "a\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}b";
        let clusters = graphemes(text);
        assert_eq!(texts(text, &clusters).len(), 3);
        assert_eq!(
            texts(text, &clusters)[1],
            "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}"
        );
    }

    #[test]
    fn combining_accents_join_their_letter() {
        let text = "e\u{301}x";
        assert_eq!(texts(text, &graphemes(text)), ["e\u{301}", "x"]);
    }

    #[test]
    fn a_hangul_syllable_of_jamo_is_one_character() {
        let text = "\u{1100}\u{1161}\u{11A8}";
        assert_eq!(graphemes(text).len(), 1);
    }

    #[test]
    fn a_devanagari_conjunct_is_one_character() {
        // "ksha": ka, virama, ssa.
        let text = "\u{915}\u{94D}\u{937}";
        assert_eq!(graphemes(text).len(), 1);
    }

    #[test]
    fn grapheme_at_finds_the_cluster_around_an_offset() {
        let text = "ae\u{301}b";
        assert_eq!(grapheme_at(text, 2), Some(1..4));
        assert_eq!(grapheme_at(text, text.len()), None);
    }

    #[test]
    fn every_kind_of_line_break_ends_a_line() {
        for text in [
            "alpha\r\nbeta",
            "alpha\rbeta",
            "alpha\nbeta",
            "alpha\u{2028}beta",
            "alpha\u{2029}beta",
        ] {
            assert_eq!(texts(text, &lines(text)), ["alpha", "beta"], "{text:?}");
        }
        // A carriage return and a line feed the other way round are two
        // breaks, with an empty line between them.
        let text = "a\n\rb";
        assert_eq!(texts(text, &lines(text)), ["a", "", "b"]);
        // Text ending in a break has an empty last line; empty text is one
        // empty line.
        let text = "gamma\rdelta\r";
        assert_eq!(texts(text, &lines(text)), ["gamma", "delta", ""]);
        assert_eq!(texts("", &lines("")), [""]);
    }

    #[test]
    fn an_offset_in_a_break_belongs_to_the_line_it_ends() {
        let text = "gamma\r\ndelta\repsilon";
        assert_eq!(line_at(text, 0), 0..5);
        assert_eq!(line_at(text, 5), 0..5);
        assert_eq!(line_at(text, 6), 0..5);
        assert_eq!(line_at(text, 7), 7..12);
        assert_eq!(line_at(text, 12), 7..12);
        assert_eq!(line_at(text, 13), 13..20);
        assert_eq!(line_at(text, 99), 13..20);
    }

    #[test]
    fn english_words_and_punctuation() {
        let segmenter = Segmenter::new();
        let text = "Hello, world.";
        assert_eq!(
            texts(text, &segmenter.words(text, WordRules::Unicode)),
            ["Hello", ",", " ", "world", "."]
        );
    }

    #[test]
    fn a_run_of_spaces_and_tabs_is_one_segment() {
        let segmenter = Segmenter::new();
        let text = "a \t  b";
        let words = segmenter.words(text, WordRules::Unicode);
        assert_eq!(texts(text, &words), ["a", " \t  ", "b"]);
        // Every offset in the run gives the same segment.
        for offset in 1..5 {
            assert_eq!(
                segmenter.word_at(text, offset, WordRules::Unicode),
                Some(1..5)
            );
        }
    }

    #[test]
    fn chinese_words_come_from_the_dictionary() {
        let segmenter = Segmenter::new();
        let text = "我们中出了一个叛徒";
        let rules = WordRules::for_text(text, None);
        assert_eq!(rules, WordRules::Chinese);
        // The dictionary's words, with the two characters it has no word
        // for joined by jieba's hidden Markov model, which the segmenter
        // turns on.
        assert_eq!(
            texts(text, &segmenter.words(text, rules)),
            ["我们", "中出", "了", "一个", "叛徒"]
        );
    }

    #[test]
    fn thai_words_are_found_without_spaces() {
        let segmenter = Segmenter::new();
        // "Hello" and "world" in Thai, written without a space: "hello",
        // then "world" as "people" and "world".
        let text = "สวัสดีชาวโลก";
        assert_eq!(
            texts(text, &segmenter.words(text, WordRules::Unicode)),
            ["สวัสดี", "ชาว", "โลก"]
        );
    }

    #[test]
    fn japanese_with_kana_keeps_unicode_rules() {
        assert_eq!(
            WordRules::for_text("日本語を話す", None),
            WordRules::Unicode
        );
        assert_eq!(
            WordRules::for_text("anything", Some("zh-TW")),
            WordRules::Chinese
        );
        assert_eq!(WordRules::for_text("中文", Some("ja")), WordRules::Unicode);
    }

    #[test]
    fn sentences_split_after_their_ends() {
        let segmenter = Segmenter::new();
        let text = "One. Two? Three!";
        assert_eq!(
            texts(text, &segmenter.sentences(text)),
            ["One. ", "Two? ", "Three!"]
        );
    }

    #[test]
    fn wide_characters_take_two_cells() {
        assert_eq!(cell_width("ab"), 2);
        assert_eq!(cell_width("中文"), 4);
        assert_eq!(cell_width("e\u{301}"), 1);
    }

    #[test]
    fn a_zero_width_space_is_white_space() {
        let segmenter = Segmenter::new();
        let text = "ក\u{200B}ខ \u{FEFF}គ";
        assert_eq!(
            texts(text, &segmenter.words(text, WordRules::Unicode)),
            ["ក", "\u{200B}", "ខ", " \u{FEFF}", "គ"]
        );
    }

    #[test]
    fn letters_marks_numbers_and_joiners_are_word_characters() {
        for grapheme in [
            "a", "É", "7", "\u{94D}", "\u{E48}", "\u{200C}", "\u{200D}", "ก่",
        ] {
            assert!(is_word_grapheme(grapheme), "{grapheme:?}");
        }
        for grapheme in [" ", ",", "\t", "-", "😀", ""] {
            assert!(!is_word_grapheme(grapheme), "{grapheme:?}");
        }
    }

    #[test]
    fn bidirectional_formatting_characters_are_stripped() {
        let mut date = "\u{200E}08/\u{200E}10/\u{200E}2026".to_owned();
        strip_bidi_controls(&mut date);
        assert_eq!(date, "08/10/2026");
        let mut text = "\u{202B}a\u{202C}\u{2067}b\u{2069}\u{200F}".to_owned();
        strip_bidi_controls(&mut text);
        assert_eq!(text, "ab");
    }

    #[test]
    fn padding_is_trimmed_whatever_its_whitespace() {
        assert_eq!(trim_padding("output line 13        "), "output line 13");
        assert_eq!(trim_padding("a b\u{3000}\u{3000}\t"), "a b");
        assert_eq!(trim_padding("   "), "");
    }
}
