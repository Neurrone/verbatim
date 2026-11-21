//! The reducer's thread and its shell (`books/dev/src/explanation/design/ownership.md`): the mailbox every producer appends to,
//! the thread that drains it into the reducer ([`super::reducer`]) and
//! applies the reducer's effects, the reducer's timers in its timed wait,
//! and the worker generations.
//!
//! The rule that lets the reducer keep time: every lock its thread takes is
//! a short hold with no call into an application and no wait inside it,
//! so a timer is never late by more than such a hold. They are the
//! mailbox's, the outbound backlog's with the store's inside it for a
//! release, the store's for its writes, and a worker's charge's.
//!
//! An object the shell lets go of, a parked event object or what a release
//! removed, is handed to the next program dispatched, and dropped on its
//! worker before the program runs, never on this thread.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;

use super::lock::{Lock, LockGuard};
use std::thread;
use std::time::Instant;

use verbatim_model::TraceId;
use verbatim_model::protocol::OutpostToCore;

use super::Context;
use super::commit::Effects;
use super::outbound::Carried;
use super::reducer::{Asker, Change, Effect, Entry, Input, Item, Now, Program, Reducer, Request};
use super::store::{Removed, Store};
use super::worker::{self, Charge, Job};
use verbatim_process::alarm::Alarm;

/// What the mailbox carries to the shell, in the order it was appended.
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from its producer into the mailbox, as it always has"
)]
pub(super) enum Posted {
    /// An entry.
    Entry(Entry),
    /// `Outpost::settle`, answered on `0` once the reducer reaches it.
    Settle(mpsc::Sender<()>),
    /// A program's commit.
    Commit {
        request: Request,
        effects: Box<Effects>,
    },
    /// A program ended.
    Finished { request: Request, panicked: bool },
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
/// appends to and only the shell drains.
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
                Posted::Entry(_)
                    | Posted::Ask(_)
                    | Posted::CancelAsk(_)
                    | Posted::WindowDestroyed(_)
                    | Posted::Settle(_)
            )
        {
            return;
        }
        state.posted.push_back(posted);
        self.rouse(state);
    }

    /// Appends `entry`, from `source`.
    pub(super) fn push(&self, entry: Entry) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        state.posted.push_back(Posted::Entry(entry));
        self.rouse(state);
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
    pub(super) parked: Vec<super::store::Parked>,
}

