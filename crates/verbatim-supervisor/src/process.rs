//! Process creation for outposts and the focus listener, through
//! `verbatim-process` (the job object, the inherited pipes, and the per-role
//! log file), and two local process and window queries the owner uses for
//! its decisions.

use std::io;
use std::os::windows::io::OwnedHandle;
use std::path::Path;

use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use verbatim_model::Pid;
use verbatim_process::arguments::{
    ListenerArguments, OutpostArguments, OutpostOptions, OutpostTarget, Pipes,
};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::EVENT_PIPE_BUFFER;
use verbatim_process::{ChildEnds, ChildSpec, Contained, launch_with_ends};

/// A 200 MB per-outpost memory cap: past it a leaking outpost's allocations
/// fail, the resulting abort ends it, and the supervisor replaces it.
const OUTPOST_MEMORY_CAP: usize = 200 * 1024 * 1024;

/// What to launch.
#[derive(Clone, Copy, Debug)]
pub(super) enum Role {
    /// An outpost watching one application for its whole life (decision D9).
    Outpost(Pid),
    /// A spare outpost, started with no application and given one later
    /// (`docs/design/focus-pipeline.md`, section 19), by its launch's id.
    Spare(verbatim_model::OutpostId),
    /// The focus listener (decision D13).
    Listener,
}

/// verbatim.exe's ends of a child's two pipes, both overlapped
/// (`docs/design/communication.md`, section 4.1); the child's ends are
/// overlapped too, served by its I/O thread's completion port, with every
/// thread of the child writing its own messages (section 4.6).
pub(super) struct ChildPipes {
    /// verbatim.exe writes commands here.
    pub(super) to_child: OwnedHandle,
    /// verbatim.exe reads the child's messages here.
    pub(super) from_child: OwnedHandle,
}

/// A launched process: its job, which kills it when dropped.
pub(super) struct Launched {
    /// The process in its job: waited on for a clean exit, and killed
    /// through the job when it does not exit in time. Every ending, for
    /// any reason, closes the job handle, which kills the process if it is
    /// still running.
    pub(super) contained: Contained,
    /// The process's own id, for logs.
    pub(super) process_id: Pid,
}

/// Launches the executable at `exe_path`, `verbatim-outpost.exe` or
/// `verbatim-listener.exe` as `role` says, contained in a kill-on-close job
/// with the memory cap (see `verbatim-process`), with the role's arguments
/// ([`verbatim_process::arguments`]). Its log is named for its application,
/// or, for a spare, whose application is not known when the log is opened,
/// `outpost-spare-<launch>`. An outpost and a spare are told `options`
/// (`--classic-uia` when remote operations are off); the listener, which
/// makes no remote operation, is told `listener`: the processes ignored
/// entirely, and the applications whose outposts have their own `WinEvent`
/// hooks in.
pub(super) fn launch(
    exe_path: &Path,
    role: Role,
    options: OutpostOptions,
    listener: ListenerArguments,
) -> io::Result<(Launched, ChildPipes)> {
    let outpost = |target| OutpostArguments { target, options };
    let (log_stem, arguments): (String, Box<dyn Fn(usize, usize) -> String>) = match role {
        Role::Outpost(pid) => (
            match image_stem(pid) {
                Some(image) => format!("outpost-{image}-{}", pid.0),
                None => format!("outpost-{}", pid.0),
            },
            {
                let arguments = outpost(OutpostTarget::Application(pid));
                Box::new(move |pipe_in, pipe_out| {
                    arguments.command_line(Pipes { pipe_in, pipe_out })
                })
            },
        ),
        Role::Spare(spare) => (format!("outpost-spare-{}", spare.0), {
            let arguments = outpost(OutpostTarget::Spare(spare));
            Box::new(move |pipe_in, pipe_out| arguments.command_line(Pipes { pipe_in, pipe_out }))
        }),
        Role::Listener => (
            "listener".to_owned(),
            Box::new(move |pipe_in, pipe_out| listener.command_line(Pipes { pipe_in, pipe_out })),
        ),
    };
    let (to_child, commands) = pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Overlapped)?;
    let (from_child, messages) = pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped)?;
    let contained = launch_with_ends(
        &ChildSpec {
            exe: exe_path,
            arguments: &*arguments,
            log_stem: &log_stem,
            memory_cap: Some(OUTPOST_MEMORY_CAP),
        },
        ChildEnds { commands, messages },
    )?;
    let pipes = ChildPipes {
        to_child,
        from_child,
    };
    let process_id = Pid(contained.pid);
    Ok((
        Launched {
            contained,
            process_id,
        },
        pipes,
    ))
}

/// Whether any visible top-level window of `pid` is reported hung by the
/// system (`IsHungAppWindow`, a local call). While it is, abandoned workers in
/// that application's outpost are expected, and a replacement outpost would
/// hang the same way.
pub(super) fn application_is_hung(pid: Pid) -> bool {
    use verbatim_win32::window::{top_level_windows, window_is_hung, window_is_visible};
    top_level_windows(pid.0)
        .into_iter()
        .any(|window| window_is_visible(window) && window_is_hung(window))
}

/// `pid`'s executable name without its extension, lower-cased, to name its
/// outpost's log: `notepad` for Notepad. `None` when it cannot be read.
fn image_stem(pid: Pid) -> Option<String> {
    // SAFETY: OpenProcess with a query-only right fails safely; the handle
    // is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid.0) }.ok()?;
    let stem = verbatim_process::held::stem_of(handle);
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
    stem
}
