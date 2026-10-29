//! Measures how long after each `EVENT_SYSTEM_FOREGROUND` the event's window
//! becomes `GetForegroundWindow`.
//!
//! An out-of-context WinEvent hook stamps each foreground event with
//! `QueryPerformanceCounter`; a poller thread reads `GetForegroundWindow`
//! every 0.5 ms from a spin loop and records each change, and each read
//! that took, or started, more than 2 ms late.
//! A driver opens Settings from the desktop as `Scenario::open_settings_page`
//! does, switches from Notepad to Settings with Alt+Tab, and opens Notepad
//! from the desktop, and the run's records are written to a folder.
//!
//! Usage: `foreground-lag-probe <out folder> <settings runs> <alt-tab runs> <notepad runs>`

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentThread, GetCurrentThreadId, OpenProcess, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, STARTF_USESHOWWINDOW,
    STARTUPINFOW, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, EnumWindows, FindWindowW,
    GetClassNameW, GetForegroundWindow, GetMessageW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, MSG, PostMessageW, PostThreadMessageW, SMTO_ABORTIFHUNG,
    SW_RESTORE, SW_SHOWMINNOACTIVE, SendMessageTimeoutW, SetForegroundWindow, ShowWindowAsync,
    TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_COMMAND, WM_QUIT,
};

static FREQ: OnceLock<i64> = OnceLock::new();
static START: OnceLock<i64> = OnceLock::new();
/// The trial under way, stamped on every record; 0 between trials.
static TRIAL: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// Each change of `GetForegroundWindow` the poller saw: the time of the
/// first read naming the new window, the window, and the time of the read
/// before it, which still named the old one.
static CHANGES: Mutex<Vec<(f64, isize, f64)>> = Mutex::new(Vec::new());
/// The driver's steps: time, trial, text.
static MARKS: Mutex<Vec<(f64, usize, String)>> = Mutex::new(Vec::new());
/// Per trial, the longest gap between two of the poller's reads, in ms.
static GAPS: Mutex<Vec<(usize, f64)>> = Mutex::new(Vec::new());
/// Each slow read: when it started, how late it started after the previous
/// read ended beyond the 0.5 ms interval, and how long the call took, in ms.
static SLOW: Mutex<Vec<(f64, f64, f64)>> = Mutex::new(Vec::new());

struct Event {
    t: f64,
    trial: usize,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    class: String,
    title: String,
    image: String,
    /// `GetForegroundWindow` read first thing in the callback.
    fg_in_callback: isize,
}

fn qpc() -> i64 {
    let mut v = 0;
    unsafe { QueryPerformanceCounter(&mut v) };
    v
}

/// Milliseconds since the run started.
fn now_ms() -> f64 {
    (qpc() - START.get().unwrap()) as f64 * 1000.0 / *FREQ.get().unwrap() as f64
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), 256) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn title_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), 256) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn image_of(hwnd: HWND) -> String {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return format!("pid {pid}");
    }
    let mut buf = [0u16; 512];
    let mut size = 512;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(process) };
    if ok == 0 {
        return format!("pid {pid}");
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    path.rsplit('\\').next().unwrap_or(&path).to_owned()
}

fn cloaked(hwnd: HWND) -> bool {
    let mut value = 0u32;
    let hr = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut value).cast(), 4)
    };
    hr == 0 && value != 0
}

fn mark(text: impl Into<String>) {
    let text = text.into();
    let t = now_ms();
    let trial = TRIAL.load(Ordering::SeqCst);
    eprintln!("{t:10.3} [{trial}] {text}");
    MARKS.lock().unwrap().push((t, trial, text));
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let t = now_ms();
    let fg_in_callback = unsafe { GetForegroundWindow() } as isize;
    let event = Event {
        t,
        trial: TRIAL.load(Ordering::SeqCst),
        hwnd: hwnd as isize,
        id_object,
        id_child,
        class: class_of(hwnd),
        title: title_of(hwnd),
        image: image_of(hwnd),
        fg_in_callback,
    };
    eprintln!(
        "{t:10.3} [{}] foreground event {:#x} {} {:?} {}",
        event.trial, event.hwnd, event.class, event.title, event.image
    );
    EVENTS.lock().unwrap().push(event);
}

