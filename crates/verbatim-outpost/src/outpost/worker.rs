//! The workers (`books/dev/src/explanation/design/ownership.md`): the threads that call into the application.
//!
//! One worker generation is in charge. It takes one program at a time from
//! its inbox, which the reducer's shell fills, runs it on the copies the
//! program came with, and posts each commit and its end to the mailbox; it
//! writes no follow state and sends nothing to Core itself. The programs
//! are the handlers below: an entry's handling, the owed work, a release's
//! drop, and the finish of a cut caret read. Each makes the
//! calls the outpost has always made for its entry, so the calls each
//! handling makes are unchanged. The worker keeps nothing between programs:
//! each runs on the copies it was dispatched with.
//!
//! The worker is also where calls are counted: the backend crates count
//! each call on the thread that makes it, and the worker takes the count
//! when it builds an event or a reply, which carries it to Core with the
//! timing ([`take_calls`]). Calls a program makes without publishing
//! anything, for an event it drops, are taken and logged when it ends and
//! belong to no trace (`books/dev/src/reference/performance.md`, "Cancelled traces").
//!
//! A hung cross-process call cannot be stopped safely, so when a program
//! passes its deadline, or waits on a window the user has left, the
//! reducer abandons its worker: it marks the generation dead in the
//! worker's charge ([`Charge`]), answers the query the program was running
//! "abandoned", and starts a replacement. An abandoned worker that
//! eventually returns has its commits refused, posts that it returned, and
//! exits. A window found hung while a program waits on it abandons
//! nothing: its hang is told to Core, and the program goes on.

#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::lock::Lock;
use std::thread;
use std::time::Instant;

use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, CaretChanges, CaretRead, CaretReply, CaretReport, Change, FocusText,
    NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, ReadPurpose, Role, TextOp, TextPoint,
    TextReply, TraceId, WindowFacts, WindowHandle, had_effect,
};
use verbatim_uia::{
    Cached, Fill, HelpTextRefresh, Interning as _, Kept, SuggestionsRefresh, nearest_window_handle,
};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_WINDOW;

use verbatim_model::protocol::{
    EventTiming, FocusNow, FocusPurpose, FocusedControl, OutpostToCore, Query, QueryOutcome,
    QueryResult, RequestId, UiaSnapshotFact,
};
use verbatim_model::{AlertOrigin, ToastId};
use verbatim_process::clock::now_us;

use super::commit::{Committed, Effects, FocusEffects, FollowWrites, KeptTarget, Stale, Target};
use super::focus_path;
use super::read::{self, Client, ReadError};
use super::reducer::{
    self, Cut, Entry, Finish, FocusPath, Followed, Handling, Item, LastReported, MarkChange,
    Object, Origin, Owed, OwedProgram, OwedWork, Program, UiaEvent, UiaKind, VerdictChange,
    WinKind, Work,
};
use super::reducer::{KeyNote, NoteFetch, NoteReads, Request, WatchProgram};
use super::shell::{Garbage, Posted};
use super::text_reads;
use super::window::{alt_is_held, console_client, window_facts};
use super::{Context, RequestContext};
use crate::arbitration::Judgement as _;
use crate::terminal::reading::{Action, Event, Reading};
use crate::text::{Checked, KeyChecked};
use verbatim_core::focus_rules;
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{self, EventName};
use verbatim_win32::window::SystemWindows;
use verbatim_win32::window::top_level_of;
use verbatim_win32::window::window_class_name;

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

fn run_job(context: &Arc<Context>, client: &mut Client, job: Job, charge: &Arc<Charge>) {
    let Job { program, garbage } = job;
    // Objects the shell let go of: releasing one can call into the
    // application, so it is done here, under the program's deadline.
    drop(garbage);
    let Program {
        request,
        facts,
        work,
    } = program;
    let started = Instant::now();
    let description = tracing::enabled!(tracing::Level::DEBUG).then(|| describe(&work));
    let tag = super::shell::tag_of(request);
    let mut request_context =
        RequestContext::for_program(context, tag, (request, facts), Arc::clone(charge));
    let mut cut = None;
    let handled = catching(|| match work {
        Work::Entry { entry, handling } => {
            let timing = EventTiming {
                dequeued_at_us: now_us(),
                ..entry.timing
            };
            // Every verdict the entry asks for is judged at its event's
            // observation, or, for an entry with no event, at its start
            // (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact").
            request_context.judge_at(judged_at(context, &entry));
            let mut worker = Worker::new(&request_context, client, timing);
            if let Handling::FinalRead { note_until } = handling {
                worker.final_note_until = note_until;
            }
            worker.handle(entry);
            cut = worker.cut.take();
        }
        Work::Owed(owed) => {
            Worker::new(&request_context, client, EventTiming::default()).attempt_owed(owed);
        }
        Work::Drop => {}
        Work::NoteRead(node) => {
            request_context.judge_at(context.observed_now());
            let timing = EventTiming {
                dequeued_at_us: now_us(),
                ..EventTiming::default()
            };
            Worker::new(&request_context, client, timing).note_poll(node);
        }
        Work::Finish { cut, waiting } => {
            Worker::new(&request_context, client, cut.timing).finish(*cut, waiting);
        }
        Work::Ask(ask) => {
            let answer = super::asked::answer(&request_context, client, &ask);
            if let Some(observer) = context.request_calls.get() {
                observer(answer.number, take_calls());
            }
            context.mailbox.post(Posted::Answered {
                request,
                answer: Box::new(answer),
            });
        }
        Work::Watch(watch) => run_watch(context, &mut request_context, request, &watch),
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
        tracing::error!(?request, "the worker failed running a program");
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
        cut,
    });
}

/// Runs a watch's check or final read, posting the watch's answer when the
/// program ends it.
fn run_watch(
    context: &Context,
    request_context: &mut RequestContext<'_>,
    request: Request,
    watch: &WatchProgram,
) {
    request_context.judge_at(context.observed_now());
    if let Some(outcome) = super::text_reads::watch(request_context, watch) {
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
        Work::Entry { entry, .. } => describe_item(&entry.item),
        Work::Owed(owed) => format!("owed work of {:?}", owed.node),
        Work::Drop => "drop".to_owned(),
        Work::NoteRead(node) => format!("the console poll's read of {node:?}"),
        Work::Finish { cut, .. } => format!("the finish of a caret read of {:?}", cut.node),
        Work::Ask(ask) => format!("{}", ask.number),
        Work::Watch(watch) => format!("the watch {}'s {:?}", watch.number, watch.work),
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
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::NotedCaret { node_id } => format!("noted caret of {node_id:?}"),
        Item::ReadTerminal { node_id } => format!("the read after a key of {node_id:?}"),
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

/// A focus-now answer's control, which the outpost adopts as its focus and
/// follows once the answer is sent ([`Worker::follow_answer`]).
struct AnsweredFocus {
    /// The control.
    node: NodeSnapshot,
    /// Its ancestors, outermost first.
    ancestors: Vec<NodeSnapshot>,
    /// What its caret read found ([`Worker::read_focus_text`]), and when
    /// that read began.
    text: Option<FocusText>,
    read_at: Option<u64>,
    /// When the answer began to be read, in microseconds on the run's
    /// shared clock.
    observed_at_us: u64,
    /// The window whose focused element the answer read.
    window: Option<isize>,
}

/// The object the focus `node` is, by its backend's registry: its runtime id
/// or its MSAA event address, as the intake records the focus.
fn object_of(context: &Context, node: &NodeSnapshot) -> Option<Object> {
    match node.backend {
        Backend::Uia => context.store.runtime_id_of(node.id).map(Object::Uia),
        Backend::Msaa => context
            .store
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child)),
    }
}

/// The on-demand reading event a terminal text request is, `None` for any
/// other request.
fn terminal_event_of(op: &TextOp) -> Option<Event> {
    match op {
        TextOp::TerminalHold => Some(Event::Hold),
        TextOp::TerminalRead { hold } => Some(Event::Read { hold: *hold }),
        TextOp::TerminalCancel => Some(Event::Cancel),
        _ => None,
    }
}

/// A toast's identity for Core's repeat rule: a hash of its runtime id, with
/// `DefaultHasher`'s fixed keys, so a repeat of the same toast hashes alike.
fn toast_id(runtime_id: &[i32]) -> ToastId {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    runtime_id.hash(&mut hasher);
    ToastId(hasher.finish())
}

/// The name NVDA's acceptance test knows a UIA event of `kind` by, or `None`
/// for one it does not test: a notification and an active text position,
/// which NVDA's handlers for them never test (`UIAHandler/__init__.py`
/// 1063 and 1158). A caret and a text change go through NVDA's automation
/// event handler, which tests them as any other (`docs/parity/focus-events.md`, "Event
/// acceptance").
fn uia_event_name(kind: &UiaKind) -> Option<EventName> {
    match kind {
        UiaKind::Property(id) if *id == UIA_NamePropertyId.0 => Some(EventName::NameChange),
        UiaKind::Property(id) if *id == UIA_HelpTextPropertyId.0 => {
            Some(EventName::DescriptionChange)
        }
        UiaKind::Property(id)
            if *id == UIA_ValueValuePropertyId.0 || *id == UIA_RangeValueValuePropertyId.0 =>
        {
            Some(EventName::ValueChange)
        }
        UiaKind::Property(_) => Some(EventName::StateChange),
        UiaKind::Selection => Some(EventName::Selection),
        UiaKind::TextSelection => Some(EventName::Caret),
        UiaKind::TextChanged => Some(EventName::TextChange),
        UiaKind::Notification(_) | UiaKind::Toast | UiaKind::ActiveTextPosition(_) => None,
    }
}

/// Whether a change of UIA property `id` is reported as a change of the
/// element's states ([`property_event`]): every property but the name, the
/// value, the range value, the help text and `ControllerFor`, such as the
/// The event for a change of property `id` of the node read as `node`,
/// whose element, for a help text or `ControllerFor` change, is
/// `refreshed` with what that change reads ([`refreshed_for`]), or `None`
/// when the change reports nothing.
fn property_event(
    id: i32,
    node: NodeSnapshot,
    refreshed: Option<&Refreshed>,
) -> Option<NormalizedEvent> {
    Some(match id {
        id if is_state_property(id) => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::States(node.states),
            child_count: None,
        },
        id if id == UIA_NamePropertyId.0 => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::Name(node.name),
            child_count: None,
        },
        id if id == UIA_ValueValuePropertyId.0 || id == UIA_RangeValueValuePropertyId.0 => {
            NormalizedEvent::ValueChanged {
                node_id: node.id,
                value: node.value,
            }
        }
        // NVDA maps the help text to a description change
        // (`UIAPropertyIdsToNVDAEventNames`), whose description is the
        // full description or the help text, read when it is handled.
        id if id == UIA_HelpTextPropertyId.0 => NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: PropertyChange::Description(match refreshed {
                Some(Refreshed::HelpText(element)) => element.read().description.description(),
                _ => None,
            }),
            child_count: None,
        },
        id if id == UIA_ControllerForPropertyId.0 => {
            let open = match refreshed {
                Some(Refreshed::Suggestions(element)) => suggestions_open(element)?,
                _ => return None,
            };
            NormalizedEvent::PropertyChanged {
                node_id: node.id,
                change: PropertyChange::Suggestions(open),
                child_count: None,
            }
        }
        _ => return None,
    })
}

/// An element refreshed with what a change of one of its properties reads
/// when it is handled ([`refreshed_for`]).
enum Refreshed {
    /// For a help text change: its description.
    HelpText(Cached<HelpTextRefresh>),
    /// For a `ControllerFor` change: whether it reports suggestions and
    /// whether its relation names any element.
    Suggestions(Cached<SuggestionsRefresh>),
}

