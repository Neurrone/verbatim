//! The reducer's thread and its shell (`docs/design/ownership-outpost-reducer.md`,
//! "The reducer and its thread"): the mailbox every producer appends to,
//! the thread that drains it into the reducer ([`super::reducer`]) and
//! applies the reducer's effects, the reducer's timers in its timed wait,
//! the queries' reply tokens, and the worker generations.
//!
//! The rule that lets the reducer keep time: every lock its thread takes is
//! a short hold with no call into an application and no wait inside it,
//! so a timer is never late by more than such a hold. They are the
//! mailbox's, the outbound backlog's with the store's inside it for a
//! release, the store's for its writes, and a walk slot's.
//!
//! What the shell holds that names an application's object, the elements
//! the focus-following subscription is moved to, is kept by token, so the
//! reducer holds none; an object the shell lets go of, an element no longer
//! followed, a parked event object, or what a release removed, is handed
//! to the next program dispatched, and dropped on its worker before the
//! program runs, never on this thread.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use verbatim_model::TraceId;
use verbatim_model::protocol::{EventTiming, GapAnswer, OutpostToCore, QueryOutcome, Responder};
use verbatim_uia::{Agile, Kept};

use super::alarm::Alarm;
use super::commit::{Effects, Writes};
use super::outbound::Carried;
use super::reducer::{
    Change, Cut, Effect, Entry, Facts, Group, Input, Item, Locals, Marks, Now, Program, Reducer,
    Request, Source, Subscription, Verdicts, Work,
};
use super::store::{Removed, Store, Tag};
use super::worker::{self, Charge, Job};
use super::{Context, RequestContext};

/// Times the gap record's bound in place of the reducer's own timer: for a
/// test that says when the bound is reached rather than waiting for it
/// (`Outpost::set_gap_timer`). Setting and cancelling never wait.
pub trait GapTimer: Send + Sync {
    /// Sets the timer for `due_us`, on the observation clock, replacing any
    /// time set before.
    fn set(&self, due_us: u64);
    /// Cancels the timer, if it is set.
    fn cancel(&self);
}

/// What the mailbox carries to the shell, in the order it was appended.
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from its producer into the mailbox, as it always has"
)]
pub(super) enum Posted {
    /// An entry, from where it came, with what a UIA focus's read answers
    /// of the gap record.
    Entry {
        entry: Entry,
        source: Source,
        read_answer: Option<GapAnswer>,
    },
    /// `Outpost::settle`, answered on `0` once handled.
    Settle(mpsc::Sender<()>),
    /// The outpost's hook is in.
    Hooked,
    /// No listener forwards to this outpost.
    WithoutListener,
    /// The listener's "forwarding ended".
    ForwardingEnded(u32),
    /// Core withdraws a query.
    Cancel(u64),
    /// A program's commit.
    Commit {
        request: Request,
        effects: Box<Effects>,
    },
    /// A program's walk was armed, its bound at the time given.
    Armed { request: Request, bound_at: Instant },
    /// A program's walk returned, past its bound or not.
    WalkReturned { request: Request, past: bool },
    /// A program ended, with the caret read it cut at its value change
    /// check, if it cut one.
    Finished {
        request: Request,
        reported: bool,
        panicked: bool,
        cut: Option<Box<Cut>>,
    },
    /// An abandoned worker returned from its call.
    Returned,
    /// A test times the gap record's bound with a timer of its own.
    GapTimerGiven,
    /// The gap record's bound was reached by the timer a test set.
    GapBound,
    /// The outpost is shutting down.
    Close,
}

/// The mailbox: an append-only queue under one lock, which every producer
/// appends to and only the shell drains, with the push-time copies of the
/// reducer's marks and verdicts that the producers' drops are judged by.
#[derive(Default)]
pub(super) struct Mailbox {
    state: Mutex<MailboxState>,
    /// The shell's wait: its event ends the wait when something is
    /// appended, and its high-resolution timer when the reducer's next
    /// timer comes.
    alarm: Alarm,
}

