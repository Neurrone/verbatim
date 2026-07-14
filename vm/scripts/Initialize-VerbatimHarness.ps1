<#
.SYNOPSIS
Provisions the milestone M2 VM harness on top of the base Windows 11 image.

.DESCRIPTION
Runs after Initialize-VerbatimBaseImage.ps1 in the same Packer build. Turns
the base image into an unattended, deterministic E2E lab machine:

- Persistent autologon (Winlogon registry keys), so the automation user is
  always signed in interactively, forever, not just for Packer's own first
  boot (the answer file's AutoLogon covers only that one).
- Everything that could interrupt an unattended interactive session:
  screensaver, workstation locking, sleep and hibernate, Windows Update
  automatic reboots, and the network-location and first-logon prompts.
- A pinned 1920x1080 display resolution, so control positions are
  deterministic across runs.
- The Scream virtual audio driver (duncanthrax/scream), best-effort: a
  missing audio device must not fail image creation, since the capture
  synth plus null sink make most E2E scenarios device-free anyway.
- C:\VerbatimLab\agent and the VerbatimAgent scheduled task, which runs
  the agent at logon of the automation user, interactively (see that
  function's own comment for why this is the one non-negotiable rule
  here). The task fails harmlessly until cargo xtask vm deploy puts
  verbatim-agent.exe in place.
- An inbound firewall rule for the agent's TCP port.

Every step is idempotent and logs one fact per line, prefixed "harness:",
so a re-run (or a build retried after a transient failure) is safe and its
log is easy to skim.

.PARAMETER Username
The automation account's name. Defaults to $env:VERBATIM_VM_USERNAME, set
by the Packer build block from var.admin_username.

.PARAMETER Password
The automation account's password. Defaults to $env:VERBATIM_VM_PASSWORD,
set by the Packer build block from var.admin_password.

.PARAMETER LabRoot
Root directory for harness state. Defaults to C:\VerbatimLab, matching
Initialize-VerbatimBaseImage.ps1. Must not contain spaces (see
Register-VerbatimAgentTask's own comment for why).

.PARAMETER AgentPort
TCP port the firewall rule opens for the in-guest agent. Defaults to
44001 (verbatim_agent::protocol::DEFAULT_PORT).

Display resolution is not set here; it belongs to the host, which pins the
synthetic video adapter with Set-VMVideo. See the comment above this
script's Scream section for why a WinRM provisioning session cannot do it.
#>
[CmdletBinding()]
param(
    [string]$Username = $env:VERBATIM_VM_USERNAME,
    [string]$Password = $env:VERBATIM_VM_PASSWORD,
    [string]$LabRoot = "C:\VerbatimLab",
    [int]$AgentPort = 44001
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-HarnessFact {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host "harness: $Message"
}

# A small helper so every registry step is idempotent (creates the key if
# missing) without repeating the same three lines at every call site.
function Set-RegistryValue {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)]$Value,
        [ValidateSet("String", "DWord", "ExpandString")]
        [string]$Type = "DWord"
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -Path $Path -Force | Out-Null
    }
    New-ItemProperty -Path $Path -Name $Name -Value $Value -PropertyType $Type -Force | Out-Null
}

function Set-PersistentAutologon {
    param(
        [Parameter(Mandatory = $true)][string]$Username,
        [Parameter(Mandatory = $true)][string]$Password
    )

    $winlogonPath = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
    Set-RegistryValue -Path $winlogonPath -Name "AutoAdminLogon" -Value "1" -Type String
    Set-RegistryValue -Path $winlogonPath -Name "DefaultUserName" -Value $Username -Type String
    Set-RegistryValue -Path $winlogonPath -Name "DefaultPassword" -Value $Password -Type String
    Set-RegistryValue -Path $winlogonPath -Name "DefaultDomainName" -Value $env:COMPUTERNAME -Type String
    # AutoLogonCount, if present, caps how many boots autologon covers,
    # exactly like the answer file's own <AutoLogon><LogonCount>1</LogonCount>
    # (a separate mechanism, but the same idea) — the harness needs
    # autologon forever, so this must not exist.
    Remove-ItemProperty -Path $winlogonPath -Name "AutoLogonCount" -ErrorAction SilentlyContinue
    Write-HarnessFact "persistent autologon configured for $Username"
}

