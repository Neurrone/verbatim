//! The review cursor over text read through the text protocol (milestone
//! M4; `docs/parity/review.md`), with NVDA's
//! messages and repeated presses (`docs/nvda/review-modes.md`).
//!
//! Every review command reads live (Dickson, 2026-10-08;
//! `docs/design/m42-parity.md`): a command that answers from the review
//! cursor's line reads it first, and a move goes straight to the read that
//! moves. A press is an entry of the key line
//! ([`crate::key_line::Press`]). A dead object reviews as an object with
//! empty text. Columns follow the design's one intentional difference from
//! NVDA (`docs/parity/review.md`, "Review cursor columns"): moving to the next or
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
    TextPoint, TextPosition, TextRead, TextReply, TextThrough, TextUnit, TraceId, UtteranceSegment,
};

use crate::editing::{is_grid, may_have_text, selection_segments, speak, speak_each};
use crate::key_line::ReviewPress;
use crate::proven;
use crate::queries::Ending;
use crate::state::{
    Landing, LandingPlace, ReviewPosition, ReviewText, SharedChunk, SrState, StartMarker,
};
use crate::text;
use verbatim_model::protocol::{QueryOutcome, QueryResult};

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

/// Whether a review command answers from the review cursor's line, which
/// is then read first: any other goes straight to the read that moves.
fn answers_from_line(command: ReviewCommand) -> bool {
    matches!(
        command,
        ReviewCommand::ReviewCurrentLine
            | ReviewCommand::ReviewCurrentWord
            | ReviewCommand::ReviewCurrentCharacter
            | ReviewCommand::ReviewStartOfLine
            | ReviewCommand::ReviewEndOfLine
            | ReviewCommand::ReviewPreviousCharacter
            | ReviewCommand::ReviewNextCharacter
            | ReviewCommand::ReviewPreviousWord
            | ReviewCommand::ReviewNextWord
            | ReviewCommand::SetStartMarker
            | ReviewCommand::SelectThenCopy
    )
}

/// Where a review press starts.
pub(crate) enum Start {
    /// On the review cursor's line as Core holds it.
    At(ReviewPosition),
    /// With a read of the line at this point, where the cursor is placed.
    Line(TextPoint),
    /// On the navigator object's flat text, read live.
    Flat,
}

/// What a review command does with the review cursor at a position: it is
/// said at once, or a read is made first.
pub(crate) enum Plan {
    Say(Vec<Effect>),
    Read(TextOp, After),
}

/// What a press's read is for.
pub(crate) enum After {
    /// The review cursor moved to another line: land on it and speak.
    Land(Landing),
    /// A select then copy selected the text.
    Selected,
    /// A select then copy read the text to copy, for the start marker
    /// whose [`SrState::start_marker_generation`] this is: the copy clears
    /// that marker only, not one set while the read was in flight.
    Copy { marker: u64 },
    /// A location report.
    Location,
}

/// Where `command` starts on the navigator's text: a command that answers
/// from the review cursor's line reads it first, and any other goes
/// straight to the read that moves.
pub(crate) fn start_of(state: &mut SrState, command: ReviewCommand) -> Option<Start> {
    let navigator = state.navigator.as_mut()?;
    let node = navigator.object.id;
    Some(match navigator.text.clone() {
        ReviewText::At(position) if answers_from_line(command) => {
            Start::Line(TextPoint::At(point_of(&position)))
        }
        ReviewText::At(position) => Start::At(position),
        ReviewText::Point(point) => Start::Line(TextPoint::At(point)),
        ReviewText::Flat => Start::Flat,
        // The caret as the outpost finds it, never Core's copy, which can
        // lag behind a key the application has already handled.
        ReviewText::Caret => Start::Line(TextPoint::Caret),
        ReviewText::Unknown => {
            let has_caret = state
                .focus
                .object()
                .is_some_and(|focus| focus.snapshot.id == node && focus.caret.is_some());
            if has_caret || may_have_text(navigator.object.role) {
                Start::Line(TextPoint::Caret)
            } else {
                navigator.text = ReviewText::Flat;
                Start::Flat
            }
        }
    })
}

