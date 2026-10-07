//! The outpost's side of the text protocol (milestone M4,
//! `docs/crates/verbatim-model.md`, "The text protocol").
//!
//! [`perform`] carries out one [`TextOp`] on a [`TextSource`], the
//! backend-neutral view of one node's text: UIA's `TextPattern`
//! ([`uia::UiaText`]) or a Win32 edit control's window messages
//! ([`edit::EditText`]). A source works in its own positions and in UTF-16;
//! this module converts at the boundary to the protocol's UTF-8 chunks and
//! opaque anchors, which [`Anchors`] keeps.
//!
//! The rules it implements, from the contract:
//!
//! - A [`TextAnchor`] names a position the outpost minted; a
//!   [`TextPosition`] is an anchor plus a byte offset into the chunk that
//!   started there. A position is resolved by moving forward from the anchor
//!   over that chunk's text, converted to UTF-16, except for positions the
//!   outpost itself reported (a caret, a selection's ends, the point of a
//!   read), which it remembers as they are.
//! - Anchors Core holds are kept; any other is forgotten once 64 newer ones
//!   were minted for the same node, and a request naming it is answered
//!   [`TextReply::AnchorLost`].
//! - A chunk is at most [`MAX_CHUNK_BYTES`], cut at a character boundary.
//! - A unit the source does not have is answered
//!   [`TextReply::UnsupportedUnit`], and movement stops at the document's
//!   ends, reporting how far it really went.
//! - [`TextOp::AwaitCaret`] waits for evidence that a caret key did
//!   something, as NVDA's caret scripts do
//!   (`docs/nvda/editable-text-and-terminals.md`): a caret event, the caret
//!   no longer where Core last knew it, the text at the caret changed, or the
//!   selection changed. It polls the caret every
//!   [`CARET_POLL`] between caret events, as NVDA polls, until the wait for
//!   the request's [`CaretWait`] runs out.
//!
//! Everything here is safe code: the sources call into the application only
//! through the backend crates' safe wrappers.

#![forbid(unsafe_code)]

mod color;
pub mod edit;
pub mod uia;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use verbatim_model::{
    CaretReply, CaretReport, CaretWait, CaretWatch, FormatRun, LanguageRun, MAX_CHUNK_BYTES,
    MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT, MAX_SELECTION_TEXT_BYTES,
    PreviousSelection, Selection, SelectionChange, TextAnchor, TextAttributes, TextChunk,
    TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReadAhead, TextReply, TextUnit,
};

/// How often the caret is read again while a caret key's wait for evidence
/// runs and no caret event arrives: NVDA's 10 ms retry interval.
pub const CARET_POLL: Duration = Duration::from_millis(10);

/// How many of its newest caret reports for a node the outpost remembers,
/// with when it read them, for a caret key's wait to find the one read
/// before the key.
const REMEMBERED_CARETS: usize = 8;

/// How many anchors a node keeps beyond those Core holds.
pub const KEPT_ANCHORS: usize = 64;

/// The most UTF-16 code units read for a range copied to the clipboard or a
/// selection change counted: what fits in [`MAX_RANGE_BYTES`] at worst.
pub(crate) const MAX_RANGE_UNITS: usize = MAX_RANGE_BYTES / 2;

/// The most UTF-16 code units read for one chunk.
pub(crate) const MAX_CHUNK_UNITS: usize = MAX_CHUNK_BYTES;

/// The formatting of a stretch of a unit's text: a start and an end UTF-16
/// offset into the unit's text, and the attributes there.
pub type Formatting = (usize, usize, TextAttributes);

/// Whose formatting a caret read reads ([`CaretRequest::formats`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatSpan {
    /// The character at the caret, one stretch.
    Character,
    /// The unit read besides the line ([`CaretRequest::unit`]).
    Unit,
    /// The caret's line.
    Line,
}

/// What a caret read covers, for [`TextSource::caret_read`]: everything a
/// caret report or a caret key's answer needs, so a source that can read it
/// in one go does.
pub struct CaretRequest<'a, P> {
    /// Where the caret was known to be, to compare it with.
    pub since: Option<&'a P>,
    /// The selection's ends as they were known (both the caret when nothing
    /// was selected), to compare it with.
    pub previous: Option<&'a (P, P)>,
    /// A unit to read at the caret besides its line: a word, a paragraph, a
    /// page.
    pub unit: Option<TextUnit>,
    /// Whose formatting to read, with the attributes the theme asks for.
    pub formats: Option<FormatSpan>,
}

/// The answer to a [`CaretRequest`].
pub struct CaretRead<P> {
    /// The caret and the selection.
    pub state: CaretState<P>,
    /// The caret is not where [`CaretRequest::since`] says.
    pub moved: bool,
    /// The selection is not what [`CaretRequest::previous`] says.
    pub selection_moved: bool,
    /// The caret's line and the caret's UTF-16 offset in it.
    pub line: (Unit<P>, usize),
    /// The request's unit at the caret, with the caret's offset in it.
    pub unit: Option<(Unit<P>, usize)>,
    /// The formatting of the request's span, UTF-16 ranges of its text.
    pub formats: Vec<Formatting>,
    /// With [`CaretRequest::previous`], how the selection changed from it
    /// (each change selected or not, and its UTF-16 text), read with the
    /// rest; `None` when the source did not read them, which the caller
    /// then does.
    pub changes: Option<Vec<(bool, Vec<u16>)>>,
}

/// A point as a source is asked to find it: the protocol's [`TextPoint`],
/// with a position Core named turned into what the outpost keeps, with no
/// call.
#[derive(Clone, Debug)]
pub enum PointFrom<P> {
    /// The caret.
    Caret,
    /// The selection's start, the caret when nothing is selected.
    SelectionStart,
    /// The selection's end, the caret when nothing is selected.
    SelectionEnd,
    /// The start of the text.
    Start,
    /// The end of the text.
    End,
    /// A position the outpost keeps: an anchor's own, or one it reported.
    At(P),
    /// The position `prefix.len()` UTF-16 code units after a kept one,
    /// `prefix` being the text between them as the outpost sent it
    /// ([`TextSource::advance`]).
    After(P, Vec<u16>),
}

/// What [`TextSource::read_units`] reads.
pub struct UnitsRequest<'a, P> {
    /// Where to start.
    pub from: &'a PointFrom<P>,
    /// How to move first: by a unit the source may not have, or to an end
    /// of the text (`TextUnit::Document`).
    pub movement: Option<TextMovement>,
    /// The unit to read (never a sentence where the source has none, nor
    /// the document).
    pub unit: TextUnit,
    /// How many units: one for a read, more for reading ahead.
    pub count: u32,
}

/// One unit [`TextSource::read_units`] read.
pub struct UnitRead<P> {
    /// The unit.
    pub unit: Unit<P>,
    /// The UTF-16 offset of the point reached in it: zero for every unit
    /// after the first.
    pub offset: usize,
    /// Its languages, as [`TextSource::languages`] gives them.
    pub languages: Vec<(usize, usize, String)>,
}

/// The answer of [`TextSource::read_units`].
pub struct UnitsRead<P> {
    /// The starting point, found.
    pub from: P,
    /// The point the movement reached.
    pub point: P,
    /// How far the movement went.
    pub moved: i32,
    /// The units, in order, at least one.
    pub units: Vec<UnitRead<P>>,
    /// No unit follows the last one read.
    pub ended: bool,
}

