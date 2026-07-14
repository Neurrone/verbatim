//! The Core-side supervisor (architecture section 1).
//!
//! The supervisor spawns outposts, holds their job handles so the kernel kills
//! them if Core dies, forwards their messages to a channel the app consumes,
//! and respawns any that exit. Each outpost is spawned suspended, placed in a
//! job object carrying kill-on-job-close plus a per-process memory cap, then
//! resumed — so it is inside the job before it runs a single instruction.
//! Communication is over two anonymous pipes whose child ends are inherited by
//! handle value; no named endpoint exists.
//!
//! State is keyed by target [`Pid`] and is N-ready, but M1 policy caps it at a
//! single outpost retargeted on foreground change (see [`Supervisor::target`]).
//! The [`ForegroundTrigger`](crate::foreground::ForegroundTrigger) drives that
//! retargeting.

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, BufReader};
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread,
    STARTUPINFOW,
};
use windows::core::PWSTR;

use verbatim_model::Pid;

use crate::protocol::{OutpostToSupervisor, SupervisorToOutpost, read_message, write_message};

/// A 200 MB per-outpost memory cap; a leaking outpost is killed by the kernel
/// and respawned by the supervisor.
const OUTPOST_MEMORY_CAP: usize = 200 * 1024 * 1024;

/// A message from an outpost, tagged with the target application it watches so
/// the app can attribute it (M1 runs one outpost, but the tag is N-ready).
pub type OutpostMessage = (Pid, OutpostToSupervisor);

/// Spawns, tracks, and respawns outpost processes.
pub struct Supervisor {
    shared: Arc<SupervisorShared>,
}

struct SupervisorShared {
    exe_path: PathBuf,
    events_tx: Sender<OutpostMessage>,
    generation: AtomicU64,
    current: Mutex<Option<Running>>,
}

/// One live outpost process and the parent ends of its pipes.
struct Running {
    generation: u64,
    target_pid: Pid,
    /// Held so the kernel kills the outpost when this handle closes.
    _job: OwnedHandle,
    /// The outpost process handle; closing it does not kill the process (the
    /// job does), it just releases our reference.
    _process: OwnedHandle,
    to_outpost: File,
}

impl Supervisor {
    /// Creates a supervisor that forwards outpost messages to `events_tx`. The
    /// outpost executable is resolved next to the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined.
    pub fn new(events_tx: Sender<OutpostMessage>) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        Ok(Self {
            shared: Arc::new(SupervisorShared {
                exe_path,
                events_tx,
                generation: AtomicU64::new(0),
                current: Mutex::new(None),
            }),
        })
    }

    /// Points the (single, M1) outpost at `target_pid`: spawns one if none is
    /// running, otherwise retargets the existing one with a `Configure` — the
    /// cheap path taken on every foreground change.
    ///
    /// # Errors
    ///
    /// Returns an error if spawning or the retargeting command fails.
    pub fn target(&self, target_pid: Pid) -> io::Result<()> {
        let mut current = self
            .shared
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = current.as_mut() {
            if running.target_pid == target_pid {
                return Ok(());
            }
            running.target_pid = target_pid;
            return write_message(
                &mut running.to_outpost,
                &SupervisorToOutpost::Configure {
                    target_pid,
                    backend_override: None,
                },
            );
        }
        let running = self.shared.spawn(target_pid)?;
        *current = Some(running);
        Ok(())
    }

    /// Sends a command to the current outpost.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no outpost or the write fails.
    pub fn send(&self, command: &SupervisorToOutpost) -> io::Result<()> {
        let mut current = self
            .shared
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let running = current
            .as_mut()
            .ok_or_else(|| io::Error::other("no outpost is running"))?;
        write_message(&mut running.to_outpost, command)
    }
}

impl SupervisorShared {
    /// Spawns an outpost watching `target_pid` and starts its reader thread.
    fn spawn(self: &Arc<Self>, target_pid: Pid) -> io::Result<Running> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let pipes = Pipes::create()?;
        let job = create_job()?;

        let command_line = format!(
            "\"{}\" --pipe-in {} --pipe-out {}",
            self.exe_path.display(),
            pipes.child_in.0 as usize,
            pipes.child_out.0 as usize,
        );
        let process = spawn_suspended(&command_line, &job)?;

        // The child has inherited its pipe ends; close ours to them so EOF is
        // observed correctly when the outpost exits.
        pipes.close_child_ends();

        // Resume now that the process is in the job.
        // SAFETY: `process.thread` is the suspended primary thread handle.
        unsafe {
            ResumeThread(process.thread);
        }
        close_handle(process.thread);

        let mut to_outpost = pipes.parent_out;
        write_message(
            &mut to_outpost,
            &SupervisorToOutpost::Configure {
                target_pid,
                backend_override: None,
            },
        )?;

        // SAFETY: `process.process` is a valid process handle we now own.
        let process_owned = unsafe { OwnedHandle::from_raw_handle(process.process.0 as RawHandle) };

        let reader_shared = Arc::clone(self);
        let from_outpost = pipes.parent_in;
        thread::Builder::new()
            .name("verbatim-outpost-reader".to_owned())
            .spawn(move || reader_loop(&reader_shared, generation, target_pid, from_outpost))
            .map_err(io::Error::other)?;

        Ok(Running {
            generation,
            target_pid,
            _job: job,
            _process: process_owned,
            to_outpost,
        })
    }

    /// Respawns the outpost if the one that exited is still the current one.
    fn respawn_if_current(self: &Arc<Self>, generation: u64) {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = current.as_ref() else {
            return;
        };
        if running.generation != generation {
            return; // Already replaced or retargeted; nothing to do.
        }
        let target_pid = running.target_pid;
        *current = None;
        match self.spawn(target_pid) {
            Ok(running) => *current = Some(running),
            Err(error) => tracing::error!(%error, "failed to respawn outpost"),
        }
    }
}

