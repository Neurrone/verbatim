//! `cargo xtask demo <scenario> [--name <name>]`: records one end-to-end
//! scenario on this machine as a video for the repository's `videos`
//! folder (see `videos/readme.md`): a test scenario's into `videos/tests`,
//! and a demonstration's, one in the registry's demo group, into
//! `videos/demos`.
//!
//! It builds and starts its own agent on a port of its own, from the
//! target directory cargo builds into (`CARGO_TARGET_DIR` when set), runs the
//! scenario the way the end-to-end suite does, with the recording's demo
//! quality (`verbatim_e2e::recording::QUALITY_ENV`), and copies the
//! scenario's video to `<folder>/<name>.mp4`. The name defaults to the
//! scenario's with hyphens for underscores, without a demonstration's
//! `demo_` prefix. Demonstrations' tests are ignored, so the suite never
//! runs them; this runs the scenario with `--include-ignored`. A scenario
//! that fails leaves `videos` untouched. Like any local run, it takes over
//! the desktop while it runs and needs an unlocked one
//! (`docs/tooling.md`).

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_e2e::registry::{Group, ScenarioDef};
use verbatim_e2e::{artifacts, recording, registry};

/// A port of the demo's own, so an agent a developer already runs on the
/// default port is left alone.
const AGENT_PORT: u16 = 44_002;

/// How long the agent is given to start listening.
const AGENT_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn run(args: &[String]) -> ExitCode {
    let (scenario, name) = match parse(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("xtask demo: {error}");
            eprintln!("usage: cargo xtask demo <scenario> [--name <name>]");
            eprintln!("demonstrations, recorded into videos/demos:");
            for def in registry::SCENARIOS
                .iter()
                .filter(|def| is_demonstration(def))
            {
                eprintln!("  {}", def.name);
            }
            eprintln!("test scenarios, recorded into videos/tests:");
            for def in registry::SCENARIOS
                .iter()
                .filter(|def| !is_demonstration(def))
            {
                eprintln!("  {}", def.name);
            }
            return ExitCode::from(2);
        }
    };
    match record(scenario, &name) {
        Ok(path) => {
            println!("xtask demo: saved {}", path.display());
            println!(
                "xtask demo: the videos are stored with Git LFS; add the file with `git add` as usual"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("xtask demo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Whether `def` is a demonstration rather than a test scenario.
fn is_demonstration(def: &ScenarioDef) -> bool {
    def.group == Group::Demo
}

/// The folder under `videos` a scenario's video goes in.
fn folder(def: &ScenarioDef) -> &'static str {
    if is_demonstration(def) {
        "demos"
    } else {
        "tests"
    }
}

/// The scenario and the video's file name, without its extension.
fn parse(args: &[String]) -> Result<(&'static ScenarioDef, String), String> {
    let mut scenario = None;
    let mut name = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => {
                name = Some(args.next().ok_or("--name needs a value")?.clone());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            positional if scenario.is_none() => scenario = Some(positional.to_owned()),
            extra => return Err(format!("unexpected argument {extra}")),
        }
    }
    let scenario: String = scenario.ok_or("name a scenario")?;
    let def =
        registry::find(&scenario).ok_or_else(|| format!("no scenario is named {scenario}"))?;
    let name = name.unwrap_or_else(|| {
        let stem = if is_demonstration(def) {
            def.name.strip_prefix("demo_").unwrap_or(def.name)
        } else {
            def.name
        };
        stem.replace('_', "-")
    });
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "{name} is not a usable file name: use letters, digits, hyphens, and underscores"
        ));
    }
    Ok((def, name))
}