/// A move of the review cursor to the previous or next command mark of the
/// navigator's terminal (`books/dev/src/explanation/design/terminal-model.md`, "Features and
/// settings"), which Core's terminal model answers as the line move it is,
/// landing at the review cursor's column and speaking the line, so the
/// review position and what is said follow a line move's: where it moves
/// from, and how it lands.
pub(crate) fn mark_move(state: &SrState, command: ReviewCommand) -> Option<(TextPoint, Landing)> {
    let navigator = state.navigator.as_ref()?;
    let (from, place) = match &navigator.text {
        ReviewText::At(position) => (
            TextPoint::At(point_of(position)),
            LandingPlace::Column(position.column),
        ),
        _ => (TextPoint::Caret, LandingPlace::Start),
    };
    Some((
        from,
        Landing {
            place,
            speak: TextUnit::Line,
            edge: None,
            command,
        },
    ))
}

/// A review position on `line` at byte `offset`, which may be on its line
/// break ([`text::position_in`]), its column worked out.
fn position_at(line: &SharedChunk, offset: usize, grid: bool) -> ReviewPosition {
    let content = text::line_content(&line.text, grid);
    let offset = text::position_in(&line.text, offset, grid);
    ReviewPosition {
        line: Arc::clone(line),
        offset,
        column: text::column_of(content, offset, grid),
    }
}

/// The read of the line at `at`, where the review cursor is then placed.
pub(crate) fn line_at(at: TextPoint) -> TextOp {
    TextOp::Read(TextRead {
        at,
        movement: None,
        unit: TextUnit::Line,
    })
}

/// The point the review cursor is at, as a position the outpost resolves.
fn point_of(position: &ReviewPosition) -> TextPosition {
    TextPosition {
        anchor: position.line.start,
        offset: u32::try_from(position.offset).unwrap_or(u32::MAX),
    }
}

