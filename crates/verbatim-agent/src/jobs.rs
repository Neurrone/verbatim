//! Watching the processes in each launched child's job object, for
//! [`Request::JobExits`](crate::protocol::Request::JobExits): every process
//! that joins the job is opened as it joins, so its image name and exit
//! code can still be read after it exits, and every exit is recorded with
//! whether Windows called it abnormal, which an unhandled exception, a
//! crash, makes it.
//!
//! One I/O completion port receives the messages of every job, and one
//! thread reads it; it blocks on the port between messages. A test that
//! needs to know a job changed waits on [`CHANGED`], which the thread
//! notifies after every message.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};
use windows::Win32::System::JobObjects::{
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JobObjectAssociateCompletionPortInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use crate::protocol::{ProcessExit, ProcessState};

/// A job message: no process in the job is running any more.
const JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO: u32 = 4;
/// A job message: a process joined the job.
const JOB_OBJECT_MSG_NEW_PROCESS: u32 = 6;
/// A job message: a process in the job exited.
const JOB_OBJECT_MSG_EXIT_PROCESS: u32 = 7;
/// A job message: a process in the job exited abnormally, on an unhandled
/// exception.
const JOB_OBJECT_MSG_ABNORMAL_EXIT_PROCESS: u32 = 8;

/// What is known about one launched child's job.
#[derive(Default)]
pub(crate) struct JobRecord {
    /// The processes in the job that have not exited, each opened when it
    /// joined, with its image name.
    pub(crate) running: HashMap<u32, (Option<OwnedHandle>, String)>,
    /// The processes that have exited, in the order they exited.
    pub(crate) exits: Vec<ProcessExit>,
    /// Whether Windows has said no process in the job is running any more,
    /// since the last process joined it.
    pub(crate) empty: bool,
}

/// Every launched child's job record, by the child's pid.
pub(crate) static JOBS: Mutex<BTreeMap<u32, JobRecord>> = Mutex::new(BTreeMap::new());

/// Notified after every job message the watching thread handles.
pub(crate) static CHANGED: Condvar = Condvar::new();

/// The completion port every job reports to, made with its watching thread
/// on first use.
fn port() -> io::Result<HANDLE> {
    static PORT: OnceLock<Result<usize, String>> = OnceLock::new();
    PORT.get_or_init(|| {
        // SAFETY: a new port, not associated with any file handle.
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }
            .map_err(|error| error.to_string())?;
        let raw = port.0 as usize;
        std::thread::Builder::new()
            .name("verbatim-agent-jobs".to_owned())
            .spawn(move || watch(HANDLE(raw as *mut _)))
            .map_err(|error| error.to_string())?;
        Ok(raw)
    })
    .clone()
    .map(|raw| HANDLE(raw as *mut _))
    .map_err(io::Error::other)
}

/// Has `job`, the job of the child `pid` is being launched into, report to
/// the port under `pid`, before the child is assigned to it, so the child
/// joining is reported too.
pub(crate) fn watch_job(job: &OwnedHandle, pid: u32) -> io::Result<()> {
    JOBS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(pid, JobRecord::default());
    let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: pid as usize as *mut _,
        CompletionPort: port()?,
    };
    // SAFETY: `association` is the structure this information class takes,
    // passed with its size; the job handle is open.
    unsafe {
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectAssociateCompletionPortInformation,
            (&raw const association).cast(),
            u32::try_from(size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>()).unwrap_or(u32::MAX),
        )
    }
    .map_err(io::Error::other)
}

