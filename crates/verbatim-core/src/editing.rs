//! Editing (milestone M4): caret keys and the selection, following NVDA's
//! wait for evidence (`docs/nvda/editable-text-and-terminals.md`), and
//! typed character and word echo with the password rules
//! (`docs/parity/text.md` and `docs/parity/terminals.md`, "Typing").
//!
//! A caret key's lifecycle is the key line (`crate::key_line`); here are
//! its words: the character after Left or Right Arrow, Home, and End; the
//! provider's word after Control with Left or Right Arrow; the line after
//! Up or Down Arrow, the page keys, and Control with Home or End; the
//! paragraph after Control with Up or Down Arrow; what Backspace deleted;
//! the character or word now at the caret after Delete. A key with Shift
//! speaks what became selected or unselected instead, as does a selection
//! Verbatim makes itself (the review copy's first press, in `review_text`);
//! one the mouse or the application makes is not spoken. A newer key, a typed
//! character, or a focus change supersedes a key still watched, so speech
//! never lags behind fast typing, a later edit is not spoken as an earlier
//! key's answer, and a focus announcement wins over a caret line. A key that
//! moves nothing says its unit at its bound, as NVDA's script does.
//!
//! Formatting (milestone M4 item 7) is spoken as it changes, as NVDA
//! speaks it (`docs/nvda/document-formatting.md`): the outpost sends the
//! formatting of the text a caret key or a focus speaks, and only what
//! differs from the formatting last reported in that node is said, a
//! spelling error where it starts, and for a character or a word also where
//! it ends.

use std::sync::Arc;

use verbatim_model::{
    CaretKey, CaretMotion, CaretReply, CaretSelection, Effect, FocusText, FocusValidity, Message,
    NodeId, NodeSnapshot, Phrase, Role, SegmentContent, SelectionChange, SelectionText,
    SpeechPriority, State, TextAttributes, TextChunk, TextOp, TextPoint, TextRead, TextReply,
    TextUnit, TraceId, TypingEcho, Utterance, UtteranceSegment,
};

use crate::key_line::{Evidence, ReviewPress};
use crate::proven;
use crate::queries::{Asking, Then};
use crate::state::{CaretContext, NavigatorRead, PendingText, ReviewText, SrState, TextFollowUp};
use crate::text;

/// The most bytes of a word typed so far that word echo keeps; a longer
/// run of letters is not a word anyone needs echoed whole.
pub(crate) const MAX_TYPED_WORD: usize = 256;

/// NVDA speaks a selection change of this many characters or more as a
/// count rather than the text.
const SELECTION_SPOKEN_AS_COUNT: u32 = 512;

/// What a protected field echoes for each typed character: NVDA's
/// protected character, spoken by its name ("star").
const PROTECTED_CHARACTER: &str = "*";

/// Whether a role may have text the caret moves in: an edit field, a
/// document, or a terminal.
pub(crate) fn may_have_text(role: Role) -> bool {
    matches!(role, Role::EditableText | Role::Document | Role::Terminal)
}

/// Whether text in a node of `role` is a grid of terminal cells.
pub(crate) fn is_grid(role: Role) -> bool {
    role == Role::Terminal
}

/// Whether a node is a place text can be typed, for the typing echo
/// settings' "only in edit controls": an edit field, a terminal, or a
/// document that is not read-only.
fn is_editable(node: &NodeSnapshot) -> bool {
    match node.role {
        Role::EditableText | Role::Terminal => true,
        Role::Document => !node.states.contains(State::ReadOnly),
        _ => false,
    }
}

/// Queued speech for how a selection changed, each phrase its own
/// utterance, as NVDA speaks each text selected or unselected, and
/// "selection removed", on its own (`docs/parity/text.md`,
/// "TextInfo-equivalent layer").
pub(crate) fn speak_each(trace_id: TraceId, segments: Vec<UtteranceSegment>) -> Vec<Effect> {
    segments
        .into_iter()
        .map(|segment| speak(trace_id, vec![segment]))
        .collect()
}

/// Queued speech with no source node.
pub(crate) fn speak(trace_id: TraceId, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )
}

/// The focus whose caret keys are watched for evidence: alive, with a
/// caret Core knows or a role that may have text ([`caret_key`]'s test).
pub(crate) fn text_focus(state: &SrState) -> Option<NodeId> {
    let focus = state.focus.object().filter(|focus| focus.alive)?;
    let node = focus.snapshot.id;
    (focus.caret.is_some() || may_have_text(focus.snapshot.role)).then_some(node)
}

/// Whether `key` selects: with Shift, or Control+A.
pub(crate) fn selects(key: CaretKey) -> bool {
    key.select || key.motion == CaretMotion::SelectAll
}

/// What a Backspace is about to delete, from the caret's line before the
/// key, `offset` being the caret's byte offset in it, or a selection's
/// start: the character before it, or for Control+Backspace the text from
/// the start of the word before it up to it. At the
/// start of a line other than a terminal's, a Backspace, with or without
/// Control, deletes the line break before it, the kind of break the text uses, `line_break`
/// ([`CaretContext::line_break`]), which NVDA names
/// (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
/// character"). `None` when that is not known, and for other keys.
fn deleted_text(
    line: &TextChunk,
    offset: u32,
    line_break: Option<&str>,
    motion: CaretMotion,
    grid: bool,
) -> Option<String> {
    let content = text::line_content(&line.text, grid);
    let offset = text::boundary(content, text::byte_offset(offset));
    match motion {
        CaretMotion::Backspace => match text::previous_grapheme(content, offset) {
            Some(range) => Some(proven::text_in(content, range).to_owned()),
            None if !grid && !line.first => line_break.map(str::to_owned),
            None => None,
        },
        CaretMotion::BackspaceWord => {
            let words = text::words(content, line.language_at(0));
            match words.iter().rev().find(|range| range.start < offset) {
                Some(word) => Some(proven::text_in(content, word.start..offset).to_owned()),
                None if offset == 0 && !grid && !line.first => line_break.map(str::to_owned),
                None => None,
            }
        }
        _ => None,
    }
}

/// What a key that deletes backward really did to the caret's line,
/// compared before and after it.
#[derive(Debug, PartialEq, Eq)]
enum Removal {
    /// This text, which ended at the caret before the key, is gone, and the
    /// caret is where it began: the guard by text positions, which keeps a
    /// selection list's marker moving between lines from being spoken as a
    /// deletion (`docs/parity/text.md`, "TextInfo-equivalent layer").
    Removed(String),
    /// The line after the key ends with the whole of the line before, and
    /// the caret is where it began: the line was joined to the one above,
    /// its line break removed.
    Joined,
    /// Nothing was removed at the caret (text was added, or the line is
    /// another).
    Nothing,
}

