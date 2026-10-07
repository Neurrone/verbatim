//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the focus,
//! the attention record, the navigator, and the one object-navigation query
//! still in flight. The reducer changes it in place, one input at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use verbatim_model::{
    CaretKey, Fetches, HeldAnchors, NodeId, NodeSnapshot, OutpostId, Pid, QueryId, ReaderSettings,
    ReviewCommand, Selection, SpeechMark, TextAttributes, TextChunk, TextPosition, TextUnit,
    TraceId, WindowFacts, WindowHandle,
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
    /// focus can be taken silently (`docs/parity.md`, "Recovery after an
    /// outpost is replaced"), but the ids name nothing any more.
    pub(crate) alive: bool,
    /// The focused node reported itself focused when it became the focus,
    /// so a later state set without the focused state means the focus has
    /// left it before the next focus event arrived (`docs/parity.md`,
    /// "State changes after the focus has left").
    #[serde(default)]
    pub(crate) reported_focused: bool,
}

/// The application and window of the most recent foreground change (decision
/// D14 as amended by the outpost redesign): the reducer's stand-in for the
/// system's foreground window, which is what NVDA classifies events against.
/// Every event other than a foreground change is classified against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
/// text (`phase6-design.md`, "Core's state").
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
    /// A position in the object's text whose line has not been read, as
    /// say-all from the review cursor leaves it; the next review command
    /// reads the line there first.
    Point(TextPosition),
}

/// The review cursor in a text: the line it is on, its byte offset into
/// that line's text, and its column (`phase6-design.md`, M4 item 5). In a
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

/// A caret key passed to the application, waiting for the outpost's
/// evidence of what it did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingCaret {
    pub(crate) query_id: QueryId,
    pub(crate) node: NodeId,
    pub(crate) key: CaretKey,
    /// What a Backspace deleted, worked out from the caret before the key.
    pub(crate) deleted: Option<String>,
}

/// A focus with text whose announcement still has its text to say: the
/// focus announcement left the value out, and the selection or the caret's
/// line follows once the outpost's first caret report arrives
/// (`docs/nvda/speech.md`, "What an object with text says").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FocusText {
    pub(crate) node: NodeId,
    /// The request reading the selected text, once one is made.
    pub(crate) selection_query: Option<QueryId>,
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
    /// A select then copy read the text to copy.
    Copy,
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
    /// A position in a node's text.
    Text { node: NodeId, at: TextPosition },
    /// A byte offset into a node's flat text.
    Flat { node: NodeId, offset: usize },
}

impl StartMarker {
    pub(crate) fn node(self) -> NodeId {
        match self {
            Self::Text { node, .. } | Self::Flat { node, .. } => node,
        }
    }
}

