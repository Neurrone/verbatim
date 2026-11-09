//! Text support per node, as the reducer remembers it
//! (`books/dev/src/explanation/design/ownership.md`): each node's anchors with their chunks' text, the positions the
//! outpost reported, and the node's one last read, the only baseline a
//! caret key or a comparison of selections has
//! (`docs/design/caret-selection-rework.md`), and for a short time after a
//! key's request a note of which unit its reads fetch. A position is a fact of its own kind: an edit control's
//! offset, or the key of a UIA range the outpost's store keeps, so no
//! application object is held here and an edit control's offsets cross
//! between programs as they are.
//!
//! A program reads these in its copies ([`super::Facts`]); its text calls
//! change them only through [`TextChange`]s, which it applies to its own
//! copies as it makes them and commits for the reducer to apply.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use std::time::Duration;

use verbatim_model::{
    CaretMotion, CaretReport, CaretSelection, Change, Compared, KeyFetch, ReadFacts, TextChunk,
    TextUnit, had_effect,
};

/// The key the store keeps a UIA position's range under.
pub type RangeKey = u64;

/// An anchor's position and the text of the chunk it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor<P> {
    /// The anchor's position.
    pub pos: P,
    /// The text of the chunk it started.
    pub text: Arc<str>,
}

/// The anchors minted for one node, the positions the outpost reported in
/// its text, and its last read: `P` is a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAnchors<P> {
    /// The anchors, by number.
    pub anchors: BTreeMap<u64, Anchor<P>>,
    /// Ordered by anchor, so a copy takes one anchor's with a range.
    pub reported: BTreeMap<(u64, u32), P>,
    /// The node's last read, replaced by every read of it; `None` until a
    /// read commits and after a focus read fails.
    pub last: Option<LastRead<P>>,
}

/// The selection of a read: its ends, its text up to the source's cap for
/// compared text when the source read it, and how the read reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadSelection<P> {
    /// Where it starts.
    pub start: P,
    /// Where it ends.
    pub end: P,
    /// Its text as it was read, up to the source's cap for compared text
    /// (513 UTF-16 units through UIA); `None` when the source did not read it.
    pub text: Option<Arc<[u16]>>,
    /// The selection as the read reported it.
    pub report: CaretSelection,
}

/// A node's one last read, the only baseline a key or a comparison of
/// selections has, its life while Core holds the node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastRead<P> {
    /// The caret.
    pub caret: P,
    /// The selection, when something was selected.
    pub selection: Option<ReadSelection<P>>,
    /// The caret's line, with the caret at the chunk's offset.
    pub line: TextChunk,
    /// The node's text changed since this read, so the next comparison of
    /// selections is made in general terms.
    pub text_changed: bool,
    /// When the read began, in microseconds on the shared clock.
    pub read_at_us: u64,
    /// The key's unit at the caret when the read fetched one, for the next
    /// read to compare with.
    pub unit: Option<TextChunk>,
}

impl<P> LastRead<P> {
    /// The same read with each position given by `position`; `None` when
    /// it gives none for one.
    pub fn filter_map<Q>(self, mut position: impl FnMut(P) -> Option<Q>) -> Option<LastRead<Q>> {
        let selection = match self.selection {
            Some(selection) => Some(ReadSelection {
                start: position(selection.start)?,
                end: position(selection.end)?,
                text: selection.text,
                report: selection.report,
            }),
            None => None,
        };
        Some(LastRead {
            caret: position(self.caret)?,
            selection,
            line: self.line,
            text_changed: self.text_changed,
            read_at_us: self.read_at_us,
            unit: self.unit,
        })
    }

    /// Every position it holds.
    pub fn positions(&self) -> impl Iterator<Item = &P> {
        std::iter::once(&self.caret).chain(
            self.selection
                .iter()
                .flat_map(|selection| [&selection.start, &selection.end]),
        )
    }
}

impl<P: Clone> LastRead<P> {
    /// The selection a comparison compares with: its ends, both the caret
    /// when nothing was selected.
    #[must_use]
    pub fn kept_selection(&self) -> KeptSelection<P> {
        match &self.selection {
            Some(selection) => KeptSelection {
                ends: (selection.start.clone(), selection.end.clone()),
                collapsed: false,
                text: selection.text.clone(),
            },
            None => KeptSelection {
                ends: (self.caret.clone(), self.caret.clone()),
                collapsed: true,
                text: None,
            },
        }
    }
}

/// The selection of a last read, as a comparison compares with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptSelection<P> {
    /// Its ends, both the caret when nothing was selected.
    pub ends: (P, P),
    /// Nothing was selected.
    pub collapsed: bool,
    /// Its text as it was read, up to the source's cap for compared text,
    /// when something was selected and the source read it.
    pub text: Option<Arc<[u16]>>,
}

impl<P> Default for NodeAnchors<P> {
    fn default() -> Self {
        Self {
            anchors: BTreeMap::new(),
            reported: BTreeMap::new(),
            last: None,
        }
    }
}

