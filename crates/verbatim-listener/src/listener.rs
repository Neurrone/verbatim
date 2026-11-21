//! The focus-listener runtime (architecture section 1, "The focus listener
//! (D13)"; decisions D13 and D14; ADR 0003, "Readers tell, Core asks";
//! design `docs/design/readers-tell-core-asks-build1.md`, "The listener in
//! this build").
//!
//! One permanent listener process holds the subscriptions that are global
//! by nature and tells Core of every event it hears; Core judges each tell
//! and asks what it needs:
//!
//! - every `WinEvent` kind it hooks desktop-wide, but the console host's
//!   five, which the console host's outpost hooks, each told after NVDA's
//!   local drops, behind its foreground hold (`tells`);
//! - the desktop-wide UI Automation focus handler and the global group NVDA
//!   registers on Windows 11 (an element selected, a menu opened,
//!   notifications, windows opened), each event checked on its
//!   application's check thread with the calls NVDA's handler makes at
//!   delivery, then told, its element kept behind the tell's token
//!   (`checking`);
//! - Core's requests on what it kept, and on an MSAA object at an event's
//!   address, each answered once on the application's reading thread
//!   (`asked`, `crate::reading`), and Core's Cancel of them;
//! - a progress bar's range value change, from the global group, told with
//!   the percentage and place its event's cache gave (a classic bar's value
//!   change is a `WinEvent` like any other, which Core asks to read);
//! - the window that came to the foreground, for the keyboard hook's view
//!   ([`ListenerToCore::Foreground`]; `verbatim_listener_logic::foreground`).
//!
//! The listener's callbacks and its writer never make a cross-process call
//! (decision D13): a UI Automation callback delivers the element with its
//! properties cached, and every call into an application is made on a
//! check thread or a reading thread, one of each per application, so a
//! hung application holds only its own. One exposure is UIA's own and
//! cannot be contained here: a hung window named in a focus `WinEvent`
//! holds UIA's delivery of every focus event to the listener until it
//! answers, as it holds NVDA's (`books/dev/src/reference/uia-bugs.md`, "A
//! hung window named in a system focus event holds a process's UIA
//! events").
//!
//! Tells are merged with NVDA's limiter's rule before they are sent: one
//! waiting tell per object, kind and raising thread, a newer one replacing
//! it and moving to the back. Pongs, `Ready` and faults go ahead of
//! everything else; answers and reports go in the order they are made.
//!
//! The processes Verbatim ignores entirely (`IgnoredProcesses`, which the
//! end-to-end harness names) are held open here too; their `WinEvent`s are
//! told, and Core drops a focus of one at its answer (a test-harness
//! rule).
//!
//! Run as `verbatim-listener.exe --listener --pipe-in <handle> --pipe-out
//! <handle> --generation <n> [--ignore-pids <pid,pid>]` — its own
//! executable, with no
//! target pid, supervised by the same machinery as the outposts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    NotificationKind, NotificationProcessing, UIA_MenuOpenedEventId, UIA_ProgressBarControlTypeId,
    UIA_RangeValueValuePropertyId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_Window_WindowOpenedEventId,
};

use verbatim_ia2::{LISTENER_SUBSCRIPTIONS, WinEvent, WinEventKind};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{Answer, Ask, Outcome, Question, Refusal};
use verbatim_model::tell::{Event, Payload};
use verbatim_model::{Percent, Pid, StateSet};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{Backlog, ChildIo, SendError, Sender, Served, Serving};
use verbatim_uia::map::{
    CachedUiaParts, ignores_notifications, is_toast_candidate, notification_kind_from_uia,
    notification_processing_from_uia,
};
use verbatim_uia::served_only_by_uia_proxies;
use verbatim_uia::{
    Cached, ListenerEvent, ListenerProgress, ListenerSelection, Part, Registration, Scope,
    Subscription,
};

use crate::reading::Readers;

pub(crate) mod asked;
mod checking;
mod tells;

use asked::{UiaJob, UiaReading};
pub use checking::{CheckEvent, KeptElements, Told, cached_of, check_event};
use checking::{Checks, Kept};

pub use tells::{Applications, EventReads, Known, Pending, PendingWindow, TellKey, msaa_tell};
use tells::{Held, SystemReads, Tells, tell_win_event};
use verbatim_ia2::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger, ThreadTimer};
use verbatim_listener_logic::foreground::{ForegroundWatch, Front, Seen, Step, TimerChange};
use verbatim_listener_logic::kept::{Reported, View};
use verbatim_listener_logic::schedule::JobKind;
use verbatim_model::protocol::{CoreToListener, ListenerToCore, UiaSnapshotFact, hop_us};
use verbatim_model::tell::ElementToken;
use verbatim_process::clock::now_us;
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::message::{decode_message, put_message};
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::{WindowReadabilityReader, window_class_name};

/// The priority of the listener's main thread, its I/O thread, which reads
/// Core's pipe, handles each command, and finishes the writes to Core that
/// pended, for microseconds per wake (`docs/design/communication.md`,
/// section 6.1).
const IO_PRIORITY: ThreadPriority = ThreadPriority::Highest;

/// The listener's messages to Core: written by the thread that produces
/// each one, through the transport's [`Sender`], and held in [`Waiting`]
/// only while a write is in flight, which the I/O thread then finishes
/// (`docs/design/communication.md`, sections 4.3 and 6.3).
struct Outgoing {
    sender: Sender<Waiting>,
    /// The processes Verbatim ignores, held open for the listener's life
    /// so that their ids name the processes Core was told of.
    _ignored: IgnoredProcesses,
    /// Whether the desktop-wide UIA subscriptions have been installed, or
    /// their installation has failed: until then every UIA event is dropped
    /// ([`Self::uia_event_process`]).
    uia_installed: AtomicBool,
    /// Whether the listener has said `Ready`, and until then the idle
    /// barrier's answers held back for it ([`Self::idle_answer`]).
    readiness: Mutex<Readiness>,
    /// The clock the stamps are read from.
    clock: Clock,
    /// The listener's order of tells and its pending foreground.
    tells: Arc<Tells>,
}

/// Whether the listener has said `Ready`: an idle barrier's answer made
/// before then is held until `Ready` has been sent, so a barrier that
/// starts while the listener starts, as after a crash, passes only once
/// its UIA events are heard (`docs/design/communication.md`, section 6.7).
enum Readiness {
    /// Not yet ready, with the rounds whose answers wait, in the order
    /// they were made.
    Starting(Vec<u64>),
    /// `Ready` has been sent: answers go at once.
    Ready,
}

/// The clock the listener's stamps are read from: the shared clock,
/// [`now_us`], except in tests, which fix it.
#[derive(Clone, Copy)]
struct Clock(fn() -> u64);

impl Default for Clock {
    fn default() -> Self {
        Self(now_us)
    }
}

/// What waits while a write to Core is in flight: urgent messages first,
/// then tells, merged one per object, kind and raising thread, with the
/// answers and reports in the order they were made. The backlog of the
/// listener's [`Sender`], which every message passes through.
#[derive(Default)]
struct Waiting {
    urgent: VecDeque<ListenerToCore>,
    /// Tells, with the key each is merged by, answers and reports, and the
    /// idle barrier's answers, in order.
    queued: VecDeque<(Option<TellKey>, ListenerToCore)>,
    /// The numbering of the messages carrying tokens, in the order they
    /// are written, as Core counts them, and where each token's tell was
    /// written, which judges Core's held views
    /// ([`verbatim_listener_logic::kept::Reported`]).
    reported: Reported,
}

/// One item the listener sends Core.
enum Sent {
    /// A message that goes ahead of everything waiting: a pong, `Ready`,
    /// or a fault.
    Urgent(ListenerToCore),
    /// A tell, with the key it is merged under.
    Tell(TellKey, ListenerToCore),
    /// An answer or a report, never merged.
    Report(ListenerToCore),
    /// The idle barrier's answer for a round, behind everything waiting
    /// ([`Outgoing::idle_answer`]).
    IdleAnswer(u64),
    /// A message taken from what waits, to be written.
    Message(ListenerToCore),
}

impl Backlog for Waiting {
    type Item = Sent;
    type Refused = Infallible;

    fn push(&mut self, item: Sent) -> Result<(), Infallible> {
        match item {
            Sent::Urgent(message) | Sent::Message(message) => self.urgent.push_back(message),
            Sent::Tell(key, message) => {
                if let Some(token) = message.carried_token() {
                    self.reported.queued(token);
                }
                self.merged(key, message);
            }
            Sent::Report(message) => self.queued.push_back((None, message)),
            Sent::IdleAnswer(round) => self
                .queued
                .push_back((None, ListenerToCore::IdleAnswer { round })),
        }
        Ok(())
    }

    fn pop(&mut self) -> Option<Sent> {
        self.urgent
            .pop_front()
            .or_else(|| self.queued.pop_front().map(|(_, message)| message))
            .map(Sent::Message)
    }

    fn is_empty(&self) -> bool {
        self.urgent.is_empty() && self.queued.is_empty()
    }

    fn encode(&mut self, item: Sent, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut message = match item {
            Sent::Urgent(message)
            | Sent::Message(message)
            | Sent::Tell(_, message)
            | Sent::Report(message) => message,
            Sent::IdleAnswer(round) => ListenerToCore::IdleAnswer { round },
        };
        // The write's stamp, for the latency ledger's split of the
        // listener's time from the pipe's, taken as the write begins,
        // whichever thread writes.
        message.written(now_us());
        put_message(buffer, &message)?;
        // Numbered once it is encoded, in pipe order: a message that could
        // not be encoded is skipped, and Core never counts it.
        if let Some(token) = message.carried_token() {
            self.reported.written(true, [token]);
        }
        Ok(())
    }
}

impl Waiting {
    /// Queues the tell `message` behind everything waiting, replacing any
    /// waiting tell merged by the same `key`, but never across the idle
    /// barrier's answer, which the one replaced would move behind.
    fn merged(&mut self, key: TellKey, message: ListenerToCore) {
        let after_answer = self
            .queued
            .iter()
            .rposition(|(_, waiting)| matches!(waiting, ListenerToCore::IdleAnswer { .. }))
            .map_or(0, |answer| answer + 1);
        let mut index = after_answer;
        while index < self.queued.len() {
            if self.queued[index].0.as_ref() == Some(&key) {
                if let Some((_, replaced)) = self.queued.remove(index)
                    && let Some(token) = replaced.carried_token()
                {
                    self.reported.withdrawn(token);
                }
            } else {
                index += 1;
            }
        }
        self.queued.push_back((Some(key), message));
    }
}

/// What reaches Core of `tells`, placed in order while a write to Core is
/// in flight: the writer's merge leaves one per object, kind and raising
/// thread, the newest at the back ([`Waiting::merged`]). Public so that
/// mockapp's tests tell Core of a burst as the listener's pipe brings it.
#[must_use]
pub fn written_together(tells: Vec<(TellKey, Event)>) -> Vec<Event> {
    let mut waiting = Waiting::default();
    for (key, tell) in tells {
        let _ = waiting.push(Sent::Tell(key, ListenerToCore::Tell(Box::new(tell))));
    }
    std::iter::from_fn(|| waiting.pop())
        .filter_map(|sent| match sent {
            Sent::Message(ListenerToCore::Tell(tell)) => Some(*tell),
            _ => None,
        })
        .collect()
}

/// A send's outcome: once the listener is shutting down, or Core's pipe
/// has gone, what is sent is dropped.
fn sent(sent: &Result<(), SendError<Infallible>>) {
    if sent.is_err() {
        tracing::debug!("a message to Core is dropped: the pipe is closed");
    }
}

/// The report to Core of the window that came to the foreground, `front`,
/// with its root owner, the process holding attention and that process's
/// integrity level, read here with local calls (`GetAncestor`,
/// `GetWindowThreadProcessId`, `GetProcessIdOfThread` for a console
/// window whose thread its event named, and the token's integrity). Sent
/// for every process, ignored ones included, since Core takes from it the
/// keyboard hook's view, attention and its foreground record; none
/// replaces another.
fn foreground_report(front: Front) -> ListenerToCore {
    let owner = window_pid(front.window);
    let process = if front.console && front.thread != 0 {
        process_of_thread(front.thread).unwrap_or(owner)
    } else {
        owner
    };
    ListenerToCore::Foreground {
        window: front.window,
        root_owner: verbatim_win32::window::root_owner_of(front.window),
        process,
        observed_at_us: front.observed_at_us,
        thread: front.thread,
        console: front.console,
        integrity: verbatim_win32::integrity::process_integrity(owner),
    }
}

