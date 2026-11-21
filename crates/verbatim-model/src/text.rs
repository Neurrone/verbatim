//! The text protocol (milestone M4): how an outpost sends text to Core and
//! how Core asks an outpost to read text, select, or move the caret (`books/dev/src/reference/crates/verbatim-model.md`, "The text protocol").
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

use crate::OutpostId;

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
/// for, and only to that node's outpost generation. The outpost keeps
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

/// A text unit, of the units NVDA's text ranges move and expand by
/// (`docs/nvda/text-infos.md`, "The contract"), as far as M4 uses them.
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
/// of interest inside it, and what is known about the document's ends. A
/// chunk whose offset, language spans or formatting stretches are not
/// character boundaries of its text, or lie past it, does not decode,
/// so Core takes each as a boundary with no clamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ChunkFields")]
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
    /// only the attributes its request asks for ([`TextDetails`]).
    #[serde(default)]
    pub formats: Vec<FormatRun>,
}

/// The formatting of a stretch of text, as an outpost read it (milestone M4
/// item 7; `docs/nvda/document-formatting.md`). An attribute the provider
/// does not expose, or that its read did not ask for ([`TextDetails`]), is
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

/// A [`TextChunk`] as it decodes, before its offsets are checked against
/// its text.
#[derive(Deserialize)]
struct ChunkFields {
    unit: TextUnit,
    text: String,
    start: TextAnchor,
    offset: u32,
    #[serde(default)]
    languages: Vec<LanguageRun>,
    #[serde(default)]
    first: bool,
    #[serde(default)]
    last: bool,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    formats: Vec<FormatRun>,
}

impl TryFrom<ChunkFields> for TextChunk {
    type Error = String;

    fn try_from(fields: ChunkFields) -> Result<Self, String> {
        let text = &fields.text;
        let at = |offset: u32| usize::try_from(offset).is_ok_and(|at| text.is_char_boundary(at));
        let spans = fields
            .languages
            .iter()
            .map(|run| (run.start, run.end))
            .chain(fields.formats.iter().map(|run| (run.start, run.end)));
        if !at(fields.offset) {
            return Err(format!(
                "offset {} is not a character boundary of a text of {} bytes",
                fields.offset,
                text.len()
            ));
        }
        for (start, end) in spans {
            if start > end || !at(start) || !at(end) {
                return Err(format!(
                    "span {start}..{end} is not on character boundaries of a text of {} bytes",
                    text.len()
                ));
            }
        }
        Ok(Self {
            unit: fields.unit,
            text: fields.text,
            start: fields.start,
            offset: fields.offset,
            languages: fields.languages,
            first: fields.first,
            last: fields.last,
            truncated: fields.truncated,
            formats: fields.formats,
        })
    }
}

impl TextChunk {
    /// The language of the text at byte `offset`, when a run covers it.
    #[must_use]
    pub fn language_at(&self, offset: usize) -> Option<&str> {
        self.languages
            .iter()
            .find(|run| {
                u32::try_from(offset).is_ok_and(|offset| (run.start..run.end).contains(&offset))
            })
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
///
/// The changes from a selection `[old_start, old_end)` to the selection
/// now `[new_start, new_end)` (each collapsed at the caret when nothing is
/// selected) are worked out by comparing endpoints, which only the outpost
/// can do:
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
pub struct SelectionChange {
    /// True for text newly selected, false for text no longer selected.
    pub selected: bool,
    /// The text, cut to [`MAX_SELECTION_TEXT_BYTES`].
    pub text: String,
    /// How many characters (grapheme clusters) the whole change has, even
    /// when `text` was cut.
    pub characters: u32,
}

/// A selection in a caret report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretSelection {
    /// The selection. Its start is the caret, as NVDA's caret is the
    /// selection's start, and so a position in the report's line, so that
    /// what comes before it is known (a Backspace over the selection says
    /// it).
    pub range: Selection,
}

/// Where the caret is: the line it is on, with
/// [`TextChunk::offset`] at the caret, and the selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretReport {
    /// The line containing the caret; its `offset` is the caret.
    pub line: TextChunk,
    /// The selection, when something is selected.
    #[serde(default)]
    pub selection: Option<CaretSelection>,
}

