//! The outpost's side of the text protocol (milestone M4,
//! `books/dev/src/reference/crates/verbatim-model.md`, "The text protocol").
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
//! - A key's read ([`check_key`]) reads the caret once and compares what it
//!   read with the node's last read; Core judges evidence from it by
//!   `verbatim_model::had_effect`.

#![forbid(unsafe_code)]

pub mod check;
mod color;
pub mod edit;
pub mod fused;
pub mod held;
pub mod ops;
pub mod sentence;
pub mod uia;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use verbatim_model::{
    CaretReport, CaretSelection, EditSelection, FormatRun, LanguageRun, MAX_CHUNK_BYTES,
    MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT, MAX_SELECTION_TEXT_BYTES, Selection,
    SelectionChange, TextAnchor, TextAttributes, TextChunk, TextMovement, TextOp, TextPoint,
    TextPosition, TextRead, TextReadAhead, TextReply, TextThrough, TextUnit,
};

use crate::outpost::{Anchor, NodeAnchors, ReadSelection};
pub use verbatim_outpost_logic::LastRead;

/// The most UTF-16 code units read for a range copied to the clipboard or a
/// selection change counted: what fits in [`MAX_RANGE_BYTES`] at worst.
pub(crate) const MAX_RANGE_UNITS: usize = MAX_RANGE_BYTES / 2;

/// The most UTF-16 code units read for one chunk.
pub(crate) const MAX_CHUNK_UNITS: usize = MAX_CHUNK_BYTES;

/// The formatting of a stretch of a unit's text: a start and an end UTF-16
/// offset into the unit's text, and the attributes there.
pub type Formatting = (usize, usize, TextAttributes);

/// What a caret read covers, for [`TextSource::caret_read`]: everything a
/// caret report or a caret key's answer needs, so a source that can read it
/// in one go does.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a separate thing the read is asked to do"
)]
pub struct CaretRequest<'a, P> {
    /// Where the caret was known to be, to compare it with.
    pub since: Option<&'a P>,
    /// The selection's ends as they were known (both the caret when nothing
    /// was selected), to compare it with.
    pub previous: Option<&'a (P, P)>,
    /// `previous` held nothing: with nothing selected now either, there is
    /// no change to tell, so the selection is not compared (a caret
    /// report's comparison; a key's read, for which a caret that moved is a
    /// selection that moved, leaves it false).
    pub previous_collapsed: bool,
    /// Whether to read the line's formatting, with the attributes the
    /// theme asks for.
    pub formats: bool,
    /// Whether to read the caret again once everything else is read, to
    /// say whether it moved meanwhile ([`CaretRead::moved_while_read`]).
    pub recheck: bool,
    /// Whether to read the selection's text, up to the source's cap for
    /// compared text (513 UTF-16 code units through UIA,
    /// `verbatim_uia_rops::MAX_COMPARED_TEXT`), when something is selected
    /// and its ends are not `previous`'s, or there is no `previous`, which
    /// is kept with the selection for the next comparison
    /// ([`CaretRead::selection_text`]).
    pub read_selection_text: bool,
    /// Whether to read the text between `previous`'s ends as they are now,
    /// up to the same cap, when it held something, to compare with its
    /// text as it was reported ([`CaretRead::previous_text`]).
    pub read_previous_text: bool,
}

/// The answer to a [`CaretRequest`].
pub struct CaretRead<P> {
    /// The caret and the selection.
    pub state: CaretState<P>,
    /// The caret is not where [`CaretRequest::since`] says.
    pub moved: bool,
    /// Where [`CaretRequest::since`] lies against the caret's line: its
    /// order against the line's start and against its end; `None` when
    /// the source did not compare them.
    pub since_place: Option<(Ordering, Ordering)>,
    /// The selection is not what [`CaretRequest::previous`] says.
    pub selection_moved: bool,
    /// The caret's line and the caret's UTF-16 offset in it.
    pub line: (Unit<P>, usize),
    /// The formatting of the request's span, UTF-16 ranges of its text.
    pub formats: Vec<Formatting>,
    /// With [`CaretRequest::previous`], how the selection changed from it
    /// (each change selected or not, and its UTF-16 text), read with the
    /// rest; `None` when the source did not read them, which the caller
    /// then does.
    pub changes: Option<Vec<(bool, Vec<u16>)>>,
    /// With [`CaretRequest::recheck`], whether the caret read again after
    /// everything else had moved: the line may show what the application
    /// wrote after the caret was read. False without a recheck.
    pub moved_while_read: bool,
    /// With [`CaretRequest::read_selection_text`], the selection's text up
    /// to the source's cap when something is selected and its ends are not
    /// `previous`'s, or there was no `previous`; `None` otherwise, and from
    /// a source that does not read it.
    pub selection_text: Option<Vec<u16>>,
    /// With [`CaretRequest::read_previous_text`] and a `previous` that held
    /// something, the text between its ends as they are now, up to the
    /// source's cap, which a source whose positions follow edits (UIA's
    /// ranges) moves as the text changes; `None` otherwise, and from a
    /// source that does not read it.
    pub previous_text: Option<Vec<u16>>,
    /// Where the selection starts, when something is selected.
    pub selection_start: SelectionStart,
}

