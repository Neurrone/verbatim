//! Core's inbox: the one place on Core's thread where everything arrives,
//! in arrival order (ADR 0003, "Core's inbox and the event queue"). What is
//! handled at once is handled here: answers, timers, the foreground
//! report, readers starting and ending, hang checks and window
//! destruction; an entry joins the back of the event queue. Nothing waits
//! here.

use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Probe, ProcessFact, Reply, Request as Wire, RequestNumber,
};
use verbatim_model::{Pid, WindowHandle};

use super::driver::{Beginning, Core, State, outcome_of, widen};
use super::limiter::{self, Item};
use super::request::Outcome;
use super::table::{Asker, Dropped, Ended, Ending, Owner};
use super::{Cause, Foreground, Instance, Logic, Reader, RequestWindow};

/// An input to Core, in the inbox's arrival order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Input<E> {
    /// An entry joins the back of the event queue.
    Entry(E),
    /// A reader's answer.
    Answer {
        /// The instance that answered.
        from: Instance,
        /// The answer.
        answer: Answer,
    },
    /// The shell found the request's window hung before sending it, and
    /// answers it "not responding" unsent.
    RefusedHung(RequestNumber),
    /// The shell answers a read of an application's current focus unsent
    /// with what it read in front, its window hung or, for the check after
    /// a menu, popup or switcher end, no window in front; the read's entry
    /// judges it: no sound plays and nothing is watched here.
    InFront {
        /// The request.
        number: RequestNumber,
        /// What is in front.
        front: verbatim_model::request::Front,
    },
    /// The shell's half-second check of a window.
    HangCheck {
        /// The window.
        window: WindowHandle,
        /// Whether Windows reports it hung.
        hung: bool,
    },
    /// The listener's foreground report.
    Foreground(Foreground),
    /// A reader's instance can take requests: the listener's
    /// `ListenerReady`, or an outpost's `Ready` frame (Table 5, column D).
    ReaderReady(Instance),
    /// A reader's instance ended: its pipe closed or its process exited
    /// (Table 5, column E).
    ReaderEnded(Instance),
    /// A process fact the owner thread reports for the outpost generation
    /// record (Table 5).
    Process(ProcessFact),
    /// A window was destroyed, its destruction raised by a thread of
    /// `process`, `None` when the listener could not open that thread.
    WindowDestroyed {
        /// The window.
        window: WindowHandle,
        /// The raising thread's process.
        process: Option<Pid>,
    },
    /// A request from outside the event queue: Verbatim's dialogs, the
    /// control plane, the tree dump. It holds no key, and Core does not
    /// stop waiting for it when the foreground changes.
    AskOutside {
        /// Who asks.
        asker: Asker,
        /// The reader asked.
        reader: Reader,
        /// The window it names.
        window: RequestWindow,
        /// The request.
        request: Wire,
        /// How long Core waits for it.
        due_after_us: u64,
    },
    /// An asker outside the queue no longer needs its request.
    WithdrawOutside(Asker),
    /// Requests Core sends outside the queue as it handles an input at
    /// once, each awaiting no answer: its ending changes nothing in Core.
    Send(Vec<CoreRequest>),
    /// Core's facts changed outside the queue: the entries they want join
    /// the back, and the front's request ends when the one judge says the
    /// settled queue ends it ([`Logic::joining`], [`Logic::ends`]).
    Settle,
    /// A timer came due.
    Tick,
}

/// A request Core sends outside the queue, in order with the queue's
/// requests: awaiting no answer, or awaited by Core's facts under a token.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreRequest {
    /// The token Core's facts await its ending by
    /// ([`Logic::facts_answered`]); `None` when nothing awaits it.
    pub(crate) token: Option<u64>,
    /// The reader asked.
    pub(crate) reader: Reader,
    /// The window it names.
    pub(crate) window: RequestWindow,
    /// The request.
    pub(crate) request: Wire,
    /// How long Core waits for it.
    pub(crate) due_after_us: u64,
}

