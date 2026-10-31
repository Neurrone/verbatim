//! The per-application outpost (architecture section 1, "Outposts";
//! decision D9). One outpost watches one application for its whole life:
//! the application is set once, when the outpost starts, or, for a spare
//! started before it had one, when the supervisor assigns it
//! (`docs/design/focus-pipeline.md`, section 19), and never changes after.
//! Its parts:
//!
//! 1. The mailbox ([`shell`]): the MSAA hook callbacks, the UIA
//!    subscription callbacks, and the reader's routed facts and queries only
//!    append an entry to it and return. They never call into the
//!    application and never wait on anything but the mailbox's lock.
//! 2. The reducer ([`reducer`]), on a thread of its own run by its shell
//!    ([`shell`]): the pure state machine that holds every remembered fact
//!    of the application, the intake's order with NVDA's limiter rules
//!    among them, and keeps the outpost's timers: a worker's deadline, the
//!    walk's bound, the caret key's watch and the gap record's bound.
//! 3. The workers ([`worker`]): the threads that call into the application,
//!    one generation in charge at a time, each running one program at a
//!    time on copies, posting its commits to the mailbox. A worker whose
//!    call hangs is abandoned and replaced by the reducer.
//! 5. The I/O thread ([`Outpost::handle_command`], driven by [`run_pipe`]
//!    on the transport's child side): reads Core's commands, answers pings
//!    itself, so a busy or hung worker never makes the outpost look dead,
//!    and finishes the writes to Core that pended.
//! 6. The outbound queue ([`outbound`]): every thread writes its own
//!    messages to Core when no write is in flight; pongs and `Ready` go
//!    first; sending never waits, and messages waiting behind a write in
//!    flight are merged by object and kind.
//!
//! Workers run in COM's multithreaded apartment; the object store
//! ([`store`]) keeps agile references, so a replacement worker can use what
//! an abandoned one minted.
//!
//! Held objects (`docs/crates/verbatim-outpost.md`): the store keeps the
//! live UIA element or MSAA object behind every node the outpost reports.
//! Each message that carries node ids has a position, counted the same way
//! by Core; when Core reports the nodes it still holds and the position of
//! the last message it has handled, the worker releases every other node
//! no live request can still name, reported at or before that position or
//! never reported. A node reported later, or named by a request still
//! running, is kept.

mod bound;
mod commit;
mod focus_path;
mod narrowing;
mod outbound;
mod read;
mod shell;
mod shell_items;
mod store;
pub(crate) mod text_reads;
pub(crate) mod window;
mod worker;

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{
    IUIAutomationTextRange, UIA_RangeValueValuePropertyId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId, UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;

use verbatim_ia2::{APP_SUBSCRIPTIONS, WinEvent, WinEventKind};
use verbatim_model::{Backend, Fetches, NodeId, Pid, TraceId};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{
    BlockingReader, ChildIo, EVENT_PIPE_BUFFER, Sender, Served, Serving,
};
use verbatim_uia::{
    Ancestor, Cached, FOCUS_PROPERTIES, Node, Part, PropertyEvent, Registration, Request, Scope,
    Subscription, TextEvent, Uia,
};

use crate::arbitration::{Arbitrator, EventWindows};
use verbatim_ia2::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger};
use verbatim_model::protocol::{
    CoreToOutpost, DeliveredFact, EventTiming, GapAnswer, OutpostToCore, Query, UiaSnapshotFact,
    Urgent, WinEventStamp,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::clock::now_us;
use verbatim_process::held::Target;
use verbatim_process::message::{decode_message, read_message};
use verbatim_reader_rules::acceptance;
use verbatim_win32::window::{ForegroundReader, WindowReadabilityReader};

use verbatim_outpost_logic as reducer;

use commit::{Committed, Effects};
use outbound::Outbound;
pub use reducer::Verdicts;
pub(crate) use reducer::{
    Anchor, AnswerCut, CaretSeen, CutLine, KeptSelection, NodeAnchors, Probe, Probed, ReportCut,
    VerdictChange,
};
use reducer::{
    Entry, Facts, FollowState, Item, Object, OpenWatch, Source, UiaEvent, UiaKind, WaitingCaret,
};
pub use shell::GapTimer;
use shell::{Mailbox, Posted, Shell};
use worker::Charge;

pub(crate) use window::now_ms;

#[cfg(test)]
mod application_names_tests {
    use std::process::{Command, Stdio};

    use super::ApplicationNames;

    #[test]
    fn a_name_does_not_outlive_its_process() {
        // Waits on its standard input, which is never written.
        let mut child = Command::new("cmd")
            .args(["/d", "/c", "pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("cmd starts");
        let pid = child.id();
        let mut names = ApplicationNames::default();
        assert_eq!(names.name(pid), "cmd");
        child.kill().expect("cmd is ended");
        child.wait().expect("the exit of cmd is seen");
        assert_eq!(names.name(pid), "");
        assert!(!names.0.contains_key(&pid), "the exited process is let go");
    }
}

/// Application names by process, each kept with its process held open, so
/// that a name never outlives its process: Windows gives a process's id to
/// no other process while a handle to it is open, and an entry whose
/// process has exited is dropped and its id read afresh. A console's
/// clients come and go while its host, the outpost's application, runs.
#[derive(Default)]
struct ApplicationNames(HashMap<u32, (Target, String)>);

impl ApplicationNames {
    /// The name of the application `pid`, empty when its process cannot be
    /// opened or its name read: two local calls when it is kept, to ask
    /// whether its process has exited.
    fn name(&mut self, pid: u32) -> String {
        if let Some((process, name)) = self.0.get(&pid)
            && !process.has_exited()
        {
            return name.clone();
        }
        // Every exited process's entry goes, so none is held open after it.
        self.0.retain(|_, (process, _)| !process.has_exited());
        let Ok(process) = Target::open(Pid(pid)) else {
            return String::new();
        };
        let name = process.image_stem().unwrap_or_default();
        self.0.insert(pid, (process, name.clone()));
        name
    }
}

/// What the outpost's threads share.
pub(crate) struct Context {
    /// The application, set once ([`Outpost::assign`]); 0 until then, a
    /// spare's value while it waits, which names no application: the
    /// System Idle Process owns no window.
    target_pid: AtomicU32,
    /// The application names a focus change carries
    /// ([`verbatim_model::WindowFacts::application`]), by process: the
    /// application's own, read when it is assigned, and a console client's,
    /// read the first time a focus in its console window is reported.
    application_names: Mutex<ApplicationNames>,
    outbound: Outbound,
    /// What every producer appends to and the reducer's shell drains
    /// ([`shell`]).
    mailbox: Mailbox,
    /// The outpost's start, from which the reducer's clock counts.
    epoch: Instant,
    /// What times the gap record's bound in place of the reducer's own
    /// timer, when a test sets one ([`Outpost::set_gap_timer`]).
    gap_timer: Mutex<Option<Arc<dyn GapTimer>>>,
    /// Abandoned workers that have not yet returned, as the reducer last
    /// counted them: an atomic, so the I/O thread answers a ping without
    /// waiting on anything.
    abandoned: AtomicUsize,
    /// The reducer's thread, which returns every worker thread started.
    shell: Mutex<Option<JoinHandle<Vec<JoinHandle<()>>>>>,
    /// Every application object the outpost keeps, and the identity it
    /// gives each ([`store`]).
    store: store::Store,
    arbitrator: Mutex<Arbitrator>,
    /// The clock the outpost's own hook and subscriptions stamp their
    /// events' observations with, and its arbitrator its verdicts' keeps
    /// by: the run's shared clock, unless a test sets its own
    /// ([`Outpost::set_observation_clock`]).
    observation: OnceLock<verbatim_process::clock::ObservationClock>,
    /// The processes Verbatim ignores, which the outpost inherits through
    /// [`IGNORE_PIDS_ENV`](verbatim_process::held::IGNORE_PIDS_ENV): a focus in
    /// one's window is not reported.
    ignored: verbatim_process::held::IgnoredProcesses,
    /// The client on a thread of its own for the worker's operations with a
    /// deadline of their own: a dialog's text.
    bounded: verbatim_uia::BoundedClient,
    /// The focus-following UIA subscription, one event handler group in
    /// two forms, which the worker moves with the focus
    /// ([`register_focus_events`]).
    focus_events: OnceLock<Registration>,
    /// Whether UIA reads may use remote operations ([`OutpostOptions`]).
    remote_operations: bool,
    /// Windows whose UIA elements could not be imported into a remote
    /// operation (client-side proxies), read the classic way for the
    /// window's lifetime, since a window's provider does not change. Each
    /// is kept with the thread that owned it, so a reused handle, whose
    /// destroy event was lost, is not read the classic way by mistake.
    classic_windows: Mutex<HashMap<isize, u32>>,
    /// The details the active theme wants read for each node
    /// ([`CoreToOutpost::Fetches`]); everything until Core says
    /// otherwise. UIA reads leave the properties of the others out of
    /// their cache requests; MSAA reads skip their calls, through the MSAA
    /// registry, which holds the same.
    fetches: Mutex<Fetches>,
    /// How the worker reads the element that has the keyboard focus, for a
    /// focus-now query: a UIA focus event's element comes from the focus
    /// listener's read instead.
    focused_element: FocusedElementReader,
    /// How the worker reads the foreground window when it records the
    /// window a focus was reported in ([`Outpost::set_foreground_reader`]).
    foreground: Mutex<ForegroundReader>,
    /// How a focus-now query reads the window with the keyboard focus
    /// ([`Outpost::set_focus_window_reader`]).
    focus_window: Mutex<FocusWindowReader>,
    /// How the worker judges whether a window can be read, or is no
    /// window, a ghost's or a hung application's
    /// ([`Outpost::set_window_readability_reader`]).
    window_readability: Mutex<WindowReadabilityReader>,
    /// The events application support asked for from any window of the
    /// application, whatever the foreground ([`acceptance`]).
    requested_events: acceptance::RequestedEvents,
    /// Who is told of each event the outpost's own handlers take in
    /// ([`Outpost::observe_heard`]), if anyone.
    heard: OnceLock<HeardObserver>,
    /// The failed cached reads the worker has made (`verbatim_uia::calls`),
    /// taken after each entry.
    failed_reads: AtomicU32,
    /// How many times Core has changed the backend override, which decides
    /// every window anew: the event thread's windows' backends are
    /// decided again after a change ([`EventWindows`]).
    override_changes: AtomicU64,
    /// When the first of the focus listener's UIA focuses reached this
    /// outpost, its read or its drop as a focus, on the run's clock in
    /// microseconds, 0 before any: the gap's read logs whether
    /// one came before the copy's observation (`uia_focus_before`).
    first_uia_focus_us: AtomicU64,
    /// What nudges the event thread, which decides whether the hook is
    /// narrowed ([`narrowing`]); set once the thread runs.
    event_thread: OnceLock<Nudger>,
}

/// An event the outpost's own UIA handlers or `WinEvent` hooks took in from
/// its application ([`Outpost::observe_heard`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A property change on the focus or one of its ancestors, by property
    /// id.
    UiaProperty(i32),
    /// A text focus's text selection changed event: its caret moved, or
    /// the application reported it.
    UiaTextSelection,
    /// A text focus's text changed event.
    UiaTextChanged,
    /// A text focus's active text position changed event.
    UiaActiveTextPosition,
    /// A `WinEvent` of one of the kinds the outpost hooks.
    Msaa(verbatim_ia2::WinEventKind),
}

/// Told of each event an outpost takes in ([`Outpost::observe_heard`]).
pub type HeardObserver = Arc<dyn Fn(Heard) + Send + Sync>;

/// Reads the handle of the window with the keyboard focus, `None` when
/// there is none. An outpost reads the system's (`GetGUIThreadInfo`); a
/// test whose application cannot take the keyboard focus on the desktop it
/// runs on supplies its own ([`Outpost::set_focus_window_reader`]).
pub type FocusWindowReader = Arc<dyn Fn() -> Option<isize> + Send + Sync>;

/// Reads the UIA element that has the keyboard focus, filled by the given
/// request, with the given client, on the worker's thread, for a
/// focus-now query, the one read of the focused element the outpost makes
/// (a UIA focus event's element is the focus listener's read). An outpost
/// reads the system's keyboard focus ([`Uia::focused`]); a test
/// that drives a real outpost against an application without taking the
/// keyboard focus from the desktop it runs on supplies its own
/// ([`Outpost::with_focused_element_reader`]).
pub type FocusedElementReader =
    Arc<dyn Fn(&Uia, &Request<Node>) -> windows::core::Result<Cached<Node>> + Send + Sync>;

/// The outpost's shared state as one request sees it: the store's handle
/// for the request ([`store::Nodes`]), carrying its mint tag, that of a
/// program, or of the request a publication at a walk's bound claims; and,
/// for a program, its copies of the facts it reads. The store tags every
/// node the request issues or looks up with it, and a program's text
/// writes are made only with a commit the reducer accepts.
struct RequestContext<'a> {
    context: &'a Context,
    nodes: store::Nodes<'a>,
    /// The facts the request reads: a program's copies, which its commits
    /// are applied to, or the reducer's own, copied for a publication.
    facts: RefCell<Facts>,
    /// What the program holds of the store's text, and its text writes
    /// not yet committed ([`text_reads::ProgramText`]).
    program_text: RefCell<text_reads::ProgramText>,
    /// The program's own, when the context is a program's: its request,
    /// the caret events waiting when it was dispatched, and its worker's
    /// charge.
    program: Option<Program<'a>>,
    /// The observation its verdicts are judged at: its entry's event's, or,
    /// when `None`, now on the arbitrator's observation clock at each
    /// judgement (`docs/design/focus-pipeline-rework.md`, "Who owns each
    /// duplicated fact", the verdicts).
    judged_at: Option<u64>,
}

/// What a program's context holds of it ([`RequestContext::program`]).
struct Program<'a> {
    request: reducer::Request,
    waiting: Vec<WaitingCaret>,
    charge: Arc<Charge>,
    _context: std::marker::PhantomData<&'a ()>,
}

