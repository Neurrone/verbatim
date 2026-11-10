//! The recording itself: start-up in the order that loses no window, the
//! window watcher, main's wait, and the order of ending.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use verbatim_process::report_error;
use verbatim_uia::{Registration, Scope, UIA_CONNECTION_TIMEOUT, Uia, WindowLookup};
use verbatim_win32::window::{
    rect, related, top_level_windows, window_class_name, window_exists, window_is_visible,
};
use windows::Win32::Foundation::{HANDLE, HWND, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_CONSOLE_CARET, EVENT_CONSOLE_LAYOUT, EVENT_CONSOLE_UPDATE_REGION,
    EVENT_CONSOLE_UPDATE_SCROLL, EVENT_CONSOLE_UPDATE_SIMPLE, EVENT_MAX, EVENT_MIN,
    EVENT_OBJECT_CREATE, EVENT_OBJECT_DESCRIPTIONCHANGE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_FOCUS,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_REORDER,
    EVENT_OBJECT_SELECTION, EVENT_OBJECT_SELECTIONADD, EVENT_OBJECT_SELECTIONREMOVE,
    EVENT_OBJECT_SELECTIONWITHIN, EVENT_OBJECT_SHOW, EVENT_OBJECT_STATECHANGE,
    EVENT_OBJECT_TEXTSELECTIONCHANGED, EVENT_OBJECT_UNCLOAKED, EVENT_OBJECT_VALUECHANGE,
    EVENT_SYSTEM_ALERT, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MENUEND, EVENT_SYSTEM_MENUPOPUPEND,
    EVENT_SYSTEM_MENUPOPUPSTART, EVENT_SYSTEM_SWITCHEND, GW_OWNER, MSG, MsgWaitForMultipleObjects,
    OBJID_WINDOW, PM_REMOVE, PeekMessageW, QS_HOTKEY, WM_HOTKEY,
};
use windows::core::{AgileReference, BOOL};

use crate::hooks::{RawEvent, Sink, spawn_hook_thread};
use crate::line::{EventData, header};
use crate::steps::{self, Agent, Step, StepsThread};
use crate::stop_key::{self, STOP_KEY_ID};
use crate::target::{Selector, Target, open_process, raw, resolve, window_in_target};
use crate::uia_events::{UiaEvents, desktop_parts, target_parts};
use crate::writer::{Arrivals, EarlyEnd, EventArrival, Message, Reason, Stamped, serve};

/// What the command line asks of a recording.
pub(crate) struct Options {
    /// The target.
    pub target: Selector,
    /// The time limit, in seconds.
    pub seconds: Option<u64>,
    /// The stop key, as the command line spelled it.
    pub stop_key: String,
    /// The file to write, or the default name.
    pub output: Option<PathBuf>,
    /// The steps file.
    pub steps: Option<PathBuf>,
    /// The agent's host and port, with `steps`.
    pub agent: String,
}

/// `idChild` of a window object.
const CHILDID_SELF: i32 = 0;

/// What the watcher is sent.
enum WindowChange {
    /// A top-level window of the target's process was shown or uncloaked.
    Shown(isize),
    /// A window of the target's process was destroyed.
    Gone(isize),
}

/// The early-end value the console control handler reaches: the handler
/// has no way to carry context of its own.
static EARLY: OnceLock<Arc<EarlyEnd>> = OnceLock::new();

/// The console control handler: ends the recording early, then waits for
/// main to end the process, so that no event reaches the DLL detach fault.
unsafe extern "system" fn on_console_control(_event: u32) -> BOOL {
    if let Some(early) = EARLY.get() {
        early.end(Reason::ConsoleControl);
    }
    loop {
        thread::park();
    }
}

/// Runs a recording and returns its exit code.
pub(crate) fn run(options: Options) -> u32 {
    match start(options) {
        Ok(recording) => recording.wait_and_end(),
        Err(message) => {
            report_error!("{message}");
            1
        }
    }
}