/// Where a selection starts, as a caret read in one go found it
/// ([`CaretRead::selection_start`]), so the report can say what comes
/// before it.
pub struct SelectionStart {
    /// When the selection starts on the caret's line, the UTF-16 offset of
    /// its start in the line's text.
    pub in_line: Option<usize>,
    /// The line the selection starts in is known to be the text's first,
    /// found when the selection starts at that line's start.
    pub first: bool,
}

impl SelectionStart {
    /// Nothing known of where a selection starts.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            in_line: None,
            first: false,
        }
    }
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

/// The points just past the characters at a range's two ends, as
/// [`TextSource::range`] takes them (`verbatim_model::TextThrough`).
pub type PastEnds<'a, P> = (&'a PointFrom<P>, &'a PointFrom<P>);

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
    /// Selects it, and reads how the selection changed
    /// ([`RangeRead::changes`]).
    SelectReporting,
}

/// The answer of [`TextSource::range`].
pub struct RangeRead<P> {
    /// The first point, found.
    pub start: P,
    /// The second point, found.
    pub end: P,
    /// The text read, for [`RangeAction::Text`].
    pub text: Vec<u16>,
    /// Whether the selection was made, for [`RangeAction::Select`] and
    /// [`RangeAction::SelectReporting`].
    pub selected: bool,
    /// For [`RangeAction::SelectReporting`] when the selection was made, how
    /// it changed from the selection read just before (each change selected
    /// or not, and its UTF-16 text), by [`selection_changes`]'s rule.
    pub changes: Option<Vec<(bool, Vec<u16>)>>,
    /// For [`RangeAction::SelectReporting`] when the selection was made and
    /// its changes read, the selection read after it, which is kept for
    /// the node's next comparison of selections; `None` otherwise.
    pub selection_after: Option<SelectionAfter<P>>,
}

/// The selection read after a select ([`RangeRead::selection_after`]).
pub struct SelectionAfter<P> {
    /// Its ends, both the caret when nothing is selected.
    pub ends: (P, P),
    /// Whether something is selected.
    pub selected: bool,
    /// Its text, up to the source's cap for compared text, when something
    /// is selected and the source read it.
    pub text: Option<Vec<u16>>,
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

    /// `at`'s offset and the text's length, for a source that measures its
    /// text by character offsets; `None` for one that does not.
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn measure(&mut self, _at: &Self::Pos) -> TextResult<Option<(u32, u32)>> {
        Ok(None)
    }

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

