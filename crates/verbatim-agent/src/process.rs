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
//! otherwise killed while the program it started keeps running.

use std::collections::BTreeMap;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tracing::warn;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, STILL_ACTIVE, SetHandleInformation,
};
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
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
};
use windows::core::{PCWSTR, PWSTR};

use crate::foreground::ForegroundNudge;
use crate::protocol::{KillOutcome, ProcessState};

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
/// The process may take the foreground with its first window; `nudge` says
/// whether a Control tap may be injected to allow that past the foreground
/// lock (`foreground::allow_foreground`).
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
    nudge: ForegroundNudge,
) -> io::Result<u32> {
    let capture = stderr_to
        .map(|path| std::fs::File::create(path).and_then(|file| inheritable(&file)))
        .transpose()?;
    let mut command_line = wide(&command_line(command, args));
    let environment = (!env.is_empty()).then(|| environment_block(env));
    let directory = working_dir.map(wide);
    let mut startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX),
        ..STARTUPINFOW::default()
    };
    if let Some(capture) = &capture {
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdOutput = HANDLE(capture.as_raw_handle());
        startup.hStdError = HANDLE(capture.as_raw_handle());
    }
    let job = create_job()?;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer passed points into a buffer that outlives the
    // call: the command line is writable and nul-terminated, as the
    // directory is, and the environment block is UTF-16 ending in two
    // nuls, as `CREATE_UNICODE_ENVIRONMENT` declares. The capture handle,
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
    crate::foreground::allow_foreground(pid, nudge);
    if let Err(error) = assign_to_job(&job, &child).and_then(|()| resume(&thread)) {
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
    launched.retain(|_, entry| {
        let finished =
            !job_has_processes(&entry.job) && already_exited(HANDLE(entry.child.as_raw_handle()));
        if !finished {
            return true;
        }
        let since = *entry.finished_at.get_or_insert(now);
        now.duration_since(since) < FINISHED_KEPT
    });
    launched.insert(
        pid,
        Launched {
            child,
            job,
            finished_at: None,
        },
    );
    Ok(pid)
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
    kill_as(pid, None)
}

/// [`kill`], except that a pid this agent did not launch is terminated only
/// when it still runs the executable `image` names, if given
/// ([`image_matches`]).
fn kill_as(pid: u32, image: Option<&str>) -> io::Result<KillOutcome> {
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
    kill_opened(pid, image)
}

