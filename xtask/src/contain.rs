//! Every Windows command runs in a job of its own that ends everything in
//! it when the command's process exits, so a process a command starts,
//! and everything that process starts, never outlives the command,
//! whether it finishes, fails, panics or is killed: Windows does not end a
//! process's children with it, and a terminal left open by a failed
//! command once had every later Windows Terminal scenario's window join it
//! (2026-11-04). Cargo runs `cargo xtask` in such a job too, so stopping
//! cargo ends the command and all it started.

use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::PCWSTR;

/// Places this process in a new job that ends every process in it when
/// its last handle closes, and returns that handle. A process this one
/// starts afterwards is in the job from its start, as is everything it
/// starts in turn, such as a console host `start` runs, so when this
/// process exits, by any path, what it started ends with it; the
/// handle is closed then, or when the returned value is dropped, which
/// ends this process as well.
pub fn this_process() -> windows::core::Result<OwnedHandle> {
    // SAFETY: creates an unnamed job; the handle is owned below.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }?;
    // SAFETY: `job` was just returned open, and nothing else owns it.
    let job = unsafe { OwnedHandle::from_raw_handle(job.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `limits` outlives the call, and its size is passed with it.
    unsafe {
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(u32::MAX),
        )
    }?;
    // SAFETY: returns the pseudo-handle naming this process; no
    // precondition.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: the job is open, and the pseudo-handle names this process.
    unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), this_process) }?;
    Ok(job)
}