#[derive(Default)]
struct MailboxState {
    posted: VecDeque<Posted>,
    /// The reducer's marks, as its last change of them left them.
    marks: Marks,
    /// The reducer's verdicts, as its last change of them left them: the
    /// event thread's drop of an MSAA event of a window known to use UIA.
    verdicts: Arc<Verdicts>,
    /// Whether the shell waits, so that an append sets the alarm's event;
    /// taken by the first to set it.
    asleep: bool,
    closed: bool,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Appends `posted`, waking the shell if it waits. Nothing is taken
    /// once the outpost is shut down but what its programs still post.
    pub(super) fn post(&self, posted: Posted) {
        let mut state = self.lock();
        if state.closed
            && matches!(
                posted,
                Posted::Entry { .. }
                    | Posted::Settle(_)
                    | Posted::Hooked
                    | Posted::WithoutListener
                    | Posted::ForwardingEnded(_)
                    | Posted::Cancel(_)
                    | Posted::GapTimerGiven
                    | Posted::GapBound
            )
        {
            return;
        }
        state.posted.push_back(posted);
        self.rouse(state);
    }

    /// Appends `entry`, from `source`, unless the reducer's marks drop it
    /// as it is pushed, waking nobody ([`Marks::drops_on_arrival`]).
    pub(super) fn push(&self, entry: Entry, source: Source, read_answer: Option<GapAnswer>) {
        let mut state = self.lock();
        if state.closed || state.marks.drops_on_arrival(&entry.item) {
            return;
        }
        state.posted.push_back(Posted::Entry {
            entry,
            source,
            read_answer,
        });
        self.rouse(state);
    }

    /// The reducer's marks, for the drops judged at push.
    fn set_marks(&self, marks: Marks) {
        self.lock().marks = marks;
    }

    /// The reducer's verdicts, for the drops judged at push.
    fn set_verdicts(&self, verdicts: Arc<Verdicts>) {
        self.lock().verdicts = verdicts;
    }

    /// The push-time copy of the reducer's verdicts: a reference count.
    pub(super) fn verdicts(&self) -> Arc<Verdicts> {
        Arc::clone(&self.lock().verdicts)
    }

    /// Everything posted, waiting for something to be posted or for `at`,
    /// whichever comes first.
    fn take(&self, at: Option<Instant>) -> VecDeque<Posted> {
        let mut state = self.lock();
        if state.posted.is_empty() {
            state.asleep = true;
            drop(state);
            match at {
                Some(at) => {
                    self.alarm.set(at);
                    // A post that ends the wait early leaves the due time
                    // set; cancelling it spares the kernel a timer interrupt
                    // no one waits for.
                    if !self.alarm.wait() {
                        self.alarm.cancel();
                    }
                }
                None => self.alarm.wait_for_event(),
            }
            state = self.lock();
            state.asleep = false;
        }
        std::mem::take(&mut state.posted)
    }

    /// Closes the mailbox to producers.
    fn close(&self) {
        self.lock().closed = true;
    }

    /// Releases `state`, ending the shell's wait if it waits.
    fn rouse(&self, mut state: MutexGuard<'_, MailboxState>) {
        let asleep = std::mem::take(&mut state.asleep);
        drop(state);
        if asleep {
            self.alarm.wake();
        }
    }
}

/// What the shell lets go of, for the next program's worker to drop.
#[derive(Default)]
pub(super) struct Garbage {
    pub(super) store: Vec<Removed>,
    pub(super) elements: Vec<Agile<Kept>>,
    pub(super) parked: Vec<super::store::Parked>,
}

impl Garbage {
    fn is_empty(&self) -> bool {
        self.store.is_empty() && self.elements.is_empty() && self.parked.is_empty()
    }
}

/// A worker generation the shell started.
struct Generation {
    /// Its inbox: dropping it closes the inbox and rings its doorbell.
    jobs: super::inbox::Sender<Job>,
    charge: Arc<Charge>,
}