impl<P> KeptSelection<P> {
    /// The same selection with each end given by `end`.
    pub fn map<Q>(&self, mut end: impl FnMut(&P) -> Q) -> KeptSelection<Q> {
        KeptSelection {
            ends: (end(&self.ends.0), end(&self.ends.1)),
            collapsed: self.collapsed,
            text: self.text.clone(),
        }
    }
}

impl<P: Clone> NodeAnchors<P> {
    /// What a text call copies of these: the anchors `named`, those of the
    /// positions its operation names, and that of the last read's line,
    /// each with the positions reported in its chunk, with the last read,
    /// so a copy costs the same however many anchors the node has. Also the
    /// anchors it holds.
    pub fn part(&self, named: &[u64]) -> (Self, BTreeSet<u64>) {
        let mut part = Self::default();
        let mut copied = BTreeSet::new();
        let line = self.last.iter().map(|last| last.line.start.0);
        for number in named.iter().copied().chain(line) {
            if !copied.insert(number) {
                continue;
            }
            if let Some(anchor) = self.anchors.get(&number) {
                part.anchors.insert(number, anchor.clone());
            }
            for (key, pos) in self.reported.range((number, 0)..=(number, u32::MAX)) {
                part.reported.insert(*key, pos.clone());
            }
        }
        part.last.clone_from(&self.last);
        (part, copied)
    }

    /// Every position these hold, once for each place it is kept.
    pub fn positions(&self) -> impl Iterator<Item = &P> {
        let anchors = self.anchors.values().map(|anchor| &anchor.pos);
        let last = self.last.iter().flat_map(LastRead::positions);
        anchors.chain(self.reported.values()).chain(last)
    }

    /// These with each position given by `position`: an anchor, a reported
    /// position or the last read whose position it gives none for is left
    /// out, as one forgotten.
    pub fn filter_map<Q>(self, mut position: impl FnMut(P) -> Option<Q>) -> NodeAnchors<Q> {
        let anchors = self
            .anchors
            .into_iter()
            .filter_map(|(number, anchor)| {
                let pos = position(anchor.pos)?;
                Some((
                    number,
                    Anchor {
                        pos,
                        text: anchor.text,
                    },
                ))
            })
            .collect();
        let reported = self
            .reported
            .into_iter()
            .filter_map(|(key, pos)| Some((key, position(pos)?)))
            .collect();
        NodeAnchors {
            anchors,
            reported,
            last: self.last.and_then(|last| last.filter_map(position)),
        }
    }

    /// Takes in `part`, what a text call left of the part it copied
    /// ([`part`](Self::part)): its anchors and reported positions are
    /// added, but for those of an anchor it held when it was copied
    /// (`copied`) that was forgotten since, which stays forgotten; its last
    /// read replaces this one. The anchors it did not copy are left as they
    /// are.
    fn put(&mut self, part: &Self, copied: &BTreeSet<u64>) {
        for (number, anchor) in &part.anchors {
            if !copied.contains(number) || self.anchors.contains_key(number) {
                self.anchors.insert(*number, anchor.clone());
            }
        }
        for (key, pos) in &part.reported {
            if !copied.contains(&key.0) || self.anchors.contains_key(&key.0) {
                self.reported.insert(*key, pos.clone());
            }
        }
        self.last.clone_from(&part.last);
    }

    /// Forgets the anchors `forgotten` names, with the positions reported
    /// in their chunks.
    fn forget_anchors(&mut self, forgotten: &BTreeSet<u64>) {
        self.anchors.retain(|number, _| !forgotten.contains(number));
        self.reported
            .retain(|(anchor, _), _| !forgotten.contains(anchor));
    }
}

/// A caret event's report of an edit control cut at its value change
/// check (`books/dev/src/explanation/design/ownership.md`): what the read part found, which the part that finishes it
/// compares and reports from. `P` is a position, an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportCut<P> {
    /// The caret and the selection's ends, when something is selected.
    pub caret: P,
    /// The selection's ends, when something is selected.
    pub selection: Option<(P, P)>,
    /// The caret is no longer where the read was asked to compare with.
    pub moved: bool,
    /// When the read began, in microseconds on the shared clock.
    pub read_at_us: u64,
    /// The selection the report compares with.
    pub last: KeptSelection<P>,
    /// What the read fetches for a key, when it was made for one.
    pub fetch: Option<NoteFetch>,
    /// The node's last read, which a read for a key compares with.
    pub previous: Option<LastRead<P>>,
}

/// A read for a key of an edit control, cut at its value change check, the
/// evidence found: what the read part found, which the part that finishes
/// it answers from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerCut<P> {
    /// The caret and the selection's ends, when something is selected.
    pub caret: P,
    /// The selection's ends, when something is selected.
    pub selection: Option<(P, P)>,
    /// When the read began, in microseconds on the shared clock.
    pub read_at_us: u64,
    /// The caret's line, when the read part read it.
    pub line: Option<CutLine<P>>,
    /// The key's unit and motion.
    pub note: NoteFetch,
    /// Whether the caret is on the line it was on in the last read.
    pub on_previous_line: bool,
    /// Whether the read found the key's effect.
    pub effect: bool,
    /// What the read found compared with the last read.
    pub comparison: verbatim_model::Compared,
    /// The selection the answer compares with.
    pub compared: KeptSelection<P>,
    /// The node's last read.
    pub previous: LastRead<P>,
}

