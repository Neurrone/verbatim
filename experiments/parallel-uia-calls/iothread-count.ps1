# What creates UIA's I/O thread in a provider process, and how many there
# are: starts the target fresh, attaches cdb before any client connects,
# with breakpoints on the I/O manager's start and on thread creation, then
# connects one client, then 8 client threads and 3 client processes, and
# lists the target's threads by start address (puia threads) after each.
# -Target mockapp: mockapp's UIA backend, one UI thread.
# -Target serve: the synthetic provider with 8 windows on 2 UI threads.
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
param([ValidateSet('mockapp', 'serve')][string]$Target = 'mockapp', [string]$Mode = 'sta')
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
if ($Target -eq 'mockapp') {
    $psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$PSScriptRoot\..\..\crates\mockapp\tests\fixtures\tree.json`" --backend uia --title puia-q2io"
    $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
    $t = [System.Diagnostics.Process]::Start($psi)
    $null = $t.StandardOutput.ReadLine()
    $hw = @(& $puia windows pid=$($t.Id) | Select-String '^top (\d+) .*title=puia-q2io' | ForEach-Object { $_.Matches[0].Groups[1].Value })
} else {
    Copy-Item $puia "$S\puia-serve.exe" -Force
    $log = "$S\q2io-serve.txt"
    $t = Start-Process "$S\puia-serve.exe" -ArgumentList 'serve', "mode=$Mode", 'layout=0,0,0,0,1,1,1,1', "log=$S\q2io-provlog.txt" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^ready' -Quiet) -and @(Select-String -Path $log -Pattern '^window').Count -eq 8)) { Start-Sleep -Milliseconds 50 }
    $hw = @(Select-String -Path $log -Pattern '^window \d hwnd=(\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value })
    Get-Content $log
}
$tp = $t.Id
"TARGET $Target pid=$tp windows=$($hw -join ',')"
function Show-Threads($when) {
    $all = @(& $puia threads pid=$tp)
    $uia = @($all | Where-Object { $_ -match 'start=uiautomationcore' })
    "THREADS $when total=$($all.Count) uiautomationcore=$($uia.Count)"
    $uia | ForEach-Object { "  $_" }
}
Show-Threads 'before any client'
$cmd = @'
bu UIAutomationCore!OverlappedIOManager::Init ".echo IOM_INIT; .echotime; ~.; k 40; gc"
bu UIAutomationCore!OverlappedIOManager::StaticIoThreadProc ".echo IO_THREAD_START; .echotime; ~.; gc"
bp KERNELBASE!CreateRemoteThreadEx ".echo CREATE_THREAD; .echotime; ~.; k 40; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\q2io.cdb" $cmd
$dl = "$S\q2io-$Target.log"
Remove-Item $dl -ErrorAction SilentlyContinue
$c = Start-Cdb $tp "$S\q2io.cdb" $dl "$S\mtarget\debug;$S"
Show-Threads 'after attach, before any client'
$p = Start-Probe $puia "hwnd=$($hw[0]) op=name elems=16"
Show-Threads 'after one client process connected'
$hl = ($hw | ForEach-Object { $_ }) -join ','
& $puia bench hwnd=$($hw[0]) hwnds=$hl label=q2io ops=name k=name:64 threads=8 reps=3 elems=0 | Select-String '^RESULT'
Show-Threads 'after 8 client threads, one window each'
$ps = @(0, 1, 2) | ForEach-Object { Start-Probe $puia "hwnd=$($hw[($hw.Count - 1 - $_ + 3 * $hw.Count) % $hw.Count]) op=name elems=0" }
$ps | ForEach-Object { "probe: $(Invoke-Probe $_)" }
Show-Threads 'after 3 more client processes'
Stop-Cdb $c
$ps | ForEach-Object { Stop-Probe $_ }
Stop-Probe $p
"DEBUGGER IOM_INIT=$(@(Select-String -Path $dl -Pattern '^IOM_INIT$').Count) IO_THREAD_START=$(@(Select-String -Path $dl -Pattern '^IO_THREAD_START$').Count) CREATE_THREAD=$(@(Select-String -Path $dl -Pattern '^CREATE_THREAD$').Count)"
if ($Target -eq 'mockapp') { $t.StandardInput.WriteLine('quit'); if (-not $t.WaitForExit(5000)) { $t.Kill() } } else { Stop-Process -Id $tp }
