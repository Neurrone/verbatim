//! The listener's tells of `WinEvent`s (ADR 0003, "Readers tell, Core
//! asks"; design `docs/design/readers-tell-core-asks-build1.md`, "The
//! listener in this build"): each event the listener hooks desktop-wide,
//! after NVDA's local drops, becomes a tell carrying what came with it and
//! the facts gathered in the callback with calls to Windows only, and goes
//! to Core in the listener's one order of tells, behind any pending
//! foreground ([`Tells`]).
//!
//! The decisions are `verbatim_listener_logic::tell`'s, pure; this module
//! makes the reads and sends.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use verbatim_ia2::{WinEvent, WinEventKind};
use verbatim_listener_logic::foreground::{Front, Pending as Foreground, Seen, TimerChange};
use verbatim_listener_logic::tell::{dropped_by_class, hang_tested, tell_window};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::protocol::ListenerToCore;
use verbatim_model::tell::{AcceptanceFacts, Checked, Event, EventObject, Payload, TellWindow};
use verbatim_model::{Pid, TraceId, WindowHandle};
use verbatim_process::clock::now_us;
use verbatim_process::held::Target;
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::Readability;
pub(super) use verbatim_win32::window_facts::SystemReads;
use verbatim_win32::window_facts::{WindowPart, WindowReads};

use super::{Sent, Waiting, foreground_report, sent};
use verbatim_process::transport::Sender;

/// What the listener reads of Windows' own state as it handles a
/// `WinEvent`, beside a window's facts: whether a window exists, whether
/// an existing window is hung, and the process of a thread. The event
/// path reads Windows' state only through it, so the event path's
/// call-count test (`a_flood_events_window_facts_are_read_once`) sees each
/// read.
pub(super) trait EventReads: WindowReads {
    /// Whether `window` is a window that exists (`IsWindow`).
    fn exists(&self, window: isize) -> bool;
    /// Whether `window`, a window found to exist, is ghosted or hung
    /// ([`verbatim_win32::window::hang_state`]).
    fn hang_state(&self, window: isize) -> Readability;
    /// The process `thread` belongs to, `None` when it cannot be read.
    fn process_of_thread(&self, thread: u32) -> Option<u32>;
    /// Whether this process can open the input desktop now
    /// ([`verbatim_win32::window::input_desktop_opens`]).
    fn input_desktop_opens(&self) -> bool;
}

impl EventReads for SystemReads {
    fn exists(&self, window: isize) -> bool {
        verbatim_win32::window::window_exists(window)
    }

    fn hang_state(&self, window: isize) -> Readability {
        verbatim_win32::window::hang_state(window)
    }

    fn process_of_thread(&self, thread: u32) -> Option<u32> {
        super::process_of_thread(thread)
    }

    fn input_desktop_opens(&self) -> bool {
        verbatim_win32::window::input_desktop_opens()
    }
}

/// A tell's key in the listener's merge: its object, kind and raising
/// thread, NVDA's limiter's key (`orderedWinEventLimiter.py` 63, 79).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TellKey {
    kind: EventKind,
    object: EventObject,
    thread: u32,
}

/// A tell before it is placed in the listener's order: everything but the
/// foreground's part of its window facts, which is read as it is placed,
/// unless its check thread read them where NVDA judges acceptance
/// ([`PendingWindow::Whole`]).
pub(super) struct Pending {
    pub(super) trace_id: TraceId,
    pub(super) kind: EventKind,
    pub(super) process: Pid,
    pub(super) application: String,
    pub(super) thread: u32,
    pub(super) at_us: u64,
    pub(super) object: EventObject,
    pub(super) cached: PropertyValues,
    pub(super) payload: Payload,
    pub(super) window: PendingWindow,
    pub(super) checked: Option<Checked>,
}

/// The window a tell is judged by, before it is placed: found, with the
/// part of its facts that depends on it alone or with all of them, or why
/// there is none.
pub(super) enum PendingWindow {
    /// Found, the foreground's part of its facts read as the tell is
    /// placed.
    Found(WindowPart),
    /// Found, with every acceptance fact read by its check thread where
    /// NVDA judges acceptance (`verbatim_listener_logic::check`).
    Whole(Box<AcceptanceFacts>),
    /// The check thread's walk found no window.
    NoWindow,
    /// The check thread's walk failed.
    Failed,
}