/// What [`TextSource::read_units`] answers: `None` when the source reads
/// call by call, or the units, or the reply that ends the request.
pub type UnitsAnswer<P> = Option<Result<UnitsRead<P>, TextReply>>;

/// What [`TextSource::point_location`] answers: `None` when the source
/// reads call by call, or the point found and its screen position, `None`
/// when the source cannot tell.
pub type Located<P> = Option<(P, Option<(i32, i32)>)>;

/// What [`TextSource::range`] does with the text between two points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeAction {
    /// Reads it, at most this many UTF-16 code units.
    Text(usize),
    /// Selects it.
    Select,
}

/// The answer of [`TextSource::range`].
pub struct RangeRead<P> {
    /// The first point, found.
    pub start: P,
    /// The second point, found.
    pub end: P,
    /// The text read, for [`RangeAction::Text`].
    pub text: Vec<u16>,
    /// Whether the selection was made, for [`RangeAction::Select`].
    pub selected: bool,
}

/// Why a source could not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    /// The node no longer exists.
    Gone,
    /// The application did not answer in time, or the read failed.
    Failed(String),
}

/// What reading text can fail with.
pub type TextResult<T> = Result<T, TextError>;

/// A value read, or the reply that ends the request instead (a forgotten
/// anchor, an unsupported unit).
type OrReply<T> = TextResult<Result<T, TextReply>>;

/// The caret and the selection as a source reads them.
#[derive(Clone, Debug)]
pub struct CaretState<P> {
    /// The caret; the start of the text when the node has no caret.
    pub caret: P,
    /// The selection's start and end, when something is selected.
    pub selection: Option<(P, P)>,
}

/// One unit of a source's text: where it starts and ends, and its text in
/// UTF-16, read up to a limit.
#[derive(Clone, Debug)]
pub struct Unit<P> {
    /// Where the unit starts.
    pub start: P,
    /// Where it ends.
    pub end: P,
    /// Its text, UTF-16, at most the limit asked for.
    pub text: Vec<u16>,
    /// The text was cut at the limit.
    pub truncated: bool,
}

/// How a source treats sentences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sentences {
    /// It has no sentence unit (UIA): a sentence read is answered
    /// [`TextReply::UnsupportedUnit`], and Core reads by line.
    Unsupported,
    /// Its text is offsets that Core splits into sentences itself (the
    /// Win32 edit controls): a sentence read is answered with the paragraph.
    ByParagraph,
}

/// One node's text, as a backend reads it. Positions are the backend's own
/// (`Pos`); every text is UTF-16.
pub trait TextSource {
    /// A position in the text.
    type Pos: Clone;

    /// The caret and the selection.
    ///
    /// # Errors
    ///
    /// When the application does not answer or the node is gone.
    fn caret(&mut self) -> TextResult<CaretState<Self::Pos>>;

    /// The start of the text.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn start(&mut self) -> TextResult<Self::Pos>;

    /// The end of the text, after its last character.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn end(&mut self) -> TextResult<Self::Pos>;

    /// The `unit` containing `at`, its text read up to `max_units` code
    /// units; `None` when the source has no such unit. At the end of the
    /// text, the last unit.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn unit_at(
        &mut self,
        at: &Self::Pos,
        unit: TextUnit,
        max_units: usize,
    ) -> TextResult<Option<Unit<Self::Pos>>>;

    /// Moves from the start of the `unit` containing `at` by `count` units,
    /// landing on a unit's start, never past the text's ends. Returns where
    /// it landed and how far it went, or `None` when the source has no such
    /// unit.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn move_by(
        &mut self,
        at: &Self::Pos,
        unit: TextUnit,
        count: i32,
    ) -> TextResult<Option<(Self::Pos, i32)>>;

    /// The text from `start` to `end`, at most `max_units` code units, and
    /// whether it was cut there.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn text(
        &mut self,
        start: &Self::Pos,
        end: &Self::Pos,
        max_units: usize,
    ) -> TextResult<(Vec<u16>, bool)>;

    /// How many code units `at` lies after the start of `unit`, at most the
    /// length of its text.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn offset_in(&mut self, unit: &Unit<Self::Pos>, at: &Self::Pos) -> TextResult<usize>;

    /// The position `prefix.len()` code units after `from`, where `prefix` is
    /// the text between them as it was read.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn advance(&mut self, from: &Self::Pos, prefix: &[u16]) -> TextResult<Self::Pos>;

    /// How `a` compares with `b` in the text.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn compare(&mut self, a: &Self::Pos, b: &Self::Pos) -> TextResult<Ordering>;

    /// Selects from `start` to `end` and puts the caret there; `false` when
    /// the text cannot be selected.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn select(&mut self, start: &Self::Pos, end: &Self::Pos) -> TextResult<bool>;

    /// The screen position of `at`, `None` when the source cannot tell.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn location(&mut self, at: &Self::Pos) -> TextResult<Option<(i32, i32)>>;

    /// The languages of spans of `unit`, as UTF-16 ranges of its text and
    /// BCP 47 tags; empty when the source reports none.
    fn languages(&mut self, unit: &Unit<Self::Pos>) -> Vec<(usize, usize, String)>;

    /// Whether `unit` (of the kind `kind`) is known, cheaply, to be the
    /// text's first and last of its kind.
    fn edges(&mut self, unit: &Unit<Self::Pos>, kind: TextUnit) -> (bool, bool);

    /// How the source treats sentences.
    fn sentences(&self) -> Sentences;

    /// Reads the caret and everything `request` asks for at once, where the
    /// source can do it in one round trip (UIA's remote operations, with
    /// its classic reads behind them); `None` for a source that reads them
    /// one at a time, which the caller then does, reading no formatting.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn caret_read(
        &mut self,
        _request: &CaretRequest<'_, Self::Pos>,
    ) -> TextResult<Option<CaretRead<Self::Pos>>> {
        Ok(None)
    }

    /// Finds a point, moves, and reads units at once, where the source can
    /// do it in one round trip, answering an unsupported unit with its
    /// reply; `None` for a source that reads them one call at a time, which
    /// the caller then does with the methods above.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn read_units(
        &mut self,
        _request: &UnitsRequest<'_, Self::Pos>,
    ) -> TextResult<UnitsAnswer<Self::Pos>> {
        Ok(None)
    }

    /// Finds two points (the second the first again when `None`) and reads
    /// or selects the text between them, whichever comes first, at once
    /// where the source can; `None` as for [`read_units`](Self::read_units).
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn range(
        &mut self,
        _start: &PointFrom<Self::Pos>,
        _end: Option<&PointFrom<Self::Pos>>,
        _action: RangeAction,
    ) -> TextResult<Option<RangeRead<Self::Pos>>> {
        Ok(None)
    }

    /// Finds a point and its screen position at once where the source can;
    /// `None` as for [`read_units`](Self::read_units).
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn point_location(&mut self, _at: &PointFrom<Self::Pos>) -> TextResult<Located<Self::Pos>> {
        Ok(None)
    }
}

/// Caret events, and the clock, for a caret key's wait for evidence.
pub trait CaretSignal {
    /// Whether a caret event for the node arrived since the wait began.
    fn caret_event(&mut self) -> bool;

    /// Waits until a caret event arrives or `timeout` passes.
    fn wait(&mut self, timeout: Duration);

    /// The time now.
    fn now(&mut self) -> Instant;

    /// Milliseconds since the Unix epoch: the clock outposts stamp events'
    /// `observed_at_ms` with and the hook stamps a caret key's
    /// `pressed_at_ms` with.
    fn now_ms(&mut self) -> u64;

