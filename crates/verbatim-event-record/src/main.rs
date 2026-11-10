//! `verbatim-event-record`: records what an application raises, through
//! MSAA and UI Automation, as JSON Lines, and prints a recording as
//! sentences (`books/dev/src/reference/crates/verbatim-event-record.md`).

mod behind;
mod hooks;
mod line;
mod print;
mod record;
mod steps;
mod stop_key;
mod target;
mod uia_events;
mod writer;

use std::io::Write as _;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use verbatim_process::child::end_without_detach;
use verbatim_process::report_error;

use record::Options;
use stop_key::DEFAULT_STOP_KEY;
use target::{Selector, parse_window};

/// The command line (Dickson's approved contract of 2026-11-10).
#[derive(Parser)]
#[command(
    name = "verbatim-event-record",
    about = "Records what an application raises through MSAA and UI Automation",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    /// `print <file>`, instead of recording.
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    record: RecordArgs,
}

/// The target: exactly one of the two.
#[derive(Args)]
#[group(required = true, multiple = false)]
struct TargetArgs {
    /// The process to record, by name or id.
    #[arg(long)]
    process: Option<String>,
    /// The top-level window to record, by handle, decimal or `0x` hex.
    #[arg(long, value_parser = parse_window)]
    window: Option<isize>,
}

/// The recording's options.
#[derive(Args)]
struct RecordArgs {
    /// Ends the recording after this many seconds.
    #[arg(long)]
    seconds: Option<u64>,
    /// The key that ends the recording, in the agent's key vocabulary.
    #[arg(long, default_value = DEFAULT_STOP_KEY)]
    stop_key: String,
    /// The file written, `verbatim-events-<UTC time>.jsonl` by default.
    #[arg(long)]
    output: Option<PathBuf>,
    /// A file of steps run through the end-to-end agent while recording.
    #[arg(long)]
    steps: Option<PathBuf>,
    /// The agent's host and port, used only with `--steps`.
    #[arg(long, requires = "steps", default_value = "127.0.0.1:44001")]
    agent: String,
}

/// The one subcommand.
#[derive(Subcommand)]
enum Command {
    /// Prints a recording as sentences.
    Print {
        /// The recording.
        file: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Print { file }) => print_file(&file),
        None => record::run(options(cli.target, cli.record)),
    };
    end_without_detach(code)
}

/// The recording's options from the command line, which `clap` has made
/// name exactly one target.
fn options(target: TargetArgs, args: RecordArgs) -> Options {
    let target = match (target.process, target.window) {
        (Some(process), None) => Selector::Process(process),
        (None, Some(window)) => Selector::Window(window),
        _ => unreachable!("clap requires exactly one of --process and --window"),
    };
    Options {
        target,
        seconds: args.seconds,
        stop_key: args.stop_key,
        output: args.output,
        steps: args.steps,
        agent: args.agent,
    }
}

/// Prints `file` as sentences, returning the exit code.
fn print_file(file: &std::path::Path) -> u32 {
    match print::print(file) {
        Ok(text) => {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
            0
        }
        Err(message) => {
            report_error!("{message}");
            1
        }
    }
}
