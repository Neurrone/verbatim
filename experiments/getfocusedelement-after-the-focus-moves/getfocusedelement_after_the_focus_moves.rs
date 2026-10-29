//! Experiment (not kept in the product): does the outpost's focused-element
//! read, `GetFocusedElementBuildCache`, connect into another application
//! when the system focus moves to it between the outpost's check of the
//! focus window and the call? `focused_control_in`
//! (`crates/verbatim-outpost/src/outpost/read.rs`) checks the focus window's
//! provider process with local calls and then calls
//! `GetFocusedElementBuildCache`, which finds the system's focus by itself.
//!
//! Two shown mockapp windows, A and B, each with a focused node set by
//! `set-focus` (A's "First", B's "Second"). Each round:
//!
//! - moved: brings A forward, waits for the evidence that A has the
//!   keyboard focus, runs the outpost's check for A, brings B forward,
//!   waits for the evidence that B has it, and makes the outpost's call;
//! - control: the same with the focus left on A.
//!
//! Each call records the element's cached process id (from the cached
//! provider description, which the outpost's request holds) and name, and
//! both windows' provider call counts and `WM_GETOBJECT` counts just before
//! and just after the call. It runs on the interactive desktop and takes
//! the foreground, so nothing else may use the desktop meanwhile.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use verbatim_uia::{Node, NodeGates, Uia};
use windows::Win32::Foundation::{HWND, WAIT_TIMEOUT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::DETACHED_PROCESS;
use windows::Win32::UI::Accessibility::{
    HWINEVENTHOOK, SetWinEventHook, UIA_ProviderDescriptionPropertyId, UnhookWinEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, IsHungAppWindow, IsIconic, IsWindow, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    SW_RESTORE, SW_SHOWMINNOACTIVE, SetForegroundWindow, ShowWindowAsync, TranslateMessage,
    WINEVENT_OUTOFCONTEXT,
};
use windows::core::{s, w};

/// Rounds of each case.
const ROUNDS: usize = 20;

/// How long one wait for evidence may take before the experiment fails:
/// a switch has taken 1.4 s (`docs/tooling/troubleshooting.md`).
const EVIDENCE_LIMIT: Duration = Duration::from_secs(10);

/// A shown mockapp, driven over its stdin.
struct App {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    hwnd: HWND,
    pid: u32,
}

impl App {
    /// Starts `mockapp --fixture tree.json --backend uia --title <title>
    /// --show` and waits for `ready`.
    fn spawn(title: &str) -> Self {
        common::contain_children();
        let mut child = Command::new(env!("CARGO_BIN_EXE_mockapp"))
            .arg("--fixture")
            .arg(common::fixture_path("tree.json"))
            .args(["--backend", "uia", "--title", title, "--show"])
            .env(
                verbatim_process::clock::OFFSET_VARIABLE,
                verbatim_process::clock::offset().to_string(),
            )
            .creation_flags(DETACHED_PROCESS.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("mockapp starts");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line.trim().to_owned()).is_err() {
                    return;
                }
            }
        });
        let first = lines
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("mockapp prints a line");
        assert_eq!(first, "ready", "mockapp's first line");
        let hwnd = common::find_window(title);
        let pid = child.id();
        Self {
            child,
            stdin,
            lines,
            hwnd,
            pid,
        }
    }

    /// Sends `line` and waits for its `applied`.
    fn apply(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("mockapp's stdin");
        self.stdin.flush().expect("mockapp's stdin");
        let answer = self
            .lines
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("mockapp answers");
        assert_eq!(answer, "applied", "mockapp's answer to {line:?}");
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

/// Every window event wakes the waiting thread, which then reads the state
/// it waits for afresh; the hook's callback does nothing else.
unsafe extern "system" fn wake(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
}

/// Waits on window events until `holds`, failing the experiment with
/// `what` after [`EVIDENCE_LIMIT`].
fn await_evidence(what: &str, mut holds: impl FnMut() -> bool) {
    let deadline = Instant::now() + EVIDENCE_LIMIT;
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a local the call writes.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // SAFETY: the message just retrieved.
            let _ = unsafe { TranslateMessage(&raw const msg) };
            // SAFETY: as above.
            unsafe { DispatchMessageW(&raw const msg) };
        }
        if holds() {
            return;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let left = u32::try_from(left.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: no handles; wakes on a message or at the deadline.
        let woke =
            unsafe { MsgWaitForMultipleObjectsEx(None, left, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        assert_ne!(
            woke, WAIT_TIMEOUT,
            "no evidence within {EVIDENCE_LIMIT:?}: {what}"
        );
    }
}

/// The outpost's focus window: `focus_window` in
/// `crates/verbatim-outpost/src/outpost/window.rs` (lines 358 to 373 at
/// 93691570), `GetGUIThreadInfo` of the foreground thread, its focus window
/// or else its active window.
fn focus_window() -> Option<HWND> {
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `info` has its size set.
    unsafe { GetGUIThreadInfo(0, &raw mut info) }.ok()?;
    let window = if info.hwndFocus.0.is_null() {
        info.hwndActive
    } else {
        info.hwndFocus
    };
    (!window.0.is_null()).then_some(window)
}

/// Whether `window` has the keyboard focus and is the foreground window.
fn has_focus(window: HWND) -> bool {
    // SAFETY: no preconditions.
    focus_window() == Some(window) && unsafe { GetForegroundWindow() } == window
}

/// Brings `window` forward as the end-to-end agent's `set_foreground` does
/// (`crates/verbatim-agent/src/desktop.rs`): a restored window not in
/// front is minimized without activating another, then restored and set as
/// the foreground, since a window restored from minimized takes the
/// foreground whatever input came last (`docs/tooling/troubleshooting.md`,
/// "Windows' foreground lock keeps launched applications behind"). Waits
/// for the evidence: the window has the keyboard focus and is in front.
fn bring_forward(window: HWND, name: &str) {
    if has_focus(window) {
        return;
    }
    // SAFETY: tolerates any handle.
    if !unsafe { IsIconic(window) }.as_bool() {
        // SAFETY: posted to the window's thread.
        let posted = unsafe { ShowWindowAsync(window, SW_SHOWMINNOACTIVE) }.as_bool();
        assert!(posted, "{name}'s minimize was not posted");
        // SAFETY: tolerates any handle.
        await_evidence(&format!("{name} minimized"), || unsafe {
            IsIconic(window).as_bool()
        });
    }
    // SAFETY: posted to the window's thread.
    let posted = unsafe { ShowWindowAsync(window, SW_RESTORE) }.as_bool();
    assert!(posted, "{name}'s restore was not posted");
    // SAFETY: tolerates any handle.
    await_evidence(&format!("{name} restored"), || unsafe {
        !IsIconic(window).as_bool()
    });
    // SAFETY: tolerates any handle.
    let _ = unsafe { SetForegroundWindow(window) };
    await_evidence(
        &format!("{name} has the keyboard focus and is the foreground window"),
        || has_focus(window),
    );
}

/// `GhostWindowFromHungWindow`, looked up as `event_window_unreadable`'s
/// helper looks it up (`window.rs`, lines 274 to 293 at 93691570).
fn ghost_window_from_hung_window() -> Option<unsafe extern "system" fn(HWND) -> HWND> {
    static FUNCTION: OnceLock<Option<unsafe extern "system" fn(HWND) -> HWND>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: user32 is loaded in this process, which has hooks.
        let user32 = unsafe { GetModuleHandleW(w!("user32.dll")) }.ok()?;
        // SAFETY: a lookup by a NUL-terminated name.
        let function = unsafe { GetProcAddress(user32, s!("GhostWindowFromHungWindow")) }?;
        // SAFETY: the export's signature is `HWND (HWND)`.
        Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(HWND) -> HWND,
            >(function)
        })
    })
}

