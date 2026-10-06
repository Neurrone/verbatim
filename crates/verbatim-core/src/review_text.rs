//! The review cursor over text read through the text protocol (milestone
//! M4; `phase6-design.md`, "The autonomous run", M4 item 5), with NVDA's
//! messages and repeated presses (`docs/nvda/review-modes.md`).
//!
//! The review cursor holds the line it is on, so moving by character or
//! word within the line, and reading the current unit, need no round
//! trip; moving to another line, page, or the document's ends reads the
//! line there. Columns follow the design's one intentional difference from
//! NVDA (`docs/parity.md`, "Review cursor columns"): moving to the next or
//! previous line keeps the column. In a terminal the column is a cell
//! column, so moving down from column 10 always lands on column 10, a
//! blank cell past the end of a shorter line; elsewhere it is counted in
//! characters, a shorter line puts the cursor on its last character, and
//! the column is remembered for the next longer line.
//!
//! The navigator object's review starts at its caret, or its start when it
//! has none, as NVDA's object review does. An object with no text interface
//! is reviewed as flat text (`review`), the M3 behavior.

use std::sync::Arc;

use verbatim_model::{
    Effect, Message, NodeId, Phrase, ReviewCommand, SegmentContent, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReply, TextRequest, TextUnit, TraceId, UtteranceSegment,
};

use crate::editing::{is_grid, may_have_text, speak};
use crate::state::{
    Landing, LandingPlace, PendingText, ReviewPosition, ReviewText, SharedChunk, SrState,
    StartMarker, TextFollowUp,
};
use crate::text;

/// Whether `command` is one this module runs: every review-cursor command
/// over text, the start marker, select then copy, and say-all from the
/// review cursor.
pub(crate) fn handles(command: ReviewCommand) -> bool {
    matches!(
        command,
        ReviewCommand::ReviewTop
            | ReviewCommand::ReviewPreviousLine
            | ReviewCommand::ReviewCurrentLine
            | ReviewCommand::ReviewNextLine
            | ReviewCommand::ReviewPreviousWord
            | ReviewCommand::ReviewCurrentWord
            | ReviewCommand::ReviewNextWord
            | ReviewCommand::ReviewStartOfLine
            | ReviewCommand::ReviewPreviousCharacter
            | ReviewCommand::ReviewCurrentCharacter
            | ReviewCommand::ReviewNextCharacter
            | ReviewCommand::ReviewEndOfLine
            | ReviewCommand::ReviewBottom
            | ReviewCommand::ReviewPreviousPage
            | ReviewCommand::ReviewNextPage
            | ReviewCommand::ReviewSelectionStart
            | ReviewCommand::ReviewSelectionEnd
            | ReviewCommand::SetStartMarker
            | ReviewCommand::MoveToStartMarker
            | ReviewCommand::SelectThenCopy
            | ReviewCommand::ReportReviewLocation
            | ReviewCommand::SayAllFromReview
    )
}

/// What [`run`] decided.
pub(crate) enum Outcome {
    /// The command ran over text, with these effects.
    Done(Vec<Effect>),
    /// The navigator object is reviewed as flat text: run the command there.
    Flat,
}

