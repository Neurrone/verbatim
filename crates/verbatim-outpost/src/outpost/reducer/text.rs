//! Text support per node, as the reducer remembers it
//! (`docs/design/ownership.md`): each node's anchors with their chunks' text, the positions the
//! outpost reported, the carets it reported and how many reports it made,
//! the selection the next report compares with and its text, and the note
//! that the node's text changed. A position is a fact of its own kind: an
//! edit control's offset, or the key of a UIA range the outpost's store
//! keeps, so no application object is held here and an
//! edit control's offsets cross between programs as they are.
//!
//! A program reads these in its copies ([`super::Facts`]); its text calls
//! change them only through [`TextChange`]s, which it applies to its own
//! copies as it makes them and commits for the reducer to apply.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use verbatim_model::{TextPosition, TextUnit};

/// The key the store keeps a UIA position's range under.
pub(crate) type RangeKey = u64;

/// An anchor's position and the text of the chunk it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Anchor<P> {
    pub(crate) pos: P,
    pub(crate) text: Arc<str>,
}

/// The anchors minted for one node, and the positions the outpost reported
/// in its text, with the caret reports it remembers: `P` is a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeAnchors<P> {
    pub(crate) anchors: BTreeMap<u64, Anchor<P>>,
    /// Ordered by anchor, so a copy takes one anchor's with a range.
    pub(crate) reported: BTreeMap<(u64, u32), P>,
    /// The carets the outpost last reported for the node, oldest first.
    pub(crate) carets: VecDeque<CaretSeen<P>>,
    /// How many caret reports the outpost has made for the node, which
    /// numbers each ([`CaretSeen::report`], [`KeptSelection::report`]).
    pub(crate) reports: u64,
    /// The selection in the outpost's last caret report for the node, which
    /// the next report's selection changes are worked out from; `None` when
    /// there is none to compare with.
    pub(crate) selection: Option<KeptSelection<P>>,
    /// The node's text changed since the next comparison's selection was
    /// reported ([`TextChange::ContentChanged`]), so that comparison is
    /// made in general terms.
    pub(crate) content_changed: bool,
}

/// A caret the outpost reported, as a caret key's watch takes its baseline
/// from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaretSeen<P> {
    /// When it was read, in milliseconds since the Unix epoch.
    pub(crate) read_at_ms: u64,
    /// The caret, as reported.
    pub(crate) caret: TextPosition,
    /// The selection's ends, when something was selected.
    pub(crate) selection: Option<(P, P)>,
    /// The report's number ([`NodeAnchors::reports`]).
    pub(crate) report: u64,
}

/// The selection of a caret report, as the next report compares with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeptSelection<P> {
    /// Its ends, both the caret when nothing was selected.
    pub(crate) ends: (P, P),
    /// Nothing was selected.
    pub(crate) collapsed: bool,
    /// Its text as it was reported, up to the source's cap for compared
    /// text, when something was selected and the source read it.
    pub(crate) text: Option<Arc<[u16]>>,
    /// The number of the report it is from ([`NodeAnchors::reports`]); 0
    /// for one Core sent.
    pub(crate) report: u64,
}

impl<P> Default for NodeAnchors<P> {
    fn default() -> Self {
        Self {
            anchors: BTreeMap::new(),
            reported: BTreeMap::new(),
            carets: VecDeque::new(),
            reports: 0,
            selection: None,
            content_changed: false,
        }
    }
}

impl<P> KeptSelection<P> {
    /// The same selection with each end given by `end`.
    pub(crate) fn map<Q>(&self, mut end: impl FnMut(&P) -> Q) -> KeptSelection<Q> {
        KeptSelection {
            ends: (end(&self.ends.0), end(&self.ends.1)),
            collapsed: self.collapsed,
            text: self.text.clone(),
            report: self.report,
        }
    }
}

