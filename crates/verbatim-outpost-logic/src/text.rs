//! Text support per node, as the outpost's object store keeps it
//! (`books/dev/src/explanation/design/ownership.md`): each node's anchors with their chunks' text, the positions the
//! outpost reported, and the node's one last read, the only baseline a
//! caret key or a comparison of selections has
//! (`books/dev/src/reference/crates/verbatim-outpost-logic.md`). A position
//! is a fact of its own kind: an edit control's
//! offset, or the key of a UIA range the store keeps beside these, so no
//! application object is held here and an edit control's offsets cross
//! between programs as they are. No rule of the reducer reads them, so the
//! store holds them; the type is pure data, here so that it is tested on
//! any host.
//!
//! A program reads a copy of them, taken as it starts; its text calls
//! change them only through [`TextChange`]s, which it applies to its own
//! copy as it makes them and commits for the shell to write to the store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use verbatim_reader_rules::text::facts::{
    Anchor, KeptSelection, LastRead, NodeAnchors, ReadSelection,
};

/// The key the store keeps a UIA position's range under.
pub type RangeKey = u64;

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
    /// Everything of the node's text is forgotten: its runtime id names a
    /// new element, or its window was destroyed.
    Forget(u64),
}

impl TextChange {
    /// The node it changes.
    #[must_use]
    pub fn node(&self) -> u64 {
        match self {
            Self::Uia { node, .. } | Self::Edit { node, .. } | Self::Forget(node) => *node,
        }
    }

    /// Whether it adds to the node's facts, so it is applied only while
    /// the store holds the node.
    #[must_use]
    pub fn adds(&self) -> bool {
        matches!(self, Self::Uia { .. } | Self::Edit { .. })
    }
}

/// One node's text facts, of one provider.
#[derive(Clone, Debug)]
enum NodeRecord {
    Uia(Arc<NodeAnchors<RangeKey>>),
    Edit(Arc<NodeAnchors<u32>>),
}

/// Every node's text facts, through UIA and through edit controls'
/// messages, so one place names every range the store must keep: a node
/// has an entry once a read has made its facts.
#[derive(Clone, Debug, Default)]
pub struct TextFacts {
    nodes: BTreeMap<u64, NodeRecord>,
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
        match self.nodes.get(&node)? {
            NodeRecord::Uia(anchors) => Some(&**anchors),
            NodeRecord::Edit(_) => None,
        }
    }

    /// Edit control `node`'s facts, when it has any.
    #[must_use]
    pub fn edit(&self, node: u64) -> Option<&NodeAnchors<u32>> {
        match self.nodes.get(&node)? {
            NodeRecord::Edit(anchors) => Some(&**anchors),
            NodeRecord::Uia(_) => None,
        }
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
                    if !kept.holds_any(anchors) {
                        continue;
                    }
                    let before: BTreeSet<RangeKey> = kept.positions().copied().collect();
                    Arc::make_mut(kept).forget_anchors(anchors);
                    let after: BTreeSet<RangeKey> = kept.positions().copied().collect();
                    dropped.extend(before.difference(&after).map(|&key| (node, key)));
                }
                NodeRecord::Edit(kept) => {
                    if kept.holds_any(anchors) {
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
    use verbatim_model::TextUnit;

    use super::*;
    use verbatim_model::{CaretSelection, TextChunk};

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
                },
            }),
            line: Some(TextChunk {
                unit: TextUnit::Line,
                text: String::new(),
                start: verbatim_model::TextAnchor(1),
                offset: 0,
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats: Vec::new(),
            }),
            read_at_us: 0,
            unit: None,
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

    /// A held read (a starting point or a selection baseline) is copied
    /// only when a call names it, survives a call that did not copy it,
    /// and is released with the anchors, its UIA ranges handed back
    /// (`books/dev/src/reference/crates/verbatim-outpost-logic.md`, "The reducer").
    #[test]
    fn a_held_read_is_copied_when_named_and_released_as_an_anchor() {
        let mut facts = TextFacts::default();
        let mut part = NodeAnchors::default();
        part.held.insert(5, last_read(20, Some((20, 21))));
        part.held.insert(7, last_read(30, None));
        let first = TextChange::Uia {
            node: 1,
            part,
            copied: BTreeSet::new(),
        };
        assert_eq!(facts.apply(&first), Vec::new());
        let kept = facts.uia(1).expect("the node's facts");
        let (named, copied) = kept.part(&[5]);
        assert_eq!(named.held.keys().copied().collect::<Vec<_>>(), [5]);
        assert_eq!(copied, BTreeSet::from([5]));
        let (unnamed, _) = kept.part(&[]);
        assert_eq!(unnamed.held, BTreeMap::new());
        let put = TextChange::Uia {
            node: 1,
            part: unnamed,
            copied: BTreeSet::new(),
        };
        assert_eq!(facts.apply(&put), Vec::new());
        assert_eq!(
            facts
                .uia(1)
                .expect("the node's facts")
                .held
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [5, 7]
        );
        assert_eq!(
            facts.release(&BTreeSet::new(), &BTreeSet::from([5])),
            vec![(1, 20), (1, 21)]
        );
        assert_eq!(
            facts
                .uia(1)
                .expect("the node's facts")
                .held
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [7]
        );
    }
}