/// What the outpost's caret read found for a focus that may have text,
/// carried with the focus
/// ([`NormalizedEvent::FocusChanged`](crate::NormalizedEvent::FocusChanged)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusText {
    /// The caret: Core speaks its line, or "selected" and the selected text
    /// when something is selected, in place of the focus's value. The
    /// caret's selection is the one the focus's later caret reports are
    /// compared with.
    Caret {
        /// The line at the caret and the selection.
        caret: CaretReport,
        /// The selected text, read with the caret when something is
        /// selected; `None` when nothing is, or it could not be read, and
        /// the line is spoken instead.
        #[serde(default)]
        selected_text: Option<String>,
    },
    /// The node has no text the outpost can read (an MSAA object that is
    /// not an edit control, a UIA element with no text pattern): Core
    /// speaks its value instead, as NVDA speaks the value of an object with
    /// no text interface.
    NoText,
    /// The node has text, or may have, but it could not be read now (its
    /// element is not known, its provider did not answer, or the read
    /// failed): Core speaks neither its line nor its value, which for a
    /// document is all of its text.
    NotRead,
}

/// What a read found compared with the node's last read, by the comparisons
/// only the provider can make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one comparison the outpost makes"
)]
pub struct Compared {
    /// The caret left where it was.
    pub caret_moved: bool,
    /// The selection changed.
    pub selection_moved: bool,
    /// The characters beside the caret changed.
    pub beside_changed: bool,
    /// The unit at the caret changed.
    pub unit_changed: bool,
    /// The caret's line changed.
    pub line_changed: bool,
}

/// Whether `compared` is evidence of a key's effect, by its wait's effect
/// test `effect`: the caret having left where it was, the characters
/// beside it changing, the selection changing, and where the test counts
/// them, the unit at the caret changing (Delete) and the caret's line
/// changing (a key that deletes) (`docs/parity/text.md`).
#[must_use]
pub fn had_effect(compared: Compared, effect: crate::batch::EffectTest) -> bool {
    compared.caret_moved
        || compared.selection_moved
        || compared.beside_changed
        || (effect.unit && compared.unit_changed)
        || (effect.line && compared.line_changed)
}

/// A movement by whole units, positive forward and negative back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMovement {
    /// The unit moved by.
    pub unit: TextUnit,
    /// How many units, and which way.
    pub count: i32,
}

/// How the outpost reads a node's text, which Core names in each caret
/// key's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextProvider {
    /// UI Automation's text pattern.
    Uia,
    /// An edit control's window messages. Of the providers Verbatim reads,
    /// only this one has a sentence unit, found by Unicode's rules.
    EditMessages,
}

impl TextProvider {
    /// Whether the provider has a sentence unit: UI Automation has none.
    #[must_use]
    pub const fn has_sentences(self) -> bool {
        matches!(self, Self::EditMessages)
    }
}

/// A formatting attribute a text read may fetch, one per attribute the
/// theme switches (`books/dev/src/reference/crates/verbatim-core.md`, "Formatting").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FormatAttribute {
    /// The font's name.
    FontName,
    /// The font's size.
    FontSize,
    /// The text's colour.
    Color,
    /// The background's colour.
    BackgroundColor,
    /// Bold, italic and underline.
    FontAttributes,
    /// Strikethrough.
    Strikethrough,
    /// The kind of underline.
    UnderlineStyle,
    /// A list item's bullet.
    BulletStyle,
}

