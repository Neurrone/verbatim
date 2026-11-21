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
// Non-test code never panics, overflows or truncates silently: arithmetic
// is checked or saturating, indexing goes through `get`, and casts through
// `From` or `TryFrom`. Tests are exempt, since a panic is how a test fails.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::exit,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::exit,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::string_slice,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::unwrap_in_result,
        clippy::unwrap_used
    )
)]

mod calls;
pub mod class;
mod event;
pub mod event_kind;
mod gesture;
pub mod hung;
mod key_seq;
mod limits;
pub mod property;
pub mod protocol;
pub mod request;
mod settings;
mod shell;
mod speech;
pub mod stroke;
pub mod tell;
mod terminal;
mod text;
mod theme;
mod tree;

pub use calls::{CallCounts, CallKind};
pub use event::{
    ActionName, Earcon, EarlierElement, Effect, FocusTextKind, GuiRequest, HookForeground, Input,
    LockKey, NormalizedEvent, NotificationKind, NotificationProcessing, Observed, Percent, Pid,
    PropertyChange, QueryKind, ReviewCommand, Script, ToastId, WindowFacts, WindowHandle,
};
pub use gesture::{GestureId, GestureParseError};
pub use key_seq::KeySeq;
pub use limits::{FOCUS_READ_DEADLINE, HANDLING_DEADLINE, UIA_TRANSACTION_TIMEOUT};
pub use settings::{
    DEFAULT_TERMINAL_LINES, KeyboardLayout, MAX_TERMINAL_LINES, ReaderSettings, SayAllUnit,
    TerminalLines, TypingEcho, VerbatimKeys,
};
pub use shell::{ShellCandidate, ShellCandidates, ShellItem, ShellItemKind};
pub use speech::{
    FocusNow, FocusValidity, Message, Phrase, SegmentContent, SelectionText, SpeechMark,
    SpeechPriority, TextFormat, Utterance, UtteranceEnding, UtteranceId, UtteranceSegment,
    UtteranceSource,
};
pub use terminal::{
    TerminalColourRun, TerminalMatch, TerminalScope, TerminalSnapshot, TerminalText,
};
pub use text::{
    BulletStyle, CaretKey, CaretMotion, CaretReport, CaretSelection, Compared, EditSelection,
    FocusText, FormatAttribute, FormatRun, FormatSet, HeldAnchors, LanguageRun, LineStyle,
    MAX_CHUNK_BYTES, MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT,
    MAX_SELECTION_TEXT_BYTES, Selection, SelectionChange, SelectionDifference, SelectionRead,
    SelectionReadReply, SentenceMove, SentenceMoved, Started, StartingRead, StartingUnit,
    TerminalRead, TerminalReadReply, TextAnchor, TextAttributes, TextChunk, TextDetails,
    TextFailure, TextMovement, TextOp, TextPoint, TextPosition, TextProvider, TextRead,
    TextReadAhead, TextReply, TextThrough, TextUnit, Watch, WatchFinalRead, Watched, WatchedRead,
    had_effect,
};
pub use theme::{
    DEFAULT_GAIN, Indication, IndicationCategory, IndicationSetting, MAX_GAIN, Presentation,
    ReportedDetails, SoundSource, SoundVolume, Theme, ThemeOptions, ThemeProblem, Tone, VoiceStyle,
    is_plain_file_name, sound_gain,
};
pub use tree::{
    Backend, CellPosition, NodeDetails, NodeSnapshot, ObjectKey, Rect, Role, State, StateSet,
    TableCounts, TableId, TreeNode, WindowIdentity,
};

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

    /// The trace ID `raw`, given rather than minted: for tests and samples,
    /// whose IDs must not depend on what else the process minted first.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
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
/// unknown, a race described in `docs/roadmap-done.md`'s M3 section.
/// `verbatim-gui` sets this property (`SetPropW`) on the frame at creation
/// and clears it (`RemovePropW`) at shutdown; every outpost checks it
/// (`GetPropW`), and that the window belongs to Core's process, before
/// emitting any `FocusChanged` — the MSAA event path, the UIA focus
/// callback, and the synthetic focus query alike. The owner check keeps
/// another application from hiding its own windows by setting the property.
pub const HIDDEN_FRAME_WINDOW_PROP: &str = "VerbatimHiddenFrame";

