//! The Core-side supervisor (architecture section 1, decision D9).
//!
//! The supervisor spawns outposts, holds their job handles so the kernel kills
//! them if Core dies, forwards their messages to a channel the app consumes,
//! and respawns any that exit unexpectedly. Each outpost is spawned suspended,
//! placed in a job object carrying kill-on-job-close plus a per-process memory
//! cap, then resumed — so it is inside the job before it runs a single
//! instruction. Communication is over two anonymous pipes whose child ends are
//! inherited by handle value; no named endpoint exists.
//!
//! State is a map keyed by target [`Pid`], N-ready by construction: one
//! outpost process per application (D9), spawned when that application first
//! gains foreground and kept alive when it loses foreground again — never
//! respawned or rebound to a different application. [`Supervisor::note_foreground`]
//! is the call the
//! [`ForegroundTrigger`](crate::foreground::ForegroundTrigger) makes on every
//! foreground change: spawn if this pid has no outpost yet, otherwise send
//! the existing one an `AnnounceFocus`.
//!
//! Idle outposts are retired on a timer so memory use stays bounded (risk R2):
//! an outpost whose application has not held foreground for
//! [`IDLE_RETIREMENT`] is sent `Shutdown` and reaped, swept both on every
//! foreground change and from a coarse background timer, and the current
//! foreground's outpost is never a candidate. Retirement removes the entry
//! from the map *before* sending `Shutdown`, which is what makes the ordinary
//! respawn-on-death path (below) correctly do nothing for a deliberately
//! retired outpost: by the time its pipe reaches end of stream, there is no
//! matching map entry left to respawn.
//!
//! Respawn-on-death is per-pid and generation-checked (an outpost that exited
//! after already being replaced or retired is not respawned), and additionally
//! checks that the watched application's process is itself still alive before
//! respawning — an outpost whose application has already exited is retired
//! instead, not resurrected to watch a pid that no longer exists.

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, BufReader};
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use std::collections::HashMap;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, OpenProcess,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread, STARTUPINFOW,
};
use windows::core::PWSTR;

use verbatim_model::Pid;

use crate::protocol::{OutpostToSupervisor, SupervisorToOutpost, read_message, write_message};

/// A 200 MB per-outpost memory cap; a leaking outpost is killed by the kernel
/// and respawned by the supervisor.
const OUTPOST_MEMORY_CAP: usize = 200 * 1024 * 1024;

/// How long an outpost's application must have last held foreground before
/// it is a candidate for idle retirement (risk R2's memory-use mitigation).
const IDLE_RETIREMENT: Duration = Duration::from_mins(2);

/// How often the background sweep thread checks for idle outposts, for
/// applications that stay backgrounded long enough that no foreground
/// change ever triggers a sweep on its own.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// A message from the supervisor to the app: either something an outpost
/// sent over its pipe, tagged with the target application it watches, or a
/// lifecycle notice the supervisor itself generates.
#[derive(Debug)]
pub enum OutpostMessage {
    /// A message an outpost sent, tagged with its target pid.
    Event(Pid, OutpostToSupervisor),
    /// The supervisor retired an outpost (idle timeout) or gave up
    /// respawning one whose watched application has itself exited; Core
    /// should drop it from any status mirror.
    Retired(Pid),
}

/// Spawns, tracks, retires, and respawns outpost processes.
pub struct Supervisor {
    shared: Arc<SupervisorShared>,
}

struct SupervisorShared {
    exe_path: PathBuf,
    events_tx: Sender<OutpostMessage>,
    generation: AtomicU64,
    outposts: Mutex<HashMap<Pid, Running>>,
    current_foreground: Mutex<Option<Pid>>,
}

/// One live outpost process, the parent end of its command pipe, and enough
/// bookkeeping to decide respawn and idle-retirement policy.
struct Running {
    generation: u64,
    /// Held so the kernel kills the outpost when this handle closes.
    _job: OwnedHandle,
    /// The outpost process handle; closing it does not kill the process (the
    /// job does), it just releases our reference.
    _process: OwnedHandle,
    to_outpost: File,
    last_foreground_at: Instant,
}

