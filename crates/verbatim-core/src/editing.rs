//! Editing (milestone M4): caret keys and the selection, following NVDA's
//! wait for evidence (`docs/nvda/editable-text-and-terminals.md`), and
//! typed character and word echo with the password rules
//! (`docs/parity/text.md` and `docs/parity/terminals.md`, "Typing").
//!
//! A caret key's lifecycle is its event-queue entry
//! (`crate::keys::caret_key`); here are its words: the character after
//! Left or Right Arrow, Home, and End; the provider's word after Control
//! with Left or Right Arrow; the line after Up or Down Arrow, the page keys,
//! and Control with Home or End; the paragraph after Control with Up or
//! Down Arrow; what Backspace deleted; the character or word now at the
//! caret after Delete. A key with Shift speaks what became selected or
//! unselected instead, as does a selection Verbatim makes itself (the
//! review copy's first press, in `review_text`); one the mouse or the
//! application makes is not spoken. A key that moves nothing says its unit
//! at its bound, as NVDA's script does.
//!
//! Formatting (milestone M4 item 7) is spoken as it changes, as NVDA
//! speaks it (`docs/nvda/document-formatting.md`): the outpost sends the
//! formatting of the text a caret key or a focus speaks, and only what
//! differs from the formatting last reported in that node is said, a
//! spelling error where it starts, and for a character or a word also where
//! it ends.

use std::sync::Arc;

use verbatim_model::{
    CaretKey, CaretMotion, CaretSelection, Effect, FocusText, FocusValidity, Message, NodeId,
    NodeSnapshot, Phrase, Role, SegmentContent, SelectionChange, SelectionText, SpeechPriority,
    State, TextAttributes, TextChunk, TextPoint, TextUnit, TraceId, TypingEcho, Utterance,
    UtteranceSegment,
};

use crate::state::{CaretContext, NavigatorRead, ReviewText, SrState};
use crate::text;
use crate::text_request::{Span, TextAnswer, TextAsk};
use verbatim_model::batch::StepRefusal;
use verbatim_text::proven;

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
/// settings' "only in edit controls": an edit field, a document or a
/// terminal that is not read-only, as NVDA excludes a read-only focus of
/// every role (`docs/parity/input.md`, "Typed-character echo").
fn is_editable(node: &NodeSnapshot) -> bool {
    may_have_text(node.role) && !node.states.contains(State::ReadOnly)
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
    let focus = state.versions.live().map(|focus| &focus.context)?;
    let node = focus.snapshot.id;
    (focus.caret.is_some() || may_have_text(focus.snapshot.role)).then_some(node)
}

/// Whether `key` selects: with Shift, or Control+A.
pub(crate) fn selects(key: CaretKey) -> bool {
    key.select || key.motion == CaretMotion::SelectAll
}

/// What a Backspace is about to delete, `offset` being the caret's byte
/// offset in the caret's `line` before the key, or a selection's start.
/// Outside a terminal it is the provider's `unit` around the character
/// before the caret, which the starting read read
/// ([`verbatim_model::StartingUnit::BeforeCaret`]): the character, or for
/// Control+Backspace the whole word, as NVDA's Backspace scripts say it
/// (`docs/parity/text.md`, "What Backspace and Delete say"), at every
/// offset: at the start of a line the provider's unit is the line break
/// before it, which NVDA names (`editableText.py` 299 to 318), and at the
/// document's start the provider moves nothing and the unit is empty. In
/// a terminal it is the character before the caret, or the text from the
/// start of the word before it up to it. `None` when that is not known,
/// and for other keys.
fn deleted_text(
    (line, offset): (&TextChunk, u32),
    unit: &TextChunk,
    motion: CaretMotion,
    grid: bool,
) -> Option<String> {
    if !grid && motion.deletes_back() {
        return Some(unit.text.clone()).filter(|deleted| !deleted.is_empty());
    }
    let content = text::line_content(&line.text, grid);
    let offset = text::boundary(content, text::byte_offset(offset));
    match motion {
        CaretMotion::Backspace => text::previous_grapheme(content, offset)
            .map(|range| proven::text_in(content, range).to_owned()),
        CaretMotion::BackspaceWord => {
            let words = text::words(content, line.language_at(0));
            words
                .iter()
                .rev()
                .find(|range| range.start < offset)
                .map(|word| proven::text_in(content, word.start..offset).to_owned())
        }
        _ => None,
    }
}

