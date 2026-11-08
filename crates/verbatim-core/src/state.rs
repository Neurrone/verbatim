//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the focus,
//! the attention record, the navigator, and the one object-navigation query
//! still in flight. The reducer changes it in place, one input at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use verbatim_model::{
    CaretKey, Fetches, HeldAnchors, HookForeground, NodeId, NodeSnapshot, OutpostId, Pid, QueryId,
    ReaderSettings, ReviewCommand, Selection, SpeechMark, StateSet, TextAttributes, TextChunk,
    TextPosition, TextUnit, TraceId, WindowFacts,
};

use crate::foreground::Foreground;
use crate::queries::Then;

/// The focused node and what the reducer knows about where it sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FocusContext {
    /// The application the focus belongs to.
    pub(crate) source: Pid,
    /// The focus event's window facts, when it carried any. The top-level
    /// window is what a foreground change is compared against.
    pub(crate) window: Option<WindowFacts>,
    /// The focused node as last read: the reading Core holds, which review
    /// and reporting the focus read, and which a repeat of the focus updates
    /// with what it carries. Changes are compared with
    /// [`spoken`](Self::spoken), not with this.
    pub(crate) snapshot: NodeSnapshot,
    /// The focused node's ancestors, outermost first, as the `FocusChanged`
    /// event carried them. The next focus change diffs its own ancestry
    /// against these and the focus itself to announce only newly entered
    /// containers (NVDA's focus-ancestry behavior); empty when the outpost's
    /// walk found nothing or timed out.
    ///
    /// The chain is the part of the state that grows with the application,
    /// so it is shared rather than owned: cloning the state, as the flight
    /// recorder does at each checkpoint, copies a pointer, not the chain.
    /// A focus change replaces the whole chain, never edits it.
    pub(crate) ancestors: Arc<[NodeSnapshot]>,
    /// The most recently announced selected item within the focused
    /// container — seeded by the focus event's own `selected_child`, then
    /// advanced by each announced `SelectionChanged` — so a selection event
    /// for the item that was just spoken is not spoken twice.
    pub(crate) last_selection: Option<NodeId>,
    /// False once the outpost that issued these node ids has ended. The
    /// copied data stays, so a replacement outpost's report of the same
    /// focus can be taken silently (`docs/parity/focus-events.md`, "Recovery after an
    /// outpost is replaced"), but the ids name nothing any more.
    pub(crate) alive: bool,
    /// The focused node reported itself focused when it became the focus,
    /// so a later state set without the focused state means the focus has
    /// left it before the next focus event arrived (`docs/parity/changes-and-selection.md`,
    /// "State changes after the focus has left").
    #[serde(default)]
    pub(crate) reported_focused: bool,
    /// The trace of the event Core took this focus from; `None` in a
    /// recording made before it was kept.
    #[serde(default)]
    pub(crate) trace_id: Option<TraceId>,
    /// When that event was observed, in microseconds on the run's shared
    /// clock (0 when not known).
    #[serde(default)]
    pub(crate) observed_at_us: u64,
    /// What Core last spoke of the focus and its ancestors. A focus taken
    /// writes it, as does a report that speaks the focus's reading, a name,
    /// value or description change on the focus, a state change on the
    /// focus other than the focused state's loss on a focus that reported
    /// itself focused, and a state change of an ancestor with an entry; a
    /// repeat never does. `None` in a recording made before it was kept,
    /// when the reading stands in ([`FocusContext::spoken`]).
    #[serde(default)]
    pub(crate) spoken: Option<SpokenRecord>,
}

/// What Core last spoke of the focus, against which a change of its name,
/// value, description or states is compared, as NVDA compares a change with
/// the properties it last spoke of the object ("When values and
/// descriptions are spoken" in `docs/nvda/speech.md`; "One judge per fact" in
/// `books/dev/src/reference/adr/0001-focus.md`). A repeat of the
/// focus updates the reading Core holds, never this, so a change it carried
/// in is still spoken when its own event comes.
///
/// Of the ancestors only the states are kept, the one change Core speaks of
/// an ancestor. An ancestor the previous focus line held, the previous
/// focus among them, keeps its entry from it, so a state change of a focus
/// just left that arrives after the new focus's report, which already
/// carries the new state, is still spoken. An ancestor with no entry, read
/// by a repeat after the focus was taken, is compared with its reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpokenRecord {
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) states: StateSet,
    /// Each ancestor's states as last spoken, outermost first, shared as
    /// the chain is, so a checkpoint copies a pointer.
    pub(crate) ancestors: Arc<[(NodeId, StateSet)]>,
}

impl SpokenRecord {
    /// The record of `node`, just spoken or taken as the focus, with the
    /// ancestors' entries given.
    pub(crate) fn of(node: &NodeSnapshot, ancestors: Arc<[(NodeId, StateSet)]>) -> Self {
        Self {
            name: node.name.clone(),
            value: node.value.clone(),
            description: node.details.description.clone(),
            states: node.states,
            ancestors,
        }
    }
}