impl<P: Clone> NodeAnchors<P> {
    /// What a text call copies of these: the anchors `named`, those of the
    /// positions its operation names, and those of the carets kept, each
    /// with the positions reported in its chunk, with the carets, the
    /// report count, the kept selection and the note, so a copy costs the
    /// same however many anchors the node has. Also the anchors it holds.
    pub(crate) fn part(&self, named: &[u64]) -> (Self, BTreeSet<u64>) {
        let mut part = Self::default();
        let mut copied = BTreeSet::new();
        let carets = self.carets.iter().map(|seen| seen.caret.anchor.0);
        for number in named.iter().copied().chain(carets) {
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
        part.carets.clone_from(&self.carets);
        part.reports = self.reports;
        part.selection.clone_from(&self.selection);
        part.content_changed = self.content_changed;
        (part, copied)
    }

    /// Every position these hold, once for each place it is kept.
    pub(crate) fn positions(&self) -> impl Iterator<Item = &P> {
        let anchors = self.anchors.values().map(|anchor| &anchor.pos);
        let carets = self
            .carets
            .iter()
            .filter_map(|seen| seen.selection.as_ref())
            .flat_map(|(start, end)| [start, end]);
        let selection = self
            .selection
            .iter()
            .flat_map(|selection| [&selection.ends.0, &selection.ends.1]);
        anchors
            .chain(self.reported.values())
            .chain(carets)
            .chain(selection)
    }

    /// These with each position given by `position`: an anchor, a reported
    /// position, a caret or the kept selection whose position it gives
    /// none for is left out, as one forgotten.
    pub(crate) fn filter_map<Q>(self, mut position: impl FnMut(P) -> Option<Q>) -> NodeAnchors<Q> {
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
        let mut ends = |(start, end): (P, P)| Some((position(start)?, position(end)?));
        let carets = self
            .carets
            .into_iter()
            .filter_map(|seen| {
                let selection = match seen.selection {
                    Some(selection) => Some(ends(selection)?),
                    None => None,
                };
                Some(CaretSeen {
                    read_at_ms: seen.read_at_ms,
                    caret: seen.caret,
                    selection,
                    report: seen.report,
                })
            })
            .collect();
        let selection = self.selection.and_then(|selection| {
            Some(KeptSelection {
                ends: ends(selection.ends)?,
                collapsed: selection.collapsed,
                text: selection.text,
                report: selection.report,
            })
        });
        NodeAnchors {
            anchors,
            reported,
            carets,
            reports: self.reports,
            selection,
            content_changed: self.content_changed,
        }
    }

    /// Takes in `part`, what a text call left of the part it copied
    /// ([`part`](Self::part)): its anchors and reported positions are
    /// added, but for those of an anchor it held when it was copied
    /// (`copied`) that was forgotten since, which stays forgotten; its
    /// carets, report count, kept selection and note replace these. The
    /// anchors it did not copy are left as they are.
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
        self.carets.clone_from(&part.carets);
        self.reports = part.reports;
        self.selection.clone_from(&part.selection);
        self.content_changed = part.content_changed;
    }

    /// Forgets the anchors `forgotten` names, with the positions reported
    /// in their chunks.
    fn forget_anchors(&mut self, forgotten: &HashSet<u64>) {
        self.anchors.retain(|number, _| !forgotten.contains(number));
        self.reported
            .retain(|(anchor, _), _| !forgotten.contains(anchor));
    }
}

/// A caret event's report of an edit control cut at its value change
/// check (`docs/design/ownership.md`): what the read part found, which the part that finishes it
/// compares and reports from. `P` is a position, an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReportCut<P> {
    /// The caret and the selection's ends, when something is selected.
    pub(crate) caret: P,
    pub(crate) selection: Option<(P, P)>,
    /// The caret is no longer where the read was asked to compare with.
    pub(crate) moved: bool,
    /// When the caret read finished, in milliseconds since the Unix epoch.
    pub(crate) read_at_ms: u64,
    /// The selection the report compares with.
    pub(crate) last: KeptSelection<P>,
}

/// A caret key's answer from an edit control cut at its value change check,
/// the evidence found: what the read part found, which the part that
/// finishes it answers from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnswerCut<P> {
    /// The caret and the selection's ends, when something is selected.
    pub(crate) caret: P,
    pub(crate) selection: Option<(P, P)>,
    /// When the caret read finished, in milliseconds since the Unix epoch.
    pub(crate) read_at_ms: u64,
    /// The caret's line, when the read part read it.
    pub(crate) line: Option<CutLine<P>>,
    /// The watch's unit.
    pub(crate) unit: TextUnit,
    /// Whether the caret landed on the line it was on, when the watch asks.
    pub(crate) same_line: Option<bool>,
    /// The selection the answer compares with.
    pub(crate) compared: KeptSelection<P>,
}

/// The caret's line as a cut read part read it: its ends, its UTF-16 text,
/// whether the text was cut at the limit, and the caret's offset in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CutLine<P> {
    pub(crate) start: P,
    pub(crate) end: P,
    pub(crate) text: Vec<u16>,
    pub(crate) truncated: bool,
    pub(crate) offset: usize,
}

/// One change of a node's text facts, as a program's text call makes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TextChange {
    /// What a text call left of the part of a UIA node's facts it copied,
    /// with the anchors it held ([`NodeAnchors::part`]).
    Uia {
        node: u64,
        part: NodeAnchors<RangeKey>,
        copied: BTreeSet<u64>,
    },
    /// The same for an edit control, its positions offsets.
    Edit {
        node: u64,
        part: NodeAnchors<u32>,
        copied: BTreeSet<u64>,
    },
    /// The selection the node's next caret report would be compared with
    /// is forgotten, through UIA or an edit control's messages.
    ForgetSelection { node: u64, uia: bool },
    /// The edit control's text changed, its value change handled, so its
    /// next comparison of selections is made in general terms.
    ContentChanged(u64),
    /// Everything of the node's text is forgotten: its runtime id names a
    /// new element, or its window was destroyed.
    Forget(u64),
}

