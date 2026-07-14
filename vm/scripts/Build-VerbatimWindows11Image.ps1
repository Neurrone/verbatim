<#
.SYNOPSIS
Builds the Verbatim Windows 11 Hyper-V base image with host-side preflight checks.

.DESCRIPTION
Creates and prepares the Packer temporary and output directories, clears NTFS
compression so Hyper-V can create and import VHDX files, runs Packer validation,
runs the Packer Hyper-V build, clears compression on the exported VM files, and
checks the exported VM configuration with Compare-VM.

Run this script from the repository root for the default local build flow.

.PARAMETER RepositoryRoot
Repository root used to resolve relative paths. Defaults to the current git
repository root.

.PARAMETER PackerDirectory
Path to the Packer template directory. Defaults to vm.

.PARAMETER VarFile
Path to the local Packer variable file. Defaults to
vm/local.pkrvars.hcl.

.PARAMETER TempPath
Temporary Hyper-V build directory. If omitted, the script reads temp_path from
the var file and then falls back to artifacts/packer/tmp.

.PARAMETER OutputDirectory
Final Packer output directory. If omitted, the script reads output_directory
from the var file and then falls back to artifacts/packer/windows11.

.PARAMETER PackerExecutable
Packer executable name or path. Defaults to packer.

.PARAMETER OscdimgPath
Optional path to oscdimg.exe. If omitted, the wrapper searches PATH and standard
Windows ADK Deployment Tools locations.

.PARAMETER Force
Passes -force to packer build, allowing Packer to overwrite an existing output
directory for the same image build.

.PARAMETER SkipValidate
Skips packer validate.

.PARAMETER SkipBuild
Runs host preflight and validation without starting the VM build.

.PARAMETER SkipCompareVm
Skips the post-build Hyper-V import compatibility check.

.EXAMPLE
.\vm\scripts\Build-VerbatimWindows11Image.ps1

Runs preflight checks, validates the Packer template, builds the image, and runs
Compare-VM on the exported VM.

.EXAMPLE
.\vm\scripts\Build-VerbatimWindows11Image.ps1 -Force

Rebuilds the image and allows Packer to replace an existing output directory.

.EXAMPLE
.\vm\scripts\Build-VerbatimWindows11Image.ps1 -SkipBuild

Creates/prepares the temporary and output directories and runs packer validate
without starting Windows setup.

.EXAMPLE
Get-Help .\vm\scripts\Build-VerbatimWindows11Image.ps1 -Examples

Shows usage examples for this wrapper.
#>
[CmdletBinding()]
param(
    [string]$RepositoryRoot,
    [string]$PackerDirectory = "vm",
    [string]$VarFile = "vm/local.pkrvars.hcl",
    [string]$TempPath,
    [string]$OutputDirectory,
    [string]$PackerExecutable = "packer",
    [string]$OscdimgPath,
    [switch]$Force,
    [switch]$SkipValidate,
    [switch]$SkipBuild,
    [switch]$SkipCompareVm
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-DefaultRepositoryRoot {
    $gitRoot = & git -C $PSScriptRoot rev-parse --show-toplevel 2>$null
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($gitRoot)) {
        return $gitRoot.Trim()
    }

    return (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path
}

function Resolve-PathForBuild {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Root
    )

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path $Root $Path))
}

function Get-HclStringValue {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }

    $content = Get-Content -Raw -LiteralPath $Path
    $pattern = "(?m)^\s*" + [regex]::Escape($Name) + "\s*=\s*`"((?:\\.|[^`"\\])*)`"\s*$"
    $match = [regex]::Match($content, $pattern)
    if (-not $match.Success) {
        return $null
    }

    return $match.Groups[1].Value.Replace('\"', '"').Replace('\\', '\')
}

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    Write-Host ("Running: {0} {1}" -f $FilePath, ($Arguments -join " "))
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE."
    }
}

function Find-OscdimgExecutable {
    param(
        [string]$Path
    )

    if (-not [string]::IsNullOrWhiteSpace($Path)) {
        $resolvedPath = Resolve-Path -LiteralPath $Path -ErrorAction Stop
        return $resolvedPath.Path
    }

    $fromPath = Get-Command oscdimg.exe -ErrorAction SilentlyContinue
    if ($null -ne $fromPath) {
        return $fromPath.Source
    }

    $candidateRoots = @()
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidateRoots += ${env:ProgramFiles(x86)}
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $candidateRoots += $env:ProgramFiles
    }

    $candidateRelativePaths = @(
        "Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\amd64\Oscdimg\oscdimg.exe",
        "Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\x86\Oscdimg\oscdimg.exe",
        "Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\arm64\Oscdimg\oscdimg.exe",
        "Windows Kits\11\Assessment and Deployment Kit\Deployment Tools\amd64\Oscdimg\oscdimg.exe",
        "Windows Kits\11\Assessment and Deployment Kit\Deployment Tools\x86\Oscdimg\oscdimg.exe",
        "Windows Kits\11\Assessment and Deployment Kit\Deployment Tools\arm64\Oscdimg\oscdimg.exe"
    )

    foreach ($root in $candidateRoots) {
        foreach ($relativePath in $candidateRelativePaths) {
            $candidate = Join-Path $root $relativePath
            if (Test-Path -LiteralPath $candidate) {
                return (Resolve-Path -LiteralPath $candidate).Path
            }
        }
    }

    throw "Could not find oscdimg.exe. Install the Windows ADK Deployment Tools, add oscdimg.exe to PATH, or pass -OscdimgPath."
}

