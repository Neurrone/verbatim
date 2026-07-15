//! `cargo xtask vm deploy`: builds the binaries the guest needs and copies
//! in whichever ones actually changed. Also used internally by `create` (so
//! the golden checkpoint already has a running agent) and `test` (so it
//! runs against the current build rather than whatever the golden
//! checkpoint happened to carry).
//!
//! Every `Copy-VMFile` call and the checkpoint restore that usually precedes
//! it are the slow parts of the `cargo xtask vm test` loop, and most of the
//! time only the debug `verbatim.exe` (or nothing at all) actually changed
//! since the last deploy. Before copying anything, this compares a SHA-256
//! hash of each local artifact against the guest's copy — one PowerShell
//! Direct call fetches all four guest-side hashes at once, since each such
//! call pays a fixed authentication cost against the guest regardless of
//! how much it asks for — and copies only the artifacts that differ. When
//! every hash matches, the guest is left completely undisturbed: no stop,
//! no copy, no scheduled-task restart.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use verbatim_config::ConfigStore;

use super::dotenv::GuestCredentials;
use super::host::{self, Host};
use super::{AGENT_DIR, VERBATIM_DIR, VM_NAME, VmResult};

/// One artifact `deploy` may need to copy into the guest.
struct Artifact {
    /// Human-readable name used in progress output.
    label: &'static str,
    /// The file's path on the host, after building.
    local_path: PathBuf,
    /// The file's destination path inside the guest.
    remote_path: String,
    /// Whether a running guest process might hold this file open for
    /// execution, and so must be stopped before it can be overwritten.
    /// `settings.toml` is data, not code, so it is never held open this way.
    is_executable: bool,
}

/// Builds `verbatim-app`, `verbatim-agent`, and `verbatim-outpost` (debug
/// profile, matching `.github/workflows/ci.yml`'s runner-direct E2E job),
/// hashes each local artifact plus a staged `settings.toml` against the
/// guest's copies, and copies only the ones that differ. Stops the guest's
/// `VerbatimAgent` scheduled task and any running Verbatim first, but only
/// when at least one executable actually needs copying (a live process can
/// hold an executable open for `Copy-VMFile`, but never `settings.toml`);
/// restarts the task afterward if it was stopped, or if `verbatim-agent.exe`
/// itself was among the copied artifacts.
///
/// # Errors
///
/// Returns an error if the build fails, an expected artifact is missing
/// afterwards, hashing (local or guest-side) fails, or any copy or in-guest
/// step fails.
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

    let artifacts = [
        Artifact {
            label: "verbatim.exe",
            local_path: verbatim_exe,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim.exe"),
            is_executable: true,
        },
        Artifact {
            label: "verbatim-outpost.exe",
            local_path: outpost_exe,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim-outpost.exe"),
            is_executable: true,
        },
        Artifact {
            label: "settings.toml",
            local_path: settings_path,
            remote_path: format!(r"{VERBATIM_DIR}\settings.toml"),
            is_executable: false,
        },
        Artifact {
            label: "verbatim-agent.exe",
            local_path: agent_exe,
            remote_path: format!(r"{AGENT_DIR}\verbatim-agent.exe"),
            is_executable: true,
        },
    ];

    let needs_copy = artifacts_needing_copy(host, credentials, &artifacts)?;
    if needs_copy.is_empty() {
        println!("xtask vm deploy: all artifacts up to date; guest left untouched");
        return Ok(());
    }

    copy_mismatched_artifacts(host, credentials, &artifacts, &needs_copy)
}

/// Hashes every local artifact and, in one PowerShell Direct call, every
/// guest destination, then compares them, printing one line per artifact
/// naming whether it matches or needs copying. Returns the indices (into
/// `artifacts`) of the ones that differ.
fn artifacts_needing_copy(
    host: &dyn Host,
    credentials: &GuestCredentials,
    artifacts: &[Artifact],
) -> VmResult<Vec<usize>> {
    println!("xtask vm deploy: hashing local artifacts");
    let local_hashes = artifacts
        .iter()
        .map(|artifact| host::local_file_hash_sha256(&artifact.local_path))
        .collect::<VmResult<Vec<_>>>()?;

    println!("xtask vm deploy: checking guest artifact hashes (one PowerShell Direct call)");
    let remote_paths: Vec<&str> = artifacts.iter().map(|a| a.remote_path.as_str()).collect();
    let guest_hashes = fetch_guest_hashes(host, credentials, &remote_paths)?;

    let mut needs_copy = Vec::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        let guest_hash = guest_hashes
            .get(artifact.remote_path.as_str())
            .map_or("MISSING", String::as_str);
        if guest_hash.eq_ignore_ascii_case(&local_hashes[index]) {
            println!(
                "xtask vm deploy: {} unchanged on the guest; skipping",
                artifact.label
            );
        } else {
            println!("xtask vm deploy: {} changed; will copy", artifact.label);
            needs_copy.push(index);
        }
    }
    Ok(needs_copy)
}

