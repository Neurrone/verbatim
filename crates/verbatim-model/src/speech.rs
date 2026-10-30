//! Structured utterances.
//!
//! The reducer composes announcements from tokens, not display text, so it
//! stays free of localization; the speech pipeline renders tokens to
//! localized words at its boundary (via `verbatim-i18n`) just before
//! dictionary and symbol processing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::{BulletStyle, LineStyle};
use crate::tree::{ObjectKey, Rect, Role, State};
use crate::{NodeId, TraceId, WindowHandle};

/// Identifies one utterance from the moment the speech pipeline accepts it
/// until its single ending (decision D17). Unlike a [`TraceId`], which names
/// the event behind speech and can be shared by several utterances, an
/// utterance id is never reused within a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UtteranceId(pub u64);

impl fmt::Display for UtteranceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "u{}", self.0)
    }
}

/// How an utterance ended (decision D17). Every utterance the speech
/// pipeline accepts ends exactly once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UtteranceEnding {
    /// The audio device played all of the utterance's audio. An utterance
    /// that produced no audio completes when the audio before it has played.
    Completed,
    /// The utterance was cut off or dropped before all of it was heard: by
    /// speech that interrupts, a synthesizer switch, or shutdown.
    Cancelled,
    /// Synthesis or audio output failed; the text says why.
    Failed(String),
}

/// Priority lane for an utterance (architecture section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeechPriority {
    /// Cancel current and queued speech, then speak.
    Interrupt,
    /// Speak after the current utterance, ahead of the queue.
    Next,
    /// Append to the queue.
    Queued,
}

/// The content of one utterance segment: a semantic span, per decision D12.
///
/// Spans stay typed all the way to the presentation stage at the end of the
/// speech pipeline, where a theme flattens them — to plain words in the
/// default theme, or (milestone M11) to earcons and voice changes keyed by
/// exactly these span kinds. The reducer never pre-flattens: a control's
/// label travels as [`Label`](Self::Label), never as anonymous text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SegmentContent {
    /// Literal text with no more specific meaning: typed characters, a
    /// spoken time, free-form message text.
    Text(String),
    /// A control's accessible name or label.
    Label(String),
    /// A control's current value — slider position, combo selection.
    Value(String),
    /// A control's accessible description, when it adds information beyond
    /// the label.
    Description(String),
    /// The keyboard shortcut a control advertises (an access key or
    /// accelerator), spoken after the description in NVDA's property order.
    Shortcut(String),
    /// A role, rendered to its localized spoken name.
    Role(Role),
    /// A state, rendered to its localized spoken name.
    State(State),
    /// The absence of a state that is worth announcing, rendered to its
    /// localized negative form — "not checked" for an unchecked check box.
    NegatedState(State),
    /// Position within a set — "2 of 5" — from the node's reported
    /// position-in-set and set-size details.
    Position {
        /// One-based position within the set.
        position: u32,
        /// Set size, when reported; a position can arrive without one.
        set_size: Option<u32>,
    },
    /// One-based nesting level (tree items, headings).
    Level(u32),
    /// The one-based row of a table cell, "row 3", from the node's
    /// reported row.
    Row(u32),
    /// The one-based column of a table cell, "column 2", from the node's
    /// reported column.
    Column(u32),
    /// A fixed reader message, rendered to its localized wording — the
    /// D12-conformant way for the reducer to say something that is not a
    /// property of any node (a navigation edge, for instance) without
    /// pre-flattening text.
    Message(Message),
    /// An uppercase character spoken while spelling, or while reading a
    /// single character: a theme speaks it at a raised pitch, as NVDA does
    /// (`docs/nvda/speech.md`, "Capitals when spelling").
    SpelledCapital(String),
    /// One character spoken on its own, as caret and review movement by
    /// character and spelling speak it: by its name from the character
    /// table of the segment's language when it has one ("comma",
    /// "space"), raised in pitch when it is a capital letter, and as itself
    /// otherwise.
    Character(String),
    /// One character's description from the character table of the
    /// segment's language ("Alpha" for a), spoken when the current
    /// character is asked for twice or a word is spelled with
    /// descriptions; a character with no description is spoken as
    /// [`Character`](Self::Character) is.
    CharacterDescription(String),
    /// A point in the utterance to report when playback reaches it, which
    /// say-all moves its position by (`docs/nvda/speech.md`, "Say-all").
    /// Says nothing.
    Mark(SpeechMark),
    /// A reader message with values in it, rendered to its localized
    /// wording ("selected hello").
    Phrase(Phrase),
    /// A change of formatting at this point of text being read: a spelling
    /// or grammar error starting or ending, or a font or color (milestone
    /// M4). How it is reported, if at all, is the theme's indication for it
    /// (`crate::Indication`).
    Format(TextFormat),
}

