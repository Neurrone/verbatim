//! Editing (milestone M4): caret keys and the selection, following NVDA's
//! wait for evidence (`docs/nvda/editable-text-and-terminals.md`), and
//! typed character and word echo with the password rules
//! (`phase6-design.md`, M4 items 3 and 4).
//!
//! A caret key reaches the application unchanged; the hook reports it, and
//! the reducer asks the focus's outpost to watch for evidence of what the
//! key did and report the caret ([`TextOp::AwaitCaret`]); the outpost never
//! waits for it, and a key that moves nothing gets no answer worth
//! speaking ([`TextReply::WatchEnded`]), so it is silent. The reply is
//! spoken: the character after Left or Right Arrow, Home, and End; the
//! provider's word after Control with Left or Right Arrow; the line after
//! Up or Down Arrow, the page keys, and Control with Home or End; the
//! paragraph after Control with Up or Down Arrow; what Backspace deleted;
//! the character or word now at the caret after Delete. A key with Shift
//! speaks what became selected or unselected instead. A newer key, a typed
//! character, or a focus change supersedes a key still watched, so speech
//! never lags behind fast typing, a later edit is not spoken as an earlier
//! key's answer, and a focus announcement wins over a caret line.
//!
//! Formatting (milestone M4 item 7) is spoken as it changes, as NVDA
//! speaks it (`docs/nvda/document-formatting.md`): the outpost sends the
//! formatting of the text a caret key or a focus speaks, and only what
//! differs from the formatting last reported in that node is said, a
//! spelling error where it starts, and for a character or a word also where
//! it ends.

use std::sync::Arc;

use verbatim_model::{
    CaretKey, CaretMotion, CaretReply, CaretReport, CaretWatch, Effect, FocusValidity, NodeId,
    NodeSnapshot, Phrase, PreviousSelection, Role, SegmentContent, Selection, SelectionChange,
    SelectionText, SpeechPriority, State, TextAttributes, TextChunk, TextOp, TextPoint,
    TextPosition, TextRead, TextReply, TextRequest, TextUnit, TraceId, TypingEcho, Utterance,
    UtteranceSegment,
};

use crate::state::{
    CARET_HISTORY, CaretContext, FocusText, NavigatorRead, PendingCaret, PendingText, ReviewText,
    SrState, TextFollowUp, TimedCaret,
};
use crate::text;

/// The most bytes of a word typed so far that word echo keeps; a longer
/// run of letters is not a word anyone needs echoed whole.
pub(crate) const MAX_TYPED_WORD: usize = 256;

/// The most bytes of typing held for a terminal to show.
pub(crate) const MAX_HELD_TYPING: usize = 1024;

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

/// Queued speech with no source node.
pub(crate) fn speak(trace_id: TraceId, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: None,
        say_all: false,
        validity: None,
    })
}

