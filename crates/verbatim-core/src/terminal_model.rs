//! Core's model of each terminal it reads (`docs/design/terminal-model.md`,
//! "Where it runs"): the reading rules, with their memory of the terminal's
//! rows, run here in the reducer, and the outpost only reads what Core asks
//! for and sends each read back as a snapshot ([`TerminalSnapshot`]).
//!
//! For each terminal node it reads, Core keeps the rules
//! ([`TerminalReader`], behind an `Arc` so that a checkpoint's clone of the
//! state shares it), the keys it took in and has not yet given the rules,
//! what the rules asked for and Core is owed (a whole read, a history-block
//! read, a fresh read before a review command, history rows above the
//! memory), the review cursor's places, the caret and the screen's shape as
//! the last read found them, and when the quiet-period timer is due.
//!
//! - Keys. Every key Core takes in while it reads the focused terminal is
//!   kept, in the order Core took it in, with when it reached the program
//!   and its trace. Before a read that began at time S is given to the
//!   rules, they are given every kept key whose time is at or before S, in
//!   order, stopping at the first later one; a key in S's own millisecond
//!   counts as before it. Before the timer at T, likewise up to T. The
//!   arrival of a message decides nothing. With each key Core asks the
//!   outpost for the screen's colours in the first read from the key's time
//!   ([`TextOp::TerminalColours`]).
//! - A read's speech key: once the keys up to its start are given, the
//!   newest key given, when its window (`KEY_WINDOW_MS` after it) holds
//!   the read's start. The read's speech is spoken under that key's trace
//!   ([`Effect::ForKey`]), and the flood policy speaks the read's output
//!   under it later too, so a key deferred behind a held key, which the
//!   read's speech answers, does not cut it off. A read with no speech key
//!   and the timer's speech carry the input's own trace.
//! - What is owed. When the rules ask for the whole document, or a first
//!   read again with its history block, Core asks for it
//!   ([`TextOp::TerminalReadNow`]) and drops the terminal's live snapshots
//!   until it comes: the outpost answers in order on one pipe, so each of
//!   them began before the read asked for, which holds everything it
//!   showed.
//! - Review commands. A request for the terminal's text that Core can
//!   answer from its memory ([`review::answers`]) is not sent: Core asks for
//!   a fresh read, aligns it, and answers the request from the memory,
//!   asking for the rows above the memory when the answer needs them
//!   ([`TextOp::TerminalHistory`]) and answering again once they come. The
//!   rows it answers with carry text anchors from a range reserved for Core
//!   ([`TERMINAL_ANCHORS`]), which no outpost mints.
//!
//! - The read before a held key. A terminal's outpost answers Core's read
//!   of the caret before a held key ([`TextOp::ReadCaret`]) with a
//!   snapshot, which is aligned as a live read is and whose caret answers
//!   the read ([`before_key`]).
//!
//! A terminal's model is made with its first snapshot.

mod review;

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    CaretReply, Effect, Input, NodeId, OutpostId, QueryId, TerminalColourRun, TerminalScope,
    TerminalSnapshot, TerminalText, TextOp, TextReply, TextRequest, TraceId,
};
use verbatim_text::terminal_reading::{
    ColourRun, KEY_WINDOW_MS, Key, KeyKind, Settings, Speech, Step, TerminalReader, Unit,
};
use verbatim_text::terminal_rows::{Lost, Read, ReadText};

use crate::state::SrState;
use crate::terminal::{Said, SaidItem, Skipped, TerminalItem, TerminalKeyKind, TerminalUnit};

pub(crate) use review::CaretAt;

/// How many terminals Core keeps a model of, the most recently read: a
/// terminal's memory lives while its node does, as the outpost kept it.
const MODELS: usize = 16;

/// How many keys wait for a read at most; past it the oldest is given to
/// the rules at once, so the state stays bounded however long no read
/// comes.
const PENDING_KEYS: usize = 256;

/// How many colours requests Core waits on the answer to at most.
const COLOURS_ASKED: usize = 16;

/// The text anchors Core mints for a terminal's review answers have this
/// bit set; an outpost's never do.
pub(crate) const TERMINAL_ANCHORS: u64 = 1 << 63;

/// A key Core took in and has not yet given the reading rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingKey {
    kind: TerminalKeyKind,
    /// When it reached the program, on the shared clock.
    at_ms: u64,
    trace: TraceId,
}

