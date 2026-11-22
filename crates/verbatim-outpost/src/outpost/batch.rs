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
//! acquisition, focused-state walk and full read, the focus-now read, and
//! the text reads through each backend's adapter. The controller-for relation and the first selected
//! item run through the program builder ([`verbatim_uia_rops::batch`]), as
//! does each run of consecutive text steps on one UI Automation node
//! (`Caret`, `ReadText`, `ReadRange`, `CompareSelection`, `Select`,
//! `Locate`); an edit control's text is read by its messages, step by
//! step.

#![forbid(unsafe_code)]

use verbatim_model::batch::{
    AncestorsEnd, Ancestry, Batch, Before, CaretAt, Descendant, Descended, Done, Gate,
    KindMismatch, Missing, Op, Point, PointOf, ReaderKind, Relation, StepOutcome, StepRefusal,
    Subject, Tested, Walk, WindowOf, parse,
};
use verbatim_model::property::{
    NodeDetailSet, PropertyAnswer, PropertyId, PropertyValue, PropertyValues,
};
use verbatim_model::request::{
    Ask, Followed, HeldIds, KnownAncestor, MsaaIds, NodeRead, Outcome, Probe, Refusal, Reply, Route,
};
use verbatim_model::{
    Backend, NodeId, NodeSnapshot, QueryKind, TextPoint, TextPosition, TextReply,
};

use super::RequestContext;
use super::asked::{self, FocusNow};
use super::read::{self, Client, ReadError};
use super::text_reads;
use super::window::{hwnd_of, window_handle};
use crate::text::TextError;

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

