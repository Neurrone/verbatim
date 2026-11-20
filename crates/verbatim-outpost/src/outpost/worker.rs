//! The workers (`books/dev/src/explanation/design/ownership.md`): the threads that call into the application.
//!
//! One worker generation is in charge. It takes one program at a time from
//! its inbox, which the reducer's shell fills, runs it on the copies the
//! program came with, and posts each commit and its end to the mailbox; it
//! writes no follow state and sends nothing to Core itself. The programs
//! are the handlers below: an entry's handling, Core's request, a watch's
//! check or final read, and a release's drop. The worker keeps nothing between programs:
//! each runs on the copies it was dispatched with.
//!
//! The worker is also where calls are counted: the backend crates count
//! each call on the thread that makes it, and the worker takes the count
//! as it answers Core's request ([`take_calls`]). Calls a program makes
//! without answering a request are taken and logged when it ends and
//! belong to no trace (`books/dev/src/reference/performance.md`, "Cancelled traces").
//!
//! A hung cross-process call cannot be stopped safely, so when Core
//! cancels the request a program runs because the foreground moved away,
//! the reducer abandons its worker: it marks the generation dead in the
//! worker's charge ([`Charge`]), answers the request "abandoned", and
//! starts a replacement. An abandoned worker that eventually returns has
//! its commits refused, posts that it returned, and exits.

#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::lock::Lock;
use std::thread;
use std::time::Instant;

use verbatim_ia2::WinEventKind;
use verbatim_model::property::{
    NodeDetailSet, PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::{CallCounts, NodeId, TraceId};
use verbatim_uia::{Cached, Kept};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};

use verbatim_model::protocol::{OutpostToCore, UiaSnapshotFact};

use super::commit::{Committed, Effects};
use super::read::Client;
use super::reducer::{
    Entry, Item, Program, Request, UiaEvent, UiaKind, VerdictChange, WatchProgram, WinKind, Work,
};
use super::shell::{Garbage, Posted};
use super::text_reads;
use super::{Context, RequestContext};
use crate::arbitration::Judgement as _;

pub(super) use super::reducer::is_state_property;

/// A program for a worker, with what the shell let go of for the worker to
/// drop before it runs.
pub(super) struct Job {
    pub(super) program: Program,
    pub(super) garbage: Garbage,
}

/// A worker generation's charge, shared between the worker and the shell
/// under a lock held for one copy: whether the generation was abandoned,
/// its commits then refused and its gate refusing every counted call
/// (`verbatim_win32::calls`).
#[derive(Default)]
pub(super) struct Charge {
    dead: Lock<bool>,
}

impl Charge {
    /// Whether the generation was abandoned.
    pub(super) fn is_dead(&self) -> bool {
        *self.dead.lock()
    }

    /// The generation is abandoned.
    pub(super) fn abandon(&self) {
        *self.dead.lock() = true;
    }
}

/// The cross-process calls this thread has made through either backend
/// since the last take, resetting the count. Only a worker calls into the
/// application, so on a worker this is what the program in hand has made.
pub(super) fn take_calls() -> CallCounts {
    verbatim_uia::calls::take() + verbatim_ia2::calls::take()
}

/// Starts a worker generation, which takes its programs from
/// `jobs`. With `warm`, it creates its UIA client at once instead of on its
/// first read: a spare's worker makes it while the spare waits for its
/// application (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts").
pub(super) fn spawn(
    context: Arc<Context>,
    (generation, warm): (u64, bool),
    jobs: crossbeam_channel::Receiver<Job>,
    charge: Arc<Charge>,
) -> std::io::Result<thread::JoinHandle<()>> {
    // A thread the system cannot start, as one out of memory refuses it.
    #[cfg(any(test, feature = "faults"))]
    if verbatim_faults::hit(
        verbatim_faults::Fault::ThreadStart,
        &(WORKER_THREAD, generation),
    ) {
        return Err(std::io::Error::from_raw_os_error(ERROR_NOT_ENOUGH_MEMORY));
    }
    thread::Builder::new()
        .name(WORKER_THREAD.to_owned())
        .spawn(move || run(&context, (generation, warm), &jobs, &charge))
}

/// A worker's thread name.
const WORKER_THREAD: &str = "verbatim-worker";