/// An input with the time it carries, in microseconds on the run's shared
/// clock.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Timed<E> {
    /// When.
    pub(crate) at_us: u64,
    /// What.
    pub(crate) input: Input<E>,
}

/// What Core does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect<S> {
    /// The shell checks the request's window for a hang, then sends it to
    /// the instance, or answers it "not responding" unsent
    /// ([`Input::RefusedHung`]).
    Send {
        /// The instance.
        to: Instance,
        /// The request.
        ask: Ask,
    },
    /// The instance is told to cancel a request.
    Cancel {
        /// The instance.
        to: Instance,
        /// The cancel.
        cancel: Cancel,
    },
    /// The shell checks these windows for a hang ([`Input::HangCheck`]).
    CheckHung(Vec<WindowHandle>),
    /// The not-responding sound.
    NotRespondingSound,
    /// The instance, the outpost of the process that raised a window's
    /// destruction, forgets that window.
    WindowDestroyed {
        /// The instance.
        to: Instance,
        /// The window.
        window: WindowHandle,
    },
    /// A request outside the queue ended, for its asker.
    Answered {
        /// The asker.
        asker: Asker,
        /// The outcome, its result as it arrived.
        outcome: Outcome<Reply>,
    },
    /// An answer was dropped: logged.
    AnswerDropped {
        /// The request it named.
        number: RequestNumber,
        /// Whether it was late or named no request out to its sender.
        late: bool,
    },
    /// A result did not decode as its request's reply: a malformed answer,
    /// logged, since every answer a correct reader gives decodes.
    Undecodable(RequestNumber),
    /// What an entry's function emitted.
    Said(S),
    /// The owner thread starts an outpost for this application.
    StartOutpost(Pid),
    /// The owner thread ends this outpost generation.
    EndOutpost(Instance),
    /// A frame of an instance that is not its reader's current one was
    /// dropped: logged.
    StaleFrame(Instance),
}

impl<L: Logic> Core<L> {
    /// Handles one input and returns what Core did, in order.
    pub(crate) fn feed(&mut self, timed: Timed<L::Entry>) -> Vec<Effect<L::Said>> {
        self.shared.borrow_mut().state.queue.now_us = timed.at_us;
        match timed.input {
            Input::Entry(entry) => self.arrive(entry),
            Input::Answer { from, answer } => self.answered(from, answer),
            Input::RefusedHung(number) => {
                let ended = self
                    .shared
                    .borrow_mut()
                    .state
                    .queue
                    .table
                    .refused_hung(number);
                if let Some(ended) = ended {
                    self.hung(&ended.window);
                    self.deliver(ended);
                }
            }
            Input::InFront { number, front } => self.in_front(number, front),
            Input::HangCheck { window, hung } => self.hang_check(window, hung),
            Input::Foreground(foreground) => self.foreground(foreground),
            Input::ReaderReady(instance) if self.admit_ready(instance) => {
                let mut shared = self.shared.borrow_mut();
                shared.state.queue.readers.insert(instance.reader, instance);
                let now_us = shared.state.queue.now_us;
                let mut orders = Vec::new();
                let state = &mut shared.state;
                state
                    .queue
                    .table
                    .reader_ready(instance, &state.queue.backend, now_us, &mut orders);
                Self::order(&mut shared, orders);
            }
            Input::ReaderEnded(instance) => self.instance_ended(instance),
            Input::Process(fact) => self.process_fact(fact),
            Input::WindowDestroyed { window, process } => {
                // Handled at once, ahead of the waiting entries, whose
                // reads in the window answer "not there" at their turn,
                // as NVDA handles a destroy in its hook's callback
                // (`IAccessibleHandler/internalWinEventHandler.py` 101 to
                // 103). What Core keeps by window is cleared whether or
                // not the raising process is known.
                let mut shared = self.shared.borrow_mut();
                let state = &mut shared.state;
                state.queue.table.unwatch(window);
                state.queue.backend.destroyed(window);
                let follows = L::window_destroyed(&mut state.facts, window);
                // Routed by the raising thread's process, since a destroyed
                // window can no longer be read for its owner; an outpost
                // not running holds nothing of it, and with no process
                // known nothing is sent.
                let to = process
                    .and_then(|process| state.queue.readers.get(&Reader::Outpost(process)))
                    .copied();
                if let Some(to) = to {
                    shared.effects.push(Effect::WindowDestroyed { to, window });
                }
                // Behind the effect on the same pipe, so each runs after
                // the outpost's forget.
                Self::begin_core(&mut shared, follows);
            }
            Input::AskOutside {
                asker,
                reader,
                window,
                request,
                due_after_us,
            } => {
                let mut shared = self.shared.borrow_mut();
                let due_us = shared.state.queue.now_us.saturating_add(due_after_us);
                shared.begin(Beginning {
                    reader,
                    window,
                    due_us,
                    request,
                    owner: Owner::Outside(asker),
                });
            }
            Input::Send(requests) => self.send_core(requests),
            Input::WithdrawOutside(asker) => {
                let numbers = self.shared.borrow().state.queue.table.asked_by(asker);
                for number in numbers {
                    self.stop(number, CancelReason::Withdrawn);
                }
            }
            Input::Tick => self.tick(),
            // Core's facts changed outside the queue: settled by the step's
            // end, as every input is. A `Ready` not admitted was logged by
            // `admit_ready`.
            Input::Settle | Input::ReaderReady(_) => {}
        }
        loop {
            self.take_next();
            let answered = self.answer_at_once();
            let wanted = self.want_outposts();
            if !answered && !wanted {
                break;
            }
        }
        self.finish_step()
    }