impl Outgoing {
    /// Sends through `sender`, dropping the UI Automation events of the
    /// processes `ignored` names.
    fn new(sender: Sender<Waiting>, ignored: IgnoredProcesses) -> Self {
        let tells = Arc::new(Tells::new(sender.clone()));
        Self::with_tells(sender, ignored, tells)
    }

    fn with_tells(sender: Sender<Waiting>, ignored: IgnoredProcesses, tells: Arc<Tells>) -> Self {
        Self {
            sender,
            _ignored: ignored,
            uia_installed: AtomicBool::new(false),
            readiness: Mutex::new(Readiness::Starting(Vec::new())),
            clock: Clock::default(),
            tells,
        }
    }

    fn urgent(&self, message: ListenerToCore) {
        sent(&self.sender.send(Sent::Urgent(message)));
    }

    /// Runs `inspect` on the writer's record of the tells it numbered,
    /// under the writer's lock, which is taken after the kept elements'.
    fn with_reported<R>(&self, inspect: impl FnOnce(&mut Reported) -> R) -> R {
        self.sender
            .with_backlog(|waiting| inspect(&mut waiting.reported))
    }

    /// Sends Core the answer to one of its requests, in the order answers
    /// are made, never merged.
    fn answer(&self, answer: Answer) {
        sent(
            &self
                .sender
                .send(Sent::Report(ListenerToCore::Answer(answer))),
        );
    }

    /// Sends Core the window that came to the foreground, `front`, as the
    /// watch's start or a console's own event found it
    /// ([`foreground_report`]).
    fn foreground(&self, front: Front) {
        sent(&self.sender.send(Sent::Report(foreground_report(front))));
    }

    /// Marks the desktop-wide UIA subscriptions as installed, or as failed:
    /// UIA events are taken from then on.
    fn uia_installation_ended(&self) {
        self.uia_installed.store(true, Ordering::Release);
    }

    /// Whether the desktop-wide UIA subscriptions have been installed, or
    /// their installation has failed: until then every UIA event is
    /// dropped.
    fn uia_installed(&self) -> bool {
        self.uia_installed.load(Ordering::Acquire)
    }

    /// Answers the idle barrier's round `round`, behind every tell placed
    /// before it, the ones the foreground hold keeps included, and every
    /// answer and report sent before it; before the listener is ready,
    /// held until it has said `Ready` ([`Self::ready`]).
    fn idle_answer(&self, round: u64) {
        let mut readiness = self
            .readiness
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match &mut *readiness {
            Readiness::Starting(held) => held.push(round),
            Readiness::Ready => self.tells.place_behind(Held::IdleAnswer(round)),
        }
    }

    /// Says the listener is `Ready`, then answers every idle barrier's
    /// round held for it, in order, behind `Ready` and every tell placed
    /// before. Under the readiness lock, so no answer made meanwhile goes
    /// ahead of them.
    fn ready(&self) {
        let mut readiness = self
            .readiness
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.urgent(ListenerToCore::Ready {
            outpost_pid: Pid(std::process::id()),
        });
        if let Readiness::Starting(held) = std::mem::replace(&mut *readiness, Readiness::Ready) {
            for round in held {
                self.tells.place_behind(Held::IdleAnswer(round));
            }
        }
    }

    /// Now, on the clock the listener's stamps are read from.
    fn now(&self) -> u64 {
        (self.clock.0)()
    }

    fn fault(&self, detail: String) {
        self.urgent(ListenerToCore::Fault { detail });
    }

    /// Takes nothing more, and has what waits written and the pipe closed,
    /// for the listener's shutdown.
    fn close(&self) {
        self.sender.close();
    }
}

/// What makes the listener's desktop-wide UIA subscriptions, on the
/// thread [`Listener::start`] starts for it, returning them, or `None`
/// when their installation failed.
type InstallSubscriptions = Box<
    dyn FnOnce(&Arc<Outgoing>, &Arc<Readers<UiaReading>>, &Arc<Checks>) -> Option<Registration>
        + Send,
>;

/// The focus listener: owns its subscriptions for the whole life of the
/// process.
struct Listener {
    outgoing: Arc<Outgoing>,
    /// Every application's reading thread, which answers Core's requests.
    readers: Arc<Readers<UiaReading>>,
    /// The elements kept for tells (Table 6).
    kept: Arc<Kept>,
    /// Every application's check thread, which checks and tells its UI
    /// Automation events.
    checks: Arc<Checks>,
    /// The thread making the desktop-wide UIA registration, which returns
    /// it, or `None` when it failed ([`Listener::start`]).
    registering: JoinHandle<Option<Registration>>,
    /// What nudges the `WinEvent` thread: the idle barrier's query goes
    /// through it, behind every event delivered to it before.
    nudger: Nudger,
    _event_thread: EventThread,
}

impl Listener {
    /// Sets up the listener, sending through `sender`, with its
    /// desktop-wide UIA subscriptions ([`install_desktop_subscriptions`]):
    /// see [`Self::start`].
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    fn new(
        sender: Sender<Waiting>,
        generation: u64,
        ignored: IgnoredProcesses,
    ) -> io::Result<Self> {
        Self::start(
            sender,
            (generation, ignored),
            Box::new(|outgoing, readers, checks| {
                install_desktop_subscriptions(
                    (outgoing, readers, checks),
                    focus_subscription(outgoing, readers, checks),
                    menu_subscription(outgoing, readers, checks),
                )
            }),
        )
    }

    /// Sets up the listener, sending through `sender`: installs its
    /// `WinEvent` hooks, and then has `install` make the desktop-wide UIA
    /// subscriptions on a thread of their own, which announces readiness
    /// once `install` returns, so Core knows the `WinEvent`s and the UIA
    /// events are heard from then on; Verbatim takes input only once the
    /// listener is ready. The hooks go in first, and the installation runs
    /// on its own thread, so the pipe thread answers the supervisor's pings
    /// meanwhile, because UIA reads the system's focus for the focus
    /// registration and waits on the foreground application for it, up to
    /// about 6 seconds for one that does not answer
    /// (`docs/design/cross-process-calls-study.md`, instance 2). Their
    /// failure is reported as a fault, before the announcement. Every UIA
    /// event is dropped until `install` returns
    /// ([`Outgoing::uia_event_process`]).
    ///
    /// # Errors
    ///
    /// Returns the error of starting the thread that installs the UIA
    /// subscriptions.
    fn start(
        sender: Sender<Waiting>,
        (generation, ignored): (u64, IgnoredProcesses),
        install: InstallSubscriptions,
    ) -> io::Result<Self> {
        let outgoing = Arc::new(Outgoing::new(sender, ignored));
        Self::with_outgoing(outgoing, generation, install)
    }

    /// [`start`](Self::start) sending through `outgoing`.
    ///
    /// # Errors
    ///
    /// Returns the error of starting the thread that installs the UIA
    /// subscriptions.
    fn with_outgoing(
        outgoing: Arc<Outgoing>,
        generation: u64,
        install: InstallSubscriptions,
    ) -> io::Result<Self> {
        let kept = Arc::new(Kept::new(generation));
        let readers = Arc::new(Readers::new(UiaReading::new(&outgoing, &kept)));
        let checks = Arc::new(Checks::new(&outgoing, &readers, &kept));
        let idle_checks = Arc::clone(&checks);
        let msaa_outgoing = Arc::clone(&outgoing);
        let idle_readers = Arc::clone(&readers);
        let make_handlers: MakeHandlers = Arc::new(move || {
            let outgoing = Arc::clone(&msaa_outgoing);
            let idle_outgoing = Arc::clone(&msaa_outgoing);
            let readers = Arc::clone(&idle_readers);
            let checks = Arc::clone(&idle_checks);
            let known = Arc::clone(&idle_checks);
            // The window in front, for the keyboard hook's view of it, and
            // the foreground hold's timer, kept on this thread by the
            // handlers.
            let thread = Rc::new(EventThreadState::default());
            let start_thread = Rc::clone(&thread);
            let timer_thread = Rc::clone(&thread);
            let timer_outgoing = Arc::clone(&msaa_outgoing);
            EventHandlers {
                win_event: Box::new(move |event: WinEvent, class: Option<&str>| {
                    on_win_event(&outgoing, &*known, &thread, (event, class), &SystemReads);
                }),
                // The idle barrier's query, its round in the first word:
                // every event delivered before it has been handled, so the
                // reading threads are waited for next. Or, once, the start
                // of the foreground's watch, once the hooks are in.
                nudge: Some(Box::new(move |round, kind| {
                    match Nudge::of((round, kind)) {
                        Nudge::ForegroundStart => {
                            let step = start_thread
                                .watch
                                .borrow_mut()
                                .start(front_now(idle_outgoing.now()));
                            foreground_step(&idle_outgoing, step);
                        }
                        Nudge::IdleRound(round) => {
                            answer_when_reads_are_done(
                                &readers,
                                &checks,
                                &idle_outgoing,
                                round as u64,
                            );
                        }
                    }
                })),
                // The foreground hold's timer, or a stale `WM_TIMER`.
                timer: Some(Box::new(move || {
                    let started = timer_thread.watch.borrow().started();
                    let front = timer_outgoing.tells.front();
                    let stepped =
                        timer_outgoing
                            .tells
                            .step(Seen::Timer, (front, started), &SystemReads);
                    stepped_on(&timer_outgoing, &timer_thread, stepped);
                })),
            }
        });
        let event_thread = EventThread::spawn(LISTENER_SUBSCRIPTIONS, make_handlers);
        if let Err(error) = event_thread.hook(0) {
            tracing::warn!(error, "failed to install WinEvent hooks");
        }
        let nudger = event_thread.nudger();
        // The window in front now, reported from the event thread behind the
        // hooks' install, so a foreground event raised since is reported
        // after it.
        nudger.nudge(Nudge::ForegroundStart.words());
        let registering = install_on_its_thread(&outgoing, &readers, &checks, install)?;
        Ok(Self {
            outgoing,
            readers,
            kept,
            checks,
            registering,
            nudger,
            _event_thread: event_thread,
        })
    }

    /// Shuts the listener down cleanly ([`CoreToListener::Shutdown`]),
    /// returning once it is done: the `WinEvent` hooks are removed with the
    /// thread that pumped them, the desktop-wide UIA subscriptions'
    /// installation is waited for if it has not ended, and the handlers
    /// removed (the registering thread waits for a callback in progress);
    /// the check and reading threads end, one inside a call let go; what
    /// waits to be written to Core is written, the I/O thread `io` serves
    /// waiting for every write in flight to complete, and the pipe closed.
    fn shutdown(self, io: &mut ChildIo) {
        let started = Instant::now();
        let Self {
            outgoing,
            readers,
            kept: _,
            checks,
            registering,
            nudger: _,
            _event_thread: event_thread,
        } = self;
        drop(event_thread);
        let registration = registering
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let registration_failed_reads = registration.as_ref().map_or(0, Registration::failed_reads);
        drop(registration);
        checks.stop();
        let reading_threads = readers.stop();
        let failed_reads = registration_failed_reads.saturating_add(readers.failed_reads());
        outgoing.close();
        io.finish(&outgoing.sender);
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            reading_threads,
            failed_reads,
            "the focus listener shut down"
        );
    }

    /// Handles one of Core's commands. `received_at_us` is when it was read
    /// off Core's pipe: the hop from verbatim.exe's write is logged at trace
    /// level, under `verbatim::stage`.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn handle_command(&self, command: CoreToListener, received_at_us: u64) {
        if let Some(us) = hop_us(command.sent_at_us(), received_at_us) {
            tracing::trace!(target: "verbatim::stage", stage = "verbatim.exe to the listener", us, ?command);
        }
        match command {
            CoreToListener::Ping { .. } => {
                self.outgoing
                    .urgent(ListenerToCore::Pong { written_at_us: 0 });
            }
            // The listener keeps no unasked read to read again: every
            // `WinEvent` is told, and Core asks what it needs. The serving
            // loop ends on `Shutdown`, before it is handled here.
            CoreToListener::Shutdown => {}
            CoreToListener::ApplicationExited { pid } => {
                self.checks.application_exited(pid);
                self.readers.application_exited(pid);
                self.outgoing.urgent(ListenerToCore::ExitHandled { pid });
            }
            CoreToListener::IdleQuery { round } => self.idle_query(round),
            CoreToListener::Request(ask) => self.request(ask),
            CoreToListener::Cancel(cancel) => {
                let abandons = cancel.reason.abandons_call();
                let number = cancel.number;
                self.readers.cancel(cancel);
                // A read abandoned for the foreground lets its kept
                // reference go; its thread releases its own as its call
                // returns.
                if abandons
                    && let Some((pid, element)) = self.kept.read_abandoned(&self.outgoing, number.0)
                {
                    self.checks.release(pid, vec![element]);
                }
            }
            CoreToListener::ElementsHeld {
                generation,
                tokens,
                acknowledged,
            } => elements_held(
                (&self.kept, &self.checks, &self.outgoing),
                generation,
                (tokens, acknowledged),
            ),
        }
    }

    /// Core's request `ask` ([`ask`]).
    fn request(&self, ask: Box<Ask>) {
        self::ask((&self.readers, &self.kept), ask);
    }

    /// The idle barrier's query for `round` (`docs/design/communication.md`,
    /// section 6.7): handed to the `WinEvent` thread behind every event
    /// delivered to it, then to the reading threads behind every job queued
    /// for them, then to the check threads behind every event handed to
    /// them, and answered behind every tell placed before. A `WinEvent`
    /// thread that has ended has nothing in progress.
    fn idle_query(&self, round: u64) {
        let posted = usize::try_from(round)
            .is_ok_and(|round| self.nudger.nudge(Nudge::IdleRound(round).words()));
        if !posted {
            answer_when_reads_are_done(&self.readers, &self.checks, &self.outgoing, round);
        }
    }
}