    /// The selection `state` read, by the character offsets an edit
    /// control's messages give; `None` from any other source.
    fn edit_selection(&self, _state: &CaretState<Self::Pos>) -> Option<EditSelection> {
        None
    }

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
    /// where the source can; with `through`, the points just past the
    /// characters at the two (`verbatim_model::TextThrough`), from
    /// whichever comes first through the character at the other. `None`
    /// as for [`read_units`](Self::read_units).
    ///
    /// # Errors
    ///
    /// As [`caret`](Self::caret).
    fn range(
        &mut self,
        _start: &PointFrom<Self::Pos>,
        _end: Option<&PointFrom<Self::Pos>>,
        _through: Option<PastEnds<'_, Self::Pos>>,
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

/// A position as a text call works with it: the source's position, and the
/// key the outpost's store keeps its range under when it was kept before
/// the call, so the call's result names it again by that key and only a
/// position the call made is given a new one ([`NodeTextCopy::finish`]).
#[derive(Clone)]
pub(crate) struct Slot<P> {
    pub(crate) at: P,
    pub(crate) key: Option<u64>,
}

impl<P> Slot<P> {
    /// A position the call made, kept under no key yet.
    fn new(at: P) -> Self {
        Self { at, key: None }
    }
}

/// Every node's anchors in one backend, and the counter anchors are
/// numbered from, for a reader that holds them for the whole read: a test,
/// or a tool that drives the text protocol. An anchor is kept until the
/// reader forgets it ([`forget_anchors`](Self::forget_anchors)). The
/// outpost keeps its anchors as facts of its reducer, and a text call works
/// on a copy of one node's (`NodeTextCopy`).
pub struct Anchors<P> {
    counter: Arc<AtomicU64>,
    /// The anchors minted or reported through [`node`](Self::node).
    touched: Vec<u64>,
    nodes: HashMap<u64, NodeAnchors<Slot<P>>>,
}

/// A copy of one node's text facts, its positions resolved, which a text
/// call works on with no lock held, and the anchors it mints or reports,
/// for the store to tag once the call's commit is applied.
pub(crate) struct NodeTextCopy<P> {
    counter: Arc<AtomicU64>,
    anchors: NodeAnchors<Slot<P>>,
    touched: Vec<u64>,
}

impl<P: Clone> NodeTextCopy<P> {
    /// A copy of `anchors`, numbering the anchors it mints from `counter`.
    pub(crate) fn new(counter: Arc<AtomicU64>, anchors: NodeAnchors<Slot<P>>) -> Self {
        Self {
            counter,
            anchors,
            touched: Vec::new(),
        }
    }

    /// The copy, as a text call reads and changes it.
    pub(crate) fn text(&mut self) -> NodeText<'_, P> {
        NodeText {
            counter: &self.counter,
            touched: &mut self.touched,
            anchors: &mut self.anchors,
        }
    }

    /// The copy as the call left it, each position given by `keep`, and
    /// the anchors it minted or reported.
    pub(crate) fn finish<K>(
        self,
        mut keep: impl FnMut(Slot<P>) -> K,
    ) -> (NodeAnchors<K>, Vec<u64>) {
        (
            self.anchors.filter_map(|slot| Some(keep(slot))),
            self.touched,
        )
    }
}

impl<P: Clone> Anchors<P> {
    /// An empty store numbering anchors from `counter`.
    #[must_use]
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            counter,
            touched: Vec::new(),
            nodes: HashMap::new(),
        }
    }

    /// Forgets the anchors `forgotten` names, of every node: Core has seen
    /// the last message that carried each, and does not hold it. Returns
    /// the positions forgotten.
    pub fn forget_anchors(&mut self, forgotten: &HashSet<u64>) -> Vec<P> {
        let mut gone = Vec::new();
        for anchors in self.nodes.values_mut() {
            let numbers: Vec<u64> = anchors
                .anchors
                .keys()
                .copied()
                .filter(|number| forgotten.contains(number))
                .collect();
            for number in numbers {
                gone.extend(anchors.anchors.remove(&number).map(|anchor| anchor.pos.at));
            }
            let reported: Vec<(u64, u32)> = anchors
                .reported
                .keys()
                .copied()
                .filter(|(anchor, _)| forgotten.contains(anchor))
                .collect();
            for key in reported {
                gone.extend(anchors.reported.remove(&key).map(|slot| slot.at));
            }
            let held: Vec<u64> = anchors
                .held
                .keys()
                .copied()
                .filter(|number| forgotten.contains(number))
                .collect();
            for number in held {
                if let Some(read) = anchors.held.remove(&number) {
                    gone.extend(read.positions().map(|slot| slot.at.clone()));
                }
            }
        }
        gone
    }

    /// The anchors of node `node`, borrowed, with what minting needs, for
    /// a reader that holds them for the whole read.
    pub fn node(&mut self, node: u64) -> NodeText<'_, P> {
        NodeText {
            counter: &self.counter,
            touched: &mut self.touched,
            anchors: self.nodes.entry(node).or_default(),
        }
    }
}

