//! Process management on behalf of host-side E2E tests: launch, status,
//! and kill. Every launch goes through `CreateProcessW` from inside the
//! agent's own process, so a spawned Verbatim or Notepad inherits the
//! agent's interactive session — the reason this exists at all rather than
//! something reachable over `WinRM` or PowerShell Direct, both of which hand
//! a process a non-interactive window station that can never host a
//! screen reader test.
//!
//! Lookups and termination operate on raw OS pids, so a caller can query
//! or kill a process this agent did not itself spawn. The agent also keeps
//! the handle of every child it launched for as long as it keeps the
//! child's entry: without a handle, an exited process's object, and with it
//! the exit code, is gone the moment it exits, and the held handle keeps
//! the pid from being reused, so a kill of that pid never reaches another
//! process.
//!
//! Each child [`launch`] starts runs in a job object of its own, created
//! suspended and assigned before its first instruction, so everything it
//! starts is in the job too, and [`kill`] ends all of it. A launcher that
//! starts the real program as its own child, such as a Chocolatey shim, is
//! otherwise killed while the program it started keeps running. Every
//! process in the job is watched as it joins and exits ([`crate::jobs`]).
//!
//! Nothing here ends a process by its image name: a process is ended by
//! its id, and a launched child's by the handle the agent holds.

use std::collections::BTreeMap;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, STILL_ACTIVE, SetHandleInformation,
};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JobObjectBasicAccountingInformation, QueryInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetExitCodeProcess, OpenProcess,
    PROCESS_ACCESS_RIGHTS, PROCESS_INFORMATION, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, ResumeThread, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::{PCWSTR, PWSTR};

use crate::protocol::{KillOutcome, ProcessInfo, ProcessState};

/// Spawns `command` with `args`, and environment (extended, not replaced,
/// by `env`).
///
/// When `stderr_to` is `None`, the child is given no standard handles at
/// all, as a program a user starts from the shell is: the agent's own,
/// often redirected to a log, are never passed on. This matters for the
/// console host: `conhost.exe` started with standard handles takes them as
/// a pseudoconsole's input and output, opens no window, and exits when they
/// close, so an explicit `conhost.exe` would never show a console window.
/// When it is `Some(path)`, `path` is created (truncating any existing
/// content, so each launch starts its own fresh log) and the child's stdout
/// and stderr both go to it, so a panic message on stderr and any
/// surrounding stdout diagnostics land together in one combined,
/// chronologically ordered log rather than two.
///
/// The command line is built from `command` and `args` by the quoting rules
/// of the Microsoft C runtime, which `CommandLineToArgvW` and Rust's own
/// argument parsing follow; `command` is found as `CreateProcessW` finds a
/// program, adding `.exe` when it has no extension and searching `PATH`.
///
/// The process may take the foreground with its first window, as a program
/// a user starts may, when Windows lets the agent allow it
/// (`AllowSetForegroundWindow`): it does when the agent may set the
/// foreground itself, as the program that injected the last input may.
/// Nothing is injected to make that so. Returns the process's id and
/// whether it was allowed.
///
/// A console program's window is titled `console_title`, when given, from
/// its first frame. With `minimized`, the program's first window opens
/// minimized and inactive (`SW_SHOWMINNOACTIVE`), for a caller that brings
/// it forward itself once it is ready.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned (bad path, permission
/// denied, and so on), or if `stderr_to` is set and the capture file cannot
/// be created.
pub fn launch(
    command: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
    stderr_to: Option<&str>,
    console_title: Option<&str>,
    minimized: bool,
) -> io::Result<(u32, bool)> {
    let capture = stderr_to
        .map(|path| std::fs::File::create(path).and_then(|file| inheritable(&file)))
        .transpose()?;
    let mut command_line = wide(&command_line(command, args));
    let environment = (!env.is_empty()).then(|| environment_block(env));
    let directory = working_dir.map(wide);
    let mut title = console_title.map(wide);
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX),
        lpTitle: title
            .as_mut()
            .map_or(PWSTR::null(), |title| PWSTR(title.as_mut_ptr())),
        ..STARTUPINFOW::default()
    };
    if minimized {
        startup.dwFlags |= STARTF_USESHOWWINDOW;
        startup.wShowWindow =
            u16::try_from(windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE.0)
                .unwrap_or_default();
    }
    if let Some(capture) = &capture {
        startup.dwFlags |= STARTF_USESTDHANDLES;
        startup.hStdOutput = HANDLE(capture.as_raw_handle());
        startup.hStdError = HANDLE(capture.as_raw_handle());
    }
    let job = create_job()?;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer passed points into a buffer that outlives the
    // call: the command line is writable and nul-terminated, as the
    // directory and the console title are, and the environment block is
    // UTF-16 ending in two nuls, as `CREATE_UNICODE_ENVIRONMENT` declares. The capture handle,
    // when there is one, is open and inheritable.
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            capture.is_some(),
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ref().map(|block| block.as_ptr().cast()),
            directory
                .as_ref()
                .map_or(PCWSTR::null(), |directory| PCWSTR(directory.as_ptr())),
            &raw const startup,
            &raw mut info,
        )
    }
    .map_err(io::Error::other)?;
    // SAFETY: both handles were just returned by CreateProcessW, and are
    // owned here alone.
    let child = unsafe { OwnedHandle::from_raw_handle(info.hProcess.0) };
    // SAFETY: as above.
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread.0) };
    let pid = info.dwProcessId;
    // SAFETY: a plain call taking a process id; failure is its answer.
    let foreground_allowed = unsafe { AllowSetForegroundWindow(pid) }.is_ok();
    if let Err(error) = crate::jobs::watch_job(&job, pid)
        .and_then(|()| assign_to_job(&job, &child))
        .and_then(|()| resume(&thread))
    {
        crate::jobs::forget(pid);
        // SAFETY: `child` is open, with the full access CreateProcessW
        // grants.
        let _ = unsafe { TerminateProcess(HANDLE(child.as_raw_handle()), 1) };
        return Err(error);
    }
    let mut launched = LAUNCHED.lock().unwrap_or_else(PoisonError::into_inner);
    // An entry is no longer needed some time after its child has exited
    // and nothing it started is still running: kept, it would hold handles
    // for the life of the agent. The time is for its exit code to be asked
    // for.
    let now = Instant::now();
    launched.retain(|launched_pid, entry| {
        let finished =
            !job_has_processes(&entry.job) && already_exited(HANDLE(entry.child.as_raw_handle()));
        if !finished {
            return true;
        }
        let since = *entry.finished_at.get_or_insert(now);
        let kept = now.duration_since(since) < FINISHED_KEPT;
        if !kept {
            crate::jobs::forget(*launched_pid);
        }
        kept
    });
    launched.insert(
        pid,
        Launched {
            child,
            job,
            finished_at: None,
        },
    );
    Ok((pid, foreground_allowed))
}