    /// The shell answered `number`, a read of an application's current
    /// focus, unsent, with `front` in front: it ends with that answer,
    /// which its entry judges. Only a read of the current focus asked by an
    /// entry decodes it; one asked from outside the queue is a defect,
    /// logged as an undecodable answer.
    fn in_front(&mut self, number: RequestNumber, front: verbatim_model::request::Front) {
        let ended = self
            .shared
            .borrow_mut()
            .state
            .queue
            .table
            .in_front(number, front);
        if let Some(ended) = ended {
            if ended.owner != Owner::Queue {
                self.shared
                    .borrow_mut()
                    .effects
                    .push(Effect::Undecodable(number));
            }
            self.deliver(ended);
        }
    }

    /// Sends `requests` as Core's own, outside the queue, each awaiting no
    /// answer or awaited by Core's facts under its token.
    fn send_core(&mut self, requests: Vec<CoreRequest>) {
        let mut shared = self.shared.borrow_mut();
        Self::begin_core(&mut shared, requests);
    }

    /// Begins `requests` as Core's own, outside the queue, in order.
    pub(super) fn begin_core(shared: &mut super::driver::Shared<L>, requests: Vec<CoreRequest>) {
        let now_us = shared.state.queue.now_us;
        for sent in requests {
            shared.begin(Beginning {
                reader: sent.reader,
                window: sent.window,
                due_us: now_us.saturating_add(sent.due_after_us),
                request: sent.request,
                owner: sent.token.map_or(Owner::Core, Owner::Facts),
            });
        }
    }