/// What a key that deletes backward did, from the caret's line `before`
/// and `after` it, each its text and the caret's byte offset in it. In a
/// terminal (`grid`) a row's padding is not text, but the caret may stand
/// in it, past the text: there the row reads as spaces up to the caret, so
/// a typed space a Backspace removes is found, which padding hides.
fn removal(before: (&str, usize), after: (&str, usize), grid: bool) -> Removal {
    let read = |(text, caret): (&str, usize)| {
        let mut content = text::line_content(text, grid).to_owned();
        if grid && caret > content.len() {
            content.push_str(&" ".repeat(caret.saturating_sub(content.len())));
        }
        (content, caret)
    };
    let ((was, b), (now, a)) = (read(before), read(after));
    if a > b
        || !was.is_char_boundary(b)
        || !now.is_char_boundary(a)
        || b > was.len()
        || a > now.len()
    {
        return if a == now.len().saturating_sub(was.len()) && now.ends_with(&was) {
            Removal::Joined
        } else {
            Removal::Nothing
        };
    }
    let kept_before = was.get(..a) == now.get(..a);
    if a == b && kept_before && was.len() > now.len() {
        // The caret stayed and text after it went (Control+K).
        let end = a.saturating_add(was.len().saturating_sub(now.len()));
        if let Some(gone) = was.get(a..end)
            && was.get(end..) == now.get(a..)
        {
            return Removal::Removed(gone.to_owned());
        }
    }
    let kept_after = was.get(b..) == now.get(a..)
        || (grid && was.get(b..).map(str::trim_end) == now.get(a..).map(str::trim_end));
    match was.get(a..b) {
        Some(gone) if kept_before && kept_after && a < b => Removal::Removed(gone.to_owned()),
        _ => Removal::Nothing,
    }
}

/// How the selection changed with a report, as its speech needs it.
pub(crate) struct ReportGeneral {
    selection: Option<SelectionChange>,
    collapsed: bool,
    in_general: bool,
}

impl ReportGeneral {
    /// The general terms of the report, when it says its changes in them.
    pub(crate) fn as_general(&self) -> Option<General<'_>> {
        self.in_general.then_some(General {
            selection: self.selection.as_ref(),
            collapsed: self.collapsed,
        })
    }
}

/// A before read's selection changes, and how they are said.
pub(crate) fn changes_of_reply(reply: &CaretReply) -> (Vec<SelectionChange>, ReportGeneral) {
    (
        reply.selection_changes.clone(),
        ReportGeneral {
            selection: reply.new_selection.clone(),
            collapsed: reply.caret.selection.is_none(),
            in_general: reply.content_changed,
        },
    )
}

/// A report's parts as the reply its key's words read: the caret, its
/// changes and how they are said, and `facts`.
pub(crate) fn report_of(
    caret: verbatim_model::CaretReport,
    changes: &[SelectionChange],
    general: Option<General<'_>>,
    facts: &verbatim_model::ReadFacts,
) -> CaretReply {
    CaretReply {
        caret,
        selection_changes: changes.to_vec(),
        content_changed: general.is_some(),
        new_selection: general.and_then(|general| general.selection.cloned()),
        facts: facts.clone(),
    }
}

/// What a before read found, as the key keeps it.
pub(crate) struct BeforeRead {
    pub(crate) unit: TextChunk,
    pub(crate) line: TextChunk,
    pub(crate) removed: Option<String>,
}

/// What `reply`'s line says a key of `motion` would delete from the
/// selection's start or the caret, by [`deleted_text`], for its focus-first
/// words: the text before the selection's start when something is selected,
/// as NVDA's caret is the selection's start.
pub(crate) fn before_of_read(
    state: &SrState,
    node: NodeId,
    motion: CaretMotion,
    reply: &CaretReply,
) -> BeforeRead {
    let line = &reply.caret.line;
    let grid = state
        .focus
        .object()
        .is_some_and(|focus| focus.snapshot.id == node && is_grid(focus.snapshot.role));
    let line_break = state
        .focus_caret(node)
        .and_then(|caret| caret.line_break.as_deref())
        .or_else(|| text::line_break(&line.text));
    let removed = motion.deletes_back().then_some(()).and_then(|()| {
        let start = selection_start(line, reply.caret.selection.as_ref());
        let (start_line, offset) = match start {
            Some(Some(found)) => found,
            Some(None) => return None,
            None => (line, line.offset),
        };
        deleted_text(start_line, offset, line_break, motion, grid)
    });
    // Backspace's words if the focus moves first start from the selection's
    // start, NVDA's caret.
    let start_line = match selection_start(line, reply.caret.selection.as_ref()) {
        Some(Some((found, offset))) if motion.deletes_back() => TextChunk {
            offset,
            ..found.clone()
        },
        _ => line.clone(),
    };
    BeforeRead {
        unit: reply.facts.unit.clone().unwrap_or_else(|| line.clone()),
        line: start_line,
        removed,
    }
}

/// Where a selection starts as a line and an offset in it: `None` with
/// nothing selected, `Some(None)` when it starts on a line not reported.
#[expect(
    clippy::option_option,
    reason = "nothing selected and a start not reported are read apart"
)]
fn selection_start<'a>(
    line: &'a TextChunk,
    selection: Option<&'a CaretSelection>,
) -> Option<Option<(&'a TextChunk, u32)>> {
    let selection = selection?;
    Some(
        [Some(line), selection.start_line.as_deref()]
            .into_iter()
            .flatten()
            .find(|candidate| candidate.start == selection.range.start.anchor)
            .map(|found| (found, selection.range.start.offset)),
    )
}

