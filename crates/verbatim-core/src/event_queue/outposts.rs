//! The outpost generation record applied by the event queue's driver
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation record"): the owner
//! thread's process facts become inputs here, and the record's
//! transitions move the requests that wait for an application's outpost
//! or are out to it. Core owns routing and judges a generation's
//! liveness; the owner thread carries out the effects, starting an outpost
//! or ending a generation. An outpost is never ended for being idle
//! (Dickson, 2026-11-10 and 2026-11-16): it ends when its application
//! exits, when it fails or stops answering pings, or when Verbatim exits.

use std::time::Duration;

use verbatim_model::Effect;
use verbatim_model::Pid;
use verbatim_model::request::{Outcome as WireOutcome, ProcessFact, QueueEffect};

use super::driver::Core;
use super::{Instance, Logic, Reader};
use crate::supervisor::record::{ABANDONED_WORKER_LIMIT, Application, Replaced, Running, Stage};

impl<L: Logic> Core<L> {
    /// Applies a process fact the owner thread reported.
    pub(super) fn process_fact(&mut self, fact: ProcessFact) {
        match fact {
            ProcessFact::OutpostStarted {
                process,
                generation,
            } => self.outpost_started(process, generation),
            ProcessFact::LaunchFailed {
                process,
                application_gone,
            } => self.launch_failed(process, application_gone),
            ProcessFact::Pong {
                instance,
                abandoned,
            } => self.ponged(instance, abandoned),
            ProcessFact::ApplicationExited(process) => self.application_exited(process),
            ProcessFact::ListenerStarted(generation) => {
                self.shared.borrow_mut().state.queue.record.listener = Some(generation);
            }
            ProcessFact::SparesReady(waiting) => {
                self.shared.borrow_mut().state.queue.record.spares_ready = waiting;
            }
            ProcessFact::OwnProcess(process) => self.own_process(process),
        }
    }

    /// The owner thread started `process`'s outpost as generation
    /// `generation` (column B): a launching record starts, its liveness
    /// armed; any other report is an older generation's, dropped.
    fn outpost_started(&mut self, process: Pid, generation: u64) {
        let mut shared = self.shared.borrow_mut();
        let now_us = shared.now_us();
        let record = &mut shared.state.queue.record;
        match record.applications.get_mut(&process) {
            Some(application) if application.stage == Stage::Launching => {
                application.stage = Stage::Starting(Running {
                    generation,
                    started_us: now_us,
                    pong_us: now_us,
                });
            }
            Some(_) | None => {
                shared
                    .effects
                    .push(Effect::Queue(QueueEffect::StaleFrame(Instance {
                        reader: Reader::Outpost(process),
                        generation,
                    })));
            }
        }
    }

    /// The owner thread could not start `process`'s outpost (column C):
    /// with the application gone it is as the application's exit; else
    /// the first failure starts again at once and a later one waits out
    /// its backoff, the requests waiting still.
    fn launch_failed(&mut self, process: Pid, gone: bool) {
        let launching = self
            .shared
            .borrow()
            .state
            .queue
            .record
            .applications
            .get(&process)
            .is_some_and(|application| application.stage == Stage::Launching);
        if !launching {
            return;
        }
        if gone {
            self.application_exited(process);
            return;
        }
        self.replace(process, Duration::ZERO);
    }

