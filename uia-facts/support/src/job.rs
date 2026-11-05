//! A job object that kills its processes when it closes, so that a fact
//! binary that ends, or panics, leaves no provider or second client
//! running.

use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

/// A job object holding one process; closing it kills the process.
pub struct Job(HANDLE);

// SAFETY: a job object handle may be used and closed from any thread.
unsafe impl Send for Job {}
// SAFETY: as above; the handle is only read.
unsafe impl Sync for Job {}

impl Job {
    /// Puts `child` in a new job that kills it when the job closes.
    ///
    /// # Panics
    ///
    /// When the job cannot be made or the process assigned to it.
    #[must_use]
    pub fn holding(child: &Child) -> Self {
        // SAFETY: an unnamed job with default security.
        let job = unsafe { CreateJobObjectW(None, None) }.expect("a job object");
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job made above and a live structure of the class's
        // type and size.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(std::mem::size_of_val(&limits)).expect("a small structure"),
            )
        }
        .expect("the job's limits");
        let process = HANDLE(child.as_raw_handle());
        // SAFETY: the job above and the child's live process handle.
        unsafe { AssignProcessToJobObject(job, process) }.expect("the process joins the job");
        Self(job)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle this job owns, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