/// The call gate a worker installs on its thread
/// (`verbatim_win32::calls`), bound to its charge: once its generation is
/// abandoned it refuses every counted call.
struct Gate {
    charge: Arc<Charge>,
}

impl verbatim_win32::calls::Charge for Gate {
    fn is_dead(&self) -> bool {
        self.charge.is_dead()
    }
}

/// The error the system refuses a thread with when it is out of memory,
/// `ERROR_NOT_ENOUGH_MEMORY`.
#[cfg(any(test, feature = "faults"))]
const ERROR_NOT_ENOUGH_MEMORY: i32 = 8;

/// The worker's thread: its loop, then the release of the UIA objects
/// `verbatim-uia` kept for this thread and of its call gate, which holds
/// the outpost's context, before the thread exits rather than from its
/// thread-local destructors under the loader lock.
fn run(
    context: &Arc<Context>,
    (generation, warm): (u64, bool),
    jobs: &crossbeam_channel::Receiver<Job>,
    charge: &Arc<Charge>,
) {
    verbatim_win32::calls::install(Arc::new(Gate {
        charge: Arc::clone(charge),
    }));
    // Held objects are agile references, resolved in this apartment.
    let joined = verbatim_uia::init_mta();
    if let Err(error) = &joined {
        tracing::warn!(%error, "the worker could not join the multithreaded apartment");
    }
    let mut client = Client::default();
    if warm {
        let _ = client.uia();
    }
    // Until the shell lets go of this generation's inbox, abandoned or at
    // the outpost's shutdown.
    while let Ok(job) = jobs.recv() {
        if charge.is_dead() {
            // An abandoned worker makes no further call.
            continue;
        }
        run_job(context, &mut client, job, charge);
    }
    if charge.is_dead() {
        context.mailbox.post(Posted::Returned(generation));
    }
    drop(client);
    verbatim_uia::release_thread_state();
    if joined.is_ok() {
        verbatim_uia::leave_mta();
    }
    verbatim_win32::calls::uninstall();
}

/// Runs one program, after dropping what the shell let go of, and posts its
/// end.
/// Runs a program's body, catching a panic, which answers the program as
/// panicked; a failed lock check's panic ends the process instead, in
/// debug builds, so no caught panic hides it.
pub(super) fn catching(body: impl FnOnce()) -> std::thread::Result<()> {
    let handled = catch_unwind(AssertUnwindSafe(body));
    if let Err(payload) = &handled {
        verbatim_win32::held_locks::abort_if_lock_check(&**payload);
    }
    handled
}

/// The node details `work`'s reads fetch: Core's request by its property
/// set; an event's own reads, which no request names, every detail; a
/// watch or a release, none.
fn node_details_of(work: &Work) -> NodeDetailSet {
    match work {
        Work::Entry(entry) => match &entry.item {
            Item::Msaa { .. } | Item::Uia(_) => NodeDetailSet::ALL,
            Item::NodesHeld { .. } | Item::Barrier { .. } => NodeDetailSet::NONE,
        },
        Work::Ask(ask) => super::asked::node_details(&ask.request),
        Work::Drop | Work::Watch(_) | Work::Forget(_) => NodeDetailSet::NONE,
    }
}

/// Forgets each window of `windows`, which Core named destroyed: its kept
/// verdicts and classic-read mark, and its MSAA nodes with their text
/// facts, so a reused window handle never inherits them. The nodes'
/// objects are released once the store's lock is let go, and their text
/// facts are forgotten with the program's commit. Nothing reads the
/// application.
fn forget(context: &RequestContext<'_>, windows: &[verbatim_model::WindowHandle]) {
    for &window in windows {
        let hwnd = super::window::hwnd_of(window);
        context.record(VerdictChange::Forget(hwnd));
        context.forget_window(hwnd);
        let (removed, gone) = context.store.forget_window(hwnd);
        drop(removed);
        text_reads::forget_nodes(context, &gone);
    }
}

