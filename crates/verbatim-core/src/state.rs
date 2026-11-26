//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the focus,
//! the attention record, the navigator, and the one object-navigation query
//! still in flight. The reducer changes it in place, one input at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use verbatim_model::{
    CaretSelection, CellPosition, HeldAnchors, HookForeground, NodeId, NodeSnapshot, OutpostId,
    Pid, ReaderSettings, ReportedDetails, ReviewCommand, SpeechMark, StateSet, TableId,
    TextAttributes, TextChunk, TextPosition, TextUnit, ToastId, TraceId, WindowFacts,
};

use crate::foreground::Foreground;

/// The spoken part of the committed focus
/// ([`crate::focus::versions::Focus`]): its node as last read, and what
/// Core spoke of it.
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
    /// The focused node reported itself focused when it became the focus,
    /// so a later state set without the focused state, from an outpost's
    /// UI Automation change, means the focus had left it when the change
    /// was raised (`docs/parity/changes-and-selection.md`, "State changes
    /// after the focus has left").
    pub(crate) reported_focused: bool,
    /// The trace of the event Core took this focus from.
    pub(crate) trace_id: Option<TraceId>,
    /// When that event was observed, in microseconds on the run's shared
    /// clock (0 when not known).
    pub(crate) observed_at_us: u64,
    /// What Core last spoke of the focus and its ancestors. A focus taken
    /// writes it, as does a report that speaks the focus's reading, a name,
    /// value or description change on the focus, a state change on the
    /// focus other than an outpost's change losing the focused state on a
    /// focus that reported itself focused, and a state change of an
    /// ancestor with an entry; a repeat never does.
    pub(crate) spoken: SpokenRecord,
    /// The formatting last reported in the focus's text (milestone M4 item
    /// 7): NVDA's per-object cache, kept on the focus object as NVDA keeps
    /// it, from which only changes are spoken
    /// (`docs/nvda/document-formatting.md`). Each focus taken starts
    /// afresh, a silent re-take after an outpost is replaced included
    /// (Dickson, 2026-11-08).
    pub(crate) reported_format: TextAttributes,
    /// The caret of the focus's text, when it has text and it was read.
    pub(crate) caret: Option<CaretContext>,
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
    /// The spoken record, to change in place.
    pub(crate) fn spoken(&mut self) -> &mut SpokenRecord {
        &mut self.spoken
    }
}

/// The navigator object and its review cursor (roadmap M3): the object
/// object-navigation commands walk, independent of keyboard focus. It
/// follows focus by default (every focus change resets it), and the
/// "to focus" command snaps it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Navigator {
    pub(crate) object: NodeSnapshot,
    /// How the review cursor reviews the object's text (milestone M4).
    pub(crate) text: ReviewText,
    /// The object's window, as the read that put the navigator there
    /// found it; `None` for the focus, put there as it was executed, whose
    /// window is the committed focus's while it is that focus's node
    /// (`crate::navigator`).
    pub(crate) window: Option<crate::event_queue::RequestWindow>,
}

impl Navigator {
    /// A navigator on `object` in `window` ([`Navigator::window`]), its
    /// review cursor at the start, how its text is reviewed not yet known.
    pub(crate) fn on(
        object: NodeSnapshot,
        window: Option<crate::event_queue::RequestWindow>,
    ) -> Self {
        Self {
            object,
            text: ReviewText::Unknown,
            window,
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
    /// The object has no text interface: its flat text, NVDA's basic text
    /// (`review_text::flat_text`), read afresh at each press, is reviewed
    /// through the text path, with the review cursor at this place in it.
    Flat(FlatPosition),
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

/// The review cursor in an object's flat text: its byte offset into the
/// whole flat text and its column, which a press keeps only while the
/// offset is a character boundary of the text it reads, at most its
/// length, and otherwise resets both to 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FlatPosition {
    pub(crate) offset: usize,
    pub(crate) column: usize,
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

/// Core's copy of the focus's caret, on the focus object so it ends with
/// the focus: the line it is on, with the caret at the chunk's offset, and
/// the selection (milestone M4). Kept current by every report, so a
/// Backspace knows what it deleted and the review cursor can follow the
/// caret without a round trip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaretContext {
    pub(crate) line: SharedChunk,
    pub(crate) selection: Option<CaretSelection>,
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
    /// At the point read.
    Point,
    /// On the character before the point read (the end of a selection).
    BeforePoint,
}

/// The select-then-copy start marker: where it is in `node`'s text
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StartMarker {
    pub(crate) node: NodeId,
    pub(crate) ends: MarkerEnds,
}

/// Which text a start marker is in. A marker in flat text holds byte
/// offsets into the whole flat text, which Core reads and no outpost
/// minted, so it can never be sent to an outpost as anchors to keep
/// (Claude, 2026-11-21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MarkerEnds {
    /// In the provider's text, at positions whose anchors its outpost
    /// minted and keeps.
    Text(Marked<TextPosition>),
    /// In the object's flat text, at byte offsets into the whole of it.
    Flat(Marked<usize>),
}

/// A start marker's places: `at`, and `after`, just past the character
/// there; and the review cursor's character, by the same two places, that
/// a select then copy's first press selected through, which its second
/// press copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Marked<P> {
    pub(crate) at: P,
    pub(crate) after: P,
    pub(crate) selected_to: Option<(P, P)>,
}

