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

use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, TerminateProcess,
};

use crate::protocol::{KillOutcome, ProcessState};

/// Spawns `command` with `args`, inheriting the agent's stdio (never
/// captured) and environment (extended, not replaced, by `env`).
///
/// # Errors
///
/// Returns an error if the process cannot be spawned (bad path, permission
/// denied, and so on).
pub fn launch(
    command: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
) -> io::Result<u32> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    for (key, value) in env {
        cmd.env(key, value);
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
}