/// A piece say-all read and has not yet handed to speech: the chunk it is
/// in, shared with the chunk's other pieces, and its byte range of the
/// chunk's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BufferedPiece {
    #[serde(
        serialize_with = "serialize_chunk",
        deserialize_with = "deserialize_chunk"
    )]
    pub(crate) chunk: SharedChunk,
    pub(crate) start: u32,
    pub(crate) end: u32,
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
    /// Pieces handed to speech whose marks playback has not reached yet,
    /// oldest first, with where each starts and its length in characters
    /// (Unicode scalar values). At most `say_all::HANDED`.
    pub(crate) queued: std::collections::VecDeque<(SpeechMark, TextPosition, u32)>,
    /// Pieces read and not yet handed to speech, in order: the chunk and
    /// the byte range of its text. At most one read-ahead batch's worth
    /// (`verbatim_model::MAX_READ_AHEAD_TEXT`).
    #[serde(default)]
    pub(crate) buffer: std::collections::VecDeque<BufferedPiece>,
    /// The trace of the latest read, which pieces handed out later carry.
    #[serde(default)]
    pub(crate) trace: Option<TraceId>,
    /// When playback last reached one of say-all's marks, in milliseconds
    /// since the Unix epoch, and the length in characters of the piece it
    /// started, to measure the pace of speech by the next.
    #[serde(default)]
    pub(crate) last_reached: Option<(u64, u32)>,
    /// The document's end has been read.
    pub(crate) finished: bool,
    /// Whether the display was asked to stay on.
    pub(crate) display_held: bool,
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
    /// same top-level window. The window is this focus's top-level window,
    /// when known.
    pub(crate) latest_focus: Option<(OutpostId, u64, Option<WindowHandle>)>,
    /// The node of the window most recently reported as the foreground,
    /// whose speech stays valid while it is in front (`FocusNow`).
    pub(crate) foreground: Option<NodeId>,
    /// The reader settings (milestone M4).
    #[serde(default)]
    pub(crate) settings: ReaderSettings,
    /// The focus's caret, when the focus has text.
    #[serde(default)]
    pub(crate) caret: Option<CaretContext>,
    /// The focus whose text is still to be spoken.
    #[serde(default)]
    pub(crate) focus_text: Option<FocusText>,
    /// The caret key waiting for evidence.
    #[serde(default)]
    pub(crate) pending_caret: Option<PendingCaret>,
    /// The review or text command's request in flight.
    #[serde(default)]
    pub(crate) pending_text: Option<PendingText>,
    /// The select-then-copy start marker.
    #[serde(default)]
    pub(crate) start_marker: Option<StartMarker>,
    /// The say-all in progress.
    #[serde(default)]
    pub(crate) say_all: Option<SayAll>,
    /// The pace say-all's speech was last measured at, in characters per
    /// minute, kept from one say-all to the next.
    #[serde(default)]
    pub(crate) say_all_pace: Option<u32>,
    /// The word typed so far, for typed word echo; at most
    /// `editing::MAX_TYPED_WORD` bytes.
    #[serde(default)]
    pub(crate) typed_word: String,
    /// Characters typed into a terminal, held until its text changes; at
    /// most `editing::MAX_HELD_TYPING` bytes.
    #[serde(default)]
    pub(crate) held_typing: String,
    /// The next index mark number.
    #[serde(default)]
    pub(crate) next_mark: u64,
    /// The details the active theme wants fetched (`Input::Fetches`).
    #[serde(default)]
    pub(crate) fetches: Fetches,
    /// The focused terminal's output still to be spoken (milestone M4 item
    /// 9), bounded by the flood policy's limits.
    #[serde(default)]
    pub(crate) terminal: crate::terminal::TerminalSpeech,
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
}

impl SrState {
    /// An initial state with no focus, no attention, and nothing pending.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// The application holding attention, if any foreground change has been
    /// seen yet. The shell derives the supervisor's view of attention from this.
    #[must_use]
    pub fn attention(&self) -> Option<Pid> {
        self.attention.map(|attention| attention.source)
    }

    /// The details to fetch, as the active theme decides: a detail whose
    /// indication is off is not fetched (`phase6-design.md`, "Themes: one
    /// model for verbosity, speech, and sounds"). Like
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

    /// How many of a change's newest lines an outpost reads from a terminal
    /// (milestone M4 item 9): as many as the flood policy's limits can keep
    /// (`ReaderSettings::terminal_read_lines`). Like
    /// [`fetches`](Self::fetches), a view the shell gives every outpost.
    #[must_use]
    pub fn terminal_read_lines(&self) -> u16 {
        self.settings.terminal_read_lines()
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
        self.latest_focus
            .map(|(_, observed_at_ms, _)| observed_at_ms)
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
                ReviewText::Unknown | ReviewText::Flat => {}
            }
        }
        if let Some(StartMarker::Text { node, at }) = self.start_marker {
            insert(node, at);
        }
        if let Some(say_all) = &self.say_all {
            if let Some((position, _)) = say_all.last_chunk {
                insert(say_all.node, position);
            }
            for (_, position, _) in &say_all.queued {
                insert(say_all.node, *position);
            }
            for piece in &say_all.buffer {
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