/// A formatting fact at a point in text, named by
/// [`SegmentContent::Format`]. Formatting is reported as it changes, as
/// `docs/nvda/document-formatting.md` describes: "spelling error" where an
/// error starts and "out of spelling error" where it ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TextFormat {
    /// A spelling error starts here.
    SpellingError,
    /// A spelling error ends here.
    NotSpellingError,
    /// A grammar error starts here.
    GrammarError,
    /// A grammar error ends here.
    NotGrammarError,
    /// The text from here is in this font.
    FontName(String),
    /// The text from here is this size, as the application words it.
    FontSize(String),
    /// The text from here is this color, as the application words it.
    Color(String),
    /// Bold text starts here.
    Bold,
    /// Bold text ends here.
    NotBold,
    /// Italic text starts here.
    Italic,
    /// Italic text ends here.
    NotItalic,
    /// Underlined text starts here.
    Underline,
    /// Underlined text ends here.
    NotUnderline,
    /// The text from here has this background color, as the application
    /// words it: "light grey background".
    BackgroundColor(String),
    /// The text from here has this background color, following a change of
    /// its color in the same place: "on light grey", completing "dark red".
    OnBackgroundColor(String),
    /// The text from here is underlined this way, or, with
    /// [`LineStyle::None`], no longer underlined.
    UnderlineStyle(LineStyle),
    /// The text from here is struck through this way, or, with
    /// [`LineStyle::None`], no longer struck through.
    Strikethrough(LineStyle),
    /// The line is a list item with this bullet.
    Bullet(BulletStyle),
    /// A link starts here.
    Link,
    /// A link ends here.
    NotLink,
}

/// An index mark the reducer places in an utterance
/// ([`SegmentContent::Mark`]); the speech pipeline reports it back when
/// playback reaches it, as `Input::MarkReached`. The reducer numbers its
/// marks in increasing order and never reuses a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SpeechMark(pub u64);

/// Text named in a selection announcement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionText {
    /// Text, spoken as it is.
    Text(String),
    /// A single character, spoken by its name as
    /// [`SegmentContent::Character`] is.
    Character(String),
    /// Too much text to speak (512 characters or more, as NVDA counts):
    /// spoken as the number of characters.
    Characters(u32),
}