/// `element` refreshed with what a change of property `id` reads when it is
/// handled, in one call: the description for a help text change, and
/// whether it is editable text or a search field and whether the relation
/// names any element for a `ControllerFor` change. `None` for any other
/// change, which is reported from the event's cache as it stands, and when
/// the refresh fails.
fn refreshed_for(
    uia: Option<&verbatim_uia::Uia>,
    id: i32,
    element: Option<&Cached<Kept>>,
) -> Option<Refreshed> {
    let uia = uia?;
    let live = element?.live();
    if id == UIA_HelpTextPropertyId.0 {
        let request = uia.request_fixed::<HelpTextRefresh>().ok()?;
        live.refresh(&request).ok().map(Refreshed::HelpText)
    } else if id == UIA_ControllerForPropertyId.0 {
        let request = uia.request_fixed::<SuggestionsRefresh>().ok()?;
        live.refresh(&request).ok().map(Refreshed::Suggestions)
    } else {
        None
    }
}

/// For a `ControllerFor` change of `element`, whether its suggestions are
/// now open, or `None` when it does not report them. Only editable text and
/// search fields report suggestions
/// (`EditableTextWithSuggestions.event_controllerForChange`,
/// `NVDAObjects/behaviors.py` 231): whether the relation now names any
/// element, read live by the refresh that read the rest, as NVDA reads
/// `controllerFor` (not in its base cache). A relation the refresh could
/// not read names none.
fn suggestions_open(element: &Cached<SuggestionsRefresh>) -> Option<bool> {
    let refreshed = element.read();
    refreshed
        .reports_suggestions()
        .then_some(refreshed.controls_any)
}

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a RequestContext<'a>,
    client: &'a mut Client,
    /// When the entry was raised, observed, relayed, and dequeued, for the
    /// latency log.
    timing: EventTiming,
    /// Whether one of this worker's commits was refused: it was abandoned,
    /// and makes no further call.
    refused: std::cell::Cell<bool>,
    /// While the worker attempts a terminal's owed first read, its node:
    /// the commit of the read's result marks it done.
    owed_attempt: Option<NodeId>,
    /// A caret read made for owed work, committed with what follows it.
    pending_caret_read: Option<(NodeId, u64)>,
    /// The end of the note this entry's final read clears, the one that
    /// lived as it arrived.
    final_note_until: Option<u64>,
    /// The caret read the entry's handling cut at its value change check,
    /// which ends the program: the reducer decides the check and
    /// dispatches its finish ([`Work::Finish`]).
    cut: Option<Box<Cut>>,
    /// The focus path of the UIA focus about to be reported, which its
    /// entry finds its element by before it is committed.
    focus_path: Option<FocusPath>,
    /// The element kept for the UIA focus about to be reported that the
    /// listener's evidence says is not its element, evicted by the
    /// report's commit ([`Stale`]).
    stale: Option<Stale>,
}

impl<'a> Worker<'a> {
    fn new(context: &'a RequestContext<'a>, client: &'a mut Client, timing: EventTiming) -> Self {
        Self {
            context,
            client,
            timing,
            refused: std::cell::Cell::new(false),
            owed_attempt: None,
            pending_caret_read: None,
            cut: None,
            final_note_until: None,
            focus_path: None,
            stale: None,
        }
    }
}