/// A read the rules asked for, which Core is owed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Owed {
    /// The whole document, after a lost read.
    Whole(QueryId),
    /// A first read again with its history block.
    HistoryBlock(QueryId),
}

impl Owed {
    fn query(self) -> QueryId {
        match self {
            Self::Whole(query) | Self::HistoryBlock(query) => query,
        }
    }
}

/// A request for a terminal's text Core answers from its memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Review {
    query_id: QueryId,
    /// The trace of the input that asked for it, when it had one; the
    /// answer's otherwise.
    trace: Option<TraceId>,
    op: TextOp,
}

/// Rows of the history above the memory, read for the review answer in
/// progress: what was asked (rows above the next read's start, or the
/// whole document) and what came.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HistoryRead {
    above: u32,
    whole: bool,
    text: String,
    rows_above: u32,
}

/// Core's model of one terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalModel {
    pub(crate) node: NodeId,
    /// The reading rules and their memory, shared with checkpoints.
    #[serde(with = "shared")]
    pub(crate) reader: Arc<TerminalReader>,
    /// The keys not yet given to the rules, in the order Core took them in.
    pending: VecDeque<PendingKey>,
    /// The newest key given to the rules: its trace and when it reached the
    /// program.
    last_key: Option<(TraceId, u64)>,
    /// A read the rules asked for and Core is owed.
    owed: Option<Owed>,
    /// The fresh read asked for before a review answer.
    fresh: Option<QueryId>,
    /// The requests waiting for an answer from the memory, oldest first.
    reviews: VecDeque<Review>,
    /// The history read asked for by the review answer in progress.
    history_asked: Option<(QueryId, u32, bool)>,
    /// The history reads that came for the review answer in progress.
    pub(crate) history: Vec<HistoryRead>,
    /// The places the review answers minted, each a text anchor's number
    /// and the memory's place for it, oldest first.
    pub(crate) places: VecDeque<(u64, u64)>,
    /// The caret as the last read found it.
    pub(crate) caret: Option<CaretAt>,
    /// Whether the last read started at the document's start.
    pub(crate) at_document_start: bool,
    /// The screen's height in rows and width in cells as the last read
    /// found them.
    pub(crate) height: u32,
    pub(crate) width: usize,
    /// The shape last told to the outpost.
    told: Option<(u32, u32)>,
    /// When the rules' quiet-period timer is due.
    due_ms: Option<u64>,
    /// The colours requests not yet answered.
    colours_asked: VecDeque<QueryId>,
}

/// The reader held behind an `Arc`, serialized as itself.
mod shared {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use verbatim_text::terminal_reading::TerminalReader;

    pub(super) fn serialize<S: Serializer>(
        reader: &Arc<TerminalReader>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        reader.as_ref().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<TerminalReader>, D::Error> {
        TerminalReader::deserialize(deserializer).map(Arc::new)
    }
}

impl TerminalModel {
    fn new(node: NodeId, settings: Settings) -> Self {
        Self {
            node,
            reader: Arc::new(TerminalReader::new(settings)),
            pending: VecDeque::new(),
            last_key: None,
            owed: None,
            fresh: None,
            reviews: VecDeque::new(),
            history_asked: None,
            history: Vec::new(),
            places: VecDeque::new(),
            caret: None,
            at_document_start: false,
            height: 0,
            width: 0,
            told: None,
            due_ms: None,
            colours_asked: VecDeque::new(),
        }
    }

    /// The rules, to change: copied first when a checkpoint shares them.
    fn rules(&mut self) -> &mut TerminalReader {
        Arc::make_mut(&mut self.reader)
    }

    /// Gives the rules every kept key that reached the program at or before
    /// `up_to_ms`, in order.
    fn give_keys(&mut self, up_to_ms: u64) {
        while self
            .pending
            .front()
            .is_some_and(|key| key.at_ms <= up_to_ms)
        {
            if let Some(key) = self.pending.pop_front() {
                self.give(&key);
            }
        }
    }

    fn give(&mut self, key: &PendingKey) {
        let step = self.rules().key(rules_key(&key.kind, key.at_ms));
        self.due_ms = step.timer;
        self.last_key = Some((key.trace, key.at_ms));
    }