fn run_job(context: &Arc<Context>, client: &mut Client, job: Job, charge: &Arc<Charge>) {
    let Job { program, garbage } = job;
    // Objects the shell let go of: releasing one can call into the
    // application, so it is done here, under the program's deadline.
    drop(garbage);
    if let Work::Ask(ask) = &program.work {
        tracing::debug!(request = %ask.number, "request received by the worker");
    }
    let Program {
        request,
        facts,
        work,
    } = program;
    let started = Instant::now();
    if let Work::Ask(ask) = &work {
        tracing::debug!(request = %ask.number, "request program begins");
    }
    let description = tracing::enabled!(tracing::Level::DEBUG).then(|| describe(&work));
    let tag = super::shell::tag_of(request);
    let details = node_details_of(&work);
    let mut request_context = RequestContext::for_program(
        context,
        (tag, details),
        (request, facts),
        Arc::clone(charge),
    );
    let handled = catching(|| match work {
        Work::Entry(entry) => {
            // Every verdict the entry asks for is judged at its event's
            // observation, or, for an entry with no event, at its start
            // (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact").
            request_context.judge_at(judged_at(context, &entry));
            Worker {
                context: &request_context,
            }
            .handle(entry);
        }
        Work::Drop => {}
        Work::Ask(ask) => {
            let answer = super::asked::answer(&request_context, client, &ask);
            if let Some(observer) = context.request_calls.get() {
                observer(answer.number, take_calls());
            }
            tracing::debug!(request = %answer.number, "request program ended, answer posted to the reducer");
            context.mailbox.post(Posted::Answered {
                request,
                answer: Box::new(answer),
            });
        }
        Work::Watch(watch) => run_watch(context, &mut request_context, request, &watch),
        Work::Forget(windows) => forget(&request_context, &windows),
    });
    // Text writes no commit after them carried: a program's text calls
    // write nothing but through a commit.
    request_context.commit_text();
    // Calls made after the program's last publish, or by a program that
    // published nothing: they belong to no trace.
    let unpublished = take_calls();
    let failed_reads = verbatim_uia::calls::take_failed_reads();
    context
        .failed_reads
        .fetch_add(failed_reads, Ordering::Relaxed);
    if handled.is_err() {
        tracing::error!(target: verbatim_model::DEFECT_TARGET, ?request, "the worker failed running a program");
    }
    tracing::debug!(
        item = description.as_deref().unwrap_or_default(),
        elapsed_us = started.elapsed().as_micros(),
        ?unpublished,
        failed_reads,
        "handled"
    );
    // Elements the program's lookups replaced, and the text objects it
    // held, dropped holding no lock.
    drop(request_context);
    context.mailbox.post(Posted::Finished {
        request,
        panicked: handled.is_err(),
    });
}

/// Runs a watch's check or final read, telling the observer of Core's
/// requests' calls of the program's, and posting the watch's answer when
/// the program ends it.
fn run_watch(
    context: &Context,
    request_context: &mut RequestContext<'_>,
    request: Request,
    watch: &WatchProgram,
) {
    request_context.judge_at(context.observed_now());
    let outcome = super::text_reads::watch(request_context, watch);
    if let Some(observer) = context.request_calls.get() {
        observer(watch.number, take_calls());
    }
    if let Some(outcome) = outcome {
        context.mailbox.post(Posted::Answered {
            request,
            answer: Box::new(verbatim_model::request::Answer {
                number: watch.number,
                outcome,
                probes: Vec::new(),
                detail: String::new(),
            }),
        });
    }
}

/// The observation `entry`'s verdicts are judged at: its event's, for an
/// event the outpost's hook or subscriptions took in or a fact the listener
/// sent; for anything else, which has no event, now, on the arbitrator's
/// observation clock, the entry's start.
fn judged_at(context: &Context, entry: &Entry) -> u64 {
    match entry.item {
        Item::Msaa { .. } | Item::Uia(_) if entry.observed_at_us != 0 => entry.observed_at_us,
        _ => context.observed_now(),
    }
}

/// A short description of a program's work, for the debug log of what the
/// worker spent its time on, formatted only when that log is enabled.
fn describe(work: &Work) -> String {
    match work {
        Work::Entry(entry) => describe_item(&entry.item),
        Work::Drop => "drop".to_owned(),
        Work::Ask(ask) => format!("{}", ask.number),
        Work::Watch(watch) => format!("the watch {}'s {:?}", watch.number, watch.work),
        Work::Forget(windows) => format!("forget {} windows destroyed", windows.len()),
    }
}

