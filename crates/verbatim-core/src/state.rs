//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the focus,
//! the attention record, the navigator, and the one object-navigation query
//! still in flight. The reducer changes it in place, one input at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use verbatim_model::{
    CaretKey, Fetches, ForegroundWindow, HeldAnchors, HookForeground, NodeId, NodeSnapshot,
    OutpostId, Pid, QueryId, ReaderSettings, ReviewCommand, Selection, SpeechMark, TextAttributes,
    TextChunk, TextPosition, TextUnit, TraceId, WindowFacts, WindowHandle,
};

/// The focused node and what the reducer knows about where it sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FocusContext {
    /// The application the focus belongs to.
    pub(crate) source: Pid,
    /// The focus event's window facts, when it carried any. The top-level
    /// window is what a foreground change is compared against.
    pub(crate) window: Option<WindowFacts>,
    /// The focused node as last announced or updated.
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
    #[serde(
        serialize_with = "serialize_shared",
        deserialize_with = "deserialize_shared"
    )]
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
    /// The trace of the event Core took this focus from, which names, for
    /// the focus listener, the element it read for a UIA focus
    /// ([`crate::focus_rules::HeldFocus`]); `None` in a recording made
    /// before it was kept.
    #[serde(default)]
    pub(crate) trace_id: Option<TraceId>,
    /// When that event was observed, its milliseconds and microseconds on
    /// the run's shared clock (0 when not known).
    #[serde(default)]
    pub(crate) observed: (u64, u64),
}

/// The application and window of the most recent foreground change (decision
/// D14): the reducer's stand-in for the
/// system's foreground window, which is what NVDA classifies events against.
/// Every event other than a foreground change is classified against it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attention {
    pub(crate) source: Pid,
    pub(crate) window: Option<WindowFacts>,
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
/// text (`docs/architecture.md`, section 2).
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
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
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
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
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
    /// When the report this caret came from was observed or read, 0 when
    /// unknown. A timed report observed before this one, arriving later,
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

/// A report of the focus's caret with the time it was observed: an event's
/// `observed_at_ms`, or the time the outpost read a caret key's reply. A
/// caret key's own path to Core (the hook, then the application, then
/// Core) can be slower than the application's (the application, then the
/// outpost, then Core), so the report Core holds when a key arrives may
/// already show what the key did. The newest report observed before the
/// key was pressed is the caret the key found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TimedCaret {
    pub(crate) observed_at_ms: u64,
    pub(crate) node: NodeId,
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
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
    pub(crate) query_id: QueryId,
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
    pub(crate) query_id: QueryId,
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
}

/// The caret keys the hook held back: the one whose caret is being read,
/// and the keys pressed while it was, each handled in the order pressed
/// (`docs/design/key-holding.md`, "Keys behind a held key").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HeldKeys {
    pub(crate) reading: Option<HeldRead>,
    pub(crate) waiting: std::collections::VecDeque<WaitingKey>,
    /// When the last held key was passed on, on the outposts' clock: a key
    /// behind it reaches the application no earlier.
    #[serde(default)]
    pub(crate) passed_at_ms: u64,
    /// The watch of the held key last passed on, until it is answered: a
    /// held key behind it has its caret read only then.
    #[serde(default)]
    pub(crate) awaiting_watch: Option<QueryId>,
    /// The held key whose wait for `awaiting_watch` the shell was asked to
    /// end at its bound, so that it is asked once.
    #[serde(default)]
    pub(crate) bounded: Option<TraceId>,
}

impl HeldKeys {
    /// Whether a key pressed now waits behind a held key.
    pub(crate) fn busy(&self) -> bool {
        self.reading.is_some() || !self.waiting.is_empty()
    }
}

/// A text request a review or text command made, and what to do with its
/// answer. A newer command's request supersedes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingText {
    pub(crate) query_id: QueryId,
    pub(crate) node: NodeId,
    pub(crate) then: TextFollowUp,
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
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
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
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
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
        &self.chunk.text[self.start as usize..self.end as usize]
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
    /// The read in flight, if any.
    pub(crate) pending: Option<QueryId>,
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