/// Plans a command with the review cursor at `position`.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per review command, each short, reads best together"
)]
pub(crate) fn execute(
    state: &mut SrState,
    trace_id: TraceId,
    press: ReviewPress,
    node: NodeId,
    position: &ReviewPosition,
) -> Plan {
    let (command, repeat) = (press.command, press.repeat);
    let grid = state
        .navigator
        .as_ref()
        .is_some_and(|navigator| is_grid(navigator.object.role));
    let line = &position.line;
    let content = text::line_content(&line.text, grid);
    let language = line.language_at(position.offset);
    // The review cursor's position, and the one just past the character
    // there: the two ends of the review copy's character at the cursor.
    let cursor = point_of(position);
    let after_cursor = || TextPosition {
        anchor: line.start,
        offset: u32::try_from(
            text::character_at(&line.text, position.offset, grid)
                .map_or(position.offset, |character| {
                    position.offset.saturating_add(character.len())
                }),
        )
        .unwrap_or(u32::MAX),
    };
    let read_line = |movement: Option<TextMovement>, at: TextPoint, landing: Landing| {
        (
            TextOp::Read(TextRead {
                at,
                movement,
                unit: TextUnit::Line,
            }),
            After::Land(landing),
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
            return Plan::Say(vec![speak(trace_id, segments)]);
        }
        ReviewCommand::ReviewCurrentWord => {
            let words = text::words(content, line.language_at(0));
            let word =
                text::word_at(&words, position.offset).map(|range| proven::text_in(content, range));
            let segments = match (word, repeat) {
                (None, _) => text::text_segments("", None),
                (Some(word), 0) => text::word_segments(word, language),
                (Some(word), 1) => text::spelled(word, false, language),
                (Some(word), _) => text::spelled(word, true, language),
            };
            return Plan::Say(vec![speak(trace_id, segments)]);
        }
        ReviewCommand::ReviewCurrentCharacter => {
            let character = text::character_at(&line.text, position.offset, grid);
            let segments = match (character, repeat) {
                (None, _) | (Some(_), 0) => text::character_segments(character, language),
                (Some(character), 1) => vec![UtteranceSegment {
                    content: SegmentContent::CharacterDescription(character.to_owned()),
                    language: language.map(str::to_owned),
                }],
                (Some(character), _) => code_segments(character),
            };
            return Plan::Say(vec![speak(trace_id, segments)]);
        }
        ReviewCommand::ReviewStartOfLine => {
            return Plan::Say(land_here(state, trace_id, position, 0, grid));
        }
        ReviewCommand::ReviewEndOfLine => {
            // The line's last character, its line break when it has one, as
            // in NVDA.
            let last = text::characters(&line.text, grid)
                .last()
                .map_or(0, |range| range.start);
            return Plan::Say(land_here(state, trace_id, position, last, grid));
        }
        ReviewCommand::ReviewPreviousCharacter => {
            let text_width = text::column_of(content, content.len(), true);
            if grid && position.offset >= content.len() && position.column > text_width {
                // Past the end of a terminal row's text: the blank cell to
                // the left.
                return Plan::Say(move_column(
                    state,
                    trace_id,
                    position,
                    position.column.saturating_sub(1),
                    grid,
                ));
            }
            return Plan::Say(
                match text::previous_character(&line.text, position.offset, grid) {
                    Some(range) => land_here(state, trace_id, position, range.start, grid),
                    None => edge(
                        trace_id,
                        Message::Left,
                        text::character_at(&line.text, position.offset, grid),
                        language,
                    ),
                },
            );
        }
        ReviewCommand::ReviewNextCharacter => {
            let character = text::character_at(&line.text, position.offset, grid);
            if grid {
                // Cells, up to the row's last character: padding is never
                // reviewed (`books/dev/src/explanation/design/terminal-model.md`, "Features and settings").
                // A cell past the text, where moving down a column leaves
                // the cursor, is at the edge already.
                let next = position.column.saturating_add(
                    character.map_or(1, |character| verbatim_text::cell_width(character).max(1)),
                );
                if next >= text::column_of(content, content.len(), true) {
                    return Plan::Say(edge(trace_id, Message::Right, character, language));
                }
                return Plan::Say(move_column(state, trace_id, position, next, grid));
            }
            return Plan::Say(
                match text::next_character(&line.text, position.offset, grid) {
                    Some(range) => land_here(state, trace_id, position, range.start, grid),
                    None => edge(trace_id, Message::Right, character, language),
                },
            );
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
                return Plan::Say(vec![speak(
                    trace_id,
                    text::word_segments(proven::text_in(content, target.clone()), language),
                )]);
            }
            let at_edge = if forward { line.last } else { line.first };
            let message = if forward {
                Message::Bottom
            } else {
                Message::Top
            };
            if at_edge {
                let word = current.map(|range| proven::text_in(content, range));
                return Plan::Say(vec![speak(
                    trace_id,
                    with_edge(message, text::word_segments(word.unwrap_or(""), language)),
                )]);
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
            state.start_marker_generation = state.start_marker_generation.saturating_add(1);
            state.start_marker = Some(StartMarker::Text {
                node,
                at: cursor,
                after: after_cursor(),
                selected_to: None,
            });
            return Plan::Say(vec![speak(trace_id, message(Message::StartMarked))]);
        }
        ReviewCommand::MoveToStartMarker => match marker_in(state, node) {
            Err(effect) => return Plan::Say(vec![speak(trace_id, effect)]),
            Ok(StartMarker::Text { at, .. }) => read_line(
                None,
                TextPoint::At(at),
                landing(LandingPlace::Point, TextUnit::Character, None),
            ),
            Ok(StartMarker::Flat { .. }) => {
                return Plan::Say(vec![speak(trace_id, message(Message::NoStartMarker))]);
            }
        },
        ReviewCommand::SelectThenCopy => {
            let (at, after, selected_to) = match marker_in(state, node) {
                Err(effect) => return Plan::Say(vec![speak(trace_id, effect)]),
                Ok(StartMarker::Text {
                    at,
                    after,
                    selected_to,
                    ..
                }) => (at, after, selected_to),
                Ok(StartMarker::Flat { .. }) => {
                    return Plan::Say(vec![speak(trace_id, message(Message::NoStartMarker))]);
                }
            };
            // From the earlier of the marker and the review cursor through
            // the character at the later, whichever that is
            // (`docs/parity/review.md`, "Select then copy from the review
            // cursor"): which comes first only the outpost can tell, so
            // both ends go with the points just past their characters.
            let range = |(end, end_after): (TextPosition, TextPosition)| {
                (
                    TextPoint::At(at),
                    TextPoint::At(end),
                    Some(TextThrough {
                        start: TextPoint::At(after),
                        end: TextPoint::At(end_after),
                    }),
                )
            };
            if repeat == 0 && selected_to.is_some() {
                // The range a first press selected is kept until a second
                // press copies it or the marker is set again; a further
                // first press only says so (`docs/parity/review.md`,
                // "Select then copy from the review cursor").
                return Plan::Say(vec![speak(trace_id, message(Message::PressTwiceToCopy))]);
            }
            if repeat == 0 {
                // The second press copies the range this one selects, as
                // NVDA's does, wherever the review cursor has gone since:
                // the caret the selection moved may have taken it along.
                state.start_marker = Some(StartMarker::Text {
                    node,
                    at,
                    after,
                    selected_to: Some((cursor, after_cursor())),
                });
                let (start, end, through) = range((cursor, after_cursor()));
                (
                    TextOp::Select {
                        start,
                        end,
                        through,
                    },
                    After::Selected,
                )
            } else {
                let (start, end, through) =
                    range(selected_to.unwrap_or_else(|| (cursor, after_cursor())));
                (
                    TextOp::ReadRange {
                        start,
                        end,
                        through,
                    },
                    After::Copy {
                        marker: state.start_marker_generation,
                    },
                )
            }
        }
        ReviewCommand::ReportReviewLocation => {
            (TextOp::Location(TextPoint::At(cursor)), After::Location)
        }
        ReviewCommand::SayAllFromReview => {
            return Plan::Say(crate::say_all::start(
                state,
                node,
                false,
                TextPoint::At(point_of(position)),
            ));
        }
        _ => return Plan::Say(Vec::new()),
    };
    Plan::Read(op, then)
}

