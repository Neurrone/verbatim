//! A console host started directly, as the console facts start it:
//! `conhost.exe cmd.exe`, with no standard handles of the starting
//! process's, so that it opens a console window of its own on the starting
//! process's desktop rather than serving a pipe. It is in the starting
//! process's job object, as every child is, so it ends with it.
//!
//! A console window answers `GetWindowThreadProcessId` with its client's
//! process (`cmd.exe`), not the console host's, so its window is found by
//! that process and the console host's own thread for it is found among
//! the console host's threads.

use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::EnumThreadWindows;
use windows::core::{BOOL, PCWSTR, PWSTR};

/// A console host and its window.
pub struct Console {
    process: HANDLE,
    /// The console host's process id.
    pub pid: u32,
    /// Its console window.
    pub hwnd: isize,
    /// The console host's thread that owns the window.
    pub thread: u32,
}

impl Console {
    /// Starts `conhost.exe cmd.exe` and waits up to 10 seconds for its
    /// window.
    ///
    /// # Panics
    ///
    /// When it does not start or its window does not appear.
    #[must_use]
    pub fn start() -> Self {
        let path = crate::apps::system_program("conhost.exe");
        let before = crate::apps::top_windows();
        let mut line: Vec<u16> = format!("\"{}\" cmd.exe", path.display())
            .encode_utf16()
            .chain([0])
            .collect();
        let startup = STARTUPINFOW {
            cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).expect("a small structure"),
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: a live, terminated, writable command line, a startup
        // structure with no handles, and a structure to fill.
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NEW_CONSOLE,
                None,
                PCWSTR::null(),
                &raw const startup,
                &raw mut process,
            )
        }
        .expect("conhost starts");
        let hwnd = crate::apps::wait_window(
            "ConsoleWindowClass",
            "cmd.exe",
            &before,
            Duration::from_secs(10),
        )
        .expect("the console window appears");
        let thread = window_thread(process.dwProcessId, hwnd)
            .expect("one of the console host's threads owns the window");
        Self {
            process: process.hProcess,
            pid: process.dwProcessId,
            hwnd,
            thread,
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: the process started above; cmd.exe ends with its console.
        let _ = unsafe { TerminateProcess(self.process, 0) };
    }
}

/// The thread of process `pid` that owns window `hwnd`.
fn window_thread(pid: u32, hwnd: isize) -> Option<u32> {
    extern "system" fn each(window: HWND, wanted: LPARAM) -> BOOL {
        // SAFETY: `wanted` points at the pair passed below, live for the
        // call.
        let wanted = unsafe { &mut *(wanted.0 as *mut (isize, bool)) };
        if window.0 as isize == wanted.0 {
            wanted.1 = true;
        }
        true.into()
    }
    // SAFETY: no precondition; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.ok()?;
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>()).expect("a small structure"),
        ..Default::default()
    };
    let mut found = None;
    // SAFETY: a live snapshot and entry.
    let mut more = unsafe { Thread32First(snapshot, &raw mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid {
            let mut wanted = (hwnd, false);
            // SAFETY: the callback above and a pointer to `wanted`, live
            // for the call.
            let _ = unsafe {
                EnumThreadWindows(
                    entry.th32ThreadID,
                    Some(each),
                    LPARAM(&raw mut wanted as isize),
                )
            };
            if wanted.1 {
                found = Some(entry.th32ThreadID);
                break;
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the snapshot opened above.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(snapshot) };
    found
}
