//! A desktop of its own, as the original experiments ran on: no input
//! reaches it and no window there has the keyboard focus, and nothing
//! shown there appears on the user's screen. A process is started on it
//! through a trampoline: the fact binary started with
//! `--ondesk <desktop> <program> <arguments>` creates the desktop if it does
//! not exist, starts the program on it with its own standard handles, and
//! ends with the program's exit code. The program is in the trampoline's
//! job object, as every child process is, so closing the job kills both.

use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::StationsAndDesktops::{CreateDesktopW, DESKTOP_CONTROL_FLAGS};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// The trampoline's flag, which must come first on the command line.
pub const FLAG: &str = "--ondesk";

/// The arguments that run `program` with `args` on desktop `name` through
/// this executable's trampoline.
#[must_use]
pub fn wrap(name: &str, program: &std::path::Path, args: &[String]) -> Vec<String> {
    let mut words = vec![
        FLAG.to_owned(),
        name.to_owned(),
        program.display().to_string(),
    ];
    words.extend(args.iter().cloned());
    words
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Quotes one argument for a Windows command line.
fn quote(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            other => {
                quoted.push_str(&"\\".repeat(slashes));
                quoted.push(other);
                slashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

/// The trampoline: when this process was started with [`FLAG`], runs the
/// program it names on the desktop it names and ends with its exit code;
/// otherwise returns.
///
/// # Panics
///
/// When the desktop cannot be made or the program started.
pub fn trampoline_if_asked() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(FLAG) {
        return;
    }
    let (name, command) = (&args[1], &args[2..]);
    let wide_name = wide(name);
    // SAFETY: a live, terminated name; no device or mode; default
    // security. The handle stays open while the program runs, so the
    // desktop lasts as long.
    let _desktop = unsafe {
        CreateDesktopW(
            PCWSTR(wide_name.as_ptr()),
            PCWSTR::null(),
            None,
            DESKTOP_CONTROL_FLAGS(0),
            GENERIC_ALL.0,
            None,
        )
    }
    .expect("the desktop is created or opened");
    let mut path = wide(&format!("WinSta0\\{name}"));
    let line: Vec<String> = command.iter().map(|a| quote(a)).collect();
    let mut line = wide(&line.join(" "));
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).expect("a small structure"),
        lpDesktop: PWSTR(path.as_mut_ptr()),
        dwFlags: STARTF_USESTDHANDLES,
        ..Default::default()
    };
    // SAFETY: no precondition.
    startup.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) }.unwrap_or(HANDLE::default());
    // SAFETY: as above.
    startup.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap_or(HANDLE::default());
    // SAFETY: as above.
    startup.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.unwrap_or(HANDLE::default());
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: a live, terminated, writable command line, a startup
    // structure naming the desktop and this process's inheritable standard
    // handles, and a structure to fill.
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &raw const startup,
            &raw mut process,
        )
    }
    .expect("the program starts on the desktop");
    crate::note::trace(&format!(
        "on desktop {name}: process {}",
        process.dwProcessId
    ));
    // SAFETY: the process started above.
    let _ = unsafe { WaitForSingleObject(process.hProcess, INFINITE) };
    let mut code = 0u32;
    // SAFETY: as above, and a live value to fill.
    let _ = unsafe { GetExitCodeProcess(process.hProcess, &raw mut code) };
    crate::exit_now(code)
}