    /// The key a read that began at `started_ms` is spoken under: the
    /// newest key given, when its window holds the read.
    fn speech_key(&self, started_ms: u64) -> Option<TraceId> {
        self.last_key
            .filter(|(_, at_ms)| at_ms + KEY_WINDOW_MS >= started_ms)
            .map(|(trace, _)| trace)
    }
}

/// The reading rules' settings from the reader settings.
fn settings_of(state: &SrState) -> Settings {
    Settings {
        report_new_output: state.settings.report_terminal_output,
        review_follows_caret: state.settings.follow_caret,
    }
}

/// The model of terminal `node`.
fn model_mut(state: &mut SrState, node: NodeId) -> Option<&mut TerminalModel> {
    state.terminals.iter_mut().find(|model| model.node == node)
}

fn model_of(state: &SrState, node: NodeId) -> Option<&TerminalModel> {
    state.terminals.iter().find(|model| model.node == node)
}

/// The model of terminal `node`, made with its first snapshot; the least
/// recently read model is forgotten past [`MODELS`].
fn model_or_new(state: &mut SrState, node: NodeId) -> &mut TerminalModel {
    let at = state.terminals.iter().position(|model| model.node == node);
    let model = match at {
        Some(at) => state.terminals.remove(at),
        None => TerminalModel::new(node, settings_of(state)),
    };
    if state.terminals.len() >= MODELS {
        state.terminals.remove(0);
    }
    state.terminals.push(model);
    let last = state.terminals.len() - 1;
    &mut state.terminals[last]
}

/// The focused terminal, when Core models it.
fn focused(state: &SrState) -> Option<NodeId> {
    crate::terminal::terminal_focus(state).filter(|node| model_of(state, *node).is_some())
}

/// A text request about terminal `node`.
fn request(state: &mut SrState, node: NodeId, op: TextOp) -> (QueryId, Effect) {
    let query_id = state.allocate_query_id();
    (
        query_id,
        Effect::Text(TextRequest {
            query_id,
            node_id: node,
            op,
        }),
    )
}

/// The timer effect for the focused terminal when its due time is no
/// longer `before`.
fn timer_effect(state: &SrState, node: NodeId, before: Option<u64>) -> Option<Effect> {
    let model = model_of(state, node)?;
    (focused(state) == Some(node) && model.due_ms != before).then_some(Effect::TerminalTimer {
        due_ms: model.due_ms,
    })
}

/// A key of `kind` that reached the focused terminal at `at_ms`, with its
/// trace: kept until a read that began at or after it, and the colours
/// asked for from its time. Nothing when Core does not read the focus.
pub(crate) fn key(
    state: &mut SrState,
    kind: TerminalKeyKind,
    at_ms: u64,
    trace: TraceId,
) -> Vec<Effect> {
    let Some(node) = focused(state) else {
        return Vec::new();
    };
    let (query_id, effect) = request(state, node, TextOp::TerminalColours { since_ms: at_ms });
    let Some(model) = model_mut(state, node) else {
        return Vec::new();
    };
    let before = model.due_ms;
    if model.pending.len() >= PENDING_KEYS
        && let Some(oldest) = model.pending.pop_front()
    {
        model.give(&oldest);
    }
    model.pending.push_back(PendingKey { kind, at_ms, trace });
    if model.colours_asked.len() >= COLOURS_ASKED {
        model.colours_asked.pop_front();
    }
    model.colours_asked.push_back(query_id);
    let mut effects = vec![effect];
    effects.extend(timer_effect(state, node, before));
    effects
}

/// The keys a key input is to the reading rules
/// (`docs/design/terminal-model.md`, "Keys"): a caret key by its unit, any
/// other key passed as `Other`, text typed by what it types, and a key that
/// clears the line as `Forget`, the caret key that follows it as `Other`.
pub(crate) fn key_input(state: &mut SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::CaretKey {
            trace_id,
            key: caret,
            pressed_at_ms,
        } => key(
            state,
            crate::terminal::caret_key_kind(*caret),
            *pressed_at_ms,
            *trace_id,
        ),
        Input::CharacterTyped {
            trace_id,
            text,
            at_ms,
        } => key(state, crate::terminal::typed_kind(text), *at_ms, *trace_id),
        Input::ClearingKey { trace_id, at_ms } => {
            key(state, TerminalKeyKind::Forget, *at_ms, *trace_id)
        }
        _ => Vec::new(),
    }
}