    /// A reader's answer: the request it names ends with it, and the
    /// probes it brings are recorded; a late answer, or one naming no
    /// request out to its sender, is dropped.
    fn answered(&mut self, from: Instance, answer: Answer) {
        let answered =
            self.shared
                .borrow_mut()
                .state
                .queue
                .table
                .answer(from, answer.number, answer.outcome);
        match answered {
            Ok(ended) => {
                // A probe of the request's own window is kept under the
                // request's key: the outpost names the window's owner by
                // `GetWindowThreadProcessId`, which for a console window
                // is its client's thread, while the tells name it by the
                // console host's thread that raised the event, so keyed by
                // the outpost's thread it would judge no later tell.
                let probed = {
                    let backend = &mut self.shared.borrow_mut().state.queue.backend;
                    for probe in &answer.probes {
                        if probe.window == ended.window.handle {
                            backend.record(&Probe {
                                thread: ended.window.thread,
                                ..*probe
                            });
                        } else {
                            backend.record(probe);
                        }
                    }
                    answer
                        .probes
                        .iter()
                        .any(|probe| probe.window == ended.window.handle)
                };
                self.deliver_probed(ended, probed);
            }
            Err(dropped) => self
                .shared
                .borrow_mut()
                .effects
                .push(Effect::AnswerDropped {
                    number: answer.number,
                    late: dropped == Dropped::Late,
                }),
        }
    }

    /// `entry` joins the back; the front's request ends when the one judge
    /// says its arrival ends it, judged before the limiter can merge it.
    fn arrive(&mut self, entry: L::Entry) {
        let ended = {
            let shared = self.shared.borrow();
            let state = &shared.state;
            state
                .queue
                .front_ends::<L>(&state.facts, Cause::Arrived(&entry))
        };
        {
            let mut shared = self.shared.borrow_mut();
            let state = &mut shared.state;
            if let Some(probe) = L::probe(&entry) {
                state.queue.backend.record(&probe);
            }
            let front = state.queue.front.as_ref().map(|front| &front.entry);
            let dropped = L::dropped_at_arrival(
                &state.facts,
                (front, &state.queue.waiting),
                &entry,
                &state.queue.backend,
                (state.queue.now_us, state.queue.foreground),
            );
            if !dropped {
                join::<L>(state, entry);
            }
        }
        if let Some((number, reason)) = ended {
            self.stop(number, reason);
        }
    }

    /// The entries Core's facts want join the back, and the front's
    /// request ends when the one judge says the settled queue ends it.
    pub(super) fn settle(&mut self) {
        let ended = {
            let mut shared = self.shared.borrow_mut();
            let wanted = L::wanted(&mut shared.state.facts);
            Self::begin_core(&mut shared, wanted);
            let state = &mut shared.state;
            for entry in L::joining(&mut state.facts) {
                join::<L>(state, entry);
            }
            state.queue.front_ends::<L>(&state.facts, Cause::Settled)
        };
        if let Some((number, reason)) = ended {
            self.stop(number, reason);
        }
    }

    /// Stops waiting for `number`, telling its reader to cancel it when it
    /// is out, and delivers its ending.
    fn stop(&mut self, number: RequestNumber, reason: CancelReason) {
        let ended = {
            let mut shared = self.shared.borrow_mut();
            let mut orders = Vec::new();
            let ended = shared.state.queue.table.stop(number, reason, &mut orders);
            Self::order(&mut shared, orders);
            ended
        };
        if let Some(ended) = ended {
            if reason == CancelReason::Hung {
                self.hung(&ended.window);
            }
            self.deliver(ended);
        }
    }

    /// A request on `window` was refused or stopped because the window is
    /// hung: while its application holds attention, the sound plays and
    /// the window joins the recovery set.
    fn hung(&mut self, window: &RequestWindow) {
        let mut shared = self.shared.borrow_mut();
        shared.sound_if_attended(window);
        shared.watch_if_attended(window);
    }

    /// Delivers a request's ending to its owner, its probe unanswered.
    pub(super) fn deliver(&mut self, ended: Ended) {
        self.deliver_probed(ended, false);
    }