/// Runs a review command against the navigator's text, reading the line at
/// its caret first when the review cursor has no line yet.
pub(crate) fn run(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Outcome {
    let Some(navigator) = state.navigator.as_mut() else {
        return Outcome::Done(Vec::new());
    };
    let node = navigator.object.id;
    match navigator.text.clone() {
        ReviewText::At(position) => {
            Outcome::Done(execute(state, trace_id, node, command, repeat, &position))
        }
        ReviewText::Point(point) => {
            Outcome::Done(seed(state, node, TextPoint::At(point), command, repeat))
        }
        ReviewText::Flat => Outcome::Flat,
        ReviewText::Unknown => {
            let grid = is_grid(navigator.object.role);
            if let Some(caret) = state.caret.as_ref().filter(|caret| caret.node == node) {
                let position = position_at(&caret.line, caret.line.offset as usize, grid);
                navigator.text = ReviewText::At(position.clone());
                return Outcome::Done(execute(state, trace_id, node, command, repeat, &position));
            }
            if may_have_text(navigator.object.role) {
                Outcome::Done(seed(state, node, TextPoint::Caret, command, repeat))
            } else {
                navigator.text = ReviewText::Flat;
                Outcome::Flat
            }
        }
    }
}

/// A review position on `line` at byte `offset`, its column worked out.
fn position_at(line: &SharedChunk, offset: usize, grid: bool) -> ReviewPosition {
    let content = text::line_content(&line.text, grid);
    let offset = offset.min(content.len());
    ReviewPosition {
        line: Arc::clone(line),
        offset,
        column: text::column_of(content, offset, grid),
    }
}

/// Asks for a text read, to be followed up by `then`. A newer request
/// supersedes this one.
fn request(state: &mut SrState, node: NodeId, op: TextOp, then: TextFollowUp) -> Vec<Effect> {
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

/// Reads the line at `at` to place the review cursor there, then runs the
/// command.
fn seed(
    state: &mut SrState,
    node: NodeId,
    at: TextPoint,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<Effect> {
    request(
        state,
        node,
        TextOp::Read(TextRead {
            at,
            movement: None,
            unit: TextUnit::Line,
        }),
        TextFollowUp::Seed { command, repeat },
    )
}

/// The point the review cursor is at, as a position the outpost resolves.
fn point_of(position: &ReviewPosition) -> TextPosition {
    TextPosition {
        anchor: position.line.start,
        offset: u32::try_from(position.offset).unwrap_or(u32::MAX),
    }
}

/// Runs a command with the review cursor at `position`.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per review command, each short, reads best together"
)]
fn execute(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    command: ReviewCommand,
    repeat: u8,
    position: &ReviewPosition,
) -> Vec<Effect> {
    let grid = state
        .navigator
        .as_ref()
        .is_some_and(|navigator| is_grid(navigator.object.role));
    let line = &position.line;
    let content = text::line_content(&line.text, grid);
    let language = line.language_at(position.offset);
    let read_line = |movement: Option<TextMovement>, at: TextPoint, landing: Landing| {
        (
            TextOp::Read(TextRead {
                at,
                movement,
                unit: TextUnit::Line,
            }),
            TextFollowUp::Land(landing),
        )
    };
    let landing = |place, speak, edge| Landing {
        place,
        speak,
        edge,
        command,
    };
    let from_line = TextPoint::At(TextPosition::at(line.start));
    let by = |unit, count| Some(TextMovement { unit, count });

    let (op, then) = match command {
        ReviewCommand::ReviewCurrentLine => {
            let segments = match repeat {
                0 => text::text_segments(content, line.language_at(0)),
                1 => spell_or_blank(content, false, line.language_at(0)),
                _ => spell_or_blank(content, true, line.language_at(0)),
            };
            return vec![speak(trace_id, segments)];
        }
        ReviewCommand::ReviewCurrentWord => {
            let words = text::words(content, line.language_at(0));
            let word = text::word_at(&words, position.offset).map(|range| &content[range]);
            let segments = match (word, repeat) {
                (None, _) => text::text_segments("", None),
                (Some(word), 0) => text::text_segments(word, language),
                (Some(word), 1) => text::spelled(word, false, language),
                (Some(word), _) => text::spelled(word, true, language),
            };
            return vec![speak(trace_id, segments)];
        }
        ReviewCommand::ReviewCurrentCharacter => {
            let character = character_at(content, position.offset);
            let segments = match (character, repeat) {
                (None, _) | (Some(_), 0) => text::character_segments(character, language),
                (Some(character), 1) => vec![UtteranceSegment {
                    content: SegmentContent::CharacterDescription(character.to_owned()),
                    language: language.map(str::to_owned),
                }],
                (Some(character), _) => code_segments(character),
            };
            return vec![speak(trace_id, segments)];
        }
        ReviewCommand::ReviewStartOfLine => {
            return land_here(state, trace_id, position, 0, grid);
        }
        ReviewCommand::ReviewEndOfLine => {
            let last =
                text::previous_grapheme(content, content.len()).map_or(0, |range| range.start);
            return land_here(state, trace_id, position, last, grid);
        }
        ReviewCommand::ReviewPreviousCharacter => {
            let text_width = text::column_of(content, content.len(), true);
            if grid && position.offset >= content.len() && position.column > text_width {
                // Past the end of a terminal row's text: the blank cell to
                // the left.
                return move_column(state, trace_id, position, position.column - 1, grid);
            }
            return match text::previous_grapheme(content, position.offset) {
                Some(range) => land_here(state, trace_id, position, range.start, grid),
                None => edge(
                    trace_id,
                    Message::Left,
                    character_at(content, position.offset),
                    language,
                ),
            };
        }
        ReviewCommand::ReviewNextCharacter => {
            let character = character_at(content, position.offset);
            if grid {
                // Cells, up to the row's width, padding included.
                let next = position.column
                    + character.map_or(1, |character| verbatim_text::cell_width(character).max(1));
                if next
                    >= text::row_width(&line.text)
                        .max(text::column_of(content, content.len(), true) + 1)
                {
                    return edge(trace_id, Message::Right, character, language);
                }
                return move_column(state, trace_id, position, next, grid);
            }
            return match text::next_grapheme(content, position.offset) {
                Some(range) => land_here(state, trace_id, position, range.start, grid),
                None => edge(trace_id, Message::Right, character, language),
            };
        }
        ReviewCommand::ReviewPreviousWord | ReviewCommand::ReviewNextWord => {
            let forward = command == ReviewCommand::ReviewNextWord;
            let words = text::words(content, line.language_at(0));
            let current = text::word_at(&words, position.offset);
            let target = if forward {
                words.iter().find(|range| {
                    current
                        .as_ref()
                        .is_none_or(|current| range.start > current.start)
                })
            } else {
                words.iter().rev().find(|range| {
                    current
                        .as_ref()
                        .is_some_and(|current| range.start < current.start)
                })
            };
            if let Some(target) = target {
                let start = target.start;
                set_position(state, position_at(line, start, grid));
                return vec![speak(
                    trace_id,
                    text::text_segments(&content[target.clone()], language),
                )];
            }
            let at_edge = if forward { line.last } else { line.first };
            let message = if forward {
                Message::Bottom
            } else {
                Message::Top
            };
            if at_edge {
                let word = current.map(|range| &content[range]);
                return vec![speak(
                    trace_id,
                    with_edge(message, text::text_segments(word.unwrap_or(""), language)),
                )];
            }
            read_line(
                by(TextUnit::Line, if forward { 1 } else { -1 }),
                from_line,
                landing(
                    if forward {
                        LandingPlace::FirstWord
                    } else {
                        LandingPlace::LastWord
                    },
                    TextUnit::Word,
                    Some(message),
                ),
            )
        }
        ReviewCommand::ReviewPreviousLine | ReviewCommand::ReviewNextLine => {
            let forward = command == ReviewCommand::ReviewNextLine;
            let message = if forward {
                Message::Bottom
            } else {
                Message::Top
            };
            if (forward && line.last) || (!forward && line.first) {
                return vec![speak(
                    trace_id,
                    with_edge(message, text::text_segments(content, line.language_at(0))),
                )];
            }
            read_line(
                by(TextUnit::Line, if forward { 1 } else { -1 }),
                from_line,
                landing(
                    LandingPlace::Column(position.column),
                    TextUnit::Line,
                    Some(message),
                ),
            )
        }
        ReviewCommand::ReviewPreviousPage | ReviewCommand::ReviewNextPage => {
            let forward = command == ReviewCommand::ReviewNextPage;
            read_line(
                by(TextUnit::Page, if forward { 1 } else { -1 }),
                from_line,
                landing(
                    LandingPlace::Column(position.column),
                    TextUnit::Line,
                    Some(if forward {
                        Message::Bottom
                    } else {
                        Message::Top
                    }),
                ),
            )
        }
        ReviewCommand::ReviewTop => read_line(
            None,
            TextPoint::Start,
            landing(LandingPlace::Start, TextUnit::Line, None),
        ),
        ReviewCommand::ReviewBottom => read_line(
            None,
            TextPoint::End,
            landing(LandingPlace::Start, TextUnit::Line, None),
        ),
        ReviewCommand::ReviewSelectionStart => read_line(
            None,
            TextPoint::SelectionStart,
            landing(LandingPlace::Point, TextUnit::Character, None),
        ),
        ReviewCommand::ReviewSelectionEnd => read_line(
            None,
            TextPoint::SelectionEnd,
            landing(LandingPlace::BeforePoint, TextUnit::Character, None),
        ),
        ReviewCommand::SetStartMarker => {
            state.start_marker = Some(StartMarker::Text {
                node,
                at: point_of(position),
            });
            return vec![speak(trace_id, message(Message::StartMarked))];
        }
        ReviewCommand::MoveToStartMarker => match marker_in(state, node) {
            Err(effect) => return vec![speak(trace_id, effect)],
            Ok(StartMarker::Text { at, .. }) => read_line(
                None,
                TextPoint::At(at),
                landing(LandingPlace::Point, TextUnit::Character, None),
            ),
            Ok(StartMarker::Flat { .. }) => {
                return vec![speak(trace_id, message(Message::NoStartMarker))];
            }
        },
        ReviewCommand::SelectThenCopy => {
            let at = match marker_in(state, node) {
                Err(effect) => return vec![speak(trace_id, effect)],
                Ok(StartMarker::Text { at, .. }) => at,
                Ok(StartMarker::Flat { .. }) => {
                    return vec![speak(trace_id, message(Message::NoStartMarker))];
                }
            };
            // Up to and including the character at the review cursor.
            let end = character_at(content, position.offset).map_or(position.offset, |character| {
                position.offset + character.len()
            });
            let end = TextPoint::At(TextPosition {
                anchor: line.start,
                offset: u32::try_from(end).unwrap_or(u32::MAX),
            });
            if repeat == 0 {
                (
                    TextOp::Select {
                        start: TextPoint::At(at),
                        end,
                    },
                    TextFollowUp::Selected,
                )
            } else {
                (
                    TextOp::ReadRange {
                        start: TextPoint::At(at),
                        end,
                    },
                    TextFollowUp::Copy,
                )
            }
        }
        ReviewCommand::ReportReviewLocation => (
            TextOp::Location(TextPoint::At(point_of(position))),
            TextFollowUp::Location,
        ),
        ReviewCommand::SayAllFromReview => {
            return crate::say_all::start(state, node, false, TextPoint::At(point_of(position)));
        }
        _ => return Vec::new(),
    };
    request(state, node, op, then)
}

/// The start marker, when it is in `node`'s text; otherwise what to say.
fn marker_in(state: &SrState, node: NodeId) -> Result<StartMarker, Vec<UtteranceSegment>> {
    match state.start_marker {
        None => Err(message(Message::NoStartMarker)),
        Some(marker) if marker.node() != node => Err(message(Message::StartMarkerElsewhere)),
        Some(marker) => Ok(marker),
    }
}

/// One message as segments.
pub(crate) fn message(message: Message) -> Vec<UtteranceSegment> {
    vec![UtteranceSegment::new(SegmentContent::Message(message))]
}

/// `segments` after an edge message.
fn with_edge(edge: Message, mut segments: Vec<UtteranceSegment>) -> Vec<UtteranceSegment> {
    segments.insert(0, UtteranceSegment::new(SegmentContent::Message(edge)));
    segments
}

/// An edge message and the character the cursor stays on.
fn edge(
    trace_id: TraceId,
    edge: Message,
    character: Option<&str>,
    language: Option<&str>,
) -> Vec<Effect> {
    vec![speak(
        trace_id,
        with_edge(edge, text::character_segments(character, language)),
    )]
}

/// `text` spelled, or "blank" when there is nothing to spell.
fn spell_or_blank(
    content: &str,
    descriptions: bool,
    language: Option<&str>,
) -> Vec<UtteranceSegment> {
    if content.is_empty() {
        text::text_segments("", None)
    } else {
        text::spelled(content, descriptions, language)
    }
}

/// The character at `offset` of `content`, `None` at its end (a blank
/// cell past a terminal row's text).
fn character_at(content: &str, offset: usize) -> Option<&str> {
    text::grapheme_at(content, offset).map(|range| &content[range])
}

/// A character's code, in decimal and then spelled in hexadecimal, one
/// code point after another (NVDA's third press of the current character).
fn code_segments(character: &str) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    for code in character.chars().map(u32::from) {
        segments.push(UtteranceSegment::text(format!("{code},")));
        segments.extend(text::spelled(&format!("{code:#x}"), false, None));
    }
    segments
}

