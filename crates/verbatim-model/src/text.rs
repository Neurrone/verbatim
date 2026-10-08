//! The text protocol (milestone M4): how an outpost sends text to Core and
//! how Core asks an outpost to read text, wait for the caret, select, or
//! move the caret (`phase6-design.md`, "M4: text, editing, and terminals"
//! and "Internationalization in the text model").
//!
//! The rules the types follow:
//!
//! - Positions are the provider's, not Core's. A position is a
//!   [`TextPosition`]: an opaque [`TextAnchor`] the outpost minted, plus a
//!   byte offset into the UTF-8 text of the [`TextChunk`] that anchor
//!   started. Core never does arithmetic on provider positions; it only
//!   slices text it received, and the outpost converts between UTF-8 bytes
//!   and its own units (UTF-16 code units, UIA text ranges) at the
//!   boundary.
//! - Text arrives with the event that needs it: a caret report carries the
//!   line at the caret, so Core can answer the next key (Backspace, the
//!   review cursor following the caret) without a round trip. Larger units
//!   are read on request, one unit at a time, never the whole document.
//! - Every chunk is capped at [`MAX_CHUNK_BYTES`], so one unit cannot grow
//!   Core's state without bound.
//! - A unit the provider does not support is answered
//!   [`TextReply::UnsupportedUnit`], never approximated silently, and
//!   movement stops at the document's ends: a [`TextReply::Read`] reports
//!   how far it really moved.
//! - Text carries its language in [`LanguageRun`]s, so speech can switch
//!   voices and word segmentation can follow the language.

use serde::{Deserialize, Serialize};

use crate::event::QueryId;
use crate::{NodeId, OutpostId};

/// The most UTF-8 bytes of text one [`TextChunk`] carries. A longer unit
/// (a minified file's single line) is cut at a character boundary at or
/// before this length and marked [`TextChunk::truncated`].
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;

/// The most UTF-8 bytes of text one [`SelectionChange`] carries; its
/// [`characters`](SelectionChange::characters) count is always complete.
pub const MAX_SELECTION_TEXT_BYTES: usize = 4 * 1024;

/// The most UTF-8 bytes of text a [`TextReply::Range`] carries, for a copy
/// to the clipboard.
pub const MAX_RANGE_BYTES: usize = 1024 * 1024;

/// The most units one [`TextOp::ReadAhead`] asks for.
pub const MAX_READ_AHEAD: u8 = 32;

/// A [`TextOp::ReadAhead`] stops adding units once the text it read
/// reaches this many UTF-16 code units (as the providers count text, so at
/// most three times as many UTF-8 bytes), so a batch of long units stays
/// bounded; it always carries at least one.
pub const MAX_READ_AHEAD_TEXT: usize = 32 * 1024;

/// An opaque position in a node's text, minted by the outpost that owns the
/// node: a UIA text range's start, a Win32 edit control's UTF-16 offset, or
/// whatever the backend needs to find the position again.
///
/// An anchor means something only together with the node it was minted
/// for, and only to that node's outpost incarnation. The outpost keeps
/// every anchor Core holds (`SrState::held_anchors` in `verbatim-core`,
/// sent to the outpost alongside the held nodes) and may forget any other
/// once it has minted 64 newer ones for the node; a request naming a
/// forgotten anchor is answered [`TextReply::AnchorLost`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TextAnchor(pub u64);

/// A position in a node's text: `offset` UTF-8 bytes into the text of the
/// chunk that starts at `anchor`, as the outpost sent that chunk. The
/// outpost resolves it by reading forward from the anchor and converting
/// the byte count into its own units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextPosition {
    /// The start of the chunk the offset counts into.
    pub anchor: TextAnchor,
    /// The byte offset into that chunk's UTF-8 text; always a character
    /// boundary of it.
    pub offset: u32,
}

impl TextPosition {
    /// The position at `anchor` itself.
    #[must_use]
    pub const fn at(anchor: TextAnchor) -> Self {
        Self { anchor, offset: 0 }
    }
}