/// Core's held view of the listener `generation`, `tokens` held with
/// `acknowledged` the position of the last message carrying tokens Core
/// handled: dropped when it names another generation, its tokens naming
/// nothing here; else every kept element it rules out is released on its
/// application's reading thread ([`Kept::view`]).
fn elements_held(
    (kept, checks, outgoing): (&Kept, &Arc<Checks>, &Outgoing),
    generation: u64,
    (tokens, acknowledged): (Vec<ElementToken>, u64),
) {
    if !kept.is_own(generation) {
        tracing::debug!(
            generation,
            "a held view of another listener generation is dropped"
        );
        return;
    }
    let view = View {
        tokens: tokens.into_iter().collect(),
        acknowledged,
    };
    for (pid, released) in kept.view(outgoing, view) {
        checks.release(pid, released);
    }
}

/// Core's request `ask`: queued for the reading thread of its object's
/// application, or answered at once when the listener holds no such
/// object, the token naming another listener generation, or cannot read
/// it.
fn ask((readers, kept): (&Readers<UiaReading>, &Kept), ask: Box<Ask>) {
    let answer = |outcome: Outcome| {
        readers.reading().outgoing.answer(Answer {
            number: ask.number,
            outcome,
            probes: Vec::new(),
            detail: String::new(),
        });
    };
    // A request is routed by the object it reads, a batch's first step's
    // subject, which every later step's kept element shares
    // (`asked::batch_object`); a terminal's text operation is the
    // outpost's.
    let object = match &ask.request {
        Question::Batch(batch) => asked::batch_object(batch),
        Question::Terminal(_) => None,
    };
    let Some(object) = object else {
        answer(Outcome::Refused(Refusal::Unsupported));
        return;
    };
    let pid = match object {
        asked::Routed::Kept(listener, token) => {
            kept.is_own(listener).then(|| kept.owner(token)).flatten()
        }
        asked::Routed::Msaa(window) => {
            // The window's own thread stands for the thread that raised
            // its events, which a read does not carry.
            let window = isize::from(window);
            let (thread, owner) = verbatim_win32::window::window_owner(window);
            tells::routed_process(&window_class_name(window), owner, || {
                (thread != 0).then(|| process_of_thread(thread)).flatten()
            })
            .map(Pid)
        }
    };
    let Some(pid) = pid else {
        answer(Outcome::NotThere);
        return;
    };
    let number = ask.number;
    readers.push(pid, JobKind::Request(number), UiaJob::Request(ask));
}

/// What the `WinEvent` thread keeps of the foreground: the watch of the
/// window in front, and the foreground hold's timer.
#[derive(Default)]
struct EventThreadState {
    watch: RefCell<ForegroundWatch>,
    timer: ThreadTimer,
}

/// What the listener does with one `WinEvent`: its tell, unless a local
/// drop takes it ([`tell_win_event`]), steps the pending foreground under
/// the tells' lock, which reports a window it finds in front before any
/// tell that step releases, since Core judges a foreground tell against
/// its foreground record (`docs/design/readers-tell-core-asks-build1.md`,
/// "Focus and foreground"); then the watch hears of the window reported,
/// and of a console's own event, and the hold's timer is set as the step
/// says. The window in front is read once
/// ([`Tells::front`](tells::Tells::front)), and the step judges the event
/// on that read; every other read of Windows' state goes through `reads`,
/// and the class of the event's window not at all when the hook's filter
/// read it, `class`. The event's arrival is the time the callback received
/// it.
fn on_win_event(
    outgoing: &Arc<Outgoing>,
    known: &dyn Known,
    thread: &EventThreadState,
    (event, class): (WinEvent, Option<&str>),
    reads: &impl EventReads,
) {
    let arrived_us = outgoing.now();
    let front = outgoing.tells.front();
    let seen = tell_win_event(known, event, (arrived_us, class), reads);
    let started = thread.watch.borrow().started();
    let stepped = outgoing.tells.step(seen, (front, started), reads);
    stepped_on(outgoing, thread, stepped);
    if matches!(
        event.kind,
        WinEventKind::ConsoleUpdate | WinEventKind::ConsoleCaret
    ) {
        let step = thread
            .watch
            .borrow_mut()
            .console_event(event.hwnd, event.thread, arrived_us);
        foreground_step(outgoing, step);
    }
}

/// What follows a step of the pending foreground on the `WinEvent` thread,
/// `reported` the window it reported and `timer` its timer's change: the
/// watch hears of the window, which ends any wait for a console's thread
/// and unhooks the consoles' events, and the timer is set or stopped.
fn stepped_on(
    outgoing: &Outgoing,
    thread: &EventThreadState,
    (reported, timer): (Option<Front>, TimerChange),
) {
    if reported.is_some() {
        let console_events = thread.watch.borrow_mut().sent();
        foreground_step(
            outgoing,
            Step {
                report: None,
                console_events,
            },
        );
    }
    match timer {
        TimerChange::Keep => {}
        TimerChange::Arm(after) => thread.timer.arm(Duration::from_micros(after)),
        TimerChange::Disarm => thread.timer.disarm(),
    }
}

/// Starts the thread that makes the listener's desktop-wide UIA
/// subscriptions with `install`, takes UIA events from then on
/// ([`Outgoing::uia_installation_ended`]), logs the time it took, says
/// the listener is `Ready`, whether or not they were installed, which
/// lets the idle barrier's answers held until then go
/// ([`Outgoing::ready`]), and returns them.
///
/// # Errors
///
/// Returns the error of starting the thread.
fn install_on_its_thread(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
    install: InstallSubscriptions,
) -> io::Result<JoinHandle<Option<Registration>>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    std::thread::Builder::new()
        .name("listener UIA subscriptions".to_owned())
        .spawn(move || {
            let started = Instant::now();
            let registration = install(&outgoing, &readers, &checks);
            outgoing.uia_installation_ended();
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                installed = registration.is_some(),
                "the listener's desktop-wide UIA subscriptions ended their installation"
            );
            outgoing.ready();
            registration
        })
}

/// Answers the idle barrier's round `round` once no reading thread has a
/// job waiting or running, behind the answer of every request queued
/// before ([`Readers::when_idle`]), then once no check thread has an event
/// waiting or being checked, behind the tell of every one handed to it
/// before ([`Checks::when_idle`]), behind every tell placed before it.
fn answer_when_reads_are_done(
    readers: &Readers<UiaReading>,
    checks: &Arc<Checks>,
    outgoing: &Arc<Outgoing>,
    round: u64,
) {
    let outgoing = Arc::clone(outgoing);
    let checks = Arc::clone(checks);
    readers.when_idle(Box::new(move || {
        checks.when_idle(Box::new(move || outgoing.idle_answer(round)));
    }));
}

/// What a UIA callback captures from an event's element, `sender`, read
/// as `fields`: its cached window handle and snapshot parts. `None` for an
/// element with no owning process. Public so that mockapp's tests make a
/// notification's fact from mockapp's own event as the listener makes it.
#[must_use]
pub fn capture(
    sender: &Cached<ListenerEvent>,
    fields: &ListenerEvent,
) -> Option<(isize, UiaSnapshotFact)> {
    fields.process_id.filter(|&pid| pid != 0)?;
    Some((
        fields.role.hwnd,
        snapshot_fact(fields.parts(sender.runtime_id())),
    ))
}

/// What the selection callback captures from the selected item, `sender`,
/// read as `fields`: as [`capture`], with the item's value, states, and
/// details from its event's cache ([`ListenerSelection`]), which Core
/// speaks of an item selected inside a focused list or tab control. Public
/// so that mockapp's tests make a selection's fact from mockapp's own
/// event as the listener makes it.
#[must_use]
pub fn capture_selection(
    sender: &Cached<ListenerSelection>,
    fields: &ListenerSelection,
) -> Option<(isize, UiaSnapshotFact)> {
    fields.event.process_id.filter(|&pid| pid != 0)?;
    Some((
        fields.event.role.hwnd,
        snapshot_fact(fields.parts(sender.runtime_id())),
    ))
}

/// A fact's snapshot of an element's cached parts.
fn snapshot_fact(parts: CachedUiaParts) -> UiaSnapshotFact {
    UiaSnapshotFact {
        runtime_id: parts.runtime_id,
        role: parts.role,
        name: parts.name,
        value: parts.value,
        states: parts.states,
        details: parts.details,
    }
}

/// The desktop-wide UIA focus subscription. Its callback makes no
/// cross-process call: it hands the event to its application's check
/// thread ([`arrived`]), which tells Core of it.
fn focus_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    let callback = Arc::new(move |sender: &Cached<ListenerEvent>| {
        let fields = sender.read();
        let told = (EventKind::UiaFocus, None, Payload::None);
        arrived(&outgoing, &readers, &checks, (sender, &fields), told);
    });
    Subscription::Focus { callback }
}

/// The desktop-wide subscription to UIA menu openings, each handed to its
/// application's check thread as a focus is: Core judges it, as NVDA's
/// automation event handler maps it to a focus on the menu.
fn menu_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_MenuOpenedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            let told = (EventKind::UiaMenuOpened, None, Payload::None);
            arrived(&outgoing, &readers, &checks, (sender, &fields), told);
        }),
    }
}

/// A UI Automation event of `kind` whose sender is `sender`, read as
/// `fields`, arrived: with `states` and `payload`, what its kind carries
/// beyond its cached properties. Dropped as NVDA's handlers drop it before
/// any call: before the subscriptions are installed, with no process, or
/// with its sender's cached window hung or
/// ghosted as it arrives (a sender with no cached window is not tested,
/// `UIAHandler/utils.py` 446 to 450); a hung window is reported to Core
/// once per stall of its thread, a ghost as the window it stands for,
/// behind any pending foreground. Otherwise its window's thread is
/// unmarked and it is handed to its application's check thread, which
/// tells Core of it. Makes no cross-process call.
fn arrived(
    outgoing: &Outgoing,
    readers: &Readers<UiaReading>,
    checks: &Arc<Checks>,
    (sender, fields): (&Cached<ListenerEvent>, &ListenerEvent),
    (kind, states, payload): (EventKind, Option<StateSet>, Payload),
) {
    let at_us = outgoing.now();
    if !outgoing.uia_installed() {
        tracing::debug!(
            ?kind,
            "UIA event dropped: received before the listener's UIA subscriptions were installed"
        );
        return;
    }
    let Some(pid) = fields.process_id.filter(|&pid| pid != 0).map(Pid) else {
        return;
    };
    let hwnd = fields.role.hwnd;
    if window_hung(&readers.reading().readability, hwnd) {
        tracing::debug!(%pid, ?kind, hwnd, "UIA event dropped as it arrived: its window is hung");
        let reported = verbatim_win32::window::hung_window_from_ghost(hwnd).unwrap_or(hwnd);
        let report = verbatim_win32::window_facts::hung(&SystemReads, reported, None, at_us);
        if let Some(report) = checks.hung(report) {
            outgoing.tells.place_behind(Held::Hung(report));
        }
        return;
    }
    if let Some(event) = CheckEvent::new(kind, (sender, fields), pid, at_us, (states, payload)) {
        let answered = if hwnd == 0 {
            0
        } else {
            verbatim_win32::window::window_owner(hwnd).0
        };
        checks.hand(pid, answered, event);
    }
}

