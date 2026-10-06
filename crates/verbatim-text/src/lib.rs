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

/// `segments` with each run of adjacent whitespace-only segments merged
/// into one, so spaces and tabs together are a single segment.
fn merge_whitespace_runs(text: &str, segments: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let is_space = |range: &Range<usize>| text[range.clone()].chars().all(char::is_whitespace);
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
        let words = texts(text, &segmenter.words(text, rules));
        assert!(words.len() > 1, "{words:?}");
        assert!(words.contains(&"我们"), "{words:?}");
    }

    #[test]
    fn thai_words_are_found_without_spaces() {
        let segmenter = Segmenter::new();
        // "Hello" and "world" in Thai, written without a space.
        let text = "สวัสดีชาวโลก";
        let words = segmenter.words(text, WordRules::Unicode);
        assert!(words.len() > 1, "{:?}", texts(text, &words));
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
    fn padding_is_trimmed_whatever_its_whitespace() {
        assert_eq!(trim_padding("output line 13        "), "output line 13");
        assert_eq!(trim_padding("a b\u{3000}\u{3000}\t"), "a b");
        assert_eq!(trim_padding("   "), "");
    }
}
