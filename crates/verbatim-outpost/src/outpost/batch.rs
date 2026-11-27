//! Core's batches, run on the worker (the verbatim-outpost crate guide,
//! "Core's requests"): parsed once as they arrive and refused whole when
//! they do not parse; each subject resolved once, at its first use, and
//! every later use taking the same object; each step run in order, under
//! its condition, by the outpost's code for its operation.
//!
//! A `Read`, `Test`, `IsOneOf` or `Activate` on a UI Automation node, and
//! every `Follow`, runs through the UI Automation executor's call arms
//! (`verbatim_uia_rops::batch`), resolved by [`super::uia_object`]; an
//! activation's walk up the parents is the dispatcher's, each parent
//! activated through its own backend. Each other operation runs on the
//! code the outpost's kinds of request run on: an MSAA node's properties
//! with those read apart, a navigation step, the ancestor walk, the
//! children walk for a walk down the tree, the MSAA candidate's
//! acquisition, and the text reads through each backend's adapter. A
//! `FocusRead`, and a `Test` of the application's focus, run through the
//! focus window's executor's arms ([`super::focus`]). The controller-for relation and the first selected
//! item run through the program builder ([`verbatim_uia_rops::batch`]), as
//! does each run of consecutive text steps on one subject that resolves
//! to a UI Automation node (`Caret`, `ReadText`, `ReadRange`,
//! `CompareSelection`, `Select`, `Locate`, `MoveBySentence`); an edit
//! control's text is read by its messages, step by step.

#![forbid(unsafe_code)]

use verbatim_model::batch::{
    AncestorsEnd, Ancestry, Batch, Before, CaretAt, Descendant, Descended, Done, Gate,
    KindMismatch, Missing, Op, Point, PointOf, ReaderKind, Relation, StepOutcome, StepRefusal,
    Subject, Tested, Walk, WindowOf, parse,
};
use verbatim_model::property::{NodeDetailSet, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    Ask, Followed, KnownAncestor, MsaaIds, NodeRead, Outcome, Refusal, Reply, Route,
};
use verbatim_model::{
    Backend, NodeId, NodeSnapshot, QueryKind, TextFailure, TextPoint, TextPosition,
};

use super::RequestContext;
use super::asked;
use super::focus::FocusNow;
use super::read::{self, Client, ReadError};
use super::text_reads;
use super::window::hwnd_of;

/// The node details a batch's reads fetch: those its steps' property sets
/// name.
pub(super) fn node_details(batch: &Batch) -> NodeDetailSet {
    let mut set = Vec::new();
    for step in &batch.steps {
        match &step.op {
            Op::Read { set: own, .. }
            | Op::Related { set: own, .. }
            | Op::Ancestors { set: own, .. }
            | Op::FocusRead { set: own, .. } => set.extend_from_slice(own),
            Op::Descendants { walk, .. } => set.extend_from_slice(&walk.set),
            Op::Test { matcher, .. } => set.extend(matcher.properties()),
            _ => {}
        }
    }
    NodeDetailSet::named_by(&set)
}

/// The answer to `batch`, `ask`'s, read on the worker; the request's
/// context keeps its probes ([`asked::told`]).
pub(super) fn answer(
    context: &RequestContext<'_>,
    client: &mut Client,
    ask: &Ask,
    batch: &Batch,
) -> Outcome {
    let parsed = match parse(batch.clone(), ReaderKind::Outpost) {
        Ok(parsed) => parsed,
        Err(malformed) => return Outcome::Refused(Refusal::Malformed(malformed)),
    };
    // The window with the keyboard focus is taken before any call that can
    // block, the probe among them, as the focus-now read takes it.
    let focus = parsed
        .batch()
        .reads_focus_first()
        .then(|| read::checked_focus_window(context).map_err(|_| ()));
    // The request's window's verdict, probed once for the batch: a subject
    // on that window takes it, so no step probes it again.
    let verdict = asked::verdict(context, hwnd_of(ask.window), ask.verdict);
    // What a read of the focus takes, which a test of the focus reads it
    // by, so that the one read serves both.
    let focus_read = parsed.steps().iter().find_map(|step| match &step.op {
        Op::FocusRead {
            subject: Subject::Focus,
            known,
            set,
            details,
        } => Some((known.clone(), set.clone(), *details)),
        _ => None,
    });
    let mut run = Run {
        context,
        client,
        given: ask.verdict,
        focus,
        focus_read,
        verdicts: vec![(ask.window, verdict)],
        held: Vec::new(),
        outcomes: Vec::new(),
    };
    if let Some(refused) = run.backend_refusal(parsed.steps()) {
        return refused;
    }
    run.steps(parsed.steps())
}

/// Runs one poll of a caret wait's batch, `program`'s: the batch's answer
/// when the poll ends the wait, with the outcomes of its later steps the
/// poll's program read, or, when it read none, those steps run after it;
/// `None` while the wait goes on.
pub(super) fn poll(
    context: &RequestContext<'_>,
    client: &mut Client,
    program: &verbatim_outpost_logic::WatchProgram,
) -> Option<Outcome> {
    let ended = text_reads::watch(context, client.uia(), program)?;
    Some(match ended {
        text_reads::WaitEnded::NotThere => Outcome::NotThere,
        text_reads::WaitEnded::Waited(waited, Some(later)) => {
            let mut outcomes = vec![StepOutcome::Done(Done::WaitForCaret(waited))];
            outcomes.extend(later);
            Outcome::Result(Reply::Steps(outcomes))
        }
        text_reads::WaitEnded::Waited(waited, None) => after_wait(context, client, program, waited),
    })
}

