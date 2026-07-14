# Windows 11 Hyper-V VM harness

This directory holds the milestone M2 VM harness: a Packer template that
builds a provisioning-ready Windows 11 Pro Hyper-V image, a harness
provisioner that turns that image into an unattended E2E lab machine, and
the assets `cargo xtask vm` drives from there (see
`docs/architecture.md` section 14 and `docs/roadmap.md` M2).

## Host requirements

- Windows host with Hyper-V enabled.
- Packer 1.15 or newer, with the HashiCorp Hyper-V plugin installed by
  `packer init`.
- A Hyper-V virtual switch, usually `Default Switch`.
- A local Windows 11 business-editions x64 ISO.
- An ISO creation tool available to Packer for `cd_content`, such as
  `oscdimg.exe` from the Windows ADK Deployment Tools.
- A repository-root `.env` file (gitignored, never committed) with
  `VERBATIM_VM_USERNAME` and `VERBATIM_VM_PASSWORD`: the guest's local
  administrator credentials, reused for autologon, the `VerbatimAgent`
  scheduled task, and every `xtask vm` verb that reaches the guest over
  PowerShell Direct.

## Local variables

Create an ignored local var file:

```powershell
Copy-Item vm\example.pkrvars.hcl vm\local.pkrvars.hcl
```

Edit `local.pkrvars.hcl` with the local ISO path, SHA-256, Hyper-V switch
name, temporary build path, and a local administrator password matching
`.env`. Do not commit `local.pkrvars.hcl`.

Set `temp_path` to an existing directory on a volume with enough free space
for the temporary VHDX. Packer's Hyper-V builder uses the host temp
directory by default.

The `temp_path` and output directories must not be NTFS-compressed.
Hyper-V cannot use or import compressed VHDX files, and child directories
can inherit compression from a parent folder.

The image is not activated during provisioning. Activation and licensing
are local operator responsibilities.

## Validate

```powershell
packer init vm
packer fmt -check vm
```

## Build

Run the wrapper from the repository root:

```powershell
.\vm\scripts\Build-VerbatimWindows11Image.ps1 -Force
```

Show wrapper help and examples:

```powershell
Get-Help .\vm\scripts\Build-VerbatimWindows11Image.ps1 -Detailed
Get-Help .\vm\scripts\Build-VerbatimWindows11Image.ps1 -Examples
```

The wrapper creates the temporary and output directories, clears NTFS
compression before Packer creates VHDX files, runs `packer validate`, runs
`packer build`, clears compression on the final export, and checks the
exported `.vmcx` with `Compare-VM`.

During the build, Packer runs two PowerShell provisioners in order:
`scripts/Initialize-VerbatimBaseImage.ps1` (WinRM automation prerequisites
and `C:\VerbatimLab\image.json`), then
`scripts/Initialize-VerbatimHarness.ps1` (everything the M2 harness needs
on top: persistent autologon, an unattended-friendly session, a pinned
1920x1080 display resolution, a best-effort Scream virtual audio device,
the `VerbatimAgent` scheduled task, and its firewall rule — see that
script's own header comment for the full, idempotent step list).

The generated image output is written under `artifacts/packer/windows11`
by default and must not be committed.

Useful wrapper parameters:

- `-Force` passes `-force` to `packer build`, allowing Packer to overwrite
  an existing output directory for the same image build. Use this when
  rebuilding into `artifacts/packer/windows11`.
- `-VarFile <path>` uses a different `.pkrvars.hcl` file. Defaults to
  `vm/local.pkrvars.hcl`.
- `-TempPath <path>` overrides the temporary Hyper-V build path instead of
  reading `temp_path` from the var file.
- `-OutputDirectory <path>` overrides the final Packer output directory
  instead of reading `output_directory` from the var file or using the
  template default.
- `-OscdimgPath <path>` uses an explicit `oscdimg.exe` path. If omitted,
  the wrapper searches `PATH` and standard Windows ADK Deployment Tools
  install locations, then adds the tool directory to the process `PATH`
  for Packer.
- `-SkipBuild` runs host preflight and `packer validate` without starting
  the VM build. Useful for checking paths and compression state.
- `-SkipCompareVm` skips the post-build Hyper-V import compatibility
  check.

## From image to a running, testable VM

Building the image only produces a `.vmcx` export; it does not register,
start, or checkpoint a VM. `cargo xtask vm create` does the rest: it runs
the build above, imports the export as a VM named `verbatim`, starts it,
deploys a debug build so the in-guest agent is actually running, waits for
the agent to answer on its TCP port, then takes a checkpoint named
`golden`. From there:

- `cargo xtask vm test` restores `golden`, deploys the current build on
  top of it, and runs `crates/verbatim-e2e`'s suite against the guest.
- `cargo xtask vm start` / `stop` / `restart` / `restore [checkpoint]`
  manage the VM's power state directly.
- `cargo xtask vm deploy` copies a fresh build in without touching
  checkpoints.
- `cargo xtask vm logs` pulls flight-recorder dumps and the agent's log
  out of the guest.
- `cargo xtask vm delete` removes the VM and its disks for a clean
  rebuild.

Run `cargo xtask vm` with no further arguments for the full verb list.

## Scope

`vm/` builds and provisions the base image and the harness on top of it.
Verbatim payload deployment, checkpoint management, and running E2E
scenarios are `cargo xtask vm`'s job, in `xtask/src/vm/`; the in-guest
agent that verb talks to is `crates/verbatim-agent`.