impl FormatAttribute {
    /// Every attribute, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::FontName,
        Self::FontSize,
        Self::Color,
        Self::BackgroundColor,
        Self::FontAttributes,
        Self::Strikethrough,
        Self::UnderlineStyle,
        Self::BulletStyle,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::FontName => 1 << 0,
            Self::FontSize => 1 << 1,
            Self::Color => 1 << 2,
            Self::BackgroundColor => 1 << 3,
            Self::FontAttributes => 1 << 4,
            Self::Strikethrough => 1 << 5,
            Self::UnderlineStyle => 1 << 6,
            Self::BulletStyle => 1 << 7,
        }
    }
}

/// A set of [`FormatAttribute`]s, stored as a bitmask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormatSet(u8);

impl FormatSet {
    /// No attribute.
    pub const NONE: Self = Self(0);
    /// Every attribute.
    pub const ALL: Self = Self::of(&FormatAttribute::ALL);

    /// The set of `attributes`.
    #[must_use]
    pub const fn of(mut attributes: &[FormatAttribute]) -> Self {
        let mut bits = 0;
        while let [first, rest @ ..] = attributes {
            bits |= first.bit();
            attributes = rest;
        }
        Self(bits)
    }

    /// Whether `attribute` is in the set.
    #[must_use]
    pub const fn contains(self, attribute: FormatAttribute) -> bool {
        self.0 & attribute.bit() != 0
    }

    /// This set with `attribute` added.
    #[must_use]
    pub const fn with(self, attribute: FormatAttribute) -> Self {
        Self(self.0 | attribute.bit())
    }

    /// This set without `attribute`.
    #[must_use]
    pub const fn without(self, attribute: FormatAttribute) -> Self {
        Self(self.0 & !attribute.bit())
    }
}

/// What a text read fetches beyond the text: the format attributes the
/// settings ask for, spelling and grammar errors, and links. Core chooses
/// it per request from its settings, the read's unit and its reason, and
/// the outpost reads exactly what it names
/// (`books/dev/src/reference/crates/verbatim-core.md`, "Formatting").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextDetails {
    /// The format attributes.
    pub formats: FormatSet,
    /// Spelling errors.
    pub spelling_errors: bool,
    /// Grammar errors.
    pub grammar_errors: bool,
    /// Links.
    pub links: bool,
}

impl TextDetails {
    /// Every detail.
    pub const ALL: Self = Self {
        formats: FormatSet::ALL,
        spelling_errors: true,
        grammar_errors: true,
        links: true,
    };

    /// Nothing beyond the text.
    pub const NONE: Self = Self {
        formats: FormatSet::NONE,
        spelling_errors: false,
        grammar_errors: false,
        links: false,
    };
}

/// The answer to [`TextOp::MoveBySentence`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentenceMoved {
    /// The caret moved to this sentence, in this line.
    Moved {
        /// The sentence.
        sentence: String,
        /// The line the caret is now on, with the caret at its offset.
        line: TextChunk,
        /// The selection baseline the move minted.
        baseline: TextAnchor,
    },
    /// The caret moved to the text's end, past its last sentence.
    End {
        /// The line the caret is now on, with the caret at its offset.
        line: TextChunk,
        /// The selection baseline the move minted.
        baseline: TextAnchor,
    },
    /// The caret stayed at the text's start, whose first sentence this
    /// is.
    StayedAtStart {
        /// The first sentence.
        sentence: String,
        /// The line the caret is on, with the caret at its offset.
        line: TextChunk,
        /// The selection baseline the move minted.
        baseline: TextAnchor,
    },
}

/// How the selection differs from the selection baseline's range as it
/// now is, in both the terms Core may speak it in: Core chooses at the
/// answer, by its text-changed flag and the value changes waiting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionDifference {
    /// What became selected and unselected, each part's text what its
    /// range now holds, by the rule [`SelectionChange`] describes; empty when the old range and the new selection
    /// are both empty.
    pub changes: Vec<SelectionChange>,
    /// The whole new selection's text, its `selected` true; `None` when
    /// nothing is selected.
    pub new_selection: Option<SelectionChange>,
}

