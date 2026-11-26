//! The one walk over the node ids a batch names, the one over the node
//! ids and text anchors its outcomes carry, and the one list of the text
//! anchors it names.
//!
//! Core stamps every node id arriving on an outpost's pipe with that
//! outpost's generation and clears it from every node id it sends
//! ([`crate::NodeId::with_outpost`], [`crate::NodeId::unstamped`]); an
//! outpost keeps every node and anchor a message carries until Core has
//! seen it ([`crate::protocol::Named`]); and Core keeps held every anchor
//! a request names while it waits or is out.

use super::{Batch, Before, Done, Missing, Op, Point, StepOutcome, Subject, Test, Through};
use crate::request::{KnownAncestor, NodeRead};
use crate::{NodeId, TextAnchor, TextPoint};

impl Batch {
    /// Calls `visit` on every node id the batch names.
    pub fn visit_nodes(&mut self, visit: &mut dyn FnMut(&mut NodeId)) {
        fn subject(subject: &mut Subject, visit: &mut dyn FnMut(&mut NodeId)) {
            if let Subject::Node(node) = subject {
                visit(node);
            }
        }
        fn known(known: &mut [KnownAncestor], visit: &mut dyn FnMut(&mut NodeId)) {
            for ancestor in known {
                if let KnownAncestor::Node(node) = ancestor {
                    visit(node);
                }
            }
        }
        for step in &mut self.steps {
            match &mut step.op {
                Op::Ancestors {
                    subject: object,
                    until,
                    ..
                } => {
                    subject(object, visit);
                    known(until, visit);
                }
                Op::FocusRead {
                    subject: object,
                    known: list,
                    ..
                } => {
                    subject(object, visit);
                    known(list, visit);
                }
                Op::IsOneOf {
                    subject: object,
                    equal,
                    at_address,
                } => {
                    subject(object, visit);
                    equal.iter_mut().chain(at_address).for_each(&mut *visit);
                }
                Op::Follow(target) => {
                    if let Some(target) = target {
                        subject(&mut target.subject, visit);
                        target.extra.iter_mut().for_each(&mut *visit);
                    }
                }
                Op::Read {
                    subject: object, ..
                }
                | Op::Test {
                    subject: object, ..
                }
                | Op::Related {
                    subject: object, ..
                }
                | Op::Activate { subject: object }
                | Op::Caret {
                    subject: object, ..
                }
                | Op::ReadText {
                    subject: object, ..
                }
                | Op::ReadRange {
                    subject: object, ..
                }
                | Op::CompareSelection {
                    subject: object, ..
                }
                | Op::Select {
                    subject: object, ..
                }
                | Op::Locate {
                    subject: object, ..
                }
                | Op::MoveBySentence {
                    subject: object, ..
                }
                | Op::Descendants {
                    subject: object, ..
                }
                | Op::Visible { subject: object }
                | Op::Move {
                    subject: object, ..
                }
                | Op::FindText {
                    subject: object, ..
                }
                | Op::ReadColours {
                    subject: object, ..
                }
                | Op::WaitForCaret {
                    subject: object, ..
                } => subject(object, visit),
            }
        }
        if let Some((node, _)) = &mut self.spare {
            visit(node);
        }
    }