impl Worker<'_> {
    /// Commits `effects` for this worker: applied to the program's own
    /// copies and posted for the reducer, unless the worker was abandoned
    /// ([`RequestContext::commit`]).
    fn commit(&self, effects: Effects) -> Committed {
        let committed = self.context.commit(effects);
        if committed == Committed::Refused {
            self.refused.set(true);
        }
        committed
    }

    /// Whether this worker was abandoned, as a refused commit told it.
    fn refused(&self) -> bool {
        self.refused.get()
    }

    fn handle(&mut self, entry: Entry) {
        let Entry {
            item,
            trace,
            observed_at_us,
            ..
        } = entry;
        match item {
            Item::Msaa {
                kind,
                hwnd,
                id_object,
                id_child,
                ..
            } => self.msaa_event(kind, (hwnd, id_object, id_child), trace, observed_at_us),
            Item::Uia(event) => self.uia_event(event, trace, observed_at_us),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
            // Read now that nothing else waits, unless a read since saw it.
            Item::NotedCaret { node_id } => self.caret_of(node_id, trace, observed_at_us),
            // The key's read: the caret under its note first, whose report
            // compares it with the read before the key, then the screen,
            // whatever read came before.
            Item::ReadTerminal { node_id } => {
                if self.focused_terminal(node_id) {
                    tracing::debug!(%trace, ?node_id, "a key's read of its terminal");
                    self.read_under_note(node_id, NoteTrigger::Now, (trace, observed_at_us));
                    self.terminal_output(node_id, trace, 0);
                }
            }
            // The reducer's own: never a program's.
            Item::NodesHeld { .. } | Item::Barrier { .. } => {}
        }
    }

    /// A read of the caret for a key ([`TextOp::ReadCaret`]).
    fn key_read(
        &mut self,
        request_id: RequestId,
        trace: TraceId,
        node_id: NodeId,
        read: CaretRead,
    ) {
        match read.purpose {
            ReadPurpose::BeforeFieldKey | ReadPurpose::BeforeTerminalKey => {
                self.before_read(request_id, trace, node_id, read);
            }
            ReadPurpose::Final => self.final_read(request_id, trace, node_id, read),
        }
    }

    /// A held key's before read: reads the caret and answers the request
    /// with the read; one that fails answers `Unanswered`.
    fn before_read(
        &mut self,
        request_id: RequestId,
        trace: TraceId,
        node_id: NodeId,
        asked: CaretRead,
    ) {
        let (read, read_at) = text_reads::before_read(self.context, node_id, asked, true);
        let caret_read = read_at.map(|at| (node_id, at));
        match read {
            Ok(Checked::Cut(cut)) => self.cut_read(
                node_id,
                (cut.edit, cut.length, read_at),
                Finish::Before {
                    request_id,
                    trace,
                    read: cut.read,
                },
            ),
            Ok(Checked::Done(reply)) => {
                self.before_answered(
                    (request_id, trace),
                    node_id,
                    reply,
                    (caret_read, CallCounts::default()),
                );
            }
            Err(_) => self.key_unanswered(request_id, trace, caret_read, CallCounts::default()),
        }
    }

    /// The key's final read: one read with the unit, compared with the
    /// node's last read, answering the request.
    fn final_read(
        &mut self,
        request_id: RequestId,
        trace: TraceId,
        node_id: NodeId,
        asked: CaretRead,
    ) {
        let note = NoteFetch {
            unit: asked.fetch.unit,
            motion: asked.fetch.motion,
        };
        let (checked, read_at) = text_reads::note_read(self.context, node_id, note, (true, false));
        if let Some(until_us) = self.final_note_until {
            text_reads::clear_note(self.context, node_id, until_us);
        }
        let caret_read = read_at.map(|at| (node_id, at));
        let reply = match checked {
            Checked::Done(KeyChecked::Read { read, .. }) => TextReply::Caret(read),
            // A final read that fails has nothing to answer from; it is
            // never cut.
            Checked::Done(KeyChecked::Moving | KeyChecked::Failed) | Checked::Cut(_) => {
                TextReply::Unanswered
            }
        };
        let result = with_calls(
            self.reply_message(
                request_id,
                trace,
                QueryOutcome::Done(QueryResult::Text(reply)),
            ),
            take_calls(),
        );
        self.commit(Effects {
            result: Some(result),
            caret_read,
            ..Effects::default()
        });
    }

    /// Answers a key's request `Unanswered`, its read having failed.
    fn key_unanswered(
        &mut self,
        request_id: RequestId,
        trace: TraceId,
        caret_read: Option<(NodeId, u64)>,
        calls: CallCounts,
    ) {
        let result = with_calls(
            self.reply_message(
                request_id,
                trace,
                QueryOutcome::Done(QueryResult::Text(TextReply::Unanswered)),
            ),
            calls,
        );
        self.commit(Effects {
            result: Some(result),
            caret_read,
            ..Effects::default()
        });
    }

    /// Answers a key's request with `reply`, its before read, carrying
    /// `calls`, made by an earlier part of the read, and on the console
    /// host starts the note's poll from the read's end.
    fn before_answered(
        &mut self,
        (request_id, trace): (RequestId, TraceId),
        node_id: NodeId,
        reply: CaretReply,
        (caret_read, calls): (Option<(NodeId, u64)>, CallCounts),
    ) {
        self.note_console_poll(node_id, reply.facts.read_ended_at_us);
        let result = with_calls(
            self.reply_message(
                request_id,
                trace,
                QueryOutcome::Done(QueryResult::Text(TextReply::Caret(Box::new(reply)))),
            ),
            calls,
        );
        self.commit(Effects {
            result: Some(result),
            caret_read,
            ..Effects::default()
        });
    }

    /// On the focused console host, the before read's end starts its
    /// note's poll, so a slow read does not use up the poll.
    fn note_console_poll(&mut self, node_id: NodeId, read_ended_at_us: u64) {
        if !text_reads::console_focus(self.context, node_id) {
            return;
        }
        if let Some(note) = self.live_note(node_id) {
            text_reads::note_poll(self.context, node_id, note.until_us, read_ended_at_us);
        }
    }

    /// Node `node_id`'s note, when it has one that has not ended by now.
    fn live_note(&self, node_id: NodeId) -> Option<KeyNote> {
        self.context
            .facts()
            .text
            .live_note(node_id.number(), now_us())
    }

    /// Reads `node_id` under its live note, as the application reports a
    /// caret change, a text change or a selection change in it: the read
    /// fetches the key's unit and is reported with its facts. An event
    /// observed before the caret was last read shows nothing that read did
    /// not see, and reads nothing. Returns whether it read, or cut its read
    /// at the value change check, to be answered by the program that
    /// finishes it: either ends the entry's handling, since the read is the
    /// caret's. A text change reads only while the note has seen no effect;
    /// a caret event after one reads as the note's before reads do.
    fn read_under_note(
        &mut self,
        node_id: NodeId,
        trigger: NoteTrigger,
        (trace, observed_at_us): (TraceId, u64),
    ) -> bool {
        if trigger.needs_new_caret()
            && self
                .context
                .caret_read_since(node_id, self.timing.observed_at_us)
        {
            return false;
        }
        let Some(note) = self.live_note(node_id) else {
            return false;
        };
        let Some(shape) = trigger.shape(note.reads) else {
            return false;
        };
        match shape {
            ReadShape::Compared => self.note_read(node_id, note, (trace, observed_at_us), true),
            ReadShape::Reported => {
                let terminal = self.focused_terminal(node_id);
                if terminal {
                    // A terminal's report compares nothing: after its first
                    // effect, its reading goes by its own path.
                    return false;
                }
                self.caret_with_facts(node_id, (trace, observed_at_us), note.fetch);
                true
            }
        }
    }

    /// One read of `node_id` under `note`, with the check a caret event
    /// makes, reported with its facts: a read showing the key's effect
    /// narrows the note. With `cuts`, the read of an edit control is cut at
    /// its value change check.
    fn note_read(
        &mut self,
        node_id: NodeId,
        note: KeyNote,
        (trace, observed_at_us): (TraceId, u64),
        cuts: bool,
    ) -> bool {
        let context = self.context;
        let (checked, read_at) = text_reads::note_read(context, node_id, note.fetch, (false, cuts));
        let caret_read = read_at.map(|at| (node_id, at));
        match checked {
            Checked::Done(found) => {
                self.note_found(
                    node_id,
                    note.until_us,
                    (trace, observed_at_us),
                    found,
                    (caret_read, CallCounts::default()),
                );
            }
            Checked::Cut(cut) => {
                self.cut_read(
                    node_id,
                    (cut.edit, cut.length, read_at),
                    Finish::Note {
                        trace,
                        observed_at_us,
                        until_us: note.until_us,
                        read: cut.read,
                    },
                );
            }
        }
        true
    }

    /// Commits what a read under a note found: the read as a report with
    /// its facts, a terminal's only when it shows the key's effect, since
    /// otherwise its reading goes by its own path; a read showing an effect
    /// narrows the note. Whether a report was committed with an effect.
    fn note_found(
        &mut self,
        node_id: NodeId,
        until_us: u64,
        (trace, observed_at_us): (TraceId, u64),
        found: KeyChecked,
        (caret_read, calls): (Option<(NodeId, u64)>, CallCounts),
    ) -> bool {
        let terminal = self.focused_terminal(node_id);
        let (read, effect) = match found {
            KeyChecked::Read { read, effect } => (Some(read), effect),
            KeyChecked::Moving | KeyChecked::Failed => (None, false),
        };
        let Some(read) = read.filter(|_| effect || !terminal) else {
            self.commit(Effects {
                caret_read,
                ..Effects::default()
            });
            return false;
        };
        if effect {
            text_reads::note_effect(self.context, node_id, until_us);
        }
        let reply = *read;
        let sent_caret = terminal.then(|| (node_id, reply.caret.clone()));
        let event = NormalizedEvent::CaretMoved {
            node_id,
            caret: reply.caret,
            selection_changes: reply.selection_changes,
            content_changed: reply.content_changed,
            new_selection: reply.new_selection,
            facts: Some(Box::new(reply.facts)),
        };
        let backend = self.node_backend(node_id);
        let window = self.context.tracked_window();
        let message = with_calls(
            self.event_message(trace, observed_at_us, backend, window, event),
            calls,
        );
        let applied = self.commit(Effects {
            result: Some(message),
            sent_caret,
            caret_read,
            ..Effects::default()
        }) == Committed::Applied;
        applied && effect
    }

    /// The node's backend, by which store knows it.
    fn node_backend(&self, node_id: NodeId) -> Backend {
        if self.context.store.runtime_id_of(node_id).is_some() {
            Backend::Uia
        } else {
            Backend::Msaa
        }
    }

    /// [`Work::NoteRead`]: one read of the console poll, under the node's
    /// note, with the check a caret event makes. On evidence it reads the
    /// terminal, as after a caret event that shows one.
    fn note_poll(&mut self, node_id: NodeId) {
        let Some(note) = self.live_note(node_id) else {
            return;
        };
        let trace = TraceId::mint();
        let (checked, read_at) =
            text_reads::note_read(self.context, node_id, note.fetch, (false, false));
        let caret_read = read_at.map(|at| (node_id, at));
        let Checked::Done(found) = checked else {
            return;
        };
        let effect = self.note_found(
            node_id,
            note.until_us,
            (trace, now_us()),
            found,
            (caret_read, CallCounts::default()),
        );
        // Core's reading rules answer the key from a read that shows what
        // it did.
        if effect && self.focused_terminal(node_id) {
            self.terminal_output(node_id, trace, 0);
        }
    }

    /// Whether `node_id` is the focus this outpost last reported.
    fn is_focus(&self, node_id: NodeId) -> bool {
        let object = if let Some(runtime_id) = self.context.store.runtime_id_of(node_id) {
            Object::Uia(runtime_id)
        } else if let Some((hwnd, object, child)) = self.context.store.key_of(node_id) {
            Object::Msaa(hwnd, object, child)
        } else {
            return false;
        };
        self.context.focused() == Some(object)
    }

    /// Reports the caret of `node_id` as `CaretMoved`, a node the outpost has
    /// not released, when the worker has not read its caret since the event
    /// that asked: a caret key's answer, read after the event, already told
    /// Core. Core targets which node's caret it takes.
    /// A focus's own caret comes with the focus
    /// ([`read_focus_text`](Self::read_focus_text)). The read is noted as
    /// covering the events observed before it only in the commit of what it
    /// sends, or of nothing when it sends nothing.
    fn caret_of(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        if !self.context.holds(node_id)
            || self
                .context
                .caret_read_since(node_id, self.timing.observed_at_us)
        {
            return;
        }
        let started = std::time::Instant::now();
        let terminal = self.focused_terminal(node_id);
        let purpose = if terminal {
            text_reads::CaretFor::Terminal
        } else {
            text_reads::CaretFor::Event
        };
        let (report, read_at) = text_reads::report_caret(self.context, node_id, purpose, None);
        // A caret event's read, which a terminal's output causes too, as
        // its cursor moves.
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            "caret read timing"
        );
        let report = match report {
            Ok(Checked::Cut(cut)) => {
                let read = cut.read;
                self.cut_read(
                    node_id,
                    (cut.edit, cut.length, read_at),
                    Finish::Report {
                        trace,
                        observed_at_us,
                        read,
                    },
                );
                return;
            }
            Ok(Checked::Done(report)) => Ok(report),
            Err(missed) => Err(missed),
        };
        let caret_read = read_at.map(|at| (node_id, at));
        self.send_caret(
            node_id,
            (trace, observed_at_us),
            (report, terminal),
            (caret_read, CallCounts::default()),
        );
    }

    /// Reports the caret of `node_id`, read with the key's unit and facts
    /// for its note: a caret event after the key's effect.
    fn caret_with_facts(&mut self, node_id: NodeId, event: (TraceId, u64), fetch: NoteFetch) {
        let (report, read_at) = text_reads::report_caret(
            self.context,
            node_id,
            text_reads::CaretFor::Event,
            Some(fetch),
        );
        self.report_read(node_id, event, report, read_at);
    }

    /// Sends or cuts a caret event's read, as a caret event's report does.
    fn report_read(
        &mut self,
        node_id: NodeId,
        (trace, observed_at_us): (TraceId, u64),
        report: text_reads::ReportOutcome,
        read_at: Option<u64>,
    ) {
        let report = match report {
            Ok(Checked::Cut(cut)) => {
                let read = cut.read;
                self.cut_read(
                    node_id,
                    (cut.edit, cut.length, read_at),
                    Finish::Report {
                        trace,
                        observed_at_us,
                        read,
                    },
                );
                return;
            }
            Ok(Checked::Done(report)) => Ok(report),
            Err(missed) => Err(missed),
        };
        let caret_read = read_at.map(|at| (node_id, at));
        self.send_caret(
            node_id,
            (trace, observed_at_us),
            (report, false),
            (caret_read, CallCounts::default()),
        );
    }

    /// Ends the program at the caret read of edit control `node_id`, its
    /// window `edit`, cut at its value change check: the read, with the
    /// text's length if it was read and when the read began, goes to the
    /// reducer with the program's end, and the calls made since the last
    /// message go with it to the finish's message ([`Work::Finish`]).
    fn cut_read(
        &mut self,
        node_id: NodeId,
        (edit, length, read_at): (isize, Option<u32>, Option<u64>),
        finish: Finish,
    ) {
        self.cut = Some(Box::new(Cut {
            edit,
            node: node_id,
            length,
            read_at,
            timing: self.timing,
            calls: take_calls(),
            finish,
        }));
    }

    /// Finishes `cut`, a caret read cut at its value change check, with
    /// the reducer's decision, `waiting`: makes the comparison and the rest
    /// of the read's calls, and sends and commits what the read would have
    /// uncut, its message carrying the read part's calls with its own.
    fn finish(&mut self, cut: Cut, waiting: bool) {
        let Cut {
            node: node_id,
            length,
            read_at,
            calls,
            finish,
            ..
        } = cut;
        let caret_read = read_at.map(|at| (node_id, at));
        match finish {
            Finish::Report {
                trace,
                observed_at_us,
                read,
            } => {
                let report =
                    text_reads::finish_caret(self.context, node_id, (length, read), waiting)
                        .map(|(caret, selection)| (caret, false, selection));
                self.send_caret(
                    node_id,
                    (trace, observed_at_us),
                    (report, false),
                    (caret_read, calls),
                );
            }
            Finish::Before {
                request_id,
                trace,
                read,
            } => match text_reads::finish_before(self.context, node_id, (length, read), waiting) {
                Ok(reply) => {
                    self.before_answered((request_id, trace), node_id, reply, (caret_read, calls));
                }
                Err(_) => self.key_unanswered(request_id, trace, caret_read, calls),
            },
            Finish::Note {
                trace,
                observed_at_us,
                until_us,
                read,
            } => {
                let found = text_reads::finish_note(self.context, node_id, (length, read), waiting);
                self.note_found(
                    node_id,
                    until_us,
                    (trace, observed_at_us),
                    found,
                    (caret_read, calls),
                );
            }
        }
    }

    /// Sends the caret `report` of `node_id` read for the caret event of
    /// `trace` observed at `observed_at_us`, as `CaretMoved`, unless it was
    /// not read or is the caret last sent to Core for the terminal it is
    /// (`terminal`), committing it with the read noted (`caret_read`), and
    /// carrying `calls`, made by an earlier part of the read, with the
    /// calls since the last message.
    fn send_caret(
        &mut self,
        node_id: NodeId,
        (trace, observed_at_us): (TraceId, u64),
        (report, terminal): (
            Result<(CaretReport, bool, text_reads::SelectionReport), text_reads::NoCaret>,
            bool,
        ),
        (caret_read, calls): (Option<(NodeId, u64)>, CallCounts),
    ) {
        let (event, sent_caret) = match report {
            Ok((caret, false, selection)) => (
                NormalizedEvent::CaretMoved {
                    node_id,
                    caret: caret.clone(),
                    selection_changes: selection.changes,
                    content_changed: selection.content_changed,
                    new_selection: selection.new_selection,
                    facts: selection.facts,
                },
                terminal.then_some((node_id, caret)),
            ),
            unsent => {
                if matches!(unsent, Ok((_, true, _))) {
                    tracing::debug!(
                        "the terminal's caret is the one last sent; it is not sent again"
                    );
                }
                self.commit(Effects {
                    caret_read,
                    ..Effects::default()
                });
                return;
            }
        };
        let backend = if self.context.store.runtime_id_of(node_id).is_some() {
            Backend::Uia
        } else {
            Backend::Msaa
        };
        let window = self.context.tracked_window();
        let message = with_calls(
            self.event_message(trace, observed_at_us, backend, window, event),
            calls,
        );
        self.commit(Effects {
            result: Some(message),
            sent_caret,
            caret_read,
            ..Effects::default()
        });
    }

    /// Whether `node_id` is the focused terminal and its reading is held
    /// with its text changed since its last read
    /// ([`crate::terminal::reading::Reading::Held`]): output is arriving
    /// that nobody reads until Core asks, and the caret moves with it.
    fn held_with_changes(&self, node_id: NodeId) -> bool {
        self.focused_terminal(node_id)
            && self.context.store.terminal_reading(node_id) == Some(Reading::Held { changed: true })
    }

    /// After a console update of the focused console terminal `node_id`
    /// in window `hwnd`: while its reading is held with its text changed
    /// and no key's note lives on it, the worker would do nothing for
    /// another update, so the intake drops the window's updates without
    /// waking it (`Marks::absorbed`) until a request, a
    /// cancel, a focus, or a caret key clears the mark. A held burst's
    /// updates, about one per line, otherwise woke the worker once each.
    fn absorb_while_held(&self, node_id: NodeId, hwnd: isize) {
        if self.held_with_changes(node_id) && self.live_note(node_id).is_none() {
            self.commit(Effects {
                mark: Some(MarkChange::Absorb(hwnd)),
                ..Effects::default()
            });
        }
    }

    /// Notes a caret event of the held terminal `node_id` whose text has
    /// changed, instead of reading its caret: during a flood held for Core
    /// it comes with nearly every write, and reading each told Core of a
    /// caret the next write moved again. Its caret is read once nothing else
    /// is waiting ([`Item::NotedCaret`]), so Core's caret, and a review
    /// cursor following it, catch up when the burst ends; the read Core
    /// asks for reads the caret with the text, and covers it if it comes
    /// first. NVDA's console handler likewise queues a caret event only
    /// when one is not already pending (`winConsoleHandler.py`, 169).
    fn note_caret(&self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        tracing::debug!(%trace, "a held terminal's caret event is noted");
        self.commit(Effects {
            noted_caret: Some((node_id, trace, observed_at_us, self.timing)),
            ..Effects::default()
        });
    }

    /// A caret event of the text focus `node_id`, from UIA or, for the
    /// console host's text area, from its console: read under a key's note
    /// when one lives, and otherwise the caret reported, or noted while
    /// the terminal's reading is held with its text changed.
    ///
    /// In a terminal Core's reading rules answer a key, so the caret event
    /// read under its note reads the terminal; and every caret event of a focused
    /// terminal reads it, the read taking the caret too, so that the rules
    /// judge the next key from the caret the terminal moved: Windows
    /// Terminal sometimes moves its caret with no text change, and the
    /// console host's caret event is its cursor's move
    /// (`books/dev/src/explanation/design/terminal-model.md`, "Where it runs": one caret).
    fn caret_event(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        if self.read_under_note(node_id, NoteTrigger::Caret, (trace, observed_at_us)) {
            if self.focused_terminal(node_id) {
                self.terminal_output(node_id, trace, observed_at_us);
            }
            return;
        }
        if self.refused() {
            return;
        }
        if self.held_with_changes(node_id) {
            self.note_caret(node_id, trace, observed_at_us);
        } else if self.focused_terminal(node_id) {
            self.terminal_output(node_id, trace, observed_at_us);
        } else {
            self.caret_of(node_id, trace, observed_at_us);
        }
    }

    /// The terminal that is the text area of the console window `hwnd`,
    /// whose console events are its own: the console terminal Core's
    /// `Follow` names, else the followed focus when it is that terminal
    /// ([`focused_terminal`](Self::focused_terminal)).
    fn focused_console_terminal(&self, hwnd: isize) -> Option<NodeId> {
        // The console terminal Core's `Follow` names.
        if let Some((node_id, true)) = super::followed::terminal(self.context) {
            return Some(node_id);
        }
        let followed = self.context.follow(|follow| {
            follow
                .followed
                .as_ref()
                .filter(|followed| followed.window == Some(hwnd))
                .map(|followed| followed.node)
        });
        followed.filter(|&node_id| self.focused_terminal(node_id))
    }

    /// Whether `node_id` is a terminal read through UIA whose new output
    /// the outpost finds by diffing its text: one it followed as a focus
    /// and has not released, which keeps its follow entry until then. Its
    /// output is read whether or not it is the newest focus, since Core
    /// speaks output only for its own focus, and Core's focus is among the
    /// nodes the outpost has not released
    /// (`books/dev/src/explanation/design/ownership.md`).
    fn focused_terminal(&self, node_id: NodeId) -> bool {
        self.context.store.runtime_id_of(node_id).is_some()
            && self.context.store.has_terminal(node_id)
    }

    /// Takes a focused terminal's on-demand reading through `event`
    /// ([`crate::terminal::reading`]) and does what the transition says:
    /// reads and sends the read, answers Core's request (`request`, its id
    /// and trace) with it, or only notes the change. A request the
    /// transition reads nothing for is answered that nothing changed. The
    /// transition is made on a copy of the terminal's entry, `sent_caret`
    /// noted on it as the caret last sent when given, and only the commit of
    /// what follows puts the copy in place.
    fn terminal_event(
        &mut self,
        node_id: NodeId,
        event: Event,
        request: Option<(RequestId, TraceId)>,
        (trace, _): (TraceId, u64),
        sent_caret: Option<CaretReport>,
    ) {
        // Anything but a change of text may end the hold, and its read must
        // see every update observed after it begins: the reducer stopped
        // absorbing them as it dispatched this program.
        let mut terminal = self.terminal_copy(node_id);
        if sent_caret.is_some() {
            terminal.sent_caret = sent_caret;
        }
        if event == Event::Focused {
            terminal.focused();
        }
        let (reading, action) = terminal.reading.next(event);
        terminal.reading = reading;
        match action {
            Action::Nothing | Action::ReplyEmpty => {
                let result = request
                    .map(|request| self.reply_text_message(request, TextReply::TerminalUnchanged));
                let owed_done = self.owed_attempt.take();
                self.commit(Effects {
                    result,
                    terminal: Some((node_id, terminal)),
                    owed_done,
                    ..Effects::default()
                });
                return;
            }
            Action::Report | Action::Reply | Action::ReplySilently | Action::Baseline => {}
        }
        let owed = request.filter(|_| matches!(action, Action::Reply | Action::ReplySilently));
        self.read_terminal(
            node_id,
            terminal,
            owed,
            (None, action == Action::Reply, false),
            trace,
        );
    }

    /// A copy of terminal `node_id`'s entry, as committed, for a read to
    /// work on; a new terminal's when it has none.
    fn terminal_copy(&self, node_id: NodeId) -> crate::terminal::Terminal {
        self.context.store.terminal(node_id).unwrap_or_default()
    }

    /// A copy of terminal `node_id`'s entry, as committed, when it has one,
    /// for a read of its history or a search, made with no lock held.
    fn terminal_seen(&self, node_id: NodeId) -> Option<crate::terminal::Terminal> {
        self.context.store.terminal(node_id)
    }

    /// Reads the focused terminal `node_id` into `terminal`, a copy of its
    /// entry, the read of `scope` (a live read for `None`), and sends it to
    /// Core with the caret read with it: as the answer to Core's request
    /// `owed`, or as an event unless it is identical to the last read sent
    /// ([`crate::terminal::Terminal::sends`]). `waited` says whether the
    /// read waited for Core's speech queue. A read that failed answers
    /// `owed` as unanswered. Whatever the read found, its commit puts the
    /// copy in place of the entry; an abandoned worker's copy is dropped.
    fn read_terminal(
        &mut self,
        node_id: NodeId,
        mut terminal: crate::terminal::Terminal,
        owed: Option<(RequestId, TraceId)>,
        (scope, waited, before_key): (Option<verbatim_model::TerminalScope>, bool, bool),
        trace: TraceId,
    ) {
        let read = match self.client.uia() {
            Some(uia) => {
                let started = std::time::Instant::now();
                let read = text_reads::terminal_read(
                    self.context,
                    uia,
                    node_id,
                    &mut terminal,
                    (scope, waited, before_key),
                );
                // Each read, end to end, and how long its event waited in
                // the queue: during a flood the reads follow one another, so
                // these say how much of the time the outpost spends in the
                // terminal.
                tracing::debug!(
                    queued_us = self
                        .timing
                        .dequeued_at_us
                        .saturating_sub(self.timing.observed_at_us),
                    elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    "terminal read timing"
                );
                read
            }
            None => None,
        };
        let Some((answer, caret, read_at)) = read else {
            // Gone or failed: nothing to send, and Core's request is
            // answered as unanswered.
            let result =
                owed.map(|request| self.reply_text_message(request, TextReply::Unanswered));
            let owed_done = self.owed_attempt.take();
            let caret_read = self.pending_caret_read.take();
            self.commit(Effects {
                result,
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        };
        let caret_read = read_at
            .map(|at| (node_id, at))
            .or_else(|| self.pending_caret_read.take());
        let mut snapshot = answer.snapshot;
        // The caret read with the text goes with it: the console host
        // raises no caret event for every character typed, so this keeps
        // Core's caret following typing. A key's note is read
        // first, as the caret may have moved since the change that caused
        // the read.
        if let Some(caret) = caret {
            if !before_key {
                self.read_under_note(node_id, NoteTrigger::Now, (trace, answer.started_us));
            }
            if self.refused() {
                return;
            }
            terminal.sent_caret = Some(caret.report.clone());
            snapshot.caret = Some(caret.report);
        }
        let read_at_us = answer.started_us;
        let owed_done = self.owed_attempt.take();
        if let Some(request) = owed {
            terminal.answered(&snapshot);
            let result =
                self.reply_text_message(request, TextReply::TerminalSnapshot(Box::new(snapshot)));
            self.commit(Effects {
                result: Some(result),
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        }
        if !terminal.sends(&snapshot) {
            tracing::debug!("the terminal's read is the one last sent; it is not sent again");
            self.commit(Effects {
                terminal: Some((node_id, terminal)),
                owed_done,
                caret_read,
                ..Effects::default()
            });
            return;
        }
        let window = self.context.tracked_window();
        // What the read found is stamped with when the read began, not with
        // when the change that caused it was observed: a read that began
        // after a key may show what the key did.
        let message = self.event_message(
            trace,
            read_at_us,
            Backend::Uia,
            window,
            NormalizedEvent::TerminalSnapshot {
                node_id,
                snapshot: Box::new(snapshot),
            },
        );
        self.commit(Effects {
            result: Some(message),
            terminal: Some((node_id, terminal)),
            owed_done,
            caret_read,
            ..Effects::default()
        });
    }

    /// The answer `reply` to Core's request `request`.
    fn reply_text_message(
        &self,
        (request_id, trace): (RequestId, TraceId),
        reply: TextReply,
    ) -> OutpostToCore {
        self.reply_message(
            request_id,
            trace,
            QueryOutcome::Done(QueryResult::Text(reply)),
        )
    }

    /// Answers Core's request `request` with `reply`.
    fn reply_text(&self, request: (RequestId, TraceId), reply: TextReply) {
        let result = self.reply_text_message(request, reply);
        self.commit(Effects {
            result: Some(result),
            ..Effects::default()
        });
    }

    /// Answers Core's request of the focused terminal `node_id` for a read
    /// at once ([`TextOp::TerminalReadNow`], and a before read ([`ReadPurpose::BeforeTerminalKey`]), the
    /// read before a held key, `books/dev/src/explanation/design/key-holding.md`), whatever its
    /// on-demand state. A fresh read before a review command clears a held
    /// state's mark that the text changed, since it reads what was held.
    fn terminal_read_now(
        &mut self,
        request: (RequestId, TraceId),
        node_id: NodeId,
        scope: verbatim_model::TerminalScope,
    ) {
        let mut terminal = self.terminal_copy(node_id);
        if scope == verbatim_model::TerminalScope::Fresh
            && terminal.reading == (Reading::Held { changed: true })
        {
            terminal.reading = Reading::Held { changed: false };
        }
        self.read_terminal(
            node_id,
            terminal,
            Some(request),
            (Some(scope), false, false),
            request.1,
        );
    }

    /// A focused terminal's text changed, or another event asks for a read:
    /// read at once or noted, as its on-demand reading stands. A change
    /// observed before the last read began is covered by that read and
    /// changes nothing (a console terminal's text change comes by UIA and
    /// as a console update both). An `observed_at_us` of 0 is covered by
    /// nothing.
    fn terminal_output(&mut self, node_id: NodeId, trace: TraceId, observed_at_us: u64) {
        let covered =
            observed_at_us != 0 && self.context.store.terminal_covers(node_id, observed_at_us);
        if covered {
            return;
        }
        self.terminal_event(
            node_id,
            Event::TextChanged,
            None,
            (trace, observed_at_us),
            None,
        );
    }

    /// What the caret read of a focus about to be reported finds, for a
    /// focus that may have text or whose role says it may, `None` for any
    /// other, with when the read began: read while the worker handles the
    /// focus, so the line comes with the focus and nothing is reported
    /// between them, as NVDA's object announcement reads an object's text
    /// within it (`docs/nvda/speech.md`, "What an object with text says").
    /// When something is selected, the selected text is read too, which
    /// Core speaks in place of the line, a protected (password) field's
    /// too, masked as the control gives it. A focus with no text to read is
    /// [`FocusText::NoText`], and Core speaks its value; one whose text
    /// could not be read now is [`FocusText::NotRead`], and Core speaks
    /// neither, never the value, which for a document is all of its text.
    /// So is a focus whose node still keeps the element the listener's
    /// evidence says is no longer its own ([`Stale`]), its own not found:
    /// that element's text is not the focus's.
    fn read_focus_text(&self, node_id: NodeId, role: Role) -> (Option<FocusText>, Option<u64>) {
        let context = self.context;
        text_reads::forget_no_text(context, node_id);
        let role_has_text = matches!(role, Role::EditableText | Role::Document | Role::Terminal);
        if !role_has_text && !text_reads::may_have_text(context, node_id, role) {
            return (None, None);
        }
        if self.keeps_stale_element(node_id) {
            tracing::debug!(
                "the focus's node still keeps the element its runtime id named before; its text is not read"
            );
            return (Some(FocusText::NotRead), None);
        }
        let started = Instant::now();
        let note = self.live_note(node_id);
        let (read, read_at) = text_reads::report_caret(
            context,
            node_id,
            text_reads::CaretFor::Focus,
            note.map(|note| note.fetch),
        );
        let text = match read {
            Ok(Checked::Done((caret, _, selection))) => {
                if let (Some(note), Some(facts)) = (note, &selection.facts)
                    && matches!(facts.change, Change::Compared { compared, .. } if had_effect(compared, note.fetch.motion))
                {
                    text_reads::note_effect(context, node_id, note.until_us);
                }
                let facts = selection.facts;
                let changes = CaretChanges {
                    selection_changes: selection.changes,
                    content_changed: selection.content_changed,
                    new_selection: selection.new_selection,
                };
                let selected_text = caret.selection.as_ref().and_then(|selection| {
                    let op = TextOp::ReadRange {
                        start: TextPoint::At(selection.range.start),
                        end: TextPoint::At(selection.range.end),
                        through: None,
                    };
                    match text_reads::answer(context, node_id, &op) {
                        TextReply::Range { text, .. } => Some(text),
                        _ => None,
                    }
                });
                FocusText::Caret {
                    caret,
                    selected_text,
                    changes,
                    facts,
                }
            }
            Err(text_reads::NoCaret::NoText) => FocusText::NoText,
            // Only a caret event's read is cut, never a focus's.
            Ok(Checked::Cut(_)) | Err(text_reads::NoCaret::NotRead) => {
                tracing::debug!("the focus's caret could not be read");
                FocusText::NotRead
            }
        };
        tracing::debug!(
            elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            "the focus's caret read"
        );
        (Some(text), read_at)
    }

    /// Attempts `owed`, the work the followed focus still owes, a program of
    /// its own, which the reducer dispatches once per worker generation
    /// under a deadline of its own with no moved-on window: a terminal's
    /// first read, which speaks nothing, since what it held before the focus
    /// arrived is not new output, taken once the subscription has moved to
    /// the terminal, so output written after the read began raises a change
    /// that reads it (`books/dev/src/explanation/design/terminal-model.md`, "Reading rules", rule
    /// 15), from the caret read with the terminal's report. The commit of
    /// the attempt's result clears it, read or failed.
    pub(super) fn attempt_owed(&mut self, program: OwedProgram) {
        let OwedProgram {
            node: node_id,
            owed,
        } = program;
        match owed.work {
            OwedWork::FirstRead(caret) => {
                if !self.focused_terminal(node_id) {
                    self.commit(Effects {
                        owed_done: Some(node_id),
                        ..Effects::default()
                    });
                    return;
                }
                if let Some(subscription) = self.context.focus_events.get() {
                    subscription.settle();
                }
                self.owed_attempt = Some(node_id);
                self.terminal_event(
                    node_id,
                    Event::Focused,
                    None,
                    (owed.trace, owed.observed_at_us),
                    Some(caret),
                );
                self.owed_attempt = None;
                self.pending_caret_read = None;
            }
        }
    }

    /// The subscription's target for the focus `node_id` of `role`, with
    /// `chain`, its ancestors and itself, once its parent, a UIA slider of
    /// class `SensitiveSlider` whose element this outpost does not keep,
    /// is found by its path (`docs/parity/focus-context.md`, "A slider that
    /// focuses its thumb"): one connection call and one remote operation,
    /// and the slider kept under the node its runtime id names, the
    /// parent's. `None` when the parent is no such slider or is not found.
    fn slider_target(
        &mut self,
        node_id: NodeId,
        role: Role,
        chain: &[NodeSnapshot],
    ) -> Option<(NodeId, KeptTarget)> {
        let parent = sensitive_slider_of(node_id, chain)?;
        // The listener's read keeps no ancestor's element: the slider is
        // found by its path, one connection call and one remote operation,
        // and kept under the node its runtime id names, the parent's.
        let runtime_id = self.context.store.runtime_id_of(parent)?;
        let found = self.element_by_focus_path(&runtime_id)?;
        let kept = self.context.nodes().id_for_element(&runtime_id, &found);
        let slider = self.context.store.element_of(kept)?;
        let (target, _) = target_for(self.context, (node_id, role), chain, false);
        let mut target = target?;
        target.slider = Some((kept, slider));
        Some((node_id, target))
    }

    /// Emits `event` stamped `observed_at_us`, in microseconds on the run's
    /// shared clock (0 when not known), as the entry's result.
    fn emit(
        &self,
        trace: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> bool {
        let message = self.event_message(trace, observed_at_us, backend, window, event);
        self.commit(Effects {
            result: Some(message),
            ..Effects::default()
        }) == Committed::Applied
    }

    /// `event` as the message that reports it ([`event_message`]).
    fn event_message(
        &self,
        trace: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<isize>,
        event: NormalizedEvent,
    ) -> OutpostToCore {
        event_message(
            self.context,
            self.timing,
            (trace, observed_at_us),
            (backend, window),
            event,
        )
    }

    /// What follows the commit that made `node_id`, of `role`, the followed
    /// focus: the lookup of the slider a `SensitiveSlider`'s thumb is in,
    /// when `slider` names one whose element is not kept, and a second
    /// commit adding it while the focus is still followed. The work the
    /// focus owes is a program of its own, which the reducer dispatches
    /// next.
    fn after_follow(&mut self, node_id: NodeId, role: Role, slider: Option<NodeId>) {
        if slider.is_some() {
            let chain = self.context.follow(followed_chain);
            if let Some(element) = self.slider_target(node_id, role, &chain) {
                self.commit(Effects {
                    element: Some(element),
                    ..Effects::default()
                });
            }
        }
    }

    /// Whether an event named `event` from `hwnd` is accepted by NVDA's
    /// test against the foreground window as the event is handled
    /// (`acceptance`), made before any call into the application. A dropped
    /// event is logged.
    fn accepts(&self, event: EventName, hwnd: isize) -> bool {
        let windows = SystemWindows {
            foreground: self.context.foreground_window(),
        };
        let accepted = acceptance::accepts_event(
            event,
            Window::new(hwnd),
            &windows,
            &self.context.requested_events,
        );
        if !accepted {
            tracing::debug!(
                hwnd,
                ?event,
                foreground = windows.foreground,
                "event dropped: its window is not related to the foreground window"
            );
        }
        accepted
    }

    /// Whether a UIA entry's own window, `hwnd`, is hung or ghosted, by the
    /// outpost's reader, local calls never made for 0, which passes, as
    /// NVDA's test passes a sender with no cached window; a hung window is
    /// the evidence of a hang its program's commit names.
    fn hung(&self, hwnd: isize) -> bool {
        hwnd != 0 && self.context.readability(hwnd).is_hung()
    }

    /// An MSAA event from this outpost's own hooks.
    fn msaa_event(
        &mut self,
        kind: WinKind,
        (hwnd, id_object, id_child): (isize, i32, i32),
        trace: TraceId,
        observed_at_us: u64,
    ) {
        match kind {
            WinKind::ConsoleUpdate => {
                // The console host's text changed: its updates are the
                // focused console terminal's text changes, as its UIA text
                // changes are not subscribed to
                // (`books/dev/src/reference/crates/verbatim-outpost.md`,
                // "Terminals").
                if let Some(node_id) = self.focused_console_terminal(hwnd) {
                    // As for a UIA text change: a key's note is read before
                    // the read, which reads the caret too and would leave the
                    // change nothing new to show.
                    self.read_under_note(node_id, NoteTrigger::Text, (trace, observed_at_us));
                    self.terminal_output(node_id, trace, observed_at_us);
                    self.absorb_while_held(node_id, hwnd);
                }
            }
            WinKind::ConsoleCaret => {
                // The console host's cursor moved: the focused console
                // terminal's caret event, as its UIA caret events are not
                // subscribed to either.
                if let Some(node_id) = self.focused_console_terminal(hwnd) {
                    // A note's poll goes on unless the event's read shows the
                    // key's effect: a blink tick left over from an earlier
                    // cursor set can come before the key's move.
                    self.caret_event(node_id, trace, observed_at_us);
                }
            }
            WinKind::Destroy => {
                if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                    self.context.record(VerdictChange::Forget(hwnd));
                    self.context.forget_window(hwnd);
                    // A reused window handle must never inherit these nodes;
                    // their objects are released once the store's lock is
                    // let go, and their text facts are forgotten with the
                    // program's commit.
                    let (removed, gone) = self.context.store.forget_window(hwnd);
                    drop(removed);
                    text_reads::forget_nodes(self.context, &gone);
                    super::followed::window_destroyed(self.context, hwnd, &gone);
                }
            }
        }
    }

    /// A UIA event from this outpost's own subscriptions, or a selection or
    /// notification fact from the listener. NVDA's order: whether the event
    /// is the focus's, else whether its window is UIA's, then the acceptance
    /// test against the foreground window, and only then the event's own
    /// handling (`IUIAutomationPropertyChangedEventHandler_HandlePropertyChangedEvent`
    /// and its siblings, `source/UIAHandler/__init__.py`). Verbatim makes
    /// the window's UIA test for the focus's own events too, where NVDA
    /// skips it; the focus's window passed it when the focus was taken, so
    /// the outcome is the same.
    fn uia_event(&mut self, event: UiaEvent, trace: TraceId, observed_at_us: u64) {
        let element = event
            .element
            .and_then(|key| self.context.store.take_parked(key))
            .and_then(super::store::Parked::element);
        // An event whose own window's application the system reports hung
        // is dropped before any window lookup or read, as NVDA's UIA
        // handlers drop it first, a window of 0 passing untested
        // (`books/dev/src/reference/testing.md`, "The crates on a reducer's path").
        // Its element and range, taken from the store, go with it.
        if self.hung(event.hwnd) {
            if let UiaKind::ActiveTextPosition(Some(range)) = event.kind {
                drop(self.context.store.take_parked(range));
            }
            tracing::debug!(%trace, hwnd = event.hwnd, "UIA event dropped: its window is hung");
            drop(element);
            return;
        }
        let of_focus = self.context.focused() == Some(Object::Uia(event.parts.runtime_id.clone()));
        let hwnd = self.uia_event_window(&event, of_focus, element.as_ref());
        // NVDA neither arbitrates nor tests a notification against the
        // foreground: it is spoken only from the focus's application, which
        // Core judges. NVDA's app module does drop one whose element has no
        // window above it, which is not ported yet (roadmap, the audits'
        // items). A terminal's were ignored by the listener
        // (`verbatim_uia::map::ignores_notifications`).
        if !matches!(event.kind, UiaKind::Notification(_) | UiaKind::Toast) {
            if let Some(hwnd) = hwnd
                && read::own_window_verdict(self.context, hwnd) != Some(true)
            {
                return; // MSAA owns this window, or another application does.
            }
            // A windowless element with no window found has no window to
            // test (0), and is accepted: the listener's nearest window for a
            // selection fact is already among those tried above.
            if let Some(name) = uia_event_name(&event.kind)
                && !self.accepts(name, hwnd.unwrap_or(event.focus_window))
            {
                return;
            }
        }
        if matches!(event.kind, UiaKind::Selection)
            && let Some(selection) = self.controlled_selection(&event.parts.runtime_id)
        {
            if let (NormalizedEvent::ControlledSelection { node, .. }, Some(window)) =
                (&selection, hwnd)
            {
                self.context.store.record_nearest_window(node.id, window);
            }
            self.emit(trace, observed_at_us, Backend::Uia, hwnd, selection);
            return;
        }
        // A caret or text event of an element of the focus-following
        // subscription, which follows every report's element the outpost
        // has not released: attributed to its node by its runtime id, with
        // no comparison, and read for that node, which Core targets.
        match event.kind {
            UiaKind::TextSelection => {
                if let Some(node_id) = self.context.nodes().existing(&event.parts.runtime_id) {
                    text_reads::text_event_from(self.context, node_id, element.as_ref());
                    if self.is_focus(node_id) {
                        // The focus reports its caret moves itself: the
                        // system caret's events are dropped until the next
                        // focus (`Worker::system_caret_of_uia_focus`).
                        self.commit(Effects {
                            mark: Some(MarkChange::DropSystemCaret),
                            ..Effects::default()
                        });
                    }
                    self.caret_event(node_id, trace, observed_at_us);
                }
                return;
            }
            UiaKind::ActiveTextPosition(range) => {
                let event = (&event.parts.runtime_id[..], range, hwnd);
                return self.active_text_position(event, (trace, observed_at_us));
            }
            UiaKind::TextChanged => {
                let changed = (&event.parts.runtime_id[..], element.as_ref());
                return self.text_changed(changed, hwnd, (trace, observed_at_us));
            }
            _ => {}
        }
        // A selection the listener heard carries what Core speaks of the
        // item, filled by UIA with the event (`verbatim_uia::ListenerSelection`),
        // and is reported with it, less the details the theme does not
        // report: no search is made for its element, which could enter a
        // child window another process serves
        // (`docs/design/cross-process-calls-study.md`, instance 3).
        let mut node = Self::uia_node(self.context, &event.parts, element.as_ref());
        if matches!(event.kind, UiaKind::Selection | UiaKind::Toast) {
            crate::gates::leave_out_unreported(&mut node.details, &self.context.fetches());
        }
        let normalized = match event.kind {
            UiaKind::Property(id) => {
                let refreshed = refreshed_for(self.client.uia(), id, element.as_ref());
                let Some(normalized) = property_event(id, node, refreshed.as_ref()) else {
                    return;
                };
                normalized
            }
            UiaKind::Selection => NormalizedEvent::SelectionChanged { node },
            UiaKind::Notification(notification) => NormalizedEvent::Notification {
                node_id: node.id,
                notification,
            },
            UiaKind::Toast => NormalizedEvent::Alert {
                origin: AlertOrigin::ToastWindowOpened(toast_id(&event.parts.runtime_id)),
                node,
            },
            UiaKind::TextSelection | UiaKind::TextChanged | UiaKind::ActiveTextPosition(_) => {
                return;
            }
        };
        self.emit(trace, observed_at_us, Backend::Uia, hwnd, normalized);
    }

    /// The window of the UIA event `event`, which is of the focus when
    /// `of_focus`: its own, else the recorded window of the focus it
    /// concerns, else its element's. Never the system's focus window, which
    /// can belong to another application.
    fn uia_event_window(
        &self,
        event: &UiaEvent,
        of_focus: bool,
        element: Option<&Cached<Kept>>,
    ) -> Option<isize> {
        if event.hwnd != 0 {
            Some(event.hwnd)
        } else if of_focus {
            self.context.tracked_window()
        } else if event.focus_window != 0 && element.is_none() {
            // A windowless selection's nearest window, which the listener's
            // reading thread walked up to (`FocusReader::check_fact`).
            Some(event.focus_window)
        } else {
            element.and_then(|element| nearest_window_handle(element.live().element()))
        }
    }

    /// A UIA text change of the text focus whose runtime id it carries: it
    /// reads under a key's note live on the node, then is reported, or,
    /// for a terminal, has its new output read.
    fn text_changed(
        &mut self,
        (runtime_id, element): (&[i32], Option<&Cached<Kept>>),
        hwnd: Option<isize>,
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let Some(node_id) = self.context.nodes().existing(runtime_id) else {
            return;
        };
        text_reads::text_event_from(self.context, node_id, element);
        let terminal = self.focused_terminal(node_id);
        self.read_under_note(node_id, NoteTrigger::Text, (trace, observed_at_us));
        if terminal {
            self.terminal_output(node_id, trace, observed_at_us);
            return;
        }
        self.emit(
            trace,
            observed_at_us,
            Backend::Uia,
            hwnd,
            NormalizedEvent::TextChanged { node_id },
        );
    }

    /// An active text position change for the text focus whose runtime id
    /// it carries, the start of its range reported as a position in the
    /// node's text. As NVDA's
    /// handler, an event whose element or range cannot be had is dropped,
    /// and one from a window the system reports hung never gets this far.
    fn active_text_position(
        &mut self,
        (runtime_id, range, hwnd): (&[i32], Option<u64>, Option<isize>),
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let range = range
            .and_then(|key| self.context.store.take_parked(key))
            .and_then(super::store::Parked::range);
        let node_id = self.context.nodes().existing(runtime_id);
        let (Some(node_id), Some(range)) = (node_id, range) else {
            return;
        };
        if let Some(position) = text_reads::active_position(self.context, node_id, &range) {
            self.emit(
                trace,
                observed_at_us,
                Backend::Uia,
                hwnd,
                NormalizedEvent::ActiveTextPositionChanged { node_id, position },
            );
        }
    }

    /// The selection of the element `runtime_id` names as a selection in a
    /// list the focus controls, when it is one: the focused element names,
    /// in its `ControllerFor` relation, the selected one or an element it is
    /// inside ("Selection in a list the focus controls" in
    /// `docs/nvda/events.md`). A selection in none of them is an ordinary
    /// selection, which NVDA's base `event_selection` hands to its state
    /// change handler (`NVDAObjects/__init__.py` 1314 to 1339).
    fn controlled_selection(&mut self, runtime_id: &[i32]) -> Option<NormalizedEvent> {
        // The committed focus, as NVDA uses its focus object.
        for focus_id in self.uia_report_focuses() {
            if let Some(selection) = self.controlled_selection_of(&focus_id, runtime_id) {
                return Some(selection);
            }
        }
        None
    }

    /// [`controlled_selection`](Self::controlled_selection) against the
    /// focus `focus_id`, found by its path when its element is not known.
    fn controlled_selection_of(
        &mut self,
        focus_id: &[i32],
        runtime_id: &[i32],
    ) -> Option<NormalizedEvent> {
        let focus_id = focus_id.to_vec();
        let known = self
            .context
            .nodes()
            .existing(&focus_id)
            .and_then(|id| self.context.store.element_of(id))
            .and_then(|agile| agile.resolve().ok());
        let focused: Cached<Kept> = match known {
            Some(element) => element,
            None => self.element_by_focus_path(&focus_id)?,
        };
        // NVDA reads the relation live for each selection event; the
        // relation and the search under what it names are one remote
        // operation where the focus's window allows one.
        let window = self.context.tracked_window();
        let uia = self.client.uia()?;
        let nodes = self.context.node_request(uia).ok()?;
        let fill = Fill::new(&nodes);
        let query = verbatim_uia_rops::ControlledQuery {
            focused: focused.live().element(),
            selected: runtime_id,
            fill: &fill,
        };
        let remote = self.context.tries_remote(window);
        let (answer, path) = verbatim_uia_rops::controlled_selection(uia, &query, remote).ok()?;
        if let verbatim_uia_rops::Path::Fallback(error) = &path {
            tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
            if let (verbatim_uia_rops::Error::Import(_), Some(hwnd)) = (error, window) {
                self.context.read_classically(top_level_of(hwnd));
            }
        }
        if answer.selected.is_none() && !answer.controlled.is_empty() {
            tracing::debug!(
                selected = ?runtime_id,
                controlled = ?answer.controlled,
                "a selection in none of the elements the focus controls"
            );
        }
        let selected = answer.selected?;
        let registry = self.context.nodes();
        // The controller's runtime id is UIA's own, kept with the element,
        // whatever its cache holds.
        let controller = focused.runtime_id();
        Some(NormalizedEvent::ControlledSelection {
            controller: registry.id_for_element(&controller, &focused),
            node: selected.snapshot(registry),
        })
    }

    /// The runtime id of the focus Core's `Follow` names, the committed
    /// focus, as NVDA reads a selection against its focus object.
    fn uia_report_focuses(&self) -> Vec<Vec<i32>> {
        super::followed::node(self.context)
            .and_then(|node| self.context.store.runtime_id_of(node))
            .into_iter()
            .collect()
    }

    /// A snapshot from a UIA element's cached parts, its id minted by the
    /// registry, which keeps the element when there is one.
    fn uia_node(
        context: &RequestContext<'_>,
        parts: &UiaSnapshotFact,
        element: Option<&Cached<Kept>>,
    ) -> NodeSnapshot {
        let registry = context.nodes();
        let id = match element {
            Some(element) => registry.id_for_element(&parts.runtime_id, element),
            None => verbatim_uia::Interning::id_for(registry, &parts.runtime_id),
        };
        NodeSnapshot {
            id,
            backend: Backend::Uia,
            role: parts.role,
            name: parts.name.clone(),
            value: parts.value.clone(),
            states: parts.states,
            details: parts.details.clone(),
        }
    }

    /// Whether `node_id` still keeps the element the listener's evidence
    /// says is no longer its own ([`note_earlier`](Self::note_earlier)),
    /// compared by agile reference: the focus's own element was not found
    /// to replace it before the report's commit evicts it.
    fn keeps_stale_element(&self, node_id: NodeId) -> bool {
        self.stale.as_ref().is_some_and(|stale| {
            stale.node == node_id
                && stale.element.as_ref().is_some_and(|element| {
                    self.context
                        .store
                        .element_of(node_id)
                        .is_some_and(|kept| kept.same_as(element))
                })
        })
    }

    /// The element `runtime_id` names, an element of the chain the
    /// listener read for the focus this outpost last reported, found by its
    /// path ([`focus_path::find`]); `None` when it is not in that chain, its
    /// path does not lead to it, or the walk failed.
    fn element_by_focus_path(&mut self, runtime_id: &[i32]) -> Option<Cached<Kept>> {
        let context = self.context;
        let route = match &self.focus_path {
            // The focus about to be reported, whose path is not committed
            // yet.
            Some(path) => context.route_in(path, runtime_id),
            None => context.focus_route(runtime_id),
        }?;
        let uia = self.client.uia()?;
        let nodes = context.node_request(uia).ok()?;
        let remote = context.tries_remote(Some(route.window.get()));
        let (found, import_failed) = focus_path::find(uia, &route, &nodes, remote);
        if import_failed {
            context.read_classically(route.window.get());
        }
        // Read as `Kept`: the callers keep it, or read only its runtime id.
        found.ok().flatten().map(|found| found.narrow())
    }

    /// Answers a text request `op` about a terminal, when it is one: the
    /// read before a held key, a read Core asks for at once, rows of the
    /// history, the colours after a key, the screen's shape, and Core's
    /// on-demand reading. Returns whether it answered.
    fn terminal_query(
        &mut self,
        request_id: RequestId,
        trace: TraceId,
        node_id: NodeId,
        op: &TextOp,
    ) -> bool {
        let request = (request_id, trace);
        let focused = self.focused_terminal(node_id);
        match op {
            // The read before a held key is a read of the terminal, whose
            // caret answers the request; its note was set as the request arrived.
            TextOp::ReadCaret(CaretRead {
                purpose: ReadPurpose::BeforeFieldKey | ReadPurpose::BeforeTerminalKey,
                ..
            }) if focused => {
                let terminal = self.terminal_copy(node_id);
                self.read_terminal(node_id, terminal, Some(request), (None, false, true), trace);
                self.note_console_poll(node_id, now_us());
                return true;
            }
            TextOp::TerminalReadNow(scope) => {
                if focused {
                    self.terminal_read_now(request, node_id, *scope);
                } else {
                    self.reply_text(request, TextReply::Unanswered);
                }
                return true;
            }
            TextOp::TerminalHistory { above, whole } => {
                let seen = self.terminal_seen(node_id);
                let reply = match self.client.uia() {
                    Some(uia) if focused => text_reads::terminal_history(
                        self.context,
                        uia,
                        (node_id, seen),
                        (*above, *whole),
                    ),
                    _ => TextReply::Unanswered,
                };
                self.reply_text(request, reply);
                return true;
            }
            TextOp::TerminalFind { needle, from, rows } => {
                let seen = self.terminal_seen(node_id);
                let reply = match self.client.uia() {
                    Some(uia) if focused => text_reads::terminal_find(
                        self.context,
                        uia,
                        (node_id, seen),
                        (needle, *from, *rows),
                    ),
                    _ => TextReply::Unanswered,
                };
                self.reply_text(request, reply);
                return true;
            }
            TextOp::TerminalColours { from_ms, until_ms } => {
                if focused {
                    let mut terminal = self.terminal_copy(node_id);
                    if terminal.colours((*from_ms, *until_ms)) {
                        // The last read ended at or after the key without
                        // the colours: one is taken at once with them, sent
                        // as a live read ("Colours").
                        self.read_terminal(node_id, terminal, None, (None, false, false), trace);
                    } else {
                        self.commit(Effects {
                            terminal: Some((node_id, terminal)),
                            ..Effects::default()
                        });
                    }
                }
                if !self.refused() {
                    self.reply_text(request, TextReply::Done);
                }
                return true;
            }
            TextOp::TerminalShape { height, width } => {
                let mut terminal = self.terminal_copy(node_id);
                terminal.shape(*height, *width);
                let result = self.reply_text_message(request, TextReply::Done);
                self.commit(Effects {
                    result: Some(result),
                    terminal: Some((node_id, terminal)),
                    ..Effects::default()
                });
                return true;
            }
            _ => {}
        }
        if let Some(event) = terminal_event_of(op) {
            // Core's on-demand reading of the focused terminal; a node that
            // is not a focused terminal has nothing to read.
            if !focused {
                let reply = if event == Event::Hold {
                    TextReply::Done
                } else {
                    TextReply::TerminalUnchanged
                };
                self.reply_text(request, reply);
                return true;
            }
            let observed_at_us = now_us();
            if event == Event::Hold {
                self.terminal_event(node_id, event, None, (trace, observed_at_us), None);
                if !self.refused() {
                    self.reply_text(request, TextReply::Done);
                }
            } else {
                self.terminal_event(node_id, event, Some(request), (trace, observed_at_us), None);
            }
            return true;
        }
        false
    }

    /// A query from Core, answered with exactly one reply.
    fn query(&mut self, request_id: RequestId, query: &Query, trace: TraceId) {
        if let Query::Text { node_id, op } = query
            && self.terminal_query(request_id, trace, *node_id, op)
        {
            return;
        }
        if let Query::Text {
            node_id,
            op: TextOp::ReadCaret(read),
        } = query
        {
            self.key_read(request_id, trace, *node_id, *read);
            return;
        }
        let context = self.context;
        let client = &mut *self.client;
        let started = std::time::Instant::now();
        // The focus-now answer's control, adopted and followed once the
        // answer is sent.
        let mut follow = None;
        let result = match query {
            Query::ReadFocus { purpose } => {
                let (answer, answered) = self.focus_now(*purpose);
                follow = answered;
                Ok(QueryResult::Focus(answer))
            }
            Query::Navigate { node_id, kind } => {
                read::navigate(context, client, *node_id, *kind).map(QueryResult::Navigated)
            }
            Query::ReadNode { node_id } => {
                read::read_node(context, client, *node_id).map(QueryResult::Node)
            }
            Query::Activate { node_id } => {
                read::activate(context, client, *node_id).map(QueryResult::Activated)
            }
            Query::Ancestors { node_id } => {
                read::ancestors(context, client, *node_id).map(QueryResult::Ancestors)
            }
            Query::DumpTree { locations } => {
                read::dump_tree(context, client, *locations).map(QueryResult::Tree)
            }
            Query::Text { node_id, op } => {
                Ok(QueryResult::Text(text_reads::answer(context, *node_id, op)))
            }
            Query::ShellItems { kind, .. } => {
                super::shell_items::shell_items(context, client, *kind).map(QueryResult::ShellItems)
            }
            Query::ShellItemPosition { kind, item, .. } => {
                super::shell_items::shell_item_position(context, client, (*kind, item))
                    .map(QueryResult::ShellItemPosition)
            }
        };
        let outcome = match result {
            Ok(result) => QueryOutcome::Done(result),
            Err(ReadError::Gone) => QueryOutcome::Gone,
            Err(ReadError::Failed(reason)) => QueryOutcome::Failed(reason),
        };
        tracing::debug!(
            ?query,
            ?outcome,
            elapsed_ms = started.elapsed().as_millis(),
            "query answered"
        );
        let result = self.reply_message(request_id, trace, outcome);
        match follow {
            Some(answered) => self.follow_answer(result, answered, trace),
            None => {
                self.commit(Effects {
                    result: Some(result),
                    ..Effects::default()
                });
            }
        }
    }

    /// The answer to a focus-now query for `purpose`, with its control,
    /// when it has one, and the control as events are judged against it,
    /// to commit with the answer and follow
    /// ([`follow_answer`](Self::follow_answer)). Core takes the control as
    /// its focus, so its text is read as a reported focus's is: a text
    /// field's line is spoken and Core has its caret. Verbatim+Tab's answer
    /// (`FocusPurpose::Report`) is spoken only, never taken by Core, so it
    /// is neither adopted nor followed, nor its text read: Core asks for
    /// its line. Each window's facts carry the application's name and
    /// whether Alt is held, as a reported focus's do, for Core's rules for
    /// an application's windows, the Alt+Tab switcher's among them
    /// ([`focus_answer_facts`]).
    fn focus_now(&mut self, purpose: FocusPurpose) -> (FocusNow, Option<AnsweredFocus>) {
        let context = self.context;
        let (mut answer, window) =
            read::focus_now(context, self.client, purpose == FocusPurpose::Fake, &[]);
        for (hwnd, facts) in answer
            .window
            .iter_mut()
            .map(|(_, facts)| facts)
            .chain(
                answer
                    .focus
                    .iter_mut()
                    .filter_map(|focus| focus.window.as_mut()),
            )
            .map(|facts| (facts.top_level, facts))
        {
            focus_answer_facts(context, hwnd, facts);
        }
        let windows = answer.window.iter_mut().map(|(window, _)| window);
        let focus = answer.focus.iter_mut().flat_map(|focus| {
            let FocusedControl {
                node, ancestors, ..
            } = focus;
            ancestors.iter_mut().chain(std::iter::once(node))
        });
        read::describe_dialogs(context, self.client, (windows.chain(focus), None), &[]);
        if purpose == FocusPurpose::Report {
            return (answer, None);
        }
        let answered = answer.focus.as_mut().map(|control| {
            let node = &control.node;
            let (text, read_at) = self.read_focus_text(node.id, node.role);
            control.text = text.clone().map(Box::new);
            AnsweredFocus {
                node: node.clone(),
                ancestors: control.ancestors.clone(),
                text,
                read_at,
                observed_at_us: answer.observed_at_us,
                window,
            }
        });
        (answer, answered)
    }

    /// Commits `result`, a focus-now answer, with its control, `answered`,
    /// made the outpost's focus and followed, as a reported focus is
    /// ([`emit_focus`](Self::emit_focus)): so what the follow-up sends, such
    /// as a terminal's first read, reaches Core after the focus it follows. A
    /// control already followed under the same node is followed again, not
    /// adopted. Either way the answer becomes the last
    /// focus queued, as NVDA's fake focus does.
    fn follow_answer(&mut self, result: OutpostToCore, answered: AnsweredFocus, trace: TraceId) {
        let AnsweredFocus {
            node,
            ancestors,
            text,
            read_at,
            observed_at_us,
            window,
        } = answered;
        let (node_id, role) = (node.id, node.role);
        let object = object_of(self.context, &node);
        // A control this outpost already has as its focus, under the same
        // node, is not adopted again, only followed.
        let followed = self.context.follow(followed_node);
        let is_focus = self.is_focus(node_id);
        let chain = ancestors.into_iter().chain(std::iter::once(node)).collect();
        let caret = match text {
            Some(FocusText::Caret { caret, .. }) => Some(caret),
            _ => None,
        };
        let (made, slider) = follow_of(
            self.context,
            FocusMade {
                node: (node_id, role),
                object,
                window,
                chain,
                event: (trace, observed_at_us),
                redirected: false,
                caret,
                origin: Origin::Answer,
            },
        );
        let committed = self.commit(Effects {
            result: Some(result),
            focus: Some(answered_focus(followed, is_focus, made)),
            caret_read: read_at.map(|at| (node_id, at)),
            ..Effects::default()
        });
        if committed == Committed::Applied {
            self.after_follow(node_id, role, slider);
        }
    }

    /// The reply to the query in hand, carrying the calls made since the
    /// last message.
    fn reply_message(
        &self,
        request_id: RequestId,
        trace: TraceId,
        outcome: QueryOutcome,
    ) -> OutpostToCore {
        OutpostToCore::Reply {
            trace_id: trace,
            request_id,
            outcome,
            timing: EventTiming {
                published_at_us: now_us(),
                calls: take_calls(),
                ..self.timing
            },
        }
    }
}

/// The focus followed's node.
fn followed_node(follow: &reducer::FollowState) -> Option<NodeId> {
    follow.followed.as_ref().map(|followed| followed.node)
}

/// The focus followed's ancestors and itself, outermost first; none when
/// nothing is followed.
fn followed_chain(follow: &reducer::FollowState) -> Vec<NodeSnapshot> {
    follow
        .followed
        .as_ref()
        .map_or_else(Vec::new, |followed| followed.chain.clone())
}

/// What causes a read under a key's note ([`Worker::read_under_note`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteTrigger {
    /// The application's caret event.
    Caret,
    /// The application's text-change event.
    Text,
    /// The caret's own report of an event a read already covered, or a
    /// terminal's screen read.
    Now,
}

impl NoteTrigger {
    /// Whether an event observed before the caret was last read shows
    /// nothing new.
    fn needs_new_caret(self) -> bool {
        self != Self::Now
    }

    /// How the read goes under a note reading as `reads`, if it reads.
    fn shape(self, reads: NoteReads) -> Option<ReadShape> {
        match (self, reads.effect_seen()) {
            (Self::Text, true) => None,
            (Self::Caret | Self::Now, true) => Some(ReadShape::Reported),
            (Self::Caret | Self::Text | Self::Now, false) => Some(ReadShape::Compared),
        }
    }
}

/// Which read a read under a note makes.
enum ReadShape {
    /// The read comparing with the last read, by its one trip or call by
    /// call, as a key's evidence is looked for.
    Compared,
    /// The caret's report with the key's unit and facts, once the note's
    /// key has shown its effect.
    Reported,
}

/// `message`, carrying `calls` too, made by the read part of a caret read
/// cut in two programs ([`Work::Finish`]).
fn with_calls(mut message: OutpostToCore, calls: CallCounts) -> OutpostToCore {
    if let OutpostToCore::Event { timing, .. } | OutpostToCore::Reply { timing, .. } = &mut message
    {
        timing.calls += calls;
    }
    message
}

/// `event` as the message that reports it, from an entry of `timing`,
/// published now with the calls this thread has made since the last.
fn event_message(
    context: &Context,
    timing: EventTiming,
    (trace, observed_at_us): (TraceId, u64),
    (backend, window): (Backend, Option<isize>),
    event: NormalizedEvent,
) -> OutpostToCore {
    debug_assert!(
        window.is_some() || !matches!(event, NormalizedEvent::FocusChanged { .. }),
        "every focus report carries window facts"
    );
    let target = context.target_pid();
    let client = window.and_then(|hwnd| console_client(hwnd, target));
    let name = || context.application_name(client.map_or(target, |client| client.0));
    OutpostToCore::Event {
        trace_id: trace,
        observed_at_us,
        backend,
        window: window
            .map(|hwnd| event_window_facts(window_facts(hwnd), &event, name, alt_is_held)),
        application: client,
        timing: EventTiming {
            published_at_us: now_us(),
            calls: take_calls(),
            ..timing
        },
        event,
    }
}

/// A focus about to be committed as the followed one ([`follow_of`]).
struct FocusMade {
    /// The focus and its role.
    node: (NodeId, Role),
    /// The object it is, as the intake records the focus.
    object: Option<Object>,
    /// The window it was reported in.
    window: Option<isize>,
    /// Its ancestors, outermost first, and itself.
    chain: Vec<NodeSnapshot>,
    /// The trace and observation of the event it came from.
    event: (TraceId, u64),
    /// Whether it was reached by redirecting a control's own focus event.
    redirected: bool,
    /// The caret read with its report, if any.
    caret: Option<CaretReport>,
    /// How it was read.
    origin: Origin,
}

/// A focus's record and what only the shell applies, before the commit
/// that follows it ([`follow_of`]).
struct FollowMade {
    /// The record, with no subscription yet.
    followed: Box<Followed>,
    /// Its report's role and batch.
    reported: LastReported,
    /// The trace and observation of the event it came from.
    event: (TraceId, u64),
    /// What only the shell applies.
    writes: FollowWrites,
}

impl FollowMade {
    /// The focus adopted.
    fn adopted(self) -> FocusEffects {
        FocusEffects::Adopt {
            followed: self.followed,
            reported: self.reported,
            writes: self.writes,
        }
    }

    /// The focus already followed, followed again.
    fn again(self) -> FocusEffects {
        let (trace, observed_at_us) = self.event;
        FocusEffects::Again {
            owed: self.followed.owed.map(|owed| owed.work),
            trace,
            observed_at_us,
            writes: self.writes,
        }
    }
}

/// The commit of a focus-now answer `made`, given `followed`, the node
/// followed, and `is_focus`, whether the answer's object is the focus last
/// reported: a control already followed under the same node is followed
/// again, and any other adopted.
fn answered_focus(followed: Option<NodeId>, is_focus: bool, made: FollowMade) -> FocusEffects {
    if followed == Some(made.followed.node) && is_focus {
        made.again()
    } else {
        made.adopted()
    }
}

/// `made` as the record its commit adopts, computed before the commit
/// with local calls only: the subscription's target, whether the focus is
/// a terminal, and what it owes: a terminal read with its caret owes its
/// first read. Also the slider to look up after the commit, for a thumb
/// whose slider's element is not kept.
fn follow_of(context: &Context, made: FocusMade) -> (FollowMade, Option<NodeId>) {
    let FocusMade {
        node: (node_id, role),
        object,
        window,
        chain,
        event: (trace, observed_at_us),
        redirected,
        caret,
        origin,
    } = made;
    // The window text of each window entry of the chain, the outpost's
    // ancestry cache, for its title check: local calls.
    let titles = chain
        .iter()
        .filter_map(|node| node.details.window)
        .map(|handle| {
            let hwnd = super::window::hwnd_of(handle);
            (hwnd, super::window::window_text(hwnd))
        })
        .collect();
    let terminal = role == Role::Terminal && matches!(object, Some(Object::Uia(_)));
    let console = window
        .is_some_and(|hwnd| window_class_name(hwnd) == verbatim_reader_rules::CONSOLE_WINDOW_CLASS);
    let (target, to_find) = target_for(context, (node_id, role), &chain, terminal && console);
    // A focus whose own element is not kept yet has the slider found with
    // it, once it is.
    let to_find = to_find.filter(|_| target.is_some());
    let owed = caret
        .as_ref()
        .filter(|_| terminal)
        .map(|caret| Owed::due(OwedWork::FirstRead(caret.clone()), trace, observed_at_us));
    let followed = Followed {
        node: node_id,
        role,
        object,
        window,
        chain,
        titles,
        origin,
        terminal: Followed::terminal_mark(terminal, window),
        redirected,
        system_caret_unused: terminal && console,
        subscription: None,
        owed,
    };
    let made = FollowMade {
        followed: Box::new(followed),
        reported: LastReported { role },
        event: (trace, observed_at_us),
        writes: FollowWrites {
            node: node_id,
            terminal,
            sent_caret: caret.filter(|_| terminal),
            target,
        },
    };
    (made, to_find)
}

/// Where the focus-following subscription is moved for the focus
/// `node_id` of `role`, with `chain`, its ancestors and itself, as NVDA
/// moves its local group with the focus (`UIAHandler/__init__.py` 642 to
/// 756; `NVDAObjects/UIA/__init__.py` 1623 to 1630;
/// `books/dev/src/reference/adr/0001-focus.md`, "Focus-following subscriptions"): to the node's
/// own element
/// when the outpost has it, with the extended group when the node may have
/// text, and the base group otherwise; the console host's text area
/// (`console`) takes the extended group without its text and caret
/// events, since its console's updates and caret events tell the outpost
/// of its output and its cursor; and to nothing for a focus whose element
/// the outpost does not have, or one read through MSAA, whose changes and
/// caret events come from the hooks. The element alone, not its
/// descendants, as the reducer acts on the focus's own changes. NVDA
/// registers its group with `TreeScope_Ancestors` as well, but UIA
/// delivers no ancestor's event to such a registration
/// (`docs/parity/backends.md`, "UIA event registration"). A focus whose
/// parent is a UIA slider of class `SensitiveSlider` has that parent
/// followed too, for its value changes alone ([`reducer::Group::Value`]),
/// since Core speaks them while the focus is in it
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb"),
/// when its element is kept; the slider is returned to look up when it is
/// not. Registry lookups only.
fn target_for(
    context: &Context,
    (node_id, role): (NodeId, Role),
    chain: &[NodeSnapshot],
    console: bool,
) -> (Target, Option<NodeId>) {
    let has_text = text_reads::may_have_text(context, node_id, role);
    let group = if !has_text {
        reducer::Group::Base
    } else if role == Role::Terminal && console {
        reducer::Group::ExtendedWithoutTextEvents
    } else {
        reducer::Group::Extended
    };
    let element = context.store.element_of(node_id);
    let parent = sensitive_slider_of(node_id, chain);
    let slider = parent.and_then(|parent| {
        context
            .store
            .element_of(parent)
            .map(|element| (parent, element))
    });
    let to_find = parent.filter(|_| slider.is_none());
    (
        element.map(|element| KeptTarget {
            group,
            element,
            slider,
        }),
        to_find,
    )
}

/// The parent of the focus `node_id` in `chain`, its ancestors and itself,
/// when it is a UIA slider of class `SensitiveSlider`.
fn sensitive_slider_of(node_id: NodeId, chain: &[NodeSnapshot]) -> Option<NodeId> {
    let [.., parent, focus] = chain else {
        return None;
    };
    (focus.id == node_id && focus_rules::is_sensitive_slider(parent)).then_some(parent.id)
}

/// `facts`, the window facts of a focus query's answer for the top-level
/// window `top_level`, with the application's name, a console window's
/// client's, and whether Alt is held, as [`event_window_facts`] fills a
/// focus report's. Local calls.
fn focus_answer_facts(
    context: &RequestContext<'_>,
    top_level: WindowHandle,
    facts: &mut WindowFacts,
) {
    let hwnd = super::window::hwnd_of(top_level);
    let target = context.target_pid();
    let client = console_client(hwnd, target);
    facts.application = context.application_name(client.map_or(target, |client| client.0));
    facts.alt_held = alt_is_held();
}

/// `facts`, the window facts of an event message carrying `event`, with
/// the window's class, the application's name that `application` gives,
/// and whether Alt is held, as `alt_held` reads it, only for a focus
/// change, the one event Core's rules for an application's windows and
/// elements match (decision D6), so that no other event carries their
/// strings to Core, nor reads the key.
fn event_window_facts(
    mut facts: WindowFacts,
    event: &NormalizedEvent,
    application: impl FnOnce() -> String,
    alt_held: impl FnOnce() -> bool,
) -> WindowFacts {
    if matches!(event, NormalizedEvent::FocusChanged { .. }) {
        facts.application = application();
        facts.alt_held = alt_held();
    } else {
        facts.class = String::new();
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A focus-now answer of node 2 as `follow_of` makes it, with no
    /// element to follow.
    fn answer_of_node_2() -> FollowMade {
        let node = NodeId::new(2);
        FollowMade {
            followed: Box::new(Followed {
                node,
                role: Role::Button,
                object: None,
                window: None,
                chain: Vec::new(),
                titles: Vec::new(),
                origin: Origin::Answer,
                terminal: reducer::TerminalMark::None,
                redirected: false,
                system_caret_unused: false,
                subscription: None,
                owed: None,
            }),
            reported: LastReported { role: Role::Button },
            event: (TraceId::from_raw(1), 0),
            writes: FollowWrites {
                node,
                terminal: false,
                sent_caret: None,
                target: None,
            },
        }
    }

    #[test]
    fn a_focus_now_answer_is_followed_again_only_on_the_followed_focus() {
        let again = |commit: &FocusEffects| matches!(commit, FocusEffects::Again { .. });
        let adopted = |commit: &FocusEffects| matches!(commit, FocusEffects::Adopt { .. });
        // The focus already followed, under the same node, is only followed
        // again.
        assert!(again(&answered_focus(
            Some(NodeId::new(2)),
            true,
            answer_of_node_2()
        )));
        // Any other is adopted: another node followed, nothing followed, or
        // a node whose object is no longer the focus last reported.
        assert!(adopted(&answered_focus(
            Some(NodeId::new(3)),
            true,
            answer_of_node_2()
        )));
        assert!(adopted(&answered_focus(None, false, answer_of_node_2())));
        assert!(adopted(&answered_focus(
            Some(NodeId::new(2)),
            false,
            answer_of_node_2()
        )));
    }
}