    /// The caret is about to be read: a caret event observed before now
    /// changed nothing the read will not see.
    fn reading(&mut self) {}

    /// The wait has ended, with evidence or at its deadline, and the reads
    /// for the reply follow: where the latency log divides the caret wait
    /// from the read.
    fn awaited(&mut self) {}
}

/// An anchor's position and the text of the chunk it started.
struct Anchor<P> {
    pos: P,
    text: Arc<str>,
}

/// The anchors minted for one node, and the positions the outpost reported
/// in its text.
pub struct NodeAnchors<P> {
    anchors: BTreeMap<u64, Anchor<P>>,
    reported: HashMap<(u64, u32), P>,
    /// The carets the outpost last reported for the node, oldest first,
    /// each with when it was read, in milliseconds since the Unix epoch.
    carets: VecDeque<(u64, TextPosition)>,
}

impl<P> Default for NodeAnchors<P> {
    fn default() -> Self {
        Self {
            anchors: BTreeMap::new(),
            reported: HashMap::new(),
            carets: VecDeque::new(),
        }
    }
}

/// Every node's anchors in one backend, the anchors Core holds, and the
/// counter anchors are numbered from, which an outpost's backends share so
/// no two anchors in an outpost have the same number.
pub struct Anchors<P> {
    counter: Arc<AtomicU64>,
    held: HashSet<u64>,
    nodes: HashMap<u64, NodeAnchors<P>>,
}

impl<P: Clone> Anchors<P> {
    /// An empty store numbering anchors from `counter`.
    #[must_use]
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            counter,
            held: HashSet::new(),
            nodes: HashMap::new(),
        }
    }

    /// Records the anchors Core holds; the rest may be forgotten.
    pub fn set_held(&mut self, held: impl IntoIterator<Item = u64>) {
        self.held = held.into_iter().collect();
    }

    /// Forgets a node's anchors, when the node is released.
    pub fn forget_node(&mut self, node: u64) {
        self.nodes.remove(&node);
    }

    /// The anchors of node `node`, with what minting needs.
    pub fn node(&mut self, node: u64) -> NodeText<'_, P> {
        NodeText {
            counter: &self.counter,
            held: &self.held,
            anchors: self.nodes.entry(node).or_default(),
        }
    }
}

/// One node's anchors, borrowed from an [`Anchors`] store.
pub struct NodeText<'a, P> {
    counter: &'a AtomicU64,
    held: &'a HashSet<u64>,
    anchors: &'a mut NodeAnchors<P>,
}

impl<P: Clone> NodeText<'_, P> {
    /// Mints an anchor at `pos` for a chunk whose text is `text`, forgetting
    /// the oldest anchors Core does not hold beyond [`KEPT_ANCHORS`].
    fn mint(&mut self, pos: P, text: &str) -> TextAnchor {
        let number = self.counter.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        self.anchors.anchors.insert(
            number,
            Anchor {
                pos,
                text: Arc::from(text),
            },
        );
        let count = self.anchors.anchors.len();
        if count > KEPT_ANCHORS {
            let forgotten: Vec<u64> = self
                .anchors
                .anchors
                .keys()
                .take(count - KEPT_ANCHORS)
                .copied()
                .filter(|number| !self.held.contains(number))
                .collect();
            let NodeAnchors {
                anchors, reported, ..
            } = &mut *self.anchors;
            for number in &forgotten {
                anchors.remove(number);
            }
            reported.retain(|(anchor, _), _| anchors.contains_key(anchor));
        }
        TextAnchor(number)
    }

    /// The text of the chunk `anchor` started, when the anchor is kept.
    fn anchor_text(&self, anchor: TextAnchor) -> Option<Arc<str>> {
        self.anchors
            .anchors
            .get(&anchor.0)
            .map(|kept| Arc::clone(&kept.text))
    }

    /// Remembers that `position` is `pos`, a position the outpost reported.
    fn remember(&mut self, position: TextPosition, pos: P) {
        self.anchors
            .reported
            .insert((position.anchor.0, position.offset), pos);
    }

    /// A position minted for `pos` alone, a new anchor there at offset 0,
    /// remembered as one the outpost reported. No call.
    pub fn position_at(&mut self, pos: P) -> TextPosition {
        self.position_of(pos)
    }

    /// A position minted for `pos` alone: a new anchor there, offset 0.
    fn position_of(&mut self, pos: P) -> TextPosition {
        let anchor = self.mint(pos.clone(), "");
        let position = TextPosition::at(anchor);
        self.remember(position, pos);
        position
    }

    /// What `position` names, with no call: a position kept, or one some
    /// text after a kept one; `None` when its anchor was forgotten.
    fn point_from(&self, position: TextPosition) -> Option<PointFrom<P>> {
        let key = (position.anchor.0, position.offset);
        if let Some(pos) = self.anchors.reported.get(&key) {
            return Some(PointFrom::At(pos.clone()));
        }
        let anchor = self.anchors.anchors.get(&position.anchor.0)?;
        if position.offset == 0 {
            return Some(PointFrom::At(anchor.pos.clone()));
        }
        let offset = floor_boundary(&anchor.text, position.offset as usize);
        let prefix: Vec<u16> = anchor.text[..offset].encode_utf16().collect();
        Some(PointFrom::After(anchor.pos.clone(), prefix))
    }

    /// Resolves `position` to a source position; `None` when its anchor was
    /// forgotten.
    fn resolve<S: TextSource<Pos = P>>(
        &mut self,
        source: &mut S,
        position: TextPosition,
    ) -> TextResult<Option<P>> {
        let key = (position.anchor.0, position.offset);
        if let Some(pos) = self.anchors.reported.get(&key) {
            return Ok(Some(pos.clone()));
        }
        let Some(anchor) = self.anchors.anchors.get(&position.anchor.0) else {
            return Ok(None);
        };
        if position.offset == 0 {
            return Ok(Some(anchor.pos.clone()));
        }
        let text = Arc::clone(&anchor.text);
        let from = anchor.pos.clone();
        let offset = floor_boundary(&text, position.offset as usize);
        let prefix: Vec<u16> = text[..offset].encode_utf16().collect();
        let pos = source.advance(&from, &prefix)?;
        self.anchors.reported.insert(key, pos.clone());
        Ok(Some(pos))
    }
}

/// The largest character boundary of `text` at or before `offset`.
fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// UTF-16 text as UTF-8, cut at a character boundary within `max_bytes`,
/// with the byte offset of each UTF-16 offset asked for in `offsets`
/// (clamped to the text kept), and whether it was cut.
fn to_utf8(units: &[u16], max_bytes: usize, offsets: &[usize]) -> (String, Vec<usize>, bool) {
    let mut text = String::new();
    let mut mapped = vec![None; offsets.len()];
    let mut consumed = 0;
    let mut truncated = false;
    for character in char::decode_utf16(units.iter().copied()) {
        for (index, &wanted) in offsets.iter().enumerate() {
            if mapped[index].is_none() && wanted <= consumed {
                mapped[index] = Some(text.len());
            }
        }
        let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
        let width = match character {
            // A replacement for a lone surrogate stands for one unit.
            char::REPLACEMENT_CHARACTER => 1,
            other => other.len_utf16(),
        };
        if text.len() + character.len_utf8() > max_bytes {
            truncated = true;
            // Cut between whole characters: back to the last grapheme
            // boundary unless the character left out starts a cluster.
            let mut probe = text.clone();
            probe.push(character);
            text.truncate(grapheme_floor(&probe, text.len()));
            break;
        }
        text.push(character);
        consumed += width;
    }
    let end = text.len();
    let mapped = mapped
        .into_iter()
        .map(|offset| offset.map_or(end, |offset| offset.min(end)))
        .collect();
    (text, mapped, truncated)
}