/// The anchors of the positions `op` names, which a copy of its node's
/// anchors must hold.
#[must_use]
pub fn named_anchors(op: &TextOp) -> Vec<u64> {
    let point = |point: &TextPoint| match point {
        TextPoint::At(position) => Some(position.anchor.0),
        _ => None,
    };
    match op {
        TextOp::Read(read) => point(&read.at).into_iter().collect(),
        TextOp::ReadAhead(ahead) => point(&ahead.at).into_iter().collect(),
        TextOp::ReadRange { start, end, .. } | TextOp::Select { start, end, .. } => {
            point(start).into_iter().chain(point(end)).collect()
        }
        TextOp::MoveCaret(at) | TextOp::Location(at) => point(at).into_iter().collect(),
        TextOp::ReadTerminal(read) => read.start.map(|start| start.0).into_iter().collect(),
        TextOp::TerminalHistory { start, .. } | TextOp::TerminalFind { start, .. } => {
            vec![start.0]
        }
        _ => Vec::new(),
    }
}

/// One node's anchors, borrowed from an [`Anchors`] store or a copy of
/// them (`NodeTextCopy`).
pub struct NodeText<'a, P> {
    counter: &'a AtomicU64,
    touched: &'a mut Vec<u64>,
    anchors: &'a mut NodeAnchors<Slot<P>>,
}