/// Handles a caret key passed to the application: when the focus has text,
/// or may have, ask its outpost to wait for evidence and report the caret.
/// `pressed_at_ms` is when the hook saw the key, on the outposts' clock.
pub(crate) fn caret_key(state: &mut SrState, key: CaretKey, pressed_at_ms: u64) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    state.typed_word.clear();
    // The key's own report says where the caret went; the focus's text at
    // the caret before it is no longer worth hearing.
    state.focus_text = None;
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    let node = focus.snapshot.id;
    let role = focus.snapshot.role;
    let current = state.caret.as_ref().filter(|caret| caret.node == node);
    if current.is_none() && !may_have_text(role) {
        return effects;
    }
    let grid = is_grid(role);
    let unit = key.motion.unit();
    // What the key deletes, the text whose change is evidence, where the
    // caret starts, and the selection the key may change all come from the
    // caret as it was when the key was pressed, which may be older than the
    // caret Core now has.
    let line_break = current.and_then(|caret| caret.line_break.as_deref());
    let context = caret_before(state, node, pressed_at_ms);
    let deleted = context.and_then(|caret| deleted_text(caret.line, line_break, key.motion, grid));
    let compare = context.and_then(|caret| compared_text(caret.line, key.motion, grid));
    let previous_selection = context.and_then(|caret| {
        if key.select || key.motion == CaretMotion::SelectAll {
            Some(match caret.selection {
                Some(selection) => PreviousSelection {
                    start: selection.start,
                    end: selection.end,
                },
                None => PreviousSelection {
                    start: caret.caret(),
                    end: caret.caret(),
                },
            })
        } else {
            // A movement that leaves a selection unselects it, spoken after
            // the unit, as NVDA reports any selection change of a control
            // that reports its selection (`docs/nvda/editable-text-and-terminals.md`,
            // "Selection changes"). A deletion replaces the selected text.
            caret
                .selection
                .filter(|_| !key.motion.deletes())
                .map(|selection| PreviousSelection {
                    start: selection.start,
                    end: selection.end,
                })
        }
    });
    let watch = CaretWatch {
        since: context.map(CaretBefore::caret),
        pressed_at_ms,
        unit,
        compare,
        previous_selection,
    };
    // The key is moving the caret, which the review cursor follows: a
    // review command from now on reads where the caret is, not where Core
    // last saw it, however late the application reports the move.
    follow_caret(state, node);
    let query_id = state.allocate_query_id();
    state.pending_caret = Some(PendingCaret {
        query_id,
        node,
        key,
        deleted,
    });
    effects.push(Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op: TextOp::AwaitCaret(watch),
    }));
    effects
}

/// The caret of a node as a caret key found it: its line, with the caret
/// at the chunk's offset, and the selection.
#[derive(Clone, Copy)]
struct CaretBefore<'a> {
    line: &'a TextChunk,
    selection: Option<Selection>,
}

impl CaretBefore<'_> {
    /// Where the caret was.
    fn caret(self) -> TextPosition {
        TextPosition {
            anchor: self.line.start,
            offset: self.line.offset,
        }
    }
}

/// The caret of `node` as a key pressed at `pressed_at_ms` found it: the
/// newest of Core's timed caret reports observed before the key was
/// pressed, as the outpost picks its own baseline (`docs/parity.md`, "Text,
/// documents, terminals", caret-key reporting). A report observed in the
/// same millisecond as the key, or later, may already show what the key
/// did, so it is never the caret before the key. With no time for the key, or no timed report,
/// the caret Core has now stands in; with timed reports but none from
/// before the key, the caret before the key is not known.
fn caret_before(state: &SrState, node: NodeId, pressed_at_ms: u64) -> Option<CaretBefore<'_>> {
    let mut timed = state
        .caret_history
        .iter()
        .flatten()
        .filter(|report| report.node == node)
        .peekable();
    if pressed_at_ms == 0 || timed.peek().is_none() {
        return state
            .caret
            .as_ref()
            .filter(|caret| caret.node == node)
            .map(|caret| CaretBefore {
                line: &caret.line,
                selection: caret.selection,
            });
    }
    // Reports arrive in the order their paths deliver them, not the order
    // they were observed, so the newest is the latest observed; of two
    // observed in the same millisecond, the later to arrive.
    timed
        .filter(|report| report.observed_at_ms < pressed_at_ms)
        .max_by_key(|report| report.observed_at_ms)
        .map(|report| CaretBefore {
            line: &report.line,
            selection: report.selection,
        })
}

/// What a Backspace is about to delete, from the caret's line before the
/// key: the character before the caret, or for Control+Backspace the text
/// from the start of the word before the caret up to the caret. At the
/// start of a line other than a terminal's, a Backspace deletes the line
/// break before it, the kind of break the text uses, `line_break`
/// ([`CaretContext::line_break`]), which NVDA names
/// (`docs/nvda/editable-text-and-terminals.md`, "A line break as a
/// character"). `None` when that is not known, and for other keys.
fn deleted_text(
    line: &TextChunk,
    line_break: Option<&str>,
    motion: CaretMotion,
    grid: bool,
) -> Option<String> {
    let content = text::line_content(&line.text, grid);
    let offset = text::boundary(content, line.offset as usize);
    match motion {
        CaretMotion::Backspace => match text::previous_grapheme(content, offset) {
            Some(range) => Some(content[range].to_owned()),
            None if !grid && !line.first => line_break.map(str::to_owned),
            None => None,
        },
        CaretMotion::BackspaceWord => {
            let words = text::words(content, line.language_at(0));
            let start = words.iter().rev().find(|range| range.start < offset)?.start;
            Some(content[start..offset].to_owned())
        }
        _ => None,
    }
}