/// A reader message with values in it, named by
/// [`SegmentContent::Phrase`] and worded at the presentation stage, like
/// [`Message`]. Wording matches NVDA's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Phrase {
    /// Text became selected: NVDA's "hello selected".
    Selected(SelectionText),
    /// Text stopped being selected: NVDA's "hello unselected".
    Unselected(SelectionText),
    /// The selection that replaced one the text changed under: NVDA's
    /// "hello selected instead".
    SelectedInstead(SelectionText),
    /// Text an object already had selected when it was announced: NVDA's
    /// "selected hello", the word first as a warning that typing would
    /// replace it.
    Preselected(SelectionText),
    /// A text position's place on the screen: NVDA's "Positioned at 10,
    /// 20".
    Positioned {
        /// Screen x, in pixels.
        x: i32,
        /// Screen y, in pixels.
        y: i32,
    },
    /// The "Speak typed characters" setting's new value, after its toggle.
    SpeakTypedCharacters(crate::TypingEcho),
    /// The "Speak typed words" setting's new value, after its toggle.
    SpeakTypedWords(crate::TypingEcho),
    /// A terminal's output was too much to read, and this many lines of it
    /// were skipped: "skipped 120 lines". A theme can also mark it with a
    /// sound (`crate::Indication::SkippedLines`).
    SkippedLines(u32),
    /// Terminal output was skipped, but how many lines is not known (the
    /// scrollback overflowed past what the reader could track): "skipped
    /// lines". Marked like [`Phrase::SkippedLines`].
    SkippedUncountedLines,
    /// Terminal output was skipped past what the terminal's history keeps,
    /// so how many lines is known only to be more than this many: "skipped
    /// more than 9001 lines". Marked like [`Phrase::SkippedLines`].
    SkippedMoreThanLines(u32),
    /// Terminal output was skipped after a read that lost track of it, its
    /// count taken from text, so it may be off by a little: "skipped about
    /// 1941 lines". Marked like [`Phrase::SkippedLines`].
    SkippedAboutLines(u32),
    /// The number of items a tree view item holds, said once it has been
    /// expanded: NVDA's "1 item" and "52 items".
    Items(u32),
    /// A lock key's new state: NVDA's "caps lock on".
    LockKeyState {
        /// Which key.
        key: crate::LockKey,
        /// Whether it is now on.
        on: bool,
    },
    /// Text copied to the clipboard: NVDA's "Copied to clipboard: hello",
    /// or the number of characters for 1,024 or more.
    CopiedToClipboard(String),
    /// The shell item list dialog's chosen item was gone: "Clock is no
    /// longer there".
    ShellItemGone(String),
    /// The shell item list dialog's chosen item was there with no point to
    /// click: "Clock cannot be clicked".
    ShellItemUnclickable(String),
}

/// A fixed reader message a [`SegmentContent::Message`] segment names.
/// Localization happens at the presentation stage, like every other span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Message {
    /// Object navigation found no next sibling — NVDA's "No next".
    NoNextObject,
    /// Object navigation found no previous sibling — NVDA's "No previous".
    NoPreviousObject,
    /// Object navigation found no parent — NVDA's "No containing object".
    NoContainingObject,
    /// Object navigation found no children — NVDA's "No objects inside".
    NoObjectsInside,
    /// The review cursor is on the first line or word — NVDA's "Top".
    Top,
    /// The review cursor is on the last line or word — NVDA's "Bottom".
    Bottom,
    /// The review cursor is on the first character of the line — "Left".
    Left,
    /// The review cursor is on the last character of the line — "Right".
    Right,
    /// The unit under the review cursor is empty — NVDA's "blank".
    Blank,
    /// The navigator returns to the focus — NVDA's "Move to focus".
    MoveToFocus,
    /// A navigator command with no navigator — "No navigator object".
    NoNavigatorObject,
    /// Reporting the focus with nothing focused — NVDA's "No focus".
    NoFocus,
    /// The navigator object was activated — NVDA's "Activate".
    Activate,
    /// Nothing could be activated — NVDA's "No action".
    NoAction,
    /// UIA's Invoke pattern was performed — NVDA's "invoke".
    Invoke,
    /// A space, spelled — NVDA's symbol name "space".
    Space,
    /// The select-then-copy start marker was set — NVDA's "Start marked".
    StartMarked,
    /// Select then copy with no start marker, or with the start marker in
    /// another object — NVDA's "No start marker set".
    NoStartMarker,
    /// Select then copy pressed once again after a first press whose range
    /// is not yet copied — NVDA's "Press twice to copy or reset the start
    /// marker".
    PressTwiceToCopy,
    /// The review cursor now follows the caret — NVDA's "caret moves review
    /// cursor".
    CaretMovesReview,
    /// The review cursor no longer follows the caret — NVDA's "caret
    /// doesn't move review cursor".
    CaretDoesNotMoveReview,
    /// The text cannot do what was asked (no page unit, no selection, no
    /// screen position) — NVDA's "Not supported in this document".
    NotSupported,
    /// Select then copy's first press could not select the text, which a
    /// second press still copies — NVDA's "Can't select text, press twice
    /// to copy".
    CannotSelect,
    /// A command that needs a caret found none — NVDA's "No caret".
    NoCaret,
    /// New terminal output is spoken again, after Verbatim+5 — "report new
    /// output on" (NVDA's toggle on the same key says "report dynamic
    /// content changes on").
    ReportNewOutputOn,
    /// New terminal output is no longer spoken, after Verbatim+5 — "report
    /// new output off".
    ReportNewOutputOff,
    /// A terminal line longer than all the output Core keeps waiting was
    /// cut, and the rest of it is not spoken — "line cut".
    TerminalLineCut,
    /// The selection went when the text changed under it — NVDA's
    /// "selection removed".
    SelectionRemoved,
    /// A copy to the clipboard failed — NVDA's "Unable to copy".
    UnableToCopy,
    /// "Report status bar" in a terminal whose screen shows no text — "no
    /// status bar information".
    NoStatusBarInformation,
    /// "Report status bar" outside a terminal Core reads — NVDA's "No
    /// status line found". Provisional until the general status bar of
    /// milestone M5.
    NoStatusLineFound,
}

