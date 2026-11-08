//! Core's focus, as NVDA's one ordered focus queue (`docs/nvda/events.md`,
//! "What a focus event actually does"; `books/dev/src/reference/adr/0001-focus.md`,
//! "One ordered focus queue").
//!
//! Two facts, as NVDA keeps them:
//!
//! - The focus object, the focus last executed, which commands act on.
//! - The last queued focus, the newest report decided in the order, which
//!   the duplicate rule, the whole-window refusal and the left mark judge:
//!   the focus object itself, or a report that ended unanswered ([`Last`]).
//!
//! Reports reach the queue in the order they reach Core, and each is
//! decided at the queue's front, once every report ahead of it is decided:
//! judged against the last queued focus (refused, a duplicate, or judged
//! left or not), planned against the focus object, and executed, or set
//! aside for a live re-read (`crate::live_reread`), which every report
//! behind it waits for. So a report behind a waiting one asks its own live
//! re-read with the input that ends the wait ahead of it.
//!
//! The left mark lives on the last queued focus (Dickson, 2026-11-08): a
//! foreground change applies to it when the change was observed after it
//! or reached Core after it, and applies to a report that was observed
//! after the change and reached Core after it ([`Change`]).

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{
    EarlierElement, FirstCachedAncestor, FocusGroup, FocusReaderKind, FocusText, ForegroundWindow,
    NodeSnapshot, OutpostId, Pid, TraceId, WindowFacts, WindowHandle,
};

use crate::state::FocusContext;

/// Core's focus ([module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FocusState {
    /// The focus object: the focus last executed, none before the first.
    object: Option<FocusContext>,
    /// The last queued focus.
    last: Last,
    /// The reports not yet decided, and the inputs that reached Core behind
    /// them, in the order they reached Core; only the first can be waiting
    /// for its live re-read.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    queue: VecDeque<Queued>,
    /// The foreground changes that reached Core and may still apply to a
    /// report, in the order observed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changes: Vec<Change>,
    /// How many of the inputs the queue orders Core has taken: reports,
    /// foreground changes, and the endings of live re-reads.
    #[serde(default)]
    inputs: u64,
}

/// What the last queued focus is ([`FocusState`]), with when it reached
/// Core, by the queue's count of inputs, and, for a report that ended
/// unanswered, the input that ended its wait for its live re-read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Last {
    /// No report has been decided.
    #[default]
    Nothing,
    /// The focus object is the last queued focus.
    Executed { arrival: u64 },
    /// A report that ended unanswered is the last queued focus: gone,
    /// refused as background when answered, or its reader ended. It is
    /// never spoken.
    Unanswered {
        report: Box<Report>,
        arrival: u64,
        waited: u64,
    },
}

/// A report as Core keeps it until it is decided: the parts of a focus
/// event the focus handling reads, owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) trace_id: TraceId,
    pub(crate) source: Pid,
    /// When its reader observed it, in microseconds; 0 when not known.
    pub(crate) observed_at_us: u64,
    pub(crate) window: Option<WindowFacts>,
    pub(crate) node: NodeSnapshot,
    pub(crate) whole_window: bool,
    /// Its ancestors as Core takes them, outermost first.
    pub(crate) ancestors: Arc<[NodeSnapshot]>,
    pub(crate) selected_child: Option<NodeSnapshot>,
    pub(crate) text: Option<FocusText>,
    pub(crate) earlier: Option<EarlierElement>,
    pub(crate) group: Option<FocusGroup>,
    /// False once the outpost generation that issued its node ids ended.
    pub(crate) alive: bool,
}

/// A report not yet decided ([`FocusState::queue`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    /// The report, or its live re-read's answer once that came.
    pub(crate) report: Report,
    /// What the report was read for, which decides the duplicate rule.
    pub(crate) purpose: Option<FocusPurpose>,
    /// When it reached Core, by the queue's count of inputs.
    pub(crate) arrival: u64,
    pub(crate) state: EntryState,
}