/// What a before read found, as the key keeps it.
pub(crate) struct BeforeRead {
    pub(crate) unit: TextChunk,
    pub(crate) line: TextChunk,
    pub(crate) removed: Option<String>,
}

/// What a caret key's starting read found before the key: its `caret`
/// and the key's `unit` read with it, in text that is a terminal's `grid`
/// or not.
pub(crate) fn before_of_caret(
    grid: bool,
    motion: CaretMotion,
    (caret, unit): (&verbatim_model::CaretReport, &TextChunk),
) -> BeforeRead {
    let line = &caret.line;
    let removed = motion.deletes_back().then_some(()).and_then(|()| {
        let start = selection_start(line, caret.selection.as_ref());
        let (start_line, offset) = match start {
            Some(Some(found)) => found,
            Some(None) => return None,
            None => (line, line.offset),
        };
        deleted_text((start_line, offset), unit, motion, grid)
    });
    // Backspace's words if the focus moves first start from the selection's
    // start, NVDA's caret.
    let start_line = match selection_start(line, caret.selection.as_ref()) {
        Some(Some((found, offset))) if motion.deletes_back() => TextChunk {
            offset,
            ..found.clone()
        },
        _ => line.clone(),
    };
    BeforeRead {
        unit: unit.clone(),
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
        (line.start == selection.range.start.anchor)
            .then_some((line, selection.range.start.offset)),
    )
}

/// The words of a key whose focus moved first, from its before read.
pub(crate) fn focus_first_words(
    grid: bool,
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
    let mut reported = TextAttributes::default();
    unit_segments(line, Some(unit), (key.motion.unit(), grid), &mut reported)
}

/// [`unit_of`], its formatting compared with and written to `reported`,
/// the memory of the focus record a key is bound to.
pub(crate) fn unit_with(
    grid: bool,
    (motion, line, unit): (CaretMotion, &TextChunk, &TextChunk),
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    unit_segments(line, Some(unit), (motion.unit(), grid), reported)
}

/// The committed focus's record, `(node, the trace it was taken from)`,
/// and its formatting memory, which a caret key bound to it captures at
/// its press (ADR 0001, "The formatting last reported goes with the focus
/// object").
pub(crate) fn focus_record(state: &SrState) -> Option<(NodeId, Option<TraceId>, TextAttributes)> {
    state
        .versions
        .focus()
        .map(|focus| &focus.context)
        .map(|focus| {
            (
                focus.snapshot.id,
                focus.trace_id,
                focus.reported_format.clone(),
            )
        })
}

/// Writes `reported`, the memory a key bound to the focus record
/// `(node, record)` updated, back to the focus object when it is still that
/// record; a later focus taken is a new record, which keeps its own.
pub(crate) fn formatting_back(
    state: &mut SrState,
    (node, record): (NodeId, Option<TraceId>),
    reported: TextAttributes,
) {
    if let Some(focus) = state
        .versions
        .focus_mut()
        .map(|focus| &mut focus.context)
        .filter(|focus| focus.snapshot.id == node && focus.trace_id == record)
    {
        focus.reported_format = reported;
    }
}

