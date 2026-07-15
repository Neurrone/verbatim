//! `cargo xtask vm connect`: opens an interactive, already-authenticated
//! desktop session into the guest with audio playing on this computer, for
//! a human to watch and listen alongside `cargo xtask vm test --audible`.
//!
//! `vmconnect.exe`'s two session kinds both fall short of that goal: an
//! Enhanced Session is RDP under the hood and always demands an interactive
//! sign-in (there is no credential API to skip it), while a Basic Session
//! needs no sign-in but redirects no audio. This verb sidesteps both by
//! driving a direct RDP connection with stored credentials instead of
//! `vmconnect.exe`: `mstsc.exe` against the guest's IP, authenticated from
//! Windows Credential Manager so no sign-in prompt appears, with audio
//! redirection turned on.
//!
//! Because this signs in as the same guest user the autologon session
//! already runs as, RDP takes over that session rather than creating a
//! second one — the same session the in-guest agent and `verbatim.exe`
//! itself run in, so E2E keeps working while a human is connected, and
//! closing the RDP window disconnects rather than logging off.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::dotenv::{self, GuestCredentials};
use super::host::{Host, wait_for_agent};
use super::{VM_NAME, VmResult};

/// Ensures the VM is running, then either opens an RDP session (the default)
/// or, when `forget` is set, removes the stored credentials for the guest's
/// current address and exits without connecting.
///
/// # Errors
///
/// Returns an error if the VM cannot be started, the guest's IP cannot be
/// discovered, the in-guest RDP enablement fails, `cmdkey.exe` fails, the
/// scratch `.rdp` file cannot be written, or `mstsc.exe` cannot be launched.
pub(crate) fn connect(host: &dyn Host, repo_root: &Path, forget: bool) -> VmResult<()> {
    let credentials = dotenv::load_guest_credentials(repo_root)?;

    println!("xtask vm connect: ensuring '{VM_NAME}' is running");
    host.start_vm(VM_NAME)?;
    wait_for_agent(host, VM_NAME)?;
    let ip = host.guest_ip(VM_NAME)?;

    if forget {
        return forget_credentials(&ip);
    }

    ensure_rdp_enabled(host, &credentials)?;
    store_credentials(&ip, &credentials)?;
    let rdp_path = write_rdp_file(repo_root, &ip, &credentials.username)?;
    launch_mstsc(&rdp_path)?;

    println!(
        "xtask vm connect: opening an RDP session to {ip} with audio redirected to this \
         computer; same-user RDP takes over the guest's autologon session — the same session \
         the agent and tests run in, so E2E keeps working while connected — and closing the \
         RDP window disconnects (the session keeps running) rather than logging off"
    );
    Ok(())
}

/// One-time, idempotent RDP enablement over PowerShell Direct: allows
/// Terminal Server connections, opens the firewall, and reports (without
/// touching) Network Level Authentication, which stays on — stored
/// credentials satisfy it, so there is no need to weaken it. Every fact
/// checked or changed is printed on its own line; a re-run against an
/// already-enabled guest prints only "already" lines.
fn ensure_rdp_enabled(host: &dyn Host, credentials: &GuestCredentials) -> VmResult<()> {
    println!("xtask vm connect: checking in-guest Remote Desktop settings");
    let script = "\
        $tsKey = 'HKLM:\\System\\CurrentControlSet\\Control\\Terminal Server'\n\
        $deny = (Get-ItemProperty -Path $tsKey -Name fDenyTSConnections).fDenyTSConnections\n\
        if ($deny -ne 0) {\n\
        Set-ItemProperty -Path $tsKey -Name fDenyTSConnections -Value 0\n\
        Write-Output 'Remote Desktop connections: were denied; allowed now'\n\
        } else {\n\
        Write-Output 'Remote Desktop connections: already allowed'\n\
        }\n\
        $disabledRules = Get-NetFirewallRule -DisplayGroup 'Remote Desktop' | Where-Object { $_.Enabled -eq 'False' }\n\
        if ($disabledRules) {\n\
        Enable-NetFirewallRule -DisplayGroup 'Remote Desktop'\n\
        Write-Output 'Remote Desktop firewall rules: were disabled; enabled now'\n\
        } else {\n\
        Write-Output 'Remote Desktop firewall rules: already enabled'\n\
        }\n\
        $rdpTcpKey = 'HKLM:\\System\\CurrentControlSet\\Control\\Terminal Server\\WinStations\\RDP-Tcp'\n\
        $nla = (Get-ItemProperty -Path $rdpTcpKey -Name UserAuthentication -ErrorAction SilentlyContinue).UserAuthentication\n\
        Write-Output \"Network Level Authentication: left on (UserAuthentication=$nla); stored credentials satisfy it\"\n\
        ";
    let output = host.run_in_guest(VM_NAME, credentials, script)?;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        println!("xtask vm connect: {line}");
    }
    Ok(())
}