/// `text` as UTF-16, nul-terminated.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An inheritable duplicate of `file`'s handle, for a child's standard
/// output and error.
fn inheritable(file: &std::fs::File) -> io::Result<OwnedHandle> {
    let handle = OwnedHandle::from(file.try_clone()?);
    // SAFETY: `handle` is open; only its inheritance flag changes.
    unsafe {
        SetHandleInformation(
            HANDLE(handle.as_raw_handle()),
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAG_INHERIT,
        )
    }
    .map_err(io::Error::other)?;
    Ok(handle)
}

/// The command line for `command` and `args`: the program quoted when it
/// holds a space or tab, and each argument quoted by the Microsoft C
/// runtime's rules, so the child parses back exactly `args`.
fn command_line(command: &str, args: &[String]) -> String {
    let mut line = if command.contains([' ', '\t']) {
        format!("\"{command}\"")
    } else {
        command.to_owned()
    };
    for arg in args {
        line.push(' ');
        quote_argument(arg, &mut line);
    }
    line
}

/// Appends `arg` to `line`, quoted when it is empty or holds a space, tab,
/// or quote: a quote is escaped with a backslash, and the backslashes
/// before a quote, or before the closing quote, are doubled.
fn quote_argument(arg: &str, line: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        line.push_str(arg);
        return;
    }
    line.push('"');
    let mut backslashes = 0;
    for character in arg.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        let escapes = if character == '"' {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        line.extend(std::iter::repeat_n('\\', escapes));
        backslashes = 0;
        line.push(character);
    }
    line.extend(std::iter::repeat_n('\\', backslashes * 2));
    line.push('"');
}