function Add-DirectoryToProcessPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory
    )

    $pathEntries = @($env:PATH -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $alreadyPresent = $pathEntries | Where-Object { $_.TrimEnd('\') -ieq $Directory.TrimEnd('\') } | Select-Object -First 1
    if ($null -eq $alreadyPresent) {
        $env:PATH = "$Directory;$env:PATH"
    }
}

function Clear-NtfsCompression {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory
    )

    New-Item -ItemType Directory -Force -Path $Directory | Out-Null

    $parent = Split-Path -Parent $Directory
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
        Invoke-CheckedCommand -FilePath "compact.exe" -Arguments @("/u", "/i", "/f", $parent)
    }

    Invoke-CheckedCommand -FilePath "compact.exe" -Arguments @("/u", "/i", "/f", $Directory)

    $attributes = (Get-Item -LiteralPath $Directory).Attributes
    $isCompressed = (($attributes -band [System.IO.FileAttributes]::Compressed) -eq [System.IO.FileAttributes]::Compressed)
    if ($isCompressed) {
        throw "Directory remains NTFS-compressed after preflight: $Directory"
    }
}

function Clear-NtfsCompressionRecursive {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory
    )

    if (Test-Path -LiteralPath $Directory) {
        Invoke-CheckedCommand -FilePath "compact.exe" -Arguments @("/u", "/i", "/f", "/s:$Directory")
    }
}

function Test-ExportedVmCompatibility {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory
    )

    $virtualMachinesPath = Join-Path $Directory "Virtual Machines"
    $vmcx = Get-ChildItem -LiteralPath $virtualMachinesPath -Filter "*.vmcx" -ErrorAction Stop | Select-Object -First 1
    if ($null -eq $vmcx) {
        throw "No exported Hyper-V .vmcx file found under $virtualMachinesPath."
    }

    $report = Compare-VM -Path $vmcx.FullName
    $incompatibilities = @($report.Incompatibilities)
    if ($incompatibilities.Count -gt 0) {
        $incompatibilities | Select-Object MessageId, Message, Source | Format-List
        throw "Exported VM has $($incompatibilities.Count) Hyper-V import incompatibility item(s)."
    }

    Write-Host "Compare-VM: no incompatibilities"
}

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Get-DefaultRepositoryRoot
}

$RepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$varFilePath = Resolve-PathForBuild -Path $VarFile -Root $RepositoryRoot
$packerDirectoryPath = Resolve-PathForBuild -Path $PackerDirectory -Root $RepositoryRoot

if (-not (Test-Path -LiteralPath $packerDirectoryPath)) {
    throw "Packer directory does not exist: $packerDirectoryPath"
}

if (-not (Test-Path -LiteralPath $varFilePath)) {
    throw "Packer var file does not exist: $varFilePath"
}

if ([string]::IsNullOrWhiteSpace($TempPath)) {
    $TempPath = Get-HclStringValue -Path $varFilePath -Name "temp_path"
}

if ([string]::IsNullOrWhiteSpace($TempPath)) {
    $TempPath = "artifacts/packer/tmp"
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Get-HclStringValue -Path $varFilePath -Name "output_directory"
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = "artifacts/packer/windows11"
}

$resolvedTempPath = Resolve-PathForBuild -Path $TempPath -Root $RepositoryRoot
$resolvedOutputDirectory = Resolve-PathForBuild -Path $OutputDirectory -Root $RepositoryRoot

Write-Host "Repository root: $RepositoryRoot"
Write-Host "Packer directory: $packerDirectoryPath"
Write-Host "Var file: $varFilePath"
Write-Host "Temporary build path: $resolvedTempPath"
Write-Host "Output directory: $resolvedOutputDirectory"

$resolvedOscdimgPath = Find-OscdimgExecutable -Path $OscdimgPath
Add-DirectoryToProcessPath -Directory (Split-Path -Parent $resolvedOscdimgPath)
Write-Host "Using oscdimg.exe: $resolvedOscdimgPath"

Clear-NtfsCompression -Directory $resolvedTempPath
Clear-NtfsCompression -Directory $resolvedOutputDirectory

Push-Location $RepositoryRoot
try {
    if (-not $SkipValidate) {
        Invoke-CheckedCommand -FilePath $PackerExecutable -Arguments @(
            "validate",
            "-var-file", $varFilePath,
            $packerDirectoryPath
        )
    }

    if (-not $SkipBuild) {
        $buildArguments = New-Object System.Collections.Generic.List[string]
        $buildArguments.Add("build")
        if ($Force) {
            $buildArguments.Add("-force")
        }
        $buildArguments.Add("-var-file")
        $buildArguments.Add($varFilePath)
        $buildArguments.Add($packerDirectoryPath)

        Invoke-CheckedCommand -FilePath $PackerExecutable -Arguments $buildArguments.ToArray()
        Clear-NtfsCompressionRecursive -Directory $resolvedOutputDirectory

        if (-not $SkipCompareVm) {
            Test-ExportedVmCompatibility -Directory $resolvedOutputDirectory
        }
    }
}
finally {
    Pop-Location
}