impl FocusContext {
    /// The spoken record, made from the reading the first time it is needed
    /// when a recording made before it was kept left it out.
    pub(crate) fn spoken(&mut self) -> &mut SpokenRecord {
        let (snapshot, ancestors) = (&self.snapshot, &self.ancestors);
        self.spoken.get_or_insert_with(|| {
            SpokenRecord::of(
                snapshot,
                ancestors
                    .iter()
                    .map(|ancestor| (ancestor.id, ancestor.states))
                    .collect(),
            )
        })
    }

    /// The states last spoken of `node`, the focus or one of its ancestors,
    /// or its reading's when nothing was recorded for it; `None` for a node
    /// not in the focus line.
    pub(crate) fn spoken_states(&self, node: NodeId) -> Option<StateSet> {
        if node == self.snapshot.id {
            return Some(
                self.spoken
                    .as_ref()
                    .map_or(self.snapshot.states, |spoken| spoken.states),
            );
        }
        self.spoken
            .as_ref()
            .and_then(|spoken| spoken.ancestors.iter().find(|(id, _)| *id == node))
            .map(|(_, states)| *states)
            .or_else(|| {
                self.ancestors
                    .iter()
                    .find(|ancestor| ancestor.id == node)
                    .map(|ancestor| ancestor.states)
            })
    }
}

/// The navigator object and its review cursor (roadmap M3): the object
/// object-navigation commands walk, independent of keyboard focus. It
/// follows focus by default (every focus change resets it), and the
/// "to focus" command snaps it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Navigator {
    pub(crate) object: NodeSnapshot,
    /// The review cursor's character offset into the object's review text
    /// (see `review::text_of`), used while the object is reviewed as flat
    /// text. Always a valid boundary within that text.
    pub(crate) review_offset: usize,
    /// How the review cursor reviews the object's text (milestone M4).
    #[serde(default)]
    pub(crate) text: ReviewText,
}

impl Navigator {
    /// A navigator on `object`, its review cursor at the start, how its text
    /// is reviewed not yet known.
    pub(crate) fn on(object: NodeSnapshot) -> Self {
        Self {
            object,
            review_offset: 0,
            text: ReviewText::Unknown,
        }
    }
}

/// A unit of text the outpost sent, shared so the state's clone, which the
/// flight recorder takes at each checkpoint, copies a pointer and not the
/// text (`books/dev/src/explanation/architecture.md`, section 2).
pub(crate) type SharedChunk = Arc<TextChunk>;

/// How the review cursor reviews the navigator object's text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReviewText {
    /// Not known yet: the first review command finds out, reading the line
    /// at the caret through the text protocol for an object that may have
    /// text, and reviewing the flat text of any other.
    #[default]
    Unknown,
    /// The object has no text interface: its value or name is reviewed as
    /// flat text (`review`), as NVDA's object review falls back to.
    Flat,
    /// A position in the object's text, with the line it is on.
    At(ReviewPosition),
    /// At the caret, following it, and not moved away since. Core's copy of
    /// the caret can lag behind the application's, whose caret event for a
    /// key may reach Core after a review command pressed later, so the next
    /// review command reads the line at the caret as the outpost finds it
    /// (`TextPoint::Caret`) and starts from there, as a caret key speaks
    /// what the outpost reads after its wait.
    Caret,
    /// A position in the object's text whose line has not been read, as
    /// say-all from the review cursor leaves it; the next review command
    /// reads the line there first.
    Point(TextPosition),
}

/// The review cursor in a text: the line it is on, its byte offset into
/// that line's text, and its column (`docs/parity/review.md`, "Review cursor
/// columns"). In a
/// terminal the column is a cell column, exact, and may lie past the end
/// of the line's text, where the cell is blank; elsewhere it is the
/// grapheme column the cursor would like to be at, remembered across
/// shorter lines, with `offset` where it really is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewPosition {
    pub(crate) line: SharedChunk,
    pub(crate) offset: usize,
    pub(crate) column: usize,
}

/// Core's copy of the focus's caret: the line it is on, with the caret at
/// the chunk's offset, and the selection (milestone M4). Kept current by
/// caret events and caret key replies, so a Backspace knows what it
/// deleted and the review cursor can follow the caret without a round
/// trip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaretContext {
    pub(crate) node: NodeId,
    pub(crate) line: SharedChunk,
    pub(crate) selection: Option<Selection>,
    /// The line the selection starts in, when it starts on a line before
    /// the caret's (`CaretReport::selection_line`).
    #[serde(default)]
    pub(crate) selection_line: Option<TextChunk>,
    /// The line break last seen ending a line of the node's text, such as
    /// a carriage return in Windows 11 Notepad or a carriage return and line
    /// feed in a standard edit control: what a Backspace at the start of a
    /// line deletes, since a text uses one kind of line break throughout,
    /// when the caret's own line has none (the last line).
    #[serde(default)]
    pub(crate) line_break: Option<String>,
    /// When the report this caret came from was observed or read, in
    /// milliseconds, 0 when unknown. A timed report observed before this one, arriving later,
    /// does not replace it, so the caret is always the newest known.
    #[serde(default)]
    pub(crate) observed_at_ms: u64,
}

