//! `verbatim-outpost.exe` — the per-application outpost process
//! (architecture section 1, decision D9).
//!
//! One outpost per target application: it owns that app's accessibility
//! subscriptions (UIA event registrations, out-of-context `WinEvent` hooks),
//! maintains the cached normalized tree fragment, and answers queries, all from
//! its own process so a hung or crashed app never touches Core.
//!
//! Three ways to run:
//!
//! - `--pipe-in <handle> --pipe-out <handle> --target-pid <pid>`: the
//!   production mode, spawned by the Core supervisor with two inherited
//!   overlapped pipe-handle values passed as decimal and the application this
//!   outpost watches for its whole life (decision D9: set once, never
//!   retargeted). Commands are read from `--pipe-in`, messages written to
//!   `--pipe-out`.
//! - `--pipe-in <handle> --pipe-out <handle> --spare <launch>`: the same,
//!   as a spare the supervisor keeps waiting
//!   (`docs/design/focus-pipeline.md`, section 19): it starts with no
//!   application, and watches the one the supervisor's `Assign` names for
//!   the rest of its life, set once as at spawn. `<launch>` is the
//!   supervisor's number for this launch, which names its log.
//! - `--listener --pipe-in <handle> --pipe-out <handle> [--ignore-pids
//!   <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]`: the focus-listener
//!   mode (decision D13), spawned by the same supervisor into its dedicated
//!   slot with no target pid. It holds the desktop-global UIA focus
//!   registration and the global MSAA hooks, reads each UIA focus from the
//!   event's own element on a thread per application, and forwards each
//!   focus fact to Core, except those of the processes `--ignore-pids` names, which
//!   Verbatim ignores entirely, and the MSAA facts of the applications
//!   `--hooks-in` names, whose outposts have their own hooks in; the same
//!   pipe conventions apply.
//! - `--attach <pid>`: a dev mode that watches `<pid>` directly and prints
//!   outbound messages as JSON lines to stdout, for standalone testing without
//!   Core.
//!
//! An outpost started for its application, with `--target-pid`, is also
//! given `--app-lock <handle>:<created>`: the application's lock
//! (`docs/design/focus-pipeline.md`, section 20.4), a handle it inherited,
//! which it sets as its process's lock before it installs its hook.
//!
//! Every mode also takes `--classic-uia`, which turns off UIA remote
//! operations for the process's whole life (`uia.remote_operations = false`
//! in `settings.toml`, which the supervisor passes on); the listener makes
//! none whatever it is given.

use std::process::ExitCode;

use verbatim_outpost::{OutpostOptions, run_attach, run_listener, run_pipe};

/// Writes a line to standard error, as `eprintln!` does, but ignores a
/// failed write instead of panicking: a full disk or a closed pipe under
/// redirected output must not take the process down.
macro_rules! report_error {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

fn main() -> ExitCode {
    // Keep this outpost's trace IDs disjoint from Core's and every other
    // outpost's; they meet in Core's latency ledger.
    verbatim_model::TraceId::namespace(std::process::id());
    // The supervisor redirects this process's stderr to a per-role log file
    // (see `verbatim-outpost::supervisor`); install a subscriber so the
    // outpost's `tracing` output actually lands there instead of nowhere.
    init_tracing();
    let args: Vec<String> = std::env::args().collect();
    match parse_args(&args) {
        Some(Mode::Pipe {
            pipe_in,
            pipe_out,
            target_pid,
            lock,
            options,
        }) => {
            if let (Some(target_pid), Some(grant)) = (target_pid, lock) {
                // SAFETY: the supervisor names the handle it had this
                // process inherit for the application's lock, used nowhere
                // else.
                unsafe {
                    verbatim_outpost::set_application_lock(verbatim_model::Pid(target_pid), grant);
                }
            }
            // SAFETY: the supervisor passes the values of the overlapped
            // pipe ends it created for this process, which inherited them
            // and uses them nowhere else; `inherited_pipe_handles` checks
            // that they are distinct open pipes, so a malformed command line
            // fails here.
            let pipes = unsafe { verbatim_process::inherited_pipe_handles(pipe_in, pipe_out) };
            let (reader, writer) = match pipes {
                Ok(pipes) => pipes,
                Err(error) => {
                    report_error!("outpost: {error}");
                    return ExitCode::from(2);
                }
            };
            match run_pipe(reader, writer, target_pid, options) {
                Ok(()) => end_without_detach(0),
                Err(error) => {
                    report_error!("outpost pipe loop ended with error: {error}");
                    end_without_detach(1)
                }
            }
        }
        Some(Mode::Listener {
            pipe_in,
            pipe_out,
            ignore_pids,
            hooks_in,
        }) => {
            // Held before anything is installed; Core holds them too, so
            // these pids still name the processes Core was told of.
            let ignored = verbatim_outpost::supervisor::IgnoredProcesses::hold(
                verbatim_outpost::supervisor::parse_pids(&ignore_pids),
            );
            // SAFETY: the supervisor passes the values of the overlapped
            // pipe ends it created for this process, which inherited them
            // and uses them nowhere else; `inherited_pipe_handles` checks
            // that they are distinct open pipes, so a malformed command line
            // fails here.
            let pipes = unsafe { verbatim_process::inherited_pipe_handles(pipe_in, pipe_out) };
            let (reader, writer) = match pipes {
                Ok(pipes) => pipes,
                Err(error) => {
                    report_error!("outpost: {error}");
                    return ExitCode::from(2);
                }
            };
            let hooks_in = verbatim_outpost::listener::parse_hooks_in(&hooks_in);
            match run_listener(reader, writer, ignored, &hooks_in) {
                Ok(()) => end_without_detach(0),
                Err(error) => {
                    report_error!("listener pipe loop ended with error: {error}");
                    end_without_detach(1)
                }
            }
        }
        Some(Mode::Attach { pid, options }) => match run_attach(pid, options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report_error!("outpost attach mode ended with error: {error}");
                ExitCode::FAILURE
            }
        },
        None => {
            report_error!(
                "verbatim-outpost is spawned by verbatim.exe. Usage:\n  \
                 verbatim-outpost --pipe-in <handle> --pipe-out <handle> --target-pid <pid> [--classic-uia]\n  \
                 verbatim-outpost --pipe-in <handle> --pipe-out <handle> --spare <launch> [--classic-uia]\n  \
                 verbatim-outpost --listener --pipe-in <handle> --pipe-out <handle> [--ignore-pids <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]\n  \
                 verbatim-outpost --attach <pid> [--classic-uia]   (dev mode: prints JSON to stdout)"
            );
            ExitCode::FAILURE
        }
    }
}