/// The start marker, when it is in `node`'s text; otherwise what to say:
/// "No start marker set" whether there is none or it is in another object,
/// which keeps it for when the navigator returns (`docs/parity/review.md`,
/// "Select then copy from the review cursor").
fn marker_in(state: &SrState, node: NodeId) -> Result<StartMarker, Vec<UtteranceSegment>> {
    match state.start_marker {
        Some(marker) if marker.node() == node => Ok(marker),
        _ => Err(message(Message::NoStartMarker)),
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
pub(crate) fn spell_or_blank(
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
    let segments = text::character_segments(
        text::character_at(&landed.line.text, landed.offset, grid),
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
    let character = text::character_at(&position.line.text, offset, grid);
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

/// The flat text of the navigator object from its live read, for a flat
/// review command or Report Current Object, the navigator taking the
/// object read. Gone, a dead object reviews as empty flat text and the
/// report says nothing; a read that gets no answer says nothing at all,
/// which is `None`.
pub(crate) fn object_read(
    state: &mut SrState,
    command: ReviewCommand,
    ending: Ending,
) -> Option<String> {
    match ending {
        Ending::Outcome(QueryOutcome::Done(QueryResult::Node(object))) => {
            let flat = crate::review::text_of(&object);
            if let Some(navigator) = state.navigator.as_mut() {
                // The desktop window keeps the name Core gives it, which a
                // live read of the window does not carry.
                if !navigator.object.details.desktop_window {
                    navigator.object = object;
                }
                if !flat.is_char_boundary(navigator.review_offset) {
                    navigator.review_offset = 0;
                }
            }
            Some(flat)
        }
        Ending::Outcome(QueryOutcome::Gone) if command == ReviewCommand::ReportObject => None,
        Ending::Outcome(QueryOutcome::Gone) => Some(String::new()),
        _ => None,
    }
}

/// The caret's or the review cursor's location answered.
pub(crate) fn location(trace_id: TraceId, reply: &TextReply) -> Vec<Effect> {
    match reply {
        TextReply::Location { x, y } => vec![speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Positioned { x: *x, y: *y },
            ))],
        )],
        // Text measured by character offsets says how far through the text
        // the place is, then where it is on the screen
        // (`docs/parity/review.md`, "Review cursor line/word/character over
        // object text").
        TextReply::OffsetLocation {
            offset,
            length,
            point,
        } => vec![speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::OffsetPositioned {
                    offset: *offset,
                    length: *length,
                    point: *point,
                },
            ))],
        )],
        TextReply::Unsupported | TextReply::UnsupportedUnit(_) | TextReply::NoText => {
            vec![speak(trace_id, message(Message::NotSupported))]
        }
        _ => Vec::new(),
    }
}