/// The answer to `batch`, `ask`'s, read on the worker; `probes` records
/// the probe of a window with no verdict in force.
pub(super) fn answer(
    context: &RequestContext<'_>,
    client: &mut Client,
    ask: &Ask,
    batch: &Batch,
    probes: &mut Vec<Probe>,
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
    let verdict = asked::verdict(context, hwnd_of(ask.window), ask.verdict, probes);
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
        probes,
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
    let mut probes = Vec::new();
    let run = Run {
        context,
        client,
        given: verbatim_model::request::WindowVerdict::ProbeFirst,
        probes: &mut probes,
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
    probes: &'r mut Vec<Probe>,
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
        // The end of a run of fusable text steps found to be read one at a
        // time, so it is not tried again for each of its steps.
        let mut single_until = 0;
        while at < steps.len() {
            if at >= single_until {
                let end = self.fusable_end(steps, at);
                if end > at {
                    let first = self.outcomes.len();
                    if let Some(outcomes) = self.fused(&steps[at..end], first) {
                        self.outcomes.extend(outcomes);
                        at = end;
                        continue;
                    }
                    single_until = end;
                }
            }
            let step = &steps[at];
            let outcome = match step.gate(&self.outcomes) {
                Ok(Gate::Skip) => StepOutcome::Skipped,
                Ok(Gate::Run) => match self.step(&step.op) {
                    Ok(outcome) => outcome,
                    Err(whole) => return whole,
                },
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
        let verdict = asked::verdict(self.context, hwnd_of(window), given, self.probes);
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

    /// The end of the run of fusable text steps on one UI Automation node
    /// that starts at `at` (the request list's design, section 5): `at`
    /// when the step there is not one.
    fn fusable_end(&self, steps: &[verbatim_model::batch::Step], at: usize) -> usize {
        let node_of = |op: &Op| match op {
            Op::Caret { subject, .. }
            | Op::ReadText { subject, .. }
            | Op::ReadRange { subject, .. }
            | Op::CompareSelection { subject, .. }
            | Op::Select { subject, .. }
            | Op::Locate { subject, .. } => match subject {
                Subject::Node(node) => Some(*node),
                _ => None,
            },
            _ => None,
        };
        let Some(node) = steps.get(at).and_then(|step| node_of(&step.op)) else {
            return at;
        };
        if self.context.store.runtime_id_of(node).is_none() {
            return at;
        }
        at + steps[at..]
            .iter()
            .take_while(|step| node_of(&step.op) == Some(node))
            .count()
    }

    /// Runs `steps`, a run of fusable text steps on one UI Automation node
    /// starting at place `first`, through the program builder: their
    /// outcomes, or `None` when the node's text is read otherwise, for the
    /// steps to run one at a time.
    fn fused(
        &mut self,
        steps: &[verbatim_model::batch::Step],
        first: usize,
    ) -> Option<Vec<StepOutcome>> {
        let subject = steps.first()?.op.subject()?;
        let Ok(Ok(node)) = self.node(subject) else {
            return None;
        };
        let context = self.context;
        let uia = self.client.uia()?;
        text_reads::fused_steps(context, uia, node, (steps, first, &self.outcomes))
    }

    fn step(&mut self, op: &Op) -> Stepped {
        if let Some(outcome) = self.on_uia_node(op)? {
            return Ok(outcome);
        }
        match op {
            Op::Read { subject, set } => self.read(subject, set),
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
            Op::IsOneOf {
                subject,
                equal,
                at_address,
            } => self.is_one_of(subject, *equal, at_address),
            Op::Activate { subject } => Ok(match self.node(subject)? {
                Err(outcome) => outcome,
                Ok(node) => match read::activate(self.context, self.client, node) {
                    Ok(action) => StepOutcome::Done(Done::Activate(action)),
                    Err(error) => failed(&error, node),
                },
            }),
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
                relation: Relation::LastChild,
                ..
            } => subject,
            Op::Follow(target) => return self.follow_step(target.as_ref()).map(Some),
            _ => return Ok(None),
        };
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(Some(outcome)),
        };
        let node = match &self.held[place].1 {
            Ok(Held::Node(node)) if self.context.store.runtime_id_of(*node).is_some() => *node,
            _ => return Ok(None),
        };
        let context = self.context;
        Ok(Some(match op {
            Op::Related { set, .. } => match read::last_child(context, self.client, node) {
                Ok(reached) => StepOutcome::Done(Done::Related(
                    reached
                        .map(|reached| asked::node_read(context, &reached, None, set))
                        .into_iter()
                        .collect(),
                )),
                Err(error) => failed(&error, node),
            },
            Op::Activate { .. } => match read::activate(context, self.client, node) {
                Ok(action) => StepOutcome::Done(Done::Activate(action)),
                Err(error) => failed(&error, node),
            },
            op => match self.client.uia() {
                Some(uia) => super::uia_object::step(context, uia, node, op),
                None => StepOutcome::Failed,
            },
        }))
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

    /// `Read`: an MSAA node's properties with those read apart; an MSAA
    /// object's each read alone; a window's own object's through its
    /// backend.
    fn read(&mut self, subject: &Subject, set: &[PropertyId]) -> Stepped {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        Ok(match &mut self.held[place].1 {
            Ok(Held::Node(node)) => {
                let node = *node;
                match asked::properties(context, self.client, node, set) {
                    Ok(mut values) => {
                        reported(context, node, &mut values);
                        let window = asked::known_nearest_of(context, node).unwrap_or(0);
                        StepOutcome::Done(Done::Read(NodeRead {
                            node: Some(node),
                            window: asked::node_window(window),
                            values,
                        }))
                    }
                    Err(error) => failed(&error, node),
                }
            }
            Ok(Held::Msaa(Some(candidate))) => {
                let hwnd = candidate.key().0;
                let client = &mut *self.client;
                let mut read = |id| candidate_property((context, client), candidate, id);
                StepOutcome::Done(Done::Read(NodeRead {
                    node: None,
                    window: asked::node_window(hwnd),
                    values: PropertyValues(
                        set.iter()
                            .map(|&id| PropertyAnswer {
                                id,
                                value: read(id),
                            })
                            .collect(),
                    ),
                }))
            }
            Ok(Held::Window(hwnd, backend)) => {
                let (hwnd, backend) = (*hwnd, *backend);
                match read::window_node(context, self.client, hwnd, backend) {
                    Some(node) => StepOutcome::Done(Done::Read(asked::node_read(
                        context,
                        &node,
                        Some(hwnd),
                        set,
                    ))),
                    None => StepOutcome::NotThere(Missing::Object),
                }
            }
            Ok(Held::Route(route)) => {
                match super::completion::object(context, self.client, route, set) {
                    Ok(read) => StepOutcome::Done(Done::Read(read)),
                    Err(unread) => unread.outcome(),
                }
            }
            Ok(Held::Msaa(None) | Held::Focus(..)) | Err(_) => {
                StepOutcome::Refused(StepRefusal::Unsupported)
            }
        })
    }

    /// `Test`: the matcher evaluated lazily, each property read at most
    /// once when the matcher first needs it, in its order.
    #[expect(clippy::too_many_lines, reason = "one arm per kind of object tested")]
    fn test(&mut self, subject: &Subject, matcher: &verbatim_model::matcher::Matcher) -> Stepped {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let mut read = Vec::new();
        let (node, window, holds) = match &mut self.held[place].1 {
            Ok(Held::Msaa(Some(candidate))) => {
                let hwnd = candidate.key().0;
                let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut read, |id| {
                    Ok(candidate_property(
                        (context, &mut *self.client),
                        candidate,
                        id,
                    ))
                });
                (None, hwnd, holds)
            }
            Ok(Held::Window(hwnd, backend)) => {
                let (hwnd, backend) = (*hwnd, *backend);
                let mut snapshot: Option<Option<NodeSnapshot>> = None;
                let client = &mut *self.client;
                let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut read, |id| {
                    let client_role = verbatim_ia2::acquire::client_role;
                    Ok(
                        window_property((hwnd, backend), id, client_role).unwrap_or_else(|| {
                            snapshot
                                .get_or_insert_with(|| {
                                    read::window_node(context, client, hwnd, backend)
                                })
                                .as_ref()
                                .and_then(|node| value_of(context, node, id))
                        }),
                    )
                });
                (None, hwnd, holds)
            }
            Ok(Held::Node(node)) => {
                let node = *node;
                let mut snapshot: Option<NodeSnapshot> = None;
                let read_node = evaluate(matcher, &mut read, |id| {
                    let mut values = PropertyValues(vec![PropertyAnswer { id, value: None }]);
                    if !is_reported(id) {
                        if snapshot.is_none() {
                            snapshot = Some(read::read_node(context, node)?);
                        }
                        if let Some(snapshot) = &snapshot {
                            values = asked::values(context, snapshot, &[id]);
                        }
                    }
                    reported(context, node, &mut values);
                    Ok(values.0.pop().and_then(|answer| answer.value))
                });
                let holds = match read_node {
                    Ok(holds) => holds,
                    Err(error) => return Ok(failed(&error, node)),
                };
                let window = asked::known_nearest_of(context, node).unwrap_or(0);
                (Some(node), window, holds)
            }
            Ok(Held::Focus(hwnd, kept)) => {
                let hwnd = *hwnd;
                // The focus read as a read of it with its focused state
                // reads it, in full only when focused, kept for the read
                // of the focus that follows.
                let (known, set, details) = self
                    .focus_read
                    .clone()
                    .unwrap_or_else(|| (Vec::new(), Vec::new(), verbatim_model::TextDetails::NONE));
                let now = asked::focus_now(
                    context,
                    self.client,
                    Ok(hwnd),
                    &set,
                    (&known, details, read::FocusDepth::FullIfFocused),
                );
                let (focused, values) = match &now {
                    FocusNow::Read(focused, focus) => (
                        focused.map(PropertyValue::Flag),
                        Some(&focus.focus.read.values),
                    ),
                    FocusNow::Unfocused => (Some(PropertyValue::Flag(false)), None),
                    FocusNow::NotThere => return Ok(StepOutcome::NotThere(Missing::Object)),
                };
                let Ok(holds) = evaluate::<std::convert::Infallible>(matcher, &mut read, |id| {
                    Ok(match id {
                        PropertyId::FocusedWithin => focused.clone(),
                        id => values
                            .and_then(|values| values.answer(id))
                            .and_then(|answer| answer.value.clone()),
                    })
                });
                *kept = Some(now);
                (None, hwnd, holds)
            }
            Ok(Held::Msaa(None) | Held::Route(_)) | Err(_) => {
                return Ok(StepOutcome::Refused(StepRefusal::Unsupported));
            }
        };
        Ok(StepOutcome::Done(Done::Test(Tested {
            matched: holds,
            node: NodeRead {
                node,
                window: asked::node_window(window),
                values: PropertyValues(read),
            },
        })))
    }

    /// `Related`: a step through the tree by the navigation the outpost
    /// makes for object navigation; the controller-for relation and the
    /// first selected item through the program builder.
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
            Relation::LastChild => {
                return Ok(match asked::children(context, self.client, node) {
                    // A list cut short has no last child known.
                    Ok((_, false)) => StepOutcome::Failed,
                    Ok((children, true)) => StepOutcome::Done(Done::Related(
                        children
                            .last()
                            .map(|last| asked::node_read(context, last, None, set))
                            .into_iter()
                            .collect(),
                    )),
                    Err(error) => failed(&error, node),
                });
            }
            Relation::ControllerFor | Relation::FirstSelectedItem => {
                return self.related_by_program(node, relation, set);
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

    /// A relation only UI Automation has, through the program builder: an
    /// MSAA object controls nothing and has no selected item of its own.
    fn related_by_program(
        &mut self,
        node: NodeId,
        relation: Relation,
        set: &[PropertyId],
    ) -> Stepped {
        use verbatim_uia_rops::batch::{Found, Ran};
        let context = self.context;
        if context.store.runtime_id_of(node).is_none() {
            return Ok(StepOutcome::Done(Done::Related(Vec::new())));
        }
        let Some(element) = context
            .store
            .element_of(node)
            .and_then(|agile| agile.resolve().ok())
        else {
            return Ok(StepOutcome::NotThere(Missing::Node(node)));
        };
        let Ok((uia, nodes)) = self.client.uia_with(context, RequestContext::node_request) else {
            return Ok(StepOutcome::Failed);
        };
        let fill = verbatim_uia::Fill::new(&nodes);
        let window = asked::known_nearest_of(context, node);
        let from = read::OneElement {
            element: element.live().element(),
            window: read::kept_own_window(context, node).unwrap_or(0),
            calls_only: false,
            fill: &fill,
            judge: None,
        };
        let op = Op::Related {
            subject: Subject::Node(node),
            relation,
            set: Vec::new(),
        };
        let answer = from.run(context, uia, op, window);
        if answer.read_classically
            && let Some(window) = window
        {
            context.read_classically(verbatim_win32::window::top_level_of(window));
        }
        Ok(match answer.steps.into_iter().next() {
            Some(Ran::Done(Found::Related(related))) => StepOutcome::Done(Done::Related(
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
            )),
            Some(Ran::Gone) => StepOutcome::NotThere(Missing::Node(node)),
            Some(Ran::TimedOut) => return Err(Outcome::NotResponding),
            _ => StepOutcome::Failed,
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
        let chain = match read::ancestors(context, self.client, node) {
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
                self.descend_by_children(start, snapshot, root, walk, &mut walked)
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
            Ok(Held::Window(hwnd, backend)) => {
                read::window_node(context, self.client, *hwnd, *backend)
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
    /// through the program builder, into `walked`: `Ok(true)` when the
    /// walk met its limit or its node cap.
    fn descend_uia(
        &mut self,
        start: NodeId,
        root: u32,
        walk: &Walk,
        walked: &mut Walked,
    ) -> Result<Result<bool, Box<StepOutcome>>, Outcome> {
        use verbatim_uia_rops::batch::{Found, Judged, Ran};
        let context = self.context;
        let Some(element) = context
            .store
            .element_of(start)
            .and_then(|agile| agile.resolve().ok())
        else {
            return Ok(Err(Box::new(StepOutcome::NotThere(Missing::Node(start)))));
        };
        let request = if walk.set.contains(&PropertyId::Rect) {
            RequestContext::located_many_node_request
        } else {
            RequestContext::many_node_request
        };
        let Ok((uia, nodes)) = self.client.uia_with(context, request) else {
            return Ok(Err(Box::new(StepOutcome::Failed)));
        };
        let fill = verbatim_uia::Fill::new(&nodes);
        let judge = |node: &verbatim_uia::Cached<verbatim_uia::Node>| {
            let (matched, pruned) = judged(context, &node.snapshot(context.nodes()), walk);
            Judged { matched, pruned }
        };
        let window = asked::known_nearest_of(context, start);
        let left =
            |cap: u32, used: usize| cap.saturating_sub(u32::try_from(used).unwrap_or(u32::MAX));
        let from = read::OneElement {
            element: element.live().element(),
            window: read::kept_own_window(context, start).unwrap_or(0),
            calls_only: false,
            fill: &fill,
            judge: Some(&judge),
        };
        let op = Op::Descendants {
            subject: Subject::Node(start),
            walk: Box::new(Walk {
                limit: left(walk.limit, walked.found.len()),
                nodes: left(walk.nodes, walked.visited),
                ..walk.clone()
            }),
        };
        let answer = from.run(context, uia, op, window);
        if answer.read_classically
            && let Some(window) = window
        {
            context.read_classically(verbatim_win32::window::top_level_of(window));
        }
        let descendants = match answer.steps.into_iter().next() {
            Some(Ran::Done(Found::Descendants(descendants))) => descendants,
            Some(Ran::Gone) => {
                return Ok(Err(Box::new(StepOutcome::NotThere(Missing::Node(start)))));
            }
            Some(Ran::TimedOut) => return Err(Outcome::NotResponding),
            _ => return Ok(Err(Box::new(StepOutcome::Failed))),
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
    /// when resolving it read one, by the children walk, into `walked`:
    /// `Ok(true)` when the walk met its limit or its node cap.
    fn descend_by_children(
        &mut self,
        start: NodeId,
        snapshot: Option<NodeSnapshot>,
        root: u32,
        walk: &Walk,
        walked: &mut Walked,
    ) -> Result<bool, Box<StepOutcome>> {
        let context = self.context;
        let limit = usize::try_from(walk.limit).unwrap_or(usize::MAX);
        let cap = usize::try_from(walk.nodes).unwrap_or(usize::MAX);
        let mut parent = None;
        if walk.include_subject {
            let snapshot = match snapshot {
                Some(snapshot) => snapshot,
                None => match read::read_node(context, start) {
                    Ok(snapshot) => snapshot,
                    Err(error) => return Err(Box::new(failed(&error, start))),
                },
            };
            let (matched, pruned) = judged(context, &snapshot, walk);
            if pruned {
                return Ok(false);
            }
            if matched {
                parent = Some(u32::try_from(walked.found.len()).unwrap_or(u32::MAX));
                walked.found.push(Descendant {
                    node: asked::node_read(context, &snapshot, None, &walk.set),
                    parent: None,
                    root,
                });
                if walked.found.len() >= limit {
                    return Ok(true);
                }
            }
        }
        // Each level's children with the depth below the start and the
        // place of the nearest answered ancestor.
        let mut stack = vec![(start, 0u32, parent)];
        while let Some((node, depth, parent)) = stack.pop() {
            if depth >= walk.depth {
                continue;
            }
            // A node gone, its children unknown, or not read, leaves the
            // walk incomplete.
            let Ok((children, complete)) = asked::children(context, self.client, node) else {
                walked.complete = false;
                continue;
            };
            walked.complete &= complete;
            let mut below = Vec::new();
            for child in children {
                walked.visited += 1;
                if walked.visited > cap {
                    walked.complete = false;
                    return Ok(true);
                }
                let (matched, pruned) = judged(context, &child, walk);
                if pruned {
                    continue;
                }
                let mut own = parent;
                if matched {
                    own = Some(u32::try_from(walked.found.len()).unwrap_or(u32::MAX));
                    walked.found.push(Descendant {
                        node: asked::node_read(context, &child, None, &walk.set),
                        parent,
                        root,
                    });
                    if walked.found.len() >= limit {
                        return Ok(true);
                    }
                }
                below.push((child.id, depth + 1, own));
            }
            // Depth first in tree order: the first child's subtree next.
            stack.extend(below.into_iter().rev());
        }
        Ok(false)
    }

    /// `IsOneOf`: the equal node met by NVDA's equality, the nodes at an
    /// address only at it; a node of the outpost by its id.
    fn is_one_of(
        &mut self,
        subject: &Subject,
        equal: Option<NodeId>,
        at_address: &[NodeId],
    ) -> Stepped {
        let place = match self.held(subject)? {
            Ok(place) => place,
            Err(outcome) => return Ok(outcome),
        };
        let context = self.context;
        let nodes: Vec<NodeId> = equal
            .into_iter()
            .chain(at_address.iter().copied())
            .collect();
        let found = match &mut self.held[place].1 {
            Ok(Held::Node(node)) => nodes.iter().position(|each| each == node),
            Ok(Held::Msaa(Some(candidate))) => {
                let mut object = candidate.object();
                let found = nodes.iter().enumerate().position(|(place, &node)| {
                    if place == 0 && equal.is_some() {
                        object.is_equal_to(node, context.nodes())
                    } else {
                        object.which_of(&[node], context.nodes()) == Some(node)
                    }
                });
                // The role the equality read serves the reads after it.
                candidate.keep_role_of(&object);
                found
            }
            Ok(Held::Msaa(None) | Held::Window(..) | Held::Focus(..) | Held::Route(_)) | Err(_) => {
                return Ok(StepOutcome::Refused(StepRefusal::Unsupported));
            }
        };
        Ok(StepOutcome::Done(Done::IsOneOf(
            found.and_then(|place| u32::try_from(place).ok()),
        )))
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
                asked::candidate_read(context, self.client, *candidate, (known, set, details))
            }),
            Ok(Held::Focus(hwnd, tested)) => {
                let hwnd = *hwnd;
                let now = tested.take().unwrap_or_else(|| {
                    asked::focus_now(
                        context,
                        self.client,
                        Ok(hwnd),
                        set,
                        (known, details, read::FocusDepth::Full),
                    )
                });
                match now {
                    FocusNow::Read(_, read) => Some(*read),
                    FocusNow::Unfocused | FocusNow::NotThere => None,
                }
            }
            Ok(Held::Route(route)) => {
                return Ok(
                    match super::completion::focus(
                        context,
                        self.client,
                        route,
                        (known, set, details),
                    ) {
                        Ok(read) => StepOutcome::Done(Done::FocusRead(read)),
                        Err(unread) => unread.outcome(),
                    },
                );
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
                return Ok(text_result(
                    text_reads::caret_step(context, node, *adopt),
                    node,
                    |caret| Ok(Done::Caret(caret)),
                ));
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
                return Ok(text_result(
                    text_reads::compare_step(context, node, (before, after)),
                    node,
                    |compared| {
                        compared
                            .map(Done::CompareSelection)
                            .map_err(Missing::Anchor)
                    },
                ));
            }
            Op::MoveBySentence {
                from,
                forward,
                details,
                ..
            } => {
                return Ok(
                    match text_reads::sentence_step(context, node, (*from, *forward), *details) {
                        Err(reply) => text_refused(&reply, node),
                        Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
                        Ok(Err(TextError::Failed(reason))) => {
                            tracing::debug!(reason, "a move by sentence failed");
                            StepOutcome::Failed
                        }
                        Ok(Ok(Err(outcome))) => outcome,
                        Ok(Ok(Ok(moved))) => StepOutcome::Done(Done::MoveBySentence(moved)),
                    },
                );
            }
            Op::TerminalRead { ask, .. } => {
                let Some(uia) = self.client.uia() else {
                    return Ok(StepOutcome::Refused(StepRefusal::NoText));
                };
                return Ok(text_reads::terminal_window_step(context, uia, node, ask));
            }
            Op::WaitForCaret { .. } => {
                // A wait is only ever a batch's first step, and such a
                // batch waits in the table of waiting requests.
                tracing::error!(target: verbatim_model::DEFECT_TARGET, "a caret wait reached the worker's batch");
                return Ok(StepOutcome::Failed);
            }
            op => match self.call_step(op, context.observed_now()) {
                Some(step) => step,
                None => return Ok(StepOutcome::Failed),
            },
        };
        let named: Vec<u64> = step.named().into_iter().map(|anchor| anchor.0).collect();
        let ran = text_reads::by_calls(context, node, step, &named);
        Ok(match ran {
            Ok(Ok(Err(refused))) => refused,
            Ok(Ok(Ok(done))) => StepOutcome::Done(done),
            Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
            Ok(Err(TextError::Failed(reason))) => {
                tracing::debug!(reason, "a batch's text step failed");
                StepOutcome::Failed
            }
            Err(reply) => text_refused(&reply, node),
        })
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
                count: *count,
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
                    (PointOf::SelectionStart, Some(selection)) => selection.start.anchor,
                    (PointOf::SelectionEnd, Some(selection)) => selection.end.anchor,
                    _ => caret.caret.anchor,
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

/// Property `id` of window `hwnd`'s own object read through `backend`, when
/// it is one the outpost reads of the window itself: whether it is shown,
/// from Windows with no call into the application; and, through MSAA, its
/// client object's role as `client_role` reads it, as the hidden-dialog
/// check reads it whatever the window's verdict (two counted calls).
/// `None` for a property read from the window's object.
#[expect(
    clippy::option_option,
    reason = "a value or none for the window's own properties, none for the rest"
)]
pub(super) fn window_property(
    (hwnd, backend): (isize, Backend),
    id: PropertyId,
    client_role: impl Fn(isize) -> Option<verbatim_model::Role>,
) -> Option<Option<PropertyValue>> {
    match id {
        PropertyId::WindowShown => Some(Some(PropertyValue::Flag(
            verbatim_win32::window::window_is_visible(hwnd),
        ))),
        PropertyId::Role if backend == Backend::Msaa => {
            Some(client_role(hwnd).map(PropertyValue::Role))
        }
        _ => None,
    }
}

/// Evaluates `matcher` with `read`, each property read kept in `read_back`
/// in the order read.
fn evaluate<E>(
    matcher: &verbatim_model::matcher::Matcher,
    read_back: &mut Vec<PropertyAnswer>,
    mut read: impl FnMut(PropertyId) -> Result<Option<PropertyValue>, E>,
) -> Result<bool, E> {
    matcher.evaluate(&mut |id| {
        let value = read(id)?;
        read_back.push(PropertyAnswer {
            id,
            value: value.clone(),
        });
        Ok(value)
    })
}

/// An MSAA candidate's property `id`: the four the outpost reports, or one
/// read alone by the identity check's reads.
fn candidate_property(
    (context, client): (&RequestContext<'_>, &mut Client),
    candidate: &mut verbatim_ia2::acquire::FocusCandidate,
    id: PropertyId,
) -> Option<PropertyValue> {
    match id {
        PropertyId::FocusedWithin => {
            let plain = |window| read::reads_as_plain(context, window);
            let mut parent =
                |window| read::parent_window_for_focused_state(context, client, window);
            Some(PropertyValue::Flag(candidate.has_focused_state(
                read::MAX_ANCESTOR_HOPS,
                &plain,
                &mut parent,
            )))
        }
        PropertyId::MsaaAddress => {
            let (window, object, child) = candidate.key();
            Some(PropertyValue::MsaaAddress(HeldIds {
                event: MsaaIds {
                    window: window_handle(window),
                    object,
                    child,
                },
                held_child: candidate.resolved_child(),
            }))
        }
        PropertyId::WindowShown => Some(PropertyValue::Flag(
            verbatim_win32::window::window_is_visible(candidate.key().0),
        )),
        id => candidate.property(id, context.nodes()),
    }
}

/// Whether property `id` is one the outpost reports itself, with no read of
/// the node's own properties.
fn is_reported(id: PropertyId) -> bool {
    matches!(
        id,
        PropertyId::FocusedWithin
            | PropertyId::MsaaAddress
            | PropertyId::WindowShown
            | PropertyId::IsContent
    )
}

/// Fills in `values` the properties of node `node` the outpost reports
/// itself: the MSAA ids it is held by, and whether its window is shown.
/// The focused state over its ancestors is an MSAA candidate's, and
/// whether it is content is the listener's and the focus read's, so a
/// node held already answers neither (Claude, 2026-11-21).
fn reported(context: &RequestContext<'_>, node: NodeId, values: &mut PropertyValues) {
    for answer in &mut values.0 {
        match answer.id {
            PropertyId::MsaaAddress => {
                answer.value = context.store.key_of(node).map(|(window, object, child)| {
                    PropertyValue::MsaaAddress(HeldIds {
                        event: MsaaIds {
                            window: window_handle(window),
                            object,
                            child,
                        },
                        held_child: child,
                    })
                });
            }
            PropertyId::WindowShown => {
                answer.value = read::kept_own_window(context, node)
                    .or_else(|| asked::known_nearest_of(context, node))
                    .map(|hwnd| {
                        PropertyValue::Flag(verbatim_win32::window::window_is_visible(hwnd))
                    });
            }
            _ => {}
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

/// The outcome of a text step on `node` whose read `ran`, its result made
/// by `done`.
fn text_result<T>(
    ran: Result<crate::text::TextResult<T>, TextReply>,
    node: NodeId,
    done: impl FnOnce(T) -> Result<Done, Missing>,
) -> StepOutcome {
    match ran {
        Err(reply) => text_refused(&reply, node),
        Ok(Err(TextError::Gone)) => StepOutcome::NotThere(Missing::Node(node)),
        Ok(Err(TextError::Failed(reason))) => {
            tracing::debug!(reason, "a batch's text step failed");
            StepOutcome::Failed
        }
        Ok(Ok(found)) => done(found).map_or_else(StepOutcome::NotThere, StepOutcome::Done),
    }
}

/// The outcome of a text step on `node` whose text could not be opened,
/// `reply` saying why (the design's section 6, the reply rows).
fn text_refused(reply: &TextReply, node: NodeId) -> StepOutcome {
    match reply {
        TextReply::NoText => StepOutcome::Refused(StepRefusal::NoText),
        TextReply::Gone => StepOutcome::NotThere(Missing::Node(node)),
        TextReply::Unanswered => StepOutcome::Failed,
        other => {
            tracing::error!(target: verbatim_model::DEFECT_TARGET, ?other, "a text step answered with a reply of another request");
            StepOutcome::Failed
        }
    }
}
