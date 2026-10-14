//! Contained child processes: what Core launches outposts, the focus
//! listener, and synthesizer hosts with (architecture section 1).
//!
//! A child is created inside a kill-on-close job object
//! (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), so it is contained from its first
//! instruction, and dies when Core's job handle closes, whatever ends Core.
//! Creation and containment are one step: a Core that dies while
//! launching a child cannot leave it outside a job, running or suspended.
//! It talks to Core over two pipes the caller makes, typically with
//! [`overlapped::pipe`], whose child ends it inherits; it inherits exactly
//! those and its log file, never another child's handles, so two launches
//! running at once cannot keep each other's pipes open. Those handles are
//! made non-inheritable and are inheritable only for the duration of the `CreateProcessW` call that launches the child, so
//! another process creation elsewhere in Core that inherits every
//! inheritable handle can pick them up only if it runs at that same moment;
//! Core creates processes only through this module today. Each child's
//! standard output and error go to a log file named for it in this
//! launch's log directory.
//!
//! The [`session`] module reads the calling process's own session, which
//! Verbatim and the test agent check before starting.

pub mod event;
pub mod overlapped;
pub mod port_queue;
pub mod scheduling;
pub mod session;
pub mod transport;

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::{fs, io};

use windows::Win32::Foundation::{
    GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, INVALID_HANDLE_VALUE,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TYPE_PIPE, GetFileType, OPEN_ALWAYS,
};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
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
}

/// The exit code a child is given when [`Contained::kill`] ends it: the
/// ASCII letters "KILL" read as a number, so a process ended this way is
/// told apart from one that exited on its own (0) or crashed (an
/// exception code such as `0xC0000005`).
pub const KILLED_EXIT_CODE: u32 = 0x4B49_4C4C;

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

impl Contained {
    /// Whether the process has exited, by whatever means. A process that
    /// is being ended counts as exited only once the system has finished
    /// ending it.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        // SAFETY: the process handle this value owns, open for its life;
        // a zero timeout only reads whether it is signaled.
        let state = unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), 0) };
        state == WAIT_OBJECT_0
    }

    /// Waits up to `timeout` for the process to exit, and says whether it
    /// has.
    #[must_use]
    pub fn wait_for_exit(&self, timeout: std::time::Duration) -> bool {
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the process handle this value owns, open for its life.
        let state =
            unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), milliseconds) };
        state == WAIT_OBJECT_0
    }

    /// The process's exit code, once it has exited; `None` while it runs
    /// or when the code cannot be read.
    #[must_use]
    pub fn exit_code(&self) -> Option<u32> {
        if !self.has_exited() {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: the process handle this value owns; `code` is a local.
        unsafe {
            windows::Win32::System::Threading::GetExitCodeProcess(
                HANDLE(self.process.as_raw_handle()),
                &raw mut code,
            )
        }
        .ok()
        .map(|()| code)
    }

    /// Ends every process in the child's job with [`KILLED_EXIT_CODE`],
    /// and waits for the system to finish ending the child, so the kill is
    /// over when this returns. Dropping a `Contained` kills it too, but
    /// with whatever exit code the system gives a job closed under it; a
    /// kill made on purpose says so in the exit code.
    ///
    /// # Errors
    ///
    /// Returns the error if the job cannot be terminated.
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: the job handle this value owns, open for its life.
        unsafe { TerminateJobObject(HANDLE(self.job.as_raw_handle()), KILLED_EXIT_CODE) }
            .map_err(io::Error::other)?;
        // SAFETY: as in `wait_for_exit`; the process ends promptly once
        // its job is terminated.
        let _ = unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), u32::MAX) };
        Ok(())
    }
}

/// The child's ends of two pipes the caller made, for
/// [`launch_with_ends`]: typically the other ends of
/// [`overlapped::pipe`]s, whose Verbatim ends the caller keeps.
pub struct ChildEnds {
    /// The end the child reads commands from.
    pub commands: OwnedHandle,
    /// The end the child writes messages to.
    pub messages: OwnedHandle,
}