/// Sets the navigator's review position.
fn set_position(state: &mut SrState, position: ReviewPosition) {
    if let Some(navigator) = state.navigator.as_mut() {
        navigator.text = ReviewText::At(position);
    }
}

/// Moves the review cursor to `offset` on its own line and speaks the
/// character there.
fn land_here(
    state: &mut SrState,
    trace_id: TraceId,
    position: &ReviewPosition,
    offset: usize,
    grid: bool,
) -> Vec<Effect> {
    let landed = position_at(&position.line, offset, grid);
    let content = text::line_content(&landed.line.text, grid);
    let segments = text::character_segments(
        character_at(content, landed.offset),
        landed.line.language_at(landed.offset),
    );
    set_position(state, landed);
    vec![speak(trace_id, segments)]
}

/// Moves the review cursor to a cell column on its own terminal row and
/// speaks the cell there, blank past the row's text.
fn move_column(
    state: &mut SrState,
    trace_id: TraceId,
    position: &ReviewPosition,
    column: usize,
    grid: bool,
) -> Vec<Effect> {
    let content = text::line_content(&position.line.text, grid);
    let offset = text::offset_at_column(content, column, grid);
    let character = character_at(content, offset);
    let language = position.line.language_at(offset);
    set_position(
        state,
        ReviewPosition {
            line: Arc::clone(&position.line),
            offset,
            column,
        },
    );
    vec![speak(
        trace_id,
        text::character_segments(character, language),
    )]
}