/// The words of a key that has its evidence.
pub(crate) fn evidence_words(
    state: &mut SrState,
    node: NodeId,
    key: CaretKey,
    (read, evidence): (&CaretReply, Evidence<'_>),
) -> Vec<UtteranceSegment> {
    if selects(key) {
        return Vec::new();
    }
    let grid = state
        .focus
        .object()
        .is_some_and(|focus| focus.snapshot.id == node && is_grid(focus.snapshot.role));
    match key.motion {
        CaretMotion::Backspace | CaretMotion::BackspaceWord => {
            backspace_segments(state, node, key.motion, (read, evidence), grid)
        }
        // The selection removed with its text: said by its change alone,
        // after, never the removed text, as NVDA's caret event says it
        // (`docs/parity/text.md`, "TextInfo-equivalent layer"). NVDA has no
        // script for these keys, and says only its caret event's words.
        CaretMotion::Other
            if evidence.previous_selection.is_some() && read.caret.selection.is_none() =>
        {
            Vec::new()
        }
        CaretMotion::Other => {
            let mut reported = reported_format(state, node);
            let before = (
                evidence.previous_line.text.clone(),
                evidence.previous_line.offset,
            );
            let segments = other_key_segments(
                Some(&before),
                &read.caret.line,
                Some(evidence.on_previous_line),
                grid,
            )
            .unwrap_or_else(|| line_segments(&read.caret.line, grid, &mut reported));
            keep_reported_format(state, node, reported);
            segments
        }
        motion => unit_of(state, node, motion, &read.caret.line, unit_read(read)),
    }
}

/// The words of a key whose final read found nothing: the unit for a
/// movement key or Delete, nothing for Backspace, a Shift key or a key
/// outside the table. A sentence key moves by sentence instead.
pub(crate) fn bound_words(
    state: &mut SrState,
    node: NodeId,
    key: CaretKey,
    read: &CaretReply,
) -> Vec<UtteranceSegment> {
    if selects(key)
        || matches!(
            key.motion,
            CaretMotion::Backspace | CaretMotion::BackspaceWord | CaretMotion::Other
        )
    {
        return Vec::new();
    }
    unit_of(state, node, key.motion, &read.caret.line, unit_read(read))
}

/// The unit `read` found; the line where it fetched none.
fn unit_read(read: &CaretReply) -> &TextChunk {
    read.facts.unit.as_ref().unwrap_or(&read.caret.line)
}

/// Whether the words of a key with evidence read its unit: a Shift key, a
/// key that deletes backward and a key outside the table say what the
/// comparison shows instead.
pub(crate) fn words_read_the_unit(key: CaretKey) -> bool {
    !selects(key) && !key.motion.deletes_back() && key.motion != CaretMotion::Other
}

/// The words of a key whose evidence had no unit, from a later read that
/// fetched it: its unit, as its evidence would have said.
pub(crate) fn unit_words(
    state: &mut SrState,
    node: NodeId,
    key: CaretKey,
    read: &CaretReply,
) -> Vec<UtteranceSegment> {
    unit_of(state, node, key.motion, &read.caret.line, unit_read(read))
}

/// The words of a key whose focus moved first, from its before read.
pub(crate) fn focus_first_words(
    state: &SrState,
    key: CaretKey,
    (unit, line, removed): (&TextChunk, &TextChunk, Option<&str>),
) -> Vec<UtteranceSegment> {
    if selects(key) || key.motion == CaretMotion::Other {
        return Vec::new();
    }
    if key.motion.deletes_back() {
        return match removed {
            Some(removed) => deleted_segments(removed, key.motion),
            None if line.first && line.offset == 0 => text::character_segments(None, None),
            None => Vec::new(),
        };
    }
    let grid = state
        .focus
        .object()
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    let mut reported = TextAttributes::default();
    unit_segments(line, Some(unit), (key.motion.unit(), grid), &mut reported)
}

/// The unit of a key at a read, its formatting spoken as it changes.
fn unit_of(
    state: &mut SrState,
    node: NodeId,
    motion: CaretMotion,
    line: &TextChunk,
    unit: &TextChunk,
) -> Vec<UtteranceSegment> {
    let grid = state
        .focus
        .object()
        .is_some_and(|focus| focus.snapshot.id == node && is_grid(focus.snapshot.role));
    let mut reported = reported_format(state, node);
    let segments = unit_segments(line, Some(unit), (motion.unit(), grid), &mut reported);
    keep_reported_format(state, node, reported);
    segments
}

/// What a Backspace said, from its read: over a selection, what was before
/// the selection's start when the key was pressed, "blank" when nothing
/// was, as NVDA's Backspace script reads it from its caret, the selection's
/// start, before passing the key
/// (`docs/nvda/editable-text-and-terminals.md`); nothing when the selection
/// started on an earlier line not reported; the selection's change follows.
/// Otherwise what the key removed, by comparing the line before and after
/// it; a caret that moved forward with nothing removed says "blank", as
/// NVDA's empty deleted text does.
fn backspace_segments(
    state: &SrState,
    node: NodeId,
    motion: CaretMotion,
    (read, evidence): (&CaretReply, Evidence<'_>),
    grid: bool,
) -> Vec<UtteranceSegment> {
    let line_break = state
        .focus_caret(node)
        .and_then(|caret| caret.line_break.as_deref());
    // The text unchanged: with the caret not moved the key says nothing and
    // the selection's change is heard, and with it moved it says "blank". A
    // selection from an earlier line can leave the caret's line the same
    // though it was removed.
    let from_earlier_line = evidence
        .previous_selection
        .is_some_and(|selection| selection.start_line.is_some());
    if !from_earlier_line && read.caret.line.text == evidence.previous_line.text {
        return if read.caret.line.offset == evidence.previous_line.offset {
            Vec::new()
        } else {
            text::character_segments(None, None)
        };
    }
    if evidence.previous_selection.is_some() {
        let Some((line, offset)) =
            selection_start(evidence.previous_line, evidence.previous_selection).flatten()
        else {
            return Vec::new();
        };
        return match deleted_text(line, offset, line_break, motion, grid) {
            Some(deleted) => deleted_segments(&deleted, motion),
            None => text::character_segments(None, None),
        };
    }
    let line = &read.caret.line;
    let before = evidence.previous_line;
    let removed = removal(
        (&before.text, text::byte_offset(before.offset)),
        (&line.text, text::byte_offset(line.offset)),
        grid,
    );
    let deleted = deleted_text(before, before.offset, line_break, motion, grid);
    match (removed, deleted) {
        (Removal::Removed(removed), _) => deleted_segments(&removed, motion),
        // A line joined to the one before it: the line break went, as
        // predicted.
        (Removal::Joined, Some(deleted)) => deleted_segments(&deleted, motion),
        (Removal::Nothing, _) if line.offset > before.offset => {
            text::character_segments(None, None)
        }
        // Nothing removed, or no deletion predicted before the key.
        _ => Vec::new(),
    }
}

/// What a key outside the caret table did (`docs/parity/text.md`,
/// "TextInfo-equivalent layer"), from the caret's line `before` it (its text and
/// the caret's byte offset) and `after` it, and whether the caret is on the
/// line it was on (`same_line`, from the outpost): text removed at the
/// caret is spoken; a caret on another line speaks that line (`None`
/// here, which the caller reads with its formatting); a caret that moved
/// over an unchanged line speaks by where it landed ([`landing_segments`]);
/// anything else (text added, which typing echo or terminal output speak)
/// says nothing.
fn other_key_segments(
    before: Option<&(String, u32)>,
    after: &TextChunk,
    same_line: Option<bool>,
    grid: bool,
) -> Option<Vec<UtteranceSegment>> {
    let Some((text, offset)) = before else {
        return (same_line != Some(false)).then(Vec::new);
    };
    let removed = removal(
        (text, text::byte_offset(*offset)),
        (&after.text, text::byte_offset(after.offset)),
        grid,
    );
    if let Removal::Removed(removed) = removed {
        return Some(removed_segments(&removed, after.language_at(0)));
    }
    if same_line == Some(false) {
        return None;
    }
    if text::line_content(text, grid) == text::line_content(&after.text, grid) {
        return Some(landing_segments(text::byte_offset(*offset), after, grid));
    }
    Some(Vec::new())
}

/// The speech for text a key outside the caret table removed: a single
/// character by its name, otherwise the text.
fn removed_segments(removed: &str, language: Option<&str>) -> Vec<UtteranceSegment> {
    if text::graphemes_count(removed) == 1 {
        text::character_segments(Some(removed), language)
    } else {
        text::text_segments(removed.trim(), language)
    }
}

/// The landing rule (`docs/parity/text.md`, "TextInfo-equivalent layer"): a
/// caret that moved from `from` (a byte offset) on a line whose text is
/// unchanged, now at `line`'s offset, speaks by where it landed. By exactly
/// one character, or at the start or end of the line: the character at the
/// caret. At the start of a word, or just after its end: the whole word.
/// Inside a word: the word and then the character at the caret.
fn landing_segments(from: usize, line: &TextChunk, grid: bool) -> Vec<UtteranceSegment> {
    let content = text::line_content(&line.text, grid);
    let at = text::boundary(content, text::byte_offset(line.offset));
    let from = text::boundary(content, from);
    let language = line.language_at(at);
    let character = || {
        text::character_segments(
            text::grapheme_at(content, at).and_then(|range| content.get(range)),
            language,
        )
    };
    let one_character = text::grapheme_at(content, from.min(at))
        .is_some_and(|range| range.start == from.min(at) && range.end == from.max(at));
    if at == 0 || at >= content.len() || one_character {
        return character();
    }
    let words = text::words(content, language);
    if let Some(word) = words
        .iter()
        .find(|word| word.start == at)
        .or_else(|| words.iter().find(|word| word.end == at))
        .and_then(|word| content.get(word.clone()))
    {
        return text::word_segments(word, language);
    }
    if let Some(word) = words
        .iter()
        .find(|word| word.start < at && at < word.end)
        .and_then(|word| content.get(word.clone()))
    {
        let mut segments = text::word_segments(word, language);
        segments.extend(character());
        return segments;
    }
    character()
}

/// A sentence found in a paragraph: its start and its text.
pub(crate) struct Sentence {
    pub(crate) offset: u32,
    pub(crate) text: String,
}

/// The sentences of `paragraph` by Unicode's rules, within its content,
/// and the caret's byte offset in it (`docs/design/caret-selection-rework.md`,
/// "Moving by sentence").
fn sentences(paragraph: &TextChunk) -> (&str, usize, Vec<std::ops::Range<usize>>) {
    let content = text::line_content(&paragraph.text, false);
    let caret = text::boundary(content, text::byte_offset(paragraph.offset));
    let ranges = verbatim_text::Segmenter::new().sentences(content);
    (content, caret, ranges)
}

/// The sentence at `range` of `content`.
fn sentence_at(content: &str, range: std::ops::Range<usize>) -> Sentence {
    Sentence {
        offset: u32::try_from(range.start).unwrap_or(u32::MAX),
        text: proven::text_in(content, range).to_owned(),
    }
}

/// The first sentence starting after `paragraph`'s caret, Alt+Down's
/// landing within it.
pub(crate) fn next_sentence(paragraph: &TextChunk) -> Option<Sentence> {
    let (content, caret, ranges) = sentences(paragraph);
    let range = ranges.into_iter().find(|range| range.start > caret)?;
    Some(sentence_at(content, range))
}

/// The sentence holding the character before `paragraph`'s caret, Alt+Up's
/// landing within it, as NVDA steps back one offset and takes that
/// sentence's start.
pub(crate) fn sentence_before(paragraph: &TextChunk) -> Option<Sentence> {
    let (content, caret, ranges) = sentences(paragraph);
    let range = ranges.into_iter().rfind(|range| range.start < caret)?;
    Some(sentence_at(content, range))
}

/// `paragraph`'s first sentence; an empty one at its start when it has
/// none.
pub(crate) fn first_sentence(paragraph: &TextChunk) -> Sentence {
    let (content, _, ranges) = sentences(paragraph);
    sentence_at(content, ranges.into_iter().next().unwrap_or(0..0))
}

/// `paragraph`'s last sentence; an empty one at its start when it has
/// none.
pub(crate) fn last_sentence(paragraph: &TextChunk) -> Sentence {
    let (content, _, ranges) = sentences(paragraph);
    sentence_at(content, ranges.into_iter().next_back().unwrap_or(0..0))
}

/// The speech for a sentence moved to, in `line`: "blank" where there is
/// none, as NVDA reads the empty text at the caret.
pub(crate) fn sentence_words(line: &TextChunk, sentence: &str) -> Vec<UtteranceSegment> {
    let spoken = sentence.trim();
    if spoken.is_empty() {
        return text::character_segments(None, None);
    }
    text::text_segments(spoken, line.language_at(text::byte_offset(line.offset)))
}

/// The speech for text a Backspace deleted: a character, or a line break
/// whichever key deleted it, by its name, and a word as text. A carriage
/// return and line feed deleted together are spoken as the line feed, as
/// NVDA speaks them (`docs/parity/text.md`, "TextInfo-equivalent layer").
fn deleted_segments(deleted: &str, motion: CaretMotion) -> Vec<UtteranceSegment> {
    if motion == CaretMotion::Backspace || text::line_break(deleted) == Some(deleted) {
        let deleted = if deleted == "\r\n" { "\n" } else { deleted };
        text::character_segments(Some(deleted), None)
    } else {
        text::text_segments(deleted.trim(), None)
    }
}

/// The formatting last reported in `node`'s text, NVDA's per-object cache,
/// kept on the focus object: nothing yet for a node other than the focus.
fn reported_format(state: &SrState, node: NodeId) -> TextAttributes {
    state
        .focus
        .object()
        .filter(|focus| focus.snapshot.id == node)
        .map(|focus| focus.reported_format.clone())
        .unwrap_or_default()
}

/// Keeps `reported` as the formatting last reported in `node`'s text, on
/// the focus object when `node` is the focus; a node no longer the focus
/// keeps nothing, as its object is gone.
fn keep_reported_format(state: &mut SrState, node: NodeId, reported: TextAttributes) {
    if let Some(focus) = state
        .focus
        .object_mut()
        .filter(|focus| focus.snapshot.id == node)
    {
        focus.reported_format = reported;
    }
}

/// The speech for a chunk's content with the formatting the outpost sent
/// for it, as [`text::formatted_segments`] makes it: the whole content
/// without its line break (and in a terminal its padding) carries the
/// formatting, and only the content without surrounding white space is
/// spoken for a word. `None` when the chunk has no formatting.
fn formatted(
    chunk: &TextChunk,
    grid: bool,
    how: text::Spoken,
    reported: &mut TextAttributes,
) -> Option<Vec<UtteranceSegment>> {
    let content = text::chunk_content(chunk, grid);
    let spoken = match how {
        text::Spoken::Word => {
            let start = content.len().saturating_sub(content.trim_start().len());
            start..content.trim_end().len().max(start)
        }
        // The character the chunk starts with, a line break included.
        text::Spoken::Character => text::characters(&chunk.text, grid)
            .into_iter()
            .next()
            .unwrap_or(0..0),
        text::Spoken::Text => 0..content.len(),
    };
    text::formatted_segments(
        chunk,
        (0..content.len(), spoken),
        how,
        reported,
        chunk.language_at(0),
    )
}

/// The speech for the unit at the caret after a caret key: the character,
/// the provider's word or paragraph when it sent one, and the line
/// otherwise. A word that is a single character is spoken by its name
/// ([`text::word_segments`]). Formatting the outpost sent with the text is
/// spoken as it changes from `reported`, which it then updates.
fn unit_segments(
    line: &TextChunk,
    unit_chunk: Option<&TextChunk>,
    (unit, grid): (TextUnit, bool),
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    let unit_chunk = unit_chunk.filter(|chunk| chunk.unit != TextUnit::Line);
    let content = text::line_content(&line.text, grid);
    let offset = text::byte_offset(line.offset);
    match (unit, unit_chunk) {
        (TextUnit::Character, chunk) => {
            if let Some(segments) =
                chunk.and_then(|chunk| formatted(chunk, grid, text::Spoken::Character, reported))
            {
                return segments;
            }
            let character = text::character_at(&line.text, offset, grid);
            text::character_segments(character, line.language_at(offset))
        }
        (TextUnit::Line, _) | (_, None) => formatted(line, grid, text::Spoken::Text, reported)
            .unwrap_or_else(|| text::text_segments(content, line.language_at(0))),
        (unit, Some(chunk)) => {
            if unit == TextUnit::Word
                && let Some(segments) = formatted(chunk, grid, text::Spoken::Word, reported)
            {
                return segments;
            }
            let read = text::chunk_content(chunk, grid).trim();
            if unit == TextUnit::Word {
                text::word_segments(read, chunk.language_at(0))
            } else {
                text::text_segments(read, chunk.language_at(0))
            }
        }
    }
}

/// The new selection of a report whose selection changes are said in
/// general terms, the text having changed since the selection compared
/// with was reported ([`selection_segments`]).
#[derive(Clone, Copy)]
pub(crate) struct General<'a> {
    /// The whole new selection's text, when it holds something.
    pub(crate) selection: Option<&'a SelectionChange>,
    /// Nothing is selected now.
    pub(crate) collapsed: bool,
}

/// The speech for a selection change: each change as NVDA's "selected" or
/// "unselected", a single character by its name, 512 characters or more as
/// a count, and a change with no text as nothing. With `general`, the text
/// having changed since the selection compared with was reported, the
/// changes say only what happened and the words come from the new
/// selection (`docs/parity/text.md`, "TextInfo-equivalent layer";
/// `docs/nvda/editable-text-and-terminals.md`, "Selection changes"): the
/// new selection's text as "selected" when something was selected, then,
/// when something was unselected, the new selection's text as "selected
/// instead", or "selection removed" when nothing is selected now.
pub(crate) fn selection_segments(
    changes: &[SelectionChange],
    general: Option<General<'_>>,
) -> Vec<UtteranceSegment> {
    let Some(general) = general else {
        return changes
            .iter()
            .filter(|change| change.characters > 0)
            .map(|change| {
                let text = selection_text(change);
                let phrase = if change.selected {
                    Phrase::Selected(text)
                } else {
                    Phrase::Unselected(text)
                };
                UtteranceSegment::new(SegmentContent::Phrase(phrase))
            })
            .collect();
    };
    let new_text = general
        .selection
        .filter(|selection| selection.characters > 0)
        .map(selection_text);
    let mut segments = Vec::new();
    if changes.iter().any(|change| change.selected)
        && let Some(text) = new_text.clone()
    {
        segments.push(UtteranceSegment::new(SegmentContent::Phrase(
            Phrase::Selected(text),
        )));
    }
    if changes.iter().any(|change| !change.selected) {
        if general.collapsed {
            segments.push(UtteranceSegment::new(SegmentContent::Message(
                Message::SelectionRemoved,
            )));
        } else if let Some(text) = new_text {
            segments.push(UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::SelectedInstead(text),
            )));
        }
    }
    segments
}

