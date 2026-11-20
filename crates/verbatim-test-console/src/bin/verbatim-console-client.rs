//! `verbatim-console-client <title> [<window title>]`: the client the test
//! console runs in its console host (`src/lib.rs`). With a window title, it
//! first makes a hidden top-level window with that title on a thread of its
//! own, another thread than the one Windows names as the console window's;
//! then it sets the console's title exactly as given and waits until the
//! console's job ends it.
//!
//! It is compiled alone by the build script, with no dependencies, so it
//! declares the few functions it calls itself;
//! `books/dev/src/reference/crates/verbatim-test-console.md` says why it
//! is native and how the library finds it.

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStrExt as _;
use std::process::ExitCode;
use std::sync::mpsc;

/// A message as `GetMessageW` writes it (`MSG`).
#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: [i32; 2],
    private: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleTitleW(title: *const u16) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *const c_void,
    ) -> *mut c_void;
    fn GetMessageW(msg: *mut Msg, hwnd: *mut c_void, first: u32, last: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
}

/// `text` as a nul-terminated wide string.
fn wide(text: &OsString) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// Makes the hidden window titled `title`, sends whether it was made, and
/// serves the window's messages until the process ends.
fn serve_window(title: &OsString, made: &mpsc::Sender<Result<(), u32>>) {
    let class = wide(&OsString::from("STATIC"));
    let title = wide(title);
    // SAFETY: a system class and a title, both nul-terminated and alive
    // across the call; no parent, menu or creation data. The style is
    // `WS_OVERLAPPED` without `WS_VISIBLE`: a hidden top-level window.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    let result = if window.is_null() {
        // SAFETY: no preconditions.
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    };
    let failed = result.is_err();
    // The main thread waits for this answer, so it is still there.
    let _ = made.send(result);
    if failed {
        return;
    }
    let mut msg = Msg {
        hwnd: std::ptr::null_mut(),
        message: 0,
        w_param: 0,
        l_param: 0,
        time: 0,
        point: [0; 2],
        private: 0,
    };
    // SAFETY: `msg` is a local each call writes; any window of this thread.
    while unsafe { GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: the message just retrieved.
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(title), window, None) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: verbatim-console-client <title> [<window title>]");
        return ExitCode::FAILURE;
    };
    if let Some(window) = window {
        let (made, answer) = mpsc::channel();
        std::thread::spawn(move || serve_window(&window, &made));
        match answer.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("verbatim-console-client: the window was not made: error {error}");
                return ExitCode::FAILURE;
            }
            Err(_) => {
                eprintln!("verbatim-console-client: the window's thread ended without an answer");
                return ExitCode::FAILURE;
            }
        }
    }
    let title = wide(&title);
    // SAFETY: a nul-terminated title alive across the call.
    if unsafe { SetConsoleTitleW(title.as_ptr()) } == 0 {
        // SAFETY: no preconditions.
        let error = unsafe { GetLastError() };
        eprintln!("verbatim-console-client: the title was not set: error {error}");
        return ExitCode::FAILURE;
    }
    // The console's job ends this process with its console host.
    loop {
        std::thread::park();
    }
}
