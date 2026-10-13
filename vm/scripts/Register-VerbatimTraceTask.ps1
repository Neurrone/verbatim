<#
.SYNOPSIS
Registers the scheduled tasks behind cargo xtask trace, which start and
stop a Windows Performance Recorder trace of the CPU's scheduling, for the
communication design's measurements.

.DESCRIPTION
docs/design/communication.md attributes each slow hand-off to its cause
from an ETW trace of context switches and ready threads, and checks that
Verbatim's raised thread priorities never keep its own GUI thread waiting.
WPR records such a trace, but it needs administrator rights, and an
unattended shell cannot answer a UAC prompt. This script, run once from an
elevated prompt, registers two tasks that run WPR as SYSTEM, which needs no
password, and lets the named user start them without elevation:

- "Verbatim trace start" cancels any trace WPR is recording and starts
  WPR's built-in CPU profile, verbose, to a file.
- "Verbatim trace stop" stops the trace into
  C:\ProgramData\Verbatim\Traces\verbatim-<time>.etl, a folder the user may
  read and delete in, and writes the file's name and WPR's exit code to
  last-trace.txt there.

cargo xtask trace start and cargo xtask trace stop start them and check the
result. The tasks' scripts are embedded in the task definitions, which only
administrators can change, rather than kept in files a standard user could
edit. Re-running this script replaces both tasks.

Run from an elevated PowerShell:

    vm\scripts\Register-VerbatimTraceTask.ps1

or, for another account:

    vm\scripts\Register-VerbatimTraceTask.ps1 -User someone
#>
#Requires -RunAsAdministrator
param(
    # The account that may start the tasks and read the traces. Defaults to
    # the caller.
    [string]$User = $env:USERNAME
)

$ErrorActionPreference = 'Stop'

# Must match START_TASK, STOP_TASK and TRACE_DIR in xtask/src/trace.rs.
$startName = 'Verbatim trace start'
$stopName = 'Verbatim trace stop'
$traceDir = 'C:\ProgramData\Verbatim\Traces'

$sid = (New-Object Security.Principal.NTAccount($User)).
    Translate([Security.Principal.SecurityIdentifier]).Value

# The traces' folder: SYSTEM and administrators have full control, the user
# may read, and delete what it has read; files made in it inherit that.
New-Item -ItemType Directory -Force $traceDir | Out-Null
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetAccessRuleProtection($true, $false)
$inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
$none = [Security.AccessControl.PropagationFlags]::None
foreach ($rule in @(
    @('S-1-5-18', 'FullControl'),
    @('S-1-5-32-544', 'FullControl'),
    @($sid, 'ReadAndExecute, Delete')
)) {
    $account = New-Object Security.Principal.SecurityIdentifier($rule[0])
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
        $account, $rule[1], $inherit, $none, 'Allow')))
}
Set-Acl -Path $traceDir -AclObject $acl

$wpr = "`$env:SystemRoot\System32\wpr.exe"
$start = @"
& "$wpr" -cancel 2>`$null | Out-Null
& "$wpr" -start CPU -filemode
exit `$LASTEXITCODE
"@
$stop = @"
`$file = Join-Path '$traceDir' ("verbatim-" + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.etl')
& "$wpr" -stop `$file
`$code = `$LASTEXITCODE
Set-Content -Path (Join-Path '$traceDir' 'last-trace.txt') -Value "`$file`r`n`$code" -Encoding UTF8
exit `$code
"@

function Register-Wpr([string]$name, [string]$script, [string]$description) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $service = New-Object -ComObject Schedule.Service
    $service.Connect()
    $definition = $service.NewTask(0)
    $definition.RegistrationInfo.Description = $description
    $definition.Settings.AllowDemandStart = $true
    $definition.Settings.DisallowStartIfOnBatteries = $false
    $definition.Settings.StopIfGoingOnBatteries = $false
    $definition.Settings.ExecutionTimeLimit = 'PT10M'
    $definition.Settings.MultipleInstances = 2 # ignore a start while one runs
    $exec = $definition.Actions.Create(0)
    $exec.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $exec.Arguments = "-NoProfile -NonInteractive -EncodedCommand $encoded"
    # SYSTEM and administrators get full control; the user may read and
    # run the task, not change it.
    $sddl = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;$sid)"
    $createOrUpdate = 6
    $serviceAccountLogon = 5
    $service.GetFolder('\').RegisterTaskDefinition(
        $name, $definition, $createOrUpdate, 'SYSTEM', $null,
        $serviceAccountLogon, $sddl) | Out-Null
}

Register-Wpr $startName $start "Starts a WPR CPU trace for cargo xtask trace start, for $User."
Register-Wpr $stopName $stop "Stops the WPR trace into $traceDir for cargo xtask trace stop, for $User."
Write-Output "trace: registered '$startName' and '$stopName' for $User ($sid); traces go to $traceDir"
