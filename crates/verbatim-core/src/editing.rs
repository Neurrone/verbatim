//! Editing (milestone M4): caret keys and the selection, following NVDA's
//! wait for evidence (`docs/nvda/editable-text-and-terminals.md`), and
//! typed character and word echo with the password rules
//! (`phase6-design.md`, M4 items 3 and 4).
//!
//! A caret key reaches the application unchanged; the hook reports it, and
//! the reducer asks the focus's outpost to wait for evidence of what the
//! key did and report the caret ([`TextOp::AwaitCaret`]). The reply is
//! spoken: the character after Left or Right Arrow, Home, and End; the
//! provider's word after Control with Left or Right Arrow; the line after
//! Up or Down Arrow, the page keys, and Control with Home or End; the
//! paragraph after Control with Up or Down Arrow; what Backspace deleted;
//! the character or word now at the caret after Delete. A key with Shift
//! speaks what became selected or unselected instead. A newer key, or a
//! focus change, supersedes a key still waiting, so speech never lags
//! behind fast typing and a focus announcement wins over a caret line.

use std::sync::Arc;

use verbatim_model::{
    CaretKey, CaretMotion, CaretReply, CaretReport, CaretWait, CaretWatch, Effect, NodeId,
    NodeSnapshot, Phrase, PreviousSelection, Role, SegmentContent, SelectionChange, SelectionText,
    SpeechPriority, State, TextOp, TextReply, TextRequest, TextUnit, TraceId, TypingEcho,
    Utterance, UtteranceSegment,
};

use crate::state::{CaretContext, PendingCaret, ReviewPosition, ReviewText, SrState};
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
        validity: None,
    })
}

/// Handles a caret key passed to the application: when the focus has text,
/// or may have, ask its outpost to wait for evidence and report the caret.
pub(crate) fn caret_key(state: &mut SrState, key: CaretKey) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    state.typed_word.clear();
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    let node = focus.snapshot.id;
    let role = focus.snapshot.role;
    let context = state.caret.as_ref().filter(|caret| caret.node == node);
    if context.is_none() && !may_have_text(role) {
        return effects;
    }
    let grid = is_grid(role);
    let unit = key.motion.unit();
    let deleted = context.and_then(|caret| deleted_text(caret, key.motion, grid));
    let compare = context.and_then(|caret| compared_text(caret, key.motion, grid));
    let previous_selection = (key.select || key.motion == CaretMotion::SelectAll)
        .then(|| {
            context.map(|caret| match caret.selection {
                Some(selection) => PreviousSelection {
                    start: selection.start,
                    end: selection.end,
                },
                None => PreviousSelection {
                    start: caret.caret(),
                    end: caret.caret(),
                },
            })
        })
        .flatten();
    let watch = CaretWatch {
        since: context.map(CaretContext::caret),
        unit,
        compare,
        previous_selection,
        wait: if grid {
            CaretWait::Extended
        } else {
            CaretWait::Standard
        },
    };
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

/// What a Backspace is about to delete, from the caret before the key: the
/// character before the caret, or for Control+Backspace the text from the
/// start of the word before the caret up to the caret. `None` at the
/// line's start, where what goes is the line break, and for other keys.
fn deleted_text(caret: &CaretContext, motion: CaretMotion, grid: bool) -> Option<String> {
    let content = text::line_content(&caret.line.text, grid);
    let offset = text::boundary(content, caret.line.offset as usize);
    match motion {
        CaretMotion::Backspace => {
            text::previous_grapheme(content, offset).map(|range| content[range].to_owned())
        }
        CaretMotion::BackspaceWord => {
            let words = text::words(content, caret.line.language_at(0));
            let start = words.iter().rev().find(|range| range.start < offset)?.start;
            Some(content[start..offset].to_owned())
        }
        _ => None,
    }
}

