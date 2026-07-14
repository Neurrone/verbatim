//! `cargo xtask vm test`: restores the golden checkpoint, deploys the
//! current build on top of it, discovers the guest's IP, then runs
//! `crates/verbatim-e2e`'s suite on the host against it.
//!
//! The suite's two host-filesystem steps — checking `verbatim.exe` exists
//! and writing the capture-synth `settings.toml` next to it — are correct
//! only in runner-direct mode, where the suite and Verbatim share a
//! filesystem. Here `VERBATIM_E2E_VERBATIM_EXE` names a path inside the
//! guest, so this verb sets `VERBATIM_E2E_REMOTE` as well, which tells
//! `verbatim_e2e::Scenario::launch` to skip both: [`super::deploy::run`]
//! has already staged that same configuration inside the guest.

use std::path::Path;
use std::process::Command;

use super::host::{Host, wait_for_agent};
use super::{AGENT_PORT, CHECKPOINT_NAME, VERBATIM_DIR, VM_NAME, VmResult, deploy, dotenv};

/// # Errors
///
/// Returns an error if the checkpoint restore, the deploy, IP discovery, or
/// the E2E suite itself fails.
pub(crate) fn test(host: &dyn Host, repo_root: &Path) -> VmResult<()> {
    let credentials = dotenv::load_guest_credentials(repo_root)?;

    println!("xtask vm test: restoring checkpoint '{CHECKPOINT_NAME}'");
    host.restore_checkpoint(VM_NAME, CHECKPOINT_NAME)?;
    host.start_vm(VM_NAME)?;
    wait_for_agent(host, VM_NAME)?;

    println!("xtask vm test: deploying the current build");
    deploy::run(host, repo_root, &credentials)?;
    wait_for_agent(host, VM_NAME)?;

    let ip = host.guest_ip(VM_NAME)?;
    let endpoint = format!("{ip}:{AGENT_PORT}");
    let guest_exe = format!(r"{VERBATIM_DIR}\verbatim.exe");
    println!("xtask vm test: running the E2E suite against {endpoint}");

    let status = Command::new(env!("CARGO"))
        .args(["test", "-p", "verbatim-e2e", "--", "--test-threads=1"])
        .env("VERBATIM_E2E_ENDPOINT", &endpoint)
        .env("VERBATIM_E2E_VERBATIM_EXE", &guest_exe)
        .env("VERBATIM_E2E_REMOTE", "1")
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("failed to launch cargo test: {error}"))?;

    if !status.success() {
        return Err(format!("E2E suite failed: {status}"));
    }
    println!("xtask vm test: E2E suite passed");
    Ok(())
}
