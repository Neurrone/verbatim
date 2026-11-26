//! Core's request table and recovery set (`docs/design/readers-tell-core-asks-build1.md`,
//! Tables 2 and 18).
//!
//! A request is waiting for its reader, out, or no longer awaited; an
//! answered request leaves the table, so any later input naming it finds
//! nothing. Every request has one owner, the front entry's function or an
//! asker outside the queue, and gets exactly one ending, which the table
//! hands back with its owner. No longer awaited is kept only for a request
//! that went out, by its number and the instance it went to, so that its
//! late answer is dropped by number and the record forgotten when that
//! instance ends.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::request::{
    Ask, CancelReason, Front, Outcome as WireOutcome, Question, RequestNumber, WindowVerdict,
};
use verbatim_model::{Pid, WindowHandle};

use super::{Instance, Reader, RequestWindow, Route};
use crate::focus::backend::BackendCache;

/// Who asked a request outside the event queue, by the token the shell
/// keeps its responder under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Asker(pub(crate) u64);

/// Who gets a request's ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Owner {
    /// The front entry's function, through the slot of its ask, which the
    /// paused function keeps beside the state.
    Queue,
    /// An asker outside the queue.
    Outside(Asker),
    /// Core itself, which awaits no answer: a request outside the queue
    /// whose ending changes nothing in Core, such as `Follow`, answered
    /// only for the test harness's idle.
    Core,
    /// Core's facts, which await the ending by the token they gave the
    /// request ([`super::Logic::facts_answered`]): a terminal's reads for
    /// its output, its `Follow` and its baseline
    /// (`docs/design/readers-tell-core-asks-build3.md`).
    Facts(u64),
}

/// Where a live request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Stage {
    /// Waiting for its reader to be able to take it.
    Waiting,
    /// Sent to an instance of its reader.
    Out {
        /// The instance.
        to: Instance,
        /// When it was sent to an outpost carrying "probe first", whose
        /// probe's answer Core records when it ends.
        probing: Option<u64>,
    },
}

/// A request waiting or out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Live {
    /// Who answers it: a reader process, or Core's plain reader, which
    /// answers it in the step it begins, so it never waits or goes out.
    pub(crate) reader: Route,
    /// The window it names.
    pub(crate) window: RequestWindow,
    /// When Core stops waiting for it.
    pub(crate) due_us: u64,
    /// The request.
    pub(crate) request: Question,
    /// Who gets its ending.
    pub(crate) owner: Owner,
    /// Where it is.
    pub(crate) stage: Stage,
}

impl Live {
    /// Sends it to `instance` at `now_us`, carrying the verdict in force
    /// for its window (Table 4, column D). Only an outpost probes on a
    /// request, so only a request to an outpost is probing: the focus
    /// listener reads the object it was told of and never probes, and its
    /// answer, which brings no probe, says nothing of the window's verdict.
    fn send(
        &mut self,
        number: RequestNumber,
        instance: Instance,
        backend: &BackendCache,
        now_us: u64,
    ) -> Order {
        let verdict = backend.for_request(&self.window.judged(), now_us);
        let probing = match (instance.reader, verdict) {
            (Reader::Outpost(_), WindowVerdict::ProbeFirst) => Some(now_us),
            (Reader::Outpost(_), WindowVerdict::Uia | WindowVerdict::Msaa)
            | (Reader::Listener, _) => None,
        };
        self.stage = Stage::Out {
            to: instance,
            probing,
        };
        Order::Send(
            instance,
            Ask {
                number,
                window: self.window.handle,
                verdict,
                request: self.request.clone(),
            },
        )
    }
}

/// How a live request ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Ending {
    /// Its reader answered, or Core answered for it: "not responding"
    /// when the shell refused to send it to a hung window, "ended" when
    /// its reader ended, and Core's plain reader's answer.
    Answered(WireOutcome),
    /// The shell answered a read of an application's current focus unsent
    /// with what it read in front ([`Front`]), which only that request's
    /// reply decodes ([`super::Request::decode_in_front`]).
    InFront(Front),
    /// Core no longer awaits it.
    NoLongerAwaited(CancelReason),
}

/// One live request's ending, handed back to the caller to deliver.
#[derive(Debug)]
pub(crate) struct Ended {
    /// Its owner.
    pub(crate) owner: Owner,
    /// How it ended.
    pub(crate) ending: Ending,
    /// The window it named.
    pub(crate) window: RequestWindow,
    /// When it was sent to an outpost carrying "probe first".
    pub(crate) probing: Option<u64>,
}

