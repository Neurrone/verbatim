//! `cargo xtask trace start` and `cargo xtask trace stop`: record a Windows
//! Performance Recorder trace of the CPU's scheduling (WPR's built-in CPU
//! profile: context switches and ready threads), which
//! `docs/design/communication.md` uses to attribute each slow hand-off to
//! its cause and to check that raised thread priorities never keep
//! Verbatim's GUI thread waiting.
//!
//! WPR needs administrator rights. `vm/scripts/Register-VerbatimTraceTask.ps1`,
//! run once elevated, registers two tasks that run it as SYSTEM and that
//! this user may start; `start` starts the first and checks that WPR
//! started, and `stop` starts the second, waits for it to write
//! `last-trace.txt`, and prints the trace's path. The traces are kept in
//! [`TRACE_DIR`], which the user may read and delete in.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

/// Must match `$startName` in `vm/scripts/Register-VerbatimTraceTask.ps1`.
const START_TASK: &str = "Verbatim trace start";

/// Must match `$stopName` in the same script.
const STOP_TASK: &str = "Verbatim trace stop";

/// Must match `$traceDir` in the same script.
const TRACE_DIR: &str = r"C:\ProgramData\Verbatim\Traces";

/// How long WPR is given to start, or to stop and write the trace, which
/// for a long verbose trace takes a while.
const TASK_TIMEOUT: Duration = Duration::from_secs(300);

const POLL: Duration = Duration::from_millis(250);

/// Entry point for `cargo xtask trace <start|stop>`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let result = match args.first().map(String::as_str) {
        Some("start") => start(),
        Some("stop") => stop(),
        _ => Err("usage: cargo xtask trace <start|stop>".to_owned()),
    };
    match result {
        Ok(message) => {
            println!("xtask trace: {message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("xtask trace: {message}");
            ExitCode::FAILURE
        }
    }
}

fn start() -> Result<String, String> {
    run_task(START_TASK)?;
    match wait_for_task(START_TASK)? {
        0 => Ok("WPR is recording the CPU profile".to_owned()),
        code => Err(format!("WPR did not start: the task ended with {code:#x}")),
    }
}

fn stop() -> Result<String, String> {
    let last = Path::new(TRACE_DIR).join("last-trace.txt");
    let before = std::fs::read_to_string(&last).ok();
    run_task(STOP_TASK)?;
    let deadline = Instant::now() + TASK_TIMEOUT;
    loop {
        let now = std::fs::read_to_string(&last).ok();
        if now.is_some() && now != before {
            let text = now.unwrap_or_default();
            let mut lines = text.lines().map(str::trim);
            let file = lines
                .next()
                .unwrap_or_default()
                .trim_start_matches('\u{feff}');
            let code = lines.next().unwrap_or_default();
            return if code == "0" {
                Ok(format!("trace written to {file}"))
            } else {
                Err(format!("WPR could not write {file}: exit code {code}"))
            };
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the trace was not written within {} seconds",
                TASK_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(POLL);
    }
}

/// Starts the registered task `name`, explaining how to register it when
/// that fails.
fn run_task(name: &str) -> Result<(), String> {
    let output = Command::new("schtasks")
        .args(["/Run", "/TN", name])
        .output()
        .map_err(|error| format!("could not run schtasks: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "could not start the task '{name}': {}\n\
         Register it once from an elevated PowerShell with \
         vm\\scripts\\Register-VerbatimTraceTask.ps1",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// Waits for the task `name` to finish, and returns its last result.
fn wait_for_task(name: &str) -> Result<u32, String> {
    let deadline = Instant::now() + TASK_TIMEOUT;
    // The task may not have begun running yet when this is first asked.
    thread::sleep(POLL);
    loop {
        let output = Command::new("schtasks")
            .args(["/Query", "/TN", name, "/FO", "LIST", "/V"])
            .output()
            .map_err(|error| format!("could not run schtasks: {error}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let field = |label: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(label))
                .map(str::trim)
                .map(str::to_owned)
        };
        let running = field("Status:").is_some_and(|status| status == "Running");
        if !running && let Some(result) = field("Last Result:") {
            return result
                .parse::<i64>()
                .map(|code| u32::try_from(code & 0xFFFF_FFFF).unwrap_or(u32::MAX))
                .map_err(|_| format!("the task's last result {result:?} is not a number"));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the task '{name}' did not finish within {} seconds",
                TASK_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(POLL);
    }
}
