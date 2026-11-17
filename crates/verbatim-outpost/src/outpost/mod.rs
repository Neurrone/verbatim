//! The per-application outpost (architecture section 1, "Outposts";
//! decision D9). One outpost watches one application for its whole life:
//! the application is set once, when the outpost starts, or, for a spare
//! started before it had one, when the supervisor assigns it
//! (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"), and never changes after.
//! Its parts:
//!
//! 1. The mailbox ([`shell`]): the MSAA hook callbacks, the UIA
//!    subscription callbacks, and Core's commands only append an entry to
//!    it and return. They never call into the
//!    application and never wait on anything but the mailbox's lock.
//! 2. The reducer ([`reducer`]), on a thread of its own run by its shell
//!    ([`shell`]): the pure state machine that holds every remembered fact
//!    of the application, the intake's order with NVDA's limiter rules
//!    among them, Core's requests waiting, and the timers of a caret key's
//!    watch.
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
//!    flight keep their order.
//!
//! Workers run in COM's multithreaded apartment; the object store
//! ([`store`]) keeps agile references, so a replacement worker can use what
//! an abandoned one minted.
//!
//! Held objects (`books/dev/src/reference/crates/verbatim-outpost.md`): the store keeps the
//! live UIA element or MSAA object behind every node the outpost reports.
//! Each message that carries node ids has a position, counted the same way
//! by Core; when Core reports the nodes it still holds and the position of
//! the last message it has handled, the worker releases every other node
//! no live request can still name, reported at or before that position or
//! never reported. A node reported later, or named by a request still
//! running, is kept.

mod asked;
mod commit;
mod completion;
mod focus_path;
mod followed;
pub(crate) mod lock;
mod outbound;
mod read;
mod shell;
mod shell_items;
mod store;
pub(crate) mod text_reads;
pub(crate) mod window;
mod worker;

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use lock::{Lock, LockGuard};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::UI::Accessibility::{
    IUIAutomationTextRange, UIA_RangeValueValuePropertyId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId, UIA_ValueValuePropertyId,
};

use verbatim_ia2::{CONSOLE_SUBSCRIPTIONS, WinEvent};
use verbatim_model::property::NodeDetailSet;
use verbatim_model::{NodeId, Pid, TraceId};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::scheduling::{ThreadPriority, prioritize_role};
use verbatim_process::transport::{
    BlockingReader, ChildIo, EVENT_PIPE_BUFFER, Sender, Served, Serving,
};
use verbatim_uia::{
    Ancestor, Cached, FOCUS_PROPERTIES, Node, Part, PropertyEvent, Registration, Request, Scope,
    Subscription, TextEvent, Uia,
};