impl<P: Clone> NodeText<'_, P> {
    /// Mints an anchor at `pos` for a chunk whose text is `text`, recorded
    /// as touched, for the message that reports it.
    fn mint(&mut self, pos: P, text: &str) -> TextAnchor {
        let number = self.next_number();
        self.anchors.anchors.insert(
            number,
            Anchor {
                pos: Slot::new(pos),
                text: Arc::from(text),
            },
        );
        self.touch(number);
        TextAnchor(number)
    }

    /// The next anchor number, shared by anchors and held reads.
    fn next_number(&self) -> u64 {
        self.counter.fetch_add(1, AtomicOrdering::Relaxed) + 1
    }

    /// Keeps the caret or selection Verbatim made as the node's last read,
    /// begun at `read_at_us`: the application's caret event for it then
    /// carries no change, and the next key compares with it. The line stays
    /// as it was read, since a move changes no text.
    fn keep_made(
        &mut self,
        (start, end): (P, P),
        selected: bool,
        text: Option<Vec<u16>>,
        read_at_us: u64,
    ) {
        let selection = selected.then(|| {
            let range = Selection {
                start: self.position_of(start.clone()),
                end: self.position_of(end.clone()),
            };
            ReadSelection {
                start: Slot::new(start.clone()),
                end: Slot::new(end.clone()),
                text: text.map(Arc::from),
                report: CaretSelection { range },
            }
        });
        if let Some(last) = self.anchors.last.as_mut() {
            last.caret = Slot::new(end);
            last.selection = selection;
            last.read_at_us = read_at_us;
        }
    }

    /// Remembers that `position` is `pos`, a position the outpost reported,
    /// its anchor recorded as touched, for the message that reports it.
    fn remember(&mut self, position: TextPosition, pos: P) {
        self.touch(position.anchor.0);
        self.anchors
            .reported
            .insert((position.anchor.0, position.offset), Slot::new(pos));
    }

    /// Records anchor `number` as touched.
    fn touch(&mut self, number: u64) {
        self.touched.push(number);
    }

    /// The source position of `position` when the outpost reported it, as
    /// a caret report's line start and caret, with no call; `None` when it
    /// was not reported or its anchor was forgotten.
    #[must_use]
    pub fn reported_point(&self, position: TextPosition) -> Option<P> {
        self.anchors
            .reported
            .get(&(position.anchor.0, position.offset))
            .map(|slot| slot.at.clone())
    }

    /// Holds `pos` as an anchor of its own with no text, recorded as
    /// touched, for a later request to name: a terminal read's start
    /// anchor (`docs/design/readers-tell-core-asks-build3.md`). No call.
    pub fn hold_point(&mut self, pos: P) -> TextAnchor {
        self.mint(pos, "")
    }

    /// The position anchor `anchor` holds, with no call; `None` when it was
    /// forgotten.
    #[must_use]
    pub fn anchor_point(&self, anchor: TextAnchor) -> Option<P> {
        self.anchors
            .anchors
            .get(&anchor.0)
            .map(|held| held.pos.at.clone())
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
    pub(crate) fn point_from(&self, position: TextPosition) -> Option<PointFrom<P>> {
        let key = (position.anchor.0, position.offset);
        if let Some(slot) = self.anchors.reported.get(&key) {
            return Some(PointFrom::At(slot.at.clone()));
        }
        let anchor = self.anchors.anchors.get(&position.anchor.0)?;
        if position.offset == 0 {
            return Some(PointFrom::At(anchor.pos.at.clone()));
        }
        let offset = verbatim_text::floor_boundary(&anchor.text, position.offset as usize);
        let prefix: Vec<u16> = anchor.text[..offset].encode_utf16().collect();
        Some(PointFrom::After(anchor.pos.at.clone(), prefix))
    }

    /// Resolves `position` to a source position; `None` when its anchor was
    /// forgotten.
    fn resolve<S: TextSource<Pos = P>>(
        &mut self,
        source: &mut S,
        position: TextPosition,
    ) -> TextResult<Option<P>> {
        let key = (position.anchor.0, position.offset);
        if let Some(slot) = self.anchors.reported.get(&key) {
            return Ok(Some(slot.at.clone()));
        }
        let Some(anchor) = self.anchors.anchors.get(&position.anchor.0) else {
            return Ok(None);
        };
        if position.offset == 0 {
            return Ok(Some(anchor.pos.at.clone()));
        }
        let text = Arc::clone(&anchor.text);
        let from = anchor.pos.at.clone();
        let offset = verbatim_text::floor_boundary(&text, position.offset as usize);
        let prefix: Vec<u16> = text[..offset].encode_utf16().collect();
        let pos = source.advance(&from, &prefix)?;
        self.anchors.reported.insert(key, Slot::new(pos.clone()));
        Ok(Some(pos))
    }
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
/// with the protocol's reply. A caret key's request ([`TextOp::Key`]) is
/// not carried out here, so it is answered [`TextReply::Unsupported`].
pub fn perform<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    op: &TextOp,
) -> TextReply {
    let result = match op {
        TextOp::Read(read) => read_unit(source, anchors, read),
        TextOp::ReadAhead(ahead) => read_ahead(source, anchors, ahead),
        TextOp::ReadRange {
            start,
            end,
            through,
        } => read_range(source, anchors, (*start, *end), *through),
        TextOp::Select {
            start,
            end,
            through,
        } => {
            // Core speaks the selection it made from this reply, and the
            // selection is kept as a caret report's is, so the
            // application's caret event for it carries no change of it,
            // and the next key compares with it.
            select_text(source, anchors, (*start, *end), *through)
        }
        TextOp::MoveCaret(point) => {
            let read_at_us = verbatim_process::clock::now_us();
            select(source, anchors, *point, None, read_at_us)
        }
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
    chunk_from(
        source,
        anchors,
        (unit, kind),
        (offset, Some(at)),
        (runs, formats),
    )
}

/// [`chunk`], remembering the point at its offset as `at` only when that
/// is known: a point not remembered is found from the chunk's start when
/// a request names it.
pub(crate) fn chunk_from<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (unit, kind): (&Unit<S::Pos>, TextUnit),
    (offset, at): (usize, Option<S::Pos>),
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
    if let Some(at) = at {
        anchors.remember(
            TextPosition {
                anchor: start,
                offset,
            },
            at,
        );
    }
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

/// The report for a caret read another program already made, begun at
/// `read_at_us` (a terminal's, read with its new output). With
/// `keeps_last_read` the node's last read stays as it was, while a key's
/// note lives: this read carries no facts, so replacing the one baseline
/// would consume the key's effect unreported.
///
/// # Errors
///
/// [`TextError`] when the line's anchor cannot be made.
pub fn caret_report_from<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read: CaretRead<S::Pos>,
    (read_at_us, keeps_last_read): (u64, bool),
) -> TextResult<CaretReport> {
    let kept = keeps_last_read.then(|| anchors.anchors.last.clone());
    let report = report_for(
        source,
        anchors,
        (&read.state, read_at_us),
        (
            Some((read.line.0, read.line.1, read.formats)),
            Some(read.selection_start),
        ),
        None,
    );
    if let Some(kept) = kept {
        anchors.anchors.last = kept;
    }
    report
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

/// A unit read at the caret: the unit, the caret's UTF-16 offset in it, and
/// its formatting, UTF-16 ranges of its text (empty when none was read).
type ReadUnit<P> = (Unit<P>, usize, Vec<Formatting>);

/// The caret's line when it was read already, and where the selection
/// starts when the caret was read in one go ([`report_for`]).
type LineRead<P> = (Option<ReadUnit<P>>, Option<SelectionStart>);

/// The report for a caret state already read, and the time its read
/// began, with its line when that was read already, and, when the caret
/// was read in one go, where the selection starts in that line
/// ([`CaretRead::selection_start`]). The selection's start is reported as
/// a position in the line's chunk when it lies on the line, so Core knows
/// what comes before it (a Backspace over the selection says that); read
/// by parts, it lies on the line where it is the caret. The report
/// replaces the node's last read, with the selection's text when the read
/// had it ([`CaretRead::kept_text`]).
fn report_for<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (state, read_at_us): (&CaretState<S::Pos>, u64),
    (line, selection_start): LineRead<S::Pos>,
    selection_text: Option<Vec<u16>>,
) -> TextResult<CaretReport> {
    let (line, offset, formats) = if let Some(read) = line {
        read
    } else {
        let (line, offset) = caret_line(source, state)?;
        (line, offset, Vec::new())
    };
    let start = match (&state.selection, selection_start) {
        (None, _) => SelectionStart::unknown(),
        (Some(_), Some(read)) => read,
        (Some((start, _)), None) => SelectionStart {
            in_line: (source.compare(start, &state.caret)? == Ordering::Equal).then_some(offset),
            ..SelectionStart::unknown()
        },
    };
    let start_in_line = start
        .in_line
        .filter(|&start| start <= line.text.len())
        .map(|start| to_utf8(&line.text, MAX_CHUNK_BYTES, &[start]).1[0]);
    let mut line = chunk(
        source,
        anchors,
        (&line, TextUnit::Line),
        (offset, state.caret.clone()),
        (Vec::new(), &formats),
    );
    line.first |= start.first && start_in_line.is_some();
    let line_anchor = line.start;
    let line_length = line.text.len();
    let selection = state.selection.as_ref().map(|(start, end)| Selection {
        start: match start_in_line.filter(|&offset| offset <= line_length) {
            Some(offset) => {
                let position = TextPosition {
                    anchor: line_anchor,
                    offset: u32::try_from(offset).unwrap_or(u32::MAX),
                };
                anchors.remember(position, start.clone());
                position
            }
            None => anchors.position_of(start.clone()),
        },
        end: anchors.position_of(end.clone()),
    });
    let selection = selection.map(|range| CaretSelection { range });

    anchors.anchors.last = Some(LastRead {
        caret: Slot::new(state.caret.clone()),
        selection: state.selection.as_ref().zip(selection.as_ref()).map(
            |((start, end), report)| ReadSelection {
                start: Slot::new(start.clone()),
                end: Slot::new(end.clone()),
                text: selection_text.map(Arc::from),
                report: report.clone(),
            },
        ),
        line: Some(line.clone()),
        read_at_us,
        unit: None,
    });
    Ok(CaretReport { line, selection })
}