/// A selection change's text as it is spoken: a single character by its
/// name, 512 characters or more as a count.
fn selection_text(change: &SelectionChange) -> SelectionText {
    if change.characters >= SELECTION_SPOKEN_AS_COUNT {
        SelectionText::Characters(change.characters)
    } else if change.characters == 1 {
        SelectionText::Character(change.text.clone())
    } else {
        SelectionText::Text(change.text.clone())
    }
}

/// Speaks how the selection of the focus `node` changed with a report no
/// key's words came from: the mouse's or the application's selection, with
/// the same words as a selecting key's ([`selection_segments`]), as NVDA
/// reports any selection change of a control that reports its selection
/// through caret events (`docs/nvda/editable-text-and-terminals.md`,
/// "Selection changes"; `docs/parity/text.md`, "TextInfo-equivalent layer").
/// While a caret key is watched, its answer speaks them after the key's own
/// words ([`crate::key_line::changes_heard`]).
pub(crate) fn selection_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    (changes, general): (&[SelectionChange], Option<General<'_>>),
) -> Vec<Effect> {
    if !state.focus_matches(node) {
        return Vec::new();
    }
    // Even with no change of its own, a report says what waits for the line.
    let segments = selection_segments(changes, general);
    crate::key_line::changes_heard(state, trace_id, node, segments)
}