/// A short description of an entry's item.
fn describe_item(item: &Item) -> String {
    match item {
        Item::Msaa {
            kind,
            hwnd,
            id_object,
            id_child,
            raiser,
        } => {
            format!("MSAA {kind:?} hwnd={hwnd} object={id_object} child={id_child} thread={raiser}")
        }
        Item::Uia(event) => format!("UIA {:?}", event.kind),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::Barrier { asker, .. } => format!("barrier of {asker:?}"),
    }
}

/// The reducer's kind of a `WinEvent` an outpost's hook delivered: only a
/// console host's console events are hooked, so any other is `None`.
pub(super) fn win_kind(kind: WinEventKind) -> Option<WinKind> {
    match kind {
        WinEventKind::ConsoleUpdate => Some(WinKind::ConsoleUpdate),
        WinEventKind::ConsoleCaret => Some(WinKind::ConsoleCaret),
        _ => None,
    }
}

/// The property and value a tell of a change of UI Automation property
/// `id` carries, from the event's element `parts`: the states, name or
/// value it carried; for a help text change a description change and for a
/// `ControllerFor` change a suggestions change, each with no value, since
/// Core asks the node for them (`docs/design/readers-tell-core-asks-build1.md`,
/// "Changes on objects"). The states of a menu item that no pattern makes
/// checkable are completed from `element`'s legacy MSAA state, as NVDA's
/// menu item reads its states (`NVDAObjects/UIA/__init__.py`, `MenuItem`).
/// `None` for a property no `Follow` group registers.
fn told_property(
    id: i32,
    parts: &UiaSnapshotFact,
    element: Option<&Cached<Kept>>,
) -> Option<(PropertyId, Option<PropertyValue>)> {
    let text = |text: &Option<String>| text.clone().map(PropertyValue::Text);
    Some(match id {
        id if is_state_property(id) => (
            PropertyId::States,
            Some(PropertyValue::States(match element {
                Some(element) => verbatim_uia::map::with_legacy_checked_states(
                    element.live().element(),
                    parts.role,
                    parts.states,
                ),
                None => parts.states,
            })),
        ),
        id if id == UIA_NamePropertyId.0 => (PropertyId::Name, text(&parts.name)),
        id if id == UIA_ValueValuePropertyId.0 || id == UIA_RangeValueValuePropertyId.0 => {
            (PropertyId::Value, text(&parts.value))
        }
        // NVDA maps the help text to a description change
        // (`UIAPropertyIdsToNVDAEventNames`), whose description it reads
        // when it handles it.
        id if id == UIA_HelpTextPropertyId.0 => (PropertyId::Description, None),
        id if id == UIA_ControllerForPropertyId.0 => (PropertyId::Suggestions, None),
        _ => return None,
    })
}

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a RequestContext<'a>,
}