/// The answer to a caret wait's batch, `program`'s, its wait ended
/// `waited`: the steps after the wait run in order under their
/// conditions, as any batch's do (the request list's design, section 5,
/// `WaitForCaret`).
fn after_wait(
    context: &RequestContext<'_>,
    client: &mut Client,
    program: &verbatim_outpost_logic::WatchProgram,
    waited: verbatim_model::batch::Waited,
) -> Outcome {
    let run = Run {
        context,
        client,
        given: verbatim_model::request::WindowVerdict::ProbeFirst,
        focus: None,
        focus_read: None,
        verdicts: Vec::new(),
        held: Vec::new(),
        outcomes: vec![StepOutcome::Done(Done::WaitForCaret(waited))],
    };
    let later = program.batch.steps.get(1..).unwrap_or_default();
    run.steps(later)
}

/// What a walk down the tree has answered and visited so far, over its
/// starts.
struct Walked {
    /// The nodes answered.
    found: Vec<Descendant>,
    /// The nodes the children walk visited and the program builder's
    /// answered, which counts no visits of its own: what is left of the
    /// node cap for a later start.
    visited: usize,
    /// Whether no cap or failed read cut it.
    complete: bool,
}

/// Whether `node`, read, is answered by `walk`, and whether its subtree is
/// left out.
fn judged(context: &RequestContext<'_>, node: &NodeSnapshot, walk: &Walk) -> (bool, bool) {
    let holds = |matcher: &verbatim_model::matcher::Matcher| {
        let Ok(holds) =
            matcher.evaluate::<std::convert::Infallible>(&mut |id| Ok(value_of(context, node, id)));
        holds
    };
    (
        walk.matcher.as_ref().is_none_or(holds),
        walk.prune.as_ref().is_some_and(holds),
    )
}

/// An object a subject names, resolved once per batch.
enum Held {
    /// A node the outpost holds.
    Node(NodeId),
    /// An MSAA object at its event's ids, acquired; given up to its full
    /// read, once.
    Msaa(Option<Box<verbatim_ia2::acquire::FocusCandidate>>),
    /// A window's own object, through the backend Core names.
    Window(isize, Backend),
    /// The application's focus, in the focused window taken first, with
    /// its read once a test of it read it.
    Focus(isize, Option<FocusNow>),
    /// The outpost's copy of an element the listener read, by its route.
    Route(Box<Route>),
}

/// A batch under way.
struct Run<'r, 'c> {
    context: &'r RequestContext<'c>,
    client: &'r mut Client,
    /// The verdict Core gave for the request's window.
    given: verbatim_model::request::WindowVerdict,
    /// The focused window, taken before anything else when the first step
    /// reads the focus.
    focus: Option<Result<isize, ()>>,
    /// Each window's verdict, taken once per batch, the request's window's
    /// first; `None` for a window the outpost's application does not
    /// serve.
    verdicts: Vec<(verbatim_model::WindowHandle, Option<bool>)>,
    /// What the batch's read of the focus takes: the ancestors Core knows,
    /// the set and the caret's details.
    focus_read: Option<(
        Vec<KnownAncestor>,
        Vec<PropertyId>,
        verbatim_model::TextDetails,
    )>,
    /// Each subject resolved, with what it resolved to.
    held: Vec<(Subject, Result<Held, Missing>)>,
    outcomes: Vec<StepOutcome>,
}

/// A step's outcome, or the whole batch's.
type Stepped = Result<StepOutcome, Outcome>;

impl Run<'_, '_> {
    /// Runs `steps`, the batch's from the next place on, each under its
    /// condition: the batch's answer.
    fn steps(mut self, steps: &[verbatim_model::batch::Step]) -> Outcome {
        let mut at = 0;
        while at < steps.len() {
            match self.fusable_end(steps, at) {
                Ok(Some((node, end))) => {
                    let outcomes = self.fused(node, &steps[at..end]);
                    self.outcomes.extend(outcomes);
                    at = end;
                    continue;
                }
                Ok(None) => {}
                Err(whole) => return whole,
            }
            let step = &steps[at];
            let outcome = match step.gate(&self.outcomes) {
                Ok(Gate::Skip) => StepOutcome::Skipped,
                Ok(Gate::Run) => match self.step(&step.op) {
                    Ok(outcome) => outcome,
                    Err(whole) => return whole,
                },
                // Only the UI Automation executor compares two points of
                // a text.
                Ok(Gate::Compare { .. }) => StepOutcome::Refused(StepRefusal::Unsupported),
                Err(KindMismatch) => {
                    tracing::error!(target: verbatim_model::DEFECT_TARGET, ?step, "a step's condition names a result it cannot test");
                    StepOutcome::Failed
                }
            };
            self.outcomes.push(outcome);
            at += 1;
        }
        Outcome::Result(Reply::Steps(self.outcomes))
    }

    /// The place in `held` of what `subject` names, resolved at its first
    /// use: `Err` with the whole batch's outcome when the subject's window
    /// is read through the other backend.
    fn resolve(&mut self, subject: &Subject) -> Result<usize, Outcome> {
        if let Some(place) = self.held.iter().position(|(held, _)| held == subject) {
            return Ok(place);
        }
        let resolved = match subject {
            Subject::Node(node) => Ok(Held::Node(*node)),
            Subject::Result(_) | Subject::Item { .. } => subject
                .earlier_nodes(&self.outcomes)
                .and_then(|nodes| nodes.first().and_then(|read| read.node))
                .map(Held::Node)
                .ok_or(Missing::Object),
            Subject::Msaa { ids, redirect } => self.msaa(*ids, *redirect)?,
            Subject::Window {
                which,
                backend,
                visible,
            } => self
                .window(which, *visible)
                .map(|hwnd| Held::Window(hwnd, *backend)),
            Subject::Focus => match self.focus_window() {
                Ok(hwnd) => Ok(Held::Focus(hwnd, None)),
                Err(()) => Err(Missing::Object),
            },
            // Read through Core's verdict for the request's window, as the
            // listener's element is UI Automation's.
            Subject::Route(route) => match self.verdict_of(route.window, self.given) {
                Some(true) => Ok(Held::Route(route.clone())),
                Some(false) => return Err(Outcome::Refused(Refusal::ReadThroughMsaa)),
                None => Err(Missing::Object),
            },
            Subject::Kept { .. } => return Err(Outcome::Refused(Refusal::Unsupported)),
        };
        self.held.push((subject.clone(), resolved));
        Ok(self.held.len() - 1)
    }

