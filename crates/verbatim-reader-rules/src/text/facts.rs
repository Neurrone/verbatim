//! One node's text facts as a reader keeps them, the anchors it minted
//! with their chunks' text, the positions it reported, its one last read
//! and the reads held behind ids Core names
//! (`books/dev/src/reference/crates/verbatim-outpost-logic.md`). A position
//! `P` is the backend's own, or the key its store keeps it under: pure
//! data, which the text algorithms ([`super`]) work on and the outpost's
//! reducer stores.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use verbatim_model::{CaretSelection, TextChunk};

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
    /// The reads held behind ids Core names, numbered as anchors are: a
    /// caret key's starting point and a selection baseline
    /// (`books/dev/src/reference/crates/verbatim-outpost.md`, "Text and caret"). Each
    /// is released with the anchors, so it lives while Core holds it or a
    /// request names it.
    pub held: BTreeMap<u64, LastRead<P>>,
}

/// The selection of a read: its ends, and how the read reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadSelection<P> {
    /// Where it starts.
    pub start: P,
    /// Where it ends.
    pub end: P,
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
    /// The caret's line, with the caret at the chunk's offset, when the
    /// read read it: a batch's `Caret` reads no line.
    pub line: Option<TextChunk>,
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
                report: selection.report,
            }),
            None => None,
        };
        Some(LastRead {
            caret: position(self.caret)?,
            selection,
            line: self.line,
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
            },
            None => KeptSelection {
                ends: (self.caret.clone(), self.caret.clone()),
                collapsed: true,
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
}

impl<P> Default for NodeAnchors<P> {
    fn default() -> Self {
        Self {
            anchors: BTreeMap::new(),
            reported: BTreeMap::new(),
            last: None,
            held: BTreeMap::new(),
        }
    }
}

impl<P> KeptSelection<P> {
    /// The same selection with each end given by `end`.
    pub fn map<Q>(&self, mut end: impl FnMut(&P) -> Q) -> KeptSelection<Q> {
        KeptSelection {
            ends: (end(&self.ends.0), end(&self.ends.1)),
            collapsed: self.collapsed,
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
        let line = self
            .last
            .iter()
            .filter_map(|last| last.line.as_ref().map(|line| line.start.0));
        for number in named.iter().copied().chain(line) {
            if !copied.insert(number) {
                continue;
            }
            if let Some(anchor) = self.anchors.get(&number) {
                part.anchors.insert(number, anchor.clone());
            }
            if let Some(read) = self.held.get(&number) {
                part.held.insert(number, read.clone());
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
        let held = self.held.values().flat_map(LastRead::positions);
        anchors
            .chain(self.reported.values())
            .chain(last)
            .chain(held)
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
        let last = self.last.and_then(|last| last.filter_map(&mut position));
        let held = self
            .held
            .into_iter()
            .filter_map(|(number, read)| Some((number, read.filter_map(&mut position)?)))
            .collect();
        NodeAnchors {
            anchors,
            reported,
            last,
            held,
        }
    }

    /// Takes in `part`, what a text call left of the part it copied
    /// ([`part`](Self::part)): its anchors and reported positions are
    /// added, but for those of an anchor it held when it was copied
    /// (`copied`) that was forgotten since, which stays forgotten; its last
    /// read replaces this one. The anchors it did not copy are left as they
    /// are.
    pub fn put(&mut self, part: &Self, copied: &BTreeSet<u64>) {
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
        for (number, read) in &part.held {
            if !copied.contains(number) || self.held.contains_key(number) {
                self.held.insert(*number, read.clone());
            }
        }
        self.last.clone_from(&part.last);
    }

    /// Whether any of `numbers` is one of these anchors or held reads.
    pub fn holds_any(&self, numbers: &BTreeSet<u64>) -> bool {
        self.anchors
            .keys()
            .chain(self.held.keys())
            .any(|number| numbers.contains(number))
    }

    /// Forgets the anchors `forgotten` names, with the positions reported
    /// in their chunks, and the reads held under them.
    pub fn forget_anchors(&mut self, forgotten: &BTreeSet<u64>) {
        self.anchors.retain(|number, _| !forgotten.contains(number));
        self.reported
            .retain(|(anchor, _), _| !forgotten.contains(anchor));
        self.held.retain(|number, _| !forgotten.contains(number));
    }
}
