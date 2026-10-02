<#
.SYNOPSIS
Registers the scheduled task behind cargo xtask park, which moves a user's
Remote Desktop session onto the machine's console so it stays unlocked and
interactive with no RDP client connected.

.DESCRIPTION
For a development machine reached over RDP that also runs the end-to-end
suite locally, such as a Proxmox or Hyper-V VM. When an RDP client
disconnects, Windows locks the session behind it, and a locked session
swallows injected input, so the suite cannot run. Running
"tscon <session> /dest:console" instead moves the session onto the
machine's own display, still signed in and unlocked.

tscon needs administrator rights, and an unattended shell cannot answer a
UAC prompt. This script, run once from an elevated prompt, registers a task
that runs tscon as SYSTEM, which needs no password, and lets the named user
start that task without elevation. cargo xtask park starts it and checks the
result.

The task's script is embedded in the task definition, which only
administrators can change, rather than kept in a file a standard user could
edit. Re-running this script replaces the task.

Run from an elevated PowerShell:

    vm\scripts\Register-VerbatimParkTask.ps1

or, for another account:

    vm\scripts\Register-VerbatimParkTask.ps1 -User someone
#>
#Requires -RunAsAdministrator
param(
    # The account whose session the task parks. Defaults to the caller.
    [string]$User = $env:USERNAME
)

$ErrorActionPreference = 'Stop'

# Must match TASK_NAME in xtask/src/park.rs.
$taskName = 'Verbatim park session'

$sid = (New-Object Security.Principal.NTAccount($User)).
    Translate([Security.Principal.SecurityIdentifier]).Value

# Runs as SYSTEM: finds the user's session in query.exe's output and moves it
# to the console. A row reads "<session name> <user> <id> <state>"; the
# session name is blank for a disconnected session, and the caller's own row
# starts with ">" rather than a space.
$action = @"
`$rows = & "`$env:SystemRoot\System32\query.exe" session
foreach (`$row in `$rows) {
    if (`$row -match '^[ >](?<name>\S*)\s+(?<user>\S+)\s+(?<id>\d+)\s+\S+') {
        if (`$Matches.user -eq '$User' -and `$Matches.name -ne 'console') {
            & "`$env:SystemRoot\System32\tscon.exe" `$Matches.id /dest:console
            exit `$LASTEXITCODE
        }
    }
}
exit 0
"@
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($action))

$service = New-Object -ComObject Schedule.Service
$service.Connect()
$definition = $service.NewTask(0)
$definition.RegistrationInfo.Description =
    "Moves $User's Remote Desktop session to the console for cargo xtask park."
$definition.Settings.AllowDemandStart = $true
$definition.Settings.DisallowStartIfOnBatteries = $false
$definition.Settings.StopIfGoingOnBatteries = $false
$definition.Settings.ExecutionTimeLimit = 'PT1M'
$definition.Settings.MultipleInstances = 2 # ignore a start while one runs
$exec = $definition.Actions.Create(0)
$exec.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$exec.Arguments = "-NoProfile -NonInteractive -EncodedCommand $encoded"

# SYSTEM and administrators get full control; the user may read and run the
# task, not change it.
$sddl = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;$sid)"
$createOrUpdate = 6
$serviceAccountLogon = 5
$service.GetFolder('\').RegisterTaskDefinition(
    $taskName, $definition, $createOrUpdate, 'SYSTEM', $null,
    $serviceAccountLogon, $sddl) | Out-Null

Write-Output "park: registered task '$taskName' for $User ($sid)"