/// A recording under way.
struct Recording {
    arrivals: Arc<Arrivals>,
    early: Arc<EarlyEnd>,
    writer: JoinHandle<Result<(), String>>,
    target_process: OwnedHandle,
    deadline: Option<Instant>,
    steps: Option<StepsThread>,
    path: PathBuf,
    /// A failure of the start-up after the file was begun, or `None`.
    failed: Option<String>,
}

/// Refuses what is wrong before anything is written, then starts recording
/// in the order that loses no window: the hook, then the listing, then the
/// registrations, then the steps.
#[allow(clippy::too_many_lines, reason = "the recording's start-up in order")]
fn start(options: Options) -> Result<Recording, String> {
    let stop_key = stop_key::parse(&options.stop_key)?;
    let target = resolve(&options.target)?;
    let target_process = open_process(target.pid())?;
    let parsed_steps = options
        .steps
        .as_ref()
        .map(|path| -> Result<Vec<Step>, String> {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("--steps {}: {error}", path.display()))?;
            steps::parse(&text)
        })
        .transpose()?;

    // Before UI Automation is first used, so that a console control event
    // during any later step, such as the agent connection, ends the
    // recording and never reaches the detach fault.
    let early = Arc::new(EarlyEnd::new()?);
    let _ = EARLY.set(Arc::clone(&early));
    // SAFETY: registers a handler for this process's console events; the
    // handler has the required signature and lives for the process.
    unsafe { SetConsoleCtrlHandler(Some(on_console_control), true) }
        .map_err(|error| format!("could not set the console handler: {error}"))?;

    let uia = Uia::new().map_err(|error| format!("could not start UI Automation: {error}"))?;
    let request = uia
        .request_fixed::<WindowLookup>()
        .map_err(|error| format!("could not make a cache request: {error}"))?;
    // A `--window` target that cannot be found is refused before anything
    // is written. It comes first, so that the agent connection brings
    // forward only a window that can be recorded.
    let mut windows: HashMap<isize, AgileReference<IUIAutomationElement>> = HashMap::new();
    if let Target::Window { hwnd, .. } = target {
        let element = find_window(&uia, &request, hwnd)
            .ok_or_else(|| format!("window {hwnd:#X} was not found under the desktop's root"))?;
        windows.insert(hwnd, agile(&element)?);
    }

    let mut agent = None;
    if parsed_steps.is_some() {
        let mut connected = Agent::connect(&options.agent)?;
        // Brought forward before any registration starts, so that its
        // minimize and restore are not recorded (Dickson, 2026-11-10); for
        // `--process` the person running it does this.
        if let Target::Window { hwnd, .. } = target {
            connected.bring_forward(hwnd)?;
        }
        agent = Some(connected);
    }

    stop_key::register(stop_key, &options.stop_key)?;

    let path = options.output.unwrap_or_else(default_path);
    let file = File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;

    let (sender, received): (Sender<Stamped>, Receiver<Stamped>) = mpsc::channel();
    let arrivals = Arc::new(Arrivals::new(sender));
    let header = header(&started_utc(), &windows_build(), &target.header_field());
    let writer = {
        let early = Arc::clone(&early);
        thread::Builder::new()
            .name("verbatim-event-record-writer".to_owned())
            .spawn(move || serve(BufWriter::new(file), &header, &received, &early))
            .map_err(|error| format!("could not start the writer: {error}"))?
    };
    let deadline = options
        .seconds
        .map(|seconds| arrivals.started() + Duration::from_secs(seconds));

    let mut recording = Recording {
        arrivals: Arc::clone(&arrivals),
        early: Arc::clone(&early),
        writer,
        target_process,
        deadline,
        steps: None,
        path,
        failed: None,
    };

    let (changes, changes_received) = mpsc::channel::<WindowChange>();
    if let Err(message) = start_hooks(target, &arrivals, &early, changes) {
        recording.failed = Some(message);
        return Ok(recording);
    }
    // A `--window` target destroyed before its hook was in has no event to
    // end the recording.
    if let Target::Window { hwnd, .. } = target
        && !window_exists(hwnd)
    {
        early.end(Reason::WindowDestroyed);
        return Ok(recording);
    }

    if let Target::Process(pid) = target {
        for window in top_level_windows(pid) {
            if !window_is_visible(window) {
                continue;
            }
            match find_window(&uia, &request, window).as_ref().map(agile) {
                Some(Ok(element)) => {
                    windows.insert(window, element);
                }
                Some(Err(message)) => report_error!("{message}"),
                None => report_not_found(window),
            }
        }
    }

    let events = UiaEvents::new(Arc::clone(&arrivals), target);
    let registration: &'static Registration = match register(&events, &windows, &arrivals) {
        Ok(registration) => registration,
        Err(message) => {
            recording.failed = Some(message);
            return Ok(recording);
        }
    };
    if let Target::Process(pid) = target {
        let arrivals = Arc::clone(&arrivals);
        let spawned = thread::Builder::new()
            .name("verbatim-event-record-watcher".to_owned())
            .spawn(move || watch(pid, windows, registration, &arrivals, &changes_received));
        if let Err(error) = spawned {
            recording.failed = Some(format!("could not start the window watcher: {error}"));
            return Ok(recording);
        }
    }
    settle(registration);

    // A console control event during start-up has already ended the
    // recording, so no key is sent for it.
    if let (Some(agent), Some(steps), None) = (agent, parsed_steps, early.reason()) {
        match steps::spawn(agent, steps, Arc::clone(&early)) {
            Ok(thread) => recording.steps = Some(thread),
            Err(message) => recording.failed = Some(message),
        }
    }
    Ok(recording)
}