/// An edit control's selection, by its character offsets as the control
/// gives them (`EM_GETSEL` or `EM_EXGETSEL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditSelection {
    /// The start's offset.
    pub start: u32,
    /// The end's offset.
    pub end: u32,
}

/// One operation Core asks an outpost to perform on a node's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextOp {
    /// Read the history above the start the next read takes: from `above`
    /// rows above it to the document's end, or the whole document when
    /// `whole`. Answered [`TextReply::TerminalHistory`], which names the
    /// read whose start that is.
    TerminalHistory {
        /// How many rows above the start.
        above: u32,
        /// Whether to read the whole document instead.
        whole: bool,
        /// The start anchor the rows are counted from, which Core names
        /// (`docs/design/readers-tell-core-asks-build3.md`, "The full read
        /// request and its starts").
        start: TextAnchor,
        /// The screen's height in rows, from Core's model.
        height: u32,
    },
    /// Search the focused terminal's text for `needle`, a row's text with
    /// no line break, outward from the row `from` rows above where the next
    /// read starts (negative below it): alternately the next match at or
    /// below that row and the next above it, that row's own first, at most
    /// 20 matches that start a row, reading `rows` rows of text from each,
    /// over a bounded number of `FindText` calls.
    /// For a command mark above the rows Core keeps ("Features and
    /// settings"). A failing search in one direction finds nothing more
    /// there. Answered [`TextReply::TerminalFound`].
    TerminalFind {
        /// The text sought.
        needle: String,
        /// The row the search starts at, in rows above the start.
        from: i32,
        /// How many rows to read from each match.
        rows: u32,
        /// The start anchor the rows are counted from, which Core names.
        start: TextAnchor,
    },
    /// Read the terminal: a full read from a screen above the start Core
    /// names to the document's end, with the screen, the caret and its line
    /// (`docs/design/readers-tell-core-asks-build3.md`, "The full read
    /// request and its starts"). Answered [`TextReply::TerminalRead`].
    ReadTerminal(TerminalRead),
}

/// Core's request to read a terminal ([`TextOp::ReadTerminal`]): every
/// fact the read needs that Core owns travels in it and is used only
/// inside its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRead {
    /// The start anchor to read from, a screen above it; `None` for a
    /// first read, which takes the screen alone.
    pub start: Option<TextAnchor>,
    /// Which read: fresh, the whole document, or the history block with
    /// the height the first read found.
    pub scope: crate::TerminalScope,
    /// The screen's height in rows from Core's model, 0 when Core has
    /// none.
    pub height: u32,
    /// The screen's width in cells from Core's model, 0 when Core has
    /// none.
    pub width: u32,
    /// Whether the read takes the screen's colours.
    pub colours: bool,
    /// Whether the terminal's window is a console host's, Core's fact from
    /// the committed focus's answer.
    pub console: bool,
    /// When a caret key's entry sends it as its starting read, the key's
    /// unit: the answer then also mints the key's caret starting point.
    pub starting: Option<TextUnit>,
    /// Whether it is an output read, which Core asks outside the event
    /// queue and the worker makes after the event queue's requests; a
    /// baseline, a caret key's starting and final reads and a review
    /// command's fresh read are the event queue's.
    pub output: bool,
}

/// The answer to [`TextOp::ReadTerminal`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalReadReply {
    /// The read.
    pub snapshot: crate::TerminalSnapshot,
    /// The start anchor this read minted, at the top of its screen, which
    /// a later read, history read or search may name.
    pub start: TextAnchor,
    /// When the read began, in microseconds on the clock every process of
    /// a run shares: a change observed before it is covered by it.
    pub started_us: u64,
    /// The caret starting point the read minted, when the request named a
    /// unit ([`TerminalRead::starting`]).
    pub point: Option<TextAnchor>,
}

