//! The outpost's commit point (`docs/design/focus-pipeline-rework.md`,
//! "The outpost's commit point"): the one place where the workers and the
//! watchdog change what the outpost follows and what it has told Core of
//! it, its follow state.
//!
//! A commit is one call, [`Watch::commit`](super::worker::Watch), made
//! under the watch lock: it checks that the committing worker generation
//! is in charge and only then applies an [`Effects`] value, recording its
//! report for Core's judgement first, then sending its messages, then
//! applying the rest. The follow state that only worker threads read lives
//! here, in [`FollowState`], inside the watch's state, so a commit takes one
//! lock; the facts other threads read stay in their own structures, whose
//! locks a commit takes nested inside the watch lock, in this order: the
//! judgement list, the intake, the outbound queue, the UI Automation
//! registry, the MSAA registry (the release of nodes takes both registries
//! inside the outbound queue's lock). Nothing holding any of them takes the
//! watch lock or calls into the application, and no lock a commit takes is
//! held across a call. The text caches come before the watch lock: they may
//! be held when it is taken, and a commit never takes them.
//!
//! One change of follow state is made outside a commit, with the same
//! in-charge check under the watch lock: the release of the nodes Core no
//! longer holds (`Worker::release`), which must take the outbound queue's
//! lock and the registries' inside it, and removes the released nodes'
//! terminals.
//!
//! What a commit displaces that may hold an application's object, a
//! terminal entry replaced or an element the subscription is not moved to,
//! is returned as [`Displaced`] and dropped once the watch lock is let go,
//! since a `Release` can call into the application; what a watchdog's
//! publication displaces is left in the follow state for the worker in
//! charge to drop after its next commit, never on the watchdog's thread.
//!
//! An [`Effects`] value is built before the commit with every call it
//! needs made, so the apply code takes the state and the nested locks
//! only: it computes nothing an accessor of the moved state would, which
//! would take the watch lock again.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use verbatim_model::{CaretReport, NodeId, NodeSnapshot, Role, StateSet, TextReply, TraceId};
use verbatim_uia::{Agile, Group, Kept};

use crate::protocol::{EventTiming, OutpostToSupervisor, QueryOutcome, QueryResult, now_us};

use super::Context;
use super::focus_path::FocusPath;
use super::held;
use super::intake::Object;
use super::read::DialogMemo;
use super::text_reads::OpenWatch;

/// What the worker remembers between entries, across worker replacements:
/// the focus it follows and the facts about it the menu rules and the
/// walks read. A cache of reads and of what was reported, which judges
/// nothing Core was told.
#[derive(Default)]
pub(super) struct Tracking {
    /// The role of the focus or foreground window this outpost last
    /// reported, which the menu rules compare with; the focus's own role is
    /// `focus_role`.
    pub(super) role: Option<Role>,
    /// The batch in which a focus was last reported.
    pub(super) batch: Option<u64>,
    /// The last focus this outpost reported and its ancestors, outermost
    /// first: where the next focus's ancestor walk can stop and reuse the
    /// rest, as NVDA's does.
    pub(super) chain: Vec<NodeSnapshot>,
    /// The window of the last focus this outpost reported, for events on
    /// that focus, which then need no call to find their window.
    pub(super) window: Option<isize>,
    /// The node of the last focus this outpost reported, which it follows:
    /// its caret, text and terminal output are read, and the
    /// focus-following subscription is on it. The rules that judge events
    /// before they are read judge them against the focus Core holds
    /// instead ([`super::held`]).
    pub(super) focus: Option<NodeId>,
    /// That focus's role, as last read, which decides how its text is
    /// read.
    pub(super) focus_role: Option<Role>,
    /// The dialog the last foreground report gathered text for, which the
    /// focus that follows it reuses.
    pub(super) dialog: DialogMemo,
    /// The UIA focus whose value is its first selected item's name, as
    /// the listener read it (`UiaFocusRead::value_from_selection`): the
    /// selection of one of its items is a change of its value.
    pub(super) value_from_selection: Option<NodeId>,
}

