//! A Win32 edit or rich edit control's text as a [`TextSource`], through
//! [`verbatim_ia2::edit`]'s window messages, as NVDA's `EditTextInfo`
//! reads it (`docs/nvda/editable-text-and-terminals.md`).
//!
//! Positions are the control's UTF-16 offsets. A line is the control's
//! line, soft-wrapped lines included, from its start to the next line's
//! start, so a hard line break belongs to the line it ends; a paragraph is
//! a line, as in NVDA. Words are rich edit's own (`EM_FINDWORDBREAK`, from
//! rich edit 2.0, as NVDA finds them) or, for the plain edit control, its
//! default word breaking: a run of characters other than spaces, tabs, and
//! line breaks, with the spaces after it. Sentences are left to Core, which
//! splits the paragraph. There are no pages.

use std::cmp::Ordering;

use verbatim_ia2::edit::{EditControl, EditError};
use verbatim_model::TextUnit;

use super::{CaretState, Sentences, TextError, TextResult, TextSource, Unit};

impl From<EditError> for TextError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::Gone => TextError::Gone,
            EditError::Failed(reason) => TextError::Failed(reason),
        }
    }
}

/// A Win32 edit control's text.
pub struct EditText {
    control: EditControl,
    /// The text's length, once read during this request.
    length: Option<u32>,
}

/// A line: its number, start, and end (the next line's start).
#[derive(Clone, Copy)]
struct Line {
    number: u32,
    start: u32,
    end: u32,
}

impl EditText {
    /// The edit control `hwnd`, of edit API `version`
    /// ([`verbatim_ia2::edit::edit_api_version`]).
    #[must_use]
    pub fn new(hwnd: isize, version: u8) -> Self {
        Self {
            control: EditControl::new(hwnd, version),
            length: None,
        }
    }

    /// The text's length, read once per request.
    fn length(&mut self) -> TextResult<u32> {
        if let Some(length) = self.length {
            return Ok(length);
        }
        let length = self.control.text_length()?;
        self.length = Some(length);
        Ok(length)
    }

    /// The line containing `offset`.
    fn line(&mut self, offset: u32) -> TextResult<Line> {
        let number = self.control.line_from_offset(offset)?;
        self.line_number(number)
    }

    /// Line `number`.
    fn line_number(&mut self, number: u32) -> TextResult<Line> {
        let start = self.control.line_start(number)?.unwrap_or(0);
        let end = match self.control.line_start(number + 1)? {
            Some(next) if next > start => next,
            _ => self.length()?,
        };
        Ok(Line { number, start, end })
    }

    /// A line's text, its line break included, at most `max` code units.
    fn line_text(&mut self, line: Line, max: usize) -> TextResult<Vec<u16>> {
        if self.control.version() >= 2 {
            let end = line.end.min(line.start.saturating_add(to_u32(max)));
            return Ok(self.control.text_range(line.start, end)?);
        }
        // `EM_GETLINE` leaves out the line break; a plain edit control's
        // break is a carriage return and line feed, or with soft breaks
        // shown, two carriage returns and a line feed.
        let mut text = self.control.line_text(line.number, max)?;
        let gap = (line.end - line.start) as usize;
        let gap = gap.saturating_sub(text.len()).min(3);
        let breaks: Vec<u16> = "\r\r\n".encode_utf16().collect();
        text.extend_from_slice(&breaks[3 - gap..]);
        text.truncate(max);
        Ok(text)
    }

    /// The span of the word containing `offset`.
    fn word(&mut self, offset: u32) -> TextResult<(u32, u32)> {
        if self.control.version() >= 2 {
            // As NVDA's `_getWordOffsets` asks a rich edit control.
            let mut start = self.control.find_word_break(false, offset)?;
            let mut end = self.control.find_word_break(true, start)?;
            if end <= offset {
                start = end;
                end = self.control.find_word_break(true, offset)?;
            }
            return Ok((start, end.max(start)));
        }
        let line = self.line(offset)?;
        let text = self.line_text(line, usize::MAX)?;
        let at = (offset - line.start) as usize;
        let (start, end) = plain_word(&text, at);
        Ok((line.start + to_u32(start), line.start + to_u32(end)))
    }