use crate::arbitration::Arbitrator;
use verbatim_ia2::event_thread::{EventHandlers, EventThread, MakeHandlers, Nudger};
use verbatim_model::protocol::{
    CoreToOutpost, EventTiming, OutpostToCore, UiaSnapshotFact, Urgent,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::clock::now_us;
use verbatim_process::held::Target;
use verbatim_process::message::{decode_message, read_message};
use verbatim_win32::window::{ForegroundReader, WindowReadabilityReader};
use verbatim_win32::window_facts::{SystemReads, WindowReads};

use verbatim_outpost_logic as reducer;

use commit::{Committed, Effects};
use outbound::Outbound;
pub use reducer::Verdicts;
pub(crate) use reducer::{Anchor, NodeAnchors, Probe, Probed, ReadSelection, VerdictChange};
use reducer::{Entry, Facts, Item, UiaEvent, UiaKind};
use shell::{Mailbox, Posted, Shell};
use worker::Charge;

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
    application_names: Lock<ApplicationNames>,
    outbound: Outbound,
    /// What every producer appends to and the reducer's shell drains
    /// ([`shell`]).
    mailbox: Mailbox,
    /// The outpost's start, from which the reducer's clock counts.
    epoch: Instant,
    /// The abandoned workers that have not yet returned, as the pong
    /// reports them. The lock is held only to assign or copy the value.
    parked: Lock<usize>,
    /// The reducer's thread, which returns every worker thread started.
    shell: Lock<Option<JoinHandle<Vec<JoinHandle<()>>>>>,
    /// Every application object the outpost keeps, and the identity it
    /// gives each ([`store`]).
    store: store::Store,
    arbitrator: Lock<Arbitrator>,
    /// The clock the outpost's own hook and subscriptions stamp their
    /// events' observations with, and its arbitrator its verdicts' keeps
    /// by: the run's shared clock, unless a test sets its own
    /// ([`Outpost::set_observation_clock`]).
    observation: OnceLock<verbatim_process::clock::ObservationClock>,
    /// The focus-following UIA subscription, one event handler group in
    /// two forms, which the worker moves with the focus
    /// ([`register_focus_events`]).
    focus_events: OnceLock<Registration>,
    /// What Core's newest `Follow` named, which the registration keeps
    /// ([`followed`]); set on the worker.
    named: Lock<Option<followed::Named>>,
    /// Whether UIA reads may use remote operations ([`OutpostOptions`]).
    remote_operations: bool,
    /// Windows whose UIA elements could not be imported into a remote
    /// operation (client-side proxies), read the classic way for the
    /// window's lifetime, since a window's provider does not change. Each
    /// is kept with the thread that owned it, so a reused handle, whose
    /// destroy event was lost, is not read the classic way by mistake.
    classic_windows: Lock<HashMap<isize, u32>>,
    /// How the worker reads the element that has the keyboard focus, for
    /// Core's request for the current focus.
    focused_element: FocusedElementReader,
    /// How the worker reads the foreground window when it records the
    /// window a focus was reported in ([`Outpost::set_foreground_reader`]).
    foreground: Lock<ForegroundReader>,
    /// How Core's request for the current focus reads the window with the
    /// keyboard focus
    /// ([`Outpost::set_focus_window_reader`]).
    focus_window: Lock<FocusWindowReader>,
    /// How the worker judges whether a window can be read, or is no
    /// window, a ghost's or a hung application's
    /// ([`Outpost::set_window_readability_reader`]).
    window_readability: Lock<WindowReadabilityReader>,
    /// Who is told of each event the outpost's own handlers take in
    /// ([`Outpost::observe_heard`]), if anyone.
    heard: OnceLock<HeardObserver>,
    /// Who is told of the calls each of Core's requests made
    /// ([`Outpost::observe_request_calls`]), if anyone.
    request_calls: OnceLock<RequestCallsObserver>,
    /// Who is told, on the worker, of each foreground change a program is
    /// about to handle ([`Outpost::observe_foreground_programs`]), if
    /// anyone.
    foreground_programs: OnceLock<ForegroundProgramObserver>,
    /// The failed cached reads the worker has made (`verbatim_uia::calls`),
    /// taken after each entry.
    failed_reads: AtomicU32,
    /// What nudges the event thread and installs its hook; set once the
    /// thread runs.
    event_thread: OnceLock<Nudger>,
    /// Whether the event thread's console hook was asked for
    /// ([`Context::hook_console`]).
    console_hooked: std::sync::atomic::AtomicBool,
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
    /// A text focus's active text position changed event, which the
    /// outpost hears and drops: Core has no handler for it.
    UiaActiveTextPosition,
    /// A `WinEvent` of one of the kinds the outpost hooks.
    Msaa(verbatim_ia2::WinEventKind),
}

/// Told of each event an outpost takes in ([`Outpost::observe_heard`]).
pub type HeardObserver = Arc<dyn Fn(Heard) + Send + Sync>;

/// Told, on the worker, of the calls into the application each of Core's
/// requests made, by its number, before its answer is sent
/// ([`Outpost::observe_request_calls`]): once for a request answered by one
/// program, once for each program of a caret key's watch.
pub type RequestCallsObserver =
    Arc<dyn Fn(verbatim_model::request::RequestNumber, verbatim_model::CallCounts) + Send + Sync>;

/// Told, on the worker, of the window of each foreground change a program
/// is about to handle ([`Outpost::observe_foreground_programs`]).
pub type ForegroundProgramObserver = Arc<dyn Fn(isize) + Send + Sync>;