/// Where a text request starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextPoint {
    /// The caret. A node with text but no caret (static text, a read-only
    /// document without one) answers from the start of its text, as NVDA's
    /// object review falls back to the first position.
    Caret,
    /// The start of the selection; the caret when nothing is selected.
    SelectionStart,
    /// The end of the selection (just past its last character); the caret
    /// when nothing is selected.
    SelectionEnd,
    /// The start of the node's text.
    Start,
    /// The end of the node's text: the position after its last character,
    /// so expanding to a line there reads the last line.
    End,
    /// A position Core holds.
    At(TextPosition),
}

/// A text unit, NVDA's `UNIT_*` vocabulary as far as M4 uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextUnit {
    /// One character: a grapheme cluster, whatever the provider's own
    /// character unit says (an emoji sequence or a letter with combining
    /// marks is one character).
    Character,
    /// The provider's word, which Core speaks where the application moved
    /// the caret itself (Control+Right Arrow), so speech matches where the
    /// caret went.
    Word,
    /// The provider's line, soft-wrapped lines included. Core never splits
    /// text on line breaks to find lines.
    Line,
    /// A sentence. Providers without one (UIA has none) answer
    /// [`TextReply::UnsupportedUnit`].
    Sentence,
    /// The provider's paragraph.
    Paragraph,
    /// The provider's page, where it has pages.
    Page,
    /// The whole text. Never read as a chunk (it can be any size); used only
    /// to move to the start or end.
    Document,
}

/// The language of a span of a chunk's text, from the provider's culture
/// or language attribute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageRun {
    /// Byte offset of the span's start in the chunk's text.
    pub start: u32,
    /// Byte offset just past the span's end.
    pub end: u32,
    /// BCP 47 language tag, such as `zh-CN`.
    pub language: String,
}

/// One unit of text an outpost read: its text, where it starts, the point
/// of interest inside it, and what is known about the document's ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    /// The unit the chunk is.
    pub unit: TextUnit,
    /// The text, as UTF-8, line breaks included as the provider gives them,
    /// cut to [`MAX_CHUNK_BYTES`].
    pub text: String,
    /// An anchor at the start of the chunk, for positions inside it.
    pub start: TextAnchor,
    /// The byte offset in `text` of the point the request was about: the
    /// caret in a caret report, the point read at after any movement in a
    /// read. Always a character boundary, and at most `text.len()`.
    pub offset: u32,
    /// The languages of spans of the text; empty when the provider reports
    /// none. Spans do not overlap and are in order; text outside every span
    /// is in the node's default language.
    #[serde(default)]
    pub languages: Vec<LanguageRun>,
    /// The chunk is known to be the document's first unit of its kind; false
    /// when it is not, or when the outpost could not tell cheaply.
    #[serde(default)]
    pub first: bool,
    /// The chunk is known to be the document's last unit of its kind.
    #[serde(default)]
    pub last: bool,
    /// The text was cut at [`MAX_CHUNK_BYTES`].
    #[serde(default)]
    pub truncated: bool,
    /// The formatting of the text, stretch by stretch, where the outpost
    /// read it (milestone M4 item 7): in order, not overlapping, each a
    /// byte range of `text` on character boundaries. Empty when none was
    /// read; a caret report carries it for the text that is spoken, with
    /// only the attributes the theme asks for ([`crate::Fetches`]).
    #[serde(default)]
    pub formats: Vec<FormatRun>,
}

/// The formatting of a stretch of text, as an outpost read it (milestone M4
/// item 7; `docs/nvda/document-formatting.md`). An attribute the provider
/// does not expose, or that was not read because its indication is off, is
/// `None` (false for the errors).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextAttributes {
    /// The text is a spelling error.
    pub spelling_error: bool,
    /// The text is a grammar error.
    pub grammar_error: bool,
    /// The font's name, "Calibri".
    pub font_name: Option<String>,
    /// The font's size as spoken, "11.0 pt".
    pub font_size: Option<String>,
    /// The text's color as spoken, "dark red".
    pub color: Option<String>,
    /// Whether the text is bold.
    pub bold: Option<bool>,
    /// Whether the text is italic.
    pub italic: Option<bool>,
    /// Whether the text is underlined, read for the font attributes.
    pub underline: Option<bool>,
    /// The kind of underline, read for its own indication; when it is
    /// read, underlining is reported by its kind.
    pub underline_style: Option<LineStyle>,
    /// The kind of line through the text, [`LineStyle::None`] for none.
    pub strikethrough: Option<LineStyle>,
    /// The background's color as spoken, "light grey".
    pub background_color: Option<String>,
    /// The bullet of the list item the text is in, [`BulletStyle::None`]
    /// outside a list.
    pub bullet: Option<BulletStyle>,
    /// The text is a link.
    pub link: bool,
}