/// The node whose caret was last read in a committed read, and when that
/// read began, in microseconds: a caret event observed before then changes
/// nothing the read did not see, and one observed later (while the read
/// was in flight included) may.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CaretRead(Option<(u64, u64)>);

impl CaretRead {
    /// Notes a read of node `node`'s caret that began at `started_us`.
    pub(super) fn note(&mut self, node: u64, started_us: u64) {
        self.0 = Some((node, started_us));
    }

    /// Whether the last read of node `node`'s caret began at or after
    /// `observed_us`, so it saw what an event observed then reports.
    pub(super) fn covers(self, node: u64, observed_us: u64) -> bool {
        self.0
            .is_some_and(|(read, at)| read == node && at >= observed_us)
    }
}

/// What the followed focus still needs after the commit that made it
/// followed: owed work, attempted once per worker by the worker in charge
/// under a deadline of its own.
#[derive(Clone, Debug)]
pub(super) struct Owed {
    /// The focus that owes it.
    pub(super) node: NodeId,
    /// What is owed.
    pub(super) work: OwedWork,
    /// The trace and observation of the event the focus came from.
    pub(super) trace: TraceId,
    pub(super) observed_at_us: u64,
    /// The worker generation attempting it, once one has started to.
    pub(super) attempting: Option<u64>,
}

/// The work a followed focus owes ([`Owed`]).
#[derive(Clone, Debug)]
pub(super) enum OwedWork {
    /// A terminal's first read, with the caret read with its report, or
    /// `None` for a terminal published at its walk's bound, whose caret is
    /// read first.
    FirstRead(Option<CaretReport>),
    /// A `SensitiveSlider`'s lookup, for a thumb published at its walk's
    /// bound, whose slider's element this outpost does not keep.
    Slider,
}

/// The follow state that only worker threads read, kept under the watch
/// lock and changed only in a commit.
#[derive(Default)]
pub(super) struct FollowState {
    /// The focus this outpost follows.
    pub(super) tracking: Tracking,
    /// The focus's node and group the focus-following subscription was
    /// last moved to, with the focus's parent followed for its value
    /// changes, if one is: `None` for nothing. A move to the same node,
    /// group and parent, on the same element, is not asked of the
    /// subscription at all.
    pub(super) followed: Option<(NodeId, Group, Option<NodeId>)>,
    /// The element the subscription was last moved to, compared by its
    /// agile reference, so a node whose element was replaced is followed
    /// again.
    pub(super) followed_element: Option<Agile<Kept>>,
    /// The caret key's watch for evidence the worker keeps open between its
    /// entries, if one is open.
    pub(super) caret_watch: Option<OpenWatch>,
    /// The last committed read of a caret.
    pub(super) caret_read: CaretRead,
    /// What the outpost keeps of each focused terminal between its reads,
    /// by node, kept while the node is (`docs/design/terminal-model.md`,
    /// "Where it runs"). A read works on a copy, which only the commit of
    /// its result puts in place.
    pub(super) terminals: HashMap<u64, crate::terminal::Terminal>,
    /// The raw-view chain the focus listener read for the UIA focus this
    /// outpost last reported, by which it finds its own copies of that
    /// focus's elements ([`super::focus_path`]).
    pub(super) focus_path: Option<FocusPath>,
    /// What the followed focus still owes.
    pub(super) owed: Option<Owed>,
    /// What a watchdog's publication displaced, for the worker in charge
    /// to drop after its next commit.
    pub(super) leftover: Displaced,
}

/// What a commit displaced that may hold the last reference to an
/// application's object, dropped once the watch lock is let go (the
/// module's documentation).
#[derive(Default)]
#[must_use = "dropped only once the watch lock is let go"]
pub(super) struct Displaced {
    /// Terminal entries replaced or removed.
    pub(super) terminals: Vec<crate::terminal::Terminal>,
    /// Elements the subscription was not moved to.
    pub(super) elements: Vec<Agile<Kept>>,
}

impl Displaced {
    /// Takes in `other`'s values.
    pub(super) fn append(&mut self, mut other: Displaced) {
        self.terminals.append(&mut other.terminals);
        self.elements.append(&mut other.elements);
    }

