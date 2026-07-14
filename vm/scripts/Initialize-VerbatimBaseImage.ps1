[CmdletBinding()]
param(
    [string]$LabRoot = "C:\VerbatimLab",
    [string]$ImageName = $env:VERBATIM_IMAGE_NAME,
    [string]$ProvisioningVersion = $env:VERBATIM_PROVISIONING_VERSION,
    [switch]$SkipAutomationPrep
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ImageName)) {
    $ImageName = "Windows 11 Pro"
}

if ([string]::IsNullOrWhiteSpace($ProvisioningVersion)) {
    $ProvisioningVersion = "phase0-base-v1"
}

if (-not $SkipAutomationPrep) {
    Set-Service -Name WinRM -StartupType Automatic
    Start-Service -Name WinRM
    Set-Item -Path WSMan:\localhost\Service\Auth\Basic -Value $true
    Set-Item -Path WSMan:\localhost\Service\AllowUnencrypted -Value $true
    Enable-NetFirewallRule -DisplayGroup "Windows Remote Management"
}

New-Item -ItemType Directory -Force -Path $LabRoot | Out-Null

$os = $null
try {
    $os = Get-CimInstance -ClassName Win32_OperatingSystem
    $osCaption = $os.Caption
    $osVersion = $os.Version
    $osBuildNumber = $os.BuildNumber
}
catch {
    $environmentOsVersion = [Environment]::OSVersion.Version
    $osCaption = "Windows OS"
    $osVersion = $environmentOsVersion.ToString()
    $osBuildNumber = $environmentOsVersion.Build.ToString()
}

$metadata = [ordered]@{
    image_name = $ImageName
    provisioning_version = $ProvisioningVersion
    provisioned_at_utc = (Get-Date).ToUniversalTime().ToString("o")
    computer_name = $env:COMPUTERNAME
    os_caption = $osCaption
    os_version = $osVersion
    os_build_number = $osBuildNumber
    lab_root = $LabRoot
}

$metadata |
    ConvertTo-Json -Depth 4 |
    Out-File -FilePath (Join-Path $LabRoot "image.json") -Encoding utf8