/// A kind of line drawn under or through text: UIA's text decoration line
/// styles, named as Microsoft Word names its underlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineStyle {
    /// No line.
    None,
    /// One line.
    Single,
    /// One line under the words but not the spaces between them.
    WordsOnly,
    /// Two lines.
    Double,
    /// Dots.
    Dotted,
    /// Dashes.
    Dashed,
    /// Dots and dashes.
    DotDash,
    /// Two dots, then a dash.
    DotDotDash,
    /// A wave.
    Wavy,
    /// One thick line.
    Thick,
    /// Two waves.
    DoubleWavy,
    /// A thick wave.
    ThickWavy,
    /// Long dashes.
    LongDash,
    /// Thick dashes.
    ThickDashed,
    /// Thick dots and dashes.
    ThickDotDash,
    /// Thick dots, two then a dash.
    ThickDotDotDash,
    /// Thick dots.
    ThickDotted,
    /// Thick long dashes.
    ThickLongDash,
    /// A line of a kind the application does not name.
    Other,
}

impl LineStyle {
    /// Whether there is a line.
    #[must_use]
    pub const fn is_drawn(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The bullet of a list item: UIA's bullet styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BulletStyle {
    /// Not a bulleted list item.
    None,
    /// A hollow round bullet.
    HollowRound,
    /// A filled round bullet.
    FilledRound,
    /// A hollow square bullet.
    HollowSquare,
    /// A filled square bullet.
    FilledSquare,
    /// A dash.
    Dash,
    /// A bullet of a kind the application does not name.
    Other,
}

/// One stretch of a chunk's text with the same formatting: a byte range of
/// the chunk's text and its attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatRun {
    /// The byte offset in the chunk's text where the stretch starts.
    pub start: u32,
    /// The byte offset just past its end.
    pub end: u32,
    /// Its formatting.
    pub attributes: TextAttributes,
}

impl TextChunk {
    /// The language of the text at byte `offset`, when a run covers it.
    #[must_use]
    pub fn language_at(&self, offset: usize) -> Option<&str> {
        self.languages
            .iter()
            .find(|run| (run.start as usize..run.end as usize).contains(&offset))
            .map(|run| run.language.as_str())
    }
}

/// A selection: its start and its end, the end just past the last selected
/// character. A collapsed selection (start equal to end) is no selection,
/// and is reported as `None` wherever a selection is optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// Where the selection starts.
    pub start: TextPosition,
    /// Where it ends.
    pub end: TextPosition,
}

/// Text that became selected or stopped being selected, which Core speaks
/// as NVDA's "selected" and "unselected" announcements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionChange {
    /// True for text newly selected, false for text no longer selected.
    pub selected: bool,
    /// The text, cut to [`MAX_SELECTION_TEXT_BYTES`].
    pub text: String,
    /// How many characters (grapheme clusters) the whole change has, even
    /// when `text` was cut.
    pub characters: u32,
}

/// Where the caret is: the line it is on, with
/// [`TextChunk::offset`] at the caret, and the selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretReport {
    /// The line containing the caret; its `offset` is the caret.
    pub line: TextChunk,
    /// The selection, when something is selected.
    #[serde(default)]
    pub selection: Option<Selection>,
}

