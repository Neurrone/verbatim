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
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use verbatim_ia2::{WinEvent, WinEventKind};
use verbatim_listener_logic::tell::{
    ForegroundHold, Placed, TimerChange, dropped_by_class, tell_window,
};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::protocol::ListenerToCore;
use verbatim_model::tell::{Checked, Event, EventObject, Payload, TellWindow};
use verbatim_model::{Pid, TraceId, WindowHandle};
use verbatim_process::clock::now_us;
use verbatim_process::held::Target;
use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;
use verbatim_win32::window::window_class_name;
pub(super) use verbatim_win32::window_facts::SystemReads;
use verbatim_win32::window_facts::{WindowPart, WindowReads};

use super::{Sent, Timed, TimerHandle, Waiting, process_of_thread, sent, window_pid};
use verbatim_process::transport::Sender;

/// A tell's key in the listener's merge: its object, kind and raising
/// thread, NVDA's limiter's key (`orderedWinEventLimiter.py` 63, 79).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TellKey {
    kind: EventKind,
    object: EventObject,
    thread: u32,
}

/// A tell before it is placed in the listener's order: everything but the
/// foreground's part of its window facts, which is read as it is placed.
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
/// part of its facts that depends on it alone, or why there is none.
pub(super) enum PendingWindow {
    /// Found.
    Found(WindowPart),
    /// The check thread's walk found no window.
    NoWindow,
    /// The check thread's walk failed.
    Failed,
}

impl Pending {
    /// The tell, its window facts completed with the foreground's part
    /// read now, and its key.
    fn finish(self, reads: &impl WindowReads) -> (TellKey, ListenerToCore) {
        let key = TellKey {
            kind: self.kind,
            object: self.object.clone(),
            thread: self.thread,
        };
        let window = match self.window {
            PendingWindow::Found(part) => TellWindow::Found(Box::new(part.with_foreground(reads))),
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
        (key, ListenerToCore::Tell(Box::new(tell)))
    }
}

/// What the listener's order of tells carries: a tell, or the idle
/// barrier's answer, which waits behind every tell placed before it.
pub(super) enum Held {
    /// A tell.
    Tell(Box<Pending>),
    /// The idle barrier's answer for a round.
    IdleAnswer(u64),
}

/// What sets the hold's timer, or stops it (`None`).
type SetHoldTimer = Box<dyn Fn(Option<Duration>) + Send + Sync>;

/// The listener's order of tells and its foreground hold
/// (`verbatim_listener_logic::tell::ForegroundHold`), under the one lock
/// every tell is placed under: the foreground facts of a tell are read,
/// and the tell sent, in one step under it, so no tell is placed after a
/// foreground its facts predate. One timer, armed only while the hold
/// keeps tells.
pub(super) struct Tells {
    state: Mutex<HoldState>,
    sender: Sender<Waiting>,
    timer: OnceLock<SetHoldTimer>,
}

/// The hold, and whether its timer is being closed.
struct HoldState {
    hold: ForegroundHold<Held>,
    closed: bool,
}

impl Tells {
    pub(super) fn new(sender: Sender<Waiting>) -> Self {
        Self {
            state: Mutex::new(HoldState {
                hold: ForegroundHold::default(),
                closed: false,
            }),
            sender,
            timer: OnceLock::new(),
        }
    }

    /// Places `held` in the order of tells: sent now, or kept behind a
    /// pending foreground.
    pub(super) fn place(&self, held: Held) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let placed = state.hold.place(held, foreground(), now_us());
        self.apply(&state, placed);
    }

    /// Places a foreground event's tell for `window`, `None` for one a
    /// local drop took, which still awaits its window.
    pub(super) fn foreground(&self, window: isize, tell: Option<Pending>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let held = tell.map(|tell| Held::Tell(Box::new(tell)));
        let placed = state.hold.foreground(window, held, foreground(), now_us());
        self.apply(&state, placed);
    }

    /// Sends what a step of the hold released, in order, each tell's
    /// foreground facts read as it goes, and sets the timer as it says,
    /// under the lock.
    fn apply(&self, state: &HoldState, placed: Placed<Held>) {
        for held in placed.send {
            match held {
                Held::Tell(tell) => {
                    let (key, message) = tell.finish(&SystemReads);
                    sent(&self.sender.send(Sent::Tell(key, message)));
                }
                Held::IdleAnswer(round) => sent(&self.sender.send(Sent::IdleAnswer(round))),
            }
        }
        let after = match placed.timer {
            TimerChange::Keep => return,
            TimerChange::Arm(after) if !state.closed => Some(Duration::from_micros(after)),
            TimerChange::Arm(_) | TimerChange::Disarm => None,
        };
        if let Some(timer) = self.timer.get() {
            timer(after);
        }
    }
}