/// What the table asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// Check the window for a hang, then send the request to the instance,
    /// or answer it "not responding" unsent.
    Send(Instance, Ask),
    /// Tell the instance to cancel the request.
    Cancel(Instance, verbatim_model::request::Cancel),
}

/// The request table, the recovery set and the half-second round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Table {
    /// The last number given; numbers increase.
    last: u64,
    /// Requests waiting or out.
    #[serde(with = "super::pairs")]
    live: BTreeMap<RequestNumber, Live>,
    /// Requests no longer awaited after they went out, with the instance
    /// they went to.
    #[serde(with = "super::pairs")]
    late: BTreeMap<RequestNumber, Instance>,
    /// Windows Core stopped waiting on, or refused unsent, as hung while
    /// their application held attention, watched until they answer again
    /// (Table 18).
    recovery: BTreeMap<u64, RequestWindow>,
    /// When the next half-second hang round is due, armed only while
    /// something is to be checked.
    round_due_us: Option<u64>,
}

impl Table {
    /// The batch request `number` is, while it waits or is out.
    #[cfg(test)]
    pub(crate) fn batch_for_tests(&self, number: u64) -> Option<verbatim_model::batch::Batch> {
        match &self.live.get(&RequestNumber(number))?.request {
            Question::Batch(batch) => Some(batch.clone()),
            Question::Terminal(_) => None,
        }
    }

    /// The batches waiting or out, by number.
    #[cfg(test)]
    pub(crate) fn batches_for_tests(&self) -> Vec<(u64, verbatim_model::batch::Batch)> {
        self.live
            .iter()
            .filter_map(|(number, live)| match &live.request {
                Question::Batch(batch) => Some((number.0, batch.clone())),
                Question::Terminal(_) => None,
            })
            .collect()
    }

    /// The number of the newest request begun.
    #[cfg(test)]
    pub(crate) fn last_for_tests(&self) -> u64 {
        self.last
    }

    /// The requests waiting or out, as sent.
    pub(crate) fn requests(&self) -> impl Iterator<Item = &Question> {
        self.live.values().map(|live| &live.request)
    }

    /// Records a new request and returns its number; sends it at once to
    /// `ready`, the reader's running instance, if there is one.
    pub(crate) fn begin(
        &mut self,
        live: Live,
        ready: Option<Instance>,
        backend: &BackendCache,
        now_us: u64,
        orders: &mut Vec<Order>,
    ) -> RequestNumber {
        self.last = self.last.saturating_add(1);
        let number = RequestNumber(self.last);
        let mut live = live;
        if let (Some(instance), Route::Reader(_)) = (ready, live.reader) {
            orders.push(live.send(number, instance, backend, now_us));
            self.arm_round(now_us);
        }
        self.live.insert(number, live);
        number
    }

    /// Core's plain reader answered `number`, a request it routes, at
    /// once: it ends with that answer.
    pub(crate) fn answered_at_once(
        &mut self,
        number: RequestNumber,
        outcome: WireOutcome,
    ) -> Option<Ended> {
        let plain = self
            .live
            .get(&number)
            .is_some_and(|live| live.reader == Route::Plain);
        if !plain {
            return None;
        }
        self.end(number, Ending::Answered(outcome), &mut Vec::new())
    }

    /// The live request `number`.
    pub(crate) fn live(&self, number: RequestNumber) -> Option<&Live> {
        self.live.get(&number)
    }

    /// Sends every request waiting for `instance`'s reader to it, in
    /// number order.
    pub(crate) fn reader_ready(
        &mut self,
        instance: Instance,
        backend: &BackendCache,
        now_us: u64,
        orders: &mut Vec<Order>,
    ) {
        let mut sent = false;
        for (number, live) in &mut self.live {
            if live.reader == Route::Reader(instance.reader) && live.stage == Stage::Waiting {
                orders.push(live.send(*number, instance, backend, now_us));
                sent = true;
            }
        }
        if sent {
            self.arm_round(now_us);
        }
    }