/// What Core asks the outpost to do after it has passed a caret key to the
/// application: watch for evidence of what the key did, then report the
/// caret (`docs/nvda/editable-text-and-terminals.md`, the wait for
/// evidence; `docs/parity.md`, "Text, documents, terminals").
///
/// The outpost reads the caret once when the request arrives, and again
/// whenever the application reports something about the node: a caret
/// event, a text change, or a selection change. It answers with
/// [`TextReply::Caret`], the caret as it then is, as soon as one of these
/// holds:
///
/// - a caret event arrives from the application and `since` is `None`;
/// - the caret is no longer at `since`;
/// - the text of `unit` at the caret differs from `compare` (Delete, which
///   changes the text without moving the caret);
/// - the selection is no longer `previous_selection`.
///
/// It never waits for any of them: the request is a watch the outpost
/// keeps while it handles everything else. A watch that ends with none of
/// them holding, because the next caret key's watch replaced it, the focus
/// moved, or the bound on a watch's age that only frees it passed
/// (`verbatim-outpost`'s `CARET_WATCH_BOUND`), is answered
/// [`TextReply::WatchEnded`], and nothing is spoken: a key that does not
/// move the caret is silent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretWatch {
    /// Where Core last knew the caret to be, before the key; `None` when it
    /// did not know.
    pub since: Option<TextPosition>,
    /// Milliseconds since the Unix epoch when the key was pressed, on the
    /// clock of an event's `observed_at_ms`; 0 when unknown. A caret the
    /// outpost read and reported before this time is where the caret was
    /// before the key, and stands in for `since`, which may be older; one
    /// read at or after it may already show the key's effect, and does not.
    #[serde(default)]
    pub pressed_at_ms: u64,
    /// The unit to report at the caret once evidence arrives, besides the
    /// line.
    pub unit: TextUnit,
    /// The text of `unit` at the caret before the key, when a change of it is
    /// evidence.
    pub compare: Option<String>,
    /// The selection before the key, for a key that changes the selection
    /// (any Shift movement, Control+A); `None` otherwise. When the request
    /// carries one, the reply's `selection_changes` describe how the
    /// selection changed from it.
    pub previous_selection: Option<PreviousSelection>,
}

/// The selection before a selecting key: the selection Core knew, or a
/// collapsed one at the caret when nothing was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousSelection {
    /// Where it started.
    pub start: TextPosition,
    /// Where it ended; equal to `start` when nothing was selected.
    pub end: TextPosition,
}

/// The outpost's answer to a [`CaretWatch`].
///
/// `selection_changes`, when the watch carried a previous selection
/// `[old_start, old_end)` and the selection is now `[new_start, new_end)`
/// (each collapsed at the caret when nothing is selected), are worked out
/// by comparing endpoints, which only the outpost can do:
///
/// - When the two do not overlap or touch (`new_end < old_start` or
///   `new_start > old_end`), the old text, if any, is unselected and the new
///   text, if any, is selected, in that order.
/// - Otherwise, first the start: `[new_start, old_start)` is selected when
///   the start moved back, `[old_start, new_start)` unselected when it
///   moved forward; then the end: `[old_end, new_end)` is selected when
///   the end moved forward, `[new_end, old_end)` unselected when it moved
///   back.
///
/// Empty changes are left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretReply {
    /// Whether evidence that the key did something arrived. An outpost
    /// answers a watch only on evidence, so it sends true; Core says
    /// nothing for a reply without it.
    pub moved: bool,
    /// The caret as it is now.
    pub caret: CaretReport,
    /// Milliseconds since the Unix epoch when the outpost read `caret`, on
    /// the clock of an event's `observed_at_ms`; 0 when unknown. Core
    /// compares it with a later key's `pressed_at_ms` to tell whether this
    /// caret was where that key found it.
    #[serde(default)]
    pub read_at_ms: u64,
    /// The watch's unit at the caret; `None` when the unit is
    /// [`TextUnit::Line`], which `caret.line` already is.
    #[serde(default)]
    pub unit: Option<TextChunk>,
    /// How the selection changed, when the watch asked.
    #[serde(default)]
    pub selection_changes: Vec<SelectionChange>,
}

/// A movement by whole units, positive forward and negative back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMovement {
    /// The unit moved by.
    pub unit: TextUnit,
    /// How many units, and which way.
    pub count: i32,
}

/// Read one unit: start at `at`, move by `movement` when there is one,
/// expand to the `unit` containing the point reached, and reply with it.
/// The reply's chunk has its `offset` at the point reached. Moving by a
/// unit lands on that unit's start, as NVDA's `move` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRead {
    /// Where to start.
    pub at: TextPoint,
    /// How to move first, if at all.
    pub movement: Option<TextMovement>,
    /// The unit to read where the movement ends.
    pub unit: TextUnit,
}