    /// An MSAA object at `ids`, its window's verdict consulted first: NVDA's
    /// focus redirect applied with `redirect`, none for a menu start
    /// (ADR 0004, "MSAA candidates").
    fn msaa(&mut self, ids: MsaaIds, redirect: bool) -> Result<Result<Held, Missing>, Outcome> {
        let hwnd = hwnd_of(ids.window);
        match self.verdict_of(
            ids.window,
            verbatim_model::request::WindowVerdict::ProbeFirst,
        ) {
            Some(true) => return Err(Outcome::Refused(Refusal::ReadThroughUia)),
            None => return Ok(Err(Missing::Object)),
            Some(false) => {}
        }
        let acquired = if redirect {
            verbatim_ia2::acquire::focus_candidate(hwnd, ids.object, ids.child)
        } else {
            verbatim_ia2::acquire::menu_start_candidate(hwnd, ids.object, ids.child)
        };
        Ok(acquired
            .map(|candidate| Held::Msaa(Some(Box::new(candidate))))
            .ok_or(Missing::Object))
    }

    /// The verdict of `window`, taken once for the batch: the request's
    /// window's, or another's by `given`.
    fn verdict_of(
        &mut self,
        window: verbatim_model::WindowHandle,
        given: verbatim_model::request::WindowVerdict,
    ) -> Option<bool> {
        if let Some((_, verdict)) = self.verdicts.iter().find(|(held, _)| *held == window) {
            return *verdict;
        }
        let verdict = asked::verdict(self.context, hwnd_of(window), given);
        self.verdicts.push((window, verdict));
        verdict
    }

    /// The whole batch's refusal when a subject's window is read through
    /// the other backend, decided before any step runs, as the backend
    /// refusals are outcomes of the whole batch: an MSAA object in a
    /// window read through UI Automation, or a listener's element in one
    /// read through MSAA.
    fn backend_refusal(&mut self, steps: &[verbatim_model::batch::Step]) -> Option<Outcome> {
        for step in steps {
            let refused = match step.op.subject() {
                Some(Subject::Msaa { ids, .. }) => {
                    let given = verbatim_model::request::WindowVerdict::ProbeFirst;
                    (self.verdict_of(ids.window, given) == Some(true))
                        .then_some(Refusal::ReadThroughUia)
                }
                Some(Subject::Route(route)) => (self.verdict_of(route.window, self.given)
                    == Some(false))
                .then_some(Refusal::ReadThroughMsaa),
                _ => None,
            };
            if let Some(refusal) = refused {
                return Some(Outcome::Refused(refusal));
            }
        }
        None
    }

    /// The window with the keyboard focus, taken once per batch.
    fn focus_window(&mut self) -> Result<isize, ()> {
        let context = self.context;
        *self
            .focus
            .get_or_insert_with(|| read::checked_focus_window(context).map_err(|_| ()))
    }

    /// The window `which` names, of the outpost's application, shown when
    /// `visible`: for the focused window, the top-level window of the
    /// window with the keyboard focus, whose own object the hidden-dialog
    /// check reads (Claude, 2026-11-22).
    fn window(&mut self, which: &WindowOf, visible: bool) -> Result<isize, Missing> {
        let hwnd = match which {
            WindowOf::Handle(window) => Some(hwnd_of(*window)),
            WindowOf::TopLevelOf(window) => {
                Some(verbatim_win32::window::top_level_of(hwnd_of(*window)))
            }
            WindowOf::ClassPath(classes) => super::class_path::class_path_window(classes),
            WindowOf::Focused => self
                .focus_window()
                .ok()
                .map(verbatim_win32::window::top_level_of),
            WindowOf::Foreground => Some(verbatim_win32::window::foreground_window_handle()),
        };
        hwnd.filter(|&hwnd| hwnd != 0 && read::serves(self.context, hwnd))
            .filter(|&hwnd| !visible || verbatim_win32::window::window_is_visible(hwnd))
            .ok_or(Missing::Object)
    }

    /// What `subject` resolved to, or the step's outcome when it is not
    /// there.
    fn held(&mut self, subject: &Subject) -> Result<Result<usize, StepOutcome>, Outcome> {
        let place = self.resolve(subject)?;
        Ok(match &self.held[place].1 {
            Ok(_) => Ok(place),
            Err(missing) => Err(StepOutcome::NotThere(match (missing, subject) {
                (Missing::Object, Subject::Node(node)) => Missing::Node(*node),
                (missing, _) => *missing,
            })),
        })
    }

    /// The node `subject` names, for an operation that runs on a node.
    fn node(&mut self, subject: &Subject) -> Result<Result<NodeId, StepOutcome>, Outcome> {
        Ok(match self.held(subject)? {
            Err(outcome) => Err(outcome),
            Ok(place) => match &self.held[place].1 {
                Ok(Held::Node(node)) => Ok(*node),
                _ => Err(StepOutcome::Refused(StepRefusal::Unsupported)),
            },
        })
    }