/// The outpost's check before the call, the local calls
/// `focused_control_in` makes (`read.rs`, lines 923 to 934 at 93691570):
/// the focus window ([`focus_window`]), which must be served by `pid`
/// (`control_window`, `served_by`, which is `verbatim_uia::served_by`), and
/// readable (`event_window_unreadable`, `window.rs` lines 252 to 269:
/// `IsWindow`, no ghost window, not `IsHungAppWindow`). Two steps are left
/// out: `window_belongs_to_hidden_frame`, which concerns Core's hidden
/// window only, and the backend verdict (`window_uses_uia`), which comes
/// before the call either way. Returns the checked window.
fn outpost_check(pid: u32) -> HWND {
    let window = focus_window().expect("a focus window");
    assert!(
        verbatim_uia::served_by(window.0 as isize, pid),
        "the focus window is served by the application checked for"
    );
    // SAFETY: tolerates any handle.
    let is_window = unsafe { IsWindow(Some(window)) }.as_bool();
    assert!(is_window, "the focus window is a window");
    if let Some(ghost) = ghost_window_from_hung_window() {
        // SAFETY: the export takes any window handle.
        let stand_in = unsafe { ghost(window) };
        assert!(stand_in.0.is_null(), "no ghost window stands in for it");
    }
    // SAFETY: tolerates any handle.
    let hung = unsafe { IsHungAppWindow(window) }.as_bool();
    assert!(!hung, "its application is not hung");
    window
}