fn record(def: &ScenarioDef, name: &str) -> Result<PathBuf, String> {
    let scenario = def.name;
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one directory under the workspace root")
        .to_path_buf();

    println!("xtask demo: building the agent");
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "verbatim-agent"])
        .current_dir(&repo_root)
        .status()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if !status.success() {
        return Err(format!("building the agent failed ({status})"));
    }

    let agent = Command::new(
        target_dir(&repo_root, std::env::var_os("CARGO_TARGET_DIR").as_deref())
            .join("debug/verbatim-agent.exe"),
    )
    .args([
        "--bind-address",
        "127.0.0.1",
        "--port",
        &AGENT_PORT.to_string(),
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|error| format!("could not start the agent: {error}"))?;
    let mut agent = KillOnDrop(agent);
    wait_for_port(AGENT_PORT)?;

    println!("xtask demo: running {scenario}; the desktop belongs to it until it ends");
    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "test",
            "-p",
            "verbatim-e2e",
            scenario,
            "--",
            "--exact",
            // A demonstration's test is ignored, so the suite never runs it.
            "--include-ignored",
            "--test-threads=1",
        ])
        .env("VERBATIM_E2E_ENDPOINT", format!("127.0.0.1:{AGENT_PORT}"))
        .env(recording::RECORD_ENV, "1")
        .env(recording::QUALITY_ENV, "demo")
        .current_dir(&repo_root);
    let status = command
        .status()
        .map_err(|error| format!("could not run cargo test: {error}"))?;
    let _ = agent.0.kill();
    let _ = agent.0.wait();
    if !status.success() {
        return Err(format!("{scenario} failed ({status}); videos is unchanged"));
    }

    let video = artifacts::scenario_dir(&artifacts::artifacts_root(), scenario)
        .join(format!("{scenario}.mp4"));
    if !video.is_file() {
        return Err(format!(
            "{scenario} passed but made no video at {}; is ffmpeg on PATH?",
            video.display()
        ));
    }
    let videos = repo_root.join("videos").join(folder(def));
    std::fs::create_dir_all(&videos)
        .map_err(|error| format!("could not create {}: {error}", videos.display()))?;
    let destination = videos.join(format!("{name}.mp4"));
    std::fs::copy(&video, &destination)
        .map_err(|error| format!("could not copy the video: {error}"))?;
    Ok(destination)
}

/// The directory cargo builds into: `target_env`, the `CARGO_TARGET_DIR`
/// value, when set, resolved against the workspace root as cargo resolves
/// a relative one from the root it runs in, else `target` under the root.
fn target_dir(repo_root: &Path, target_env: Option<&std::ffi::OsStr>) -> PathBuf {
    target_env.map_or_else(|| repo_root.join("target"), |dir| repo_root.join(dir))
}

fn wait_for_port(port: u16) -> Result<(), String> {
    let deadline = Instant::now() + AGENT_TIMEOUT;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(format!("the agent never listened on port {port}"))
}

/// Ends the agent however the demo ends.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|&arg| arg.to_owned()).collect()
    }

    #[test]
    fn names_the_video_after_the_scenario_unless_told_otherwise() {
        let scenario = registry::SCENARIOS[0].name;
        let (def, name) = parse(&args(&[scenario])).expect("parses");
        assert_eq!(name, scenario.replace('_', "-"));
        assert_eq!(folder(def), "tests");
        let (_, name) = parse(&args(&[scenario, "--name", "reading-settings"])).expect("parses");
        assert_eq!(name, "reading-settings");
        assert!(parse(&args(&[scenario, "--name", "../escape"])).is_err());
        assert!(parse(&args(&["no_such_scenario"])).is_err());
    }

    #[test]
    fn the_agent_comes_from_cargo_target_dir_when_set() {
        let root = Path::new(r"C:\repo");
        assert_eq!(target_dir(root, None), root.join("target"));
        assert_eq!(
            target_dir(root, Some(std::ffi::OsStr::new(r"D:\builds"))),
            Path::new(r"D:\builds")
        );
        assert_eq!(
            target_dir(root, Some(std::ffi::OsStr::new("out"))),
            root.join("out")
        );
    }

    #[test]
    fn a_demonstration_goes_in_demos_without_its_prefix() {
        let (def, name) = parse(&args(&["demo_notepad_editing"])).expect("parses");
        assert_eq!(folder(def), "demos");
        assert_eq!(name, "notepad-editing");
    }
}