/// One segment of an utterance, with an optional language override.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtteranceSegment {
    /// What to speak.
    pub content: SegmentContent,
    /// BCP 47 language tag for this segment, when it differs from the
    /// utterance default; utterances are multilingual end-to-end.
    pub language: Option<String>,
}

impl UtteranceSegment {
    /// A segment in the utterance's default language.
    #[must_use]
    pub fn new(content: SegmentContent) -> Self {
        Self {
            content,
            language: None,
        }
    }

    /// A literal-text segment in the utterance's default language.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Text(text.into()))
    }

    /// A label segment in the utterance's default language.
    #[must_use]
    pub fn label(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Label(text.into()))
    }

    /// A value segment in the utterance's default language.
    #[must_use]
    pub fn value(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Value(text.into()))
    }
}

/// The node an utterance describes, carried alongside its segments.
///
/// This exists for presentation themes (decision D12): an earcon theme keys
/// sounds off the source node's role, and a positional-audio theme (milestone
/// M11, in the audio-themes add-on tradition) pans them by its screen
/// rectangle. Core-originated speech with no source node — the startup
/// announcement, the spoken time — carries none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtteranceSource {
    /// The described node's role.
    pub role: Role,
    /// Its bounding rectangle in screen coordinates, when reported.
    pub rect: Option<Rect>,
}

/// A structured utterance flowing from the reducer into the speech pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utterance {
    /// The trace this speech belongs to, for end-to-end latency timelines.
    pub trace_id: TraceId,
    /// Priority lane.
    pub priority: SpeechPriority,
    /// Segments, spoken in order.
    pub segments: Vec<UtteranceSegment>,
    /// The node this utterance describes, when there is one, for
    /// presentation themes. `#[serde(default)]` keeps utterances recorded
    /// before this field existed deserializing unchanged.
    #[serde(default)]
    pub source: Option<UtteranceSource>,
    /// For focus speech: the node it announces, so the speech manager can
    /// drop it once that node is no longer relevant to the focus
    /// ([`FocusValidity`]). `None` for every other utterance, which only a
    /// cancel ends early.
    #[serde(default)]
    pub validity: Option<FocusValidity>,
    /// Whether say-all is reading this utterance, so the theme's "play
    /// sounds during say all" setting decides whether its indications play
    /// their sounds or are spoken instead (`docs/crates/verbatim-config.md`,
    /// "Themes"). `false` for every other utterance.
    #[serde(default)]
    pub say_all: bool,
}

/// What focus speech is about, for dropping it once the focus has moved on
/// (`docs/nvda/speech.md`, "Cancellation", and `docs/nvda/events.md`):
/// queued or playing speech announcing a node stays valid while that node
/// is the same object as the focus, an ancestor of the focus, or the
/// foreground window ([`ObjectKey::is_same_object`]), if the node never
/// had the focus at all, as a dialog announced on entering it never does,
/// or if it is the menu item the focus just left for a menu above it
/// ([`FocusNow::menu_item_of_focus`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusValidity {
    /// The node the speech announces.
    pub node: NodeId,
    /// Whether that node was the focus when the speech was made.
    pub had_focus: bool,
    /// The window that node is, when it is one, so that another outpost's
    /// node for the same window is the same object.
    #[serde(default)]
    pub window: Option<WindowHandle>,
}