/// The reading rules' key for `kind`, reaching the program at `at_ms`.
fn rules_key(kind: &TerminalKeyKind, at_ms: u64) -> Key {
    Key {
        kind: match kind {
            TerminalKeyKind::Typed(text) => KeyKind::Typed(text.clone()),
            TerminalKeyKind::Caret(unit) => KeyKind::Caret(match unit {
                TerminalUnit::Character => Unit::Character,
                TerminalUnit::Word => Unit::Word,
                TerminalUnit::Line => Unit::Line,
            }),
            TerminalKeyKind::Enter => KeyKind::Enter,
            TerminalKeyKind::Forget => KeyKind::Forget,
            TerminalKeyKind::Delete => KeyKind::Delete,
            TerminalKeyKind::Other => KeyKind::Other,
        },
        at_ms,
    }
}

/// The reading rules' read of `snapshot`: its text as it came, without
/// its lines' padding, which the rules take as each line's length; the
/// screen and the rows, a screen at most, padded again.
#[must_use]
pub fn read_of(snapshot: &TerminalSnapshot) -> Read {
    Read {
        text: ReadText {
            text: snapshot.text.text.clone(),
            lengths: snapshot.text.lengths.clone(),
        },
        screen: snapshot.screen.padded(),
        from_start: snapshot.from_start,
        shrank: snapshot.shrank,
        at_document_start: snapshot.at_document_start,
        alternate: snapshot.alternate,
        first_row: snapshot.first_row.padded(),
        caret_rows_to_end: snapshot.caret_rows_to_end,
        caret_line: snapshot.caret_line.padded(),
        caret_column: usize::try_from(snapshot.caret_column).unwrap_or(usize::MAX),
        at_ms: snapshot.started_ms,
        above: usize::try_from(snapshot.above).unwrap_or(usize::MAX),
    }
}

/// The snapshot of `read`, as an outpost sends it: its texts without their
/// padding, with whether it waited for Core's speech queue and the colours
/// it took. For the tests and the measurements, which read the simulated
/// terminal as the outpost reads a real one.
#[must_use]
pub fn snapshot_of(
    read: &Read,
    waited: bool,
    colours: Option<&[Vec<ColourRun>]>,
) -> TerminalSnapshot {
    let cells = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    TerminalSnapshot {
        text: TerminalText {
            text: read.text.text.clone(),
            lengths: read.text.lengths.clone(),
        },
        screen: TerminalText::unpadded(&read.screen),
        from_start: read.from_start,
        shrank: read.shrank,
        at_document_start: read.at_document_start,
        alternate: read.alternate,
        first_row: TerminalText::unpadded(&read.first_row),
        caret_rows_to_end: read.caret_rows_to_end,
        caret_line: TerminalText::unpadded(&read.caret_line),
        caret_column: cells(read.caret_column),
        above: cells(read.above),
        started_ms: read.at_ms,
        waited,
        colours: colours.map(|rows| {
            rows.iter()
                .map(|runs| {
                    runs.iter()
                        .map(|run| TerminalColourRun {
                            column: cells(run.column),
                            length: cells(run.length),
                            foreground: run.foreground,
                            background: run.background,
                        })
                        .collect()
                })
                .collect()
        }),
        caret: None,
    }
}

/// The reading rules' colours of a snapshot.
fn colours_of(colours: &[Vec<TerminalColourRun>]) -> Vec<Vec<ColourRun>> {
    colours
        .iter()
        .map(|row| {
            row.iter()
                .map(|run| ColourRun {
                    column: run.column as usize,
                    length: run.length as usize,
                    foreground: run.foreground,
                    background: run.background,
                })
                .collect()
        })
        .collect()
}

/// What the reading rules said, as Core's flood policy takes it: the
/// speech's text moved, not copied, and a new line's left as the rules
/// share it with the read.
fn speech_of(step: Step) -> Said {
    let items = step
        .speech
        .into_iter()
        .map(|speech| {
            SaidItem::Item(match speech {
                Speech::Line { text, .. } => return SaidItem::Line(text),
                Speech::Changed { text, .. } | Speech::Held { text } => TerminalItem::Output(text),
                // What a line gained, without the space that parts it from the
                // text it follows, which says nothing.
                Speech::Growth { text, .. } => TerminalItem::Growth(text.trim_start().to_owned()),
                // After a lost read, its lines follow a count of none of how
                // they are counted, to which each adds when the flood policy
                // skips it ("Core's flood policy").
                Speech::LostRead(Lost::About) => TerminalItem::Skipped(Skipped::About(0)),
                Speech::LostRead(Lost::MoreThan) => TerminalItem::Skipped(Skipped::MoreThan(0)),
                Speech::LostRead(Lost::Uncounted) => TerminalItem::Skipped(Skipped::Uncounted),
                Speech::Echo { text, .. } => TerminalItem::Echo(text),
                Speech::Caret { unit, text, .. } => TerminalItem::Caret {
                    unit: match unit {
                        Unit::Character => TerminalUnit::Character,
                        Unit::Word => TerminalUnit::Word,
                        Unit::Line => TerminalUnit::Line,
                    },
                    text,
                },
                Speech::Gained { text, .. } => TerminalItem::Gained(text),
                Speech::Lost { text, .. } => TerminalItem::Lost(text),
                Speech::Answer { text, .. } => TerminalItem::Answer(text),
                Speech::Suggestion { text, .. } => TerminalItem::Suggestion(text),
            })
        })
        .collect();
    Said {
        items,
        followed: step.followed,
    }
}