/// Handles the answer to a review or text command's request.
pub(crate) fn reply(
    state: &mut SrState,
    trace_id: TraceId,
    pending: PendingText,
    reply: TextReply,
) -> Vec<Effect> {
    let on_navigator = state
        .navigator
        .as_ref()
        .is_some_and(|navigator| navigator.object.id == pending.node);
    match (pending.then, reply) {
        (TextFollowUp::Location, TextReply::Location { x, y }) => vec![speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Positioned { x, y },
            ))],
        )],
        (TextFollowUp::Copy, TextReply::Range { text, .. }) if !text.is_empty() => {
            vec![Effect::CopyToClipboard(text)]
        }
        (TextFollowUp::Selected, TextReply::Done) => Vec::new(),
        (_, TextReply::Unsupported | TextReply::UnsupportedUnit(_))
        | (
            TextFollowUp::Location | TextFollowUp::Selected | TextFollowUp::Copy,
            TextReply::NoText,
        ) => vec![speak(trace_id, message(Message::NotSupported))],
        (_, _) if !on_navigator => Vec::new(),
        (TextFollowUp::Seed { command, repeat }, TextReply::Read { chunk, .. }) => {
            let grid = navigator_grid(state);
            let offset = chunk.offset as usize;
            let position = position_at(&Arc::new(chunk), offset, grid);
            set_position(state, position.clone());
            execute(state, trace_id, pending.node, command, repeat, &position)
        }
        (TextFollowUp::Seed { command, repeat }, TextReply::NoText) => {
            if let Some(navigator) = state.navigator.as_mut() {
                navigator.text = ReviewText::Flat;
            }
            crate::reduce::flat_review_command(state, trace_id, command, repeat)
        }
        (TextFollowUp::Land(landing), TextReply::Read { moved, chunk }) => {
            land(state, trace_id, landing, moved, chunk)
        }
        (
            TextFollowUp::Land(Landing { command, .. }) | TextFollowUp::Seed { command, .. },
            TextReply::AnchorLost,
        ) => {
            // The position the cursor was at is gone: start again from the
            // caret.
            if let Some(navigator) = state.navigator.as_mut() {
                navigator.text = ReviewText::Unknown;
            }
            seed(state, pending.node, TextPoint::Caret, command, 0)
        }
        _ => Vec::new(),
    }
}

