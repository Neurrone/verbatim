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
- The VB-CABLE virtual audio driver (VB-Audio), required, unlike the Scream
  driver this replaces (which failed to root-enumerate a device node under
  Secure Boot): cargo xtask vm test is audible by default now, and needs a
  real WASAPI render endpoint to speak through. VB-CABLE becomes the guest's
  only render endpoint, so Windows auto-selects it as the default with no
  separate pinning step here — see Install-VbCableAudioDriver's own comment.
  Set-DefaultAudioRenderDevice.ps1 is still staged to C:\VerbatimLab\tools (by
  the Packer file provisioner ahead of this script) for cargo xtask vm test
  --record to re-assert that pin at record time, in case a prior cargo xtask
  vm connect session left a stale default. ffmpeg and ffprobe, which --record
  launches and probes with, are NOT installed here: cargo xtask vm deploy
  copies them into C:\VerbatimLab\tools over PowerShell Direct at deploy time
  (see vm/vendor/ffmpeg/README.md), so they are not part of this image.
- C:\VerbatimLab\agent and the VerbatimAgent scheduled task, which runs
  the agent at logon of the automation user, interactively (see that
  function's own comment for why this is the one non-negotiable rule
  here), with restart-on-failure so an agent killed by, for example, an RDP
  disconnect tearing down its session comes back on its own. The task
  fails harmlessly until cargo xtask vm deploy puts verbatim-agent.exe in
  place.
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
synthetic video adapter with Set-VMVideo. See the comment preceding
Install-VcRedistributable for why a WinRM provisioning session cannot do
it.
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

