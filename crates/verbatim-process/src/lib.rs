//! Contained child processes: what Core launches outposts, the focus
//! listener, and synthesizer hosts with (architecture section 1).
//!
//! A child is spawned suspended, placed in a kill-on-close job object, and
//! only then resumed, so it is contained before it runs a single
//! instruction, and dies when Core's job handle closes, whatever ends Core.
//! It talks to Core over two anonymous pipes whose child ends it inherits;
//! it inherits exactly those and its log file, never another child's
//! handles, so two launches running at once cannot keep each other's pipes
//! open. Each child's standard output and error go to a log file named for
//! it in this launch's log directory.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::{fs, io, ptr};

use windows::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_ALWAYS,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
};
use windows::core::{PCWSTR, PWSTR};

/// What to launch.
pub struct ChildSpec<'a> {
    /// The executable.
    pub exe: &'a Path,
    /// The command-line arguments, given the raw values of the child's two
    /// inherited pipe handles: the one it reads commands from, then the one
    /// it writes messages to.
    pub arguments: &'a dyn Fn(usize, usize) -> String,
    /// Names the child's log file, `<log_stem>.log`, in
    /// [`launch_log_dir`].
    pub log_stem: &'a str,
    /// A memory limit for the child; past it its allocations fail.
    pub memory_cap: Option<usize>,
    /// The buffer size to ask for on the pipe the child writes to, in bytes;
    /// `0` for the system default. A small buffer keeps a child that
    /// streams data (a synthesizer host) from running far ahead of Core.
    pub from_child_buffer: u32,
}

/// A launched child: its job, which kills it when dropped.
pub struct Contained {
    /// Held so the kernel kills the process when this handle closes. Every
    /// ending, for any reason, closes it.
    pub job: OwnedHandle,
    /// The process handle; closing it only releases this reference.
    pub process: OwnedHandle,
    /// The process's own id, for logs.
    pub pid: u32,
}

/// Core's ends of a launched child's two pipes.
pub struct ChildPipes {
    /// Core writes commands here.
    pub to_child: File,
    /// Core reads the child's messages here.
    pub from_child: File,
}

/// Launches a contained child as `spec` describes.
///
/// # Errors
///
/// Returns the error of whichever step failed: creating the job or the
/// pipes, or spawning the process.
pub fn launch(spec: &ChildSpec<'_>) -> io::Result<(Contained, ChildPipes)> {
    let job = create_job(spec.memory_cap)?;
    let pipes = Pipes::create(spec.from_child_buffer)?;
    let arguments = (spec.arguments)(pipes.child_in.0 as usize, pipes.child_out.0 as usize);
    let command_line = format!("\"{}\" {arguments}", spec.exe.display());
    // Best-effort: a failed log open leaves the child unredirected, never
    // unspawned.
    let log = child_log_handle(spec.exe, spec.log_stem);
    let spawned = spawn_suspended(&command_line, &job, &pipes, log);
    if let Some(log) = log {
        // The child inherited its own copy; drop ours whether or not the
        // spawn succeeded.
        close_handle(log);
    }
    // The child has inherited its pipe ends; close ours so end of stream is
    // seen when it exits.
    close_handle(pipes.child_in);
    close_handle(pipes.child_out);
    let spawned = spawned?;
    // SAFETY: `spawned.thread` is the suspended primary thread handle.
    unsafe {
        ResumeThread(spawned.thread);
    }
    close_handle(spawned.thread);
    Ok((
        Contained {
            job,
            // SAFETY: `spawned.process` is a valid process handle we now own.
            process: unsafe { OwnedHandle::from_raw_handle(spawned.process.0 as RawHandle) },
            pid: spawned.pid,
        },
        ChildPipes {
            to_child: pipes.parent_out,
            from_child: pipes.parent_in,
        },
    ))
}

/// The four pipe handles: parent and child ends of two anonymous pipes.
struct Pipes {
    /// Parent end: Core reads the child's messages.
    parent_in: File,
    /// Child end: the child writes messages.
    child_out: HANDLE,
    /// Parent end: Core writes commands.
    parent_out: File,
    /// Child end: the child reads commands.
    child_in: HANDLE,
}