/// Read several units ahead, for say-all: the first as [`TextRead`] reads
/// it (from `at`, after `movement`), then each next unit of the same kind,
/// up to `count` units (at most [`MAX_READ_AHEAD`]), stopping early once
/// the text read reaches [`MAX_READ_AHEAD_TEXT`]. One round trip where the
/// provider runs remote operations, however many units it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextReadAhead {
    /// Where to start.
    pub at: TextPoint,
    /// How to move first, if at all.
    pub movement: Option<TextMovement>,
    /// The unit to read.
    pub unit: TextUnit,
    /// How many units to read.
    pub count: u8,
}

/// One operation Core asks an outpost to perform on a node's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextOp {
    /// Watch for a passed caret key's evidence, then report the caret:
    /// answered [`TextReply::Caret`] once evidence arrives, or
    /// [`TextReply::WatchEnded`] when the watch ends without it.
    AwaitCaret(CaretWatch),
    /// Read one unit: answered [`TextReply::Read`].
    Read(TextRead),
    /// Read the text between two points, in document order whichever comes
    /// first, for a copy: answered [`TextReply::Range`].
    ReadRange {
        /// One end.
        start: TextPoint,
        /// The other end.
        end: TextPoint,
    },
    /// Select the text between two points, and move the caret there where
    /// the application allows: answered [`TextReply::Done`], or
    /// [`TextReply::Unsupported`] when the text cannot be selected.
    Select {
        /// One end.
        start: TextPoint,
        /// The other end.
        end: TextPoint,
    },
    /// Move the caret to a point (say-all moving the caret as it reads):
    /// answered [`TextReply::Done`] or [`TextReply::Unsupported`].
    MoveCaret(TextPoint),
    /// The screen position of a point: answered [`TextReply::Location`], or
    /// [`TextReply::Unsupported`] when the provider cannot tell.
    Location(TextPoint),
    /// Read several units ahead (say-all): answered [`TextReply::Chunks`],
    /// or as [`TextOp::Read`] is when the first unit cannot be read.
    ReadAhead(TextReadAhead),
}

/// A text request from Core to the outpost that owns `node_id`; the answer
/// comes back as `Input::TextCompleted` with the same `query_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRequest {
    /// Correlates the answer with this request.
    pub query_id: QueryId,
    /// The node whose text to use. Its outpost is the one asked.
    pub node_id: NodeId,
    /// What to do.
    pub op: TextOp,
}

/// An outpost's answer to a [`TextRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextReply {
    /// The answer to [`TextOp::AwaitCaret`].
    Caret(Box<CaretReply>),
    /// The answer to [`TextOp::Read`]: `moved` is how many units the
    /// movement really went, which is less than asked (and zero for a
    /// movement that could not start) at the document's ends; movement
    /// never wraps.
    Read {
        /// Units moved, signed like the request.
        moved: i32,
        /// The unit read where the movement ended.
        chunk: TextChunk,
    },
    /// The answer to [`TextOp::ReadAhead`]: `moved` as for
    /// [`TextReply::Read`], and the units read, in order, at least one. The
    /// last is marked [`TextChunk::last`] when the outpost found no unit
    /// after it; fewer than asked otherwise means the batch reached
    /// [`MAX_READ_AHEAD_TEXT`].
    Chunks {
        /// Units moved, signed like the request.
        moved: i32,
        /// The units read.
        chunks: Vec<TextChunk>,
    },
    /// The answer to [`TextOp::ReadRange`].
    Range {
        /// The text, cut to [`MAX_RANGE_BYTES`].
        text: String,
        /// The text was cut.
        truncated: bool,
    },
    /// A [`TextOp::Select`] or [`TextOp::MoveCaret`] was done.
    Done,
    /// The answer to [`TextOp::Location`]: screen coordinates of the point,
    /// in pixels.
    Location {
        /// Screen x.
        x: i32,
        /// Screen y.
        y: i32,
    },
    /// The provider has no such unit (UIA has no sentence; a plain edit
    /// control has no page). Core falls back to a unit it has.
    UnsupportedUnit(TextUnit),
    /// The provider cannot do this operation on this text.
    Unsupported,
    /// The node has no text interface at all. Core falls back to the node's
    /// value or name, as NVDA's object review does.
    NoText,
    /// A [`TextPosition`] named an anchor the outpost no longer has.
    AnchorLost,
    /// The node no longer exists.
    Gone,
    /// The application did not answer in time, or the read failed.
    Unanswered,
    /// A [`TextOp::AwaitCaret`] whose watch ended before any evidence that
    /// the key did something arrived: the next caret key's watch replaced
    /// it, the focus moved, or the bound on its age passed. Core says
    /// nothing.
    WatchEnded,
}