/// What aligning a snapshot gave.
struct Taken {
    /// What the rules said, unless they asked for another read instead.
    speech: Option<Said>,
    /// The read's speech key.
    key: Option<TraceId>,
    /// The requests and the timer it caused.
    effects: Vec<Effect>,
}

/// Gives terminal `node`'s rules `snapshot`, after the keys that reached
/// the program by its start: asks for the whole document or the history
/// block when the rules ask, keeps the caret and the shape the read found,
/// tells the outpost a new shape, and takes the read's caret as Core's.
fn take_snapshot(state: &mut SrState, node: NodeId, snapshot: &TerminalSnapshot) -> Taken {
    let started_ms = snapshot.started_ms;
    let model = model_or_new(state, node);
    let before = model.due_ms;
    model.give_keys(started_ms);
    let read = read_of(snapshot);
    let colours = snapshot.colours.as_deref().map(colours_of);
    let step = model
        .rules()
        .read(&read, snapshot.waited, colours.as_deref());
    model.due_ms = step.timer;
    let key = model.speech_key(started_ms);
    let height = step.outcome.as_ref().map_or(0, |outcome| {
        u32::try_from(outcome.height).unwrap_or(u32::MAX)
    });
    let mut effects = Vec::new();
    if step.whole_document || step.history_block {
        let scope = if step.whole_document {
            TerminalScope::Whole
        } else {
            TerminalScope::HistoryBlock { height }
        };
        let (query_id, effect) = request(state, node, TextOp::TerminalReadNow(scope));
        if let Some(model) = model_mut(state, node) {
            model.owed = Some(if step.whole_document {
                Owed::Whole(query_id)
            } else {
                Owed::HistoryBlock(query_id)
            });
        }
        effects.push(effect);
        effects.extend(timer_effect(state, node, before));
        return Taken {
            speech: None,
            key,
            effects,
        };
    }
    let mut shape = None;
    if let Some(outcome) = &step.outcome {
        model.height = height;
        model.width = outcome.width;
        model.caret = outcome.caret.map(|row| CaretAt {
            row,
            column: read.caret_column,
        });
        let width = u32::try_from(outcome.width).unwrap_or(u32::MAX);
        if model.told != Some((height, width)) && !outcome.blank {
            model.told = Some((height, width));
            shape = Some(TextOp::TerminalShape { height, width });
        }
    }
    model.at_document_start = read.at_document_start;
    if let Some(op) = shape {
        effects.push(request(state, node, op).1);
    }
    effects.extend(timer_effect(state, node, before));
    if let Some(caret) = &snapshot.caret {
        crate::editing::update_caret(state, node, caret.clone(), started_ms);
    }
    Taken {
        speech: Some(speech_of(step)),
        key,
        effects,
    }
}

/// Hands what a read said to the flood policy, under its speech key when
/// it has one, as a live read's speech is.
fn speak(
    state: &mut SrState,
    trace_id: TraceId,
    (node, started_ms): (NodeId, u64),
    speech: Said,
    key: Option<TraceId>,
) -> Vec<Effect> {
    let effects = crate::terminal::speech(state, key.unwrap_or(trace_id), started_ms, node, speech);
    under(key, effects)
}

/// `effects`, each on behalf of `key` when there is one.
pub(crate) fn under(key: Option<TraceId>, effects: Vec<Effect>) -> Vec<Effect> {
    match key {
        Some(key) => effects
            .into_iter()
            .map(|effect| Effect::ForKey(key, Box::new(effect)))
            .collect(),
        None => effects,
    }
}

