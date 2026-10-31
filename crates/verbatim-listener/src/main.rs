//! `verbatim-listener.exe` — the focus listener process (decision D13).
//!
//! Spawned by Core's supervisor with `--listener --pipe-in <handle>
//! --pipe-out <handle> [--ignore-pids <pid,pid>] [--hooks-in
//! <pid:outpost,pid:outpost>]` (`verbatim_process::arguments::ListenerArguments`).
//! It holds the desktop-global UIA focus registration and the global MSAA
//! hooks, reads each UIA focus from the event's own element on a thread per
//! application, and forwards each focus fact to Core, except those of the
//! processes `--ignore-pids` names, which Verbatim ignores entirely, and
//! the MSAA facts of the applications `--hooks-in` names, whose outposts
//! have their own hooks in. Commands are read from `--pipe-in`, messages
//! written to `--pipe-out`.

use std::process::ExitCode;

use verbatim_listener::run_listener;
use verbatim_process::arguments::ListenerArguments;
use verbatim_process::child::{end_without_detach, init_tracing};
use verbatim_process::held::IgnoredProcesses;
use verbatim_process::report_error;

fn main() -> ExitCode {
    // Keep the listener's trace IDs disjoint from Core's and every
    // outpost's; they meet in Core's latency ledger.
    verbatim_model::TraceId::namespace(std::process::id());
    // The supervisor redirects this process's stderr to its log file;
    // install a subscriber so the listener's `tracing` output lands there.
    init_tracing();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((pipes, arguments)) = ListenerArguments::parse(&args) else {
        report_error!(
            "verbatim-listener is spawned by verbatim.exe. Usage:\n  \
             verbatim-listener --listener --pipe-in <handle> --pipe-out <handle> [--ignore-pids <pid,pid>] [--hooks-in <pid:outpost,pid:outpost>]"
        );
        return ExitCode::FAILURE;
    };
    // Held before anything is installed; Core holds them too, so these
    // pids still name the processes Core was told of.
    let ignored = IgnoredProcesses::hold(arguments.ignored);
    // SAFETY: the supervisor passes the values of the overlapped pipe ends
    // it created for this process, which inherited them and uses them
    // nowhere else; `inherited_pipe_handles` checks that they are distinct
    // open pipes, so a malformed command line fails here.
    let pipes = unsafe { verbatim_process::inherited_pipe_handles(pipes.pipe_in, pipes.pipe_out) };
    let (reader, writer) = match pipes {
        Ok(pipes) => pipes,
        Err(error) => {
            report_error!("listener: {error}");
            return ExitCode::from(2);
        }
    };
    match run_listener(reader, writer, ignored, &arguments.hooks_in) {
        Ok(()) => end_without_detach(0),
        Err(error) => {
            report_error!("listener pipe loop ended with error: {error}");
            end_without_detach(1)
        }
    }
}
