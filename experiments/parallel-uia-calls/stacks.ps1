# Takes every thread's stack in an application while calls into it wait:
# runs a hung arm in the background, waits for its PROBES_STARTED line, then
# attaches cdb non-invasively (-pv), prints ~*k and detaches (qd).
# A non-invasive attach suspends every thread of the target until cdb
# detaches, and the first run spends minutes downloading symbols, so the
# target is frozen that long: never aim it at the shell's explorer.exe
# (on 2026-11-02 it froze the taskbar for about eight minutes), and fill the
# symbol cache first on a throwaway process.
# Usage: stacks.ps1 -Exe <puia.exe> -Cdb <cdb.exe> -Symbols <cache folder> -Out <file> -TargetPid <pid> -DelayMs <ms> -HungArgs <args...>
param([string]$Exe, [string]$Cdb, [string]$Symbols, [string]$Out, [int]$TargetPid, [int]$DelayMs = 1000, [string[]]$HungArgs)
$log = "$Out.hung.txt"
$p = Start-Process -FilePath $Exe -ArgumentList (@('hung') + $HungArgs) -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern 'PROBES_STARTED' -Quiet))) { Start-Sleep -Milliseconds 20 }
Start-Sleep -Milliseconds $DelayMs
& $Cdb -pv -p $TargetPid -y "srv*$Symbols*https://msdl.microsoft.com/download/symbols" -c '~*k 40; qd' *> $Out
$p.WaitForExit()