/// The agent's environment with `env` set over it, as a block for
/// `CreateProcessW`: `NAME=value` entries sorted by name without regard to
/// case, as Windows requires, each nul-terminated, and a final nul.
fn environment_block(env: &[(String, String)]) -> Vec<u16> {
    let mut variables: BTreeMap<String, (String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .map(|(name, value)| (name.to_uppercase(), (name, value)))
        .collect();
    for (name, value) in env {
        variables.insert(name.to_uppercase(), (name.clone(), value.clone()));
    }
    let mut block: Vec<u16> = Vec::new();
    for (name, value) in variables.values() {
        block.extend(format!("{name}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// A child [`launch`] started.
struct Launched {
    /// Its process handle, kept with the entry.
    child: OwnedHandle,
    /// The job holding it and everything it started.
    job: OwnedHandle,
    /// When a later launch first found the child exited and its job empty.
    finished_at: Option<Instant>,
}

/// How long an entry is kept after its child has exited and its job has
/// emptied, for its exit code to be asked for.
const FINISHED_KEPT: Duration = Duration::from_mins(5);

/// The children [`launch`] started, by pid.
static LAUNCHED: Mutex<BTreeMap<u32, Launched>> = Mutex::new(BTreeMap::new());

fn create_job() -> io::Result<OwnedHandle> {
    // SAFETY: no name and default security; the handle is owned below.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(io::Error::other)?;
    // SAFETY: `job` is a fresh handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(job.0) })
}

fn assign_to_job(job: &OwnedHandle, child: &OwnedHandle) -> io::Result<()> {
    // SAFETY: both handles are open for the duration of the call.
    unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle())) }
        .map_err(io::Error::other)
}

/// Resumes the only thread of a process created suspended.
fn resume(thread: &OwnedHandle) -> io::Result<()> {
    // SAFETY: `thread` is open, with the resume access CreateProcessW
    // grants.
    if unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } == u32::MAX {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Whether any process is still running in `job`.
fn job_has_processes(job: &OwnedHandle) -> bool {
    let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    // SAFETY: `info` is the structure this information class fills, and its
    // size is passed with it.
    unsafe {
        QueryInformationJobObject(
            Some(HANDLE(job.as_raw_handle())),
            JobObjectBasicAccountingInformation,
            (&raw mut info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                .unwrap_or(u32::MAX),
            None,
        )
    }
    .is_ok_and(|()| info.ActiveProcesses > 0)
}

/// Reports whether `pid` is running, and its exit code when it is not and
/// the code can be read.
///
/// A child [`launch`] started is answered from its kept handle, so its exit
/// code is reported for as long as the entry is kept (a later launch drops
/// entries finished more than five minutes earlier).
///
/// A pid that cannot be opened at all — already exited and its process
/// object gone, or one that never named a live process — is reported as
/// exited with no exit code, rather than an error: from a test's
/// perspective "I can no longer see this process" and "it exited" are the
/// same fact.
///
/// # Errors
///
/// Returns an error if the process can be opened but its exit code cannot
/// be read.
pub fn status(pid: u32) -> io::Result<ProcessState> {
    {
        let launched = LAUNCHED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = launched.get(&pid) {
            // The same reading as for any other process: `TerminateProcess`
            // sets the exit code before the process is signalled.
            return read_exit_code(HANDLE(entry.child.as_raw_handle()));
        }
    }
    let Some(handle) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return Ok(ProcessState::Exited { exit_code: None });
    };
    let result = read_exit_code(handle);
    close(handle);
    result
}

/// Terminates `pid`. For a child [`launch`] started, everything in its job
/// ends with it, including what it started, whatever their names; it is
/// reported [`KillOutcome::AlreadyExited`] when the child itself had already
/// exited, even if something it started was still running. Two distinct "already gone" cases are both reported
/// as [`KillOutcome::AlreadyExited`] rather than an error — the tolerance
/// the M2 harness needs when a test's own cleanup races the target's
/// normal exit: a pid that cannot be opened at all, and a pid that opens
/// fine but has already exited (Windows keeps a process object, and its
/// pid, alive as a zombie for a short window after exit as long as a
/// handle references it, and `TerminateProcess` on one of those fails
/// with access denied rather than succeeding or reporting "not found").
///
/// # Errors
///
/// Returns an error if the process can be opened, is still running, but
/// `TerminateProcess` itself fails for a reason other than the exit race
/// above.
pub fn kill(pid: u32) -> io::Result<KillOutcome> {
    {
        let launched = LAUNCHED.lock().unwrap_or_else(PoisonError::into_inner);
        // The entry's held handle keeps the pid from being reused, so the
        // entry is certainly this process's, and the pid is never opened
        // afresh while it exists.
        if let Some(entry) = launched.get(&pid) {
            // A child this agent launched: end it and everything it started.
            let running = !already_exited(HANDLE(entry.child.as_raw_handle()));
            if job_has_processes(&entry.job) {
                // SAFETY: the job handle stays open while the lock is held.
                unsafe { TerminateJobObject(HANDLE(entry.job.as_raw_handle()), 1) }
                    .map_err(io::Error::other)?;
            }
            return Ok(if running {
                KillOutcome::Terminated
            } else {
                KillOutcome::AlreadyExited
            });
        }
    }
    kill_opened(pid)
}

/// Terminates `pid`, which this agent did not launch, opening it afresh.
fn kill_opened(pid: u32) -> io::Result<KillOutcome> {
    let Some(handle) = open_process(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION)
    else {
        return Ok(KillOutcome::AlreadyExited);
    };
    if already_exited(handle) {
        close(handle);
        return Ok(KillOutcome::AlreadyExited);
    }
    // SAFETY: `handle` was just opened above with PROCESS_TERMINATE access
    // and is closed unconditionally below.
    let result = unsafe { TerminateProcess(handle, 1) };
    let outcome = match result {
        Ok(()) => Ok(KillOutcome::Terminated),
        Err(_) if already_exited(handle) => Ok(KillOutcome::AlreadyExited),
        Err(error) => Err(io::Error::other(error)),
    };
    close(handle);
    outcome
}

/// Decodes a `PROCESSENTRY32W::szExeFile` fixed-size, nul-terminated,
/// UTF-16 buffer into an owned `String`.
fn exe_file_name(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

/// The full path of the executable an open process handle's process runs,
/// `None` when it cannot be read.
fn image_path(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `handle` is open with query access; the buffer outlives the
    // call, which writes at most `length` units and updates it.
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(
        buffer.get(..usize::try_from(length).ok()?)?,
    ))
}

/// Whether an already-open process handle's process has exited, used to
/// distinguish the exit race documented on [`kill`] from a genuine
/// `TerminateProcess` failure.
fn already_exited(handle: HANDLE) -> bool {
    matches!(read_exit_code(handle), Ok(ProcessState::Exited { .. }))
}

/// Opens `pid` with `access`, returning `None` when the pid does not name
/// a live process rather than treating that as an error — the common case
/// callers here need to distinguish from a genuine failure.
fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<HANDLE> {
    // SAFETY: `pid` is a plain OS process id; the result is checked before
    // any use, and closed by every caller.
    unsafe { OpenProcess(access, false, pid) }.ok()
}

fn read_exit_code(handle: HANDLE) -> io::Result<ProcessState> {
    let mut code = 0u32;
    // SAFETY: `handle` is a valid, open process handle for the duration of
    // this call; `code` is a valid out-pointer.
    unsafe { GetExitCodeProcess(handle, &raw mut code) }.map_err(io::Error::other)?;
    let still_active =
        u32::try_from(STILL_ACTIVE.0).expect("STILL_ACTIVE is the small positive constant 259");
    if code == still_active {
        Ok(ProcessState::Running)
    } else {
        Ok(ProcessState::Exited {
            #[allow(
                clippy::cast_possible_wrap,
                reason = "exit codes are conventionally small; Windows itself defines them as DWORD but callers set them from an i32-shaped exit status"
            )]
            exit_code: Some(code as i32),
        })
    }
}

fn close(handle: HANDLE) {
    // SAFETY: `handle` was returned by a successful `OpenProcess` call
    // above and is not used again after this point.
    unsafe {
        let _ = CloseHandle(handle);
    }
}

/// The executable file name, such as `verbatim-outpost.exe`, of the process
/// an open handle names.
pub(crate) fn image_name_of(handle: &OwnedHandle) -> Option<String> {
    let path = image_path(HANDLE(handle.as_raw_handle()))?;
    path.rsplit(['\\', '/']).next().map(str::to_owned)
}

/// Whether the process an open handle names is running, or how it exited.
///
/// # Errors
///
/// Returns an error if its exit code cannot be read.
pub(crate) fn exit_state(handle: HANDLE) -> io::Result<ProcessState> {
    read_exit_code(handle)
}

/// Waits up to `timeout` for `pid` to exit, on its process handle, and
/// reports whether it is still running. A pid that names no process is
/// reported as exited.
///
/// # Errors
///
/// Returns an error if the wait fails, or the exit code cannot be read.
pub fn wait_for_exit(pid: u32, timeout: Duration) -> io::Result<ProcessState> {
    let Some(handle) = open_process(pid, PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)
    else {
        return status(pid);
    };
    let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: `handle` is open with SYNCHRONIZE access, and closed below.
    let waited = unsafe { WaitForSingleObject(handle, milliseconds) };
    close(handle);
    if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
        return Err(io::Error::last_os_error());
    }
    status(pid)
}

