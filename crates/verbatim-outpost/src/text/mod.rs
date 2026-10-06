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

pub mod edit;
pub mod uia;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use verbatim_model::{
    CaretReply, CaretReport, CaretWait, CaretWatch, LanguageRun, MAX_CHUNK_BYTES, MAX_RANGE_BYTES,
    MAX_SELECTION_TEXT_BYTES, PreviousSelection, Selection, SelectionChange, TextAnchor, TextChunk,
    TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReply, TextUnit,
};

/// How often the caret is read again while a caret key's wait for evidence
/// runs and no caret event arrives: NVDA's 10 ms retry interval.
pub const CARET_POLL: Duration = Duration::from_millis(10);

/// How many anchors a node keeps beyond those Core holds.
pub const KEPT_ANCHORS: usize = 64;

/// The most UTF-16 code units read for a range copied to the clipboard or a
/// selection change counted: what fits in [`MAX_RANGE_BYTES`] at worst.
const MAX_RANGE_UNITS: usize = MAX_RANGE_BYTES / 2;

/// The most UTF-16 code units read for one chunk.
const MAX_CHUNK_UNITS: usize = MAX_CHUNK_BYTES;

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
}

/// Caret events, and the clock, for a caret key's wait for evidence.
pub trait CaretSignal {
    /// Whether a caret event for the node arrived since the wait began.
    fn caret_event(&mut self) -> bool;

    /// Waits until a caret event arrives or `timeout` passes.
    fn wait(&mut self, timeout: Duration);

    /// The time now.
    fn now(&mut self) -> Instant;

    /// The caret is about to be read: a caret event observed before now
    /// changed nothing the read will not see.
    fn reading(&mut self) {}
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
    /// The caret the outpost last reported for the node.
    last_caret: Option<TextPosition>,
}

impl<P> Default for NodeAnchors<P> {
    fn default() -> Self {
        Self {
            anchors: BTreeMap::new(),
            reported: HashMap::new(),
            last_caret: None,
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

    /// A position minted for `pos` alone: a new anchor there, offset 0.
    fn position_of(&mut self, pos: P) -> TextPosition {
        let anchor = self.mint(pos.clone(), "");
        let position = TextPosition::at(anchor);
        self.remember(position, pos);
        position
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
            break;
        }
        text.push(character);
        consumed += width;
    }
    let end = text.len();
    let mapped = mapped
        .into_iter()
        .map(|offset| offset.unwrap_or(end))
        .collect();
    (text, mapped, truncated)
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
        TextOp::ReadRange { start, end } => read_range(source, anchors, *start, *end),
        TextOp::Select { start, end } => select(source, anchors, *start, *end),
        TextOp::MoveCaret(point) => select(source, anchors, *point, *point),
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

/// Builds the chunk for `unit` read as `kind`, with its offset at the
/// UTF-16 offset `offset`, the source position `at`, and mints its anchor.
fn chunk<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (unit, kind): (&Unit<S::Pos>, TextUnit),
    (offset, at): (usize, S::Pos),
    languages: bool,
) -> TextChunk {
    let runs = if languages {
        source.languages(unit)
    } else {
        Vec::new()
    };
    let mut wanted = vec![offset];
    for (start, end, _) in &runs {
        wanted.push(*start);
        wanted.push(*end);
    }
    let (text, mapped, cut) = to_utf8(&unit.text, MAX_CHUNK_BYTES, &wanted);
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
    }
}

