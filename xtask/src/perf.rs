//! Performance measurement on this machine, against a locally running
//! agent (`docs/tooling.md`, "Measuring performance"):
//!
//! - `cargo xtask measure`: the measure set's scenarios, one after another,
//!   then each latency stage's distribution over every announcement, with
//!   the totals along the event's way (`stages.txt`).
//! - `cargo xtask trace scenario`: one scenario with a CPU trace of a
//!   window inside it, read with `verbatim-trace-waits` (each thread's
//!   wake-ups, processor time and ready waits; their totals by thread name;
//!   and any `--wakers`, `--stacks` and `--samples` reports asked for), the
//!   trace then deleted unless `--keep` is given.
//!
//! Both take over the desktop, as every end-to-end run does, and leave
//! their output under `target\perf`. Neither builds Verbatim: build first
//! (`docs/tooling.md`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

/// The agent's address for a local run.
const ENDPOINT: &str = "127.0.0.1:44001";

/// The measure set's scenarios a silent run takes; `measure_speech_real`
/// needs a real device and an audible run, so it is asked for by name.
const MEASURE_SET: &[&str] = &[
    "measure_focus_windows_terminal",
    "measure_focus_windows_terminal_tabs",
    "measure_focus_explorer",
    "measure_focus_text_box",
    "measure_focus_new_application",
    "measure_keys_text_box",
    "measure_keys_conhost",
    "measure_keys_windows_terminal",
];

/// The stages summed per announcement into the totals `stages.txt` ends
/// with: what the user waits on along each way, as named in the latency
/// line.
const TOTALS: &[(&str, &[&str])] = &[
    (
        "total: verbatim.exe, its hops to and from the outpost, and the wait for the reducer",
        &[
            "verbatim.exe",
            "verbatim.exe to outpost",
            "outpost to verbatim.exe",
            "outpost outbound",
            "outpost's pipe to verbatim.exe",
            "waiting for the reducer",
        ],
    ),
    (
        "total: a key to its speech queued (hook to Core, to speech)",
        &["hook to Core", "to speech"],
    ),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the workspace")
        .to_path_buf()
}

/// `target\perf\<label>`, made fresh.
fn output_folder(label: &str) -> Result<PathBuf, String> {
    let folder = workspace().join("target").join("perf").join(label);
    if folder.exists() {
        fs::remove_dir_all(&folder)
            .map_err(|error| format!("could not clear {}: {error}", folder.display()))?;
    }
    fs::create_dir_all(&folder)
        .map_err(|error| format!("could not create {}: {error}", folder.display()))?;
    Ok(folder)
}

/// The agent, started for the run and ended when dropped.
struct Agent(Child);

impl Agent {
    fn start(folder: &Path) -> Result<Self, String> {
        let exe = workspace().join(r"target\debug\verbatim-agent.exe");
        if !exe.exists() {
            return Err(format!(
                "{} is missing: cargo build -p verbatim-agent",
                exe.display()
            ));
        }
        let log = |name: &str| {
            fs::File::create(folder.join(name)).map_or_else(|_| Stdio::null(), Stdio::from)
        };
        let child = Command::new(exe)
            .args(["--bind-address", "127.0.0.1", "--port", "44001"])
            .stdout(log("agent.out.log"))
            .stderr(log("agent.err.log"))
            .spawn()
            .map_err(|error| format!("could not start the agent: {error}"))?;
        Ok(Self(child))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How a scenario is run: which build is staged, and whether it is heard.
#[derive(Clone, Copy)]
struct RunOptions {
    release: bool,
    audible: bool,
}

/// `cargo test` running scenario `name` against the agent, its output in
/// `folder` as `<name>.log`.
fn scenario(name: &str, options: RunOptions, folder: &Path) -> Result<Child, String> {
    let log = fs::File::create(folder.join(format!("{name}.log")))
        .map_err(|error| format!("could not create the scenario's log: {error}"))?;
    let errors = log
        .try_clone()
        .map_err(|error| format!("could not share the scenario's log: {error}"))?;
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(workspace())
        .args(["test", "-p", "verbatim-e2e", "--test", "e2e", "--"])
        .args(["--ignored", "--exact", name, "--test-threads=1"])
        .env("VERBATIM_E2E_ENDPOINT", ENDPOINT)
        .env("VERBATIM_E2E_RECORD", "0")
        .env_remove("VERBATIM_E2E_AUDIBLE")
        .env_remove("VERBATIM_E2E_VERBATIM_EXE")
        .stdout(log)
        .stderr(errors);
    if options.audible {
        command.env("VERBATIM_E2E_AUDIBLE", "1");
    }
    if options.release {
        command.env(
            "VERBATIM_E2E_VERBATIM_EXE",
            workspace().join(r"target\release\verbatim.exe"),
        );
    }
    command
        .spawn()
        .map_err(|error| format!("could not run cargo test: {error}"))
}

/// Whether the scenario whose output is `log` passed.
fn passed(log: &Path) -> bool {
    fs::read_to_string(log).is_ok_and(|text| text.contains("test result: ok. 1 passed"))
}

/// The value after `flag` in `args`, every time it appears.
fn values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .collect()
}

fn value(args: &[String], flag: &str) -> Option<String> {
    values(args, flag).into_iter().next()
}

fn now_label() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    seconds.to_string()
}

