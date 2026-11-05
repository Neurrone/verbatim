//! Real applications and the desktop, for the facts that need them:
//! starting an application, finding its windows, bringing a window to the
//! foreground, and pressing keys. Every fact that uses these first checks
//! [`crate::precondition::unlocked_desktop`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
    SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::BOOL;

/// The process owning window `hwnd`, 0 when it has none.
#[must_use]
pub fn window_pid(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    // SAFETY: any window handle is accepted; a live out value.
    let _ = unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&raw mut pid)) };
    pid
}

/// A window's class name.
#[must_use]
pub fn class_of(hwnd: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: any window handle; a live buffer.
    let length = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

/// The full path of process `pid`'s executable, when it can be read.
///
/// # Panics
///
/// Never: the buffer's length fits a `u32`.
#[must_use]
pub fn image_of(pid: u32) -> Option<PathBuf> {
    // SAFETY: no precondition; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).expect("a small buffer");
    // SAFETY: the process opened above and a live buffer of that length.
    let read = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(process) };
    read.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// Ends process `pid` at once.
pub fn kill(pid: u32) {
    // SAFETY: no precondition; the handle is closed below.
    if let Ok(process) = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
        // SAFETY: the process opened above.
        let _ = unsafe { TerminateProcess(process, 1) };
        // SAFETY: the handle opened above, closed once.
        let _ = unsafe { CloseHandle(process) };
    }
}

/// Every visible top-level window, in z order.
#[must_use]
pub fn top_windows() -> Vec<isize> {
    extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector passed below, live for the call.
        let found = unsafe { &mut *(found.0 as *mut Vec<isize>) };
        // SAFETY: a window EnumWindows names.
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            found.push(hwnd.0 as isize);
        }
        true.into()
    }
    let mut found: Vec<isize> = Vec::new();
    // SAFETY: the callback above and a pointer to `found`, live for the call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut found as isize)) };
    found
}

/// Waits up to `limit` for a visible top-level window of class `class`
/// whose process's executable is named `exe`, not among `before`.
#[must_use]
pub fn wait_window(class: &str, exe: &str, before: &[isize], limit: Duration) -> Option<isize> {
    let start = Instant::now();
    loop {
        let found = top_windows().into_iter().find(|hwnd| {
            !before.contains(hwnd)
                && class_of(*hwnd) == class
                && image_of(window_pid(*hwnd))
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .is_some_and(|name| name.eq_ignore_ascii_case(exe))
        });
        if found.is_some() || start.elapsed() >= limit {
            return found;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Brings window `hwnd` to the foreground, attaching to the foreground
/// window's thread so that Windows allows it; answers whether it is then
/// the foreground window.
#[must_use]
pub fn foreground(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: no precondition.
    let current = unsafe { GetForegroundWindow() };
    // SAFETY: a null process id pointer is allowed.
    let theirs = unsafe { GetWindowThreadProcessId(current, None) };
    // SAFETY: no precondition.
    let ours = unsafe { GetCurrentThreadId() };
    // SAFETY: two thread ids; attaching shares their input state.
    let attached = theirs != 0 && unsafe { AttachThreadInput(ours, theirs, true) }.as_bool();
    // SAFETY: any window handle.
    let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    // SAFETY: any window handle.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    if attached {
        // SAFETY: undoing the attachment above.
        let _ = unsafe { AttachThreadInput(ours, theirs, false) };
    }
    // SAFETY: no precondition.
    let now = unsafe { GetForegroundWindow() };
    now == hwnd
}

/// The foreground window.
#[must_use]
pub fn foreground_window() -> isize {
    // SAFETY: no precondition.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Presses keys: each `(virtual key, down)` in turn, as one input batch.
///
/// # Panics
///
/// Never: an `INPUT`'s size fits an `i32`.
pub fn keys(strokes: &[(u16, bool)]) {
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|(key, down)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(*key),
                    wScan: 0,
                    dwFlags: if *down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
        .collect();
    let size = i32::try_from(std::mem::size_of::<INPUT>()).expect("a small structure");
    // SAFETY: a live slice of keyboard inputs of the size given.
    let _ = unsafe { SendInput(&inputs, size) };
}

/// `VK_MENU`, Alt.
pub const ALT: u16 = 0x12;
/// `VK_TAB`.
pub const TAB: u16 = 0x09;
/// `VK_ESCAPE`.
pub const ESCAPE: u16 = 0x1B;
/// The F key.
pub const F: u16 = 0x46;

/// Alt+Tab: switches to the window behind the foreground one.
pub fn alt_tab() {
    keys(&[(ALT, true), (TAB, true), (TAB, false), (ALT, false)]);
}

/// Alt+F: opens an application's File menu.
pub fn alt_f() {
    keys(&[(ALT, true), (F, true), (F, false), (ALT, false)]);
}

/// Escape, pressed twice: closes a menu and leaves its menu bar.
pub fn escape() {
    keys(&[
        (ESCAPE, true),
        (ESCAPE, false),
        (ESCAPE, true),
        (ESCAPE, false),
    ]);
}

/// The path of an executable in the system folder, such as `notepad.exe`.
#[must_use]
pub fn system_program(name: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(name)
}
