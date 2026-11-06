//! The UIA facts' provider program (`uia-facts/readme.md`, "The provider
//! program"): hidden top-level windows on chosen UI threads, each a UIA
//! window serving a fragment tree of set size, a window with its own
//! `IAccessible`, or a plain Win32 window. A fact binary starts it through
//! `uia_facts_support::Provider::launch` and drives it with one request a
//! line on its standard input; each request is answered on its standard
//! output, with `ok` or `err`, once the provider has acted on it.
//!
//! Command line:
//!
//! - `--windows <kind>:<thread>,...`: window i is of `kind` (`uia`, `msaa`
//!   or `plain`), made on UI thread `thread`. Default `uia:0`.
//! - `--threading com|nocom`: the UIA providers declare
//!   `ProviderOptions_UseComThreading` (the default, served on their UI
//!   thread) or only `ServerSideProvider` (served on UIA's I/O thread).
//! - `--nodes <n>`: the nodes of each UIA window's tree, its root included.
//!   Default 24.
//! - `--bridge`: a UIA window also answers `OBJID_CLIENT` with its UIA
//!   provider, so an MSAA client reaches UIA's MSAA bridge.
//!
//! At start it prints `window <i> <kind> <hwnd> <thread id>` for each
//! window, then `ready <process id>`.
//!
//! Requests (w a window index, n a node index):
//!
//! - `hold <method|*> [w=<w>] [n=<n>] [pump]`: every provider call of that
//!   method (`*` any method, `GetObject` the window's `WM_GETOBJECT`),
//!   limited to the window and node when given, waits until `release`,
//!   without pumping, or with `pump` in `CoWaitForMultipleHandles`. Each
//!   call that starts to wait prints `held <method> <w> <n> <thread id>`.
//! - `release`: ends every hold and every busy UI thread waiting for it.
//! - `slow <ms>`: every provider call waits `ms` while its thread keeps
//!   dispatching messages and calls; `slow 0` ends it.
//! - `busy <w> [ms] [focus-at <ms> [child]]`: window w's UI thread runs
//!   without taking a message, for `ms` or until `release`; answered once
//!   it has started, with the time it started. Each arrival of a sent or
//!   posted message meanwhile is logged as a `queued` entry named `sent` or
//!   `posted`. With `focus-at`, the busy thread itself moves the window's
//!   focus to `child` (0, the window's own object, when not given) and
//!   raises a focus `WinEvent` naming it that long into the busy time, as
//!   an application moving its own focus while it does not answer.
//! - `focus <w> <n>`: the tree's keyboard focus moves to node n, with no
//!   event.
//! - `rename <w> <n> <name>`: node n's name.
//! - `raise <focus|name|notification|structure|text> <w> <n>`: raises the
//!   event on window w's UI thread; answered with the time it was raised.
//!   On an MSAA or plain window, `focus` moves the focus to child n of its
//!   client object (0 for the object itself) and raises the focus
//!   `WinEvent` naming it (`OBJID_CLIENT`, child n), from its UI thread.
//!   The MSAA window's object has two children, `First` (1) and `Second`
//!   (2); it and each child report themselves focused, as mockapp's MSAA
//!   objects did, and `accFocus` answers the one with the focus.
//! - `winevent <focus|name|foreground> <w>`: raises that `WinEvent`
//!   (`EVENT_OBJECT_FOCUS`, `EVENT_OBJECT_NAMECHANGE` or
//!   `EVENT_SYSTEM_FOREGROUND`) naming window w (`OBJID_CLIENT`,
//!   `CHILDID_SELF`) from the control thread, not the window's own, so it
//!   is raised while that thread is busy; answered with the time it was
//!   raised.
//! - `raise-rate <kind> <w> <n> <per second> <count>`: raises `count` of
//!   them at that pace on the UI thread, dispatching the messages and calls
//!   waiting for it between two events; a pace of 0 raises them back to
//!   back, dispatching nothing; answered once all are raised.
//! - `foreground <w>`: shows window w and brings it to the foreground;
//!   answered with whether it is the foreground window.
//! - `show <w>` and `hide <w>`: shows or hides window w, without changing
//!   the foreground.
//! - `post-null <w>`: posts `WM_NULL` to window w.
//! - `hung <w>`: answers `IsHungAppWindow` of window w, `true` or `false`.
//! - `log`: prints every log entry since the last `log`, one
//!   `entry <start µs> <end µs> <kind> <name> <w> <n> <thread id> <extra>`
//!   line each, then `ok <count>`. Kinds are `call` (a provider method),
//!   `msg` (a window message, its object id or registered name in
//!   `extra`) and `advise` (an event added or removed, its id in `extra`).
//!   Times are `QueryPerformanceCounter` in microseconds, a clock every
//!   process shares.
//! - `peak`: answers the most provider calls ever in progress at once, and
//!   resets it.