/// What waits in the queue ([`FocusState::queue`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Queued {
    /// A focus report.
    Report(Box<Entry>),
    /// A key command or script, or a menu's or the switcher's end, that
    /// reached Core while reports waited: handled in its turn, as NVDA
    /// queues gestures with events (Dickson, 2026-11-08).
    Input(Box<verbatim_model::Input>),
}

impl Queued {
    fn report(&self) -> Option<&Entry> {
        match self {
            Self::Report(entry) => Some(entry.as_ref()),
            Self::Input(_) => None,
        }
    }

    fn report_mut(&mut self) -> Option<&mut Entry> {
        match self {
            Self::Report(entry) => Some(entry.as_mut()),
            Self::Input(_) => None,
        }
    }
}

/// Where a report not yet decided is in its lifecycle ([`Entry`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum EntryState {
    /// Waiting for the reports ahead of it to be decided; `cached` names
    /// where its cached ancestors begin.
    Pending { cached: Option<FirstCachedAncestor> },
    /// At the front, judged `left` or not, waiting for its live re-read.
    Rereading {
        left: bool,
        reader: (OutpostId, FocusReaderKind),
        request: u64,
    },
    /// Its live re-read answered: executed now.
    Answered { left: bool },
    /// Its live re-read ended unanswered at input `ended`.
    Unanswered { ended: u64 },
}

/// A foreground change that reached Core, as the left mark keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Change {
    pub(crate) observed_at_us: u64,
    pub(crate) window: WindowHandle,
    pub(crate) root_owner: WindowHandle,
    /// When it reached Core, by the queue's count of inputs.
    pub(crate) arrival: u64,
}

impl Change {
    /// Whether the change is into a window unrelated to `facts`: neither its
    /// top-level window nor of its root owner.
    fn leaves(&self, facts: &WindowFacts) -> bool {
        self.window != facts.top_level && self.root_owner != facts.root_owner
    }
}

/// The last queued focus as the rules judge it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LastQueued<'a> {
    pub(crate) node: &'a NodeSnapshot,
    pub(crate) window: Option<&'a WindowFacts>,
    observed_at_us: u64,
    arrival: u64,
    /// The input that ended its wait for its live re-read, for a report
    /// that ended unanswered: a repeat that reached Core before then is no
    /// duplicate of it, since its reader answered it gone for the repeat.
    /// A report answered and executed carries no such exception.
    pub(crate) waited: Option<u64>,
}

/// What the queue's front needs next ([`FocusState::front`]).
pub(crate) enum Front {
    /// A report to judge and plan, taken out of the queue: put it back
    /// with [`FocusState::reread_first`] if it is set aside.
    Decide(Box<Entry>, Option<FirstCachedAncestor>),
    /// A report answered live, taken out: execute it, judged `left` or not,
    /// with its arrival.
    Execute(Box<Report>, bool, u64),
    /// An input that waited behind reports, taken out: handle it now.
    Input(Box<verbatim_model::Input>),
    /// Nothing can be decided now.
    Wait,
}

impl FocusState {
    /// The focus object, which commands act on.
    pub(crate) fn object(&self) -> Option<&FocusContext> {
        self.object.as_ref()
    }

    /// The focus object, to change in place.
    pub(crate) fn object_mut(&mut self) -> Option<&mut FocusContext> {
        self.object.as_mut()
    }

    /// The report that reached Core at input `arrival` was executed as
    /// `focus`: it is the focus object and the last queued focus.
    pub(crate) fn execute(&mut self, focus: FocusContext, arrival: u64) {
        self.object = Some(focus);
        self.decided(Last::Executed { arrival });
    }

    /// `last` is the last queued focus now.
    fn decided(&mut self, last: Last) {
        self.last = last;
    }

