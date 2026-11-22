//! The MSAA executor's text (the request list's design, section 5): a
//! Win32 edit or rich edit control's text as a [`TextSource`], through
//! [`crate::edit`]'s window messages, as NVDA's `EditTextInfo` reads it
//! (`docs/nvda/editable-text-and-terminals.md`), and the text arms, each
//! text operation's one implementation on an MSAA node, over
//! `verbatim_reader_rules::text`'s algorithms.
//!
//! Positions are the control's UTF-16 offsets. A line is the control's
//! line, soft-wrapped lines included, from its start to the next line's
//! start, so a hard line break belongs to the line it ends; a paragraph is
//! a line, as in NVDA. Words are rich edit's own (`EM_FINDWORDBREAK`, from
//! rich edit 2.0, as NVDA finds them) or, for the plain edit control,
//! Uniscribe's ([`crate::uniscribe`]), each with the white space
//! after it, and a line break character a word of its own, as NVDA's
//! `EditTextInfo` forces Uniscribe for a plain edit control's line
//! ([`plain_word`]), so a run of one punctuation mark, such as a password
//! field's stars, is one word. Characters are Uniscribe's too, in every
//! edit control, as NVDA's `EditTextInfo` forces them. Sentences are left
//! to Core, which splits the paragraph. There are no pages.

use std::cmp::Ordering;