function Disable-UnattendedInterruptions {
    # HKLM policy paths only, deliberately: the Packer WinRM session that
    # runs this script does not reliably load the automation user's own
    # HKCU hive (WinRM's logon type does not always mount a normal user
    # profile), so a per-user HKCU setting here could silently fail to
    # apply to the profile that later autologons. Machine-wide policy
    # values are honored regardless of which profile is loaded.
    $desktopPolicy = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Control Panel\Desktop"
    Set-RegistryValue -Path $desktopPolicy -Name "ScreenSaveActive" -Value "0" -Type String
    Set-RegistryValue -Path $desktopPolicy -Name "ScreenSaverIsSecure" -Value "0" -Type String
    Set-RegistryValue -Path $desktopPolicy -Name "ScreenSaveTimeOut" -Value "0" -Type String
    Write-HarnessFact "screensaver disabled by policy"

    $winlogonPath = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
    Set-RegistryValue -Path $winlogonPath -Name "DisableLockWorkstation" -Value 1 -Type DWord
    Write-HarnessFact "workstation locking disabled"

    $auPolicy = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU"
    Set-RegistryValue -Path $auPolicy -Name "NoAutoRebootWithLoggedOnUsers" -Value 1 -Type DWord
    Set-RegistryValue -Path $auPolicy -Name "AlwaysAutoRebootAtScheduledTime" -Value 0 -Type DWord
    Write-HarnessFact "Windows Update automatic reboots disabled"

    $networkPolicy = "HKLM:\SYSTEM\CurrentControlSet\Control\Network"
    Set-RegistryValue -Path $networkPolicy -Name "NewNetworkWindowOff" -Value "" -Type String
    try {
        Get-NetConnectionProfile -ErrorAction Stop |
            Where-Object { $_.NetworkCategory -ne "Private" } |
            Set-NetConnectionProfile -NetworkCategory Private -ErrorAction Stop
        Write-HarnessFact "network location prompt suppressed; adapters set to Private"
    }
    catch {
        Write-HarnessFact "could not confirm every network adapter is Private: $($_.Exception.Message)"
    }

    $cloudContentPolicy = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\CloudContent"
    Set-RegistryValue -Path $cloudContentPolicy -Name "DisableWindowsConsumerFeatures" -Value 1 -Type DWord
    $explorerPolicy = "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Explorer"
    Set-RegistryValue -Path $explorerPolicy -Name "DisableFirstLogonAnimation" -Value 1 -Type DWord
    Write-HarnessFact "first-logon nag content and animation disabled"
}

function Disable-SleepAndHibernate {
    $changeArgumentSets = @(
        @("/change", "standby-timeout-ac", "0"),
        @("/change", "standby-timeout-dc", "0"),
        @("/change", "hibernate-timeout-ac", "0"),
        @("/change", "hibernate-timeout-dc", "0"),
        @("/change", "monitor-timeout-ac", "0"),
        @("/change", "monitor-timeout-dc", "0")
    )
    foreach ($changeArguments in $changeArgumentSets) {
        & powercfg.exe @changeArguments | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "powercfg $($changeArguments -join ' ') failed with exit code $LASTEXITCODE"
        }
    }

    & powercfg.exe /hibernate off | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "powercfg /hibernate off failed with exit code $LASTEXITCODE"
    }
    Write-HarnessFact "sleep, monitor timeout, and hibernate disabled"
}

# Display resolution is deliberately NOT set from inside the guest.
#
# This script runs as a Packer provisioner over WinRM, and a WinRM session
# has no interactive window station and therefore no display: the Win32
# display APIs (EnumDisplaySettings and ChangeDisplaySettings) fail outright
# there, whatever the P/Invoke marshaling looks like. This is the same
# session-isolation rule the whole harness is built around, and an earlier
# version of this script learned it the hard way by failing the image build.
#
# The synthetic video adapter's resolution belongs to the host anyway, so it
# is set from there with Set-VMVideo (see xtask's vm module), which needs no
# guest session at all and applies before the guest ever boots.