/// The caret's line as a cut read part read it: its ends, its UTF-16 text,
/// whether the text was cut at the limit, and the caret's offset in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutLine<P> {
    /// The line's start.
    pub start: P,
    /// The line's end.
    pub end: P,
    /// The line's text, in UTF-16.
    pub text: Vec<u16>,
    /// Whether the text was cut at the limit.
    pub truncated: bool,
    /// The caret's offset in the text.
    pub offset: usize,
}

/// One change of a node's text facts, as a program's text call makes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextChange {
    /// What a text call left of the part of a UIA node's facts it copied,
    /// with the anchors it held ([`NodeAnchors::part`]).
    Uia {
        /// The node's number.
        node: u64,
        /// What the text call left of the part it copied.
        part: NodeAnchors<RangeKey>,
        /// The anchors the part held.
        copied: BTreeSet<u64>,
    },
    /// The same for an edit control, its positions offsets.
    Edit {
        /// The node's number.
        node: u64,
        /// What the text call left of the part it copied.
        part: NodeAnchors<u32>,
        /// The anchors the part held.
        copied: BTreeSet<u64>,
    },
    /// A focus read failed: the node's last read is dropped, so the next
    /// read compares with nothing.
    LastReadFailed(u64),
    /// The edit control's text changed, its value change handled, so its
    /// next comparison of selections is made in general terms.
    ContentChanged(u64),
    /// A caret event of the edit control that a caret read made after it
    /// already covered was handled: the note of its text's changes
    /// ([`TextChange::ContentChanged`]) is cleared, as the comparison NVDA
    /// makes at each caret event clears it, and kept while there is no
    /// selection to compare with, as NVDA's comparison keeps it then.
    ContentCounted(u64),
    /// Everything of the node's text is forgotten: its runtime id names a
    /// new element, or its window was destroyed.
    Forget(u64),
    /// A read showed its key's effect: the note ending at `until_us`, when
    /// it is still the node's, reads no more text changes.
    NoteEffect {
        /// The node's number.
        node: u64,
        /// The end of the note the read was made under, which names it.
        until_us: u64,
    },
    /// The key's final read committed: the note ending at `until_us`, when
    /// it is still the node's, is cleared.
    NoteCleared {
        /// The node's number.
        node: u64,
        /// The end of the note the read was made under, which names it.
        until_us: u64,
    },
    /// The console host's before read committed, ending at `read_ended_at_us`:
    /// the note ending at `until_us`, when still the node's, polls for
    /// [`POLL_BOUND`] from then.
    NotePoll {
        /// The node's number.
        node: u64,
        /// The end of the note the read was made under, which names it.
        until_us: u64,
        /// When the read ended.
        read_ended_at_us: u64,
    },
}

impl TextChange {
    /// The node it changes.
    #[must_use]
    pub fn node(&self) -> u64 {
        match self {
            Self::Uia { node, .. }
            | Self::Edit { node, .. }
            | Self::LastReadFailed(node)
            | Self::ContentChanged(node)
            | Self::ContentCounted(node)
            | Self::Forget(node)
            | Self::NoteEffect { node, .. }
            | Self::NoteCleared { node, .. }
            | Self::NotePoll { node, .. } => *node,
        }
    }

    /// Whether it adds to the node's facts, so it is applied only while
    /// the store holds the node.
    #[must_use]
    pub fn adds(&self) -> bool {
        matches!(
            self,
            Self::Uia { .. } | Self::Edit { .. } | Self::ContentChanged(_)
        )
    }
}

/// How long a text field's or terminal's note lasts from its request's
/// arrival: the 150 ms bound on the before read, the 100 ms bound, and a
/// margin for the pipe.
pub const NOTE_LIFE: Duration = Duration::from_millis(300);

/// How long after one of the console poll's reads the next is made.
pub const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long after the before read ended, or the key's note arrived, the
/// console poll ends.
pub const POLL_BOUND: Duration = Duration::from_millis(50);

/// What a key's reads fetch at the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteFetch {
    /// The unit reads fetch.
    pub unit: TextUnit,
    /// What the key does, which says what counts as its evidence.
    pub motion: CaretMotion,
}

/// What reads a text field's note causes besides its caret events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldReads {
    /// Text changes are read too.
    CaretAndText,
    /// A read showed the key's effect: caret events are still read with
    /// their facts, and text changes are not read.
    CaretOnly,
}

/// What reads a terminal's note causes besides its caret events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalReads {
    /// Text changes are read too.
    CaretAndText,
    /// A read showed the key's effect: caret events are still read with
    /// their facts, and text changes are not read.
    CaretOnly,
    /// The console host, which raises no prompt caret event, polled too.
    Poll {
        /// When the next read is due.
        next_us: u64,
        /// When the polling ends.
        until_us: u64,
    },
}

/// What reads a node's note causes besides the node's caret events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteReads {
    /// A text field's note.
    Field(FieldReads),
    /// A terminal's note.
    Terminal(TerminalReads),
}