impl<'a> RequestContext<'a> {
    /// The context of a thread that runs no program, tagging what it mints
    /// with `tag` and reading `facts`: the reducer's publication at a
    /// walk's bound.
    fn new(context: &'a Context, tag: store::Tag, facts: Facts) -> Self {
        Self {
            context,
            nodes: context.store.nodes_for(tag),
            facts: RefCell::new(facts),
            program_text: RefCell::default(),
            program: None,
            judged_at: None,
        }
    }

    /// The context of program `request`, given `facts`, `waiting`, and its
    /// worker's `charge`.
    fn for_program(
        context: &'a Context,
        tag: store::Tag,
        (request, facts, waiting): (reducer::Request, Facts, Vec<WaitingCaret>),
        charge: Arc<Charge>,
    ) -> Self {
        Self {
            context,
            nodes: context.store.nodes_for(tag),
            facts: RefCell::new(facts),
            program_text: RefCell::default(),
            program: Some(Program {
                request,
                waiting,
                charge,
                _context: std::marker::PhantomData,
            }),
            judged_at: None,
        }
    }

    /// Judges every verdict the program asks for at the observation
    /// `at_us`.
    fn judge_at(&mut self, at_us: u64) {
        self.judged_at = Some(at_us);
    }

    /// The program this context runs.
    ///
    /// # Panics
    ///
    /// When the context runs no program: only a program's code asks.
    fn program(&self) -> &Program<'a> {
        self.program.as_ref().expect("a program's context")
    }

    fn tag(&self) -> store::Tag {
        self.nodes.tag()
    }

    /// The store's handle for this request, through which walks issue and
    /// look up nodes.
    fn nodes(&self) -> &store::Nodes<'a> {
        &self.nodes
    }

    /// What the request's lookups replaced, to drop once no lock is held.
    fn take_removed(&self) -> store::Removed {
        self.nodes.take_removed()
    }

    /// The program's request.
    fn request(&self) -> reducer::Request {
        self.program().request
    }

    /// The program's worker's charge.
    fn charge(&self) -> &Charge {
        &self.program().charge
    }

    /// The caret events waiting when the program was dispatched.
    fn waiting(&self) -> &[WaitingCaret] {
        self.program
            .as_ref()
            .map_or(&[], |program| program.waiting.as_slice())
    }

    /// The facts as the program's copies hold them, or the reducer's own,
    /// copied, for a publication.
    fn facts(&self) -> Ref<'_, Facts> {
        self.facts.borrow()
    }

    /// Reads the follow state with `read`.
    fn follow<T>(&self, read: impl FnOnce(&FollowState) -> T) -> T {
        read(&self.facts().follow)
    }

    /// The reports whose focus node this outpost has not released.
    fn held(&self) -> Ref<'_, reducer::Held> {
        Ref::map(self.facts(), |facts| &facts.held)
    }

    /// The intake's marks.
    fn marks(&self) -> Ref<'_, reducer::Marks> {
        Ref::map(self.facts(), |facts| &facts.marks)
    }

    /// The object last reported as the focus.
    fn focused(&self) -> Option<Object> {
        self.marks().focused.clone()
    }

    /// Whether a focus event on `object` is the redirected-to child's own,
    /// arriving after its control's: `object` is the focus, reached by a
    /// redirect, and no other focus came between. The mark is left set; the
    /// commit that drops the event clears it.
    fn is_redirected_focus(&self, object: &Object) -> bool {
        let marks = self.marks();
        marks.focus_redirected && marks.focused.as_ref() == Some(object)
    }

    /// Whether a read of `node`'s caret committed since `observed_us`
    /// covers an event observed then, which needs no report of its own.
    fn caret_read_since(&self, node: NodeId, observed_us: u64) -> bool {
        self.follow(|follow| follow.caret_read.covers(node.number(), observed_us))
    }

    /// The window of the last focus this outpost reported.
    fn tracked_window(&self) -> Option<isize> {
        self.follow(|follow| follow.tracking.window)
    }

    /// Now, on the reducer's clock.
    fn now(&self) -> Duration {
        self.context.epoch.elapsed()
    }

    /// The route to the element `runtime_id` names, when it is in the chain
    /// the focus listener read for the UIA focus this outpost last reported,
    /// and its window is one the outpost's application serves: the outpost
    /// looks up no window of another process by its handle
    /// (`docs/design/listener-without-connections.md`, section 4.3).
    fn focus_route(&self, runtime_id: &[i32]) -> Option<focus_path::Route> {
        let route = self.follow(|follow| {
            follow
                .focus_path
                .as_ref()
                .and_then(|path| path.route_to(runtime_id))
        })?;
        read::serves(self, route.window).then_some(route)
    }

    /// Applies `change`, a change of text facts one of the program's text
    /// calls made, to the program's copies at once, so what it reads after
    /// is what the reducer will hold once its commit is applied.
    fn apply_text(&self, change: &reducer::TextChange) {
        let mut facts = self.facts.borrow_mut();
        let _ = Arc::make_mut(&mut facts.text).apply(change);
    }

    /// Commits the program's text writes not yet committed, if it has
    /// any: at the program's end, for the writes no commit after them
    /// carried.
    fn commit_text(&self) {
        if self.program.is_some() && self.program_text.borrow().has_pending() {
            let _ = self.commit(Effects::default());
        }
    }

    /// Commits `effects`, with the program's text writes since its last
    /// commit: unless the worker was abandoned, applies their pure part to
    /// the program's copies, so what it reads after is what the reducer
    /// will hold (the text facts' changes were applied as they were made),
    /// and posts them for the reducer, which applies them only while the
    /// request is live. Returns what was done, and the caret key's watch a
    /// [`reducer::WatchChange::Take`] took out.
    fn commit(&self, effects: Effects) -> (Committed, Option<OpenWatch>) {
        let program = self.program();
        let effects = self.program_text.borrow_mut().seal(effects);
        if program.charge.is_dead() {
            drop(effects);
            return (Committed::Refused, None);
        }
        let local = effects.local();
        let applied = reducer::apply_local(
            &mut self.facts.borrow_mut(),
            program.request.generation,
            &local,
        );
        self.mailbox.post(Posted::Commit {
            request: program.request,
            effects: Box::new(effects),
        });
        (Committed::Applied, applied.taken)
    }
}