/// The processes whose parent is `pid`, with their image names.
///
/// # Errors
///
/// Returns an error if the system's process list cannot be read.
pub fn child_processes(pid: u32) -> io::Result<Vec<ProcessInfo>> {
    // SAFETY: snapshots every process; the handle is closed below.
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(io::Error::other)?;
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut children = Vec::new();
    // SAFETY: `snapshot` is open and `entry` has its size set.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
    while has_entry {
        if entry.th32ParentProcessID == pid {
            children.push(ProcessInfo {
                pid: entry.th32ProcessID,
                image: exe_file_name(&entry.szExeFile),
            });
        }
        // SAFETY: as above.
        has_entry = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
    }
    close(snapshot);
    Ok(children)
}

/// Ends every child the agent launched that is still running, with
/// everything in its job, and returns how many were running.
///
/// # Errors
///
/// Returns an error if a job cannot be terminated.
pub fn end_launched() -> io::Result<u32> {
    let launched = LAUNCHED.lock().unwrap_or_else(PoisonError::into_inner);
    let mut ended = 0u32;
    for entry in launched.values() {
        if job_has_processes(&entry.job) {
            if !already_exited(HANDLE(entry.child.as_raw_handle())) {
                ended += 1;
            }
            // SAFETY: the job handle stays open while the lock is held.
            unsafe { TerminateJobObject(HANDLE(entry.job.as_raw_handle()), 1) }
                .map_err(io::Error::other)?;
        }
    }
    Ok(ended)
}