/// An outpost's answer to a text request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextReply {
    /// The answer to [`TextOp::ReadTerminal`].
    TerminalRead(Box<TerminalReadReply>),
    /// The answer to [`TextOp::TerminalHistory`]: the text read, padding
    /// kept, and how many rows it holds above the start the next read
    /// takes.
    TerminalHistory {
        /// The text.
        text: String,
        /// Rows above the start.
        above: u32,
    },
    /// The answer to [`TextOp::TerminalFind`].
    TerminalFound {
        /// The matches, alternately below and above the row sought from,
        /// nearest first in each direction.
        matches: Vec<crate::TerminalMatch>,
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
}

/// The answers an outpost can give any text request in place of its op's
/// own reply: the variants of [`TextReply`] that no op names as its own.
/// Core parses every text request's result into either the op's own reply
/// or one of these, so no answer a correct outpost gives fails to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFailure {
    /// [`TextReply::UnsupportedUnit`].
    UnsupportedUnit(TextUnit),
    /// [`TextReply::Unsupported`].
    Unsupported,
    /// [`TextReply::NoText`]: the node has no text, so no caret and no
    /// selection.
    NoText,
    /// [`TextReply::AnchorLost`].
    AnchorLost,
    /// [`TextReply::Gone`].
    Gone,
    /// [`TextReply::Unanswered`].
    Unanswered,
}

impl TextReply {
    /// The failure this reply is, or the reply itself when it is an op's
    /// own reply.
    ///
    /// # Errors
    ///
    /// The reply itself, when it is no failure.
    pub fn into_failure(self) -> Result<TextFailure, Self> {
        match self {
            TextReply::UnsupportedUnit(unit) => Ok(TextFailure::UnsupportedUnit(unit)),
            TextReply::Unsupported => Ok(TextFailure::Unsupported),
            TextReply::NoText => Ok(TextFailure::NoText),
            TextReply::AnchorLost => Ok(TextFailure::AnchorLost),
            TextReply::Gone => Ok(TextFailure::Gone),
            TextReply::Unanswered => Ok(TextFailure::Unanswered),
            own @ (TextReply::TerminalRead(_)
            | TextReply::TerminalHistory { .. }
            | TextReply::TerminalFound { .. }) => Err(own),
        }
    }
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
    /// Alt+Up Arrow: the previous sentence, where the application moves
    /// by sentence.
    PreviousSentence,
    /// Alt+Down Arrow: the next sentence, where the application moves by
    /// sentence.
    NextSentence,
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
    /// Enter in a text field other than a terminal: the application
    /// breaks the line, and the new line's text before the caret is spoken,
    /// as NVDA's new-line script speaks it (`docs/parity/text.md`, "Enter
    /// in a text field").
    NewLine,
    /// Any other key passed to the application that types no text (bash's
    /// Control+A and Control+E, Alt+B and Alt+F, Control+W): what it did is
    /// judged by its evidence, text removed at the caret or where the caret
    /// landed (`docs/parity/text.md`, "TextInfo-equivalent layer").
    Other,
}

impl CaretMotion {
    /// Whether the key deletes backward from the caret: Backspace, or with
    /// Control, the word before it.
    #[must_use]
    pub fn deletes_back(self) -> bool {
        matches!(self, Self::Backspace | Self::BackspaceWord)
    }

    /// Whether the key deletes text rather than moving the caret: Backspace
    /// and Delete, alone or with Control.
    #[must_use]
    pub const fn deletes(self) -> bool {
        matches!(
            self,
            Self::Backspace | Self::BackspaceWord | Self::Delete | Self::DeleteWord
        )
    }

    /// Whether a change of the unit at the caret is evidence of the key:
    /// Delete and Control+Delete, which change the text without moving the
    /// caret.
    #[must_use]
    pub const fn compares_unit(self) -> bool {
        matches!(self, Self::Delete | Self::DeleteWord)
    }

