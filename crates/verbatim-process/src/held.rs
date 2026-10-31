//! Processes held open by handle, so that a pid names one process and no
//! other for as long as it is held: Windows never gives a process's id to
//! another process while a handle to it is open. The supervisor holds its
//! target applications ([`Target`]), and the supervisor, the focus listener
//! and the outposts hold the processes Verbatim was told to ignore
//! ([`IgnoredProcesses`]).
//!
//! Processes Verbatim was told to ignore entirely: the end-to-end harness
//! names the owner's own Windows Terminal, which must never be read while a
//! test runs on the owner's desktop, since it once crashed inside UI
//! Automation while the suite read it (`docs/tooling/e2e.md`). Each is held
//! open from the moment it is named, so
//! its pid names that process and no other for as long as Verbatim runs,
//! and a process that later reuses the pid of one that exited is never
//! ignored by mistake.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;

use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::core::PWSTR;

use verbatim_model::Pid;

/// A target application's process, held open for as long as the supervisor
/// has an outpost for it or remembers its crashes. Windows never gives a
/// process's id to another process while a handle to it is open, so the
/// pid the supervisor knows the target by, and passes to its outpost, names
/// this process and no other for as long as this is held. An outpost holds
/// the processes it names applications by the same way.
#[derive(Debug)]
pub struct Target {
    process: OwnedHandle,
}

/// Why a target was not opened.
#[derive(Debug)]
pub enum NotHeld {
    /// No process has that id, or the one that has it has exited.
    Exited,
    /// The process could not be opened, so it cannot be held, and an
    /// outpost for it could not tell its exit from another process's start.
    Denied(windows::core::Error),
}

impl std::fmt::Display for NotHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotHeld::Exited => f.write_str("the process has exited"),
            NotHeld::Denied(error) => write!(f, "the process cannot be opened: {error}"),
        }
    }
}

impl Target {
    /// Opens the process `pid` names now, if it is running. It is opened to
    /// be waited on, or, when that is refused, for limited queries only;
    /// either right is enough to hold it and to tell whether it has exited.
    ///
    /// # Errors
    ///
    /// [`NotHeld`] when the process has exited or cannot be opened.
    pub fn open(pid: Pid) -> Result<Self, NotHeld> {
        let mut opened = Err(NotHeld::Exited);
        for rights in [
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION,
        ] {
            // SAFETY: OpenProcess with these rights fails safely on an
            // invalid or inaccessible pid; a handle it returns is owned
            // below.
            match unsafe { OpenProcess(rights, false, pid.0) } {
                Ok(handle) => {
                    opened = Ok(handle);
                    break;
                }
                Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => {
                    return Err(NotHeld::Exited);
                }
                Err(error) => opened = Err(NotHeld::Denied(error)),
            }
        }
        let handle = opened?;
        // SAFETY: the handle was just returned by OpenProcess, is valid,
        // and is owned by nothing else.
        let process = unsafe { OwnedHandle::from_raw_handle(handle.0 as RawHandle) };
        let target = Self { process };
        if target.has_exited() {
            return Err(NotHeld::Exited);
        }
        Ok(target)
    }

    /// The process's executable name without its extension, lower-cased,
    /// as [`stem_of`] reads it. `None` when it cannot be read.
    #[must_use]
    pub fn image_stem(&self) -> Option<String> {
        stem_of(HANDLE(self.process.as_raw_handle()))
    }

    /// Whether the process has exited.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        let handle = HANDLE(self.process.as_raw_handle());
        let mut exit_code = 0u32;
        // SAFETY: the handle is open with at least limited query access;
        // `exit_code` is a local.
        let running = unsafe { GetExitCodeProcess(handle, &raw mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0.cast_unsigned();
        if !running {
            return true;
        }
        // A process can end with STILL_ACTIVE as its exit code; a handle that
        // can be waited on tells for certain.
        // SAFETY: a zero-timeout wait on an open handle; one opened without
        // SYNCHRONIZE fails the wait, and the exit code above stands.
        let waited = unsafe { WaitForSingleObject(handle, 0) };
        waited == WAIT_OBJECT_0
    }
}

/// The executable name without its extension, lower-cased, of the process
/// `handle`, open with at least limited query access.
#[must_use]
pub fn stem_of(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `handle` is open with query access; the buffer outlives the
    // call, which writes at most `length` units.
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
}

/// The environment variable the harness sets, before Verbatim starts, to
/// the pids of the processes to ignore, separated by commas. Real users
/// never set it, so for them nothing is ignored.
pub const IGNORE_PIDS_ENV: &str = "VERBATIM_IGNORE_PIDS";

/// Processes to ignore entirely: no fact from them is routed, no outpost is
/// started for them, and Core never takes them as its focus or attention.
#[derive(Debug, Default)]
pub struct IgnoredProcesses {
    held: Vec<(Pid, Target)>,
}

impl IgnoredProcesses {
    /// Holds each process `pids` names that is running now; one that has
    /// exited, or cannot be opened, is left out and logged, as it can
    /// raise no event.
    #[must_use]
    pub fn hold(pids: impl IntoIterator<Item = Pid>) -> Self {
        let mut held: Vec<(Pid, Target)> = Vec::new();
        for pid in pids {
            if held.iter().any(|(known, _)| *known == pid) {
                continue;
            }
            match Target::open(pid) {
                Ok(target) => held.push((pid, target)),
                Err(not_held) => {
                    tracing::info!(%pid, %not_held, "a process to ignore is not held");
                }
            }
        }
        if !held.is_empty() {
            let pids: Vec<u32> = held.iter().map(|(pid, _)| pid.0).collect();
            tracing::info!(?pids, "these processes are ignored entirely");
        }
        Self { held }
    }

    /// The processes [`IGNORE_PIDS_ENV`] names in this process's
    /// environment, held; none when it is unset.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var(IGNORE_PIDS_ENV) {
            Ok(list) => Self::hold(parse_pids(&list)),
            Err(_) => Self::default(),
        }
    }

    /// Whether `pid` names an ignored process.
    #[must_use]
    pub fn contains(&self, pid: Pid) -> bool {
        self.held.iter().any(|(held, _)| *held == pid)
    }

    /// Whether nothing is ignored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The ignored pids, as the listener's command line takes them:
    /// separated by commas.
    #[must_use]
    pub fn list(&self) -> String {
        self.held
            .iter()
            .map(|(pid, _)| pid.0.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The pids in `list`, separated by commas; anything that is not a pid is
/// skipped.
#[must_use]
pub fn parse_pids(list: &str) -> Vec<Pid> {
    list.split(',')
        .filter_map(|pid| pid.trim().parse().ok())
        .filter(|&pid| pid != 0)
        .map(Pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_pids_parses_and_skips_what_is_not_one() {
        assert_eq!(parse_pids(" 12,x,,0, 345 "), vec![Pid(12), Pid(345)]);
    }

    #[test]
    fn this_process_is_held_and_ignored_and_others_are_not() {
        let own = Pid(std::process::id());
        let ignored = IgnoredProcesses::hold([own, own]);
        assert!(ignored.contains(own));
        assert_eq!(ignored.list(), own.0.to_string());
        assert!(!ignored.contains(Pid(own.0 + 4)));
    }
}
