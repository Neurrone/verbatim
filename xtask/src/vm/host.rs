//! The host abstraction: every Hyper-V-specific detail lives behind the
//! [`Host`] trait, expressed as PowerShell run through [`HyperVHost`]. Verb
//! modules (`create`, `deploy`, `test`, `lifecycle`, `logs`) only ever see
//! `&dyn Host`, so a future QEMU/KVM implementation (the deferred CI story
//! in `docs/architecture.md` section 14) can be dropped in without touching
//! them.
//!
//! PowerShell is invoked non-interactively against a temp script file
//! (`-File`, not `-Command`) so argument quoting never has to survive two
//! layers of shell parsing. Methods that need to return a value out of
//! PowerShell wrap it in a pair of unique text markers and read only what
//! falls between them, rather than trusting that a cmdlet's own stdout is
//! clean: Hyper-V cmdlets occasionally write extra informational text to
//! the success stream, and a naive whole-output parse would break on it.

use std::fmt::Write as _;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{fs, thread};

use base64::Engine as _;

use super::dotenv::GuestCredentials;

/// The error type every `vm` verb uses: a plain message, matching `ci()`'s
/// own style in `xtask/src/main.rs` rather than introducing an error-handling
/// dependency for a command-line tool whose failures are meant to be read,
/// not programmatically matched.
pub(crate) type VmResult<T> = Result<T, String>;

/// Marker text bracketing a script's JSON result on stdout, so it can be
/// extracted from whatever other output the script produced.
const RESULT_BEGIN: &str = "===XTASK-VM-RESULT-BEGIN===";
const RESULT_END: &str = "===XTASK-VM-RESULT-END===";

/// The statement forms scripts embed to *emit* the markers. The bare marker
/// text is not a valid PowerShell statement — interpolating it directly
/// into a script makes PowerShell try to run it as a command, which is
/// exactly the failure the first live run of `import_vm` produced.
const RESULT_BEGIN_STMT: &str = "Write-Output '===XTASK-VM-RESULT-BEGIN==='";
const RESULT_END_STMT: &str = "Write-Output '===XTASK-VM-RESULT-END==='";

/// How long [`wait_for_agent`] waits for the in-guest agent to accept a TCP
/// connection before giving up.
const AGENT_WAIT_TIMEOUT: Duration = Duration::from_mins(5);
/// Interval between [`wait_for_agent`]'s polling attempts.
const AGENT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Per-attempt timeout for the TCP connect probe itself.
const PORT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Every Hyper-V (or, later, other hypervisor) operation the `vm` verbs
/// need, expressed abstractly enough that a non-Hyper-V implementation is
/// plausible.
pub(crate) trait Host {
    /// Whether a VM named `name` is currently registered.
    fn vm_exists(&self, name: &str) -> VmResult<bool>;

    /// Imports the VM described by an exported `.vmcx` file as a copy under
    /// `destination_dir`, returning the name the hypervisor assigned it
    /// (the exported VM's own name).
    ///
    /// A copying import, not register-in-place, and deliberately so: the
    /// Packer-exported VHDX is owned by SYSTEM, and an in-place
    /// `Import-VM` fails with access denied for an unelevated caller
    /// because the security rewrite it needs impersonates that caller.
    /// With a copy the virtualization service itself does the copying and
    /// owns the result, so no ACL surgery is needed — and the export stays
    /// pristine for the next `create --skip-build`.
    fn import_vm(&self, vmcx_path: &Path, destination_dir: &Path) -> VmResult<String>;

    /// Renames a registered VM.
    fn rename_vm(&self, current_name: &str, new_name: &str) -> VmResult<()>;

    /// Ensures the channel file copies into the guest travel over is
    /// enabled (Hyper-V's Guest Service Interface integration service,
    /// which `Copy-VMFile` depends on and which is off by default).
    fn ensure_guest_file_transfer(&self, name: &str) -> VmResult<()>;