/// Takes a caret report of `node` as the focus's caret when it is newest,
/// by the time its read began, and moves the review cursor to it when the
/// review cursor follows the caret and the navigator is on that node (to
/// read the caret afresh, as the review cursor does at
/// [`ReviewText::Caret`]). A report read before the caret Core holds, such
/// as a key's answer read before a caret event that reached Core first,
/// is only looked at for the kind of line break the text uses: the caret
/// stays the newest known, and the review cursor stays with it.
pub(crate) fn update_caret(state: &mut SrState, node: NodeId, caret: verbatim_model::CaretReport) {
    if !state.focus_matches(node) {
        return;
    }
    let line = Arc::new(caret.line);
    let read_at_us = caret.read_at_us;
    if let Some(known) = state
        .focus
        .object_mut()
        .and_then(|focus| focus.caret.as_mut())
        .filter(|known| known.read_at_us > read_at_us)
    {
        // The older report still shows the kind of line break the text uses.
        if known.line_break.is_none() {
            known.line_break = text::line_break(&line.text).map(str::to_owned);
        }
        return;
    }
    follow_caret(state, node);
    let Some(focus) = state.focus.object_mut() else {
        return;
    };
    let line_break = text::line_break(&line.text)
        .map(str::to_owned)
        .or_else(|| focus.caret.take().and_then(|caret| caret.line_break));
    focus.caret = Some(CaretContext {
        line,
        selection: caret.selection,
        line_break,
        read_at_us,
    });
}

/// Takes a caret Core itself moved `node`'s caret to, `line` with the caret
/// at its offset and nothing selected, as the focus's caret, stamped with
/// the time the move began.
pub(crate) fn caret_moved_by_core(
    state: &mut SrState,
    node: NodeId,
    line: TextChunk,
    read_at_us: u64,
) {
    update_caret(
        state,
        node,
        verbatim_model::CaretReport {
            line,
            selection: None,
            read_at_us,
        },
    );
}