impl Pipes {
    /// Creates both pipes with inheritable child ends and non-inheritable
    /// parent ends.
    fn create(from_child_buffer: u32) -> io::Result<Self> {
        let (child_in, parent_out) = anonymous_pipe(PipeInherit::Read, 0)?;
        // SAFETY: the parent end is a valid pipe handle we own.
        let parent_out = unsafe { File::from_raw_handle(parent_out.0 as RawHandle) };
        let (child_out, parent_in) = match anonymous_pipe(PipeInherit::Write, from_child_buffer) {
            Ok(ends) => ends,
            Err(error) => {
                close_handle(child_in);
                return Err(error);
            }
        };
        // SAFETY: the parent end is a valid pipe handle we own.
        let parent_in = unsafe { File::from_raw_handle(parent_in.0 as RawHandle) };
        Ok(Self {
            parent_in,
            child_out,
            parent_out,
            child_in,
        })
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
fn anonymous_pipe(inherit: PipeInherit, buffer: u32) -> io::Result<(HANDLE, HANDLE)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: both out-handles are written by CreatePipe before use; the
    // security attributes live for the duration of the call.
    unsafe {
        CreatePipe(
            &raw mut read,
            &raw mut write,
            Some(&raw const attributes),
            buffer,
        )
        .map_err(to_io)?;
    }
    let (child_end, parent_end) = match inherit {
        PipeInherit::Read => (read, write),
        PipeInherit::Write => (write, read),
    };
    // SAFETY: `parent_end` is a valid handle just created.
    if let Err(error) =
        unsafe { SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
    {
        close_handle(child_end);
        close_handle(parent_end);
        return Err(to_io(error));
    }
    Ok((child_end, parent_end))
}

/// Creates a job object with kill-on-close and, when given, a per-process
/// memory cap.
fn create_job(memory_cap: Option<usize>) -> io::Result<OwnedHandle> {
    // SAFETY: CreateJobObjectW with null attributes and name creates an
    // unnamed job; the returned handle is validated before use.
    let job = unsafe { CreateJobObjectW(None, PWSTR::null()).map_err(to_io)? };
    if job.is_invalid() {
        return Err(io::Error::other(
            "CreateJobObjectW returned an invalid handle",
        ));
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if let Some(cap) = memory_cap {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.ProcessMemoryLimit = cap;
    }
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

/// The handles and id a spawned process yields.
struct Spawned {
    process: HANDLE,
    thread: HANDLE,
    pid: u32,
}

/// Creates the child suspended and assigns it to `job` before it runs. The
/// child inherits only the handles named in a `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`:
/// its two pipe ends and, when `log` is `Some`, the log file its standard
/// output and error are redirected to (its standard input too, harmlessly — a
/// child reads its command pipe, never stdin).
fn spawn_suspended(
    command_line: &str,
    job: &OwnedHandle,
    pipes: &Pipes,
    log: Option<HANDLE>,
) -> io::Result<Spawned> {
    let mut command: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut inherited = vec![pipes.child_in, pipes.child_out];
    inherited.extend(log);

    // Size, allocate, and fill the attribute list naming the inherited
    // handles.
    let mut size = 0usize;
    // SAFETY: the first call only reports the size needed; its failure with
    // "insufficient buffer" is expected.
    unsafe {
        let _ = InitializeProcThreadAttributeList(None, 1, None, &raw mut size);
    }
    let mut buffer = vec![0u8; size];
    let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
    // SAFETY: `buffer` is `size` bytes as the first call asked for, and
    // outlives every use of `attributes` below, which ends with the delete.
    unsafe {
        InitializeProcThreadAttributeList(Some(attributes), 1, None, &raw mut size)
            .map_err(to_io)?;
    }
    let result = (|| {
        // SAFETY: `inherited` outlives the CreateProcessW call that reads it.
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(inherited.as_ptr().cast()),
                inherited.len() * size_of::<HANDLE>(),
                None,
                None,
            )
            .map_err(to_io)?;
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>()).unwrap_or(0);
        startup.lpAttributeList = attributes;
        if let Some(log) = log {
            startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = log;
            startup.StartupInfo.hStdOutput = log;
            startup.StartupInfo.hStdError = log;
        }
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: `command` is a NUL-terminated writable UTF-16 buffer; the
        // startup information is correctly sized and carries the attribute
        // list; bInheritHandles is true, restricted by that list.
        unsafe {
            CreateProcessW(
                None,
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                None,
                None,
                (&raw const startup).cast(),
                &raw mut info,
            )
            .map_err(to_io)?;
            // Contained before it runs.
            let job_handle = HANDLE(job.as_raw_handle().cast::<c_void>());
            if let Err(error) = AssignProcessToJobObject(job_handle, info.hProcess) {
                // Outside the job nothing would ever kill it.
                let _ = TerminateProcess(info.hProcess, 1);
                close_handle(info.hThread);
                close_handle(info.hProcess);
                return Err(to_io(error));
            }
        }
        Ok(Spawned {
            process: info.hProcess,
            thread: info.hThread,
            pid: info.dwProcessId,
        })
    })();
    // SAFETY: `attributes` was initialized above and is no longer used.
    unsafe {
        DeleteProcThreadAttributeList(attributes);
    }
    result
}

/// Closes a raw handle we own, ignoring invalid ones.
fn close_handle(handle: HANDLE) {
    if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: `handle` is a valid handle we own and are done with.
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
}

/// How many Verbatim launches keep their log directories: this one and the
/// newest earlier ones.
const KEPT_LAUNCH_LOGS: usize = 10;

/// The directory this Verbatim launch's child logs go to:
/// `logs\<Verbatim's pid>` next to the executables in `exe_dir`. One
/// directory per launch, so the end-to-end harness collects exactly one
/// launch's logs and a reused application pid never appends to an older
/// application's log. A later Verbatim that reuses this pid finds no writer
/// left in the directory, because every child sits in a kill-on-close job
/// and dies with the Verbatim that started it.
#[must_use]
pub fn launch_log_dir(exe_dir: &Path) -> PathBuf {
    exe_dir.join("logs").join(std::process::id().to_string())
}

/// Prepares this launch's log directory, best effort: empties it if an
/// earlier process with the same pid left one, and removes all but the
/// newest [`KEPT_LAUNCH_LOGS`] launch directories (by the time a file was
/// last created in each), along with any log the earlier layout left
/// directly in `logs`. Removal is partial for a launch that is still
/// running: the log files its processes hold open stay, and a later launch
/// removes them.
pub fn prepare_launch_logs(exe_dir: &Path) {
    let own = launch_log_dir(exe_dir);
    let _ = fs::remove_dir_all(&own);
    if let Err(error) = fs::create_dir_all(&own) {
        tracing::warn!(%error, "could not create this launch's log directory");
        return;
    }
    let Ok(entries) = fs::read_dir(exe_dir.join("logs")) else {
        return;
    };
    let mut launches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path != own {
                let modified = entry.metadata().and_then(|metadata| metadata.modified());
                launches.push((modified.ok(), path));
            }
        } else if path.extension().is_some_and(|extension| extension == "log") {
            let _ = fs::remove_file(&path);
        }
    }
    launches.sort_by_key(|launch| std::cmp::Reverse(launch.0));
    for (_, path) in launches.into_iter().skip(KEPT_LAUNCH_LOGS - 1) {
        let _ = fs::remove_dir_all(path);
    }
}