impl<P: Copy> Marked<P> {
    /// The same places, each turned by `place`.
    pub(crate) fn map<Q>(self, place: impl Fn(P) -> Q) -> Marked<Q> {
        Marked {
            at: place(self.at),
            after: place(self.after),
            selected_to: self
                .selected_to
                .map(|(end, end_after)| (place(end), place(end_after))),
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
        verbatim_text::proven::text_in(&self.chunk.text, range)
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
    pub(crate) buffer: std::collections::VecDeque<Vec<BufferedPiece>>,
    /// Text read with no sentence end after it, held back to be spoken
    /// with what follows it (`docs/nvda/speech.md`, "Say-all speaks
    /// without pauses").
    pub(crate) held: Vec<BufferedPiece>,
    /// How many units in a row were read with no sentence end, blank ones
    /// included: at `say_all::MAX_HELD`, what is held back is spoken.
    pub(crate) held_units: usize,
    /// The document's end has been read.
    pub(crate) finished: bool,
    /// Whether the display was asked to stay on.
    pub(crate) display_held: bool,
    /// Its run in the event queue, which makes its reads and caret moves.
    pub(crate) run: crate::say_all::QueueRun,
}

/// The table, row and column last spoken of a cell, each valid alone
/// (`SrState::cell_memory`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellMemory {
    table: Option<TableId>,
    row: Option<u32>,
    column: Option<u32>,
}

impl CellMemory {
    /// Forgets the table, as NVDA does after saying a table's counts, so
    /// the next cell says both its row and column even in the same table.
    pub(crate) fn forget_table(&mut self) {
        self.table = None;
    }

    /// The row and column to say of `cell`, each `None` when it is unchanged
    /// within one table, and remembers it. A cell with no table says both
    /// and leaves the table remembered as it was.
    pub(crate) fn said(&mut self, cell: &CellPosition) -> (Option<u32>, Option<u32>) {
        let same_table = cell.table.is_some() && cell.table == self.table;
        let row = (!same_table || self.row != Some(cell.row)).then_some(cell.row);
        let column = (!same_table || self.column != Some(cell.column)).then_some(cell.column);
        if cell.table.is_some() {
            self.table = cell.table;
        }
        self.row = Some(cell.row);
        self.column = Some(cell.column);
        (row, column)
    }
}

/// The last toast spoken (`SrState::last_toast`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastToast {
    /// The toast's identity.
    pub(crate) id: ToastId,
    /// When it was observed, in microseconds on the run's shared clock.
    pub(crate) at_us: u64,
}