/// Puts the review cursor at the caret of `node` when it follows the caret
/// and the navigator is on that node: the next review command reads the
/// line at the caret as the outpost finds it ([`ReviewText::Caret`]).
///
/// A terminal's review cursor follows the caret only when its reading rules
/// find that a key moved the caret, which its outpost tells
/// (`crate::terminal`), never on a caret report alone, since a program
/// moves the cursor as it draws (`books/dev/src/explanation/design/terminal-model.md`,
/// "Reading rules", rule 24).
pub(crate) fn follow_caret(state: &mut SrState, node: NodeId) {
    if state.settings.follow_caret
        && let Some(navigator) = state.navigator.as_mut()
        && navigator.object.id == node
        && !is_grid(navigator.object.role)
    {
        navigator.text = ReviewText::Caret;
    }
}

/// Ends the announcement of the focus `node` with what the outpost's caret
/// read found, carried with the focus (`docs/nvda/speech.md`, "What an
/// object with text says"): the caret's line, or "selected" and the
/// selected text when text is selected, or, for an object with no text to
/// read, its value, which the focus's announcement left out. Its name and
/// role were not held back for it, and nothing comes between them, since
/// both come from the one message. A protected field's text is read as any
/// field's, masked as the control gives it (`docs/parity/focus-announcement.md`,
/// "Text on focus"), and one whose text could not be read speaks neither
/// its line nor its value. The caret, when read, becomes Core's.
pub(crate) fn focus_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    text: Option<&FocusText>,
) -> Vec<Effect> {
    keep_focus_caret(state, node.id, text);
    if !may_have_text(node.role) {
        return Vec::new();
    }
    match text {
        Some(FocusText::Caret {
            caret,
            selected_text,
            ..
        }) => {
            let selected = caret
                .selection
                .as_ref()
                .and(selected_text.clone())
                .and_then(selected_segments);
            match selected {
                Some(segments) => focus_speech(state, trace_id, node.id, segments),
                None => focus_line(state, trace_id, node.id),
            }
        }
        Some(FocusText::NoText) => focus_value(state, trace_id, node.id),
        Some(FocusText::NotRead) | None => Vec::new(),
    }
}

/// Takes the caret a focus `node` came with, when its text was read, as
/// Core's caret, without speaking: for a focus announced
/// ([`focus_text`]) and for one taken silently, a duplicate or the same
/// focus reported again by a replacement outpost.
pub(crate) fn keep_focus_caret(state: &mut SrState, node: NodeId, text: Option<&FocusText>) {
    if let Some(FocusText::Caret { caret, .. }) = text {
        update_caret(state, node, caret.clone());
    }
}

/// The speech for an object's selected text: "selected" and the text (NVDA's
/// word order for text already selected, unlike a change's), or
/// its number of characters when there are 512 or more. `None` when nothing
/// is selected.
fn selected_segments(text: String) -> Option<Vec<UtteranceSegment>> {
    let characters = u32::try_from(verbatim_text::graphemes(&text).len()).unwrap_or(u32::MAX);
    if characters == 0 {
        return None;
    }
    let selected = if characters >= SELECTION_SPOKEN_AS_COUNT {
        SelectionText::Characters(characters)
    } else {
        SelectionText::Text(text)
    };
    Some(vec![UtteranceSegment::new(SegmentContent::Phrase(
        Phrase::Preselected(selected),
    ))])
}

/// Speaks the value of a focus that turned out to have no text to read, as
/// any object without text speaks its value.
fn focus_value(state: &SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(value) = state
        .focus
        .object()
        .and_then(|focus| focus.snapshot.value.clone())
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    focus_speech(state, trace_id, node, vec![UtteranceSegment::value(value)])
}

/// Speaks the caret's line of the focus `node`, "blank" when it has nothing
/// to read, with the formatting at its start and wherever it changes, as a
/// change from none reported yet.
fn focus_line(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(segments) = caret_line(state, node) else {
        return Vec::new();
    };
    focus_speech(state, trace_id, node, segments)
}

/// The speech for the caret's line in the focus `node`, "blank" when it has
/// nothing to read, with the formatting at its start and wherever it
/// changes, as a change from the formatting last reported there. `None`
/// when Core does not know the caret of `node`.
fn caret_line(state: &mut SrState, node: NodeId) -> Option<Vec<UtteranceSegment>> {
    let caret = state.focus_caret(node)?;
    let grid = state
        .focus
        .object()
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    let mut reported = reported_format(state, node);
    let segments = line_segments(&caret.line, grid, &mut reported);
    keep_reported_format(state, node, reported);
    Some(segments)
}

/// The speech for a line read as a caret movement reads it: its content,
/// "blank" when it has none, with the formatting the outpost sent spoken as
/// it changes from `reported`.
fn line_segments(
    line: &TextChunk,
    grid: bool,
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    formatted(line, grid, text::Spoken::Text, reported).unwrap_or_else(|| {
        text::text_segments(text::line_content(&line.text, grid), line.language_at(0))
    })
}

/// Reads the text of the navigator object `node` for `read`: for its
/// announcement, made by object navigation or by reporting the current
/// object, an object that may have text says its text in place of its
/// value, as a focus does (`docs/nvda/speech.md`, "What an object with text
/// says"), whether or not it has the focus; reporting it a second or third
/// time spells or copies its name and that text. The text is the selection,
/// announced as "selected" and the selected text, or the line at the caret;
/// a control that reports no caret reads its first line. The focus's caret,
/// once Core knows it, is read without asking the outpost; otherwise the
/// outpost is asked for the selected text, then, when nothing is selected,
/// the caret's line ([`navigator_text_reply`]). A protected field's text is
/// read as any field's, as on focus.
pub(crate) fn navigator_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
    press: Option<ReviewPress>,
) -> Vec<Effect> {
    text_of_object(state, trace_id, node, (read, false), press)
}

/// Reads the text of `node`, Verbatim+Tab's answer, which is neither the
/// navigator object nor the focus Core holds, for its announcement, as
/// [`navigator_text`] reads the navigator object's: its follow-ups carry
/// the object (`PendingText::about`).
pub(crate) fn reported_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
) -> Vec<Effect> {
    text_of_object(state, trace_id, node, (NavigatorRead::Announce, true), None)
}

/// [`navigator_text`], its follow-ups carrying `node` when `carried`.
fn text_of_object(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    (read, carried): (NavigatorRead, bool),
    press: Option<ReviewPress>,
) -> Vec<Effect> {
    if !may_have_text(node.role) {
        return Vec::new();
    }
    let (start, end) = match state.focus_caret(node.id) {
        Some(caret) => match &caret.selection {
            Some(selection) => (
                TextPoint::At(selection.range.start),
                TextPoint::At(selection.range.end),
            ),
            None if read == NavigatorRead::Announce => {
                return caret_line(state, node.id)
                    .map(|segments| vec![speak_about(trace_id, node, segments)])
                    .unwrap_or_default();
            }
            None => {
                let line = text::line_content(&caret.line.text, is_grid(node.role)).to_owned();
                return named_text(trace_id, node, read, &line);
            }
        },
        None => (TextPoint::SelectionStart, TextPoint::SelectionEnd),
    };
    navigator_request(
        state,
        (node.id, carried.then(|| Box::new(node.clone()))),
        TextOp::ReadRange {
            start,
            end,
            through: None,
        },
        TextFollowUp::NavigatorSelection(read),
        press,
    )
}