/// Entry point for `cargo xtask measure`.
pub(crate) fn measure(args: &[String]) -> ExitCode {
    match run_measure(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask measure: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_measure(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--help") {
        println!(
            "usage: cargo xtask measure [--release] [--audible] [--scenario <name>]... [--label <label>]"
        );
        return Ok(());
    }
    let options = RunOptions {
        release: args.iter().any(|arg| arg == "--release"),
        audible: args.iter().any(|arg| arg == "--audible"),
    };
    let mut names = values(args, "--scenario");
    if names.is_empty() {
        names = MEASURE_SET.iter().map(|&name| name.to_owned()).collect();
    }
    let label = value(args, "--label").unwrap_or_else(|| {
        format!(
            "measure-{}-{}",
            if options.release { "release" } else { "debug" },
            now_label()
        )
    });
    let folder = output_folder(&label)?;
    let agent = Agent::start(&folder)?;
    let mut results = String::new();
    for name in &names {
        let mut test = scenario(name, options, &folder)?;
        let _ = test.wait();
        let ok = passed(&folder.join(format!("{name}.log")));
        let _ = writeln!(results, "{name}: {}", if ok { "passed" } else { "FAILED" });
        println!(
            "xtask measure: {name}: {}",
            if ok { "passed" } else { "FAILED" }
        );
        // Each run's artifacts are replaced by the next; keep its log.
        let artifacts = workspace().join(r"target\e2e-artifacts").join(name);
        let kept = folder.join("artifacts").join(name);
        let _ = fs::create_dir_all(&kept);
        let _ = fs::copy(artifacts.join("stderr.log"), kept.join("stderr.log"));
    }
    drop(agent);
    fs::write(folder.join("results.txt"), &results).map_err(|error| error.to_string())?;
    let report = stages_report(&folder.join("artifacts"));
    fs::write(folder.join("stages.txt"), &report).map_err(|error| error.to_string())?;
    println!("{report}");
    println!("xtask measure: written to {}", folder.display());
    Ok(())
}

/// Every `verbatim::latency` line under `folder`'s `stderr.log` files, as
/// each stage's distribution: count, median, 99th and 99.9th percentiles,
/// maximum, and the share over 1 and 2 ms; then the [`TOTALS`].
fn stages_report(folder: &Path) -> String {
    let mut stages: Vec<(String, Vec<f64>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut lines = 0usize;
    let mut add = |stages: &mut Vec<(String, Vec<f64>)>, label: &str, ms: f64| {
        let at = *index.entry(label.to_owned()).or_insert_with(|| {
            stages.push((label.to_owned(), Vec::new()));
            stages.len() - 1
        });
        stages[at].1.push(ms);
    };
    for log in stderr_logs(folder) {
        let text = fs::read_to_string(&log).unwrap_or_default();
        for line in text.lines() {
            let Some(entries) = latency_entries(line) else {
                continue;
            };
            lines += 1;
            let mut sums: Vec<(&str, f64)> = Vec::new();
            for (label, ms) in &entries {
                add(&mut stages, label, *ms);
                for (total, parts) in TOTALS {
                    if parts.contains(&label.as_str()) {
                        match sums.iter_mut().find(|(name, _)| name == total) {
                            Some((_, sum)) => *sum += ms,
                            None => sums.push((total, *ms)),
                        }
                    }
                }
            }
            for (total, sum) in sums {
                add(&mut stages, total, sum);
            }
        }
    }
    let mut report = format!("{lines} announcements\n");
    for (label, mut values) in stages {
        values.sort_by(f64::total_cmp);
        let at = |percent: f64| {
            let last = values.len() - 1;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "an index into a list of announcements"
            )]
            let index = ((percent / 100.0) * last as f64).round() as usize;
            values[index.min(last)]
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count of announcements, far below f64's precision"
        )]
        let share = |limit: f64| {
            values.iter().filter(|&&value| value > limit).count() as f64 * 100.0
                / values.len() as f64
        };
        let _ = writeln!(
            report,
            "{label}: n={} median={:.2} p99={:.2} p99.9={:.2} max={:.2} over1ms={:.2}% over2ms={:.2}%",
            values.len(),
            at(50.0),
            at(99.0),
            at(99.9),
            values.last().copied().unwrap_or(0.0),
            share(1.0),
            share(2.0),
        );
    }
    report
}

