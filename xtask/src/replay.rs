//! `cargo xtask replay <flight-recorder.jsonl> [--from <line>] [--to
//! <line>]`: runs `verbatim-replay` (`docs/crates/verbatim-replay.md`) with
//! the arguments given, so xtask itself builds none of Core, which keeps
//! its own build, and the documentation job's, small.

use std::process::{Command, ExitCode};

/// Runs `verbatim-replay` through cargo with `args`, and exits as it does.
pub fn run(args: &[String]) -> ExitCode {
    let status = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--package", "verbatim-replay", "--"])
        .args(args)
        .status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
        Err(error) => {
            eprintln!("xtask replay: could not run cargo: {error}");
            ExitCode::FAILURE
        }
    }
}