    /// Pins the guest's display resolution.
    ///
    /// Deliberately a host operation. The guest cannot do this to itself
    /// while it is being provisioned: a Packer `WinRM` session has no
    /// interactive window station, so the Win32 display APIs
    /// (`EnumDisplaySettings` and friends) fail there outright — the same
    /// session-isolation rule the whole harness is built around. The
    /// synthetic video adapter belongs to the hypervisor anyway, so setting
    /// it here needs no guest session at all.
    fn set_display_resolution(&self, name: &str, width: u32, height: u32) -> VmResult<()>;

    /// Starts a VM. A no-op, not an error, if it is already running.
    fn start_vm(&self, name: &str) -> VmResult<()>;

    /// Stops a VM.
    fn stop_vm(&self, name: &str) -> VmResult<()>;

    /// Restarts a VM.
    fn restart_vm(&self, name: &str) -> VmResult<()>;

    /// Takes a checkpoint.
    fn checkpoint_vm(&self, name: &str, checkpoint_name: &str) -> VmResult<()>;

    /// Restores a named checkpoint.
    fn restore_checkpoint(&self, name: &str, checkpoint_name: &str) -> VmResult<()>;

    /// Removes a VM and its virtual hard disks entirely.
    fn delete_vm(&self, name: &str) -> VmResult<()>;

    /// The guest's IPv4 address, once its integration services have
    /// reported one.
    fn guest_ip(&self, name: &str) -> VmResult<String>;

    /// Copies a local file into the guest at `remote_path`, creating any
    /// missing destination directories.
    fn copy_file_to_guest(&self, name: &str, local_path: &Path, remote_path: &str) -> VmResult<()>;

    /// Runs a PowerShell script block inside the guest over PowerShell
    /// Direct, returning its output as text.
    fn run_in_guest(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        script_block: &str,
    ) -> VmResult<String>;

    /// Reads a guest file's raw bytes over PowerShell Direct.
    fn read_guest_file(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        remote_path: &str,
    ) -> VmResult<Vec<u8>>;

    /// Lists the file names (not full paths) directly inside a guest
    /// directory, or an empty list if it does not exist.
    fn list_guest_dir(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        remote_dir: &str,
    ) -> VmResult<Vec<String>>;
}

/// [`Host`] implemented over Hyper-V's PowerShell module.
pub(crate) struct HyperVHost;

impl Host for HyperVHost {
    fn vm_exists(&self, name: &str) -> VmResult<bool> {
        let script = format!(
            "if (Get-VM -Name {name} -ErrorAction SilentlyContinue) {{ Write-Output 'EXISTS: yes' }} else {{ Write-Output 'EXISTS: no' }}",
            name = ps_quote(name)
        );
        let stdout = run_ps("checking whether the VM exists", &script)?;
        Ok(stdout.lines().any(|line| line.trim() == "EXISTS: yes"))
    }

    fn import_vm(&self, vmcx_path: &Path, destination_dir: &Path) -> VmResult<String> {
        let vhd_dir = destination_dir.join("Virtual Hard Disks");
        // The destination must not be NTFS-compressed: Hyper-V refuses to
        // place a VHDX in a compressed directory (the same rule the Packer
        // build wrapper's preflight handles for its own directories), and
        // directories under the repo can inherit compression from a parent.
        let script = format!(
            "New-Item -ItemType Directory -Force -Path {dest}, {vhd} | Out-Null\n\
             compact.exe /u /i /f {dest} | Out-Null\n\
             compact.exe /u /i /f {vhd} | Out-Null\n\
             $vm = Import-VM -Path {path} -Copy -GenerateNewId -VirtualMachinePath {dest} -VhdDestinationPath {vhd}\n{begin}\n$vm.Name | ConvertTo-Json -Compress\n{end}",
            path = ps_quote(&vmcx_path.display().to_string()),
            dest = ps_quote(&destination_dir.display().to_string()),
            vhd = ps_quote(&vhd_dir.display().to_string()),
            begin = RESULT_BEGIN_STMT,
            end = RESULT_END_STMT,
        );
        let stdout = run_ps("importing the exported VM", &script)?;
        extract_json_string(&stdout)
    }

    fn rename_vm(&self, current_name: &str, new_name: &str) -> VmResult<()> {
        let script = format!(
            "Rename-VM -Name {current} -NewName {new}",
            current = ps_quote(current_name),
            new = ps_quote(new_name)
        );
        run_ps("renaming the VM", &script)?;
        Ok(())
    }