fn stderr_logs(folder: &Path) -> Vec<PathBuf> {
    let mut logs = Vec::new();
    let Ok(entries) = fs::read_dir(folder) else {
        return logs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            logs.extend(stderr_logs(&path));
        } else if path.file_name().is_some_and(|name| name == "stderr.log") {
            logs.push(path);
        }
    }
    logs.sort();
    logs
}

/// `line` without the terminal's colour codes (`ESC [ ... m`), which the
/// log's console output carries between a line's target and its colon.
fn without_colours(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// A latency line's stages:`... verbatim::latency: ... Event: a 0.1, b
/// 2.0 (3 calls). Speech: c 1.0.`, as (stage, milliseconds).
fn latency_entries(line: &str) -> Option<Vec<(String, f64)>> {
    let line = without_colours(line);
    let rest = &line[line.find("verbatim::latency:")?..];
    let event = &rest[rest.find(" Event: ")? + " Event: ".len()..];
    let (event, speech) = event.split_once(". Speech: ")?;
    let speech = speech.trim_end().strip_suffix('.').unwrap_or(speech);
    let mut entries = Vec::new();
    for part in event.split(", ").chain(speech.split(", ")) {
        let part = part.trim();
        let part = match part.rfind(" (") {
            Some(at) if part.ends_with(" calls)") => &part[..at],
            _ => part,
        };
        let Some((label, ms)) = part.rsplit_once(' ') else {
            continue;
        };
        if let Ok(ms) = ms.parse::<f64>() {
            entries.push((label.to_owned(), ms));
        }
    }
    Some(entries)
}

/// Entry point for `cargo xtask trace scenario`.
pub(crate) fn trace_scenario(args: &[String]) -> ExitCode {
    match run_trace_scenario(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask trace scenario: {message}");
            ExitCode::FAILURE
        }
    }
}

const TRACE_SCENARIO_USAGE: &str = "usage: cargo xtask trace scenario <name> --after <seconds> --seconds <seconds> \
     [--release] [--audible] [--wakers <thread>]... [--stacks <thread>]... \
     [--samples <thread>]... [--symbol-server] [--keep] [--label <label>]";

fn run_trace_scenario(args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .filter(|name| !name.starts_with("--"))
        .ok_or_else(|| TRACE_SCENARIO_USAGE.to_owned())?;
    let seconds_of = |flag: &str| -> Result<u64, String> {
        value(args, flag)
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("{flag} is needed\n{TRACE_SCENARIO_USAGE}"))
    };
    let after = seconds_of("--after")?;
    let seconds = seconds_of("--seconds")?;
    let options = RunOptions {
        release: args.iter().any(|arg| arg == "--release"),
        audible: args.iter().any(|arg| arg == "--audible"),
    };
    let keep = args.iter().any(|arg| arg == "--keep");
    let label = value(args, "--label").unwrap_or_else(|| format!("{name}-{}", now_label()));
    let folder = output_folder(&label)?;
    let reader = build_reader()?;

    let agent = Agent::start(&folder)?;
    let mut test = scenario(name, options, &folder)?;
    // The window is time inside the scenario, chosen by the caller: a
    // trace that covers a scenario's start or end can leave File Explorer
    // stuck putting the focus on the Start button (`docs/tooling.md`).
    thread::sleep(Duration::from_secs(after));
    crate::trace::start()?;
    thread::sleep(Duration::from_secs(seconds));
    let stopped = crate::trace::stop();
    let _ = test.wait();
    drop(agent);
    let trace = crate::trace::last_trace()?;
    stopped?;
    let ok = passed(&folder.join(format!("{name}.log")));
    println!(
        "xtask trace scenario: {name} {}",
        if ok { "passed" } else { "FAILED" }
    );

    let symbols = workspace().join(if options.release {
        r"target\release"
    } else {
        r"target\debug"
    });
    let reports = reader_reports(args, &symbols);
    for (file, report) in &reports {
        let output = Command::new(&reader)
            .arg(&trace)
            .args(report)
            .output()
            .map_err(|error| format!("could not run the reader: {error}"))?;
        let mut report_text = output.stdout;
        report_text.extend(output.stderr);
        fs::write(folder.join(file), report_text).map_err(|error| error.to_string())?;
    }
    if keep {
        println!(
            "xtask trace scenario: the trace is kept at {}",
            trace.display()
        );
    } else {
        crate::trace::delete(&trace);
    }
    if let Ok(text) = fs::read_to_string(folder.join("by-name.txt")) {
        println!("{}", text.lines().take(25).collect::<Vec<_>>().join("\n"));
    }
    println!("xtask trace scenario: written to {}", folder.display());
    if ok {
        Ok(())
    } else {
        Err(format!("{name} failed; see its log"))
    }
}