impl NoteReads {
    /// Whether a read has shown the key's effect.
    #[must_use]
    pub fn effect_seen(self) -> bool {
        matches!(
            self,
            Self::Field(FieldReads::CaretOnly) | Self::Terminal(TerminalReads::CaretOnly)
        )
    }

    /// These reads once a read has shown the key's effect.
    fn narrowed(self) -> Self {
        match self {
            Self::Field(_) => Self::Field(FieldReads::CaretOnly),
            Self::Terminal(_) => Self::Terminal(TerminalReads::CaretOnly),
        }
    }
}

/// Which node a note is set on, which says what it reads at first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteOn {
    /// A text field.
    Field,
    /// A terminal other than the console host.
    Terminal,
    /// The console host's text area, whose note is polled.
    ConsoleHost,
}

/// A node's note: for a short time after a key's request, which unit its
/// reads fetch and that text changes are read too. It knows nothing of the
/// key's progress and ends by the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyNote {
    /// The key's unit and motion.
    pub fetch: NoteFetch,
    /// When it ends, on the shared clock.
    pub until_us: u64,
    /// What it reads.
    pub reads: NoteReads,
}

/// `line`'s caret as a byte offset; past any text when it does not fit.
fn caret_offset(line: &TextChunk) -> usize {
    usize::try_from(line.offset).unwrap_or(usize::MAX)
}

/// The character at `line`'s caret; empty at its end.
fn character_at(line: &TextChunk) -> &str {
    verbatim_text::grapheme_at(&line.text, caret_offset(line))
        .and_then(|range| line.text.get(range))
        .unwrap_or_default()
}

/// The characters just before and at `line`'s caret.
fn beside(line: &TextChunk) -> (&str, &str) {
    let offset = caret_offset(line);
    let before = line
        .text
        .get(..offset)
        .and_then(|text| {
            verbatim_text::graphemes(text)
                .pop()
                .and_then(|range| text.get(range))
        })
        .unwrap_or_default();
    (before, character_at(line))
}

/// `duration` in microseconds.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// One node's text facts, of one provider.
#[derive(Clone, Debug)]
enum NodeRecord {
    Uia(Arc<NodeAnchors<RangeKey>>),
    Edit(Arc<NodeAnchors<u32>>),
}

/// Everything kept for one node: its text facts, once a read has made them,
/// and its note, which end with it.
#[derive(Clone, Debug, Default)]
struct NodeEntry {
    record: Option<NodeRecord>,
    note: Option<KeyNote>,
}

/// Every node's text facts, through UIA and through edit controls' messages,
/// and their notes, so one place names every range the store must keep.
#[derive(Clone, Debug, Default)]
pub struct TextFacts {
    nodes: BTreeMap<u64, NodeEntry>,
    /// The one console poll, whose node's note holds its times, so its due
    /// time is found without walking the nodes.
    polling: Option<u64>,
    /// The nodes that were given a note, as a read of an event not known
    /// to be the focus's looks one up without walking the nodes; ones past
    /// their note are removed as the next note is set.
    noted: BTreeSet<u64>,
}

/// The keys `record` names.
fn keys(record: Option<&NodeEntry>) -> BTreeSet<RangeKey> {
    match record.and_then(|entry| entry.record.as_ref()) {
        Some(NodeRecord::Uia(anchors)) => anchors.positions().copied().collect(),
        Some(NodeRecord::Edit(_)) | None => BTreeSet::new(),
    }
}

impl TextFacts {
    /// UIA node `node`'s facts, when it has any.
    #[must_use]
    pub fn uia(&self, node: u64) -> Option<&NodeAnchors<RangeKey>> {
        match self.nodes.get(&node)?.record.as_ref()? {
            NodeRecord::Uia(anchors) => Some(&**anchors),
            NodeRecord::Edit(_) => None,
        }
    }

    /// Edit control `node`'s facts, when it has any.
    #[must_use]
    pub fn edit(&self, node: u64) -> Option<&NodeAnchors<u32>> {
        match self.nodes.get(&node)?.record.as_ref()? {
            NodeRecord::Edit(anchors) => Some(&**anchors),
            NodeRecord::Uia(_) => None,
        }
    }

    /// Node `node`'s note, when it has one and it has not passed its end
    /// by `now_us`: a note that has passed its end counts as absent.
    #[must_use]
    pub fn live_note(&self, node: u64, now_us: u64) -> Option<KeyNote> {
        self.nodes
            .get(&node)?
            .note
            .filter(|note| note.until_us > now_us)
    }

    /// Node `node`'s last read's line and selection.
    fn last_line(&self, node: u64) -> Option<(TextChunk, Option<CaretSelection>)> {
        let selection = |report: CaretSelection| report;
        match self.nodes.get(&node)?.record.as_ref()? {
            NodeRecord::Uia(anchors) => anchors.last.as_ref().map(|last| {
                let report = last.selection.as_ref().map(|s| selection(s.report.clone()));
                (last.line.clone(), report)
            }),
            NodeRecord::Edit(anchors) => anchors.last.as_ref().map(|last| {
                let report = last.selection.as_ref().map(|s| selection(s.report.clone()));
                (last.line.clone(), report)
            }),
        }
    }