impl CaretContext {
    /// Where the caret is.
    pub(crate) fn caret(&self) -> TextPosition {
        TextPosition {
            anchor: self.line.start,
            offset: self.line.offset,
        }
    }
}

/// How many of the focus's timed caret reports Core keeps, as the outpost
/// keeps its own: enough to reach back past the reports a fast typist's
/// keys raise while one key is on its way to Core.
pub(crate) const CARET_HISTORY: usize = 8;

/// A report of the focus's caret with the time it was observed, in
/// milliseconds to compare with a caret key's `pressed_at_ms`: an event's
/// `observed_at_us` in whole milliseconds, or the time the outpost read a
/// caret key's reply. A caret key's own path to Core (the hook, then the
/// application, then Core) can be slower than the application's (the
/// application, then the outpost, then Core), so the report Core holds when
/// a key arrives may already show what the key did. The newest report observed before the
/// key was pressed is the caret the key found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TimedCaret {
    pub(crate) observed_at_ms: u64,
    pub(crate) node: NodeId,
    pub(crate) line: SharedChunk,
    pub(crate) selection: Option<Selection>,
    /// The line the selection starts in, when it starts on a line before
    /// the caret's (`CaretReport::selection_line`).
    #[serde(default)]
    pub(crate) selection_line: Option<TextChunk>,
}

/// A caret key passed to the application, waiting for the outpost's
/// evidence of what it did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingCaret {
    pub(crate) node: NodeId,
    pub(crate) key: CaretKey,
    /// What a Backspace deleted, worked out from the caret before the key:
    /// the prediction, used only when the line after the key cannot be
    /// compared with the line before (`before`).
    pub(crate) deleted: Option<String>,
    /// For a Backspace, the caret's line before the key and the caret's
    /// offset in it, which the line after the key is compared with to find
    /// what the key really removed.
    #[serde(default)]
    pub(crate) before: Option<(String, u32)>,
    /// When the next key after this one was pressed (Unix milliseconds),
    /// once one has been: an answer read at or after then may show that
    /// key's effect, not this one's, so it only keeps the caret current.
    #[serde(default)]
    pub(crate) later_key_at_ms: Option<u64>,
    /// Whether text was selected when the key was pressed, by the caret
    /// Core found the key at: a Backspace over a selection says what was
    /// before the caret, and a key outside the caret table whose answer
    /// shows that selection removed says only the selection's change.
    #[serde(default)]
    pub(crate) selected_before: bool,
    /// The key deletes backward over a selection that starts on an
    /// earlier line than the caret's, so what comes before its start is
    /// not known and the key says only the selection's change.
    #[serde(default)]
    pub(crate) before_unknown: bool,
    /// The selection changes of caret reports that came while the key
    /// waited for its answer, as they are spoken: the outpost compared them
    /// with the selection kept for the node, which the key's answer then
    /// compares with no more, so they are said after the key's own words,
    /// as NVDA says its caret event's words after the key's script's
    /// (`editing::selection_changed`).
    #[serde(default)]
    pub(crate) heard: Vec<verbatim_model::UtteranceSegment>,
}

/// A caret key the keyboard hook held back from the application
/// (`Input::HeldKey`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HeldKey {
    /// The key's trace, which also names it to the shell that passes it on.
    pub(crate) trace_id: TraceId,
    pub(crate) key: CaretKey,
    /// When the hook saw it, on the outposts' clock; 0 when unknown.
    pub(crate) pressed_at_ms: u64,
}

/// A held key whose caret is being read before it is passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HeldRead {
    pub(crate) node: NodeId,
    pub(crate) key: HeldKey,
}

/// A key waiting behind a held key whose caret is being read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WaitingKey {
    /// A caret key held back too, whose caret is read in its turn.
    Held(HeldKey),
    /// A key the hook deferred behind the held key, which reaches the
    /// application after it: a caret key judged by where the caret lands,
    /// text typed, or a key that clears a terminal's line. Handled in its
    /// turn, as the input it came as.
    Later(Box<verbatim_model::Input>),
    /// A review command or a script (`Input::Command`, `Input::Script`)
    /// pressed while a held key waited: run once the key ahead of it has
    /// been passed on and its watch answered, or the bound since the pass
    /// has run out, as NVDA runs scripts one at a time in the order
    /// queued (`books/dev/src/explanation/design/ownership.md`).
    Command(Box<verbatim_model::Input>),
}

/// The caret keys the hook held back and pressed while one's caret is
/// read, each handled in the order pressed (`books/dev/src/explanation/design/key-holding.md`,
/// "Keys behind a held key"); the read itself is a query
/// ([`crate::queries::Then::HeldRead`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HeldKeys {
    pub(crate) waiting: std::collections::VecDeque<WaitingKey>,
    /// When the last held key was passed on, on the outposts' clock: a key
    /// behind it reaches the application no earlier.
    #[serde(default)]
    pub(crate) passed_at_ms: u64,
    /// The watch of the held key last passed on, until it is answered: a
    /// held key behind it has its caret read only then.
    #[serde(default)]
    pub(crate) awaiting_watch: Option<QueryId>,
    /// The held key whose one deadline the shell keeps
    /// ([`verbatim_model::Effect::BoundHeldKey`]): for its wait for
    /// `awaiting_watch`, or for the read of its caret.
    #[serde(default)]
    pub(crate) bounded: Option<TraceId>,
}