/// Stops the guest (only if an executable needs copying, since a live
/// process can hold an executable open but never `settings.toml`), copies
/// every mismatched artifact, then restarts the `VerbatimAgent` scheduled
/// task if it was stopped or `verbatim-agent.exe` itself was copied.
fn copy_mismatched_artifacts(
    host: &dyn Host,
    credentials: &GuestCredentials,
    artifacts: &[Artifact],
    needs_copy: &[usize],
) -> VmResult<()> {
    let executable_needs_copy = needs_copy
        .iter()
        .any(|&index| artifacts[index].is_executable);
    let agent_needs_copy = needs_copy
        .iter()
        .any(|&index| artifacts[index].label == "verbatim-agent.exe");

    let mut stopped_guest = false;
    if executable_needs_copy {
        // The golden checkpoint deliberately carries a *running* agent (and
        // a restore resumes it mid-flight), so the previous binaries may be
        // open for execution right now — and Copy-VMFile cannot overwrite a
        // file a live process holds. Stop the task and kill every deployed
        // process before copying; the task is started again below.
        println!("xtask vm deploy: stopping the agent and any running Verbatim in the guest");
        host.run_in_guest(
            VM_NAME,
            credentials,
            "Stop-ScheduledTask -TaskName 'VerbatimAgent' -ErrorAction SilentlyContinue\n\
             Stop-Process -Name 'verbatim-agent','verbatim','verbatim-outpost' -Force -ErrorAction SilentlyContinue\n\
             Start-Sleep -Seconds 1",
        )?;
        stopped_guest = true;
    }

    let mut copied_labels = Vec::new();
    let mut skipped_labels = Vec::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        if needs_copy.contains(&index) {
            println!(
                "xtask vm deploy: copying {} to {}",
                artifact.label, artifact.remote_path
            );
            host.copy_file_to_guest(VM_NAME, &artifact.local_path, &artifact.remote_path)?;
            copied_labels.push(artifact.label);
        } else {
            skipped_labels.push(artifact.label);
        }
    }

    if stopped_guest || agent_needs_copy {
        println!("xtask vm deploy: starting the VerbatimAgent scheduled task");
        host.run_in_guest(
            VM_NAME,
            credentials,
            "Start-ScheduledTask -TaskName 'VerbatimAgent'",
        )?;
    }

    println!(
        "xtask vm deploy: done; copied [{}], skipped [{}]",
        copied_labels.join(", "),
        skipped_labels.join(", ")
    );
    Ok(())
}

/// Fetches the SHA-256 hashes of `remote_paths` inside the guest in one
/// PowerShell Direct call, keyed by the exact path text passed in. A path
/// that does not exist in the guest maps to `"MISSING"` rather than being
/// omitted, so a caller can tell "not present yet" from a request that
/// somehow got no answer at all.
fn fetch_guest_hashes(
    host: &dyn Host,
    credentials: &GuestCredentials,
    remote_paths: &[&str],
) -> VmResult<HashMap<String, String>> {
    let quoted_paths: Vec<String> = remote_paths
        .iter()
        .map(|path| host::ps_quote(path))
        .collect();
    let mut script = format!("$paths = @({})\n", quoted_paths.join(", "));
    script.push_str(
        "foreach ($p in $paths) {\n\
         if (Test-Path -LiteralPath $p) {\n\
         \"$p=$((Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash)\"\n\
         } else {\n\
         \"$p=MISSING\"\n\
         }\n\
         }",
    );
    let output = host.run_in_guest(VM_NAME, credentials, &script)?;
    Ok(parse_guest_hashes(&output))
}

/// Parses `fetch_guest_hashes`'s `path=hash` (or `path=MISSING`) lines into
/// a lookup map. Pure and PowerShell-free so it can be unit tested directly.
fn parse_guest_hashes(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .map(|(path, hash)| (path.to_owned(), hash.to_owned()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_guest_hashes_reads_path_equals_hash_lines() {
        let output = "C:\\a\\verbatim.exe=ABCD1234\nC:\\a\\settings.toml=MISSING\n";
        let parsed = parse_guest_hashes(output);
        assert_eq!(
            parsed.get("C:\\a\\verbatim.exe").map(String::as_str),
            Some("ABCD1234")
        );
        assert_eq!(
            parsed.get("C:\\a\\settings.toml").map(String::as_str),
            Some("MISSING")
        );
    }

    #[test]
    fn parse_guest_hashes_ignores_blank_lines() {
        let output = "\nC:\\a\\verbatim.exe=ABCD1234\n\n";
        let parsed = parse_guest_hashes(output);
        assert_eq!(parsed.len(), 1);
    }
}
