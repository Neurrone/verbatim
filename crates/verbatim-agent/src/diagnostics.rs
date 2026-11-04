//! [`Request::ProcessDiagnostics`](crate::protocol::Request::ProcessDiagnostics):
//! what a launched process was doing when the window it was to open did
//! not appear in time, beside its threads' wait chains
//! ([`crate::wait_chain`]): the processor time it has spent, which tells a
//! process busy working from one blocked or idle; the modules it has
//! loaded, which tell, for a Windows Forms window, whether
//! `System.Windows.Forms` was loaded from its precompiled native image
//! (`.ni.dll`) or compiled as it loaded (`.dll`); and a minidump with its
//! threads' information, for a debugger.

use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_VM_READ,
};
use windows::core::{BOOL, s, w};

use crate::process;
use crate::protocol::{ProcessDiagnostics, ProcessState};
use crate::wait_chain::answer_within;

/// How long [`diagnostics_within`] waits for the reading: under the
/// harness's 30-second bound on a reply, as for the wait chains.
pub const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(10);

/// The minidump's contents: the threads with their stacks, as every
/// minidump has, and each thread's state and times
/// (`MiniDumpWithThreadInfo`), the handles open (`MiniDumpWithHandleData`),
/// and the modules unloaded (`MiniDumpWithUnloadedModules`).
const DUMP_TYPE: u32 = 0x1000 | 0x4 | 0x20;

/// `MiniDumpWriteDump`'s signature, as `dbghelp.dll` exports it.
type MiniDumpWriteDump = unsafe extern "system" fn(
    HANDLE,
    u32,
    HANDLE,
    u32,
    *const c_void,
    *const c_void,
    *const c_void,
) -> BOOL;

/// Held while a minidump is written: `dbghelp.dll`'s functions are not
/// safe to call from two threads at once.
static DUMPING: Mutex<()> = Mutex::new(());

/// [`diagnostics`], read on a thread of its own and answered within
/// `timeout`, so the connection's thread is never held longer: writing a
/// minidump reads the process's memory, which a process being ended can
/// hold up.
///
/// # Errors
///
/// Returns the reason when the thread cannot be started or the reading
/// does not finish within `timeout`.
pub fn diagnostics_within(
    pid: u32,
    dump_path: String,
    timeout: Duration,
) -> Result<ProcessDiagnostics, String> {
    answer_within("the process's diagnostics", timeout, move || {
        diagnostics(pid, &dump_path)
    })
}