/// Cuts `text`, which a read limit cut short, back to the end of its last
/// whole character, so a surrogate pair or a letter with its marks is never
/// split: the last grapheme cluster may have lost its rest to the cut (a
/// surrogate cut in half reads as a replacement character), so it goes
/// too. Text of one cluster is kept as it is.
fn keep_whole_characters(text: &mut String) {
    let clusters = verbatim_text::graphemes(text);
    if clusters.len() > 1
        && let Some(last) = clusters.last()
    {
        text.truncate(last.start);
    }
}

/// The largest grapheme cluster boundary of `text` at or before `offset`.
fn grapheme_floor(text: &str, offset: usize) -> usize {
    verbatim_text::graphemes(text)
        .into_iter()
        .map(|range| range.end)
        .take_while(|&end| end <= offset)
        .last()
        .unwrap_or(0)
}

/// The number of characters (grapheme clusters) in `text`.
fn characters(text: &str) -> u32 {
    u32::try_from(verbatim_text::graphemes(text).len()).unwrap_or(u32::MAX)
}

/// Carries out `op` on `source`, whose anchors are `anchors`, answering
/// with the protocol's reply. `signal` serves a caret key's wait.
pub fn perform<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    op: &TextOp,
    signal: &mut dyn CaretSignal,
) -> TextReply {
    let result = match op {
        TextOp::AwaitCaret(watch) => await_caret(source, anchors, watch, signal),
        TextOp::Read(read) => read_unit(source, anchors, read),
        TextOp::ReadAhead(ahead) => read_ahead(source, anchors, ahead),
        TextOp::ReadRange { start, end } => read_range(source, anchors, *start, *end),
        TextOp::Select { start, end } => select(source, anchors, *start, Some(*end)),
        TextOp::MoveCaret(point) => select(source, anchors, *point, None),
        TextOp::Location(point) => location(source, anchors, *point),
        _ => Ok(TextReply::Unsupported),
    };
    result.unwrap_or_else(|error| match error {
        TextError::Gone => TextReply::Gone,
        TextError::Failed(reason) => {
            tracing::debug!(reason, "a text request was not answered");
            TextReply::Unanswered
        }
    })
}

/// A point resolved, or the reply that ends the request.
fn point<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    point: TextPoint,
) -> OrReply<S::Pos> {
    Ok(Ok(match point {
        TextPoint::Caret => source.caret()?.caret,
        TextPoint::SelectionStart => {
            let state = source.caret()?;
            state.selection.map_or(state.caret, |(start, _)| start)
        }
        TextPoint::SelectionEnd => {
            let state = source.caret()?;
            state.selection.map_or(state.caret, |(_, end)| end)
        }
        TextPoint::Start => source.start()?,
        TextPoint::End => source.end()?,
        TextPoint::At(position) => match anchors.resolve(source, position)? {
            Some(pos) => pos,
            None => return Ok(Err(TextReply::AnchorLost)),
        },
    }))
}

/// A point as a source finds it, and the position to remember it by once
/// found when finding it costs a read; or the reply that ends the request.
type From<P> = Result<(PointFrom<P>, Option<TextPosition>), TextReply>;

/// What `point` names, with no call.
fn point_from<P: Clone>(anchors: &NodeText<'_, P>, point: TextPoint) -> From<P> {
    Ok(match point {
        TextPoint::Caret => (PointFrom::Caret, None),
        TextPoint::SelectionStart => (PointFrom::SelectionStart, None),
        TextPoint::SelectionEnd => (PointFrom::SelectionEnd, None),
        TextPoint::Start => (PointFrom::Start, None),
        TextPoint::End => (PointFrom::End, None),
        TextPoint::At(position) => match anchors.point_from(position) {
            Some(from @ PointFrom::After(..)) => (from, Some(position)),
            Some(from) => (from, None),
            None => return Err(TextReply::AnchorLost),
        },
    })
}

/// Remembers a point found by reading, so a later request naming it costs
/// nothing.
fn remember_found<P>(anchors: &mut NodeText<'_, P>, position: Option<TextPosition>, pos: &P)
where
    P: Clone,
{
    if let Some(position) = position {
        anchors.remember(position, pos.clone());
    }
}

/// Builds the chunk for `unit` read as `kind`, with its offset at the
/// UTF-16 offset `offset`, the source position `at`, and mints its anchor;
/// with `languages`, its languages read already, and `formats`, its
/// formatting read already.
fn chunk<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (unit, kind): (&Unit<S::Pos>, TextUnit),
    (offset, at): (usize, S::Pos),
    (runs, formats): (Vec<(usize, usize, String)>, &[Formatting]),
) -> TextChunk {
    let mut wanted = vec![offset];
    for (start, end, _) in &runs {
        wanted.push(*start);
        wanted.push(*end);
    }
    for (start, end, _) in formats {
        wanted.push(*start);
        wanted.push(*end);
    }
    let (mut text, mut mapped, cut) = to_utf8(&unit.text, MAX_CHUNK_BYTES, &wanted);
    // A cut by bytes is already between whole characters; a unit the
    // source cut short may end inside one.
    if unit.truncated {
        keep_whole_characters(&mut text);
        for offset in &mut mapped {
            *offset = (*offset).min(text.len());
        }
    }
    let languages = runs
        .iter()
        .enumerate()
        .filter_map(|(index, (_, _, language))| {
            let (start, end) = (mapped[1 + 2 * index], mapped[2 + 2 * index]);
            (start < end).then(|| LanguageRun {
                start: u32::try_from(start).unwrap_or(u32::MAX),
                end: u32::try_from(end).unwrap_or(u32::MAX),
                language: language.clone(),
            })
        })
        .collect();
    let base = 1 + 2 * runs.len();
    let formats = formats
        .iter()
        .enumerate()
        .filter_map(|(index, (_, _, attributes))| {
            let (start, end) = (mapped[base + 2 * index], mapped[base + 1 + 2 * index]);
            (start < end).then(|| FormatRun {
                start: u32::try_from(start).unwrap_or(u32::MAX),
                end: u32::try_from(end).unwrap_or(u32::MAX),
                attributes: attributes.clone(),
            })
        })
        .collect();
    let (first, last) = source.edges(unit, kind);
    let start = anchors.mint(unit.start.clone(), &text);
    let offset = u32::try_from(mapped[0]).unwrap_or(u32::MAX);
    anchors.remember(
        TextPosition {
            anchor: start,
            offset,
        },
        at,
    );
    TextChunk {
        unit: kind,
        text,
        start,
        offset,
        languages,
        first,
        last,
        truncated: unit.truncated || cut,
        formats,
    }
}

