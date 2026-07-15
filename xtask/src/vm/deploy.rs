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
//! Direct call fetches every guest-side hash at once, since each such
//! call pays a fixed authentication cost against the guest regardless of
//! how much it asks for — and copies only the artifacts that differ. When
//! every hash matches, the guest is left completely undisturbed: no stop,
//! no copy, no scheduled-task restart.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use verbatim_config::{ConfigStore, Settings};

use super::dotenv::GuestCredentials;
use super::host::{self, Host};
use super::{AGENT_DIR, TOOLS_DIR, VERBATIM_DIR, VM_NAME, VmResult};

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

/// The host paths [`build`] produces, threaded into [`stage_and_copy`]. A
/// named struct rather than a tuple so `xtask::vm::test` (which calls the
/// two phases separately — see that module's doc comment for why) reads as
/// clearly as [`run`]'s own internal composition of them.
pub(crate) struct BuiltArtifacts {
    verbatim: PathBuf,
    outpost: PathBuf,
    agent: PathBuf,
}

/// Builds `verbatim-app`, `verbatim-agent`, and `verbatim-outpost` (debug
/// profile, matching `.github/workflows/ci.yml`'s runner-direct E2E job) and
/// confirms each expected artifact exists afterward. Deliberately makes no
/// contact with the guest at all: `xtask::vm::test` calls this before
/// restoring a checkpoint precisely so a compile failure is caught before
/// any VM state changes, not after.
///
/// # Errors
///
/// Returns an error if the build fails or an expected artifact is missing
/// afterwards.
pub(crate) fn build(repo_root: &Path) -> VmResult<BuiltArtifacts> {
    println!("xtask vm deploy: building verbatim-app, verbatim-agent, verbatim-outpost (debug)");
    build_binaries(repo_root)?;

    let target_dir = repo_root.join("target").join("debug");
    Ok(BuiltArtifacts {
        verbatim: require_artifact(&target_dir, "verbatim.exe")?,
        outpost: require_artifact(&target_dir, "verbatim-outpost.exe")?,
        agent: require_artifact(&target_dir, "verbatim-agent.exe")?,
    })
}

/// Stages a `settings.toml` selecting the real `OneCore` synthesizer,
/// hashes it, `built`'s three binaries, and the vendored `ffmpeg.exe` and
/// `ffprobe.exe` against the guest's copies, and copies only the ones that
/// differ. Stops the guest's `VerbatimAgent`
/// scheduled task and any running Verbatim first, but only when at least
/// one executable actually needs copying (a live process can hold an
/// executable open for `Copy-VMFile`, but never `settings.toml`); restarts
/// the task afterward if it was stopped, or if `verbatim-agent.exe` itself
/// was among the copied artifacts.
///
/// Always `OneCore`, unconditionally — see [`write_synth_settings`]. The VM
/// path deploys the real synthesizer only now: `cargo xtask vm test` is
/// audible by default (no more `--audible` flag choosing between two
/// staged configurations), so there is nothing left for this function to
/// branch on. The capture synth remains available, independently, only for
/// runner-direct mode (`verbatim_e2e::scenario::Scenario::launch`) and unit
/// tests.
///
/// # Errors
///
/// Returns an error if staging `settings.toml`, hashing (local or
/// guest-side), or any copy or in-guest step fails.
pub(crate) fn stage_and_copy(
    host: &dyn Host,
    repo_root: &Path,
    credentials: &GuestCredentials,
    built: BuiltArtifacts,
) -> VmResult<()> {
    let settings_path = write_synth_settings(repo_root)?;
    let ffmpeg_dir = repo_root.join("vm").join("vendor").join("ffmpeg");
    ensure_vendored_ffmpeg(&ffmpeg_dir)?;

    let artifacts = [
        Artifact {
            label: "verbatim.exe",
            local_path: built.verbatim,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim.exe"),
            is_executable: true,
        },
        Artifact {
            label: "verbatim-outpost.exe",
            local_path: built.outpost,
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
            local_path: built.agent,
            remote_path: format!(r"{AGENT_DIR}\verbatim-agent.exe"),
            is_executable: true,
        },
        // The LFS-vendored ffmpeg/ffprobe (vm/vendor/ffmpeg) that
        // `--record` launches and probes with, staged the same fast
        // PowerShell Direct way as everything else rather than baked into the
        // golden image (see vm/vendor/ffmpeg/README.md). They hash-skip after
        // the first deploy, so the ~200 MB copy is paid once, into golden.
        // Not marked executable: neither is ever held open at deploy time
        // (ffmpeg runs only during an active `--record` capture, which never
        // overlaps a deploy), so a lone version bump need not stop the guest.
        Artifact {
            label: "ffmpeg.exe",
            local_path: ffmpeg_dir.join("ffmpeg.exe"),
            remote_path: format!(r"{TOOLS_DIR}\ffmpeg.exe"),
            is_executable: false,
        },
        Artifact {
            label: "ffprobe.exe",
            local_path: ffmpeg_dir.join("ffprobe.exe"),
            remote_path: format!(r"{TOOLS_DIR}\ffprobe.exe"),
            is_executable: false,
        },
    ];

    let needs_copy = artifacts_needing_copy(host, credentials, &artifacts)?;
    if needs_copy.is_empty() {
        println!("xtask vm deploy: all artifacts up to date; guest left untouched");
        return Ok(());
    }

    copy_mismatched_artifacts(host, credentials, &artifacts, &needs_copy)
}