/// The speech for Enter's new line, the caret's `line` after the key, as
/// NVDA's new-line script speaks it (`docs/parity/text.md`, "Enter in a
/// text field"): the line's text from its start up to the caret, with its
/// formatting as it changes from `reported`; where that text is blank,
/// nothing of it, NVDA suppressing blanks, but the formatting at its start,
/// or where it is empty, at the caret's character, NVDA's opening fields.
/// `reported` becomes the formatting last spoken.
pub(crate) fn new_line_segments(
    line: &TextChunk,
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    let content = text::line_content(&line.text, false);
    // The chunk's offset decoded as a boundary of its text, so of its
    // content up to the content's end.
    let offset = text::byte_offset(line.offset).min(content.len());
    let before = verbatim_text::proven::text_in(content, ..offset);
    if text::is_blank(before) {
        let at = if before.is_empty() { offset } else { 0 };
        return text::opening_format(line, at, reported);
    }
    let language = line.language_at(0);
    text::formatted_segments(
        line,
        (0..offset, 0..offset),
        text::Spoken::Text,
        reported,
        language,
    )
    .unwrap_or_else(|| text::text_segments(before, language))
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

/// The speech for text a Backspace deleted, as NVDA's Backspace scripts
/// speak it: one character, or a line break whichever key deleted it, by
/// its name, and anything longer as text (`editableText.py` 314 to 318). A
/// carriage return and line feed deleted together are spoken as the line
/// feed, as NVDA speaks them (`docs/parity/text.md`, "TextInfo-equivalent
/// layer").
fn deleted_segments(deleted: &str, motion: CaretMotion) -> Vec<UtteranceSegment> {
    if motion == CaretMotion::Backspace
        || text::line_break(deleted) == Some(deleted)
        || verbatim_text::graphemes(deleted).len() == 1
    {
        let deleted = if deleted == "\r\n" { "\n" } else { deleted };
        text::character_segments(Some(deleted), None)
    } else {
        text::text_segments(deleted.trim(), None)
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
        // A word of nothing but white space is spoken whole
        // ([`text::word_text`]).
        text::Spoken::Word if !content.is_empty() && text::is_blank(content) => 0..content.len(),
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
            let how = if unit == TextUnit::Word {
                text::Spoken::Word
            } else {
                text::Spoken::Text
            };
            if let Some(segments) = formatted(chunk, grid, how, reported) {
                return segments;
            }
            let read = text::chunk_content(chunk, grid);
            if unit == TextUnit::Word {
                text::word_segments(read, chunk.language_at(0))
            } else {
                text::text_segments(read.trim(), chunk.language_at(0))
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

/// The speech for a selection change: every selected change, then every
/// unselected one, as NVDA's `speakSelectionChange` speaks its two lists
/// (`speech/speech.py` 1389 to 1407), each as NVDA's "selected" or
/// "unselected", a single character by its name, 512 characters or more as
/// a count, and a change with no text as nothing. With `general`, the text
/// having changed since the selection compared with was reported, the
/// changes say only what happened and the words come from the new
/// selection (`docs/parity/text.md`, "TextInfo-equivalent layer";
/// `docs/nvda/editable-text-and-terminals.md`, "Selection changes"): the
/// new selection's text as "selected" when something was selected, then,
/// when something was unselected, its text deleted or not, the new
/// selection's text as "selected instead", or "selection removed" when
/// nothing is selected now.
pub(crate) fn selection_segments(
    changes: &[SelectionChange],
    general: Option<General<'_>>,
) -> Vec<UtteranceSegment> {
    let Some(general) = general else {
        let (selected, unselected): (Vec<_>, Vec<_>) =
            changes.iter().partition(|change| change.selected);
        return selected
            .into_iter()
            .chain(unselected)
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
    if change.characters == 1 {
        SelectionText::Character(change.text.clone())
    } else {
        text_or_count(change.characters, change.text.clone())
    }
}

/// Selected `text` of `characters` characters as it is spoken: the text,
/// or its number of characters when there are 512 or more.
fn text_or_count(characters: u32, text: String) -> SelectionText {
    if characters >= SELECTION_SPOKEN_AS_COUNT {
        SelectionText::Characters(characters)
    } else {
        SelectionText::Text(text)
    }
}

/// Takes `caret`, read of the focus `node`, as Core's caret, and moves the
/// review cursor to it when the review cursor follows the caret and the
/// navigator is on that node. An event-queue entry's answers are taken in
/// arrival order, as one worker answers the queue's requests one at a
/// time and an abandoned worker's commits are refused
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts").
pub(crate) fn take_caret(state: &mut SrState, node: NodeId, caret: verbatim_model::CaretReport) {
    place_caret(state, node, caret.line, caret.selection);
}

/// Takes a caret Core itself moved `node`'s caret to, `line` with the caret
/// at its offset and nothing selected, as the focus's caret, in arrival
/// order ([`take_caret`]).
pub(crate) fn caret_placed(state: &mut SrState, node: NodeId, line: TextChunk) {
    place_caret(state, node, line, None);
}

/// [`take_caret`]'s work, from the caret's `line` and `selection`.
fn place_caret(
    state: &mut SrState,
    node: NodeId,
    line: TextChunk,
    selection: Option<verbatim_model::CaretSelection>,
) {
    if state
        .versions
        .live()
        .is_none_or(|focus| focus.context.snapshot.id != node)
    {
        return;
    }
    follow_caret(state, node);
    let Some(focus) = state.versions.focus_mut().map(|focus| &mut focus.context) else {
        return;
    };
    focus.caret = Some(CaretContext {
        line: Arc::new(line),
        selection,
    });
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
        take_caret(state, node, caret.clone());
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
    Some(vec![UtteranceSegment::new(SegmentContent::Phrase(
        Phrase::Preselected(text_or_count(characters, text)),
    ))])
}

/// Speaks the value of a focus that turned out to have no text to read, as
/// any object without text speaks its value.
fn focus_value(state: &SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(value) = state
        .versions
        .focus()
        .map(|focus| &focus.context)
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
        .versions
        .focus()
        .map(|focus| &focus.context)
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    let (_, record, mut reported) = focus_record(state)?;
    let segments = line_segments(&caret.line, grid, &mut reported);
    formatting_back(state, (node, record), reported);
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

/// What [`navigator_text`] does first: its words, when nothing is to be
/// asked, or the read of the selected text to ask.
pub(crate) fn navigator_text_start(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
) -> Result<Vec<Effect>, TextAsk> {
    if !may_have_text(node.role) {
        return Ok(Vec::new());
    }
    let (start, end) = match state.focus_caret(node.id) {
        Some(caret) => match &caret.selection {
            Some(selection) => (
                TextPoint::At(selection.range.start),
                TextPoint::At(selection.range.end),
            ),
            None if read == NavigatorRead::Announce => {
                return Ok(caret_line(state, node.id)
                    .map(|segments| vec![speak_about(trace_id, node, segments)])
                    .unwrap_or_default());
            }
            None => {
                let line = text::line_content(&caret.line.text, is_grid(node.role)).to_owned();
                return Ok(named_text(trace_id, node, read, &line));
            }
        },
        None => (TextPoint::SelectionStart, TextPoint::SelectionEnd),
    };
    Err(TextAsk::Range(Span {
        start,
        end,
        through: None,
    }))
}

/// The read of the navigator object's line at its caret, or its first
/// line when it reports no caret.
pub(crate) fn navigator_line_read() -> TextAsk {
    TextAsk::Read {
        at: TextPoint::Caret,
        movement: None,
        unit: TextUnit::Line,
        count: 1,
    }
}

/// What an answer to a navigator text read gives.
pub(crate) enum NavigatorText {
    /// These words, and the read ends.
    Said(Vec<Effect>),
    /// The line is to be read next ([`navigator_line_read`]).
    ReadLine,
}

/// The answer `answer` to the read of `object`'s selected text, when
/// `selection`, else of its line, for `read`.
pub(crate) fn navigator_text_answered(
    trace_id: TraceId,
    object: &NodeSnapshot,
    read: NavigatorRead,
    selection: bool,
    answer: TextAnswer,
) -> NavigatorText {
    let segments = match (selection, answer) {
        (true, TextAnswer::Range(text)) => {
            if verbatim_text::graphemes(&text).is_empty() {
                return NavigatorText::ReadLine;
            }
            if read != NavigatorRead::Announce {
                return NavigatorText::Said(named_text(trace_id, object, read, &text));
            }
            selected_segments(text).unwrap_or_default()
        }
        (true, TextAnswer::Refused(StepRefusal::Unsupported)) => return NavigatorText::ReadLine,
        (false, TextAnswer::Read { chunks, .. }) => {
            let Some(chunk) = chunks.into_iter().next() else {
                return NavigatorText::Said(Vec::new());
            };
            let grid = is_grid(object.role);
            if read != NavigatorRead::Announce {
                return NavigatorText::Said(named_text(
                    trace_id,
                    object,
                    read,
                    text::line_content(&chunk.text, grid),
                ));
            }
            line_segments(&chunk, grid, &mut TextAttributes::default())
        }
        (_, TextAnswer::Refused(StepRefusal::NoText | StepRefusal::Unsupported)) => {
            if read != NavigatorRead::Announce {
                return NavigatorText::Said(named_text(
                    trace_id,
                    object,
                    read,
                    object.value.as_deref().unwrap_or_default(),
                ));
            }
            match object.value.as_ref().filter(|value| !value.is_empty()) {
                Some(value) => vec![UtteranceSegment::value(value.clone())],
                None => return NavigatorText::Said(Vec::new()),
            }
        }
        _ => return NavigatorText::Said(Vec::new()),
    };
    NavigatorText::Said(vec![speak_about(trace_id, object, segments)])
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

/// Queued speech about `node`, valid however the focus moves, as the rest
/// of a navigator announcement is.
pub(crate) fn speak_about(
    trace_id: TraceId,
    node: &NodeSnapshot,
    segments: Vec<UtteranceSegment>,
) -> Effect {
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
        .versions
        .live()
        .map(|focus| &focus.context)
        .filter(|focus| focus.snapshot.id == node)
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
        .versions
        .live()
        .map(|focus| &focus.context)
        .is_some_and(|focus| focus.snapshot.role == Role::Terminal);
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
    // moves: the typing's key press has marked it (`crate::keys::caret_key`).
    let Some(focus) = state.versions.live().map(|focus| &focus.context) else {
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

/// Handles a control character typed into the focused application, one
/// other than a tab, a carriage return, Backspace's and
/// Control+Backspace's (Control+C, Control+Enter; never Escape): the word being
/// typed ends, spoken when word echo applies, and nothing is spelled, as
/// NVDA's typed-character rule takes any such character
/// (`docs/parity/input.md`, "Typed-character echo"). In a terminal, whose
/// typing is echoed once it shows, only while its passwords are spoken, as
/// [`character_typed`] echoes there.
pub(crate) fn control_typed(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    let Some(focus) = state.versions.live().map(|focus| &focus.context) else {
        return effects;
    };
    if focus.snapshot.role == Role::Terminal && !state.settings.speak_terminal_passwords {
        return effects;
    }
    let echo_words = echoes(state).is_some_and(|(_, words)| words);
    effects.extend(end_word(state, trace_id, echo_words));
    effects
}

/// Whether the focus echoes typed characters and typed words, by the
/// settings; `None` with no live focus.
fn echoes(state: &SrState) -> Option<(bool, bool)> {
    let focus = state.versions.live().map(|focus| &focus.context)?;
    let editable = is_editable(&focus.snapshot);
    let protected = focus.snapshot.states.contains(State::Protected);
    let applies = |mode: TypingEcho| match mode {
        TypingEcho::Off => false,
        TypingEcho::EditControls => editable,
        TypingEcho::Always => true,
    };
    Some((
        applies(state.settings.speak_typed_characters),
        applies(state.settings.speak_typed_words) && !protected,
    ))
}

/// Ends the word being typed: it is forgotten, and spoken when
/// `echo_words` and there is one.
fn end_word(state: &mut SrState, trace_id: TraceId, echo_words: bool) -> Option<Effect> {
    let word = std::mem::take(&mut state.typed_word);
    (echo_words && !word.is_empty()).then(|| speak(trace_id, vec![UtteranceSegment::text(word)]))
}

/// Echoes typed text by the settings: a finished word first, when word echo
/// is on and the text ends one, then the characters.
pub(crate) fn echo(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let Some((echo_characters, echo_words)) = echoes(state) else {
        return Vec::new();
    };
    let protected = state
        .versions
        .live()
        .map(|focus| &focus.context)
        .is_some_and(|focus| focus.snapshot.states.contains(State::Protected));
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
            effects.extend(end_word(state, trace_id, echo_words));
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
    use verbatim_model::{Backend, NodeDetails, StateSet};

    use super::*;

    /// A node of `role`, read-only or not.
    fn node(role: Role, read_only: bool) -> NodeSnapshot {
        let states = if read_only {
            StateSet::new().with(State::ReadOnly)
        } else {
            StateSet::new()
        };
        NodeSnapshot {
            id: NodeId::new(1),
            backend: Backend::Msaa,
            role,
            name: None,
            value: None,
            states,
            details: NodeDetails::default(),
        }
    }

    /// An edit field, a document or a terminal is a place text is typed
    /// unless it is read-only, as NVDA's test excludes a read-only focus of
    /// every role; nothing else is, read-only or not.
    #[test]
    fn only_a_writable_field_document_or_terminal_is_editable() {
        for role in [Role::EditableText, Role::Document, Role::Terminal] {
            assert!(is_editable(&node(role, false)), "{role:?}");
            assert!(!is_editable(&node(role, true)), "{role:?} read-only");
        }
        for role in [Role::Button, Role::StaticText, Role::ComboBox] {
            assert!(!is_editable(&node(role, false)), "{role:?}");
            assert!(!is_editable(&node(role, true)), "{role:?} read-only");
        }
    }
}