    /// The UI Automation node of the run of text steps on one subject
    /// that starts at `at` (the request list's design, section 5), and the
    /// run's end: `None` when the step there is no text step the UI
    /// Automation executor runs, or its subject is no UI Automation node.
    fn fusable_end(
        &mut self,
        steps: &[verbatim_model::batch::Step],
        at: usize,
    ) -> Result<Option<(NodeId, usize)>, Outcome> {
        fn subject_of(op: &Op) -> Option<&Subject> {
            match op {
                Op::Caret { subject, .. }
                | Op::ReadText { subject, .. }
                | Op::ReadRange { subject, .. }
                | Op::CompareSelection { subject, .. }
                | Op::Select { subject, .. }
                | Op::Locate { subject, .. }
                | Op::MoveBySentence { subject, .. }
                | Op::Visible { subject }
                | Op::Move { subject, .. }
                | Op::FindText { subject, .. }
                | Op::ReadColours { subject, .. } => Some(subject),
                _ => None,
            }
        }
        let Some(subject) = steps.get(at).and_then(|step| subject_of(&step.op)) else {
            return Ok(None);
        };
        let node = match self.node(subject)? {
            Ok(node) if self.context.store.runtime_id_of(node).is_some() => node,
            _ => return Ok(None),
        };
        let end = at
            + steps[at..]
                .iter()
                .take_while(|step| subject_of(&step.op) == Some(subject))
                .count();
        Ok(Some((node, end)))
    }

    /// Runs `steps`, a run of text steps on UI Automation node `node`,
    /// through the UI Automation executor: their outcomes, each step
    /// refused as its text's opening answered, under its condition, when
    /// the node's text cannot be opened.
    fn fused(&mut self, node: NodeId, steps: &[verbatim_model::batch::Step]) -> Vec<StepOutcome> {
        let context = self.context;
        let first = self.outcomes.len();
        let reply = match self.client.uia() {
            Some(uia) => {
                match text_reads::fused_steps(context, uia, node, (steps, first, &self.outcomes)) {
                    Ok(outcomes) => return outcomes,
                    Err(reply) => reply,
                }
            }
            None => TextFailure::Unanswered,
        };
        let mut outcomes = self.outcomes.clone();
        for step in steps {
            outcomes.push(match step.gate(&outcomes) {
                Ok(Gate::Skip) => StepOutcome::Skipped,
                // The text did not open, so there are no points to
                // compare: the step answers that.
                Ok(Gate::Run | Gate::Compare { .. }) => text_refused(reply, node),
                Err(KindMismatch) => {
                    tracing::error!(target: verbatim_model::DEFECT_TARGET, ?step, "a step's condition names a result it cannot test");
                    StepOutcome::Failed
                }
            });
        }
        outcomes.split_off(first)
    }

    fn step(&mut self, op: &Op) -> Stepped {
        if let Some(outcome) = self.on_uia_node(op)? {
            return Ok(outcome);
        }
        if let Some(outcome) = self.on_msaa(op)? {
            return Ok(outcome);
        }
        match op {
            // Every other subject's read is refused: a focus is read by
            // `FocusRead`.
            Op::Read { .. } => Ok(StepOutcome::Refused(StepRefusal::Unsupported)),
            Op::Test { subject, matcher } => self.test(subject, matcher),
            Op::Related {
                subject,
                relation,
                set,
            } => self.related(subject, *relation, set),
            Op::Ancestors {
                subject,
                until,
                depth,
                set,
            } => self.ancestors(subject, until, *depth, set),
            Op::Descendants { subject, walk } => self.descendants(subject, walk),
            Op::IsOneOf { .. } | Op::Activate { .. } => {
                Ok(StepOutcome::Refused(StepRefusal::Unsupported))
            }
            Op::FocusRead {
                subject,
                known,
                set,
                details,
            } => self.focus_read(subject, (known, set, *details)),
            _ => self.text(op),
        }
    }

