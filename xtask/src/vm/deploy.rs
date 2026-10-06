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
use super::{AGENT_DIR, FFMPEG_GUEST_PATH, VERBATIM_DIR, VM_NAME, VmResult};

/// One artifact `deploy` may need to copy into the guest.
struct Artifact {
    /// Human-readable name used in progress output.
    label: String,
    /// The file's path on the host, after building.
    local_path: PathBuf,
    /// The file's destination path inside the guest.
    remote_path: String,
    /// Whether a running guest process might hold this file open for
    /// execution, and so must be stopped before it can be overwritten.
    /// `settings.toml` is data, not code, so it is never held open this way.
    is_executable: bool,
    /// A guest PowerShell script to run once the file has been copied.
    after_copy: Option<String>,
}

/// The host paths [`build`] produces, threaded into [`stage_and_copy`]. A
/// named struct rather than a tuple so `xtask::vm::test` (which calls the
/// two phases separately — see that module's doc comment for why) reads as
/// clearly as [`run`]'s own internal composition of them.
pub(crate) struct BuiltArtifacts {
    verbatim: PathBuf,
    outpost: PathBuf,
    synth_host: PathBuf,
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
    println!(
        "xtask vm deploy: building verbatim-app, verbatim-agent, verbatim-outpost, \
         verbatim-synth-host (debug)"
    );
    build_binaries(repo_root)?;

    let target_dir = repo_root.join("target").join("debug");
    Ok(BuiltArtifacts {
        verbatim: require_artifact(&target_dir, "verbatim.exe")?,
        outpost: require_artifact(&target_dir, "verbatim-outpost.exe")?,
        synth_host: require_artifact(&target_dir, "verbatim-synth-host.exe")?,
        agent: require_artifact(&target_dir, "verbatim-agent.exe")?,
    })
}

/// Stages a `settings.toml` selecting eSpeak NG, hashes it, `built`'s four
/// binaries, and the vendored `ffmpeg.exe` against the guest's copies, and
/// copies only the ones that differ. Stops the guest's `VerbatimAgent`
/// scheduled task and any running Verbatim first, but only when at least
/// one executable actually needs copying (a live process can hold an
/// executable open for `Copy-VMFile`, but never `settings.toml`); restarts
/// the task afterward if it was stopped, or if `verbatim-agent.exe` itself
/// was among the copied artifacts.
///
/// Always eSpeak NG, unconditionally — see [`write_synth_settings`]: `cargo
/// xtask vm test` is always audible, so there is nothing for this function
/// to branch on.
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

    let data_dir = built.synth_host.with_file_name(ESPEAK_DATA);
    let mut artifacts = vec![
        Artifact {
            label: "verbatim.exe".to_owned(),
            local_path: built.verbatim,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim.exe"),
            is_executable: true,
            after_copy: None,
        },
        Artifact {
            label: "verbatim-outpost.exe".to_owned(),
            local_path: built.outpost,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim-outpost.exe"),
            is_executable: true,
            after_copy: None,
        },
        Artifact {
            label: "verbatim-synth-host.exe".to_owned(),
            local_path: built.synth_host,
            remote_path: format!(r"{VERBATIM_DIR}\verbatim-synth-host.exe"),
            is_executable: true,
            after_copy: None,
        },
        Artifact {
            label: "settings.toml".to_owned(),
            local_path: settings_path,
            remote_path: format!(r"{VERBATIM_DIR}\settings.toml"),
            is_executable: false,
            after_copy: None,
        },
        Artifact {
            label: "verbatim-agent.exe".to_owned(),
            local_path: built.agent,
            remote_path: format!(r"{AGENT_DIR}\verbatim-agent.exe"),
            is_executable: true,
            after_copy: None,
        },
        // The LFS-vendored ffmpeg (vm/vendor/ffmpeg) that each scenario's
        // recording runs in the guest (`verbatim_e2e::recording`), staged the
        // same fast PowerShell Direct way as everything else rather than
        // baked into the golden image (see vm/vendor/ffmpeg/README.md). It
        // hash-skips after the first deploy, so the copy is paid once, into
        // golden. Not marked executable: it is never held open at deploy time
        // (ffmpeg runs only inside a scenario, which never overlaps a
        // deploy), so a lone version bump need not stop the guest.
        Artifact {
            label: "ffmpeg.exe".to_owned(),
            local_path: ffmpeg_dir.join("ffmpeg.exe"),
            remote_path: FFMPEG_GUEST_PATH.to_owned(),
            is_executable: false,
            after_copy: None,
        },
    ];

    // eSpeak NG's data, which the synthesizer host reads next to itself:
    // about 380 files, shipped as one archive so it is hashed and copied
    // once, then unpacked in the guest. Marked executable because a running
    // host may hold the files open.
    let archive = archive_espeak_data(repo_root, &data_dir)?;
    artifacts.push(Artifact {
        label: ESPEAK_ARCHIVE.to_owned(),
        local_path: archive,
        remote_path: format!(r"{VERBATIM_DIR}\{ESPEAK_ARCHIVE}"),
        is_executable: true,
        after_copy: Some(
            [
                format!(
                    r"Remove-Item -Recurse -Force -LiteralPath '{VERBATIM_DIR}\{ESPEAK_DATA}' -ErrorAction SilentlyContinue"
                ),
                format!(r"tar -xf '{VERBATIM_DIR}\{ESPEAK_ARCHIVE}' -C '{VERBATIM_DIR}'"),
                format!(
                    r#"if ($LASTEXITCODE -ne 0) {{ throw "unpacking {ESPEAK_ARCHIVE} failed" }}"#
                ),
            ]
            .join("\n"),
        ),
    });

    let needs_copy = artifacts_needing_copy(host, credentials, &artifacts)?;
    if needs_copy.is_empty() {
        println!("xtask vm deploy: all artifacts up to date; guest left untouched");
        return Ok(());
    }

    copy_mismatched_artifacts(host, credentials, &artifacts, &needs_copy)
}