impl SrState {
    /// Whether a key pressed now waits behind a held key: one's caret is
    /// being read, or keys wait behind it.
    pub(crate) fn held_busy(&self) -> bool {
        self.reading_held().is_some() || !self.held_keys.waiting.is_empty()
    }

    /// The held key whose caret is being read, and its query.
    pub(crate) fn reading_held(&self) -> Option<(QueryId, HeldRead)> {
        self.queries
            .find(|then| matches!(then, Then::HeldRead(_)))
            .and_then(|(query_id, then)| match then {
                Then::HeldRead(reading) => Some((query_id, *reading)),
                _ => None,
            })
    }
}

/// A text request a review or text command made, and what to do with its
/// answer. A newer command's request supersedes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingText {
    pub(crate) node: NodeId,
    pub(crate) then: TextFollowUp,
    /// The object whose text a navigator follow-up reads when it is
    /// neither the navigator object nor the focus: Verbatim+Tab's answer,
    /// which Core never takes (`crate::report_focus`).
    #[serde(default)]
    pub(crate) about: Option<Box<NodeSnapshot>>,
}

/// What to do with the answer to a [`PendingText`] request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TextFollowUp {
    /// The review cursor's line was read: place the cursor at the point
    /// read, then run the command.
    Seed { command: ReviewCommand, repeat: u8 },
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
    /// The selected text of the navigator object was read: use it, or with
    /// nothing selected read the caret's line.
    NavigatorSelection(NavigatorRead),
    /// The caret's line in the navigator object was read: use it.
    NavigatorLine(NavigatorRead),
}

/// What the navigator object's text is read for: its announcement, or
/// reporting the current object a second time (spelling) or a third
/// (copying), which use the name followed by the text, as NVDA's do
/// (`docs/nvda/speech.md`, "What an object with text says").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NavigatorRead {
    /// Said after the rest of the announcement, in place of the value.
    Announce,
    /// Spelled with the name, character by character.
    Spell,
    /// Copied to the clipboard with the name.
    Copy,
}

/// Where the review cursor lands on a line it moved to, and what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Landing {
    /// Where on the line.
    pub(crate) place: LandingPlace,
    /// The unit spoken there.
    pub(crate) speak: TextUnit,
    /// What to say, before the current unit, when the movement could not
    /// move ("Top", "Bottom").
    pub(crate) edge: Option<verbatim_model::Message>,
    /// The command that moved, for re-running it from the caret if the
    /// review position was lost.
    pub(crate) command: ReviewCommand,
}

/// Where on a line the review cursor lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LandingPlace {
    /// At the remembered column.
    Column(usize),
    /// At the line's start.
    Start,
    /// On the first word.
    FirstWord,
    /// On the last word.
    LastWord,
    /// At the point read.
    Point,
    /// On the character before the point read (the end of a selection).
    BeforePoint,
}

/// The select-then-copy start marker: where in which node's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum StartMarker {
    /// A position in a node's text, `at`, and `after`, just past the
    /// character there; and the review cursor's character, by the same two
    /// positions, that a select then copy's first press selected through,
    /// which its second press copies (`docs/parity/review.md`, "Select then
    /// copy from the review cursor").
    Text {
        node: NodeId,
        at: TextPosition,
        after: TextPosition,
        selected_to: Option<(TextPosition, TextPosition)>,
    },
    /// A byte offset into a node's flat text, and the review cursor's
    /// offset when a select then copy's first press built its range, which
    /// flat text cannot select but its second press copies.
    Flat {
        node: NodeId,
        offset: usize,
        selected_to: Option<usize>,
    },
}

impl StartMarker {
    pub(crate) fn node(self) -> NodeId {
        match self {
            Self::Text { node, .. } | Self::Flat { node, .. } => node,
        }
    }
}

/// A piece of text say-all read and has not yet handed to speech: the
/// chunk it is in, shared with the chunk's other pieces, its byte range of
/// the chunk's text, and whether it starts a unit read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BufferedPiece {
    pub(crate) chunk: SharedChunk,
    pub(crate) start: u32,
    pub(crate) end: u32,
    /// Whether the piece starts a unit read (a line or a sentence), so
    /// playback reaching it moves the cursor there; not for the rest of a
    /// unit held back after its last sentence end.
    pub(crate) marked: bool,
}

/// The start of a unit say-all read: the chunk it is in and its byte
/// offset there, which becomes Core's caret when say-all moves the caret
/// there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UnitStart {
    pub(crate) chunk: SharedChunk,
    pub(crate) offset: u32,
}

impl UnitStart {
    /// Where the unit starts.
    pub(crate) fn position(&self) -> TextPosition {
        TextPosition {
            anchor: self.chunk.start,
            offset: self.offset,
        }
    }
}

/// An index mark say-all handed to speech and playback has not reached
/// yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueuedMark {
    pub(crate) mark: SpeechMark,
    /// Where the cursor moves when playback reaches the mark: the start of
    /// the unit read that follows it, or `None` for the mark opening an
    /// utterance that starts with the rest of a unit already reached.
    pub(crate) unit: Option<UnitStart>,
    /// Whether the mark opens its utterance.
    pub(crate) opens: bool,
}