    /// What a caret event's read, dispatched before node `node`'s live note
    /// arrived and so made with no facts, found: the comparison with the
    /// node's last read by the text both hold, as a report's is, which
    /// needs no call, and the key's character cut from the line or its line;
    /// a word it did not fetch. `None` with no note to judge by.
    /// `selection_moved` is whether the report's selection changed. A read
    /// that shows the effect narrows the note.
    pub fn facts_at_commit(
        &mut self,
        node: u64,
        (caret, selection_moved): (&CaretReport, bool),
        now_us: u64,
    ) -> Option<ReadFacts> {
        let key_fetch = self.live_note(node, now_us)?.fetch;
        let line = &caret.line;
        let unit = match key_fetch.unit {
            TextUnit::Character => Some(TextChunk {
                unit: TextUnit::Character,
                text: character_at(line).to_owned(),
                offset: 0,
                languages: Vec::new(),
                formats: Vec::new(),
                ..line.clone()
            }),
            TextUnit::Line => Some(line.clone()),
            _ => None,
        };
        let change = self
            .last_line(node)
            .map_or(Change::NoPrevious, |(was, selection)| {
                let line_changed = line.text != was.text;
                let compared = Compared {
                    caret_moved: line_changed || line.offset != was.offset,
                    selection_moved,
                    beside_changed: beside(line) != beside(&was),
                    unit_changed: key_fetch.unit == TextUnit::Character
                        && character_at(line) != character_at(&was),
                    line_changed,
                };
                Change::Compared {
                    compared,
                    previous_selection: selection,
                    previous_line: was,
                    on_previous_line: !line_changed,
                }
            });
        if matches!(change, Change::Compared { compared, .. } if had_effect(compared, key_fetch.motion))
            && let Some(narrowed) = self
                .nodes
                .get_mut(&node)
                .and_then(|entry| entry.note.as_mut())
        {
            narrowed.reads = narrowed.reads.narrowed();
        }
        Some(ReadFacts {
            unit,
            change,
            read_ended_at_us: now_us,
        })
    }