/// The shell's state on the reducer's thread.
pub(super) struct Shell {
    context: Arc<Context>,
    reducer: Reducer,
    /// The reply tokens of the queries the reducer records, by request id.
    tokens: HashMap<u64, Responder<OutpostToCore>>,
    /// Who waits on each settle, by its number.
    settles: HashMap<u64, mpsc::Sender<()>>,
    settles_taken: u64,
    /// The elements the subscription follows, by token.
    elements: Elements,
    garbage: Garbage,
    /// The generation in charge, and the threads of every worker started.
    generations: HashMap<u64, Generation>,
    threads: Vec<thread::JoinHandle<()>>,
    closing: bool,
}

/// The elements the focus-following subscription follows, by the tokens
/// the reducer names them by: an element given that is the same as one
/// kept is given its token, so a focus that returns to an element keeps
/// its group.
#[derive(Default)]
struct Elements {
    kept: HashMap<u64, Agile<Kept>>,
    taken: u64,
}

impl Elements {
    /// The token of `element`, kept, or the one of the same element
    /// already kept, `element` then let go of into `garbage`.
    fn token(&mut self, element: Agile<Kept>, garbage: &mut Garbage) -> u64 {
        if let Some((&token, _)) = self.kept.iter().find(|(_, kept)| kept.same_as(&element)) {
            garbage.elements.push(element);
            return token;
        }
        self.taken += 1;
        self.kept.insert(self.taken, element);
        self.taken
    }
}

/// The shell's local reads for the reducer.
struct ShellLocals<'a> {
    context: &'a Arc<Context>,
    charge: Option<&'a Charge>,
    elements: &'a mut Elements,
    garbage: &'a mut Garbage,
}

impl Locals for ShellLocals<'_> {
    fn window_is_hung(&self, hwnd: isize) -> bool {
        super::window::window_is_hung(hwnd)
    }

    fn moved_on(&self, window: isize) -> bool {
        super::window::front_is_another_thread_of_its_application(window)
    }

    fn publication(&mut self, request: Request, facts: &Facts) -> Option<Change> {
        let progress = self.charge?.publish(request)?;
        let tag = tag_of(request);
        let context = RequestContext::new(self.context, tag, facts.clone());
        let effects = worker::publication(&context, &progress);
        self.garbage.store.push(context.take_removed());
        tracing::info!(
            trace = %progress.focus.trace,
            name = ?progress.focus.node.name,
            "a focus walk was under way at its bound or its worker's abandonment; the focus is published as read so far"
        );
        Some(write(
            &self.context.store,
            tag,
            effects,
            self.elements,
            self.garbage,
        ))
    }
}

/// The store's tag for `request`.
pub(super) fn tag_of(request: Request) -> Tag {
    Tag {
        generation: request.generation,
        request: request.number,
    }
}

/// The change the reducer is given for `effects`, a commit of `request`:
/// for a commit it accepts, its writes made ([`write`]); for any other,
/// only the verdicts the reducer records from any request, its writes not
/// made and its objects let go of into `garbage`. `None` when the reducer
/// is given nothing.
fn admit(
    store: &Store,
    reducer: &Reducer,
    (request, effects): (Request, Effects),
    elements: &mut Elements,
    garbage: &mut Garbage,
) -> Option<Change> {
    if reducer.accepts(request) {
        return Some(write(store, tag_of(request), effects, elements, garbage));
    }
    let mut effects = effects;
    let verdicts = std::mem::take(&mut effects.verdicts);
    discard(effects, garbage);
    (!verdicts.is_empty()).then(|| Change {
        verdicts,
        ..Change::default()
    })
}

