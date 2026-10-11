<#
.SYNOPSIS
Configures Windows Error Reporting to keep a crash dump of each of
Verbatim's processes that crashes, in the folder the end-to-end suite
collects them from.

.DESCRIPTION
The end-to-end suite fails a scenario when any of Verbatim's processes
(verbatim.exe, verbatim-outpost.exe, verbatim-synth-host.exe) exits
unexpectedly. With this configured, Windows also writes a dump of the
process as it crashes into C:\ProgramData\Verbatim\CrashDumps, and the
suite copies every dump written during a scenario into that scenario's
artifacts and reports it. Without it the suite still fails the scenario;
there is just no dump to read.

The setting is Windows Error Reporting's LocalDumps key, under
HKEY_LOCAL_MACHINE, so it needs administrator rights, once. For verbatim.exe
and verbatim-synth-host.exe it keeps a minidump (DumpType 1), with the call
stacks and the memory they reference, and at most 20 dumps for each program,
so the folder cannot fill the disk.

For verbatim-outpost.exe it keeps a full-memory dump: a custom dump
(DumpType 0) whose CustomDumpFlags are MiniDumpWithFullMemory,
MiniDumpWithHandleData, MiniDumpWithUnloadedModules,
MiniDumpWithFullMemoryInfo and MiniDumpWithThreadInfo (0x1826). An outpost
has crashed inside UI Automation's own event delivery, and which event and
handler were involved is only on the heap, which a minidump leaves out. An
outpost's full dump is expected to be about 100 to 300 megabytes, so at
most 10 are kept. Windows Error Reporting writes the dump while the crashed
process is held, so a dump is complete by the time the suite sees the
process exit; in-process code could not take one for every crash, since a
fail-fast ends the process without running any of its handlers.
The folder is created with read access for the machine's users, so the
suite's agent, running as the signed-in user, can read the dumps.
It also keeps dumps of the terminals the suite drives (WindowsTerminal.exe,
OpenConsole.exe and conhost.exe), so that a crash inside an application
Verbatim reads can be traced to the call that caused it. These are full
dumps (DumpType 2), since a crash inside UI Automation's own code needs its
internal state to diagnose, and at most 5 for each program, as each can be
several hundred megabytes.

Re-running this script sets the same values again.

Run once from an elevated PowerShell:

    vm\scripts\Enable-VerbatimCrashDumps.ps1

To undo it, delete the keys under
HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps named
after the programs listed above.
#>
#Requires -RunAsAdministrator

$ErrorActionPreference = 'Stop'

$folder = 'C:\ProgramData\Verbatim\CrashDumps'
$images = 'verbatim.exe', 'verbatim-synth-host.exe'
$outpost = 'verbatim-outpost.exe'
# MiniDumpWithFullMemory (0x2), MiniDumpWithHandleData (0x4),
# MiniDumpWithUnloadedModules (0x20), MiniDumpWithFullMemoryInfo (0x800)
# and MiniDumpWithThreadInfo (0x1000).
$outpostDumpFlags = 0x1826
$terminals = 'WindowsTerminal.exe', 'OpenConsole.exe', 'conhost.exe'
$root = 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps'

New-Item -ItemType Directory -Force -Path $folder | Out-Null
$acl = Get-Acl -Path $folder
$users = New-Object System.Security.Principal.SecurityIdentifier('S-1-5-32-545')
$rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
    $users, 'ReadAndExecute', 'ContainerInherit, ObjectInherit', 'None', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -Path $folder -AclObject $acl

if (-not (Test-Path -Path $root)) {
    New-Item -Path $root -Force | Out-Null
}
function Set-LocalDumps($image, $type, $count, $flags) {
    $key = Join-Path $root $image
    New-Item -Path $key -Force | Out-Null
    New-ItemProperty -Path $key -Name DumpFolder -PropertyType ExpandString -Value $folder -Force | Out-Null
    New-ItemProperty -Path $key -Name DumpType -PropertyType DWord -Value $type -Force | Out-Null
    New-ItemProperty -Path $key -Name DumpCount -PropertyType DWord -Value $count -Force | Out-Null
    if ($null -ne $flags) {
        New-ItemProperty -Path $key -Name CustomDumpFlags -PropertyType DWord -Value $flags -Force | Out-Null
    } else {
        Remove-ItemProperty -Path $key -Name CustomDumpFlags -ErrorAction SilentlyContinue
    }
    Write-Output "Crash dumps of $image go to $folder"
}
foreach ($image in $images) {
    Set-LocalDumps $image 1 20 $null
}
Set-LocalDumps $outpost 0 10 $outpostDumpFlags
foreach ($image in $terminals) {
    Set-LocalDumps $image 2 5
}