/// Spells or copies, by `read`, the name of `node` followed by `body`, the
/// text it would announce, as reporting the current object a second or
/// third time does for an object with text.
fn named_text(
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
    body: &str,
) -> Vec<Effect> {
    let text = [node.name.as_deref().unwrap_or_default(), body]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Vec::new();
    }
    match read {
        NavigatorRead::Copy => vec![Effect::CopyToClipboard(text)],
        _ => vec![speak_about(trace_id, node, crate::reduce::spelled(&text))],
    }
}

/// Asks the outpost of `node` for its text, for the navigator's
/// announcement to go on with `then`: a press's follow-up read, which keeps
/// the press current until it is answered, or a read of its own, which a
/// newer text request supersedes.
fn navigator_request(
    state: &mut SrState,
    (node, about): (NodeId, Option<Box<NodeSnapshot>>),
    op: TextOp,
    then: TextFollowUp,
    press: Option<ReviewPress>,
) -> Vec<Effect> {
    if let Some(press) = press {
        let (query, effect) = state
            .queries
            .ask(Asking::Text(node, op), Then::Review(then));
        let mut effects = crate::key_line::press_read(state, node, query, press);
        effects.push(effect);
        return effects;
    }
    state.queries.forget(|then| matches!(then, Then::Text(_)));
    let (_, effect) = state.queries.ask(
        Asking::Text(node, op),
        Then::Text(PendingText { node, then, about }),
    );
    vec![effect]
}

/// Ends what [`navigator_text`] started with the answer it asked for: the
/// selected text when there is some, else, after asking for it, the caret's
/// line; the value when the object turns out to have no text to read, and
/// for spelling or copying the name and value, as for any object. Nothing
/// is done once the object asked about is neither the navigator object nor
/// the focus, unless the request carries it, as Verbatim+Tab's answer's
/// does (`PendingText::about`).
pub(crate) fn navigator_text_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingText,
    reply: TextReply,
    press: Option<ReviewPress>,
) -> Vec<Effect> {
    let navigator = state.navigator.as_ref().map(|navigator| &navigator.object);
    let focus = state
        .focus
        .object()
        .filter(|focus| focus.alive)
        .map(|focus| &focus.snapshot);
    let carried = pending.about.as_deref();
    let Some(object) = carried
        .into_iter()
        .chain(navigator)
        .chain(focus)
        .find(|object| object.id == pending.node)
        .cloned()
    else {
        return Vec::new();
    };
    let (TextFollowUp::NavigatorSelection(read) | TextFollowUp::NavigatorLine(read)) = pending.then
    else {
        return Vec::new();
    };
    let segments = match (&pending.then, reply) {
        (TextFollowUp::NavigatorSelection(_), TextReply::Range { text, .. }) => {
            if verbatim_text::graphemes(&text).is_empty() {
                return navigator_line(state, (pending.node, pending.about.clone()), read, press);
            }
            if read != NavigatorRead::Announce {
                return named_text(trace_id, &object, read, &text);
            }
            selected_segments(text).unwrap_or_default()
        }
        (TextFollowUp::NavigatorSelection(_), TextReply::Unsupported) => {
            return navigator_line(state, (pending.node, pending.about.clone()), read, press);
        }
        (TextFollowUp::NavigatorLine(_), TextReply::Read { chunk, .. }) => {
            let grid = is_grid(object.role);
            if read != NavigatorRead::Announce {
                return named_text(
                    trace_id,
                    &object,
                    read,
                    text::line_content(&chunk.text, grid),
                );
            }
            line_segments(&chunk, grid, &mut TextAttributes::default())
        }
        (_, TextReply::NoText | TextReply::Unsupported) => {
            if read != NavigatorRead::Announce {
                return named_text(
                    trace_id,
                    &object,
                    read,
                    object.value.as_deref().unwrap_or_default(),
                );
            }
            match object.value.as_ref().filter(|value| !value.is_empty()) {
                Some(value) => vec![UtteranceSegment::value(value.clone())],
                None => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };
    vec![speak_about(trace_id, &object, segments)]
}

/// Asks for the line at the caret of the navigator object `node`, or its
/// first line when it reports no caret, for `read`.
fn navigator_line(
    state: &mut SrState,
    node: (NodeId, Option<Box<NodeSnapshot>>),
    read: NavigatorRead,
    press: Option<ReviewPress>,
) -> Vec<Effect> {
    navigator_request(
        state,
        node,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        }),
        TextFollowUp::NavigatorLine(read),
        press,
    )
}

/// Queued speech about `node`, valid however the focus moves, as the rest
/// of a navigator announcement is.
fn speak_about(trace_id: TraceId, node: &NodeSnapshot, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(node)),
            say_all: false,
            validity: None,
        },
        None,
    )
}

/// The rest of a focus announcement, queued and valid while `node` is the
/// focus, as the announcement itself is.
fn focus_speech(
    state: &SrState,
    trace_id: TraceId,
    node: NodeId,
    segments: Vec<UtteranceSegment>,
) -> Vec<Effect> {
    let Some(focus) = state
        .focus
        .object()
        .filter(|focus| focus.alive && focus.snapshot.id == node)
    else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(crate::reduce::source_of(&focus.snapshot)),
            validity: Some(FocusValidity {
                node,
                had_focus: true,
                window: verbatim_model::WindowIdentity::of(&focus.snapshot),
            }),
            say_all: false,
        },
        None,
    )]
}

/// Handles a key that ends or clears the command line being typed (Escape,
/// Control+C, Control+D, Control+Break): in a terminal, the word being
/// typed is forgotten; the terminal's reading rules, told of the key by the
/// hook, forget the typing they hold, never shown, as NVDA forgets its
/// queued characters on Enter, Tab, Control+C, Control+D, and
/// Control+Break (`docs/parity/terminals.md`, "New terminal output").
pub(crate) fn clearing_key(state: &mut SrState) {
    let terminal = state
        .focus
        .object()
        .is_some_and(|focus| focus.alive && focus.snapshot.role == Role::Terminal);
    if terminal {
        state.typed_word.clear();
    }
}

/// Handles text typed into the focused application: echoes characters and
/// words by the settings, a protected field's characters as the protected
/// character only. What is typed into a terminal is echoed once the
/// terminal shows it, which its reading rules tell (`crate::terminal`),
/// unless the user asked for terminal passwords to be spoken, when it is
/// echoed as it is typed.
pub(crate) fn character_typed(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    // A caret key still watched is not answered by the caret the typing
    // moves: the typing's key press has marked it (`key_line::mark_later`),
    // so its answer only keeps the caret current.
    let Some(focus) = state.focus.object().filter(|focus| focus.alive) else {
        return effects;
    };
    if focus.snapshot.role == Role::Terminal {
        if typed.chars().any(|c| matches!(c, '\r' | '\n' | '\t')) {
            // Enter or Tab ends the word being typed, which in a terminal
            // is never echoed whole (Escape and the Control keys that clear
            // a line come as [`clearing_key`]).
            state.typed_word.clear();
        } else if state.settings.speak_terminal_passwords {
            effects.extend(echo(state, trace_id, typed));
        }
        return effects;
    }
    effects.extend(echo(state, trace_id, typed));
    effects
}