    /// Lets go of the changes that can apply to no report Core has or can
    /// still accept, so that only those still to be read are kept: those
    /// that do not apply to the last queued focus and were observed before
    /// `floor`, the observation before which no report Core receives from
    /// now on can be accepted, and before every report not yet decided,
    /// either of which could become the last queued focus. With no floor,
    /// no report has been accepted from any generation, and none is let go.
    pub(crate) fn let_go_of_changes(&mut self, floor: Option<u64>) {
        let Some(floor) = floor else {
            return;
        };
        let floor = self
            .queue
            .iter()
            .filter_map(Queued::report)
            .map(|entry| entry.report.observed_at_us)
            .fold(floor, u64::min);
        let (observed, arrival) = self
            .last_queued()
            .map_or((0, 0), |last| (last.observed_at_us, last.arrival));
        self.changes.retain(|change| {
            change.observed_at_us >= floor
                || (observed != 0 && change.observed_at_us > observed)
                || change.arrival > arrival
        });
    }

    /// With nothing waiting, only whether each change reached Core before
    /// or after the last queued focus matters, and no later report can
    /// arrive before a wait ended: the count of inputs starts again, so the
    /// state does not grow with the inputs taken.
    pub(crate) fn restart_count_when_idle(&mut self) {
        if !self.queue.is_empty() {
            return;
        }
        let arrival = match &mut self.last {
            Last::Nothing => 0,
            Last::Executed { arrival } => {
                let was = *arrival;
                *arrival = 0;
                was
            }
            Last::Unanswered {
                arrival, waited, ..
            } => {
                let was = *arrival;
                *arrival = 0;
                *waited = 0;
                was
            }
        };
        for change in &mut self.changes {
            change.arrival = u64::from(change.arrival > arrival);
        }
        self.inputs = 1;
    }