/// Lets go of what a refused commit holds into `garbage`.
fn discard(effects: Effects, garbage: &mut Garbage) {
    let (_, writes) = effects.split();
    if let Some((_, _, _, target)) = writes.follow {
        garbage.elements.extend(target.element);
        garbage.elements.extend(target.slider);
    }
    if let Some(target) = writes.element {
        garbage.elements.extend(target.element);
        garbage.elements.extend(target.slider);
    }
    if let Some(stale) = writes.stale {
        garbage.elements.extend(stale.element);
    }
    garbage.store.push(Removed::of_text(writes.text));
    drop(writes.terminal);
}

/// Makes the writes of `effects`, a commit of request `tag`, that only the
/// shell applies, to the store and the elements followed, and returns the
/// reducer's change, its targets' elements given their tokens and its text
/// facts' changes those the store made. Called only for a commit the
/// reducer accepts, in the order the program made them: its text writes,
/// made before the rest as its text calls came first; the stale element
/// evicted before the focus is followed; then the follow's caret; then a
/// terminal's entry and the caret sent.
fn write(
    store: &Store,
    tag: Tag,
    effects: Effects,
    elements: &mut Elements,
    garbage: &mut Garbage,
) -> Change {
    let (mut change, writes) = effects.split();
    let Writes {
        follow,
        element,
        terminal,
        sent_caret,
        stale,
        text,
    } = writes;
    if !text.is_empty() {
        let (changes, removed) = store.write_text(tag, text);
        change.text = changes;
        garbage.store.push(removed);
    }
    let mut follow = follow;
    if let Some(stale) = stale {
        let evicted = stale
            .element
            .and_then(|element| store.evict_element_if(stale.node, &element));
        // The subscription is not moved to the element evicted: the focus's
        // own is found after its report.
        if let (Some(evicted), Some((node, _, _, target))) = (&evicted, follow.as_mut())
            && *node == stale.node
            && target
                .element
                .as_ref()
                .is_some_and(|element| element.same_as(evicted))
        {
            garbage.elements.extend(target.element.take());
            garbage.elements.extend(target.slider.take());
            target.wanted = None;
            if let Some(pure) = change.follow.as_mut() {
                pure.target.wanted = None;
            }
        }
        garbage.elements.extend(evicted);
        if stale.gone {
            garbage.store.push(store.remove_terminal(stale.node));
        }
    }
    if let Some((node, terminal, sent, target)) = follow {
        // A terminal gaining the focus has its caret reported afresh: the
        // caret read with its report is the one sent.
        if terminal {
            store.set_sent_caret(node, sent, true);
        } else {
            store.set_sent_caret(node, None, false);
        }
        if let Some(pure) = change.follow.as_mut() {
            pure.target.element = target
                .element
                .map(|element| elements.token(element, garbage));
            pure.target.slider = target.slider.map(|slider| elements.token(slider, garbage));
        }
    }
    if let (Some(target), Some((_, pure))) = (element, change.element.as_mut()) {
        pure.element = target
            .element
            .map(|element| elements.token(element, garbage));
        pure.slider = target.slider.map(|slider| elements.token(slider, garbage));
    }
    if let Some((node, entry)) = terminal {
        garbage.store.push(store.put_terminal(node, entry));
    }
    if let Some((node, caret)) = sent_caret {
        store.set_sent_caret(node, Some(caret), true);
    }
    change
}