/// The reports `cargo xtask trace scenario` asks the reader for, each as
/// the file it is written to and the reader's arguments after the trace:
/// the waits and their totals by name, then a report for each `--wakers`,
/// `--stacks` and `--samples` thread, the stacks and samples reports with
/// `--symbols <symbols>` and, when `--symbol-server` was given,
/// `--symbol-server`.
fn reader_reports(args: &[String], symbols: &Path) -> Vec<(String, Vec<String>)> {
    let symbol_server = args.iter().any(|arg| arg == "--symbol-server");
    let mut reports: Vec<(String, Vec<String>)> = vec![
        ("waits.txt".to_owned(), Vec::new()),
        ("by-name.txt".to_owned(), vec!["--by-name".to_owned()]),
    ];
    for (flag, kind) in [
        ("--wakers", "wakers"),
        ("--stacks", "stacks"),
        ("--samples", "samples"),
    ] {
        for thread in values(args, flag) {
            let file = format!("{kind}-{}.txt", thread.replace(':', "_"));
            let mut report = vec![flag.to_owned(), thread];
            if kind != "wakers" {
                report.push("--symbols".to_owned());
                report.push(symbols.display().to_string());
                if symbol_server {
                    report.push("--symbol-server".to_owned());
                }
            }
            reports.push((file, report));
        }
    }
    reports
}

/// Builds `verbatim-trace-waits` in release, before anything is traced.
fn build_reader() -> Result<PathBuf, String> {
    let status = Command::new(env!("CARGO"))
        .current_dir(workspace())
        .args(["build", "--release", "-p", "verbatim-trace-waits"])
        .status()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if !status.success() {
        return Err("building verbatim-trace-waits failed".to_owned());
    }
    Ok(workspace().join(r"target\release\verbatim-trace-waits.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stacks_report_names_frames_from_the_build_and_the_symbol_server() {
        let args: Vec<String> = [
            "measure_keys_text_box",
            "--stacks",
            "verbatim-worker",
            "--symbol-server",
        ]
        .map(str::to_owned)
        .to_vec();
        let reports = reader_reports(&args, Path::new(r"C:\build\target\debug"));
        assert_eq!(
            reports.last(),
            Some(&(
                "stacks-verbatim-worker.txt".to_owned(),
                [
                    "--stacks",
                    "verbatim-worker",
                    "--symbols",
                    r"C:\build\target\debug",
                    "--symbol-server",
                ]
                .map(str::to_owned)
                .to_vec()
            ))
        );
    }

    #[test]
    fn a_coloured_latency_line_reads_as_its_stages_with_calls_set_aside() {
        let line = "\u{1b}[2m2026-10-13T06:31:21.330982Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \
                    \u{1b}[2mverbatim::latency\u{1b}[0m\u{1b}[2m:\u{1b}[0m 54.9 ms for \"Desktop list\": \
                    0.4 ms to speech. Event: verbatim.exe 0.0, outpost read 19.1 (99 calls). \
                    Speech: to speech 0.4, mixer and device 54.4.";
        assert_eq!(
            latency_entries(line),
            Some(vec![
                ("verbatim.exe".to_owned(), 0.0),
                ("outpost read".to_owned(), 19.1),
                ("to speech".to_owned(), 0.4),
                ("mixer and device".to_owned(), 54.4),
            ])
        );
    }

    #[test]
    fn a_line_that_is_not_a_latency_line_reads_as_none() {
        assert_eq!(latency_entries("INFO verbatim: started"), None);
    }
}