/// The caret's line and selection, reported for a caret event or an answer
/// to a caret key, with the caret state they were read from. `now_ms` is
/// the clock of [`CaretSignal::now_ms`], read once the caret has been.
/// With `formats`, the line carries its formatting where the source reads
/// it in one go ([`TextSource::caret_read`]): the report after a focus,
/// whose line is spoken, and not the one after each typed character.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn caret_report<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    now_ms: &mut dyn FnMut() -> u64,
    formats: bool,
) -> TextResult<(CaretReport, CaretState<S::Pos>)> {
    let request = CaretRequest {
        since: None,
        previous: None,
        unit: None,
        formats: formats.then_some(FormatSpan::Line),
    };
    if let Some(read) = source.caret_read(&request)? {
        let read_at_ms = now_ms();
        let report = report_for(
            source,
            anchors,
            (&read.state, read_at_ms),
            Some((read.line.0, read.line.1, read.formats)),
        )?;
        return Ok((report, read.state));
    }
    let state = source.caret()?;
    let read_at_ms = now_ms();
    let report = report_for(source, anchors, (&state, read_at_ms), None)?;
    Ok((report, state))
}

/// The report for a caret read another program already made, finished at
/// `read_at_ms` (a terminal's, read with its new output).
///
/// # Errors
///
/// [`TextError`] when the line's anchor cannot be made.
pub fn caret_report_from<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: CaretRead<S::Pos>,
    read_at_ms: u64,
) -> TextResult<CaretReport> {
    report_for(
        source,
        anchors,
        (&read.state, read_at_ms),
        Some((read.line.0, read.line.1, read.formats)),
    )
}

/// The line containing the caret, and the caret's UTF-16 offset in it.
fn caret_line<S: TextSource>(
    source: &mut S,
    state: &CaretState<S::Pos>,
) -> TextResult<(Unit<S::Pos>, usize)> {
    let line = source
        .unit_at(&state.caret, TextUnit::Line, MAX_CHUNK_UNITS)?
        .ok_or_else(|| TextError::Failed("the text has no lines".to_owned()))?;
    let offset = source.offset_in(&line, &state.caret)?;
    Ok((line, offset))
}

/// A unit read at the caret and the caret's UTF-16 offset in it.
type UnitAt<P> = (Unit<P>, usize);

/// A unit read at the caret: the unit, the caret's UTF-16 offset in it, and
/// its formatting, UTF-16 ranges of its text (empty when none was read).
type ReadUnit<P> = (Unit<P>, usize, Vec<Formatting>);

/// The report for a caret state already read, and the time its read
/// finished, with its line when that was read already.
fn report_for<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (state, read_at_ms): (&CaretState<S::Pos>, u64),
    line: Option<ReadUnit<S::Pos>>,
) -> TextResult<CaretReport> {
    let (line, offset, formats) = if let Some(read) = line {
        read
    } else {
        let (line, offset) = caret_line(source, state)?;
        (line, offset, Vec::new())
    };
    let line = chunk(
        source,
        anchors,
        (&line, TextUnit::Line),
        (offset, state.caret.clone()),
        (Vec::new(), &formats),
    );
    let carets = &mut anchors.anchors.carets;
    if carets.len() == REMEMBERED_CARETS {
        carets.pop_front();
    }
    carets.push_back((
        read_at_ms,
        TextPosition {
            anchor: line.start,
            offset: line.offset,
        },
    ));
    let selection = state.selection.as_ref().map(|(start, end)| Selection {
        start: anchors.position_of(start.clone()),
        end: anchors.position_of(end.clone()),
    });
    Ok(CaretReport { line, selection })
}

/// Whether `state`'s selection differs from `previous`, a selection given
/// by its ends (equal ends for none).
fn selection_moved<S: TextSource>(
    source: &mut S,
    state: &CaretState<S::Pos>,
    previous: &(S::Pos, S::Pos),
) -> TextResult<bool> {
    let (start, end) = state
        .selection
        .clone()
        .unwrap_or_else(|| (state.caret.clone(), state.caret.clone()));
    Ok(source.compare(&start, &previous.0)? != Ordering::Equal
        || source.compare(&end, &previous.1)? != Ordering::Equal)
}

/// The text of `unit` at the caret, for comparing with what it was before a
/// Delete: the character cut from the caret's line (`line`, UTF-8 with the
/// caret's byte offset), the unit already read, or the source's unit.
fn text_at_caret<S: TextSource>(
    source: &mut S,
    state: &CaretState<S::Pos>,
    (unit, read): (TextUnit, Option<&Unit<S::Pos>>),
    (line, offset): (&str, usize),
) -> TextResult<String> {
    if unit == TextUnit::Character {
        return Ok(verbatim_text::grapheme_at(line, offset)
            .map(|range| line[range].to_owned())
            .unwrap_or_default());
    }
    if let Some(read) = read {
        return Ok(to_utf8(&read.text, MAX_CHUNK_BYTES, &[]).0);
    }
    Ok(source
        .unit_at(&state.caret, unit, MAX_CHUNK_UNITS)?
        .map(|unit| to_utf8(&unit.text, MAX_CHUNK_BYTES, &[]).0)
        .unwrap_or_default())
}

/// The characters (grapheme clusters) just before and at byte `offset` of a
/// line, either empty at the line's ends.
fn beside(line: &str, offset: usize) -> (String, String) {
    let offset = floor_boundary(line, offset);
    let before = verbatim_text::graphemes(&line[..offset])
        .pop()
        .map(|range| line[range].to_owned())
        .unwrap_or_default();
    let at = verbatim_text::grapheme_at(line, offset)
        .map(|range| line[range].to_owned())
        .unwrap_or_default();
    (before, at)
}

/// The longest a caret key's wait for evidence lasts.
fn wait_length(wait: CaretWait) -> Duration {
    match wait {
        CaretWait::Standard => Duration::from_millis(100),
        CaretWait::Extended => Duration::from_millis(300),
    }
}

/// The unit a caret key's answer reads besides the line, as the source has
/// it: none for a character (cut from the line), a line, or the document;
/// the paragraph for a sentence where the source splits text by paragraph,
/// and none where it has no sentences.
fn extra_unit<S: TextSource>(source: &S, unit: TextUnit) -> Option<TextUnit> {
    match unit {
        TextUnit::Character | TextUnit::Line | TextUnit::Document => None,
        TextUnit::Sentence => match source.sentences() {
            Sentences::Unsupported => None,
            Sentences::ByParagraph => Some(TextUnit::Paragraph),
        },
        other => Some(other),
    }
}

/// Whose formatting a caret key's answer carries: the character, word, or
/// line spoken. None for a paragraph or a page, as NVDA reports no spelling
/// errors when the caret moves by paragraph, for speed
/// (`docs/nvda/document-formatting.md`).
fn format_span(unit: TextUnit) -> Option<FormatSpan> {
    match unit {
        TextUnit::Character => Some(FormatSpan::Character),
        TextUnit::Word => Some(FormatSpan::Unit),
        TextUnit::Line => Some(FormatSpan::Line),
        _ => None,
    }
}

/// One read of a caret key's wait: the caret, the evidence found by
/// comparing positions, and whatever was read with them.
struct Polled<P> {
    state: CaretState<P>,
    read_at_ms: u64,
    /// The caret is not where it was known to be.
    caret_moved: bool,
    /// The selection is not what it was.
    selection_moved: bool,
    line: Option<(Unit<P>, usize)>,
    unit: Option<(Unit<P>, usize)>,
    formats: Vec<Formatting>,
    /// The selection's changes, when the read found them.
    changes: Option<Vec<(bool, Vec<u16>)>>,
}