impl Shell {
    /// Starts the reducer's thread and the first worker for `context`,
    /// whose worker creates its UIA client at once with `warm`.
    pub(super) fn start(
        context: &Arc<Context>,
        warm: bool,
    ) -> thread::JoinHandle<Vec<thread::JoinHandle<()>>> {
        let mut shell = Shell {
            context: Arc::clone(context),
            reducer: Reducer::new(),
            tokens: HashMap::new(),
            settles: HashMap::new(),
            settles_taken: 0,
            elements: Elements::default(),
            garbage: Garbage::default(),
            generations: HashMap::new(),
            threads: Vec::new(),
            closing: false,
        };
        shell.spawn_worker(0, warm);
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || shell.run())
            .expect("spawn the reducer's thread")
    }

    /// Starts worker generation `generation`.
    fn spawn_worker(&mut self, generation: u64, warm: bool) {
        let (jobs, receiver) = super::inbox::inbox();
        let charge = Arc::new(Charge::default());
        let thread = worker::spawn(
            Arc::clone(&self.context),
            generation,
            warm,
            receiver,
            Arc::clone(&charge),
        );
        self.threads.retain(|thread| !thread.is_finished());
        self.threads.push(thread);
        self.generations
            .insert(generation, Generation { jobs, charge });
    }

    /// Now on the shell's clocks.
    fn now(&self) -> Now {
        Now {
            at: self.context.epoch.elapsed(),
            observed_us: self.context.observed_now(),
            wall_us: crate::protocol::now_us(),
        }
    }

    /// The thread's loop: drains the mailbox into the reducer, and ticks it
    /// when its next timer comes, until the outpost shuts down and no
    /// program is in flight. Returns every worker thread started, for the
    /// shutdown to wait for.
    fn run(mut self) -> Vec<thread::JoinHandle<()>> {
        loop {
            let at = self
                .reducer
                .next_timer(self.now())
                .map(|at| self.context.epoch + at);
            let posted = self.context.mailbox.take(at);
            for posted in posted {
                self.take(posted);
            }
            let now = self.now();
            let effects = self.with_locals(|reducer, locals| reducer.tick(now, locals));
            self.apply(effects);
            if self.closing && !self.reducer.busy() {
                break;
            }
        }
        // No program is in flight: the workers end once their channels
        // close.
        self.generations.clear();
        drop(std::mem::take(&mut self.garbage));
        drop(std::mem::take(&mut self.elements.kept));
        std::mem::take(&mut self.threads)
    }

    /// Runs `step` on the reducer with the shell's local reads.
    fn with_locals(
        &mut self,
        step: impl FnOnce(&mut Reducer, &mut dyn Locals) -> Vec<Effect>,
    ) -> Vec<Effect> {
        let charge = self
            .generations
            .get(&self.reducer.generation())
            .map(|generation| Arc::clone(&generation.charge));
        let mut locals = ShellLocals {
            context: &self.context,
            charge: charge.as_deref(),
            elements: &mut self.elements,
            garbage: &mut self.garbage,
        };
        step(&mut self.reducer, &mut locals)
    }

    /// Takes one posted input into the reducer and applies what it asks.
    fn take(&mut self, posted: Posted) {
        let input = match posted {
            Posted::Entry {
                entry,
                source,
                read_answer,
            } => {
                if let Item::Query { request_id, .. } = &entry.item
                    && !self.closing
                {
                    self.tokens
                        .insert(*request_id, token(&self.context, *request_id, entry.trace));
                }
                Input::Entry {
                    entry,
                    source,
                    read_answer,
                }
            }
            Posted::Settle(done) => {
                self.settles_taken += 1;
                self.settles.insert(self.settles_taken, done);
                let now = self.now();
                Input::Entry {
                    entry: Entry::own(
                        Item::Settle(self.settles_taken),
                        TraceId::mint(),
                        now.wall_us,
                    ),
                    source: Source::Other,
                    read_answer: None,
                }
            }
            Posted::Hooked => Input::Hooked,
            Posted::WithoutListener => Input::WithoutListener,
            Posted::ForwardingEnded(through) => Input::ForwardingEnded(through),
            Posted::Cancel(request_id) => Input::Cancel(request_id),
            Posted::Commit { request, effects } => {
                // A dead generation's or a claimed request's commit gives
                // the reducer only the verdicts it records from any
                // request, and its objects go to a worker.
                let Some(change) = admit(
                    &self.context.store,
                    &self.reducer,
                    (request, *effects),
                    &mut self.elements,
                    &mut self.garbage,
                ) else {
                    return;
                };
                Input::Commit {
                    request,
                    change: Box::new(change),
                }
            }
            Posted::Armed { request, bound_at } => Input::Armed {
                request,
                bound: bound_at.saturating_duration_since(self.context.epoch),
            },
            Posted::WalkReturned { request, past } => Input::WalkReturned { request, past },
            Posted::Finished {
                request,
                reported,
                panicked,
                cut,
            } => Input::Finished {
                request,
                reported,
                panicked,
                cut,
            },
            Posted::Returned => Input::Returned,
            Posted::GapTimerGiven => Input::GapTimerGiven,
            Posted::GapBound => Input::GapBound,
            Posted::Close => {
                self.closing = true;
                self.context.mailbox.close();
                Input::Close
            }
        };
        let now = self.now();
        let effects = self.with_locals(|reducer, locals| reducer.input(input, now, locals));
        self.apply(effects);
    }

    /// Applies the reducer's effects, in order.
    fn apply(&mut self, effects: Vec<Effect>) {
        let mut effects: VecDeque<Effect> = effects.into();
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Dispatch(program) => self.dispatch(*program),
                Effect::Send(message) => self.send(*message),
                Effect::Release {
                    nodes,
                    anchors,
                    acknowledged,
                    live,
                } => {
                    let released = self.release(&nodes, &anchors, acknowledged, &live);
                    let now = self.now();
                    let more =
                        self.with_locals(|reducer, locals| reducer.input(released, now, locals));
                    // What the release's input asks comes before anything
                    // asked after the release.
                    for effect in more.into_iter().rev() {
                        effects.push_front(effect);
                    }
                }
                Effect::Subscribe(subscription) => self.subscribe(&subscription),
                Effect::Unfollow(tokens) => {
                    for token in tokens {
                        if let Some(element) = self.elements.kept.remove(&token) {
                            self.garbage.elements.push(element);
                        }
                    }
                }
                Effect::ForgetRanges(ranges) => {
                    self.garbage
                        .store
                        .push(self.context.store.forget_ranges(&ranges));
                }
                Effect::Unpark(keys) => {
                    for key in keys {
                        self.garbage
                            .parked
                            .extend(self.context.store.take_parked(key));
                    }
                }
                Effect::Abandon {
                    dead,
                    replacement,
                    count,
                    reason,
                } => {
                    tracing::warn!(
                        abandoned = count,
                        reason,
                        "the worker is abandoned and replaced"
                    );
                    if let Some(generation) = self.generations.remove(&dead) {
                        generation.charge.abandon();
                    }
                    self.context.abandoned.store(count, Ordering::Relaxed);
                    self.spawn_worker(replacement, false);
                }
                Effect::Returned { count } => {
                    tracing::info!("an abandoned worker returned; its result is discarded");
                    self.context.abandoned.store(count, Ordering::Relaxed);
                }
                Effect::Marks(marks) => self.context.mailbox.set_marks(marks),
                Effect::Verdicts(verdicts) => self.context.mailbox.set_verdicts(verdicts),
                // After the copy above, so the event thread decides on it.
                Effect::Narrow => self
                    .context
                    .nudge_event_thread(super::narrowing::Nudge::Decide),
                Effect::WatchEnded { trace, why, waited } => {
                    tracing::debug!(
                        %trace,
                        why,
                        waited_ms = waited.as_millis(),
                        "a caret key's watch ended without evidence"
                    );
                }
                Effect::GapTimer(due) => {
                    if let Some(timer) = self.context.gap_timer() {
                        match due {
                            Some(due_us) => timer.set(due_us),
                            None => timer.cancel(),
                        }
                    }
                }
            }
        }
        debug_assert_eq!(
            self.tokens
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<u64>>(),
            *self.reducer.queries(),
            "a reply token for every query the reducer records, and no other"
        );
    }

    /// Hands `program` to the worker in charge, with what the shell has let
    /// go of for it to drop, and, for a settle, who waits on it.
    fn dispatch(&mut self, program: Program) {
        let settle = if let Work::Settle(id) = &program.work {
            self.settles.remove(id)
        } else {
            None
        };
        let garbage = std::mem::take(&mut self.garbage);
        let job = Job {
            program,
            garbage,
            settle,
        };
        let request = job.program.request;
        let sent = match self.generations.get(&self.reducer.generation()) {
            Some(generation) => generation.jobs.send(job).is_ok(),
            None => false,
        };
        if !sent {
            // Its end is posted, so the reducer goes on.
            tracing::error!("the worker in charge is gone; its program is dropped");
            self.context.mailbox.post(Posted::Finished {
                request,
                reported: false,
                panicked: true,
                cut: None,
            });
        }
    }

    /// Sends `message`: a reply through its query's token.
    fn send(&mut self, message: OutpostToCore) {
        if let OutpostToCore::Reply { request_id, .. } = &message
            && let Some(token) = self.tokens.remove(request_id)
        {
            token.answer(message);
            return;
        }
        self.context.send(message);
    }

    /// Releases every node and text anchor no live request can still name
    /// (`super::store`, the release rule), for Core's held lists, which
    /// acknowledge position `acknowledged`; the removed objects go to the
    /// next program's worker.
    fn release(
        &mut self,
        nodes: &[u64],
        anchors: &[u64],
        acknowledged: u64,
        live: &[Request],
    ) -> Input {
        let held: HashSet<Carried> = nodes
            .iter()
            .map(|&number| Carried::Node(number))
            .chain(anchors.iter().map(|&number| Carried::Anchor(number)))
            .collect();
        let live: Vec<Tag> = live.iter().copied().map(tag_of).collect();
        let (position, released) =
            self.context
                .store
                .release(&self.context.outbound, &held, acknowledged, &live);
        let Some(released) = released else {
            tracing::debug!(?held, acknowledged, position, "nothing to release");
            return Input::Released {
                nodes: HashSet::new(),
                anchors: HashSet::new(),
                objects: false,
            };
        };
        tracing::debug!(
            ?held,
            acknowledged,
            position,
            nodes = ?released.nodes,
            anchors = ?released.anchors,
            "releasing the nodes and anchors no live request can name"
        );
        self.garbage.store.push(released.objects);
        Input::Released {
            nodes: released.nodes,
            anchors: released.anchors,
            objects: !self.garbage.is_empty(),
        }
    }

    /// Moves the focus-following subscription as `subscription` says. The
    /// subscription keeps references of its own; a slider's element is
    /// let go of once it is moved.
    fn subscribe(&mut self, subscription: &Subscription) {
        let reference = |token: u64| self.elements.kept.get(&token).map(Agile::reference);
        let kept: Vec<_> = subscription
            .kept
            .iter()
            .filter_map(|&(token, group)| {
                reference(token).map(|element| (element, uia_group(group)))
            })
            .collect();
        let slider = subscription
            .slider
            .and_then(reference)
            .map(|element| (element, verbatim_uia::Group::Value));
        let newest = subscription
            .newest
            .and_then(|(token, group)| reference(token).map(|element| (element, group)));
        tracing::debug!(
            newest = ?subscription.newest,
            kept = subscription.kept.len(),
            "the focus-following subscription is moved"
        );
        if let Some(registration) = self.context.focus_events.get() {
            match newest {
                Some((element, group)) => registration.retarget_with(
                    verbatim_uia::Scope::Elements(vec![element]),
                    uia_group(group),
                    slider.into_iter().chain(kept).collect(),
                ),
                None if !kept.is_empty() => registration.retarget_with(
                    verbatim_uia::Scope::Elements(Vec::new()),
                    verbatim_uia::Group::Base,
                    kept,
                ),
                None => registration.retarget(verbatim_uia::Scope::Nothing),
            }
        }
        if let Some(slider) = subscription.slider
            && let Some(element) = self.elements.kept.remove(&slider)
        {
            self.garbage.elements.push(element);
        }
    }
}