    /// Ends `number` as `ending`, telling its instance to cancel it when it
    /// is out and Core stopped waiting for it; a request out that Core no
    /// longer awaits is kept as late.
    fn end(
        &mut self,
        number: RequestNumber,
        ending: Ending,
        orders: &mut Vec<Order>,
    ) -> Option<Ended> {
        let live = self.live.remove(&number)?;
        if let (Stage::Out { to: instance, .. }, Ending::NoLongerAwaited(reason)) =
            (live.stage, &ending)
        {
            orders.push(Order::Cancel(
                instance,
                verbatim_model::request::Cancel {
                    number,
                    reason: *reason,
                },
            ));
            self.late.insert(number, instance);
        }
        Some(Ended {
            owner: live.owner,
            ending,
            window: live.window,
            probing: match live.stage {
                Stage::Out { probing, .. } => probing,
                Stage::Waiting => None,
            },
        })
    }

    /// An answer from `from` to `number`: the live request's ending when
    /// `from` is the instance it is out to; `Err` with whether it is late
    /// (no longer awaited) otherwise, dropped by number.
    pub(crate) fn answer(
        &mut self,
        from: Instance,
        number: RequestNumber,
        outcome: WireOutcome,
    ) -> Result<Ended, Dropped> {
        if self
            .live
            .get(&number)
            .is_some_and(|live| matches!(live.stage, Stage::Out { to, .. } if to == from))
        {
            return self
                .end(number, Ending::Answered(outcome), &mut Vec::new())
                .ok_or(Dropped::Unknown);
        }
        if self.late.get(&number) == Some(&from) {
            self.late.remove(&number);
            return Err(Dropped::Late);
        }
        Err(Dropped::Unknown)
    }

    /// The shell refused to send `number` because its window was hung: it
    /// ends answered "not responding".
    pub(crate) fn refused_hung(&mut self, number: RequestNumber) -> Option<Ended> {
        let out = self
            .live
            .get(&number)
            .is_some_and(|live| matches!(live.stage, Stage::Out { .. }));
        if !out {
            return None;
        }
        self.end(
            number,
            Ending::Answered(WireOutcome::NotResponding),
            &mut Vec::new(),
        )
    }

    /// The shell answered `number`, a read of an application's current
    /// focus, unsent, its window hung or no window in front, with `front`
    /// in front: it ends with that answer, which its entry judges. Nothing is
    /// watched and no sound plays here.
    pub(crate) fn in_front(&mut self, number: RequestNumber, front: Front) -> Option<Ended> {
        let out = self
            .live
            .get(&number)
            .is_some_and(|live| matches!(live.stage, Stage::Out { .. }));
        if !out {
            return None;
        }
        self.end(number, Ending::InFront(front), &mut Vec::new())
    }

    /// Stops waiting for `number` for `reason`, cancelling it if it is out.
    pub(crate) fn stop(
        &mut self,
        number: RequestNumber,
        reason: CancelReason,
        orders: &mut Vec<Order>,
    ) -> Option<Ended> {
        self.end(number, Ending::NoLongerAwaited(reason), orders)
    }

    /// Every live request whose due time has come.
    pub(crate) fn due(&self, now_us: u64) -> Vec<RequestNumber> {
        self.live
            .iter()
            .filter(|(_, live)| live.due_us <= now_us)
            .map(|(number, _)| *number)
            .collect()
    }

    /// Every request out naming `window`.
    pub(crate) fn out_on(&self, window: WindowHandle) -> Vec<RequestNumber> {
        self.live
            .iter()
            .filter(|(_, live)| {
                matches!(live.stage, Stage::Out { .. }) && live.window.handle == window
            })
            .map(|(number, _)| *number)
            .collect()
    }

    /// Every live request outside the queue asked by `asker`.
    pub(crate) fn asked_by(&self, asker: Asker) -> Vec<RequestNumber> {
        self.live
            .iter()
            .filter(|(_, live)| live.owner == Owner::Outside(asker))
            .map(|(number, _)| *number)
            .collect()
    }

    /// `instance` ended: every request out to it ends answered "ended";
    /// its late requests are forgotten; waiting ones stay waiting for the
    /// next instance.
    pub(crate) fn reader_ended(&mut self, instance: Instance) -> Vec<Ended> {
        self.late.retain(|_, to| *to != instance);
        let out: Vec<RequestNumber> = self
            .live
            .iter()
            .filter(|(_, live)| matches!(live.stage, Stage::Out { to, .. } if to == instance))
            .map(|(number, _)| *number)
            .collect();
        out.into_iter()
            .filter_map(|number| {
                self.end(
                    number,
                    Ending::Answered(WireOutcome::Ended),
                    &mut Vec::new(),
                )
            })
            .collect()
    }