/// Builds then stages-and-copies in one call — the composition the
/// standalone `deploy` verb and `create` use, where there is no separate
/// checkpoint restore to build ahead of (see [`build`] and
/// [`stage_and_copy`] for the two phases `xtask::vm::test` calls
/// separately).
///
/// # Errors
///
/// Returns an error if either phase fails; see [`build`] and
/// [`stage_and_copy`].
pub(crate) fn run(
    host: &dyn Host,
    repo_root: &Path,
    credentials: &GuestCredentials,
) -> VmResult<()> {
    let built = build(repo_root)?;
    stage_and_copy(host, repo_root, credentials, built)
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

/// `verbatim-app` pulls in `verbatim-gui`, whose wxDragon dependency needs
/// `libclang.dll` for bindgen at build time (`CLAUDE.md`'s Coding Standards
/// section). `cargo xtask ci` probes for it automatically; this plain
/// `cargo build` needs the same probe, or it fails outright whenever
/// `LIBCLANG_PATH` is not already set in the caller's shell — reusing
/// `crate::find_libclang` rather than duplicating the candidate list keeps
/// the two probes in lockstep.
fn build_binaries(repo_root: &Path) -> VmResult<()> {
    let libclang = crate::find_libclang();
    match &libclang {
        Some(dir) => println!("xtask vm deploy: using libclang from {}", dir.display()),
        None => println!(
            "xtask vm deploy: libclang not found in known locations; relying on LIBCLANG_PATH or PATH"
        ),
    }

    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "build",
            "-p",
            "verbatim-app",
            "-p",
            "verbatim-agent",
            "-p",
            "verbatim-outpost",
        ])
        .current_dir(repo_root);
    if let Some(dir) = &libclang {
        command.env("LIBCLANG_PATH", dir);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to launch cargo build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed: {status}"));
    }
    Ok(())
}

/// Confirms the vendored `ffmpeg.exe` and `ffprobe.exe` are the real
/// binaries and not Git LFS pointer stubs (which are only a couple of
/// hundred bytes). A clone made without `git lfs` leaves those stubs in
/// place; copying one into the guest would let `--record` fail later with a
/// cryptic in-guest ffmpeg launch error instead of the clear, actionable
/// message here. The real static builds are ~100 MB each, so a 1 MB floor
/// distinguishes them from a pointer with no risk of a false alarm.
fn ensure_vendored_ffmpeg(ffmpeg_dir: &Path) -> VmResult<()> {
    const LFS_POINTER_CEILING: u64 = 1_000_000;
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let path = ffmpeg_dir.join(name);
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "vendored {name} missing at {}: {error} (run `git lfs pull`)",
                path.display()
            )
        })?;
        if metadata.len() < LFS_POINTER_CEILING {
            return Err(format!(
                "vendored {name} at {} is only {} bytes — this looks like a Git LFS \
                 pointer, not the real binary; run `git lfs pull` to fetch it",
                path.display(),
                metadata.len()
            ));
        }
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

/// Writes a `settings.toml` selecting the real `OneCore` synthesizer into a
/// staging directory on the host, mirroring `verbatim_e2e::scenario`'s
/// runner-direct staging step — just staged here rather than written
/// straight into a guest path, because in VM mode that directory only
/// exists inside the guest, not on the host running this command.
///
/// Always [`Settings::for_e2e`] written fresh, never a load-modify-save of
/// whatever this staging directory already held from a previous `deploy` —
/// this directory is reused run over run (it is not cleaned up between
/// invocations), so an existing file is removed first and a brand-new store
/// is built from defaults, guaranteeing the result is always exactly
/// [`Settings::for_e2e`]'s fixed shape and never accumulated state. See its
/// doc comment for why this same guarantee also lives in `verbatim-e2e`'s
/// `Scenario::launch`, independently, in lockstep.
///
/// Always `"onecore"`: unlike runner-direct mode, which defaults to the
/// audio-free capture synth and opts into `OneCore` only by hand, the VM
/// path deploys the real synthesizer unconditionally now — see
/// [`stage_and_copy`]'s own doc comment.
fn write_synth_settings(repo_root: &Path) -> VmResult<PathBuf> {
    let staging_dir = repo_root.join("target").join("xtask-vm-staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("could not create {}: {error}", staging_dir.display()))?;
    let settings_path = staging_dir.join(ConfigStore::SETTINGS_FILE);
    if settings_path.exists() {
        fs::remove_file(&settings_path).map_err(|error| {
            format!(
                "could not remove stale {}: {error}",
                settings_path.display()
            )
        })?;
    }
    let store = ConfigStore::from_settings(&staging_dir, Settings::for_e2e("onecore"));
    store
        .save_settings()
        .map_err(|error| format!("could not write settings.toml: {error}"))?;
    Ok(settings_path)
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