/// The UIA group the reducer's `group` names.
fn uia_group(group: Group) -> verbatim_uia::Group {
    match group {
        Group::Base => verbatim_uia::Group::Base,
        Group::Extended => verbatim_uia::Group::Extended,
        Group::ExtendedWithoutTextEvents => verbatim_uia::Group::ExtendedWithoutTextEvents,
    }
}

/// The reply token of query `request_id`, traced `trace`: its one answer
/// is sent to Core, and dropped unanswered it answers "failed", the
/// model's one outcome for a responder dropped unanswered on the outpost's
/// wire, since the outpost's process lives.
fn token(context: &Arc<Context>, request_id: u64, trace: TraceId) -> Responder<OutpostToCore> {
    let context = Arc::clone(context);
    Responder::new(move |answer: Option<OutpostToCore>| {
        let message = answer.unwrap_or_else(|| OutpostToCore::Reply {
            trace_id: trace,
            request_id,
            outcome: QueryOutcome::Failed("the outpost dropped the query".to_owned()),
            timing: EventTiming::default(),
        });
        context.send(message);
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use verbatim_model::protocol::Query;

    use super::super::reducer::{Anchor, NodeAnchors, TextChange};
    use super::super::store::TextWrite;
    use super::*;

    /// No local read: a test's reducer is never asked one.
    struct NoLocals;

    impl Locals for NoLocals {
        fn window_is_hung(&self, _: isize) -> bool {
            false
        }

        fn moved_on(&self, _: isize) -> bool {
            false
        }

        fn publication(&mut self, _: Request, _: &Facts) -> Option<Change> {
            None
        }
    }

    /// A commit whose text call minted anchor `anchor` in edit control
    /// `node`'s text.
    fn minted(node: u64, anchor: u64) -> (Effects, TextChange) {
        let mut part = NodeAnchors::default();
        part.anchors.insert(
            anchor,
            Anchor {
                pos: 4,
                text: Arc::from(""),
            },
        );
        let change = TextChange::Edit {
            node,
            part,
            copied: BTreeSet::new(),
        };
        let effects = Effects {
            text: vec![TextWrite::Facts {
                change: change.clone(),
                ranges: Vec::new(),
                touched: vec![anchor],
            }],
            ..Effects::default()
        };
        (effects, change)
    }

    /// A commit the reducer refuses makes none of its text writes: the
    /// anchor its text call minted is not tagged in the store, and the
    /// reducer is given nothing. The same commit of the program in flight
    /// has its anchor tagged and its change given to the reducer.
    #[test]
    fn a_refused_commits_text_writes_are_not_made() {
        let store = Store::new();
        let mut reducer = Reducer::new();
        let entry = Entry::own(
            Item::Query {
                request_id: 1,
                query: Query::FocusNow,
            },
            TraceId::mint(),
            1,
        );
        let effects = reducer.input(
            Input::Entry {
                entry,
                source: Source::Other,
                read_answer: None,
            },
            Now::default(),
            &mut NoLocals,
        );
        let in_flight = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Dispatch(program) => Some(program.request),
                _ => None,
            })
            .expect("the query's program dispatched");
        let node = store.nodes_for(tag_of(in_flight)).window(5).number();
        let dead = Request {
            generation: 7,
            number: 99,
        };
        let (mut elements, mut garbage) = (Elements::default(), Garbage::default());
        let (refused, _) = minted(node, 7);
        let admitted = admit(
            &store,
            &reducer,
            (dead, refused),
            &mut elements,
            &mut garbage,
        );
        assert!(admitted.is_none());
        assert!(!store.tags_anchor(7));
        let (accepted, change) = minted(node, 8);
        let admitted = admit(
            &store,
            &reducer,
            (in_flight, accepted),
            &mut elements,
            &mut garbage,
        )
        .expect("the change of the program in flight");
        assert_eq!(admitted.text, vec![change]);
        assert!(store.tags_anchor(8));
    }
}