/// Reducer state: focus, attention, navigator, and Core's queries.
///
/// The flight recorder clones the state at each checkpoint, once per
/// segment of its window, and serializes the clone into a dump, so a
/// recorded window replays from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrState {
    /// The foreground record: the foreground object and the entered
    /// window, under the keys the state held before the record existed.
    #[serde(flatten)]
    pub(crate) foreground: Foreground,
    /// The virtual desktop's name, kept until the next focus Core takes
    /// or its due time (`crate::desktop`).
    pub(crate) desktop_name: Option<crate::desktop::DesktopName>,
    /// The navigator object and review cursor (roadmap M3). `None` until
    /// the first focus lands; from then it tracks focus unless an
    /// object-navigation command moves it away.
    pub(crate) navigator: Option<Navigator>,
    /// The reader settings (milestone M4).
    pub(crate) settings: ReaderSettings,
    /// The lock keys pressed whose new state is still to be announced, in
    /// the order pressed (`Input::ToggleKeyPressed`), each with when its
    /// state is read, on the run's shared clock.
    pub(crate) toggle_keys: std::collections::VecDeque<(verbatim_model::LockKey, u64)>,
    /// The select-then-copy start marker.
    pub(crate) start_marker: Option<StartMarker>,
    /// The say-all in progress.
    pub(crate) say_all: Option<SayAll>,
    /// The word typed so far, for typed word echo; at most
    /// `editing::MAX_TYPED_WORD` bytes.
    pub(crate) typed_word: String,
    /// The next index mark number.
    pub(crate) next_mark: u64,
    /// The details the active theme reports (`Input::Reported`), from
    /// which each read's details are chosen ([`crate::details`]).
    pub(crate) reported: ReportedDetails,
    /// The focused terminal's output still to be spoken (milestone M4 item
    /// 9), bounded by the flood policy's limits.
    pub(crate) terminal: crate::terminal::TerminalSpeechState,
    /// Core's model of each terminal it reads, the most recently read last,
    /// at most 16 (`crate::terminal_model`): none until a terminal's first
    /// read is taken.
    pub(crate) terminals: crate::terminal_model::Terminals,
    /// The focused terminal's life and its reading
    /// (`crate::terminal::reading`).
    pub(crate) terminal_focus: crate::terminal::reading::TerminalFocus,
    /// The number of the last text anchor Core minted for a terminal's
    /// review answers, in a range no outpost mints from
    /// (`crate::terminal_model::TERMINAL_ANCHORS`).
    pub(crate) next_terminal_anchor: u64,
    /// The last tree or list item level spoken first, which an item at
    /// the same level speaks last instead ("Where the level goes" in
    /// `docs/nvda/speech.md`).
    pub(crate) last_tree_level: Option<u32>,
    /// The table, row and column last spoken of a cell, which the next cell
    /// in that table leaves out when unchanged
    /// (`docs/parity/focus-announcement.md`, "Table cell coordinates").
    pub(crate) cell_memory: CellMemory,
    /// The percentage each progress bar last indicated, in hundredths, by
    /// the centre of its location on the screen, as NVDA remembers it
    /// (`docs/nvda/object-model.md`, "How a progress bar reports its
    /// value"), the most recent last, at most [`PROGRESS_BARS_KEPT`].
    pub(crate) progress_reported: Vec<((i32, i32), u32)>,
    /// Each classic progress bar's one read out, by the bar's MSAA ids,
    /// from its want to its one ending, so that an ending is routed to the
    /// bar and a flood of one bar's changes keeps one read out
    /// ([`crate::progress`]).
    #[serde(with = "crate::event_queue::pairs")]
    pub(crate) bar_reads:
        std::collections::BTreeMap<crate::progress::BarKey, crate::progress::BarRead>,
    /// The requests Core's facts want sent outside the queue, and their
    /// tokens ([`crate::wants`]).
    pub(crate) wants: crate::wants::Wants,
    /// The last toast spoken, which a repeat within a second is compared
    /// with (`docs/parity/alerts.md`, "Toasts"). A toast dropped as a repeat
    /// leaves it, so a third is measured from the spoken one; only a toast
    /// is remembered, since only a toast is compared.
    pub(crate) last_toast: Option<LastToast>,
    /// Verbatim is exiting (`Input::Exiting`): no later input does
    /// anything, so nothing is said after the exit sound, as NVDA says
    /// nothing once it has stopped its handlers.
    pub(crate) exiting: bool,
    /// The two versions of the focus, the committed focus with what Core
    /// keeps of its focus answer (`crate::focus::versions`).
    pub(crate) versions: crate::focus::versions::Versions,
    /// The node Core last named in `Follow` per outpost
    /// (`crate::focus::follow`, Table 10).
    pub(crate) follows: crate::focus::follow::Follows,
    /// Core's plain reader and the plain object it holds
    /// (`crate::focus::plain`).
    pub(crate) plain: crate::focus::plain::PlainReader,
    /// The processes whose focus is dropped at its answer: the end-to-end
    /// harness's own, none for real users (`crate::focus::execute`).
    pub(crate) ignored: Vec<verbatim_model::Pid>,
    /// The trace of the queue input being handled, which an entry Core
    /// makes itself runs under.
    pub(crate) queue_trace: crate::entries::InputTrace,
    /// Whether the entry that ran last was a run that executed a focus,
    /// which the menu slot's entry judges (`crate::focus::menus`).
    pub(crate) run_executed: bool,
    /// The keys that joined the event queue: the newest number, which
    /// every arrival's fence and the caret keys' marks read
    /// (`crate::keys::entry`).
    pub(crate) arrivals: crate::keys::entry::Arrivals,
    /// The key transitions the hook held or deferred that Core has not yet
    /// sent (`crate::keys::strokes`).
    pub(crate) strokes: crate::keys::strokes::StrokeLine,
    /// Whether Alt is down, as the hook's last report of it said: a tell
    /// reads it at its arrival, in the inbox's order (the task switcher's
    /// item, `crate::applications::drops_ancestry`).
    pub(crate) alt_down: bool,
    /// The selection offsets of the last caret tell's answer read through
    /// an edit control's messages, which the next one is compared with
    /// (`crate::text_facts::edit_selection_repeated`).
    pub(crate) edit_selection: Option<verbatim_model::EditSelection>,
    /// The input desktop and the lock state (`crate::input_desktop`).
    pub(crate) desktop: crate::input_desktop::Desktop,
    /// The event queue (`crate::entries`): its entries, its requests and
    /// the front entry's checkpoint, plain data. Empty while a step of the
    /// queue runs, when the driver holds it. The paused function sits
    /// beside the state, in a [`crate::Runtime`].
    pub(crate) queue: crate::event_queue::Queue<crate::entries::Entry, SrState>,
}

