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

use super::lock::{Lock, LockGuard};
use std::thread;
use std::time::Instant;

use verbatim_ia2::acquire::Purpose;
use verbatim_ia2::{CHILDID_SELF, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, CaretChanges, CaretRead, CaretReply, CaretReport, Change, EarlierElement,
    FirstCachedAncestor, FocusGroup, FocusReaderKind, FocusText, NodeId, NodeSnapshot,
    NormalizedEvent, PropertyChange, ReadPurpose, Role, State, StateSet, TextOp, TextPoint,
    TextReply, TraceId, WindowFacts, WindowHandle, had_effect,
};
use verbatim_uia::{
    Cached, Fill, HelpTextRefresh, Interning as _, Kept, Place, SelectedItem, SuggestionsRefresh,
    nearest_window_handle,
};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_RangeValueValuePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_model::protocol::{
    DeliveredFact, EventTiming, FocusNow, FocusPurpose, FocusedControl, OutpostToCore, Query,
    QueryOutcome, QueryResult, RequestId, UiaFocusRead, UiaSnapshotFact,
};
use verbatim_model::{AlertOrigin, ProgressReading, ToastId};
use verbatim_process::clock::now_us;

use super::commit::{Committed, Effects, FocusEffects, FollowWrites, KeptTarget, Stale, Target};
use super::focus_path;
use super::read::{self, Client, ReadError};
use super::reducer::{
    self, Cut, Entry, Evidence, Finish, FocusPath, Followed, Front, Handling, Item, LastReported,
    MarkChange, MsaaReport, Object, Origin, Owed, OwedProgram, OwedWork, Program, Report, Request,
    UiaEvent, UiaKind, VerdictChange, WinKind, Work,
};
use super::reducer::{KeyNote, NoteFetch, NoteReads};
use super::shell::{Garbage, Posted};
use super::text_reads;
use super::window::{alt_is_held, console_client, window_facts};
use super::{Context, RequestContext};
use crate::arbitration::Judgement as _;
use crate::terminal::reading::{Action, Event, Reading};
use crate::text::{Checked, KeyChecked};
use verbatim_core::focus_rules::{self, ChangeKind, HeldNode};
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{self, EventName};
use verbatim_reader_rules::ancestry_cache::{CacheUse, title_check};
use verbatim_win32::window::SystemWindows;
use verbatim_win32::window::window_class_name;
use verbatim_win32::window::{
    foreground_window_handle, top_level_of, window_belongs_to_hidden_frame, window_is_hidden_frame,
};

pub(super) use super::reducer::is_state_property;

/// A program for a worker, with what the shell let go of for the worker to
/// drop before it runs.
pub(super) struct Job {
    pub(super) program: Program,
    pub(super) garbage: Garbage,
}

/// A worker generation's charge, shared between the worker and the shell
/// under a lock held for one copy: whether the generation was abandoned,
/// and the window its program in hand names.
#[derive(Default)]
pub(super) struct Charge {
    state: Lock<ChargeState>,
}

#[derive(Default)]
struct ChargeState {
    /// Whether the generation was abandoned: its commits are refused, and
    /// its gate refuses every counted call (`verbatim_win32::calls`).
    dead: bool,
    /// The window the program in hand calls into, as its worker named it,
    /// and what the program found there.
    named: Option<Named>,
}

/// The window a program names in its charge before its first call there,
/// keyed by its request so a judgement never reads an earlier program's,
/// with whether the program found it hung, which its commits name.
#[derive(Clone, Copy, Debug)]
struct Named {
    request: Request,
    window: verbatim_reader_rules::Window,
    /// Its check before a call found the window hung or ghosted.
    hung: bool,
}