/// Opens (creating if needed) the log file a spawned child's output is
/// redirected into, returning an inheritable, append-mode handle — or `None`
/// if the logs directory or file could not be created, since logging is
/// diagnostics and must never fail a spawn. `file_stem` names the file
/// inside this launch's log directory ([`launch_log_dir`]), where the
/// end-to-end harness reads it. Append mode so a crashed child's log and
/// its replacement's both survive in one file.
fn child_log_handle(exe_path: &Path, file_stem: &str) -> Option<HANDLE> {
    let dir = launch_log_dir(exe_path.parent()?);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%error, "could not create the child logs directory");
        return None;
    }
    let path = dir.join(format!("{file_stem}.log"));
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    // SAFETY: `wide` is a NUL-terminated path alive across the call; the
    // attributes live for the call; append-mode writes are atomic at end of
    // file, so concurrent writers do not interleave.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_APPEND_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&raw const attributes),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match handle {
        Ok(handle) if !handle.is_invalid() => Some(handle),
        Ok(handle) => {
            close_handle(handle);
            None
        }
        Err(error) => {
            tracing::warn!(%error, "could not open a child log file");
            None
        }
    }
}

/// Turns the raw values of a child's two inherited pipe handles, as its
/// command line passed them, into the files it reads commands from and
/// writes messages to.
///
/// # Safety
///
/// Each value must be an inherited pipe handle this process owns and
/// nothing else uses; each is owned by exactly one returned file.
#[must_use]
pub unsafe fn inherited_pipes(pipe_in: usize, pipe_out: usize) -> (File, File) {
    // SAFETY: the caller guarantees both are owned, unshared handles.
    unsafe {
        (
            File::from_raw_handle(pipe_in as *mut c_void),
            File::from_raw_handle(pipe_out as *mut c_void),
        )
    }
}

fn to_io(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
