//! The outpost generation record, owned by Core's reducer
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost
//! generation record", Table 5; ruled 2026-11-15): one record per
//! application, none, launching, starting, ready or waiting out a
//! backoff. The supervisor's owner thread reports the process facts as
//! inputs and carries out the effects the record decides (start an
//! outpost, end a generation); it judges no generation itself.
//!
//! This module holds the record and its pure transitions; the event
//! queue's driver applies them, since a generation's start and end move
//! the requests that wait for it or are out to it
//! (`crate::event_queue::outposts`).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use verbatim_model::Pid;

use super::policy::Backoff;

/// How long a generation lives with no pong: three ping intervals of 3
/// seconds (`verbatim-supervisor`'s `Timings`).
pub(crate) const LIVENESS_US: u64 = 9_000_000;

/// The backoff's three times: the second failure's wait, the longest
/// wait, and how long an outpost must have stayed up for its failure to
/// count as the first again (`verbatim-supervisor`'s `Timings` and
/// `STAYED_UP`).
pub(crate) const BACKOFF: (Duration, Duration, Duration) = (
    Duration::from_secs(1),
    Duration::from_secs(30),
    Duration::from_mins(1),
);

/// When idle outposts are retired: how long an application must go
/// without attention, and how often the idle ones are looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retirement {
    /// How long without attention, in microseconds: two minutes.
    pub(crate) idle_us: u64,
    /// How often the check runs, in microseconds: 30 seconds.
    pub(crate) sweep_us: u64,
}

impl Default for Retirement {
    fn default() -> Self {
        Self {
            idle_us: 120_000_000,
            sweep_us: 30_000_000,
        }
    }
}

/// One application's outpost, by the stage of its generation's life
/// (Table 5's rows; none is the application's absence from the record).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Stage {
    /// Core asked for an outpost; no process has been reported.
    Launching,
    /// Its process started; its `Ready` has not arrived.
    Starting(Running),
    /// It takes requests.
    Ready(Running),
    /// It failed; its replacement waits until `due_us`.
    WaitingOut {
        /// When the backoff ends, on the run's shared clock.
        due_us: u64,
    },
}

/// A generation whose process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Running {
    /// Its generation, never reused.
    pub(crate) generation: u64,
    /// When its process was reported started.
    pub(crate) started_us: u64,
    /// When it last answered a ping, or started.
    pub(crate) pong_us: u64,
}

impl Running {
    /// When it is ended for want of a pong.
    pub(crate) fn liveness_due(self) -> u64 {
        self.pong_us.saturating_add(LIVENESS_US)
    }
}

/// One application's record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Application {
    /// Its outpost's stage.
    pub(crate) stage: Stage,
    /// The backoff between its outposts' failures.
    pub(crate) backoff: Backoff,
    /// When it last held attention, which its retirement is judged from.
    pub(crate) attended_us: u64,
}

impl Application {
    /// The generation running for it, starting or ready.
    pub(crate) fn running(&self) -> Option<Running> {
        match self.stage {
            Stage::Starting(running) | Stage::Ready(running) => Some(running),
            Stage::Launching | Stage::WaitingOut { .. } => None,
        }
    }
}

/// What a failure leaves of an application's record (Table 5, columns C
/// and E): a first failure since the count started again is replaced at
/// once, a later one waits out its backoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Replaced {
    /// Launched again at once.
    AtOnce,
    /// Waiting out the backoff.
    Later,
}

/// The record of every application's outpost, the newest listener the
/// owner thread reported started, Verbatim's own process, and how many
/// spares wait.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    /// Each application with an outpost asked for, starting, running or
    /// waiting out a backoff.
    #[serde(with = "crate::event_queue::pairs")]
    pub(crate) applications: BTreeMap<Pid, Application>,
    /// The newest listener the owner thread reported started: frames of
    /// any other are dropped.
    pub(crate) listener: Option<u64>,
    /// Verbatim's own process, whose outpost is never retired.
    pub(crate) own: Option<Pid>,
    /// How many spare outposts wait, as the owner thread last reported:
    /// a fact for the test harness's readiness, which judges nothing.
    pub(crate) spares_ready: usize,
    /// When idle outposts are retired.
    pub(crate) retirement: Retirement,
    /// When the next retirement check runs, armed only while an outpost
    /// other than Verbatim's own is ready.
    pub(crate) sweep_due_us: Option<u64>,
}

impl Record {
    /// Core asks for `process`'s outpost: a record launching, with the
    /// failures counted from none.
    pub(crate) fn launch(&mut self, process: Pid, now_us: u64) {
        self.applications.insert(
            process,
            Application {
                stage: Stage::Launching,
                backoff: Backoff::default(),
                attended_us: now_us,
            },
        );
    }

    /// The generation of `process` that is starting or ready.
    pub(crate) fn running(&self, process: Pid) -> Option<Running> {
        self.applications.get(&process)?.running()
    }

    /// Whether `generation` is `process`'s current one, starting or ready.
    pub(crate) fn current(&self, process: Pid, generation: u64) -> bool {
        self.running(process)
            .is_some_and(|running| running.generation == generation)
    }

    /// `process`'s outpost failed at `now_us`, having been up for `up`:
    /// replaced at once on its first failure, else after its backoff.
    pub(crate) fn failed(&mut self, process: Pid, up: Duration, now_us: u64) -> Option<Replaced> {
        let application = self.applications.get_mut(&process)?;
        let wait = application.backoff.failed(up, now_us, BACKOFF);
        if wait.is_zero() {
            application.stage = Stage::Launching;
            Some(Replaced::AtOnce)
        } else {
            let due_us = now_us.saturating_add(u64::try_from(wait.as_micros()).unwrap_or(u64::MAX));
            application.stage = Stage::WaitingOut { due_us };
            Some(Replaced::Later)
        }
    }

    /// The earliest time the record waits for: a liveness or backoff due
    /// time, or the retirement check. A due time saturated at the clock's
    /// end never comes, and is no wake.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.applications
            .values()
            .filter_map(|application| match application.stage {
                Stage::Starting(running) | Stage::Ready(running) => Some(running.liveness_due()),
                Stage::WaitingOut { due_us } => Some(due_us),
                Stage::Launching => None,
            })
            .chain(self.sweep_due_us)
            .filter(|due_us| *due_us != u64::MAX)
            .min()
    }

    /// Arms the retirement check from `now_us` while an outpost other
    /// than Verbatim's own is ready whose application, `attended` not
    /// being it, does not hold attention, and disarms it otherwise, so
    /// that no wake is timed that would not be acted on.
    pub(crate) fn settle_sweep(&mut self, now_us: u64, attended: Option<Pid>) {
        let retirable = self.applications.iter().any(|(process, application)| {
            Some(*process) != self.own
                && Some(*process) != attended
                && matches!(application.stage, Stage::Ready(_))
        });
        match (retirable, self.sweep_due_us) {
            (true, None) => {
                self.sweep_due_us = Some(now_us.saturating_add(self.retirement.sweep_us));
            }
            (false, Some(_)) => self.sweep_due_us = None,
            (true, Some(_)) | (false, None) => {}
        }
    }
}