    /// `process`'s outpost failed after `up`: replaced at once on its
    /// first failure, else after its backoff.
    fn replace(&mut self, process: Pid, up: Duration) {
        let mut shared = self.shared.borrow_mut();
        let now_us = shared.now_us();
        if shared.state.queue.record.failed(process, up, now_us) == Some(Replaced::AtOnce) {
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StartOutpost(process)));
        }
    }

    /// Whether `instance`'s `Ready` is its reader's current one (column
    /// D): the newest listener the owner reported started, or an
    /// application's starting generation, which becomes ready. Any other
    /// is dropped and logged.
    pub(super) fn admit_ready(&mut self, instance: Instance) -> bool {
        let mut shared = self.shared.borrow_mut();
        let record = &mut shared.state.queue.record;
        let admitted = match instance.reader {
            Reader::Listener => record.listener == Some(instance.generation),
            Reader::Outpost(process) => match record.applications.get_mut(&process) {
                Some(application) => match application.stage {
                    Stage::Starting(running) if running.generation == instance.generation => {
                        application.stage = Stage::Ready(running);
                        true
                    }
                    _ => false,
                },
                None => false,
            },
        };
        if !admitted {
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StaleFrame(instance)));
        }
        admitted
    }

    /// `instance`'s pipe closed or its process exited (column E). The
    /// newest listener ends as today; an application's current generation
    /// ends: one still starting waits out its backoff, a ready one ends
    /// its requests and is replaced. Any other is an older generation's,
    /// dropped.
    pub(super) fn instance_ended(&mut self, instance: Instance) {
        let process = match instance.reader {
            Reader::Listener => {
                let newest = self.shared.borrow().state.queue.record.listener;
                if newest == Some(instance.generation) {
                    self.reader_ended(instance, true);
                } else {
                    self.stale(instance);
                }
                return;
            }
            Reader::Outpost(process) => process,
        };
        let (stage, now_us) = {
            let shared = self.shared.borrow();
            let queue = &shared.state.queue;
            let stage = queue
                .record
                .applications
                .get(&process)
                .map(|application| application.stage);
            (stage, shared.now_us())
        };
        match stage {
            Some(Stage::Starting(running)) if running.generation == instance.generation => {
                self.replace(process, up(running, now_us));
            }
            Some(Stage::Ready(running)) if running.generation == instance.generation => {
                self.reader_ended(instance, true);
                self.replace(process, up(running, now_us));
            }
            Some(_) | None => self.stale(instance),
        }
    }

    /// Logs a frame of `instance`, which is not its reader's current one.
    fn stale(&mut self, instance: Instance) {
        self.shared
            .borrow_mut()
            .effects
            .push(Effect::Queue(QueueEffect::StaleFrame(instance)));
    }

    /// `instance` answered a ping: its liveness is re-armed when it is its
    /// application's current generation.
    fn ponged(&mut self, instance: Instance, abandoned: usize) {
        let Reader::Outpost(process) = instance.reader else {
            return;
        };
        let current = {
            let mut shared = self.shared.borrow_mut();
            let now_us = shared.now_us();
            if let Some(application) = shared.state.queue.record.applications.get_mut(&process)
                && let Stage::Starting(running) | Stage::Ready(running) = &mut application.stage
                && running.generation == instance.generation
            {
                running.pong_us = now_us;
                true
            } else {
                false
            }
        };
        // The current generation that piled up workers abandoned to calls
        // that never returned, other than those parked on a window judged
        // hung, is ended as one that stopped answering is (column F).
        if current && abandoned >= ABANDONED_WORKER_LIMIT {
            self.shared
                .borrow_mut()
                .effects
                .push(Effect::Queue(QueueEffect::EndOutpost(instance)));
            self.instance_ended(instance);
        }
    }

    /// The application `process` exited (column J): every request waiting
    /// for its outpost or out to it is answered "not there", a starting
    /// generation is ended, a ready one's held ids are dropped with no
    /// re-take, and the record goes to none with no backoff.
    fn application_exited(&mut self, process: Pid) {
        let (application, ended) = {
            let mut shared = self.shared.borrow_mut();
            let queue = &mut shared.state.queue;
            let Some(application) = queue.record.applications.remove(&process) else {
                return;
            };
            let ended = queue
                .table
                .answer_all(Reader::Outpost(process), &WireOutcome::NotThere);
            (application, ended)
        };
        for ended in ended {
            self.deliver(ended);
        }
        match application.stage {
            Stage::Starting(running) => {
                self.shared
                    .borrow_mut()
                    .effects
                    .push(Effect::Queue(QueueEffect::EndOutpost(Instance {
                        reader: Reader::Outpost(process),
                        generation: running.generation,
                    })));
            }
            Stage::Ready(running) => self.reader_ended(
                Instance {
                    reader: Reader::Outpost(process),
                    generation: running.generation,
                },
                false,
            ),
            Stage::Launching | Stage::WaitingOut { .. } => {}
        }
    }

    /// Verbatim's own process: its outpost is started at once, as
    /// `ensure_spawned` did.
    fn own_process(&mut self, process: Pid) {
        let mut shared = self.shared.borrow_mut();
        let record = &mut shared.state.queue.record;
        record.own = Some(process);
        if !record.applications.contains_key(&process) {
            record.launch(process);
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StartOutpost(process)));
        }
    }

    /// The listener's foreground report names `process` (column I): with
    /// no record, its outpost is started as an attended application's;
    /// a backoff that runs ends at once.
    pub(super) fn attended(&mut self, process: Pid) {
        let mut shared = self.shared.borrow_mut();
        if !L::watched(&shared.state.facts, process) {
            return;
        }
        let record = &mut shared.state.queue.record;
        let Some(application) = record.applications.get_mut(&process) else {
            record.launch(process);
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StartOutpost(process)));
            return;
        };
        application.backoff.foreground();
        if let Stage::WaitingOut { .. } = application.stage {
            application.stage = Stage::Launching;
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StartOutpost(process)));
        }
    }

    /// The record's timers at `now_us`: a generation past its liveness
    /// due time is ended (column F); a backoff that ended launches again
    /// when a request waits or the application holds attention, else the
    /// record goes (column H).
    pub(super) fn outpost_timers(&mut self) {
        let (now_us, applications) = {
            let shared = self.shared.borrow();
            let queue = &shared.state.queue;
            let applications: Vec<(Pid, Application)> = queue
                .record
                .applications
                .iter()
                .map(|(process, application)| (*process, application.clone()))
                .collect();
            (shared.now_us(), applications)
        };
        for (process, application) in applications {
            match application.stage {
                Stage::Starting(running) | Stage::Ready(running)
                    if running.liveness_due() <= now_us =>
                {
                    let instance = Instance {
                        reader: Reader::Outpost(process),
                        generation: running.generation,
                    };
                    self.shared
                        .borrow_mut()
                        .effects
                        .push(Effect::Queue(QueueEffect::EndOutpost(instance)));
                    self.instance_ended(instance);
                }
                Stage::WaitingOut { due_us } if due_us <= now_us => {
                    self.backoff_ended(process);
                }
                Stage::Launching
                | Stage::Starting(_)
                | Stage::Ready(_)
                | Stage::WaitingOut { .. } => {}
            }
        }
    }

    /// `process`'s backoff ended (column H).
    fn backoff_ended(&mut self, process: Pid) {
        let mut shared = self.shared.borrow_mut();
        let queue = &mut shared.state.queue;
        let reader = Reader::Outpost(process);
        let wanted = queue.table.waiting_for(reader)
            || queue
                .foreground
                .is_some_and(|foreground| foreground.process == Some(process));
        if wanted {
            if let Some(application) = queue.record.applications.get_mut(&process) {
                application.stage = Stage::Launching;
            }
            shared
                .effects
                .push(Effect::Queue(QueueEffect::StartOutpost(process)));
        } else {
            queue.record.applications.remove(&process);
        }
    }

    /// The requests waiting for an application with no record ask for its
    /// outpost (column A, from none): it is started, or, for an
    /// application whose outpost Core never starts, the request is
    /// answered "not there". Returns whether an ending was delivered.
    pub(super) fn want_outposts(&mut self) -> bool {
        let wanted: Vec<Pid> = {
            let shared = self.shared.borrow();
            let queue = &shared.state.queue;
            queue
                .table
                .waiting_readers()
                .filter_map(|reader| match reader {
                    Reader::Outpost(process)
                        if !queue.record.applications.contains_key(&process) =>
                    {
                        Some(process)
                    }
                    Reader::Outpost(_) | Reader::Listener => None,
                })
                .collect()
        };
        let mut delivered = false;
        for process in wanted {
            let watched = {
                let shared = self.shared.borrow();
                L::watched(&shared.state.facts, process)
            };
            if watched {
                let mut shared = self.shared.borrow_mut();
                shared.state.queue.record.launch(process);
                shared
                    .effects
                    .push(Effect::Queue(QueueEffect::StartOutpost(process)));
                continue;
            }
            let ended = self
                .shared
                .borrow_mut()
                .state
                .queue
                .table
                .answer_all(Reader::Outpost(process), &WireOutcome::NotThere);
            for ended in ended {
                delivered = true;
                self.deliver(ended);
            }
        }
        delivered
    }
}

/// How long `running` was up at `now_us`.
fn up(running: Running, now_us: u64) -> Duration {
    Duration::from_micros(now_us.saturating_sub(running.started_us))
}

#[cfg(test)]
#[path = "outposts_tests.rs"]
mod tests;