    fn ensure_guest_file_transfer(&self, name: &str) -> VmResult<()> {
        let script = format!(
            "Enable-VMIntegrationService -VMName {name} -Name 'Guest Service Interface'",
            name = ps_quote(name)
        );
        run_ps("enabling the Guest Service Interface", &script)?;
        Ok(())
    }

    fn set_display_resolution(&self, name: &str, width: u32, height: u32) -> VmResult<()> {
        let script = format!(
            "Set-VMVideo -VMName {name} -HorizontalResolution {width} -VerticalResolution {height} -ResolutionType Single",
            name = ps_quote(name)
        );
        run_ps("pinning the guest display resolution", &script)?;
        Ok(())
    }

    fn start_vm(&self, name: &str) -> VmResult<()> {
        let script = format!(
            "$vm = Get-VM -Name {name}\nif ($vm.State -ne 'Running') {{ Start-VM -Name {name} }}",
            name = ps_quote(name)
        );
        run_ps("starting the VM", &script)?;
        Ok(())
    }

    fn stop_vm(&self, name: &str) -> VmResult<()> {
        let script = format!("Stop-VM -Name {name} -Force", name = ps_quote(name));
        run_ps("stopping the VM", &script)?;
        Ok(())
    }

    fn restart_vm(&self, name: &str) -> VmResult<()> {
        let script = format!("Restart-VM -Name {name} -Force", name = ps_quote(name));
        run_ps("restarting the VM", &script)?;
        Ok(())
    }

    fn checkpoint_vm(&self, name: &str, checkpoint_name: &str) -> VmResult<()> {
        let script = format!(
            "Checkpoint-VM -Name {name} -SnapshotName {checkpoint}",
            name = ps_quote(name),
            checkpoint = ps_quote(checkpoint_name)
        );
        run_ps("checkpointing the VM", &script)?;
        Ok(())
    }

    fn restore_checkpoint(&self, name: &str, checkpoint_name: &str) -> VmResult<()> {
        let script = format!(
            "$checkpoint = Get-VMSnapshot -VMName {name} -Name {checkpoint} -ErrorAction Stop\nRestore-VMSnapshot -VMSnapshot $checkpoint -Confirm:$false",
            name = ps_quote(name),
            checkpoint = ps_quote(checkpoint_name)
        );
        run_ps("restoring the checkpoint", &script)?;
        Ok(())
    }

    fn delete_vm(&self, name: &str) -> VmResult<()> {
        let script = format!(
            "if (-not (Get-VM -Name {name} -ErrorAction SilentlyContinue)) {{ Write-Output 'not found; nothing to delete'; exit 0 }}\n\
             $disks = Get-VMHardDiskDrive -VMName {name} | Select-Object -ExpandProperty Path\n\
             Stop-VM -Name {name} -TurnOff -Force -ErrorAction SilentlyContinue\n\
             Get-VMSnapshot -VMName {name} -ErrorAction SilentlyContinue | Remove-VMSnapshot -Confirm:$false\n\
             Remove-VM -Name {name} -Force\n\
             foreach ($disk in $disks) {{ if (Test-Path -LiteralPath $disk) {{ Remove-Item -LiteralPath $disk -Force }} }}",
            name = ps_quote(name)
        );
        run_ps("deleting the VM and its disks", &script)?;
        Ok(())
    }

    fn guest_ip(&self, name: &str) -> VmResult<String> {
        let script = format!(
            "$address = (Get-VMNetworkAdapter -VMName {name}).IPAddresses | Where-Object {{ $_ -match '^\\d+\\.\\d+\\.\\d+\\.\\d+$' }} | Select-Object -First 1\n{begin}\n$address | ConvertTo-Json -Compress\n{end}",
            name = ps_quote(name),
            begin = RESULT_BEGIN_STMT,
            end = RESULT_END_STMT,
        );
        let stdout = run_ps("discovering the guest IP address", &script)?;
        let value = extract_json_string(&stdout)?;
        if value.is_empty() {
            return Err(format!(
                "VM '{name}' has no IPv4 address yet (Get-VMNetworkAdapter reported none)"
            ));
        }
        Ok(value)
    }