mod providers;
mod state;

use std::io::{BufRead, Write};
use std::sync::mpsc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE,
    EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    IsHungAppWindow, MSG, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
    SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_NULL, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

use state::{Kind, Window};

/// Posted to a window to run a request on its UI thread.
const WM_REQUEST: u32 = WM_APP + 1;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut layout = "uia:0".to_owned();
    let mut com = true;
    let mut nodes = 24usize;
    let mut bridge = false;
    let mut tree_view = false;
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--windows" => {
                layout = value;
                i += 1;
            }
            "--threading" => {
                com = match value.as_str() {
                    "com" => true,
                    "nocom" => false,
                    other => panic!("--threading takes com or nocom, not {other}"),
                };
                i += 1;
            }
            "--nodes" => {
                nodes = value.parse().expect("--nodes takes a count");
                assert!(
                    nodes >= 5,
                    "--nodes must be at least 5, for the patterns' nodes"
                );
                i += 1;
            }
            "--bridge" => bridge = true,
            "--tree-view" => tree_view = true,
            other => panic!("unknown argument {other}"),
        }
        i += 1;
    }
    let specs: Vec<(Kind, usize)> = layout
        .split(',')
        .map(|spec| {
            let (kind, thread) = spec.split_once(':').expect("a window is <kind>:<thread>");
            let kind = match kind {
                "uia" => Kind::Uia,
                "msaa" => Kind::Msaa,
                "plain" => Kind::Plain,
                other => panic!("unknown window kind {other}"),
            };
            (kind, thread.parse().expect("a thread number"))
        })
        .collect();
    state::configure(com, nodes, bridge, tree_view, &specs);

    // SAFETY: a null module name asks for this executable's own module.
    let instance = unsafe { GetModuleHandleW(None) }.expect("this module's handle");
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_procedure),
        hInstance: instance.into(),
        lpszClassName: w!("UiaFactsProvider"),
        ..Default::default()
    };
    // SAFETY: the class structure is complete and its name a static string.
    let atom = unsafe { RegisterClassW(&raw const class) };
    assert!(atom != 0, "the window class registers");

    let threads = specs.iter().map(|s| s.1).max().unwrap_or(0) + 1;
    let (made, windows_made) = mpsc::channel();
    for thread in 0..threads {
        let mine: Vec<usize> = (0..specs.len()).filter(|i| specs[*i].1 == thread).collect();
        let made = made.clone();
        std::thread::spawn(move || ui_thread(&mine, &made));
    }
    drop(made);
    let mut windows: Vec<(usize, Window)> = windows_made.iter().take(specs.len()).collect();
    windows.sort_by_key(|w| w.0);
    assert_eq!(windows.len(), specs.len(), "every window was made");
    for (index, window) in &windows {
        say(&format!(
            "window {index} {} {} {}",
            window.kind.name(),
            window.hwnd,
            window.thread
        ));
    }
    state::set_windows(windows.into_iter().map(|w| w.1).collect());
    // SAFETY: no precondition.
    say(&format!("ready {}", unsafe { GetCurrentProcessId() }));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let answer = handle(line.trim());
        say(&answer);
    }
    // The fact binary closed its end. The provider ends itself at once
    // rather than running UIA's detach code, which can hang or crash
    // (`books/dev/src/reference/uia-bugs.md`, "A process that used UIA can
    // hang or crash as it exits").
    // SAFETY: no precondition.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: the pseudo-handle of this process.
    let _ = unsafe { TerminateProcess(process, 0) };
}