impl Pending {
    /// The tell, its window facts completed with the foreground's part
    /// read now, and its key.
    fn finish(self, reads: &impl WindowReads) -> (TellKey, Event) {
        let key = TellKey {
            kind: self.kind,
            object: self.object.clone(),
            thread: self.thread,
        };
        let window = match self.window {
            PendingWindow::Found(part) => TellWindow::Found(Box::new(part.with_foreground(reads))),
            PendingWindow::Whole(facts) => TellWindow::Found(facts),
            PendingWindow::NoWindow => TellWindow::NoWindow,
            PendingWindow::Failed => TellWindow::Failed,
        };
        let tell = Event {
            trace_id: self.trace_id,
            kind: self.kind,
            process: self.process,
            application: self.application,
            thread: self.thread,
            at_us: self.at_us,
            object: self.object,
            cached: self.cached,
            payload: self.payload,
            window,
            checked: self.checked,
        };
        (key, tell)
    }
}

/// What the listener's order of tells carries: a tell, a window's
/// destruction, or the idle barrier's answer, which waits behind every
/// tell placed before it.
pub(super) enum Held {
    /// A tell.
    Tell(Box<Pending>),
    /// A window's destruction, sent on the lane that keeps order and never
    /// merges, so that no tell placed before it moves behind it and no
    /// later one replaces it ([`ListenerToCore::Destroyed`]).
    Destroyed {
        /// The window.
        window: WindowHandle,
        /// The process of the thread that raised it, `None` when that
        /// thread cannot be opened.
        process: Option<Pid>,
        /// When the callback observed it.
        at_us: u64,
    },
    /// The idle barrier's answer for a round.
    IdleAnswer(u64),
}

/// The listener's order of tells and its pending foreground
/// (`verbatim_listener_logic::foreground::Pending`), under the one lock
/// every tell is placed under: the foreground facts of a tell whose check
/// did not read them are read, and the tell sent, in one step under it,
/// so no such tell is placed after a foreground its facts predate, and the
/// window a step finds in front is reported before the tells it releases.
/// A UI Automation focus, menu opening, element selected or window opened
/// has its facts read by its check where NVDA judges acceptance, before
/// the object's own calls, and may be placed after a foreground they
/// predate, as NVDA's event is (Dickson, 2026-11-19). Only the `WinEvent`
/// thread steps the pending foreground ([`step`](Self::step)), which owns
/// the hold's timer; the other threads place behind it. Its read of the
/// window in front, `GetForegroundWindow` in the listener, is the one
/// every step judges by ([`front`](Self::front)).
pub(super) struct Tells {
    pending: Mutex<Foreground<Held>>,
    sender: Sender<Waiting>,
    front: fn() -> isize,
}

impl Tells {
    pub(super) fn new(sender: Sender<Waiting>) -> Self {
        Self::with_front(sender, foreground)
    }

    /// [`new`](Self::new) reading the window in front with `front`, a
    /// test's stand-in for `GetForegroundWindow`.
    pub(super) fn with_front(sender: Sender<Waiting>, front: fn() -> isize) -> Self {
        Self {
            pending: Mutex::new(Foreground::Clear),
            sender,
            front,
        }
    }

    /// The window in front now, 0 for none.
    pub(super) fn front(&self) -> isize {
        (self.front)()
    }