/// Ends this process with `code` once an outpost or the listener has shut
/// down cleanly: every UIA handler and hook removed, every call finished,
/// every object released, and COM left. It ends with `TerminateProcess`
/// rather than by returning from `main`, so no DLL's process-detach code
/// runs: `UIAutomationCore.dll`'s sometimes hangs or crashes in a process
/// that has used UIA as a client (`docs/architecture.md`, "Process
/// lifetime"). Nothing is left for that code to do; standard error, the
/// log, is written unbuffered.
fn end_without_detach(code: u32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: the calling process's pseudo-handle, which needs no closing.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends the calling process.
    let ended = unsafe { TerminateProcess(this_process, code) };
    // Unreachable unless the call failed; exit the ordinary way then.
    report_error!("the process could not end itself ({ended:?}); exiting normally");
    std::process::exit(i32::try_from(code).unwrap_or(1))
}

/// Installs a tracing subscriber writing to this process's stderr (which the
/// supervisor redirects to a log file). Respects `RUST_LOG`, defaulting to
/// `info` so the worker and watchdog diagnostics land without extra
/// configuration. Never a hard failure: if a subscriber is somehow already
/// set, `try_init` returns an error that is ignored.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // A write that fails (a full disk, a closed pipe) is dropped: by
        // default the formatter reports it with `eprint!`, which panics when
        // standard error fails too.
        .log_internal_errors(false)
        .with_writer(std::io::stderr)
        .try_init();
}

enum Mode {
    Pipe {
        pipe_in: usize,
        pipe_out: usize,
        /// The application, or `None` for a spare.
        target_pid: Option<u32>,
        /// The application's lock it inherited, for an outpost started for
        /// it.
        lock: Option<verbatim_outpost::protocol::LockGrant>,
        options: OutpostOptions,
    },
    Listener {
        pipe_in: usize,
        pipe_out: usize,
        /// The pids of the processes to ignore, separated by commas.
        ignore_pids: String,
        /// The applications whose outposts have their own hooks in, as
        /// `pid:outpost` pairs separated by commas.
        hooks_in: String,
    },
    Attach {
        pid: u32,
        options: OutpostOptions,
    },
}

/// Parses the command line into a run [`Mode`]. Returns `None` on unrecognized
/// or incomplete arguments.
fn parse_args(args: &[String]) -> Option<Mode> {
    let mut pipe_in = None;
    let mut pipe_out = None;
    let mut target_pid = None;
    let mut spare = None::<u64>;
    let mut attach = None;
    let mut listener = false;
    let mut ignore_pids = String::new();
    let mut hooks_in = String::new();
    let mut lock = None;
    let mut options = OutpostOptions::default();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--pipe-in" => pipe_in = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--pipe-out" => pipe_out = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--target-pid" => target_pid = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--spare" => spare = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--attach" => attach = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--ignore-pids" => ignore_pids = args.get(index + 1).cloned().unwrap_or_default(),
            "--hooks-in" => hooks_in = args.get(index + 1).cloned().unwrap_or_default(),
            verbatim_outpost::protocol::LockGrant::ARG => {
                lock = args
                    .get(index + 1)
                    .and_then(|value| verbatim_outpost::protocol::LockGrant::parse_arg(value));
            }
            // A bare flag, not a valued option; do not consume a following arg.
            "--listener" => {
                listener = true;
                index += 1;
                continue;
            }
            "--classic-uia" => {
                options.remote_operations = false;
                index += 1;
                continue;
            }
            _ => {}
        }
        index += 2;
    }
    if let Some(pid) = attach {
        return Some(Mode::Attach { pid, options });
    }
    if listener {
        return match (pipe_in, pipe_out) {
            (Some(pipe_in), Some(pipe_out)) => Some(Mode::Listener {
                pipe_in,
                pipe_out,
                ignore_pids,
                hooks_in,
            }),
            _ => None,
        };
    }
    match (pipe_in, pipe_out, target_pid, spare) {
        (Some(pipe_in), Some(pipe_out), Some(target_pid), None) => Some(Mode::Pipe {
            pipe_in,
            pipe_out,
            target_pid: Some(target_pid),
            lock,
            options,
        }),
        (Some(pipe_in), Some(pipe_out), None, Some(_)) => Some(Mode::Pipe {
            pipe_in,
            pipe_out,
            target_pid: None,
            lock: None,
            options,
        }),
        _ => None,
    }
}