/// Forwards outpost messages until the pipe closes, then requests a respawn.
fn reader_loop(
    shared: &Arc<SupervisorShared>,
    generation: u64,
    target_pid: Pid,
    from_outpost: File,
) {
    let mut reader = BufReader::new(from_outpost);
    while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        if shared.events_tx.send((target_pid, message)).is_err() {
            return; // The app dropped the receiver; stop without respawning.
        }
    }
    // Reached on end of stream or a pipe error: the outpost has exited.
    shared.respawn_if_current(generation);
}

/// The four pipe handles: parent and child ends of two anonymous pipes.
struct Pipes {
    /// Parent end: supervisor reads outpost messages.
    parent_in: File,
    /// Child end: outpost writes messages (its `--pipe-out`).
    child_out: HANDLE,
    /// Parent end: supervisor writes commands.
    parent_out: File,
    /// Child end: outpost reads commands (its `--pipe-in`).
    child_in: HANDLE,
}

impl Pipes {
    /// Creates both pipes with inheritable child ends and non-inheritable
    /// parent ends.
    fn create() -> io::Result<Self> {
        // Command pipe: supervisor writes (parent_out), outpost reads (child_in).
        let (child_in, parent_out) = anonymous_pipe(PipeInherit::Read)?;
        // Event pipe: outpost writes (child_out), supervisor reads (parent_in).
        let (child_out, parent_in) = anonymous_pipe(PipeInherit::Write)?;
        // SAFETY: the parent ends are valid pipe handles we own.
        let parent_out = unsafe { File::from_raw_handle(parent_out.0 as RawHandle) };
        let parent_in = unsafe { File::from_raw_handle(parent_in.0 as RawHandle) };
        Ok(Self {
            parent_in,
            child_out,
            parent_out,
            child_in,
        })
    }

    /// Closes the parent's copies of the child-end handles after the child has
    /// inherited them.
    fn close_child_ends(&self) {
        close_handle(self.child_in);
        close_handle(self.child_out);
    }
}

/// Which end of an anonymous pipe should be inheritable by a child.
#[derive(Clone, Copy)]
enum PipeInherit {
    /// The read end is inheritable (the child reads).
    Read,
    /// The write end is inheritable (the child writes).
    Write,
}

/// Creates one anonymous pipe, returning `(child_end, parent_end)` with the
/// child end marked inheritable and the parent end not.
fn anonymous_pipe(inherit: PipeInherit) -> io::Result<(HANDLE, HANDLE)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: both out-handles are written by CreatePipe before use; the
    // security attributes live for the duration of the call.
    unsafe {
        CreatePipe(
            &raw mut read,
            &raw mut write,
            Some(&raw const attributes),
            0,
        )
        .map_err(to_io)?;
    }
    // CreatePipe made both ends inheritable; clear inheritance on the parent's.
    let (child_end, parent_end) = match inherit {
        PipeInherit::Read => (read, write),
        PipeInherit::Write => (write, read),
    };
    // SAFETY: `parent_end` is a valid handle just created.
    unsafe {
        SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)).map_err(to_io)?;
    }
    Ok((child_end, parent_end))
}

/// Creates a job object with kill-on-close and the per-process memory cap.
fn create_job() -> io::Result<OwnedHandle> {
    // SAFETY: CreateJobObjectW with null attributes and name creates an
    // unnamed job; the returned handle is validated before use.
    let job = unsafe { CreateJobObjectW(None, PWSTR::null()).map_err(to_io)? };
    if job.is_invalid() {
        return Err(io::Error::other(
            "CreateJobObjectW returned an invalid handle",
        ));
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.ProcessMemoryLimit = OUTPOST_MEMORY_CAP;
    // SAFETY: `limits` is a correctly sized JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    // matching the information class.
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
        )
        .map_err(to_io)?;
    }
    // SAFETY: `job` is a valid job handle we now own.
    Ok(unsafe { OwnedHandle::from_raw_handle(job.0 as RawHandle) })
}

/// The handles a spawned process yields.
struct Spawned {
    process: HANDLE,
    thread: HANDLE,
}

/// Creates the outpost process suspended, inheriting handles, and assigns it to
/// `job` before it runs.
fn spawn_suspended(command_line: &str, job: &OwnedHandle) -> io::Result<Spawned> {
    let mut command: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let startup = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: `command` is a NUL-terminated writable UTF-16 buffer; startup and
    // info are correctly sized; bInheritHandles is true so the inheritable
    // child pipe ends pass to the child.
    unsafe {
        CreateProcessW(
            None,
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            None,
            None,
            &raw const startup,
            &raw mut info,
        )
        .map_err(to_io)?;
        // Assign to the job while still suspended, so it is contained before it
        // runs. The job handle is the OwnedHandle's raw value.
        let job_handle = HANDLE(job_raw(job));
        AssignProcessToJobObject(job_handle, info.hProcess).map_err(to_io)?;
    }
    Ok(Spawned {
        process: info.hProcess,
        thread: info.hThread,
    })
}

/// Returns the raw `HANDLE` value of an `OwnedHandle` without consuming it.
fn job_raw(job: &OwnedHandle) -> *mut c_void {
    use std::os::windows::io::AsRawHandle;
    job.as_raw_handle().cast()
}

fn close_handle(handle: HANDLE) {
    if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: `handle` is a valid handle we own and are done with.
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
}

fn to_io(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