    /// The span of the character (grapheme cluster) containing `offset`.
    fn character(&mut self, offset: u32) -> TextResult<(u32, u32)> {
        let line = self.line(offset)?;
        let text = self.line_text(line, usize::MAX)?;
        let at = (offset - line.start) as usize;
        let (start, end) = grapheme_units(&text, at);
        Ok((line.start + to_u32(start), line.start + to_u32(end)))
    }

    /// The span of `unit` containing `offset`, `None` for one the control
    /// does not have.
    fn span(&mut self, offset: u32, unit: TextUnit) -> TextResult<Option<(u32, u32)>> {
        Ok(Some(match unit {
            TextUnit::Line | TextUnit::Paragraph => {
                let line = self.line(offset)?;
                (line.start, line.end)
            }
            TextUnit::Word => self.word(offset)?,
            TextUnit::Character => self.character(offset)?,
            TextUnit::Document => (0, self.length()?),
            _ => return Ok(None),
        }))
    }
}

/// A count of code units as an offset.
fn to_u32(units: usize) -> u32 {
    u32::try_from(units).unwrap_or(u32::MAX)
}

/// Whether a code unit breaks words in a plain edit control.
fn is_delimiter(unit: u16) -> bool {
    matches!(unit, 0x20 | 0x09 | 0x0D | 0x0A)
}

/// The word containing UTF-16 offset `at` of a line, by the plain edit
/// control's default rule: characters other than delimiters, with the
/// delimiters after them; a run of delimiters at the line's start is a
/// word of its own.
fn plain_word(text: &[u16], at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let mut start = at;
    // Within delimiters, back over them, then over the word they follow.
    if at == text.len() || is_delimiter(text[at]) {
        while start > 0 && is_delimiter(text[start - 1]) {
            start -= 1;
        }
    }
    while start > 0 && !is_delimiter(text[start - 1]) {
        start -= 1;
    }
    if start < text.len() && is_delimiter(text[start]) {
        // Leading delimiters: the word is the delimiter run.
        let mut end = start;
        while end < text.len() && is_delimiter(text[end]) {
            end += 1;
        }
        return (start, end);
    }
    let mut end = start;
    while end < text.len() && !is_delimiter(text[end]) {
        end += 1;
    }
    while end < text.len() && is_delimiter(text[end]) {
        end += 1;
    }
    (start, end.max(at.min(text.len())))
}

/// The grapheme cluster containing UTF-16 offset `at` of `text`, as UTF-16
/// offsets; an empty span at the end.
fn grapheme_units(text: &[u16], at: usize) -> (usize, usize) {
    let string = String::from_utf16_lossy(text);
    let mut units = 0;
    for range in verbatim_text::graphemes(&string) {
        let length: usize = string[range].chars().map(char::len_utf16).sum();
        if at < units + length {
            return (units, units + length);
        }
        units += length;
    }
    (units, units)
}

impl TextSource for EditText {
    type Pos = u32;

    fn caret(&mut self) -> TextResult<CaretState<u32>> {
        // As NVDA's `EditTextInfo`, the caret is the selection's start.
        let (start, end) = self.control.selection()?;
        let (start, end) = (start.min(end), start.max(end));
        Ok(CaretState {
            caret: start,
            selection: (start != end).then_some((start, end)),
        })
    }

    fn start(&mut self) -> TextResult<u32> {
        Ok(0)
    }

    fn end(&mut self) -> TextResult<u32> {
        self.length()
    }

    fn unit_at(
        &mut self,
        at: &u32,
        unit: TextUnit,
        max_units: usize,
    ) -> TextResult<Option<Unit<u32>>> {
        let Some((start, end)) = self.span(*at, unit)? else {
            return Ok(None);
        };
        let text = if matches!(unit, TextUnit::Line | TextUnit::Paragraph) {
            let line = self.line(start)?;
            self.line_text(line, max_units.saturating_add(1))?
        } else {
            let cut = end.min(start.saturating_add(to_u32(max_units.saturating_add(1))));
            self.control.text_range(start, cut)?
        };
        let truncated = text.len() > max_units;
        let mut text = text;
        text.truncate(max_units);
        Ok(Some(Unit {
            start,
            end,
            text,
            truncated,
        }))
    }

