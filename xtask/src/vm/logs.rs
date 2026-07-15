//! `cargo xtask vm logs`: pulls flight-recorder dumps and the agent's log
//! out of the guest over PowerShell Direct — the simpler alternative to the
//! agent's own `ReadFile` request that
//! `crates/verbatim-agent/src/protocol.rs` documents for exactly this kind
//! of host-side pull, and one `xtask vm` already needs `Host::run_in_guest`
//! for, so reusing it here needs no new machinery.

use std::fs;
use std::path::Path;

use super::dotenv::GuestCredentials;
use super::host::Host;
use super::{AGENT_DIR, VERBATIM_DIR, VM_NAME, VmResult};

const AGENT_LOG_NAME: &str = "agent.log";

/// Name (and, joined with [`VERBATIM_DIR`], guest path) of the stdout and
/// stderr capture file a launched Verbatim's `LaunchProcess.stderr_to`
/// names, matching `crates/verbatim-e2e/src/scenario.rs`'s remote-mode
/// path. This is exactly what a crash-diagnosis pull needs: the flight
/// recorder proves a panic happened, this file says what the panic message
/// was.
const VERBATIM_STDERR_LOG_NAME: &str = "stderr-e2e.log";

/// # Errors
///
/// Returns an error if `out_dir` cannot be created. Individual file
/// fetch failures (a missing dumps folder, no agent log yet) are logged and
/// skipped rather than failing the whole command, since "nothing to
/// collect yet" is an ordinary, expected state early in a VM's life.
pub(crate) fn run(host: &dyn Host, credentials: &GuestCredentials, out_dir: &Path) -> VmResult<()> {
    fs::create_dir_all(out_dir)
        .map_err(|error| format!("could not create {}: {error}", out_dir.display()))?;

    println!("xtask vm logs: fetching flight-recorder dumps");
    fetch_dir(
        host,
        credentials,
        &format!(r"{VERBATIM_DIR}\dumps"),
        &out_dir.join("dumps"),
    )?;

    println!("xtask vm logs: fetching the agent log");
    match host.read_guest_file(
        VM_NAME,
        credentials,
        &format!(r"{AGENT_DIR}\{AGENT_LOG_NAME}"),
    ) {
        Ok(bytes) => {
            let path = out_dir.join(AGENT_LOG_NAME);
            fs::write(&path, bytes)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        }
        Err(error) => println!("xtask vm logs: could not fetch the agent log: {error}"),
    }

    println!("xtask vm logs: fetching Verbatim's captured stderr log");
    match host.read_guest_file(
        VM_NAME,
        credentials,
        &format!(r"{VERBATIM_DIR}\{VERBATIM_STDERR_LOG_NAME}"),
    ) {
        Ok(bytes) => {
            let path = out_dir.join(VERBATIM_STDERR_LOG_NAME);
            fs::write(&path, bytes)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        }
        Err(error) => println!("xtask vm logs: could not fetch Verbatim's stderr log: {error}"),
    }

    println!("xtask vm logs: wrote logs to {}", out_dir.display());
    Ok(())
}

fn fetch_dir(
    host: &dyn Host,
    credentials: &GuestCredentials,
    remote_dir: &str,
    local_dir: &Path,
) -> VmResult<()> {
    let names = host.list_guest_dir(VM_NAME, credentials, remote_dir)?;
    if names.is_empty() {
        println!("xtask vm logs: {remote_dir} is empty or does not exist yet");
        return Ok(());
    }
    fs::create_dir_all(local_dir)
        .map_err(|error| format!("could not create {}: {error}", local_dir.display()))?;
    for name in names {
        let remote_path = format!(r"{remote_dir}\{name}");
        match host.read_guest_file(VM_NAME, credentials, &remote_path) {
            Ok(bytes) => {
                if let Err(error) = fs::write(local_dir.join(&name), bytes) {
                    println!("xtask vm logs: could not write {name}: {error}");
                }
            }
            Err(error) => println!("xtask vm logs: could not fetch {remote_path}: {error}"),
        }
    }
    Ok(())
}