/// Writes one line to standard output, whole.
pub(crate) fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn ui_thread(mine: &[usize], made: &mpsc::Sender<(usize, Window)>) {
    // SAFETY: called once on this new thread, before any COM use.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .expect("the UI thread joins an STA");
    // SAFETY: no precondition.
    let thread = unsafe { GetCurrentThreadId() };
    for index in mine {
        let kind = state::kind(*index);
        let title: Vec<u16> = format!("uia-facts {} window {index}", kind.name())
            .encode_utf16()
            .chain([0])
            .collect();
        // SAFETY: the class is registered and the title a live, terminated
        // string for the call's length.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("UiaFactsProvider"),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
        }
        .expect("the window is made");
        state::register_hwnd(hwnd.0 as isize, *index);
        made.send((
            *index,
            Window {
                kind,
                hwnd: hwnd.0 as isize,
                thread,
            },
        ))
        .expect("the main thread waits for the windows");
    }
    let mut msg = MSG::default();
    // SAFETY: `msg` is a live structure for each call.
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.as_bool() {
        // SAFETY: as above, a message GetMessageW filled.
        let _ = unsafe { TranslateMessage(&raw const msg) };
        // SAFETY: as above.
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let index = state::index_of(hwnd.0 as isize);
    state::log_message(index, message, lparam);
    if message == WM_REQUEST {
        state::run_request(wparam.0 as u64);
        return LRESULT(0);
    }
    if let Some(index) = index
        && let Some(answer) = providers::get_object(hwnd, index, message, wparam, lparam)
    {
        return answer;
    }
    // SAFETY: the arguments are the ones this window was sent.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Runs `request` on window `index`'s UI thread and waits for its answer.
fn on_ui_thread(
    index: usize,
    request: impl FnOnce(&mpsc::Sender<String>) + Send + 'static,
) -> String {
    let (answer, answered) = mpsc::channel();
    let id = state::queue_request(Box::new(move || request(&answer)));
    let hwnd = HWND(state::window(index).hwnd as *mut _);
    // SAFETY: a window of this process, alive while the process runs.
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_REQUEST,
            WPARAM(usize::try_from(id).expect("a request id fits")),
            LPARAM(0),
        )
    }
    .expect("the request is posted");
    answered.recv().expect("the UI thread answers")
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per request, read as a table"
)]
fn handle(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((&verb, rest)) = words.split_first() else {
        return "err empty request".to_owned();
    };
    let number = |i: usize| -> Option<usize> { rest.get(i).and_then(|s| s.parse().ok()) };
    match verb {
        "hold" => {
            let Some(method) = rest.first() else {
                return "err hold takes a method".to_owned();
            };
            let mut filter = state::Filter {
                method: (*method).to_owned(),
                window: None,
                node: None,
                pump: false,
            };
            for word in &rest[1..] {
                if let Some(w) = word.strip_prefix("w=") {
                    filter.window = w.parse().ok();
                } else if let Some(n) = word.strip_prefix("n=") {
                    filter.node = n.parse().ok();
                } else if *word == "pump" {
                    filter.pump = true;
                } else {
                    return format!("err unknown hold option {word}");
                }
            }
            state::arm(filter);
            "ok".to_owned()
        }
        "release" => {
            state::release();
            "ok".to_owned()
        }
        "slow" => match number(0) {
            Some(ms) => {
                state::set_slow(ms as u64);
                "ok".to_owned()
            }
            None => "err slow takes milliseconds".to_owned(),
        },
        "busy" => {
            let Some(index) = number(0) else {
                return "err busy takes a window".to_owned();
            };
            let ms = number(1).unwrap_or(0) as u64;
            let focus_at = match (rest.get(2), number(3)) {
                (Some(&"focus-at"), Some(at)) => Some((at as u64, number(4).unwrap_or(0))),
                (None, _) => None,
                _ => {
                    return "err busy takes a window, milliseconds and focus-at <ms> [child]"
                        .to_owned();
                }
            };
            on_ui_thread(index, move |answer| {
                let _ = answer.send(format!("ok {}", state::now_us()));
                state::busy(ms, focus_at.map(|(at, child)| (at, index, child)));
            })
        }
        "focus" => match (number(0), number(1)) {
            (Some(w), Some(n)) => {
                state::set_focus(w, n);
                "ok".to_owned()
            }
            _ => "err focus takes a window and a node".to_owned(),
        },
        "rename" => match (number(0), number(1), rest.get(2)) {
            (Some(w), Some(n), Some(name)) => {
                state::rename(w, n, (*name).to_owned());
                "ok".to_owned()
            }
            _ => "err rename takes a window, a node and a name".to_owned(),
        },
        "raise" | "raise-rate" => {
            let (Some(kind), Some(w), Some(n)) = (rest.first().copied(), number(1), number(2))
            else {
                return "err raise takes a kind, a window and a node".to_owned();
            };
            let Some(event) = providers::Event::parse(kind) else {
                return format!("err unknown event {kind}");
            };
            let (pace, count) = if verb == "raise" {
                (0, 1)
            } else {
                match (number(3), number(4)) {
                    (Some(pace), Some(count)) => (pace, count),
                    _ => return "err raise-rate takes a pace and a count".to_owned(),
                }
            };
            on_ui_thread(w, move |answer| {
                let _ = answer.send(providers::raise(event, w, n, pace, count));
            })
        }
        "winevent" => {
            let (Some(kind), Some(index)) = (rest.first().copied(), number(1)) else {
                return "err winevent takes a kind and a window".to_owned();
            };
            let event = match kind {
                "focus" => EVENT_OBJECT_FOCUS,
                "name" => EVENT_OBJECT_NAMECHANGE,
                "foreground" => EVENT_SYSTEM_FOREGROUND,
                other => return format!("err unknown winevent {other}"),
            };
            format!("ok {}", state::win_event(event, index, 0))
        }
        "show" | "hide" => {
            let Some(index) = number(0) else {
                return format!("err {verb} takes a window");
            };
            let hwnd = HWND(state::window(index).hwnd as *mut _);
            let command = if verb == "show" {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            };
            // SAFETY: a window of this process; ShowWindow may be called
            // from any thread.
            let _ = unsafe { ShowWindow(hwnd, command) };
            "ok".to_owned()
        }
        "post-null" | "hung" => {
            let Some(index) = number(0) else {
                return format!("err {verb} takes a window");
            };
            let hwnd = HWND(state::window(index).hwnd as *mut _);
            if verb == "hung" {
                // SAFETY: a window of this process.
                return format!("ok {}", unsafe { IsHungAppWindow(hwnd) }.as_bool());
            }
            // SAFETY: a window of this process; the message waits in its
            // queue.
            match unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) } {
                Ok(()) => "ok".to_owned(),
                Err(error) => format!("err {error}"),
            }
        }
        "foreground" => {
            let Some(index) = number(0) else {
                return "err foreground takes a window".to_owned();
            };
            on_ui_thread(index, move |answer| {
                let _ = answer.send(foreground(index));
            })
        }
        "log" => {
            let entries = state::take_log();
            for entry in &entries {
                say(&entry.line());
            }
            format!("ok {}", entries.len())
        }
        "peak" => format!("ok {}", state::take_peak()),
        other => format!("err unknown request {other}"),
    }
}