impl TextChange {
    /// The node it changes.
    pub(crate) fn node(&self) -> u64 {
        match self {
            Self::Uia { node, .. }
            | Self::Edit { node, .. }
            | Self::ForgetSelection { node, .. }
            | Self::ContentChanged(node)
            | Self::Forget(node) => *node,
        }
    }

    /// Whether it adds to the node's facts, so it is applied only while
    /// the store holds the node.
    pub(crate) fn adds(&self) -> bool {
        matches!(
            self,
            Self::Uia { .. } | Self::Edit { .. } | Self::ContentChanged(_)
        )
    }
}

/// Every node's text facts, through UIA and through edit controls' messages.
#[derive(Clone, Debug, Default)]
pub(in crate::outpost) struct TextFacts {
    uia: HashMap<u64, Arc<NodeAnchors<RangeKey>>>,
    edit: HashMap<u64, Arc<NodeAnchors<u32>>>,
}

/// The keys `anchors` names.
fn keys(anchors: Option<&Arc<NodeAnchors<RangeKey>>>) -> BTreeSet<RangeKey> {
    anchors
        .into_iter()
        .flat_map(|anchors| anchors.positions().copied())
        .collect()
}

impl TextFacts {
    /// UIA node `node`'s facts, when it has any.
    pub(crate) fn uia(&self, node: u64) -> Option<&NodeAnchors<RangeKey>> {
        self.uia.get(&node).map(|anchors| &**anchors)
    }

    /// Edit control `node`'s facts, when it has any.
    pub(crate) fn edit(&self, node: u64) -> Option<&NodeAnchors<u32>> {
        self.edit.get(&node).map(|anchors| &**anchors)
    }

    /// Applies `change`. Returns the UIA ranges no fact names any more, by
    /// node and key: those it replaced or forgot, and those a text call
    /// minted for an anchor forgotten meanwhile, for the store to let go
    /// of.
    pub(crate) fn apply(&mut self, change: &TextChange) -> Vec<(u64, RangeKey)> {
        let node = change.node();
        let before = keys(self.uia.get(&node));
        let mut named = before.clone();
        match change {
            TextChange::Uia { part, copied, .. } => {
                named.extend(part.positions().copied());
                Arc::make_mut(self.uia.entry(node).or_default()).put(part, copied);
            }
            TextChange::Edit { part, copied, .. } => {
                Arc::make_mut(self.edit.entry(node).or_default()).put(part, copied);
            }
            TextChange::ForgetSelection { uia: true, .. } => {
                if let Some(anchors) = self.uia.get_mut(&node)
                    && anchors.selection.is_some()
                {
                    Arc::make_mut(anchors).selection = None;
                }
            }
            TextChange::ForgetSelection { uia: false, .. } => {
                if let Some(anchors) = self.edit.get_mut(&node)
                    && anchors.selection.is_some()
                {
                    Arc::make_mut(anchors).selection = None;
                }
            }
            TextChange::ContentChanged(_) => {
                Arc::make_mut(self.edit.entry(node).or_default()).content_changed = true;
            }
            TextChange::Forget(_) => {
                self.uia.remove(&node);
                self.edit.remove(&node);
            }
        }
        let after = keys(self.uia.get(&node));
        named.difference(&after).map(|&key| (node, key)).collect()
    }

    /// Forgets the nodes `nodes` and the anchors `anchors` a release freed.
    /// Returns the UIA ranges of the anchors no fact names any more, by
    /// node and key; a freed node's ranges the store let go of with it.
    pub(crate) fn release(
        &mut self,
        nodes: &HashSet<u64>,
        anchors: &HashSet<u64>,
    ) -> Vec<(u64, RangeKey)> {
        self.uia.retain(|node, _| !nodes.contains(node));
        self.edit.retain(|node, _| !nodes.contains(node));
        if anchors.is_empty() {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        for (&node, kept) in &mut self.uia {
            if !kept.anchors.keys().any(|number| anchors.contains(number)) {
                continue;
            }
            let before = keys(Some(kept));
            Arc::make_mut(kept).forget_anchors(anchors);
            let after = keys(Some(kept));
            dropped.extend(before.difference(&after).map(|&key| (node, key)));
        }
        for kept in self.edit.values_mut() {
            if kept.anchors.keys().any(|number| anchors.contains(number)) {
                Arc::make_mut(kept).forget_anchors(anchors);
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
            facts.release(&HashSet::new(), &HashSet::from([4])),
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
        part.selection = Some(KeptSelection {
            ends: (11, 12),
            collapsed: false,
            text: None,
            report: 1,
        });
        let first = TextChange::Uia {
            node: 1,
            part,
            copied: BTreeSet::new(),
        };
        assert_eq!(facts.apply(&first), Vec::new());
        let (mut part, copied) = facts.uia(1).expect("the node's facts").part(&[3]);
        part.reported.insert((3, 1), 13);
        part.selection = Some(KeptSelection {
            ends: (14, 14),
            collapsed: true,
            text: None,
            report: 2,
        });
        assert_eq!(
            facts.release(&HashSet::new(), &HashSet::from([3])),
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
            [14, 14]
        );
    }
}