/// Whether the navigator object is a terminal.
fn navigator_grid(state: &SrState) -> bool {
    state
        .navigator
        .as_ref()
        .is_some_and(|navigator| is_grid(navigator.object.role))
}

/// Lands the review cursor on a line it moved to and speaks there; when the
/// movement could not move, says the edge and speaks the current unit.
fn land(
    state: &mut SrState,
    trace_id: TraceId,
    landing: Landing,
    moved: i32,
    chunk: verbatim_model::TextChunk,
) -> Vec<Effect> {
    let grid = navigator_grid(state);
    if let Some(edge) = landing.edge
        && moved == 0
    {
        let Some(ReviewText::At(position)) = state
            .navigator
            .as_ref()
            .map(|navigator| navigator.text.clone())
        else {
            return Vec::new();
        };
        let content = text::line_content(&position.line.text, grid);
        let language = position.line.language_at(position.offset);
        let segments = match landing.speak {
            TextUnit::Word => {
                let words = text::words(content, position.line.language_at(0));
                let word =
                    text::word_at(&words, position.offset).map_or("", |range| &content[range]);
                text::text_segments(word, language)
            }
            _ => text::text_segments(content, position.line.language_at(0)),
        };
        return vec![speak(trace_id, with_edge(edge, segments))];
    }
    let line = Arc::new(chunk);
    let content = text::line_content(&line.text, grid);
    let words = || text::words(content, line.language_at(0));
    let position = match landing.place {
        LandingPlace::Column(column) => ReviewPosition {
            line: Arc::clone(&line),
            offset: text::offset_at_column(content, column, grid),
            column,
        },
        LandingPlace::Start => position_at(&line, 0, grid),
        LandingPlace::FirstWord => {
            position_at(&line, words().first().map_or(0, |range| range.start), grid)
        }
        LandingPlace::LastWord => {
            position_at(&line, words().last().map_or(0, |range| range.start), grid)
        }
        LandingPlace::Point => position_at(&line, line.offset as usize, grid),
        LandingPlace::BeforePoint => {
            let offset = (line.offset as usize).min(content.len());
            let before =
                text::previous_grapheme(content, offset).map_or(offset, |range| range.start);
            position_at(&line, before, grid)
        }
    };
    let language = line.language_at(position.offset);
    let segments = match landing.speak {
        TextUnit::Character => {
            text::character_segments(character_at(content, position.offset), language)
        }
        TextUnit::Word => {
            let words = words();
            let word = text::word_at(&words, position.offset).map_or("", |range| &content[range]);
            text::text_segments(word, language)
        }
        _ => text::text_segments(content, line.language_at(0)),
    };
    set_position(state, position);
    vec![speak(trace_id, segments)]
}