    /// Every request waiting for `reader` or out to it ends answered
    /// `outcome`, and its late requests are forgotten: its application
    /// exited (Table 5, column J).
    pub(crate) fn answer_all(&mut self, reader: Reader, outcome: &WireOutcome) -> Vec<Ended> {
        self.late.retain(|_, instance| instance.reader != reader);
        let numbers: Vec<RequestNumber> = self
            .live
            .iter()
            .filter(|(_, live)| live.reader == Route::Reader(reader))
            .map(|(number, _)| *number)
            .collect();
        numbers
            .into_iter()
            .filter_map(|number| {
                self.end(number, Ending::Answered(outcome.clone()), &mut Vec::new())
            })
            .collect()
    }

    /// Whether a request waits for `reader`.
    pub(crate) fn waiting_for(&self, reader: Reader) -> bool {
        self.live
            .values()
            .any(|live| live.reader == Route::Reader(reader) && live.stage == Stage::Waiting)
    }

    /// Whether no request waits or is out and no window is watched for
    /// recovery.
    pub(crate) fn idle(&self) -> bool {
        self.live.is_empty() && self.recovery.is_empty()
    }

    /// The readers requests wait for, each once.
    pub(crate) fn waiting_readers(&self) -> impl Iterator<Item = Reader> + '_ {
        let mut readers: Vec<Reader> = self
            .live
            .values()
            .filter(|live| live.stage == Stage::Waiting)
            .filter_map(|live| match live.reader {
                Route::Reader(reader) => Some(reader),
                Route::Plain => None,
            })
            .collect();
        readers.sort();
        readers.dedup();
        readers.into_iter()
    }

    /// Adds `window` to the recovery set, to be watched every half second.
    pub(crate) fn watch(&mut self, window: RequestWindow, now_us: u64) {
        self.recovery.insert(window.handle.0, window);
        self.arm_round(now_us);
    }

    /// Whether a window of `process` is in the recovery set.
    pub(crate) fn recovering(&self, process: Pid) -> bool {
        self.recovery
            .values()
            .any(|window| window.process == process)
    }

    /// Removes `window` from the recovery set.
    pub(crate) fn unwatch(&mut self, window: WindowHandle) -> Option<RequestWindow> {
        self.recovery.remove(&window.0)
    }

    /// Removes every window of the recovery set that `keep` refuses.
    pub(crate) fn unwatch_unless(&mut self, keep: impl Fn(&RequestWindow) -> bool) {
        self.recovery.retain(|_, window| keep(window));
    }

    /// The windows a hang round checks: every request out's and every
    /// recovering window, each once.
    fn checked(&self) -> Vec<WindowHandle> {
        let mut windows: Vec<WindowHandle> = self
            .live
            .values()
            .filter(|live| matches!(live.stage, Stage::Out { .. }))
            .map(|live| live.window.handle)
            .chain(self.recovery.values().map(|window| window.handle))
            .collect();
        windows.sort_by_key(|window| window.0);
        windows.dedup();
        windows
    }

    /// Arms the half-second round from `now_us`, unless it is armed.
    fn arm_round(&mut self, now_us: u64) {
        if self.round_due_us.is_none() {
            self.round_due_us = Some(now_us.saturating_add(super::HANG_ROUND_US));
        }
    }

    /// The round, if it is due at `now_us`: the windows to check, the
    /// round re-armed while something is to be checked.
    pub(crate) fn round(&mut self, now_us: u64) -> Option<Vec<WindowHandle>> {
        if self.round_due_us.is_none_or(|due| due > now_us) {
            return None;
        }
        self.round_due_us = None;
        let windows = self.checked();
        if windows.is_empty() {
            return None;
        }
        self.arm_round(now_us);
        Some(windows)
    }

    /// Disarms the round when nothing is left to check, so that no wake
    /// is timed that would not be acted on.
    pub(crate) fn settle_round(&mut self) {
        if self.checked().is_empty() {
            self.round_due_us = None;
        }
    }

    /// The earliest time the table waits for: a due time, or the round,
    /// armed only while something is to be checked.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.live
            .values()
            .map(|live| live.due_us)
            .chain(self.round_due_us)
            .min()
    }
}

/// Why an answer was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dropped {
    /// Its request is no longer awaited: a late answer.
    Late,
    /// No request of that number is out to the instance that answered.
    Unknown,
}