/// Launches a contained child as `spec` describes, over pipes the caller
/// made: the child inherits `ends`, and the caller keeps its own ends of
/// those pipes, overlapped or synchronous as it made them. The ends are
/// closed here once the child has inherited them, whether or not the spawn
/// succeeded, so the child's exit is seen as the end of the stream on the
/// caller's end.
///
/// # Errors
///
/// Returns the error of whichever step failed: creating the job or
/// spawning the process.
pub fn launch_with_ends(spec: &ChildSpec<'_>, ends: ChildEnds) -> io::Result<Contained> {
    let child_in = HANDLE(ends.commands.into_raw_handle());
    let child_out = HANDLE(ends.messages.into_raw_handle());
    match create_job(spec.memory_cap) {
        Ok(job) => spawn_contained(spec, job, [child_in, child_out]),
        Err(error) => {
            close_handle(child_in);
            close_handle(child_out);
            Err(error)
        }
    }
}

/// Spawns `spec`'s executable in `job`, inheriting `pipe_ends` (the end it
/// reads commands from, then the end it writes messages to), which are
/// closed here whatever the outcome.
fn spawn_contained(
    spec: &ChildSpec<'_>,
    job: OwnedHandle,
    pipe_ends: [HANDLE; 2],
) -> io::Result<Contained> {
    let [child_in, child_out] = pipe_ends;
    let arguments = (spec.arguments)(child_in.0 as usize, child_out.0 as usize);
    let command_line = format!("\"{}\" {arguments}", spec.exe.display());
    // Best-effort: a failed log open leaves the child unredirected, never
    // unspawned.
    let log = child_log_handle(spec.exe, spec.log_stem);
    let spawned = spawn_in_job(&command_line, &job, pipe_ends, log);
    if let Some(log) = log {
        // The child inherited its own copy; drop ours whether or not the
        // spawn succeeded.
        close_handle(log);
    }
    // The child has inherited its pipe ends; close ours so end of stream is
    // seen when it exits.
    close_handle(child_in);
    close_handle(child_out);
    let spawned = spawned?;
    Ok(Contained {
        job,
        // SAFETY: `spawned.process` is a valid process handle we now own.
        process: unsafe { OwnedHandle::from_raw_handle(spawned.process.0 as RawHandle) },
        pid: spawned.pid,
    })
}

/// Marks `handles` inheritable, or not.
fn set_inheritable(handles: &[HANDLE], inheritable: bool) -> io::Result<()> {
    let flags = if inheritable {
        HANDLE_FLAG_INHERIT
    } else {
        HANDLE_FLAGS(0)
    };
    for &handle in handles {
        // SAFETY: each handle is a valid handle this process owns; the call
        // changes only its inherit flag.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, flags) }.map_err(to_io)?;
    }
    Ok(())
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
    // SAFETY: `job` is a valid job handle we now own; owned from here, it
    // is closed on every path below.
    let job = unsafe { OwnedHandle::from_raw_handle(job.0 as RawHandle) };
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
            HANDLE(job.as_raw_handle().cast::<c_void>()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
        )
        .map_err(to_io)?;
    }
    Ok(job)
}

/// The handle and id a spawned process yields.
struct Spawned {
    process: HANDLE,
    pid: u32,
}