    /// Takes in the elements of `target`.
    fn target(&mut self, target: Target) {
        self.elements.extend(target.element);
        self.elements.extend(target.slider);
    }
}

/// The element kept for a UIA focus's node that the focus listener's
/// evidence says is no longer the node's element: the runtime id names a
/// new element, the one it named having lost the focus or gone
/// (`docs/design/focus-pipeline-rework.md`, "One object, one identity").
pub(super) struct Stale {
    /// The node.
    pub(super) node: NodeId,
    /// The element kept for it before the focus was read, evicted only if
    /// it is still the one kept, compared by agile reference.
    pub(super) element: Option<Agile<Kept>>,
    /// Whether the earlier element is gone: its terminal entry goes too.
    pub(super) gone: bool,
}

/// Where the focus-following subscription is to be moved for a focus,
/// computed before the commit.
pub(super) struct Target {
    /// What is followed: the node, its group, and the slider followed for
    /// its value changes; `None` for nothing.
    pub(super) wanted: Option<(NodeId, Group, Option<NodeId>)>,
    /// The node's element and the slider's, when they are kept.
    pub(super) element: Option<Agile<Kept>>,
    pub(super) slider: Option<Agile<Kept>>,
}

/// A focus becoming the followed one ([`Effects::follow`]).
#[expect(
    clippy::struct_excessive_bools,
    reason = "facts about one focus, each read before the commit"
)]
pub(super) struct Follow {
    /// The focus and its role.
    pub(super) node: NodeId,
    pub(super) role: Role,
    /// The object it is, as the intake records the focus.
    pub(super) object: Option<Object>,
    /// The window it was reported in.
    pub(super) window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    pub(super) chain: Vec<NodeSnapshot>,
    /// The batch its entry belongs to.
    pub(super) batch: u64,
    /// Whether it was reached by redirecting a control's own focus event
    /// to the control's focused child.
    pub(super) redirected: bool,
    /// Whether it is a terminal read through UI Automation, and whether its
    /// window is the console host's.
    pub(super) terminal: bool,
    pub(super) console: bool,
    /// For a terminal, the caret read with its report, noted as the caret
    /// last sent to Core.
    pub(super) sent_caret: Option<CaretReport>,
    /// Where the subscription is moved.
    pub(super) target: Target,
    /// What the focus owes once followed.
    pub(super) owed: Option<OwedWork>,
    /// The trace and observation of the event it came from.
    pub(super) trace: TraceId,
    pub(super) observed_at_us: u64,
    /// Whether it is adopted as the followed focus: false for a focus-now
    /// answer naming the focus already followed, under the same node,
    /// which keeps the tracking and the intake's marks as they are and is
    /// only followed again.
    pub(super) adopt: bool,
}

