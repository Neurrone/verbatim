//! The review cursor over text read through the text protocol (milestone
//! M4; `docs/parity/review.md`), with NVDA's
//! messages and repeated presses (`docs/nvda/review-modes.md`).
//!
//! Every review command reads live (Dickson, 2026-10-08;
//! `docs/design/m42-parity.md`): a command that answers from the review
//! cursor's line reads it first, and a move goes straight to the read that
//! moves. A press is an entry of the event queue (`crate::keys::entry`).
//! A dead object reviews as an object with
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
//! is reviewed through the same path over its flat text, NVDA's basic text
//! ([`flat_text`]), read live at each press, whose requests Core answers
//! itself ([`flat_answer`]), as NVDA reviews such an object through an
//! offsets text info over its basic text (`NVDAObjects/__init__.py` 54 to
//! 79).

use std::sync::Arc;

use verbatim_model::{
    Effect, Message, NodeId, Phrase, ReviewCommand, SegmentContent, TextMovement, TextOp,
    TextPoint, TextPosition, TextRead, TextReply, TextThrough, TextUnit, TraceId, UtteranceSegment,
};

use crate::editing::{is_grid, may_have_text, selection_segments, speak, speak_each};
use crate::entries::Entries;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome};
use crate::keys::entry::Fence;
use crate::keys::entry::ReviewPress;
use crate::navigator::Found;
use crate::state::{
    FlatPosition, Landing, LandingPlace, Marked, MarkerEnds, PendingText, ReviewPosition,
    ReviewText, SharedChunk, SrState, StartMarker, TextFollowUp,
};
use crate::text;
use crate::text_request::TextRequest;
use verbatim_text::proven;

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

/// What a review press does next: its words, a read of the navigator's
/// text followed up by a [`TextFollowUp`], a live read of the navigator
/// object for flat review, the press run on flat text, or say-all from the
/// review cursor.
#[derive(Debug)]
pub(crate) enum Next {
    /// These words, and the press ends.
    Said(Vec<Effect>),
    /// A read or move of `node`'s text, followed up as the follow-up says.
    Text(NodeId, TextOp, TextFollowUp),
    /// A live read of the navigator object, whose text is reviewed flat.
    ReadObject,
    /// The press run on this flat text of the navigator object, every
    /// request it makes answered from the text.
    Flat(String),
    /// Say-all from this point of `node`'s text.
    SayAll(NodeId, TextPoint),
}

/// What a review press does first against the navigator's text: the line
/// at its review position or caret is read first for a command that
/// answers from it.
pub(crate) fn begin(state: &mut SrState, press: ReviewPress) -> Next {
    let Some(navigator) = state.navigator.as_mut() else {
        return Next::Said(Vec::new());
    };
    let node = navigator.object.id;
    match navigator.text.clone() {
        ReviewText::At(position) if answers_from_line(press.command) => {
            seed(press, node, TextPoint::At(point_of(&position)))
        }
        ReviewText::At(position) => execute(state, press.trace_id, press, node, &position),
        ReviewText::Point(point) => seed(press, node, TextPoint::At(point)),
        ReviewText::Flat(_) => Next::ReadObject,
        // The caret as the outpost finds it, never Core's copy, which can
        // lag behind a key the application has already handled.
        ReviewText::Caret => seed(press, node, TextPoint::Caret),
        ReviewText::Unknown => {
            let has_caret = state
                .versions
                .focus()
                .map(|focus| &focus.context)
                .is_some_and(|focus| focus.snapshot.id == node && focus.caret.is_some());
            if has_caret || may_have_text(navigator.object.role) {
                seed(press, node, TextPoint::Caret)
            } else {
                navigator.text = ReviewText::Flat(FlatPosition::default());
                Next::ReadObject
            }
        }
    }
}