/// The characters (grapheme clusters) just before and at byte `offset` of a
/// line, either empty at the line's ends.
fn beside(line: &str, offset: usize) -> (String, String) {
    let offset = verbatim_text::floor_boundary(line, offset);
    let before = verbatim_text::graphemes(&line[..offset])
        .pop()
        .map(|range| line[range].to_owned())
        .unwrap_or_default();
    let at = verbatim_text::grapheme_at(line, offset)
        .map(|range| line[range].to_owned())
        .unwrap_or_default();
    (before, at)
}

/// Whether caret states `a` and `b` have the same caret and selection.
fn same_state<S: TextSource>(
    source: &mut S,
    a: &CaretState<S::Pos>,
    b: &CaretState<S::Pos>,
) -> TextResult<bool> {
    if source.compare(&a.caret, &b.caret)? != Ordering::Equal {
        return Ok(false);
    }
    match (&a.selection, &b.selection) {
        (None, None) => Ok(true),
        (Some((a_start, a_end)), Some((b_start, b_end))) => Ok(source.compare(a_start, b_start)?
            == Ordering::Equal
            && source.compare(a_end, b_end)? == Ordering::Equal),
        _ => Ok(false),
    }
}

/// One selection change between two positions, its text read.
fn change<S: TextSource>(
    source: &mut S,
    selected: bool,
    start: &S::Pos,
    end: &S::Pos,
) -> TextResult<SelectionChange> {
    let (units, _) = source.text(start, end, MAX_RANGE_UNITS)?;
    Ok(change_of(selected, &units))
}