/// A change of the caret key's watch ([`Effects::watch`]).
pub(super) enum WatchChange {
    /// A watch opened, the worker to be woken at the instant given; the
    /// query it answers is no longer the entry's.
    Open(OpenWatch, std::time::Instant),
    /// A watch taken out to be checked, put back with the query no longer
    /// the entry's.
    PutBack(OpenWatch),
    /// The watch on this node taken out to be checked, its query made the
    /// one an abandonment answers: the taken watch is handed back.
    Take(NodeId),
    /// The open watch ended with no evidence, answered `WatchEnded`; the
    /// reason is logged.
    End(&'static str),
    /// The open watch for this request had a poll read at the instant
    /// given that found no evidence.
    Polled(u64, std::time::Instant),
    /// The worker is to be woken at the instant given for the open watch,
    /// or not at all.
    Wake(Option<std::time::Instant>),
}

/// The report to record for Core's judgement ([`Effects::report`]).
pub(super) enum Report {
    /// A focus or foreground change reported.
    Reported(held::Focus),
    /// A focus-now answer's control, which keeps the event of the same
    /// focus judged against before it (`worker::answer_to_judge`).
    Answered(held::Focus),
}

/// What one commit does, built on the stack before the commit with every
/// call it needs already made.
#[derive(Default)]
pub(super) struct Effects {
    /// The entry's result, which ends its query.
    pub(super) result: Option<OutpostToSupervisor>,
    /// An answer given outside its query's own entry (a caret key's
    /// watch), with its request.
    pub(super) aside: Option<(u64, OutpostToSupervisor)>,
    /// The report to record for Core's judgement.
    pub(super) report: Option<Report>,
    /// A focus becoming the followed one.
    pub(super) follow: Option<Follow>,
    /// A foreground report's role and batch, with a caret key's watch on
    /// another node than this window's ended.
    pub(super) foreground: Option<(NodeId, Role, u64)>,
    /// The dialog memo the report leaves.
    pub(super) dialog: Option<DialogMemo>,
    /// The focus listener's mark of a UIA focus whose value is its first
    /// selected item; `None` leaves the mark as it was.
    #[expect(
        clippy::option_option,
        reason = "a mark to set, to clear, or to leave as it was"
    )]
    pub(super) value_from_selection: Option<Option<NodeId>>,
    /// The focus path of a UIA focus.
    pub(super) focus_path: Option<FocusPath>,
    /// The focus's element found after its report, with its slider's when
    /// found: the subscription is moved to them only while that node is
    /// the followed focus.
    pub(super) element: Option<(NodeId, Target)>,
    /// A terminal's new entry, the copy its read worked on.
    pub(super) terminal: Option<(NodeId, crate::terminal::Terminal)>,
    /// The caret last sent to Core for a focused terminal.
    pub(super) sent_caret: Option<(NodeId, CaretReport)>,
    /// The owed work of this node is done, read or failed.
    pub(super) owed_done: Option<NodeId>,
    /// A read of this node's caret began at the instant given, in
    /// microseconds.
    pub(super) caret_read: Option<(NodeId, u64)>,
    /// A change of the caret key's watch.
    pub(super) watch: Option<WatchChange>,
    /// The intake's marks: the system caret's events dropped until the
    /// next focus; the console window's updates absorbed; the redirect mark
    /// cleared.
    pub(super) drop_system_caret: bool,
    pub(super) absorb: Option<isize>,
    pub(super) clear_redirect: bool,
    /// A held node's states, as an MSAA state change read them.
    pub(super) states: Option<(NodeId, StateSet)>,
    /// A UIA focus's stale element, evicted before the focus is followed.
    pub(super) stale: Option<Stale>,
}

/// What [`Watch::commit`](super::worker::Watch) did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Committed {
    /// The effects were applied.
    Applied,
    /// Nothing was applied: the worker was abandoned.
    Refused,
}

/// Sends `message` under the watch lock, recording every node issued or
/// looked up since the last send as reported at the position it takes
/// (`worker::take_touched`).
pub(super) fn send(context: &Context, message: OutpostToSupervisor) {
    context
        .outbound
        .publish(message, super::worker::take_touched(context));
}

/// The `WatchEnded` answer to `open`, which Core says nothing for.
pub(super) fn watch_ended(open: &OpenWatch) -> OutpostToSupervisor {
    OutpostToSupervisor::Reply {
        trace_id: open.trace,
        request_id: open.request_id,
        outcome: QueryOutcome::Done(QueryResult::Text(TextReply::WatchEnded)),
        timing: EventTiming {
            calls: open.calls,
            published_at_us: now_us(),
            ..open.timing
        },
    }
}

impl FollowState {
    /// Ends the open caret key's watch, if there is one, with its
    /// `WatchEnded` answer, the intake's wake cancelled with it.
    pub(super) fn end_watch(&mut self, context: &Context, why: &str) {
        if let Some(open) = self.caret_watch.take() {
            context.intake.wake_at(None);
            send(context, watch_ended(&open));
            tracing::debug!(
                trace = %open.trace,
                why,
                waited_ms = open.opened.elapsed().as_millis(),
                "a caret key's watch ended without evidence"
            );
        }
    }

    /// Ends a caret key's watch on a node other than `node`, the new focus
    /// or foreground window, as NVDA's wait gives way to a focus change.
    fn end_watch_elsewhere(&mut self, context: &Context, node: NodeId) {
        if self
            .caret_watch
            .as_ref()
            .is_some_and(|open| open.node_id != node)
        {
            self.end_watch(context, "the focus moved");
        }
    }