/// The text at the caret before a Delete, whose change is evidence the key
/// did something even though the caret stays where it is, from the caret's
/// line before the key.
fn compared_text(line: &TextChunk, motion: CaretMotion, grid: bool) -> Option<String> {
    let content = text::line_content(&line.text, grid);
    let offset = line.offset as usize;
    match motion {
        CaretMotion::Delete => Some(
            text::grapheme_at(content, offset)
                .map_or(String::new(), |range| content[range].to_owned()),
        ),
        CaretMotion::DeleteWord => {
            let words = text::words(content, line.language_at(0));
            Some(
                text::word_at(&words, offset)
                    .map_or(String::new(), |range| content[range].to_owned()),
            )
        }
        _ => None,
    }
}

/// Handles the outpost's answer to a caret key's watch: keeps the caret
/// current, moves the review cursor with it, and speaks what the key did.
pub(crate) fn caret_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingCaret,
    reply: TextReply,
) -> Vec<Effect> {
    // A watch that ended without evidence (`TextReply::WatchEnded`), or a
    // reply without it, says nothing: a key that did not move the caret is
    // silent (`docs/parity.md`, "Text, documents, terminals").
    let TextReply::Caret(reply) = reply else {
        return Vec::new();
    };
    if !reply.moved {
        return Vec::new();
    }
    let Some(focus) = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.id == pending.node)
    else {
        return Vec::new();
    };
    let grid = is_grid(focus.snapshot.role);
    let CaretReply {
        moved,
        caret,
        read_at_ms,
        unit,
        selection_changes,
    } = *reply;
    let segments = if pending.key.select || pending.key.motion == CaretMotion::SelectAll {
        selection_segments(&selection_changes)
    } else {
        match pending.key.motion {
            CaretMotion::Backspace | CaretMotion::BackspaceWord => match &pending.deleted {
                Some(deleted) if moved => deleted_segments(deleted, pending.key.motion),
                _ => Vec::new(),
            },
            motion => {
                let mut reported = reported_format(state, pending.node);
                let segments =
                    unit_segments(&caret, unit.as_ref(), (motion.unit(), grid), &mut reported);
                state.reported_format = Some((pending.node, reported));
                segments
            }
        }
    };
    update_caret(state, pending.node, caret, read_at_ms);
    let mut effects = Vec::new();
    if !segments.is_empty() {
        effects.push(speak(trace_id, segments));
    }
    // A movement that unselected text says so after the unit.
    let unselected = if pending.key.select || pending.key.motion == CaretMotion::SelectAll {
        Vec::new()
    } else {
        selection_segments(&selection_changes)
    };
    if !unselected.is_empty() {
        effects.push(speak(trace_id, unselected));
    }
    effects
}

/// The speech for text a Backspace deleted. A carriage return and line feed
/// deleted together are spoken as the line feed, as NVDA speaks them.
fn deleted_segments(deleted: &str, motion: CaretMotion) -> Vec<UtteranceSegment> {
    if motion == CaretMotion::Backspace {
        let deleted = if deleted == "\r\n" { "\n" } else { deleted };
        text::character_segments(Some(deleted), None)
    } else {
        text::text_segments(deleted.trim(), None)
    }
}