    /// Places `held`, from any thread, the idle barrier's answer: sent
    /// now, or held behind a hold in force, which it does not release.
    pub(super) fn place_behind(&self, held: Held) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = pending.place_behind(held) {
            self.apply(None, vec![held], &SystemReads);
        }
    }

    /// Sends a UI Automation tell `held` at once, past any pending
    /// foreground, which keeps what it holds (the hold holds MSAA events
    /// only, Dickson, 2026-11-17).
    pub(super) fn pass_hold(&self, held: Held) {
        let _pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        self.apply(None, vec![held], &SystemReads);
    }

    /// A step of the pending foreground on the `WinEvent` thread, `seen`
    /// an event or the hold's timer, `front` the window in front as the
    /// step judges it, and `started` whether the foreground's watch has
    /// started: the window found in front reported, then what the step
    /// releases sent, each tell's foreground facts read through `reads`.
    /// Returns the window reported, for the watch, and the timer's change,
    /// which the `WinEvent` thread makes.
    pub(super) fn step(
        &self,
        seen: Seen<Held>,
        (front, started): (isize, bool),
        reads: &impl WindowReads,
    ) -> (Option<Front>, TimerChange) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let placed = pending.step(seen, front, now_us(), started);
        self.apply(placed.report, placed.send, reads);
        (placed.report, placed.timer)
    }

    /// Sends the report of `report`, then `send`, in order, each tell's
    /// foreground facts read through `reads` as it goes, under the lock.
    /// The foreground window, the active window and whether one window is
    /// inside another are read here, under the lock.
    fn apply(&self, report: Option<Front>, send: Vec<Held>, reads: &impl WindowReads) {
        if let Some(front) = report {
            sent(&self.sender.send(Sent::Report(foreground_report(front))));
        }
        for held in send {
            match held {
                Held::Tell(tell) => {
                    let (key, event) = tell.finish(reads);
                    let message = ListenerToCore::Tell(Box::new(event));
                    sent(&self.sender.send(Sent::Tell(key, message)));
                }
                Held::Destroyed {
                    window,
                    process,
                    at_us,
                } => sent(&self.sender.send(Sent::Report(ListenerToCore::Destroyed {
                    window,
                    process,
                    at_us,
                }))),
                Held::IdleAnswer(round) => sent(&self.sender.send(Sent::IdleAnswer(round))),
            }
        }
    }
}

/// The window `GetForegroundWindow` names now, 0 for none.
fn foreground() -> isize {
    verbatim_win32::window::foreground_window_handle()
}

/// The listener's record of the applications that raised events: each
/// process's executable name, read once per process generation with
/// `QueryFullProcessImageNameW`, a call to Windows and not into the
/// application. The process is held open while it is known, so its id
/// names it and no other until it is seen to have exited; one that cannot
/// be opened, a protected process such as csrss, is known by its id alone,
/// with no name.
#[derive(Default)]
pub(super) struct Applications {
    known: BTreeMap<Pid, (Option<Target>, String)>,
    /// How many were known at the last sweep of the exited.
    swept_at: usize,
}

impl Applications {
    /// The executable name of `process`, lower-cased with no folder or
    /// extension, empty when it cannot be read.
    pub(super) fn name(&mut self, process: Pid) -> String {
        if let Some((target, name)) = self.known.get(&process)
            && target.as_ref().is_none_or(|target| !target.has_exited())
        {
            return name.clone();
        }
        let target = Target::open(process).ok();
        let name = target
            .as_ref()
            .and_then(Target::image_stem)
            .unwrap_or_default();
        self.known.insert(process, (target, name.clone()));
        self.sweep();
        name
    }

    /// Lets go of every process seen to have exited, once the record has
    /// doubled since it last did, so a run of short-lived processes holds
    /// no handle for long.
    fn sweep(&mut self) {
        if self.known.len() <= self.swept_at.saturating_mul(2).saturating_add(16) {
            return;
        }
        self.known
            .retain(|_, (target, _)| target.as_ref().is_none_or(|target| !target.has_exited()));
        self.swept_at = self.known.len();
    }
}

/// `window` as the model names a window.
fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