/// Reads the caret for a caret key's wait: in one go where the source can
/// ([`TextSource::caret_read`]), else by its parts, comparing positions as
/// it goes and reading no more than the comparisons need.
fn poll<S: TextSource>(
    source: &mut S,
    request: &CaretRequest<'_, S::Pos>,
    signal: &mut dyn CaretSignal,
) -> TextResult<Polled<S::Pos>> {
    signal.reading();
    if let Some(read) = source.caret_read(request)? {
        return Ok(Polled {
            state: read.state,
            read_at_ms: signal.now_ms(),
            caret_moved: read.moved,
            selection_moved: read.selection_moved,
            line: Some(read.line),
            unit: read.unit,
            formats: read.formats,
            changes: read.changes,
        });
    }
    let state = source.caret()?;
    let read_at_ms = signal.now_ms();
    let mut caret_moved = false;
    if let Some(since) = request.since {
        caret_moved = source.compare(&state.caret, since)? != Ordering::Equal;
    }
    let mut selection_moved = false;
    if !caret_moved && let Some(previous) = request.previous {
        selection_moved = self::selection_moved(source, &state, previous)?;
    }
    Ok(Polled {
        state,
        read_at_ms,
        caret_moved,
        selection_moved,
        line: None,
        unit: None,
        formats: Vec::new(),
        changes: None,
    })
}

/// Waits for evidence that a caret key did something, then reports the
/// caret, the watch's unit at it, and how the selection changed. Each read
/// while waiting is one round trip where the source reads the caret in one
/// go ([`TextSource::caret_read`]), with what the answer needs, so the read
/// that finds the evidence is the answer.
fn await_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    watch: &CaretWatch,
    signal: &mut dyn CaretSignal,
) -> TextResult<TextReply> {
    let deadline = signal.now() + wait_length(watch.wait);
    let Baseline {
        since,
        previous,
        known,
    } = baseline(source, anchors, watch)?;
    let request = CaretRequest {
        since: since.as_ref(),
        previous: previous.as_ref(),
        unit: extra_unit(source, watch.unit),
        formats: format_span(watch.unit),
    };
    let polled = loop {
        let mut polled = poll(source, &request, signal)?;
        // A caret event alone is evidence only when Core did not know where
        // the caret was: otherwise it may be the application's late report
        // of something earlier, and the caret is compared instead.
        let mut moved = (since.is_none() && signal.caret_event())
            || polled.caret_moved
            || polled.selection_moved;
        if !moved && (known.is_some() || watch.compare.is_some()) {
            let (unit, offset) = match polled.line.take() {
                Some(line) => line,
                None => caret_line(source, &polled.state)?,
            };
            let (text, mapped, _) = to_utf8(&unit.text, MAX_CHUNK_BYTES, &[offset]);
            if let Some(known) = &known {
                moved = beside(&text, mapped[0]) != *known;
            }
            if !moved && let Some(compare) = &watch.compare {
                let read = polled.unit.as_ref().map(|(unit, _)| unit);
                moved = text_at_caret(
                    source,
                    &polled.state,
                    (watch.unit, read),
                    (&text, mapped[0]),
                )? != *compare;
            }
            polled.line = Some((unit, offset));
        }
        let now = signal.now();
        if moved || now >= deadline {
            polled.caret_moved = moved;
            break polled;
        }
        signal.wait(CARET_POLL.min(deadline - now));
    };
    signal.awaited();
    answer_caret(
        source,
        anchors,
        polled,
        (request.formats, watch.unit),
        previous.as_ref(),
    )
}

/// What a caret key's wait compares with: where the caret was, the
/// selection's ends, and the characters either side of the caret, as known
/// before the key.
struct Baseline<P> {
    since: Option<P>,
    previous: Option<(P, P)>,
    known: Option<(String, String)>,
}

/// The baseline of a caret key's wait.
fn baseline<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    watch: &CaretWatch,
) -> TextResult<Baseline<S::Pos>> {
    // Where the caret was before the key: the newest caret this outpost
    // reported from a read that finished before the key was pressed, which
    // Core may not have had yet when the key came (a caret event from an
    // earlier key or a paste), else where Core knew it. A caret read once
    // the key was pressed may already show what the key did (the
    // application's caret event can arrive before this request does), so
    // it is never the baseline; nor is one read in the same millisecond as
    // the key, which could have come after it. A position whose anchor was
    // forgotten is no evidence either way.
    let reported = anchors
        .anchors
        .carets
        .iter()
        .rev()
        .find(|(read_at_ms, _)| *read_at_ms < watch.pressed_at_ms)
        .map(|&(_, position)| position);
    let baseline = reported.or(watch.since);
    let since = match baseline {
        Some(position) => anchors.resolve(source, position)?,
        None => None,
    };
    let previous = match watch.previous_selection {
        Some(PreviousSelection { start, end }) => {
            match (
                anchors.resolve(source, start)?,
                anchors.resolve(source, end)?,
            ) {
                (Some(start), Some(end)) => Some((start, end)),
                _ => None,
            }
        }
        None => None,
    };
    // The characters either side of the caret where Core last knew it. A
    // provider's positions follow edits (a deleted character takes the
    // position Core knew with it), and the application may have handled the
    // key before this wait began, so those characters changing is evidence
    // too. The rest of the line is not compared: a line that wraps anew
    // changes with no key at all.
    let known = baseline.and_then(|position| {
        anchors
            .anchor_text(position.anchor)
            .map(|text| beside(&text, position.offset as usize))
    });
    Ok(Baseline {
        since,
        previous,
        known,
    })
}

/// The answer to a caret key once its wait has ended: the caret's line,
/// the unit at the caret, and the selection's changes, from what the last
/// read found, with the formatting read for `span`.
fn answer_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    polled: Polled<S::Pos>,
    (span, unit): (Option<FormatSpan>, TextUnit),
    previous: Option<&(S::Pos, S::Pos)>,
) -> TextResult<TextReply> {
    let Polled {
        state,
        read_at_ms,
        caret_moved: moved,
        line,
        unit: read_unit,
        formats,
        changes,
        ..
    } = polled;
    let (line_formats, unit_formats) = if span == Some(FormatSpan::Line) {
        (formats, Vec::new())
    } else {
        (Vec::new(), formats)
    };
    let line = line.map(|(unit, offset)| (unit, offset, line_formats));
    let caret = report_for(source, anchors, (&state, read_at_ms), line)?;
    let unit = unit_at_caret(
        source,
        anchors,
        (&state, &caret.line),
        unit,
        (read_unit, unit_formats),
    )?;
    let selection_changes = match (previous, changes) {
        (Some(_), Some(changes)) => changes
            .into_iter()
            .filter_map(|(selected, text)| change_of(selected, &text))
            .collect(),
        (Some(previous), None) => selection_changes(source, previous, &state)?,
        (None, _) => Vec::new(),
    };
    Ok(TextReply::Caret(Box::new(CaretReply {
        moved,
        caret,
        read_at_ms,
        unit,
        selection_changes,
    })))
}