/// The element for `window`, found from the root, or `None` when it is
/// not there.
fn find_window(
    uia: &Uia,
    request: &verbatim_uia::Request<WindowLookup>,
    window: isize,
) -> Option<IUIAutomationElement> {
    match uia.window_element_from_root(window, request) {
        Ok(found) => found.map(|element| element.live().element().clone()),
        Err(error) => {
            report_error!("window {window:#X} could not be looked up: {error}");
            None
        }
    }
}

/// Says on standard error which window of the target was not found, with
/// what explains it: its class, owner, rectangle and whether it is cloaked.
fn report_not_found(window: isize) {
    let owner = related(window, GW_OWNER);
    let rectangle = rect(window).map_or_else(
        || "none".to_owned(),
        |r| format!("{},{},{},{}", r.left, r.top, r.right, r.bottom),
    );
    report_error!(
        "window {window:#X} of the target was not found: class {}, owner {owner:#X}, rectangle {rectangle}, cloaked {}",
        window_class_name(window),
        is_cloaked(window),
    );
}

/// Whether the desktop's compositor hides `window` (`DWMWA_CLOAKED`), as
/// a suspended Store application's window is. A local read.
fn is_cloaked(window: isize) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: the out-parameter is a u32, the size DWMWA_CLOAKED writes;
    // any handle is tolerated, failing.
    let read = unsafe {
        DwmGetWindowAttribute(
            HWND(window as *mut _),
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            4,
        )
    };
    read.is_ok() && cloaked != 0
}

/// `element` as a reference a registration can use on its thread.
fn agile(element: &IUIAutomationElement) -> Result<AgileReference<IUIAutomationElement>, String> {
    AgileReference::new(element).map_err(|error| format!("could not move an element: {error}"))
}