    /// The nodes whose text the batch reads or moves, each with the text
    /// anchors its step names there, in order: each text operation's node
    /// subject with the anchors its points, baseline or starting point
    /// name, with those its condition compares, then the spare's node and
    /// anchor. A step names an anchor only
    /// on a node subject, which the parse checks
    /// ([`super::Malformed::AnchorWithoutNode`]) and Core's builder writes
    /// only so.
    #[must_use]
    pub fn anchors(&self) -> Vec<(NodeId, Vec<TextAnchor>)> {
        fn point(point: Point) -> Option<TextAnchor> {
            match point {
                Point::At(TextPoint::At(position)) => Some(position.anchor),
                Point::At(_) | Point::Of(..) => None,
            }
        }
        fn ends(start: Point, end: Point, through: Option<Through>) -> Vec<TextAnchor> {
            [start, end]
                .into_iter()
                .chain(
                    through
                        .into_iter()
                        .flat_map(|through| [through.start, through.end]),
                )
                .filter_map(point)
                .collect()
        }
        let mut anchors = Vec::new();
        for step in &self.steps {
            let Some(Subject::Node(node)) = step.op.subject() else {
                continue;
            };
            if !step.op.kind().is_text() {
                continue;
            }
            let mut named: Vec<TextAnchor> = match &step.op {
                Op::ReadText { at, .. }
                | Op::Locate { at, .. }
                | Op::Move { at, .. }
                | Op::FindText { at, .. } => point(*at).into_iter().collect(),
                Op::ReadRange {
                    start,
                    end,
                    through,
                    ..
                }
                | Op::Select {
                    start,
                    end,
                    through,
                    ..
                } => ends(*start, *end, *through),
                Op::CompareSelection {
                    before: Before::Baseline(baseline),
                    ..
                } => vec![*baseline],
                Op::MoveBySentence { from, .. } => vec![*from],
                Op::WaitForCaret { wait, .. } => vec![wait.from],
                Op::CompareSelection {
                    before: Before::SelectionOf(_),
                    ..
                }
                | Op::Read { .. }
                | Op::Test { .. }
                | Op::Related { .. }
                | Op::Ancestors { .. }
                | Op::Descendants { .. }
                | Op::IsOneOf { .. }
                | Op::Activate { .. }
                | Op::Follow(_)
                | Op::FocusRead { .. }
                | Op::Caret { .. }
                | Op::Visible { .. }
                | Op::ReadColours { .. } => Vec::new(),
            };
            if let Some(Test::NotAfter { point: at, than }) =
                step.when.map(|condition| condition.test)
            {
                named.extend([at, than].into_iter().filter_map(point));
            }
            anchors.push((*node, named));
        }
        anchors.extend(self.spare.map(|(node, anchor)| (node, vec![anchor])));
        anchors
    }
}

/// Calls `nodes` on every node id, and `anchors` on every text anchor,
/// that `outcomes` carry, in order.
pub fn visit_outcomes(
    outcomes: &mut [StepOutcome],
    nodes: &mut dyn FnMut(&mut NodeId),
    anchors: &mut dyn FnMut(&mut TextAnchor),
) {
    fn read(read: &mut NodeRead, nodes: &mut dyn FnMut(&mut NodeId)) {
        if let Some(node) = &mut read.node {
            nodes(node);
        }
    }
    for outcome in outcomes {
        let done = match outcome {
            StepOutcome::Done(done) => done,
            StepOutcome::NotThere(Missing::Node(node)) => {
                nodes(node);
                continue;
            }
            StepOutcome::NotThere(Missing::Anchor(anchor)) => {
                anchors(anchor);
                continue;
            }
            StepOutcome::NotThere(Missing::Object)
            | StepOutcome::Refused(_)
            | StepOutcome::Failed
            | StepOutcome::Skipped => continue,
        };
        match done {
            Done::Read(node) => read(node, nodes),
            Done::Test(tested) => read(&mut tested.node, nodes),
            Done::Related(list) => list.iter_mut().for_each(|node| read(node, nodes)),
            Done::Ancestors(ancestry) => ancestry
                .ancestors
                .iter_mut()
                .for_each(|node| read(node, nodes)),
            Done::Descendants(descended) => descended
                .nodes
                .iter_mut()
                .for_each(|descendant| read(&mut descendant.node, nodes)),
            Done::FocusRead(focus) => focus.visit(nodes, anchors),
            Done::Caret(caret) => {
                anchors(&mut caret.caret);
                if let Some(selection) = &mut caret.selection {
                    anchors(&mut selection.start);
                    anchors(&mut selection.end);
                }
                anchors(&mut caret.held);
            }
            Done::ReadText(read) => read
                .chunks
                .iter_mut()
                .for_each(|chunk| anchors(&mut chunk.start)),
            Done::MoveBySentence(moved) => match moved {
                crate::SentenceMoved::Moved { line, baseline, .. }
                | crate::SentenceMoved::StayedAtStart { line, baseline, .. }
                | crate::SentenceMoved::End { line, baseline } => {
                    anchors(&mut line.start);
                    anchors(baseline);
                }
            },
            Done::Visible(visible) => {
                anchors(&mut visible.start);
                anchors(&mut visible.end);
            }
            Done::Move(moved) => {
                anchors(&mut moved.from);
                anchors(&mut moved.to);
            }
            Done::IsOneOf(_)
            | Done::Activate(_)
            | Done::Follow
            | Done::ReadRange(_)
            | Done::CompareSelection(_)
            | Done::Select(_)
            | Done::Locate(_)
            | Done::WaitForCaret(_)
            | Done::FindText(_)
            | Done::ReadColours(_) => {}
        }
    }
}