/// eSpeak NG's data directory, next to the synthesizer host.
const ESPEAK_DATA: &str = "espeak-ng-data";

/// The archive eSpeak NG's data is shipped to the guest as.
const ESPEAK_ARCHIVE: &str = "espeak-ng-data.tar";

/// Archives eSpeak NG's data directory (next to the built executables) with
/// Windows' own `tar`, into the deploy staging directory.
fn archive_espeak_data(repo_root: &Path, data_dir: &Path) -> VmResult<PathBuf> {
    let staging_dir = repo_root.join("target").join("xtask-vm-staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("could not create {}: {error}", staging_dir.display()))?;
    let archive = staging_dir.join(ESPEAK_ARCHIVE);
    let parent = data_dir
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", data_dir.display()))?;
    // Windows' own tar, by its full path: under Git Bash a bare `tar` is
    // GNU tar, which reads `C:\...` as a remote host.
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let tar = Path::new(&system_root).join("System32").join("tar.exe");
    let status = std::process::Command::new(tar)
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(parent)
        .arg(ESPEAK_DATA)
        .status()
        .map_err(|error| format!("could not run tar: {error}"))?;
    if !status.success() {
        return Err(format!("archiving {} failed: {status}", data_dir.display()));
    }
    Ok(archive)
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
/// task if it was stopped or `verbatim-agent.exe` itself was copied — even
/// when a copy failed (see [`copy_then_restart`]).
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
             Stop-Process -Name 'verbatim-agent','verbatim','verbatim-outpost','verbatim-synth-host' -Force -ErrorAction SilentlyContinue\n\
             Start-Sleep -Seconds 1",
        )?;
        stopped_guest = true;
    }

    let mut copied_labels = Vec::new();
    let mut skipped_labels = Vec::new();
    let copy_all = || -> VmResult<()> {
        for (index, artifact) in artifacts.iter().enumerate() {
            if needs_copy.contains(&index) {
                println!(
                    "xtask vm deploy: copying {} to {}",
                    artifact.label, artifact.remote_path
                );
                host.copy_file_to_guest(VM_NAME, &artifact.local_path, &artifact.remote_path)?;
                if let Some(script) = &artifact.after_copy {
                    host.run_in_guest(VM_NAME, credentials, script)?;
                }
                copied_labels.push(artifact.label.as_str());
            } else {
                skipped_labels.push(artifact.label.as_str());
            }
        }
        Ok(())
    };
    let restart_agent = || -> VmResult<()> {
        println!("xtask vm deploy: starting the VerbatimAgent scheduled task");
        host.run_in_guest(
            VM_NAME,
            credentials,
            "Start-ScheduledTask -TaskName 'VerbatimAgent'",
        )?;
        Ok(())
    };
    copy_then_restart(copy_all, stopped_guest || agent_needs_copy, restart_agent)?;

    println!(
        "xtask vm deploy: done; copied [{}], skipped [{}]",
        copied_labels.join(", "),
        skipped_labels.join(", ")
    );
    Ok(())
}

