//! Normalized accessibility model (architecture section 3).
//!
//! Verbatim's own vocabulary — roles, states, properties, node identity,
//! events, reducer inputs and effects, structured utterances, and gesture
//! identifiers — defined as a superset that every backend (UIA, MSAA/IA2,
//! later JAB) maps into. The reducer, browse mode, extensions, and all tests
//! speak this model only. This crate has no I/O dependencies by design;
//! serde derives exist so the same types travel over the Core-outpost pipe,
//! the control plane, and the flight recorder unchanged.

#![forbid(unsafe_code)]

mod calls;
mod event;
mod gesture;
mod settings;
mod speech;
mod terminal;
mod text;
mod theme;
mod tree;

pub use calls::{CallCounts, CallKind};
pub use event::{
    ActionName, Earcon, Effect, FetchResult, Input, NormalizedEvent, Notification,
    NotificationKind, NotificationProcessing, Pid, PropertyChange, Query, QueryId, QueryKind,
    ReviewCommand, WindowFacts, WindowHandle,
};
pub use gesture::{GestureId, GestureParseError};
pub use settings::{
    DEFAULT_TERMINAL_LINES, MAX_TERMINAL_LINES, ReaderSettings, SayAllUnit, TypingEcho,
};
pub use speech::{
    FocusNow, FocusValidity, Message, Phrase, SegmentContent, SelectionText, SpeechMark,
    SpeechPriority, TextFormat, Utterance, UtteranceEnding, UtteranceId, UtteranceSegment,
    UtteranceSource,
};
pub use terminal::{LineChange, Skipped, TerminalOutput};
pub use text::{
    BulletStyle, CaretKey, CaretMotion, CaretReply, CaretReport, CaretWatch, FormatRun,
    HeldAnchors, LanguageRun, LineStyle, MAX_CHUNK_BYTES, MAX_RANGE_BYTES, MAX_READ_AHEAD,
    MAX_READ_AHEAD_TEXT, MAX_SELECTION_TEXT_BYTES, PreviousSelection, Selection, SelectionChange,
    TextAnchor, TextAttributes, TextChunk, TextMovement, TextOp, TextPoint, TextPosition, TextRead,
    TextReadAhead, TextReply, TextRequest, TextUnit,
};
pub use theme::{
    DEFAULT_GAIN, Fetches, Indication, IndicationCategory, IndicationSetting, MAX_GAIN,
    Presentation, SoundSource, Theme, ThemeOptions, ThemeProblem, Tone, VoiceStyle,
    is_plain_file_name, progress_frequency,
};
pub use tree::{Backend, NodeDetails, NodeSnapshot, Rect, Role, State, StateSet, TreeNode};

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Correlates one observed OS event or keypress with everything it causes.
///
/// A `TraceId` is minted the moment an OS event or keypress is first
/// observed and is carried through the outpost, the reducer, the speech
/// queue, the synth, and audio submission, so the full timeline of any
/// utterance — event observed, speech queued, audio started — is a single
/// query (architecture section 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TraceId(u64);

/// The mint counter behind [`TraceId::mint`].
static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

impl TraceId {
    /// Mints a trace ID that is unique within this process and strictly
    /// greater than every ID minted before it.
    #[must_use]
    pub fn mint() -> Self {
        Self(NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Namespaces this process's trace IDs by seeding the mint counter with
    /// the process id in the high 32 bits. Verbatim runs as several
    /// processes (Core and one outpost per application), each minting trace
    /// IDs independently; their IDs meet in Core's latency ledger and the
    /// flight recorder, so every process calls this once at startup, before
    /// any ID is minted, to keep the ID spaces disjoint.
    pub fn namespace(pid: u32) {
        NEXT_TRACE_ID.store((u64::from(pid) << 32) | 1, Ordering::Relaxed);
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Name of the window property `verbatim-gui` stamps on Core's hidden 1x1
/// main frame (decision D9) so every outpost can recognize and suppress it.
///
/// The frame transits real focus during the prePopup show/raise/foreground
/// dance around the Verbatim menu and the settings dialog; without this
/// marker it can be announced as a nameless "Verbatim" window with role
/// unknown, a race described in `docs/roadmap.md`'s M3 section.
/// `verbatim-gui` sets this property (`SetPropW`) on the frame at creation
/// and clears it (`RemovePropW`) at shutdown; every outpost checks it
/// (`GetPropW`), and that the window belongs to Core's process, before
/// emitting any `FocusChanged` — the MSAA event path, the UIA focus
/// callback, and the synthetic focus query alike. The owner check keeps
/// another application from hiding its own windows by setting the property.
pub const HIDDEN_FRAME_WINDOW_PROP: &str = "VerbatimHiddenFrame";

/// Names one outpost process incarnation.
///
/// The supervisor assigns a fresh one at every spawn and never reuses one, so
/// a node id from a replaced outpost can never name a node in its successor.
/// Core attaches it to everything an outpost sends according to the pipe the
/// message arrived on, never from the message body; an outpost itself only
/// ever mints ids with [`OutpostId::UNASSIGNED`].
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct OutpostId(pub u64);

impl OutpostId {
    /// The placeholder an outpost mints its node ids with, before Core stamps
    /// the real incarnation on them. Never assigned to a running outpost.
    pub const UNASSIGNED: OutpostId = OutpostId(0);
}

impl fmt::Display for OutpostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identity of one node: the outpost incarnation that issued it plus a number
/// unique within that incarnation.
///
/// Backend runtime identifiers (UIA runtime IDs, MSAA object and child IDs)
/// are mapped to numbers by the owning outpost; the reducer and everything
/// above it never see backend identifiers. Node ids are comparable only within
/// one outpost: the same window seen by two outposts has two different ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId {
    outpost: OutpostId,
    number: u64,
}

impl NodeId {
    /// A node id as an outpost mints it: `number` with the outpost left
    /// [`OutpostId::UNASSIGNED`] for Core to stamp.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self {
            outpost: OutpostId::UNASSIGNED,
            number,
        }
    }

    /// A node id issued by `outpost`.
    #[must_use]
    pub const fn in_outpost(outpost: OutpostId, number: u64) -> Self {
        Self { outpost, number }
    }

    /// The outpost incarnation that issued this id.
    #[must_use]
    pub const fn outpost(self) -> OutpostId {
        self.outpost
    }

    /// The number the issuing outpost gave the node.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.number
    }

    /// This id with its outpost replaced by `outpost`, the stamp Core applies
    /// to everything arriving on that outpost's pipe.
    #[must_use]
    pub const fn with_outpost(self, outpost: OutpostId) -> Self {
        Self {
            outpost,
            number: self.number,
        }
    }

    /// This id as its issuing outpost knows it, with the outpost part
    /// cleared, for looking the node up inside that outpost.
    #[must_use]
    pub const fn unstamped(self) -> Self {
        Self::new(self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ids_are_unique_and_increasing() {
        let first = TraceId::mint();
        let second = TraceId::mint();
        assert!(second > first);
    }

    #[test]
    fn node_ids_compare_by_value() {
        assert_eq!(NodeId::new(7), NodeId::new(7));
        assert_ne!(NodeId::new(7), NodeId::new(8));
    }

    #[test]
    fn the_same_number_from_two_outposts_names_two_nodes() {
        let old = NodeId::in_outpost(OutpostId(1), 7);
        let new = NodeId::in_outpost(OutpostId(2), 7);
        assert_ne!(old, new);
        assert_eq!(old.unstamped(), new.unstamped());
        assert_eq!(NodeId::new(7).with_outpost(OutpostId(2)), new);
    }
}