    /// Moves the focus-following subscription to `target`, unless it is
    /// already there. The subscription keeps references of its own, so
    /// `target`'s elements are displaced either way.
    fn retarget(&mut self, context: &Context, target: Target, displaced: &mut Displaced) {
        let same_element = match (&self.followed_element, &target.element) {
            (Some(followed), Some(element)) => followed.same_as(element),
            (None, None) => true,
            _ => false,
        };
        if self.followed == target.wanted && same_element {
            tracing::debug!(
                wanted = ?target.wanted,
                "the focus-following subscription already follows this focus; it is not moved"
            );
            displaced.target(target);
            return;
        }
        tracing::debug!(
            from = ?self.followed,
            to = ?target.wanted,
            "the focus-following subscription is moved"
        );
        self.followed = target.wanted;
        displaced.elements.extend(std::mem::replace(
            &mut self.followed_element,
            target.element.clone(),
        ));
        if let Some(subscription) = context.focus_events.get() {
            match (target.wanted, &target.element) {
                (Some((_, group, _)), Some(element)) => subscription.retarget_with(
                    verbatim_uia::Scope::Elements(vec![element.reference()]),
                    group,
                    target
                        .slider
                        .as_ref()
                        .map(|slider| (slider.reference(), Group::Value))
                        .into_iter()
                        .collect(),
                ),
                _ => subscription.retarget(verbatim_uia::Scope::Nothing),
            }
        }
        displaced.target(target);
    }

    /// Makes `follow` the followed focus.
    fn follow(&mut self, context: &Context, follow: Follow, displaced: &mut Displaced) {
        let Follow {
            node,
            role,
            object,
            window,
            chain,
            batch,
            redirected,
            terminal,
            console,
            sent_caret,
            target,
            owed,
            trace,
            observed_at_us,
            adopt,
        } = follow;
        if adopt {
            self.adopt(
                context,
                (node, role),
                (object, window, chain, batch),
                (terminal, console, redirected),
            );
        }
        // A terminal gaining the focus has its caret reported afresh: the
        // caret read with its report is the one sent.
        if terminal {
            self.terminals.entry(node.number()).or_default().sent_caret = sent_caret;
        } else if let Some(entry) = self.terminals.get_mut(&node.number()) {
            entry.sent_caret = None;
        }
        self.retarget(context, target, displaced);
        self.owed = owed.map(|work| Owed {
            node,
            work,
            trace,
            observed_at_us,
            attempting: None,
        });
    }

    /// Makes `node`, of `role`, the focus this outpost follows: the object
    /// `object` in `window`, with `chain`, its ancestors and itself, from
    /// an entry of `batch`. A caret key's watch on another node ends, the
    /// intake is told, and the tracking takes it.
    fn adopt(
        &mut self,
        context: &Context,
        (node, role): (NodeId, Role),
        (object, window, chain, batch): (Option<Object>, Option<isize>, Vec<NodeSnapshot>, u64),
        (terminal, console, redirected): (bool, bool, bool),
    ) {
        self.end_watch_elsewhere(context, node);
        // The intake's marks in one acquisition: a terminal read through
        // UIA has its caret event planned after its text changes, and a
        // console host's terminal, whose console caret event is its caret
        // event, has the system caret's events dropped
        // (`docs/parity/text.md`).
        context.intake.note_focus(
            object,
            terminal.then(|| window.unwrap_or(0)),
            terminal && console,
            redirected,
        );
        let tracking = &mut self.tracking;
        tracking.role = Some(role);
        tracking.focus = Some(node);
        tracking.focus_role = Some(role);
        tracking.window = window;
        tracking.chain = chain;
        tracking.batch = Some(batch);
    }