/// The watch's unit at the caret, as a chunk: `None` for a line, which the
/// caret's line already is, and for a unit the source does not have. A
/// character is cut from the line already read; any other unit is the one
/// already read (`read`), or read now. `formats` is the formatting read for
/// the unit, UTF-16 ranges of its text; a character's covers it.
fn unit_at_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (state, line): (&CaretState<S::Pos>, &TextChunk),
    unit: TextUnit,
    (read, formats): (Option<UnitAt<S::Pos>>, Vec<Formatting>),
) -> TextResult<Option<TextChunk>> {
    match unit {
        TextUnit::Line | TextUnit::Document => Ok(None),
        TextUnit::Character => {
            let offset = line.offset as usize;
            let text = verbatim_text::grapheme_at(&line.text, offset)
                .map(|range| line.text[range].to_owned())
                .unwrap_or_default();
            // The character's formatting, read for it alone, covers it.
            let formats = formats
                .into_iter()
                .next()
                .map(|(_, _, attributes)| FormatRun {
                    start: 0,
                    end: u32::try_from(text.len()).unwrap_or(u32::MAX),
                    attributes,
                })
                .into_iter()
                .collect();
            let start = anchors.mint(state.caret.clone(), &text);
            anchors.remember(TextPosition::at(start), state.caret.clone());
            Ok(Some(TextChunk {
                unit: TextUnit::Character,
                text,
                start,
                offset: 0,
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats,
            }))
        }
        other => {
            let Some(reported) = extra_unit(source, other) else {
                return Ok(None);
            };
            let (found, offset) = if let Some(read) = read {
                read
            } else {
                let Some(found) = source.unit_at(&state.caret, reported, MAX_CHUNK_UNITS)? else {
                    return Ok(None);
                };
                let offset = source.offset_in(&found, &state.caret)?;
                (found, offset)
            };
            Ok(Some(chunk(
                source,
                anchors,
                (&found, reported),
                (offset, state.caret.clone()),
                (Vec::new(), &formats),
            )))
        }
    }
}

/// One selection change between two positions, its text read.
fn change<S: TextSource>(
    source: &mut S,
    selected: bool,
    start: &S::Pos,
    end: &S::Pos,
) -> TextResult<Option<SelectionChange>> {
    let (units, _) = source.text(start, end, MAX_RANGE_UNITS)?;
    Ok(change_of(selected, &units))
}

/// A selection change of the UTF-16 text `units`, `None` for none.
fn change_of(selected: bool, units: &[u16]) -> Option<SelectionChange> {
    if units.is_empty() {
        return None;
    }
    let (full, _, _) = to_utf8(units, MAX_RANGE_BYTES, &[]);
    let count = characters(&full);
    let mut text = full;
    // Cut, when it is cut, between whole characters.
    if text.len() > MAX_SELECTION_TEXT_BYTES {
        text.truncate(grapheme_floor(&text, MAX_SELECTION_TEXT_BYTES));
    }
    Some(SelectionChange {
        selected,
        text,
        characters: count,
    })
}

/// How the selection changed from `previous` (its ends, equal for none) to
/// what `state` holds, by the contract's rule: two selections that neither
/// overlap nor touch are an unselection then a selection; otherwise the
/// start side changes, then the end side.
fn selection_changes<S: TextSource>(
    source: &mut S,
    previous: &(S::Pos, S::Pos),
    state: &CaretState<S::Pos>,
) -> TextResult<Vec<SelectionChange>> {
    let (old_start, old_end) = previous;
    let (new_start, new_end) = state
        .selection
        .clone()
        .unwrap_or_else(|| (state.caret.clone(), state.caret.clone()));
    let mut changes = Vec::new();
    let apart = source.compare(&new_end, old_start)? == Ordering::Less
        || source.compare(&new_start, old_end)? == Ordering::Greater;
    if apart {
        changes.extend(change(source, false, old_start, old_end)?);
        changes.extend(change(source, true, &new_start, &new_end)?);
        return Ok(changes);
    }
    match source.compare(&new_start, old_start)? {
        Ordering::Less => changes.extend(change(source, true, &new_start, old_start)?),
        Ordering::Greater => changes.extend(change(source, false, old_start, &new_start)?),
        Ordering::Equal => {}
    }
    match source.compare(&new_end, old_end)? {
        Ordering::Greater => changes.extend(change(source, true, old_end, &new_end)?),
        Ordering::Less => changes.extend(change(source, false, &new_end, old_end)?),
        Ordering::Equal => {}
    }
    Ok(changes)
}

/// Reads one unit: moves first when asked, then reads the unit at the point
/// reached.
fn read_unit<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: &TextRead,
) -> TextResult<TextReply> {
    Ok(match read_units(source, anchors, read, 1)? {
        Ok((moved, chunks)) => match chunks.into_iter().next() {
            Some(chunk) => TextReply::Read { moved, chunk },
            None => TextReply::Unanswered,
        },
        Err(reply) => reply,
    })
}

/// Reads several units ahead, for say-all.
fn read_ahead<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    ahead: &TextReadAhead,
) -> TextResult<TextReply> {
    let read = TextRead {
        at: ahead.at,
        movement: ahead.movement,
        unit: ahead.unit,
    };
    let count = ahead.count.clamp(1, MAX_READ_AHEAD);
    Ok(
        match read_units(source, anchors, &read, u32::from(count))? {
            Ok((moved, chunks)) => TextReply::Chunks { moved, chunks },
            Err(reply) => reply,
        },
    )
}

/// Reads `count` units: the first as [`TextOp::Read`] reads it, then each
/// next one, while the text read is under [`MAX_READ_AHEAD_TEXT`]; in one
/// go where the source can ([`TextSource::read_units`]), else call by
/// call. Answers how far the movement went and the chunks, the last marked
/// as the text's last when no unit follows it, or the reply that ends the
/// request.
fn read_units<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: &TextRead,
    count: u32,
) -> OrReply<(i32, Vec<TextChunk>)> {
    let (from, position) = match point_from(anchors, read.at) {
        Ok(found) => found,
        Err(reply) => return Ok(Err(reply)),
    };
    let kind = match read.unit {
        TextUnit::Document => return Ok(Err(TextReply::UnsupportedUnit(TextUnit::Document))),
        TextUnit::Sentence => match source.sentences() {
            Sentences::Unsupported => {
                return Ok(Err(TextReply::UnsupportedUnit(TextUnit::Sentence)));
            }
            Sentences::ByParagraph => TextUnit::Paragraph,
        },
        other => other,
    };
    let movement = read.movement.map(|movement| TextMovement {
        unit: if movement.unit == TextUnit::Document {
            TextUnit::Document
        } else {
            movement_unit(source, movement.unit)
        },
        count: movement.count,
    });
    let request = UnitsRequest {
        from: &from,
        movement,
        unit: kind,
        count,
    };
    if let Some(answer) = source.read_units(&request)? {
        let read = match answer {
            Ok(read) => read,
            Err(reply) => return Ok(Err(reply)),
        };
        remember_found(anchors, position, &read.from);
        let mut chunks = Vec::with_capacity(read.units.len());
        for (index, unit) in read.units.into_iter().enumerate() {
            let at = if index == 0 {
                read.point.clone()
            } else {
                unit.unit.start.clone()
            };
            chunks.push(chunk(
                source,
                anchors,
                (&unit.unit, kind),
                (unit.offset, at),
                (unit.languages, &[]),
            ));
        }
        if read.ended
            && let Some(last) = chunks.last_mut()
        {
            last.last = true;
        }
        return Ok(Ok((read.moved, chunks)));
    }
    read_units_classic(source, anchors, (&request, position), read.unit)
}