impl BufferedPiece {
    /// The piece's text.
    pub(crate) fn text(&self) -> &str {
        // A piece is cut from its chunk's text at character boundaries
        // (`say_all`'s pieces), so the range is always found.
        let range = crate::text::byte_offset(self.start)..crate::text::byte_offset(self.end);
        crate::proven::text_in(&self.chunk.text, range)
    }
}

/// A say-all in progress (`docs/nvda/speech.md`, "Say-all").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SayAll {
    /// The node being read.
    pub(crate) node: NodeId,
    /// Whether reading moves the caret (say-all from the caret) or the
    /// review cursor.
    pub(crate) moves_caret: bool,
    /// Where reading started.
    pub(crate) start: verbatim_model::TextPoint,
    /// The unit asked for in each read.
    pub(crate) unit: TextUnit,
    /// The start of the last chunk read and its unit, which the next read
    /// moves on from.
    pub(crate) last_chunk: Option<(TextPosition, TextUnit)>,
    /// The marks of utterances handed to speech that playback has not
    /// reached yet, oldest first. At most `say_all::HANDED` of them open
    /// an utterance.
    pub(crate) queued: std::collections::VecDeque<QueuedMark>,
    /// Utterances read and not yet handed to speech, in order, each its
    /// pieces. At most one read-ahead batch's worth
    /// (`verbatim_model::MAX_READ_AHEAD_TEXT`), with what was held back.
    #[serde(default)]
    pub(crate) buffer: std::collections::VecDeque<Vec<BufferedPiece>>,
    /// Text read with no sentence end after it, held back to be spoken
    /// with what follows it (`docs/nvda/speech.md`, "Say-all speaks
    /// without pauses").
    #[serde(default)]
    pub(crate) held: Vec<BufferedPiece>,
    /// How many units in a row were read with no sentence end, blank ones
    /// included: at `say_all::MAX_HELD`, what is held back is spoken.
    #[serde(default)]
    pub(crate) held_units: usize,
    /// The trace of the latest read, which pieces handed out later carry.
    #[serde(default)]
    pub(crate) trace: Option<TraceId>,
    /// The document's end has been read.
    pub(crate) finished: bool,
    /// Whether the display was asked to stay on.
    pub(crate) display_held: bool,
}