/// Reads the handle of the window with the keyboard focus, `None` when
/// there is none. An outpost reads the system's (`GetGUIThreadInfo`); a
/// test whose application cannot take the keyboard focus on the desktop it
/// runs on supplies its own ([`Outpost::set_focus_window_reader`]).
pub type FocusWindowReader = Arc<dyn Fn() -> Option<isize> + Send + Sync>;

/// Reads the UIA element that has the keyboard focus, filled by the given
/// request, with the given client, on the worker's thread, for Core's
/// request for the current focus, the one read of the focused element the
/// outpost makes. An outpost
/// reads the system's keyboard focus ([`Uia::focused`]); a test
/// that drives a real outpost against an application without taking the
/// keyboard focus from the desktop it runs on supplies its own
/// ([`Outpost::with_focused_element_reader`]).
pub type FocusedElementReader =
    Arc<dyn Fn(&Uia, &Request<Node>) -> windows::core::Result<Cached<Node>> + Send + Sync>;

/// The outpost's shared state as one program sees it: the store's handle
/// for the request ([`store::Nodes`]), carrying its mint tag, and its
/// copies of the facts it reads. The store tags every
/// node the request issues or looks up with it, and a program's text
/// writes are made only with a commit the reducer accepts.
struct RequestContext<'a> {
    context: &'a Context,
    nodes: store::Nodes<'a>,
    /// The facts the program reads: its copies, which its commits are
    /// applied to.
    facts: RefCell<Facts>,
    /// What the program holds of the store's text, and its text writes
    /// not yet committed ([`text_reads::ProgramText`]).
    program_text: RefCell<text_reads::ProgramText>,
    /// The program's own: its request, the caret events waiting when it
    /// was dispatched, and its worker's charge.
    program: Program<'a>,
    /// The nodes the program's walks took from the outpost's ancestry
    /// cache, the previous chain, above their cache join
    /// ([`read::join_cache`]): the report's cached ancestors, from which its
    /// `first_cached_ancestor` is computed as it is emitted.
    joined_cache: RefCell<Vec<NodeId>>,
    /// The observation its verdicts are judged at: its entry's event's, or,
    /// when `None`, now on the arbitrator's observation clock at each
    /// judgement (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact").
    judged_at: Option<u64>,
    /// Core's verdict for the window its request names, which the request
    /// carries (`docs/design/readers-tell-core-asks-build1.md`, Table 4):
    /// every judgement of that window while the request is answered takes
    /// it, with no probe ([`read::window_uses_uia`]).
    given_verdict: Cell<Option<(isize, bool)>>,
}

/// What a program's context holds of it ([`RequestContext::program`]).
struct Program<'a> {
    request: reducer::Request,
    charge: Arc<Charge>,
    _context: std::marker::PhantomData<&'a ()>,
}

impl<'a> RequestContext<'a> {
    /// The context of program `request`, given `facts`, `waiting`, and its
    /// worker's `charge`.
    fn for_program(
        context: &'a Context,
        (tag, details): (store::Tag, NodeDetailSet),
        (request, facts): (reducer::Request, Facts),
        charge: Arc<Charge>,
    ) -> Self {
        Self {
            context,
            nodes: context.store.nodes_for(tag, details),
            facts: RefCell::new(facts),
            program_text: RefCell::default(),
            joined_cache: RefCell::default(),
            program: Program {
                request,
                charge,
                _context: std::marker::PhantomData,
            },
            judged_at: None,
            given_verdict: Cell::new(None),
        }
    }

    /// Notes `nodes`, entries a walk took from the ancestry cache above its
    /// cache join, as cached ancestors of the report the program makes.
    fn note_cached(&self, nodes: impl IntoIterator<Item = NodeId>) {
        self.joined_cache.borrow_mut().extend(nodes);
    }

    /// Judges every verdict the program asks for at the observation
    /// `at_us`.
    fn judge_at(&mut self, at_us: u64) {
        self.judged_at = Some(at_us);
    }