/// Whether `window`, an event's sender's cached window, is hung or
/// ghosted, by the listener's one readability reader; a sender with no
/// window, 0, is not tested. Local calls.
fn window_hung(readability: &WindowReadabilityReader, window: isize) -> bool {
    window != 0 && readability(window).is_hung()
}

/// Installs the desktop-wide UIA subscriptions, some of those NVDA
/// registers globally on Windows 11: `focus` ([`focus_subscription`]) on
/// the registration's client, then an element selected, `menu`
/// ([`menu_subscription`]), notifications, windows opened and range
/// values, all on one client as NVDA's global handlers are
/// (`verbatim_uia::Subscription::Focus` says why they must share it), each
/// part shaped as `verbatim_uia::ListenerEvent`: NVDA's base cache
/// request, with the two properties that refine a role for dialogs and
/// toggle buttons; an element selected also carries the item's value,
/// states, and details (`verbatim_uia::ListenerSelection`). A failure is
/// reported as a fault and leaves the `WinEvent`s running on their own.
fn install_desktop_subscriptions(
    (outgoing, readers, checks): (&Arc<Outgoing>, &Arc<Readers<UiaReading>>, &Arc<Checks>),
    focus: Subscription<Cached<ListenerEvent>>,
    menu: Subscription<Cached<ListenerEvent>>,
) -> Option<Registration> {
    let selection = selection_subscription(outgoing, readers, checks);
    let notifications = notification_subscription(outgoing, readers, checks);
    let toasts = toast_subscription(outgoing, readers, checks);
    let progress = progress_subscription(outgoing, readers, checks);
    let parts = vec![
        Part::shaped(focus),
        Part::shaped::<ListenerSelection>(selection),
        Part::shaped(menu),
        Part::shaped(notifications),
        Part::shaped(toasts),
        Part::shaped::<ListenerProgress>(progress),
    ];
    match Registration::with_parts(parts, Scope::Desktop) {
        Ok(registration) => Some(registration),
        Err(error) => {
            outgoing.fault(format!(
                "listener desktop-wide UIA subscriptions failed: {error}"
            ));
            None
        }
    }
}

/// The desktop-wide subscription to UIA element selected events, each
/// handed to its application's check thread with the item's states from
/// its cache ([`arrived`]).
fn selection_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerSelection>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_SelectionItem_ElementSelectedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerSelection>| {
            let fields = sender.read();
            let Some((_, snapshot)) = capture_selection(sender, &fields) else {
                return;
            };
            let narrowed = sender.narrow();
            let told = (
                EventKind::UiaElementSelected,
                Some(snapshot.states),
                Payload::None,
            );
            arrived(
                &outgoing,
                &readers,
                &checks,
                (&narrowed, &fields.event),
                told,
            );
        }),
    }
}

/// The desktop-wide subscription to UIA notifications, each handed to its
/// application's check thread with its text, kind, processing and
/// activity ([`arrived`]), but a terminal text area's, which Core's rule
/// would drop, dropped here on its cached class and automation id.
fn notification_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Notifications {
        callback: Arc::new(
            move |sender: &Cached<ListenerEvent>,
                  kind: NotificationKind,
                  processing: NotificationProcessing,
                  display_string: Option<String>,
                  activity_id: Option<String>| {
                let fields = sender.read();
                if ignores_notifications(
                    fields.role.class_name.as_deref(),
                    fields.automation_id.as_deref(),
                ) {
                    return;
                }
                let told = (
                    EventKind::UiaNotification,
                    None,
                    Payload::Notification {
                        text: display_string.unwrap_or_default(),
                        kind: notification_kind_from_uia(kind),
                        processing: notification_processing_from_uia(processing),
                        activity: activity_id.unwrap_or_default(),
                    },
                );
                arrived(&outgoing, &readers, &checks, (sender, &fields), told);
            },
        ),
    }
}

/// The desktop-wide subscription to UIA window openings, kept only for a
/// candidate for a Windows toast by what the event's cache answers of
/// NVDA's rule ([`is_toast_candidate`]), which Core's rule would otherwise
/// drop; a candidate is handed to its application's check thread
/// ([`arrived`]). It runs once per window opened anywhere and makes no
/// call.
fn toast_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerEvent>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Event {
        event: UIA_Window_WindowOpenedEventId,
        callback: Arc::new(move |sender: &Cached<ListenerEvent>| {
            let fields = sender.read();
            if !is_toast_candidate(fields.role.control_type, fields.automation_id.as_deref()) {
                tracing::debug!(
                    control_type = ?fields.role.control_type,
                    automation_id = ?fields.automation_id,
                    "a window opened is no toast"
                );
                return;
            }
            let told = (EventKind::UiaWindowOpened, None, Payload::None);
            arrived(&outgoing, &readers, &checks, (sender, &fields), told);
        }),
    }
}

/// The desktop-wide subscription to UIA range value changes, the listener's
/// path for every UIA progress bar, foreground or background. Its callback
/// makes no cross-process call: an element whose cached control type is not
/// a progress bar, whose provider is UIA's proxy for a classic window
/// (heard by the hook instead), that is off the screen, or whose range is
/// not a number is dropped, the percentage and place parsed once here from
/// the event's cache ([`bar_of`]). The bar is then handed to its
/// application's check thread as every UIA event is ([`arrived`]), which
/// finds its nearest window, reads the window's acceptance facts and probes
/// it, and tells Core, which judges it at its turn.
fn progress_subscription(
    outgoing: &Arc<Outgoing>,
    readers: &Arc<Readers<UiaReading>>,
    checks: &Arc<Checks>,
) -> Subscription<Cached<ListenerProgress>> {
    let outgoing = Arc::clone(outgoing);
    let readers = Arc::clone(readers);
    let checks = Arc::clone(checks);
    Subscription::Properties {
        properties: vec![UIA_RangeValueValuePropertyId],
        callback: Arc::new(move |sender: &Cached<ListenerProgress>, _| {
            let fields = sender.read();
            let Some((percent, place)) = bar_of(&fields) else {
                return;
            };
            let narrowed = sender.narrow();
            let told = (
                EventKind::UiaProgress,
                None,
                Payload::Progress { percent, place },
            );
            arrived(
                &outgoing,
                &readers,
                &checks,
                (&narrowed, &fields.event),
                told,
            );
        }),
    }
}

/// What the callback of [`progress_subscription`] makes of `fields`, an
/// event's cache: the percentage and the centre of its rectangle, (0, 0)
/// when it reports none, as NVDA places a bar whose location it cannot read
/// (`nvda/source/NVDAObjects/behaviors.py` 62 to 67), when the element is
/// an on-screen progress bar with a provider of its own and a range that
/// is a number; `None` for anything else, a slider's range value among
/// them. Public so that mockapp's tests judge mockapp's own event as the
/// listener judges it.
#[must_use]
pub fn bar_of(fields: &ListenerProgress) -> Option<(Percent, (i32, i32))> {
    progress_of(
        (
            fields.event.role.control_type,
            fields.event.provider_description.as_deref(),
        ),
        (fields.value, fields.minimum, fields.maximum),
        (fields.place.rect, fields.place.offscreen),
    )
}

/// [`bar_of`], from what a callback reads of an event's cache: its control
/// type and provider, its range, and its rectangle and whether it is off
/// the screen.
fn progress_of(
    (control_type, provider): (Option<i32>, Option<&str>),
    (value, minimum, maximum): (Option<f64>, Option<f64>, Option<f64>),
    (rect, offscreen): (Option<verbatim_model::Rect>, bool),
) -> Option<(Percent, (i32, i32))> {
    if control_type != Some(UIA_ProgressBarControlTypeId.0)
        || provider.is_some_and(served_only_by_uia_proxies)
        || offscreen
    {
        return None;
    }
    let percent = Percent::of_range(value?, minimum?, maximum?)?;
    let place = rect.map_or((0, 0), |rect| {
        (
            rect.left.saturating_add(rect.width / 2),
            rect.top.saturating_add(rect.height / 2),
        )
    });
    Some((percent, place))
}

/// What a nudge of the `WinEvent` thread asks, in its two words: the idle
/// barrier's query for a round, the round in the first word and 0 in the
/// second, or the start of the watch of the window in front
/// ([`ForegroundWatch::start`]), 1 in the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nudge {
    /// The idle barrier's query for this round.
    IdleRound(usize),
    /// The start of the foreground's watch, once the hooks are in.
    ForegroundStart,
}

impl Nudge {
    /// The second word of the foreground's start.
    const FOREGROUND_START: isize = 1;

    /// The nudge two words carry.
    fn of((round, kind): (usize, isize)) -> Self {
        if kind == Self::FOREGROUND_START {
            Self::ForegroundStart
        } else {
            Self::IdleRound(round)
        }
    }

    /// Its two words.
    fn words(self) -> (usize, isize) {
        match self {
            Self::ForegroundStart => (0, Self::FOREGROUND_START),
            Self::IdleRound(round) => (round, 0),
        }
    }
}

/// The system's foreground window now, read locally; 0 for none.
fn front_window() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: a local read with no preconditions.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// The window in the foreground now, with the thread Windows names as its
/// owner, read locally and observed at `observed_at_us`; `None` when no
/// window is in front.
fn front_now(observed_at_us: u64) -> Option<Front> {
    let window = front_window();
    if window == 0 {
        return None;
    }
    Some(Front {
        window,
        thread: verbatim_win32::window::window_owner(window).0,
        console: window_class_name(window) == CONSOLE_WINDOW_CLASS,
        observed_at_us,
    })
}

/// Carries out a step of the foreground's watch, on the `WinEvent` thread:
/// sends Core the window that came to the foreground, with its root owner,
/// its process and that process's integrity level read locally
/// ([`ListenerToCore::Foreground`]), and hooks or unhooks every console's
/// events.
fn foreground_step(outgoing: &Outgoing, step: Step) {
    if let Some(front) = step.report {
        outgoing.foreground(front);
    }
    if let Some(hooked) = step.console_events {
        let suspended: &[WinEventKind] = if hooked {
            &[]
        } else {
            &[WinEventKind::ConsoleUpdate, WinEventKind::ConsoleCaret]
        };
        if let Err(error) = verbatim_ia2::suspend_kinds(suspended) {
            tracing::warn!(
                error,
                hooked,
                "the consoles' events could not be hooked or unhooked"
            );
        }
    }
}

/// The owning process id of `hwnd`, or 0 for an invalid or ownerless window.
fn window_pid(hwnd: isize) -> u32 {
    verbatim_win32::window::window_owner(hwnd).1
}

/// The process `thread` belongs to, read with `GetProcessIdOfThread`, a
/// local call; `None` when the thread cannot be opened, such as one that
/// has ended.
fn process_of_thread(thread: u32) -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenThread has no preconditions; the handle is owned here and
    // closed below.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) }.ok()?;
    // SAFETY: `handle` is an open thread handle with the access the call
    // needs.
    let pid = unsafe { GetProcessIdOfThread(handle) };
    // SAFETY: `handle` was opened above and is closed once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    (pid != 0).then_some(pid)
}