/// Whether the navigator is on `node`: a read answered after it moved off
/// places nothing.
fn on_navigator(state: &SrState, node: NodeId) -> bool {
    state
        .navigator
        .as_ref()
        .is_some_and(|navigator| navigator.object.id == node)
}

/// What the read of the review cursor's line found.
pub(crate) enum Seeded {
    /// The line, the review cursor placed on it.
    At(ReviewPosition),
    /// No text: the object is reviewed as flat text from now on.
    Flat,
    /// The position the cursor was at is gone: start again from the caret.
    Lost,
    /// Nothing to go on with, and this said.
    Said(Vec<Effect>),
}

/// Places the review cursor from `reply`, the read of its line in `node`.
/// A dead object is one rule at the boundary: the press run on empty flat
/// text.
pub(crate) fn seeded(
    state: &mut SrState,
    trace_id: TraceId,
    press: ReviewPress,
    node: NodeId,
    reply: TextReply,
) -> Seeded {
    let on_navigator = on_navigator(state, node);
    match reply {
        TextReply::Gone => Seeded::Said(crate::reduce::flat_review_command(
            state,
            trace_id,
            press.command,
            press.repeat,
            "",
        )),
        TextReply::Unsupported | TextReply::UnsupportedUnit(_) => {
            Seeded::Said(vec![speak(trace_id, message(Message::NotSupported))])
        }
        TextReply::Read { chunk, .. } if on_navigator => {
            let grid = navigator_grid(state);
            let offset = text::byte_offset(chunk.offset);
            let mut position = position_at(&Arc::new(chunk), offset, grid);
            // A line read again where the review cursor was, unchanged,
            // keeps its column, which may lie past the end of the line's
            // text, as moving down a column of a table leaves it, so that
            // Down Arrow returns to it.
            if let Some(ReviewText::At(was)) =
                state.navigator.as_ref().map(|navigator| &navigator.text)
                && was.line.text == position.line.text
                && was.offset == position.offset
            {
                position.column = was.column;
            }
            set_position(state, position.clone());
            Seeded::At(position)
        }
        TextReply::NoText if on_navigator => {
            if let Some(navigator) = state.navigator.as_mut() {
                navigator.text = ReviewText::Flat;
            }
            Seeded::Flat
        }
        TextReply::AnchorLost if on_navigator => {
            forget_position(state);
            Seeded::Lost
        }
        _ => Seeded::Said(Vec::new()),
    }
}

/// The review cursor's position is gone from the application.
fn forget_position(state: &mut SrState) {
    if let Some(navigator) = state.navigator.as_mut() {
        navigator.text = ReviewText::Unknown;
    }
}