impl Worker<'_> {
    /// Commits `effects` for this worker: applied to the program's own
    /// copies and posted for the reducer, unless the worker was abandoned
    /// ([`RequestContext::commit`]).
    fn commit(&self, effects: Effects) -> Committed {
        self.context.commit(effects)
    }

    fn handle(&self, entry: Entry) {
        let Entry {
            item,
            trace,
            observed_at_us,
            ..
        } = entry;
        match item {
            Item::Msaa { kind, hwnd, .. } => self.msaa_event(kind, hwnd, trace, observed_at_us),
            Item::Uia(event) => self.uia_event(&event, trace, observed_at_us),
            // The reducer's own: never a program's.
            Item::NodesHeld { .. } | Item::Barrier { .. } => {}
        }
    }

    /// The terminal `Follow` names when it is the text area of the console
    /// window `hwnd`, whose console events are its own
    /// (`docs/design/readers-tell-core-asks-build3.md`, "The outpost: change
    /// tells"). A console window is known by its terminal's nearest
    /// window, recorded when its node was minted; the console host serves
    /// one console window, so a terminal whose nearest window is not
    /// recorded takes its process's console events.
    fn followed_console_terminal(&self, hwnd: isize) -> Option<NodeId> {
        let (node_id, console) = super::followed::terminal(self.context)?;
        let in_window = self
            .context
            .store
            .nearest_window_of(node_id)
            .is_none_or(|window| window == hwnd);
        (console && in_window).then_some(node_id)
    }

    /// Whether `node_id` is the terminal Core's `Follow` names.
    fn followed_terminal(&self, node_id: NodeId) -> bool {
        super::followed::terminal(self.context).is_some_and(|(followed, _)| followed == node_id)
    }

    /// Tells Core of a change signal of the terminal `node_id` `Follow`
    /// names, in window `window`, observed at `observed_at_us`
    /// (`docs/design/readers-tell-core-asks-build3.md`, "The outpost: change
    /// tells"): one `Event` tell of kind "terminal changed". The outpost
    /// reads nothing for it and judges nothing of it; Core reads the
    /// terminal when it chooses. The intake's limiter merged the signals of
    /// the same window or element and kind still waiting.
    fn terminal_tell(
        &self,
        node_id: NodeId,
        window: isize,
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let object = verbatim_model::tell::EventObject::Terminal {
            node: node_id,
            window: super::window::window_handle(window),
        };
        let tell = self.followed_tell(
            verbatim_model::event_kind::EventKind::TerminalChanged,
            object,
            (
                PropertyValues(Vec::new()),
                verbatim_model::tell::Payload::None,
            ),
            verbatim_model::tell::TellWindow::NoWindow,
            (trace, observed_at_us),
        );
        self.commit(Effects {
            result: Some(OutpostToCore::Tell(Box::new(tell))),
            ..Effects::default()
        });
    }

    /// An MSAA event from this outpost's own hooks.
    fn msaa_event(&self, kind: WinKind, hwnd: isize, trace: TraceId, observed_at_us: u64) {
        match kind {
            // The console host's text changed or its cursor moved: the
            // change signals of the console terminal `Follow` names, whose
            // UIA text and caret events are not subscribed to
            // (`books/dev/src/reference/crates/verbatim-outpost.md`,
            // "Terminals").
            WinKind::ConsoleUpdate | WinKind::ConsoleCaret => {
                if let Some(node_id) = self.followed_console_terminal(hwnd) {
                    self.terminal_tell(node_id, hwnd, (trace, observed_at_us));
                }
            }
        }
    }

    /// A UIA event from this outpost's `Follow` registration
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
    /// build"): a change signal of the terminal `Follow` names, its text
    /// change or caret event, a caret event, or a property change, each
    /// told to Core. A text change of any other element tells nothing: its
    /// element is let go of.
    fn uia_event(&self, event: &UiaEvent, trace: TraceId, observed_at_us: u64) {
        if let Some(node_id) = self.of_terminal(event) {
            if let Some(key) = event.element {
                drop(self.context.store.take_parked(key));
            }
            let window = if event.hwnd == 0 {
                self.context.store.nearest_window_of(node_id).unwrap_or(0)
            } else {
                event.hwnd
            };
            return self.terminal_tell(node_id, window, (trace, observed_at_us));
        }
        match &event.kind {
            UiaKind::TextSelection => self.caret_tell(event, trace, observed_at_us),
            UiaKind::Property {
                id,
                text_pattern,
                automation_id,
                window,
            } => {
                let element = event
                    .element
                    .and_then(|key| self.context.store.take_parked(key))
                    .and_then(super::store::Parked::element);
                self.property_tell(
                    (event, element.as_ref()),
                    (
                        *id,
                        (*text_pattern, automation_id.as_deref()),
                        (**window).clone(),
                    ),
                    trace,
                    observed_at_us,
                );
            }
            UiaKind::TextChanged => {
                if let Some(key) = event.element {
                    drop(self.context.store.take_parked(key));
                }
            }
        }
    }

    /// The terminal `Follow` names when `event`, a text change or caret
    /// event, is of its element: its change signals
    /// (`docs/design/readers-tell-core-asks-build3.md`, "The outpost: change
    /// tells").
    fn of_terminal(&self, event: &UiaEvent) -> Option<NodeId> {
        if !matches!(event.kind, UiaKind::TextSelection | UiaKind::TextChanged) {
            return None;
        }
        self.context
            .nodes()
            .existing(&event.parts.runtime_id)
            .filter(|&node_id| self.followed_terminal(node_id))
    }

    /// Tells Core of a UI Automation caret event (`Text_TextSelectionChanged`)
    /// that the `Follow` registration delivered for `event`'s element, the
    /// event's element kept behind the token Core's caret read names for
    /// the outpost to adopt when the node's own answered no text pattern
    /// (`docs/design/readers-tell-core-asks-build1.md`, "Selection and the
    /// caret's text facts"). The outpost reads nothing for it and judges
    /// nothing of it: Core asks the caret read at the tell's turn, when its
    /// element is the committed focus. An element UI Automation could not
    /// make agile is told with nothing kept.
    fn caret_tell(&self, event: &UiaEvent, trace: TraceId, observed_at_us: u64) {
        let runtime_id = RuntimeId(event.parts.runtime_id.clone());
        let object = match event.element {
            Some(key) => {
                drop(self.context.store.keep_told(key, observed_at_us));
                verbatim_model::tell::EventObject::Uia {
                    token: verbatim_model::tell::ElementToken(key),
                    runtime_id,
                }
            }
            None => verbatim_model::tell::EventObject::Followed { runtime_id },
        };
        let tell = self.followed_tell(
            verbatim_model::event_kind::EventKind::UiaTextSelectionChanged,
            object,
            (
                PropertyValues(Vec::new()),
                verbatim_model::tell::Payload::None,
            ),
            super::followed_window(self.context),
            (trace, observed_at_us),
        );
        self.commit(Effects {
            result: Some(OutpostToCore::Tell(Box::new(tell))),
            ..Effects::default()
        });
    }

    /// The tell of an event of kind `kind` that the `Follow` registration
    /// delivered for `object`, with its cached properties and payload,
    /// judged by `window`, the followed node's.
    fn followed_tell(
        &self,
        kind: verbatim_model::event_kind::EventKind,
        object: verbatim_model::tell::EventObject,
        (cached, payload): (PropertyValues, verbatim_model::tell::Payload),
        window: verbatim_model::tell::TellWindow,
        (trace, observed_at_us): (TraceId, u64),
    ) -> verbatim_model::tell::Event {
        let thread = match &window {
            verbatim_model::tell::TellWindow::Found(facts) => facts.window.thread,
            verbatim_model::tell::TellWindow::NoWindow
            | verbatim_model::tell::TellWindow::Failed => 0,
        };
        let target = self.context.target_pid();
        verbatim_model::tell::Event {
            trace_id: trace,
            kind,
            process: verbatim_model::Pid(target),
            application: self.context.application_name(target),
            thread,
            at_us: observed_at_us,
            object,
            cached,
            payload,
            window,
            checked: None,
        }
    }

    /// Tells Core of the change of UI Automation property `id` that the
    /// `Follow` registration delivered for `event`'s element, with the
    /// value it carried, whether the element has the Text pattern
    /// (`text_pattern`), for a suggestions change its automation id
    /// (`automation_id`), both from the event's cache, which Core's rule
    /// for a suggestions change off the focus reads, and the facts of the
    /// followed node's window its callback gathered, which Core's
    /// acceptance reads, as NVDA applies `shouldAcceptEvent` in its
    /// property-change handler. The outpost
    /// judges nothing of it: Core finds the
    /// node by its runtime id among the committed focus and its ancestors
    /// (`docs/design/readers-tell-core-asks-build1.md`, "Readers' local
    /// drops", "Changes on objects").
    fn property_tell(
        &self,
        (event, element): (&UiaEvent, Option<&Cached<Kept>>),
        (id, (text_pattern, automation_id), window): (
            i32,
            (bool, Option<&str>),
            verbatim_model::tell::TellWindow,
        ),
        trace: TraceId,
        observed_at_us: u64,
    ) {
        let Some((property, value)) = told_property(id, &event.parts, element) else {
            return;
        };
        let mut cached = vec![PropertyAnswer {
            id: PropertyId::HasTextPattern,
            value: Some(PropertyValue::Flag(text_pattern)),
        }];
        if property == PropertyId::Suggestions {
            cached.push(PropertyAnswer {
                id: PropertyId::AutomationId,
                value: automation_id.map(|id| PropertyValue::Text(id.to_owned())),
            });
        }
        let tell = self.followed_tell(
            verbatim_model::event_kind::EventKind::UiaPropertyChanged,
            verbatim_model::tell::EventObject::Followed {
                runtime_id: RuntimeId(event.parts.runtime_id.clone()),
            },
            (
                PropertyValues(cached),
                verbatim_model::tell::Payload::PropertyChanged { property, value },
            ),
            window,
            (trace, observed_at_us),
        );
        self.commit(Effects {
            result: Some(OutpostToCore::Tell(Box::new(tell))),
            ..Effects::default()
        });
    }
}