/// The title of `verbatim-gui`'s hidden main frame, the window a new
/// instance finds to replace a running one. Not localized, so a Verbatim
/// in any language finds one in any other.
pub const HIDDEN_FRAME_TITLE: &str = "Verbatim";

/// The log target of a message that states a defect of Verbatim's own, in
/// any of its processes: an end-to-end scenario whose logs hold a line of
/// it fails (`books/dev/src/reference/testing.md`, "Failures are never
/// hidden"). A warning about an application that misbehaves, or a call
/// the system refused, is not logged to it.
pub const DEFECT_TARGET: &str = "verbatim::defect";

/// The log target of a remote operation an outpost fell back from to the
/// classic calls, or of a window it marked for classic reading: an
/// end-to-end scenario whose logs hold a line of it fails, unless the
/// scenario expects one (`books/dev/src/how-to/e2e.md`, "Remote-operation
/// fallbacks fail a scenario"), because such fallbacks went unseen behind
/// speech that came out right.
pub const REMOTE_FALLBACK_TARGET: &str = "verbatim::remote_fallback";

/// Names one outpost process generation.
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
    /// the real generation on them. Never assigned to a running outpost.
    pub const UNASSIGNED: OutpostId = OutpostId(0);
}

impl fmt::Display for OutpostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Who issued a node id: an outpost generation, or Core's plain reader,
/// which answers for a plain window (NVDA's `NVDAObjects.window.Window`) a
/// read NVDA makes itself found in front, with no call into any
/// application. A plain node id can never compare equal to an outpost's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Issuer {
    /// An outpost generation, [`OutpostId::UNASSIGNED`] in an id as its
    /// outpost mints it.
    Outpost(OutpostId),
    /// Core's plain reader, by the generation of the plain object it holds.
    Plain(u64),
}

/// Identity of one node: who issued it, an outpost generation or Core's
/// plain reader, plus a number unique within that issuer.
///
/// Backend runtime identifiers (UIA runtime IDs, MSAA object and child IDs)
/// are mapped to numbers by the owning outpost; the reducer and everything
/// above it never see backend identifiers. Node ids are comparable only within
/// one issuer: the same window seen by two outposts has two different ids.
///
/// In a message an outpost's id is `{"outpost": n, "number": m}` and a
/// plain one `{"plain": n, "number": m}`, written and read by hand, with no
/// buffering of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    issuer: Issuer,
    number: u64,
}

impl Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut node = serializer.serialize_struct("NodeId", 2)?;
        match self.issuer {
            Issuer::Outpost(outpost) => node.serialize_field("outpost", &outpost)?,
            Issuer::Plain(generation) => node.serialize_field("plain", &generation)?,
        }
        node.serialize_field("number", &self.number)?;
        node.end()
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The map of a node id, its issuer's key and `number`.
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = NodeId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a node id: \"outpost\" or \"plain\", and \"number\"")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<NodeId, A::Error> {
                use serde::de::Error;
                let mut issuer = None;
                let mut number = None;
                while let Some(key) = map.next_key::<NodeIdKey>()? {
                    match key {
                        NodeIdKey::Outpost | NodeIdKey::Plain if issuer.is_some() => {
                            return Err(A::Error::custom("a node id names one issuer"));
                        }
                        NodeIdKey::Outpost => {
                            issuer = Some(Issuer::Outpost(map.next_value()?));
                        }
                        NodeIdKey::Plain => issuer = Some(Issuer::Plain(map.next_value()?)),
                        NodeIdKey::Number if number.is_some() => {
                            return Err(A::Error::duplicate_field("number"));
                        }
                        NodeIdKey::Number => number = Some(map.next_value()?),
                    }
                }
                Ok(NodeId {
                    issuer: issuer.ok_or_else(|| A::Error::missing_field("outpost"))?,
                    number: number.ok_or_else(|| A::Error::missing_field("number"))?,
                })
            }
        }

        deserializer.deserialize_struct("NodeId", &["outpost", "plain", "number"], Visitor)
    }
}