/// A live read of terminal `node` (`NormalizedEvent::TerminalSnapshot`):
/// taken only for the focus, and dropped while a read the rules asked for
/// is owed.
pub(crate) fn snapshot_event(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    snapshot: &TerminalSnapshot,
) -> Vec<Effect> {
    if !state.focus_matches(node) || crate::terminal::terminal_focus(state) != Some(node) {
        return Vec::new();
    }
    if model_of(state, node).is_some_and(|model| model.owed.is_some()) {
        return Vec::new();
    }
    let taken = take_snapshot(state, node, snapshot);
    let mut effects = taken.effects;
    if let Some(speech) = taken.speech {
        effects.extend(speak(
            state,
            trace_id,
            (node, snapshot.started_ms),
            speech,
            taken.key,
        ));
    }
    effects
}

/// The speech of a snapshot answering the flood policy's own request
/// (`TextOp::TerminalRead` or `TextOp::TerminalCancel`), the requests it
/// caused, and its speech key; `None` for any other answer.
pub(crate) fn answered_read(
    state: &mut SrState,
    reply: &TextReply,
) -> Option<(Said, Option<TraceId>, Vec<Effect>)> {
    let node = state.terminal.node?;
    // While a read the rules asked for is owed, a read begun before it
    // says nothing: the owed read holds everything it showed.
    if model_of(state, node).is_some_and(|model| model.owed.is_some()) {
        return Some((Said::default(), None, Vec::new()));
    }
    match reply {
        TextReply::TerminalSnapshot(snapshot) => {
            let taken = take_snapshot(state, node, snapshot);
            Some((taken.speech.unwrap_or_default(), taken.key, taken.effects))
        }
        TextReply::TerminalUnchanged => Some((Said::default(), None, Vec::new())),
        _ => None,
    }
}

/// The read before a held key in terminal `node`
/// (`docs/design/key-holding.md`), a snapshot: aligned as a live read is,
/// and its caret given as the answer [`crate::editing::held_read`] takes, as
/// read when the read began, since Core takes a read's caret as of its
/// start. Without a caret, the read is unanswered.
pub(crate) fn before_key(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    snapshot: &TerminalSnapshot,
) -> (Vec<Effect>, TextReply) {
    let effects = snapshot_event(state, trace_id, node, snapshot);
    let reply = match &snapshot.caret {
        Some(caret) => TextReply::Caret(Box::new(CaretReply {
            moved: false,
            caret: caret.clone(),
            read_at_ms: snapshot.started_ms,
            unit: None,
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
            same_line: None,
            removed: None,
        })),
        None => TextReply::Unanswered,
    };
    (effects, reply)
}

/// Whether `query_id` is a request of a terminal model Core waits on.
pub(crate) fn is_pending(state: &SrState, query_id: QueryId) -> bool {
    state.terminals.iter().any(|model| {
        model.owed.is_some_and(|owed| owed.query() == query_id)
            || model.fresh == Some(query_id)
            || model
                .history_asked
                .is_some_and(|(asked, ..)| asked == query_id)
            || model.colours_asked.contains(&query_id)
    })
}