impl Timed for Tells {
    fn take_timer(&self, timer: TimerHandle) {
        let _ = self.timer.set(Box::new(move |after| match after {
            Some(after) => timer.set(after),
            None => timer.stop(),
        }));
    }

    fn fired(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let placed = state.hold.fired(foreground(), now_us());
        self.apply(&state, placed);
    }

    fn closing(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
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

/// Tells Core of `event`, received at `at_us`, unless a local drop takes
/// it, placing its tell in the listener's order through `tells`; a
/// foreground event's window is awaited by the foreground hold whether or
/// not its tell is dropped after the window is known. The local drops, in
/// NVDA's order (design, "Readers' local drops"): the window's mapping
/// ([`tell_window`]), the process that serves the window, which must be
/// known (a console window's is its console host, the process of the
/// thread that raised the event, as Verbatim routes it today), the
/// window's class and the shown object ([`dropped_by_class`]), and a hung
/// or ghosted window, read live.
pub(super) fn tell_win_event(
    tells: &Tells,
    applications: &mut Applications,
    event: WinEvent,
    at_us: u64,
) {
    let Some(kind) = verbatim_ia2::tell_kind(&event) else {
        return;
    };
    let reads = SystemReads;
    let window = match tell_window(
        kind,
        (event.hwnd, event.id_object, event.id_child),
        verbatim_win32::window::window_exists,
        reads.desktop(),
    ) {
        Ok(window) => window,
        Err(reason) => {
            tracing::trace!(?kind, hwnd = event.hwnd, reason, "a WinEvent is not told");
            return;
        }
    };
    let awaited = (event.kind == WinEventKind::Foreground).then_some(window);
    let dropped = |reason: &'static str| {
        tracing::trace!(?kind, window, reason, "a WinEvent is not told");
        if let Some(window) = awaited {
            tells.foreground(window, None);
        }
    };
    let class = window_class_name(window);
    let console = class == CONSOLE_WINDOW_CLASS;
    let owner = window_pid(window);
    // A console window's events, and a destroyed window's, are served by
    // the process of the thread that raised them.
    let process = if console || kind == EventKind::Destroy || owner == 0 {
        process_of_thread(event.thread)
    } else {
        Some(owner)
    };
    let Some(process) = process.map(Pid) else {
        return dropped("the process serving the window cannot be read");
    };
    if let Some(reason) = dropped_by_class(kind, &class, event.id_object) {
        return dropped(reason);
    }
    if kind != EventKind::Destroy && verbatim_win32::window::readability(window).is_hung() {
        return dropped("its window is hung");
    }
    let payload = if kind == EventKind::DesktopSwitch {
        Payload::DesktopSwitch {
            input_desktop_open: verbatim_win32::window::input_desktop_opens(),
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
            &reads,
            window,
            console.then_some((event.thread, process.0)),
        )),
        checked: None,
    };
    match awaited {
        Some(window) => tells.foreground(window, Some(tell)),
        None => tells.place(Held::Tell(Box::new(tell))),
    }
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
        tell_win_event(&tells, &mut Applications::default(), name_change, 5_000);
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
            tell_win_event(&tells, &mut applications, told, 1);
        }
        assert_eq!(sent_all(&sender, &mut reader), Vec::<ListenerToCore>::new());
    }

    #[test]
    fn a_windows_destruction_is_told_for_the_process_of_its_thread() {
        let gone = OwnWindow::new().0;
        let (tells, sender, mut reader) = tells();
        let destroy = event(WinEventKind::Destroy, EVENT_OBJECT_DESTROY, gone, 0);
        tell_win_event(&tells, &mut Applications::default(), destroy, 1);
        let sent = sent_all(&sender, &mut reader);
        let tell = only_tell(&sent);
        assert_eq!(tell.kind, EventKind::Destroy);
        assert_eq!(tell.process, Pid(std::process::id()));
        assert_eq!(
            tell.object,
            EventObject::Msaa {
                window: handle(gone),
                object: 0,
                child: 0,
            }
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
        tell_win_event(&tells, &mut applications, foreground_event, 1);
        let name_change = event(
            WinEventKind::NameChange,
            EVENT_OBJECT_NAMECHANGE,
            window.0,
            OBJID_CLIENT.0,
        );
        tell_win_event(&tells, &mut applications, name_change, 2);
        tells.place(Held::IdleAnswer(7));
        // A foreground event for the window in front, or for none when
        // nothing is, ends the hold; dropped itself, it is not told.
        tells.foreground(foreground(), None);
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
}