    /// The program this context runs.
    fn program(&self) -> &Program<'a> {
        &self.program
    }

    /// The store's handle for this request, through which walks issue and
    /// look up nodes.
    fn nodes(&self) -> &store::Nodes<'a> {
        &self.nodes
    }

    /// The node details the program's reads fetch, as its request names
    /// them.
    fn node_details(&self) -> NodeDetailSet {
        verbatim_ia2::Interning::details(&self.nodes)
    }

    /// The request a node is fetched with, for the details the program's
    /// request names.
    fn node_request(&self, uia: &Uia) -> windows::core::Result<Request<Node>> {
        uia.request(crate::gates::node_gates(self.node_details()))
    }

    /// The request a review command's live read of a node
    /// (`Query::ReadNode`) is made with: a node read alone, with its table's
    /// row and column counts.
    fn read_node_request(&self, uia: &Uia) -> windows::core::Result<Request<Node>> {
        uia.request(verbatim_uia::NodeGates {
            counts: Some(()),
            ..crate::gates::node_gates(self.node_details())
        })
    }

    /// The request an ancestor is fetched with, for the details the
    /// program's request names.
    fn ancestor_request(&self, uia: &Uia) -> windows::core::Result<Request<Ancestor>> {
        uia.request(crate::gates::ancestor_gates(self.node_details()))
    }

    /// The program's worker's charge.
    fn charge(&self) -> &Charge {
        &self.program().charge
    }

    /// The facts as the program's copies hold them.
    fn facts(&self) -> Ref<'_, Facts> {
        self.facts.borrow()
    }

    /// The route to the element `runtime_id` names, kept with the node a
    /// listener's read minted, when its window is one the outpost's
    /// application serves: the outpost looks up no window of another
    /// process by its handle
    /// (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.3).
    fn focus_route(&self, runtime_id: &[i32]) -> Option<focus_path::Route> {
        let route = self.store.route_to(runtime_id)?;
        read::serves(self, route.window.get()).then_some(route)
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
        if self.program_text.borrow().has_pending() {
            let _ = self.commit(Effects::default());
        }
    }

    /// Commits `effects`, with the program's text writes since its last
    /// commit, their text facts' changes already applied to the program's
    /// copies as they were made: unless the worker was abandoned, posts
    /// them for the reducer, which applies them only while the request is
    /// live. Returns what was done.
    fn commit(&self, effects: Effects) -> Committed {
        let program = self.program();
        let effects = self.program_text.borrow_mut().seal(effects);
        if program.charge.is_dead() {
            drop(effects);
            return Committed::Refused;
        }
        self.mailbox.post(Posted::Commit {
            request: program.request,
            effects: Box::new(effects),
        });
        Committed::Applied
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
    /// timed out is recorded from any.
    fn record(&self, change: VerdictChange) {
        let _ = Arc::make_mut(&mut self.facts.borrow_mut().verdicts).apply(&change);
        self.mailbox.post(Posted::Verdict {
            generation: self.program.request.generation,
            change,
        });
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
        self.application_names.lock().name(pid)
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

    #[track_caller]
    fn arbitrator(&self) -> LockGuard<'_, Arbitrator> {
        self.arbitrator.lock()
    }

    /// Now on the clock the outpost stamps its own events' observations
    /// with ([`Outpost::set_observation_clock`]).
    fn observed_now(&self) -> u64 {
        self.observation.get().map_or_else(now_us, |clock| clock())
    }

    /// The handle of the window with the keyboard focus, by the reader the
    /// outpost was given ([`Outpost::set_focus_window_reader`]).
    fn focus_window(&self) -> Option<isize> {
        let reader = Arc::clone(&self.focus_window.lock());
        reader()
    }

    /// Whether window `hwnd` can be read, by the reader the outpost was
    /// given ([`Outpost::set_window_readability_reader`]): the outpost's
    /// one judge of whether a window is hung, asked at each decision.
    fn readability(&self, hwnd: isize) -> verbatim_win32::window::Readability {
        let reader = Arc::clone(&self.window_readability.lock());
        reader(hwnd)
    }

    /// The answer to a ping.
    fn pong(&self) -> Urgent {
        Urgent::Pong {
            parked_count: *self.parked.lock(),
            // The outpost judges no hang.
            parked_on_hung: 0,
        }
    }

    /// Why window `hwnd` cannot be read, `None` when it can
    /// ([`readability`](Self::readability)).
    fn window_unreadable(&self, hwnd: isize) -> Option<&'static str> {
        self.readability(hwnd).reason()
    }

    /// The foreground window's handle, by the reader the outpost was given
    /// ([`Outpost::set_foreground_reader`]).
    fn foreground_window(&self) -> isize {
        let reader = Arc::clone(&self.foreground.lock());
        reader()
    }

    /// Sends `message` to Core without waiting, the nodes and anchors it
    /// names recorded as carried by it ([`store::Recorded`]).
    fn send(&self, message: OutpostToCore) {
        self.outbound.publish(self.store.record(message));
    }

    #[track_caller]
    fn classic_windows(&self) -> LockGuard<'_, HashMap<isize, u32>> {
        self.classic_windows.lock()
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

    /// Appends `item` to the mailbox.
    fn push(&self, item: Item, (trace, observed_at_us, timing): (TraceId, u64, EventTiming)) {
        self.mailbox.push(Entry {
            item,
            trace,
            observed_at_us,
            timing,
        });
    }

    /// Queues an event one of the outpost's own handlers took in, observed
    /// now, then tells the observer, if there is one, that it was heard.
    fn take_in(&self, item: Item, heard: Heard, timing: EventTiming) {
        self.push(
            item,
            (TraceId::mint(), window::observed_us(&timing), timing),
        );
        self.tell_heard(heard);
    }

    /// Tells the observer, if there is one, that an event was heard.
    fn tell_heard(&self, heard: Heard) {
        if let Some(observer) = self.heard.get() {
            observer(heard);
        }
    }

    /// Hooks the console's events for this outpost's application, once
    /// for its life, the first time `Follow` names a terminal in a
    /// console's window (`docs/design/readers-tell-core-asks-build1.md`:
    /// "An outpost holds no `WinEvent` hook, except a console host's outpost,
    /// which keeps the console's five events"). The application serving a
    /// console's window is its console host whatever its image: `conhost`,
    /// `OpenConsole` behind Windows Terminal, or a test's own stand-in.
    /// The hook is in before `Follow` is answered, so every console event
    /// raised after the baseline read Core sends behind it is heard;
    /// one raised before is covered by that read. A hook that cannot be
    /// installed is reported to Core ([`OutpostToCore::Fault`]).
    fn hook_console(&self) {
        if self.console_hooked.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(nudger) = self.event_thread.get() else {
            return;
        };
        let target_pid = self.target_pid();
        if let Err(error) = nudger.hook(target_pid) {
            let detail = format!("the console's WinEvent hook could not be installed: {error}");
            tracing::error!(target_pid, detail, "the outpost cannot hear its console");
            fault(self, detail);
        }
    }

    /// Takes in a console's update or caret event the outpost's own hook
    /// delivered, as it is.
    fn take_in_win_event(&self, event: WinEvent, timing: EventTiming) {
        let WinEvent {
            kind,
            hwnd,
            id_object,
            id_child,
            thread,
            ..
        } = event;
        let Some(reducer_kind) = worker::win_kind(kind) else {
            tracing::debug!(?kind, "a WinEvent the outpost does not hook; dropped");
            return;
        };
        self.take_in(
            Item::Msaa {
                kind: reducer_kind,
                hwnd,
                id_object,
                id_child,
                raiser: thread,
            },
            Heard::Msaa(kind),
            timing,
        );
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
        read::serves(self, route.window.get()).then_some(route)
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
    /// focus for Core's request for the current focus with `focused_element` rather than from
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
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"): it does everything
    /// of an outpost's start that needs no application, its worker's UIA
    /// client included, writes [`OutpostToCore::SpareReady`] to
    /// `pipe`, and watches the application [`Outpost::assign`] gives it
    /// from then on. It reads the keyboard focus for Core's request for the current focus with
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
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts").
    fn build(
        outbound: Outbound,
        finisher: Finisher,
        options: OutpostOptions,
        (focused_element, warm): (FocusedElementReader, bool),
    ) -> Self {
        let context = Arc::new(Context {
            target_pid: AtomicU32::new(0),
            application_names: Lock::default(),
            outbound,
            mailbox: Mailbox::default(),
            epoch: Instant::now(),
            parked: Lock::new(0),
            shell: Lock::new(None),
            store: store::Store::new(),
            arbitrator: Lock::new(Arbitrator::new(&[])),
            observation: OnceLock::new(),
            focus_events: OnceLock::new(),
            named: Lock::new(None),
            remote_operations: options.remote_operations,
            classic_windows: Lock::new(HashMap::new()),
            focused_element,
            foreground: Lock::new(Arc::new(verbatim_win32::window::foreground_window_handle)),
            focus_window: Lock::new(Arc::new(verbatim_win32::window::focus_window)),
            window_readability: Lock::new(Arc::new(verbatim_win32::window::readability)),
            heard: OnceLock::new(),
            request_calls: OnceLock::new(),
            foreground_programs: OnceLock::new(),
            failed_reads: AtomicU32::new(0),
            event_thread: OnceLock::new(),
            console_hooked: std::sync::atomic::AtomicBool::new(false),
        });
        if let Some(registration) = register_focus_events(&context) {
            let _ = context.focus_events.set(registration);
        }

        let shell = Shell::start(&context, warm);
        *context.shell.lock() = Some(shell);

        // The console's update and caret events, process-scoped (decision
        // D13), hooked once the outpost has its application and only when
        // that application is a console host: no other outpost hooks a
        // `WinEvent`, the listener telling Core of every other kind.
        let hook_context = Arc::clone(&context);
        let make_handlers: MakeHandlers = Arc::new(move || {
            let context = Arc::clone(&hook_context);
            EventHandlers {
                win_event: Box::new(move |event: WinEvent| {
                    take_in_hooked(
                        event,
                        &mut (),
                        || context.observed_now(),
                        |(), _| {},
                        |(), event, timing| context.take_in_win_event(event, timing),
                    );
                }),
                nudge: None,
            }
        });
        let event_thread = EventThread::spawn(CONSOLE_SUBSCRIPTIONS, make_handlers);
        let _ = context.event_thread.set(event_thread.nudger());
        Self {
            context,
            event_thread,
            finisher,
        }
    }

    /// Gives the outpost its application, `target_pid`, for the rest of
    /// its life, and sends [`OutpostToCore::Ready`]. Its `WinEvent` hook is
    /// installed only once `Follow` names a console's terminal
    /// ([`Context::hook_console`]).
    /// Called once: by [`run_pipe`] at the start of an outpost started for
    /// an application, or for a spare's [`CoreToOutpost::Assign`];
    /// by the constructors of an outpost in a test's own process; and by a
    /// test for an [`Outpost::spare`].
    ///
    /// # Errors
    ///
    /// Returns why the outpost cannot watch the application: it already
    /// has one. The failure is also reported to Core
    /// ([`OutpostToCore::Fault`]), and no `Ready` is sent: the caller ends
    /// the outpost.
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
    /// before this call, with no program in flight, then until the focus-following subscriptions
    /// have made every move asked of them before it and every message
    /// published before it has been written to the pipe. A test that
    /// measures what the outpost's handling cost the application, or that
    /// asserts the outpost said nothing more, waits on this for its
    /// evidence. Unlike Core's idle round, it waits for no caret key's
    /// watch, and it divides batches as the round does. Returns once the outpost shuts down.
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
    /// without running DLL detach code (`books/dev/src/explanation/architecture.md`, "Process
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
        // An observation point: a test waits for it before it checks that
        // a call in progress holds the shutdown here.
        #[cfg(any(test, feature = "faults"))]
        verbatim_faults::reach(
            verbatim_faults::Fault::ShutdownWaitsForCalls,
            &context.target_pid(),
        );
        let workers = stop(&context);
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
        *self.context.foreground.lock() = reader;
    }

    /// Reads the window with the keyboard focus with `reader` from now on,
    /// in place of the system's, when answering Core's request for the
    /// current focus, whose
    /// focused element is read only in a window the application serves,
    /// and when judging whether the user has moved on from a program's
    /// window: a test whose application runs on a desktop where no window
    /// can take the keyboard focus says which window has it.
    pub fn set_focus_window_reader(&self, reader: FocusWindowReader) {
        *self.context.focus_window.lock() = reader;
    }

    /// Judges whether a window can be read with `reader` rather than by
    /// asking the system: for a test that has a window judged hung, which
    /// Windows does only after the application has not answered for five
    /// seconds.
    pub fn set_window_readability_reader(&self, reader: WindowReadabilityReader) {
        *self.context.window_readability.lock() = reader;
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

    /// Tells `observer`, on the worker, the calls into the application
    /// each of Core's requests made, by its number, before its answer is
    /// sent, so that a test pins a request's calls: a watch's in one telling
    /// per check or final read. Only the first observer
    /// counts; later calls change nothing.
    pub fn observe_request_calls(&self, observer: RequestCallsObserver) {
        let _ = self.context.request_calls.set(observer);
    }

    /// Tells `observer`, on the worker, the window of each foreground
    /// change a program is about to handle, before it handles it, so that a
    /// test can hold the worker in that program while it has the
    /// application raise events, which then wait in the intake. The
    /// outpost's test of the change against the foreground was made as the
    /// reducer took it in, before this. Only the first observer counts;
    /// later calls change nothing.
    pub fn observe_foreground_programs(&self, observer: ForegroundProgramObserver) {
        let _ = self.context.foreground_programs.set(observer);
    }

    /// Handles one command from Core, on the I/O thread. Pings are
    /// answered here, the pong written at once unless a write is in flight;
    /// everything that reads the application is queued for the worker.
    #[deny(clippy::wildcard_enum_match_arm)]
    pub fn handle_command(&self, command: &CoreToOutpost) {
        let context = &self.context;
        match command {
            CoreToOutpost::IdleQuery { round } => context.push(
                Item::Barrier {
                    asker: reducer::Asker::Round(*round),
                },
                (TraceId::mint(), now_us(), EventTiming::default()),
            ),
            CoreToOutpost::Request(ask) => {
                tracing::debug!(request = %ask.number, "request received and posted");
                context.mailbox.post(Posted::Ask(unstamped_ask(ask)));
            }
            CoreToOutpost::Cancel(cancel) => context.mailbox.post(Posted::CancelAsk(*cancel)),
            CoreToOutpost::WindowDestroyed(window) => {
                // Its queued requests answered "not there" at once, then
                // its nodes, text facts and followed registration
                // forgotten by a program, as an object destroyed always
                // was.
                context.mailbox.post(Posted::WindowDestroyed(*window));
                context.push(
                    Item::Msaa {
                        kind: reducer::WinKind::Destroy,
                        hwnd: window::hwnd_of(*window),
                        id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW.0,
                        id_child: verbatim_ia2::CHILDID_SELF,
                        raiser: 0,
                    },
                    (TraceId::mint(), now_us(), EventTiming::default()),
                );
            }
            CoreToOutpost::Ping { .. } => {
                context.outbound.urgent(context.pong());
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
                );
            }
        }
    }
}