/// Installs the desktop's focus hook and the target's hook, sending the
/// watcher the target's window changes. The target's hook comes first, so
/// that a window shown while the desktop's is installed is not missed.
fn start_hooks(
    target: Target,
    arrivals: &Arc<Arrivals>,
    early: &Arc<EarlyEnd>,
    changes: Sender<WindowChange>,
) -> Result<(), String> {
    {
        let arrivals = Arc::clone(arrivals);
        let early = Arc::clone(early);
        spawn_hook_thread(
            "verbatim-event-record-target",
            (EVENT_MIN, EVENT_MAX),
            target.pid(),
            move || target_sink(target, arrivals, early, changes),
        )?;
    }
    let arrivals = Arc::clone(arrivals);
    spawn_hook_thread(
        "verbatim-event-record-focus",
        (EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS),
        0,
        move || focus_sink(target, arrivals),
    )
}

/// What the target's hook does with each event but focus, which the
/// desktop's hook takes.
fn target_sink(
    target: Target,
    arrivals: Arc<Arrivals>,
    early: Arc<EarlyEnd>,
    changes: Sender<WindowChange>,
) -> Sink {
    Box::new(move |raw: RawEvent| {
        if raw.event == EVENT_OBJECT_FOCUS {
            return;
        }
        let window_object = raw.object == OBJID_WINDOW.0 && raw.child == CHILDID_SELF;
        let destroyed = raw.event == EVENT_OBJECT_DESTROY && window_object;
        if let Target::Window { hwnd, .. } = target
            && !(raw.hwnd == hwnd || window_in_target(hwnd, target.pid(), raw.hwnd))
        {
            return;
        }
        arrivals.arrive(Message::Event(Box::new(EventArrival {
            data: msaa_line(raw, true, Some(target.pid())),
            focus: None,
            raised_ms_ago: Some(raw.raised_ms_ago),
        })));
        match target {
            Target::Window { hwnd, .. } => {
                if destroyed && raw.hwnd == hwnd {
                    early.end(Reason::WindowDestroyed);
                }
            }
            Target::Process(_) => {
                if destroyed {
                    let _ = changes.send(WindowChange::Gone(raw.hwnd));
                } else if shows_window(raw.event, window_object)
                    && verbatim_uia::is_top_level_window(raw.hwnd)
                {
                    let _ = changes.send(WindowChange::Shown(raw.hwnd));
                }
            }
        }
    })
}

/// Whether `event` makes a window of the target one the watcher should
/// register: its window object was shown, or uncloaked, as a modern
/// application's frame is some time after it exists.
fn shows_window(event: u32, window_object: bool) -> bool {
    window_object && (event == EVENT_OBJECT_SHOW || event == EVENT_OBJECT_UNCLOAKED)
}

/// What the desktop's focus hook does with each focus event.
fn focus_sink(target: Target, arrivals: Arc<Arrivals>) -> Sink {
    Box::new(move |raw: RawEvent| {
        let provider = verbatim_uia::provider_process(raw.hwnd);
        let in_target = match target {
            Target::Process(pid) => provider == Some(pid),
            Target::Window { hwnd, pid } => window_in_target(hwnd, pid, raw.hwnd),
        };
        let focus = provider.map(|process| crate::behind::Focus::Msaa {
            process,
            window: raw.hwnd,
            object: raw.object,
            child: raw.child,
        });
        arrivals.arrive(Message::Event(Box::new(EventArrival {
            data: msaa_line(raw, in_target, provider),
            focus,
            raised_ms_ago: Some(raw.raised_ms_ago),
        })));
    })
}

/// An MSAA event's line: local facts only (Dickson, 2026-11-10).
fn msaa_line(raw: RawEvent, in_target: bool, pid: Option<u32>) -> EventData {
    let class = window_class_name(raw.hwnd);
    EventData {
        kind: msaa_kind(raw.event),
        in_target,
        pid,
        window: (raw.hwnd != 0).then_some(raw.hwnd),
        class: (!class.is_empty()).then_some(class),
        event: Some(raw.event),
        object: Some(raw.object),
        child: Some(raw.child),
        ..EventData::default()
    }
}