/// The formatting last reported in `node`'s text, NVDA's per-object cache:
/// nothing yet for a node other than the one last spoken in.
fn reported_format(state: &SrState, node: NodeId) -> TextAttributes {
    state
        .reported_format
        .as_ref()
        .filter(|(reported, _)| *reported == node)
        .map(|(_, attributes)| attributes.clone())
        .unwrap_or_default()
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
            let start = content.len() - content.trim_start().len();
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
    caret: &CaretReport,
    unit_chunk: Option<&TextChunk>,
    (unit, grid): (TextUnit, bool),
    reported: &mut TextAttributes,
) -> Vec<UtteranceSegment> {
    let line = &caret.line;
    let content = text::line_content(&line.text, grid);
    let offset = line.offset as usize;
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

/// The speech for a selection change: each change as NVDA's "selected" or
/// "unselected", a single character by its name, 512 characters or more as
/// a count.
fn selection_segments(changes: &[SelectionChange]) -> Vec<UtteranceSegment> {
    changes
        .iter()
        .filter(|change| change.characters > 0)
        .map(|change| {
            let text = if change.characters >= SELECTION_SPOKEN_AS_COUNT {
                SelectionText::Characters(change.characters)
            } else if change.characters == 1 {
                SelectionText::Character(change.text.clone())
            } else {
                SelectionText::Text(change.text.clone())
            };
            let phrase = if change.selected {
                Phrase::Selected(text)
            } else {
                Phrase::Unselected(text)
            };
            UtteranceSegment::new(SegmentContent::Phrase(phrase))
        })
        .collect()
}

/// Takes a caret report for `node` as the focus's caret, and moves the
/// review cursor to it when the review cursor follows the caret and the
/// navigator is on that node (to read the caret afresh, as the review
/// cursor does at [`ReviewText::Caret`]). `observed_at_ms` is when the report was
/// observed or read, 0 when unknown; a timed report is also kept for a
/// later caret key to find the caret as it was when that key was pressed
/// ([`caret_before`]). A timed report observed before the caret Core holds,
/// such as a caret key's reply read before a caret event that reached Core
/// first, is only kept: the caret stays the newest known, and the review
/// cursor stays with it. An untimed report is taken as it arrives.
pub(crate) fn update_caret(
    state: &mut SrState,
    node: NodeId,
    caret: CaretReport,
    observed_at_ms: u64,
) {
    if !state.focus_matches(node) {
        return;
    }
    let line = Arc::new(caret.line);
    if observed_at_ms == 0 || state.caret.as_ref().is_some_and(|known| known.node != node) {
        state.caret_history = Default::default();
    }
    if observed_at_ms != 0 {
        state.caret_history.rotate_left(1);
        state.caret_history[CARET_HISTORY - 1] = Some(TimedCaret {
            observed_at_ms,
            node,
            line: Arc::clone(&line),
            selection: caret.selection,
        });
    }
    if let Some(known) = state.caret.as_mut().filter(|known| {
        known.node == node && observed_at_ms != 0 && known.observed_at_ms > observed_at_ms
    }) {
        // The older report still shows the kind of line break the text uses.
        if known.line_break.is_none() {
            known.line_break = text::line_break(&line.text).map(str::to_owned);
        }
        return;
    }
    follow_caret(state, node);
    let line_break = text::line_break(&line.text).map(str::to_owned).or_else(|| {
        state
            .caret
            .take()
            .filter(|caret| caret.node == node)
            .and_then(|caret| caret.line_break)
    });
    state.caret = Some(CaretContext {
        node,
        line,
        selection: caret.selection,
        line_break,
        observed_at_ms,
    });
}

/// Puts the review cursor at the caret of `node` when it follows the caret
/// and the navigator is on that node: the next review command reads the
/// line at the caret as the outpost finds it ([`ReviewText::Caret`]).
fn follow_caret(state: &mut SrState, node: NodeId) {
    if state.settings.follow_caret
        && let Some(navigator) = state.navigator.as_mut()
        && navigator.object.id == node
    {
        navigator.text = ReviewText::Caret;
    }
}

/// Sets the state to wait for a new focus's text at the caret, which it
/// says in place of its value (`docs/nvda/speech.md`, "What an object with
/// text says"): an object that may have text does, once the outpost's first
/// caret report tells Core what that text is. Its name and role are not held
/// back for it. A protected field leaves its value out and never has its
/// text read.
pub(crate) fn await_focus_text(state: &mut SrState, node: &NodeSnapshot) {
    // A new focus is a new object, whose formatting has not been reported.
    state.reported_format = None;
    if may_have_text(node.role) && !node.states.contains(State::Protected) {
        state.focus_text = Some(FocusText {
            node: node.id,
            selection_query: None,
        });
    }
}

/// Ends the announcement of a focus with text once its first caret report
/// has arrived (`docs/nvda/speech.md`, "What an object with text says"):
/// the caret's line, or, when text is selected, asks the outpost for the
/// selected text, which [`focus_selection`] speaks.
pub(crate) fn focus_caret(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    let Some(pending) = state
        .focus_text
        .filter(|pending| pending.node == node && pending.selection_query.is_none())
    else {
        return Vec::new();
    };
    let Some(caret) = state.caret.as_ref().filter(|caret| caret.node == node) else {
        return Vec::new();
    };
    if let Some(selection) = caret.selection {
        let query_id = state.allocate_query_id();
        state.focus_text = Some(FocusText {
            selection_query: Some(query_id),
            ..pending
        });
        return vec![Effect::Text(TextRequest {
            query_id,
            node_id: node,
            op: TextOp::ReadRange {
                start: TextPoint::At(selection.start),
                end: TextPoint::At(selection.end),
            },
        })];
    }
    state.focus_text = None;
    focus_line(state, trace_id, node)
}

/// Speaks the selected text a focus had when it gained the focus, read for
/// [`focus_caret`]: "selected" and the text, or its number of characters
/// when there are 512 or more. With no selected text after all, or no
/// answer, the caret's line is spoken instead.
pub(crate) fn focus_selection(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    reply: TextReply,
) -> Vec<Effect> {
    let text = match reply {
        TextReply::Range { text, .. } => text,
        TextReply::Gone => return Vec::new(),
        _ => String::new(),
    };
    match selected_segments(text) {
        Some(segments) => focus_speech(state, trace_id, node, segments),
        None => focus_line(state, trace_id, node),
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
pub(crate) fn focus_value(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    if state.focus_text.is_none_or(|pending| pending.node != node) {
        return Vec::new();
    }
    state.focus_text = None;
    let Some(value) = state
        .focus
        .as_ref()
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
    let caret = state.caret.as_ref().filter(|caret| caret.node == node)?;
    let grid = state
        .focus
        .as_ref()
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    let mut reported = reported_format(state, node);
    let segments = line_segments(&caret.line, grid, &mut reported);
    state.reported_format = Some((node, reported));
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

/// Whether the navigator object `node` reads its text in place of its value
/// (`docs/nvda/speech.md`, "What an object with text says"): an object that
/// may have text and is not a protected field, whose text is never read.
pub(crate) fn reads_text(node: &NodeSnapshot) -> bool {
    may_have_text(node.role) && !node.states.contains(State::Protected)
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
/// never read, as on focus.
pub(crate) fn navigator_text(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
    read: NavigatorRead,
) -> Vec<Effect> {
    if !reads_text(node) {
        return Vec::new();
    }
    let (start, end) = match state.caret.as_ref().filter(|caret| caret.node == node.id) {
        Some(caret) => match caret.selection {
            Some(selection) => (TextPoint::At(selection.start), TextPoint::At(selection.end)),
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
        node.id,
        TextOp::ReadRange { start, end },
        TextFollowUp::NavigatorSelection(read),
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
/// announcement to go on with `then`. A newer text request supersedes it.
fn navigator_request(
    state: &mut SrState,
    node: NodeId,
    op: TextOp,
    then: TextFollowUp,
) -> Vec<Effect> {
    let query_id = state.allocate_query_id();
    state.pending_text = Some(PendingText {
        query_id,
        node,
        then,
    });
    vec![Effect::Text(TextRequest {
        query_id,
        node_id: node,
        op,
    })]
}

/// Ends what [`navigator_text`] started with the answer it asked for: the
/// selected text when there is some, else, after asking for it, the caret's
/// line; the value when the object turns out to have no text to read, and
/// for spelling or copying the name and value, as for any object. Nothing
/// is done once the object asked about is neither the navigator object nor
/// the focus (reporting the focus reads the focus's text the same way).
pub(crate) fn navigator_text_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingText,
    reply: TextReply,
) -> Vec<Effect> {
    let navigator = state.navigator.as_ref().map(|navigator| &navigator.object);
    let focus = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive)
        .map(|focus| &focus.snapshot);
    let Some(object) = navigator
        .into_iter()
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
                return navigator_line(state, pending.node, read);
            }
            if read != NavigatorRead::Announce {
                return named_text(trace_id, &object, read, &text);
            }
            selected_segments(text).unwrap_or_default()
        }
        (TextFollowUp::NavigatorSelection(_), TextReply::Unsupported) => {
            return navigator_line(state, pending.node, read);
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
fn navigator_line(state: &mut SrState, node: NodeId, read: NavigatorRead) -> Vec<Effect> {
    navigator_request(
        state,
        node,
        TextOp::Read(TextRead {
            at: TextPoint::Caret,
            movement: None,
            unit: TextUnit::Line,
        }),
        TextFollowUp::NavigatorLine(read),
    )
}

/// Queued speech about `node`, valid however the focus moves, as the rest
/// of a navigator announcement is.
fn speak_about(trace_id: TraceId, node: &NodeSnapshot, segments: Vec<UtteranceSegment>) -> Effect {
    Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(crate::reduce::source_of(node)),
        say_all: false,
        validity: None,
    })
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
        .as_ref()
        .filter(|focus| focus.alive && focus.snapshot.id == node)
    else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(crate::reduce::source_of(&focus.snapshot)),
        validity: Some(FocusValidity {
            node,
            had_focus: true,
        }),
        say_all: false,
    })]
}

/// Handles text typed into the focused application: echoes characters and
/// words by the settings, a protected field's characters as the protected
/// character only, and holds what is typed into a terminal until the
/// terminal shows it, unless the user asked for terminal passwords to be
/// spoken.
pub(crate) fn character_typed(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    // The typing supersedes a caret key still watched: the caret the typing
    // moves is not that key's answer.
    state.pending_caret = None;
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    let terminal = focus.snapshot.role == Role::Terminal;
    if terminal && state.settings.speak_terminal_passwords {
        crate::terminal::typed(state, typed);
    } else if terminal {
        if typed.chars().any(|c| c == '\r' || c == '\n') {
            // Enter: whatever was held was never shown, a password perhaps.
            state.held_typing.clear();
            state.typed_word.clear();
        } else if state.held_typing.len() + typed.len() <= MAX_HELD_TYPING {
            state.held_typing.push_str(typed);
        }
        return effects;
    }
    effects.extend(echo(state, trace_id, typed));
    effects
}

/// Handles a change of a node's text: characters held for a terminal are
/// echoed now that it has shown something. An outpost that diffs a
/// terminal's text sends `TerminalOutput` instead, which echoes only what
/// the terminal shows (`crate::terminal`).
pub(crate) fn text_changed(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    if !state.focus_matches(node) || state.held_typing.is_empty() {
        return Vec::new();
    }
    let held = std::mem::take(&mut state.held_typing);
    echo(state, trace_id, &held)
}

/// Echoes typed text by the settings: a finished word first, when word echo
/// is on and the text ends one, then the characters.
pub(crate) fn echo(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
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
    for range in verbatim_text::graphemes(typed) {
        let character = &typed[range];
        // A letter, mark, or number continues the word, so a virama, a
        // Thai tone mark, or a zero-width non-joiner typed on its own does
        // not end it.
        let word_character = verbatim_text::is_word_grapheme(character);
        if word_character {
            if echo_words && state.typed_word.len() + character.len() <= MAX_TYPED_WORD {
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