fn hook_thread(id: std::sync::mpsc::Sender<u32>) {
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(on_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    assert!(!hook.is_null(), "SetWinEventHook failed");
    id.send(unsafe { GetCurrentThreadId() }).unwrap();
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unsafe { UnhookWinEvent(hook) };
}

/// Reads `GetForegroundWindow` every 0.5 ms, spinning between reads at the
/// highest thread priority: a first run waiting on a high-resolution
/// waitable timer left gaps of up to 61 ms between reads.
fn poller() {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
    let mut last: isize = -1;
    let mut last_read = now_ms();
    let mut next = last_read;
    let mut gap_trial = 0;
    let mut gap = 0.0f64;
    while !STOP.load(Ordering::SeqCst) {
        let now = now_ms();
        if now < next {
            std::hint::spin_loop();
            continue;
        }
        next = now + 0.5;
        let fg = unsafe { GetForegroundWindow() } as isize;
        let t = now_ms();
        let late = now - last_read - 0.5;
        if late > 2.0 || t - now > 2.0 {
            SLOW.lock().unwrap().push((now, late, t - now));
        }
        let trial = TRIAL.load(Ordering::SeqCst);
        if trial != gap_trial {
            if gap_trial != 0 {
                GAPS.lock().unwrap().push((gap_trial, gap));
            }
            gap_trial = trial;
            gap = 0.0;
        }
        gap = gap.max(t - last_read);
        let last_read_before = last_read;
        last_read = t;
        if fg != last {
            CHANGES.lock().unwrap().push((t, fg, last_read_before));
            last = fg;
        }
    }
    if gap_trial != 0 {
        GAPS.lock().unwrap().push((gap_trial, gap));
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn wait_until(limit_ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = now_ms() + limit_ms as f64;
    loop {
        if f() {
            return true;
        }
        if now_ms() > end {
            return false;
        }
        sleep_ms(5);
    }
}

fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// The taskbar's Minimize All, then Program Manager given the foreground,
/// as the agent's `minimize_all` does.
fn to_desktop() {
    const MINIMIZE_ALL: usize = 419;
    let tray = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null()) };
    if !tray.is_null() {
        unsafe {
            SendMessageTimeoutW(
                tray,
                WM_COMMAND,
                MINIMIZE_ALL as WPARAM,
                0,
                SMTO_ABORTIFHUNG,
                5000,
                std::ptr::null_mut(),
            )
        };
    }
    let progman = unsafe { FindWindowW(wide("Progman").as_ptr(), std::ptr::null()) };
    sleep_ms(300);
    let accepted = unsafe { SetForegroundWindow(progman) };
    let held = wait_until(2000, || foreground() == progman);
    mark(format!(
        "desktop: SetForegroundWindow(Progman) {accepted}, in front {held}, foreground {}",
        class_of(foreground())
    ));
}

fn launch(command_line: &str, minimized: bool) -> u32 {
    let mut line = wide(command_line);
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    if minimized {
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_SHOWMINNOACTIVE as u16;
    }
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut info,
        )
    };
    assert!(ok != 0, "CreateProcessW failed for {command_line}");
    unsafe {
        AllowSetForegroundWindow(info.dwProcessId);
        CloseHandle(info.hThread);
        CloseHandle(info.hProcess);
    }
    info.dwProcessId
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> i32 {
        unsafe { (*(list as *mut Vec<HWND>)).push(hwnd) };
        1
    }
    let mut list: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), (&raw mut list) as LPARAM) };
    list
}

fn find_shown(class: &str, title: &str, not_in: &[HWND]) -> Option<HWND> {
    top_level_windows().into_iter().find(|&w| {
        (unsafe { IsWindowVisible(w) } != 0)
            && !cloaked(w)
            && class_of(w) == class
            && title_of(w).contains(title)
            && !not_in.contains(&w)
    })
}