function Install-VcRedistributable {
    <#
    Installs the Visual C++ x64 redistributable. Rust MSVC binaries link
    the C runtime dynamically by default, so verbatim-agent.exe and
    verbatim.exe need vcruntime140.dll — which a fresh Windows 11 image
    does not have. The first live deploy discovered this the hard way: the
    agent's scheduled task exited instantly with 0xC0000135
    (STATUS_DLL_NOT_FOUND) and an empty log. Unlike Scream this is NOT
    best-effort: without the runtime nothing deployed to this image can
    run, so a failure here should fail the image build loudly.
    #>
    if (Test-Path 'C:\Windows\System32\vcruntime140.dll') {
        Write-HarnessFact "VC++ runtime already present"
        return
    }
    $installer = Join-Path $env:TEMP "vc_redist.x64.exe"
    # Microsoft's evergreen link for the latest supported x64 redistributable.
    Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vc_redist.x64.exe" -OutFile $installer -UseBasicParsing
    $process = Start-Process -FilePath $installer -ArgumentList "/install", "/quiet", "/norestart" -Wait -PassThru
    # 0 is success; 3010 is success, restart required (fine: the image is
    # rebooted by sysprep/first boot anyway).
    if ($process.ExitCode -ne 0 -and $process.ExitCode -ne 3010) {
        throw "vc_redist.x64.exe failed with exit code $($process.ExitCode)"
    }
    Remove-Item $installer -Force -ErrorAction SilentlyContinue
    Write-HarnessFact "VC++ x64 redistributable installed"
}

function Install-ScreamAudioDriver {
    <#
    Installs the Scream virtual audio driver so the guest always has a
    WASAPI render endpoint, even with no Enhanced Session RDP audio
    redirection attached. Best-effort: any failure here is logged and
    swallowed, never propagated — a missing audio device must not fail
    image creation, per this script's own header comment.

    Stages the driver with pnputil (as instructed: import the signing
    certificate to the trusted publisher store, then pnputil /add-driver),
    but pnputil alone cannot create a device node for a driver with no
    matching hardware present — Scream is a software-only device, and
    Windows has no built-in way to root-enumerate one. So the last step
    uses devcon, bundled in the same downloaded, hash-verified zip, exactly
    the way Scream's own Install-x64.bat does it (devcon install
    Scream.inf *Scream). This is a deliberate, documented deviation from a
    pnputil-only install; see the Phase 4 report for the reasoning.
    #>
    param(
        [string]$Version = "4.0",
        [string]$DownloadUrl = "https://github.com/duncanthrax/scream/releases/download/4.0/Scream4.0.zip",
        # Verified by downloading this exact release asset and computing
        # its SHA-256 with Get-FileHash.
        [string]$ExpectedSha256 = "FA33E25F9A46C61E4E0CD83362C51C3D2A45C6FE4091AAD7507E240E40F1A520"
    )

    try {
        if (Get-PnpDevice -Class MEDIA -FriendlyName "Scream*" -ErrorAction SilentlyContinue) {
            Write-HarnessFact "Scream audio driver already present; skipping install"
            return
        }

        $workDir = Join-Path $env:TEMP "verbatim-scream-$Version"
        New-Item -ItemType Directory -Force -Path $workDir | Out-Null
        $zipPath = Join-Path $workDir "Scream$Version.zip"

        Write-HarnessFact "downloading Scream $Version from $DownloadUrl"
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $zipPath -UseBasicParsing

        $actualSha256 = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash
        if ($actualSha256 -ne $ExpectedSha256) {
            throw "Scream download SHA-256 mismatch: expected $ExpectedSha256, got $actualSha256"
        }
        Write-HarnessFact "Scream download SHA-256 verified"

        $extractDir = Join-Path $workDir "extracted"
        Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

        $architecture = switch ($env:PROCESSOR_ARCHITECTURE) {
            "ARM64" { "arm64" }
            default { "x64" }
        }
        $driverDir = Join-Path $extractDir "Install\driver\$architecture"
        $infPath = Join-Path $driverDir "Scream.inf"
        $catPath = Join-Path $driverDir "scream.cat"
        if (-not (Test-Path -LiteralPath $infPath)) {
            throw "Scream driver INF not found for architecture $architecture at $infPath"
        }

        $signature = Get-AuthenticodeSignature -FilePath $catPath
        if ($signature.SignerCertificate) {
            $certPath = Join-Path $workDir "scream.cer"
            Export-Certificate -Cert $signature.SignerCertificate -FilePath $certPath | Out-Null
            Import-Certificate -FilePath $certPath -CertStoreLocation "Cert:\LocalMachine\TrustedPublisher" | Out-Null
            Import-Certificate -FilePath $certPath -CertStoreLocation "Cert:\LocalMachine\Root" | Out-Null
            Write-HarnessFact "imported the Scream driver signing certificate to the trusted publisher store"
        }
        else {
            Write-HarnessFact "scream.cat has no readable signing certificate; attempting install anyway"
        }

        $pnputilOutput = & pnputil.exe /add-driver $infPath 2>&1
        $pnputilOutput | ForEach-Object { Write-HarnessFact "pnputil: $_" }
        if ($LASTEXITCODE -ne 0) {
            throw "pnputil /add-driver exited with code $LASTEXITCODE"
        }

        $devconPath = Join-Path $extractDir "Install\helpers\devcon-$architecture.exe"
        if (-not (Test-Path -LiteralPath $devconPath)) {
            throw "devcon helper not found for architecture $architecture at $devconPath"
        }
        & $devconPath remove "*Scream" 2>&1 | ForEach-Object { Write-HarnessFact "devcon: $_" }
        $installOutput = & $devconPath install $infPath "*Scream" 2>&1
        $installOutput | ForEach-Object { Write-HarnessFact "devcon: $_" }
        if ($LASTEXITCODE -ne 0) {
            throw "devcon install exited with code $LASTEXITCODE"
        }

        Write-HarnessFact "Scream audio driver installed for architecture $architecture"
    }
    catch {
        Write-HarnessFact "Scream audio driver install failed, continuing without it: $($_.Exception.Message)"
    }
}