    fn copy_file_to_guest(&self, name: &str, local_path: &Path, remote_path: &str) -> VmResult<()> {
        let script = format!(
            "Copy-VMFile -Name {name} -SourcePath {local} -DestinationPath {remote} -FileSource Host -CreateFullPath -Force",
            name = ps_quote(name),
            local = ps_quote(&local_path.display().to_string()),
            remote = ps_quote(remote_path)
        );
        run_ps(
            &format!("copying {} to the guest", local_path.display()),
            &script,
        )?;
        Ok(())
    }

    fn run_in_guest(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        script_block: &str,
    ) -> VmResult<String> {
        let script = format!(
            "$securePassword = ConvertTo-SecureString {password} -AsPlainText -Force\n\
             $credential = New-Object System.Management.Automation.PSCredential({username}, $securePassword)\n\
             {begin}\n\
             $result = Invoke-Command -VMName {name} -Credential $credential -ScriptBlock {{ {block} }}\n\
             $result | Out-String\n\
             {end}",
            password = ps_quote(&credentials.password),
            username = ps_quote(&credentials.username),
            name = ps_quote(name),
            block = script_block,
            begin = RESULT_BEGIN_STMT,
            end = RESULT_END_STMT,
        );
        let stdout = run_ps(
            "running a command in the guest over PowerShell Direct",
            &script,
        )?;
        extract_between(&stdout, RESULT_BEGIN, RESULT_END)
            .map(str::trim)
            .map(str::to_owned)
    }

    fn read_guest_file(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        remote_path: &str,
    ) -> VmResult<Vec<u8>> {
        let escaped = remote_path.replace('\'', "''");
        let block = format!(
            "if (Test-Path -LiteralPath '{escaped}') {{ [Convert]::ToBase64String([System.IO.File]::ReadAllBytes('{escaped}')) }} else {{ '' }}"
        );
        let base64_text = self.run_in_guest(name, credentials, &block)?;
        if base64_text.is_empty() {
            return Err(format!("{remote_path} does not exist on the guest"));
        }
        base64::engine::general_purpose::STANDARD
            .decode(base64_text)
            .map_err(|error| format!("{remote_path}: guest returned invalid base64: {error}"))
    }

    fn list_guest_dir(
        &self,
        name: &str,
        credentials: &GuestCredentials,
        remote_dir: &str,
    ) -> VmResult<Vec<String>> {
        let escaped = remote_dir.replace('\'', "''");
        let block = format!(
            "if (Test-Path -LiteralPath '{escaped}') {{ (Get-ChildItem -LiteralPath '{escaped}' -File | Select-Object -ExpandProperty Name) -join \"`n\" }} else {{ '' }}"
        );
        let output = self.run_in_guest(name, credentials, &block)?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }
}

/// Polls `host.guest_ip` and then a raw TCP connect to the agent's port
/// until one succeeds or [`AGENT_WAIT_TIMEOUT`] elapses. Free rather than a
/// [`Host`] method: it is pure orchestration over other `Host` calls, so a
/// future non-Hyper-V host gets it for free.
pub(crate) fn wait_for_agent(host: &dyn Host, vm_name: &str) -> VmResult<()> {
    let deadline = Instant::now() + AGENT_WAIT_TIMEOUT;
    let mut last_error: String;
    loop {
        match host.guest_ip(vm_name).and_then(|ip| probe_port(&ip)) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "agent on '{vm_name}' never answered on port {}: {last_error}",
                super::AGENT_PORT
            ));
        }
        thread::sleep(AGENT_POLL_INTERVAL);
    }
}

fn probe_port(ip: &str) -> VmResult<()> {
    let addr: SocketAddr = format!("{ip}:{}", super::AGENT_PORT)
        .parse()
        .map_err(|error| format!("'{ip}' is not a valid IPv4 address: {error}"))?;
    TcpStream::connect_timeout(&addr, PORT_PROBE_TIMEOUT)
        .map(|_stream| ())
        .map_err(|error| error.to_string())
}

