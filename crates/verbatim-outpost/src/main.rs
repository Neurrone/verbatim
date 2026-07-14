//! `verbatim-outpost.exe` — the per-application outpost process
//! (architecture section 1, decision D9).
//!
//! One outpost per target application: it owns that app's accessibility
//! subscriptions (UIA event registrations, out-of-context `WinEvent` hooks),
//! maintains the cached normalized tree fragment, and answers queries, all from
//! its own process so a hung or crashed app never touches Core.
//!
//! Two ways to run:
//!
//! - `--pipe-in <handle> --pipe-out <handle>`: the production mode, spawned by
//!   the Core supervisor with two inherited anonymous-pipe handle values passed
//!   as decimal. Commands are read from `--pipe-in`, messages written to
//!   `--pipe-out`.
//! - `--attach <pid>`: a dev mode that watches `<pid>` directly and prints
//!   outbound messages as JSON lines to stdout, for standalone testing without
//!   Core.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::process::ExitCode;

use verbatim_outpost::{run_attach, run_pipe};

fn main() -> ExitCode {
    // Keep this outpost's trace IDs disjoint from Core's and every other
    // outpost's; they meet in Core's latency ledger.
    verbatim_model::TraceId::namespace(std::process::id());
    let args: Vec<String> = std::env::args().collect();
    match parse_args(&args) {
        Some(Mode::Pipe { pipe_in, pipe_out }) => {
            // SAFETY: the handle values name pipe ends the supervisor created
            // and this process inherited; each is owned by exactly one File.
            let reader = unsafe { File::from_raw_handle(pipe_in as *mut c_void) };
            let writer = unsafe { File::from_raw_handle(pipe_out as *mut c_void) };
            match run_pipe(Box::new(reader), Box::new(writer)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("outpost pipe loop ended with error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Mode::Attach { pid }) => match run_attach(pid) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("outpost attach mode ended with error: {error}");
                ExitCode::FAILURE
            }
        },
        None => {
            eprintln!(
                "verbatim-outpost is spawned by verbatim.exe. Usage:\n  \
                 verbatim-outpost --pipe-in <handle> --pipe-out <handle>\n  \
                 verbatim-outpost --attach <pid>   (dev mode: prints JSON to stdout)"
            );
            ExitCode::FAILURE
        }
    }
}

enum Mode {
    Pipe { pipe_in: usize, pipe_out: usize },
    Attach { pid: u32 },
}

/// Parses the command line into a run [`Mode`]. Returns `None` on unrecognized
/// or incomplete arguments.
fn parse_args(args: &[String]) -> Option<Mode> {
    let mut pipe_in = None;
    let mut pipe_out = None;
    let mut attach = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--pipe-in" => pipe_in = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--pipe-out" => pipe_out = args.get(index + 1).and_then(|v| v.parse().ok()),
            "--attach" => attach = args.get(index + 1).and_then(|v| v.parse().ok()),
            _ => {}
        }
        index += 2;
    }
    if let Some(pid) = attach {
        return Some(Mode::Attach { pid });
    }
    match (pipe_in, pipe_out) {
        (Some(pipe_in), Some(pipe_out)) => Some(Mode::Pipe { pipe_in, pipe_out }),
        _ => None,
    }
}