/// `ask` with the node ids it names as this outpost knows them.
fn unstamped_ask(ask: &verbatim_model::request::Ask) -> Box<verbatim_model::request::Ask> {
    let mut ask = Box::new(ask.clone());
    ask.unstamp();
    ask
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
            event: 0x8005,
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

/// What a UIA callback captures of a property change on the focus: the
/// element's parts and window handle, read from its cache, the element for
/// anything the worker must ask it, and the acceptance facts of the
/// followed node's known window, read with calls to Windows only. No call
/// reaches the application.
fn capture_property(
    context: &Context,
    event: &Cached<PropertyEvent>,
    property_id: i32,
) -> UiaEvent {
    let fields = event.read();
    let parts = fields.parts(event.runtime_id());
    UiaEvent {
        kind: UiaKind::Property {
            id: property_id,
            text_pattern: fields.text_available,
            window: Box::new(followed_window(context)),
        },
        parts: UiaSnapshotFact {
            runtime_id: parts.runtime_id,
            role: parts.role,
            name: parts.name,
            value: parts.value,
            states: parts.states,
            details: parts.details,
        },
        hwnd: fields.role.hwnd,
        element: event
            .agile()
            .ok()
            .map(|agile| context.store.park(store::Parked::Property(agile))),
    }
}

/// The acceptance facts of the window of the node Core's newest `Follow`
/// names, as the outpost recorded it when the node was built: no window
/// when no node is followed or its window is not known.
pub(super) fn followed_window(context: &Context) -> verbatim_model::tell::TellWindow {
    use verbatim_win32::window_facts::WindowPart;
    let reads = OutpostReads(context);
    match followed::node(context).and_then(|node| context.store.nearest_window_of(node)) {
        Some(window) if window != 0 => verbatim_model::tell::TellWindow::Found(Box::new(
            WindowPart::read(&reads, window, None).with_foreground(&reads),
        )),
        _ => verbatim_model::tell::TellWindow::NoWindow,
    }
}

/// Windows' own state as the outpost reads it: the system's, but the
/// foreground window by the outpost's foreground reader
/// ([`Outpost::set_foreground_reader`]).
struct OutpostReads<'a>(&'a Context);