/// Reducer state: focus, attention, navigator, and Core's queries.
///
/// The flight recorder clones the state at each checkpoint, once per
/// segment of its window, and serializes the clone into a dump, so a
/// recorded window replays from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrState {
    /// Core's focus: the focus object and the last queued focus
    /// (`crate::focus_queue`).
    #[serde(default)]
    pub(crate) focus: crate::focus_queue::FocusState,
    /// The foreground record: attention, the foreground window and object,
    /// and the stamps that order focuses, under the keys the state held
    /// before the record existed.
    #[serde(flatten)]
    pub(crate) foreground: Foreground,
    /// The newest focus group whose report Core took: the generation of
    /// the report's node, the group's number and the report's rank. An
    /// older report of the same group is not taken
    /// ([`verbatim_model::FocusGroup`]). Only the newest group taken is
    /// kept: when another outpost's report is taken between two reports of
    /// one group, the older entry was observed before that report, and the
    /// stale rule drops it instead.
    #[serde(default)]
    pub(crate) group_taken: Option<(OutpostId, verbatim_model::FocusGroup)>,
    /// The live re-reads Core waits for, one per focus reader, and how
    /// many it has asked for (`crate::live_reread`).
    #[serde(default)]
    pub(crate) live_rereads: crate::live_reread::LiveRereads,
    /// Verbatim+Tab's press waiting for its live query's answer
    /// (`crate::report_focus`).
    #[serde(default)]
    pub(crate) report_press: Option<crate::report_focus::WaitingPress>,
    /// The virtual desktop's name, kept until the next focus Core takes
    /// or its due time (`crate::desktop`).
    #[serde(default)]
    pub(crate) desktop_name: Option<String>,
    /// Core's queries not yet ended, each from its request to its one
    /// ending (`crate::queries`).
    #[serde(default)]
    pub(crate) queries: crate::queries::Queries,
    /// The navigator object and review cursor (roadmap M3). `None` until
    /// the first focus lands; from then it tracks focus unless an
    /// object-navigation command moves it away.
    pub(crate) navigator: Option<Navigator>,
    /// The reader settings (milestone M4).
    #[serde(default)]
    pub(crate) settings: ReaderSettings,
    /// The focus's caret, when the focus has text.
    #[serde(default)]
    pub(crate) caret: Option<CaretContext>,
    /// The focus's latest caret reports whose time is known, oldest first,
    /// all for the node of `caret`: up to [`CARET_HISTORY`], held inline so
    /// a checkpoint copies them without allocating. A report whose time is
    /// not known empties it, since its order among them is unknown; a
    /// caret Core moved itself is kept at time 0, before any key
    /// (`editing::caret_moved_by_core`).
    #[serde(default)]
    pub(crate) caret_history: [Option<TimedCaret>; CARET_HISTORY],
    /// The caret keys the keyboard hook held back
    /// (`books/dev/src/explanation/design/key-holding.md`).
    #[serde(default)]
    pub(crate) held_keys: HeldKeys,
    /// The key numbers of what recent user inputs caused, by trace, which
    /// Core stamps their speech with (`crate::keys`).
    #[serde(default)]
    pub(crate) key_origins: crate::keys::KeyOrigins,
    /// The lock keys pressed whose new state is still to be announced, in
    /// the order pressed (`Input::ToggleKeyPressed`), each with when its
    /// state is read, on the run's shared clock.
    #[serde(default)]
    pub(crate) toggle_keys: std::collections::VecDeque<(verbatim_model::LockKey, u64)>,
    /// The select-then-copy start marker.
    #[serde(default)]
    pub(crate) start_marker: Option<StartMarker>,
    /// How many times a start marker has been set: which marker a copy's
    /// read was made for, so its reply clears that one and keeps a marker
    /// set since (`docs/parity/review.md`, "Select then copy from the
    /// review cursor").
    #[serde(default)]
    pub(crate) start_marker_generation: u64,
    /// The say-all in progress.
    #[serde(default)]
    pub(crate) say_all: Option<SayAll>,
    /// The word typed so far, for typed word echo; at most
    /// `editing::MAX_TYPED_WORD` bytes.
    #[serde(default)]
    pub(crate) typed_word: String,
    /// The next index mark number.
    #[serde(default)]
    pub(crate) next_mark: u64,
    /// The details the active theme wants fetched (`Input::Fetches`).
    #[serde(default)]
    pub(crate) fetches: Fetches,
    /// The focused terminal's output still to be spoken (milestone M4 item
    /// 9), bounded by the flood policy's limits.
    #[serde(default)]
    pub(crate) terminal: crate::terminal::TerminalSpeechState,
    /// Core's model of each terminal it reads, the most recently read last,
    /// at most 16 (`crate::terminal_model`): none until an outpost sends a
    /// terminal snapshot.
    #[serde(default)]
    pub(crate) terminals: crate::terminal_model::Terminals,
    /// The number of the last text anchor Core minted for a terminal's
    /// review answers, in a range no outpost mints from
    /// (`crate::terminal_model::TERMINAL_ANCHORS`).
    #[serde(default)]
    pub(crate) next_terminal_anchor: u64,
    /// The formatting last reported in a node's text (milestone M4 item 7):
    /// NVDA's per-object cache, from which only changes are spoken
    /// (`docs/nvda/document-formatting.md`). A new focus starts afresh.
    #[serde(default)]
    pub(crate) reported_format: Option<(NodeId, TextAttributes)>,
    /// The last tree or list item level spoken first, which an item at
    /// the same level speaks last instead ("Where the level goes" in
    /// `docs/nvda/speech.md`).
    #[serde(default)]
    pub(crate) last_tree_level: Option<u32>,
    /// The percentage each progress bar last indicated, in hundredths, by
    /// the centre of its location on the screen, as NVDA remembers it
    /// (`docs/nvda/object-model.md`, "How a progress bar reports its
    /// value"), the most recent last, at most [`PROGRESS_BARS_KEPT`].
    #[serde(default)]
    pub(crate) progress_reported: Vec<((i32, i32), u32)>,
    /// Verbatim is exiting (`Input::Exiting`): no later input does
    /// anything, so nothing is said after the exit sound, as NVDA says
    /// nothing once it has stopped its handlers.
    #[serde(default)]
    pub(crate) exiting: bool,
}

/// How many progress bars' last indications are remembered: more than are
/// ever reporting at once.
pub(crate) const PROGRESS_BARS_KEPT: usize = 32;

impl SrState {
    /// An initial state with no focus, no attention, and nothing pending.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether Verbatim is exiting (`Input::Exiting`), so that nothing more
    /// is said and the exit sound is the last.
    #[must_use]
    pub fn exiting(&self) -> bool {
        self.exiting
    }

    /// Whether a caret key pressed now is held back until the focus's
    /// caret has been read (`books/dev/src/explanation/design/key-holding.md`): the focus has
    /// text, or may have, as for the caret keys whose evidence is watched.
    /// The keyboard hook holds caret keys only while this holds.
    #[must_use]
    pub fn holds_caret_keys(&self) -> bool {
        crate::editing::text_focus(self).is_some()
    }

    /// Whether a key or command waits in Core's held-key line: Verbatim is
    /// not idle while one does.
    #[must_use]
    pub fn holds_inputs_back(&self) -> bool {
        self.held_busy()
    }

    /// How many of Core's queries an outpost has yet to answer that the
    /// idle barrier waits for, with the report queued for a live re-read: a caret
    /// key's watch, which waits for the application, and a request Core
    /// answers from a terminal's memory are not counted.
    #[must_use]
    pub fn outstanding_queries(&self) -> usize {
        self.queries
            .outstanding()
            .saturating_add(self.focus.rereading())
    }