    /// The live notes, by node.
    pub fn live_notes(&self, now_us: u64) -> impl Iterator<Item = (u64, KeyNote)> + '_ {
        self.noted
            .iter()
            .filter_map(move |&node| Some((node, self.live_note(node, now_us)?)))
    }

    /// Sets node `node`'s note as a request for it arrives at `now_us`, one
    /// on the console host polling for [`POLL_BOUND`].
    pub fn set_note(&mut self, node: u64, fetch: KeyFetch, now_us: u64, on: NoteOn) {
        let nodes = &self.nodes;
        self.noted.retain(|noted| {
            nodes
                .get(noted)
                .and_then(|entry| entry.note)
                .is_some_and(|note| note.until_us > now_us)
        });
        self.noted.insert(node);
        let until_us = now_us.saturating_add(micros(NOTE_LIFE));
        let reads = match on {
            NoteOn::Field => NoteReads::Field(FieldReads::CaretAndText),
            NoteOn::Terminal => NoteReads::Terminal(TerminalReads::CaretAndText),
            NoteOn::ConsoleHost => {
                self.polling = Some(node);
                let until = now_us.saturating_add(micros(POLL_BOUND));
                NoteReads::Terminal(TerminalReads::Poll {
                    next_us: now_us.saturating_add(micros(POLL_INTERVAL)).min(until),
                    until_us: until,
                })
            }
        };
        self.nodes.entry(node).or_default().note = Some(KeyNote {
            fetch: NoteFetch {
                unit: fetch.unit,
                motion: fetch.motion,
            },
            until_us,
            reads,
        });
    }

    /// Clears node `node`'s note, when it has one.
    pub fn clear_note(&mut self, node: u64) {
        if let Some(entry) = self.nodes.get_mut(&node) {
            entry.note = None;
        }
    }

    /// When the console poll is next due, while its note lives; the poll
    /// is acted on only when due.
    #[must_use]
    pub fn poll_due(&self) -> Option<u64> {
        let note = self.nodes.get(&self.polling?)?.note?;
        match note.reads {
            NoteReads::Terminal(TerminalReads::Poll { next_us, .. }) => Some(next_us),
            NoteReads::Field(_) | NoteReads::Terminal(_) => None,
        }
    }

    /// The console poll's due time, reached at `now_us`: the node whose
    /// read is due, its next due time moved on, or, past the note's end or
    /// the poll's last read, the note goes on without polling.
    pub fn reach_poll(&mut self, now_us: u64) -> Option<u64> {
        let number = self.polling?;
        let Some(note) = self
            .nodes
            .get_mut(&number)
            .and_then(|entry| entry.note.as_mut())
        else {
            self.polling = None;
            return None;
        };
        let NoteReads::Terminal(TerminalReads::Poll { next_us, until_us }) = note.reads else {
            self.polling = None;
            return None;
        };
        if note.until_us <= now_us {
            note.reads = NoteReads::Terminal(TerminalReads::CaretAndText);
            self.polling = None;
            return None;
        }
        if now_us < next_us {
            return None;
        }
        if next_us >= until_us {
            note.reads = NoteReads::Terminal(TerminalReads::CaretAndText);
            self.polling = None;
        } else {
            note.reads = NoteReads::Terminal(TerminalReads::Poll {
                next_us: now_us.saturating_add(micros(POLL_INTERVAL)).min(until_us),
                until_us,
            });
        }
        Some(number)
    }

    /// The keys of `node`'s facts.
    fn named(&self, node: u64) -> BTreeSet<RangeKey> {
        keys(self.nodes.get(&node))
    }

    /// Applies `change`. Returns the UIA ranges no fact names any more, by
    /// node and key: those it replaced or forgot, and those a text call
    /// minted for an anchor forgotten meanwhile, for the store to let go
    /// of.
    pub fn apply(&mut self, change: &TextChange) -> Vec<(u64, RangeKey)> {
        let node = change.node();
        let before = self.named(node);
        let mut named = before.clone();
        match change {
            TextChange::Uia { part, copied, .. } => {
                named.extend(part.positions().copied());
                let entry = self.nodes.entry(node).or_default();
                let record = entry
                    .record
                    .get_or_insert_with(|| NodeRecord::Uia(Arc::default()));
                if let NodeRecord::Uia(anchors) = record {
                    Arc::make_mut(anchors).put(part, copied);
                } else {
                    let mut anchors = NodeAnchors::default();
                    anchors.put(part, copied);
                    *record = NodeRecord::Uia(Arc::new(anchors));
                }
            }
            TextChange::Edit { part, copied, .. } => {
                let entry = self.nodes.entry(node).or_default();
                let record = entry
                    .record
                    .get_or_insert_with(|| NodeRecord::Edit(Arc::default()));
                if let NodeRecord::Edit(anchors) = record {
                    Arc::make_mut(anchors).put(part, copied);
                } else {
                    let mut anchors = NodeAnchors::default();
                    anchors.put(part, copied);
                    *record = NodeRecord::Edit(Arc::new(anchors));
                }
            }
            TextChange::LastReadFailed(_) => match self.record_mut(node) {
                Some(NodeRecord::Uia(anchors)) if anchors.last.is_some() => {
                    Arc::make_mut(anchors).last = None;
                }
                Some(NodeRecord::Edit(anchors)) if anchors.last.is_some() => {
                    Arc::make_mut(anchors).last = None;
                }
                _ => {}
            },
            TextChange::ContentChanged(_) => {
                if let Some(NodeRecord::Edit(anchors)) = self.record_mut(node)
                    && let Some(last) = Arc::make_mut(anchors).last.as_mut()
                {
                    last.text_changed = true;
                }
            }
            TextChange::ContentCounted(_) => {
                if let Some(NodeRecord::Edit(anchors)) = self.record_mut(node)
                    && anchors.last.as_ref().is_some_and(|last| last.text_changed)
                    && let Some(last) = Arc::make_mut(anchors).last.as_mut()
                {
                    last.text_changed = false;
                }
            }
            TextChange::Forget(_) => {
                self.nodes.remove(&node);
            }
            TextChange::NoteEffect { until_us, .. } => {
                if let Some(note) = self.note_named(node, *until_us) {
                    note.reads = note.reads.narrowed();
                }
            }
            TextChange::NoteCleared { until_us, .. } => {
                if self.note_named(node, *until_us).is_some() {
                    self.clear_note(node);
                }
            }
            TextChange::NotePoll {
                until_us,
                read_ended_at_us,
                ..
            } => {
                if let Some(note) = self
                    .note_named(node, *until_us)
                    .filter(|note| matches!(note.reads, NoteReads::Terminal(_)))
                {
                    let until = read_ended_at_us.saturating_add(micros(POLL_BOUND));
                    note.reads = NoteReads::Terminal(TerminalReads::Poll {
                        next_us: read_ended_at_us
                            .saturating_add(micros(POLL_INTERVAL))
                            .min(until),
                        until_us: until,
                    });
                    self.polling = Some(node);
                }
            }
        }
        let after = self.named(node);
        named.difference(&after).map(|&key| (node, key)).collect()
    }

    /// Node `node`'s record, to change.
    fn record_mut(&mut self, node: u64) -> Option<&mut NodeRecord> {
        self.nodes.get_mut(&node)?.record.as_mut()
    }

    /// Node `node`'s note named by the end `until_us` of the note a program
    /// read, when it is still the node's.
    fn note_named(&mut self, node: u64, until_us: u64) -> Option<&mut KeyNote> {
        self.nodes
            .get_mut(&node)?
            .note
            .as_mut()
            .filter(|note| note.until_us == until_us)
    }

    /// Forgets the nodes `nodes` and the anchors `anchors` a release freed.
    /// Returns the UIA ranges of the anchors no fact names any more, by
    /// node and key; a freed node's ranges the store let go of with it.
    pub fn release(
        &mut self,
        nodes: &BTreeSet<u64>,
        anchors: &BTreeSet<u64>,
    ) -> Vec<(u64, RangeKey)> {
        self.nodes.retain(|node, _| !nodes.contains(node));
        self.noted.retain(|node| !nodes.contains(node));
        if anchors.is_empty() {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        for (&node, entry) in &mut self.nodes {
            let Some(record) = entry.record.as_mut() else {
                continue;
            };
            match record {
                NodeRecord::Uia(kept) => {
                    if !kept.anchors.keys().any(|number| anchors.contains(number)) {
                        continue;
                    }
                    let before: BTreeSet<RangeKey> = kept.positions().copied().collect();
                    Arc::make_mut(kept).forget_anchors(anchors);
                    let after: BTreeSet<RangeKey> = kept.positions().copied().collect();
                    dropped.extend(before.difference(&after).map(|&key| (node, key)));
                }
                NodeRecord::Edit(kept) => {
                    if kept.anchors.keys().any(|number| anchors.contains(number)) {
                        Arc::make_mut(kept).forget_anchors(anchors);
                    }
                }
            }
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit control's facts with anchors `numbers`, each at its own
    /// number as offset, and a position reported in each.
    fn edit_part(numbers: &[u64]) -> NodeAnchors<u32> {
        let mut part = NodeAnchors::default();
        for &number in numbers {
            let offset = u32::try_from(number).expect("a small number");
            part.anchors.insert(
                number,
                Anchor {
                    pos: offset,
                    text: Arc::from(""),
                },
            );
            part.reported.insert((number, 1), offset + 1);
        }
        part
    }

    /// A last read at `caret`, with the selection's ends when given.
    fn last_read(caret: u64, selection: Option<(u64, u64)>) -> LastRead<u64> {
        let position = verbatim_model::TextPosition::at(verbatim_model::TextAnchor(1));
        LastRead {
            caret,
            selection: selection.map(|(start, end)| ReadSelection {
                start,
                end,
                text: None,
                report: CaretSelection {
                    range: verbatim_model::Selection {
                        start: position,
                        end: position,
                    },
                    start_line: None,
                },
            }),
            line: TextChunk {
                unit: TextUnit::Line,
                text: String::new(),
                start: verbatim_model::TextAnchor(1),
                offset: 0,
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats: Vec::new(),
            },
            text_changed: false,
            read_at_us: 0,
            unit: None,
        }
    }

    /// A note's fetch.
    fn fetch(motion: CaretMotion) -> KeyFetch {
        KeyFetch {
            unit: TextUnit::Character,
            motion,
        }
    }

    /// A note lives from its request's arrival for 300 ms, and is the
    /// node's until then, whatever else happens to the node's text.
    #[test]
    fn a_note_ends_by_the_clock() {
        let mut facts = TextFacts::default();
        facts.set_note(1, fetch(CaretMotion::NextCharacter), 1_000, NoteOn::Field);
        let until = 301_000;
        assert_eq!(
            facts.live_note(1, until - 1).map(|note| note.until_us),
            Some(until)
        );
        assert_eq!(facts.live_note(1, until), None);
        assert_eq!(facts.live_note(2, 1_000), None);
        assert_eq!(facts.live_notes(until - 1).count(), 1);
        assert_eq!(facts.live_notes(until).count(), 0);
    }

    /// A read's effect narrows the note it was made under, and a later
    /// note replacing it keeps its own reads.
    #[test]
    fn an_effect_narrows_only_the_note_it_was_read_under() {
        let mut facts = TextFacts::default();
        facts.set_note(1, fetch(CaretMotion::NextCharacter), 1_000, NoteOn::Field);
        let until = facts.live_note(1, 1_000).expect("the note").until_us;
        facts.set_note(1, fetch(CaretMotion::Delete), 2_000, NoteOn::Field);
        // An effect read under the older note does not touch the newer one.
        facts.apply(&TextChange::NoteEffect {
            node: 1,
            until_us: until,
        });
        assert_eq!(
            facts.live_note(1, 2_000).map(|note| note.reads),
            Some(NoteReads::Field(FieldReads::CaretAndText))
        );
        let until = facts.live_note(1, 2_000).expect("the note").until_us;
        facts.apply(&TextChange::NoteEffect {
            node: 1,
            until_us: until,
        });
        assert_eq!(
            facts.live_note(1, 2_000).map(|note| note.reads),
            Some(NoteReads::Field(FieldReads::CaretOnly))
        );
    }

    /// A caret report with the note's facts attached at its commit: the
    /// character from the line, the comparison by the text, and the note
    /// narrowed by the effect; a key that reads another unit gets none.
    #[test]
    fn a_read_dispatched_before_its_note_gets_facts_at_commit() {
        let report = |offset: u32| CaretReport {
            line: TextChunk {
                text: "abc".to_owned(),
                offset,
                ..last_read(0, None).line
            },
            selection: None,
            read_at_us: 9,
        };
        let mut facts = TextFacts::default();
        let part = NodeAnchors {
            last: Some(LastRead {
                line: report(0).line,
                ..last_read(0, None)
            }),
            ..NodeAnchors::default()
        };
        facts.apply(&TextChange::Uia {
            node: 1,
            part,
            copied: BTreeSet::new(),
        });
        assert_eq!(facts.facts_at_commit(1, (&report(1), false), 0), None);
        facts.set_note(1, fetch(CaretMotion::NextCharacter), 0, NoteOn::Field);
        let found = facts
            .facts_at_commit(1, (&report(1), false), 5)
            .expect("the facts");
        assert_eq!(found.unit.map(|unit| unit.text), Some("b".to_owned()));
        assert!(matches!(
            found.change,
            Change::Compared { compared, .. } if compared.caret_moved
        ));
        assert_eq!(found.read_ended_at_us, 5);
        assert_eq!(
            facts.live_note(1, 5).map(|note| note.reads),
            Some(NoteReads::Field(FieldReads::CaretOnly))
        );
        let mut word = fetch(CaretMotion::NextWord);
        word.unit = TextUnit::Word;
        facts.set_note(1, word, 6, NoteOn::Field);
        assert_eq!(
            facts
                .facts_at_commit(1, (&report(2), false), 7)
                .expect("the facts")
                .unit,
            None
        );
        let mut line = fetch(CaretMotion::NextLine);
        line.unit = TextUnit::Line;
        facts.set_note(1, line, 8, NoteOn::Field);
        assert_eq!(
            facts
                .facts_at_commit(1, (&report(2), false), 9)
                .expect("the facts")
                .unit,
            Some(report(2).line)
        );
    }

    /// A note goes with its node's record.
    #[test]
    fn a_note_is_dropped_with_its_node() {
        let mut facts = TextFacts::default();
        facts.set_note(1, fetch(CaretMotion::NextCharacter), 0, NoteOn::Field);
        facts.apply(&TextChange::Forget(1));
        assert_eq!(facts.live_note(1, 0), None);
        facts.set_note(2, fetch(CaretMotion::NextCharacter), 0, NoteOn::Field);
        facts.release(&BTreeSet::from([2]), &BTreeSet::new());
        assert_eq!(facts.live_note(2, 0), None);
    }

    /// The console's before read starts the poll from its end, and the
    /// poll's next due time moves on 5 ms each time it is reached.
    #[test]
    fn a_console_note_polls_from_its_before_reads_end() {
        let mut facts = TextFacts::default();
        facts.set_note(1, fetch(CaretMotion::NextCharacter), 0, NoteOn::Terminal);
        let until_us = facts.live_note(1, 0).expect("the note").until_us;
        facts.apply(&TextChange::NotePoll {
            node: 1,
            until_us,
            read_ended_at_us: 10_000,
        });
        assert_eq!(facts.poll_due(), Some(15_000));
        assert_eq!(facts.reach_poll(14_999), None);
        assert_eq!(facts.reach_poll(15_000), Some(1));
        assert_eq!(facts.poll_due(), Some(20_000));
    }

    /// A text call's part is taken in without an anchor it copied that was
    /// forgotten meanwhile, keeping one it minted and leaving an anchor it
    /// did not copy as it was.
    #[test]
    fn a_part_put_back_keeps_only_what_is_still_held() {
        let mut facts = TextFacts::default();
        let first = TextChange::Edit {
            node: 1,
            part: edit_part(&[4, 6]),
            copied: BTreeSet::new(),
        };
        assert_eq!(facts.apply(&first), Vec::new());
        let (mut part, copied) = facts.edit(1).expect("the node's facts").part(&[4]);
        assert_eq!(copied, BTreeSet::from([4]));
        part.anchors.extend(edit_part(&[9]).anchors);
        assert_eq!(
            facts.release(&BTreeSet::new(), &BTreeSet::from([4])),
            Vec::new()
        );
        let put = TextChange::Edit {
            node: 1,
            part,
            copied,
        };
        assert_eq!(facts.apply(&put), Vec::new());
        let kept = facts.edit(1).expect("the node's facts");
        assert_eq!(kept.anchors.keys().copied().collect::<Vec<_>>(), [6, 9]);
        assert_eq!(kept.reported.keys().copied().collect::<Vec<_>>(), [(6, 1)]);
    }

    /// The UIA ranges a change stops naming are handed back for the store:
    /// a replaced selection's ends, and a range minted for an anchor
    /// forgotten meanwhile.
    #[test]
    fn the_ranges_no_fact_names_are_handed_back() {
        let mut facts = TextFacts::default();
        let mut part = NodeAnchors::default();
        part.anchors.insert(
            3,
            Anchor {
                pos: 10,
                text: Arc::from("a"),
            },
        );
        part.last = Some(last_read(11, Some((11, 12))));
        let first = TextChange::Uia {
            node: 1,
            part,
            copied: BTreeSet::new(),
        };
        assert_eq!(facts.apply(&first), Vec::new());
        let (mut part, copied) = facts.uia(1).expect("the node's facts").part(&[3]);
        part.reported.insert((3, 1), 13);
        part.last = Some(last_read(14, None));
        assert_eq!(
            facts.release(&BTreeSet::new(), &BTreeSet::from([3])),
            vec![(1, 10)]
        );
        let put = TextChange::Uia {
            node: 1,
            part,
            copied,
        };
        // The released anchor's range is named again: the store lets go of
        // a range once, whatever names it.
        assert_eq!(facts.apply(&put), vec![(1, 10), (1, 11), (1, 12), (1, 13)]);
        assert_eq!(
            facts
                .uia(1)
                .expect("the node's facts")
                .positions()
                .copied()
                .collect::<Vec<_>>(),
            [14]
        );
    }
}