impl FocusValidity {
    /// Whether speech with this validity is still worth hearing, given
    /// where the focus now is.
    #[must_use]
    pub fn holds(&self, now: &FocusNow) -> bool {
        let key = ObjectKey {
            id: self.node,
            window: self.window,
        };
        !self.had_focus
            || key.is_same_object(&now.focus)
            || now
                .ancestors
                .iter()
                .any(|ancestor| key.is_same_object(ancestor))
            || now
                .foreground
                .is_some_and(|foreground| key.is_same_object(&foreground))
            || now.menu_item_of_focus == Some(self.node)
    }
}

/// Where the focus is, for judging [`FocusValidity`], each object by its
/// [`ObjectKey`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusNow {
    /// The focus.
    pub focus: ObjectKey,
    /// The focus's ancestors.
    pub ancestors: Vec<ObjectKey>,
    /// The foreground object, when known.
    pub foreground: Option<ObjectKey>,
    /// The MSAA menu item the focus has just moved from to an MSAA popup
    /// menu that contains it, as some applications focus a submenu's first
    /// item and then the submenu: the item's speech is still heard
    /// (`docs/nvda/speech.md`, "Expired focus speech").
    #[serde(default)]
    pub menu_item_of_focus: Option<NodeId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window `handle`'s node `id`, or a windowless node's for `None`.
    fn key(id: u64, handle: Option<u64>) -> ObjectKey {
        ObjectKey {
            id: NodeId::new(id),
            window: handle.map(WindowHandle),
        }
    }

    #[test]
    fn a_window_node_is_the_same_object_as_another_node_of_its_window() {
        assert!(key(1, Some(7)).is_same_object(&key(2, Some(7))));
        assert!(!key(1, Some(7)).is_same_object(&key(2, Some(8))));
    }

    #[test]
    fn windowless_nodes_are_the_same_object_only_by_id() {
        assert!(key(1, None).is_same_object(&key(1, Some(7))));
        assert!(!key(1, None).is_same_object(&key(2, None)));
        assert!(!key(1, Some(7)).is_same_object(&key(2, None)));
    }

    /// A window's focus speech, reported by one outpost, holds while the
    /// focus is another outpost's node for the same window.
    #[test]
    fn focus_speech_holds_while_the_focus_is_its_object_under_another_node() {
        let spoken = FocusValidity {
            node: NodeId::new(1),
            had_focus: true,
            window: Some(WindowHandle(7)),
        };
        let now = |focus| FocusNow {
            focus,
            ancestors: Vec::new(),
            foreground: None,
            menu_item_of_focus: None,
        };
        assert!(spoken.holds(&now(key(2, Some(7)))));
        assert!(!spoken.holds(&now(key(2, Some(8)))));
    }

    /// A window's focus speech holds while the foreground is that window as
    /// another outpost reported it.
    #[test]
    fn focus_speech_holds_while_the_foreground_is_its_window_from_another_outpost() {
        let spoken = FocusValidity {
            node: NodeId::new(1),
            had_focus: true,
            window: Some(WindowHandle(7)),
        };
        let now = |foreground| FocusNow {
            focus: key(3, None),
            ancestors: vec![key(4, None)],
            foreground: Some(foreground),
            menu_item_of_focus: None,
        };
        assert!(spoken.holds(&now(key(2, Some(7)))));
        assert!(!spoken.holds(&now(key(2, None))));
    }

    #[test]
    fn text_segment_defaults_to_utterance_language() {
        let segment = UtteranceSegment::text("Settings");
        assert_eq!(segment.content, SegmentContent::Text("Settings".into()));
        assert!(segment.language.is_none());
    }
}