/// The kind a line gives raw event `event`, `msaa_other` for one the
/// table does not name.
fn msaa_kind(event: u32) -> &'static str {
    match event {
        EVENT_OBJECT_FOCUS => "msaa_focus",
        EVENT_SYSTEM_FOREGROUND => "msaa_foreground",
        EVENT_OBJECT_VALUECHANGE => "msaa_value",
        EVENT_OBJECT_STATECHANGE
        | EVENT_OBJECT_SELECTIONADD
        | EVENT_OBJECT_SELECTIONREMOVE
        | EVENT_OBJECT_SELECTIONWITHIN => "msaa_state",
        EVENT_OBJECT_NAMECHANGE => "msaa_name",
        EVENT_OBJECT_DESCRIPTIONCHANGE => "msaa_description",
        EVENT_OBJECT_SELECTION => "msaa_selection",
        EVENT_SYSTEM_MENUPOPUPSTART => "msaa_menu_start",
        EVENT_SYSTEM_MENUPOPUPEND | EVENT_SYSTEM_MENUEND => "msaa_menu_end",
        EVENT_SYSTEM_SWITCHEND => "msaa_switch_end",
        EVENT_OBJECT_DESTROY => "msaa_destroy",
        EVENT_SYSTEM_ALERT => "msaa_alert",
        EVENT_OBJECT_CREATE => "msaa_create",
        EVENT_OBJECT_HIDE => "msaa_hide",
        EVENT_OBJECT_REORDER => "msaa_reorder",
        EVENT_CONSOLE_CARET => "msaa_console_caret",
        EVENT_CONSOLE_UPDATE_REGION
        | EVENT_CONSOLE_UPDATE_SIMPLE
        | EVENT_CONSOLE_UPDATE_SCROLL
        | EVENT_CONSOLE_LAYOUT => "msaa_console_update",
        EVENT_OBJECT_LOCATIONCHANGE => "msaa_location",
        EVENT_OBJECT_TEXTSELECTIONCHANGED => "msaa_text_selection",
        EVENT_OBJECT_SHOW => "msaa_show",
        _ => "msaa_other",
    }
}

/// Registers the desktop-wide parts and the target's, returning the
/// target's registration, and writes a `registered` line for each window.
fn register(
    events: &UiaEvents,
    windows: &HashMap<isize, AgileReference<IUIAutomationElement>>,
    arrivals: &Arrivals,
) -> Result<&'static Registration, String> {
    // Both registrations stay until the process ends: closing one waits
    // for its running callbacks, and exit must never wait on a hung
    // application.
    Box::leak(Box::new(
        Registration::with_parts(desktop_parts(events), Scope::Desktop)
            .map_err(|error| format!("the desktop-wide registration failed: {error}"))?,
    ));
    let registration: &'static Registration = Box::leak(Box::new(
        Registration::with_parts(target_parts(events), subtrees(windows))
            .map_err(|error| format!("the target's registration failed: {error}"))?,
    ));
    for window in windows.keys() {
        arrivals.arrive(Message::Registered(*window));
    }
    Ok(registration)
}

/// A scope over the subtrees of `windows`.
fn subtrees(windows: &HashMap<isize, AgileReference<IUIAutomationElement>>) -> Scope {
    Scope::Subtrees(windows.values().cloned().collect())
}

/// Waits for `registration`'s moves to be made, saying so on standard
/// error when a registration timed out on its way.
fn settle(registration: &Registration) {
    let started = Instant::now();
    registration.settle();
    if started.elapsed() > UIA_CONNECTION_TIMEOUT {
        report_error!(
            "a UI Automation registration timed out; the `registered` lines show which windows are missing"
        );
    }
}

