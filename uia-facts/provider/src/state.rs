//! The provider program's state: its configuration, its windows and trees,
//! the holds, and the logs. A test program's state, shared by the UI
//! threads, UIA's I/O thread and the control thread under plain locks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, LPARAM, RPC_E_CALL_CANCELED};
use windows::Win32::System::Com::{
    COWAIT_DISPATCH_CALLS, COWAIT_DISPATCH_WINDOW_MESSAGES, CoWaitForMultipleHandles,
};
use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GetQueueStatus, QS_POSTMESSAGE, QS_SENDMESSAGE, WM_GETOBJECT, WM_GETTEXT, WM_GETTEXTLENGTH,
};

/// `CB_GETCOMBOBOXINFO`, which UIA's window proxy sends a window.
const CB_GETCOMBOBOXINFO: u32 = 0x0164;

/// A window's kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// A UIA provider's window.
    Uia,
    /// A window with its own `IAccessible` and no UIA provider.
    Msaa,
    /// A plain Win32 window, read through oleacc's proxy.
    Plain,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Uia => "uia",
            Kind::Msaa => "msaa",
            Kind::Plain => "plain",
        }
    }
}

/// One window.
#[derive(Clone, Copy)]
pub(crate) struct Window {
    pub(crate) kind: Kind,
    pub(crate) hwnd: isize,
    pub(crate) thread: u32,
}

/// A hold's target.
pub(crate) struct Filter {
    pub(crate) method: String,
    pub(crate) window: Option<usize>,
    pub(crate) node: Option<usize>,
    pub(crate) pump: bool,
}

/// One log entry.
pub(crate) struct Entry {
    start: u64,
    end: u64,
    kind: &'static str,
    name: String,
    window: Option<usize>,
    node: Option<usize>,
    thread: u32,
    extra: String,
}

impl Entry {
    pub(crate) fn line(&self) -> String {
        let opt = |v: Option<usize>| v.map_or("-".to_owned(), |v| v.to_string());
        let extra = if self.extra.is_empty() {
            "-"
        } else {
            &self.extra
        };
        format!(
            "entry {} {} {} {} {} {} {} {}",
            self.start,
            self.end,
            self.kind,
            self.name,
            opt(self.window),
            opt(self.node),
            self.thread,
            extra
        )
    }
}

pub(crate) struct Tree {
    pub(crate) names: Vec<String>,
    pub(crate) focus: usize,
}

struct Holds {
    armed: Option<Filter>,
    generation: u64,
}

pub(crate) struct State {
    pub(crate) com: bool,
    pub(crate) nodes: usize,
    pub(crate) bridge: bool,
    kinds: Vec<Kind>,
    windows: OnceLock<Vec<Window>>,
    by_hwnd: Mutex<HashMap<isize, usize>>,
    pub(crate) trees: Vec<Mutex<Tree>>,
    holds: Mutex<Holds>,
    released: Condvar,
    slow: AtomicU64,
    log: Mutex<Vec<Entry>>,
    requests: Mutex<HashMap<u64, Box<dyn FnOnce() + Send>>>,
    next_request: AtomicU64,
    current: AtomicU64,
    peak: AtomicU64,
}

pub(crate) static STATE: OnceLock<State> = OnceLock::new();

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn state() -> &'static State {
    STATE.get().expect("the state is configured first")
}

pub(crate) fn configure(com: bool, nodes: usize, bridge: bool, specs: &[(Kind, usize)]) {
    let trees = specs
        .iter()
        .enumerate()
        .map(|(w, _)| {
            Mutex::new(Tree {
                names: (0..nodes)
                    .map(|n| match n {
                        0 => format!("window {w}"),
                        1 => "First".to_owned(),
                        2 => "Second".to_owned(),
                        3 => "Notes".to_owned(),
                        _ => format!("node {n}"),
                    })
                    .collect(),
                focus: 0,
            })
        })
        .collect();
    let set = STATE.set(State {
        com,
        nodes,
        bridge,
        kinds: specs.iter().map(|s| s.0).collect(),
        windows: OnceLock::new(),
        by_hwnd: Mutex::new(HashMap::new()),
        trees,
        holds: Mutex::new(Holds {
            armed: None,
            generation: 0,
        }),
        released: Condvar::new(),
        slow: AtomicU64::new(0),
        log: Mutex::new(Vec::with_capacity(1 << 16)),
        requests: Mutex::new(HashMap::new()),
        next_request: AtomicU64::new(1),
        current: AtomicU64::new(0),
        peak: AtomicU64::new(0),
    });
    assert!(set.is_ok(), "configured once");
}

pub(crate) fn kind(index: usize) -> Kind {
    state().kinds[index]
}

pub(crate) fn register_hwnd(hwnd: isize, index: usize) {
    lock(&state().by_hwnd).insert(hwnd, index);
}

pub(crate) fn index_of(hwnd: isize) -> Option<usize> {
    STATE
        .get()
        .and_then(|s| lock(&s.by_hwnd).get(&hwnd).copied())
}