/// Held for reading by every test that launches a child, and for writing
/// by the test that ends every launched child, so that test ends no other
/// test's child.
#[cfg(test)]
pub(crate) static LAUNCHING: std::sync::RwLock<()> = std::sync::RwLock::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest a test waits for a child to start or exit.
    const WAIT: Duration = Duration::from_secs(30);

    /// A child that runs until it is ended: PowerShell waiting for an event
    /// that never comes.
    fn long_running() -> u32 {
        launch(
            "powershell",
            &[
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "Wait-Event".to_owned(),
            ],
            None,
            &[],
            None,
            None,
            false,
        )
        .expect("spawns powershell")
        .0
    }

    /// Waits, on the job watcher's notifications, until `met` holds for the
    /// job of the launched child `pid`.
    fn wait_for_job(pid: u32, met: impl Fn(&crate::jobs::JobRecord) -> bool) {
        let jobs = crate::jobs::JOBS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (jobs, timeout) = crate::jobs::CHANGED
            .wait_timeout_while(jobs, WAIT, |jobs| !jobs.get(&pid).is_some_and(&met))
            .unwrap_or_else(PoisonError::into_inner);
        assert!(
            !timeout.timed_out(),
            "the job of {pid} did not change as expected: running {:?}, exits {:?}",
            jobs.get(&pid).map(|job| job
                .running
                .iter()
                .map(|(pid, (_, image))| (*pid, image.clone()))
                .collect::<Vec<_>>()),
            jobs.get(&pid).map(|job| job.exits.clone())
        );
    }

    #[test]
    fn launch_status_kill_status_lifecycle() {
        let _launching = crate::process::LAUNCHING
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let pid = long_running();
        assert_eq!(status(pid).expect("queries status"), ProcessState::Running);
        assert_eq!(
            kill(pid).expect("kills the process"),
            KillOutcome::Terminated
        );
        assert!(
            matches!(
                wait_for_exit(pid, WAIT).expect("waits"),
                ProcessState::Exited { .. }
            ),
            "the process exits once killed"
        );
        assert_eq!(kill(pid).expect("kills again"), KillOutcome::AlreadyExited);
    }

    #[test]
    fn launch_with_stderr_to_captures_the_childs_stderr() {
        let _launching = crate::process::LAUNCHING
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let path = std::env::temp_dir().join(format!(
            "verbatim-agent-test-stderr-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path_str = path.to_str().expect("utf8 temp path").to_owned();
        let (pid, _) = launch(
            "cmd",
            &[
                "/c".to_owned(),
                "echo agent stderr capture test 1>&2".to_owned(),
            ],
            None,
            &[],
            Some(&path_str),
            None,
            false,
        )
        .expect("spawns cmd with a stderr capture path");
        assert_eq!(
            wait_for_exit(pid, WAIT).expect("waits"),
            ProcessState::Exited { exit_code: Some(0) }
        );
        // Exactly what cmd's echo wrote: the text up to the redirection,
        // with the space before it, and a line break.
        let captured =
            std::fs::read_to_string(&path).expect("reads the captured stderr/stdout file");
        assert_eq!(captured, "agent stderr capture test \r\n");
        std::fs::remove_file(&path).expect("removes the capture file");
    }

    #[test]
    fn reports_the_exit_code_of_a_launched_child_after_it_exited() {
        let _launching = crate::process::LAUNCHING
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let (pid, _) = launch(
            "cmd",
            &["/C".to_owned(), "exit 3".to_owned()],
            None,
            &[],
            None,
            None,
            false,
        )
        .expect("spawns cmd");
        assert_eq!(
            wait_for_exit(pid, WAIT).expect("waits"),
            ProcessState::Exited { exit_code: Some(3) }
        );
    }

    #[test]
    fn killing_a_launched_child_ends_what_it_started_and_the_job_records_both_exits() {
        let _launching = crate::process::LAUNCHING
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let (pid, _) = launch(
            "cmd",
            &[
                "/C".to_owned(),
                "powershell -NoProfile -Command Wait-Event".to_owned(),
            ],
            None,
            &[],
            None,
            None,
            false,
        )
        .expect("spawns cmd");
        wait_for_job(pid, |job| job.running.len() == 2);
        let grandchild = child_processes(pid)
            .expect("lists children")
            .into_iter()
            .map(|child| child.pid)
            .next()
            .expect("cmd started powershell");
        assert_eq!(kill(pid).expect("kills"), KillOutcome::Terminated);
        assert!(matches!(
            wait_for_exit(grandchild, WAIT).expect("waits"),
            ProcessState::Exited { .. }
        ));
        wait_for_job(pid, |job| job.exits.len() == 2);
        let exits = crate::jobs::exits(pid).expect("the job is known");
        let mut images: Vec<&str> = exits.iter().map(|exit| exit.image.as_str()).collect();
        images.sort_unstable();
        assert_eq!(images, ["cmd.exe", "powershell.exe"]);
        assert!(
            exits
                .iter()
                .all(|exit| exit.exit_code == Some(1) && !exit.abnormal)
        );
    }

    #[test]
    fn ending_the_launched_children_ends_one_still_running() {
        let _launching = crate::process::LAUNCHING
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let pid = long_running();
        assert!(end_launched().expect("ends") >= 1);
        assert!(matches!(
            wait_for_exit(pid, WAIT).expect("waits"),
            ProcessState::Exited { .. }
        ));
    }

    #[test]
    fn arguments_are_quoted_by_the_c_runtime_s_rules() {
        let args = [
            "plain",
            "",
            "two words",
            r#"say "hi""#,
            r"C:\dir with space\",
            r"a\b",
        ]
        .map(str::to_owned);
        assert_eq!(
            command_line(r"C:\Program Files\x.exe", &args),
            r#""C:\Program Files\x.exe" plain "" "two words" "say \"hi\"" "C:\dir with space\\" a\b"#
        );
    }

    #[test]
    fn status_of_a_pid_that_never_existed_reports_exited() {
        let state = status(0xFFFF_FFF0).expect("querying an invalid pid is not an error");
        assert_eq!(state, ProcessState::Exited { exit_code: None });
    }

    #[test]
    fn kill_of_a_pid_that_never_existed_is_already_exited() {
        let outcome = kill(0xFFFF_FFF0).expect("killing an invalid pid is not an error");
        assert_eq!(outcome, KillOutcome::AlreadyExited);
    }
}