    /// Delivers a request's ending to its owner: the front's function,
    /// resumed, or the asker outside the queue. A request sent to an
    /// outpost carrying "probe first" whose answer brought no probe of its
    /// window counts as a probe that timed out, begun when it was sent
    /// (Table 4, row "MSAA in force"); one its entry withdrew does not,
    /// since its reader runs it on and its probe may yet answer, so its
    /// window keeps no verdict and the next request or tell is judged by a
    /// probe of its own. A request to the focus listener is never probing,
    /// since the listener never probes on a request.
    fn deliver_probed(&mut self, ended: Ended, probed: bool) {
        let withdrawn = matches!(
            ended.ending,
            Ending::NoLongerAwaited(CancelReason::Withdrawn)
        );
        if let (Some(sent_us), false, false) = (ended.probing, probed, withdrawn) {
            self.shared
                .borrow_mut()
                .state
                .queue
                .backend
                .unanswered(ended.window.judged().key, sent_us);
        }
        match ended.owner {
            Owner::Queue => self.resume(ended.ending),
            Owner::Core => {}
            Owner::Facts(token) => {
                let outcome = match outcome_of(ended.ending) {
                    Ok(reply) => Outcome::Result(reply),
                    Err(outcome) => widen(outcome),
                };
                let mut shared = self.shared.borrow_mut();
                let foreground = shared.state.queue.foreground;
                let now_us = shared.state.queue.now_us;
                let mut said = Vec::new();
                let state = &mut shared.state;
                let joining = L::facts_answered(
                    &mut state.facts,
                    token,
                    outcome,
                    (now_us, foreground),
                    &mut said,
                );
                for entry in joining {
                    join::<L>(state, entry);
                }
                shared.effects.extend(said.into_iter().map(Effect::Said));
            }
            Owner::Outside(asker) => {
                let outcome = match outcome_of(ended.ending) {
                    Ok(reply) => Outcome::Result(reply),
                    Err(outcome) => widen(outcome),
                };
                self.shared
                    .borrow_mut()
                    .effects
                    .push(Effect::Answered { asker, outcome });
            }
        }
    }

    /// The shell's check of `window`: every request out on it stops when it
    /// is hung; a recovering window that answers again leaves the set, and
    /// the logic's entry for it joins the back.
    fn hang_check(&mut self, window: WindowHandle, hung: bool) {
        if hung {
            let out = self.shared.borrow().state.queue.table.out_on(window);
            for number in out {
                self.stop(number, CancelReason::Hung);
            }
            return;
        }
        let mut shared = self.shared.borrow_mut();
        let Some(recovered) = shared.state.queue.table.unwatch(window) else {
            return;
        };
        let state = &mut shared.state;
        if let Some(entry) = L::recovered(&mut state.facts, &recovered) {
            join::<L>(state, entry);
        }
    }

    /// The listener's foreground report: the front's request ends when the
    /// one judge says the new record ends it; a waiting request is never
    /// sent. Requests
    /// outside the queue are untouched. Recovering windows of applications
    /// that lost attention leave the set.
    fn foreground(&mut self, foreground: Foreground) {
        self.attended(foreground.process);
        let stopped = {
            let mut shared = self.shared.borrow_mut();
            let first = shared.state.queue.foreground.is_none();
            shared.state.queue.foreground = Some(foreground);
            if first && let Some(entry) = L::started(&shared.state.facts, &foreground) {
                let state = &mut shared.state;
                join::<L>(state, entry);
            }
            shared
                .state
                .queue
                .table
                .unwatch_unless(|window| foreground.attends(window));
            let state = &shared.state;
            state
                .queue
                .front_ends::<L>(&state.facts, Cause::Foreground(&foreground))
        };
        if let Some((number, reason)) = stopped {
            self.stop(number, reason);
        }
        let mut shared = self.shared.borrow_mut();
        let state = &mut shared.state;
        let facts = &state.facts;
        let mut ended = Vec::new();
        state.queue.waiting.retain_mut(|item| match item {
            Item::One(entry) | Item::Menu { entry, .. } => {
                let ends = L::ends_at_foreground(facts, entry, &foreground);
                if ends {
                    ended.push(entry.clone());
                }
                !ends
            }
            Item::Run(candidates) => {
                candidates.retain(|entry| {
                    let ends = L::ends_at_foreground(facts, entry, &foreground);
                    if ends {
                        ended.push(entry.clone());
                    }
                    !ends
                });
                !candidates.is_empty()
            }
        });
        let mut said = Vec::new();
        for entry in &ended {
            L::ended_at_foreground(&mut state.facts, entry, &mut said);
        }
        if let Some(marker) = L::foreground_marker(&foreground) {
            join::<L>(state, marker);
        }
        shared.effects.extend(said.into_iter().map(Effect::Said));
    }