/// What `event`, received at `at_us`, gives the pending foreground: its
/// tell, unless a local drop takes it, as a foreground event's, the
/// shell's or any other's ([`Seen`]); a foreground event's window is
/// awaited whether or not its tell is dropped after the window is known.
/// A window's destruction is told at once after the window's mapping,
/// with the process of the thread that raised it when that thread can be
/// opened and none when it cannot, so it is never dropped for its process:
/// NVDA handles a destroy before any drop that applies to a window's
/// destruction (`IAccessibleHandler/internalWinEventHandler.py` 101 to
/// 103). The local drops, in NVDA's order (design, "Readers' local
/// drops"): the window's mapping ([`tell_window`]), the process that
/// serves the window, which must be known (a console window's is its
/// console host, the process of the thread that raised the event, as
/// Verbatim routes it today), the window's class and the shown object
/// ([`dropped_by_class`]), and a hung or ghosted window, read live, for
/// the kinds NVDA tests ([`hang_tested`]). `class` is the class of the
/// event's window when the hook's filter read it; every other fact is
/// read through `reads`.
pub(super) fn tell_win_event(
    applications: &mut Applications,
    event: WinEvent,
    (at_us, class): (u64, Option<&str>),
    reads: &impl EventReads,
) -> Seen<Held> {
    let other = |item| Seen::Other {
        item,
        observed_at_us: at_us,
    };
    let Some(kind) = verbatim_ia2::tell_kind(&event) else {
        return other(None);
    };
    let window = match tell_window(
        kind,
        (event.hwnd, event.id_object, event.id_child),
        |window| reads.exists(window),
        reads.desktop(),
    ) {
        Ok(window) => window,
        Err(reason) => {
            tracing::trace!(?kind, hwnd = event.hwnd, reason, "a WinEvent is not told");
            return other(None);
        }
    };
    if kind == EventKind::Destroy {
        return other(Some(Held::Destroyed {
            window: handle(window),
            process: reads.process_of_thread(event.thread).map(Pid),
            at_us,
        }));
    }
    let class = match class {
        Some(class) if event.hwnd == window => class.to_owned(),
        _ => reads.class(window),
    };
    let console = class == CONSOLE_WINDOW_CLASS;
    let seen = |item| {
        let front = Front {
            window,
            thread: event.thread,
            console,
            observed_at_us: at_us,
        };
        match event.kind {
            WinEventKind::Foreground => Seen::Foreground { front, item },
            WinEventKind::ShellForeground => Seen::Shell {
                front: Front {
                    console: false,
                    ..front
                },
                item,
            },
            _ => other(item),
        }
    };
    let dropped = |reason: &'static str| {
        tracing::trace!(?kind, window, reason, "a WinEvent is not told");
        seen(None)
    };
    let owner = reads.owner(window).1;
    // A console window's events are served by the process of the thread
    // that raised them.
    let process = if console || owner == 0 {
        reads.process_of_thread(event.thread)
    } else {
        Some(owner)
    };
    let Some(process) = process.map(Pid) else {
        return dropped("the process serving the window cannot be read");
    };
    if let Some(reason) = dropped_by_class(kind, &class, event.id_object) {
        return dropped(reason);
    }
    if hang_tested(kind) && reads.hang_state(window).is_hung() {
        return dropped("its window is hung");
    }
    let payload = if kind == EventKind::DesktopSwitch {
        Payload::DesktopSwitch {
            input_desktop_open: reads.input_desktop_opens(),
        }
    } else {
        Payload::None
    };
    let tell = Pending {
        trace_id: TraceId::mint(),
        kind,
        application: applications.name(process),
        process,
        thread: event.thread,
        at_us,
        object: EventObject::Msaa {
            window: handle(window),
            object: event.id_object,
            child: event.id_child,
        },
        cached: PropertyValues(Vec::new()),
        payload,
        window: PendingWindow::Found(WindowPart::read(
            reads,
            window,
            console.then_some((event.thread, process.0)),
        )),
        checked: None,
    };
    seen(Some(Held::Tell(Box::new(tell))))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::BufReader;

    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, EVENT_OBJECT_DESTROY, EVENT_OBJECT_NAMECHANGE,
        EVENT_SYSTEM_FOREGROUND, HWND_MESSAGE, OBJID_CLIENT, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    use verbatim_process::message::read_message;
    use verbatim_process::overlapped::{End, Flow, pipe};
    use verbatim_process::transport::{Backlog, EVENT_PIPE_BUFFER};

    use super::*;

    /// A message-only window of this thread's, of class `Static`, which no
    /// desktop shows and which is never in front: destroyed when dropped.
    struct OwnWindow(isize);

    impl OwnWindow {
        fn new() -> Self {
            // SAFETY: a predefined class, the message-only parent, no menu
            // or creation data; destroyed on this thread.
            let window = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("verbatim-listener test window"),
                    WS_OVERLAPPED,
                    0,
                    0,
                    10,
                    10,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .expect("a window");
            Self(window.0 as isize)
        }
    }

    impl Drop for OwnWindow {
        fn drop(&mut self) {
            // SAFETY: made on this thread, destroyed once.
            let _ = unsafe { DestroyWindow(windows::Win32::Foundation::HWND(self.0 as _)) };
        }
    }

    /// The order of tells, writing to a pipe whose other end the test
    /// reads.
    fn tells() -> (Tells, Sender<Waiting>, BufReader<File>) {
        let (ours, theirs) =
            pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Synchronous).expect("a pipe");
        let (sender, _finishing) =
            Sender::start(ours, Waiting::default(), "tells-test", || {}).expect("a sender");
        (
            Tells::new(sender.clone()),
            sender,
            BufReader::new(File::from(theirs)),
        )
    }

    /// Everything sent, once the sender is closed.
    fn sent_all(sender: &Sender<Waiting>, reader: &mut BufReader<File>) -> Vec<ListenerToCore> {
        sender.close();
        std::iter::from_fn(|| read_message(reader).expect("read")).collect()
    }

    fn this_thread() -> u32 {
        // SAFETY: no preconditions.
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
    }

    fn event(kind: WinEventKind, raw: u32, hwnd: isize, id_object: i32) -> WinEvent {
        WinEvent {
            kind,
            event: raw,
            hwnd,
            id_object,
            id_child: 0,
            thread: this_thread(),
            time: 0,
            raised_ms_ago: 0,
        }
    }

    /// [`tell_win_event`] with the system's reads, the hook having read
    /// no class, stepped with the window in front `at.1`, before the
    /// foreground's watch starts, so that no window is reported.
    fn tell(tells: &Tells, applications: &mut Applications, event: WinEvent, at: (u64, isize)) {
        let seen = tell_win_event(applications, event, (at.0, None), &SystemReads);
        let _ = tells.step(seen, (at.1, false), &SystemReads);
    }

    /// A foreground event for the window in front, or for none when
    /// nothing is, its tell dropped: it ends the hold.
    fn foreground_in_front(tells: &Tells) {
        let front = Front {
            window: foreground(),
            thread: 0,
            console: false,
            observed_at_us: 9,
        };
        let seen = Seen::Foreground { front, item: None };
        let _ = tells.step(seen, (foreground(), false), &SystemReads);
    }

    /// This test process's executable name, as the listener reads it.
    fn own_name() -> String {
        std::env::current_exe()
            .expect("the test's executable")
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a name")
            .to_lowercase()
    }

    /// The one tell `sent` holds.
    fn only_tell(sent: &[ListenerToCore]) -> &Event {
        let [ListenerToCore::Tell(tell)] = sent else {
            panic!("one tell: {sent:?}");
        };
        tell
    }

    #[test]
    fn a_name_change_is_told_with_what_came_with_it_and_its_windows_facts() {
        let window = OwnWindow::new();
        let (tells, sender, mut reader) = tells();
        let name_change = event(
            WinEventKind::NameChange,
            EVENT_OBJECT_NAMECHANGE,
            window.0,
            OBJID_CLIENT.0,
        );
        tell(
            &tells,
            &mut Applications::default(),
            name_change,
            (5_000, foreground()),
        );
        let sent = sent_all(&sender, &mut reader);
        let tell = only_tell(&sent);
        assert_eq!(tell.kind, EventKind::NameChange);
        assert_eq!(tell.process, Pid(std::process::id()));
        assert_eq!(tell.application, own_name());
        assert_eq!(tell.thread, this_thread());
        assert_eq!(tell.at_us, 5_000);
        assert_eq!(
            tell.object,
            EventObject::Msaa {
                window: handle(window.0),
                object: OBJID_CLIENT.0,
                child: 0,
            }
        );
        assert_eq!(tell.payload, Payload::None);
        assert_eq!(tell.checked, None);
        let TellWindow::Found(facts) = &tell.window else {
            panic!("its window's facts: {:?}", tell.window);
        };
        assert_eq!(facts.window.handle, handle(window.0));
        assert_eq!(facts.window.class, "Static");
        assert_eq!(facts.window.process, Pid(std::process::id()));
        assert_eq!(facts.window.thread, this_thread());
        assert!(!facts.desktop);
        assert!(!facts.hidden_frame);
    }

    #[test]
    fn an_invalid_window_and_a_destroyed_object_are_not_told() {
        let window = OwnWindow::new();
        let gone = OwnWindow::new().0;
        let (tells, sender, mut reader) = tells();
        let mut applications = Applications::default();
        for told in [
            event(
                WinEventKind::NameChange,
                EVENT_OBJECT_NAMECHANGE,
                gone,
                OBJID_CLIENT.0,
            ),
            event(
                WinEventKind::Destroy,
                EVENT_OBJECT_DESTROY,
                window.0,
                OBJID_CLIENT.0,
            ),
        ] {
            tell(&tells, &mut applications, told, (1, foreground()));
        }
        assert_eq!(sent_all(&sender, &mut reader), Vec::<ListenerToCore>::new());
    }

    #[test]
    fn a_windows_destruction_is_sent_with_the_process_of_its_thread() {
        let gone = OwnWindow::new().0;
        let (tells, sender, mut reader) = tells();
        let destroy = event(WinEventKind::Destroy, EVENT_OBJECT_DESTROY, gone, 0);
        tell(
            &tells,
            &mut Applications::default(),
            destroy,
            (1, foreground()),
        );
        assert_eq!(
            sent_all(&sender, &mut reader),
            [ListenerToCore::Destroyed {
                window: handle(gone),
                process: Some(Pid(std::process::id())),
                at_us: 1,
            }]
        );
    }

    /// A destroy raised on a thread the listener cannot open, as the
    /// desktop window manager's thread that destroys a ghost, or a thread
    /// that has exited, is sent with no process, never dropped. Thread 0
    /// is one no `OpenThread` opens.
    #[test]
    fn a_destruction_on_a_thread_that_cannot_be_opened_is_sent_with_no_process() {
        let gone = OwnWindow::new().0;
        let (tells, sender, mut reader) = tells();
        let destroy = WinEvent {
            thread: 0,
            ..event(WinEventKind::Destroy, EVENT_OBJECT_DESTROY, gone, 0)
        };
        tell(
            &tells,
            &mut Applications::default(),
            destroy,
            (2, foreground()),
        );
        assert_eq!(
            sent_all(&sender, &mut reader),
            [ListenerToCore::Destroyed {
                window: handle(gone),
                process: None,
                at_us: 2,
            }]
        );
    }

    /// A destroy placed while a foreground hold keeps tells leaves after
    /// the tells held before it and ahead of those placed after it.
    #[test]
    fn a_destruction_keeps_its_place_among_the_held_tells() {
        let window = OwnWindow::new();
        let other = OwnWindow::new();
        let gone = OwnWindow::new().0;
        let (tells, sender, mut reader) = tells();
        let mut applications = Applications::default();
        let foreground_event = event(
            WinEventKind::Foreground,
            EVENT_SYSTEM_FOREGROUND,
            window.0,
            OBJID_CLIENT.0,
        );
        tell(
            &tells,
            &mut applications,
            foreground_event,
            (1, foreground()),
        );
        let name_change = event(
            WinEventKind::NameChange,
            EVENT_OBJECT_NAMECHANGE,
            window.0,
            OBJID_CLIENT.0,
        );
        tell(&tells, &mut applications, name_change, (2, foreground()));
        let destroy = event(WinEventKind::Destroy, EVENT_OBJECT_DESTROY, gone, 0);
        tell(&tells, &mut applications, destroy, (3, foreground()));
        let other_change = event(
            WinEventKind::NameChange,
            EVENT_OBJECT_NAMECHANGE,
            other.0,
            OBJID_CLIENT.0,
        );
        tell(&tells, &mut applications, other_change, (4, foreground()));
        foreground_in_front(&tells);
        let order: Vec<String> = sent_all(&sender, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Tell(tell) => format!("{:?} {}", tell.kind, tell.at_us),
                ListenerToCore::Destroyed { at_us, .. } => format!("Destroyed {at_us}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            [
                "Foreground 1",
                "NameChange 2",
                "Destroyed 3",
                "NameChange 4"
            ]
        );
    }

    /// A foreground event whose window is not in front holds every tell
    /// behind it, the idle barrier's answer included, until a foreground
    /// event whose window is in front sends them all, in order.
    #[test]
    fn tells_and_the_idle_answer_wait_behind_a_foreground_not_yet_in_front() {
        let window = OwnWindow::new();
        let (tells, sender, mut reader) = tells();
        let mut applications = Applications::default();
        let foreground_event = event(
            WinEventKind::Foreground,
            EVENT_SYSTEM_FOREGROUND,
            window.0,
            OBJID_CLIENT.0,
        );
        tell(
            &tells,
            &mut applications,
            foreground_event,
            (1, foreground()),
        );
        let name_change = event(
            WinEventKind::NameChange,
            EVENT_OBJECT_NAMECHANGE,
            window.0,
            OBJID_CLIENT.0,
        );
        tell(&tells, &mut applications, name_change, (2, foreground()));
        tells.place_behind(Held::IdleAnswer(7));
        foreground_in_front(&tells);
        let order: Vec<String> = sent_all(&sender, &mut reader)
            .into_iter()
            .map(|message| match message {
                ListenerToCore::Tell(tell) => format!("{:?} {}", tell.kind, tell.at_us),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            ["Foreground 1", "NameChange 2", "IdleAnswer { round: 7 }"]
        );
    }

    /// The recorded pointer change, a name change on the desktop window of
    /// object `OBJID_CURSOR` and child 0, is not told, and the desktop's
    /// own name change after it, a virtual desktop's, is.
    #[test]
    fn a_pointer_shape_change_is_not_told_and_the_desktops_name_change_after_it_is() {
        // SAFETY: GetDesktopWindow has no preconditions.
        let desktop =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize;
        let (tells, sender, mut reader) = tells();
        let mut applications = Applications::default();
        for (told, at_us) in [
            (
                event(
                    WinEventKind::NameChange,
                    EVENT_OBJECT_NAMECHANGE,
                    desktop,
                    verbatim_listener_logic::tell::OBJID_CURSOR,
                ),
                1,
            ),
            (
                event(
                    WinEventKind::NameChange,
                    EVENT_OBJECT_NAMECHANGE,
                    desktop,
                    OBJID_CLIENT.0,
                ),
                2,
            ),
        ] {
            tell(&tells, &mut applications, told, (at_us, foreground()));
        }
        let sent = sent_all(&sender, &mut reader);
        let tell = only_tell(&sent);
        assert_eq!(tell.kind, EventKind::NameChange);
        assert_eq!(tell.at_us, 2);
        assert_eq!(
            tell.object,
            EventObject::Msaa {
                window: handle(desktop),
                object: OBJID_CLIENT.0,
                child: 0,
            }
        );
        let TellWindow::Found(facts) = &tell.window else {
            panic!("its window's facts: {:?}", tell.window);
        };
        assert_eq!(facts.window.class, "#32769");
        assert!(facts.desktop);
    }

    /// Waiting tells merge, one per object, kind and raising thread, the
    /// newest at the back.
    #[test]
    fn waiting_tells_merge_by_object_kind_and_thread() {
        let key = |window: u64, thread: u32| TellKey {
            kind: EventKind::NameChange,
            object: EventObject::Msaa {
                window: WindowHandle(window),
                object: -4,
                child: 0,
            },
            thread,
        };
        let mut waiting = Waiting::default();
        for (window, thread, round) in [(1, 9, 1), (2, 9, 2), (1, 8, 3), (1, 9, 4)] {
            let _ = waiting.push(Sent::Tell(
                key(window, thread),
                ListenerToCore::Fault {
                    detail: round.to_string(),
                },
            ));
        }
        let order: Vec<String> = std::iter::from_fn(|| waiting.pop())
            .map(|sent| match sent {
                Sent::Message(ListenerToCore::Fault { detail }) => detail,
                _ => "another".to_owned(),
            })
            .collect();
        assert_eq!(order, ["2", "3", "4"]);
    }

    /// A window's destruction waits on the lane that keeps order and never
    /// merges: two destroys of one window both stay, in the order they
    /// were sent, among the tells.
    #[test]
    fn destructions_wait_in_order_and_are_never_merged() {
        let destroyed = |at_us: u64| ListenerToCore::Destroyed {
            window: WindowHandle(5),
            process: None,
            at_us,
        };
        let key = TellKey {
            kind: EventKind::NameChange,
            object: EventObject::Msaa {
                window: WindowHandle(5),
                object: -4,
                child: 0,
            },
            thread: 9,
        };
        let mut waiting = Waiting::default();
        let _ = waiting.push(Sent::Report(destroyed(1)));
        let _ = waiting.push(Sent::Tell(
            key,
            ListenerToCore::Fault {
                detail: "2".to_owned(),
            },
        ));
        let _ = waiting.push(Sent::Report(destroyed(3)));
        let order: Vec<String> = std::iter::from_fn(|| waiting.pop())
            .map(|sent| match sent {
                Sent::Message(ListenerToCore::Destroyed { at_us, .. }) => {
                    format!("Destroyed {at_us}")
                }
                Sent::Message(ListenerToCore::Fault { detail }) => detail,
                _ => "another".to_owned(),
            })
            .collect();
        assert_eq!(order, ["Destroyed 1", "2", "Destroyed 3"]);
    }
}