/// The window watcher: finds the target's new windows and moves the
/// registration. Ends when the process ends.
fn watch(
    pid: u32,
    mut windows: HashMap<isize, AgileReference<IUIAutomationElement>>,
    registration: &Registration,
    arrivals: &Arrivals,
    changes: &Receiver<WindowChange>,
) {
    let Ok(uia) = Uia::new() else {
        report_error!("the window watcher could not start UI Automation");
        return;
    };
    let Ok(request) = uia.request_fixed::<WindowLookup>() else {
        report_error!("the window watcher could not make a cache request");
        return;
    };
    while let Ok(change) = changes.recv() {
        let mut added = Vec::new();
        let mut gone = false;
        for change in std::iter::once(change).chain(changes.try_iter()) {
            match change {
                WindowChange::Shown(window)
                    if !windows.contains_key(&window)
                        && window_is_visible(window)
                        && verbatim_uia::served_by(window, pid) =>
                {
                    match find_window(&uia, &request, window).as_ref().map(agile) {
                        Some(Ok(element)) => {
                            windows.insert(window, element);
                            added.push(window);
                        }
                        Some(Err(message)) => report_error!("{message}"),
                        None => report_not_found(window),
                    }
                }
                WindowChange::Shown(_) => {}
                WindowChange::Gone(window) => {
                    gone |= windows.remove(&window).is_some();
                }
            }
        }
        if added.is_empty() && !gone {
            continue;
        }
        registration.retarget(subtrees(&windows));
        settle(registration);
        for window in added {
            arrivals.arrive(Message::Registered(window));
        }
    }
}

/// What ended the recording.
enum Ending {
    StopKey,
    TimeLimit,
    TargetExited,
    Early,
}

impl Recording {
    /// Waits for the first ending, then ends in order: the Steps thread,
    /// the writer, and the process, so that exit never waits on a hung
    /// application. Returns the exit code.
    fn wait_and_end(self) -> u32 {
        if let Some(message) = self.failed.clone() {
            report_error!("{message}");
            return self.end(1, &message);
        }
        let ending = self.wait();
        let reason = match ending {
            Ending::StopKey => "the stop key was pressed".to_owned(),
            Ending::TimeLimit => "the time limit passed".to_owned(),
            Ending::TargetExited => "the target exited".to_owned(),
            Ending::Early => self
                .early
                .reason()
                .map_or_else(|| "it ended early".to_owned(), Reason::describe),
        };
        let code = if matches!(ending, Ending::Early) {
            self.early.reason().map_or(1, Reason::exit_code)
        } else {
            0
        };
        self.end(code, &reason)
    }

    /// Waits on the hot key, the target's process handle and the
    /// early-end event, with the time limit as the timeout.
    fn wait(&self) -> Ending {
        let handles: [HANDLE; 2] = [raw(&self.target_process), self.early.handle()];
        loop {
            let timeout = self.deadline.map_or(u32::MAX, |deadline| {
                let left = deadline.saturating_duration_since(Instant::now());
                u32::try_from(left.as_millis()).unwrap_or(u32::MAX)
            });
            // SAFETY: both handles are open for this call; the wake mask
            // takes the hot key's message.
            let woken =
                unsafe { MsgWaitForMultipleObjects(Some(&handles), false, timeout, QS_HOTKEY) };
            if woken == WAIT_TIMEOUT {
                return Ending::TimeLimit;
            }
            if woken == WAIT_FAILED {
                self.early.end(Reason::WaitFailed(
                    windows::core::Error::from_thread().to_string(),
                ));
                return Ending::Early;
            }
            match woken.0 - WAIT_OBJECT_0.0 {
                0 => return Ending::TargetExited,
                1 => return Ending::Early,
                _ => {
                    let mut message = MSG::default();
                    // SAFETY: `message` is a valid out-parameter for this
                    // thread's own queue.
                    while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool()
                    {
                        if message.message == WM_HOTKEY
                            && usize::try_from(STOP_KEY_ID).is_ok_and(|id| message.wParam.0 == id)
                        {
                            return Ending::StopKey;
                        }
                    }
                }
            }
        }
    }