/// Starts or reads the review command's flat-text counterparts that M3's
/// flat review does not have: page movement, the selection, location, and
/// say-all have nothing to work on in flat text; the start marker and
/// select then copy work on its byte offsets.
pub(crate) fn flat_extra(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Option<Vec<Effect>> {
    let navigator = state.navigator.as_ref()?;
    let node = navigator.object.id;
    let offset = navigator.review_offset;
    let flat = crate::review::text_of(&navigator.object);
    Some(match command {
        ReviewCommand::ReviewPreviousPage
        | ReviewCommand::ReviewNextPage
        | ReviewCommand::ReviewSelectionStart
        | ReviewCommand::ReviewSelectionEnd
        | ReviewCommand::ReportReviewLocation => {
            vec![speak(trace_id, message(Message::NotSupported))]
        }
        ReviewCommand::SayAllFromReview => {
            let rest = flat.get(offset..).unwrap_or("");
            vec![speak(trace_id, text::text_segments(rest, None))]
        }
        ReviewCommand::SetStartMarker => {
            state.start_marker = Some(StartMarker::Flat { node, offset });
            vec![speak(trace_id, message(Message::StartMarked))]
        }
        ReviewCommand::MoveToStartMarker => match marker_in(state, node) {
            Ok(StartMarker::Flat { offset, .. }) if flat.is_char_boundary(offset) => {
                if let Some(navigator) = state.navigator.as_mut() {
                    navigator.review_offset = offset;
                }
                let character = text::grapheme_at(&flat, offset).map(|range| &flat[range]);
                vec![speak(trace_id, text::character_segments(character, None))]
            }
            Ok(_) => vec![speak(trace_id, message(Message::NoStartMarker))],
            Err(segments) => vec![speak(trace_id, segments)],
        },
        ReviewCommand::SelectThenCopy => match marker_in(state, node) {
            Ok(StartMarker::Flat { offset: start, .. }) => {
                if repeat == 0 {
                    // Flat text cannot be selected; pressed again, it is
                    // copied.
                    vec![speak(trace_id, message(Message::NotSupported))]
                } else {
                    let end = text::grapheme_at(&flat, offset).map_or(offset, |range| range.end);
                    let (from, to) = (start.min(end), start.max(end));
                    match flat.get(from..to) {
                        Some(copied) if !copied.is_empty() => {
                            vec![Effect::CopyToClipboard(copied.to_owned())]
                        }
                        _ => Vec::new(),
                    }
                }
            }
            Ok(_) => vec![speak(trace_id, message(Message::NoStartMarker))],
            Err(segments) => vec![speak(trace_id, segments)],
        },
        _ => return None,
    })
}