impl WindowReads for OutpostReads<'_> {
    fn class(&self, window: isize) -> String {
        SystemReads.class(window)
    }

    fn owner(&self, window: isize) -> (u32, u32) {
        SystemReads.owner(window)
    }

    fn console_host(&self, window: isize) -> Option<u32> {
        SystemReads.console_host(window)
    }

    fn parent(&self, window: isize) -> isize {
        SystemReads.parent(window)
    }

    fn root(&self, window: isize) -> isize {
        SystemReads.root(window)
    }

    fn root_owner(&self, window: isize) -> isize {
        SystemReads.root_owner(window)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        SystemReads.is_child(parent, child)
    }

    fn topmost(&self, window: isize) -> bool {
        SystemReads.topmost(window)
    }

    fn desktop(&self) -> isize {
        SystemReads.desktop()
    }

    fn foreground(&self) -> isize {
        self.0.foreground_window()
    }

    fn active_window(&self) -> isize {
        SystemReads.active_window()
    }

    fn hidden_frame(&self, window: isize) -> bool {
        SystemReads.hidden_frame(window)
    }
}

/// What a UIA callback captures of a caret or text change: the element's
/// runtime id, which is all its handling reads of it, and its window
/// handle, read from its cache, and the element. No call reaches the
/// application.
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
        element: event
            .agile()
            .ok()
            .map(|agile| context.store.park(store::Parked::Text(agile))),
    }
}