    fn move_by(&mut self, at: &u32, unit: TextUnit, count: i32) -> TextResult<Option<(u32, i32)>> {
        match unit {
            TextUnit::Line | TextUnit::Paragraph => {
                let line = self.line(*at)?;
                let last = self.control.line_count()?.saturating_sub(1);
                let target = (i64::from(line.number) + i64::from(count)).clamp(0, i64::from(last));
                let target = u32::try_from(target).unwrap_or(0);
                let landed = self.line_number(target)?.start;
                let moved = i32::try_from(i64::from(target) - i64::from(line.number)).unwrap_or(0);
                Ok(Some((landed, moved)))
            }
            TextUnit::Word | TextUnit::Character => {
                let length = self.length()?;
                let Some((mut pos, _)) = self.span(*at, unit)? else {
                    return Ok(None);
                };
                let mut moved = 0;
                while moved != count {
                    if count > 0 {
                        let Some((_, end)) = self.span(pos, unit)? else {
                            break;
                        };
                        if end >= length || end <= pos {
                            break;
                        }
                        pos = end;
                        moved += 1;
                    } else {
                        if pos == 0 {
                            break;
                        }
                        let Some((start, _)) = self.span(pos - 1, unit)? else {
                            break;
                        };
                        pos = start;
                        moved -= 1;
                    }
                }
                Ok(Some((pos, moved)))
            }
            _ => Ok(None),
        }
    }

    fn text(&mut self, start: &u32, end: &u32, max_units: usize) -> TextResult<(Vec<u16>, bool)> {
        let cut = (*end).min(start.saturating_add(to_u32(max_units)));
        Ok((self.control.text_range(*start, cut)?, cut < *end))
    }

    fn offset_in(&mut self, unit: &Unit<u32>, at: &u32) -> TextResult<usize> {
        Ok((at.saturating_sub(unit.start) as usize).min(unit.text.len()))
    }

    fn advance(&mut self, from: &u32, prefix: &[u16]) -> TextResult<u32> {
        Ok(from.saturating_add(to_u32(prefix.len())))
    }

    fn compare(&mut self, a: &u32, b: &u32) -> TextResult<Ordering> {
        Ok(a.cmp(b))
    }

    fn select(&mut self, start: &u32, end: &u32) -> TextResult<bool> {
        self.control.set_selection(*start, *end)?;
        Ok(true)
    }

    fn location(&mut self, at: &u32) -> TextResult<Option<(i32, i32)>> {
        Ok(self.control.position_of(*at)?)
    }

    fn languages(&mut self, _unit: &Unit<u32>) -> Vec<(usize, usize, String)> {
        Vec::new()
    }

    fn edges(&mut self, unit: &Unit<u32>, _kind: TextUnit) -> (bool, bool) {
        (
            unit.start == 0,
            self.length.is_some_and(|length| unit.end >= length),
        )
    }

    fn sentences(&self) -> Sentences {
        Sentences::ByParagraph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_plain_edit_word_is_its_characters_and_the_spaces_after() {
        let line = units("  one two\r\n");
        assert_eq!(plain_word(&line, 0), (0, 2), "leading spaces");
        assert_eq!(plain_word(&line, 2), (2, 6));
        assert_eq!(plain_word(&line, 4), (2, 6));
        assert_eq!(plain_word(&line, 5), (2, 6), "the space after the word");
        assert_eq!(
            plain_word(&line, 6),
            (6, 11),
            "the last word takes the break"
        );
        assert_eq!(plain_word(&line, 11), (6, 11), "the end");
    }

    #[test]
    fn a_character_is_a_grapheme_cluster_in_code_units() {
        let text = units("a😀e\u{301}");
        assert_eq!(grapheme_units(&text, 0), (0, 1));
        assert_eq!(
            grapheme_units(&text, 2),
            (1, 3),
            "inside the surrogate pair"
        );
        assert_eq!(grapheme_units(&text, 3), (3, 5), "a letter and its accent");
        assert_eq!(grapheme_units(&text, 5), (5, 5), "the end");
    }
}