/// Shows window `index` and brings it to the foreground, attaching to the
/// foreground window's thread so that Windows allows it.
fn foreground(index: usize) -> String {
    let hwnd = HWND(state::window(index).hwnd as *mut _);
    // SAFETY: a window of this thread.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    // SAFETY: no precondition.
    let current = unsafe { GetForegroundWindow() };
    // SAFETY: a null process id pointer is allowed.
    let theirs = unsafe { GetWindowThreadProcessId(current, None) };
    // SAFETY: no precondition.
    let ours = unsafe { GetCurrentThreadId() };
    // SAFETY: two thread ids; attaching shares their input state.
    let attached = theirs != 0 && unsafe { AttachThreadInput(ours, theirs, true) }.as_bool();
    // Windows lets the process that sent the last input take the
    // foreground: F24, bound to nothing, pressed and released first.
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, VK_F24,
        };
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_F24,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        let size = i32::try_from(std::mem::size_of::<INPUT>()).expect("a small structure");
        // SAFETY: a live array of keyboard inputs of the size given.
        let _ = unsafe { SendInput(&inputs, size) };
    }
    // SAFETY: a window of this thread.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    if attached {
        // SAFETY: as above, undoing the attachment.
        let _ = unsafe { AttachThreadInput(ours, theirs, false) };
    }
    // SAFETY: no precondition.
    let now = unsafe { GetForegroundWindow() };
    format!("ok {}", now == hwnd)
}