/// A review press at its turn as an event-queue entry
/// (`books/dev/src/reference/crates/verbatim-core.md`, "The review cursor over text"): it resolves
/// the navigator, then makes each read of its
/// text, or the live read of the object for flat review, as the queue's
/// request on the navigator's window, holding the queue until its last
/// answer, as NVDA runs its scripts on its queue. One due time covers the
/// whole chain. An object not there reviews as empty flat text; any other
/// outcome but a result says nothing.
pub(crate) async fn entry(mut cx: Cx<Entries>, press: ReviewPress, fence: Fence) {
    let trace_id = press.trace_id;
    let found = crate::navigator::found(&cx.facts());
    let window = match found {
        Found::Nothing => return crate::navigator::no_navigator(&mut cx, trace_id),
        Found::Unread => return,
        Found::On(on) => on.window,
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let next = begin(&mut cx.facts_mut(), press);
    run_next(cx, (press, fence), window, next, (due_us, false)).await;
}

/// Runs a review press from `next` on: each read of its text, or the live
/// read of the object for flat review, as the queue's request on the
/// navigator's `window`, under the due time `due_us`; a read of the
/// focused terminal's text is answered from Core's memory after one fresh
/// read a command (`docs/design/readers-tell-core-asks-build3.md`,
/// "Review commands as chained requests"), `fresh` when it was taken; and
/// a read of flat text from the text the press read ([`flat_answer`]).
async fn run_next(
    mut cx: Cx<Entries>,
    (press, fence): (ReviewPress, Fence),
    window: crate::event_queue::RequestWindow,
    mut next: Next,
    (due_us, mut fresh): (u64, bool),
) {
    let trace_id = press.trace_id;
    let mut flat: Option<String> = None;
    loop {
        next = match next {
            Next::Said(effects) => {
                for effect in effects {
                    cx.emit(effect);
                }
                return;
            }
            Next::Flat(text) => {
                let next = on_flat(&mut cx.facts_mut(), press, &text);
                flat = Some(text);
                next
            }
            Next::Text(node, op, then) if flat.is_some() => {
                let reply = flat_answer(flat.as_deref().unwrap_or_default(), &op);
                answered(&mut cx.facts_mut(), trace_id, (node, press), then, reply)
            }
            Next::SayAll(_, at) if flat.is_some() => {
                // Say-all from the review cursor in flat text speaks the
                // rest of it at once, with no caret or reading to move.
                let text = flat.as_deref().unwrap_or_default();
                let rest = proven::text_in(text, flat_offset(text, at)..);
                Next::Said(vec![speak(trace_id, text::text_segments(rest, None))])
            }
            Next::Text(node, op, then)
                if crate::terminal::commands::answers(&cx.facts(), node, &op) =>
            {
                if !fresh {
                    crate::terminal::commands::fresh(&mut cx, trace_id, node, due_us).await;
                    fresh = true;
                }
                let Some(reply) =
                    crate::terminal::commands::answer(&mut cx, (window, node), &op, due_us).await
                else {
                    return;
                };
                answered(&mut cx.facts_mut(), trace_id, (node, press), then, reply)
            }
            Next::Text(node, op, then) => {
                let request = TextRequest {
                    process: window.process,
                    window,
                    node,
                    op,
                };
                let reply = match cx.ask(request, due_us).await {
                    // A selection made on the focused text mints the next
                    // selection baseline, and is said as any selection the
                    // command makes (Table 17, column F).
                    Outcome::Result(TextReply::SelectionPlaced { changes, baseline }) => {
                        crate::text_facts::minted(&mut cx.facts_mut(), node, baseline);
                        TextReply::Selected(changes)
                    }
                    Outcome::Result(reply) => reply,
                    Outcome::NotThere => TextReply::Gone,
                    _ => return,
                };
                answered(&mut cx.facts_mut(), trace_id, (node, press), then, reply)
            }
            Next::ReadObject => {
                let Some(object) = cx
                    .facts()
                    .navigator
                    .as_ref()
                    .map(|navigator| navigator.object.clone())
                else {
                    return;
                };
                // The description is read whatever the theme reports, as
                // NVDA's basic text has it.
                let request = crate::navigator::Properties {
                    window,
                    node: object.id,
                    set: crate::details::description_kept(
                        &cx.facts(),
                        &crate::report_focus::ANNOUNCED_SET,
                    ),
                };
                match cx.ask(request, due_us).await {
                    Outcome::Result(values) => {
                        let read = verbatim_model::request::NodeRead {
                            node: Some(object.id),
                            window: window.node_window(),
                            values,
                        };
                        match crate::navigator::snapshot(read, &object) {
                            Some(live) => Next::Flat(object_read(&mut cx.facts_mut(), live)),
                            None => return,
                        }
                    }
                    Outcome::NotThere => gone(&mut cx.facts_mut()),
                    _ => return,
                }
            }
            Next::SayAll(node, at) => Next::Said(crate::say_all::start_in_queue(
                &mut cx.facts_mut(),
                (node, window),
                (false, at),
                (trace_id, fence),
            )),
        };
    }
}

/// Moves the review cursor to the previous or next command mark of the
/// focused terminal, the navigator (`books/dev/src/explanation/design/terminal-model.md`,
/// "Features and settings"), as an event-queue entry: after the fresh
/// read, Core's terminal model answers it as the line move it is, landing
/// at the review cursor's column and speaking the line, so the review
/// position and what is said follow a line move's; nothing is said past
/// the last mark that way, or when the navigator is not the terminal Core
/// models.
pub(crate) async fn command_mark(mut cx: Cx<Entries>, press: ReviewPress, fence: Fence) {
    let command = press.command;
    let window = match crate::navigator::found(&cx.facts()) {
        Found::On(on) => on.window,
        Found::Nothing | Found::Unread => return,
    };
    let (node, from, place) = {
        let facts = cx.facts();
        let Some(navigator) = facts.navigator.as_ref() else {
            return;
        };
        let (from, place) = match &navigator.text {
            ReviewText::At(position) => (
                TextPoint::At(point_of(position)),
                LandingPlace::Column(position.column),
            ),
            _ => (TextPoint::Caret, LandingPlace::Start),
        };
        (navigator.object.id, from, place)
    };
    if crate::terminal::reading::modelled(&cx.facts()) != Some(node) {
        return;
    }
    let forward = command == ReviewCommand::NextCommandMark;
    let then = TextFollowUp::Land(Landing {
        place,
        speak: TextUnit::Line,
        edge: None,
        command,
    });
    let due_us = cx.due_in(OTHER_DUE_US);
    crate::terminal::commands::fresh(&mut cx, press.trace_id, node, due_us).await;
    let Some(reply) =
        crate::terminal::commands::command_mark(&mut cx, (window, node), (forward, from), due_us)
            .await
    else {
        return;
    };
    let next = answered(
        &mut cx.facts_mut(),
        press.trace_id,
        (node, press),
        then,
        reply,
    );
    run_next(cx, (press, fence), window, next, (due_us, true)).await;
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

/// Reads the line at `at` to place the review cursor there, then runs the
/// command.
fn seed(press: ReviewPress, node: NodeId, at: TextPoint) -> Next {
    Next::Text(
        node,
        TextOp::Read(TextRead {
            at,
            movement: None,
            unit: TextUnit::Line,
        }),
        TextFollowUp::Seed {
            command: press.command,
            repeat: press.repeat,
            at_caret: at == TextPoint::Caret,
        },
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
    press: ReviewPress,
    node: NodeId,
    position: &ReviewPosition,
) -> Next {
    let (command, repeat) = (press.command, press.repeat);
    let grid = navigator_grid(state);
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
            return Next::Said(vec![speak(trace_id, segments)]);
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
            return Next::Said(vec![speak(trace_id, segments)]);
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
            return Next::Said(vec![speak(trace_id, segments)]);
        }
        ReviewCommand::ReviewStartOfLine => {
            return Next::Said(land_here(state, trace_id, position, 0, grid));
        }
        ReviewCommand::ReviewEndOfLine => {
            // The line's last character, its line break when it has one, as
            // in NVDA.
            let last = text::characters(&line.text, grid)
                .last()
                .map_or(0, |range| range.start);
            return Next::Said(land_here(state, trace_id, position, last, grid));
        }
        ReviewCommand::ReviewPreviousCharacter => {
            let text_width = text::column_of(content, content.len(), true);
            if grid && position.offset >= content.len() && position.column > text_width {
                // Past the end of a terminal row's text: the blank cell to
                // the left.
                return Next::Said(move_column(
                    state,
                    trace_id,
                    position,
                    position.column.saturating_sub(1),
                    grid,
                ));
            }
            return Next::Said(
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
                    return Next::Said(edge(trace_id, Message::Right, character, language));
                }
                return Next::Said(move_column(state, trace_id, position, next, grid));
            }
            return Next::Said(
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
                return Next::Said(vec![speak(
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
                return Next::Said(vec![speak(
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
            state.start_marker = Some(marker(
                state,
                node,
                Marked {
                    at: cursor,
                    after: after_cursor(),
                    selected_to: None,
                },
            ));
            return Next::Said(vec![speak(trace_id, message(Message::StartMarked))]);
        }
        ReviewCommand::MoveToStartMarker => match marker_in(state, node) {
            Err(effect) => return Next::Said(vec![speak(trace_id, effect)]),
            Ok(Marked { at, .. }) => read_line(
                None,
                TextPoint::At(at),
                landing(LandingPlace::Point, TextUnit::Character, None),
            ),
        },
        ReviewCommand::SelectThenCopy => {
            let Marked {
                at,
                after,
                selected_to,
            } = match marker_in(state, node) {
                Err(effect) => return Next::Said(vec![speak(trace_id, effect)]),
                Ok(marker) => marker,
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
                return Next::Said(vec![speak(trace_id, message(Message::PressTwiceToCopy))]);
            }
            if repeat == 0 {
                // The second press copies the range this one selects, as
                // NVDA's does, wherever the review cursor has gone since:
                // the caret the selection moved may have taken it along.
                state.start_marker = Some(marker(
                    state,
                    node,
                    Marked {
                        at,
                        after,
                        selected_to: Some((cursor, after_cursor())),
                    },
                ));
                let (start, end, through) = range((cursor, after_cursor()));
                (
                    TextOp::Select {
                        start,
                        end,
                        through,
                    },
                    TextFollowUp::Selected,
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
                    TextFollowUp::Copy,
                )
            }
        }
        ReviewCommand::ReportReviewLocation => (
            TextOp::Location(TextPoint::At(cursor)),
            TextFollowUp::Location,
        ),
        ReviewCommand::SayAllFromReview => {
            return Next::SayAll(node, TextPoint::At(point_of(position)));
        }
        _ => return Next::Said(Vec::new()),
    };
    Next::Text(node, op, then)
}

/// Whether the navigator's text is reviewed flat, the text a press on it
/// runs over.
fn reviewing_flat(state: &SrState) -> bool {
    state
        .navigator
        .as_ref()
        .is_some_and(|navigator| matches!(navigator.text, ReviewText::Flat(_)))
}

/// The start marker at `marked` in `node`'s text, positions the press made
/// in the navigator's text: in flat text, each its byte offset in the
/// whole flat text, which is no anchor of the provider's.
fn marker(state: &SrState, node: NodeId, marked: Marked<TextPosition>) -> StartMarker {
    let ends = if reviewing_flat(state) {
        MarkerEnds::Flat(marked.map(flat_place))
    } else {
        MarkerEnds::Text(marked)
    };
    StartMarker { node, ends }
}

/// The start marker's places, when it is in `node`'s text as the press
/// reviews it, flat or not, as positions in that text; otherwise what to
/// say: "No start marker set" whether there is none or it is in another
/// object, which keeps it for when the navigator returns
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
fn marker_in(state: &SrState, node: NodeId) -> Result<Marked<TextPosition>, Vec<UtteranceSegment>> {
    let ends = state
        .start_marker
        .filter(|start| start.node == node)
        .map(|start| start.ends);
    match (ends, reviewing_flat(state)) {
        (Some(MarkerEnds::Text(places)), false) => Ok(places),
        (Some(MarkerEnds::Flat(places)), true) => Ok(places.map(flat_position)),
        // None, or a marker in the other kind of text, which a press on
        // this one cannot place.
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

/// Sets the navigator's review position: in flat text its offset in the
/// whole text, the line's anchor being where the line starts there.
fn set_position(state: &mut SrState, position: ReviewPosition) {
    if let Some(navigator) = state.navigator.as_mut() {
        navigator.text = match navigator.text {
            ReviewText::Flat(_) => ReviewText::Flat(FlatPosition {
                offset: flat_start(&position.line).saturating_add(position.offset),
                column: position.column,
            }),
            _ => ReviewText::At(position),
        };
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

/// What a press does with `reply`, the answer to its read of `node`'s text
/// followed up by `then`.
pub(crate) fn answered(
    state: &mut SrState,
    trace_id: TraceId,
    (node, press): (NodeId, ReviewPress),
    then: TextFollowUp,
    reply: TextReply,
) -> Next {
    match (then, reply) {
        // A dead object: one rule at the boundary, the press run on empty
        // flat text. A copy clears its marker as it does for any answer,
        // and says nothing.
        (then @ TextFollowUp::Copy, TextReply::Gone) => reply_to(
            state,
            trace_id,
            press,
            PendingText { node, then },
            TextReply::Gone,
        ),
        (_, TextReply::Gone) => gone(state),
        (_, TextReply::Unanswered) => Next::Said(Vec::new()),
        (then, reply) => reply_to(state, trace_id, press, PendingText { node, then }, reply),
    }
}

/// The navigator object gone: it reviews as empty flat text, the review
/// cursor at its start, column 0.
fn gone(state: &mut SrState) -> Next {
    if let Some(navigator) = state.navigator.as_mut() {
        navigator.text = ReviewText::Flat(FlatPosition::default());
    }
    Next::Flat(String::new())
}

/// A live read of the navigator object answered: the object becomes the
/// navigator's, and its flat text is what the press reviews. The
/// description, read whatever the theme reports, is kept only where the
/// theme reports it.
fn object_read(state: &mut SrState, mut live: verbatim_model::NodeSnapshot) -> String {
    let text = flat_text(&live);
    if crate::details::node_set(state, &[verbatim_model::property::PropertyId::Description])
        .is_empty()
    {
        live.details.description = None;
    }
    if let Some(navigator) = state.navigator.as_mut()
        // The desktop window keeps the name Core gives it, which a live
        // read of the window does not carry.
        && !navigator.object.details.desktop_window
    {
        navigator.object = live;
    }
    text
}

/// The flat text of `object`, NVDA's basic text: its name, value and
/// description, each one that is not empty or only white space, joined by
/// spaces (`NVDAObjects/__init__.py` 1450 to 1463).
pub(crate) fn flat_text(object: &verbatim_model::NodeSnapshot) -> String {
    [
        object.name.as_deref(),
        object.value.as_deref(),
        object.details.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.chars().all(char::is_whitespace))
    .collect::<Vec<_>>()
    .join(" ")
}

/// A press on flat text `text` at its turn: the review cursor at its flat
/// position, kept where it is still a character boundary of the text and
/// otherwise at the start, on the line there, and the press run there.
fn on_flat(state: &mut SrState, press: ReviewPress, text: &str) -> Next {
    let Some(navigator) = state.navigator.as_mut() else {
        return Next::Said(Vec::new());
    };
    let node = navigator.object.id;
    let at = match navigator.text {
        ReviewText::Flat(at) if at.offset <= text.len() && text.is_char_boundary(at.offset) => at,
        _ => FlatPosition::default(),
    };
    navigator.text = ReviewText::Flat(at);
    let line = Arc::new(flat_line(text, at.offset));
    let position = ReviewPosition {
        offset: at.offset.saturating_sub(flat_start(&line)),
        column: at.column,
        line,
    };
    execute(state, press.trace_id, press, node, &position)
}

/// Where `line`, a line of flat text, starts in the text: its anchor.
fn flat_start(line: &verbatim_model::TextChunk) -> usize {
    usize::try_from(line.start.0).unwrap_or(usize::MAX)
}

/// The byte offset in flat `text` of `point`, a position Core made in it
/// (its line's start plus its offset), the text's start or its end; held
/// across presses, it is moved back onto the text read now.
fn flat_offset(text: &str, point: TextPoint) -> usize {
    let offset = match point {
        TextPoint::At(position) => flat_place(position),
        TextPoint::End => text.len(),
        _ => 0,
    };
    text::boundary(text, offset)
}

/// The byte offset in the whole flat text of `position`, a position Core
/// made in it: its line's start plus its offset.
fn flat_place(position: TextPosition) -> usize {
    usize::try_from(position.anchor.0)
        .unwrap_or(usize::MAX)
        .saturating_add(text::byte_offset(position.offset))
}

/// The position Core makes in flat text at byte `offset` of the whole of
/// it, which [`flat_offset`] reads back.
fn flat_position(offset: usize) -> TextPosition {
    TextPosition::at(verbatim_model::TextAnchor(
        u64::try_from(offset).unwrap_or(u64::MAX),
    ))
}

/// The index of the line of `lines` holding byte `offset`.
fn flat_line_index(lines: &[std::ops::Range<usize>], offset: usize) -> usize {
    lines
        .iter()
        .rposition(|line| line.start <= offset)
        .unwrap_or_default()
}

/// The line of flat `text` holding byte `offset`, as the outpost would read
/// it: its text with its line break, its anchor the byte offset where it
/// starts in `text`, the offset in it, and whether it is the first or the
/// last.
fn flat_line(text: &str, offset: usize) -> verbatim_model::TextChunk {
    let lines = verbatim_text::lines(text);
    flat_chunk(text, &lines, flat_line_index(&lines, offset), offset)
}

/// Line `index` of flat `text`, whose `lines` they are, with `offset` in
/// it where it falls there, else at its end.
fn flat_chunk(
    text: &str,
    lines: &[std::ops::Range<usize>],
    index: usize,
    offset: usize,
) -> verbatim_model::TextChunk {
    let start = lines.get(index).map_or(0, |line| line.start);
    let end = lines
        .get(index.saturating_add(1))
        .map_or(text.len(), |next| next.start);
    let line = proven::text_in(text, start..end);
    verbatim_model::TextChunk {
        unit: TextUnit::Line,
        text: line.to_owned(),
        start: verbatim_model::TextAnchor(u64::try_from(start).unwrap_or(u64::MAX)),
        offset: u32::try_from(offset.saturating_sub(start).min(line.len())).unwrap_or(u32::MAX),
        languages: Vec::new(),
        first: index == 0,
        last: index.saturating_add(1) >= lines.len(),
        truncated: false,
        formats: Vec::new(),
    }
}

/// The answer to `op` over flat `text`, as Core answers a review press's
/// request on an object with no text interface: a line read at a point,
/// the start or the end, moving by line or not, and a range read for a
/// copy. A move that cannot move answers the review cursor's own line.
/// Flat text has no screen place, selection or caret, so a selection, a
/// location or a selection point is unsupported, and it has no pages.
fn flat_answer(text: &str, op: &TextOp) -> TextReply {
    match op {
        TextOp::Read(TextRead {
            at: at @ (TextPoint::At(_) | TextPoint::Start | TextPoint::End),
            movement,
            unit: TextUnit::Line,
        }) => {
            let offset = flat_offset(text, *at);
            let lines = verbatim_text::lines(text);
            let index = flat_line_index(&lines, offset);
            let target = match movement {
                None => index,
                Some(TextMovement {
                    unit: TextUnit::Line,
                    count,
                }) => index
                    .checked_add_signed(isize::try_from(*count).unwrap_or_default())
                    .filter(|target| *target < lines.len())
                    .unwrap_or(index),
                Some(TextMovement { unit, .. }) => return TextReply::UnsupportedUnit(*unit),
            };
            if target == index {
                return TextReply::Read {
                    moved: 0,
                    chunk: flat_chunk(text, &lines, index, offset),
                };
            }
            let moved = i32::try_from(target.abs_diff(index)).unwrap_or(i32::MAX);
            TextReply::Read {
                moved: if target < index {
                    moved.saturating_neg()
                } else {
                    moved
                },
                chunk: flat_chunk(text, &lines, target, 0),
            }
        }
        TextOp::Read(TextRead { unit, .. }) if *unit != TextUnit::Line => {
            TextReply::UnsupportedUnit(*unit)
        }
        TextOp::ReadRange {
            start,
            end,
            through,
        } => {
            let (start, end) = (flat_offset(text, *start), flat_offset(text, *end));
            let (from, to) = match through {
                Some(through) if end < start => (end, flat_offset(text, through.start)),
                Some(through) => (start, flat_offset(text, through.end)),
                None => (start.min(end), start.max(end)),
            };
            TextReply::Range {
                text: proven::text_in(text, from..to.max(from)).to_owned(),
                truncated: false,
            }
        }
        _ => TextReply::Unsupported,
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

/// The answer to a press's text read.
fn reply_to(
    state: &mut SrState,
    trace_id: TraceId,
    press: ReviewPress,
    pending: PendingText,
    reply: TextReply,
) -> Next {
    let on_navigator = state
        .navigator
        .as_ref()
        .is_some_and(|navigator| navigator.object.id == pending.node);
    let said = match (pending.then, reply) {
        (TextFollowUp::Location, reply) => location(trace_id, &reply),
        // A copy clears the start marker it copied from whatever it found,
        // so a further press has none, but keeps a marker set while the
        // read was in flight; a range with nothing in it copies nothing and
        // says nothing (`docs/parity/review.md`, "Select then copy from
        // the review cursor").
        (TextFollowUp::Copy, reply) => {
            state.start_marker = None;
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
        // The selection the command made is spoken as a caret key's is,
        // "selected" and "unselected" from the same comparison
        // (`docs/parity/review.md`, "Select then copy from the review
        // cursor").
        (TextFollowUp::Selected, TextReply::Selected(changes)) => {
            speak_each(trace_id, selection_segments(&changes, None))
        }
        // The text cannot be selected: the second press still copies it.
        (TextFollowUp::Selected, TextReply::Unsupported | TextReply::NoText) => {
            vec![speak(trace_id, message(Message::CannotSelect))]
        }
        (_, TextReply::Unsupported | TextReply::UnsupportedUnit(_)) => {
            vec![speak(trace_id, message(Message::NotSupported))]
        }
        (_, _) if !on_navigator => Vec::new(),
        (
            TextFollowUp::Seed {
                command, repeat, ..
            },
            TextReply::Read { chunk, .. },
        ) => {
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
            let press = ReviewPress {
                trace_id: press.trace_id,
                command,
                repeat,
            };
            return execute(state, trace_id, press, pending.node, &position);
        }
        (TextFollowUp::Seed { .. }, TextReply::NoText) => {
            if let Some(navigator) = state.navigator.as_mut() {
                navigator.text = ReviewText::Flat(FlatPosition::default());
            }
            return Next::ReadObject;
        }
        (TextFollowUp::Land(landing), TextReply::Read { moved, chunk }) => {
            land(state, trace_id, landing, moved, chunk)
        }
        (
            TextFollowUp::Land(Landing { command, .. })
            | TextFollowUp::Seed {
                command,
                at_caret: false,
                ..
            },
            TextReply::AnchorLost,
        ) => {
            // The position the cursor was at is gone: start again from the
            // caret. The caret's own line lost as it was read has nowhere to
            // start again from, and ends the command saying nothing below,
            // as a read that fails does.
            if let Some(navigator) = state.navigator.as_mut() {
                navigator.text = ReviewText::Unknown;
            }
            let press = ReviewPress {
                trace_id: press.trace_id,
                command,
                repeat: 0,
            };
            return seed(press, pending.node, TextPoint::Caret);
        }
        _ => Vec::new(),
    };
    Next::Said(said)
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
        let position = match state.navigator.as_ref().map(|navigator| &navigator.text) {
            Some(ReviewText::At(position)) => position.clone(),
            // Flat text answers a move that could not move with the
            // review cursor's own line.
            Some(ReviewText::Flat(at)) => ReviewPosition {
                offset: text::byte_offset(chunk.offset),
                column: at.column,
                line: Arc::new(chunk),
            },
            _ => return Vec::new(),
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
            let offset = text::byte_offset(line.offset).min(content.len());
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

#[cfg(test)]
#[path = "review_text_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "flat_review_tests.rs"]
mod flat_tests;
