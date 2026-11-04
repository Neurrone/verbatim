//! `cargo xtask e2e <scenario>... [--runs <n>]`: runs end-to-end scenarios
//! against this machine, each `n` times, with an agent this command starts
//! and ends (`docs/tooling/e2e.md`, "Running scenarios locally"). The
//! agent is this command's child, inside its job (`crate::contain`), so
//! stopping the command, by any means, ends the agent, and the agent's
//! launches end with it; no agent is left listening for the next run.
//!
//! Each run is its own `cargo test` of the one scenario, so each has its
//! own log, `target/e2e-runs/<scenario>-<run>.log`, and its own artifacts
//! folder as the suite writes it. A summary line per run is printed as it
//! ends, and the command fails when any run failed.

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// Where the agent listens: the endpoint `docs/tooling/e2e.md` names.
const ENDPOINT: &str = "127.0.0.1:44001";

/// How long the agent has to start listening.
const AGENT_START: Duration = Duration::from_secs(30);

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
    cargo(&root, &["build", "-p", "verbatim-agent"])?;
    cargo(
        &root,
        &["test", "-p", "verbatim-e2e", "--test", "e2e", "--no-run"],
    )?;
    let logs = root.join(r"target\e2e-runs");
    fs::create_dir_all(&logs)
        .map_err(|error| format!("could not create {}: {error}", logs.display()))?;
    let _agent = Agent::start(&root, &logs)?;
    let mut all_passed = true;
    for run in 1..=runs {
        for scenario in scenarios {
            let log = logs.join(format!("{scenario}-{run}.log"));
            let passed = run_one(&root, scenario, &log)?;
            all_passed &= passed;
            println!(
                "run {run} {scenario}: {} ({})",
                if passed { "pass" } else { "fail" },
                log.display()
            );
        }
    }
    Ok(all_passed)
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

/// The agent, started for the command and ended when dropped; the
/// command's job ends it on every other path.
struct Agent(Child);

impl Agent {
    /// Starts the agent, logging to `logs`, and waits until it listens.
    fn start(root: &Path, logs: &Path) -> Result<Self, String> {
        if TcpStream::connect(ENDPOINT).is_ok() {
            return Err(format!(
                "something already listens on {ENDPOINT}, such as an agent started by hand; end it first"
            ));
        }
        let log = |name: &str| {
            fs::File::create(logs.join(name)).map_or_else(|_| Stdio::null(), Stdio::from)
        };
        let mut child = Command::new(root.join(r"target\debug\verbatim-agent.exe"))
            .args(["--bind-address", "127.0.0.1", "--port", "44001"])
            .stdout(log("agent.out.log"))
            .stderr(log("agent.err.log"))
            .spawn()
            .map_err(|error| format!("could not start the agent: {error}"))?;
        let deadline = Instant::now() + AGENT_START;
        // Listening is the evidence it is ready; its exit ends the wait.
        while TcpStream::connect(ENDPOINT).is_err() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("the agent exited before listening ({status})"));
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(format!("the agent did not listen within {AGENT_START:?}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Self(child))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

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
}