/// The newest focus event applied, by which a focus from another outpost is
/// ordered ([`SrState::latest_focus`]). It reads from a dump written as the
/// tuple of its first three fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LatestFocus {
    /// The outpost that reported it.
    pub(crate) outpost: OutpostId,
    /// When it was observed, in milliseconds on the run's shared clock.
    pub(crate) observed_at_ms: u64,
    /// Its top-level window, when known.
    pub(crate) top_level: Option<WindowHandle>,
    /// When it was observed, in microseconds from the same clock read; 0
    /// when its source gave milliseconds only.
    #[serde(default)]
    pub(crate) observed_at_us: u64,
}

impl LatestFocus {
    /// Whether this focus was observed after a focus observed at `ms`
    /// milliseconds, and at `micros` microseconds when not 0: to the
    /// microsecond when both are known, else to the millisecond, a tie
    /// being no later.
    pub(crate) fn observed_after(&self, (ms, micros): (u64, u64)) -> bool {
        if micros != 0 && self.observed_at_us != 0 {
            micros < self.observed_at_us
        } else {
            ms < self.observed_at_ms
        }
    }
}

/// The most recently issued object-navigation query whose completion has not
/// landed yet, and the node it navigates from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingNavigation {
    pub(crate) query_id: QueryId,
    pub(crate) from: NodeId,
}

