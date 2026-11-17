//! Characters and words as Uniscribe breaks them, ported from NVDA's
//! `_calculateUniscribeOffsets` (`nvdaHelper/local/textUtils.cpp`; this
//! crate is GPL like NVDA), which NVDA's `EditTextInfo` uses for every
//! Win32 edit control's characters and a plain edit control's words, as
//! Notepad's own segmentation does.
//!
//! A line of text is itemized into script runs (`ScriptItemize`) and broken
//! (`ScriptBreak`) into logical attributes per UTF-16 code unit: a
//! character starts where `fCharStop` is set, a word where `fWordStop` is.
//! As in NVDA, each run is broken from its start to the end of the text,
//! last run first, so the first run's analysis is the one left for the
//! whole text: breaking each run alone segments differently from Notepad.
//!
//! A word takes the white space after it and never the white space before
//! it, which `fWordStop` alone does not give where punctuation follows white
//! space (NVDA's #1656 corrections); white space is what the C runtime's
//! `iswspace` says it is, as for NVDA.

use windows::Win32::Globalization::{SCRIPT_ITEM, SCRIPT_LOGATTR, ScriptBreak, ScriptItemize};
use windows::core::PCWSTR;

/// `SCRIPT_LOGATTR`'s `fCharStop` bit.
const CHAR_STOP: u8 = 1 << 2;

/// `SCRIPT_LOGATTR`'s `fWordStop` bit.
const WORD_STOP: u8 = 1 << 3;

unsafe extern "C" {
    /// The C runtime's white space test, which NVDA's helper uses.
    fn iswspace(c: u16) -> i32;
}

/// Whether `unit` is white space, as the C runtime says.
fn is_space(unit: u16) -> bool {
    // SAFETY: `iswspace` reads only its argument.
    unsafe { iswspace(unit) != 0 }
}

/// The logical attributes of each code unit of `text`, which is not empty.
fn log_attrs(text: &[u16]) -> windows::core::Result<Vec<u8>> {
    let length = text.len();
    // ScriptItemize needs room for at least two items and writes a
    // terminating item after the last: room for every code unit as an
    // item, the terminator, and one more, as NVDA allocates.
    let mut items = vec![SCRIPT_ITEM::default(); length + 2];
    let mut count = 0i32;
    // SAFETY: `items` outlives the call and holds the `length + 1` items
    // the call is told of, of which it writes at most `length + 1`.
    unsafe { ScriptItemize(text, None, None, &mut items[..=length], &raw mut count)? };
    let count = usize::try_from(count).unwrap_or(0);
    if count == 0 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        ));
    }
    let mut attrs = vec![SCRIPT_LOGATTR::default(); length];
    for item in items[..count].iter().rev() {
        let start = usize::try_from(item.iCharPos).unwrap_or(0).min(length);
        let run = i32::try_from(length - start).unwrap_or(i32::MAX);
        // SAFETY: the text from `start` holds `run` code units, and
        // `attrs` from `start` has room for an attribute for each.
        unsafe {
            ScriptBreak(
                PCWSTR(text[start..].as_ptr()),
                run,
                &raw const item.a,
                attrs[start..].as_mut_ptr(),
            )?;
        }
    }
    Ok(attrs.into_iter().map(|attr| attr._bitfield).collect())
}

/// The span of the character containing UTF-16 offset `at` of `text`, as
/// Uniscribe's character stops give it: from the last stop at or before
/// `at` to the next stop after it, or the text's end. At or past the end,
/// the one code unit at `at`, as NVDA's helper answers.
///
/// # Errors
///
/// When Uniscribe fails to itemize or break the text.
pub fn character(text: &[u16], at: usize) -> windows::core::Result<(usize, usize)> {
    if at >= text.len() {
        return Ok((at, at + 1));
    }
    let attrs = log_attrs(text)?;
    let start = (0..=at)
        .rev()
        .find(|&i| attrs[i] & CHAR_STOP != 0)
        .unwrap_or(0);
    let end = (at + 1..text.len())
        .find(|&i| attrs[i] & CHAR_STOP != 0)
        .unwrap_or(text.len());
    Ok((start, end))
}