/// A selection change of the UTF-16 text `units`, which may be empty: a
/// part whose text the application deleted is still a change, as NVDA's
/// `speakSelectionChange` lists it, and Core decides what it says.
fn change_of(selected: bool, units: &[u16]) -> SelectionChange {
    let (full, _, _) = to_utf8(units, MAX_RANGE_BYTES, &[]);
    let count = characters(&full);
    let mut text = full;
    // Cut, when it is cut, between whole characters.
    if text.len() > MAX_SELECTION_TEXT_BYTES {
        text.truncate(grapheme_floor(&text, MAX_SELECTION_TEXT_BYTES));
    }
    SelectionChange {
        selected,
        text,
        characters: count,
    }
}

/// How the selection changed from `previous` (its ends, equal for none,
/// and whether nothing was selected) to what `state` holds, by the
/// contract's rule: two selections that neither overlap nor touch, or
/// either of which holds nothing, are an unselection of the whole old then
/// a selection of the whole new; otherwise the start side changes, then the
/// end side. Each part is listed whatever its text, empty included: Core
/// decides the order the parts are spoken in and what an empty one says.
fn selection_changes<S: TextSource>(
    source: &mut S,
    (previous, old_collapsed): (&(S::Pos, S::Pos), bool),
    state: &CaretState<S::Pos>,
) -> TextResult<Vec<SelectionChange>> {
    let (old_start, old_end) = previous;
    let (new_start, new_end) = state
        .selection
        .clone()
        .unwrap_or_else(|| (state.caret.clone(), state.caret.clone()));
    // The old selection holds nothing when it held nothing, or when its
    // ends now meet, as a UI Automation range's do once its text was
    // deleted: NVDA judges its old range as it now is.
    let old_collapsed = old_collapsed || source.compare(old_start, old_end)? == Ordering::Equal;
    let mut changes = Vec::new();
    // NVDA speaks the whole new selection when the old held nothing, and

    // the whole old one when the new holds nothing, wherever the caret was
    // in it (`speakSelectionChange`, `nvda/source/speech/speech.py` 1362 to
    // 1365): a double-click inside a word is the word selected, once.
    let whole = old_collapsed
        || state.selection.is_none()
        || source.compare(&new_end, old_start)? == Ordering::Less
        || source.compare(&new_start, old_end)? == Ordering::Greater;
    if whole {
        // A side that holds nothing has no text to read.
        if !old_collapsed {
            changes.push(change(source, false, old_start, old_end)?);
        }
        if state.selection.is_some() {
            changes.push(change(source, true, &new_start, &new_end)?);
        }
        return Ok(changes);
    }
    match source.compare(&new_start, old_start)? {
        Ordering::Less => changes.push(change(source, true, &new_start, old_start)?),
        Ordering::Greater => changes.push(change(source, false, old_start, &new_start)?),
        Ordering::Equal => {}
    }
    match source.compare(&new_end, old_end)? {
        Ordering::Greater => changes.push(change(source, true, old_end, &new_end)?),
        Ordering::Less => changes.push(change(source, false, &new_end, old_end)?),
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

/// Two points resolved, in document order; with `through`, the earlier and
/// the point just past the character at the later.
fn ordered<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (start, end): (TextPoint, TextPoint),
    through: Option<TextThrough>,
) -> OrReply<(S::Pos, S::Pos)> {
    let start = match point(source, anchors, start)? {
        Ok(pos) => pos,
        Err(reply) => return Ok(Err(reply)),
    };
    let end = match point(source, anchors, end)? {
        Ok(pos) => pos,
        Err(reply) => return Ok(Err(reply)),
    };
    let reversed = source.compare(&start, &end)? == Ordering::Greater;
    let Some(through) = through else {
        return Ok(Ok(if reversed { (end, start) } else { (start, end) }));
    };
    let (first, past) = if reversed {
        (end, through.start)
    } else {
        (start, through.end)
    };
    Ok(match point(source, anchors, past)? {
        Ok(past) => Ok((first, past)),
        Err(reply) => Err(reply),
    })
}

/// The text between two points, for a copy; with `through`, through the
/// character at the later.
fn read_range<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (start, end): (TextPoint, TextPoint),
    through: Option<TextThrough>,
) -> TextResult<TextReply> {
    if let Some(reply) = range_at_once(
        source,
        anchors,
        (start, Some(end), through),
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
    let (start, end) = match ordered(source, anchors, (start, end), through)? {
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
    (start, end, through): (TextPoint, Option<TextPoint>, Option<TextThrough>),
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
    // The points just past the two characters are found by the program
    // and not remembered: the review copy sends them once a press.
    let through = match through
        .map(|through| {
            Ok::<_, TextReply>((
                point_from(anchors, through.start)?.0,
                point_from(anchors, through.end)?.0,
            ))
        })
        .transpose()
    {
        Ok(through) => through,
        Err(reply) => return Ok(Some(Err(reply))),
    };
    let Some(read) = source.range(
        &start,
        end.as_ref().map(|(end, _)| end),
        through
            .as_ref()
            .map(|(past_start, past_end)| (past_start, past_end)),
        action,
    )?
    else {
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
    read_at_us: u64,
) -> TextResult<TextReply> {
    let moving = end.is_none();
    if let Some(reply) = range_at_once(source, anchors, (start, end, None), RangeAction::Select)? {
        return Ok(match reply {
            Ok(read) if read.selected => {
                if moving {
                    anchors.keep_made((read.start.clone(), read.start), false, None, read_at_us);
                }
                TextReply::Moved { read_at_us }
            }
            Ok(_) => TextReply::Unsupported,
            Err(reply) => reply,
        });
    }
    let (start, end) = match ordered(source, anchors, (start, end.unwrap_or(start)), None)? {
        Ok(range) => range,
        Err(reply) => return Ok(reply),
    };
    Ok(if source.select(&start, &end)? {
        if moving {
            anchors.keep_made((start.clone(), start), false, None, read_at_us);
        }
        TextReply::Moved { read_at_us }
    } else {
        TextReply::Unsupported
    })
}

/// Selects between two points, answering how the selection changed: the
/// selection read before selecting, then compared with the one read after,
/// by the rule a caret key's reply follows ([`selection_changes`]), so Core
/// speaks a selection it made as it speaks one a key made. A source that
/// can does all of it at once ([`RangeAction::SelectReporting`], one round
/// trip through UIA); the others read, select, and read again.
fn select_text<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (start, end): (TextPoint, TextPoint),
    through: Option<TextThrough>,
) -> TextResult<TextReply> {
    let read_at_us = verbatim_process::clock::now_us();
    if let Some(reply) = range_at_once(
        source,
        anchors,
        (start, Some(end), through),
        RangeAction::SelectReporting,
    )? {
        return Ok(match reply {
            Ok(read) if read.selected => {
                if let Some(after) = read.selection_after {
                    anchors.keep_made(after.ends, after.selected, after.text, read_at_us);
                }
                TextReply::Selected(
                    read.changes
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(selected, text)| change_of(selected, &text))
                        .collect(),
                )
            }
            Ok(_) => TextReply::Unsupported,
            Err(reply) => reply,
        });
    }
    let before = source.caret()?;
    let was_collapsed = before.selection.is_none();
    let previous = before
        .selection
        .unwrap_or_else(|| (before.caret.clone(), before.caret));
    let (start, end) = match ordered(source, anchors, (start, end), through)? {
        Ok(range) => range,
        Err(reply) => return Ok(reply),
    };
    if !source.select(&start, &end)? {
        return Ok(TextReply::Unsupported);
    }
    let state = source.caret()?;
    let changes = selection_changes(source, (&previous, was_collapsed), &state)?;
    let selected = state.selection.is_some();
    let ends = state
        .selection
        .unwrap_or_else(|| (state.caret.clone(), state.caret));
    anchors.keep_made(ends, selected, None, read_at_us);
    Ok(TextReply::Selected(changes))
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
    let measured = source.measure(&at)?;
    let point = source.location(&at)?;
    Ok(match (measured, point) {
        // Text measured by offsets says how far through it the point is,
        // with or without its place on the screen
        // (`docs/parity/review.md`, "Review cursor line/word/character
        // over object text").
        (Some((offset, length)), point) => TextReply::OffsetLocation {
            offset,
            length,
            point,
        },
        (None, Some((x, y))) => TextReply::Location { x, y },
        (None, None) => TextReply::Unsupported,
    })
}

#[cfg(test)]
mod tests;