/// Handles the answer to a terminal model's request.
pub(crate) fn reply(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    reply: TextReply,
) -> Vec<Effect> {
    let Some(model) = state.terminals.iter_mut().find(|model| {
        model.owed.is_some_and(|owed| owed.query() == query_id)
            || model.fresh == Some(query_id)
            || model
                .history_asked
                .is_some_and(|(asked, ..)| asked == query_id)
            || model.colours_asked.contains(&query_id)
    }) else {
        return Vec::new();
    };
    let node = model.node;
    if model.owed.is_some_and(|owed| owed.query() == query_id) {
        model.owed = None;
        let mut effects = match reply {
            TextReply::TerminalSnapshot(snapshot) => {
                snapshot_event(state, trace_id, node, &snapshot)
            }
            _ => Vec::new(),
        };
        // Review answers that waited for it.
        if model_of(state, node).is_some_and(|model| {
            model.owed.is_none()
                && model.fresh.is_none()
                && model.history_asked.is_none()
                && !model.reviews.is_empty()
        }) {
            effects.extend(answer_reviews(state, node, trace_id));
        }
        return effects;
    }
    if model.colours_asked.contains(&query_id) {
        model.colours_asked.retain(|asked| *asked != query_id);
        return match reply {
            TextReply::TerminalSnapshot(snapshot) => {
                snapshot_event(state, trace_id, node, &snapshot)
            }
            _ => Vec::new(),
        };
    }
    let mut effects = Vec::new();
    if model.fresh == Some(query_id) {
        model.fresh = None;
        // A read owed by the rules first: this one, begun before it, is
        // dropped as a live one is, and the review answers wait for it.
        if model.owed.is_some() {
            return effects;
        }
        if let TextReply::TerminalSnapshot(snapshot) = reply {
            let taken = take_snapshot(state, node, &snapshot);
            effects.extend(taken.effects);
            if let Some(speech) = taken.speech
                && state.focus_matches(node)
            {
                effects.extend(speak(
                    state,
                    trace_id,
                    (node, snapshot.started_ms),
                    speech,
                    taken.key,
                ));
            }
        }
        // The read asked for another first: the review answers wait for it.
        if model_of(state, node).is_some_and(|model| model.owed.is_some()) {
            return effects;
        }
    } else if let Some((_, above, whole)) = model.history_asked.take() {
        let (text, rows_above) = match reply {
            TextReply::TerminalHistory { text, above } => (text, above),
            _ => (String::new(), 0),
        };
        model.history.push(HistoryRead {
            above,
            whole,
            text,
            rows_above,
        });
    }
    effects.extend(answer_reviews(state, node, trace_id));
    effects
}

/// The trace of an input, for a request it causes that Core answers later.
pub(crate) fn input_trace(input: &Input) -> Option<TraceId> {
    match input {
        Input::Event { trace_id, .. }
        | Input::FetchCompleted { trace_id, .. }
        | Input::Command { trace_id, .. }
        | Input::ActivationCompleted { trace_id, .. }
        | Input::TextCompleted { trace_id, .. }
        | Input::CaretKey { trace_id, .. }
        | Input::HeldKey { trace_id, .. }
        | Input::HeldKeyWaitEnded { trace_id }
        | Input::CharacterTyped { trace_id, .. }
        | Input::ClearingKey { trace_id, .. }
        | Input::TerminalTimer { trace_id, .. } => Some(*trace_id),
        _ => None,
    }
}

/// The request `effect` makes of a terminal Core models that Core answers
/// from its memory.
fn answerable(state: &SrState, effect: &Effect) -> Option<(QueryId, NodeId, TextOp)> {
    match effect {
        Effect::Text(TextRequest {
            query_id,
            node_id,
            op,
        }) => {
            let model = model_of(state, *node_id)?;
            review::answers(model, op).then(|| (*query_id, *node_id, op.clone()))
        }
        Effect::ForKey(_, effect) => answerable(state, effect),
        _ => None,
    }
}

/// `effects` with each request for a terminal's text that Core answers from
/// its memory taken out and kept for the answer, after a fresh read that
/// Core asks for here.
pub(crate) fn intercept(
    state: &mut SrState,
    effects: Vec<Effect>,
    trace: Option<TraceId>,
) -> Vec<Effect> {
    if state.terminals.is_empty() {
        return effects;
    }
    let mut kept = Vec::with_capacity(effects.len());
    for effect in effects {
        let Some((query_id, node, op)) = answerable(state, &effect) else {
            kept.push(effect);
            continue;
        };
        let fresh = {
            let Some(model) = model_mut(state, node) else {
                continue;
            };
            model.reviews.push_back(Review {
                query_id,
                trace,
                op,
            });
            model.fresh.is_none() && model.history_asked.is_none()
        };
        if fresh {
            let (fresh_query, effect) =
                request(state, node, TextOp::TerminalReadNow(TerminalScope::Fresh));
            if let Some(model) = model_mut(state, node) {
                model.fresh = Some(fresh_query);
            }
            kept.push(effect);
        }
    }
    kept
}

