//! `cargo xtask vm create`: builds the base image with Packer, imports and
//! renames the exported VM, starts it, deploys a build so the golden
//! checkpoint already has a running agent (see [`super::deploy`]'s doc
//! comment), then checkpoints it as `golden`.

use std::path::Path;

use super::host::{Host, wait_for_agent};
use super::{CHECKPOINT_NAME, VM_NAME, VmResult, deploy, dotenv, packer_build};

/// Pinned guest display width; a stable, ordinary desktop size, set from
/// the host because a provisioning session cannot set it from inside (see
/// [`Host::set_display_resolution`]).
const DISPLAY_WIDTH: u32 = 1920;

/// Pinned guest display height; see [`DISPLAY_WIDTH`].
const DISPLAY_HEIGHT: u32 = 1080;

/// # Errors
///
/// Returns an error, with a message naming the stage that failed, if a VM
/// named [`VM_NAME`] already exists, the Packer build fails (or, with
/// `skip_build`, no exported image exists to reuse), the exported VM cannot
/// be found or imported, or the guest never reaches a state where its agent
/// answers.
pub(crate) fn create(host: &dyn Host, repo_root: &Path, skip_build: bool) -> VmResult<()> {
    if host.vm_exists(VM_NAME)? {
        return Err(format!(
            "VM '{VM_NAME}' already exists; run `cargo xtask vm delete` first for a clean rebuild"
        ));
    }

    if skip_build {
        println!("xtask vm create: reusing the existing Packer export (--skip-build)");
    } else {
        println!("xtask vm create: running the Packer build (this takes a long time)");
        packer_build::build_image(repo_root)?;
    }

    println!("xtask vm create: locating the exported .vmcx");
    let vmcx = packer_build::locate_exported_vmcx(repo_root)?;
    println!("xtask vm create: found {}", vmcx.display());

    // The virtualization service copies the export here and owns the
    // copy; see `Host::import_vm` for why a register-in-place import
    // cannot work unelevated.
    let vm_home = repo_root.join("artifacts").join("vm");
    println!(
        "xtask vm create: importing a copy of the exported VM into {}",
        vm_home.display()
    );
    let imported_name = host.import_vm(&vmcx, &vm_home)?;
    println!("xtask vm create: imported as '{imported_name}'");

    println!("xtask vm create: renaming '{imported_name}' to '{VM_NAME}'");
    host.rename_vm(&imported_name, VM_NAME)?;

    println!("xtask vm create: enabling guest file transfer");
    host.ensure_guest_file_transfer(VM_NAME)?;

    println!("xtask vm create: pinning the display to {DISPLAY_WIDTH}x{DISPLAY_HEIGHT}");
    host.set_display_resolution(VM_NAME, DISPLAY_WIDTH, DISPLAY_HEIGHT)?;

    println!("xtask vm create: starting '{VM_NAME}'");
    host.start_vm(VM_NAME)?;

    let credentials = dotenv::load_guest_credentials(repo_root)?;
    println!("xtask vm create: deploying a build so the golden checkpoint has a running agent");
    deploy::run(host, repo_root, &credentials)?;

    println!("xtask vm create: waiting for the agent to answer");
    wait_for_agent(host, VM_NAME)?;

    println!("xtask vm create: checkpointing '{CHECKPOINT_NAME}'");
    host.checkpoint_vm(VM_NAME, CHECKPOINT_NAME)?;

    println!("xtask vm create: done; 'cargo xtask vm test' is ready to run");
    Ok(())
}