/// The watching thread: reads the port for as long as the agent runs.
fn watch(port: HANDLE) {
    loop {
        let mut message = 0u32;
        let mut key = 0usize;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: every out-parameter is a local of the right type.
        let read = unsafe {
            GetQueuedCompletionStatus(
                port,
                &raw mut message,
                &raw mut key,
                &raw mut overlapped,
                INFINITE,
            )
        };
        if read.is_err() {
            continue;
        }
        let root = u32::try_from(key).unwrap_or(0);
        // A job message carries the process id in place of the overlapped
        // structure's address.
        let pid = u32::try_from(overlapped as usize).unwrap_or(0);
        let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(record) = jobs.get_mut(&root) {
            match message {
                JOB_OBJECT_MSG_NEW_PROCESS => {
                    record.empty = false;
                    let handle = open(pid);
                    let image = handle
                        .as_ref()
                        .and_then(crate::process::image_name_of)
                        .unwrap_or_else(|| "(exited before it could be opened)".to_owned());
                    record.running.insert(pid, (handle, image));
                }
                JOB_OBJECT_MSG_EXIT_PROCESS | JOB_OBJECT_MSG_ABNORMAL_EXIT_PROCESS => {
                    let entry = record
                        .running
                        .remove(&pid)
                        .unwrap_or((None, "(not seen joining)".to_owned()));
                    record.exits.push(exit_of(
                        pid,
                        entry,
                        message == JOB_OBJECT_MSG_ABNORMAL_EXIT_PROCESS,
                    ));
                }
                // Windows sends no exit message for a process ended by
                // terminating its job, only this one once the job is empty:
                // every process still recorded as running has ended.
                JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO => {
                    let mut ended: Vec<_> = record.running.drain().collect();
                    ended.sort_by_key(|(pid, _)| *pid);
                    for (pid, entry) in ended {
                        record.exits.push(exit_of(pid, entry, false));
                    }
                    record.empty = true;
                }
                _ => {}
            }
        }
        drop(jobs);
        CHANGED.notify_all();
    }
}

/// The exit of process `pid`, from the handle and image name recorded as
/// it joined.
fn exit_of(
    pid: u32,
    (handle, image): (Option<OwnedHandle>, String),
    abnormal: bool,
) -> ProcessExit {
    let exit_code = handle.as_ref().and_then(|handle| {
        match crate::process::exit_state(HANDLE(handle.as_raw_handle())) {
            Ok(ProcessState::Exited { exit_code }) => exit_code,
            _ => None,
        }
    });
    ProcessExit {
        pid,
        image,
        exit_code,
        abnormal,
    }
}

/// Opens `pid` to read its image name and exit code later.
fn open(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: a query-and-wait open of a plain process id; the handle is
    // owned below.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .ok()?;
    // SAFETY: a handle just opened, owned here alone.
    Some(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// The processes that have exited in the job of the launched child `pid`.
///
/// # Errors
///
/// Returns an error when the agent did not launch `pid`, or has forgotten
/// it.
pub(crate) fn exits(pid: u32) -> io::Result<Vec<ProcessExit>> {
    JOBS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&pid)
        .map(|record| record.exits.clone())
        .ok_or_else(|| io::Error::other(format!("the agent did not launch process {pid}")))
}

/// Waits up to `timeout` for Windows to say that no process in the job of
/// the launched child `pid` is running any more, on the job watcher's
/// notifications, and returns every exit in the job, in the order the
/// processes exited.
///
/// # Errors
///
/// Returns an error when the agent did not launch `pid`, or when processes
/// are still running in its job once the wait runs out, naming them.
pub(crate) fn wait_until_empty(
    pid: u32,
    timeout: std::time::Duration,
) -> io::Result<Vec<ProcessExit>> {
    let jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
    let (jobs, _) = CHANGED
        .wait_timeout_while(jobs, timeout, |jobs| {
            jobs.get(&pid).is_some_and(|record| !record.empty)
        })
        .unwrap_or_else(PoisonError::into_inner);
    let record = jobs
        .get(&pid)
        .ok_or_else(|| io::Error::other(format!("the agent did not launch process {pid}")))?;
    if record.empty {
        return Ok(record.exits.clone());
    }
    let running: Vec<(u32, &str)> = record
        .running
        .iter()
        .map(|(pid, (_, image))| (*pid, image.as_str()))
        .collect();
    Err(io::Error::other(format!(
        "processes in the job of {pid} were still running {timeout:?} later: {running:?}"
    )))
}

/// Forgets the job of the launched child `pid`.
pub(crate) fn forget(pid: u32) {
    JOBS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&pid);
}
