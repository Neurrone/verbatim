//! Core's inbox: the one place on Core's thread where everything arrives,
//! in arrival order (ADR 0003, "Core's inbox and the event queue"). What is
//! handled at once is handled here: answers, timers, the foreground
//! report, readers starting and ending, hang checks and window
//! destruction; an entry joins the back of the event queue. Nothing waits
//! here.

use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Reply, Request as Wire, RequestNumber,
};
use verbatim_model::{Pid, WindowHandle};

use super::driver::{Core, State, outcome_of, widen};
use super::limiter::{self, Item};
use super::request::Outcome;
use super::table::{Asker, Dropped, Ended, Live, Owner, Stage};
use super::{Foreground, Instance, Logic, Reader, RequestWindow};

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
    /// The shell's half-second check of a window.
    HangCheck {
        /// The window.
        window: WindowHandle,
        /// Whether Windows reports it hung.
        hung: bool,
    },
    /// The listener's foreground report.
    Foreground(Foreground),
    /// A reader's instance can take requests.
    ReaderReady(Instance),
    /// A reader's instance ended.
    ReaderEnded(Instance),
    /// A window was destroyed, its destruction raised by a thread of
    /// `process`.
    WindowDestroyed {
        /// The window.
        window: WindowHandle,
        /// The raising thread's process.
        process: Pid,
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
    /// Core's facts changed outside the queue: the entries they want join
    /// the back, and the front's request is withdrawn when it is no longer
    /// needed ([`Logic::joining`], [`Logic::unneeded`]).
    Settle,
    /// A timer came due.
    Tick,
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
    /// A result did not decode as its request's reply: logged as the
    /// reader's fault.
    Undecodable(RequestNumber),
    /// What an entry's function emitted.
    Said(S),
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
            Input::HangCheck { window, hung } => self.hang_check(window, hung),
            Input::Foreground(foreground) => self.foreground(foreground),
            Input::ReaderReady(instance) => {
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
            Input::ReaderEnded(instance) => self.reader_ended(instance),
            Input::WindowDestroyed { window, process } => {
                let mut shared = self.shared.borrow_mut();
                shared.state.queue.table.unwatch(window);
                shared.state.queue.backend.destroyed(window);
                L::window_destroyed(&mut shared.state.facts, window);
                // Routed by the raising thread's process, since a destroyed
                // window can no longer be read for its owner; an outpost
                // not running holds nothing of it.
                let to = shared
                    .state
                    .queue
                    .readers
                    .get(&Reader::Outpost(process))
                    .copied();
                if let Some(to) = to {
                    shared.effects.push(Effect::WindowDestroyed { to, window });
                }
            }
            Input::AskOutside {
                asker,
                reader,
                window,
                request,
                due_after_us,
            } => {
                let mut shared = self.shared.borrow_mut();
                let ready = shared.state.queue.readers.get(&reader).copied();
                let now_us = shared.state.queue.now_us;
                let mut orders = Vec::new();
                let state = &mut shared.state;
                state.queue.table.begin(
                    Live {
                        reader,
                        window,
                        due_us: now_us.saturating_add(due_after_us),
                        request,
                        owner: Owner::Outside(asker),
                        stage: Stage::Waiting,
                        probing: None,
                    },
                    ready,
                    &state.queue.backend,
                    now_us,
                    &mut orders,
                );
                Self::order(&mut shared, orders);
            }
            Input::WithdrawOutside(asker) => {
                let numbers = self.shared.borrow().state.queue.table.asked_by(asker);
                for number in numbers {
                    self.stop(number, CancelReason::Withdrawn);
                }
            }
            Input::Tick => self.tick(),
            // Settled by the step's end, as every input is.
            Input::Settle => {}
        }
        self.take_next();
        self.finish_step()
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
                let probed = {
                    let backend = &mut self.shared.borrow_mut().state.queue.backend;
                    for probe in &answer.probes {
                        backend.record(probe);
                    }
                    answer.probes.iter().any(|probe| {
                        probe.window == ended.window.handle && probe.thread == ended.window.thread
                    })
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

    /// `entry` joins the back; it withdraws the front's request when the
    /// logic says so.
    fn arrive(&mut self, entry: L::Entry) {
        let withdrawn = {
            let shared = self.shared.borrow();
            let state = &shared.state;
            state.queue.front.as_ref().and_then(|front| {
                front
                    .asked
                    .filter(|_| L::withdraws(&state.facts, &front.entry, &entry))
            })
        };
        {
            let mut shared = self.shared.borrow_mut();
            let state = &mut shared.state;
            if let Some(probe) = L::probe(&entry) {
                state.queue.backend.record(&probe);
            }
            let ahead = state
                .queue
                .front
                .iter()
                .map(|front| &front.entry)
                .chain(state.queue.waiting.iter().flat_map(|item| match item {
                    Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_ref(entry),
                    Item::Run(candidates) => candidates.as_slice(),
                }))
                .collect::<Vec<_>>();
            let dropped = L::dropped_at_arrival(
                &state.facts,
                &ahead,
                &entry,
                &state.queue.backend,
                state.queue.now_us,
            );
            if !dropped {
                join::<L>(state, entry);
            }
        }
        if let Some(number) = withdrawn {
            self.stop(number, CancelReason::Withdrawn);
        }
    }

    /// The entries Core's facts want join the back, and the front's
    /// request is withdrawn when its entry no longer needs it.
    pub(super) fn settle(&mut self) {
        let unneeded = {
            let mut shared = self.shared.borrow_mut();
            let state = &mut shared.state;
            for entry in L::joining(&mut state.facts) {
                join::<L>(state, entry);
            }
            state.queue.front.as_ref().and_then(|front| {
                front
                    .asked
                    .filter(|_| L::unneeded(&state.facts, &front.entry))
            })
        };
        if let Some(number) = unneeded {
            self.stop(number, CancelReason::Withdrawn);
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
        let attended = shared
            .state
            .queue
            .foreground
            .is_some_and(|foreground| foreground.attends(window));
        if attended {
            shared.effects.push(Effect::NotRespondingSound);
            let now_us = shared.state.queue.now_us;
            shared.state.queue.table.watch(*window, now_us);
        }
    }

    /// Delivers a request's ending to its owner, its probe unanswered.
    fn deliver(&mut self, ended: Ended) {
        self.deliver_probed(ended, false);
    }

    /// Delivers a request's ending to its owner: the front's function,
    /// resumed, or the asker outside the queue. A request sent carrying
    /// "probe first" whose answer brought no probe of its window counts as
    /// a probe that timed out, begun when it was sent (Table 4, row "MSAA
    /// in force").
    fn deliver_probed(&mut self, ended: Ended, probed: bool) {
        if let (Some(sent_us), false) = (ended.probing, probed) {
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
        if let Some(entry) = L::recovered(&state.facts, &recovered) {
            join::<L>(state, entry);
        }
    }

    /// The listener's foreground report: the front's request stops when its
    /// window is outside the new foreground window's tree and owner chain,
    /// unless its entry is exempt; a waiting request is never sent. Requests
    /// outside the queue are untouched. Recovering windows of applications
    /// that lost attention leave the set.
    fn foreground(&mut self, foreground: Foreground) {
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
            state.queue.front.as_ref().and_then(|front| {
                let number = front.asked?;
                let live = state.queue.table.live(number)?;
                (!L::exempt_from_foreground(&front.entry, &live.window)
                    && !foreground.holds(&live.window))
                .then_some(number)
            })
        };
        if let Some(number) = stopped {
            self.stop(number, CancelReason::Foreground);
        }
        let mut shared = self.shared.borrow_mut();
        let state = &mut shared.state;
        let facts = &state.facts;
        state.queue.waiting.retain_mut(|item| match item {
            Item::One(entry) | Item::Menu { entry, .. } => {
                !L::ends_at_foreground(facts, entry, &foreground)
            }
            Item::Run(candidates) => {
                candidates.retain(|entry| !L::ends_at_foreground(facts, entry, &foreground));
                !candidates.is_empty()
            }
        });
        if let Some(marker) = L::foreground_marker(&foreground) {
            join::<L>(state, marker);
        }
    }

    /// `instance` ended: its requests out end "reader ended", its late ones
    /// are forgotten, and its application's recovering windows leave the
    /// set; requests waiting for its reader wait for the next instance.
    fn reader_ended(&mut self, instance: Instance) {
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
            for entry in L::reader_ended(&mut state.facts, instance, foreground, &mut said) {
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
        let mut shared = self.shared.borrow_mut();
        if let Some(windows) = shared.state.queue.table.round(now_us) {
            shared.effects.push(Effect::CheckHung(windows));
        }
        let state = &mut shared.state;
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
fn join<L: Logic>(state: &mut State<L::Entry, L::Facts>, mut entry: L::Entry) {
    L::arrival(&mut state.facts, &mut entry);
    limiter::arrive::<L>(&state.facts, &mut state.queue.waiting, entry);
}