/// Answers terminal `node`'s waiting requests from its memory, in order,
/// until one needs rows of the history that have not come, which are then
/// asked for. A request an answer causes is answered in turn. An answer
/// whose request came with no trace is spoken under `trace_id`, the trace
/// of the read or history that came.
fn answer_reviews(state: &mut SrState, node: NodeId, trace_id: TraceId) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let held = held_anchors(state, node);
        let mut mint = state.next_terminal_anchor;
        let Some(model) = model_mut(state, node) else {
            break;
        };
        let Some(waiting) = model.reviews.front().cloned() else {
            model.history.clear();
            break;
        };
        let answer = review::answer(model, &waiting.op, &mut mint, &held);
        state.next_terminal_anchor = mint;
        match answer {
            Ok(reply) => {
                if let Some(model) = model_mut(state, node) {
                    model.reviews.pop_front();
                }
                let trace = waiting.trace.unwrap_or(trace_id);
                let caused = crate::reduce::text_completed(state, trace, waiting.query_id, reply);
                effects.extend(queue_only(state, caused, trace));
            }
            Err(review::Need { above, whole }) => {
                let (query_id, effect) =
                    request(state, node, TextOp::TerminalHistory { above, whole });
                if let Some(model) = model_mut(state, node) {
                    model.history_asked = Some((query_id, above, whole));
                }
                effects.push(effect);
                break;
            }
        }
    }
    effects
}

/// `effects` with each request Core answers from a terminal's memory kept
/// for [`answer_reviews`], which is answering already, with no fresh read.
fn queue_only(state: &mut SrState, effects: Vec<Effect>, trace: TraceId) -> Vec<Effect> {
    let mut kept = Vec::with_capacity(effects.len());
    for effect in effects {
        match answerable(state, &effect) {
            Some((query_id, node, op)) => {
                if let Some(model) = model_mut(state, node) {
                    model.reviews.push_back(Review {
                        query_id,
                        trace: Some(trace),
                        op,
                    });
                }
            }
            None => kept.push(effect),
        }
    }
    kept
}

/// The anchors Core holds of terminal `node`'s outpost.
fn held_anchors(state: &SrState, node: NodeId) -> std::collections::HashSet<u64> {
    state
        .held_anchors()
        .remove(&node.outpost())
        .unwrap_or_default()
        .into_iter()
        .map(|anchor| anchor.0)
        .collect()
}

/// The quiet-period timer came due at `at_ms`: the focused terminal's rules
/// are given the keys that reached the program by then, then the timer.
pub(crate) fn timer(state: &mut SrState, trace_id: TraceId, at_ms: u64) -> Vec<Effect> {
    let Some(node) = focused(state) else {
        return Vec::new();
    };
    let Some(model) = model_mut(state, node) else {
        return Vec::new();
    };
    let before = model.due_ms;
    model.give_keys(at_ms);
    let step = model.rules().timer(at_ms);
    model.due_ms = step.timer;
    let mut effects: Vec<Effect> = timer_effect(state, node, before).into_iter().collect();
    effects.extend(speak(state, trace_id, (node, at_ms), speech_of(step), None));
    effects
}

/// The focus moved from `from` to `to` at `at_ms`: a terminal Core models
/// that lost it is told so and its waiting keys dropped; one that gained
/// it is told so, and the timer follows the focus.
pub(crate) fn focus_moved(
    state: &mut SrState,
    (from, to): (Option<NodeId>, Option<NodeId>),
    at_ms: u64,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(from) = from
        && let Some(model) = model_mut(state, from)
    {
        let had = model.due_ms.is_some();
        let step = model.rules().blur();
        model.due_ms = step.timer;
        model.pending.clear();
        model.last_key = None;
        if had {
            effects.push(Effect::TerminalTimer { due_ms: None });
        }
    }
    if let Some(to) = to
        && let Some(model) = model_mut(state, to)
    {
        let step = model.rules().focus(at_ms);
        model.due_ms = step.timer;
        if let Some(due_ms) = model.due_ms {
            effects.push(Effect::TerminalTimer {
                due_ms: Some(due_ms),
            });
        }
    }
    effects
}

/// Gives every model's rules the reader settings when they changed:
/// "report new output" and "review follows caret".
pub(crate) fn sync_settings(state: &mut SrState) -> Vec<Effect> {
    let settings = settings_of(state);
    let focus = focused(state);
    let mut effects = Vec::new();
    for model in &mut state.terminals {
        if model.reader.settings() != settings {
            let before = model.due_ms;
            let step = model.rules().set_settings(settings);
            model.due_ms = step.timer;
            if Some(model.node) == focus && model.due_ms != before {
                effects.push(Effect::TerminalTimer {
                    due_ms: model.due_ms,
                });
            }
        }
    }
    effects
}

/// An outpost ended: the terminals it read are forgotten.
pub(crate) fn outpost_ended(state: &mut SrState, outpost: OutpostId) {
    state
        .terminals
        .retain(|model| model.node.outpost() != outpost);
}

#[cfg(test)]
mod tests;