/// Stores `credentials` in the host's Windows Credential Manager, keyed to
/// `TERMSRV/<ip>` — the target name `mstsc.exe` looks up when connecting to
/// that address — so the RDP session authenticates without a sign-in
/// prompt. This writes the guest's test credentials onto the host running
/// `cargo xtask vm connect`; run with `--forget` to remove them again.
fn store_credentials(ip: &str, credentials: &GuestCredentials) -> VmResult<()> {
    println!(
        "xtask vm connect: storing credentials for TERMSRV/{ip} in the host's Credential Manager"
    );
    let output = Command::new("cmdkey.exe")
        .arg(format!("/add:TERMSRV/{ip}"))
        .arg(format!("/user:{}", credentials.username))
        .arg(format!("/pass:{}", credentials.password))
        .output()
        .map_err(|error| format!("failed to launch cmdkey.exe: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cmdkey /add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    println!("xtask vm connect: credentials stored");
    Ok(())
}

/// Removes whatever credentials `store_credentials` wrote for `ip`'s
/// `TERMSRV/<ip>` target. A missing entry is not treated as an error by
/// `cmdkey.exe` itself failing loudly here — the point is a clean state
/// either way, and `cmdkey`'s own message is passed through.
fn forget_credentials(ip: &str) -> VmResult<()> {
    println!("xtask vm connect: removing stored credentials for TERMSRV/{ip}");
    let output = Command::new("cmdkey.exe")
        .arg(format!("/delete:TERMSRV/{ip}"))
        .output()
        .map_err(|error| format!("failed to launch cmdkey.exe: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        println!("xtask vm connect: {line}");
    }
    if !output.status.success() {
        return Err(format!(
            "cmdkey /delete failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Writes a scratch `.rdp` file selecting audio redirection to this
/// computer, a windowed 1920x1080 geometry, clipboard redirection, and the
/// guest username so `mstsc.exe` matches the credential `store_credentials`
/// stored under the same address.
fn write_rdp_file(repo_root: &Path, ip: &str, username: &str) -> VmResult<PathBuf> {
    let staging_dir = repo_root.join("target").join("xtask-vm-staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("could not create {}: {error}", staging_dir.display()))?;
    let path = staging_dir.join("verbatim.rdp");
    fs::write(&path, rdp_file_contents(ip, username))
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(path)
}

/// The `.rdp` file body itself, pure so it can be unit tested without
/// touching the filesystem. `audiomode:i:0` plays sound on this computer;
/// `authentication level:i:0` connects even though the guest's self-signed
/// RDP certificate is not trusted, which is expected for a lab VM.
fn rdp_file_contents(ip: &str, username: &str) -> String {
    format!(
        "full address:s:{ip}\r\n\
         username:s:{username}\r\n\
         audiomode:i:0\r\n\
         authentication level:i:0\r\n\
         desktopwidth:i:1920\r\n\
         desktopheight:i:1080\r\n\
         screen mode id:i:1\r\n\
         redirectclipboard:i:1\r\n"
    )
}

/// Launches `mstsc.exe` against the scratch `.rdp` file, detached: this
/// process does not wait for it, since the whole point is a session the
/// human keeps open after `cargo xtask vm connect` itself has returned.
fn launch_mstsc(rdp_path: &Path) -> VmResult<()> {
    Command::new("mstsc.exe")
        .arg(rdp_path)
        .spawn()
        .map_err(|error| format!("failed to launch mstsc.exe: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_file_contents_names_the_guest_address_and_audio_mode() {
        let contents = rdp_file_contents("10.0.0.5", "verbatim");
        assert!(contents.contains("full address:s:10.0.0.5\r\n"));
        assert!(contents.contains("username:s:verbatim\r\n"));
        assert!(contents.contains("audiomode:i:0\r\n"));
        assert!(contents.contains("redirectclipboard:i:1\r\n"));
    }
}