    /// The last queued focus, if any.
    pub(crate) fn last_queued(&self) -> Option<LastQueued<'_>> {
        match &self.last {
            Last::Nothing => None,
            Last::Executed { arrival } => self.object.as_ref().map(|focus| LastQueued {
                node: &focus.snapshot,
                window: focus.window.as_ref(),
                observed_at_us: focus.observed_at_us,
                arrival: *arrival,
                waited: None,
            }),
            Last::Unanswered {
                report,
                arrival,
                waited,
            } => Some(LastQueued {
                node: &report.node,
                window: report.window.as_ref(),
                observed_at_us: report.observed_at_us,
                arrival: *arrival,
                waited: Some(*waited),
            }),
        }
    }

    /// The window the last queued focus was left for, for a report observed
    /// at `observed_at_us` (0 for unknown) that reached Core at input
    /// `arrival`: the newest change, in the order observed, that applies to
    /// the last queued focus and to that report, into a window unrelated to
    /// the last queued focus. A focus with no window facts is never left.
    pub(crate) fn left_for(&self, observed_at_us: u64, arrival: u64) -> Option<WindowHandle> {
        let last = self.last_queued()?;
        let facts = last.window?;
        self.changes
            .iter()
            .filter(|change| {
                change.arrival < arrival
                    && (observed_at_us == 0 || change.observed_at_us < observed_at_us)
                    && ((last.observed_at_us != 0 && change.observed_at_us > last.observed_at_us)
                        || change.arrival > last.arrival)
                    && change.leaves(facts)
            })
            .max_by_key(|change| change.observed_at_us)
            .map(|change| change.window)
    }

    /// The verdict for the report `entry`: whether the last queued focus was
    /// left for a window other than the report's own.
    pub(crate) fn left_against(&self, entry: &Entry) -> bool {
        self.left_for(entry.report.observed_at_us, entry.arrival)
            .is_some_and(|left_for| {
                entry
                    .report
                    .window
                    .as_ref()
                    .is_none_or(|window| left_for != window.top_level)
            })
    }

    /// Whether the last queued focus is marked left by every change that
    /// reached Core, for a report yet to come.
    pub(crate) fn last_left(&self) -> bool {
        self.left_for(0, u64::MAX).is_some()
    }

    /// Whether the focus object is the last queued focus and marked left.
    pub(crate) fn object_left(&self) -> bool {
        matches!(self.last, Last::Executed { .. }) && self.last_left()
    }

    /// Whether the focus object is the last queued focus, not left.
    pub(crate) fn applied_unmarked(&self) -> bool {
        matches!(self.last, Last::Executed { .. }) && !self.last_left()
    }

    /// The next input the queue orders: its number.
    fn next_input(&mut self) -> u64 {
        self.inputs = self.inputs.saturating_add(1);
        self.inputs
    }

    /// A foreground change reached Core. One into Verbatim's own windows or
    /// with no observation time leaves nothing.
    pub(crate) fn foreground_changed(&mut self, change: &ForegroundWindow) {
        let arrival = self.next_input();
        if change.own || change.observed_at_us == 0 {
            return;
        }
        self.changes.push(Change {
            observed_at_us: change.observed_at_us,
            window: change.window,
            root_owner: change.root_owner,
            arrival,
        });
    }

    /// `report`, read for `purpose`, with its cached ancestors from
    /// `cached`, reached Core and was accepted into its order: it waits
    /// behind the reports not yet decided.
    pub(crate) fn queue(
        &mut self,
        report: Report,
        purpose: Option<FocusPurpose>,
        cached: Option<FirstCachedAncestor>,
    ) {
        let arrival = self.next_input();
        self.queue.push_back(Queued::Report(Box::new(Entry {
            report,
            purpose,
            arrival,
            state: EntryState::Pending { cached },
        })));
    }

    /// Whether reports wait, so that a key command, a script or a menu's
    /// end reaching Core now is handled after them.
    pub(crate) fn waiting(&self) -> bool {
        !self.queue.is_empty()
    }

    /// `input` reached Core while reports wait: it is handled in its turn.
    pub(crate) fn queue_input(&mut self, input: verbatim_model::Input) {
        self.queue.push_back(Queued::Input(Box::new(input)));
    }

    /// What the queue's front needs: a report to decide, or one answered to
    /// execute, taken out of the queue; a report that ended unanswered is
    /// the last queued focus, and the next is looked at.
    pub(crate) fn front(&mut self) -> Front {
        loop {
            let entry = match self.queue.pop_front() {
                None => return Front::Wait,
                Some(Queued::Input(input)) => return Front::Input(input),
                Some(Queued::Report(entry)) => *entry,
            };
            match entry.state {
                EntryState::Pending { cached } => {
                    return Front::Decide(Box::new(entry), cached);
                }
                EntryState::Answered { left } => {
                    return Front::Execute(Box::new(entry.report), left, entry.arrival);
                }
                EntryState::Unanswered { ended } => self.decided(Last::Unanswered {
                    report: Box::new(entry.report),
                    arrival: entry.arrival,
                    waited: ended,
                }),
                // A report waiting for its live re-read stays at the front,
                // and everything behind it waits.
                EntryState::Rereading { .. } => {
                    self.queue.push_front(Queued::Report(Box::new(entry)));
                    return Front::Wait;
                }
            }
        }
    }

    /// `entry`, taken from the front and judged `left` or not, waits at the
    /// front for live re-read `request` of `reader`.
    pub(crate) fn reread_first(
        &mut self,
        mut entry: Entry,
        left: bool,
        (reader, request): ((OutpostId, FocusReaderKind), u64),
    ) {
        entry.state = EntryState::Rereading {
            left,
            reader,
            request,
        };
        self.queue.push_front(Queued::Report(Box::new(entry)));
    }

    /// `entry`, taken from the front, must wait for a live re-read but its
    /// outpost generation has ended, so none can be asked: it ends
    /// unanswered now and is the last queued focus, unspoken.
    pub(crate) fn unanswered_now(&mut self, entry: Entry) {
        let waited = self.inputs;
        self.decided(Last::Unanswered {
            report: Box::new(entry.report),
            arrival: entry.arrival,
            waited,
        });
    }

    /// The report waiting for live re-read `request` asked through outpost
    /// generation `outpost`.
    fn rereading_mut(&mut self, outpost: OutpostId, request: u64) -> Option<&mut Entry> {
        self.queue
            .front_mut()
            .and_then(Queued::report_mut)
            .filter(|entry| {
                matches!(entry.state, EntryState::Rereading { reader, request: asked, .. }
                    if reader.0 == outpost && asked == request)
            })
    }

    /// Whether a report waits for live re-read `request` asked through
    /// outpost generation `outpost`.
    pub(crate) fn waits_for(&mut self, outpost: OutpostId, request: u64) -> bool {
        self.rereading_mut(outpost, request).is_some()
    }

    /// The live re-read `request` was answered with `answer`: its report is
    /// executed with the answer's reading and its own verdict.
    pub(crate) fn answered(&mut self, outpost: OutpostId, request: u64, answer: Report) {
        self.next_input();
        if let Some(entry) = self.rereading_mut(outpost, request)
            && let EntryState::Rereading { left, .. } = entry.state
        {
            entry.report = answer;
            entry.state = EntryState::Answered { left };
        }
    }

    /// The report waiting for live re-read `request` ends unanswered.
    pub(crate) fn unanswered(&mut self, outpost: OutpostId, request: u64) {
        let ended = self.next_input();
        if let Some(entry) = self.rereading_mut(outpost, request) {
            entry.state = EntryState::Unanswered { ended };
        }
    }

    /// The report waiting for a live re-read of a reader `ended` names ends
    /// unanswered.
    pub(crate) fn readers_ended(&mut self, ended: impl Fn((OutpostId, FocusReaderKind)) -> bool) {
        let at = self.next_input();
        if let Some(entry) = self.queue.front_mut().and_then(Queued::report_mut)
            && let EntryState::Rereading { reader, .. } = entry.state
            && ended(reader)
        {
            entry.state = EntryState::Unanswered { ended: at };
        }
    }

    /// Outpost generation `outpost` ended: the focus and the reports it
    /// issued keep their data with their ids dead, and a report waiting for
    /// its readers' live re-read ends unanswered.
    pub(crate) fn outpost_ended(&mut self, outpost: OutpostId) {
        if let Some(focus) = self.object.as_mut()
            && focus.snapshot.id.outpost() == outpost
        {
            focus.alive = false;
        }
        self.readers_ended(|reader| reader.0 == outpost);
        if let Last::Unanswered { report, .. } = &mut self.last
            && report.node.id.outpost() == outpost
        {
            report.alive = false;
        }
        for entry in self.queue.iter_mut().filter_map(Queued::report_mut) {
            if entry.report.node.id.outpost() == outpost {
                entry.report.alive = false;
            }
        }
    }

    /// How many reports wait for their live re-reads.
    pub(crate) fn rereading(&self) -> usize {
        // Only the front can wait for its live re-read.
        usize::from(
            self.queue
                .front()
                .and_then(Queued::report)
                .is_some_and(|entry| matches!(entry.state, EntryState::Rereading { .. })),
        )
    }

    /// The changes and the count of inputs, for a test that compares
    /// everything else across a foreground change.
    #[cfg(test)]
    pub(crate) fn take_changes_of(&mut self, other: &Self) {
        self.changes.clone_from(&other.changes);
        self.inputs = other.inputs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed dump reports its own error.
    #[test]
    fn a_malformed_current_dump_reports_its_own_error() {
        let error = serde_json::from_value::<FocusState>(
            serde_json::json!({"object": null, "last": "Bogus"}),
        )
        .expect_err("a malformed dump is refused");
        assert_eq!(
            error.to_string(),
            "unknown variant `Bogus`, expected one of `Nothing`, `Executed`, `Unanswered`"
        );
    }
}
