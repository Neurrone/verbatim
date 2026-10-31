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
//! - `--attach <pid>`: a dev mode that watches `<pid>` directly and prints
//!   outbound messages as JSON lines to stdout, for standalone testing without
//!   Core.
//!
//! Each mode also takes `--classic-uia`, which turns off UIA remote
//! operations for the process's whole life (`uia.remote_operations = false`
//! in `settings.toml`, which the supervisor passes on). The first two are
//! `verbatim_process::arguments::OutpostArguments`, which refuses the focus
//! listener's arguments: the listener is `verbatim-listener.exe`.

use std::process::ExitCode;

use verbatim_outpost::{run_attach, run_pipe};
use verbatim_process::arguments::{OutpostArguments, OutpostOptions, OutpostTarget};
use verbatim_process::child::{end_without_detach, init_tracing};
use verbatim_process::report_error;

fn main() -> ExitCode {
    // Keep this outpost's trace IDs disjoint from Core's and every other
    // outpost's; they meet in Core's latency ledger.
    verbatim_model::TraceId::namespace(std::process::id());
    // The supervisor redirects this process's stderr to a per-role log file
    // (see `verbatim-supervisor`); install a subscriber so the outpost's
    // `tracing` output actually lands there instead of nowhere.
    init_tracing();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pid) = attach_pid(&args) {
        let options = OutpostOptions {
            remote_operations: !args.iter().any(|arg| arg == "--classic-uia"),
        };
        return match run_attach(pid, options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report_error!("outpost attach mode ended with error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some((pipes, arguments)) = OutpostArguments::parse(&args) else {
        report_error!(
            "verbatim-outpost is spawned by verbatim.exe. Usage:\n  \
             verbatim-outpost --pipe-in <handle> --pipe-out <handle> --target-pid <pid> [--classic-uia]\n  \
             verbatim-outpost --pipe-in <handle> --pipe-out <handle> --spare <launch> [--classic-uia]\n  \
             verbatim-outpost --attach <pid> [--classic-uia]   (dev mode: prints JSON to stdout)"
        );
        return ExitCode::FAILURE;
    };
    // SAFETY: the supervisor passes the values of the overlapped pipe ends
    // it created for this process, which inherited them and uses them
    // nowhere else; `inherited_pipe_handles` checks that they are distinct
    // open pipes, so a malformed command line fails here.
    let pipes = unsafe { verbatim_process::inherited_pipe_handles(pipes.pipe_in, pipes.pipe_out) };
    let (reader, writer) = match pipes {
        Ok(pipes) => pipes,
        Err(error) => {
            report_error!("outpost: {error}");
            return ExitCode::from(2);
        }
    };
    let target_pid = match arguments.target {
        OutpostTarget::Application(pid) => Some(pid.0),
        OutpostTarget::Spare(_) => None,
    };
    match run_pipe(reader, writer, target_pid, arguments.options) {
        Ok(()) => end_without_detach(0),
        Err(error) => {
            report_error!("outpost pipe loop ended with error: {error}");
            end_without_detach(1)
        }
    }
}

/// The application `--attach` names, for the dev mode: the value after the
/// flag, which takes precedence over every other mode.
fn attach_pid(args: &[String]) -> Option<u32> {
    let at = args.iter().position(|arg| arg == "--attach")?;
    args.get(at + 1)?.parse().ok()
}