/// The caret's line and selection, reported for a caret event or an answer
/// to a caret key, with the caret state they were read from.
///
/// # Errors
///
/// When the application does not answer or the node is gone.
pub fn caret_report<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
) -> TextResult<(CaretReport, CaretState<S::Pos>)> {
    let state = source.caret()?;
    let report = report_for(source, anchors, &state, None)?;
    Ok((report, state))
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

/// The report for a caret state already read, with its line when that was
/// read already.
fn report_for<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    state: &CaretState<S::Pos>,
    line: Option<(Unit<S::Pos>, usize)>,
) -> TextResult<CaretReport> {
    let (line, offset) = match line {
        Some(read) => read,
        None => caret_line(source, state)?,
    };
    let line = chunk(
        source,
        anchors,
        (&line, TextUnit::Line),
        (offset, state.caret.clone()),
        false,
    );
    anchors.anchors.last_caret = Some(TextPosition {
        anchor: line.start,
        offset: line.offset,
    });
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
/// caret's byte offset), or the source's unit.
fn text_at_caret<S: TextSource>(
    source: &mut S,
    state: &CaretState<S::Pos>,
    unit: TextUnit,
    (line, offset): (&str, usize),
) -> TextResult<String> {
    if unit == TextUnit::Character {
        return Ok(verbatim_text::grapheme_at(line, offset)
            .map(|range| line[range].to_owned())
            .unwrap_or_default());
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

/// Waits for evidence that a caret key did something, then reports the
/// caret, the watch's unit at it, and how the selection changed.
fn await_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    watch: &CaretWatch,
    signal: &mut dyn CaretSignal,
) -> TextResult<TextReply> {
    let deadline = signal.now() + wait_length(watch.wait);
    // Where the caret was known to be before the key: the caret this
    // outpost last reported, which Core may not have had when it asked
    // (a caret event handled just before the request, from an earlier key
    // or a paste), else where Core knew it. A position whose anchor was
    // forgotten is no evidence either way.
    let baseline = anchors.anchors.last_caret.or(watch.since);
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
    let (state, moved, line) = loop {
        signal.reading();
        let state = source.caret()?;
        let mut line = None;
        // A caret event alone is evidence only when Core did not know where
        // the caret was: otherwise it may be the application's late report
        // of something earlier, and the caret is compared instead.
        let mut moved = since.is_none() && signal.caret_event();
        if !moved && let Some(since) = &since {
            moved = source.compare(&state.caret, since)? != Ordering::Equal;
        }
        if !moved && let Some(previous) = &previous {
            moved = selection_moved(source, &state, previous)?;
        }
        if !moved && (known.is_some() || watch.compare.is_some()) {
            let (unit, offset) = caret_line(source, &state)?;
            let (text, mapped, _) = to_utf8(&unit.text, MAX_CHUNK_BYTES, &[offset]);
            if let Some(known) = &known {
                moved = beside(&text, mapped[0]) != *known;
            }
            if !moved && let Some(compare) = &watch.compare {
                moved = text_at_caret(source, &state, watch.unit, (&text, mapped[0]))? != *compare;
            }
            line = Some((unit, offset));
        }
        let now = signal.now();
        if moved || now >= deadline {
            break (state, moved, line);
        }
        signal.wait(CARET_POLL.min(deadline - now));
    };
    let caret = report_for(source, anchors, &state, line)?;
    let unit = unit_at_caret(source, anchors, &state, &caret.line, watch.unit)?;
    let selection_changes = match &previous {
        Some(previous) => selection_changes(source, previous, &state)?,
        None => Vec::new(),
    };
    Ok(TextReply::Caret(Box::new(CaretReply {
        moved,
        caret,
        unit,
        selection_changes,
    })))
}

/// The watch's unit at the caret, as a chunk: `None` for a line, which the
/// caret's line already is, and for a unit the source does not have. A
/// character is cut from the line already read.
fn unit_at_caret<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    state: &CaretState<S::Pos>,
    line: &TextChunk,
    unit: TextUnit,
) -> TextResult<Option<TextChunk>> {
    match unit {
        TextUnit::Line | TextUnit::Document => Ok(None),
        TextUnit::Character => {
            let offset = line.offset as usize;
            let text = verbatim_text::grapheme_at(&line.text, offset)
                .map(|range| line.text[range].to_owned())
                .unwrap_or_default();
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
            }))
        }
        other => {
            let reported = if other == TextUnit::Sentence {
                match source.sentences() {
                    Sentences::Unsupported => return Ok(None),
                    Sentences::ByParagraph => TextUnit::Paragraph,
                }
            } else {
                other
            };
            let Some(found) = source.unit_at(&state.caret, reported, MAX_CHUNK_UNITS)? else {
                return Ok(None);
            };
            let offset = source.offset_in(&found, &state.caret)?;
            Ok(Some(chunk(
                source,
                anchors,
                (&found, reported),
                (offset, state.caret.clone()),
                false,
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
    if units.is_empty() {
        return Ok(None);
    }
    let (full, _, _) = to_utf8(&units, MAX_RANGE_BYTES, &[]);
    let count = characters(&full);
    let mut text = full;
    text.truncate(floor_boundary(&text, MAX_SELECTION_TEXT_BYTES));
    Ok(Some(SelectionChange {
        selected,
        text,
        characters: count,
    }))
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
    let at = match point(source, anchors, read.at)? {
        Ok(at) => at,
        Err(reply) => return Ok(reply),
    };
    let (at, moved, on_start) = match read.movement {
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
        Some(TextMovement { unit, count }) => {
            let unit = movement_unit(source, unit);
            match source.move_by(&at, unit, count)? {
                Some((landed, moved)) => (landed, moved, unit == read.unit),
                None => return Ok(TextReply::UnsupportedUnit(unit)),
            }
        }
    };
    let kind = match read.unit {
        TextUnit::Document => return Ok(TextReply::UnsupportedUnit(TextUnit::Document)),
        TextUnit::Sentence => match source.sentences() {
            Sentences::Unsupported => return Ok(TextReply::UnsupportedUnit(TextUnit::Sentence)),
            Sentences::ByParagraph => TextUnit::Paragraph,
        },
        other => other,
    };
    let Some(found) = source.unit_at(&at, kind, MAX_CHUNK_UNITS)? else {
        return Ok(TextReply::UnsupportedUnit(kind));
    };
    let offset = if on_start {
        0
    } else {
        source.offset_in(&found, &at)?
    };
    let chunk = chunk(source, anchors, (&found, kind), (offset, at), true);
    Ok(TextReply::Read { moved, chunk })
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
    let (start, end) = match ordered(source, anchors, start, end)? {
        Ok(range) => range,
        Err(reply) => return Ok(reply),
    };
    let (units, read_cut) = source.text(&start, &end, MAX_RANGE_UNITS)?;
    let (text, _, cut) = to_utf8(&units, MAX_RANGE_BYTES, &[]);
    Ok(TextReply::Range {
        text,
        truncated: read_cut || cut,
    })
}

/// Selects between two points, or moves the caret to one.
fn select<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    start: TextPoint,
    end: TextPoint,
) -> TextResult<TextReply> {
    let (start, end) = match ordered(source, anchors, start, end)? {
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