impl Supervisor {
    /// Creates a supervisor that forwards outpost messages to `events_tx` and
    /// starts its background idle-retirement sweep thread. The outpost
    /// executable is resolved next to the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error if the current executable path cannot be determined.
    pub fn new(events_tx: Sender<OutpostMessage>) -> io::Result<Self> {
        let exe_path = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("current exe has no parent directory"))?
            .join("verbatim-outpost.exe");
        let shared = Arc::new(SupervisorShared {
            exe_path,
            events_tx,
            generation: AtomicU64::new(0),
            outposts: Mutex::new(HashMap::new()),
            current_foreground: Mutex::new(None),
        });
        spawn_sweep_thread(&shared);
        Ok(Self { shared })
    }

    /// Reports a foreground change to `target_pid`: spawns an outpost if
    /// none exists for this pid yet, otherwise sends the existing one an
    /// `AnnounceFocus` — never respawning or rebinding it. Outposts stay
    /// alive when their application loses foreground; call this on every
    /// foreground change, including the first, so the first application's
    /// outpost exists too.
    ///
    /// # Errors
    ///
    /// Returns an error if spawning or sending fails.
    pub fn note_foreground(&self, target_pid: Pid) -> io::Result<()> {
        *self
            .shared
            .current_foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(target_pid);

        let mut outposts = self
            .shared
            .outposts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let result = if let Some(running) = outposts.get_mut(&target_pid) {
            running.last_foreground_at = Instant::now();
            write_message(
                &mut running.to_outpost,
                &SupervisorToOutpost::AnnounceFocus {
                    trace_id: verbatim_model::TraceId::mint(),
                },
            )
        } else {
            match self.shared.spawn(target_pid) {
                Ok(running) => {
                    outposts.insert(target_pid, running);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        };
        drop(outposts);
        self.shared.sweep_idle();
        result
    }

    /// Spawns an outpost for `target_pid` if none exists yet, without
    /// touching foreground tracking — a warm-up call, not a foreground
    /// report. Does nothing if an outpost already watches this pid.
    ///
    /// Exists for one specific case: Core's own process. Verbatim reading
    /// its own GUI is a first-class scenario (M1's defining test), and a
    /// cold outpost spawn (process creation, `WinEvent` hook install, UIA
    /// registration) measurably races a real down-arrow keypress sent
    /// immediately after the popup menu takes foreground — confirmed live
    /// against the VM: without this, the menu's own top-level-window
    /// announcement lands, but the `WinEvent` for the arrow-key-selected menu
    /// item can fire before the hook is installed and is lost for good,
    /// since (unlike the initial `AnnounceFocus`) ordinary live navigation
    /// events are not retried. Calling this once at startup for Core's own
    /// pid gives its outpost the whole time between startup and the first
    /// gesture to become warm, which in practice is always enough.
    ///
    /// # Errors
    ///
    /// Returns an error if spawning fails.
    pub fn ensure_spawned(&self, target_pid: Pid) -> io::Result<()> {
        let mut outposts = self
            .shared
            .outposts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if outposts.contains_key(&target_pid) {
            return Ok(());
        }
        let running = self.shared.spawn(target_pid)?;
        outposts.insert(target_pid, running);
        Ok(())
    }

    /// Sends a command to a specific outpost.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no outpost for `target_pid` or the write
    /// fails.
    pub fn send_to(&self, target_pid: Pid, command: &SupervisorToOutpost) -> io::Result<()> {
        let mut outposts = self
            .shared
            .outposts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let running = outposts
            .get_mut(&target_pid)
            .ok_or_else(|| io::Error::other(format!("no outpost is watching pid {target_pid}")))?;
        write_message(&mut running.to_outpost, command)
    }
}

impl SupervisorShared {
    /// Spawns an outpost watching `target_pid` for its whole life (decision
    /// D9: fixed at spawn, never retargeted) and starts its reader thread.
    fn spawn(self: &Arc<Self>, target_pid: Pid) -> io::Result<Running> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let pipes = Pipes::create()?;
        let job = create_job()?;

        let command_line = format!(
            "\"{}\" --pipe-in {} --pipe-out {} --target-pid {}",
            self.exe_path.display(),
            pipes.child_in.0 as usize,
            pipes.child_out.0 as usize,
            target_pid.0,
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
        // The spawn itself is the first foreground change this outpost has
        // to announce; it saw none of the events that led to it.
        write_message(
            &mut to_outpost,
            &SupervisorToOutpost::AnnounceFocus {
                trace_id: verbatim_model::TraceId::mint(),
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
            _job: job,
            _process: process_owned,
            to_outpost,
            last_foreground_at: Instant::now(),
        })
    }

    /// Respawns the outpost that exited, if it is still this generation's
    /// current entry for `target_pid` and its watched application process is
    /// itself still alive. If the entry is gone (already retired or
    /// replaced), does nothing. If the entry is still there but the
    /// application has exited, removes it and reports it retired rather
    /// than resurrecting an outpost for a pid that no longer exists.
    fn respawn_if_alive(self: &Arc<Self>, target_pid: Pid, generation: u64) {
        let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = outposts.get(&target_pid) else {
            return; // Already retired or replaced; nothing to do.
        };
        if running.generation != generation {
            return; // Already replaced; nothing to do.
        }
        if !process_is_alive(target_pid.0) {
            outposts.remove(&target_pid);
            drop(outposts);
            let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            return;
        }
        outposts.remove(&target_pid);
        drop(outposts);
        match self.spawn(target_pid) {
            Ok(running) => {
                self.outposts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(target_pid, running);
            }
            Err(error) => {
                tracing::error!(%error, %target_pid, "failed to respawn outpost");
                let _ = self.events_tx.send(OutpostMessage::Retired(target_pid));
            }
        }
    }

    /// Sends `Shutdown` to and removes every outpost whose application has
    /// not held foreground for [`IDLE_RETIREMENT`], skipping the current
    /// foreground's outpost and Core's own. The map entry is removed
    /// *before* `Shutdown` is written, so the reader thread's subsequent
    /// end-of-stream finds no matching generation and does not respawn (see
    /// this module's doc).
    ///
    /// Core's own outpost is exempt for the same reason
    /// [`Supervisor::ensure_spawned`] pre-warms it at startup: retiring it
    /// after two idle minutes would make the next Verbatim menu open spawn
    /// it cold, recreating the lost-keystroke race the warm-up exists to
    /// prevent — and "open the Verbatim menu after a while working in other
    /// applications" is the common case, not the edge. One permanently warm
    /// outpost watching our own process is a fixed, known cost.
    fn sweep_idle(&self) {
        let current = *self
            .current_foreground
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let own_pid = Pid(std::process::id());
        let now = Instant::now();
        let mut retiring = Vec::new();
        {
            let mut outposts = self.outposts.lock().unwrap_or_else(PoisonError::into_inner);
            let idle_pids: Vec<Pid> = outposts
                .iter()
                .filter(|&(&pid, running)| {
                    Some(pid) != current
                        && pid != own_pid
                        && idle_decision(running.last_foreground_at, now, IDLE_RETIREMENT)
                })
                .map(|(&pid, _)| pid)
                .collect();
            for pid in idle_pids {
                if let Some(running) = outposts.remove(&pid) {
                    retiring.push((pid, running));
                }
            }
        }
        for (pid, mut running) in retiring {
            let _ = write_message(&mut running.to_outpost, &SupervisorToOutpost::Shutdown);
            let _ = self.events_tx.send(OutpostMessage::Retired(pid));
            tracing::info!(%pid, "retired idle outpost");
        }
    }
}

/// The pure idle-retirement decision, factored out for unit testing: whether
/// an outpost last foregrounded at `last_foreground_at` counts as idle at
/// `now`, against threshold `idle_after`.
fn idle_decision(last_foreground_at: Instant, now: Instant, idle_after: Duration) -> bool {
    now.saturating_duration_since(last_foreground_at) >= idle_after
}

/// Spawns the coarse background thread that sweeps idle outposts even when
/// no foreground change happens to trigger one — a long-backgrounded
/// application's outpost still needs to be reaped eventually. Lives for the
/// process's whole life, like the supervisor itself.
fn spawn_sweep_thread(shared: &Arc<SupervisorShared>) {
    let shared = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("verbatim-outpost-sweep".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(SWEEP_INTERVAL);
                shared.sweep_idle();
            }
        });
}

