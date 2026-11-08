//! Text support per node, as the reducer remembers it
//! (`books/dev/src/explanation/design/ownership.md`): each node's anchors with their chunks' text, the positions the
//! outpost reported, and the node's one last read, the only baseline a
//! caret key or a comparison of selections has
//! (`docs/design/caret-selection-rework.md`), with the caret key's watch
//! that pins it. A position is a fact of its own kind: an edit control's
//! offset, or the key of a UIA range the outpost's store keeps, so no
//! application object is held here and an edit control's offsets cross
//! between programs as they are.
//!
//! A program reads these in its copies ([`super::Facts`]); its text calls
//! change them only through [`TextChange`]s, which it applies to its own
//! copies as it makes them and commits for the reducer to apply.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use verbatim_model::{CaretSelection, TextChunk, TextUnit};

use super::watch::{Baseline, Watch};

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
    /// The unit a before read asked for at the caret.
    pub unit: Option<TextUnit>,
}

/// A caret key's answer from an edit control cut at its value change check,
/// the evidence found: what the read part found, which the part that
/// finishes it answers from.
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
    /// The watch's unit.
    pub unit: TextUnit,
    /// Whether the caret is on the line it was on in the baseline.
    pub on_baseline_line: bool,
    /// Whether the check found the key's effect.
    pub effect: bool,
    /// The selection the answer compares with.
    pub compared: KeptSelection<P>,
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
            | Self::Forget(node) => *node,
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

/// One node's text facts, of one provider.
#[derive(Clone, Debug)]
enum NodeRecord {
    Uia(Arc<NodeAnchors<RangeKey>>),
    Edit(Arc<NodeAnchors<u32>>),
}

/// Every node's text facts, through UIA and through edit controls' messages,
/// and the caret key's watch, so one place names every range the store must
/// keep.
#[derive(Clone, Debug, Default)]
pub struct TextFacts {
    nodes: BTreeMap<u64, NodeRecord>,
    /// The caret key's watch for evidence.
    pub watch: Watch,
}

/// The keys `record` names.
fn keys(record: Option<&NodeRecord>) -> BTreeSet<RangeKey> {
    match record {
        Some(NodeRecord::Uia(anchors)) => anchors.positions().copied().collect(),
        Some(NodeRecord::Edit(_)) | None => BTreeSet::new(),
    }
}

impl TextFacts {
    /// UIA node `node`'s facts, when it has any.
    #[must_use]
    pub fn uia(&self, node: u64) -> Option<&NodeAnchors<RangeKey>> {
        match self.nodes.get(&node) {
            Some(NodeRecord::Uia(anchors)) => Some(&**anchors),
            _ => None,
        }
    }

    /// Edit control `node`'s facts, when it has any.
    #[must_use]
    pub fn edit(&self, node: u64) -> Option<&NodeAnchors<u32>> {
        match self.nodes.get(&node) {
            Some(NodeRecord::Edit(anchors)) => Some(&**anchors),
            _ => None,
        }
    }

    /// The baseline a key arriving now would pin: node `node`'s last read,
    /// when it has one.
    #[must_use]
    pub fn baseline(&self, node: u64) -> Option<Baseline> {
        match self.nodes.get(&node)? {
            NodeRecord::Uia(anchors) => anchors.last.clone().map(Baseline::Uia),
            NodeRecord::Edit(anchors) => anchors.last.clone().map(Baseline::Edit),
        }
    }

    /// The keys of `node`'s facts and of the watch's baseline when it is
    /// `node`'s.
    fn named(&self, node: u64) -> BTreeSet<RangeKey> {
        let mut named = keys(self.nodes.get(&node));
        named.extend(self.watch.baseline_keys(node));
        named
    }

    /// Applies `f` to the watch. Returns what it returned, and the UIA
    /// ranges no fact names any more once the watch has let them go, by
    /// node and key, for the store to let go of.
    pub fn with_watch<T>(&mut self, f: impl FnOnce(&mut Watch) -> T) -> (T, Vec<(u64, RangeKey)>) {
        let node = self.watch.open().map(|key| key.node.number());
        let before = node.map(|node| (node, self.named(node)));
        let result = f(&mut self.watch);
        let mut dropped = Vec::new();
        if let Some((node, before)) = before {
            let after = self.named(node);
            dropped.extend(before.difference(&after).map(|&key| (node, key)));
        }
        (result, dropped)
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
                let record = self
                    .nodes
                    .entry(node)
                    .or_insert_with(|| NodeRecord::Uia(Arc::default()));
                if let NodeRecord::Uia(anchors) = record {
                    Arc::make_mut(anchors).put(part, copied);
                } else {
                    let mut anchors = NodeAnchors::default();
                    anchors.put(part, copied);
                    *record = NodeRecord::Uia(Arc::new(anchors));
                }
            }
            TextChange::Edit { part, copied, .. } => {
                let record = self
                    .nodes
                    .entry(node)
                    .or_insert_with(|| NodeRecord::Edit(Arc::default()));
                if let NodeRecord::Edit(anchors) = record {
                    Arc::make_mut(anchors).put(part, copied);
                } else {
                    let mut anchors = NodeAnchors::default();
                    anchors.put(part, copied);
                    *record = NodeRecord::Edit(Arc::new(anchors));
                }
            }
            TextChange::LastReadFailed(_) => match self.nodes.get_mut(&node) {
                Some(NodeRecord::Uia(anchors)) if anchors.last.is_some() => {
                    Arc::make_mut(anchors).last = None;
                }
                Some(NodeRecord::Edit(anchors)) if anchors.last.is_some() => {
                    Arc::make_mut(anchors).last = None;
                }
                _ => {}
            },
            TextChange::ContentChanged(_) => {
                if let Some(NodeRecord::Edit(anchors)) = self.nodes.get_mut(&node)
                    && let Some(last) = Arc::make_mut(anchors).last.as_mut()
                {
                    last.text_changed = true;
                }
            }
            TextChange::ContentCounted(_) => {
                if let Some(NodeRecord::Edit(anchors)) = self.nodes.get_mut(&node)
                    && anchors.last.as_ref().is_some_and(|last| last.text_changed)
                    && let Some(last) = Arc::make_mut(anchors).last.as_mut()
                {
                    last.text_changed = false;
                }
            }
            TextChange::Forget(_) => {
                self.nodes.remove(&node);
            }
        }
        let after = self.named(node);
        named.difference(&after).map(|&key| (node, key)).collect()
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
        if anchors.is_empty() {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        for (&node, record) in &mut self.nodes {
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
        }
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