impl crate::arbitration::Judgement for RequestContext<'_> {
    fn judged_at(&self, arbitrator: &Arbitrator) -> u64 {
        self.judged_at.unwrap_or_else(|| arbitrator.observed_now())
    }

    fn verdicts(&self) -> Arc<Verdicts> {
        Arc::clone(&self.facts().verdicts)
    }

    /// Keeps `change` in the program's copies and posts it at once, as a
    /// message of its own, even from an abandoned worker: the reducer
    /// judges by its generation whether to record it, and a probe that
    /// timed out is recorded from any. A context that runs no program keeps
    /// it in its copies only.
    fn record(&self, change: VerdictChange) {
        let _ = Arc::make_mut(&mut self.facts.borrow_mut().verdicts).apply(&change);
        if let Some(program) = &self.program {
            self.mailbox.post(Posted::Verdict {
                generation: program.request.generation,
                change,
            });
        }
    }
}

impl std::ops::Deref for RequestContext<'_> {
    type Target = Context;

    fn deref(&self) -> &Context {
        self.context
    }
}

impl Context {
    /// The application this outpost watches; 0 before it has one.
    fn target_pid(&self) -> u32 {
        self.target_pid.load(Ordering::Acquire)
    }

    /// The name of the application `pid`, its executable's file name
    /// without the extension, lower-cased, as NVDA names an application
    /// module; empty when it cannot be read. Read once per process, then
    /// remembered while the process runs ([`ApplicationNames`]).
    fn application_name(&self, pid: u32) -> String {
        self.application_names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .name(pid)
    }

    /// [`Outpost::failed_reads`].
    fn failed_reads(&self) -> u32 {
        let callbacks = self
            .focus_events
            .get()
            .map_or(0, Registration::failed_reads);
        self.failed_reads
            .load(Ordering::Relaxed)
            .saturating_add(callbacks)
    }