function Register-VerbatimAgentTask {
    param(
        [Parameter(Mandatory = $true)][string]$Username,
        [Parameter(Mandatory = $true)][string]$AgentPath,
        [Parameter(Mandatory = $true)][string]$LogPath
    )

    # The cmd.exe wrapper below has no quoting around either path: it
    # relies on LabRoot (and so both paths) containing no spaces. That is
    # true of the C:\VerbatimLab default and worth checking rather than
    # producing a task that silently never starts anything.
    if ($AgentPath -match '\s' -or $LogPath -match '\s') {
        throw "Register-VerbatimAgentTask: paths must not contain spaces (agent: '$AgentPath', log: '$LogPath')"
    }

    $action = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c $AgentPath > $LogPath 2>&1"
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $Username
    # LogonType Interactive plus a UserId (no password/S4U) is Task
    # Scheduler's "run only when user is logged on": the task gets the
    # user's real interactive token, the only kind a screen reader can
    # speak through (docs/architecture.md's window-station rule, verified
    # at agent startup by verbatim-agent's own SessionInfo check). SYSTEM
    # or "run whether user is logged on or not" both hand it a
    # non-interactive session instead, which must never happen here.
    $principal = New-ScheduledTaskPrincipal -UserId $Username -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -DontStopOnIdleEnd `
        -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -RestartCount 999 `
        -RestartInterval (New-TimeSpan -Minutes 1)

    Register-ScheduledTask -TaskName "VerbatimAgent" -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
    Write-HarnessFact "VerbatimAgent scheduled task registered for $Username (interactive logon only)"
}

function New-VerbatimAgentFirewallRule {
    param([Parameter(Mandatory = $true)][int]$Port)

    $displayName = "Verbatim Agent (TCP $Port)"
    if (Get-NetFirewallRule -DisplayName $displayName -ErrorAction SilentlyContinue) {
        Write-HarnessFact "firewall rule '$displayName' already present"
        return
    }
    New-NetFirewallRule -DisplayName $displayName -Direction Inbound -Protocol TCP -LocalPort $Port -Action Allow -Profile Private, Domain | Out-Null
    Write-HarnessFact "firewall rule '$displayName' created (Private, Domain profiles)"
}

if ([string]::IsNullOrWhiteSpace($Username)) {
    throw "Username was not supplied and VERBATIM_VM_USERNAME is not set"
}
if ([string]::IsNullOrWhiteSpace($Password)) {
    throw "Password was not supplied and VERBATIM_VM_PASSWORD is not set"
}

$agentDir = Join-Path $LabRoot "agent"
New-Item -ItemType Directory -Force -Path $agentDir | Out-Null
Write-HarnessFact "agent install directory ready at $agentDir"

Set-PersistentAutologon -Username $Username -Password $Password
Disable-UnattendedInterruptions
Disable-SleepAndHibernate
Install-VcRedistributable
Install-ScreamAudioDriver
Register-VerbatimAgentTask -Username $Username -AgentPath (Join-Path $agentDir "verbatim-agent.exe") -LogPath (Join-Path $agentDir "agent.log")
New-VerbatimAgentFirewallRule -Port $AgentPort

Write-HarnessFact "provisioning complete"