function Install-VbCableAudioDriver {
    <#
    Installs the VB-CABLE virtual audio driver (VB-Audio) so the guest has
    a real WASAPI render endpoint ("Speakers (VB-Audio Virtual Cable)") and
    a matching loopback capture endpoint ("CABLE Output (VB-Audio Virtual
    Cable)") that cargo xtask vm test --record captures audio from. This
    replaces Scream, which failed to root-enumerate a device node under
    Secure Boot. Unlike Scream's own best-effort install, this one is NOT
    best-effort: a recorded run needs a real audio device, so a failure
    here fails the image build loudly rather than continuing without one.

    Recipe, proven against a live running guest before being folded into
    this idempotent provisioner:

    1. Download and hash-verify the VB-CABLE driver pack, and (for its
       devcon helper only, not for the driver itself) the Scream release
       zip.
    2. Trust the driver's Authenticode signing certificate — read off
       vbaudio_cable64_win7.sys, not the .cat file Scream's own install
       used — to Cert:\LocalMachine\TrustedPublisher only. No
       Cert:\LocalMachine\Root change is needed: this certificate chains to
       a trusted VeriSign root already, confirmed live.
    3. Stage the driver into the driver store with pnputil, then use devcon
       (pnputil alone cannot create a device node for a driver with no
       matching hardware present, exactly the constraint Scream's own
       install needed devcon for) to root-enumerate the device against
       hardware id VBAudioVACWDM.
    4. Assert a MEDIA-class PnP device and a Win32_SoundDevice entry both
       name VB-Audio, so a silent partial install fails the build instead
       of surfacing only later as a recorded run with no sound.

    The guest's only render endpoint becomes VB-CABLE's, so Windows
    auto-selects it as the default with no separate "set default device"
    step — confirmed live: OneCore played to it immediately with no
    Set-AudioDevice-equivalent call anywhere in this harness.

    Today's guest is x64 only (per docs/tooling.md's Windows 11 x64 ISO
    prerequisite), and the driver pack itself ships no ARM64 variant
    (only 32- and 64-bit x86 INFs), so this function is deliberately
    x64-only rather than branching on PROCESSOR_ARCHITECTURE the way
    Scream's install used to for a possible future ARM64 guest.
    #>
    param(
        [string]$CableDownloadUrl = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack43.zip",
        # Verified by downloading this exact release asset and computing
        # its SHA-256 with Get-FileHash.
        [string]$CableExpectedSha256 = "66FD0A4D9F4896FF41632B7E3D53892C085C4561F53E8AE8D0F0BC10EEDD1CDD",
        # Only devcon.exe is taken from this zip, not the Scream driver
        # itself; see this function's own doc comment.
        [string]$DevconDownloadUrl = "https://github.com/duncanthrax/scream/releases/download/4.0/Scream4.0.zip",
        [string]$DevconExpectedSha256 = "FA33E25F9A46C61E4E0CD83362C51C3D2A45C6FE4091AAD7507E240E40F1A520"
    )

    if (Get-PnpDevice -Class MEDIA -FriendlyName "*VB-Audio*" -ErrorAction SilentlyContinue) {
        Write-HarnessFact "VB-CABLE audio driver already present; skipping install"
        return
    }

    $workDir = Join-Path $env:TEMP "verbatim-vbcable"
    New-Item -ItemType Directory -Force -Path $workDir | Out-Null

    $cableZip = Join-Path $workDir "VBCABLE_Driver_Pack43.zip"
    Write-HarnessFact "downloading VB-CABLE from $CableDownloadUrl"
    Invoke-WebRequest -Uri $CableDownloadUrl -OutFile $cableZip -UseBasicParsing
    $cableActualSha256 = (Get-FileHash -Algorithm SHA256 -Path $cableZip).Hash
    if ($cableActualSha256 -ne $CableExpectedSha256) {
        throw "VB-CABLE download SHA-256 mismatch: expected $CableExpectedSha256, got $cableActualSha256"
    }
    Write-HarnessFact "VB-CABLE download SHA-256 verified"

    $cableDir = Join-Path $workDir "cable"
    Expand-Archive -Path $cableZip -DestinationPath $cableDir -Force

    $infPath = Join-Path $cableDir "vbMmeCable64_win7.inf"
    $sysPath = Join-Path $cableDir "vbaudio_cable64_win7.sys"
    if (-not (Test-Path -LiteralPath $infPath)) {
        throw "VB-CABLE driver INF not found at $infPath"
    }
    if (-not (Test-Path -LiteralPath $sysPath)) {
        throw "VB-CABLE driver binary not found at $sysPath"
    }

    $signature = Get-AuthenticodeSignature -FilePath $sysPath
    if (-not $signature.SignerCertificate) {
        throw "vbaudio_cable64_win7.sys has no readable signing certificate; refusing to install an unsigned driver"
    }
    # Add the signing certificate to the machine's TrustedPublisher store via
    # the .NET X509Store API rather than the Import-Certificate cmdlet. In the
    # Packer WinRM provisioning session, Import-Certificate throws
    # UnauthorizedAccessException opening that store, whereas the direct
    # X509Store ReadWrite open succeeds — confirmed live, this exact swap is
    # what let the headless install go through.
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store("TrustedPublisher", "LocalMachine")
    $store.Open("ReadWrite")
    $store.Add($signature.SignerCertificate)
    $store.Close()
    Write-HarnessFact "imported the VB-CABLE driver signing certificate to the trusted publisher store"

    $pnputilOutput = & pnputil.exe /add-driver $infPath /install 2>&1
    $pnputilOutput | ForEach-Object { Write-HarnessFact "pnputil: $_" }
    if ($LASTEXITCODE -ne 0) {
        throw "pnputil /add-driver /install exited with code $LASTEXITCODE"
    }

    $devconZip = Join-Path $workDir "Scream4.0.zip"
    Write-HarnessFact "downloading Scream (its devcon helper only) from $DevconDownloadUrl"
    Invoke-WebRequest -Uri $DevconDownloadUrl -OutFile $devconZip -UseBasicParsing
    $devconActualSha256 = (Get-FileHash -Algorithm SHA256 -Path $devconZip).Hash
    if ($devconActualSha256 -ne $DevconExpectedSha256) {
        throw "Scream (devcon source) download SHA-256 mismatch: expected $DevconExpectedSha256, got $devconActualSha256"
    }
    Write-HarnessFact "Scream (devcon source) download SHA-256 verified"

    $devconExtractDir = Join-Path $workDir "scream"
    Expand-Archive -Path $devconZip -DestinationPath $devconExtractDir -Force
    $devconPath = Join-Path $devconExtractDir "Install\helpers\devcon-x64.exe"
    if (-not (Test-Path -LiteralPath $devconPath)) {
        throw "devcon helper not found at $devconPath"
    }

    $installOutput = & $devconPath install $infPath "VBAudioVACWDM" 2>&1
    $installOutput | ForEach-Object { Write-HarnessFact "devcon: $_" }
    if ($LASTEXITCODE -ne 0) {
        throw "devcon install exited with code $LASTEXITCODE"
    }

    $mediaDevice = Get-PnpDevice -Class MEDIA -FriendlyName "*VB-Audio*" -ErrorAction SilentlyContinue
    if (-not $mediaDevice) {
        throw "VB-CABLE install completed but no MEDIA-class 'VB-Audio' PnP device is present"
    }
    $soundDevice = Get-CimInstance -ClassName Win32_SoundDevice -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match "VB-Audio" }
    if (-not $soundDevice) {
        throw "VB-CABLE install completed but no 'VB-Audio' Win32_SoundDevice entry is present"
    }

    Write-HarnessFact "VB-CABLE audio driver installed; render endpoint auto-selected as default (confirmed live)"
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
    # RestartCount/RestartInterval: a real gap seen live — an RDP session
    # disconnecting (or the workstation locking behind it) can tear down the
    # interactive session the agent's process lives in, killing it with no
    # trigger left to bring it back on its own (AtLogOn only fires at an
    # actual logon, not a session teardown). This restarts the task itself
    # after such a death without needing a fresh logon.
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
Install-VbCableAudioDriver
Register-VerbatimAgentTask -Username $Username -AgentPath (Join-Path $agentDir "verbatim-agent.exe") -LogPath (Join-Path $agentDir "agent.log")
New-VerbatimAgentFirewallRule -Port $AgentPort

Write-HarnessFact "provisioning complete"
