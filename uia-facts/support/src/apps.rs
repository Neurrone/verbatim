//! Real applications and the desktop, for the facts that need them: the
//! foreground window and its class, ending a process, pressing keys, and
//! the system folder's programs. Every fact that uses these first checks
//! [`crate::precondition::unlocked_desktop`].

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

/// A window's class name.
#[must_use]
pub fn class_of(hwnd: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: any window handle; a live buffer.
    let length = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
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

/// Alt+Tab: switches to the window behind the foreground one.
pub fn alt_tab() {
    keys(&[(ALT, true), (TAB, true), (TAB, false), (ALT, false)]);
}

/// The path of an executable in the system folder, such as `notepad.exe`.
#[must_use]
pub fn system_program(name: &str) -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(name)
}