/// The text at the caret before a Delete, whose change is evidence the key
/// did something even though the caret stays where it is.
fn compared_text(caret: &CaretContext, motion: CaretMotion, grid: bool) -> Option<String> {
    let content = text::line_content(&caret.line.text, grid);
    let offset = caret.line.offset as usize;
    match motion {
        CaretMotion::Delete => Some(
            text::grapheme_at(content, offset)
                .map_or(String::new(), |range| content[range].to_owned()),
        ),
        CaretMotion::DeleteWord => {
            let words = text::words(content, caret.line.language_at(0));
            Some(
                text::word_at(&words, offset)
                    .map_or(String::new(), |range| content[range].to_owned()),
            )
        }
        _ => None,
    }
}

/// Handles the outpost's answer to a caret key's wait: keeps the caret
/// current, moves the review cursor with it, and speaks what the key did.
pub(crate) fn caret_reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingCaret,
    reply: TextReply,
) -> Vec<Effect> {
    let TextReply::Caret(reply) = reply else {
        return Vec::new();
    };
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
            motion => unit_segments(&caret, unit.as_ref(), motion.unit(), grid),
        }
    };
    update_caret(state, pending.node, caret);
    if segments.is_empty() {
        Vec::new()
    } else {
        vec![speak(trace_id, segments)]
    }
}

/// The speech for text a Backspace deleted.
fn deleted_segments(deleted: &str, motion: CaretMotion) -> Vec<UtteranceSegment> {
    if motion == CaretMotion::Backspace {
        text::character_segments(Some(deleted), None)
    } else {
        text::text_segments(deleted.trim(), None)
    }
}

/// The speech for the unit at the caret after a caret key: the character,
/// the provider's word or paragraph when it sent one, and the line
/// otherwise.
fn unit_segments(
    caret: &CaretReport,
    unit_chunk: Option<&verbatim_model::TextChunk>,
    unit: TextUnit,
    grid: bool,
) -> Vec<UtteranceSegment> {
    let line = &caret.line;
    let content = text::line_content(&line.text, grid);
    let offset = line.offset as usize;
    match (unit, unit_chunk) {
        (TextUnit::Character, _) => {
            let character = text::grapheme_at(content, offset).map(|range| &content[range]);
            text::character_segments(character, line.language_at(offset))
        }
        (TextUnit::Line, _) | (_, None) => text::text_segments(content, line.language_at(0)),
        (_, Some(chunk)) => text::text_segments(
            text::chunk_content(chunk, grid).trim(),
            chunk.language_at(0),
        ),
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
/// navigator is on that node.
pub(crate) fn update_caret(state: &mut SrState, node: NodeId, caret: CaretReport) {
    if !state.focus_matches(node) {
        return;
    }
    let line = Arc::new(caret.line);
    if state.settings.follow_caret
        && let Some(navigator) = state.navigator.as_mut()
        && navigator.object.id == node
    {
        let grid = is_grid(navigator.object.role);
        let content = text::line_content(&line.text, grid);
        let offset = text::boundary(content, line.offset as usize);
        navigator.text = ReviewText::At(ReviewPosition {
            line: Arc::clone(&line),
            offset,
            column: text::column_of(content, offset, grid),
        });
    }
    state.caret = Some(CaretContext {
        node,
        line,
        selection: caret.selection,
    });
}

/// Handles text typed into the focused application: echoes characters and
/// words by the settings, a protected field's characters as the protected
/// character only, and holds what is typed into a terminal until the
/// terminal shows it, unless the user asked for terminal passwords to be
/// spoken.
pub(crate) fn character_typed(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return effects;
    };
    if focus.snapshot.role == Role::Terminal && !state.settings.speak_terminal_passwords {
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
/// echoed now that it has shown something.
pub(crate) fn text_changed(state: &mut SrState, trace_id: TraceId, node: NodeId) -> Vec<Effect> {
    if !state.focus_matches(node) || state.held_typing.is_empty() {
        return Vec::new();
    }
    let held = std::mem::take(&mut state.held_typing);
    echo(state, trace_id, &held)
}

/// Echoes typed text by the settings: a finished word first, when word echo
/// is on and the text ends one, then the characters.
fn echo(state: &mut SrState, trace_id: TraceId, typed: &str) -> Vec<Effect> {
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
        let word_character = character.chars().all(char::is_alphanumeric);
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
        let printable = !character.chars().any(char::is_control) || character == "\t";
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