/// Runs the focus listener on the transport's child side
/// (`docs/design/communication.md`, sections 4.6 and 6.3): the calling
/// thread becomes its I/O thread, which reads Core's commands from
/// `pipe_in` and finishes the writes to `pipe_out` that pend, while every
/// thread that produces a message writes it itself. Returns once Core asks
/// it to shut down or its command pipe ends, after a clean shutdown that
/// waits for every write in flight. Both ends must be overlapped.
/// `generation` is the listener's, which Core's views and requests name,
/// and `ignored` names the processes none of whose UI Automation events is
/// told.
///
/// # Errors
///
/// Returns the error of setting up the pipes or the listener, or any I/O
/// error reading the command stream, after the shutdown.
pub fn run_listener(
    pipe_in: OwnedHandle,
    pipe_out: OwnedHandle,
    generation: u64,
    ignored: IgnoredProcesses,
) -> io::Result<()> {
    let (mut io, sender) = ChildIo::new(pipe_in, pipe_out, Waiting::default())?;
    let listener = Listener::new(sender.clone(), generation, ignored)?;
    // This thread is the listener's I/O thread from here on.
    prioritize_role("listener I/O", IO_PRIORITY);
    let served = io.serve(&sender, |body| {
        // The command's receipt, one clock read, for the hop from
        // verbatim.exe's write.
        let received_at_us = now_us();
        match decode_message::<CoreToListener>(body)? {
            CoreToListener::Shutdown => return Ok(Serving::Stop),
            command => listener.handle_command(command, received_at_us),
        }
        Ok(Serving::Continue)
    });
    let ended = match served {
        Ok(Served::Stopped) => {
            tracing::info!("shutting down, as Core asked");
            Ok(())
        }
        Ok(Served::CommandsEnded) => {
            tracing::warn!("Core's command pipe closed; shutting down");
            Ok(())
        }
        Err(error) => Err(error),
    };
    listener.shutdown(&mut io);
    // The process has finished with UIA: only its exit gives back the
    // process-wide hold ([`verbatim_uia::release_mta_usage`]).
    verbatim_uia::release_mta_usage();
    ended
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32};
    use std::sync::mpsc;
    use std::sync::{Mutex, PoisonError};

    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::EVENT_PIPE_BUFFER;

    use super::*;
    use verbatim_model::WindowHandle;
    use verbatim_model::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
    use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, RuntimeId};
    use verbatim_model::request::{NodeRead, Reply, RequestNumber};
    use verbatim_model::tell::{ElementToken, EventObject, TellWindow};
    use verbatim_process::message::read_message;
    use verbatim_win32::window::Readability;
    use verbatim_win32::window_facts::WindowReads;

    /// An `Outgoing` writing to a pipe with a buffer of `buffer` bytes, its
    /// writes that pend finished by a thread of the sender's own, as the
    /// I/O thread finishes them, telling nothing of the processes
    /// `ignored` holds; and the pipe's other end, which the test reads.
    fn outgoing_with(buffer: u32, ignored: IgnoredProcesses) -> (Outgoing, BufReader<File>) {
        let (ours, theirs) = pipe(Flow::Outbound, buffer, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        (
            Outgoing::new(sender, ignored),
            BufReader::new(File::from(theirs)),
        )
    }

    fn outgoing() -> (Outgoing, BufReader<File>) {
        outgoing_with(EVENT_PIPE_BUFFER, IgnoredProcesses::default())
    }

    /// Everything `outgoing` sent, which is closed first, as Core reads it.
    fn sent(outgoing: &Outgoing, reader: &mut BufReader<File>) -> Vec<ListenerToCore> {
        outgoing.close();
        std::iter::from_fn(|| read_message(reader).expect("read")).collect()
    }

    /// Reads the messages `reader` brings on another thread, since a read
    /// blocks while nothing has been written.
    fn drain(mut reader: BufReader<File>) -> mpsc::Receiver<ListenerToCore> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut reader) {
                if tx.send(message).is_err() {
                    return;
                }
            }
        });
        rx
    }

    #[test]
    fn a_threads_process_is_read_from_the_thread() {
        // SAFETY: GetCurrentThreadId has no preconditions.
        let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        assert_eq!(process_of_thread(thread), Some(std::process::id()));
    }

    /// The desktop window, which the desktop's root element has as its
    /// own window.
    fn desktop_window() -> isize {
        // SAFETY: GetDesktopWindow has no preconditions.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize
    }

    /// The desktop window as the model names it.
    fn desktop_handle() -> WindowHandle {
        WindowHandle(u64::try_from(desktop_window()).expect("a handle"))
    }

    /// The desktop's root element as the listener's registration delivers
    /// an event's sender of shape `S`: with its own window, the desktop
    /// window, and its process cached.
    fn desktop_sender<S: verbatim_uia::Shape + verbatim_uia::Ungated>(
        uia: &verbatim_uia::Uia,
    ) -> Cached<S> {
        let request = uia
            .request_fixed::<S>()
            .expect("the listener's cache request");
        uia.any_window_element::<S>(desktop_window(), &request)
            .expect("the desktop's root element")
    }

    /// A provider that is UIA's proxy for a classic window is dropped, so
    /// a classic bar proxied through UIA is heard once, from the hook; a
    /// slider by its control type; a bar off the screen; and a range that
    /// is no number. A bar's place is the centre of its rectangle, (0, 0)
    /// with none.
    #[test]
    fn the_callback_keeps_only_a_native_numeric_progress_bar_on_screen() {
        const PROXIED: &str = "Main:Microsoft: MSAA Proxy (unmanaged:uiautomationcore.dll)";
        const NATIVE: &str = "Main:App provider (unmanaged:app.dll)";
        let bar = Some(UIA_ProgressBarControlTypeId.0);
        let rect = Some(verbatim_model::Rect {
            left: 10,
            top: 20,
            width: 100,
            height: 10,
        });
        let of = |control_type, provider, value, place| {
            progress_of(
                (control_type, Some(provider)),
                (value, Some(0.0), Some(1000.0)),
                place,
            )
        };
        let fifty = Percent::parse("50").unwrap();
        assert_eq!(
            of(bar, NATIVE, Some(500.0), (rect, false)),
            Some((fifty, (60, 25)))
        );
        assert_eq!(
            of(bar, NATIVE, Some(500.0), (None, false)),
            Some((fifty, (0, 0)))
        );
        assert_eq!(
            of(bar, PROXIED, Some(500.0), (rect, false)),
            None,
            "a proxied classic bar"
        );
        assert_eq!(
            of(Some(0), NATIVE, Some(500.0), (rect, false)),
            None,
            "a slider"
        );
        assert_eq!(
            of(bar, NATIVE, Some(500.0), (rect, true)),
            None,
            "off the screen"
        );
        assert_eq!(
            of(bar, NATIVE, None, (rect, false)),
            None,
            "a non-numeric value"
        );
    }

    /// Whether `readers` hold no job: the idle barrier's waiter runs at
    /// once, under their lock.
    fn nothing_queued(readers: &Readers<UiaReading>) -> bool {
        let ran = Arc::new(AtomicBool::new(false));
        let set = Arc::clone(&ran);
        readers.when_idle(Box::new(move || set.store(true, Ordering::SeqCst)));
        ran.load(Ordering::SeqCst)
    }

    /// The listener's sending side, taking UIA events, its kept elements,
    /// readers and check threads, and what it sends, read on a thread of
    /// its own.
    struct Listening {
        outgoing: Arc<Outgoing>,
        kept: Arc<Kept>,
        readers: Arc<Readers<UiaReading>>,
        checks: Arc<Checks>,
        sent: mpsc::Receiver<ListenerToCore>,
    }

    impl Listening {
        /// Stops the check and reading threads and closes the sending
        /// side: every message sent from now on, in order.
        fn stop(self) -> Vec<ListenerToCore> {
            self.checks.stop();
            self.readers.stop();
            self.outgoing.close();
            self.sent.iter().collect()
        }

        /// The next message sent within 20 seconds.
        fn next(&self) -> ListenerToCore {
            self.sent
                .recv_timeout(Duration::from_secs(20))
                .expect("a message")
        }
    }

    /// [`Listening`], whose readability reader answers the `n`th window it
    /// is asked, counted from 0, as `answer` says, each window asked kept in
    /// `asked`, and whose MSAA reads are the system's.
    fn listening(answer: fn(usize) -> Readability, asked: &Arc<Mutex<Vec<isize>>>) -> Listening {
        listening_reading(answer, asked, verbatim_ia2::acquire::read_at_event)
    }

    /// [`listening`] with the MSAA reads `read_at_event`.
    fn listening_reading(
        answer: fn(usize) -> Readability,
        asked: &Arc<Mutex<Vec<isize>>>,
        read_at_event: asked::MsaaReads,
    ) -> Listening {
        let (outgoing, reader) = outgoing();
        outgoing.uia_installation_ended();
        let asked = Arc::clone(asked);
        let readability: WindowReadabilityReader = Arc::new(move |window| {
            let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
            asked.push(window);
            answer(asked.len() - 1)
        });
        let outgoing = Arc::new(outgoing);
        let kept = Arc::new(Kept::new(1));
        let readers = Arc::new(Readers::new(UiaReading {
            outgoing: Arc::clone(&outgoing),
            kept: Arc::clone(&kept),
            readability,
            read_at_event,
        }));
        let checks = Arc::new(Checks::new(&outgoing, &readers, &kept));
        Listening {
            outgoing,
            kept,
            readers,
            checks,
            sent: drain(reader),
        }
    }

    /// Core's one-step read of `subject` by `set`, the request `number`,
    /// naming `window`.
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the reading threads take boxed asks"
    )]
    fn read_of(
        number: u64,
        window: WindowHandle,
        (subject, set): (Subject, Vec<PropertyId>),
    ) -> Box<Ask> {
        Box::new(Ask {
            number: RequestNumber(number),
            window,
            verdict: verbatim_model::request::WindowVerdict::Uia,
            request: Batch {
                steps: vec![Step {
                    op: Op::Read { subject, set },
                    when: None,
                }],
                placement: Placement::Queue,
                spare: None,
            }
            .into(),
        })
    }

    /// The MSAA reads the generic read test injects: at the desktop
    /// window's client, a progress bar named "Desktop 2" at 40 percent, on
    /// screen, each property of the set asked answered; nothing at any
    /// other address.
    fn bar_at_the_desktop(
        hwnd: isize,
        object: i32,
        child: i32,
        set: &[PropertyId],
    ) -> Option<verbatim_model::property::PropertyValues> {
        let at_the_bar = hwnd == desktop_window()
            && object == windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0
            && child == 0;
        at_the_bar.then(|| {
            verbatim_model::property::PropertyValues(
                set.iter()
                    .map(|&id| PropertyAnswer {
                        id,
                        value: match id {
                            PropertyId::Name => Some(PropertyValue::Text("Desktop 2".to_owned())),
                            PropertyId::Role => {
                                Some(PropertyValue::Role(verbatim_model::Role::ProgressBar))
                            }
                            PropertyId::Value => Some(PropertyValue::Text("40%".to_owned())),
                            PropertyId::States => Some(PropertyValue::States(StateSet::new())),
                            PropertyId::Rect => Some(PropertyValue::Rect(verbatim_model::Rect {
                                left: 0,
                                top: 0,
                                width: 10,
                                height: 10,
                            })),
                            _ => None,
                        },
                    })
                    .collect(),
            )
        })
    }

    /// A batch reading an MSAA object at its own address runs on the
    /// reading thread of its window's process, each step by the set it
    /// asks of the five the listener reads, an address with nothing there
    /// answering its step not there; a batch asking a property beyond the
    /// five, or an operation the listener does not run, is refused whole
    /// with none of it run.
    #[test]
    fn a_batch_reads_an_msaa_object_at_its_address_and_one_it_cannot_run_is_refused_whole() {
        use verbatim_model::batch::{
            Batch, Done, Malformed, Missing, Op, OpKind, Place, Placement, Step, StepOutcome,
            Subject,
        };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening_reading(|_| Readability::Readable, &asked, bar_at_the_desktop);
        let client = windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0;
        let window = windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW.0;
        let at = |object| Subject::Msaa {
            ids: verbatim_model::request::MsaaIds {
                window: desktop_handle(),
                object,
                child: 0,
            },
            redirect: false,
        };
        let read = |object, set: &[PropertyId]| Step {
            op: Op::Read {
                subject: at(object),
                set: set.to_vec(),
            },
            when: None,
        };
        let batch = |number, steps| {
            ask(
                (&parts.readers, &parts.kept),
                Box::new(Ask {
                    number: RequestNumber(number),
                    window: desktop_handle(),
                    verdict: verbatim_model::request::WindowVerdict::Msaa,
                    request: Batch {
                        steps,
                        placement: Placement::Queue,
                        spare: None,
                    }
                    .into(),
                }),
            );
        };
        let outcome = |message| match message {
            ListenerToCore::Answer(answer) => answer.outcome,
            other => panic!("an answer: {other:?}"),
        };
        let bar = [PropertyId::Role, PropertyId::Value];
        batch(
            1,
            vec![
                read(client, &[PropertyId::Name]),
                read(client, &bar),
                read(window, &[PropertyId::Name]),
            ],
        );
        let read_back = |set: &[PropertyId]| {
            StepOutcome::Done(Done::Read(NodeRead {
                node: None,
                window: verbatim_win32::window_facts::node_window(
                    &tells::SystemReads,
                    desktop_window(),
                ),
                values: bar_at_the_desktop(desktop_window(), client, 0, set)
                    .expect("the bar's values"),
            }))
        };
        assert_eq!(
            outcome(parts.next()),
            Outcome::Result(Reply::Steps(vec![
                read_back(&[PropertyId::Name]),
                read_back(&bar),
                StepOutcome::NotThere(Missing::Object),
            ]))
        );
        let activate = Step {
            op: Op::Activate {
                subject: at(client),
            },
            when: None,
        };
        let description = read(client, &[PropertyId::Description]);
        for (number, steps, (step, op)) in [
            (2, vec![read(client, &[]), description], (1, OpKind::Read)),
            (3, vec![activate], (0, OpKind::Activate)),
        ] {
            batch(number, steps);
            assert_eq!(
                outcome(parts.next()),
                Outcome::Refused(Refusal::Malformed(Malformed::NotRun {
                    step: Place(step),
                    op,
                }))
            );
        }
        assert_eq!(parts.stop(), []);
    }

    /// A request on an element the listener does not keep, or a node it
    /// does not hold, is answered at once.
    #[test]
    fn a_request_on_nothing_the_listener_holds_is_answered_at_once() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let read = |subject| (subject, vec![PropertyId::Name]);
        ask(
            (&parts.readers, &parts.kept),
            read_of(
                1,
                desktop_handle(),
                read(Subject::Kept {
                    listener: 1,
                    token: ElementToken(9),
                    keep: false,
                }),
            ),
        );
        ask(
            (&parts.readers, &parts.kept),
            read_of(
                2,
                desktop_handle(),
                read(Subject::Node(verbatim_model::NodeId::new(1))),
            ),
        );
        let outcomes: Vec<(u64, Outcome)> = parts
            .stop()
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => (answer.number.0, answer.outcome),
                other => panic!("an answer: {other:?}"),
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                (1, Outcome::NotThere),
                (2, Outcome::Refused(Refusal::Unsupported))
            ]
        );
    }

    /// The one report `sent` holds, of the desktop window, the senders'
    /// cached window in these tests, with the facets Windows gives it:
    /// its own, and no top-level window or root owner, which the desktop
    /// window has none of. Its time is the listener's clock's at arrival.
    fn only_the_desktop_reported(sent: &[ListenerToCore]) {
        let [ListenerToCore::Hung(hung)] = sent else {
            panic!("one report of a hung window: {sent:?}");
        };
        let (thread, process) = verbatim_win32::window::window_owner(desktop_window());
        let none = verbatim_model::tell::WindowFacet {
            handle: WindowHandle(0),
            class: String::new(),
            process: Pid(0),
            thread: 0,
        };
        assert_eq!(
            (&hung.window, &hung.root, &hung.root_owner),
            (
                &verbatim_model::tell::WindowFacet {
                    handle: desktop_handle(),
                    class: "#32769".to_owned(),
                    process: Pid(process),
                    thread,
                },
                &none,
                &none
            )
        );
    }

    /// A UIA focus whose sender's cached window the listener's reader
    /// answers hung as it arrives is dropped in the callback: nothing is
    /// told, the window is reported hung, and the reader was asked of the
    /// sender's window once.
    #[test]
    fn a_focus_whose_window_is_hung_as_it_arrives_is_dropped_in_the_callback() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        assert!(
            sender.read().process_id.is_some_and(|pid| pid != 0),
            "the sender has a process, so only its window can drop it"
        );
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        only_the_desktop_reported(&parts.stop());
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()]
        );
    }

    /// A UIA focus whose sender's window hangs after it arrives is checked
    /// and told with no second test of its window by the listener's
    /// readability reader: asked once, as the focus arrived.
    #[test]
    fn a_focus_whose_window_hangs_after_it_arrives_is_told() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(
            |nth| {
                if nth == 0 {
                    Readability::Readable
                } else {
                    Readability::Hung
                }
            },
            &asked,
        );
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let ListenerToCore::Tell(tell) = parts.next() else {
            panic!("a tell");
        };
        assert_eq!(tell.kind, EventKind::UiaFocus);
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window()],
            "asked as it arrived, and not by its check"
        );
    }

    /// The next tell `parts` sends.
    fn next_tell(parts: &Listening) -> Box<verbatim_model::tell::Event> {
        match parts.next() {
            ListenerToCore::Tell(tell) => tell,
            other => panic!("the listener sent {other:?}, not a tell"),
        }
    }

    /// A UIA focus is checked on its application's check thread and told
    /// with what the check found: the walk to its nearest window, here the
    /// desktop's own, and the probe of that window, which is on neither of
    /// NVDA's lists and has no provider, so the check makes no other call,
    /// as NVDA's handler returns at a window read through MSAA; its element
    /// is kept behind the token the tell names. A second focus on the same
    /// element finds the probe's answer in the thread's record and makes no
    /// call past the walk.
    #[test]
    fn a_focus_is_told_from_its_check_thread_with_what_its_check_found() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        let first = next_tell(&parts);
        callback(&sender);
        let second = next_tell(&parts);
        drop(callback);
        let runtime_id = RuntimeId(sender.runtime_id());
        let desktop = desktop_handle();
        let (desktop_thread, _) = verbatim_win32::window::window_owner(desktop_window());
        assert_eq!(first.kind, EventKind::UiaFocus);
        assert_eq!(
            first.object,
            EventObject::Uia {
                token: ElementToken(1),
                runtime_id: runtime_id.clone(),
            }
        );
        assert_eq!(first.thread, desktop_thread);
        let TellWindow::Found(facts) = &first.window else {
            panic!("the desktop window: {:?}", first.window);
        };
        assert_eq!(facts.window.handle, desktop);
        assert!(facts.desktop);
        let checked = first.checked.as_ref().expect("checked");
        let probe = checked.probe.expect("the desktop window probed");
        assert_eq!(
            (probe.window, probe.thread, probe.provider),
            (desktop, desktop_thread, false)
        );
        // A window the probe finds read through MSAA still has its
        // keyboard-focus reads made: Core judges the verdict.
        assert_eq!(
            checked.keyboard_focus,
            Some(verbatim_model::tell::Read::Answered(false))
        );
        assert_eq!(checked.kept, Vec::new());
        assert_eq!(checked.previous_sibling, None);
        assert_eq!(
            second.object,
            EventObject::Uia {
                token: ElementToken(2),
                runtime_id,
            }
        );
        assert_eq!(
            second.checked,
            Some(verbatim_model::tell::Checked {
                probe: None,
                keyboard_focus: Some(verbatim_model::tell::Read::Answered(false)),
                kept: vec![(ElementToken(1), verbatim_model::tell::Read::Answered(false))],
                previous_sibling: None,
            })
        );
        parts.stop();
    }

    /// A held view and a request naming another listener generation, as
    /// one written for the listener before it crashed reaches its
    /// replacement: the view is dropped and releases nothing, and the
    /// request is answered not there, the token naming nothing here. The
    /// listener's own view, its tell handled, lets the element go.
    #[test]
    fn a_view_and_a_request_of_another_generation_name_nothing_here() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let tell = next_tell(&parts);
        let EventObject::Uia { token, .. } = tell.object else {
            panic!("a kept element: {:?}", tell.object);
        };
        let pid = Pid(sender.read().process_id.expect("a process"));
        elements_held(
            (&parts.kept, &parts.checks, &parts.outgoing),
            2,
            (Vec::new(), 40),
        );
        assert_eq!(parts.kept.owner(token), Some(pid));
        let read = |listener| {
            (
                Subject::Kept {
                    listener,
                    token,
                    keep: false,
                },
                vec![PropertyId::Name],
            )
        };
        ask(
            (&parts.readers, &parts.kept),
            read_of(1, desktop_handle(), read(2)),
        );
        let ListenerToCore::Answer(answer) = parts.next() else {
            panic!("an answer");
        };
        assert_eq!(
            (answer.number, answer.outcome),
            (RequestNumber(1), Outcome::NotThere)
        );
        elements_held(
            (&parts.kept, &parts.checks, &parts.outgoing),
            1,
            (Vec::new(), 1),
        );
        assert_eq!(parts.kept.owner(token), None);
        let stopped = parts.stop();
        assert_eq!(stopped, Vec::new());
    }

    /// Core's read of a kept element, here the desktop's root element a
    /// focus event named, is answered on its application's reading thread
    /// with its properties by the set, from the same element the event
    /// carried; a read that is not a focus read lets the element go, so a
    /// second read of its token is answered not there.
    #[test]
    fn a_kept_element_is_read_once_for_cores_request() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let sender = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Readable, &asked);
        let Subscription::Focus { callback } =
            focus_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the focus subscription");
        };
        callback(&sender);
        drop(callback);
        let tell = next_tell(&parts);
        let EventObject::Uia { token, .. } = tell.object else {
            panic!("a kept element: {:?}", tell.object);
        };
        let set = vec![
            PropertyId::Name,
            PropertyId::NativeWindow,
            PropertyId::Process,
            PropertyId::RuntimeId,
        ];
        let read = (
            Subject::Kept {
                listener: 1,
                token,
                keep: false,
            },
            set.clone(),
        );
        for number in [1, 2] {
            ask(
                (&parts.readers, &parts.kept),
                read_of(number, desktop_handle(), read.clone()),
            );
        }
        let fields = sender.read();
        let pid = Pid(fields.process_id.expect("a process"));
        let expected = verbatim_model::property::PropertyValues(vec![
            PropertyAnswer {
                id: PropertyId::Name,
                value: fields
                    .parts(sender.runtime_id())
                    .name
                    .map(PropertyValue::Text),
            },
            PropertyAnswer {
                id: PropertyId::NativeWindow,
                value: Some(PropertyValue::Window(desktop_handle())),
            },
            PropertyAnswer {
                id: PropertyId::Process,
                value: Some(PropertyValue::Process(pid)),
            },
            PropertyAnswer {
                id: PropertyId::RuntimeId,
                value: Some(PropertyValue::RuntimeId(RuntimeId(sender.runtime_id()))),
            },
        ]);
        let ListenerToCore::Answer(first) = parts.next() else {
            panic!("an answer");
        };
        let Outcome::Result(Reply::Steps(steps)) = first.outcome else {
            panic!("a result: {:?}", first.outcome);
        };
        let [StepOutcome::Done(Done::Read(object))] = steps.as_slice() else {
            panic!("one read: {steps:?}");
        };
        assert_eq!(first.number, RequestNumber(1));
        assert_eq!(object.values, expected);
        assert_eq!(
            object.window,
            verbatim_win32::window_facts::node_window(&tells::SystemReads, desktop_window())
        );
        assert_eq!(
            parts.next(),
            ListenerToCore::Answer(Answer {
                number: RequestNumber(2),
                outcome: Outcome::NotThere,
                probes: Vec::new(),
                detail: String::new(),
            })
        );
        parts.stop();
    }

    /// A selection and a notification whose sender's cached window the
    /// listener's reader answers hung as they arrive are dropped in their
    /// callbacks: nothing is told, and the window is reported once, its
    /// thread marked by the first drop.
    #[test]
    fn a_selection_and_a_notification_whose_window_is_hung_are_dropped_in_their_callbacks() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let selected = desktop_sender::<ListenerSelection>(&uia);
        let notifying = desktop_sender::<ListenerEvent>(&uia);
        assert!(
            capture_selection(&selected, &selected.read()).is_some()
                && capture(&notifying, &notifying.read()).is_some(),
            "both are captured, so only their window can drop them"
        );
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Event { callback, .. } =
            selection_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the selection subscription");
        };
        callback(&selected);
        drop(callback);
        let Subscription::Notifications { callback } =
            notification_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the notification subscription");
        };
        callback(
            &notifying,
            windows::Win32::UI::Accessibility::NotificationKind_Other,
            windows::Win32::UI::Accessibility::NotificationProcessing_All,
            Some("saved".to_owned()),
            None,
        );
        drop(callback);
        only_the_desktop_reported(&parts.stop());
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            [desktop_window(), desktop_window()]
        );
    }

    /// The listener subscribes to exactly the automation events UIA's
    /// proxies are told not to raise from `WinEvent`s, which one table
    /// names (`verbatim_uia::LISTENER_EVENTS`).
    #[test]
    fn the_listeners_automation_events_are_the_ones_the_proxies_ignore() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let event = |subscription: Subscription<Cached<ListenerEvent>>| match subscription {
            Subscription::Event { event, .. } => event,
            Subscription::Notifications { .. } => {
                windows::Win32::UI::Accessibility::UIA_NotificationEventId
            }
            _ => unreachable!("an automation event's subscription"),
        };
        let Subscription::Event {
            event: selection, ..
        } = selection_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the selection subscription");
        };
        let subscribed = [
            selection,
            event(menu_subscription(
                &parts.outgoing,
                &parts.readers,
                &parts.checks,
            )),
            event(notification_subscription(
                &parts.outgoing,
                &parts.readers,
                &parts.checks,
            )),
            event(toast_subscription(
                &parts.outgoing,
                &parts.readers,
                &parts.checks,
            )),
        ];
        assert_eq!(subscribed, verbatim_uia::LISTENER_EVENTS);
        assert_eq!(parts.stop(), []);
    }

    /// A window opened that is not a toast by NVDA's rule costs only the
    /// callback: nothing is told and no window is tested.
    #[test]
    fn a_window_opened_that_is_no_toast_is_not_told() {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let opened = desktop_sender::<ListenerEvent>(&uia);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let parts = listening(|_| Readability::Hung, &asked);
        let Subscription::Event { callback, .. } =
            toast_subscription(&parts.outgoing, &parts.readers, &parts.checks)
        else {
            unreachable!("the window-opened subscription");
        };
        callback(&opened);
        drop(callback);
        assert_eq!(parts.stop(), []);
        assert_eq!(
            *asked.lock().unwrap_or_else(PoisonError::into_inner),
            Vec::<isize>::new()
        );
    }

    #[test]
    fn a_uia_event_received_before_the_subscriptions_are_installed_is_dropped() {
        let (outgoing, _reader) = outgoing();
        assert!(!outgoing.uia_installed());
        outgoing.uia_installation_ended();
        assert!(outgoing.uia_installed());
    }

    /// The listener says `Ready` only once its UIA subscriptions'
    /// installation returns, as on a foreground application that does not
    /// answer, with its hooks in during the wait: an MSAA event raised
    /// meanwhile is told with no `Ready` before it, UIA events are dropped,
    /// and `Ready` arrives once the installation is released.
    #[test]
    fn the_listener_is_ready_once_its_uia_subscriptions_are_installed() {
        use windows::Win32::UI::Accessibility::NotifyWinEvent;
        use windows::Win32::UI::WindowsAndMessaging::{
            CHILDID_SELF, CreateWindowExW, DestroyWindow, EVENT_SYSTEM_ALERT, OBJID_CLIENT,
            WINDOW_EX_STYLE, WS_OVERLAPPED,
        };
        use windows::core::w;

        let (ours, theirs) =
            pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        let mut reader = BufReader::new(File::from(theirs));
        let (release, released) = mpsc::channel::<()>();
        let install: InstallSubscriptions = Box::new(move |_, _, _| {
            // Released once the test has read the alert below; the bound
            // only ends a run whose alert is never told, which then fails
            // on the `Ready` that follows.
            let _ = released.recv_timeout(Duration::from_secs(10));
            None
        });
        let listener = Listener::start(sender, (1, IgnoredProcesses::default()), install)
            .expect("the listener starts");

        // A hidden window of this thread's, with no parent, to raise an
        // alert from while the installation waits.
        // SAFETY: a window with no parent, made on this thread.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a window to raise an alert from");
        // SAFETY: raising an event for a live window of this thread.
        unsafe {
            NotifyWinEvent(
                EVENT_SYSTEM_ALERT,
                window,
                OBJID_CLIENT.0,
                CHILDID_SELF.cast_signed(),
            );
        };
        let alert = EventObject::Msaa {
            window: WindowHandle(u64::try_from(window.0 as isize).expect("a handle")),
            object: OBJID_CLIENT.0,
            child: CHILDID_SELF.cast_signed(),
        };
        // The global hooks tell this desktop's events too: the first of
        // the alert and `Ready` to arrive, `Ready` as `None`.
        let first =
            std::iter::from_fn(|| read_message(&mut reader).expect("read")).find_map(|message| {
                match message {
                    ListenerToCore::Ready { .. } => Some(None),
                    ListenerToCore::Tell(tell)
                        if tell.kind == EventKind::Alert && tell.object == alert =>
                    {
                        Some(Some(tell.object))
                    }
                    _ => None,
                }
            });
        assert_eq!(
            first,
            Some(Some(alert)),
            "the alert, with no Ready before it"
        );
        assert!(!listener.outgoing.uia_installed());
        // An idle barrier's query while the installation waits, as when a
        // barrier follows the listener's restart: answered only after
        // `Ready`.
        listener.idle_query(9);

        release.send(()).expect("the installation waits");
        let ready_and_answer: Vec<ListenerToCore> =
            std::iter::from_fn(|| read_message(&mut reader).expect("read"))
                .filter(|message| {
                    matches!(
                        message,
                        ListenerToCore::Ready { .. } | ListenerToCore::IdleAnswer { .. }
                    )
                })
                .take(2)
                .collect();
        assert_eq!(
            ready_and_answer,
            [
                ListenerToCore::Ready {
                    outpost_pid: Pid(std::process::id()),
                },
                ListenerToCore::IdleAnswer { round: 9 },
            ]
        );
        assert!(listener.outgoing.uia_installed());

        stop_with_no_registration(listener);
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(window) }.expect("the window destroyed");
    }

    /// Stops `listener`, whose UIA installation made no registration, as
    /// its shutdown does, with no I/O thread to finish its writes.
    fn stop_with_no_registration(listener: Listener) {
        let Listener {
            outgoing,
            readers,
            checks,
            registering,
            _event_thread: event_thread,
            ..
        } = listener;
        drop(event_thread);
        assert!(registering.join().expect("the installation ends").is_none());
        checks.stop();
        readers.stop();
        outgoing.close();
    }

    /// The idle barrier's answer waits behind every answer and report sent
    /// before it, a large one included whose write pends, and a pong goes
    /// ahead of them.
    #[test]
    fn the_idle_answer_waits_behind_every_answer_and_report_before_it() {
        let (outgoing, mut reader) = outgoing_with(4096, IgnoredProcesses::default());
        outgoing.ready();
        let answer = |number, detail: String| Answer {
            number: RequestNumber(number),
            outcome: Outcome::NotThere,
            probes: Vec::new(),
            detail,
        };
        outgoing.answer(answer(1, "x".repeat(20_000)));
        outgoing.answer(answer(2, String::new()));
        outgoing.idle_answer(1);
        outgoing.answer(answer(3, String::new()));
        outgoing.urgent(ListenerToCore::Pong { written_at_us: 0 });
        outgoing.idle_answer(2);
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => format!("answer {}", answer.number.0),
                ListenerToCore::Pong { .. } => "pong".to_owned(),
                ListenerToCore::Ready { .. } => "ready".to_owned(),
                ListenerToCore::IdleAnswer { round } => format!("idle {round}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "ready", "answer 1", "pong", "answer 2", "idle 1", "answer 3", "idle 2"
            ]
        );
    }

    /// An idle barrier's answer made before the listener is ready is held
    /// until `Ready` has been sent, behind it and every report sent before,
    /// and an answer made after goes behind the held ones.
    #[test]
    fn an_idle_answer_before_the_listener_is_ready_follows_ready() {
        let (outgoing, mut reader) = outgoing();
        outgoing.idle_answer(1);
        outgoing.answer(Answer {
            number: RequestNumber(3),
            outcome: Outcome::NotThere,
            probes: Vec::new(),
            detail: String::new(),
        });
        outgoing.idle_answer(2);
        outgoing.ready();
        outgoing.idle_answer(3);
        let order: Vec<String> = sent(&outgoing, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Answer(answer) => format!("answer {}", answer.number.0),
                ListenerToCore::Ready { .. } => "ready".to_owned(),
                ListenerToCore::IdleAnswer { round } => format!("idle {round}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(order, ["answer 3", "ready", "idle 1", "idle 2", "idle 3"]);
    }

    /// A plain window of this process, of class `Static`, destroyed on
    /// drop by the thread that made it.
    struct StaticWindow(isize);

    impl StaticWindow {
        fn new() -> Self {
            use windows::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, WINDOW_EX_STYLE, WINDOW_STYLE,
            };
            // SAFETY: a plain window of a system class, destroyed on drop.
            let window = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    windows::core::w!("STATIC"),
                    None,
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("a window of this process");
            Self(window.0 as isize)
        }
    }

    impl Drop for StaticWindow {
        fn drop(&mut self) {
            use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
            // SAFETY: made on this thread, destroyed once.
            let _ = unsafe { DestroyWindow(windows::Win32::Foundation::HWND(self.0 as _)) };
        }
    }

    /// One MSAA event of `kind` on `window`'s client area, raised by this
    /// thread.
    fn client_event(kind: WinEventKind, raw: u32, window: isize) -> WinEvent {
        WinEvent {
            kind,
            event: raw,
            hwnd: window,
            id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
            id_child: 0,
            // SAFETY: no preconditions.
            thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
            time: 0,
            raised_ms_ago: 0,
        }
    }

    /// What `messages` are, in order: a foreground report or a tell, by
    /// kind.
    fn reports_and_tells(messages: Vec<ListenerToCore>) -> Vec<String> {
        messages
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Foreground { window, .. } => format!("report {window}"),
                ListenerToCore::Tell(tell) => format!("tell {:?}", tell.kind),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// The listener's parts for `on_win_event`, its window in front read
    /// by `front`, and the pipe's other end.
    fn listener_reading_front(
        front: fn() -> isize,
    ) -> (Arc<Outgoing>, Arc<Readers<UiaReading>>, BufReader<File>) {
        let (ours, theirs) =
            pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "listener-test", || {}).expect("a sender");
        let tells = Arc::new(tells::Tells::with_front(sender.clone(), front));
        let outgoing = Arc::new(Outgoing::with_tells(
            sender,
            IgnoredProcesses::default(),
            tells,
        ));
        let kept = Arc::new(Kept::new(1));
        let readers = Arc::new(Readers::new(UiaReading::new(&outgoing, &kept)));
        (outgoing, readers, BufReader::new(File::from(theirs)))
    }

    /// A foreground event whose window is in front as it is handled: Core
    /// is sent the report that makes the window the foreground before the
    /// event's tell, since Core judges the tell against the window in
    /// front (`msaa_flood_then_focus`, `background_progress_bar`).
    #[test]
    fn a_foreground_in_front_is_reported_before_its_tell() {
        static FRONT: AtomicIsize = AtomicIsize::new(0);
        let window = StaticWindow::new();
        FRONT.store(window.0, Ordering::SeqCst);
        let (outgoing, readers, mut reader) =
            listener_reading_front(|| FRONT.load(Ordering::SeqCst));
        let thread = EventThreadState::default();
        let _ = thread.watch.borrow_mut().start(None);
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                client_event(
                    WinEventKind::Foreground,
                    windows::Win32::UI::WindowsAndMessaging::EVENT_SYSTEM_FOREGROUND,
                    window.0,
                ),
                None,
            ),
            &SystemReads,
        );
        readers.stop();
        assert_eq!(
            reports_and_tells(sent(&outgoing, &mut reader)),
            [format!("report {}", window.0), "tell Foreground".to_owned()]
        );
    }

    /// A foreground event whose window is not yet in front, then a focus
    /// in it once it is: the focus's event sends the report first, then
    /// the held foreground tell and the focus's tell, in order.
    #[test]
    fn a_foreground_found_in_front_by_a_focus_is_reported_before_both_tells() {
        static FRONT: AtomicIsize = AtomicIsize::new(0);
        let window = StaticWindow::new();
        let (outgoing, readers, mut reader) =
            listener_reading_front(|| FRONT.load(Ordering::SeqCst));
        let thread = EventThreadState::default();
        let _ = thread.watch.borrow_mut().start(None);
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                client_event(
                    WinEventKind::Foreground,
                    windows::Win32::UI::WindowsAndMessaging::EVENT_SYSTEM_FOREGROUND,
                    window.0,
                ),
                None,
            ),
            &SystemReads,
        );
        FRONT.store(window.0, Ordering::SeqCst);
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                client_event(
                    WinEventKind::Focus,
                    windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS,
                    window.0,
                ),
                None,
            ),
            &SystemReads,
        );
        readers.stop();
        assert_eq!(
            reports_and_tells(sent(&outgoing, &mut reader)),
            [
                format!("report {}", window.0),
                "tell Foreground".to_owned(),
                "tell MsaaFocus".to_owned(),
            ]
        );
    }

    /// A foreground event whose window is not yet in front, then a name
    /// change once it is: as the name change's tell releases the held
    /// foreground tell, the report goes first, so no tell is judged by
    /// Core before the report of the window it found in front.
    #[test]
    fn a_foreground_found_in_front_by_any_event_is_reported_before_the_tells_it_releases() {
        static FRONT: AtomicIsize = AtomicIsize::new(0);
        let window = StaticWindow::new();
        let (outgoing, readers, mut reader) =
            listener_reading_front(|| FRONT.load(Ordering::SeqCst));
        let thread = EventThreadState::default();
        let _ = thread.watch.borrow_mut().start(None);
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                client_event(
                    WinEventKind::Foreground,
                    windows::Win32::UI::WindowsAndMessaging::EVENT_SYSTEM_FOREGROUND,
                    window.0,
                ),
                None,
            ),
            &SystemReads,
        );
        FRONT.store(window.0, Ordering::SeqCst);
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                client_event(
                    WinEventKind::NameChange,
                    windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_NAMECHANGE,
                    window.0,
                ),
                None,
            ),
            &SystemReads,
        );
        readers.stop();
        assert_eq!(
            reports_and_tells(sent(&outgoing, &mut reader)),
            [
                format!("report {}", window.0),
                "tell Foreground".to_owned(),
                "tell NameChange".to_owned(),
            ]
        );
    }

    /// A made-up desktop's handles: the desktop, the flood's top-level
    /// window, a tooltip and the window in front, none a real window, so
    /// that no read can reach a real one.
    const FLOOD_DESKTOP: isize = 0x7FFF_0001;
    const FLOOD_WINDOW: isize = 0x7FFF_0010;
    const FLOOD_TOOLTIP: isize = 0x7FFF_0030;
    const FLOOD_FRONT: isize = 0x7FFF_0020;

    /// The flood's desktop as the listener reads it, every read counted by
    /// its name: the flood's window and its tooltip are top-level windows
    /// of this process, behind the window in front, which is active. A
    /// desk made [`owned_in_front`](Self::owned_in_front) has the flood's
    /// window in front instead, and gives it an owner after its root owner
    /// is first read.
    #[derive(Default)]
    struct FloodDesk {
        counts: RefCell<std::collections::BTreeMap<&'static str, u32>>,
        owned_in_front: bool,
    }

    /// The owner the flood's window is given in an
    /// [`owned_in_front`](FloodDesk::owned_in_front) desk.
    const FLOOD_OWNER: isize = 0x7FFF_0040;

    impl FloodDesk {
        fn owned_in_front() -> Self {
            Self {
                owned_in_front: true,
                ..Self::default()
            }
        }

        fn count<T>(&self, name: &'static str, value: T) -> T {
            *self.counts.borrow_mut().entry(name).or_default() += 1;
            value
        }
    }

    impl WindowReads for FloodDesk {
        fn class(&self, window: isize) -> String {
            let class = match window {
                FLOOD_DESKTOP => "#32769",
                FLOOD_TOOLTIP => "tooltips_class32",
                FLOOD_FRONT => "Front",
                _ => "MockAppWindow",
            };
            self.count("class", class.to_owned())
        }
        fn owner(&self, _window: isize) -> (u32, u32) {
            self.count("owner", (1, std::process::id()))
        }
        fn console_host(&self, _window: isize) -> Option<u32> {
            self.count("console_host", None)
        }
        fn parent(&self, _window: isize) -> isize {
            self.count("parent", FLOOD_DESKTOP)
        }
        fn root(&self, window: isize) -> isize {
            self.count("root", window)
        }
        fn root_owner(&self, window: isize) -> isize {
            let earlier = self.counts.borrow().get("root_owner").copied();
            let owner = if self.owned_in_front && window == FLOOD_WINDOW && earlier.is_some() {
                FLOOD_OWNER
            } else {
                window
            };
            self.count("root_owner", owner)
        }
        fn is_child(&self, _parent: isize, _child: isize) -> bool {
            self.count("is_child", false)
        }
        fn topmost(&self, _window: isize) -> bool {
            self.count("topmost", false)
        }
        fn desktop(&self) -> isize {
            self.count("desktop", FLOOD_DESKTOP)
        }
        fn foreground(&self) -> isize {
            let front = if self.owned_in_front {
                FLOOD_WINDOW
            } else {
                FLOOD_FRONT
            };
            self.count("foreground", front)
        }
        fn active_window(&self) -> isize {
            let active = if self.owned_in_front {
                FLOOD_WINDOW
            } else {
                FLOOD_FRONT
            };
            self.count("active_window", active)
        }
        fn hidden_frame(&self, _window: isize) -> bool {
            self.count("hidden_frame", false)
        }
    }

    impl EventReads for FloodDesk {
        fn exists(&self, _window: isize) -> bool {
            self.count("exists", true)
        }
        fn hang_state(&self, _window: isize) -> Readability {
            self.count("hang_state", Readability::Readable)
        }
        fn stood_for(&self, _window: isize) -> Option<isize> {
            self.count("stood_for", None)
        }
        fn process_of_thread(&self, _thread: u32) -> Option<u32> {
            self.count("process_of_thread", Some(std::process::id()))
        }
        fn input_desktop_opens(&self) -> bool {
            self.count("input_desktop_opens", true)
        }
    }

    /// A tell's foreground facts are read under the tells' lock, not taken
    /// from what the event read before it: a name change in the window in
    /// front, which is given an owner between the read of its own facts
    /// and the tell's placing, is told with that owner as the foreground's
    /// root owner, and its own root owner as read before.
    #[test]
    fn a_tells_foreground_facts_are_read_as_it_is_placed() {
        use windows::Win32::UI::WindowsAndMessaging::{EVENT_OBJECT_NAMECHANGE, OBJID_CLIENT};
        let (outgoing, readers, sent) =
            flood_listening_front(Arc::new(|_| Readability::Readable), || FLOOD_FRONT);
        let thread = EventThreadState::default();
        let _ = thread.watch.borrow_mut().start(None);
        let desk = FloodDesk::owned_in_front();
        on_win_event(
            &outgoing,
            &tells::known(),
            &thread,
            (
                WinEvent {
                    kind: WinEventKind::NameChange,
                    event: EVENT_OBJECT_NAMECHANGE,
                    hwnd: FLOOD_WINDOW,
                    id_object: OBJID_CLIENT.0,
                    id_child: 0,
                    thread: 1,
                    time: 0,
                    raised_ms_ago: 0,
                },
                Some("MockAppWindow"),
            ),
            &desk,
        );
        readers.stop();
        outgoing.close();
        let facts: Vec<(WindowHandle, WindowHandle, bool)> = sent
            .iter()
            .filter_map(|message| match message {
                ListenerToCore::Tell(tell) => match &tell.window {
                    TellWindow::Found(facts) => Some((
                        facts.root_owner.handle,
                        facts.foreground_root_owner,
                        facts.root_owner_is_foregrounds,
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let handle = |window: isize| WindowHandle(window.cast_unsigned() as u64);
        assert_eq!(facts, [(handle(FLOOD_WINDOW), handle(FLOOD_OWNER), false)]);
    }

    /// How many times the window in front was read for the hold, by the
    /// one test at a time that holds [`FLOOD_COUNTING`].
    static FLOOD_FRONT_READS: AtomicU32 = AtomicU32::new(0);

    /// Held by a test while it counts [`FLOOD_FRONT_READS`].
    static FLOOD_COUNTING: Mutex<()> = Mutex::new(());

    /// The listener's sending side and readers for the flood, judging
    /// readability with `readability`, and reading the window in front for
    /// the hold as the flood's, each read counted; what it sends, read on a
    /// thread of its own.
    fn flood_listening(
        readability: WindowReadabilityReader,
    ) -> (
        Arc<Outgoing>,
        Arc<Readers<UiaReading>>,
        mpsc::Receiver<ListenerToCore>,
    ) {
        flood_listening_front(readability, || {
            FLOOD_FRONT_READS.fetch_add(1, Ordering::SeqCst);
            FLOOD_FRONT
        })
    }

    /// [`flood_listening`], reading the window in front for the hold with
    /// `front`.
    fn flood_listening_front(
        readability: WindowReadabilityReader,
        front: fn() -> isize,
    ) -> (
        Arc<Outgoing>,
        Arc<Readers<UiaReading>>,
        mpsc::Receiver<ListenerToCore>,
    ) {
        let (sender_end, theirs) =
            pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(sender_end, Waiting::default(), "flood-test", || {}).expect("a sender");
        let tells = Arc::new(tells::Tells::with_front(sender.clone(), front));
        let outgoing = Arc::new(Outgoing::with_tells(
            sender,
            IgnoredProcesses::default(),
            tells,
        ));
        let sent = drain(BufReader::new(File::from(theirs)));
        let kept = Arc::new(Kept::new(1));
        let readers = Arc::new(Readers::new(UiaReading {
            outgoing: Arc::clone(&outgoing),
            kept,
            readability,
            read_at_event: |_, _, _, _| None,
        }));
        (outgoing, readers, sent)
    }

    /// The flood's two kinds of event, each named, with the class of its
    /// window the hook read: a value change, and a tooltip's show.
    fn flood_events() -> [(&'static str, WinEvent, &'static str); 2] {
        use windows::Win32::UI::WindowsAndMessaging::{
            EVENT_OBJECT_SHOW, EVENT_OBJECT_VALUECHANGE,
        };
        let flood_event = |kind, raw, hwnd, id_object| WinEvent {
            kind,
            event: raw,
            hwnd,
            id_object,
            id_child: 0,
            thread: 1,
            time: 0,
            raised_ms_ago: 0,
        };
        [
            (
                "a value change",
                flood_event(
                    WinEventKind::ValueChange,
                    EVENT_OBJECT_VALUECHANGE,
                    FLOOD_WINDOW,
                    1000,
                ),
                "MockAppWindow",
            ),
            (
                "a tooltip's show",
                flood_event(WinEventKind::Show, EVENT_OBJECT_SHOW, FLOOD_TOOLTIP, 1000),
                "tooltips_class32",
            ),
        ]
    }

    /// What [`flood_reads`] found.
    type FloodReads = (Vec<String>, Vec<(&'static str, ReadCounts)>, bool);

    /// Each call to Windows an event made, by name.
    type ReadCounts = std::collections::BTreeMap<&'static str, u32>;

    /// What the flood's events read: the kinds told, each event's calls to
    /// Windows by name, and whether the readers hold no job once the events
    /// are handled.
    fn flood_reads() -> FloodReads {
        let _counting = FLOOD_COUNTING
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let readability_reads = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&readability_reads);
        let readability: WindowReadabilityReader = Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Readability::Readable
        });
        let (outgoing, readers, sent) = flood_listening(readability);
        let thread = EventThreadState::default();
        let _ = thread.watch.borrow_mut().start(None);
        let mut counts = Vec::new();
        for (name, event, class) in flood_events() {
            let desk = FloodDesk::default();
            FLOOD_FRONT_READS.store(0, Ordering::SeqCst);
            readability_reads.store(0, Ordering::SeqCst);
            on_win_event(
                &outgoing,
                &tells::known(),
                &thread,
                (event, Some(class)),
                &desk,
            );
            let mut read = desk.counts.into_inner();
            read.insert(
                "front for the hold",
                FLOOD_FRONT_READS.load(Ordering::SeqCst),
            );
            read.insert("readability", readability_reads.load(Ordering::SeqCst));
            counts.push((name, read));
        }
        let idle = nothing_queued(&readers);
        readers.stop();
        outgoing.close();
        let tells: Vec<String> = sent
            .iter()
            .filter_map(|message| match message {
                ListenerToCore::Tell(tell) => Some(format!("{:?}", tell.kind)),
                _ => None,
            })
            .collect();
        (tells, counts, idle)
    }

    /// The calls to Windows a tooltip's show makes on the event thread.
    fn shown_reads() -> ReadCounts {
        std::collections::BTreeMap::from([
            ("active_window", 1),
            ("class", 5),
            ("desktop", 2),
            ("exists", 1),
            ("foreground", 1),
            ("front for the hold", 1),
            ("hang_state", 1),
            ("hidden_frame", 2),
            ("is_child", 3),
            ("owner", 4),
            ("parent", 1),
            ("readability", 0),
            ("root", 1),
            ("root_owner", 2),
            ("topmost", 2),
        ])
    }

    /// The calls to Windows the event thread makes for each `WinEvent` of
    /// `msaa_flood_then_focus`'s floods, behind the one class read of the
    /// hook's filter, which it hands on (`verbatim_ia2`'s
    /// `the_class_the_filter_read_is_handed_on_with_the_event`): a value
    /// change of a window in the background, and a tooltip's show, each
    /// read alike, 27 local calls. Each fact is read where it is needed,
    /// with no record kept for the event, so the window's class and owner
    /// are read again by the reads of its facts; the window in front is
    /// read for the hold and again under the tells' lock, with the active
    /// window and the windows' relations to them, as the tell is placed. A
    /// value change is told and read by no thread of the listener's: Core
    /// asks for it.
    #[test]
    fn a_flood_events_window_facts_are_read_once() {
        let (tells, counts, idle) = flood_reads();
        assert_eq!(tells, ["ValueChange", "Show"]);
        let shown = shown_reads();
        assert_eq!(
            counts,
            [
                ("a value change", shown.clone()),
                ("a tooltip's show", shown)
            ]
        );
        assert!(idle, "the readers hold no job");
    }
}
