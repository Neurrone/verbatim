//! The straightforward lifecycle verbs: `start`, `stop`, `restart`,
//! `restore`, and `delete`. `create`, `deploy`, and `test` each need enough
//! orchestration to warrant their own modules; these do not.

use std::path::Path;

use super::host::{Host, renew_guest_dhcp, wait_for_agent};
use super::{CHECKPOINT_NAME, VM_NAME, VmResult, dotenv};

pub(crate) fn start(host: &dyn Host) -> VmResult<()> {
    println!("xtask vm start: starting '{VM_NAME}'");
    host.start_vm(VM_NAME)?;
    wait_for_agent(host, VM_NAME)?;
    println!("xtask vm start: agent is answering");
    Ok(())
}

pub(crate) fn stop(host: &dyn Host) -> VmResult<()> {
    println!("xtask vm stop: stopping '{VM_NAME}'");
    host.stop_vm(VM_NAME)?;
    println!("xtask vm stop: stopped");
    Ok(())
}

pub(crate) fn restart(host: &dyn Host) -> VmResult<()> {
    println!("xtask vm restart: restarting '{VM_NAME}'");
    host.restart_vm(VM_NAME)?;
    wait_for_agent(host, VM_NAME)?;
    println!("xtask vm restart: agent is answering");
    Ok(())
}

pub(crate) fn restore(host: &dyn Host, repo_root: &Path, checkpoint: Option<&str>) -> VmResult<()> {
    let checkpoint = checkpoint.unwrap_or(CHECKPOINT_NAME);
    println!("xtask vm restore: restoring checkpoint '{checkpoint}'");
    host.restore_checkpoint(VM_NAME, checkpoint)?;
    // A checkpoint taken while the VM was running restores running, but
    // start_vm is a no-op in that case (see its own doc comment) and a
    // cheap safety net if a checkpoint was ever taken while stopped.
    host.start_vm(VM_NAME)?;
    // The resumed guest may hold a DHCP lease from a Default Switch subnet
    // that no longer exists (see renew_guest_dhcp); renew before waiting.
    let credentials = dotenv::load_guest_credentials(repo_root)?;
    if let Err(error) = renew_guest_dhcp(host, VM_NAME, &credentials) {
        eprintln!("xtask vm restore: guest DHCP renewal failed (continuing): {error}");
    }
    wait_for_agent(host, VM_NAME)?;
    println!("xtask vm restore: agent is answering");
    Ok(())
}

pub(crate) fn delete(host: &dyn Host) -> VmResult<()> {
    println!("xtask vm delete: deleting '{VM_NAME}' and its disks");
    host.delete_vm(VM_NAME)?;
    println!("xtask vm delete: done");
    Ok(())
}