/// [`read_units`] call by call, for a source that cannot read them in one
/// go; `unit` is the unit the request named, before a sentence became a
/// paragraph.
fn read_units_classic<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (request, position): (&UnitsRequest<'_, S::Pos>, Option<TextPosition>),
    unit: TextUnit,
) -> OrReply<(i32, Vec<TextChunk>)> {
    let (kind, count) = (request.unit, request.count);
    let at = resolve_from(source, request.from)?;
    remember_found(anchors, position, &at);
    let (at, moved, on_start) = match request.movement {
        None => (at, 0, false),
        Some(TextMovement {
            unit: TextUnit::Document,
            count,
        }) => {
            let target = if count < 0 {
                source.start()?
            } else {
                source.end()?
            };
            let moved = if count == 0 || source.compare(&at, &target)? == Ordering::Equal {
                0
            } else {
                count.signum()
            };
            (target, moved, false)
        }
        Some(TextMovement { unit: by, count }) => match source.move_by(&at, by, count)? {
            Some((landed, moved)) => (landed, moved, by == unit),
            None => return Ok(Err(TextReply::UnsupportedUnit(by))),
        },
    };
    let Some(found) = source.unit_at(&at, kind, MAX_CHUNK_UNITS)? else {
        return Ok(Err(TextReply::UnsupportedUnit(kind)));
    };
    let offset = if on_start {
        0
    } else {
        source.offset_in(&found, &at)?
    };
    let languages = source.languages(&found);
    let mut total = found.text.len();
    let mut chunks = vec![chunk(
        source,
        anchors,
        (&found, kind),
        (offset, at),
        (languages, &[]),
    )];
    let mut last = found;
    while chunks.len() < count as usize && total < MAX_READ_AHEAD_TEXT {
        let Some((next, moved)) = source.move_by(&last.start, kind, 1)? else {
            break;
        };
        if moved == 0 {
            if let Some(chunk) = chunks.last_mut() {
                chunk.last = true;
            }
            break;
        }
        let Some(found) = source.unit_at(&next, kind, MAX_CHUNK_UNITS)? else {
            break;
        };
        let languages = source.languages(&found);
        total += found.text.len();
        chunks.push(chunk(
            source,
            anchors,
            (&found, kind),
            (0, found.start.clone()),
            (languages, &[]),
        ));
        last = found;
    }
    Ok(Ok((moved, chunks)))
}

/// Finds a point the classic way, call by call.
fn resolve_from<S: TextSource>(source: &mut S, from: &PointFrom<S::Pos>) -> TextResult<S::Pos> {
    Ok(match from {
        PointFrom::Caret => source.caret()?.caret,
        PointFrom::SelectionStart => {
            let state = source.caret()?;
            state.selection.map_or(state.caret, |(start, _)| start)
        }
        PointFrom::SelectionEnd => {
            let state = source.caret()?;
            state.selection.map_or(state.caret, |(_, end)| end)
        }
        PointFrom::Start => source.start()?,
        PointFrom::End => source.end()?,
        PointFrom::At(pos) => pos.clone(),
        PointFrom::After(pos, prefix) => source.advance(pos, prefix)?,
    })
}

/// The unit a movement goes by: a sentence movement over text split by
/// paragraph goes by paragraph.
fn movement_unit<S: TextSource>(source: &S, unit: TextUnit) -> TextUnit {
    if unit == TextUnit::Sentence && source.sentences() == Sentences::ByParagraph {
        TextUnit::Paragraph
    } else {
        unit
    }
}

/// Two points resolved, in document order.
fn ordered<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    start: TextPoint,
    end: TextPoint,
) -> OrReply<(S::Pos, S::Pos)> {
    let start = match point(source, anchors, start)? {
        Ok(pos) => pos,
        Err(reply) => return Ok(Err(reply)),
    };
    let end = match point(source, anchors, end)? {
        Ok(pos) => pos,
        Err(reply) => return Ok(Err(reply)),
    };
    Ok(Ok(if source.compare(&start, &end)? == Ordering::Greater {
        (end, start)
    } else {
        (start, end)
    }))
}

/// The text between two points, for a copy.
fn read_range<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    start: TextPoint,
    end: TextPoint,
) -> TextResult<TextReply> {
    if let Some(reply) = range_at_once(
        source,
        anchors,
        (start, Some(end)),
        RangeAction::Text(MAX_RANGE_UNITS + 1),
    )? {
        let read = match reply {
            Ok(read) => read,
            Err(reply) => return Ok(reply),
        };
        let read_cut = read.text.len() > MAX_RANGE_UNITS;
        let units = &read.text[..read.text.len().min(MAX_RANGE_UNITS)];
        let (mut text, _, cut) = to_utf8(units, MAX_RANGE_BYTES, &[]);
        if read_cut && !cut {
            keep_whole_characters(&mut text);
        }
        return Ok(TextReply::Range {
            text,
            truncated: read_cut || cut,
        });
    }
    let (start, end) = match ordered(source, anchors, start, end)? {
        Ok(range) => range,
        Err(reply) => return Ok(reply),
    };
    let (units, read_cut) = source.text(&start, &end, MAX_RANGE_UNITS)?;
    let (mut text, _, cut) = to_utf8(&units, MAX_RANGE_BYTES, &[]);
    if read_cut && !cut {
        keep_whole_characters(&mut text);
    }
    Ok(TextReply::Range {
        text,
        truncated: read_cut || cut,
    })
}

/// The text between two points read or selected in one go where the
/// source can ([`TextSource::range`]), remembering the points it found;
/// `None` when it reads call by call.
fn range_at_once<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (start, end): (TextPoint, Option<TextPoint>),
    action: RangeAction,
) -> TextResult<Option<Result<RangeRead<S::Pos>, TextReply>>> {
    let (start, start_position) = match point_from(anchors, start) {
        Ok(found) => found,
        Err(reply) => return Ok(Some(Err(reply))),
    };
    let end = match end.map(|end| point_from(anchors, end)).transpose() {
        Ok(end) => end,
        Err(reply) => return Ok(Some(Err(reply))),
    };
    let Some(read) = source.range(&start, end.as_ref().map(|(end, _)| end), action)? else {
        return Ok(None);
    };
    remember_found(anchors, start_position, &read.start);
    if let Some((_, end_position)) = end {
        remember_found(anchors, end_position, &read.end);
    }
    Ok(Some(Ok(read)))
}

/// Selects between two points, or moves the caret to one (`end` `None`).
fn select<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    start: TextPoint,
    end: Option<TextPoint>,
) -> TextResult<TextReply> {
    if let Some(reply) = range_at_once(source, anchors, (start, end), RangeAction::Select)? {
        return Ok(match reply {
            Ok(read) if read.selected => TextReply::Done,
            Ok(_) => TextReply::Unsupported,
            Err(reply) => reply,
        });
    }
    let (start, end) = match ordered(source, anchors, start, end.unwrap_or(start))? {
        Ok(range) => range,
        Err(reply) => return Ok(reply),
    };
    Ok(if source.select(&start, &end)? {
        TextReply::Done
    } else {
        TextReply::Unsupported
    })
}

/// The screen position of a point.
fn location<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: TextPoint,
) -> TextResult<TextReply> {
    let (from, position) = match point_from(anchors, at) {
        Ok(found) => found,
        Err(reply) => return Ok(reply),
    };
    if let Some((pos, location)) = source.point_location(&from)? {
        remember_found(anchors, position, &pos);
        return Ok(match location {
            Some((x, y)) => TextReply::Location { x, y },
            None => TextReply::Unsupported,
        });
    }
    let at = match point(source, anchors, at)? {
        Ok(at) => at,
        Err(reply) => return Ok(reply),
    };
    Ok(match source.location(&at)? {
        Some((x, y)) => TextReply::Location { x, y },
        None => TextReply::Unsupported,
    })
}

#[cfg(test)]
mod tests;