    fn arbitrator(&self) -> MutexGuard<'_, Arbitrator> {
        self.arbitrator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The gap record's timer a test set, if one did.
    fn gap_timer(&self) -> Option<Arc<dyn GapTimer>> {
        self.gap_timer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Now on the clock the outpost stamps its own events' observations
    /// with ([`Outpost::set_observation_clock`]).
    fn observed_now(&self) -> u64 {
        self.observation.get().map_or_else(now_us, |clock| clock())
    }

    /// The outpost's clock ([`Outpost::set_clock`]), which its classic
    /// walks measure their time budgets by.
    fn clock(&self) -> verbatim_process::clock::Clock {
        self.arbitrator().clock()
    }

    /// The handle of the window with the keyboard focus, by the reader the
    /// outpost was given ([`Outpost::set_focus_window_reader`]).
    fn focus_window(&self) -> Option<isize> {
        let reader = Arc::clone(
            &self
                .focus_window
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        reader()
    }

    /// Why window `hwnd` cannot be read, `None` when it can, by the reader
    /// the outpost was given ([`Outpost::set_window_readability_reader`]).
    fn window_unreadable(&self, hwnd: isize) -> Option<&'static str> {
        let reader = Arc::clone(
            &self
                .window_readability
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        reader(hwnd)
    }

    /// The foreground window's handle, by the reader the outpost was given
    /// ([`Outpost::set_foreground_reader`]).
    fn foreground_window(&self) -> isize {
        let reader = Arc::clone(
            &self
                .foreground
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        reader()
    }

    /// Whether window `hwnd`'s top-level window belongs to a process
    /// Verbatim ignores. Local calls, made only while one is ignored.
    fn in_ignored_window(&self, hwnd: isize) -> bool {
        !self.ignored.is_empty() && {
            let (_, pid) =
                verbatim_win32::window::window_owner(verbatim_win32::window::top_level_of(hwnd));
            self.ignored.contains(verbatim_model::Pid(pid))
        }
    }

    /// Whether the store holds `node`: either registry keeps it.
    fn holds(&self, node: NodeId) -> bool {
        self.store.holds(node)
    }

    /// Sends `message` to Core without waiting, the nodes and anchors it
    /// names recorded as carried by it ([`store::Recorded`]).
    fn send(&self, message: OutpostToCore) {
        self.outbound.publish(self.store.record(message));
    }

    fn classic_windows(&self) -> MutexGuard<'_, HashMap<isize, u32>> {
        self.classic_windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The details the active theme wants read for each node.
    fn fetches(&self) -> Fetches {
        *self.fetches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The request a node is fetched with, for the details the active
    /// theme wants.
    fn node_request(&self, uia: &Uia) -> windows::core::Result<Request<Node>> {
        uia.request(crate::gates::node_gates(&self.fetches()))
    }

    /// The request an ancestor is fetched with, for the details the active
    /// theme wants.
    fn ancestor_request(&self, uia: &Uia) -> windows::core::Result<Request<Ancestor>> {
        uia.request(crate::gates::ancestor_gates(&self.fetches()))
    }

    /// Whether a UIA read for an element of `hwnd` should try a remote
    /// operation: the setting allows it and no import for the window has
    /// failed.
    fn tries_remote(&self, hwnd: Option<isize>) -> bool {
        self.remote_operations
            && hwnd.is_none_or(|hwnd| {
                self.classic_windows().get(&hwnd)
                    != Some(&verbatim_win32::window::window_thread(hwnd))
            })
    }

    /// Reads `hwnd`'s UIA elements the classic way from now on.
    fn read_classically(&self, hwnd: isize) {
        self.classic_windows()
            .insert(hwnd, verbatim_win32::window::window_thread(hwnd));
    }

    /// Forgets what was learned about a destroyed window, whose handle may
    /// be reused.
    fn forget_window(&self, hwnd: isize) {
        self.classic_windows().remove(&hwnd);
    }

    /// Releases every object the outpost holds, for its shutdown, once no
    /// worker runs: every node leaves the store, and its text
    /// patterns, what its text is known to support, its anchors, and its
    /// terminal memory go with it. Returns how many nodes there were.
    fn release_everything(&self) -> usize {
        let (nodes, objects) = self.store.release_everything();
        // Outside the store's lock: releasing an object can call into the
        // application.
        drop(objects);
        nodes
    }

    /// Appends `item` to the mailbox, from `source`.
    fn push(
        &self,
        item: Item,
        (trace, observed_at_us, timing): (TraceId, u64, EventTiming),
        source: Source,
    ) {
        let read_answer = read_answer(&item);
        self.mailbox.push(
            Entry {
                item,
                trace,
                observed_at_us,
                timing,
            },
            source,
            read_answer,
        );
    }

    /// Queues an event one of the outpost's own handlers took in, observed
    /// now, then tells the observer, if there is one, that it was heard.
    fn take_in(&self, item: Item, heard: Heard, timing: EventTiming, source: Source) {
        self.push(
            item,
            (TraceId::mint(), window::observed_us(&timing), timing),
            source,
        );
        self.tell_heard(heard);
    }

    /// Tells the observer, if there is one, that an event was heard.
    fn tell_heard(&self, heard: Heard) {
        if let Some(observer) = self.heard.get() {
            observer(heard);
        }
    }

    /// Core's backend override, forced on every window, or none
    /// ([`CoreToOutpost::SetBackendOverride`]): every window is
    /// decided anew, the event thread's too, which decides its hook again.
    fn set_backend_override(&self, backend_override: Option<Backend>) {
        self.arbitrator()
            .set_forced(backend_override.map(|backend| backend == Backend::Uia));
        self.override_changes.fetch_add(1, Ordering::Release);
        self.nudge_event_thread(narrowing::Nudge::Decide);
    }

    /// Nudges the event thread, once it runs ([`narrowing`]).
    fn nudge_event_thread(&self, nudge: narrowing::Nudge) {
        if let Some(nudger) = self.event_thread.get() {
            nudger.nudge(nudge.words());
        }
    }

    /// Takes in a `WinEvent` the outpost's own hook delivered. A focus,
    /// foreground change, menu opening, alert, or tooltip becomes the fact
    /// the listener forwards for one, held by the reducer while the
    /// listener forwards ([`reducer`], the hand-over's hold); one with no
    /// window is dropped. Any other event is queued as it is, unless its window is already known to be read through UIA:
    /// the worker would drop it at its window's UIA test, after acceptance
    /// and the window's state, which have no effect but a log line, so it
    /// is dropped here instead, and the worker is not woken for it. NVDA
    /// drops a `WinEvent` from a UIA window the same way, and a console
    /// flood raises them by the thousand. A console's update or caret
    /// event, the system caret's event, and a destroyed window are kept:
    /// the worker uses them for UIA windows too, though the intake drops
    /// the system caret's while the focus has caret events of its own
    /// (`Intake::drop_system_caret`). Whether a window is known to be read through UIA is
    /// asked of `windows`, the event thread's own record, so a flood from
    /// one window costs one read of its classes.
    fn take_in_win_event(&self, event: WinEvent, timing: EventTiming, windows: &mut EventWindows) {
        let WinEvent {
            kind,
            hwnd,
            id_object,
            id_child,
            thread,
            time,
            ..
        } = event;
        if !matches!(
            kind,
            WinEventKind::Focus
                | WinEventKind::Foreground
                | WinEventKind::MenuPopupStart
                | WinEventKind::Alert
                | WinEventKind::Show
        ) {
            // The system caret's events are a UIA focus's caret events
            // too, and are kept for the worker.
            if !matches!(
                kind,
                WinEventKind::ConsoleUpdate
                    | WinEventKind::ConsoleCaret
                    | WinEventKind::Destroy
                    | WinEventKind::Caret
            ) && windows.known_to_use_uia(
                (
                    &self.arbitrator,
                    || self.mailbox.verdicts(),
                    self.override_changes.load(Ordering::Acquire),
                ),
                (hwnd, verbatim_win32::window::window_thread(hwnd)),
                verbatim_win32::window::window_classes,
            ) {
                self.tell_heard(Heard::Msaa(kind));
                return;
            }
            self.take_in(
                Item::Msaa {
                    kind: worker::win_kind(kind),
                    hwnd,
                    id_object,
                    id_child,
                    raiser: thread,
                },
                Heard::Msaa(kind),
                timing,
                Source::Hook,
            );
            return;
        }
        let Some(fact) = verbatim_ia2::fact_of_win_event(kind, (hwnd, id_object, id_child)) else {
            return;
        };
        let stamp = WinEventStamp { thread, time };
        self.take_in(
            Item::Fact(fact, Some(stamp)),
            Heard::Msaa(kind),
            timing,
            Source::Hook,
        );
    }

    /// Queues a fact the supervisor routed from the listener, handled as it
    /// comes while the reducer holds the hook's copies ([`reducer`], the
    /// hand-over's hold). It is ordered by the listener's observation,
    /// `timing`'s `observed_at_us`.
    fn take_in_routed_fact(
        &self,
        fact: &DeliveredFact,
        win_event: Option<WinEventStamp>,
        trace: TraceId,
        timing: EventTiming,
    ) {
        let item = Item::Fact(fact.clone(), win_event);
        let stamps = (trace, timing.observed_at_us, timing);
        // The first UIA focus's arrival, stamped with the relay's own
        // stamp: one load per fact, and one store, once.
        if self.first_uia_focus_us.load(Ordering::Relaxed) == 0
            && matches!(
                fact,
                DeliveredFact::UiaFocusRead { focus: true, .. }
                    | DeliveredFact::UiaFocusDropped { focus: true, .. }
            )
        {
            self.first_uia_focus_us
                .store(timing.relayed_at_us, Ordering::Relaxed);
        }
        let source = if win_event.is_some() && fact.is_win_event() {
            Source::Listener
        } else {
            Source::Other
        };
        self.push(item, stamps, source);
        // The event thread takes the application's focus and foreground
        // windows from these until its own hook delivers them.
        match fact {
            DeliveredFact::MsaaFocus { hwnd, .. } | DeliveredFact::MenuPopup { hwnd, .. } => {
                self.nudge_event_thread(narrowing::Nudge::RoutedFocus(*hwnd));
            }
            DeliveredFact::Foreground { hwnd } => {
                self.nudge_event_thread(narrowing::Nudge::RoutedForeground(*hwnd));
            }
            _ => {}
        }
    }

    /// The route to the element `runtime_id` names in `path`, the focus
    /// path of a focus not yet committed, as [`focus_route`](Self::focus_route)
    /// finds it in the committed one.
    fn route_in(
        &self,
        path: &focus_path::FocusPath,
        runtime_id: &[i32],
    ) -> Option<focus_path::Route> {
        let route = path.route_to(runtime_id)?;
        read::serves(self, route.window).then_some(route)
    }
}

/// The outpost: owns the context and the event thread for one target
/// application, set once and fixed for the rest of the outpost's life.
pub struct Outpost {
    context: Arc<Context>,
    event_thread: EventThread,
    finisher: Finisher,
}

/// The thread that finishes the outpost's writes to Core that pend.
enum Finisher {
    /// A thread of the outbound sender's own, for an outpost made in a
    /// test's process ([`Outpost::new`]), which ends once the sender has
    /// written everything.
    Thread(JoinHandle<()>),
    /// The I/O thread [`run_pipe`] serves Core's pipes on.
    Io,
}

/// The priority of a thread that only finishes the outpost's writes to
/// Core, or reads Core's pipe and hands each command on, for microseconds
/// per wake (`docs/design/communication.md`, section 6.1).
const IO_PRIORITY: ThreadPriority = ThreadPriority::Highest;

impl Outpost {
    /// Creates an outpost watching `target_pid` for its whole life, with
    /// the default [`OutpostOptions`]; see [`Outpost::with_options`].
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources, or the `WinEvent` hook cannot be installed.
    #[must_use]
    pub fn new(pipe: OwnedHandle, target_pid: u32) -> Self {
        Self::with_options(pipe, target_pid, OutpostOptions::default())
    }

    /// Creates an outpost watching `target_pid` for its whole life, writing
    /// its messages to Core to `pipe`, an overlapped pipe end: starts the
    /// reducer's thread and the first worker, installs the MSAA hooks and the
    /// focus-following UIA subscription, and announces readiness. The
    /// thread that sends a message writes it; a thread of the sender's own
    /// finishes the writes that pend, as the I/O thread does in
    /// [`run_pipe`].
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources, `pipe` cannot be used for overlapped
    /// writes, or the `WinEvent` hook cannot be installed.
    #[must_use]
    pub fn with_options(pipe: OwnedHandle, target_pid: u32, options: OutpostOptions) -> Self {
        Self::with_focused_element_reader(pipe, target_pid, options, Arc::new(Uia::focused))
    }

    /// [`Outpost::with_options`], reading the element that has the keyboard
    /// focus for a focus-now query with `focused_element` rather than from
    /// the system: for a test that asks the outpost for the focus of an
    /// application that does not have the keyboard focus, and measures
    /// everything the outpost does with it.
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources, or the `WinEvent` hook cannot be installed.
    #[must_use]
    pub fn with_focused_element_reader(
        pipe: OwnedHandle,
        target_pid: u32,
        options: OutpostOptions,
        focused_element: FocusedElementReader,
    ) -> Self {
        let (sender, finishing) =
            Sender::start(pipe, Outbound::backlog(), "verbatim-outbound", || {
                prioritize_role("outpost outbound", IO_PRIORITY);
            })
            .expect("the outbound sender starts");
        let outpost = Self::build(
            Outbound::new(sender),
            Finisher::Thread(finishing),
            options,
            (focused_element, false),
        );
        if let Err(error) = outpost.assign(target_pid) {
            panic!("the outpost could not watch {target_pid}: {error}");
        }
        outpost
    }

    /// A spare outpost, with no application, in a test's own process
    /// (`docs/design/focus-pipeline.md`, section 19.2): it does everything
    /// of an outpost's start that needs no application, its worker's UIA
    /// client included, writes [`OutpostToCore::SpareReady`] to
    /// `pipe`, and watches the application [`Outpost::assign`] gives it
    /// from then on. It reads the keyboard focus for a focus-now query with
    /// `focused_element`, as [`Outpost::with_focused_element_reader`] does.
    ///
    /// # Panics
    ///
    /// Panics if a thread cannot be spawned, which means the process is out
    /// of OS thread resources, or `pipe` cannot be used for overlapped
    /// writes.
    #[must_use]
    pub fn spare(
        pipe: OwnedHandle,
        options: OutpostOptions,
        focused_element: FocusedElementReader,
    ) -> Self {
        let (sender, finishing) =
            Sender::start(pipe, Outbound::backlog(), "verbatim-outbound", || {
                prioritize_role("outpost outbound", IO_PRIORITY);
            })
            .expect("the outbound sender starts");
        let outpost = Self::build(
            Outbound::new(sender),
            Finisher::Thread(finishing),
            options,
            (focused_element, true),
        );
        outpost.context.outbound.urgent(Urgent::SpareReady {
            outpost_pid: Pid(std::process::id()),
        });
        outpost
    }

    /// The outpost, with no application yet ([`Outpost::assign`] gives it
    /// one), sending through `outbound`, whose writes that pend `finisher`
    /// finishes, and reading the keyboard focus with `focused_element`.
    /// When `warm` is set, its worker creates its UIA client at once
    /// rather than on its first read: a spare does, so the client is
    /// ready before its application's first focus
    /// (`docs/design/focus-pipeline.md`, section 19.2).
    fn build(
        outbound: Outbound,
        finisher: Finisher,
        options: OutpostOptions,
        (focused_element, warm): (FocusedElementReader, bool),
    ) -> Self {
        let context = Arc::new(Context {
            target_pid: AtomicU32::new(0),
            application_names: Mutex::default(),
            outbound,
            mailbox: Mailbox::default(),
            epoch: Instant::now(),
            gap_timer: Mutex::new(None),
            abandoned: AtomicUsize::new(0),
            shell: Mutex::new(None),
            store: store::Store::new(),
            arbitrator: Mutex::new(Arbitrator::new(&[])),
            observation: OnceLock::new(),
            ignored: verbatim_process::held::IgnoredProcesses::from_env(),
            bounded: verbatim_uia::BoundedClient::default(),
            focus_events: OnceLock::new(),
            remote_operations: options.remote_operations,
            classic_windows: Mutex::new(HashMap::new()),
            fetches: Mutex::new(Fetches::default()),
            focused_element,
            foreground: Mutex::new(Arc::new(verbatim_win32::window::foreground_window_handle)),
            focus_window: Mutex::new(Arc::new(verbatim_win32::window::focus_window)),
            window_readability: Mutex::new(Arc::new(
                verbatim_win32::window::event_window_unreadable,
            )),
            requested_events: acceptance::RequestedEvents::default(),
            heard: OnceLock::new(),
            failed_reads: AtomicU32::new(0),
            override_changes: AtomicU64::new(0),
            first_uia_focus_us: AtomicU64::new(0),
            event_thread: OnceLock::new(),
        });
        if let Some(registration) = register_focus_events(&context) {
            let _ = context.focus_events.set(registration);
        }

        let shell = Shell::start(&context, warm);
        *context.shell.lock().unwrap_or_else(PoisonError::into_inner) = Some(shell);

        // The MSAA hooks, process-scoped (decision D13), installed once the
        // outpost has its application: focus, foreground, menu opening,
        // alert, and tooltip events, which the listener forwards only until
        // it hears this hook is in (`docs/design/focus-pipeline.md`,
        // section 12.4), and value, state, name, selection, and destroy
        // events. A destroy is queued only for a window, the one kind of
        // object whose end the outpost tracks.
        let hook_context = Arc::clone(&context);
        let make_handlers: MakeHandlers = Arc::new(move || {
            // The event thread's own record of its windows and of the
            // application's focus and foreground windows, which only it
            // touches.
            let state = std::rc::Rc::new(RefCell::new(narrowing::EventThreadState::default()));
            let context = Arc::clone(&hook_context);
            let nudge_context = Arc::clone(&hook_context);
            let nudge_state = std::rc::Rc::clone(&state);
            EventHandlers {
                win_event: Box::new(move |event: WinEvent| {
                    let mut state = state.borrow_mut();
                    // A destroy is queued only for a window.
                    if event.kind == WinEventKind::Destroy
                        && (event.id_object
                            != windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW.0
                            || event.id_child != verbatim_ia2::CHILDID_SELF)
                    {
                        return;
                    }
                    take_in_hooked(
                        event,
                        &mut *state,
                        || context.observed_now(),
                        |state, event| {
                            if event.kind == WinEventKind::Destroy {
                                state.windows.forget(event.hwnd);
                            }
                            // Before the event is queued, so a focus moving
                            // into a window read through MSAA hooks its
                            // kinds again at once.
                            state.note_win_event(event, &context);
                        },
                        |state, event, timing| {
                            context.take_in_win_event(event, timing, &mut state.windows);
                        },
                    );
                }),
                nudge: Some(Box::new(move |word, hwnd| {
                    if let Some(nudge) = narrowing::Nudge::from_words(word, hwnd) {
                        nudge_state.borrow_mut().nudged(nudge, &nudge_context);
                    }
                })),
            }
        });
        let event_thread = EventThread::spawn(APP_SUBSCRIPTIONS, make_handlers);
        let _ = context.event_thread.set(event_thread.nudger());
        Self {
            context,
            event_thread,
            finisher,
        }
    }

    /// Gives the outpost its application, `target_pid`, for the rest of
    /// its life: the MSAA hand-over's clock is read, the process-scoped
    /// `WinEvent` hook installed, and [`OutpostToCore::Ready`] sent.
    /// Called once: by [`run_pipe`] at the start of an outpost started for
    /// an application, or for a spare's [`CoreToOutpost::Assign`];
    /// by the constructors of an outpost in a test's own process; and by a
    /// test for an [`Outpost::spare`].
    ///
    /// # Errors
    ///
    /// Returns why the outpost cannot watch the application: it already
    /// has one, or its hook could not be installed. The failure is also
    /// reported to Core ([`OutpostToCore::Fault`]), and no `Ready` is
    /// sent: the caller ends the outpost, since an application no hook
    /// watches would have its `WinEvent`s handed over to nothing
    /// (`docs/design/focus-pipeline.md`, section 19.8, item 5).
    pub fn assign(&self, target_pid: u32) -> Result<(), String> {
        let context = &self.context;
        if context
            .target_pid
            .compare_exchange(0, target_pid, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            let detail = format!("the outpost already watches {}", context.target_pid());
            tracing::error!(
                target_pid,
                detail,
                "the outpost cannot watch another application"
            );
            fault(context, detail.clone());
            return Err(detail);
        }
        // Before the hook is installed, so the reducer holds every copy the
        // hook takes until the listener's forwarding ends.
        context.mailbox.post(Posted::Hooked);
        if let Err(error) = self.event_thread.hook(target_pid) {
            let detail = format!("the WinEvent hook could not be installed: {error}");
            tracing::error!(
                target_pid,
                detail,
                "the outpost cannot watch its application"
            );
            fault(context, detail.clone());
            return Err(detail);
        }
        // A verdict the worker kept before the hook was in changed nothing.
        context.nudge_event_thread(narrowing::Nudge::Decide);
        tracing::info!(
            target_pid,
            image = %context.application_name(target_pid),
            "watching its application"
        );
        context.outbound.urgent(Urgent::Ready {
            outpost_pid: Pid(std::process::id()),
            target_pid: Pid(target_pid),
        });
        Ok(())
    }

    /// Waits until the reducer reaches a barrier behind everything queued
    /// before this call and the noted caret those entries left, with no
    /// program in flight, then until the focus-following subscriptions
    /// have made every move asked of them before it and every message
    /// published before it has been written to the pipe. A test that
    /// measures what the outpost's handling cost the application, or that
    /// asserts the outpost said nothing more, waits on this for its
    /// evidence. Unlike Core's idle round, it waits for no open caret
    /// watch, open gap record or held copies of the hook's, and it divides
    /// batches as the round does. Returns once the outpost shuts down.
    pub fn settle(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        self.context.mailbox.post(Posted::Settle(done_tx));
        // An error means the settle was dropped: the outpost shut down.
        // Every move and message before the answer was handed to its
        // thread before the shell answered, so these waits come after
        // them, on this thread rather than the reducer's.
        if done_rx.recv().is_ok() {
            if let Some(registration) = self.context.focus_events.get() {
                registration.settle();
            }
            self.context.outbound.flush();
        }
        // The hook changes the programs asked for are made too.
        if let Some(nudger) = self.context.event_thread.get() {
            nudger.sync();
        }
    }

    /// Shuts the outpost down cleanly ([`CoreToOutpost::Shutdown`]),
    /// on the calling thread, returning once it is done:
    ///
    /// 1. The intake closes: nothing new is taken, and what was waiting is
    ///    dropped.
    /// 2. The `WinEvent` hooks are removed, with the thread that pumped
    ///    them.
    /// 3. The focus-following UIA subscription is closed: the process's
    ///    registering thread removes each of its groups
    ///    (`RemoveEventHandlerGroup`), each removal waiting for callbacks
    ///    in progress; `RemoveAllEventHandlers` comes only at the process's
    ///    exit ([`run_pipe`]).
    /// 4. The worker finishes the entry in hand, and every abandoned worker
    ///    returns from its call, however long the application takes to
    ///    answer or UIA takes to end the call (its connection and
    ///    transaction timeouts). So does every thread of the client for
    ///    operations with a deadline of their own, abandoned or not. No
    ///    call is cut off.
    /// 5. Every object held is released: the registries' UIA elements and
    ///    MSAA objects, and the text patterns and ranges kept for text and
    ///    terminals.
    /// 6. What waits to be written to Core is written, every write in
    ///    flight completes, and the pipe is closed.
    /// 7. The calling thread leaves COM. The process's hold on the
    ///    multithreaded apartment is not given back here but at the
    ///    process's exit ([`run_pipe`]), since giving it back ends the
    ///    registering thread every outpost in the process shares.
    ///
    /// The calling thread must be in COM's multithreaded apartment, or able
    /// to join it, since it releases UIA objects. What happens to the
    /// process afterwards is the caller's: the outpost binary ends itself
    /// without running DLL detach code (`docs/architecture.md`, "Process
    /// lifetime").
    pub fn shutdown(self) {
        self.shut_down(None);
    }

    /// [`shutdown`](Self::shutdown), on the I/O thread `io` serves, which
    /// finishes the writes in flight.
    fn shut_down(self, io: Option<&mut ChildIo>) {
        let started = std::time::Instant::now();
        let joined = verbatim_uia::init_mta();
        let Self {
            context,
            event_thread,
            finisher,
        } = self;
        // The reducer takes nothing new, and stops once the program in
        // flight, if any, has ended.
        context.mailbox.post(Posted::Close);
        drop(event_thread);
        if let Some(registration) = context.focus_events.get() {
            registration.close();
        }
        let handlers_removed = started.elapsed();
        let workers = stop(&context);
        let bounded = context.bounded.close();
        let calls_finished = started.elapsed();
        let objects = context.release_everything();
        let failed_reads = context.failed_reads();
        verbatim_uia::release_thread_state();
        context.outbound.close();
        match (finisher, io) {
            (Finisher::Thread(finishing), _) => {
                let _ = finishing.join();
            }
            (Finisher::Io, Some(io)) => io.finish(context.outbound.sender()),
            (Finisher::Io, None) => {
                tracing::error!("no I/O thread finishes the outpost's writes");
            }
        }
        if joined.is_ok() {
            verbatim_uia::leave_mta();
        }
        tracing::info!(
            target_pid = context.target_pid(),
            handlers_removed_ms = handlers_removed.as_millis(),
            calls_finished_ms = calls_finished.as_millis(),
            elapsed_ms = started.elapsed().as_millis(),
            workers,
            bounded,
            objects,
            failed_reads,
            "the outpost shut down"
        );
    }

    /// The failed cached reads the outpost has made so far: its worker's,
    /// and its focus-following subscription's callbacks' on UIA's threads
    /// (`verbatim_uia::calls`). A read of a property or pattern its
    /// element's cache request did not hold fails inside UIA, which costs
    /// it an error raised and reported, so a test of a read path expects
    /// none.
    #[must_use]
    pub fn failed_reads(&self) -> u32 {
        self.context.failed_reads()
    }

    /// Reads the time for the time budgets of the outpost's classic walks
    /// from `clock` from now on, in place of the system's
    /// ([`Arbitrator::set_clock`]): a test that counts an operation's calls
    /// decides when a walk's time is up, however long its calls take.
    pub fn set_clock(&self, clock: verbatim_process::clock::Clock) {
        self.context.arbitrator().set_clock(clock);
    }

    /// Times the gap record's bound with `timer` from now on, in place of
    /// the outpost's threadpool timer: a test says when the bound is
    /// reached ([`gap_bound_reacher`](Self::gap_bound_reacher)) rather than
    /// waiting for it.
    pub fn set_gap_timer(&self, timer: Arc<dyn GapTimer>) {
        *self
            .context
            .gap_timer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(timer);
        self.context.mailbox.post(Posted::GapTimerGiven);
    }

    /// What reaches the gap record's bound, as the outpost's timer reaches
    /// it when it fires, from any thread: its entry is queued behind every
    /// marker queued before it. It does nothing once the outpost is gone.
    #[must_use]
    pub fn gap_bound_reacher(&self) -> Arc<dyn Fn() + Send + Sync> {
        let context = Arc::downgrade(&self.context);
        Arc::new(move || {
            if let Some(context) = context.upgrade() {
                context.mailbox.post(Posted::GapBound);
            }
        })
    }

    /// Stamps the observation of every event the outpost's own hook and
    /// subscriptions take in with `clock` from now on, in place of the
    /// run's shared clock, and times its verdicts' keeps by it: a test that
    /// counts an operation's calls says when a window's verdict of no UIA
    /// provider runs out and its probe is made again, by moving that time
    /// on, since a verdict is compared with the observation of the event it
    /// judges. Set once; a later call changes nothing.
    pub fn set_observation_clock(&self, clock: verbatim_process::clock::ObservationClock) {
        self.context
            .arbitrator()
            .set_observation_clock(Arc::clone(&clock));
        let _ = self.context.observation.set(clock);
    }

    /// Reads the foreground window with `reader` from now on, in place of
    /// the system's, when testing an event for acceptance against the
    /// foreground window, as NVDA's `shouldAcceptEvent` does, and when
    /// recording the window a focus was reported in, whose own object NVDA
    /// holds as the foreground and so never speaks a state change on as an
    /// ancestor: a test whose application runs on a desktop where no window
    /// can take the foreground says which window is the foreground.
    pub fn set_foreground_reader(&self, reader: ForegroundReader) {
        *self
            .context
            .foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = reader;
    }

    /// Reads the window with the keyboard focus with `reader` from now on,
    /// in place of the system's, when answering a focus-now query, whose
    /// focused element is read only in a window the application serves: a
    /// test whose application runs on a desktop where no window can take
    /// the keyboard focus says which window has it.
    pub fn set_focus_window_reader(&self, reader: FocusWindowReader) {
        *self
            .context
            .focus_window
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = reader;
    }

    /// Judges whether a window can be read with `reader` rather than by
    /// asking the system: for a test that has a window judged hung, which
    /// Windows does only after the application has not answered for five
    /// seconds.
    pub fn set_window_readability_reader(&self, reader: WindowReadabilityReader) {
        *self
            .context
            .window_readability
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = reader;
    }

    /// Tells the outpost that no focus listener forwards its application's
    /// `WinEvent`s to it: its own hook's copies are handled as they come,
    /// never held for the hand-over (`docs/design/ownership.md`), as for an outpost run alone
    /// ([`run_attach`]) or one a test hands facts to itself.
    pub fn without_listener(&self) {
        self.context.mailbox.post(Posted::WithoutListener);
    }

    /// Tells `observer` of every event the outpost's own UIA handlers and
    /// `WinEvent` hooks take in from the application, once it is queued for
    /// the worker. Events reach a client asynchronously, after the
    /// application's call that raised them has returned, so a test that has
    /// its application raise an event waits to hear it before it waits on
    /// [`Outpost::settle`], which then covers its handling. Only the first
    /// observer counts; later calls change nothing.
    pub fn observe_heard(&self, observer: HeardObserver) {
        let _ = self.context.heard.set(observer);
    }

    /// Handles one command from Core, on the I/O thread. Pings are
    /// answered here, the pong written at once unless a write is in flight;
    /// everything that reads the application is queued for the worker.
    #[deny(clippy::wildcard_enum_match_arm)]
    pub fn handle_command(&self, command: &CoreToOutpost) {
        let context = &self.context;
        match command {
            CoreToOutpost::SetBackendOverride { backend_override } => {
                context.set_backend_override(*backend_override);
            }
            CoreToOutpost::DeliverFact {
                trace_id,
                timing,
                fact,
                win_event,
            } => context.take_in_routed_fact(
                fact,
                *win_event,
                *trace_id,
                EventTiming {
                    relayed_at_us: now_us(),
                    ..*timing
                },
            ),
            CoreToOutpost::ForwardingEnded { through } => {
                context.mailbox.post(Posted::ForwardingEnded(*through));
            }
            CoreToOutpost::Query {
                trace_id,
                request_id,
                query,
                timing,
            } => context.push(
                Item::Query {
                    request_id: *request_id,
                    query: unstamped(query),
                },
                (
                    *trace_id,
                    now_us(),
                    // Only Core's relay stamps come with a request.
                    EventTiming {
                        relayed_at_us: now_us(),
                        core_received_at_us: timing.core_received_at_us,
                        core_relay_us: timing.core_relay_us,
                        core_sent_at_us: timing.core_sent_at_us,
                        ..EventTiming::default()
                    },
                ),
                Source::Other,
            ),
            CoreToOutpost::IdleQuery { round } => context.push(
                Item::Barrier {
                    asker: reducer::Asker::Round(*round),
                    behind_noted: false,
                },
                (TraceId::mint(), now_us(), EventTiming::default()),
                Source::Other,
            ),
            // The reducer withdraws the query if it has not dispatched it,
            // and answers it "not started" through its token.
            CoreToOutpost::Cancel { request_id } => {
                context.mailbox.post(Posted::Cancel(*request_id));
            }
            CoreToOutpost::Ping { seq, .. } => {
                context.outbound.urgent(Urgent::Pong {
                    seq: *seq,
                    parked_count: context.abandoned.load(Ordering::Relaxed),
                });
            }
            CoreToOutpost::Fetches(fetches) => {
                *context
                    .fetches
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = *fetches;
                context.store.set_fetches(*fetches);
            }
            // `run_pipe` ends its loop on `Shutdown` and calls `shutdown`,
            // which takes the outpost, and handles `Assign`, which ends the
            // outpost when it fails.
            CoreToOutpost::Shutdown | CoreToOutpost::Assign { .. } => {}
            CoreToOutpost::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            } => {
                context.push(
                    Item::NodesHeld {
                        nodes: nodes.clone(),
                        anchors: anchors.clone(),
                        acknowledged: *acknowledged,
                    },
                    (TraceId::mint(), now_us(), EventTiming::default()),
                    Source::Other,
                );
            }
        }
    }
}

/// Handles `event`, a `WinEvent` the outpost's own hook delivered: stamps
/// its observation from `clock` as the callback begins, before `note`, the
/// event thread's own bookkeeping, which can take the arbitrator's lock and
/// hook or unhook kinds when the narrowing changes, as it does when the
/// user switches applications; then takes it in with that stamp, as the
/// listener stamps its forwarded copy before its own reads.
fn take_in_hooked<S>(
    event: WinEvent,
    state: &mut S,
    clock: impl FnOnce() -> u64,
    note: impl FnOnce(&mut S, &WinEvent),
    take_in: impl FnOnce(&mut S, WinEvent, EventTiming),
) {
    let timing = EventTiming {
        raised_ms_ago: Some(event.raised_ms_ago),
        observed_at_us: clock(),
        ..EventTiming::default()
    };
    note(state, &event);
    take_in(state, event, timing);
}

#[cfg(test)]
mod hook_stamp_tests {
    use std::cell::Cell;

    use verbatim_ia2::{WinEvent, WinEventKind};

    use super::take_in_hooked;
    use verbatim_model::protocol::EventTiming;

    /// A hooked event is stamped when its callback began, not after the
    /// event thread's bookkeeping, however long that took. This exercises
    /// `take_in_hooked` alone: that the outpost's hook callback calls it,
    /// rather than stamping on its own, rests on reading the callback.
    #[test]
    fn a_hooked_event_is_stamped_before_its_bookkeeping() {
        let clock = Cell::new(7_000_000);
        let event = WinEvent {
            kind: WinEventKind::Focus,
            hwnd: 0x10,
            id_object: -4,
            id_child: 0,
            thread: 1,
            time: 2,
            raised_ms_ago: 3,
        };
        let mut taken = None::<EventTiming>;
        take_in_hooked(
            event,
            &mut taken,
            || clock.get(),
            // The bookkeeping takes 2 ms: hooking kinds again, say.
            |_, _| clock.set(7_002_000),
            |taken, _, timing| *taken = Some(timing),
        );
        let timing = taken.expect("the event is taken in");
        assert_eq!(
            (timing.observed_at_us, timing.raised_ms_ago),
            (7_000_000, Some(3))
        );
    }
}

/// A query with its node ids as this outpost issued them: they arrive
/// stamped with Core's outpost id.
fn unstamped(query: &Query) -> Query {
    match query {
        Query::Navigate { node_id, kind } => Query::Navigate {
            node_id: node_id.unstamped(),
            kind: *kind,
        },
        Query::Activate { node_id } => Query::Activate {
            node_id: node_id.unstamped(),
        },
        Query::Ancestors { node_id } => Query::Ancestors {
            node_id: node_id.unstamped(),
        },
        Query::Text { node_id, op } => Query::Text {
            node_id: node_id.unstamped(),
            op: op.clone(),
        },
        other => other.clone(),
    }
}

/// What a UIA callback captures of a property change on the focus: the
/// element's parts and window handle, read from its cache, and the element
/// for anything the worker must ask it. No call reaches the application.
fn capture_property(
    context: &Context,
    event: &Cached<PropertyEvent>,
    property_id: i32,
) -> UiaEvent {
    let fields = event.read();
    let parts = fields.parts(event.runtime_id());
    UiaEvent {
        kind: UiaKind::Property(property_id),
        parts: UiaSnapshotFact {
            runtime_id: parts.runtime_id,
            role: parts.role,
            name: parts.name,
            value: parts.value,
            states: parts.states,
            details: parts.details,
        },
        hwnd: fields.role.hwnd,
        focus_window: 0,
        element: event
            .agile()
            .ok()
            .map(|agile| context.store.park(store::Parked::Property(agile))),
    }
}

/// What a UIA callback captures of a caret, text, or active text position
/// change: the element's runtime id, which is all its handling reads of
/// it, and its window handle, read from its cache, and the element. No
/// call reaches the application.
fn capture_text(context: &Context, event: &Cached<TextEvent>, kind: UiaKind) -> UiaEvent {
    UiaEvent {
        kind,
        parts: UiaSnapshotFact {
            runtime_id: event.runtime_id(),
            role: verbatim_model::Role::Unknown,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        hwnd: event.read().hwnd,
        focus_window: 0,
        element: event
            .agile()
            .ok()
            .map(|agile| context.store.park(store::Parked::Text(agile))),
    }
}

/// Starts the focus-following UIA subscription
/// (`docs/design/focus-pipeline.md`, section 17): one registration, on one thread and client, listening
/// nowhere until the worker reports a focus whose element it has. Its two
/// groups are NVDA's two local groups (`UIAHandler/__init__.py`,
/// `_createLocalEventHandlerGroup`): the base group, given every UIA focus,
/// holds the name, value, and state changes ([`FOCUS_PROPERTIES`]) and the
/// caret event (`Text_TextSelectionChanged`); the extended group, given a
/// focus with text read through UIA, also holds the text changes
/// (`Text_TextChanged`) and the active text position changes. A third
/// group, given only the parent of a focus that is a `SensitiveSlider`'s
/// thumb, holds its value changes alone. Selections and notifications come
/// desktop-wide from the listener instead. A caret or text change also has
/// the worker check a caret key's watch.
///
/// One group carries a cache request per handler, so each event's element
/// arrives with what its handling reads: a property change with what the
/// focus's name, value, and states are read from
/// ([`verbatim_uia::PropertyEvent`]), and a caret, text, or active text
/// position change with little more than its window
/// ([`verbatim_uia::TextEvent`]), since the worker reads the text itself.
fn register_focus_events(context: &Arc<Context>) -> Option<Registration> {
    let property_context = Arc::clone(context);
    let property_callback: verbatim_uia::PropertyCallback<Cached<PropertyEvent>> =
        Arc::new(move |event: &Cached<PropertyEvent>, property_id: i32| {
            property_context.take_in(
                Item::Uia(capture_property(&property_context, event, property_id)),
                Heard::UiaProperty(property_id),
                EventTiming {
                    observed_at_us: property_context.observed_now(),
                    ..EventTiming::default()
                },
                Source::Other,
            );
        });
    let properties = Subscription::Properties {
        properties: FOCUS_PROPERTIES.to_vec(),
        callback: Arc::clone(&property_callback),
    };
    let values = Subscription::Properties {
        properties: vec![UIA_ValueValuePropertyId, UIA_RangeValueValuePropertyId],
        callback: property_callback,
    };
    let text_context = Arc::clone(context);
    let text_callback: verbatim_uia::EventCallback<Cached<TextEvent>> =
        Arc::new(move |event: &Cached<TextEvent>, event_id: i32| {
            let (kind, heard) = if event_id == UIA_Text_TextSelectionChangedEventId.0 {
                (UiaKind::TextSelection, Heard::UiaTextSelection)
            } else {
                (UiaKind::TextChanged, Heard::UiaTextChanged)
            };
            text_context.take_in(
                Item::Uia(capture_text(&text_context, event, kind)),
                heard,
                EventTiming {
                    observed_at_us: text_context.observed_now(),
                    ..EventTiming::default()
                },
                Source::Other,
            );
        });
    let caret = Subscription::Events {
        events: vec![UIA_Text_TextSelectionChangedEventId],
        callback: Arc::clone(&text_callback),
    };
    let text = Subscription::Events {
        events: vec![UIA_Text_TextChangedEventId],
        callback: text_callback,
    };
    let position_context = Arc::clone(context);
    let position = Subscription::ActiveTextPosition {
        callback: Arc::new(
            move |event: &Cached<TextEvent>, range: Option<&IUIAutomationTextRange>| {
                let range = range
                    .and_then(|range| AgileReference::new(range).ok())
                    .map(|range| position_context.store.park(store::Parked::Range(range)));
                position_context.take_in(
                    Item::Uia(capture_text(
                        &position_context,
                        event,
                        UiaKind::ActiveTextPosition(range),
                    )),
                    Heard::UiaActiveTextPosition,
                    EventTiming {
                        observed_at_us: position_context.observed_now(),
                        ..EventTiming::default()
                    },
                    Source::Other,
                );
            },
        ),
    };
    let parts = vec![
        Part::shaped::<PropertyEvent>(properties),
        Part::shaped::<PropertyEvent>(values).value_only(),
        Part::shaped::<TextEvent>(caret).text_events(),
        Part::shaped::<TextEvent>(text).extended().text_events(),
        Part::shaped::<TextEvent>(position).extended(),
    ];
    match Registration::with_parts(parts, Scope::Nothing) {
        Ok(registration) => Some(registration),
        Err(error) => {
            fault(
                context,
                format!("UIA focus-following subscription failed: {error}"),
            );
            None
        }
    }
}

/// What a focus's read in `item` answers of the gap record: a copy in the
/// top-level window of its own window, else of its nearest window, or in
/// any window when it has neither; `None` for anything else, a menu
/// opening's read among them. One local call, `GetAncestor`, made as the
/// read is pushed, so the reducer makes none.
fn read_answer(item: &Item) -> Option<GapAnswer> {
    let Item::Fact(
        DeliveredFact::UiaFocusRead {
            read, focus: true, ..
        },
        _,
    ) = item
    else {
        return None;
    };
    let window = if read.hwnd != 0 {
        read.hwnd
    } else {
        read.window.unwrap_or(0)
    };
    let top = (window != 0).then(|| verbatim_win32::window::top_level_of(window));
    Some(GapAnswer::Answers(top.filter(|&top| top != 0)))
}

/// Stops the reducer and the workers for the outpost's shutdown, once the
/// mailbox is closed: the reducer's thread ends once the program in flight
/// has ended, with no worker abandoned meanwhile, and every worker thread
/// is waited for, the one in charge and each abandoned one returning from
/// its call, however long UIA takes to end a call the application does not
/// answer. Returns how many worker threads were waited for.
fn stop(context: &Context) -> usize {
    let shell = context
        .shell
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let Some(shell) = shell else {
        return 0;
    };
    let workers = shell.join().unwrap_or_default();
    let count = workers.len();
    for worker in workers {
        let _ = worker.join();
    }
    count
}

fn fault(context: &Context, detail: String) {
    context.send(OutpostToCore::Fault { detail });
}

/// Runs an outpost driven by the Core pipes, watching `target_pid` for its
/// whole life, or, for `None`, as a spare (`docs/design/focus-pipeline.md`,
/// section 19.2): it sends [`OutpostToCore::SpareReady`] and watches
/// the application Core's [`CoreToOutpost::Assign`] gives it from
/// then on. It runs on the transport's child side
/// (`docs/design/communication.md`, sections 4.6 and 6.4): the calling
/// thread becomes the outpost's I/O thread, which reads commands from
/// `pipe_in` and finishes the writes to `pipe_out` that pend, while every
/// thread that sends Core a message writes it itself. It runs until Core
/// asks the outpost to shut down ([`CoreToOutpost::Shutdown`]) or
/// the command stream ends, or its application cannot be watched
/// ([`Outpost::assign`]), then shuts it down cleanly
/// ([`Outpost::shutdown`]), waiting for every write in flight, and
/// returns. When the target application exits, Core is told
/// ([`OutpostToCore::TargetExited`]) and asks for the shutdown. Both
/// ends must be overlapped.
///
/// # Errors
///
/// Returns the error of setting up the pipes, any I/O error reading the
/// command stream, or why the application cannot be watched, after the
/// shutdown.
pub fn run_pipe(
    pipe_in: OwnedHandle,
    pipe_out: OwnedHandle,
    target_pid: Option<u32>,
    options: OutpostOptions,
) -> io::Result<()> {
    let (mut io, sender) = ChildIo::new(pipe_in, pipe_out, Outbound::backlog())?;
    let outpost = Outpost::build(
        Outbound::new(sender.clone()),
        Finisher::Io,
        options,
        (Arc::new(Uia::focused), target_pid.is_none()),
    );
    // Why the outpost cannot watch its application, if it cannot.
    let mut unwatched = None;
    if let Some(target_pid) = target_pid {
        match outpost.assign(target_pid) {
            Ok(()) => watch_target(&outpost.context),
            Err(error) => unwatched = Some(error),
        }
    } else {
        tracing::info!("a spare, waiting for its application");
        outpost.context.outbound.urgent(Urgent::SpareReady {
            outpost_pid: Pid(std::process::id()),
        });
    }
    // This thread is the outpost's I/O thread from here on.
    prioritize_role("outpost I/O", IO_PRIORITY);
    let served = if unwatched.is_some() {
        Ok(Served::Stopped)
    } else {
        io.serve(&sender, |body| {
            match decode_message::<CoreToOutpost>(body)? {
                CoreToOutpost::Shutdown => return Ok(Serving::Stop),
                CoreToOutpost::Assign { target_pid } => match outpost.assign(target_pid.0) {
                    Ok(()) => watch_target(&outpost.context),
                    Err(error) => {
                        unwatched = Some(error);
                        return Ok(Serving::Stop);
                    }
                },
                command => outpost.handle_command(&command),
            }
            Ok(Serving::Continue)
        })
    };
    let ended = match (served, unwatched) {
        (_, Some(error)) => {
            tracing::error!(error, "shutting down: the application cannot be watched");
            Err(io::Error::other(error))
        }
        (Ok(Served::Stopped), None) => {
            tracing::info!("shutting down, as Core asked");
            Ok(())
        }
        (Ok(Served::CommandsEnded), None) => {
            tracing::warn!("Core's command pipe closed; shutting down");
            Ok(())
        }
        (Err(error), None) => Err(error),
    };
    outpost.shut_down(Some(&mut io));
    // The process has finished with UIA: only its exit gives back the
    // process-wide hold, which ends the registering thread every
    // registration in the process shares.
    verbatim_uia::release_mta_usage();
    ended
}

/// Tells Core when the target application exits, from a thread of its own
/// that waits on the application's process. The thread is not waited for:
/// it holds nothing of COM's and ends with the process.
fn watch_target(context: &Arc<Context>) {
    let context = Arc::clone(context);
    let spawned = thread::Builder::new()
        .name("verbatim-target-watch".to_owned())
        .spawn(move || {
            if window::wait_for_process_exit(context.target_pid()) {
                tracing::info!(
                    target_pid = context.target_pid(),
                    "the target application exited"
                );
                context.outbound.urgent(Urgent::TargetExited);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the target application's exit is not watched");
    }
}

/// Runs an outpost in dev-attach mode: prints each outbound message's JSON
/// as a line on stdout, watches the given target for its whole life, asks
/// for the current focus, and streams events until the process is killed.
///
/// # Errors
///
/// Returns an I/O error only if dev-mode setup fails before the event loop
/// begins; once running it blocks until the process is terminated.
pub fn run_attach(target_pid: u32, options: OutpostOptions) -> io::Result<()> {
    let (printed, written) = pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped)?;
    let mut printed = io::BufReader::new(BlockingReader::new(printed)?);
    thread::Builder::new()
        .name("verbatim-attach-printer".to_owned())
        .spawn(move || {
            use std::io::Write as _;
            // Each message is printed as the model's JSON, a terminal's
            // text in it rather than in the frame's raw section.
            while let Ok(Some(message)) = read_message::<_, OutpostToCore>(&mut printed) {
                let Ok(json) = serde_json::to_vec(&message) else {
                    continue;
                };
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(&json);
                let _ = stdout.write_all(b"\n");
                let _ = stdout.flush();
            }
        })?;
    let outpost = Outpost::with_options(written, target_pid, options);
    // No listener forwards anything here: the outpost's own hook takes in
    // every `WinEvent`.
    outpost.context.mailbox.post(Posted::WithoutListener);
    outpost.handle_command(&CoreToOutpost::Query {
        trace_id: TraceId::mint(),
        request_id: 0,
        query: Query::FocusNow,
        timing: EventTiming::default(),
    });
    loop {
        thread::park();
    }
}

#[cfg(test)]
mod shutdown_tests {
    use std::io::BufReader;

    use verbatim_model::protocol::OutpostToCore;
    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::{BlockingReader, EVENT_PIPE_BUFFER};

    use super::Outpost;
    use verbatim_process::message::read_message;

    /// An outpost of this process watching this process, its messages read
    /// and dropped on a thread of their own, so its writes complete.
    fn outpost() -> Outpost {
        let (ours, theirs) =
            pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped).expect("a pipe");
        std::thread::spawn(move || {
            let mut reader = BufReader::new(BlockingReader::new(ours).expect("a reader"));
            while let Ok(Some(_)) = read_message::<_, OutpostToCore>(&mut reader) {}
        });
        Outpost::new(theirs, std::process::id())
    }

    /// One outpost's shutdown leaves the process's UIA registering thread,
    /// which every outpost in the process registers on, to the process's
    /// exit: ending it would silence another outpost's subscription.
    #[test]
    fn an_outposts_shutdown_leaves_the_registering_thread_running() {
        let staying = outpost();
        let leaving = outpost();
        assert!(staying.context.focus_events.get().is_some());
        leaving.shutdown();
        assert!(verbatim_uia::registering_thread_runs());
        staying.shutdown();
    }
}