pub(crate) fn set_windows(windows: Vec<Window>) {
    assert!(
        state().windows.set(windows).is_ok(),
        "the windows are set once"
    );
}

pub(crate) fn window(index: usize) -> Window {
    state().windows.get().expect("the windows are made")[index]
}

/// Microseconds on `QueryPerformanceCounter`'s clock.
pub(crate) fn now_us() -> u64 {
    let mut count = 0i64;
    let mut frequency = 0i64;
    // SAFETY: a live `i64` to fill.
    let _ = unsafe { QueryPerformanceCounter(&raw mut count) };
    // SAFETY: as above.
    let _ = unsafe { QueryPerformanceFrequency(&raw mut frequency) };
    u64::try_from(i128::from(count) * 1_000_000 / i128::from(frequency)).unwrap_or(0)
}

fn thread_id() -> u32 {
    // SAFETY: no precondition.
    unsafe { GetCurrentThreadId() }
}

/// A provider call in progress: logged when it ends, counted toward the
/// peak of calls in progress at once while it lasts.
pub(crate) struct Call {
    start: u64,
    name: &'static str,
    window: usize,
    node: Option<usize>,
}

impl Call {
    /// Enters provider method `name` of window `window`'s node `node`:
    /// waits out `slow` and any hold that matches.
    pub(crate) fn enter(name: &'static str, window: usize, node: Option<usize>) -> Self {
        let state = state();
        let start = now_us();
        let now = state.current.fetch_add(1, Ordering::SeqCst) + 1;
        state.peak.fetch_max(now, Ordering::SeqCst);
        let slow = state.slow.load(Ordering::SeqCst);
        if slow > 0 {
            pumping_wait(Duration::from_millis(slow), || false);
        }
        hold_if_armed(name, window, node);
        Self {
            start,
            name,
            window,
            node,
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let state = state();
        state.current.fetch_sub(1, Ordering::SeqCst);
        lock(&state.log).push(Entry {
            start: self.start,
            end: now_us(),
            kind: "call",
            name: self.name.to_owned(),
            window: Some(self.window),
            node: self.node,
            thread: thread_id(),
            extra: String::new(),
        });
    }
}

fn hold_if_armed(name: &str, window: usize, node: Option<usize>) {
    let state = state();
    let mut holds = lock(&state.holds);
    let Some(filter) = &holds.armed else { return };
    let matches = (filter.method == "*" || filter.method == name)
        && filter.window.is_none_or(|w| w == window)
        && filter.node.is_none_or(|n| Some(n) == node);
    if !matches {
        return;
    }
    let pump = filter.pump;
    let generation = holds.generation;
    crate::say(&format!(
        "held {name} {window} {} {}",
        node.map_or("-".to_owned(), |n| n.to_string()),
        thread_id()
    ));
    if pump {
        drop(holds);
        pumping_wait(Duration::MAX, || {
            lock(&state.holds).generation != generation
        });
    } else {
        while holds.generation == generation {
            holds = state
                .released
                .wait(holds)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Waits up to `length`, or until `done`, in `CoWaitForMultipleHandles`,
/// dispatching window messages and incoming COM calls meanwhile, as a
/// provider waiting on its own outgoing call does.
fn pumping_wait(length: Duration, done: impl Fn() -> bool) {
    // SAFETY: an unnamed manual-reset event, never set, closed below.
    let event = unsafe { CreateEventW(None, true, false, None) }.expect("an event");
    let end = std::time::Instant::now().checked_add(length);
    loop {
        if done() {
            break;
        }
        let left = end.map_or(Duration::from_millis(10), |end| {
            end.saturating_duration_since(std::time::Instant::now())
        });
        if left.is_zero() {
            break;
        }
        let step = u32::try_from(left.as_millis().min(10)).unwrap_or(10).max(1);
        // SAFETY: one live event handle.
        let waited = unsafe {
            CoWaitForMultipleHandles(
                (COWAIT_DISPATCH_CALLS.0 | COWAIT_DISPATCH_WINDOW_MESSAGES.0) as u32,
                step,
                &[event],
            )
        };
        // A timed-out step answers RPC_S_CALLPENDING, an error HRESULT;
        // anything else but a cancel would be a fault of this program.
        if let Err(error) = waited {
            assert!(
                error.code().0 == 0x8001_0115_u32.cast_signed()
                    || error.code() == RPC_E_CALL_CANCELED,
                "CoWaitForMultipleHandles failed: {error}"
            );
        }
    }
    // SAFETY: the event made above.
    let _ = unsafe { CloseHandle(event) };
}

pub(crate) fn arm(filter: Filter) {
    lock(&state().holds).armed = Some(filter);
}

pub(crate) fn release() {
    let state = state();
    let mut holds = lock(&state.holds);
    holds.armed = None;
    holds.generation += 1;
    state.released.notify_all();
}

/// Keeps this UI thread from taking any message, for `ms` or, when 0,
/// until `release`. It looks at its message queue every millisecond
/// without taking anything, and logs a `queued` entry, named `sent` or
/// `posted`, whenever a sent or posted message has arrived since it last
/// looked: the evidence that a call has reached the busy thread. Which
/// message it is cannot be read without taking it, since reading a sent
/// message delivers it.
pub(crate) fn busy(ms: u64) {
    let state = state();
    let until = (ms > 0).then(|| std::time::Instant::now() + Duration::from_millis(ms));
    // SAFETY: no precondition; the call reads this thread's queue status
    // and resets only its "new since last look" bits.
    let _ = unsafe { GetQueueStatus(QS_POSTMESSAGE | QS_SENDMESSAGE) };
    let mut holds = lock(&state.holds);
    let generation = holds.generation;
    loop {
        {
            // SAFETY: as above.
            let status = unsafe { GetQueueStatus(QS_POSTMESSAGE | QS_SENDMESSAGE) };
            let arrived = status & 0xFFFF;
            for (flag, name) in [(QS_SENDMESSAGE.0, "sent"), (QS_POSTMESSAGE.0, "posted")] {
                if arrived & flag != 0 {
                    let now = now_us();
                    lock(&state.log).push(Entry {
                        start: now,
                        end: now,
                        kind: "queued",
                        name: name.to_owned(),
                        window: None,
                        node: None,
                        thread: thread_id(),
                        extra: String::new(),
                    });
                }
            }
        }
        let ended = match until {
            Some(until) => std::time::Instant::now() >= until,
            None => holds.generation != generation,
        };
        if ended {
            return;
        }
        holds = state
            .released
            .wait_timeout(holds, Duration::from_millis(1))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

pub(crate) fn set_slow(ms: u64) {
    state().slow.store(ms, Ordering::SeqCst);
}

pub(crate) fn set_focus(window: usize, node: usize) {
    lock(&state().trees[window]).focus = node;
}

pub(crate) fn rename(window: usize, node: usize, name: String) {
    lock(&state().trees[window]).names[node] = name;
}

pub(crate) fn tree(window: usize) -> MutexGuard<'static, Tree> {
    lock(&state().trees[window])
}

pub(crate) fn take_log() -> Vec<Entry> {
    std::mem::take(&mut *lock(&state().log))
}

pub(crate) fn take_peak() -> u64 {
    let state = state();
    state
        .peak
        .swap(state.current.load(Ordering::SeqCst), Ordering::SeqCst)
}

/// Logs an advise call on window `window`'s root.
pub(crate) fn log_advise(name: &'static str, window: usize, event: i32) {
    let now = now_us();
    lock(&state().log).push(Entry {
        start: now,
        end: now,
        kind: "advise",
        name: name.to_owned(),
        window: Some(window),
        node: Some(0),
        thread: thread_id(),
        extra: event.to_string(),
    });
}

/// Logs the window messages the facts read: `WM_GETOBJECT` with its object
/// id, `WM_GETTEXT`, `WM_GETTEXTLENGTH`, `CB_GETCOMBOBOXINFO` and every
/// registered message by its name, such as UIA's
/// `UiaServerConnection_HookMessage`.
pub(crate) fn log_message(window: Option<usize>, message: u32, lparam: LPARAM) {
    let Some(state) = STATE.get() else { return };
    let (name, extra) = match message {
        WM_GETOBJECT => ("WM_GETOBJECT".to_owned(), object_id(lparam).to_string()),
        WM_GETTEXT => ("WM_GETTEXT".to_owned(), String::new()),
        WM_GETTEXTLENGTH => ("WM_GETTEXTLENGTH".to_owned(), String::new()),
        CB_GETCOMBOBOXINFO => ("CB_GETCOMBOBOXINFO".to_owned(), String::new()),
        0xC000..=0xFFFF => {
            let mut buffer = [0u16; 128];
            // SAFETY: a live buffer; a registered message's number is an
            // atom of the table registered clipboard formats share.
            let length = unsafe { GetClipboardFormatNameW(message, &mut buffer) };
            let length = usize::try_from(length).unwrap_or(0);
            (
                "registered".to_owned(),
                String::from_utf16_lossy(&buffer[..length]),
            )
        }
        _ => return,
    };
    let now = now_us();
    lock(&state.log).push(Entry {
        start: now,
        end: now,
        kind: "msg",
        name,
        window,
        node: None,
        thread: thread_id(),
        extra,
    });
}

/// A `WM_GETOBJECT`'s object id: its `lParam` is a 32-bit value, which
/// callers may pass zero-extended or sign-extended, so only its low 32 bits
/// count.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the object id is the low 32 bits"
)]
pub(crate) fn object_id(lparam: LPARAM) -> i32 {
    (lparam.0 as u32).cast_signed()
}

pub(crate) fn queue_request(request: Box<dyn FnOnce() + Send>) -> u64 {
    let state = state();
    let id = state.next_request.fetch_add(1, Ordering::SeqCst);
    lock(&state.requests).insert(id, request);
    id
}

pub(crate) fn run_request(id: u64) {
    let request = lock(&state().requests).remove(&id);
    let request = request.expect("a posted request is queued first");
    request();
}