impl Charge {
    #[track_caller]
    fn lock(&self) -> LockGuard<'_, ChargeState> {
        self.state.lock()
    }

    /// Whether the generation was abandoned.
    pub(super) fn is_dead(&self) -> bool {
        self.lock().dead
    }

    /// The generation is abandoned.
    pub(super) fn abandon(&self) {
        self.lock().dead = true;
    }

    /// `request`'s program calls into `window` from now on, `None` for
    /// none: what its judgement reads, and its commits name.
    pub(super) fn name(&self, request: Request, window: Option<verbatim_reader_rules::Window>) {
        self.lock().named = window.map(|window| Named {
            request,
            window,
            hung: false,
        });
    }

    /// The window `request`'s program named, if it is the program in hand.
    pub(super) fn named(&self, request: Request) -> Option<verbatim_reader_rules::Window> {
        self.lock()
            .named
            .filter(|named| named.request == request)
            .map(|named| named.window)
    }

    /// The window `request`'s program found hung at a check, if any.
    pub(super) fn found(&self, request: Request) -> Option<verbatim_reader_rules::Window> {
        self.lock()
            .named
            .filter(|named| named.request == request && named.hung)
            .map(|named| named.window)
    }

    /// Window `hwnd` was found hung at a check before a call into it: so
    /// was the program's own, when it is that one.
    pub(super) fn found_hung(&self, hwnd: isize) {
        if let Some(named) = self.lock().named.as_mut()
            && named.window.get() == hwnd
        {
            named.hung = true;
        }
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
/// abandoned it refuses every counted call, and it tests a window message's
/// window with the outpost's readability reader, telling the charge of a
/// window found hung.
struct Gate {
    context: Arc<Context>,
    charge: Arc<Charge>,
}

impl verbatim_win32::calls::Charge for Gate {
    fn is_dead(&self) -> bool {
        self.charge.is_dead()
    }

    fn found_hung(&self, hwnd: isize) {
        self.charge.found_hung(hwnd);
    }

    fn readability(&self, hwnd: isize) -> verbatim_win32::window::Readability {
        self.context.readability(hwnd)
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
        context: Arc::clone(context),
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
        window,
        facts,
        work,
    } = program;
    // A program whose entry names its window calls into it; the others
    // name theirs as they find it.
    charge.name(request, window);
    let started = Instant::now();
    let description = tracing::enabled!(tracing::Level::DEBUG).then(|| describe(&work));
    let tag = super::shell::tag_of(request);
    let mut request_context =
        RequestContext::for_program(context, tag, (request, facts), Arc::clone(charge));
    // A program with no window at dispatch names the one it calls into,
    // before its first call there, with no call into the application.
    if window.is_none() {
        charge.name(request, named_window(&request_context, &work));
    }
    let mut reported = false;
    let mut cut = None;
    let handled = catching(|| match work {
        Work::Entry {
            entry,
            handling,
            batch,
            group,
        } => {
            let timing = EventTiming {
                dequeued_at_us: now_us(),
                ..entry.timing
            };
            // Every verdict the entry asks for is judged at its event's
            // observation, or, for an entry with no event, at its start
            // (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact").
            request_context.judge_at(judged_at(context, &entry));
            let mut worker = Worker::new(&request_context, client, batch, timing);
            worker.group = group;
            if handling == Handling::Menu {
                worker.menu_opened(&entry);
            } else {
                worker.handle(entry);
            }
            reported = worker.reported;
            cut = worker.cut.take();
        }
        Work::Owed(owed) => {
            Worker::new(&request_context, client, 0, EventTiming::default()).attempt_owed(owed);
        }
        Work::Drop => {}
        Work::FindBar(window) => {
            Worker::new(&request_context, client, 0, EventTiming::default()).find_bar(window);
        }
        Work::NoteRead(node) => {
            request_context.judge_at(context.observed_now());
            let timing = EventTiming {
                dequeued_at_us: now_us(),
                ..EventTiming::default()
            };
            Worker::new(&request_context, client, 0, timing).note_poll(node);
        }
        Work::Finish { cut, waiting } => {
            Worker::new(&request_context, client, 0, cut.timing).finish(*cut, waiting);
        }
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
        reported,
        panicked: handled.is_err(),
        cut,
    });
}

/// The window a program with none at dispatch calls into, named with no
/// call into the application: a query's node's own window, or the
/// application's focus window for a focus read and a node of no window of
/// its own; a tree dump's application's main window, which it walks from;
/// a shell item query's taskbar window; the owed work's and the console
/// poll's node's; none when the application has no such window, so the
/// program is checked by its deadline alone.
fn named_window(context: &RequestContext<'_>, work: &Work) -> Option<Window> {
    match work {
        Work::Entry { entry, .. } => match &entry.item {
            Item::Query { query, .. } => match query {
                Query::ReadFocus { .. } => application_focus_window(context),
                Query::Navigate { node_id, .. }
                | Query::Activate { node_id }
                | Query::Ancestors { node_id }
                | Query::Text { node_id, .. } => node_window(context, *node_id),
                Query::DumpTree { .. } => {
                    super::window::main_window_of(context.target_pid()).and_then(Window::new)
                }
                Query::ShellItems { .. } | Query::ShellItemPosition { .. } => {
                    super::shell_items::taskbar_window().and_then(Window::new)
                }
            },
            // A live re-read calls into its report's window, by which the
            // moved-on rule judges it as it judged the report's read.
            Item::Fact(DeliveredFact::MsaaLiveReread { trace_id, .. }, _) => context
                .follow(followed_msaa_report)
                .filter(|report| report.trace == *trace_id)
                .and_then(|report| Window::new(report.event.0))
                .or_else(|| application_focus_window(context)),
            _ => application_focus_window(context),
        },
        Work::Owed(owed) => node_window(context, owed.node),
        Work::NoteRead(node) => node_window(context, *node),
        Work::Drop | Work::Finish { .. } | Work::FindBar(_) => None,
    }
}

/// Node `node`'s own window, with no call: the window an MSAA node's
/// object is kept under, or a UIA element's whose runtime id is a window's
/// (`[42, hwnd]`); else the application's focus window.
fn node_window(context: &RequestContext<'_>, node: NodeId) -> Option<Window> {
    if let Some(key) = context.store.key_of(node) {
        return Window::new(key.0);
    }
    if let Some([42, hwnd]) = context.store.runtime_id_of(node).as_deref() {
        return Window::new(isize::try_from(*hwnd).ok()?);
    }
    application_focus_window(context)
}

/// The application's focus window, with no call into it: the focus window
/// of the foreground thread when that thread is the application's, else
/// the top-level window of the focus the outpost expects Core to hold.
fn application_focus_window(context: &RequestContext<'_>) -> Option<Window> {
    context
        .focus_window()
        .filter(|&hwnd| read::serves(context, hwnd))
        .and_then(Window::new)
        .or_else(|| context.held().expected().and_then(|focus| focus.top_level))
}

/// The observation `entry`'s verdicts are judged at: its event's, for an
/// event the outpost's hook or subscriptions took in or a fact the listener
/// sent; for anything else, which has no event, now, on the arbitrator's
/// observation clock, the entry's start.
fn judged_at(context: &Context, entry: &Entry) -> u64 {
    match entry.item {
        Item::Msaa { .. } | Item::Uia(_) | Item::Fact(..) if entry.observed_at_us != 0 => {
            entry.observed_at_us
        }
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
        Work::FindBar(window) => format!("the foreground window {window:#x}'s element"),
        Work::Finish { cut, .. } => format!("the finish of a caret read of {:?}", cut.node),
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
        Item::Fact(DeliveredFact::UiaFocusRead { marker, .. }, _) => {
            format!("UIA focus read {marker}")
        }
        Item::Fact(fact, stamp) => format!("fact {:?}", fact.key(*stamp)),
        Item::Query { query, .. } => format!("query {query:?}"),
        Item::NodesHeld { nodes, .. } => format!("nodes held ({})", nodes.len()),
        Item::NotedCaret { node_id } => format!("noted caret of {node_id:?}"),
        Item::Barrier { asker, .. } => format!("barrier of {asker:?}"),
    }
}

/// The `WinEvent` kind the reducer's `kind` names.
pub(super) fn win_event_kind(kind: WinKind) -> WinEventKind {
    match kind {
        WinKind::Focus => WinEventKind::Focus,
        WinKind::Foreground => WinEventKind::Foreground,
        WinKind::ValueChange => WinEventKind::ValueChange,
        WinKind::StateChange => WinEventKind::StateChange,
        WinKind::NameChange => WinEventKind::NameChange,
        WinKind::DescriptionChange => WinEventKind::DescriptionChange,
        WinKind::Selection => WinEventKind::Selection,
        WinKind::MenuPopupStart => WinEventKind::MenuPopupStart,
        WinKind::MenuEnd => WinEventKind::MenuEnd,
        WinKind::SwitchEnd => WinEventKind::SwitchEnd,
        WinKind::Destroy => WinEventKind::Destroy,
        WinKind::Alert => WinEventKind::Alert,
        WinKind::Caret => WinEventKind::Caret,
        WinKind::TextSelectionChange => WinEventKind::TextSelectionChange,
        WinKind::Show => WinEventKind::Show,
        WinKind::ConsoleUpdate => WinEventKind::ConsoleUpdate,
        WinKind::ShellForeground => WinEventKind::ShellForeground,
        WinKind::ConsoleCaret => WinEventKind::ConsoleCaret,
        WinKind::DesktopSwitch => WinEventKind::DesktopSwitch,
        WinKind::DesktopName => WinEventKind::DesktopName,
    }
}

/// The reducer's name for the `WinEvent` kind `kind`.
pub(super) fn win_kind(kind: WinEventKind) -> WinKind {
    match kind {
        WinEventKind::Focus => WinKind::Focus,
        WinEventKind::Foreground => WinKind::Foreground,
        WinEventKind::ValueChange => WinKind::ValueChange,
        WinEventKind::StateChange => WinKind::StateChange,
        WinEventKind::NameChange => WinKind::NameChange,
        WinEventKind::DescriptionChange => WinKind::DescriptionChange,
        WinEventKind::Selection => WinKind::Selection,
        WinEventKind::MenuPopupStart => WinKind::MenuPopupStart,
        WinEventKind::MenuEnd => WinKind::MenuEnd,
        WinEventKind::SwitchEnd => WinKind::SwitchEnd,
        WinEventKind::Destroy => WinKind::Destroy,
        WinEventKind::Alert => WinKind::Alert,
        WinEventKind::Caret => WinKind::Caret,
        WinEventKind::TextSelectionChange => WinKind::TextSelectionChange,
        WinEventKind::Show => WinKind::Show,
        WinEventKind::ConsoleUpdate => WinKind::ConsoleUpdate,
        WinEventKind::ShellForeground => WinKind::ShellForeground,
        WinEventKind::ConsoleCaret => WinKind::ConsoleCaret,
        WinEventKind::DesktopSwitch => WinKind::DesktopSwitch,
        WinEventKind::DesktopName => WinKind::DesktopName,
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

/// The event address a foreground change has: the system raises its
/// foreground event on the window object, `OBJID_WINDOW` and
/// `CHILDID_SELF`, which the hook takes as the client object, as NVDA's
/// event callback does (`IAccessibleHandler/internalWinEventHandler.py`
/// 104 to 106); the fact forwarded carries the window alone.
fn foreground_event(hwnd: isize) -> (isize, i32, i32) {
    (hwnd, OBJID_CLIENT.0, CHILDID_SELF)
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

/// The name NVDA gives a `WinEvent` of `kind` when it tests it for
/// acceptance, or `None` for a kind the worker handles outside the test:
/// an object destroyed, which NVDA handles in its event callback, and
/// Verbatim's own kinds, the text selection of edit controls, the console
/// host's updates and caret, and the shell's coming to the front and the
/// desktop's switch and name, which only the listener takes (`docs/parity/focus-events.md`, "Event
/// acceptance").
fn event_name_of(kind: WinEventKind) -> Option<EventName> {
    Some(match kind {
        WinEventKind::Focus | WinEventKind::Foreground => EventName::GainFocus,
        WinEventKind::MenuPopupStart => EventName::MenuStart,
        WinEventKind::MenuEnd => EventName::MenuEnd,
        WinEventKind::SwitchEnd => EventName::SwitchEnd,
        WinEventKind::Show => EventName::Show,
        WinEventKind::Alert => EventName::Alert,
        WinEventKind::ValueChange => EventName::ValueChange,
        WinEventKind::StateChange => EventName::StateChange,
        WinEventKind::NameChange => EventName::NameChange,
        WinEventKind::DescriptionChange => EventName::DescriptionChange,
        WinEventKind::Selection => EventName::Selection,
        WinEventKind::Caret => EventName::LocationChange,
        WinEventKind::Destroy
        | WinEventKind::TextSelectionChange
        | WinEventKind::ConsoleUpdate
        | WinEventKind::ConsoleCaret
        | WinEventKind::ShellForeground
        | WinEventKind::DesktopSwitch
        | WinEventKind::DesktopName => return None,
    })
}

/// A toast's identity for Core's repeat rule: a hash of its runtime id, with
/// `DefaultHasher`'s fixed keys, so a repeat of the same toast hashes alike.
fn toast_id(runtime_id: &[i32]) -> ToastId {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    runtime_id.hash(&mut hasher);
    ToastId(hasher.finish())
}

/// Whether `event` is a progress bar's range value change, which is reported
/// as a progress bar's rather than as a value change.
fn is_progress_change(event: &UiaEvent) -> bool {
    is_progress_change_of(&event.kind, event.parts.role)
}

/// Whether a UIA event of `kind` on an element of `role` is a progress bar's
/// range value change.
fn is_progress_change_of(kind: &UiaKind, role: Role) -> bool {
    role == Role::ProgressBar
        && matches!(kind, UiaKind::Property(id) if *id == UIA_RangeValueValuePropertyId.0)
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

/// A focus that is not reported, for the reason logged where it was found.
struct Dropped;

/// One entry's handling, by the worker in charge.
struct Worker<'a> {
    context: &'a RequestContext<'a>,
    client: &'a mut Client,
    /// The batch the entry belongs to.
    batch: u64,
    /// When the entry was raised, observed, relayed, and dequeued, for the
    /// latency log.
    timing: EventTiming,
    /// The MSAA event address of the focus about to be reported, and
    /// whether its object was acquired as a simple child, which the focus
    /// is recorded with for Core's judgement ([`Worker::focus_to_judge`]).
    msaa_event: Option<((isize, i32, i32), bool)>,
    /// Whether the entry ended its focus group: it reported a focus, or a
    /// foreground change of a window that holds no unreleased node of a
    /// report's focus line. A focus group's older entries are then not
    /// tried.
    reported: bool,
    /// The entry's place in its focus group, which its report carries
    /// (`verbatim_model::FocusGroup`); `None` for an entry of no group.
    group: Option<FocusGroup>,
    /// Whether one of this worker's commits was refused: it was abandoned,
    /// and makes no further call.
    refused: std::cell::Cell<bool>,
    /// While the worker attempts a terminal's owed first read, its node:
    /// the commit of the read's result marks it done.
    owed_attempt: Option<NodeId>,
    /// A caret read made for owed work, committed with what follows it.
    pending_caret_read: Option<(NodeId, u64)>,
    /// The caret read the entry's handling cut at its value change check,
    /// which ends the program: the reducer decides the check and
    /// dispatches its finish ([`Work::Finish`]).
    cut: Option<Box<Cut>>,
    /// Whether the focus about to be reported was reached by redirecting
    /// a control's own focus event to its focused child.
    redirected: bool,
    /// The listener's mark, for the UIA focus about to be reported, that
    /// its value is its first selected item's name, which its held report
    /// keeps (`reducer::Focus::value_from_selection`).
    value_from_selection: bool,
    /// Core's live re-read the focus about to be reported answers, until
    /// its report is sent; one its program ends or is abandoned without
    /// answering is answered gone by the reducer.
    answers: Option<u64>,
    /// The focus path of the UIA focus about to be reported, which its
    /// entry finds its element by before it is committed.
    focus_path: Option<FocusPath>,
    /// What the listener read of the element it held before under the UIA
    /// focus about to be reported, which its report carries.
    earlier: Option<EarlierElement>,
    /// The element kept for the UIA focus about to be reported that the
    /// listener's evidence says is not its element, evicted by the
    /// report's commit ([`Stale`]).
    stale: Option<Stale>,
}

impl<'a> Worker<'a> {
    fn new(
        context: &'a RequestContext<'a>,
        client: &'a mut Client,
        batch: u64,
        timing: EventTiming,
    ) -> Self {
        Self {
            context,
            client,
            batch,
            timing,
            msaa_event: None,
            reported: false,
            group: None,
            refused: std::cell::Cell::new(false),
            owed_attempt: None,
            pending_caret_read: None,
            cut: None,
            redirected: false,
            value_from_selection: false,
            answers: None,
            focus_path: None,
            earlier: None,
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
            front,
            ..
        } = entry;
        match item {
            Item::Msaa {
                kind,
                hwnd,
                id_object,
                id_child,
                ..
            } => self.msaa_event(
                win_event_kind(kind),
                hwnd,
                id_object,
                id_child,
                trace,
                observed_at_us,
            ),
            Item::Uia(event) => self.uia_event(event, trace, observed_at_us),
            Item::Fact(fact, _) => self.fact(fact, front, trace, observed_at_us),
            Item::Query { request_id, query } => self.query(request_id, &query, trace),
            // Read now that nothing else waits, unless a read since saw it.
            Item::NotedCaret { node_id } => self.caret_of(node_id, trace, observed_at_us),
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
            ReadPurpose::BeforeKey => self.before_read(request_id, trace, node_id, read),
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
        let live = self.live_note(node_id);
        let (checked, read_at) = text_reads::note_read(self.context, node_id, note, (true, false));
        if let Some(live) = live {
            text_reads::clear_note(self.context, node_id, live.until_us);
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

    /// The node of a live note, when it is the client area of edit control
    /// `hwnd`: an edit control Core took as its focus from a focus-now answer.
    fn noted_edit(&self, hwnd: isize) -> Option<NodeId> {
        let facts = self.context.facts();
        let mut live = facts
            .text
            .live_notes(now_us())
            .map(|(node, _)| NodeId::new(node));
        live.find(|&node_id| {
            self.context.store.key_of(node_id) == Some((hwnd, OBJID_CLIENT.0, CHILDID_SELF))
        })
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
    /// whose console events are its own: the followed focus when it is
    /// that terminal, else the newest report in the window whose node is a
    /// terminal this outpost has not released
    /// ([`focused_terminal`](Self::focused_terminal)).
    fn focused_console_terminal(&self, hwnd: isize) -> Option<NodeId> {
        let followed = self.context.follow(|follow| {
            follow
                .followed
                .as_ref()
                .filter(|followed| followed.window == Some(hwnd))
                .map(|followed| followed.node)
        });
        if let Some(focus) = followed.filter(|&node_id| self.focused_terminal(node_id)) {
            return Some(focus);
        }
        let top_level = verbatim_reader_rules::Window::new(top_level_of(hwnd));
        let reported: Vec<NodeId> = self
            .context
            .held()
            .focuses()
            .rev()
            .filter(|focus| top_level.is_some() && focus.top_level == top_level)
            .map(|focus| focus.node.id)
            .collect();
        reported
            .into_iter()
            .find(|&node_id| self.focused_terminal(node_id))
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
    /// at once ([`TextOp::TerminalReadNow`], and a before read ([`ReadPurpose::BeforeKey`]), the
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

    /// Reports a focus, or a foreground change, and makes it the focus this
    /// outpost follows, in one commit: the report recorded for Core's
    /// judgement, sent, and a focus followed, its caret, text and the
    /// focus-following subscription following it, so the limiter keeps its
    /// events and the menu rules know where focus is. Then, once committed,
    /// the slider a `SensitiveSlider`'s thumb is in is looked up, and the
    /// work a terminal owes is attempted.
    ///
    /// Its source names the focus reader that read it, for its
    /// `first_cached_ancestor`, computed here from the entries it emits,
    /// after every filter and rule: the innermost of them that a walk took
    /// from the ancestry cache ([`RequestContext::note_cached`]). A report
    /// answering Core's live re-read ([`Worker::answers`]) carries the
    /// request and no cached ancestor.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "a focus report has this many parts, built and committed in order"
    )]
    fn emit_focus(
        &mut self,
        trace: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<isize>,
        node: NodeSnapshot,
        source: FocusSource,
        object: Option<Object>,
        (ancestors, selected_child): read::Enrichment,
    ) {
        let foreground = matches!(source, FocusSource::WholeWindow);
        // A focus in a window of a process Verbatim ignores is not
        // reported, as Core would drop it (`books/dev/src/how-to/e2e.md`).
        if let Some(hwnd) = window
            && self.context.in_ignored_window(hwnd)
        {
            tracing::info!(hwnd, "a focus in a window of an ignored process is dropped");
            return;
        }
        let mut node = node;
        let mut ancestors = ancestors;
        if backend == Backend::Uia {
            read::console_text_area(&mut node, window);
        }
        // A dialog the focus has newly entered, or one in front, says its
        // own text as its description.
        let mut memo = self.context.follow(|follow| follow.dialog.clone());
        read::describe_dialogs(
            self.context,
            self.client,
            (
                ancestors.iter_mut().chain(std::iter::once(&mut node)),
                self.focus_path.as_ref(),
            ),
            (&mut memo, foreground),
        );
        let role = node.role;
        let node_id = node.id;
        let earlier = self.earlier.take();
        // A foreground change is recorded by its window's event address.
        let evidence = if backend == Backend::Uia && !foreground {
            Evidence::Uia(earlier)
        } else {
            Evidence::Msaa
        };
        let mut reported = self.focus_to_judge(&node, &ancestors, window, foreground);
        reported.value_from_selection = std::mem::take(&mut self.value_from_selection);
        let answers = self.answers;
        let first_cached_ancestor = self.first_cached_ancestor(&source, &ancestors);
        let (text, read_at) = if foreground {
            (None, None)
        } else {
            self.read_focus_text(node.id, node.role)
        };
        tracing::debug!(
            ?role,
            name = ?node.name,
            foreground,
            ?backend,
            ancestors = ancestors.len(),
            "focus reported"
        );
        let chain: Vec<NodeSnapshot> = ancestors
            .iter()
            .cloned()
            .chain(std::iter::once(node.clone()))
            .collect();
        let caret = match &text {
            Some(FocusText::Caret { caret, .. }) => Some(caret.clone()),
            _ => None,
        };
        let event = NormalizedEvent::FocusChanged {
            node,
            whole_window: foreground,
            ancestors,
            selected_child,
            text: text.map(Box::new),
            earlier,
            group: self.group,
            first_cached_ancestor,
            answers,
        };
        let message = self.event_message(trace, observed_at_us, backend, window, event);
        let origin = source.origin(trace, observed_at_us);
        self.focus_path = None;
        let mut effects = Effects {
            result: Some(message),
            report: Some(Report::Reported(reported, evidence)),
            caret_read: read_at.map(|at| (node_id, at)),
            stale: self.stale.take(),
            ..Effects::default()
        };
        // A foreground report is a window, not the control focus is in: it
        // is not followed.
        let reported = LastReported {
            role,
            batch: self.batch,
        };
        let mut slider = None;
        effects.focus = Some(match origin {
            None => FocusEffects::Foreground {
                node: node_id,
                reported,
                dialog: memo,
            },
            Some(origin) => {
                let (made, to_find) = follow_of(
                    self.context,
                    FocusMade {
                        node: (node_id, role),
                        object,
                        window,
                        chain,
                        batch: self.batch,
                        event: (trace, observed_at_us),
                        redirected: std::mem::take(&mut self.redirected),
                        caret,
                        origin,
                    },
                );
                slider = to_find;
                made.adopted(Some(memo))
            }
        });
        if self.commit(effects) != Committed::Applied {
            return;
        }
        self.reported = true;
        // The live re-read is answered by the report sent.
        self.answers = None;
        self.after_follow(node_id, role, slider);
    }

    /// The `first_cached_ancestor` of a report from `source` whose
    /// ancestors are `ancestors`, as emitted: the innermost of them a walk
    /// took from the ancestry cache, taking the cached ancestors noted;
    /// none for a report answering a live re-read, or from a source that
    /// reuses no cache.
    fn first_cached_ancestor(
        &self,
        source: &FocusSource,
        ancestors: &[NodeSnapshot],
    ) -> Option<FirstCachedAncestor> {
        let cached = self.context.take_cached();
        let reader = source.reader().filter(|_| self.answers.is_none())?;
        let ids: Vec<NodeId> = ancestors.iter().map(|ancestor| ancestor.id).collect();
        focus_rules::first_cached_ancestor(&ids, |id| cached.contains(&id))
            .map(|index| FirstCachedAncestor { index, reader })
    }

    /// Core's live re-read `request` of this outpost's MSAA focus report
    /// traced `trace`, its last report ([`reducer::MsaaReport`]): the object
    /// at the report's event address read again, acquired from the event
    /// as it was (`AccessibleObjectFromEvent`), its ancestors walked live
    /// with no ancestry cache, and reported carrying `request`. The
    /// report's verdict stands: NVDA's checks are not made again. Reports
    /// nothing when the last report is not that one, or the address gives
    /// no object, and the reducer answers it gone as the program ends.
    fn msaa_live_reread(&mut self, request: u64, trace: TraceId) {
        self.answers = Some(request);
        let stored = self
            .context
            .follow(followed_msaa_report)
            .filter(|report| report.trace == trace);
        if let Some(report) = stored {
            let (hwnd, id_object, id_child) = report.event;
            if let Some(candidate) =
                verbatim_ia2::acquire::focus_candidate(hwnd, id_object, id_child)
            {
                let candidates = read::msaa_candidates(self.context);
                let node = candidate.read(self.context.nodes(), Some(&candidates));
                self.report_msaa_focus(
                    node,
                    (report.event, report.simple_child),
                    (trace, report.observed_at_us),
                    &candidates,
                );
            }
        }
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

    /// The focus `node`, with `ancestors`, reported in `window`, as the
    /// outpost records it among its reports ([`reducer::Held`]): a
    /// foreground change carries its event address.
    /// NVDA makes its foreground object when the
    /// focus enters a top-level window, so the window keeps the verdict it
    /// had then while the focus moves within it
    /// ([`is_foreground_window`](Self::is_foreground_window)).
    fn focus_to_judge(
        &mut self,
        node: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
        window: Option<isize>,
        foreground: bool,
    ) -> reducer::Focus {
        judged_focus(
            self.context,
            (node, ancestors),
            (window, foreground),
            self.msaa_event.take(),
        )
    }

    /// The last focus this outpost reported, with its ancestors: the
    /// outpost's ancestry cache, where the next walk may stop and reuse the
    /// rest. Empty, so that the walk reads live to the top or the process
    /// boundary, when a window title saved with it reads otherwise now or
    /// its window is gone (the title check, local calls), and for a read
    /// answering Core's live re-read, which reuses nothing.
    fn focus_chain(&self) -> Vec<NodeSnapshot> {
        if self.answers.is_some() {
            return Vec::new();
        }
        let (chain, titles) = self.context.follow(|follow| {
            (
                followed_chain(follow),
                follow
                    .followed
                    .as_ref()
                    .map_or_else(Vec::new, |followed| followed.titles.clone()),
            )
        });
        if title_check(&titles, verbatim_win32::window::live_title) == CacheUse::WalkLive {
            tracing::debug!("a window title of the ancestry cache changed; the walk reads live");
            return Vec::new();
        }
        read::held_prefix(self.context, chain)
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

    /// Whether the event's own window, `hwnd`, can be read, by the
    /// outpost's reader: NVDA's checks after acceptance and before the
    /// window's UIA test and the object's acquisition, local calls. A
    /// dropped event is logged; one whose window is hung or ghosted is the
    /// evidence of a hang its program's commit names
    /// ([`hang_found`](Self::hang_found)).
    fn readable(&self, hwnd: isize) -> bool {
        let readability = self.context.readability(hwnd);
        let Some(reason) = readability.reason() else {
            return true;
        };
        tracing::debug!(hwnd, reason, "event dropped: its window cannot be read");
        if readability.is_hung() {
            self.hang_found(hwnd);
        }
        false
    }

    /// Whether a UIA entry's own window, `hwnd`, is hung or ghosted, by the
    /// outpost's reader, local calls never made for 0, which passes, as
    /// NVDA's test passes a sender with no cached window; a hung window is
    /// the evidence of a hang its program's commit names.
    fn hung(&self, hwnd: isize) -> bool {
        if hwnd == 0 || !self.context.readability(hwnd).is_hung() {
            return false;
        }
        self.hang_found(hwnd);
        true
    }

    /// Window `hwnd`, which the program's check found hung before any call
    /// into it, is named in a commit as hung, for the reducer's record of
    /// hung windows.
    fn hang_found(&self, hwnd: isize) {
        let charge = self.context.charge();
        let request = self.context.request();
        if charge.named(request).map(Window::get) != Some(hwnd) {
            charge.name(request, Window::new(hwnd));
        }
        charge.found_hung(hwnd);
        self.commit(Effects::default());
    }

    /// An MSAA event from this outpost's own hooks.
    fn msaa_event(
        &mut self,
        kind: WinEventKind,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_us: u64,
    ) {
        if kind == WinEventKind::ConsoleUpdate {
            // The console host's text changed: its updates are the focused
            // console terminal's text changes, as its UIA text changes are
            // not subscribed to (`books/dev/src/reference/crates/verbatim-outpost.md`,
            // "Terminals").
            if let Some(node_id) = self.focused_console_terminal(hwnd) {
                // As for a UIA text change: a key's note is read before
                // the read, which reads the caret too and would leave the
                // change nothing new to show.
                self.read_under_note(node_id, NoteTrigger::Text, (trace, observed_at_us));
                self.terminal_output(node_id, trace, observed_at_us);
                self.absorb_while_held(node_id, hwnd);
            }
            return;
        }
        if kind == WinEventKind::ConsoleCaret {
            // The console host's cursor moved: the focused console
            // terminal's caret event, as its UIA caret events are not
            // subscribed to either.
            if let Some(node_id) = self.focused_console_terminal(hwnd) {
                // A note's poll goes on unless the event's read shows the
                // key's effect: a blink tick left over from an earlier
                // cursor set can come before the key's move.
                self.caret_event(node_id, trace, observed_at_us);
            }
            return;
        }
        if kind == WinEventKind::Destroy {
            if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
                self.context.record(VerdictChange::Forget(hwnd));
                self.context.forget_window(hwnd);
                // A reused window handle must never inherit these nodes;
                // their objects are released once the store's lock is let
                // go, and their text facts are forgotten with the
                // program's commit.
                let (removed, gone) = self.context.store.forget_window(hwnd);
                drop(removed);
                text_reads::forget_nodes(self.context, &gone);
            }
            return;
        }
        // NVDA's order of checks: acceptance, the window's state, the
        // window's UIA test, and only then the object.
        if let Some(event) = event_name_of(kind)
            && !self.accepts(event, hwnd)
        {
            return;
        }
        if !self.readable(hwnd) {
            return;
        }
        // The system caret's event is a caret event on the focus, from
        // whichever window it comes, as NVDA maps it onto its last queued
        // focus. An edit control Core took as its focus from a focus-now
        // answer, with a key's note on it, is Core's focus rather than the
        // UIA focus this outpost last reported, so its caret events read
        // the note first.
        if kind == WinEventKind::Caret
            && let Some(Object::Uia(runtime_id)) = self.context.focused()
        {
            if let Some(node_id) = self.noted_edit(hwnd) {
                self.read_under_note(node_id, NoteTrigger::Caret, (trace, observed_at_us));
            } else {
                self.system_caret_of_uia_focus(&runtime_id, trace, observed_at_us);
            }
            return;
        }
        // UIA owns this window, or another application does.
        if read::own_window_verdict(self.context, hwnd) != Some(false) {
            return;
        }
        if self.edit_event(kind, (hwnd, id_object, id_child), trace, observed_at_us) {
            return;
        }
        let Some(mut object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child)
        else {
            return;
        };
        let registry = self.context.nodes();
        if kind == WinEventKind::Selection {
            self.msaa_selection(object, hwnd, (trace, observed_at_us));
            return;
        }
        let spoken = self.spoken_node(kind, &mut object);
        if kind == WinEventKind::ValueChange {
            self.value_change(object, spoken, (hwnd, trace, observed_at_us));
            return;
        }
        let Some((node_id, role)) = spoken else {
            return;
        };
        // Only the property the event names is read, as NVDA reads only
        // that property of its focus or ancestor object. A state change's
        // states are recorded for the held focus in its commit.
        let mut states_read = None;
        let event = match kind {
            WinEventKind::NameChange => NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::Name(object.name(role, registry)),
                child_count: None,
            },
            WinEventKind::DescriptionChange => NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::Description(object.description(role, registry)),
                child_count: None,
            },
            WinEventKind::StateChange => {
                let states = object.states(role);
                states_read = Some((node_id, states));
                NormalizedEvent::PropertyChanged {
                    node_id,
                    child_count: self
                        .expanded_child_count((node_id, role, states), (hwnd, id_child)),
                    change: PropertyChange::States(states),
                }
            }
            _ => return,
        };
        let message = self.event_message(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
        self.commit(Effects {
            result: Some(message),
            states: states_read,
            ..Effects::default()
        });
    }

    /// The system caret's location change or show, accepted from any
    /// window NVDA's test accepts a location change from: a caret event on
    /// the UIA focus whose `runtime_id` is given, by the path a UIA caret
    /// event takes. NVDA handles these events as a caret event on its last
    /// queued focus whatever API the focus is read through and whichever
    /// window raised them, since some UIA documents, such as Visual
    /// Studio's, raise no UIA caret event of their own. Its two exceptions
    /// are not here: a focus read through IA2, which Verbatim does not read
    /// yet (milestone M7), and Word's UIA document, flooded with them by
    /// Chinese input methods, which belongs to Word's application support
    /// (the roadmap's app-module track, tier D).
    ///
    /// Once the focus has raised a UIA caret event of its own, and in a
    /// focused console terminal, whose console caret event is its caret
    /// event, the system caret's events are not used until the next focus:
    /// they come with the focus's own, as Windows 11 Notepad raises both
    /// for every move, the system caret's first, and each would read the
    /// caret once more for the same move (the move's own event is observed
    /// after that read began, so `Context::caret_read_since` cannot count
    /// the read as covering it). The intake drops them as they arrive
    /// (`Intake::drop_system_caret`); one queued before the mark is
    /// dropped here.
    fn system_caret_of_uia_focus(
        &mut self,
        runtime_id: &[i32],
        trace: TraceId,
        observed_at_us: u64,
    ) {
        if self.context.follow(|follow| {
            follow
                .followed
                .as_ref()
                .is_some_and(|followed| followed.system_caret_unused)
        }) {
            return;
        }
        if let Some(node_id) = self.context.nodes().existing(runtime_id) {
            self.caret_event(node_id, trace, observed_at_us);
        }
    }

    /// An MSAA selection of `object`, from `hwnd`'s hooks, read only as far
    /// as it can be spoken, by the rule the reducer speaks it by
    /// (`focus_rules::selection_target`), judged against the focus Core
    /// holds and those reported that Core has not judged yet
    /// ([`reducer::Held`]). Selecting the focus or one of its ancestors is a
    /// change of its state, as NVDA handles it, so only its states are
    /// read. Any other selection is spoken only inside a focused selection
    /// container of this outpost, so it is read in full only while one of
    /// those focuses is one, and dropped otherwise, having cost its
    /// acquisition alone.
    fn msaa_selection(
        &mut self,
        object: verbatim_ia2::acquire::EventObject,
        hwnd: isize,
        (trace, observed_at_us): (TraceId, u64),
    ) {
        let registry = self.context.nodes();
        // A tree view item selected on its way to the focus, from one of
        // its children, is one of the focus's logical ancestors only: NVDA,
        // whose ancestors are reached through `accParent`, sees it as
        // neither the focus nor an ancestor, and says nothing of it.
        if object
            .which_of(&self.logical_ancestors(), registry)
            .is_some()
        {
            tracing::debug!(
                hwnd,
                "MSAA selection dropped: a logical ancestor of the focus only"
            );
            return;
        }
        let (chain, container) = {
            let held = self.context.held();
            let chain: Vec<NodeId> = held.focuses().flat_map(reducer::Focus::chain_ids).collect();
            let container = held
                .focuses()
                .any(|focus| focus_rules::is_selection_container(focus.node.role));
            (chain, container)
        };
        let event = if let Some(node_id) = object.which_of(&chain, registry)
            && self.selection_states_spoken(node_id)
            && let Some(role) = self.known_role(node_id)
        {
            NormalizedEvent::PropertyChanged {
                node_id,
                change: PropertyChange::States(object.states(role)),
                child_count: None,
            }
        } else if container {
            NormalizedEvent::SelectionChanged {
                node: object.read(registry, Purpose::Announce),
            }
        } else {
            tracing::debug!(
                hwnd,
                "MSAA selection dropped: the focus is no selection container"
            );
            return;
        };
        self.emit(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
    }

    /// The caret, a text selection, or the text of the focus, when it is an
    /// edit control: reported from the control's messages, without reading
    /// its MSAA object (whose value is its whole text). Whether the event
    /// was one of these, and so handled.
    fn edit_event(
        &mut self,
        kind: WinEventKind,
        (hwnd, id_object, id_child): (isize, i32, i32),
        trace: TraceId,
        observed_at_us: u64,
    ) -> bool {
        let client = Object::Msaa(hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        let of_focus = self.context.focused() == Some(client);
        match kind {
            WinEventKind::Caret | WinEventKind::TextSelectionChange => {
                if of_focus {
                    let node_id = self.context.nodes().client(hwnd);
                    if self
                        .context
                        .caret_read_since(node_id, self.timing.observed_at_us)
                    {
                        // The read that covers it made the comparison NVDA
                        // makes at this event, which clears the note.
                        text_reads::note_caret_event_counted(self.context, node_id);
                    } else if !self.read_under_note(
                        node_id,
                        NoteTrigger::Now,
                        (trace, observed_at_us),
                    ) {
                        self.caret_of(node_id, trace, observed_at_us);
                    }
                } else if let Some(node_id) = self.noted_edit(hwnd) {
                    // A key's note on an edit control Core took as its
                    // focus from a focus-now answer.
                    self.read_under_note(node_id, NoteTrigger::Caret, (trace, observed_at_us));
                }
                true
            }
            WinEventKind::ValueChange
                if of_focus
                    && id_object == OBJID_CLIENT.0
                    && id_child == CHILDID_SELF
                    && verbatim_ia2::edit::edit_api_version(
                        &verbatim_reader_rules::class::normalize_class_name(&window_class_name(
                            hwnd,
                        )),
                    )
                    .is_some() =>
            {
                let node_id = self.context.nodes().client(hwnd);
                text_reads::note_value_change(self.context, node_id);
                self.read_under_note(node_id, NoteTrigger::Text, (trace, observed_at_us));
                self.emit(
                    trace,
                    observed_at_us,
                    Backend::Msaa,
                    Some(hwnd),
                    NormalizedEvent::TextChanged { node_id },
                );
                true
            }
            _ => false,
        }
    }

    /// The number of children of `node`, of `role` and now with `states`, a
    /// Win32 tree view item at `(hwnd, id_child)`, when its state change
    /// newly expands it and it is the focus, for Core to say how many it
    /// holds ("How many items an expanded tree view item holds" in
    /// `docs/nvda/speech.md`). Whether it was expanded is known from the
    /// focus's states as last read, which this change then updates.
    fn expanded_child_count(
        &self,
        (node, role, states): (NodeId, Role, StateSet),
        (hwnd, id_child): (isize, i32),
    ) -> Option<u32> {
        let was_expanded = self.context.held().expanded_before(node)?;
        (role == Role::TreeItem && states.contains(State::Expanded) && !was_expanded)
            .then(|| verbatim_ia2::acquire::tree_view_child_count(hwnd, id_child))
            .flatten()
    }

    /// A value change on `object`, the focus when `focus` names its node and
    /// the role it was last read with: spoken as a value change only for the
    /// focus, and reported as a progress bar's whether or not it is the
    /// focus, as NVDA's progress bar behavior reports one
    /// (`docs/nvda/object-model.md`, "How a progress bar reports its
    /// value"). The focus has only its value read, or, when it is a progress
    /// bar, what a progress bar's report uses; an object that is not the
    /// focus has its role read alone first, and only a progress bar is read
    /// further.
    fn value_change(
        &mut self,
        mut object: verbatim_ia2::acquire::EventObject,
        focus: Option<(NodeId, Role)>,
        (hwnd, trace, observed_at_us): (isize, TraceId, u64),
    ) {
        let registry = self.context.nodes();
        let progress_bar = match focus {
            Some((_, role)) => role == Role::ProgressBar,
            None => object.role() == Role::ProgressBar,
        };
        let progress = progress_bar
            .then(|| object.progress(focus, registry))
            .flatten();
        let event = match (progress, focus) {
            (Some(bar), _) => NormalizedEvent::ProgressChanged { bar },
            (None, Some((node_id, role))) => NormalizedEvent::ValueChanged {
                node_id,
                value: object.value(role),
            },
            (None, None) => return,
        };
        self.emit(trace, observed_at_us, Backend::Msaa, Some(hwnd), event);
    }

    /// The role a node of a focus's chain was last read with: one of the
    /// focuses events are judged against, or one of its ancestors
    /// ([`reducer::Held`]).
    fn known_role(&self, node: NodeId) -> Option<Role> {
        self.context.held().role_of(node)
    }

    /// Whether a selection of `node`, the focus or an ancestor of one of
    /// the focuses events are judged against, is spoken as a change of its
    /// states (`focus_rules::selection_target`): not for a logical ancestor
    /// ([`Self::is_logical_ancestor`]).
    fn selection_states_spoken(&self, node: NodeId) -> bool {
        let held = self.context.held();
        held.focuses().any(|focus| {
            focus_rules::selection_target(
                node,
                (focus.node.id, focus.node.role),
                focus.ancestors.iter().map(|ancestor| ancestor.id),
                |ancestor| !self.is_logical_ancestor(ancestor),
            )
            .is_some()
        })
    }

    /// The node `object` is, with the role it was last read with, when a
    /// change of `kind` on it may be spoken, by the rule the reducer speaks
    /// it by (`focus_rules::change_target`), among the nodes this outpost
    /// has not released ([`reducer::Held`]): a name, description, or value
    /// change for the focus of a report, and a state change for any node
    /// of a report's line or carried by a message still waiting, as NVDA
    /// speaks them on the focus or an ancestor; Core targets which is
    /// spoken. Any other object is not read: it is told from those by its
    /// identity first, without reading any of its properties.
    fn spoken_node(
        &self,
        kind: WinEventKind,
        object: &mut verbatim_ia2::acquire::EventObject,
    ) -> Option<(NodeId, Role)> {
        let change = match kind {
            WinEventKind::NameChange => ChangeKind::Name,
            WinEventKind::DescriptionChange => ChangeKind::Description,
            WinEventKind::StateChange => ChangeKind::States,
            _ => ChangeKind::Value,
        };
        let candidates: Vec<NodeId> = {
            let held = self.context.held();
            // The ancestors NVDA has are those reached through `accParent`,
            // not the focus's logical ones ([`Self::logical_ancestors`]):
            // their state changes are not spoken, as selecting a tree item's
            // parent on its way to the focus would otherwise say "selected".
            // Nor is the foreground window an ancestor here
            // ([`Self::is_foreground_window`]). Ancestors first, then the
            // focus, of each focus judged against.
            let mut candidates: Vec<NodeId> = Vec::new();
            for focus in held.focuses() {
                let ancestors: Vec<NodeId> =
                    focus.ancestors.iter().map(|ancestor| ancestor.id).collect();
                for node in
                    focus_rules::change_candidates(change, focus.node.id, &ancestors, |id| {
                        !self.is_logical_ancestor(id)
                            && !self.is_foreground_window(id, focus.entered_in_front)
                    })
                {
                    if !candidates.contains(&node) {
                        candidates.push(node);
                    }
                }
            }
            candidates
        };
        // A node a message still waiting carries, such as a report's new
        // node, which Core will hold by the time it judges the change.
        let mut candidates = candidates;
        if change == ChangeKind::States {
            for number in self.context.outbound.waiting_nodes() {
                let node = NodeId::new(number);
                if !candidates.contains(&node) && !self.is_logical_ancestor(node) {
                    candidates.push(node);
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }
        // The followed focus, when no key places the object: matched by
        // NVDA's equality against the event's own address, as NVDA directs
        // a change event to the focus object it equals, such as a client
        // of role window kept under its window object's key.
        let node = object
            .which_of(&candidates, self.context.nodes())
            .or_else(|| {
                let followed = self.context.follow(followed_node)?;
                (candidates.contains(&followed)
                    && object.is_equal_to(followed, self.context.nodes()))
                .then_some(followed)
            })?;
        // A carried node no report's line has is read for its role.
        let role = self.known_role(node).unwrap_or_else(|| object.role());
        Some((node, role))
    }

    /// Whether `node`, one of the focus's ancestors, was reached through a
    /// tree view item's logical parents
    /// (`verbatim_ia2::acquire::ancestor_chain`) rather than `accParent`:
    /// `accParent` only ever reaches whole objects, so an ancestor that is a
    /// simple child of its window's object is a logical one.
    fn is_logical_ancestor(&self, node: NodeId) -> bool {
        self.context
            .store
            .key_of(node)
            .is_some_and(|(_, _, child)| child != CHILDID_SELF)
    }

    /// Whether `node`, one of the focus's ancestors, is the foreground
    /// window's own object, its client area or its window object, the
    /// window being the focus's top-level window and the foreground when
    /// the focus entered it: NVDA never speaks a state change on
    /// it, as a modal dialog's owner being disabled when the dialog opens,
    /// live-checked against NVDA on 2026-10-08 (eleven captures, never
    /// "unavailable"). NVDA's state-change gate speaks for an ancestor only
    /// when the event's object is the very instance in its ancestor list,
    /// and an event finds an existing instance only in its live-object
    /// table, keyed by the event's address. When the focus enters a
    /// top-level window, NVDA reads its ancestors and then creates the
    /// foreground object, for the foreground window's client area, which
    /// takes the address's place in the table and is held as the
    /// foreground, so an event on that window resolves to the foreground
    /// object, never to the ancestor (`docs/parity/focus-announcement.md`, "A top-level
    /// window's state change"). A top-level window that is not the
    /// foreground, such as a popup menu's, keeps its ancestor.
    fn is_foreground_window(&self, node: NodeId, foreground: Option<Window>) -> bool {
        foreground.is_some_and(|foreground| {
            self.context
                .store
                .key_of(node)
                .is_some_and(|(hwnd, id_object, child)| {
                    hwnd == foreground.get()
                        && child == CHILDID_SELF
                        && (id_object == OBJID_CLIENT.0 || id_object == OBJID_WINDOW.0)
                })
        })
    }

    /// The logical ancestors ([`Self::is_logical_ancestor`]) of the
    /// focuses events are judged against ([`reducer::Held`]).
    fn logical_ancestors(&self) -> Vec<NodeId> {
        let chain: Vec<NodeId> = self
            .context
            .held()
            .focuses()
            .flat_map(reducer::Focus::chain_ids)
            .collect();
        chain
            .into_iter()
            .filter(|&id| self.is_logical_ancestor(id))
            .collect()
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
            self.emit(trace, observed_at_us, Backend::Uia, hwnd, selection);
            return;
        }
        if matches!(event.kind, UiaKind::Selection)
            && let Some(change) = self.selection_as_value(&event.parts)
        {
            self.emit(trace, observed_at_us, Backend::Uia, hwnd, change);
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
            UiaKind::Property(_) if is_progress_change_of(&event.kind, node.role) => {
                let Some(bar) = self.progress_reading(element.as_ref(), node) else {
                    return;
                };
                NormalizedEvent::ProgressChanged { bar }
            }
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
    /// can belong to another application. A progress bar the foreground
    /// window's subtree reported may belong to a window of another process,
    /// as a Settings page's content does under its frame: it is judged by
    /// the window attention names when that window's verdict is UIA, which
    /// also saves the call to find its nearest window, and otherwise by its
    /// nearest window, as a classic frame hosting a XAML island needs.
    fn uia_event_window(
        &self,
        event: &UiaEvent,
        of_focus: bool,
        element: Option<&Cached<Kept>>,
    ) -> Option<isize> {
        let bar_window = (is_progress_change(event) && !of_focus)
            .then(|| self.context.follow(|follow| follow.bar.window()))
            .flatten()
            .filter(|&window| read::window_uses_uia(self.context, window));
        if event.hwnd != 0 {
            Some(event.hwnd)
        } else if of_focus {
            self.context.tracked_window()
        } else if bar_window.is_some() {
            bar_window
        } else if event.focus_window != 0 && element.is_none() {
            // A windowless selection's nearest window, which the listener's
            // reading thread walked up to (`FocusReader::check_fact`).
            Some(event.focus_window)
        } else {
            element.and_then(|element| nearest_window_handle(element.live().element()))
        }
    }

    /// A UIA progress bar's change as its report uses it: its value and
    /// states from the event's cache (`node`), and where it is on the screen
    /// and whether it is off it, one call, as a progress bar's report needs
    /// them and an event's cache never holds them (`verbatim_uia::Place`).
    fn progress_reading(
        &mut self,
        element: Option<&Cached<Kept>>,
        node: NodeSnapshot,
    ) -> Option<ProgressReading> {
        let element = element?;
        let uia = self.client.uia()?;
        let place = uia
            .request_fixed::<Place>()
            .and_then(|request| element.live().refresh(&request))
            .ok()?;
        let place = place.read();
        Some(ProgressReading {
            id: node.id,
            value: node.value,
            offscreen: place.offscreen,
            rect: place.rect,
        })
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
    /// in its `ControllerFor` relation, an element the selected one is inside
    /// ("Selection in a list the focus controls" in `docs/nvda/events.md`).
    /// When the selected one is inside none, the first controlled element's
    /// current selection is reported in its place
    /// (`verbatim_uia_rops::Controlled::selected`), unless the selection
    /// names the focus itself, which the reducer reads as the focus's own
    /// (`reduce_selection_changed`): NVDA's base `event_selection` finds the
    /// focus in none of the elements it controls and falls through to its
    /// state change handler (`NVDAObjects/__init__.py` 1314 to 1339).
    fn controlled_selection(&mut self, runtime_id: &[i32]) -> Option<NormalizedEvent> {
        // Every report's focus, newest first, one of which is Core's, as
        // NVDA uses its focus object; Core speaks the selection only while
        // its controller is Core's focus.
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
        let current_instead = runtime_id != focus_id.as_slice();
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
            current_instead,
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
            // The search ran and did not find the element, and no current
            // selection was read from the first controlled element: it was
            // not asked for, the event named that element, it has no
            // Selection pattern, or nothing is selected in it. What the
            // relation named.
            tracing::debug!(
                selected = ?runtime_id,
                controlled = ?answer.controlled,
                current_instead,
                "a selection in none of the elements the focus controls, and no current selection read from the first"
            );
        }
        let selected = answer.selected?;
        let reported = selected.runtime_id();
        if reported != runtime_id {
            // Not found, so the first controlled element's current
            // selection, for a selection Settings' search suggestions raise
            // for an item it has already replaced
            // (`docs/parity/changes-and-selection.md`).
            tracing::debug!(
                selected = ?runtime_id,
                ?reported,
                controlled = ?answer.controlled,
                "a selection in none of the elements the focus controls; the first one's current selection is reported"
            );
        }
        let registry = self.context.nodes();
        // The controller's runtime id is UIA's own, kept with the element,
        // whatever its cache holds.
        let controller = focused.runtime_id();
        Some(NormalizedEvent::ControlledSelection {
            controller: registry.id_for_element(&controller, &focused),
            node: selected.snapshot(registry),
        })
    }

    /// The selection of the item `parts` names as a change of the focus's
    /// value, when the focus's value is its first selected item's name
    /// (`verbatim_uia::map::value_from_selection`) and the item, which
    /// does not have the keyboard focus, is one of the focus's children:
    /// NVDA's list item, whose state change under such a focused parent
    /// raises a value change on it, read as the selected item's name
    /// (`NVDAObjects/UIA/__init__.py` 2746 to 2767 and 2786 to 2800).
    /// One call finds the item among the focus's children with its name
    /// and whether it has the keyboard focus, both as they are now, as
    /// NVDA reads the item's `hasFocus` and the combo box's value live: the
    /// event's cache can hold no name for an item an application names
    /// late.
    fn selection_as_value(&mut self, parts: &UiaSnapshotFact) -> Option<NormalizedEvent> {
        // The reports' focuses the listener marked as ones whose value is
        // their first selected item's name, of every report the outpost has
        // not released, the newest first: the selected item is the child of
        // one of them.
        let marked: Vec<NodeId> = self.context.held().value_from_selection().collect();
        let uia = self.client.uia()?;
        let request = uia.request_fixed::<SelectedItem>().ok()?;
        let found = marked.into_iter().find_map(|focus| {
            let focused = self
                .context
                .store
                .element_of(focus)
                .and_then(|agile| agile.resolve().ok())?;
            uia.find_child_by_runtime_id(focused.live(), &parts.runtime_id, &request)
                .ok()
                .flatten()
                .map(|child| (focus, child))
        });
        let Some((focus, child)) = found else {
            tracing::debug!("a selection under no focus whose value it is: not its child");
            return None;
        };
        let item = child.read();
        if item.has_focus {
            // Its focus event says it, as NVDA's item with the focus
            // raises no value change.
            tracing::debug!("a selection of the focus's item that has the keyboard focus");
            return None;
        }
        Some(NormalizedEvent::ValueChanged {
            node_id: focus,
            value: item.name(),
        })
    }

    /// The runtime ids of the UIA focuses of the outpost's reports, newest
    /// first: the focuses a selection is read against, one of which is
    /// Core's, as NVDA reads it against its focus object.
    fn uia_report_focuses(&self) -> Vec<Vec<i32>> {
        let nodes: Vec<NodeId> = self
            .context
            .held()
            .focuses()
            .rev()
            .map(|focus| focus.node.id)
            .collect();
        let mut focuses: Vec<Vec<i32>> = Vec::new();
        for node in nodes {
            if let Some(runtime_id) = self.context.store.runtime_id_of(node)
                && !focuses.contains(&runtime_id)
            {
                focuses.push(runtime_id);
            }
        }
        focuses
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

    /// A fact routed from the listener, or one the outpost's own hook took
    /// in: an MSAA focus, foreground change, menu opening, alert, or
    /// tooltip, whichever brought it.
    /// A fact, with the outpost's test of it against the foreground, made
    /// as the reducer took it in ([`Front`]).
    fn fact(&mut self, fact: DeliveredFact, front: Front, trace: TraceId, observed_at_us: u64) {
        match fact {
            DeliveredFact::Foreground { hwnd } => {
                self.foreground(hwnd, front, trace, observed_at_us);
            }
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => self.msaa_focus((hwnd, id_object, id_child), front, trace, observed_at_us),
            DeliveredFact::UiaFocusRead { read, .. } => {
                self.answers = read.answers;
                self.uia_focus(&read, trace, observed_at_us);
            }
            // The listener's answer to Core's live re-read of gone, passed on
            // in order with this outpost's events.
            DeliveredFact::UiaLiveRereadGone { request } => {
                self.commit(Effects {
                    result: Some(OutpostToCore::LiveRereadGone { request }),
                    ..Effects::default()
                });
            }
            DeliveredFact::MsaaLiveReread { request, trace_id } => {
                self.msaa_live_reread(request, trace_id);
            }
            // The intake resolves these; none reaches the worker.
            DeliveredFact::UiaFocusMarker { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded => {}
            // The intake unwraps a marked event as it takes it in.
            DeliveredFact::UiaMarked { fact, .. } => {
                self.fact(*fact, front, trace, observed_at_us);
            }
            DeliveredFact::UiaSelection {
                hwnd,
                focus_window,
                snapshot,
            } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Selection,
                    parts: snapshot,
                    hwnd,
                    focus_window,
                    element: None,
                },
                trace,
                observed_at_us,
            ),
            DeliveredFact::UiaNotification {
                hwnd,
                snapshot,
                notification,
            } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Notification(notification),
                    parts: snapshot,
                    hwnd,
                    focus_window: 0,
                    element: None,
                },
                trace,
                observed_at_us,
            ),
            DeliveredFact::UiaToast { hwnd, snapshot } => self.uia_event(
                UiaEvent {
                    kind: UiaKind::Toast,
                    parts: snapshot,
                    hwnd,
                    focus_window: 0,
                    element: None,
                },
                trace,
                observed_at_us,
            ),
            DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            } => self.alert(hwnd, id_object, id_child, trace, observed_at_us),
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => self.tooltip_shown(hwnd, id_object, id_child, trace, observed_at_us),
            // Menu openings are planned separately; one reaching here is
            // handled the same way.
            menu @ DeliveredFact::MenuPopup { .. } => {
                self.menu_opened(&Entry {
                    item: Item::Fact(menu, None),
                    trace,
                    observed_at_us,
                    timing: verbatim_model::protocol::EventTiming::default(),
                    front: Front::Untested,
                });
            }
        }
    }

    /// A tooltip window shown. A help balloon is reported as an alert, for
    /// the reducer to speak queued from any application, as NVDA's
    /// notification behavior speaks a help balloon, which NVDA reports by
    /// default; an ordinary tooltip is not, as NVDA does not report
    /// tooltips by default.
    fn tooltip_shown(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_us: u64,
    ) {
        // NVDA accepts a show event by its window's class alone, whatever
        // the foreground; the hook that took this one, the outpost's own or
        // the focus listener's, took it only from a tooltip window
        // (`verbatim_ia2::hook`), so it passes. Its object from a native UIA
        // window is dropped, as NVDA drops every `WinEvent` of one.
        if !self.readable(hwnd) || read::window_uses_uia(self.context, hwnd) {
            return;
        }
        let Some(mut object) = verbatim_ia2::acquire::event_object(hwnd, id_object, id_child)
        else {
            return;
        };
        if object.role() != Role::HelpBalloon {
            return;
        }
        let node = object.read(self.context.nodes(), Purpose::Announce);
        self.emit(
            trace,
            observed_at_us,
            Backend::Msaa,
            Some(hwnd),
            NormalizedEvent::Alert {
                node,
                origin: AlertOrigin::MsaaAlert,
            },
        );
    }

    /// An MSAA alert. Only a toast (its window's parent has the class
    /// `ToastChildWindowClass`) is reported, for the reducer to speak queued
    /// from any application, as NVDA's notification behavior speaks it.
    /// NVDA speaks other alerts only when the object's role is alert, it has
    /// content, and it is not already among the focus's ancestors; Verbatim
    /// has no alert role yet, so those are not reported.
    fn alert(
        &mut self,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        trace: TraceId,
        observed_at_us: u64,
    ) {
        if !self.accepts(EventName::Alert, hwnd) {
            return;
        }
        if super::window::parent_class(hwnd).as_deref() != Some("ToastChildWindowClass") {
            return;
        }
        // Its object from a native UIA window is dropped, as NVDA drops
        // every `WinEvent` of one (`docs/parity/alerts.md`, "Toasts").
        if !self.readable(hwnd) || read::window_uses_uia(self.context, hwnd) {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            self.context.nodes(),
            verbatim_ia2::acquire::Purpose::Announce,
        ) else {
            return;
        };
        self.emit(
            trace,
            observed_at_us,
            Backend::Msaa,
            Some(hwnd),
            NormalizedEvent::Alert {
                node,
                origin: AlertOrigin::MsaaAlert,
            },
        );
    }

    /// Finds the foreground window's element, for its progress bars
    /// (`docs/parity/changes-and-selection.md`, "Progress bars that are not
    /// the focus"): one call. The element is committed, under a token the
    /// shell gives it; a window with no element commits nothing, and the
    /// program's ending answers the lookup.
    fn find_bar(&mut self, window: isize) {
        let Some(uia) = self.client.uia() else {
            return;
        };
        let element = uia
            .request_fixed::<Kept>()
            .and_then(|request| uia.any_window_element::<Kept>(window, &request))
            .and_then(|element| element.agile());
        match element {
            Ok(element) => {
                self.commit(Effects {
                    bar_found: Some((window, element)),
                    ..Effects::default()
                });
            }
            Err(error) => tracing::debug!(%error, window, "the foreground window's element"),
        }
    }

    /// A foreground change: reported at once, named or not, as a focus on
    /// the window, unless the outpost's one test, made on the reducer's
    /// thread as the change was taken in (`front`), found the window not to
    /// be the system's foreground window, or the window is read through UI
    /// Automation. The test is not made here, since this worker can wait
    /// behind a call into a held application
    /// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title").
    ///
    /// It is stamped with the time that test found its window in front,
    /// never before the event was observed, so the old foreground window's
    /// own focus events raised before the switch are never newer than the
    /// change. NVDA tests a foreground event once against the foreground
    /// window, and drops it when they differ (`processForegroundWinEvent`,
    /// `IAccessibleHandler/__init__.py` 834 to 841), with no test after its
    /// object is made; so does this, without waiting for the window to come
    /// in front. Measured live: the system's
    /// foreground event reached an out-of-context hook only once
    /// `GetForegroundWindow` named its window, and the events that did not
    /// were stale or raised early by the application itself, as File
    /// Explorer raises one as it creates its window and the system's own
    /// follows once the window is in front (`books/dev/src/reference/crates/verbatim-outpost.md`,
    /// "Foreground changes and focus-now").
    ///
    /// A window not in front at the test is dropped and never kept, as NVDA drops
    /// it; so is a window read through UI Automation, for which NVDA drops
    /// every MSAA event, a foreground event included (`winEventToNVDAEvent`,
    /// `IAccessibleHandler/__init__.py` 576 to 582). Either window is
    /// announced as the focus enters it, from the focus's ancestry, where the
    /// top-level window is an ancestor like any other: the reducer speaks
    /// the entered ancestors when the focus's top-level window changes
    /// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title"). A
    /// console window's
    /// foreground event comes before the window is in front, and the window
    /// is read through UI Automation besides, so its title is the console
    /// window element's, the text area's parent.
    ///
    /// Every change in front is read and reported: whether the focus is
    /// already in its window, NVDA's two tests against its last queued
    /// focus, is Core's verdict, which refuses a foreground change of the
    /// window its focus is in (`books/dev/src/explanation/design/ownership.md`).
    ///
    /// Within a focus group, a change dropped lets the next older entry be
    /// tried, newest first ([`super::reducer`]), as NVDA's `pumpAll`
    /// stops at the first foreground or focus event processed; so does a
    /// change reported whose window holds an unreleased node of a report's
    /// line, which Core may refuse, and every report of the group carries
    /// its rank, so Core takes the older entry only when it refused the
    /// change. A change reported of any other window ends the group.
    fn foreground(&mut self, hwnd: isize, front: Front, trace: TraceId, observed_at_us: u64) {
        if let Some(observer) = self.context.foreground_programs.get() {
            observer(hwnd);
        }
        if window_belongs_to_hidden_frame(hwnd) {
            tracing::debug!(hwnd, "foreground dropped: Core's hidden frame");
            return;
        }
        let Front::Foreground {
            in_front_at_us: Some(in_front_at_us),
            ..
        } = front
        else {
            debug_assert!(
                matches!(front, Front::Foreground { .. }),
                "a foreground change is tested as it is taken in"
            );
            tracing::debug!(hwnd, "foreground dropped: not the foreground window");
            return;
        };
        // NVDA makes the foreground's object as it makes any MSAA event's
        // (`processForegroundWinEvent` through `winEventToNVDAEvent`): a
        // window gone, ghosted, or hung is dropped before the UIA test.
        if !self.readable(hwnd) {
            return;
        }
        match read::own_window_verdict(self.context, hwnd) {
            None => return,
            Some(true) => {
                tracing::debug!(hwnd, "foreground dropped: UIA owns the window");
                return;
            }
            Some(false) => {}
        }
        // Ordered by when its window was found in front, which is never
        // before the event was observed.
        self.report_foreground(hwnd, trace, in_front_at_us.max(observed_at_us));
    }

    /// Reads the foreground window `hwnd`'s name and reports it, stamped
    /// `confirmed_at_us`, when the outpost's one test found it in front: a
    /// window that leaves the front during the read is still reported, and
    /// judged by Core's foreground record.
    fn report_foreground(&mut self, hwnd: isize, trace: TraceId, confirmed_at_us: u64) {
        // Whether the window holds an unreleased node of a report's line,
        // judged before this change is recorded among the reports: then
        // the group's next older entry is tried after it.
        let top_level = Window::new(top_level_of(hwnd));
        let holds_a_line = top_level.is_some()
            && self
                .context
                .held()
                .focuses()
                .any(|focus| focus.top_level == top_level);
        let (backend, node) = read::foreground_window(self.context, self.client, hwnd);
        self.emit_focus(
            trace,
            confirmed_at_us,
            backend,
            Some(hwnd),
            node,
            FocusSource::WholeWindow,
            None,
            (Vec::new(), None),
        );
        if self.reported && holds_a_line {
            self.reported = false;
        }
    }

    /// An MSAA focus fact.
    ///
    /// NVDA's acceptance test against the foreground window was made on the
    /// reducer's thread as the fact was taken in (`front`), as NVDA makes it
    /// in its pump, not here, where this worker can wait behind a call into
    /// a held application.
    fn msaa_focus(
        &mut self,
        (hwnd, id_object, id_child): (isize, i32, i32),
        front: Front,
        trace: TraceId,
        observed_at_us: u64,
    ) {
        if id_child == CHILDID_SELF && window_belongs_to_hidden_frame(hwnd) {
            return;
        }
        debug_assert!(
            matches!(front, Front::Accepted(_)),
            "an MSAA focus is tested as it is taken in"
        );
        if front != Front::Accepted(true) {
            tracing::debug!(
                hwnd,
                "event dropped: its window is not related to the foreground window"
            );
            return;
        }
        if !self.readable(hwnd) {
            return;
        }
        let Some(uia_owns) = read::own_window_verdict(self.context, hwnd) else {
            return;
        };
        if uia_owns {
            // UIA owns this window; its UIA fact reports the focus, as
            // NVDA's `winEventToNVDAEvent` drops a native UIA window's
            // event.
            tracing::debug!(hwnd, "MSAA focus dropped: UIA owns the window");
            return;
        }
        // A control taking the focus raised focus on itself and then on its
        // focused child, and the control's event was already reported as
        // the child (`verbatim_ia2::acquire::focus_candidate`): NVDA reports
        // the child once when the two events fall in one of its pumps,
        // trying them newest first; when the child's comes in a later pump,
        // NVDA reports the control and then the child (`docs/parity/windows-and-processes.md`,
        // "A control's own focus with a focused child").
        if self
            .context
            .is_redirected_focus(&Object::Msaa(hwnd, id_object, id_child))
        {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: already reported from its control's focus"
            );
            self.commit(Effects {
                mark: Some(MarkChange::ClearRedirect),
                ..Effects::default()
            });
            return;
        }
        let Some(mut candidate) = verbatim_ia2::acquire::focus_candidate(hwnd, id_object, id_child)
        else {
            tracing::debug!(hwnd, id_object, id_child, "MSAA focus dropped: unreadable");
            return;
        };
        // A repeat of the focus is read in full and judged in Core, whose
        // duplicate rule compares it with the last focus it took, of any
        // application, as NVDA's `isDuplicateIAccessibleEvent` compares
        // with its last queued focus (`books/dev/src/explanation/design/ownership.md`).
        let resolved_child = candidate.resolved_child();
        // NVDA accepts an MSAA focus only when the object or one of its
        // ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`),
        // which weeds out stale and spurious focus events. It checks the
        // states after the duplicate test, reading them live, going on
        // past a failed `accParent` from the parent window's own object,
        // and above the process boundary by plain windows, never focused,
        // with no call there.
        let context = self.context;
        let client = &mut *self.client;
        if !candidate.has_focused_state(
            read::MAX_ANCESTOR_HOPS,
            &|window| read::reads_as_plain(context, window),
            &mut |window| read::parent_window_for_focused_state(context, client, window),
        ) {
            tracing::debug!(
                hwnd,
                id_object,
                id_child,
                "MSAA focus dropped: nothing has the focused state"
            );
            return;
        }
        // The focus and the ancestors its walk reads are compared with the
        // focus line Core holds and the newest focus Core has not judged, by
        // NVDA's equality (`books/dev/src/reference/adr/0001-focus.md`, "Object identity").
        let candidates = read::msaa_candidates(self.context);
        let node = candidate.read(self.context.nodes(), Some(&candidates));
        self.report_msaa_focus(
            node,
            ((hwnd, id_object, id_child), resolved_child > 0),
            (trace, observed_at_us),
            &candidates,
        );
    }

    /// Reports `node`, the MSAA focus NVDA's checks accepted from the
    /// event at `event`, acquired as a simple child when `simple_child`,
    /// traced `trace` and observed at `observed_at_us`, with its ancestors
    /// walked, once the walk returns, however long it takes within the
    /// program's deadline.
    fn report_msaa_focus(
        &mut self,
        node: NodeSnapshot,
        (event, simple_child): ((isize, i32, i32), bool),
        (trace, observed_at_us): (TraceId, u64),
        candidates: &verbatim_ia2::equality::Candidates,
    ) {
        let (hwnd, id_object, id_child) = event;
        let previous = self.focus_chain();
        let object = self
            .context
            .store
            .key_of(node.id)
            .map(|(hwnd, object, child)| Object::Msaa(hwnd, object, child));
        let redirected = object
            .as_ref()
            .is_some_and(|object| *object != Object::Msaa(hwnd, id_object, id_child));
        let msaa_event = (event, simple_child);
        let Ok(enrichment) = read::msaa_enrichment(
            self.context,
            self.client,
            (&node, &previous, Some(candidates)),
        ) else {
            tracing::debug!(%trace, "MSAA focus lost: a UIA window's element on its chain's way could not be read");
            return;
        };
        if self.refused() {
            return;
        }
        self.msaa_event = Some(msaa_event);
        self.redirected = redirected;
        self.emit_focus(
            trace,
            observed_at_us,
            Backend::Msaa,
            Some(hwnd),
            node,
            FocusSource::MsaaFocus(msaa_event.0, msaa_event.1),
            object.clone(),
            enrichment,
        );
    }

    /// A UIA focus as the focus listener judged and read it from the
    /// event's own element (`books/dev/src/explanation/design/focus.md`, "One UI Automation focus,
    /// end to end"):
    /// it was no repeat of the focus Core holds, its nearest window
    /// passed NVDA's acceptance test against the foreground window, the
    /// element had the keyboard focus when the listener read it, as
    /// NVDA's `shouldAllowUIAFocusEvent` requires, and its states, value,
    /// details, ancestors, and selected child are the element's own, read
    /// with the classic walk, one call at a time on the element the
    /// listener holds. The outpost reports it at once, with no test of its
    /// window's hang: a window that hangs is waited for, each call within
    /// the program's deadline, and its hang told by the program's check
    /// (Dickson, 2026-11-07). Its node
    /// minted from its runtime id; the listener's evidence that the element
    /// it read before under that runtime id has lost the focus or died
    /// gives the id a new node first. Only then, off the way to speech,
    /// does the outpost find its own copy of the element, which no process
    /// can hand it (experiment E-marshal), by the path the listener read
    /// ([`find_focus_element`](Self::find_focus_element)); for a focus that
    /// may have text, before it is reported, since its caret is read with
    /// it.
    fn uia_focus(&mut self, read: &UiaFocusRead, trace: TraceId, observed_at_us: u64) {
        let context = self.context;
        let judged = (|| {
            // NVDA's acceptance test against the foreground window was
            // made by the listener, before it read the focus, as NVDA
            // makes it before it queues one
            // (`books/dev/src/reference/adr/0001-focus.md`, "The listener's checks, ported from
            // NVDA's focus handler").
            let reported = Self::uia_focus_window(context, read.hwnd, read.window)?;
            // The listener makes no backend verdict; the outpost's one
            // verdict per window decides which backend's focus counts in
            // each of its windows, judged at the read's observation
            // (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact"):
            // in a window it judges not
            // UIA, its MSAA focus speaks, and the listener's read is
            // dropped.
            // A live re-read's answer keeps the verdict its report was
            // given, as it reads the same element again.
            let judged_window = read.window.or(Some(read.hwnd).filter(|&hwnd| hwnd != 0));
            if read.answers.is_none()
                && let Some(window) = judged_window
                && read::own_window_verdict(context, window) != Some(true)
            {
                tracing::debug!(
                    %trace,
                    window,
                    "UIA focus dropped: the outpost judges its window MSAA, whose focus speaks for it, or another application serves it"
                );
                return Err(Dropped);
            }
            Ok(reported)
        })();
        let Ok(reported) = judged else {
            return;
        };
        let runtime_id = &read.snapshot.runtime_id;
        self.note_earlier(read);
        let reading = Instant::now();
        let previous = self.focus_chain();
        let node = Self::uia_node(context, &read.snapshot, None);
        let selected = read
            .selected_child
            .as_ref()
            .map(|child| Self::uia_node(context, child, None));
        let Ok(ancestors) = read::uia_chain_from_read(
            context,
            self.client,
            (&read.ancestors, read.hwnd, read.window, read.ancestors_end),
            &previous,
        ) else {
            tracing::debug!(%trace, "UIA focus lost: a UIA window's element on its chain's way could not be read");
            return;
        };
        // The ancestors the listener took from its ancestry cache, the
        // facts from its cache join on, are cached ancestors of the report.
        if let Some(from) = read.cached_from {
            context.note_cached(
                read.ancestors
                    .iter()
                    .skip(usize::try_from(from).unwrap_or(usize::MAX))
                    .filter_map(|ancestor| context.nodes().existing(&ancestor.snapshot.runtime_id)),
            );
        }
        // Committed with the focus; the entry finds the focus's element by
        // it before then.
        let path = FocusPath::of(read);
        self.focus_path = Some(path.clone());
        tracing::debug!(
            %trace,
            enrichment_us = reading.elapsed().as_micros(),
            "UIA focus reported from the listener's read"
        );
        // A focus on the element this outpost already found under its
        // runtime id keeps it: the listener read that it still has the
        // keyboard focus. A repeat comes this far only when the focus
        // before it waited for Core's judgement as it was read, which Core's
        // own rule then drops (section 16.3); otherwise this is the focus
        // coming back to the element after one in another application.
        let found = read.same_as_last == Some(EarlierElement::Focused)
            && context.store.element_of(node.id).is_some();
        // A focus that may have text needs its own element before it is
        // reported, since its caret is read with it; any other's is found
        // after, off the way to speech.
        let text_first = !found
            && matches!(
                node.role,
                Role::EditableText | Role::Document | Role::Terminal
            );
        if text_first {
            self.find_focus_element(runtime_id, true);
        }
        if self.refused() {
            return;
        }
        self.value_from_selection = read.value_from_selection;
        self.emit_focus(
            trace,
            observed_at_us,
            Backend::Uia,
            reported,
            node,
            FocusSource::ListenerRead(path),
            Some(Object::Uia(runtime_id.clone())),
            (ancestors, selected),
        );
        if !found
            && !text_first
            && !self.refused()
            && context.focused() == Some(Object::Uia(runtime_id.clone()))
            && let Some(id) = self.find_focus_element(runtime_id, false)
            && let Some((role, chain)) = context.follow(|follow| {
                follow
                    .followed
                    .as_ref()
                    .map(|followed| (followed.role, followed.chain.clone()))
            })
        {
            // The subscription follows the element, and the slider a thumb
            // is in, in a commit made only while the node is the followed
            // focus.
            let element = match target_for(context, (id, role), &chain, false) {
                (_, Some(_)) => self.slider_target(id, role, &chain),
                (target, None) => target.map(|target| (id, target)),
            };
            if element.is_some() {
                self.commit(Effects {
                    element,
                    ..Effects::default()
                });
            }
        }
    }

    /// Notes what the listener read of the element it held before under
    /// the UIA focus `read` names, which its report carries
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity").
    fn note_earlier(&mut self, read: &UiaFocusRead) {
        let runtime_id = &read.snapshot.runtime_id;
        // A runtime id keeps its node when its application gives it to a
        // new element (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"): on the
        // listener's evidence that the element it
        // stood for lost the focus or is gone, the element kept for it is
        // evicted by the report's commit, if still kept then, and its
        // terminal entry goes in the commit; its text is forgotten now, in
        // the program's copies, and from the store with its next commit, so
        // the focus's caret read fetches the new element's pattern, never
        // the earlier element's (`docs/parity/focus-events.md`, "Duplicate
        // focus suppression").
        self.earlier = read.same_as_last;
        if let Some(EarlierElement::NotFocused | EarlierElement::Gone) = read.same_as_last
            && let Some(node) = self.context.nodes().existing(runtime_id)
        {
            tracing::debug!(
                earlier = ?read.same_as_last,
                "UIA focus: its runtime id named an element that lost the focus or is gone; the node's element and text are dropped"
            );
            text_reads::forget_text(self.context, node);
            self.stale = Some(Stale {
                node,
                element: self.context.store.element_of(node),
            });
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

    /// Finds this outpost's own copy of the focus `runtime_id` names, from
    /// the listener's read, by the path the listener read: from the nearest
    /// window in the focus's chain, down by runtime ids, one connection
    /// call and one transaction, and keeps it under the focus's node, whose
    /// id it returns. A `text` focus with no window of its own whose walk up
    /// the listener cut, so that no path leads to it, is found as the
    /// application's focused element instead
    /// ([`element_after_cut_walk`](Self::element_after_cut_walk)). Not
    /// found, as Windows Terminal's tab focus elements,
    /// which are in no tree, are not, nothing is kept, its text cannot be
    /// read, and nothing is followed, as NVDA hears nothing local before
    /// its group is added to a focus (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's
    /// own element").
    fn find_focus_element(&mut self, runtime_id: &[i32], text: bool) -> Option<NodeId> {
        let finding = Instant::now();
        let found = self
            .element_by_focus_path(runtime_id)
            .or_else(|| text.then(|| self.element_after_cut_walk(runtime_id))?);
        tracing::debug!(
            found = found.is_some(),
            elapsed_us = finding.elapsed().as_micros(),
            "the focus's own element looked for by its path"
        );
        let Some(element) = found else {
            tracing::debug!("the focus's element was not found; nothing is followed");
            return None;
        };
        Some(self.context.nodes().id_for_element(runtime_id, &element))
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

    /// The element of a windowless focus whose walk up the listener cut
    /// ([`FocusPath::is_cut_at`]): the application's focused element, read
    /// with the focus read's checks, its process-id check among them
    /// ([`read::focused_element_in`]), and taken when its runtime id is the
    /// focus's (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's own element"), since a
    /// walk down from the nearest window would find
    /// it only as that window's direct child, and a search below the window
    /// costs a round of provider calls per element. Found only while the
    /// element still has the keyboard focus, a limit.
    fn element_after_cut_walk(&mut self, runtime_id: &[i32]) -> Option<Cached<Kept>> {
        let context = self.context;
        let cut = match &self.focus_path {
            Some(path) => path.is_cut_at(runtime_id),
            None => context.follow(|follow| {
                follow
                    .followed
                    .as_ref()
                    .and_then(Followed::focus_path)
                    .is_some_and(|path| path.is_cut_at(runtime_id))
            }),
        };
        if !cut {
            return None;
        }
        let element =
            read::focused_element_in(context, self.client, read::FocusRead::CutWalk).ok()?;
        if element.runtime_id() != runtime_id {
            tracing::debug!(
                "the application's focused element is not the focus the listener read; the focus's element is not found"
            );
            return None;
        }
        Some(element.narrow())
    }

    /// The window a UIA focus is reported in, or [`Dropped`] when the focus
    /// is in Core's hidden frame. Whether the window uses UIA is the
    /// outpost's verdict, judged by the caller.
    fn uia_focus_window(
        context: &Context,
        fact_hwnd: isize,
        remote_window: Option<isize>,
    ) -> Result<Option<isize>, Dropped> {
        // The event's own window; else the element's nearest, which the
        // listener's read found with its ancestors and sends with every
        // read; else, for a read without one, which only a test makes, this
        // application's focus window now, so every focus report carries
        // window facts. A read with none of them is not reported.
        let reported = if fact_hwnd != 0 {
            Some(fact_hwnd)
        } else {
            remote_window
        };
        let reported = reported.or_else(|| {
            context
                .focus_window()
                .filter(|&window| verbatim_win32::window::served_by(window, context.target_pid()))
        });
        if reported.is_none_or(window_belongs_to_hidden_frame) {
            return Err(Dropped);
        }
        Ok(reported)
    }

    /// A menu opening, handled after the batch's focus events, as NVDA's
    /// MSAA handler does. Ignored when a focus reported in the same batch
    /// already put focus on a menu or menu item (NVDA's condition is a
    /// valid focus in its pump, a duplicate included, while its last
    /// queued focus is a menu or menu item), when the window belongs to UIA
    /// (NVDA does not use MSAA objects proxied from native UIA), or when its
    /// object is not a popup menu; otherwise it becomes a focus on the
    /// popup menu.
    fn menu_opened(&mut self, entry: &Entry) {
        let Item::Fact(
            DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            },
            _,
        ) = entry.item
        else {
            return;
        };
        let last = self.context.follow(|follow| follow.last_reported);
        if last.is_some_and(|last| {
            last.batch == self.batch && matches!(last.role, Role::Menu | Role::MenuItem)
        }) {
            return;
        }
        if !self.accepts(EventName::MenuStart, hwnd) || !self.readable(hwnd) {
            return;
        }
        // UIA owns this window, or another application does.
        if read::own_window_verdict(self.context, hwnd) != Some(false) {
            return;
        }
        let Some(node) = verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            self.context.nodes(),
            verbatim_ia2::acquire::Purpose::Announce,
        ) else {
            return;
        };
        if node.role != Role::Menu {
            return;
        }
        // Verbatim's own menu opens from Core's hidden frame, shown and
        // brought to the foreground for it, whose own foreground report is
        // dropped because the frame transits focus. NVDA announces the
        // foreground window a menu opens from before the menu, so the menu
        // is reported in the frame's window, the foreground window, with the
        // frame, titled "Verbatim", as its ancestor: the reducer names a new
        // foreground window from the top of the focus's ancestry when no
        // foreground report named it, then the menu.
        let foreground = foreground_window_handle();
        let (window, ancestors) = if window_is_hidden_frame(foreground) {
            let (_, frame) = read::foreground_window(self.context, self.client, foreground);
            (foreground, vec![frame])
        } else {
            (hwnd, Vec::new())
        };
        self.emit_focus(
            entry.trace,
            entry.observed_at_us,
            Backend::Msaa,
            Some(window),
            node,
            FocusSource::MenuOpening,
            Some(Object::Msaa(hwnd, id_object, id_child)),
            (ancestors, None),
        );
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
                purpose: ReadPurpose::BeforeKey,
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
        // The check before the query's first call: a query whose named
        // window is hung or ghosted is answered abandoned with no call, its
        // commit naming the window hung, so a query on a hung window parks
        // no worker. A focus read is not refused: its own check of the
        // focus window answers no focused control in a window that cannot
        // be read, so nothing is said for it (Dickson, 2026-11-07).
        if !matches!(query, Query::ReadFocus { .. })
            && let Some(window) = self.context.charge().named(self.context.request())
            && self.context.readability(window.get()).is_hung()
        {
            self.context.charge().found_hung(window.get());
            tracing::debug!(
                ?query,
                window = window.get(),
                "query refused: its window is not responding"
            );
            let result = self.reply_message(request_id, trace, QueryOutcome::Abandoned);
            self.commit(Effects {
                result: Some(result),
                ..Effects::default()
            });
            return;
        }
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
            Some((answered, judged)) => self.follow_answer(result, answered, judged, trace),
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
    fn focus_now(
        &mut self,
        purpose: FocusPurpose,
    ) -> (FocusNow, Option<(AnsweredFocus, reducer::Focus)>) {
        let context = self.context;
        let (mut answer, window) =
            read::focus_now(context, self.client, purpose == FocusPurpose::Fake);
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
        read::describe_dialogs(
            context,
            self.client,
            (windows.chain(focus), None),
            (&mut None, false),
        );
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
        // Core takes the answer's control as its focus, as NVDA queues its
        // fake focus: until Core has judged it, events are judged against it
        // too, recorded with the answer's commit.
        let answered = answered.map(|answered| {
            let judged = self.focus_to_judge(&answered.node, &answered.ancestors, window, false);
            (answered, judged)
        });
        (answer, answered)
    }

    /// Commits `result`, a focus-now answer, with its control, `answered`,
    /// made the outpost's focus and followed, as a reported focus is
    /// ([`emit_focus`](Self::emit_focus)), and `judged` recorded for Core's
    /// judgement, keeping the MSAA event address of the same focus judged
    /// against before it, which NVDA's second foreground test compares
    /// with ([`answer_to_judge`]): so what the follow-up sends, such as a
    /// terminal's first read, reaches Core after the focus it follows. A
    /// control already followed under the same node is followed again, not
    /// adopted. Either way the answer becomes the last
    /// focus queued, as NVDA's fake focus does.
    fn follow_answer(
        &mut self,
        result: OutpostToCore,
        answered: AnsweredFocus,
        judged: reducer::Focus,
        trace: TraceId,
    ) {
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
                batch: self.batch,
                event: (trace, observed_at_us),
                redirected: false,
                caret,
                origin: Origin::Answer,
            },
        );
        let committed = self.commit(Effects {
            result: Some(result),
            report: Some(Report::Answered(judged)),
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

/// The focus `node`, with `ancestors`, reported in `window`, as the
/// outpost records it among its reports ([`reducer::Held`]): an MSAA focus carries `msaa_event`, its event's
/// address and whether it is a simple child, and a foreground change its
/// window's event address; each dialog of its line carries the text the
/// report gave it. NVDA makes its foreground object when the focus enters
/// a top-level window, so the window keeps the verdict it had then while
/// the focus moves within it ([`Worker::is_foreground_window`]): the
/// report carries what was read of its top-level window being in front,
/// and the rule that decides it runs as the report is recorded
/// (`reducer::Held::entered_in_front`), on the program's copy and in the
/// reducer alike. Local calls.
fn judged_focus(
    context: &RequestContext<'_>,
    (node, ancestors): (&NodeSnapshot, &[NodeSnapshot]),
    (window, foreground): (Option<isize>, bool),
    msaa_event: Option<((isize, i32, i32), bool)>,
) -> reducer::Focus {
    let top_level = window.and_then(|window| Window::new(top_level_of(window)));
    // The evidence; the reducer's rule decides what the window entered
    // with as the report is recorded (`reducer::Held::entered_in_front`).
    let in_front = if foreground {
        reducer::InFront::Foreground(top_level)
    } else {
        reducer::InFront::Read(
            top_level.filter(|top_level| top_level.get() == context.foreground_window()),
        )
    };
    let dialog_texts = ancestors
        .iter()
        .chain(std::iter::once(node))
        .filter(|node| crate::dialog_text::is_dialog(node.role))
        .map(|node| (node.id, node.details.description.clone()))
        .collect();
    let ancestors = ancestors.iter().map(HeldNode::of).collect();
    let (event, simple_child) = match msaa_event {
        Some((event, simple_child)) => (Some(event), simple_child),
        None => (window.filter(|_| foreground).map(foreground_event), false),
    };
    reducer::Focus {
        node: HeldNode::of(node),
        ancestors,
        top_level,
        in_front,
        entered_in_front: None,
        event,
        simple_child,
        dialog_texts,
        value_from_selection: false,
    }
}

/// What a focus report came from, for [`Worker::emit_focus`]: the focus
/// reader that read it, when it is one whose walks may reuse an ancestry
/// cache, and whether it is a whole-window focus.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FocusSource {
    /// The focus listener's read of a UIA focus, its live re-read's answer
    /// among them, with the raw-view chain it read.
    ListenerRead(FocusPath),
    /// The outpost's own MSAA focus, its live re-read's answer among them,
    /// with its event's address and whether it is a simple child.
    MsaaFocus((isize, i32, i32), bool),
    /// A whole-window focus, from a foreground change, which reads no
    /// chain.
    WholeWindow,
    /// An MSAA menu opening, whose chain is at most Core's hidden frame.
    MenuOpening,
}

impl FocusSource {
    /// The focus reader whose ancestry cache the report's walk may have
    /// reused, which a live re-read of it is asked of.
    fn reader(&self) -> Option<FocusReaderKind> {
        match self {
            Self::ListenerRead(_) => Some(FocusReaderKind::Listener),
            Self::MsaaFocus(..) => Some(FocusReaderKind::Outpost),
            Self::WholeWindow | Self::MenuOpening => None,
        }
    }

    /// How a report from this source was read, for the focus it adopts,
    /// the event `trace` observed at `observed_at_us`; `None` for a
    /// whole-window focus, which is a foreground report and adopts nothing.
    fn origin(self, trace: TraceId, observed_at_us: u64) -> Option<Origin> {
        match self {
            Self::ListenerRead(path) => Some(Origin::Listener(path)),
            Self::MsaaFocus(event, simple_child) => Some(Origin::Msaa(MsaaReport {
                trace,
                event,
                simple_child,
                observed_at_us,
            })),
            Self::MenuOpening => Some(Origin::MenuOpening),
            Self::WholeWindow => None,
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

/// What a live re-read of the focus followed needs, when it is an MSAA
/// focus.
fn followed_msaa_report(follow: &reducer::FollowState) -> Option<MsaaReport> {
    follow.followed.as_ref().and_then(Followed::msaa_report)
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
        match (self, reads) {
            (Self::Text, NoteReads::CaretOnly) => None,
            (Self::Caret | Self::Now, NoteReads::CaretOnly) => Some(ReadShape::Reported),
            (
                Self::Caret | Self::Text | Self::Now,
                NoteReads::CaretAndText | NoteReads::Poll { .. },
            ) => Some(ReadShape::Compared),
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
    /// Its entry's batch.
    batch: u64,
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
    /// The focus adopted, its report leaving the dialog memo `dialog`.
    fn adopted(self, dialog: Option<reducer::DialogMemo>) -> FocusEffects {
        FocusEffects::Adopt {
            followed: self.followed,
            reported: self.reported,
            dialog,
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
        made.adopted(None)
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
        batch,
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
        reported: LastReported { role, batch },
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
            reported: LastReported {
                role: Role::Button,
                batch: 1,
            },
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

    /// Each kind of focus source adopts with its own origin, and a
    /// whole-window focus adopts nothing.
    #[test]
    fn a_focus_is_adopted_with_the_origin_of_its_source() {
        let trace = TraceId::from_raw(4);
        let event = (9, -4, 1);
        let origin = |source: FocusSource| source.origin(trace, 50);
        assert_eq!(
            origin(FocusSource::ListenerRead(FocusPath::default())),
            Some(Origin::Listener(FocusPath::default()))
        );
        assert_eq!(
            origin(FocusSource::MsaaFocus(event, true)),
            Some(Origin::Msaa(MsaaReport {
                trace,
                event,
                simple_child: true,
                observed_at_us: 50,
            }))
        );
        assert_eq!(origin(FocusSource::MenuOpening), Some(Origin::MenuOpening));
        assert_eq!(origin(FocusSource::WholeWindow), None);
    }

    #[test]
    fn the_system_carets_event_is_tested_as_a_location_change() {
        assert_eq!(
            event_name_of(WinEventKind::Caret),
            Some(EventName::LocationChange)
        );
    }
}
