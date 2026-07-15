//! Process management on behalf of host-side E2E tests: launch, status,
//! and kill. Every launch goes through `std::process::Command` from inside
//! the agent's own process, so a spawned Verbatim or Notepad inherits the
//! agent's interactive session — the reason this exists at all rather than
//! something reachable over `WinRM` or PowerShell Direct, both of which hand
//! a process a non-interactive window station that can never host a
//! screen reader test.
//!
//! Lookups and termination operate on raw OS pids rather than any handle
//! kept from [`launch`], so a caller can query or kill a process this
//! agent did not itself spawn.

use std::io;
use std::process::Command;

use tracing::warn;
use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, TerminateProcess,
};

use crate::protocol::{KillOutcome, ProcessState};

/// Spawns `command` with `args`, and environment (extended, not replaced,
/// by `env`).
///
/// When `stderr_to` is `None`, stdio is inherited from the agent exactly as
/// before (never captured). When it is `Some(path)`, `path` is created
/// (truncating any existing content, so each launch starts its own fresh
/// log) and the child's stderr is redirected into it; stdout is redirected
/// to the same file too, via a cloned handle — a single `File` cannot back
/// two separate `Stdio` conversions, since each takes ownership of it — so
/// a panic message on stderr and any surrounding stdout diagnostics land
/// together in one combined, chronologically ordered log rather than two.
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
) -> io::Result<u32> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(path) = stderr_to {
        let capture_file = std::fs::File::create(path)?;
        let stdout_handle = capture_file.try_clone()?;
        cmd.stderr(capture_file);
        cmd.stdout(stdout_handle);
    }
    // The child keeps running independently of this handle; dropping it
    // only releases our reference, it does not terminate the process.
    let child = cmd.spawn()?;
    Ok(child.id())
}

/// Reports whether `pid` is running, and its exit code when it is not and
/// the code can be read.
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
    let Some(handle) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return Ok(ProcessState::Exited { exit_code: None });
    };
    let result = read_exit_code(handle);
    close(handle);
    result
}

/// Terminates `pid`. Two distinct "already gone" cases are both reported
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

/// Terminates every currently running process whose image (executable file)
/// name matches `name` (case-insensitive, comparing only the file name —
/// `"notepad.exe"`, never a full path), and returns how many were actually
/// terminated. Zero is a normal, successful outcome, not an error: it
/// simply means no matching process was running.
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
    for pid in matching_pids(name)? {
        match kill(pid) {
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
fn matching_pids(name: &str) -> io::Result<Vec<u32>> {
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

        let pid = launch(
            "powershell",
            &[
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "[Console]::Error.WriteLine('agent stderr capture test')".to_owned(),
            ],
            None,
            &[],
            Some(&path_str),
        )
        .expect("spawns powershell with a stderr capture path");

        let mut final_state = ProcessState::Running;
        for _ in 0..100 {
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
        )
        .expect("spawns the renamed powershell.exe");

        // Give the OS a moment to register the process in the snapshot
        // kill_by_name walks, before this test's own kill_by_name call
        // races the snapshot against a process that only just started.
        std::thread::sleep(std::time::Duration::from_millis(200));

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