/// Whether `pid` names a process that is still running. Used before
/// respawning an outpost whose process just exited, so a dead application's
/// outpost is retired instead of resurrected to watch a pid that no longer
/// exists. Pid reuse is a known, accepted imprecision here, the same trade
/// every Win32 API taking a bare pid makes.
fn process_is_alive(pid: u32) -> bool {
    // SAFETY: OpenProcess with a query-only access right fails safely on an
    // invalid or inaccessible pid; the handle is closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0u32;
        let alive = GetExitCodeProcess(handle, &raw mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0.cast_unsigned();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        alive
    }
}

/// Forwards outpost messages until the pipe closes, then requests a
/// respawn-if-alive decision.
fn reader_loop(
    shared: &Arc<SupervisorShared>,
    generation: u64,
    target_pid: Pid,
    from_outpost: File,
) {
    let mut reader = BufReader::new(from_outpost);
    while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
        if shared
            .events_tx
            .send(OutpostMessage::Event(target_pid, message))
            .is_err()
        {
            return; // The app dropped the receiver; stop without respawning.
        }
    }
    // Reached on end of stream or a pipe error: the outpost has exited.
    shared.respawn_if_alive(target_pid, generation);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_decision_is_false_before_the_threshold() {
        let start = Instant::now();
        let now = start + Duration::from_secs(119);
        assert!(!idle_decision(start, now, Duration::from_mins(2)));
    }

    #[test]
    fn idle_decision_is_true_at_and_past_the_threshold() {
        let start = Instant::now();
        assert!(idle_decision(
            start,
            start + Duration::from_mins(2),
            Duration::from_mins(2)
        ));
        assert!(idle_decision(
            start,
            start + Duration::from_secs(200),
            Duration::from_mins(2)
        ));
    }

    #[test]
    fn idle_decision_never_panics_when_now_precedes_last_foreground() {
        // saturating_duration_since guards against a caller-supplied `now`
        // that is somehow earlier than `last_foreground_at`.
        let start = Instant::now();
        let earlier = start.checked_sub(Duration::from_secs(5)).unwrap_or(start);
        assert!(!idle_decision(start, earlier, Duration::from_mins(2)));
    }
}