    /// `instance` ended: its requests out end "reader ended", its late ones
    /// are forgotten, and its application's recovering windows leave the
    /// set; requests waiting for its reader wait for the next instance.
    /// With `retake`, Core's facts may ask the focus again (Table 5,
    /// column E); an application's exit or its outpost's retirement takes
    /// nothing again.
    pub(super) fn reader_ended(&mut self, instance: Instance, retake: bool) {
        let ended = {
            let mut shared = self.shared.borrow_mut();
            if shared.state.queue.readers.get(&instance.reader) == Some(&instance) {
                shared.state.queue.readers.remove(&instance.reader);
            }
            if let Reader::Outpost(process) = instance.reader {
                shared
                    .state
                    .queue
                    .table
                    .unwatch_unless(|window| window.process != process);
            }
            shared.state.queue.table.reader_ended(instance)
        };
        {
            let mut shared = self.shared.borrow_mut();
            let state = &mut shared.state;
            state.queue.waiting.retain_mut(|item| match item {
                Item::One(entry) | Item::Menu { entry, .. } => !L::kept_by(entry, instance),
                Item::Run(candidates) => {
                    candidates.retain(|entry| !L::kept_by(entry, instance));
                    !candidates.is_empty()
                }
            });
            let foreground = state.queue.foreground;
            let mut said = Vec::new();
            for entry in
                L::reader_ended(&mut state.facts, instance, (foreground, retake), &mut said)
            {
                join::<L>(state, entry);
            }
            shared.effects.extend(said.into_iter().map(Effect::Said));
        }
        for ended in ended {
            self.deliver(ended);
        }
    }

    /// A timer came due: requests past their due time stop; the hang round
    /// asks the shell to check its windows.
    fn tick(&mut self) {
        let now_us = self.shared.borrow().state.queue.now_us;
        let due = self.shared.borrow().state.queue.table.due(now_us);
        for number in due {
            self.stop(number, CancelReason::DueTime);
        }
        self.outpost_timers();
        let mut shared = self.shared.borrow_mut();
        if let Some(windows) = shared.state.queue.table.round(now_us) {
            shared.effects.push(Effect::CheckHung(windows));
        }
        let mut said = Vec::new();
        let foreground = shared.state.queue.foreground;
        let joining = L::tick(&mut shared.state.facts, now_us, foreground, &mut said);
        shared.effects.extend(said.into_iter().map(Effect::Said));
        let state = &mut shared.state;
        for entry in joining {
            join::<L>(state, entry);
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut state.queue.timers)
            .into_iter()
            .partition(|(at_us, _)| *at_us <= now_us);
        state.queue.timers = later;
        for (_, entry) in due {
            join::<L>(state, entry);
        }
    }
}

/// `entry` joins the back of the queue: the logic sees it arrive, then the
/// limiter places it.
pub(super) fn join<L: Logic>(state: &mut State<L::Entry, L::Facts>, mut entry: L::Entry) {
    let queue = &state.queue;
    let front = queue.front.as_ref().map(|front| {
        let request = front
            .asked
            .and_then(|number| queue.table.live(number))
            .map(|live| &live.request);
        (&front.entry, request)
    });
    L::arrival(&mut state.facts, front, &mut entry);
    limiter::arrive::<L>(&state.facts, &mut state.queue.waiting, entry);
}