/// Echoes typed text by the settings: a finished word first, when word echo
/// is on and the text ends one, then the characters.
pub(crate) fn echo(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let Some(focus) = state.focus.object().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let editable = is_editable(&focus.snapshot);
    let protected = focus.snapshot.states.contains(State::Protected);
    let applies = |mode: TypingEcho| match mode {
        TypingEcho::Off => false,
        TypingEcho::EditControls => editable,
        TypingEcho::Always => true,
    };
    let echo_characters = applies(state.settings.speak_typed_characters);
    let echo_words = applies(state.settings.speak_typed_words) && !protected;
    let mut effects = Vec::new();
    for character in verbatim_text::graphemes(typed)
        .into_iter()
        .filter_map(|range| typed.get(range))
    {
        // A letter, mark, or number continues the word, so a virama, a
        // Thai tone mark, or a zero-width non-joiner typed on its own does
        // not end it.
        let word_character = verbatim_text::is_word_grapheme(character);
        if word_character {
            if echo_words
                && state.typed_word.len().saturating_add(character.len()) <= MAX_TYPED_WORD
            {
                state.typed_word.push_str(character);
            }
        } else {
            let word = std::mem::take(&mut state.typed_word);
            if echo_words && !word.is_empty() {
                effects.push(speak(trace_id, vec![UtteranceSegment::text(word)]));
            }
        }
        // A control character, Tab, Enter, or Backspace among them, ends a
        // word but is never spelled, as in NVDA (docs/nvda/input.md, "Typed
        // characters, IME, and composition"): Tab in a dialog moves focus,
        // and saying "tab" there would cut off the control it reached.
        let printable = !character.chars().any(char::is_control);
        if echo_characters && printable {
            let spoken = if protected {
                PROTECTED_CHARACTER
            } else {
                character
            };
            effects.push(speak(
                trace_id,
                vec![UtteranceSegment::new(SegmentContent::Character(
                    spoken.to_owned(),
                ))],
            ));
        }
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::{Removal, landing_segments, other_key_segments, removal};
    use verbatim_model::{SegmentContent, TextAnchor, TextChunk, TextUnit, UtteranceSegment};

    /// The line `text` with the caret at byte `offset`.
    fn line(text: &str, offset: u32) -> TextChunk {
        TextChunk {
            unit: TextUnit::Line,
            text: text.to_owned(),
            start: TextAnchor(1),
            offset,
            languages: Vec::new(),
            first: false,
            last: false,
            truncated: false,
            formats: Vec::new(),
        }
    }

    /// What the segments say: characters and text as they are, "blank" for
    /// the blank message.
    fn said(segments: &[UtteranceSegment]) -> Vec<String> {
        segments
            .iter()
            .map(|segment| match &segment.content {
                SegmentContent::Character(character) | SegmentContent::Text(character) => {
                    character.clone()
                }
                SegmentContent::Message(verbatim_model::Message::Blank) => "blank".to_owned(),
                other => panic!("unexpected segment {other:?}"),
            })
            .collect()
    }

    fn removed(text: &str) -> Removal {
        Removal::Removed(text.to_owned())
    }

    #[test]
    fn what_a_backspace_removed_comes_from_the_line_before_and_after() {
        // A character, and with Control a word.
        assert_eq!(removal(("abc\n", 3), ("ab\n", 2), false), removed("c"));
        assert_eq!(
            removal(("hello world\r\n", 11), ("hello \r\n", 6), false),
            removed("world")
        );
        // The classic edit control's Control+Backspace inserts a DEL
        // character: nothing was removed.
        assert_eq!(
            removal(("hello\r\n", 5), ("hello\u{7f}\r\n", 6), false),
            Removal::Nothing
        );
        // A Hindi vowel sign removed on its own, as edit controls do.
        assert_eq!(removal(("कि", 6), ("क", 3), false), removed("ि"));
        // Backspace at a line's start joins it to the line above.
        assert_eq!(
            removal(("def\r\n", 0), ("abcdef\r\n", 3), false),
            Removal::Joined
        );
        // Text changed elsewhere is not a removal at the caret.
        assert_eq!(removal(("abc", 3), ("xb", 2), false), Removal::Nothing);
    }

    #[test]
    fn a_terminal_space_hidden_by_padding_is_found() {
        // A typed space after "echo": the row reads "ready> echo" and padding,
        // with the caret one past the text; Backspace leaves the caret at
        // its end.
        assert_eq!(
            removal(("ready> echo     ", 12), ("ready> echo     ", 11), true),
            removed(" ")
        );
        assert_eq!(
            removal(("ready> echo     ", 11), ("ready> ech      ", 10), true),
            removed("o")
        );
    }

    #[test]
    fn a_key_outside_the_table_speaks_by_where_the_caret_landed() {
        let text = "ready> echo hello world";
        let landed = |from: usize, at: u32| said(&landing_segments(from, &line(text, at), true));
        // To the start or the end of the line: the character there.
        assert_eq!(landed(12, 0), ["r"]);
        assert_eq!(landed(12, 23), ["blank"]);
        // By exactly one character: the character at the caret.
        assert_eq!(landed(12, 13), ["e"]);
        // To a word's start, or just after its end: the word.
        assert_eq!(landed(23, 12), ["hello"]);
        assert_eq!(landed(7, 17), ["hello"]);
        // Inside a word: the word, then the character at the caret.
        assert_eq!(landed(7, 14), ["hello", "l"]);
    }

    #[test]
    fn a_key_outside_the_table_speaks_text_it_removed_and_a_new_line() {
        let before = ("ready> echo hello".to_owned(), 17);
        // Control+W: the word before the caret.
        assert_eq!(
            other_key_segments(Some(&before), &line("ready> echo ", 12), Some(true), true)
                .map(|segments| said(&segments)),
            Some(vec!["hello".to_owned()])
        );
        // Control+K from "echo": what followed the caret.
        let at_echo = ("ready> echo hello".to_owned(), 7);
        assert_eq!(
            other_key_segments(Some(&at_echo), &line("ready> ", 7), Some(true), true)
                .map(|segments| said(&segments)),
            Some(vec!["echo hello".to_owned()])
        );
        // Another line: read by the caller, with its formatting.
        assert_eq!(
            other_key_segments(Some(&before), &line("other", 0), Some(false), true),
            None
        );
        // Text added (a paste): typing echo or output speak it.
        assert_eq!(
            other_key_segments(
                Some(&before),
                &line("ready> echo hello!", 18),
                Some(true),
                true
            ),
            Some(Vec::new())
        );
    }
}