/// Ends a press with `reply`, the answer to the read its command planned
/// for `after`, in `node`; `None` when the position it moved from is gone,
/// and the press starts again from the caret.
pub(crate) fn after_read(
    state: &mut SrState,
    trace_id: TraceId,
    press: ReviewPress,
    node: NodeId,
    after: After,
    reply: TextReply,
) -> Option<Vec<Effect>> {
    let on_navigator = on_navigator(state, node);
    Some(match (after, reply) {
        (_, TextReply::Unanswered) => Vec::new(),
        // A copy clears the start marker it copied from whatever it found,
        // so a further press has none, but keeps a marker set while the
        // read was in flight; a range with nothing in it copies nothing and
        // says nothing (`docs/parity/review.md`, "Select then copy from
        // the review cursor").
        (After::Copy { marker }, reply) => {
            if state.start_marker_generation == marker {
                state.start_marker = None;
            }
            match reply {
                TextReply::Range { text, .. } if !text.is_empty() => {
                    vec![Effect::CopyToClipboard(text)]
                }
                TextReply::Unsupported | TextReply::UnsupportedUnit(_) | TextReply::NoText => {
                    vec![speak(trace_id, message(Message::NotSupported))]
                }
                _ => Vec::new(),
            }
        }
        // A dead object: the press run on empty flat text.
        (_, TextReply::Gone) => {
            crate::reduce::flat_review_command(state, trace_id, press.command, press.repeat, "")
        }
        (After::Location, reply) => location(trace_id, &reply),
        // The selection the command made is spoken as a caret key's is,
        // "selected" and "unselected" from the same comparison
        // (`docs/parity/review.md`, "Select then copy from the review
        // cursor").
        (After::Selected, TextReply::Selected(changes)) => {
            speak_each(trace_id, selection_segments(&changes, None))
        }
        // The text cannot be selected: the second press still copies it.
        (After::Selected, TextReply::Unsupported | TextReply::NoText) => {
            vec![speak(trace_id, message(Message::CannotSelect))]
        }
        (_, TextReply::Unsupported | TextReply::UnsupportedUnit(_)) => {
            vec![speak(trace_id, message(Message::NotSupported))]
        }
        (_, _) if !on_navigator => Vec::new(),
        (After::Land(landing), TextReply::Read { moved, chunk }) => {
            land(state, trace_id, landing, moved, chunk)
        }
        (After::Land(_), TextReply::AnchorLost) => {
            forget_position(state);
            return None;
        }
        _ => Vec::new(),
    })
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
                let word = text::word_at(&words, position.offset)
                    .map_or("", |range| proven::text_in(content, range));
                text::word_segments(word, language)
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
        LandingPlace::Point => position_at(&line, text::byte_offset(line.offset), grid),
        LandingPlace::BeforePoint => {
            let offset = text::boundary(content, text::byte_offset(line.offset));
            let before =
                text::previous_grapheme(content, offset).map_or(offset, |range| range.start);
            position_at(&line, before, grid)
        }
    };
    let language = line.language_at(position.offset);
    let segments = match landing.speak {
        TextUnit::Character => text::character_segments(
            text::character_at(&line.text, position.offset, grid),
            language,
        ),
        TextUnit::Word => {
            let words = words();
            let word = text::word_at(&words, position.offset)
                .map_or("", |range| proven::text_in(content, range));
            text::word_segments(word, language)
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
    flat: &str,
) -> Option<Vec<Effect>> {
    let navigator = state.navigator.as_ref()?;
    let node = navigator.object.id;
    let offset = navigator.review_offset;
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
            state.start_marker_generation = state.start_marker_generation.saturating_add(1);
            state.start_marker = Some(StartMarker::Flat {
                node,
                offset,
                selected_to: None,
            });
            vec![speak(trace_id, message(Message::StartMarked))]
        }
        ReviewCommand::MoveToStartMarker => match marker_in(state, node) {
            Ok(StartMarker::Flat { offset, .. }) if flat.is_char_boundary(offset) => {
                if let Some(navigator) = state.navigator.as_mut() {
                    navigator.review_offset = offset;
                }
                let character = text::grapheme_at(flat, offset).and_then(|range| flat.get(range));
                vec![speak(trace_id, text::character_segments(character, None))]
            }
            Ok(_) => vec![speak(trace_id, message(Message::NoStartMarker))],
            Err(segments) => vec![speak(trace_id, segments)],
        },
        ReviewCommand::SelectThenCopy => match marker_in(state, node) {
            Ok(StartMarker::Flat {
                offset: start,
                selected_to,
                ..
            }) => {
                if repeat == 0 && selected_to.is_some() {
                    vec![speak(trace_id, message(Message::PressTwiceToCopy))]
                } else if repeat == 0 {
                    // Flat text cannot be selected, but the range is built
                    // now, and the second press copies it wherever the
                    // review cursor has gone since.
                    state.start_marker = Some(StartMarker::Flat {
                        node,
                        offset: start,
                        selected_to: Some(offset),
                    });
                    vec![speak(trace_id, message(Message::CannotSelect))]
                } else {
                    // From the earlier of the marker and the review cursor
                    // through the character at the later
                    // (`docs/parity/review.md`, "Select then copy from the
                    // review cursor"), the review cursor taken where a first
                    // press found it.
                    let end = selected_to.unwrap_or(offset);
                    let after =
                        |at: usize| text::grapheme_at(flat, at).map_or(at, |range| range.end);
                    let (from, to) = if end < start {
                        (end, after(start))
                    } else {
                        (start, after(end))
                    };
                    // The copy clears the marker whatever it found.
                    state.start_marker = None;
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