use verbatim_model::batch::{
    CaretAt, Done, Missing, SelectionCompared, StepOutcome, StepRefusal, Waited,
};
use verbatim_model::property::TextSummary;
use verbatim_model::request::{FocusCaret, FocusCaretRead};
use verbatim_model::{NodeId, TextAnchor, TextReply, TextUnit};
use verbatim_reader_rules::text::{
    self as rules, CallStep, CaretState, NodeText, Sentences, TextError, TextResult, TextSource,
    Unit,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

use crate::edit::{EditControl, EditError};

/// What the text arms need of the outpost: an MSAA node's ids, and its
/// text facts, which the outpost's store keeps.
pub trait TextResolve {
    /// The window, object id and child id node `node` is kept under;
    /// `None` for a node the outpost no longer knows.
    fn key_of(&self, node: NodeId) -> Option<(isize, i32, i32)>;

    /// Runs `read` on a copy of node `node`'s edit control text facts
    /// holding the anchors `named`, and commits what it changed.
    fn edit_anchors(
        &self,
        node: NodeId,
        named: &[u64],
        read: &mut dyn FnMut(&mut NodeText<'_, u32>),
    );

    /// The run's shared clock, in microseconds.
    fn now_us(&self) -> u64;
}

/// The edit API version of an MSAA object that is an edit control's client
/// area, `None` for any other object.
#[must_use]
pub fn edit_version(hwnd: isize, object: i32, child: i32) -> Option<u8> {
    if object != OBJID_CLIENT.0 || child != crate::CHILDID_SELF {
        return None;
    }
    crate::edit::edit_api_version(verbatim_model::class::normalize(
        &verbatim_win32::window::window_class_name(hwnd),
    ))
}

/// Runs `read` on node `node`'s text with a copy of its facts holding the
/// anchors `named`, committing what the read changed: `Err` with the reply
/// a node with no text, or gone, answers.
fn on_text<T>(
    resolver: &dyn TextResolve,
    node: NodeId,
    named: &[u64],
    mut read: impl FnMut(&mut EditText, &mut NodeText<'_, u32>) -> T,
) -> Result<T, TextReply> {
    let (hwnd, object, child) = resolver.key_of(node).ok_or(TextReply::Gone)?;
    let version = edit_version(hwnd, object, child).ok_or(TextReply::NoText)?;
    let mut source = EditText::new(hwnd, version);
    let mut found = None;
    resolver.edit_anchors(node, named, &mut |anchors| {
        found = Some(read(&mut source, anchors));
    });
    // The resolver runs the read exactly once.
    found.ok_or(TextReply::Unanswered)
}

/// A text step on an MSAA node, its points and held reads resolved by the
/// dispatcher.
#[derive(Clone, Copy, Debug)]
pub enum TextStep {
    /// `ReadText`, `ReadRange`, `Select` or `Locate`, run call by call.
    Calls(CallStep),
    /// `Caret`.
    Caret,
    /// `CompareSelection` between two held reads.
    Compare(TextAnchor, TextAnchor),
    /// `MoveBySentence` from a held read, forward or back.
    Sentence(TextAnchor, bool),
}

/// The text arms: `step` on MSAA node `node`'s text, and its outcome.
pub fn step(step: TextStep, node: NodeId, resolver: &dyn TextResolve) -> StepOutcome {
    match step {
        TextStep::Calls(call) => {
            let named: Vec<u64> = call.named().into_iter().map(|anchor| anchor.0).collect();
            let ran = on_text(resolver, node, &named, |source, anchors| {
                rules::by_calls(source, anchors, call)
            });
            match ran {
                Ok(Ok(Err(refused))) => refused,
                Ok(Ok(Ok(done))) => StepOutcome::Done(done),
                Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
                Ok(Err(TextError::Failed(reason))) => {
                    tracing::debug!(reason, "a batch's text step failed");
                    StepOutcome::Failed
                }
                Err(reply) => refused(&reply, node),
            }
        }
        TextStep::Caret => {
            let read_at_us = resolver.now_us();
            let ran = on_text(resolver, node, &[], |source, anchors| {
                rules::ops::caret(source, anchors, read_at_us)
            });
            outcome(ran, node, |caret: CaretAt| Ok(Done::Caret(caret)))
        }
        TextStep::Compare(before, after) => {
            let ran = on_text(resolver, node, &[before.0, after.0], |source, anchors| {
                rules::ops::compare_selection(source, anchors, (before, after))
            });
            outcome(
                ran,
                node,
                |compared: Result<SelectionCompared, TextAnchor>| {
                    compared
                        .map(Done::CompareSelection)
                        .map_err(Missing::Anchor)
                },
            )
        }
        TextStep::Sentence(from, forward) => {
            let ran = on_text(resolver, node, &[from.0], |source, anchors| {
                rules::sentence::move_by_sentence(source, anchors, (from, forward), &mut || {
                    verbatim_process::clock::now_us()
                })
            });
            match ran {
                Err(reply) => refused(&reply, node),
                Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
                Ok(Err(TextError::Failed(reason))) => {
                    tracing::debug!(reason, "a move by sentence failed");
                    StepOutcome::Failed
                }
                Ok(Ok(Err(outcome))) => outcome,
                Ok(Ok(Ok(moved))) => StepOutcome::Done(Done::MoveBySentence(moved)),
            }
        }
    }
}

/// The outcome of a text step on `node` whose read `ran`, its result made
/// by `done`.
fn outcome<T>(
    ran: Result<TextResult<T>, TextReply>,
    node: NodeId,
    done: impl FnOnce(T) -> Result<Done, Missing>,
) -> StepOutcome {
    match ran {
        Err(reply) => refused(&reply, node),
        Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
        Ok(Err(TextError::Failed(reason))) => {
            tracing::debug!(reason, "a batch's text step failed");
            StepOutcome::Failed
        }
        Ok(Ok(found)) => done(found).map_or_else(StepOutcome::NotThere, StepOutcome::Done),
    }
}

/// The outcome of a text step on `node` whose text could not be opened,
/// `reply` saying why.
fn refused(reply: &TextReply, node: NodeId) -> StepOutcome {
    match reply {
        TextReply::NoText => StepOutcome::Refused(StepRefusal::NoText),
        TextReply::Gone => StepOutcome::NotThere(Missing::Node(node)),
        TextReply::Unanswered => StepOutcome::Failed,
        other => {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, ?other, "a text step answered with a reply of another request");
            StepOutcome::Failed
        }
    }
}

/// The text summary of node `node` for report focus, as NVDA's object
/// speech reads it ([`rules::held::summary`]): `None` when it has no text
/// or the read failed, as NVDA then speaks no text.
#[must_use]
pub fn summary(resolver: &dyn TextResolve, node: NodeId) -> Option<TextSummary> {
    let read_at_us = resolver.now_us();
    on_text(resolver, node, &[], |source, anchors| {
        rules::held::summary(source, anchors, (None, read_at_us))
    })
    .ok()?
    .ok()
}

/// One poll of a caret wait over node `node`'s text, by parts
/// ([`rules::check::check`]): the check's end, if it ended the wait; `Err`
/// with the reply its text's opening answered.
///
/// # Errors
///
/// The reply a node with no text, or gone, answers.
pub fn check(
    resolver: &dyn TextResolve,
    node: NodeId,
    ask: rules::check::CheckAsk<'_>,
) -> Result<TextResult<Option<Waited>>, TextReply> {
    on_text(
        resolver,
        node,
        &[ask.wait.from.0],
        |source, anchors| match anchors.held_read(ask.wait.from) {
            Some(start) => rules::check::check(source, &start, ask),
            None => Err(TextError::Gone),
        },
    )
}

/// A focus's caret through node `node`'s text, read by parts: the line at
/// the caret and the selection, the next baseline held from the read,
/// compared with nothing. A node with no text answers
/// [`FocusCaret::NoText`], one whose read failed [`FocusCaret::NotRead`].
#[must_use]
pub fn focus_caret(resolver: &dyn TextResolve, node: NodeId) -> FocusCaret {
    let read_at_us = resolver.now_us();
    let read = on_text(resolver, node, &[], |source, anchors| {
        rules::held::read_focus(source, anchors, (None, read_at_us))
    });
    match read {
        Err(TextReply::NoText) => FocusCaret::NoText,
        Err(_) | Ok(Err(_)) => FocusCaret::NotRead,
        Ok(Ok((caret, selected, baseline))) => FocusCaret::Read(Box::new(FocusCaretRead {
            caret,
            selected_text: selected.map(|selected| selected.text),
            baseline,
        })),
    }
}

impl From<EditError> for TextError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::Gone => TextError::Gone,
            EditError::Failed(reason) => TextError::Failed(reason),
            EditError::Abandoned => {
                TextError::Failed("the worker was abandoned; nothing was sent".to_owned())
            }
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
    /// ([`crate::edit::edit_api_version`]).
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

    /// The span of the word containing `offset`, empty at the end of the
    /// text, where NVDA expands a collapsed position to no word
    /// (`OffsetsTextInfo.expand` in `nvda/source/textInfos/offsets.py`).
    /// A plain edit control's end is found from its last line's text, with
    /// no read of the length.
    fn word(&mut self, offset: u32) -> TextResult<(u32, u32)> {
        if self.control.version() >= 2 {
            if offset >= self.length()? {
                return Ok((offset, offset));
            }
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
        let (start, end) = plain_word(&text, at).map_err(|error| uniscribe_failed(&error))?;
        Ok((line.start + to_u32(start), line.start + to_u32(end)))
    }

    /// The span of the character containing `offset`, as Uniscribe breaks
    /// its line, which NVDA's `EditTextInfo` forces for every edit
    /// control's characters (`charSegFlag` in
    /// `nvda/source/NVDAObjects/window/edit.py`); empty at the end of the
    /// text.
    fn character(&mut self, offset: u32) -> TextResult<(u32, u32)> {
        let line = self.line(offset)?;
        let text = self.line_text(line, usize::MAX)?;
        let at = (offset - line.start) as usize;
        let (start, end) = if at >= text.len() {
            (text.len(), text.len())
        } else {
            crate::uniscribe::character(&text, at).map_err(|error| uniscribe_failed(&error))?
        };
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

/// Whether a code unit is part of a line break.
fn is_break(unit: u16) -> bool {
    matches!(unit, 0x0D | 0x0A)
}

/// A failed Uniscribe call as a text error.
fn uniscribe_failed(error: &windows::core::Error) -> TextError {
    TextError::Failed(format!("Uniscribe could not break the line: {error}"))
}

/// The word containing UTF-16 offset `at` of a line, its line break
/// included, as NVDA finds a plain edit control's word
/// (`docs/nvda/text-infos.md`, "Word and character segmentation"): a
/// carriage return or line feed at `at` is a word of its own
/// (`EditTextInfo._getWordOffsets` in
/// `nvda/source/NVDAObjects/window/edit.py`); otherwise the line, a null
/// character or no-break space read as a space and two letters appended,
/// is broken by Uniscribe ([`crate::uniscribe::word`]), and the
/// word found ends at the line's end at the latest
/// (`UniscribeWordSegmentationStrategy` in
/// `nvda/source/textUtils/_wordSeg/wordSegStrategy.py`, which appends the
/// letters because Uniscribe breaks a line with no run of more than two
/// letters oddly). So each word takes the white space after it, the line's
/// break included, and a run of one punctuation mark, such as "****", is
/// one word. Past the line's text, which is the end of the text, there is
/// no word: the span is empty.
fn plain_word(text: &[u16], at: usize) -> windows::core::Result<(usize, usize)> {
    if at >= text.len() {
        return Ok((text.len(), text.len()));
    }
    if is_break(text[at]) {
        return Ok((at, at + 1));
    }
    let units: Vec<u16> = text
        .iter()
        .map(|&unit| if matches!(unit, 0 | 0xA0) { 0x20 } else { unit })
        .chain("xx".encode_utf16())
        .collect();
    let (start, end) = crate::uniscribe::word(&units, at)?;
    Ok((start, end.min(text.len())))
}

impl TextSource for EditText {
    type Pos = u32;

    fn edit_selection(&self, state: &CaretState<u32>) -> Option<verbatim_model::EditSelection> {
        let (start, end) = state.selection.unwrap_or((state.caret, state.caret));
        Some(verbatim_model::EditSelection { start, end })
    }

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

    fn measure(&mut self, at: &u32) -> TextResult<Option<(u32, u32)>> {
        // Only the plain edit control is measured by offsets: NVDA reads a
        // rich edit through its text object model, whose location has no
        // percentage (`docs/parity/review.md`, "Review cursor
        // line/word/character over object text").
        if self.control.version() != 0 {
            return Ok(None);
        }
        Ok(Some((*at, self.length()?)))
    }

    fn unit_at(
        &mut self,
        at: &u32,
        unit: TextUnit,
        max_units: usize,
    ) -> TextResult<Option<Unit<u32>>> {
        let (start, end, text) = if matches!(unit, TextUnit::Line | TextUnit::Paragraph) {
            let line = self.line(*at)?;
            let text = self.line_text(line, max_units.saturating_add(1))?;
            (line.start, line.end, text)
        } else {
            let Some((start, end)) = self.span(*at, unit)? else {
                return Ok(None);
            };
            let cut = end.min(start.saturating_add(to_u32(max_units.saturating_add(1))));
            (start, end, self.control.text_range(start, cut)?)
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

    fn edges(&mut self, unit: &Unit<u32>, kind: TextUnit) -> (bool, bool) {
        // A text ending in a line break has an empty last line after it,
        // as the control counts its lines, so the line or paragraph that
        // ends with that break is not the last.
        let followed_by_empty_line = matches!(kind, TextUnit::Line | TextUnit::Paragraph)
            && unit.text.last() == Some(&u16::from(b'\n'));
        (
            unit.start == 0,
            self.length.is_some_and(|length| unit.end >= length) && !followed_by_empty_line,
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

    /// The plain edit word at `at` of `line`.
    fn word_at(line: &str, at: usize) -> (usize, usize) {
        plain_word(&units(line), at).expect("Uniscribe breaks the line")
    }

    // Every expected span below is what NVDA's own helper
    // (`calculateWordOffsets` in the pinned build's `nvdaHelperLocal.dll`)
    // gives for the same line with NVDA's two letters appended, ended at
    // the line's end.

    #[test]
    fn a_plain_edit_word_is_its_characters_and_the_white_space_after() {
        let line = "  one two\r\n";
        assert_eq!(word_at(line, 0), (0, 2), "leading spaces");
        assert_eq!(word_at(line, 2), (2, 6));
        assert_eq!(word_at(line, 4), (2, 6));
        assert_eq!(word_at(line, 5), (2, 6), "the space after the word");
        assert_eq!(
            word_at(line, 6),
            (6, 11),
            "the break is white space after it"
        );
        assert_eq!(word_at(line, 9), (9, 10), "the carriage return");
        assert_eq!(word_at(line, 10), (10, 11), "the line feed");
        assert_eq!(word_at(line, 11), (11, 11), "the end");
        assert_eq!(word_at("one two", 7), (7, 7), "the end after a word");
    }

    #[test]
    fn a_plain_edit_word_is_uniscribes() {
        // A password field's stars, and WPF's circles, are one word each.
        for at in 0..4 {
            assert_eq!(word_at("****", at), (0, 4), "the stars at {at}");
            assert_eq!(
                word_at("\u{25cf}\u{25cf}\u{25cf}\u{25cf}", at),
                (0, 4),
                "the circles at {at}"
            );
        }
        assert_eq!(
            word_at("**  **", 3),
            (0, 4),
            "the spaces after the first run"
        );
        assert_eq!(word_at("**  **", 4), (4, 6), "the second run");
        // Thai, written without spaces: "สวัสดี", "ชาว", "โลก".
        assert_eq!(word_at("สวัสดีชาวโลก", 6), (6, 9));
        assert_eq!(word_at("สวัสดีชาวโลก", 9), (9, 12));
        // A full stop stays in its word, with the space after it.
        assert_eq!(word_at("tset. has", 4), (0, 6));
        // A no-break space separates words as a space does.
        assert_eq!(word_at("a\u{A0}b", 0), (0, 2));
        assert_eq!(word_at("a\u{A0}b", 2), (2, 3));
    }

    #[test]
    fn a_rich_edit_is_not_measured_by_offsets() {
        // No window: a rich edit answers before asking the control.
        for version in [1, 2, 5] {
            let mut text = EditText::new(0, version);
            assert!(matches!(text.measure(&3), Ok(None)), "version {version}");
        }
    }
}