/// Starts the focus-following UIA subscription
/// (`books/dev/src/reference/adr/0001-focus.md`, "Focus-following subscriptions"): one
/// registration, on one thread and client, listening
/// nowhere until the worker reports a focus whose element it has. Its two
/// groups are NVDA's two local groups (`UIAHandler/__init__.py`,
/// `_createLocalEventHandlerGroup`): the base group, given every UIA focus,
/// holds the name, value, and state changes ([`FOCUS_PROPERTIES`]) and the
/// caret event (`Text_TextSelectionChanged`); the extended group, given a
/// focus with text read through UIA, also holds the text changes
/// (`Text_TextChanged`) and the active text position changes. A third
/// group, given only the parent of a focus that is a `SensitiveSlider`'s
/// thumb, holds its value changes alone. Selections and notifications come
/// desktop-wide from the listener instead.
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
    // NVDA's extended group holds the active text position changes too;
    // the outpost tells Core nothing of them, Core having no handler for
    // them, so each is heard and dropped.
    let position_context = Arc::clone(context);
    let position = Subscription::ActiveTextPosition {
        callback: Arc::new(
            move |_: &Cached<TextEvent>, _: Option<&IUIAutomationTextRange>| {
                position_context.tell_heard(Heard::UiaActiveTextPosition);
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

/// Stops the reducer and the workers for the outpost's shutdown, once the
/// mailbox is closed: the reducer's thread ends once the program in flight
/// has ended, with no worker abandoned meanwhile, and every worker thread
/// is waited for, the one in charge and each abandoned one returning from
/// its call, however long UIA takes to end a call the application does not
/// answer. Returns how many worker threads were waited for.
fn stop(context: &Context) -> usize {
    let shell = context.shell.lock().take();
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
/// whole life, or, for `None`, as a spare (`books/dev/src/reference/adr/0001-focus.md`, "Spare
/// outposts"): it sends [`OutpostToCore::SpareReady`] and watches
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
    // The application's focus, asked of its main window as Core asks it of
    // the foreground window at its start.
    let main = window::main_window_of(target_pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the application has no window"))?;
    crate::driver::Driver::new(&outpost).current_focus(window::window_handle(main));
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
