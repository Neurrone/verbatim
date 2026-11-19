//! `cargo xtask e2e <scenario>... [--runs <n>]`: runs end-to-end scenarios
//! against this machine, each `n` times, with an agent this command starts
//! and ends (`books/dev/src/how-to/e2e.md`, "Running scenarios locally"). The
//! agent is this command's child, inside its job (`crate::contain`), so
//! stopping the command, by any means, ends the agent, and the agent's
//! launches end with it; no agent is left listening for the next run.
//!
//! Each run is its own `cargo test` of the one scenario, so each has its
//! own log, `target/e2e-runs/<scenario>-<run>.log`. The suite writes a
//! scenario's artifacts to one folder that its next run replaces, so a
//! failed run's folder is moved to `target/e2e-runs/<scenario>-<run>`,
//! where the runs after it cannot overwrite it. The command empties
//! `target/e2e-runs` as it starts, so everything there is from its own
//! runs. A line per run, with how long it took, is printed as it ends, and
//! the command fails when any run failed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use crate::agent::{Agent, ENDPOINT};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the workspace")
        .to_path_buf()
}

/// Runs the scenarios.
pub fn run(args: &[String]) -> ExitCode {
    let (scenarios, runs) = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: cargo xtask e2e <scenario>... [--runs <n>]");
            return ExitCode::from(2);
        }
    };
    match run_all(&scenarios, runs) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// The scenarios named, and how many times each runs.
fn parse(args: &[String]) -> Result<(Vec<String>, u32), String> {
    let mut scenarios = Vec::new();
    let mut runs = 1;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--runs" {
            runs = args
                .next()
                .and_then(|value| value.parse().ok())
                .filter(|runs| *runs > 0)
                .ok_or("--runs takes a count above zero")?;
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            scenarios.push(arg.clone());
        }
    }
    if scenarios.is_empty() {
        return Err("name at least one scenario".to_owned());
    }
    Ok((scenarios, runs))
}

/// Builds, starts the agent, and runs each scenario `runs` times, round
/// after round; true when every run passed.
fn run_all(scenarios: &[String], runs: u32) -> Result<bool, String> {
    let root = workspace();
    crate::agent::build(&root)?;
    cargo(
        &root,
        &["test", "-p", "verbatim-e2e", "--test", "e2e", "--no-run"],
    )?;
    let logs = root.join(r"target\e2e-runs");
    match fs::remove_dir_all(&logs) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not empty {}: {error}", logs.display())),
    }
    fs::create_dir_all(&logs)
        .map_err(|error| format!("could not create {}: {error}", logs.display()))?;
    let artifacts = verbatim_e2e::artifacts::artifacts_root();
    let _agent = Agent::start(&root, &logs)?;
    let mut all_passed = true;
    for run in 1..=runs {
        for scenario in scenarios {
            let log = logs.join(format!("{scenario}-{run}.log"));
            let started = Instant::now();
            let passed = run_one(&root, scenario, &log)?;
            let seconds = started.elapsed().as_secs();
            all_passed &= passed;
            if passed {
                println!(
                    "run {run} {scenario}: pass in {seconds} s ({})",
                    log.display()
                );
            } else {
                let kept = logs.join(format!("{scenario}-{run}"));
                keep_artifacts(
                    &verbatim_e2e::artifacts::scenario_dir(&artifacts, scenario),
                    &kept,
                )?;
                println!(
                    "run {run} {scenario}: fail in {seconds} s ({}, artifacts in {})",
                    log.display(),
                    kept.display()
                );
            }
        }
    }
    Ok(all_passed)
}

/// Moves a failed run's artifacts folder, `written`, to `kept`. A run that
/// failed before writing any artifacts leaves no folder, and nothing is
/// moved.
fn keep_artifacts(written: &Path, kept: &Path) -> Result<(), String> {
    match fs::rename(written, kept) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not move {} to {}: {error}",
            written.display(),
            kept.display()
        )),
    }
}

/// Runs `cargo` with `args` in `root`, failing unless it succeeds.
fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("could not run cargo {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed ({status})", args.join(" ")))
    }
}

/// Runs `scenario` once, its output in `log`; true when it passed.
fn run_one(root: &Path, scenario: &str, log: &Path) -> Result<bool, String> {
    let file = fs::File::create(log)
        .map_err(|error| format!("could not create {}: {error}", log.display()))?;
    let errors = file
        .try_clone()
        .map_err(|error| format!("could not open {}: {error}", log.display()))?;
    let status = Command::new(env!("CARGO"))
        .args([
            "test",
            "-q",
            "-p",
            "verbatim-e2e",
            "--test",
            "e2e",
            "--",
            "--ignored",
            "--exact",
            scenario,
            "--test-threads=1",
        ])
        .current_dir(root)
        .env("VERBATIM_E2E_ENDPOINT", ENDPOINT)
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        .status()
        .map_err(|error| format!("could not run {scenario}: {error}"))?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{keep_artifacts, parse};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|&value| value.to_owned()).collect()
    }

    #[test]
    fn scenarios_and_runs_are_read_in_any_order() {
        assert_eq!(
            parse(&args(&["a", "--runs", "3", "b"])),
            Ok((args(&["a", "b"]), 3))
        );
    }

    #[test]
    fn no_scenario_or_a_zero_count_is_refused() {
        assert!(parse(&args(&["--runs", "2"])).is_err());
        assert!(parse(&args(&["a", "--runs", "0"])).is_err());
    }

    #[test]
    fn a_failed_runs_artifacts_move_out_of_the_next_runs_way() {
        let root =
            std::env::temp_dir().join(format!("xtask-keep-artifacts-{}", std::process::id()));
        let written = root.join("scenario");
        let kept = root.join("scenario-2");
        fs::create_dir_all(&written).unwrap();
        fs::write(written.join("summary.txt"), "result: fail").unwrap();

        keep_artifacts(&written, &kept).unwrap();

        assert!(!written.exists());
        assert_eq!(
            fs::read_to_string(kept.join("summary.txt")).unwrap(),
            "result: fail"
        );
        // A failed run that wrote no folder leaves nothing to move.
        keep_artifacts(&written, &root.join("scenario-3")).unwrap();
        assert!(!root.join("scenario-3").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