/// Wraps `value` in single quotes for interpolation into a PowerShell
/// script, doubling any embedded single quotes (PowerShell's own escaping
/// rule for single-quoted string literals).
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn extract_between<'text>(text: &'text str, begin: &str, end: &str) -> VmResult<&'text str> {
    let start = text
        .find(begin)
        .ok_or_else(|| format!("PowerShell output is missing the {begin} marker:\n{text}"))?;
    let after_begin = start + begin.len();
    let stop = text[after_begin..]
        .find(end)
        .ok_or_else(|| format!("PowerShell output is missing the {end} marker:\n{text}"))?;
    Ok(text[after_begin..after_begin + stop].trim())
}

fn extract_json_string(stdout: &str) -> VmResult<String> {
    let raw = extract_between(stdout, RESULT_BEGIN, RESULT_END)?;
    if raw.is_empty() {
        return Ok(String::new());
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| format!("could not parse PowerShell JSON result {raw:?}: {error}"))?;
    match value {
        serde_json::Value::String(text) => Ok(text),
        serde_json::Value::Null => Ok(String::new()),
        other => Err(format!("expected a JSON string result, got {other}")),
    }
}

/// Runs `script` non-interactively via `powershell.exe -File`, returning
/// its stdout. Every script is prefixed with `$ErrorActionPreference =
/// 'Stop'` (so a failing cmdlet fails the whole invocation instead of being
/// silently skipped) and `$ProgressPreference = 'SilentlyContinue'` (so
/// progress bars from cmdlets such as `Copy-VMFile` cannot leak stray text
/// into stdout that a result-marker parse would trip over).
fn run_ps(description: &str, script: &str) -> VmResult<String> {
    let prefixed = format!(
        "$ErrorActionPreference = 'Stop'\n$ProgressPreference = 'SilentlyContinue'\n{script}"
    );
    let temp_script = TempScript::write(&prefixed).map_err(|error| {
        format!("{description}: could not write a temp PowerShell script: {error}")
    })?;

    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(temp_script.path())
        .output()
        .map_err(|error| format!("{description}: failed to launch powershell.exe: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let mut message = format!("{description}: powershell exited with {}", output.status);
        if !stdout.trim().is_empty() {
            let _ = write!(message, "\nstdout:\n{stdout}");
        }
        if !stderr.trim().is_empty() {
            let _ = write!(message, "\nstderr:\n{stderr}");
        }
        return Err(message);
    }
    Ok(stdout)
}

/// A temp `.ps1` file, removed on drop, that outlives the `powershell.exe`
/// invocation reading it. Using `-File` instead of `-Command` means every
/// script here needs to survive exactly one layer of parsing (PowerShell's
/// own), not a shell's quoting rules as well.
struct TempScript {
    path: PathBuf,
}

impl TempScript {
    fn write(contents: &str) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "xtask-vm-{}-{:?}.ps1",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::write(&path, contents)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_quote_doubles_embedded_single_quotes() {
        assert_eq!(ps_quote("plain"), "'plain'");
        assert_eq!(ps_quote("has'quote"), "'has''quote'");
    }

    #[test]
    fn extract_between_finds_the_marked_payload() {
        let text = format!("noise before\n{RESULT_BEGIN}\npayload\n{RESULT_END}\nnoise after");
        assert_eq!(
            extract_between(&text, RESULT_BEGIN, RESULT_END).unwrap(),
            "payload"
        );
    }

    #[test]
    fn extract_between_reports_a_missing_marker() {
        let error = extract_between("no markers here", RESULT_BEGIN, RESULT_END).unwrap_err();
        assert!(error.contains(RESULT_BEGIN));
    }

    #[test]
    fn extract_json_string_reads_a_quoted_value() {
        let text = format!("{RESULT_BEGIN}\n\"10.0.0.5\"\n{RESULT_END}");
        assert_eq!(extract_json_string(&text).unwrap(), "10.0.0.5");
    }

    #[test]
    fn extract_json_string_treats_null_as_empty() {
        let text = format!("{RESULT_BEGIN}\nnull\n{RESULT_END}");
        assert_eq!(extract_json_string(&text).unwrap(), "");
    }
}
