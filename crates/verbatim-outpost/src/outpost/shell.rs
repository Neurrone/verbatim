//! The reducer's thread and its shell (`docs/design/ownership.md`): the mailbox every producer appends to,
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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use super::lock::{Lock, LockGuard};
use std::thread;
use std::time::Instant;

use verbatim_model::TraceId;
use verbatim_model::protocol::{EventTiming, OutpostToCore, QueryOutcome, RequestId, Responder};
use verbatim_uia::{Agile, Kept};

use super::commit::{Effects, Writes};
use super::outbound::Carried;
use super::reducer::{
    Asker, Change, Cut, Effect, Entry, Facts, Group, Input, Item, Marks, Now, Program, Reducer,
    Request, Source, Subscription, VerdictChange, Verdicts,
};
use super::store::{Removed, Store, Tag};
use super::worker::{self, Charge, Job};
use super::{Context, RequestContext};
use verbatim_process::alarm::Alarm;

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
    /// An entry, and where it came from.
    Entry { entry: Entry, source: Source },
    /// `Outpost::settle`, answered on `0` once the reducer reaches it.
    Settle(mpsc::Sender<()>),
    /// The outpost's hook is in.
    Hooked,
    /// No listener forwards to this outpost.
    WithoutListener,
    /// The listener's "forwarding ended".
    ForwardingEnded(u32),
    /// Core withdraws a query.
    Cancel(RequestId),
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
    /// The reducer's verdicts, as its last change of them left them: the
    /// event thread's drop of an MSAA event of a window known to use UIA.
    verdicts: Arc<Verdicts>,
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
    /// The abandoned workers that have not returned: one more as the shell
    /// carries out an abandonment, one fewer as one returns.
    abandoned: usize,
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
        garbage.store.push(store.remove_terminal(stale.node));
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
            abandoned: 0,
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

    /// The publication of the walk `request` armed, if it is still armed
    /// in the charge of `request`'s own generation and not returned before
    /// its bound: its progress as it stands, claimed so its worker sends
    /// nothing more for it, completed by plain windows with local calls
    /// only, judged against `facts`, and written into the store as a commit
    /// of its program.
    fn publication(&mut self, request: Request, facts: Facts) -> Option<Change> {
        let charge = Arc::clone(&self.generations.get(&request.generation)?.charge);
        let progress = charge.publish(request)?;
        let tag = tag_of(request);
        let context = RequestContext::new(&self.context, tag, facts);
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
            &mut self.elements,
            &mut self.garbage,
        ))
    }

    /// The gap record's test at its bound, of its top-level window
    /// `top_level` against the system's foreground, made from Windows'
    /// state with local calls only on this thread: whether the window is
    /// related to the foreground window by the acceptance test
    /// (`acceptance::foreground_relation`: the same top-level window or
    /// root owner, topmost, or a `Windows.UI.Core` window under the active
    /// window), and, when it is the foreground window itself, its window
    /// text sent as a whole-window focus stamped `stamp_us`: the plain
    /// window, or for a window not known to be read through UIA its MSAA
    /// client pane, read from local data, minted under `request`'s tag with the
    /// store's lock, a short hold, and written as a commit of its
    /// (`docs/design/focus-pipeline-rework-remaining.md`, "The held UIA
    /// application's title"). No call is made into the application.
    fn gap_bound_test(
        &mut self,
        request: Request,
        top_level: verbatim_reader_rules::Window,
        (stamp_us, trace): (u64, TraceId),
        facts: Facts,
    ) -> (bool, Option<Change>) {
        let foreground = self.context.foreground_window();
        let class = verbatim_win32::window::window_class_name(top_level.get());
        let related = verbatim_reader_rules::acceptance::foreground_relation(
            top_level,
            &class,
            &verbatim_win32::window::SystemWindows { foreground },
        )
        .related();
        if !related || foreground != top_level.get() {
            tracing::debug!(
                top_level = top_level.get(),
                foreground,
                related,
                "the gap record's bound: no window text is sent"
            );
            return (related, None);
        }
        // Said as the switch without a stall says the window: a window the
        // outpost reads through UIA by its window element, "<title>
        // window"; any other by its MSAA client, its title alone.
        let uia = self.context.known_to_use_uia(top_level.get(), stamp_us);
        let tag = tag_of(request);
        let context = RequestContext::new(&self.context, tag, facts);
        let effects = worker::window_text(&context, top_level.get(), uia, (trace, stamp_us));
        self.garbage.store.push(context.take_removed());
        tracing::info!(
            %trace,
            top_level = top_level.get(),
            "the gap record's bound passed with no focus reported; the window's text is sent"
        );
        let change = write(
            &self.context.store,
            tag,
            effects,
            &mut self.elements,
            &mut self.garbage,
        );
        (true, Some(change))
    }

    /// Takes one posted input into the reducer and applies what it asks.
    fn take(&mut self, posted: Posted) {
        let input = match posted {
            Posted::Entry { mut entry, source } => {
                // The one test of an MSAA focus or a foreground change
                // against the foreground, made as the reducer takes it in,
                // never on the worker ([`super::reducer::Front`]).
                entry.front = self.context.front_test(&entry.item, entry.observed_at_us);
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
            Posted::Hooked => Input::Hooked,
            Posted::WithoutListener => Input::WithoutListener,
            Posted::ForwardingEnded(through) => Input::ForwardingEnded(through),
            Posted::Cancel(request_id) => Input::Cancel(request_id),
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
            Posted::Returned => {
                self.abandoned = self.abandoned.saturating_sub(1);
                self.context
                    .abandoned
                    .store(self.abandoned, Ordering::Relaxed);
                tracing::info!(
                    abandoned = self.abandoned,
                    "an abandoned worker returned; its result is discarded"
                );
                return;
            }
            Posted::GapTimerGiven => Input::GapTimerGiven,
            Posted::GapBound => Input::GapBound,
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
    /// release, whether the user moved on, or a walk's publication, is
    /// answered at once, exactly once, as the reducer's next input, before
    /// the shell takes another message, and what the answer asks comes
    /// before anything asked after the question.
    fn apply(&mut self, effects: Vec<Effect>) {
        let mut effects: VecDeque<Effect> = effects.into();
        while let Some(effect) = effects.pop_front() {
            let Some(answer) = self.carry_out(effect) else {
                continue;
            };
            let now = self.now();
            let more = self.reducer.input(answer, now);
            for effect in more.into_iter().rev() {
                effects.push_front(effect);
            }
        }
        self.check_tables();
    }

    /// Carries out `effect`, returning the answer to one that asks.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per effect, each carried out in place or by a function of its own"
    )]
    fn carry_out(&mut self, effect: Effect) -> Option<Input> {
        match effect {
            Effect::Dispatch(program) => self.dispatch(*program),
            Effect::Send(message) => self.send(*message),
            Effect::Release {
                nodes,
                anchors,
                acknowledged,
                live,
            } => return Some(self.release(&nodes, &anchors, acknowledged, &live)),
            Effect::AskMovedOn { request, window } => {
                return Some(Input::MovedOn {
                    request,
                    moved_on: verbatim_win32::window::front_is_another_thread_of_its_application(
                        window.get(),
                    ),
                });
            }
            Effect::Publish {
                request,
                facts,
                then_abandon,
            } => {
                return Some(Input::Published {
                    request,
                    change: self.publication(request, *facts).map(Box::new),
                    then_abandon,
                });
            }
            Effect::TestGapBound {
                request,
                top_level,
                stamp_us,
                trace,
                facts,
            } => {
                let (related, title) =
                    self.gap_bound_test(request, top_level, (stamp_us, trace), *facts);
                return Some(Input::GapBoundTested {
                    request,
                    related,
                    title: title.map(Box::new),
                });
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
                reason,
                in_hand,
            } => {
                self.abandoned += 1;
                tracing::warn!(
                    abandoned = self.abandoned,
                    reason,
                    in_hand = in_hand.work,
                    trace = in_hand.trace.map(|trace| trace.to_string()),
                    window = in_hand.window.map(|window| format!("{window:#x}")),
                    "the worker is abandoned and replaced"
                );
                if let Some(generation) = self.generations.remove(&dead) {
                    generation.charge.abandon();
                }
                self.context
                    .abandoned
                    .store(self.abandoned, Ordering::Relaxed);
                self.spawn_worker(replacement, false);
            }
            Effect::Marks(marks) => self.context.mailbox.set_marks(marks),
            Effect::Verdicts(verdicts) => self.context.mailbox.set_verdicts(verdicts),
            Effect::Settled(id) => {
                if let Some(done) = self.settles.remove(&id) {
                    let _ = done.send(());
                }
            }
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
        None
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

    /// Moves the focus-following subscription as `subscription` says. The
    /// subscription keeps references of its own; the elements stay kept
    /// under their tokens until the reducer unfollows them, so an element
    /// given again is known by its token.
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

    use verbatim_model::protocol::{Query, RequestId};

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
                query: Query::FocusNow,
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

    /// The shell answers a publication it is asked for at once, as the
    /// reducer's next input, and carries out what the answer asks before it
    /// returns to its mailbox: asked by an abandonment for a program whose
    /// charge holds no walk, it answers with nothing published, and the
    /// abandonment completes, its replacement started and its query
    /// answered, within the one step.
    #[test]
    fn an_ask_is_answered_before_the_shell_takes_another_message() {
        let mut shell = Shell {
            context: context(),
            reducer: Reducer::new(),
            tokens: HashMap::new(),
            settles: HashMap::new(),
            settles_taken: 0,
            elements: Elements::default(),
            garbage: Garbage::default(),
            generations: HashMap::new(),
            threads: Vec::new(),
            abandoned: 0,
            closing: false,
            warm: false,
        };
        shell.take(Posted::Entry {
            entry: Entry::own(
                Item::Query {
                    request_id: RequestId::shell(1),
                    query: Query::FocusNow,
                },
                TraceId::from_raw(1),
                1,
            ),
            source: Source::Other,
        });
        let request = Request {
            generation: 0,
            number: 1,
        };
        assert!(
            shell.reducer.accepts(request),
            "the query's program in flight"
        );
        assert_eq!(shell.tokens.len(), 1);
        shell.take(Posted::Armed {
            request,
            bound_at: shell.context.epoch + std::time::Duration::from_secs(60),
        });
        shell.apply(vec![Effect::Publish {
            request,
            facts: Box::default(),
            then_abandon: Some(verbatim_outpost_logic::Reason::Deadline),
        }]);
        assert!(!shell.reducer.accepts(request), "abandoned by the answer");
        assert_eq!(shell.reducer.generation(), 1);
        assert!(shell.tokens.is_empty(), "its query answered");
        assert!(
            shell.generations.contains_key(&1),
            "its replacement started"
        );
        shell.generations.clear();
        for thread in shell.threads.drain(..) {
            thread
                .join()
                .expect("the worker ends once its inbox closes");
        }
    }
}