/// Provider calls and `WM_GETOBJECT`s a window's mockapp counted.
#[derive(Clone)]
struct Counts {
    calls: Vec<(&'static str, u32)>,
    get_objects: Vec<(&'static str, u32)>,
}

impl Counts {
    fn read(window: HWND) -> Self {
        Self {
            calls: common::read_hits(window),
            get_objects: common::read_get_objects(window),
        }
    }

    fn total(&self) -> u32 {
        self.calls
            .iter()
            .chain(&self.get_objects)
            .map(|(_, n)| n)
            .sum()
    }

    /// What `after` counted beyond `self`, by name.
    fn rise(&self, after: &Self) -> String {
        let before = |list: &[(&'static str, u32)], name: &str| {
            list.iter()
                .find(|(n, _)| *n == name)
                .map_or(0, |(_, count)| *count)
        };
        let mut out = Vec::new();
        for (name, count) in &after.calls {
            let rise = count.saturating_sub(before(&self.calls, name));
            if rise > 0 {
                out.push(format!("{name}+{rise}"));
            }
        }
        for (name, count) in &after.get_objects {
            let rise = count.saturating_sub(before(&self.get_objects, name));
            if rise > 0 {
                out.push(format!("WM_GETOBJECT({name})+{rise}"));
            }
        }
        if out.is_empty() {
            "none".to_owned()
        } else {
            out.join(",")
        }
    }
}

/// Every process id a provider description names, each `[pid:<n>,`: a
/// fragment's names its provider's process alone, and an element hosted by
/// a window also names, first, the process whose UIA serves the window's
/// own proxies, as `answered.rs`'s test descriptions show.
fn described_pids(description: &str) -> Vec<u32> {
    description
        .split("[pid:")
        .skip(1)
        .filter_map(|rest| rest.split(',').next()?.trim().parse().ok())
        .collect()
}

/// One call's record.
struct Outcome {
    element_pids: Vec<u32>,
    name: Option<String>,
    b_rose: bool,
    a_rose: bool,
}

/// One round of `case`: the check for A, the move to B when `moved`, and the
/// outpost's call.
fn round(
    uia: &Uia,
    request: &verbatim_uia::Request<Node>,
    (a, b): (&App, &App),
    case: &str,
    index: usize,
) -> Outcome {
    let moved = case == "moved";
    bring_forward(a.hwnd, "A");
    let checked = outpost_check(a.pid);
    assert_eq!(checked, a.hwnd, "the checked window is A's");
    let checked_at = Instant::now();
    if moved {
        bring_forward(b.hwnd, "B");
    } else {
        assert!(has_focus(a.hwnd), "A still has the focus");
    }
    let a_before = Counts::read(a.hwnd);
    let b_before = Counts::read(b.hwnd);
    let focus_before = focus_window();
    let started = Instant::now();
    // The outpost's own reader (`Arc::new(Uia::focused)`, outpost/mod.rs)
    // with its node request.
    let element = uia.focused(request);
    let returned = Instant::now();
    let b_after = Counts::read(b.hwnd);
    let a_after = Counts::read(a.hwnd);
    let (description, name, error) = match &element {
        Ok(element) => {
            let description = element
                .observe_cached(UIA_ProviderDescriptionPropertyId, false)
                .ok()
                .and_then(|value| windows::core::BSTR::try_from(&value).ok())
                .map(|text| text.to_string());
            (description, element.read().name, None)
        }
        Err(error) => (None, None, Some(error.to_string())),
    };
    let element_pids = description
        .as_deref()
        .map(described_pids)
        .unwrap_or_default();
    let who = |pids: &[u32]| {
        let names: Vec<String> = pids
            .iter()
            .map(|&pid| match pid {
                pid if pid == a.pid => "A".to_owned(),
                pid if pid == b.pid => "B".to_owned(),
                pid => format!("other({pid})"),
            })
            .collect();
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join("+")
        }
    };
    let window_who = |window: Option<HWND>| match window {
        Some(window) if window == a.hwnd => "A",
        Some(window) if window == b.hwnd => "B",
        Some(_) => "other",
        None => "none",
    };
    println!(
        "RESULT case={case} round={index} focus_at_call={} element={} name={:?} \
         error={:?} b_calls={} a_calls={} check_to_return_us={} call_us={} description={:?}",
        window_who(focus_before),
        who(&element_pids),
        name.as_deref().unwrap_or(""),
        error.as_deref().unwrap_or(""),
        b_before.rise(&b_after),
        a_before.rise(&a_after),
        (returned - checked_at).as_micros(),
        (returned - started).as_micros(),
        description.as_deref().unwrap_or(""),
    );
    Outcome {
        element_pids,
        name,
        b_rose: b_after.total() > b_before.total(),
        a_rose: a_after.total() > a_before.total(),
    }
}

fn main() {
    common::init_com();
    // SAFETY: out-of-context hooks with a callback that does nothing; this
    // thread pumps its messages in every wait.
    let hook = |first, last| unsafe {
        SetWinEventHook(first, last, None, Some(wake), 0, 0, WINEVENT_OUTOFCONTEXT)
    };
    let hooks = [
        hook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
        hook(EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS),
    ];
    assert!(
        hooks.iter().all(|hook| !hook.is_invalid()),
        "the wake hooks are set"
    );

    let tag = std::process::id();
    let mut a = App::spawn(&format!("getfocus-proof-A-{tag}"));
    let mut b = App::spawn(&format!("getfocus-proof-B-{tag}"));
    a.apply("set-focus item1");
    b.apply("set-focus item2");
    println!(
        "RESULT setup a_pid={} b_pid={} a_focus=First b_focus=Second",
        a.pid, b.pid
    );

    let uia = Uia::new().expect("a UIA client");
    // `Context::node_request` (outpost/mod.rs) with `gates::node_gates` of
    // the outpost's initial `Fetches::default()`, every detail on.
    let request = uia
        .request::<Node>(NodeGates {
            role: (),
            value: (),
            states: (),
            description: Some(()),
            shortcut: Some(()),
            position: Some(()),
            place: Some(()),
            cell: Some(()),
            level: true,
        })
        .expect("the outpost's node request");

    let mut tallies = Vec::new();
    for case in ["moved", "control"] {
        let mut in_b = 0;
        let mut in_a = 0;
        let mut b_rose = 0;
        let mut a_rose = 0;
        let mut b_named = 0;
        for index in 0..ROUNDS {
            let outcome = round(&uia, &request, (&a, &b), case, index);
            in_b += usize::from(outcome.element_pids.contains(&b.pid));
            in_a += usize::from(outcome.element_pids.contains(&a.pid));
            b_rose += usize::from(outcome.b_rose);
            a_rose += usize::from(outcome.a_rose);
            b_named += usize::from(outcome.name.as_deref() == Some("Second"));
        }
        tallies.push(format!(
            "SUMMARY case={case} rounds={ROUNDS} element_in_A={in_a} element_in_B={in_b} \
             named_Second={b_named} B_counted_calls={b_rose} A_counted_calls={a_rose}"
        ));
    }
    for line in tallies {
        println!("{line}");
    }

    for hook in hooks {
        // SAFETY: hooks this thread set.
        let _ = unsafe { UnhookWinEvent(hook) };
    }
}