/// The agent's `set_foreground`: minimize without activating if restored and
/// behind, restore, then `SetForegroundWindow`.
fn bring_forward(window: HWND) {
    if foreground() != window && unsafe { IsIconic(window) } == 0 {
        mark("restored and behind; minimizing");
        unsafe { ShowWindowAsync(window, SW_SHOWMINNOACTIVE) };
        wait_until(2000, || unsafe { IsIconic(window) } != 0);
    }
    if unsafe { IsIconic(window) } != 0 {
        mark("restore requested");
        unsafe { ShowWindowAsync(window, SW_RESTORE) };
        let restored = wait_until(2000, || unsafe { IsIconic(window) } == 0);
        mark(format!("restored {restored}"));
    }
    let accepted = unsafe { SetForegroundWindow(window) };
    mark(format!("SetForegroundWindow {accepted}"));
    let held = wait_until(2000, || foreground() == window);
    mark(format!("in front {held}"));
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn alt_tab() {
    let inputs = [
        key(VK_MENU, false),
        key(VK_TAB, false),
        key(VK_TAB, true),
        key(VK_MENU, true),
    ];
    mark("Alt+Tab sent");
    unsafe { SendInput(4, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}

fn close(window: HWND) {
    mark(format!("WM_CLOSE to {:#x}", window as isize));
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
    let gone = wait_until(5000, || {
        let alive = unsafe { IsWindow(window) } != 0;
        let visible = unsafe { IsWindowVisible(window) } != 0;
        !alive || !visible || cloaked(window)
    });
    mark(format!("closed {gone}"));
}

fn open_settings() -> HWND {
    mark("launch explorer.exe ms-settings:display minimized");
    launch("explorer.exe ms-settings:display", true);
    let mut found = None;
    wait_until(10_000, || {
        found = find_shown("ApplicationFrameWindow", "Settings", &[]);
        found.is_some()
    });
    let window = found.expect("no Settings window within 10 s");
    mark(format!(
        "Settings frame present {:#x}, minimized {}",
        window as isize,
        unsafe { IsIconic(window) } != 0
    ));
    bring_forward(window);
    window
}

fn open_notepad() -> HWND {
    let before: Vec<HWND> = top_level_windows();
    mark("launch notepad.exe minimized");
    launch("notepad.exe", true);
    let mut found = None;
    wait_until(10_000, || {
        found = find_shown("Notepad", "", &before);
        found.is_some()
    });
    let window = found.expect("no Notepad window within 10 s");
    mark(format!(
        "Notepad present {:#x}, minimized {}",
        window as isize,
        unsafe { IsIconic(window) } != 0
    ));
    bring_forward(window);
    window
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(&args[1]);
    let settings_runs: usize = args[2].parse().unwrap();
    let alt_tab_runs: usize = args[3].parse().unwrap();
    let notepad_runs: usize = args[4].parse().unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let mut freq = 0;
    unsafe { QueryPerformanceFrequency(&mut freq) };
    FREQ.set(freq).unwrap();
    START.set(qpc()).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let hook = std::thread::spawn(move || hook_thread(tx));
    let hook_id = rx.recv().unwrap();
    let poll = std::thread::spawn(poller);
    sleep_ms(500);

    let mut kinds: Vec<(usize, &str)> = Vec::new();
    let mut trial = 0;
    for _ in 0..settings_runs {
        to_desktop();
        sleep_ms(700);
        trial += 1;
        kinds.push((trial, "settings-from-desktop"));
        TRIAL.store(trial, Ordering::SeqCst);
        let window = open_settings();
        sleep_ms(1500);
        TRIAL.store(0, Ordering::SeqCst);
        close(window);
        sleep_ms(1000);
    }

    if alt_tab_runs > 0 {
        to_desktop();
        sleep_ms(700);
        trial += 1;
        kinds.push((trial, "settings-from-desktop"));
        TRIAL.store(trial, Ordering::SeqCst);
        let settings = open_settings();
        sleep_ms(1500);
        TRIAL.store(0, Ordering::SeqCst);
        let notepad = open_notepad();
        sleep_ms(1500);
        for _ in 0..alt_tab_runs {
            trial += 1;
            kinds.push((trial, "alt-tab-notepad-to-settings"));
            TRIAL.store(trial, Ordering::SeqCst);
            if foreground() != notepad {
                mark("Notepad not in front before the switch");
            }
            alt_tab();
            sleep_ms(1500);
            trial += 1;
            kinds.push((trial, "alt-tab-settings-to-notepad"));
            TRIAL.store(trial, Ordering::SeqCst);
            alt_tab();
            sleep_ms(1500);
            TRIAL.store(0, Ordering::SeqCst);
        }
        close(notepad);
        close(settings);
        sleep_ms(1000);
    }

    for _ in 0..notepad_runs {
        to_desktop();
        sleep_ms(700);
        trial += 1;
        kinds.push((trial, "notepad-from-desktop"));
        TRIAL.store(trial, Ordering::SeqCst);
        let window = open_notepad();
        sleep_ms(1500);
        TRIAL.store(0, Ordering::SeqCst);
        close(window);
        sleep_ms(1000);
    }

    to_desktop();
    sleep_ms(500);
    STOP.store(true, Ordering::SeqCst);
    poll.join().unwrap();
    unsafe { PostThreadMessageW(hook_id, WM_QUIT, 0, 0) };
    hook.join().unwrap();
    write_results(&out, &kinds);
}

/// The change that put `event`'s window in front: the last change at or
/// before the event if it named the window, otherwise the first change to
/// it after the event within 1 s.
fn change_for<'a>(event: &Event, changes: &'a [(f64, isize, f64)]) -> Option<&'a (f64, isize, f64)> {
    let before = changes.iter().rev().find(|c| c.0 <= event.t);
    if let Some(c) = before
        && c.1 == event.hwnd
    {
        return Some(c);
    }
    changes
        .iter()
        .find(|c| c.0 > event.t && c.1 == event.hwnd)
        .filter(|c| c.0 - event.t <= 1000.0)
}

/// The lag of `event`: from the event to the first read naming its window,
/// negative when it was named before the event.
fn lag(event: &Event, changes: &[(f64, isize, f64)]) -> Option<f64> {
    change_for(event, changes).map(|c| c.0 - event.t)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    // Nearest rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

fn write_results(out: &std::path::Path, kinds: &[(usize, &str)]) {
    let events = EVENTS.lock().unwrap();
    let changes = CHANGES.lock().unwrap();
    let marks = MARKS.lock().unwrap();
    let gaps = GAPS.lock().unwrap();
    let kind_of = |trial: usize| {
        kinds
            .iter()
            .find(|k| k.0 == trial)
            .map_or("between-trials", |k| k.1)
    };
    let mut tsv = String::from(
        "trial\tkind\tt_ms\thwnd\tclass\ttitle\timage\tid_object\tid_child\tfg_in_callback_class\tlag_ms\tread_before_ms\n",
    );
    for e in events.iter() {
        let lag = lag(e, &changes);
        let fg_class = class_of(e.fg_in_callback as HWND);
        writeln!(
            tsv,
            "{}\t{}\t{:.3}\t{:#x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            e.trial,
            kind_of(e.trial),
            e.t,
            e.hwnd,
            e.class,
            e.title,
            e.image,
            e.id_object,
            e.id_child,
            if e.fg_in_callback == e.hwnd { "(itself)".to_owned() } else { fg_class },
            lag.map_or("never within 1 s".to_owned(), |l| format!("{l:.3}")),
            change_for(e, &changes).map_or(String::new(), |c| format!("{:.3}", c.2 - e.t)),
        )
        .unwrap();
    }
    std::fs::write(out.join("events.tsv"), tsv).unwrap();

    let mut ch = String::from("t_ms\thwnd\tclass\tread_before_ms\n");
    for &(t, w, before) in changes.iter() {
        writeln!(ch, "{t:.3}\t{w:#x}\t{}\t{before:.3}", class_of(w as HWND)).unwrap();
    }
    std::fs::write(out.join("foreground-changes.tsv"), ch).unwrap();

    let mut mk = String::from("t_ms\ttrial\tstep\n");
    for (t, trial, text) in marks.iter() {
        writeln!(mk, "{t:.3}\t{trial}\t{text}").unwrap();
    }
    std::fs::write(out.join("steps.tsv"), mk).unwrap();

    let mut sl = String::from("t_ms	started_late_ms	call_ms
");
    for &(t, late, call) in SLOW.lock().unwrap().iter() {
        writeln!(sl, "{t:.3}	{late:.3}	{call:.3}").unwrap();
    }
    std::fs::write(out.join("slow-reads.tsv"), sl).unwrap();

    // Summary: per kind and class, the lags' distribution; per trial, the events.
    let mut summary = String::new();
    let max_gap = gaps.iter().map(|g| g.1).fold(0.0, f64::max);
    writeln!(summary, "Longest gap between two foreground reads in any trial: {max_gap:.3} ms").unwrap();
    let mut groups: Vec<(String, String)> = Vec::new();
    for e in events.iter().filter(|e| e.trial != 0) {
        let key = (kind_of(e.trial).to_owned(), e.class.clone());
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    for (kind, class) in &groups {
        let selected: Vec<&Event> = events
            .iter()
            .filter(|e| e.trial != 0 && kind_of(e.trial) == kind && &e.class == class)
            .collect();
        let mut lags: Vec<f64> = selected.iter().filter_map(|e| lag(e, &changes)).collect();
        let never = selected.len() - lags.len();
        lags.sort_by(f64::total_cmp);
        let trials: std::collections::BTreeSet<usize> = selected.iter().map(|e| e.trial).collect();
        write!(
            summary,
            "\n{kind}, class {class}: {} events in {} trials, {never} never within 1 s",
            selected.len(),
            trials.len()
        )
        .unwrap();
        if !lags.is_empty() {
            write!(
                summary,
                "; lag min {:.2}, median {:.2}, P90 {:.2}, max {:.2} ms; within 20 ms {}, within 35 ms {}",
                lags[0],
                percentile(&lags, 50.0),
                percentile(&lags, 90.0),
                lags[lags.len() - 1],
                lags.iter().filter(|&&l| l <= 20.0).count(),
                lags.iter().filter(|&&l| l <= 35.0).count(),
            )
            .unwrap();
        }
        writeln!(summary).unwrap();
    }
    writeln!(summary, "\nPer trial:").unwrap();
    for &(trial, kind) in kinds {
        let list: Vec<String> = events
            .iter()
            .filter(|e| e.trial == trial)
            .map(|e| {
                format!(
                    "{} {:?} {}",
                    e.class,
                    e.title,
                    lag(e, &changes).map_or("never".to_owned(), |l| format!("{l:.2} ms"))
                )
            })
            .collect();
        writeln!(summary, "{trial} {kind}: {}", list.join("; ")).unwrap();
    }
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    print!("{summary}");
}