    /// How many live re-reads Core has asked for since it started: each a
    /// focus report set aside because it would have spoken an ancestor its
    /// focus reader took from its ancestry cache (`crate::live_reread`).
    /// The control protocol's status reply carries it.
    #[must_use]
    pub fn live_re_reads(&self) -> u64 {
        self.live_rereads.count()
    }

    /// Whether a lock key's new state is still to be announced: Verbatim is
    /// not idle while one is, and the shell holds the harness's marks of
    /// handled input behind it.
    #[must_use]
    pub fn announces_toggle_key(&self) -> bool {
        !self.toggle_keys.is_empty()
    }

    /// The lock key whose state the shell reads next, and when, on the
    /// run's shared clock: the front of those pressed, read once its time
    /// is past and entered as [`verbatim_model::Input::LockKeyToggled`],
    /// which takes it from the front. A view of the state, read after each
    /// step, so the shell keeps no copy.
    #[must_use]
    pub fn toggle_key_read_due(&self) -> Option<(verbatim_model::LockKey, u64)> {
        self.toggle_keys.front().copied()
    }

    /// When the focused terminal's quiet-period timer is due, on the run's
    /// shared clock: the shell enters [`verbatim_model::Input::TerminalTimer`]
    /// with this time once it is past. A view of the focused terminal's
    /// model, read after each step, so a model dropped, or a terminal left,
    /// has no timer; after the timer fires, the due time is later than the
    /// time it fired at, or none.
    #[must_use]
    pub fn terminal_timer_due(&self) -> Option<u64> {
        crate::terminal_model::due(self)
    }

    /// What the keyboard hook reads of the foreground window, decided from
    /// the window the focus listener last reported in front (architecture
    /// section 5), or `None` before it has reported one. Keys may be held
    /// back for the window only when its process's integrity is known and
    /// no higher than Verbatim's, since Windows drops a key Verbatim sends
    /// again to a window of higher integrity, and a key lost costs more
    /// than a key judged without its caret (`books/dev/src/explanation/design/key-holding.md`,
    /// "Elevated windows"). Caret keys are held while the focus has text
    /// ([`holds_caret_keys`](Self::holds_caret_keys)). A console host
    /// window's keys are translated with the keyboard layout of the console
    /// host's own thread, named by its event, since Windows names the
    /// console's first attached process as the window's owner
    /// (`books/dev/src/reference/crates/verbatim-input-windows.md`, "Typed text"); any other
    /// window's with its owner's, which the hook reads itself.
    #[must_use]
    pub fn hook_foreground(&self) -> Option<HookForeground> {
        let foreground = self.foreground.window()?;
        Some(HookForeground {
            window: foreground.window,
            holds_keys: matches!(
                (foreground.integrity, foreground.own_integrity),
                (Some(window), Some(own)) if window <= own
            ),
            focus_has_text: self.holds_caret_keys(),
            keyboard_thread: (foreground.console && foreground.thread != 0)
                .then_some(foreground.thread),
        })
    }

    /// Forgets the focus's caret and its timed reports.
    pub(crate) fn forget_caret(&mut self) {
        self.caret = None;
        self.caret_history = Default::default();
    }

    /// The currently focused node and the application it came from, if
    /// anything is focused and its outpost is still running.
    #[must_use]
    pub fn focused(&self) -> Option<(Pid, &NodeSnapshot)> {
        self.focus
            .object()
            .filter(|focus| focus.alive)
            .map(|focus| (focus.source, &focus.snapshot))
    }

    /// The focused node as last announced, its ancestors outermost first,
    /// and the navigator object, as the reducer knows them, whether or not
    /// the focus's outpost is still running: what a failed end-to-end test
    /// saves to show where Verbatim thought it was.
    #[must_use]
    pub fn focus_report(
        &self,
    ) -> (
        Option<&NodeSnapshot>,
        &[NodeSnapshot],
        Option<&NodeSnapshot>,
    ) {
        (
            self.focus.object().map(|focus| &focus.snapshot),
            self.focus
                .object()
                .map_or(&[][..], |focus| &focus.ancestors[..]),
            self.navigator.as_ref().map(|navigator| &navigator.object),
        )
    }

    /// The application holding attention, if any foreground change has been
    /// seen yet. The shell derives the supervisor's view of attention from this.
    #[must_use]
    pub fn attention(&self) -> Option<Pid> {
        self.foreground
            .attention()
            .map(|attention| attention.source)
    }

    /// The details to fetch, as the active theme decides: a detail whose
    /// indication is off is not fetched, so off also saves the
    /// cross-process call (`books/dev/src/reference/crates/verbatim-core.md`). Like
    /// [`held_nodes`](Self::held_nodes), this is a view the shell sends
    /// out: every outpost is given it, and leaves out what is not wanted
    /// when it reads a node or text. Everything until the shell says
    /// otherwise.
    #[must_use]
    pub fn fetches(&self) -> Fetches {
        self.fetches
    }

    /// The reader settings as the reducer has them now: the last
    /// `Input::Settings`, with the toggle keys' changes since. The shell
    /// merges a change from the settings dialog into these, so a toggle key
    /// pressed meanwhile is not undone.
    #[must_use]
    pub fn settings(&self) -> ReaderSettings {
        self.settings
    }