/// What the test harness's readiness reads of Core's outpost generation
/// record (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation record"):
/// the listener's readiness and Verbatim's own
/// outpost's from the record, the spares' as the owner thread last
/// counted them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readiness {
    /// How many spare outposts wait.
    pub spares_ready: usize,
    /// Whether the newest listener is ready.
    pub listener_ready: bool,
    /// Whether Verbatim's own outpost is ready.
    pub own_outpost_ready: bool,
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

    /// What text the focus has, as the keyboard hook holds keys for it:
    /// none while [`holds_caret_keys`](Self::holds_caret_keys) does not
    /// hold, a terminal's for a terminal, whose Enter is typed text and not
    /// held, and a field's otherwise, whose Enter is held as NVDA's new-line
    /// script holds it (`docs/parity/text.md`, "Enter in a text field").
    #[must_use]
    pub fn focus_text_kind(&self) -> verbatim_model::FocusTextKind {
        let Some(focus) = crate::editing::text_focus(self) else {
            return verbatim_model::FocusTextKind::None;
        };
        let terminal = self.versions.live().is_some_and(|live| {
            let live = &live.context.snapshot;
            live.id == focus && crate::editing::is_grid(live.role)
        });
        if terminal {
            verbatim_model::FocusTextKind::Terminal
        } else {
            verbatim_model::FocusTextKind::Field
        }
    }

    /// The committed focus's record a caret key pressed now is bound to,
    /// as the hook's view publishes it
    /// (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event queue"):
    /// `None` with nothing committed, a focus whose reader ended,
    /// Core's own focus, or a focus that cannot have text.
    #[must_use]
    pub fn caret_binding(&self) -> Option<verbatim_model::request::CaretBinding> {
        crate::keys::caret_key::binding(self)
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

    /// Whether a check after a menu, popup or switcher end waits for its
    /// time (`crate::focus::menus`): the shell then reads what Windows
    /// names in front as it makes Core's wake, and the wake carries it
    /// ([`verbatim_model::request::QueueInput::Due`]). A view of the state,
    /// read as the wake is made, so the shell keeps no copy.
    #[must_use]
    pub fn menu_check_waits(&self) -> bool {
        self.queue
            .later()
            .any(|entry| matches!(entry, crate::entries::Entry::MenuCheck(_)))
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
    /// section 5), the ghost window when one stands in for a hung window,
    /// or `None` before it has reported one. Keys may be held
    /// back for the window only when its process's integrity is known and
    /// no higher than Verbatim's, since Windows drops a key Verbatim sends
    /// again to a window of higher integrity, and a key lost costs more
    /// than a key judged without its caret (`books/dev/src/explanation/design/key-holding.md`,
    /// "Elevated windows"). Caret keys are held while the focus has text,
    /// and Enter while it is a field's other than a terminal's
    /// ([`focus_text_kind`](Self::focus_text_kind)). A console host
    /// window's keys are translated with the keyboard layout of the console
    /// host's own thread, named by its event, since Windows names the
    /// console's first attached process as the window's owner
    /// (`books/dev/src/reference/crates/verbatim-input-windows.md`, "Typed text"); any other
    /// window's with its owner's, which the hook reads itself.
    #[must_use]
    pub fn hook_foreground(&self) -> Option<HookForeground> {
        let foreground = self.queue.foreground()?;
        Some(HookForeground {
            window: foreground.ghost.unwrap_or(foreground.window),
            holds_keys: matches!(
                (foreground.integrity, foreground.own_integrity),
                (Some(window), Some(own)) if window <= own
            ),
            focus_text: self.focus_text_kind(),
            keyboard_thread: (foreground.console && foreground.thread != 0)
                .then_some(foreground.thread),
            desktop_switches: self.desktop.switches_away,
        })
    }

    /// Whether Core's event queue is idle for the test harness
    /// (`books/dev/src/reference/crates/verbatim-app.md`, `AwaitIdle`): the queue empty, with
    /// nothing to be appended
    /// later; no request waiting for its outpost or out, and no window
    /// watched for recovery; no stroke in Core's line, so no key held; no
    /// lock key's new state still to be announced; and no delay of Core's
    /// own due (the return from another desktop).
    #[must_use]
    pub fn queue_idle(&self) -> bool {
        self.queue.idle()
            && self.strokes.is_empty()
            && !self.announces_toggle_key()
            && crate::input_desktop::due(self).is_none()
    }

    /// The number of the oldest key whose entry is still in Core's event
    /// queue, at the front or waiting: the test harness's mark that a key
    /// was handled is held until the entries of every key before it have
    /// ended (the design, "The test harness's idle").
    #[must_use]
    pub fn oldest_key_entry(&self) -> Option<verbatim_model::KeySeq> {
        self.queue
            .entries()
            .filter_map(|entry| match entry {
                crate::entries::Entry::Press(press) => Some(press.press.key_seq),
                _ => None,
            })
            .min()
    }

    /// Whether Core has committed a focus: the test harness's readiness
    /// waits for one (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation
    /// record").
    #[must_use]
    pub fn focus_committed(&self) -> bool {
        !matches!(
            self.versions.committed,
            crate::focus::versions::Committed::Nothing
        )
    }

    /// What the test harness's readiness reads of the outpost generation
    /// record.
    #[must_use]
    pub fn readiness(&self) -> Readiness {
        let record = &self.queue.record;
        Readiness {
            spares_ready: record.spares_ready,
            listener_ready: self.queue.ready(crate::event_queue::Reader::Listener),
            own_outpost_ready: record.own.is_some_and(|own| {
                record.applications.get(&own).is_some_and(|application| {
                    matches!(
                        application.stage,
                        crate::supervisor::record::Stage::Ready(_)
                    )
                })
            }),
        }
    }

    /// How many outpost generations have started and are not ready yet,
    /// as the generation record has them: the shell's idle conditions
    /// wait for none.
    #[must_use]
    pub fn outposts_starting(&self) -> usize {
        self.queue
            .record
            .applications
            .values()
            .filter(|application| {
                matches!(
                    application.stage,
                    crate::supervisor::record::Stage::Starting(_)
                )
            })
            .count()
    }

    /// The outpost generations ready now, by application: the control
    /// plane's outpost list.
    #[must_use]
    pub fn ready_outposts(&self) -> Vec<(verbatim_model::Pid, OutpostId)> {
        self.queue
            .record
            .applications
            .iter()
            .filter_map(|(process, application)| match application.stage {
                crate::supervisor::record::Stage::Ready(running) => {
                    Some((*process, OutpostId(running.generation)))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether the session counts as locked: locked, or its lock state
    /// unknown (`books/dev/src/reference/crates/verbatim-core.md`, "The secure desktop and the
    /// lock state"; security mitigation B3).
    #[must_use]
    pub fn session_locked(&self) -> bool {
        self.desktop.locked()
    }

    /// The caret of `node`'s text, while it is the focus's.
    pub(crate) fn focus_caret(&self, node: NodeId) -> Option<&CaretContext> {
        self.versions
            .focus()
            .map(|focus| &focus.context)
            .filter(|focus| focus.snapshot.id == node)
            .and_then(|focus| focus.caret.as_ref())
    }

    /// The caret Core holds on the focus, its offset in its line.
    #[must_use]
    pub fn focus_caret_offset(&self) -> Option<u32> {
        self.versions
            .focus()
            .and_then(|focus| focus.context.caret.as_ref())
            .map(|caret| caret.line.offset)
    }

    /// A focus read failed: Core forgets the focus's caret, and the
    /// committed focus's selection baseline and text-changed flag (Table
    /// 17, column C). NVDA: `editableText.py` 421 to 424.
    pub(crate) fn focus_read_failed(&mut self) {
        self.forget_caret();
        if let Some(focus) = self.versions.live_mut() {
            focus.kept.selection = crate::text_facts::SelectionFacts::default();
        }
    }

    /// Forgets the focus's caret.
    pub(crate) fn forget_caret(&mut self) {
        if let Some(focus) = self.versions.focus_mut() {
            focus.context.caret = None;
        }
    }

    /// The currently focused node and the application it came from, if
    /// anything is focused and its outpost is still running.
    #[must_use]
    pub fn focused(&self) -> Option<(Pid, &NodeSnapshot)> {
        self.versions
            .live()
            .map(|focus| (focus.context.source, &focus.context.snapshot))
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
        Vec<NodeSnapshot>,
        Option<&NodeSnapshot>,
    ) {
        let focus = self.versions.focus();
        (
            focus.map(|focus| &focus.context.snapshot),
            focus.map_or_else(Vec::new, |focus| {
                focus
                    .line
                    .iter()
                    .map(|entry| entry.snapshot.clone())
                    .collect()
            }),
            self.navigator.as_ref().map(|navigator| &navigator.object),
        )
    }

    /// The application holding attention: the application of the window
    /// the listener last reported in front, `None` before its first
    /// report.
    #[must_use]
    pub fn attention(&self) -> Option<Pid> {
        self.queue.foreground_process()
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
        self.versions.focus().map(|focus| focus.context.source)
    }

    /// When the focus object, the newest focus the reducer took, was
    /// observed, in microseconds since the Unix epoch on the run's shared
    /// clock; `None` before any, or when its observation is not known.
    #[must_use]
    pub fn latest_focus_observed_at(&self) -> Option<u64> {
        self.versions
            .focus()
            .map(|focus| focus.context.observed_at_us)
            .filter(|&observed| observed != 0)
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
        // A plain node is Core's own, held by no outpost.
        let mut insert = |id: NodeId| {
            if let Some(outpost) = id.outpost() {
                held.entry(outpost).or_default().insert(id);
            }
        };
        if let Some(focus) = self.versions.live() {
            insert(focus.context.snapshot.id);
            for entry in focus.line.iter() {
                insert(entry.snapshot.id);
            }
        }
        if let Some(navigator) = &self.navigator {
            insert(navigator.object.id);
        }
        // Nodes whose text the state holds positions in or waits on.
        if let Some(marker) = self.start_marker {
            insert(marker.node);
        }
        if let Some(say_all) = &self.say_all {
            insert(say_all.node);
        }
        // The nodes the event queue's requests and entries name: a request
        // waiting or out, and a caret key bound at its press to a node that
        // may no longer be the focus (design, "Held anchors").
        for request in self.queue.requests() {
            for (node, _) in crate::text_facts::named_by(request) {
                insert(node);
            }
        }
        for entry in self.queue.entries() {
            if let Some(bound) = crate::keys::caret_key::of_entry(entry).and_then(|key| key.bound) {
                insert(bound.node);
            }
        }
        held
    }

    /// The tokens of the caret tells' elements Core holds in each ready
    /// outpost generation: those its caret entries waiting or running
    /// name, for the caret read to adopt (`CoreToOutpost::NodesHeld`'s
    /// `told`). The shell leaves each outpost its own set with its held
    /// nodes, and the outpost keeps these elements.
    #[must_use]
    pub fn held_told(&self) -> BTreeMap<OutpostId, BTreeSet<u64>> {
        let ready: BTreeMap<verbatim_model::Pid, OutpostId> =
            self.ready_outposts().into_iter().collect();
        let mut held: BTreeMap<OutpostId, BTreeSet<u64>> = BTreeMap::new();
        for entry in self.queue.entries() {
            if let crate::entries::Entry::Caret(caret) = entry
                && let Some(token) = caret.adopted()
                && let Some(outpost) = ready.get(&caret.tell.process)
            {
                held.entry(*outpost).or_default().insert(token.0);
            }
        }
        held
    }

    /// The tokens of the running listener's told elements Core holds
    /// (`crate::held_tokens`): those its waiting entries, its front entry
    /// and the entries it appends later name, and the readers' last
    /// focus's; `None` while no listener runs. The shell sends the
    /// listener a view of them, and the listener lets go of every element
    /// it keeps that is left out and whose tell Core has handled.
    #[must_use]
    pub fn held_tokens(&self) -> Option<crate::HeldTokens> {
        use crate::event_queue::Logic as _;
        let generation = self.queue.listener()?;
        let mut tokens: BTreeSet<verbatim_model::tell::ElementToken> = self
            .queue
            .entries()
            .chain(self.queue.later())
            .filter_map(crate::entries::Entries::told_token)
            .filter(|(listener, _)| *listener == generation)
            .map(|(_, token)| token)
            .collect();
        // The readers' last focus's token is cleared when its listener
        // ends, so it is the running listener's.
        if let Some(last) = &self.versions.last
            && let crate::focus::versions::ReadBy::Uia {
                token: Some(token), ..
            } = &last.read_by
        {
            tokens.insert(*token);
        }
        Some(crate::HeldTokens { generation, tokens })
    }

    /// Every text anchor the state refers to, grouped by the outpost that
    /// minted it: the caret's line and selection, the review cursor's line or
    /// point, a start marker outside flat text, and say-all's positions.
    /// The shell sends each outpost its own set with its held nodes, and
    /// the outpost keeps these anchors (`verbatim_model::TextAnchor`).
    #[must_use]
    pub fn held_anchors(&self) -> HeldAnchors {
        let mut held = HeldAnchors::new();
        let mut insert = |node: NodeId, position: TextPosition| {
            if let Some(outpost) = node.outpost() {
                held.entry(outpost).or_default().insert(position.anchor);
            }
        };
        if let Some(focus) = self.versions.focus().map(|focus| &focus.context)
            && let Some(caret) = &focus.caret
        {
            let node = focus.snapshot.id;
            insert(node, caret.caret());
            if let Some(selection) = &caret.selection {
                insert(node, selection.range.start);
                insert(node, selection.range.end);
            }
        }
        if let Some(navigator) = &self.navigator {
            match &navigator.text {
                ReviewText::At(position) => {
                    insert(navigator.object.id, TextPosition::at(position.line.start));
                }
                ReviewText::Point(point) => insert(navigator.object.id, *point),
                ReviewText::Unknown | ReviewText::Flat(_) | ReviewText::Caret => {}
            }
        }
        // A marker in flat text holds no anchor of the provider's.
        if let Some(StartMarker {
            node,
            ends:
                MarkerEnds::Text(Marked {
                    at,
                    after,
                    selected_to,
                }),
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
        // The selection baseline, and every anchor a request of the event
        // queue names, a caret key's starting point among them (design,
        // "Held anchors").
        if let Some((node, baseline)) = crate::text_facts::held(self) {
            insert(node, TextPosition::at(baseline));
        }
        // Each terminal model's named start, which its next read, history
        // read or search counts from (build 3, Table 7).
        for model in self.terminals.iter() {
            if let Some(start) = model.start {
                insert(model.node, TextPosition::at(start));
            }
        }
        for request in self.queue.requests() {
            for (node, anchors) in crate::text_facts::named_by(request) {
                for anchor in anchors {
                    insert(node, TextPosition::at(anchor));
                }
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

    /// Whether the committed focus's outpost ended: its data stays, so a
    /// replacement outpost's report of the same focus can be taken
    /// silently (`docs/parity/focus-events.md`, "Recovery after an outpost
    /// is replaced"), but its ids name nothing any more.
    pub(crate) fn focus_ended(&self) -> bool {
        matches!(
            self.versions.committed,
            crate::focus::versions::Committed::Ended(_)
        )
    }

    /// Removes every terminal model `which` picks: the one way a model
    /// goes.
    pub(crate) fn drop_models(
        &mut self,
        which: impl Fn(&crate::terminal_model::TerminalModel) -> bool,
    ) {
        self.terminals.remove_where(which);
    }
}
