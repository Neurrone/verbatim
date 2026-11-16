//! The reducer's thread and its shell (`books/dev/src/explanation/design/ownership.md`): the mailbox every producer appends to,
//! the thread that drains it into the reducer ([`super::reducer`]) and
//! applies the reducer's effects, the reducer's timers in its timed wait,
//! the queries' reply tokens, and the worker generations.
//!
//! The rule that lets the reducer keep time: every lock its thread takes is
//! a short hold with no call into an application and no wait inside it,
//! so a timer is never late by more than such a hold. They are the
//! mailbox's, the outbound backlog's with the store's inside it for a
//! release, the store's for its writes, and a worker's charge's.
//!
//! What the shell holds that names an application's object, the elements
//! the focus-following subscription is moved to, is kept by token, so the
//! reducer holds none; an object the shell lets go of, an element no longer
//! followed, a parked event object, or what a release removed, is handed
//! to the next program dispatched, and dropped on its worker before the
//! program runs, never on this thread.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;

use super::lock::{Lock, LockGuard};
use std::thread;
use std::time::Instant;

use verbatim_model::NoteRequest;
use verbatim_model::TraceId;
use verbatim_model::protocol::{
    EventTiming, FrontWindow, OutpostToCore, QueryOutcome, RequestId, Responder,
};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_uia::{Agile, Kept};
use verbatim_win32::window::window_class_name;

use super::Context;
use super::commit::{Effects, FocusEffects, KeptTarget, Writes};
use super::outbound::Carried;
use super::reducer::{
    Asker, Change, Cut, Effect, Entry, Input, Item, Marks, NoteOn, Now, Program, Reducer, Request,
    Source, Subscribed, VerdictChange,
};
use super::store::{Removed, Store, Tag};
use super::worker::{self, Charge, Job};
use verbatim_process::alarm::Alarm;

/// What the mailbox carries to the shell, in the order it was appended.
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from its producer into the mailbox, as it always has"
)]
pub(super) enum Posted {
    /// An entry, and where it came from.
    Entry { entry: Entry, source: Source },
    /// `Outpost::settle`, answered on `0` once the reducer reaches it.
    Settle(mpsc::Sender<()>),
    /// Core's word on where attention is.
    Front(FrontWindow),
    /// Core withdraws a query.
    Cancel(RequestId),
    /// Core's note of a key with no before read.
    KeyNote(NoteRequest),
    /// A verdict a program of generation `generation` made, posted alone,
    /// and taken after the mailbox closes, as a commit is.
    Verdict {
        generation: u64,
        change: VerdictChange,
    },
    /// A program's commit.
    Commit {
        request: Request,
        effects: Box<Effects>,
    },
    /// A program ended, with the caret read it cut at its value change
    /// check, if it cut one.
    Finished {
        request: Request,
        reported: bool,
        panicked: bool,
        cut: Option<Box<Cut>>,
    },
    /// An abandoned worker of this generation returned from its call.
    Returned(u64),
    /// Core's request.
    Ask(Box<verbatim_model::request::Ask>),
    /// Core cancels a request.
    CancelAsk(verbatim_model::request::Cancel),
    /// A window was destroyed: Core's requests queued on it are answered
    /// "not there".
    WindowDestroyed(verbatim_model::WindowHandle),
    /// A program's answer to the Core request it runs.
    Answered {
        request: Request,
        answer: Box<verbatim_model::request::Answer>,
    },
    /// The outpost is shutting down.
    Close,
}

/// The mailbox: an append-only queue under one lock, which every producer
/// appends to and only the shell drains, with the push-time copies of the
/// reducer's marks that the producers' drops are judged by.
#[derive(Default)]
pub(super) struct Mailbox {
    state: Lock<MailboxState>,
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
    /// Whether the shell waits, so that an append sets the alarm's event;
    /// taken by the first to set it.
    asleep: bool,
    closed: bool,
}

