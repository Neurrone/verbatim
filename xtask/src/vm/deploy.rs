//! `cargo xtask vm deploy`: builds the binaries the guest needs and copies
//! them in. Also used internally by `create` (so the golden checkpoint
//! already has a running agent) and `test` (so it runs against the current
//! build rather than whatever the golden checkpoint happened to carry).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use verbatim_config::ConfigStore;

use super::dotenv::GuestCredentials;
use super::host::Host;
use super::{AGENT_DIR, VERBATIM_DIR, VM_NAME, VmResult};

/// Builds `verbatim-app`, `verbatim-agent`, and `verbatim-outpost` (debug
/// profile, matching `.github/workflows/ci.yml`'s runner-direct E2E job),
/// then copies `verbatim.exe`, `verbatim-outpost.exe`, a staged
/// `settings.toml`, and `verbatim-agent.exe` into the guest, and restarts
/// the `VerbatimAgent` scheduled task so the freshly deployed agent is the
/// one actually running.
///
/// # Errors
///
/// Returns an error if the build fails, an expected artifact is missing
/// afterwards, or any copy or in-guest step fails.
pub(crate) fn run(
    host: &dyn Host,
    repo_root: &Path,
    credentials: &GuestCredentials,
) -> VmResult<()> {
    println!("xtask vm deploy: building verbatim-app, verbatim-agent, verbatim-outpost (debug)");
    build_binaries(repo_root)?;

    let target_dir = repo_root.join("target").join("debug");
    let verbatim_exe = require_artifact(&target_dir, "verbatim.exe")?;
    let outpost_exe = require_artifact(&target_dir, "verbatim-outpost.exe")?;
    let agent_exe = require_artifact(&target_dir, "verbatim-agent.exe")?;
    let settings_path = write_capture_synth_settings(repo_root)?;

    // The golden checkpoint deliberately carries a *running* agent (and a
    // restore resumes it mid-flight), so the previous binaries may be open
    // for execution right now — and Copy-VMFile cannot overwrite a file a
    // live process holds. Stop the task and kill every deployed process
    // before copying; the task is started again below.
    println!("xtask vm deploy: stopping the agent and any running Verbatim in the guest");
    host.run_in_guest(
        VM_NAME,
        credentials,
        "Stop-ScheduledTask -TaskName 'VerbatimAgent' -ErrorAction SilentlyContinue\n\
         Stop-Process -Name 'verbatim-agent','verbatim','verbatim-outpost' -Force -ErrorAction SilentlyContinue\n\
         Start-Sleep -Seconds 1",
    )?;

    println!("xtask vm deploy: copying artifacts to {VERBATIM_DIR}");
    host.copy_file_to_guest(
        VM_NAME,
        &verbatim_exe,
        &format!(r"{VERBATIM_DIR}\verbatim.exe"),
    )?;
    host.copy_file_to_guest(
        VM_NAME,
        &outpost_exe,
        &format!(r"{VERBATIM_DIR}\verbatim-outpost.exe"),
    )?;
    host.copy_file_to_guest(
        VM_NAME,
        &settings_path,
        &format!(r"{VERBATIM_DIR}\settings.toml"),
    )?;

    println!("xtask vm deploy: copying the agent to {AGENT_DIR}");
    host.copy_file_to_guest(
        VM_NAME,
        &agent_exe,
        &format!(r"{AGENT_DIR}\verbatim-agent.exe"),
    )?;

    println!("xtask vm deploy: starting the VerbatimAgent scheduled task");
    host.run_in_guest(
        VM_NAME,
        credentials,
        "Start-ScheduledTask -TaskName 'VerbatimAgent'",
    )?;

    println!("xtask vm deploy: done");
    Ok(())
}

fn build_binaries(repo_root: &Path) -> VmResult<()> {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "verbatim-app",
            "-p",
            "verbatim-agent",
            "-p",
            "verbatim-outpost",
        ])
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("failed to launch cargo build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed: {status}"));
    }
    Ok(())
}

fn require_artifact(target_dir: &Path, file_name: &str) -> VmResult<PathBuf> {
    let path = target_dir.join(file_name);
    if !path.is_file() {
        return Err(format!(
            "expected build artifact missing: {} (did the build succeed?)",
            path.display()
        ));
    }
    Ok(path)
}

/// Writes a `settings.toml` selecting the capture synthesizer into a
/// staging directory on the host, mirroring
/// `verbatim_e2e::scenario::configure_capture_synth`'s choice and for the
/// same reason (audio-free, no installed voices required) — just staged
/// here rather than written straight into `exe_dir`, because in VM mode
/// that directory only exists inside the guest, not on the host running
/// this command.
fn write_capture_synth_settings(repo_root: &Path) -> VmResult<PathBuf> {
    let staging_dir = repo_root.join("target").join("xtask-vm-staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("could not create {}: {error}", staging_dir.display()))?;
    let mut store = ConfigStore::load(&staging_dir)
        .map_err(|error| format!("could not load a staging config store: {error}"))?;
    store.settings_mut().speech.synthesizer = Some("capture".to_owned());
    store
        .save_settings()
        .map_err(|error| format!("could not write settings.toml: {error}"))?;
    Ok(staging_dir.join(ConfigStore::SETTINGS_FILE))
}