/// Process `pid`'s state, processor time and modules, with a minidump
/// written to `dump_path`; each part that cannot be read says why.
#[must_use]
pub fn diagnostics(pid: u32, dump_path: &str) -> ProcessDiagnostics {
    let state = process::status(pid).unwrap_or(ProcessState::Exited { exit_code: None });
    let (user_ms, kernel_ms) = processor_time(pid).unzip();
    let (modules, modules_error) = match modules(pid) {
        Ok(modules) => (modules, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let dump_error = write_dump(pid, dump_path).err();
    ProcessDiagnostics {
        state,
        user_ms,
        kernel_ms,
        modules,
        modules_error,
        dump_error,
    }
}

/// The processor time `pid` has spent in user and in kernel mode, in
/// milliseconds; `None` when it cannot be opened or read.
fn processor_time(pid: u32) -> Option<(u64, u64)> {
    // SAFETY: opens the process for querying; closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `handle` is open with query access; the out-parameters are
    // locals.
    let read = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    read.ok()?;
    // Filetime ticks are 100 nanoseconds.
    let milliseconds = |time: FILETIME| {
        ((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)) / 10_000
    };
    Some((milliseconds(user), milliseconds(kernel)))
}

/// The full path of each module `pid` has loaded, in load order.
///
/// # Errors
///
/// Returns why they cannot be read: the process has exited, cannot be
/// opened, or was loading a module as the list was taken.
fn modules(pid: u32) -> Result<Vec<String>, String> {
    // SAFETY: snapshots the process's modules; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) }
        .map_err(|error| format!("the modules could not be listed: {error}"))?;
    let mut entry = MODULEENTRY32W {
        dwSize: u32::try_from(size_of::<MODULEENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut modules = Vec::new();
    // SAFETY: `snapshot` is open and `entry` has its size set.
    let mut has_entry = unsafe { Module32FirstW(snapshot, &raw mut entry) }.is_ok();
    while has_entry {
        let length = entry
            .szExePath
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExePath.len());
        modules.push(String::from_utf16_lossy(&entry.szExePath[..length]));
        // SAFETY: as above.
        has_entry = unsafe { Module32NextW(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the snapshot opened above, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    Ok(modules)
}

/// Writes a minidump of `pid` to `path` ([`DUMP_TYPE`]).
///
/// # Errors
///
/// Returns why it could not be written.
fn write_dump(pid: u32, path: &str) -> Result<(), String> {
    let write = dump_writer()?;
    let file = std::fs::File::create(path)
        .map_err(|error| format!("the minidump's file {path} could not be made: {error}"))?;
    // SAFETY: opens the process for reading its memory; closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
        .map_err(|error| format!("the process could not be opened: {error}"))?;
    let written = {
        let _dumping = DUMPING.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: `process` is open with the access the call needs, the
        // file's handle is open for writing for the call, and the three
        // optional parameters are null.
        unsafe {
            write(
                process,
                pid,
                HANDLE(file.as_raw_handle()),
                DUMP_TYPE,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        }
    };
    let error = std::io::Error::last_os_error();
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(process) };
    if written.as_bool() {
        Ok(())
    } else {
        Err(format!("the minidump could not be written: {error}"))
    }
}

/// `MiniDumpWriteDump`, from `dbghelp.dll` in the system folder, loaded
/// once and kept loaded for the process's life.
///
/// # Errors
///
/// Returns why it could not be found.
fn dump_writer() -> Result<MiniDumpWriteDump, String> {
    // SAFETY: loads a system library from the system folder; it stays
    // loaded for the process's life, as the function pointer needs.
    let dbghelp = unsafe { LoadLibraryExW(w!("dbghelp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|error| format!("dbghelp.dll could not be loaded: {error}"))?;
    // SAFETY: looks up an export of the module just loaded.
    let found = unsafe { GetProcAddress(dbghelp, s!("MiniDumpWriteDump")) }
        .ok_or_else(|| "dbghelp.dll has no MiniDumpWriteDump".to_owned())?;
    // SAFETY: the export has exactly the signature `MiniDumpWriteDump`
    // names, as its documentation gives it.
    Ok(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, MiniDumpWriteDump>(found)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent's own process, read as a launched one is: it runs, has
    /// spent processor time, has its own executable as its first module,
    /// and its minidump is written, beginning with the minidump signature.
    #[test]
    fn a_running_process_is_read_and_dumped() {
        let dir = std::env::temp_dir().join(format!("verbatim-agent-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("makes the folder");
        let path = dir.join("self.dmp");
        let read = diagnostics(
            std::process::id(),
            path.to_str().expect("a UTF-8 temporary path"),
        );
        let exe = std::env::current_exe().expect("this executable's path");
        let dump = std::fs::read(&path).expect("reads the minidump");
        std::fs::remove_dir_all(&dir).expect("removes the folder");
        assert_eq!(read.state, ProcessState::Running);
        assert!(read.user_ms.is_some(), "user time read");
        assert!(read.kernel_ms.is_some(), "kernel time read");
        assert_eq!(
            read.modules.first().map(std::path::PathBuf::from),
            Some(exe),
            "the executable is the first module"
        );
        assert_eq!(read.modules_error, None);
        assert_eq!(read.dump_error, None);
        assert_eq!(&dump[..4], b"MDMP", "a minidump");
    }
}