    /// The unit spoken after the motion: the character for character keys,
    /// Home, End, and Delete; the provider's word for word keys; the
    /// paragraph for paragraph keys; the line for line, sentence, page, and
    /// document keys, a sentence key's caret having been moved by the
    /// application.
    #[must_use]
    pub const fn unit(self) -> TextUnit {
        match self {
            Self::PreviousCharacter
            | Self::NextCharacter
            | Self::StartOfLine
            | Self::EndOfLine
            | Self::Backspace
            | Self::Delete => TextUnit::Character,
            Self::PreviousWord
            | Self::NextWord
            | Self::BackspaceWord
            | Self::DeleteWord
            | Self::Other => TextUnit::Word,
            Self::PreviousParagraph | Self::NextParagraph => TextUnit::Paragraph,
            Self::PreviousLine
            | Self::NextLine
            | Self::PreviousSentence
            | Self::NextSentence
            | Self::PreviousPage
            | Self::NextPage
            | Self::Top
            | Self::Bottom
            | Self::SelectAll
            | Self::NewLine => TextUnit::Line,
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
    fn the_judgement_counts_what_the_effect_test_names() {
        use crate::batch::EffectTest;
        let moves = EffectTest {
            unit: false,
            line: false,
        };
        let deletes_back = EffectTest {
            unit: false,
            line: true,
        };
        let deletes = EffectTest {
            unit: true,
            line: true,
        };
        let caret = Compared {
            caret_moved: true,
            ..Compared::default()
        };
        assert!(had_effect(caret, moves));
        let unit = Compared {
            unit_changed: true,
            ..Compared::default()
        };
        assert!(!had_effect(unit, moves));
        assert!(!had_effect(unit, deletes_back));
        assert!(had_effect(unit, deletes));
        let line = Compared {
            line_changed: true,
            ..Compared::default()
        };
        assert!(!had_effect(line, moves));
        assert!(had_effect(line, deletes_back));
    }

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
    fn a_read_of_text_round_trips_through_json() {
        let reply =
            crate::batch::StepOutcome::Done(crate::batch::Done::ReadText(crate::batch::Chunks {
                moved: -1,
                chunks: vec![TextChunk {
                    unit: TextUnit::Line,
                    text: "line\r\n".to_owned(),
                    start: TextAnchor(7),
                    offset: 0,
                    languages: Vec::new(),
                    first: false,
                    last: true,
                    truncated: false,
                    formats: Vec::new(),
                }],
            }));
        let json = serde_json::to_string(&reply).expect("serializes");
        let back: crate::batch::StepOutcome = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, reply);
    }

    /// A chunk's offsets are parsed once, as it decodes: one inside a
    /// character or past the text does not decode.
    #[test]
    fn a_chunk_off_its_character_boundaries_does_not_decode() {
        let chunk = |offset: u32, format: (u32, u32)| {
            serde_json::json!({
                "unit": "Line",
                "text": "a\u{4E2D}b",
                "start": 7,
                "offset": offset,
                "formats": [{
                    "start": format.0,
                    "end": format.1,
                    "attributes": {},
                }],
            })
        };
        let decoded = |value| serde_json::from_value::<TextChunk>(value).map(|chunk| chunk.offset);
        assert_eq!(decoded(chunk(4, (1, 5))).ok(), Some(4));
        assert_eq!(
            decoded(chunk(2, (0, 1))).map_err(|error| error.to_string()),
            Err("offset 2 is not a character boundary of a text of 5 bytes".to_owned())
        );
        assert!(decoded(chunk(6, (0, 1))).is_err());
        assert_eq!(
            decoded(chunk(0, (1, 3))).map_err(|error| error.to_string()),
            Err("span 1..3 is not on character boundaries of a text of 5 bytes".to_owned())
        );
        assert!(decoded(chunk(0, (4, 1))).is_err());
    }
}