impl Garbage {
    fn is_empty(&self) -> bool {
        self.store.is_empty() && self.parked.is_empty()
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
    /// Who waits on each test's settle, by its number: kept while the
    /// settle waits in the intake ([`Reducer::settles`]), answered as the
    /// reducer reaches it, and dropped as the outpost closes, which ends
    /// the wait too.
    settles: HashMap<u64, mpsc::Sender<()>>,
    /// The last settle numbered.
    settles_taken: u64,
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

/// The change the reducer is given for `effects`, a commit of `request`:
/// for a commit it accepts, its text writes made to the store, in the
/// order the program made them ([`Store::write_text`]). Any other is
/// refused whole, `None`: its writes are not made and its objects are let
/// go of into `garbage`.
fn admit(
    store: &Store,
    reducer: &Reducer,
    (request, effects): (Request, Effects),
    garbage: &mut Garbage,
) -> Option<Change> {
    let (change, text) = effects.split();
    if reducer.accepts(request) {
        if !text.is_empty() {
            garbage.store.push(store.write_text(text));
        }
        return Some(change);
    }
    garbage.store.push(Removed::of_text(text));
    None
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
            settles: HashMap::new(),
            settles_taken: 0,
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
        // No program is in flight: what the shell let go of is dropped by
        // the worker in charge, as at runtime, and the workers end once
        // their channels close.
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
            Posted::Entry(entry) => Input::Entry(entry),
            Posted::Settle(done) => {
                let Some(input) = self.settle(done) else {
                    return;
                };
                input
            }
            Posted::Ask(ask) => {
                tracing::debug!(request = %ask.number, "request taken by the reducer");
                Input::Ask(ask)
            }
            Posted::CancelAsk(cancel) => Input::CancelAsk(cancel),
            Posted::WindowDestroyed(window) => Input::WindowDestroyed(window),
            Posted::Answered { request, answer } => {
                tracing::debug!(request = %answer.number, "answer taken by the reducer");
                Input::Answered { request, answer }
            }
            Posted::Commit { request, effects } => {
                // A dead generation's or a claimed request's commit gives
                // the reducer nothing, and its objects go to a worker.
                let Some(change) = admit(
                    &self.context.store,
                    &self.reducer,
                    (request, *effects),
                    &mut self.garbage,
                ) else {
                    return;
                };
                Input::Commit {
                    request,
                    change: Box::new(change),
                }
            }

            Posted::Finished { request, panicked } => Input::Finished {
                request,
                reported: false,
                panicked,
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
        };
        Some(Input::Entry(Entry::own(
            barrier,
            TraceId::mint(),
            self.now().wall_us,
        )))
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
                told,
                acknowledged,
            } => return vec![self.release((&nodes, &anchors, &told), acknowledged)],
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

    /// Checks, in debug builds, that the shell's table matches the
    /// reducer's: a waiter for every settle waiting in the intake, and no
    /// other.
    fn check_tables(&self) {
        debug_assert_eq!(
            self.settles
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<u64>>(),
            self.reducer.settles(),
            "a waiter for every settle waiting in the intake, and no other"
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
        if let verbatim_outpost_logic::Work::Ask(ask) = &program.work {
            tracing::debug!(request = %ask.number, "request dispatched to the worker");
        }
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
                panicked: true,
            });
        }
    }

    /// Sends `message` to Core.
    fn send(&self, message: OutpostToCore) {
        self.context.send(message);
    }

    /// Releases every node, text anchor and caret tell's element Core does
    /// not hold and has handled every message carrying (`super::store`, the
    /// release rule), for Core's held lists, which acknowledge position
    /// `acknowledged`; the removed objects go to the next program's worker.
    fn release(
        &mut self,
        (nodes, anchors, told): (&[u64], &[u64], &[u64]),
        acknowledged: u64,
    ) -> Input {
        let held: HashSet<Carried> = nodes
            .iter()
            .map(|&number| Carried::Node(number))
            .chain(anchors.iter().map(|&number| Carried::Anchor(number)))
            .chain(told.iter().map(|&key| Carried::Told(key)))
            .collect();
        let (position, released) =
            self.context
                .store
                .release(&self.context.outbound, &held, acknowledged);
        let Some(released) = released else {
            tracing::debug!(?held, acknowledged, position, "nothing to release");
            return Input::Released { objects: false };
        };
        tracing::debug!(
            ?held,
            acknowledged,
            position,
            nodes = ?released.nodes,
            anchors = ?released.anchors,
            "releasing the nodes, anchors and caret tells' elements Core has let go of"
        );
        self.garbage.store.push(released.objects);
        Input::Released {
            objects: !self.garbage.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use verbatim_model::protocol::Urgent;

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
    /// anchor its text call minted is not kept in the store, its change is
    /// not in the store's text facts, and the reducer is given nothing.
    /// The same commit of the program in flight has its anchor kept and its
    /// change in the text facts the next program's copy is taken from.
    #[test]
    fn a_refused_commits_text_writes_are_not_made() {
        let store = Store::new();
        let mut reducer = Reducer::new();
        let entry = Entry::own(
            Item::Msaa {
                kind: super::super::reducer::WinKind::ConsoleUpdate,
                hwnd: 5,
                id_object: 3,
                id_child: 65,
                raiser: 3,
            },
            TraceId::mint(),
            1,
        );
        let effects = reducer.input(Input::Entry(entry), Now::default());
        let in_flight = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Dispatch(program) => Some(program.request),
                _ => None,
            })
            .expect("the entry's program dispatched");
        let node = store
            .nodes_for(verbatim_model::property::NodeDetailSet::ALL)
            .window(5)
            .number();
        let dead = Request {
            generation: 7,
            number: 99,
        };
        let mut garbage = Garbage::default();
        let (refused, _) = minted(node, 7);
        let admitted = admit(&store, &reducer, (dead, refused), &mut garbage);
        assert!(admitted.is_none());
        assert!(!store.keeps_anchor(7));
        assert_eq!(store.text_facts().edit(node), None);
        let (accepted, change) = minted(node, 8);
        admit(&store, &reducer, (in_flight, accepted), &mut garbage)
            .expect("the change of the program in flight");
        let TextChange::Edit { part, .. } = change else {
            unreachable!("an edit control's change")
        };
        assert_eq!(store.text_facts().edit(node), Some(&part));
        assert!(store.keeps_anchor(8));
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
            settles: HashMap::new(),
            settles_taken: 0,
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
            Urgent::Pong { parked_count } => parked_count,
            other => panic!("not the ping's pong: {other:?}"),
        };
        assert_eq!(counts(&shell), 0);
        shell.apply(vec![abandon(0)]);
        assert_eq!(counts(&shell), 1);
        shell.apply(vec![abandon(1)]);
        assert_eq!(counts(&shell), 2, "one more");
        shell.take(Posted::Returned(0));
        assert_eq!(counts(&shell), 1, "one returned");
        end_workers(&mut shell);
    }
}