/// Creates the child inside `job`. The child inherits only the handles
/// named in a `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`:
/// its two pipe ends, `pipe_ends`, and, when `log` is `Some`, the log file
/// its standard output and error are redirected to (its standard input
/// too, harmlessly — a child reads its command pipe, never stdin).
fn spawn_in_job(
    command_line: &str,
    job: &OwnedHandle,
    pipe_ends: [HANDLE; 2],
    log: Option<HANDLE>,
) -> io::Result<Spawned> {
    let mut command: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut inherited = pipe_ends.to_vec();
    inherited.extend(log);
    let jobs = [HANDLE(job.as_raw_handle().cast::<c_void>())];

    // Size, allocate, and fill the attribute list naming the inherited
    // handles.
    let mut size = 0usize;
    // SAFETY: the first call only reports the size needed; its failure with
    // "insufficient buffer" is expected.
    unsafe {
        let _ = InitializeProcThreadAttributeList(None, 2, None, &raw mut size);
    }
    // The list holds pointer-sized fields, so its buffer is allocated in
    // `usize`s, which aligns it for them.
    let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
    let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
    // SAFETY: `buffer` is at least `size` bytes, as the first call asked
    // for, aligned for a pointer, and outlives every use of `attributes`
    // below, which ends with the delete.
    unsafe {
        InitializeProcThreadAttributeList(Some(attributes), 2, None, &raw mut size)
            .map_err(to_io)?;
    }
    let result = (|| {
        // SAFETY: `attributes` is initialized; `inherited` outlives the
        // CreateProcessW call that reads it, and its size is given.
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
        }
        .map_err(to_io)?;
        // SAFETY: as above, for `jobs`, one handle.
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                Some(jobs.as_ptr().cast()),
                size_of::<HANDLE>(),
                None,
                None,
            )
        }
        .map_err(to_io)?;
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
        // The handle list names only inheritable handles, so the child's
        // handles are inheritable for this call alone.
        if let Err(error) = set_inheritable(&inherited, true) {
            let _ = set_inheritable(&inherited, false);
            return Err(error);
        }
        // SAFETY: `command` is a NUL-terminated writable UTF-16 buffer; the
        // startup information is correctly sized and carries the attribute
        // list; bInheritHandles is true, restricted by that list.
        let created = unsafe {
            CreateProcessW(
                None,
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                None,
                None,
                (&raw const startup).cast(),
                &raw mut info,
            )
        };
        // Best effort: the caller closes these handles next anyway.
        let _ = set_inheritable(&inherited, false);
        created.map_err(to_io)?;
        close_handle(info.hThread);
        Ok(Spawned {
            process: info.hProcess,
            pid: info.dwProcessId,
        })
    })();
    // SAFETY: `attributes` was initialized above (a failed initialization
    // returned before here) and is no longer used.
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
/// redirected into, returning an append-mode handle, not yet inheritable,
/// or `None`
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
    // SAFETY: `wide` is a NUL-terminated path alive across the call; no
    // security attributes, so the handle is not inheritable; append-mode
    // writes are atomic at end of file, so concurrent writers do not
    // interleave.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_APPEND_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
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
/// writes messages to: [`inherited_pipe_handles`], each wrapped in a
/// [`File`]. For synchronous ends only: a `File` must never be given an
/// overlapped handle.
///
/// # Errors
///
/// As [`inherited_pipe_handles`].
///
/// # Safety
///
/// As [`inherited_pipe_handles`].
pub unsafe fn inherited_pipes(pipe_in: usize, pipe_out: usize) -> io::Result<(File, File)> {
    // SAFETY: the caller's guarantee, passed on.
    let (reader, writer) = unsafe { inherited_pipe_handles(pipe_in, pipe_out) }?;
    Ok((File::from(reader), File::from(writer)))
}

/// Turns the raw values of a child's two inherited pipe handles, as its
/// command line passed them, into the handles it reads commands from and
/// writes messages to, whether they were opened synchronous or overlapped
/// ([`launch_with_ends`]).
///
/// The values come from the command line, so what can be checked is: they
/// must differ, and each must name an open pipe handle in this process. A
/// value that fails the check is an error rather than a handle that would
/// close some other handle when dropped.
///
/// # Errors
///
/// Returns an error when the values are equal or either is not an open
/// pipe handle.
///
/// # Safety
///
/// Each value must be an inherited pipe handle this process owns and
/// nothing else uses; each is owned by exactly one returned handle. The
/// checks above catch a malformed command line, not a pipe handle some
/// other part of the process owns.
pub unsafe fn inherited_pipe_handles(
    pipe_in: usize,
    pipe_out: usize,
) -> io::Result<(OwnedHandle, OwnedHandle)> {
    if pipe_in == pipe_out {
        return Err(io::Error::other(
            "the two pipe handle values on the command line are the same",
        ));
    }
    for value in [pipe_in, pipe_out] {
        let handle = HANDLE(value as *mut c_void);
        let mut flags = 0u32;
        // SAFETY: GetHandleInformation only reads the handle table, and
        // fails for a value that names no open handle.
        let open = unsafe { GetHandleInformation(handle, &raw mut flags) };
        // SAFETY: GetFileType only queries an open handle, checked above.
        let is_pipe = open.is_ok() && unsafe { GetFileType(handle) } == FILE_TYPE_PIPE;
        if !is_pipe {
            return Err(io::Error::other(format!(
                "the handle value {value} on the command line is not an open pipe"
            )));
        }
    }
    // SAFETY: both values name open pipe handles (checked above), distinct
    // from each other, and the caller guarantees they are owned and
    // unshared.
    let reader = unsafe { OwnedHandle::from_raw_handle(pipe_in as *mut c_void) };
    // SAFETY: as above.
    let writer = unsafe { OwnedHandle::from_raw_handle(pipe_out as *mut c_void) };
    Ok((reader, writer))
}

fn to_io(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