/// The span of the word containing UTF-16 offset `at` of `text`, as NVDA's
/// helper finds it: from the last word stop at or before `at` to the next
/// one after it, or the text's end, then corrected for white space. The
/// start moves after the nearest white space before `at` that follows
/// other text back from `at`, since white space before a word is not its
/// own (white space at `at` and just before it belongs to the word before);
/// the end moves to the end of the first white space from `at`, since a
/// word ends with the white space after it. At or past the end, the one
/// code unit at `at`, as NVDA's helper answers.
///
/// # Errors
///
/// When Uniscribe fails to itemize or break the text.
pub fn word(text: &[u16], at: usize) -> windows::core::Result<(usize, usize)> {
    if at >= text.len() {
        return Ok((at, at + 1));
    }
    let attrs = log_attrs(text)?;
    let mut start = (0..=at)
        .rev()
        .find(|&i| attrs[i] & WORD_STOP != 0)
        .unwrap_or(0);
    // White space at `at` and back from it belongs to the word before:
    // the word starts after the first white space found once other text
    // has been passed.
    let mut skipping = true;
    for i in (start..=at).rev() {
        if is_space(text[i]) {
            if skipping {
                continue;
            }
            start = i + 1;
            break;
        }
        skipping = false;
    }
    let mut end = (at + 1..text.len())
        .find(|&i| attrs[i] & WORD_STOP != 0)
        .unwrap_or(text.len());
    // The word ends after the first run of white space from `at`.
    if let Some(space) = (at..end).find(|&i| is_space(text[i])) {
        end = (space..end).find(|&i| !is_space(text[i])).unwrap_or(end);
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts the word and character spans at every offset of `text`, up
    /// to one past its end, as NVDA's own helper gives them for the same
    /// text: each table here was taken from `calculateWordOffsets` and
    /// `calculateCharacterOffsets` in the `nvdaHelperLocal.dll` of the
    /// pinned NVDA build (732d1ce2), called on this machine's Uniscribe.
    fn assert_spans(text: &str, words: &[(usize, usize)], characters: &[(usize, usize)]) {
        let text: Vec<u16> = text.encode_utf16().collect();
        let found: Vec<_> = (0..=text.len())
            .map(|at| word(&text, at).expect("Uniscribe breaks the text"))
            .collect();
        assert_eq!(found, words, "words");
        let found: Vec<_> = (0..=text.len())
            .map(|at| character(&text, at).expect("Uniscribe breaks the text"))
            .collect();
        assert_eq!(found, characters, "characters");
    }

    /// One character span per code unit, and one past the end.
    fn singles(length: usize) -> Vec<(usize, usize)> {
        (0..=length).map(|at| (at, at + 1)).collect()
    }

    /// `span` at each of `count` offsets.
    fn times(span: (usize, usize), count: usize) -> Vec<(usize, usize)> {
        vec![span; count]
    }

    #[test]
    fn a_word_takes_the_white_space_after_it() {
        let words = [times((0, 4), 4), times((4, 9), 5), times((9, 14), 5)].concat();
        assert_spans(
            "one two  three",
            &[words, vec![(14, 15)]].concat(),
            &singles(14),
        );
        // A tab and an ideographic space are white space too.
        assert_spans(
            "a\tb\u{3000}c",
            &[(0, 2), (0, 2), (2, 4), (2, 4), (4, 5), (5, 6)],
            &singles(5),
        );
    }

    #[test]
    fn a_run_of_one_punctuation_mark_is_one_word() {
        assert_spans("*", &[(0, 1), (1, 2)], &singles(1));
        assert_spans("**", &[(0, 2), (0, 2), (2, 3)], &singles(2));
        assert_spans(
            "****",
            &[(0, 4), (0, 4), (0, 4), (0, 4), (4, 5)],
            &singles(4),
        );
        assert_spans("...", &[(0, 3), (0, 3), (0, 3), (3, 4)], &singles(3));
        assert_spans(
            "\u{25cf}\u{25cf}\u{25cf}",
            &[(0, 3), (0, 3), (0, 3), (3, 4)],
            &singles(3),
        );
        assert_spans(
            "a -- b",
            &[(0, 2), (0, 2), (2, 5), (2, 5), (2, 5), (5, 6), (6, 7)],
            &singles(6),
        );
        // With the two letters NVDA's word segmentation appends: the stars
        // and the letters are one word, two dashes and the letters two.
        assert_spans(
            "****xx",
            &[times((0, 6), 6), vec![(6, 7)]].concat(),
            &singles(6),
        );
        assert_spans(
            "--xx",
            &[(0, 2), (0, 2), (2, 4), (2, 4), (4, 5)],
            &singles(4),
        );
    }

    #[test]
    fn an_apostrophe_and_a_full_stop_stay_in_their_word() {
        assert_spans(
            "don't stop",
            &[times((0, 6), 6), times((6, 10), 4), vec![(10, 11)]].concat(),
            &singles(10),
        );
        assert_spans(
            "tset. has",
            &[times((0, 6), 6), times((6, 9), 3), vec![(9, 10)]].concat(),
            &singles(9),
        );
    }

    #[test]
    fn white_space_before_punctuation_is_the_word_befores() {
        // NVDA's #1656: Uniscribe puts no word stop at punctuation after
        // white space, and the corrections end the word before after the
        // white space.
        assert_spans(
            "the end .",
            &[times((0, 4), 4), times((4, 8), 4), vec![(8, 9), (9, 10)]].concat(),
            &singles(9),
        );
        assert_spans(
            "x  .y",
            &[(0, 3), (0, 3), (0, 3), (3, 5), (3, 5), (5, 6)],
            &singles(5),
        );
    }

    #[test]
    fn a_line_break_is_white_space_after_a_word() {
        assert_spans(
            "  one two\r\nxx",
            &[
                times((0, 2), 2),
                times((2, 6), 4),
                times((6, 11), 5),
                times((11, 13), 2),
                vec![(13, 14)],
            ]
            .concat(),
            &singles(13),
        );
    }

    #[test]
    fn a_character_is_from_one_character_stop_to_the_next() {
        // A surrogate pair and a letter with its accent are one character
        // each; a carriage return and its line feed are two.
        assert_spans(
            "a\u{1F600}e\u{301}\r\n",
            &[
                (0, 1),
                (1, 3),
                (1, 3),
                (3, 7),
                (3, 7),
                (3, 7),
                (3, 7),
                (7, 8),
            ],
            &[
                (0, 1),
                (1, 3),
                (1, 3),
                (3, 5),
                (3, 5),
                (5, 6),
                (6, 7),
                (7, 8),
            ],
        );
    }

    #[test]
    fn thai_is_broken_by_its_dictionary() {
        // "สวัสดี", "ชาว" and "โลก", with the two letters NVDA appends.
        assert_spans(
            "\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}\u{e0a}\u{e32}\u{e27}\u{e42}\u{e25}\u{e01}xx",
            &[
                times((0, 6), 6),
                times((6, 9), 3),
                times((9, 14), 5),
                vec![(14, 15)],
            ]
            .concat(),
            &[
                vec![(0, 1), (1, 3), (1, 3), (3, 4), (4, 6), (4, 6)],
                (6..=14).map(|at| (at, at + 1)).collect(),
            ]
            .concat(),
        );
    }
}