/// Terminates `pid`, which this agent did not launch, opening it afresh;
/// when `image` is given, only if the opened process still runs the
/// executable it names, so a pid reused since it was looked up is left
/// alone.
fn kill_opened(pid: u32, image: Option<&str>) -> io::Result<KillOutcome> {
    let Some(handle) = open_process(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION)
    else {
        return Ok(KillOutcome::AlreadyExited);
    };
    if let Some(image) = image
        && !image_path(handle).is_some_and(|path| image_matches(&path, image))
    {
        close(handle);
        return Ok(KillOutcome::AlreadyExited);
    }
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

/// Terminates every currently running process whose image (executable file)
/// matches `name`, and returns how many were actually terminated. A bare
/// file name (`"notepad.exe"`) matches that file name wherever it runs
/// from; a full path (`r"C:\stage\mockapp.exe"`) matches only processes
/// running that very file, so a program the harness deploys is swept
/// without ending the same program run from elsewhere, such as the
/// `mockapp` a concurrent `cargo test` drives. Both compare
/// case-insensitively ([`image_matches`]). Zero is a normal, successful
/// outcome, not an error: it simply means no matching process was running.
///
/// Exists for the handoff case Windows 11 Notepad exhibits: launching it
/// when an instance already exists hands the window off to that existing
/// process and the newly launched one exits immediately, so a pid-based
/// kill (recorded from the launch that got handed off) can miss the
/// process actually holding the window. Sweeping by image name catches it
/// regardless of which launch's pid ended up owning it.
///
/// Individual per-pid kill failures (a genuine `TerminateProcess` error,
/// not the ordinary already-exited race [`kill`] already tolerates) are
/// logged and skipped rather than aborting the sweep — this is a
/// best-effort mass cleanup, and one uncooperative process should not stop
/// the rest from being cleaned up.
///
/// # Errors
///
/// Returns an error if the system process snapshot itself cannot be taken
/// or walked; a failure killing an individual matched process does not
/// propagate.
pub fn kill_by_name(name: &str) -> io::Result<u32> {
    let mut terminated = 0u32;
    // The snapshot holds file names only; a path is checked once each
    // match is opened.
    let file_name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    for pid in matching_pids(file_name)? {
        // Checked again once opened: the pid may have been reused since
        // the snapshot.
        match kill_as(pid, Some(name)) {
            Ok(KillOutcome::Terminated) => terminated += 1,
            Ok(KillOutcome::AlreadyExited) => {}
            Err(error) => {
                warn!(pid, name, %error, "failed to kill a process matched by name; skipping");
            }
        }
    }
    Ok(terminated)
}

/// Snapshots every running process and returns the pids whose image file
/// name matches `name`, case-insensitively.
pub(crate) fn matching_pids(name: &str) -> io::Result<Vec<u32>> {
    // SAFETY: `TH32CS_SNAPPROCESS` with a `th32ProcessID` of 0 snapshots
    // every process system-wide; the returned handle is checked below and
    // closed via `CloseHandle` before returning.
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(io::Error::other)?;

    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>())
            .expect("PROCESSENTRY32W's size fits in a u32"),
        ..Default::default()
    };
    let mut pids = Vec::new();
    // SAFETY: `snapshot` is a valid, just-created snapshot handle; `entry`
    // is zero-initialized with `dwSize` set as `Process32FirstW` requires.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
    while has_entry {
        if exe_file_name(&entry.szExeFile).eq_ignore_ascii_case(name) {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: `snapshot` and `entry` are the same valid values as above;
        // `Process32NextW` overwrites `entry` in place for the next process.
        has_entry = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
    }

    close(snapshot);
    Ok(pids)
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

/// Whether a process running the executable at `path` (a full path, as
/// Windows reports it) matches `image`: by its full path when `image` is
/// one, by its file name otherwise; case-insensitively either way, and
/// with either slash as the separator.
fn image_matches(path: &str, image: &str) -> bool {
    let normalize = |text: &str| text.replace('/', "\\").to_lowercase();
    let (path, image) = (normalize(path), normalize(image));
    if image.contains('\\') {
        path == image
    } else {
        path.rsplit('\\').next() == Some(image.as_str())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawns a long-running harmless child (`powershell Start-Sleep`),
    /// exercising launch, running status, kill, and exited status in
    /// sequence — the lifecycle host-side E2E tests depend on. Cleans up
    /// even on assertion failure by killing unconditionally at the end.
    #[test]
    fn launch_status_kill_status_lifecycle() {
        let pid = launch(
            "powershell",
            &[
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "Start-Sleep -Seconds 300".to_owned(),
            ],
            None,
            &[],
            None,
            ForegroundNudge::Never,
        )
        .expect("spawns powershell");
        assert!(pid > 0, "pid is a valid nonzero process id");

        let running = status(pid).expect("queries status");
        assert_eq!(running, ProcessState::Running);

        let outcome = kill(pid).expect("kills the process");
        assert_eq!(outcome, KillOutcome::Terminated);

        // GetExitCodeProcess can lag TerminateProcess by a moment; poll
        // briefly rather than asserting on the first read.
        let mut final_state = ProcessState::Running;
        for _ in 0..50 {
            final_state = status(pid).expect("queries status again");
            if final_state != ProcessState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            matches!(final_state, ProcessState::Exited { .. }),
            "process is reported exited after being killed, got {final_state:?}"
        );

        // Killing an already-exited process is tolerated, not an error.
        let second_kill = kill(pid).expect("kills a second time without error");
        assert_eq!(second_kill, KillOutcome::AlreadyExited);
    }

    /// Exercises `stderr_to`: a child that writes to stderr and exits, with
    /// stdout and stderr captured into a file that outlives the process.
    #[test]
    fn launch_with_stderr_to_captures_the_childs_stderr() {
        let path = std::env::temp_dir().join(format!(
            "verbatim-agent-test-stderr-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path_str = path.to_str().expect("utf8 temp path").to_owned();

        // cmd starts in milliseconds; PowerShell's cold start took more
        // than two seconds on GitHub's ARM64 runner.
        let pid = launch(
            "cmd",
            &[
                "/c".to_owned(),
                "echo agent stderr capture test 1>&2".to_owned(),
            ],
            None,
            &[],
            Some(&path_str),
            ForegroundNudge::Never,
        )
        .expect("spawns cmd with a stderr capture path");

        // Waits for the exit, however long the machine takes to start the
        // child; the test is about the capture, not the start-up time.
        let mut final_state = ProcessState::Running;
        for _ in 0..1500 {
            final_state = status(pid).expect("queries status");
            if final_state != ProcessState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            matches!(final_state, ProcessState::Exited { .. }),
            "expected the child to have exited, got {final_state:?}"
        );

        let captured =
            std::fs::read_to_string(&path).expect("reads the captured stderr/stdout file");
        assert!(
            captured.contains("agent stderr capture test"),
            "expected the captured file to contain the child's stderr, got: {captured}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_path_matches_only_that_file_and_a_name_matches_it_anywhere() {
        let running = r"C:\Stage\mockapp.exe";
        assert!(image_matches(running, "MOCKAPP.EXE"));
        assert!(image_matches(running, r"c:\stage\mockapp.exe"));
        assert!(image_matches(running, "C:/Stage/mockapp.exe"));
        assert!(!image_matches(running, r"C:\target\debug\mockapp.exe"));
        assert!(!image_matches(running, "notepad.exe"));
    }

    #[test]
    fn reports_the_exit_code_of_a_launched_child_after_it_exited() {
        let pid = launch(
            "cmd",
            &["/C".to_owned(), "exit 3".to_owned()],
            None,
            &[],
            None,
            ForegroundNudge::Never,
        )
        .expect("spawns cmd");
        // Waits for the exit, however long the machine takes to start the
        // child, as above.
        let mut state = ProcessState::Running;
        for _ in 0..1500 {
            state = status(pid).expect("queries status");
            if state != ProcessState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(state, ProcessState::Exited { exit_code: Some(3) });
    }

    #[test]
    fn killing_a_launched_child_ends_what_it_started() {
        let pid = launch(
            "cmd",
            &["/C".to_owned(), "ping -n 60 127.0.0.1 >nul".to_owned()],
            None,
            &[],
            None,
            ForegroundNudge::Never,
        )
        .expect("spawns cmd");
        let mut grandchild = None;
        for _ in 0..100 {
            grandchild = children_of(pid).into_iter().next();
            if grandchild.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let grandchild = grandchild.expect("cmd starts ping");

        assert_eq!(kill(pid).expect("kills"), KillOutcome::Terminated);
        let mut state = ProcessState::Running;
        for _ in 0..100 {
            state = status(grandchild).expect("queries ping");
            if state != ProcessState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            matches!(state, ProcessState::Exited { .. }),
            "ping, started by the killed cmd, still runs"
        );
    }

    /// The processes whose parent is `pid`.
    fn children_of(pid: u32) -> Vec<u32> {
        // SAFETY: CreateToolhelp32Snapshot has no preconditions.
        let snapshot =
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.expect("snapshot");
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap(),
            ..PROCESSENTRY32W::default()
        };
        let mut children = Vec::new();
        // SAFETY: `snapshot` is open and `entry` has its size set.
        let mut has_entry = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
        while has_entry {
            if entry.th32ParentProcessID == pid {
                children.push(entry.th32ProcessID);
            }
            // SAFETY: as above.
            has_entry = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
        }
        close(snapshot);
        children
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
        // A pid this unlikely to be live keeps the test independent of
        // whatever else is running on the machine.
        let state = status(0xFFFF_FFF0).expect("querying an invalid pid is not an error");
        assert_eq!(state, ProcessState::Exited { exit_code: None });
    }

    #[test]
    fn kill_of_a_pid_that_never_existed_is_already_exited() {
        let outcome = kill(0xFFFF_FFF0).expect("killing an invalid pid is not an error");
        assert_eq!(outcome, KillOutcome::AlreadyExited);
    }

    #[test]
    fn kill_by_name_of_an_unmatched_name_returns_zero() {
        let terminated = kill_by_name("verbatim-agent-test-nonexistent-image-name.exe")
            .expect("querying an unmatched name is not an error");
        assert_eq!(terminated, 0);
    }

    /// Windows identifies a process's image name from the executable file
    /// itself, not the command line, so a uniquely named copy of
    /// `powershell.exe` lets this test match by name deterministically,
    /// with no risk of also catching an unrelated `powershell.exe` already
    /// running on the machine this test happens to run on. `Start-Sleep` is
    /// a PowerShell cmdlet, not a separately resolved executable, so unlike
    /// an external command name it cannot be shadowed by a same-named tool
    /// earlier on `PATH` (confirmed live: an earlier attempt using a
    /// renamed `cmd.exe` running `timeout /t 300` failed exactly that way,
    /// resolving to a Git-Bash-provided `timeout` with an incompatible
    /// argument syntax instead of Windows' own).
    #[test]
    fn kill_by_name_terminates_every_matching_process() {
        let unique_name = format!(
            "verbatim-agent-test-killbyname-{}-{:?}.exe",
            std::process::id(),
            std::thread::current().id()
        );
        let exe_path = std::env::temp_dir().join(&unique_name);
        std::fs::copy(
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            &exe_path,
        )
        .expect("copies powershell.exe under a unique name");

        let pid = launch(
            exe_path.to_str().expect("utf8 path"),
            &[
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "Start-Sleep -Seconds 300".to_owned(),
            ],
            None,
            &[],
            None,
            ForegroundNudge::Never,
        )
        .expect("spawns the renamed powershell.exe");

        // `launch` returns once `CreateProcessW` has, by which time the
        // process is in every later snapshot.
        let terminated = kill_by_name(&unique_name).expect("kills by name");
        assert_eq!(
            terminated, 1,
            "expected exactly the one process spawned under this unique name"
        );

        let mut final_state = ProcessState::Running;
        for _ in 0..50 {
            final_state = status(pid).expect("queries status");
            if final_state != ProcessState::Running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            matches!(final_state, ProcessState::Exited { .. }),
            "process is reported exited after kill_by_name, got {final_state:?}"
        );

        std::fs::remove_file(&exe_path).ok();
    }
}