    /// Evicts `stale`, a UIA focus's element the focus listener's evidence
    /// says is no longer its node's, if it is still the one kept, so
    /// `follow`, the focus's, does not move the subscription to it; for one
    /// gone, the node's terminal entry goes too.
    fn evict(
        &mut self,
        context: &Context,
        stale: Stale,
        follow: Option<&mut Follow>,
        displaced: &mut Displaced,
    ) {
        let Stale {
            node,
            element,
            gone,
        } = stale;
        let evicted =
            element.and_then(|element| context.uia_registry.evict_element_if(node, &element));
        // The subscription is not moved to the element evicted: the
        // focus's own is found after its report.
        if let (Some(evicted), Some(follow)) = (&evicted, follow)
            && follow.node == node
            && follow
                .target
                .element
                .as_ref()
                .is_some_and(|element| element.same_as(evicted))
        {
            let target = std::mem::replace(
                &mut follow.target,
                Target {
                    wanted: None,
                    element: None,
                    slider: None,
                },
            );
            displaced.target(target);
        }
        displaced.elements.extend(evicted);
        if gone {
            displaced
                .terminals
                .extend(self.terminals.remove(&node.number()));
        }
    }

    /// Applies `effects` but for their messages and report, which
    /// [`Watch::commit`](super::worker::Watch) sends and records first.
    /// Returns the watch taken out by [`WatchChange::Take`], and what was
    /// displaced, to be dropped once the watch lock is let go.
    pub(super) fn apply(
        &mut self,
        context: &Context,
        generation: u64,
        effects: Effects,
    ) -> (Option<OpenWatch>, Displaced) {
        let Effects {
            follow,
            foreground,
            dialog,
            value_from_selection,
            focus_path,
            element,
            terminal,
            sent_caret,
            owed_done,
            caret_read,
            watch,
            drop_system_caret,
            absorb,
            clear_redirect,
            states,
            stale,
            ..
        } = effects;
        let (mut follow, mut displaced) = (follow, Displaced::default());
        if let Some(memo) = dialog {
            self.tracking.dialog = memo;
        }
        if let Some(stale) = stale {
            self.evict(context, stale, follow.as_mut(), &mut displaced);
        }
        if let Some(follow) = follow {
            self.follow(context, follow, &mut displaced);
        }
        if let Some(path) = focus_path {
            self.focus_path = Some(path);
        }
        if let Some(value_from_selection) = value_from_selection {
            self.tracking.value_from_selection = value_from_selection;
        }
        if let Some((node, role, batch)) = foreground {
            self.end_watch_elsewhere(context, node);
            self.tracking.role = Some(role);
            self.tracking.batch = Some(batch);
        }
        if let Some((node, target)) = element {
            if self.tracking.focus == Some(node) {
                self.retarget(context, target, &mut displaced);
            } else {
                displaced.target(target);
            }
        }
        if let Some((node, entry)) = terminal {
            displaced
                .terminals
                .extend(self.terminals.insert(node.number(), entry));
        }
        if let Some((node, caret)) = sent_caret {
            self.terminals.entry(node.number()).or_default().sent_caret = Some(caret);
        }
        if let Some(node) = owed_done
            && self
                .owed
                .as_ref()
                .is_some_and(|owed| owed.node == node && owed.attempting == Some(generation))
        {
            self.owed = None;
        }
        if let Some((node, started_us)) = caret_read {
            self.caret_read.note(node.number(), started_us);
        }
        if drop_system_caret {
            context.intake.drop_system_caret();
        }
        if absorb.is_some() {
            context.intake.absorb_console_updates(absorb);
        }
        if clear_redirect {
            context.intake.clear_redirected();
        }
        if let Some((node, states)) = states {
            let _ = context.held().replace_states(node, states);
        }
        let mut taken = None;
        match watch {
            None => {}
            Some(WatchChange::Open(open, wake)) => {
                context.intake.wake_at(Some(wake));
                self.caret_watch = Some(open);
            }
            Some(WatchChange::PutBack(open)) => self.caret_watch = Some(open),
            Some(WatchChange::Take(node)) => {
                taken = self.caret_watch.take_if(|open| open.node_id == node);
            }
            Some(WatchChange::End(why)) => self.end_watch(context, why),
            Some(WatchChange::Polled(request_id, at)) => {
                if super::caret_poll::after_read(&mut self.caret_watch, request_id, at)
                    == super::caret_poll::AfterRead::Last
                {
                    self.end_watch(context, "its poll's bound passed");
                }
            }
            Some(WatchChange::Wake(at)) => context.intake.wake_at(at),
        }
        (taken, displaced)
    }
}