    /// The application the focus belongs to; `None` before any focus.
    #[must_use]
    pub fn focus_source(&self) -> Option<Pid> {
        self.focus.object().map(|focus| focus.source)
    }

    /// When the newest focus the reducer took was observed, in
    /// microseconds since the Unix epoch on the run's shared clock; `None`
    /// before any (the applied stamp).
    #[must_use]
    pub fn latest_focus_observed_at(&self) -> Option<u64> {
        self.foreground.applied_at()
    }

    /// Every node the state refers to, grouped by the outpost that issued
    /// it: the focus, its ancestors, its last announced selection, the
    /// navigator, and the nodes Core's open queries are about (the node a
    /// navigation starts from, a caret key's, a text command's and a held
    /// key's). The shell
    /// sends each outpost its own set, so the outpost keeps exactly these
    /// nodes' live objects. A dead focus contributes nothing.
    #[must_use]
    pub fn held_nodes(&self) -> BTreeMap<OutpostId, BTreeSet<NodeId>> {
        let mut held: BTreeMap<OutpostId, BTreeSet<NodeId>> = BTreeMap::new();
        let mut insert = |id: NodeId| {
            held.entry(id.outpost()).or_default().insert(id);
        };
        if let Some(focus) = self.focus.object().filter(|focus| focus.alive) {
            insert(focus.snapshot.id);
            for ancestor in focus.ancestors.iter() {
                insert(ancestor.id);
            }
            if let Some(selected) = focus.last_selection {
                insert(selected);
            }
        }
        if let Some(navigator) = &self.navigator {
            insert(navigator.object.id);
        }
        for then in self.queries.continuations() {
            match then {
                Then::Navigation { from, .. } => insert(*from),
                Then::Caret(PendingCaret { node, .. })
                | Then::Text(PendingText { node, .. })
                | Then::HeldRead(HeldRead { node, .. }) => insert(*node),
                _ => {}
            }
        }
        // Nodes whose text the state holds positions in or waits on.
        if let Some(caret) = &self.caret {
            insert(caret.node);
        }
        if let Some(marker) = self.start_marker {
            insert(marker.node());
        }
        if let Some(say_all) = &self.say_all {
            insert(say_all.node);
        }
        held
    }

    /// Every text anchor the state refers to, grouped by the outpost that
    /// minted it: the caret's line and selection, the review cursor's line or
    /// point, the start marker, and say-all's positions. The shell sends
    /// each outpost its own set with its held nodes, and the outpost keeps
    /// these anchors (`verbatim_model::TextAnchor`).
    #[must_use]
    pub fn held_anchors(&self) -> HeldAnchors {
        let mut held = HeldAnchors::new();
        let mut insert = |node: NodeId, position: TextPosition| {
            held.entry(node.outpost())
                .or_default()
                .insert(position.anchor);
        };
        if let Some(caret) = &self.caret {
            insert(caret.node, caret.caret());
            if let Some(selection) = caret.selection {
                insert(caret.node, selection.start);
                insert(caret.node, selection.end);
            }
        }
        if let Some(navigator) = &self.navigator {
            match &navigator.text {
                ReviewText::At(position) => {
                    insert(navigator.object.id, TextPosition::at(position.line.start));
                }
                ReviewText::Point(point) => insert(navigator.object.id, *point),
                ReviewText::Unknown | ReviewText::Flat | ReviewText::Caret => {}
            }
        }
        if let Some(StartMarker::Text {
            node,
            at,
            after,
            selected_to,
        }) = self.start_marker
        {
            insert(node, at);
            insert(node, after);
            if let Some((end, end_after)) = selected_to {
                insert(node, end);
                insert(node, end_after);
            }
        }
        if let Some(say_all) = &self.say_all {
            if let Some((position, _)) = say_all.last_chunk {
                insert(say_all.node, position);
            }
            for unit in say_all
                .queued
                .iter()
                .filter_map(|queued| queued.unit.as_ref())
            {
                insert(say_all.node, unit.position());
            }
            for piece in say_all.buffer.iter().flatten().chain(&say_all.held) {
                insert(say_all.node, TextPosition::at(piece.chunk.start));
            }
        }
        held
    }

    /// Allocates the next index mark.
    pub(crate) fn allocate_mark(&mut self) -> SpeechMark {
        let mark = SpeechMark(self.next_mark);
        self.next_mark = self.next_mark.saturating_add(1);
        mark
    }

    /// Whether the focused node is exactly `node_id` and still alive.
    pub(crate) fn focus_matches(&self, node_id: NodeId) -> bool {
        self.focus
            .object()
            .is_some_and(|focus| focus.alive && focus.snapshot.id == node_id)
    }

    /// Removes every terminal model `which` picks, and with each the
    /// review commands waiting for its answer, whose queries end with no
    /// answer: the one way a model goes.
    pub(crate) fn drop_models(
        &mut self,
        which: impl Fn(&crate::terminal_model::TerminalModel) -> bool,
    ) {
        let reviews = self.terminals.remove_where(which);
        self.queries.forget_memory(reviews);
    }
}