    /// Ends the recording: the Steps thread first, then the writer, then
    /// the closing message.
    fn end(self, code: u32, reason: &str) -> u32 {
        if let Some(steps) = self.steps {
            steps.end();
        }
        self.arrivals.arrive(Message::End);
        let written = self
            .writer
            .join()
            .unwrap_or_else(|_| Err("the writer panicked".to_owned()));
        let (code, message) = closing(written, code, reason, &self.path);
        if code == 0 {
            let mut stdout = std::io::stdout();
            let _ = writeln!(stdout, "{message}");
            let _ = stdout.flush();
        } else {
            report_error!("{message}");
        }
        code
    }
}

/// The exit code and closing message: the writer's result decides, read
/// after `End`, so that a failed write makes the recording a failure even
/// when the ending itself would be 0.
fn closing(written: Result<(), String>, code: u32, reason: &str, path: &Path) -> (u32, String) {
    match written {
        Ok(()) => (
            code,
            format!("Recording written to {}: {reason}.", path.display()),
        ),
        Err(error) => (
            1,
            format!(
                "The recording could not be written to {}: {error}.",
                path.display()
            ),
        ),
    }
}

/// `verbatim-events-<UTC time>.jsonl` in the working directory.
fn default_path() -> PathBuf {
    PathBuf::from(format!("verbatim-events-{}.jsonl", utc_stamp(false)))
}

/// The UTC start time, as the header gives it.
fn started_utc() -> String {
    utc_stamp(true)
}

/// The UTC time now, with its separators when `separated`, without them
/// otherwise, for a file name.
fn utc_stamp(separated: bool) -> String {
    // SAFETY: GetSystemTime has no preconditions.
    let now = unsafe { GetSystemTime() };
    if separated {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
        )
    } else {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
        )
    }
}

/// The Windows build, as `RtlGetVersion` reports it, the one number the
/// OS gives no application a compatibility answer for, `unknown` when it
/// cannot be read.
fn windows_build() -> String {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    use windows::core::{PCSTR, s, w};
    let read = || -> Option<String> {
        // SAFETY: ntdll.dll is loaded in every process; a null answer is
        // an error, not undefined behaviour.
        let module = unsafe { GetModuleHandleW(w!("ntdll.dll")) }.ok()?;
        // SAFETY: `module` is ntdll's handle and the name is a valid C
        // string.
        let address = unsafe { GetProcAddress(module, PCSTR(s!("RtlGetVersion").0)) }?;
        // SAFETY: `RtlGetVersion` has this signature in every Windows
        // release; the transmute is between two function pointer types.
        let rtl_get_version: unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32 =
            unsafe { std::mem::transmute(address) };
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: u32::try_from(size_of::<OSVERSIONINFOW>()).ok()?,
            ..Default::default()
        };
        // SAFETY: `info` has its size set, as the call requires.
        let status = unsafe { rtl_get_version(&raw mut info) };
        (status == 0).then(|| info.dwBuildNumber.to_string())
    };
    read().unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{closing, shows_window};
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    };

    #[test]
    fn a_window_object_shown_or_uncloaked_is_one_to_register() {
        assert!(shows_window(EVENT_OBJECT_SHOW, true));
        assert!(shows_window(EVENT_OBJECT_UNCLOAKED, true));
        assert!(!shows_window(EVENT_OBJECT_UNCLOAKED, false));
        assert!(!shows_window(EVENT_OBJECT_HIDE, true));
    }

    #[test]
    fn a_write_that_failed_makes_the_exit_code_one_and_says_so() {
        assert_eq!(
            closing(
                Err("the disk is full".to_owned()),
                0,
                "the time limit passed",
                Path::new("file.jsonl")
            ),
            (
                1,
                "The recording could not be written to file.jsonl: the disk is full.".to_owned()
            )
        );
    }

    #[test]
    fn a_written_recording_keeps_the_endings_code() {
        assert_eq!(
            closing(Ok(()), 0, "the time limit passed", Path::new("file.jsonl")),
            (
                0,
                "Recording written to file.jsonl: the time limit passed.".to_owned()
            )
        );
    }
}