/// Runs `copy`, then `restart` when `restart_needed`, whether or not `copy`
/// succeeded: a copy that fails partway must not leave the guest with its
/// agent stopped, or the next run waits for an agent that never comes back.
/// The copy's error, when there is one, is the one returned; a restart that
/// also fails is appended to it, since the guest's agent is then down.
fn copy_then_restart(
    copy: impl FnOnce() -> VmResult<()>,
    restart_needed: bool,
    restart: impl FnOnce() -> VmResult<()>,
) -> VmResult<()> {
    let copied = copy();
    let restarted = if restart_needed { restart() } else { Ok(()) };
    match (copied, restarted) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(copy_error), Err(restart_error)) => Err(format!(
            "{copy_error}; restarting the VerbatimAgent scheduled task afterwards also failed, \
             so the guest's agent is stopped: {restart_error}"
        )),
    }
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

/// Builds Verbatim, its child processes, and the agent (debug profile).
fn build_binaries(repo_root: &Path) -> VmResult<()> {
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
            "-p",
            "verbatim-synth-host",
        ])
        .current_dir(repo_root);
    let status = command
        .status()
        .map_err(|error| format!("failed to launch cargo build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed: {status}"));
    }
    Ok(())
}

/// Confirms the vendored `ffmpeg.exe` is the real binary and not a Git LFS
/// pointer stub (which is only a couple of hundred bytes). A clone made
/// without `git lfs` leaves that stub in place; copying it into the guest
/// would make every scenario's recording fail later with a cryptic in-guest
/// ffmpeg launch warning instead of the clear, actionable message here. The
/// real static build is about 100 MB, so a 1 MB floor distinguishes it from
/// a pointer with no risk of a false alarm.
fn ensure_vendored_ffmpeg(ffmpeg_dir: &Path) -> VmResult<()> {
    const LFS_POINTER_CEILING: u64 = 1_000_000;
    let path = ffmpeg_dir.join("ffmpeg.exe");
    let metadata = fs::metadata(&path).map_err(|error| {
        format!(
            "vendored ffmpeg.exe missing at {}: {error} (run `git lfs pull`)",
            path.display()
        )
    })?;
    if metadata.len() < LFS_POINTER_CEILING {
        return Err(format!(
            "vendored ffmpeg.exe at {} is only {} bytes — this looks like a Git LFS \
             pointer, not the real binary; run `git lfs pull` to fetch it",
            path.display(),
            metadata.len()
        ));
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

/// Writes a `settings.toml` selecting eSpeak NG into a
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
/// Always `"espeak"`: the VM path deploys a real synthesizer
/// unconditionally — see [`stage_and_copy`]'s own doc comment.
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
    let store = ConfigStore::from_settings(&staging_dir, Settings::for_e2e("espeak"));
    store
        .save_settings()
        .map_err(|error| format!("could not write settings.toml: {error}"))?;
    Ok(settings_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn espeak_data_is_archived_as_one_tar_of_its_directory() {
        let root =
            std::env::temp_dir().join(format!("verbatim-xtask-archive-{}", std::process::id()));
        let data = root.join("debug").join(ESPEAK_DATA);
        fs::create_dir_all(data.join("voices")).expect("creates the data");
        fs::write(data.join("voices").join("en"), b"voice").expect("writes a file");

        let archive = archive_espeak_data(&root, &data).expect("archives");
        let listing = std::process::Command::new(
            Path::new(&std::env::var_os("SystemRoot").expect("set on Windows"))
                .join("System32")
                .join("tar.exe"),
        )
        .arg("-tf")
        .arg(&archive)
        .output()
        .expect("lists the archive");
        let listing = String::from_utf8_lossy(&listing.stdout);
        assert!(listing.contains("espeak-ng-data/voices/en"), "{listing}");
        let _ = fs::remove_dir_all(root);
    }

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
    fn a_failed_copy_still_restarts_the_agent_and_reports_the_copy_error() {
        let mut restarted = false;
        let result = copy_then_restart(
            || Err("copy failed".to_owned()),
            true,
            || {
                restarted = true;
                Ok(())
            },
        );
        assert!(restarted, "the agent must be restarted after a failed copy");
        assert_eq!(result, Err("copy failed".to_owned()));
    }

    #[test]
    fn a_failed_copy_and_restart_report_both() {
        let result = copy_then_restart(
            || Err("copy failed".to_owned()),
            true,
            || Err("restart failed".to_owned()),
        );
        let error = result.expect_err("both failures are an error");
        assert!(error.contains("copy failed") && error.contains("restart failed"));
    }

    #[test]
    fn no_restart_when_none_is_needed() {
        let mut restarted = false;
        let result = copy_then_restart(
            || Ok(()),
            false,
            || {
                restarted = true;
                Ok(())
            },
        );
        assert!(!restarted);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn parse_guest_hashes_ignores_blank_lines() {
        let output = "\nC:\\a\\verbatim.exe=ABCD1234\n\n";
        let parsed = parse_guest_hashes(output);
        assert_eq!(parsed.len(), 1);
    }
}