/// What a caret key does, as Core's caret handling knows it: the motion
/// the application performs, and whether it extends the selection (the
/// key was pressed with Shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CaretKey {
    /// The motion.
    pub motion: CaretMotion,
    /// Shift was held: the selection changes, and the change is spoken.
    pub select: bool,
}

/// The caret motions of NVDA's editable-text commands
/// (`docs/nvda/editable-text-and-terminals.md`). The key is always passed
/// to the application, which moves the caret; Core only reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CaretMotion {
    /// Left Arrow.
    PreviousCharacter,
    /// Right Arrow.
    NextCharacter,
    /// Control+Left Arrow.
    PreviousWord,
    /// Control+Right Arrow.
    NextWord,
    /// Up Arrow.
    PreviousLine,
    /// Down Arrow.
    NextLine,
    /// Control+Up Arrow.
    PreviousParagraph,
    /// Control+Down Arrow.
    NextParagraph,
    /// Home.
    StartOfLine,
    /// End.
    EndOfLine,
    /// Page Up.
    PreviousPage,
    /// Page Down.
    NextPage,
    /// Control+Home.
    Top,
    /// Control+End.
    Bottom,
    /// Backspace: deletes the character before the caret.
    Backspace,
    /// Control+Backspace: deletes the word before the caret.
    BackspaceWord,
    /// Delete: deletes the character at the caret.
    Delete,
    /// Control+Delete: deletes the word at the caret.
    DeleteWord,
    /// Control+A: selects everything.
    SelectAll,
}

impl CaretMotion {
    /// Whether the key deletes text rather than moving the caret: Backspace
    /// and Delete, alone or with Control.
    #[must_use]
    pub const fn deletes(self) -> bool {
        matches!(
            self,
            Self::Backspace | Self::BackspaceWord | Self::Delete | Self::DeleteWord
        )
    }

    /// The unit spoken after the motion: the character for character keys,
    /// Home, End, and Delete; the provider's word for word keys; the
    /// paragraph for paragraph keys; the line for line, page, and document
    /// keys.
    #[must_use]
    pub const fn unit(self) -> TextUnit {
        match self {
            Self::PreviousCharacter
            | Self::NextCharacter
            | Self::StartOfLine
            | Self::EndOfLine
            | Self::Backspace
            | Self::Delete => TextUnit::Character,
            Self::PreviousWord | Self::NextWord | Self::BackspaceWord | Self::DeleteWord => {
                TextUnit::Word
            }
            Self::PreviousParagraph | Self::NextParagraph => TextUnit::Paragraph,
            Self::PreviousLine
            | Self::NextLine
            | Self::PreviousPage
            | Self::NextPage
            | Self::Top
            | Self::Bottom
            | Self::SelectAll => TextUnit::Line,
        }
    }
}

/// Text anchors grouped by the outpost that minted them.
pub type HeldAnchors =
    std::collections::BTreeMap<OutpostId, std::collections::BTreeSet<TextAnchor>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_finds_the_language_at_an_offset() {
        let chunk = TextChunk {
            unit: TextUnit::Line,
            text: "hello 你好".to_owned(),
            start: TextAnchor(1),
            offset: 0,
            languages: vec![LanguageRun {
                start: 6,
                end: 12,
                language: "zh-CN".to_owned(),
            }],
            first: true,
            last: false,
            truncated: false,
            formats: Vec::new(),
        };
        assert_eq!(chunk.language_at(0), None);
        assert_eq!(chunk.language_at(6), Some("zh-CN"));
    }

    #[test]
    fn a_reply_round_trips_through_json() {
        let reply = TextReply::Read {
            moved: -1,
            chunk: TextChunk {
                unit: TextUnit::Line,
                text: "line\r\n".to_owned(),
                start: TextAnchor(7),
                offset: 0,
                languages: Vec::new(),
                first: false,
                last: true,
                truncated: false,
                formats: Vec::new(),
            },
        };
        let json = serde_json::to_string(&reply).expect("serializes");
        let back: TextReply = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, reply);
    }
}