impl Mailbox {
    #[track_caller]
    fn lock(&self) -> LockGuard<'_, MailboxState> {
        self.state.lock()
    }

    /// Appends `posted`, waking the shell if it waits. Nothing is taken
    /// once the outpost is shut down but what its programs still post.
    pub(super) fn post(&self, posted: Posted) {
        let mut state = self.lock();
        if state.closed
            && matches!(
                posted,
                Posted::Entry { .. }
                    | Posted::Ask(_)
                    | Posted::CancelAsk(_)
                    | Posted::WindowDestroyed(_)
                    | Posted::Settle(_)
                    | Posted::Cancel(_)
                    | Posted::Front(_)
                    | Posted::KeyNote(_)
            )
        {
            return;
        }
        state.posted.push_back(posted);
        self.rouse(state);
    }

    /// Appends `entry`, from `source`, unless the reducer's marks drop it
    /// as it is pushed, waking nobody ([`Marks::drops_on_arrival`]).
    pub(super) fn push(&self, entry: Entry, source: Source) {
        let mut state = self.lock();
        if state.closed || state.marks.drops_on_arrival(&entry.item) {
            return;
        }
        state.posted.push_back(Posted::Entry { entry, source });
        self.rouse(state);
    }

    /// The reducer's marks, for the drops judged at push.
    fn set_marks(&self, marks: Marks) {
        self.lock().marks = marks;
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
    fn rouse(&self, mut state: LockGuard<'_, MailboxState>) {
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
    /// Its inbox: dropping it closes the inbox, which ends the worker once
    /// it has taken what was sent before.
    jobs: crossbeam_channel::Sender<Job>,
    charge: Arc<Charge>,
}

/// The shell's state on the reducer's thread.
pub(super) struct Shell {
    context: Arc<Context>,
    reducer: Reducer,
    /// The reply tokens of the queries the reducer records, by request id.
    tokens: HashMap<RequestId, Responder<OutpostToCore>>,
    /// Who waits on each test's settle, by its number: kept while the
    /// settle waits in the intake ([`Reducer::settles`]), answered as the
    /// reducer reaches it, and dropped as the outpost closes, which ends
    /// the wait too.
    settles: HashMap<u64, mpsc::Sender<()>>,
    /// The last settle numbered.
    settles_taken: u64,
    /// The elements the subscription follows, by token.
    elements: Elements,
    garbage: Garbage,
    /// The generation in charge, and the threads of every worker started.
    generations: HashMap<u64, Generation>,
    threads: Vec<thread::JoinHandle<()>>,
    /// The abandoned workers that have not returned, by generation: one
    /// more as the shell carries out an abandonment, one fewer as one
    /// returns.
    parked: HashSet<u64>,
    closing: bool,
    /// Whether a worker creates its UIA client at once, as the first one
    /// was started: a worker started at a dispatch, its generation's start
    /// having failed, is started so too.
    warm: bool,
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

/// The store's tag for `request`.
pub(super) fn tag_of(request: Request) -> Tag {
    Tag {
        generation: request.generation,
        request: request.number,
    }
}

/// The change the reducer is given for `effects`, a commit of `request`:
/// for a commit it accepts, its writes made ([`write`]). Any other is
/// refused whole, `None`: its writes are not made and its objects are let
/// go of into `garbage`.
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
    discard(effects, garbage);
    None
}

/// Lets go of the elements of `target` into `garbage`.
fn let_go_of_target(target: KeptTarget, garbage: &mut Garbage) {
    garbage.elements.push(target.element);
    garbage
        .elements
        .extend(target.slider.map(|(_, slider)| slider));
}

/// `target` as the reducer takes it, its elements given their tokens.
fn tokens_of(target: KeptTarget, elements: &mut Elements, garbage: &mut Garbage) -> Subscribed {
    Subscribed {
        group: target.group,
        element: elements.token(target.element, garbage),
        slider: target
            .slider
            .map(|(node, slider)| (node, elements.token(slider, garbage))),
    }
}

/// Lets go of what a refused commit holds into `garbage`.
fn discard(effects: Effects, garbage: &mut Garbage) {
    let (_, writes) = effects.split();
    if let Some(target) = writes
        .focus
        .and_then(FocusEffects::into_writes)
        .and_then(|writes| writes.target)
    {
        let_go_of_target(target, garbage);
    }
    if let Some((_, target)) = writes.element {
        let_go_of_target(target, garbage);
    }
    if let Some((_, element)) = writes.bar_found {
        garbage.elements.push(element);
    }
    if let Some(stale) = writes.stale {
        garbage.elements.extend(stale.element);
    }
    garbage
        .store
        .push(Removed::of_text(writes.text).with_terminal(writes.terminal.map(|(_, entry)| entry)));
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
        bar_found,
        mut focus,
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
    if let Some(stale) = stale {
        let evicted = stale
            .element
            .and_then(|element| store.evict_element_if(stale.node, &element));
        // The subscription is not moved to the element evicted: the focus's
        // own is found after its report.
        if let (Some(evicted), Some(writes)) =
            (&evicted, focus.as_mut().and_then(FocusEffects::writes_mut))
            && writes.node == stale.node
            && writes
                .target
                .as_ref()
                .is_some_and(|target| target.element.same_as(evicted))
            && let Some(target) = writes.target.take()
        {
            let_go_of_target(target, garbage);
        }
        garbage.elements.extend(evicted);
        garbage.store.push(store.remove_terminal(stale.node));
    }
    change.bar_found =
        bar_found.map(|(window, element)| (window, elements.token(element, garbage)));
    change.focus = focus.map(|focus| {
        focus.into_commit(|writes| {
            // A terminal gaining the focus has its caret reported afresh:
            // the caret read with its report is the one sent.
            if writes.terminal {
                store.set_sent_caret(writes.node, writes.sent_caret, true);
            } else {
                store.set_sent_caret(writes.node, None, false);
            }
            writes
                .target
                .map(|target| tokens_of(target, elements, garbage))
        })
    });
    change.element = element.map(|(node, target)| (node, tokens_of(target, elements, garbage)));
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
            parked: HashSet::new(),
            closing: false,
            warm,
        };
        shell.spawn_worker(0, warm);
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || shell.run())
            .expect("spawn the reducer's thread")
    }

    /// Starts worker generation `generation`. A worker that cannot be
    /// started leaves the generation with none, which the shell already
    /// handles: each program is answered as failed, and each dispatch tries
    /// once to start one ([`Shell::dispatch`]).
    fn spawn_worker(&mut self, generation: u64, warm: bool) {
        let (jobs, receiver) = crossbeam_channel::unbounded();
        let charge = Arc::new(Charge::default());
        let thread = match worker::spawn(
            Arc::clone(&self.context),
            (generation, warm),
            receiver,
            Arc::clone(&charge),
        ) {
            Ok(thread) => thread,
            Err(error) => {
                tracing::error!(
                    generation,
                    %error,
                    "a worker could not be started; its programs are answered as failed until one is"
                );
                return;
            }
        };
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
            wall_us: verbatim_process::clock::now_us(),
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
            let effects = self.reducer.tick(now);
            self.apply(effects);
            if self.closing && !self.reducer.busy() {
                break;
            }
        }
        // No program is in flight: what the shell let go of and the
        // elements followed are dropped by the worker in charge, as at
        // runtime, and the workers end once their channels close.
        let kept = std::mem::take(&mut self.elements.kept);
        self.garbage.elements.extend(kept.into_values());
        if !self.garbage.is_empty() {
            let program = self.reducer.shutdown_drop();
            self.dispatch(program);
        }
        self.generations.clear();
        std::mem::take(&mut self.threads)
    }

    /// Takes one posted input into the reducer and applies what it asks.
    fn take(&mut self, posted: Posted) {
        let input = match posted {
            Posted::Entry { mut entry, source } => {
                // The one test of an MSAA focus or a foreground change
                // against the foreground, made as the reducer takes it in,
                // never on the worker ([`super::reducer::Front`]).
                entry.front = self.context.front_test(&entry.item);
                if let Item::Query { request_id, .. } = &entry.item
                    && !self.closing
                {
                    self.tokens
                        .insert(*request_id, token(&self.context, *request_id, entry.trace));
                }
                Input::Entry { entry, source }
            }
            Posted::Settle(done) => {
                let Some(input) = self.settle(done) else {
                    return;
                };
                input
            }
            Posted::Cancel(request_id) => Input::Cancel(request_id),
            Posted::Ask(ask) => Input::Ask(ask),
            Posted::CancelAsk(cancel) => Input::CancelAsk(cancel),
            Posted::WindowDestroyed(window) => Input::WindowDestroyed(window),
            Posted::Answered { request, answer } => Input::Answered { request, answer },
            Posted::Front(front) => Input::Front(front),
            Posted::KeyNote(note) => {
                // Only the console host polls its note; finding it is a
                // local call, whatever the application is doing.
                let on = match note {
                    NoteRequest::Field { .. } => NoteOn::Field,
                    NoteRequest::Terminal { node, .. }
                        if self.context.store.has_terminal(node)
                            && self.reducer.followed_window().is_some_and(|hwnd| {
                                window_class_name(hwnd) == CONSOLE_WINDOW_CLASS
                            }) =>
                    {
                        NoteOn::ConsoleHost
                    }
                    NoteRequest::Terminal { .. } => NoteOn::Terminal,
                };
                Input::KeyNote {
                    node: note.node(),
                    fetch: note.fetch(),
                    on,
                }
            }
            Posted::Verdict { generation, change } => Input::Verdict { generation, change },
            Posted::Commit { request, effects } => {
                // A dead generation's or a claimed request's commit gives
                // the reducer nothing, and its objects go to a worker.
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
            Posted::Returned(generation) => {
                self.parked.remove(&generation);
                self.count_parked();
                tracing::info!(
                    generation,
                    abandoned = self.parked.len(),
                    "an abandoned worker returned; its result is discarded"
                );
                return;
            }
            Posted::Close => {
                self.closing = true;
                self.context.mailbox.close();
                // The reducer drops every settle waiting, so `Outpost::settle`
                // returns.
                self.settles.clear();
                Input::Close
            }
        };
        let now = self.now();
        let effects = self.reducer.input(input, now);
        self.apply(effects);
    }

    /// Numbers a test's settle that `done` waits on, and gives the reducer
    /// its barrier; once the outpost is closing it is dropped, which
    /// answers it.
    fn settle(&mut self, done: mpsc::Sender<()>) -> Option<Input> {
        if self.closing {
            return None;
        }
        self.settles_taken += 1;
        let id = self.settles_taken;
        self.settles.insert(id, done);
        let barrier = Item::Barrier {
            asker: Asker::Settle(id),
            behind_noted: false,
        };
        Some(Input::Entry {
            entry: Entry::own(barrier, TraceId::mint(), self.now().wall_us),
            source: Source::Other,
        })
    }

    /// Applies the reducer's effects, in order. An effect that asks, a
    /// release, a program's check or the check of the window in front, is
    /// answered at once, as the reducer's next inputs, before the shell
    /// takes another message, and what the answers ask comes before
    /// anything asked after the question.
    fn apply(&mut self, effects: Vec<Effect>) {
        let mut effects: VecDeque<Effect> = effects.into();
        while let Some(effect) = effects.pop_front() {
            let mut more = Vec::new();
            for answer in self.carry_out(effect) {
                let now = self.now();
                more.extend(self.reducer.input(answer, now));
            }
            for effect in more.into_iter().rev() {
                effects.push_front(effect);
            }
        }
        self.check_tables();
    }

    /// Carries out `effect`, returning the answers to one that asks, in
    /// order.
    fn carry_out(&mut self, effect: Effect) -> Vec<Input> {
        match effect {
            Effect::Dispatch(program) => self.dispatch(*program),
            Effect::Send(message) => self.send(*message),
            Effect::Release {
                nodes,
                anchors,
                acknowledged,
                live,
            } => return vec![self.release(&nodes, &anchors, acknowledged, &live)],
            // Core's `Follow` and the focus completion alone move the
            // registration (`super::followed`).
            Effect::Subscribe(_) => {}
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
            Effect::Abandon { dead, replacement } => {
                self.parked.insert(dead);
                tracing::warn!(
                    abandoned = self.parked.len(),
                    "Core cancelled its request as the foreground moved away: the worker is abandoned and replaced"
                );
                if let Some(generation) = self.generations.remove(&dead) {
                    generation.charge.abandon();
                }
                self.count_parked();
                self.spawn_worker(replacement, false);
            }
            Effect::Marks(marks) => self.context.mailbox.set_marks(marks),
            Effect::LogicError(error) => {
                tracing::error!(?error, "a commit asked for what cannot occur");
            }
            Effect::Settled(id) => {
                if let Some(done) = self.settles.remove(&id) {
                    let _ = done.send(());
                }
            }
        }
        Vec::new()
    }

    /// The pong's count of the abandoned workers that have not returned.
    fn count_parked(&self) {
        *self.context.parked.lock() = self.parked.len();
    }

    /// Checks, in debug builds, that the shell's tables match the
    /// reducer's: a waiter for every settle waiting in the intake, and a
    /// reply token for every query the reducer records, and no other.
    fn check_tables(&self) {
        debug_assert_eq!(
            self.settles
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<u64>>(),
            self.reducer.settles(),
            "a waiter for every settle waiting in the intake, and no other"
        );
        debug_assert_eq!(
            self.tokens
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<RequestId>>(),
            *self.reducer.queries(),
            "a reply token for every query the reducer records, and no other"
        );
    }

    /// Hands `program` to the worker in charge, with what the shell has let
    /// go of for it to drop. A generation whose worker could not be started
    /// is tried once more here, warm as the first worker was; with still no
    /// worker, the shell keeps what it let go of, which holds the
    /// application's objects, whose release the reducer's thread must never
    /// make, and the next worker to start drops it.
    fn dispatch(&mut self, program: Program) {
        let in_charge = self.reducer.generation();
        if !self.generations.contains_key(&in_charge) {
            self.spawn_worker(in_charge, self.warm);
        }
        let request = program.request;
        let sent = match self.generations.get(&in_charge) {
            Some(generation) => {
                let garbage = std::mem::take(&mut self.garbage);
                match generation.jobs.send(Job { program, garbage }) {
                    Ok(()) => true,
                    Err(crossbeam_channel::SendError(job)) => {
                        self.garbage = job.garbage;
                        false
                    }
                }
            }
            None => false,
        };
        if !sent {
            // Its end is posted, so the reducer goes on.
            tracing::error!("the generation in charge has no worker; its program is dropped");
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
                nodes: BTreeSet::new(),
                anchors: BTreeSet::new(),
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
}

/// The reply token of query `request_id`, traced `trace`: its one answer
/// is sent to Core, and dropped unanswered it answers "failed", the
/// model's one outcome for a responder dropped unanswered on the outpost's
/// wire, since the outpost's process lives.
fn token(
    context: &Arc<Context>,
    request_id: RequestId,
    trace: TraceId,
) -> Responder<OutpostToCore> {
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

    use verbatim_model::protocol::{Query, RequestId, Urgent};

    use super::super::reducer::{Anchor, NodeAnchors, TextChange};
    use super::super::store::TextWrite;
    use super::*;

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
                request_id: RequestId::shell(1),
                query: Query::ReadFocus {
                    purpose: verbatim_model::protocol::FocusPurpose::Plain,
                },
            },
            TraceId::mint(),
            1,
        );
        let effects = reducer.input(
            Input::Entry {
                entry,
                source: Source::Other,
            },
            Now::default(),
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
    /// The context of an outpost of this process watching this process,
    /// its own reducer's thread and workers shut down, so the shell a test
    /// builds on it is the only one.
    fn context() -> Arc<Context> {
        use std::io::BufReader;

        use verbatim_process::message::read_message;
        use verbatim_process::overlapped::{End, Flow, pipe};
        use verbatim_process::transport::{BlockingReader, EVENT_PIPE_BUFFER};

        let (ours, theirs) =
            pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
        std::thread::spawn(move || {
            let mut reader = BufReader::new(BlockingReader::new(ours).expect("a reader"));
            while let Ok(Some(_)) = read_message::<_, OutpostToCore>(&mut reader) {}
        });
        let outpost = super::super::Outpost::new(theirs, std::process::id());
        let context = Arc::clone(&outpost.context);
        outpost.shutdown();
        context
    }

    /// A shell on [`context`], with nothing in flight and no worker.
    fn shell() -> Shell {
        Shell {
            context: context(),
            reducer: Reducer::new(),
            tokens: HashMap::new(),
            settles: HashMap::new(),
            settles_taken: 0,
            elements: Elements::default(),
            garbage: Garbage::default(),
            generations: HashMap::new(),
            threads: Vec::new(),
            parked: HashSet::new(),
            closing: false,
            warm: false,
        }
    }

    /// Ends the workers `shell` started and waits for them.
    fn end_workers(shell: &mut Shell) {
        shell.generations.clear();
        for thread in shell.threads.drain(..) {
            thread
                .join()
                .expect("the worker ends once its inbox closes");
        }
    }

    /// Each abandoned worker is counted once in a pong, none of them on a
    /// hung window, since the outpost judges no hang, and a worker that
    /// returns leaves the count.
    #[test]
    fn each_parked_worker_is_counted_once_in_a_pong() {
        let mut shell = shell();
        let abandon = |dead| Effect::Abandon {
            dead,
            replacement: dead + 1,
        };
        let counts = |shell: &Shell| match shell.context.pong() {
            Urgent::Pong {
                parked_count,
                parked_on_hung,
            } => (parked_count, parked_on_hung),
            other => panic!("not the ping's pong: {other:?}"),
        };
        assert_eq!(counts(&shell), (0, 0));
        shell.apply(vec![abandon(0)]);
        assert_eq!(counts(&shell), (1, 0));
        shell.apply(vec![abandon(1)]);
        assert_eq!(counts(&shell), (2, 0), "one more");
        shell.take(Posted::Returned(0));
        assert_eq!(counts(&shell), (1, 0), "one returned");
        end_workers(&mut shell);
    }
}