/// A key of a node id's map.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum NodeIdKey {
    Outpost,
    Plain,
    Number,
}

impl NodeId {
    /// A node id as an outpost mints it: `number` with the outpost left
    /// [`OutpostId::UNASSIGNED`] for Core to stamp.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self {
            issuer: Issuer::Outpost(OutpostId::UNASSIGNED),
            number,
        }
    }

    /// A node id issued by `outpost`.
    #[must_use]
    pub const fn in_outpost(outpost: OutpostId, number: u64) -> Self {
        Self {
            issuer: Issuer::Outpost(outpost),
            number,
        }
    }

    /// A node id Core's plain reader issued for its plain object of
    /// `generation`.
    #[must_use]
    pub const fn plain(generation: u64, number: u64) -> Self {
        Self {
            issuer: Issuer::Plain(generation),
            number,
        }
    }

    /// Who issued this id.
    #[must_use]
    pub const fn issuer(self) -> Issuer {
        self.issuer
    }

    /// The outpost generation that issued this id, `None` for a plain
    /// node.
    #[must_use]
    pub const fn outpost(self) -> Option<OutpostId> {
        match self.issuer {
            Issuer::Outpost(outpost) => Some(outpost),
            Issuer::Plain(_) => None,
        }
    }

    /// The number the issuer gave the node.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.number
    }

    /// This id stamped with `outpost`, the stamp Core applies to everything
    /// arriving on that outpost's pipe: an outpost's id takes the
    /// generation, and a plain id is kept as it is, so no stamp can make a
    /// plain node an outpost's, whatever an outpost echoed back.
    #[must_use]
    pub const fn with_outpost(self, outpost: OutpostId) -> Self {
        match self.issuer {
            Issuer::Outpost(_) => Self {
                issuer: Issuer::Outpost(outpost),
                number: self.number,
            },
            Issuer::Plain(_) => self,
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
    #[expect(
        clippy::disallowed_methods,
        reason = "the mint's own test: listed under the determinism rule's exceptions"
    )]
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

    /// A plain node never compares equal to an outpost's node of the same
    /// numbers, and each issuer's id keeps its message form.
    #[test]
    fn a_plain_node_is_never_an_outposts_node() {
        let plain = NodeId::plain(2, 7);
        let outpost = NodeId::in_outpost(OutpostId(2), 7);
        assert_ne!(plain, outpost);
        assert_eq!(plain.outpost(), None);
        assert_eq!(outpost.outpost(), Some(OutpostId(2)));
        assert_eq!(
            serde_json::to_string(&outpost).unwrap(),
            r#"{"outpost":2,"number":7}"#
        );
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"plain":2,"number":7}"#
        );
        assert_eq!(
            serde_json::from_str::<NodeId>(r#"{"plain":2,"number":7}"#).unwrap(),
            plain
        );
        assert_eq!(
            serde_json::from_str::<NodeId>(r#"{"outpost":2,"number":7}"#).unwrap(),
            outpost
        );
        assert_eq!(
            serde_json::from_str::<NodeId>(r#"{"number":7,"plain":2}"#).unwrap(),
            plain
        );
        assert!(serde_json::from_str::<NodeId>(r#"{"number":7}"#).is_err());
        assert!(serde_json::from_str::<NodeId>(r#"{"outpost":2,"plain":2,"number":7}"#).is_err());
    }

    /// The stamp on an outpost's pipe never makes a plain id an outpost's.
    #[test]
    fn the_outposts_stamp_keeps_a_plain_id_plain() {
        let plain = NodeId::plain(2, 7);
        assert_eq!(plain.with_outpost(OutpostId(9)), plain);
        assert_eq!(
            NodeId::new(7).with_outpost(OutpostId(9)),
            NodeId::in_outpost(OutpostId(9), 7)
        );
    }
}