    /// An object step on a UI Automation node through the executor's call
    /// arm (`Read`, `Test`, `IsOneOf`, `Activate`), a `Follow`, or a last
    /// child in one step: `None` for a step on any other subject, which
    /// the outpost's MSAA and window reads run.
    fn on_uia_node(&mut self, op: &Op) -> Result<Option<StepOutcome>, Outcome> {
        let subject = match op {
            Op::Read { subject, .. }
            | Op::Test { subject, .. }
            | Op::IsOneOf { subject, .. }
            | Op::Activate { subject }
            | Op::Related {
                subject,
                relation:
                    Relation::LastChild | Relation::ControllerFor | Relation::FirstSelectedItem,
                ..
            } => subject,
            Op::Follow(target) => return self.follow_step(target.as_ref()).map(Some),
            _ => return Ok(None),
        };
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(Some(outcome)),
        };
        let context = self.context;
        let node = match &self.held[place].1 {
            Ok(Held::Node(node)) if self.context.store.runtime_id_of(*node).is_some() => *node,
            Ok(Held::Window(hwnd, Backend::Uia))
                if matches!(op, Op::Read { .. } | Op::Test { .. }) =>
            {
                return Ok(Some(super::uia_object::window_step(
                    context,
                    self.client,
                    *hwnd,
                    op,
                )));
            }
            Ok(Held::Route(route)) if matches!(op, Op::Read { .. }) => {
                return Ok(Some(super::uia_object::route_read(
                    context,
                    self.client,
                    route,
                    op,
                )));
            }
            _ => return Ok(None),
        };
        Ok(Some(match op {
            Op::Related {
                relation: relation @ (Relation::ControllerFor | Relation::FirstSelectedItem),
                set,
                ..
            } => {
                let op = Op::Related {
                    subject: Subject::Node(node),
                    relation: *relation,
                    set: Vec::new(),
                };
                match super::uia_object::programmed(context, self.client, (node, op), None)? {
                    Ok(verbatim_uia_rops::batch::Found::Related(related)) => {
                        StepOutcome::Done(Done::Related(
                            related
                                .nodes
                                .iter()
                                .map(|reached| {
                                    asked::node_read(
                                        context,
                                        &reached.snapshot(context.nodes()),
                                        related.window,
                                        set,
                                    )
                                })
                                .collect(),
                        ))
                    }
                    Ok(_) => StepOutcome::Failed,
                    Err(outcome) => outcome,
                }
            }
            Op::Related { set, .. } => match read::last_child(context, self.client, node) {
                Ok(reached) => StepOutcome::Done(Done::Related(
                    reached
                        .map(|reached| asked::node_read(context, &reached, None, set))
                        .into_iter()
                        .collect(),
                )),
                Err(error) => failed(&error, node),
            },
            Op::Activate { .. } => self.activate(node),
            op => match self.client.uia() {
                Some(uia) => super::uia_object::step(context, uia, node, op),
                None => StepOutcome::Failed,
            },
        }))
    }

    /// An object step on an MSAA node or an acquired MSAA object through the
    /// MSAA executor (`Read`, `Test`, `Related`, `IsOneOf`, `Activate`):
    /// `None` for a step on any other subject.
    fn on_msaa(&mut self, op: &Op) -> Result<Option<StepOutcome>, Outcome> {
        let (Op::Read { subject, .. }
        | Op::Test { subject, .. }
        | Op::IsOneOf { subject, .. }
        | Op::Activate { subject }
        | Op::Related { subject, .. }) = op
        else {
            return Ok(None);
        };
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(Some(outcome)),
        };
        let context = self.context;
        let client = &mut *self.client;
        Ok(match &mut self.held[place].1 {
            Ok(Held::Node(node)) if context.store.runtime_id_of(*node).is_none() => {
                let node = *node;
                Some(match op {
                    Op::Activate { .. } => activation_outcome(
                        read::activate_walk(&mut read::LiveActivation { context, client }, node),
                        node,
                    ),
                    op => super::msaa_object::step(
                        context,
                        client,
                        (verbatim_ia2::batch::Object::Node(node), 0),
                        op,
                    ),
                })
            }
            Ok(Held::Window(hwnd, Backend::Msaa))
                if matches!(op, Op::Read { .. } | Op::Test { .. }) =>
            {
                let hwnd = *hwnd;
                Some(super::msaa_object::step(
                    context,
                    client,
                    (verbatim_ia2::batch::Object::Window(hwnd), hwnd),
                    op,
                ))
            }
            Ok(Held::Msaa(Some(candidate))) => {
                let window = candidate.key().0;
                Some(super::msaa_object::step(
                    context,
                    client,
                    (verbatim_ia2::batch::Object::Candidate(candidate), window),
                    op,
                ))
            }
            _ => None,
        })
    }

    /// The dispatcher's activation of node `node`, up its parents
    /// ([`read::activate_walk`]).
    fn activate(&mut self, node: NodeId) -> StepOutcome {
        let context = self.context;
        activation_outcome(
            read::activate_walk(
                &mut read::LiveActivation {
                    context,
                    client: self.client,
                },
                node,
            ),
            node,
        )
    }

    /// `Follow` through the executor's call arm: the registration moved to
    /// the node named, or removed.
    fn follow_step(
        &mut self,
        target: Option<&verbatim_model::batch::FollowTarget>,
    ) -> Result<StepOutcome, Outcome> {
        let followed = match target {
            None => None,
            Some(target) => match self.node(&target.subject)? {
                Ok(node) => Some(Followed {
                    node,
                    group: target.group,
                    extra: target.extra,
                }),
                Err(outcome) => return Ok(outcome),
            },
        };
        let context = self.context;
        Ok(match self.client.uia() {
            Some(uia) => super::uia_object::follow(context, uia, (target, followed)),
            None => StepOutcome::Failed,
        })
    }

    /// `Test` of the application's focus, through its backend's arm with
    /// its focused state, the read kept for the read of the focus that
    /// follows.
    fn test(&mut self, subject: &Subject, matcher: &verbatim_model::matcher::Matcher) -> Stepped {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let Ok(Held::Focus(hwnd, kept)) = &mut self.held[place].1 else {
            return Ok(StepOutcome::Refused(StepRefusal::Unsupported));
        };
        let hwnd = *hwnd;
        let (known, set, details) = self
            .focus_read
            .clone()
            .unwrap_or_else(|| (Vec::new(), Vec::new(), verbatim_model::TextDetails::NONE));
        let (now, tested) = super::focus::focus_now(
            context,
            self.client,
            (hwnd, Some(matcher)),
            (&known, &set, details),
        );
        let Some((holds, values)) = tested else {
            return Ok(StepOutcome::NotThere(Missing::Object));
        };
        *kept = Some(now);
        Ok(StepOutcome::Done(Done::Test(Tested {
            matched: holds,
            node: NodeRead {
                node: None,
                window: asked::node_window(hwnd),
                values: PropertyValues(values),
            },
        })))
    }

    /// `Related` of a UI Automation node: a step through the tree by the
    /// navigation the outpost makes for object navigation; the
    /// controller-for relation and the first selected item through the
    /// program builder.
    fn related(&mut self, subject: &Subject, relation: Relation, set: &[PropertyId]) -> Stepped {
        let node = match self.node(subject)? {
            Ok(node) => node,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let kind = match relation {
            Relation::Parent => QueryKind::Parent,
            Relation::FirstChild => QueryKind::FirstChild,
            Relation::NextSibling => QueryKind::NextSibling,
            Relation::PreviousSibling => QueryKind::PreviousSibling,
            // A UI Automation node's last child, controller and first
            // selected item are the executor's ([`Self::on_uia_node`]),
            // and an MSAA node's every relation the MSAA executor's.
            Relation::LastChild | Relation::ControllerFor | Relation::FirstSelectedItem => {
                tracing::error!(target: verbatim_model::DEFECT_TARGET, ?relation, "an executor's relation reached the dispatcher's own");
                return Ok(StepOutcome::Failed);
            }
        };
        Ok(match read::navigate(context, self.client, node, kind) {
            Ok(reached) => StepOutcome::Done(Done::Related(
                reached
                    .map(|reached| asked::node_read(context, &reached, None, set))
                    .into_iter()
                    .collect(),
            )),
            Err(error) => failed(&error, node),
        })
    }

    /// `Ancestors`: the walk object navigation makes, cut at the first
    /// ancestor Core knows and at `depth`.
    fn ancestors(
        &mut self,
        subject: &Subject,
        until: &[KnownAncestor],
        depth: u32,
        set: &[PropertyId],
    ) -> Stepped {
        let node = match self.node(subject)? {
            Ok(node) => node,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let chain = match self.chain(node) {
            Ok(chain) => chain,
            Err(error) => return Ok(failed(&error, node)),
        };
        let (mut ancestors, met) = asked::cut_at_known(context, chain, until);
        let end = match met {
            Some(place) => AncestorsEnd::Met(place),
            None if ancestors.len() > usize::try_from(depth).unwrap_or(usize::MAX) => {
                ancestors.truncate(usize::try_from(depth).unwrap_or(usize::MAX));
                AncestorsEnd::DepthLimit
            }
            None => AncestorsEnd::Top,
        };
        let nearest = asked::known_nearest_of(context, node);
        Ok(StepOutcome::Done(Done::Ancestors(Ancestry {
            ancestors: asked::ancestor_reads(context, &ancestors, nearest, set),
            end,
        })))
    }

    /// The whole chain of ancestors above `node`, outermost first: a UI
    /// Automation node's by its executor's walk chained above where it
    /// ends ([`read::chain`]), as object navigation reads it, reusing
    /// nothing and waiting for the application as long as it takes; an
    /// MSAA node's by its executor's walk to the top.
    fn chain(&mut self, node: NodeId) -> Result<Vec<NodeSnapshot>, ReadError> {
        let context = self.context;
        if context.store.runtime_id_of(node).is_none() {
            return super::msaa_object::ancestors(context, self.client, node);
        }
        let (uia, _, element) = read::uia_node(context, self.client, node)?;
        let request = context.ancestor_request(uia).map_err(|error| {
            ReadError::Failed(format!("could not build a cache request: {error}"))
        })?;
        let own_window = element.read().role.hwnd;
        read::chain(
            context,
            read::ChainStart::Uia {
                uia: (uia, &request),
                element: (element.live(), own_window, own_window),
                parent_automation_id: &mut None,
                visited: None,
            },
            (&[], None),
        )
        .map_err(|read::FocusLost| {
            ReadError::Failed("a UIA window's element on the way could not be read".to_owned())
        })
    }

    /// `Descendants`: each start's subtree in tree order, each node judged
    /// by the matcher and the pruning, the walk stopped at its limit and
    /// its node cap: a UI Automation start's through the program builder,
    /// which walks the control view by calls; any other's by the children
    /// walk. A window's start is its own object, and an MSAA object's is
    /// read as an object an event names.
    fn descendants(&mut self, subject: &Subject, walk: &Walk) -> Stepped {
        let starts: Vec<(NodeId, Option<NodeSnapshot>)> =
            match subject.earlier_nodes(&self.outcomes) {
                Some(nodes) => nodes
                    .iter()
                    .filter_map(|read| read.node)
                    .map(|node| (node, None))
                    .collect(),
                None => match self.start_of(subject)? {
                    Ok(start) => vec![start],
                    Err(outcome) => return Ok(outcome),
                },
            };
        let mut walked = Walked {
            found: Vec::new(),
            visited: 0,
            complete: true,
        };
        for (root, (start, snapshot)) in starts.into_iter().enumerate() {
            let root = u32::try_from(root).unwrap_or(u32::MAX);
            let ended = if self.context.store.runtime_id_of(start).is_some() {
                self.descend_uia(start, root, walk, &mut walked)?
            } else {
                self.descend_msaa((start, snapshot), root, walk, &mut walked)
            };
            match ended {
                Ok(true) => break,
                Ok(false) => {}
                Err(outcome) => return Ok(*outcome),
            }
        }
        Ok(StepOutcome::Done(Done::Descendants(Descended {
            nodes: walked.found,
            complete: walked.complete,
        })))
    }

    /// The node a walk down the tree starts from, as `subject` names it,
    /// with its snapshot when resolving it read one.
    fn start_of(
        &mut self,
        subject: &Subject,
    ) -> Result<Result<(NodeId, Option<NodeSnapshot>), StepOutcome>, Outcome> {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(Err(outcome)),
        };
        let context = self.context;
        let read = match &self.held[place].1 {
            Ok(Held::Node(node)) => return Ok(Ok((*node, None))),
            Ok(Held::Window(hwnd, Backend::Uia)) => {
                super::uia_object::window(context, self.client, *hwnd)
            }
            Ok(Held::Window(hwnd, Backend::Msaa)) => {
                super::msaa_object::window(context, self.client, *hwnd)
            }
            Ok(Held::Msaa(Some(candidate))) => Some(
                candidate
                    .object()
                    .read(context.nodes(), verbatim_ia2::acquire::Purpose::Announce),
            ),
            Ok(Held::Msaa(None) | Held::Focus(..) | Held::Route(_)) | Err(_) => {
                return Ok(Err(StepOutcome::Refused(StepRefusal::Unsupported)));
            }
        };
        Ok(match read {
            Some(snapshot) => Ok((snapshot.id, Some(snapshot))),
            None => Err(StepOutcome::NotThere(Missing::Object)),
        })
    }

    /// Walks the UI Automation node `start`, the walk's start at `root`,
    /// through the executor ([`super::uia_object::programmed`]), into
    /// `walked`, what is left of the limit and the node cap after the
    /// starts before it: `Ok(true)` when the walk met its limit or its
    /// node cap.
    fn descend_uia(
        &mut self,
        start: NodeId,
        root: u32,
        walk: &Walk,
        walked: &mut Walked,
    ) -> Result<Result<bool, Box<StepOutcome>>, Outcome> {
        use verbatim_uia_rops::batch::{Found, Judged};
        let context = self.context;
        let judge = |node: &verbatim_uia::Cached<verbatim_uia::Node>| {
            let (matched, pruned) = judged(context, &node.snapshot(context.nodes()), walk);
            Judged { matched, pruned }
        };
        let left =
            |cap: u32, used: usize| cap.saturating_sub(u32::try_from(used).unwrap_or(u32::MAX));
        let op = Op::Descendants {
            subject: Subject::Node(start),
            walk: Box::new(Walk {
                limit: left(walk.limit, walked.found.len()),
                nodes: left(walk.nodes, walked.visited),
                ..walk.clone()
            }),
        };
        let descendants =
            match super::uia_object::programmed(context, self.client, (start, op), Some(&judge))? {
                Ok(Found::Descendants(descendants)) => descendants,
                Ok(_) => return Ok(Err(Box::new(StepOutcome::Failed))),
                Err(outcome) => return Ok(Err(Box::new(outcome))),
            };
        let offset = u32::try_from(walked.found.len()).unwrap_or(u32::MAX);
        walked.visited = walked.visited.saturating_add(descendants.nodes.len());
        walked.complete &= descendants.complete;
        for descendant in descendants.nodes {
            walked.found.push(Descendant {
                node: asked::node_read(
                    context,
                    &descendant.node.snapshot(context.nodes()),
                    None,
                    &walk.set,
                ),
                parent: descendant
                    .parent
                    .map(|parent| parent.saturating_add(offset)),
                root,
            });
        }
        Ok(Ok(!descendants.complete
            || walked.found.len()
                >= usize::try_from(walk.limit).unwrap_or(usize::MAX)))
    }

    /// Walks `start`, the walk's start at `root`, its snapshot `snapshot`
    /// when resolving it read one, through the MSAA executor, into
    /// `walked`: `true` when the walk met its limit or its node cap.
    fn descend_msaa(
        &mut self,
        (start, snapshot): (NodeId, Option<NodeSnapshot>),
        root: u32,
        walk: &Walk,
        walked: &mut Walked,
    ) -> Result<bool, Box<StepOutcome>> {
        let context = self.context;
        let mut descended = verbatim_ia2::batch::Descended {
            before: walked.found.len(),
            reached: Vec::new(),
            visited: walked.visited,
            complete: walked.complete,
        };
        let ended = verbatim_ia2::batch::descend(
            &super::msaa_object::MsaaNodes::new(context, self.client),
            (start, snapshot),
            walk,
            &mut descended,
        );
        walked.visited = descended.visited;
        walked.complete = descended.complete;
        walked
            .found
            .extend(descended.reached.into_iter().map(|reached| Descendant {
                node: asked::node_read(context, &reached.node, None, &walk.set),
                parent: reached.parent,
                root,
            }));
        ended.map_err(|error| Box::new(failed(&ReadError::from(error), start)))
    }

    /// `FocusRead`: an MSAA candidate's full read, the focus-now read, or
    /// the completion of a route the listener read.
    fn focus_read(
        &mut self,
        subject: &Subject,
        (known, set, details): (&[KnownAncestor], &[PropertyId], verbatim_model::TextDetails),
    ) -> Stepped {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let read = match &mut self.held[place].1 {
            Ok(Held::Msaa(candidate)) => candidate.take().and_then(|candidate| {
                super::focus::candidate(context, self.client, *candidate, (known, set, details))
            }),
            Ok(Held::Focus(hwnd, tested)) => {
                let hwnd = *hwnd;
                let now = tested.take().unwrap_or_else(|| {
                    super::focus::focus_now(
                        context,
                        self.client,
                        (hwnd, None),
                        (known, set, details),
                    )
                    .0
                });
                match now {
                    FocusNow::Read(read) => Some(*read),
                    FocusNow::Unread => None,
                }
            }
            Ok(Held::Route(route)) => {
                let read =
                    super::uia_focus::route(context, self.client, route, (known, set, details));
                return Ok(match read {
                    Ok(read) => StepOutcome::Done(Done::FocusRead(read)),
                    Err(unread) => unread.outcome(),
                });
            }
            Ok(Held::Node(_) | Held::Window(..)) | Err(_) => {
                return Ok(StepOutcome::Refused(StepRefusal::Unsupported));
            }
        };
        Ok(read.map_or(StepOutcome::NotThere(Missing::Object), |read| {
            StepOutcome::Done(Done::FocusRead(Box::new(read)))
        }))
    }

    /// A text operation, through its node's text, read by its backend.
    fn text(&mut self, op: &Op) -> Stepped {
        let Some(subject) = op.subject() else {
            return Ok(StepOutcome::Failed);
        };
        let node = match self.node(subject)? {
            Ok(node) => node,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let step = match op {
            Op::Caret { adopt, .. } => {
                text_reads::adopt_told(context, node, *adopt);
                verbatim_ia2::text::TextStep::Caret
            }
            Op::CompareSelection { before, after, .. } => {
                let before = match before {
                    Before::Baseline(anchor) => Some(*anchor),
                    Before::SelectionOf(place) => self.caret_of(*place).map(|caret| caret.held),
                };
                let after = self.caret_of(*after).map(|caret| caret.held);
                let (Some(before), Some(after)) = (before, after) else {
                    return Ok(StepOutcome::Failed);
                };
                verbatim_ia2::text::TextStep::Compare(before, after)
            }
            Op::MoveBySentence { from, forward, .. } => {
                verbatim_ia2::text::TextStep::Sentence(*from, *forward)
            }
            Op::Visible { .. }
            | Op::Move { .. }
            | Op::FindText { .. }
            | Op::ReadColours { .. }
            | Op::ReadText { whole: true, .. }
            | Op::ReadText { trim: true, .. }
            | Op::ReadRange { trim: true, .. } => verbatim_ia2::text::TextStep::Unsupported,
            Op::WaitForCaret { .. } => {
                // A wait is only ever a batch's first step, and such a
                // batch waits in the table of waiting requests.
                tracing::error!(target: verbatim_model::DEFECT_TARGET, "a caret wait reached the worker's batch");
                return Ok(StepOutcome::Failed);
            }
            op => match self.call_step(op, context.observed_now()) {
                Some(step) => verbatim_ia2::text::TextStep::Calls(step),
                None => return Ok(StepOutcome::Failed),
            },
        };
        Ok(verbatim_ia2::text::step(
            step,
            node,
            &text_reads::EditFacts(context),
        ))
    }

    /// The step a read or move by points runs as call by call, its points
    /// resolved, begun at `began_us`: `None` when a point names a result
    /// not of its kind, which the parse rules out.
    fn call_step(&self, op: &Op, began_us: u64) -> Option<crate::text::CallStep> {
        let point = |point: &Point| self.point(*point);
        let through = |through: &Option<verbatim_model::batch::Through>| match through {
            Some(through) => Some(Some((point(&through.start)?, point(&through.end)?))),
            None => Some(None),
        };
        Some(match op {
            Op::ReadText {
                at,
                movement,
                unit,
                count,
                ..
            } => crate::text::CallStep::Read {
                at: point(at)?,
                movement: *movement,
                unit: *unit,
                count: u8::try_from(*count).ok()?,
            },
            Op::ReadRange {
                start,
                end,
                through: past,
                ..
            } => crate::text::CallStep::Range {
                ends: (point(start)?, point(end)?),
                through: through(past)?,
            },
            Op::Select {
                start,
                end,
                through: past,
                ..
            } => crate::text::CallStep::Select {
                ends: (point(start)?, point(end)?),
                through: through(past)?,
                began_us,
            },
            Op::Locate { at, .. } => crate::text::CallStep::Locate(point(at)?),
            _ => return None,
        })
    }

    /// The text point `point` names: one Core names, or a point of an
    /// earlier `Caret` or `ReadText`, as the anchor it holds there.
    fn point(&self, point: Point) -> Option<TextPoint> {
        let at = |anchor| TextPoint::At(TextPosition::at(anchor));
        match point {
            Point::At(point) => Some(point),
            Point::Of(place, PointOf::ChunkStart) => match self.outcomes.get(place.index()) {
                Some(StepOutcome::Done(Done::ReadText(read))) => {
                    read.chunks.first().map(|chunk| at(chunk.start))
                }
                _ => None,
            },
            Point::Of(place, which) => {
                let caret = self.caret_of(place)?;
                let selection = caret.selection;
                Some(at(match (which, selection) {
                    (PointOf::SelectionStart, Some(selection)) => selection.start,
                    (PointOf::SelectionEnd, Some(selection)) => selection.end,
                    _ => caret.caret,
                }))
            }
        }
    }

    /// The result of the earlier `Caret` at `place`.
    fn caret_of(&self, place: verbatim_model::batch::Place) -> Option<CaretAt> {
        match self.outcomes.get(place.index()) {
            Some(StepOutcome::Done(Done::Caret(caret))) => Some(*caret),
            _ => None,
        }
    }
}

/// Property `id` of a snapshot read already.
fn value_of(
    context: &RequestContext<'_>,
    node: &NodeSnapshot,
    id: PropertyId,
) -> Option<PropertyValue> {
    asked::values(context, node, &[id])
        .0
        .pop()
        .and_then(|answer| answer.value)
}

/// The outcome of a read of node `node` that failed: not there for a node
/// gone, else failed, its reason logged.
fn failed(error: &ReadError, node: NodeId) -> StepOutcome {
    match error {
        ReadError::Gone => StepOutcome::NotThere(Missing::Node(node)),
        ReadError::Failed(reason) => {
            tracing::debug!(reason, "a batch step's read failed");
            StepOutcome::Failed
        }
    }
}

/// The outcome of node `node`'s activation that walked up to `activated`:
/// refused as unsupported when every node tried was there and none had an
/// action, not there for the node itself gone, and failed when a call
/// failed or found a parent gone.
fn activation_outcome(activated: read::Activated, node: NodeId) -> StepOutcome {
    match activated {
        read::Activated::Done(action) => StepOutcome::Done(Done::Activate(action)),
        read::Activated::NoAction => StepOutcome::Refused(StepRefusal::Unsupported),
        read::Activated::Gone => StepOutcome::NotThere(Missing::Node(node)),
        read::Activated::Failed(reason) => {
            tracing::debug!(reason, "an activation failed");
            StepOutcome::Failed
        }
    }
}

/// The outcome of a text step on `node` whose text could not be opened,
/// `reply` saying why (the design's section 6, the reply rows).
fn text_refused(reply: TextFailure, node: NodeId) -> StepOutcome {
    match reply {
        TextFailure::NoText => StepOutcome::Refused(StepRefusal::NoText),
        TextFailure::Gone => StepOutcome::NotThere(Missing::Node(node)),
        TextFailure::Unanswered => StepOutcome::Failed,
    }
}