/// Reducer state: focus, attention, navigator, and the latest navigation.
///
/// Small control state is held in plain fields; anything that grows with
/// the application is held behind `Arc`, so a clone of the whole state
/// costs the same however large the application is. The flight recorder
/// relies on that to snapshot the state at each checkpoint, and serializes
/// the snapshot into a dump, so a recorded window replays from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrState {
    pub(crate) focus: Option<FocusContext>,
    /// How many focuses the reducer has taken as the focus, the silent
    /// re-read of a replacement outpost's report included: what NVDA's
    /// last queued focus follows, so the host can tell the focus listener
    /// when a focus it did not accept was queued (`docs/parity/focus-events.md`,
    /// "Duplicate focus suppression").
    #[serde(default)]
    pub(crate) focuses_queued: u64,
    pub(crate) attention: Option<Attention>,
    pub(crate) next_query_id: u64,
    /// The navigator object and review cursor (roadmap M3). `None` until
    /// the first focus lands; from then it tracks focus unless an
    /// object-navigation command moves it away.
    pub(crate) navigator: Option<Navigator>,
    /// The most recently issued object-navigation query, if its completion
    /// has not landed yet.
    ///
    /// A completion is applied only when it answers this query: a later
    /// navigation command supersedes an earlier still-pending one, so a
    /// completion arriving after it is dropped rather than clobbering where
    /// the user has since moved. A `FocusChanged` event snaps the navigator
    /// to the new focus (review follows focus) but deliberately leaves this
    /// field alone — an app-initiated focus event must not be able to
    /// discard the user's own, more recent, in-flight navigation. `ToFocus`
    /// clears it explicitly, since it is itself the user's explicit, newer
    /// intent superseding whatever navigation was pending; so does the end of
    /// the outpost it was sent to.
    pub(crate) latest_navigation: Option<PendingNavigation>,
    /// The outpost and observation time of the newest focus event applied.
    /// Each outpost keeps its own events in order, but two outposts can
    /// deliver theirs out of the order they were observed in, where NVDA
    /// handles every event in one queue; a focus event from another outpost
    /// observed before this one is stale and dropped, unless it is in the
    /// same top-level window.
    pub(crate) latest_focus: Option<LatestFocus>,
    /// When the newest foreground change applied was observed, in
    /// milliseconds since the Unix epoch: a focus with a known observation
    /// time that brought a new foreground window and cut speech off. Speech
    /// from elsewhere observed before it, but reaching the reducer after it,
    /// is dropped, as NVDA's foreground change cancels speech queued before
    /// it (`docs/design/focus-pipeline.md`, section 7).
    #[serde(default)]
    pub(crate) foreground_changed_at: Option<u64>,
    /// The node of the window most recently reported as the foreground,
    /// whose speech stays valid while it is in front (`FocusNow`).
    pub(crate) foreground: Option<NodeId>,
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
    /// The caret key waiting for evidence.
    #[serde(default)]
    pub(crate) pending_caret: Option<PendingCaret>,
    /// The query and node of the caret key a later key on the same node
    /// superseded while it waited, whose answer may still carry a change
    /// of the selection (`editing::superseded_reply`).
    #[serde(default)]
    pub(crate) superseded_caret: Option<(QueryId, NodeId)>,
    /// The caret keys the keyboard hook held back
    /// (`docs/design/key-holding.md`).
    #[serde(default)]
    pub(crate) held_keys: HeldKeys,
    /// The review or text command's request in flight.
    #[serde(default)]
    pub(crate) pending_text: Option<PendingText>,
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
    pub(crate) terminals: Vec<crate::terminal_model::TerminalModel>,
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
    /// The window the focus listener last reported in the system's
    /// foreground (`Input::ForegroundWindow`), from which the keyboard
    /// hook's view of the foreground is decided
    /// ([`SrState::hook_foreground`]).
    #[serde(default)]
    pub(crate) foreground_window: Option<ForegroundWindow>,
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

    /// How many focuses the reducer has taken as the focus so far; it grows
    /// by one with each, so a host that compares it before and after an
    /// input knows whether that input's focus was queued.
    #[must_use]
    pub fn focuses_queued(&self) -> u64 {
        self.focuses_queued
    }

    /// Whether a caret key pressed now is held back until the focus's
    /// caret has been read (`docs/design/key-holding.md`): the focus has
    /// text, or may have, as for the caret keys whose evidence is watched.
    /// The keyboard hook holds caret keys only while this holds.
    #[must_use]
    pub fn holds_caret_keys(&self) -> bool {
        crate::editing::text_focus(self).is_some()
    }

    /// What the keyboard hook reads of the foreground window, decided from
    /// the window the focus listener last reported in front (architecture
    /// section 5), or `None` before it has reported one. Keys may be held
    /// back for the window only when its process's integrity is known and
    /// no higher than Verbatim's, since Windows drops a key Verbatim sends
    /// again to a window of higher integrity, and a key lost costs more
    /// than a key judged without its caret (`docs/design/key-holding.md`,
    /// "Elevated windows"). A console host window's keys are translated
    /// with the keyboard layout of the console host's own thread, named by
    /// its event, since Windows names the console's first attached process
    /// as the window's owner (`docs/crates/verbatim-input-windows.md`,
    /// "Typed text"); any other window's with its owner's, which the hook
    /// reads itself.
    #[must_use]
    pub fn hook_foreground(&self) -> Option<HookForeground> {
        let foreground = self.foreground_window?;
        Some(HookForeground {
            window: foreground.window,
            holds_keys: matches!(
                (foreground.integrity, foreground.own_integrity),
                (Some(window), Some(own)) if window <= own
            ),
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
            .as_ref()
            .filter(|focus| focus.alive)
            .map(|focus| (focus.source, &focus.snapshot))
    }

    /// The outpost whose node the focus Core holds is, and whether that
    /// focus is alive: what the host compares across an input to tell
    /// whether its focus changed, without copying the focus
    /// ([`held_focus`](Self::held_focus)).
    #[must_use]
    pub fn held_focus_owner(&self) -> Option<(OutpostId, bool)> {
        self.focus
            .as_ref()
            .filter(|focus| focus.trace_id.is_some())
            .map(|focus| (focus.snapshot.id.outpost(), focus.alive))
    }

    /// The focus Core holds, as the focus listener and the outposts judge
    /// events against it before they read them (decision D14, "Where the
    /// rules run"), alive or not; `None` before any focus, or for a focus
    /// taken from a recording made before its trace was kept.
    #[must_use]
    pub fn held_focus(&self) -> Option<crate::focus_rules::HeldFocus> {
        let focus = self.focus.as_ref()?;
        let held = |node: &NodeSnapshot| crate::focus_rules::HeldNode {
            id: node.id,
            backend: node.backend,
            role: node.role,
            name: node.name.clone(),
            states: node.states,
        };
        Some(crate::focus_rules::HeldFocus {
            trace_id: focus.trace_id?,
            observed: focus.observed,
            focus: held(&focus.snapshot),
            ancestors: focus.ancestors.iter().map(held).collect(),
            top_level: focus.window.as_ref().map(|window| window.top_level),
            alive: focus.alive,
        })
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
            self.focus.as_ref().map(|focus| &focus.snapshot),
            self.focus
                .as_ref()
                .map_or(&[][..], |focus| &focus.ancestors[..]),
            self.navigator.as_ref().map(|navigator| &navigator.object),
        )
    }

    /// The application holding attention, if any foreground change has been
    /// seen yet. The shell derives the supervisor's view of attention from this.
    #[must_use]
    pub fn attention(&self) -> Option<Pid> {
        self.attention.as_ref().map(|attention| attention.source)
    }

    /// The details to fetch, as the active theme decides: a detail whose
    /// indication is off is not fetched, so off also saves the
    /// cross-process call (`docs/crates/verbatim-core.md`). Like
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
        self.focus.as_ref().map(|focus| focus.source)
    }

    /// When the newest focus the reducer applied was observed, in
    /// milliseconds since the Unix epoch; `None` before any.
    #[must_use]
    pub fn latest_focus_observed_at(&self) -> Option<u64> {
        self.latest_focus.map(|latest| latest.observed_at_ms)
    }

    /// Every node the state refers to, grouped by the outpost that issued
    /// it: the focus, its ancestors, its last announced selection, the
    /// navigator, and the node the latest navigation starts from. The shell
    /// sends each outpost its own set, so the outpost keeps exactly these
    /// nodes' live objects. A dead focus contributes nothing.
    #[must_use]
    pub fn held_nodes(&self) -> BTreeMap<OutpostId, BTreeSet<NodeId>> {
        let mut held: BTreeMap<OutpostId, BTreeSet<NodeId>> = BTreeMap::new();
        let mut insert = |id: NodeId| {
            held.entry(id.outpost()).or_default().insert(id);
        };
        if let Some(focus) = self.focus.as_ref().filter(|focus| focus.alive) {
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
        if let Some(pending) = self.latest_navigation {
            insert(pending.from);
        }
        // Nodes whose text the state holds positions in or waits on.
        if let Some(caret) = &self.caret {
            insert(caret.node);
        }
        if let Some(pending) = &self.pending_caret {
            insert(pending.node);
        }
        if let Some(pending) = &self.pending_text {
            insert(pending.node);
        }
        if let Some(reading) = &self.held_keys.reading {
            insert(reading.node);
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
        self.next_mark += 1;
        mark
    }

    /// Whether the focused node is exactly `node_id` and still alive.
    pub(crate) fn focus_matches(&self, node_id: NodeId) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|focus| focus.alive && focus.snapshot.id == node_id)
    }

    /// Allocates a fresh, process-unique-within-this-state `QueryId`.
    pub(crate) fn allocate_query_id(&mut self) -> QueryId {
        let id = self.next_query_id;
        self.next_query_id += 1;
        QueryId(id)
    }
}

/// Serializes a shared slice as a plain sequence.
fn serialize_shared<S: Serializer>(
    items: &Arc<[NodeSnapshot]>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    items[..].serialize(serializer)
}

/// Deserializes a plain sequence into a shared slice.
fn deserialize_shared<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Arc<[NodeSnapshot]>, D::Error> {
    Vec::<NodeSnapshot>::deserialize(deserializer).map(Arc::from)
}

/// Serializes a shared chunk as a plain chunk.
fn serialize_chunk<S: Serializer>(chunk: &SharedChunk, serializer: S) -> Result<S::Ok, S::Error> {
    chunk.as_ref().serialize(serializer)
}

/// Deserializes a plain chunk into a shared one.
fn deserialize_chunk<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SharedChunk, D::Error> {
    TextChunk::deserialize(deserializer).map(Arc::new)
}
