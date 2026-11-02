# The AccessKit provider (akapp, two windows on two UI threads): which
# thread runs its provider code, throughput, a slow action beside cheap
# calls, and a busy UI thread or a slow action with probes beside it.
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and akapp\target\release\akapp.exe; logs go there.
param([string]$Out)
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$log = "$S\akapp-windows.txt"
$ak = Start-Process "$S\akapp\target\release\akapp.exe" -ArgumentList 'threads=2' -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^ready' -Quiet))) { Start-Sleep -Milliseconds 50 }
$h = @{}; Select-String -Path $log -Pattern '^window (\d) hwnd=(\d+)' | ForEach-Object { $h[$_.Matches[0].Groups[1].Value] = $_.Matches[0].Groups[2].Value }
$w0 = $h['0']; $w1 = $h['1']
Remove-Item $Out -ErrorAction SilentlyContinue
Get-Content $log | Add-Content $Out
& $puia elems hwnd=$w0 elems=16 *>> $Out
# Which thread runs AccessKit's code: its property read, its invoke, and the app's action handler.
$cmd = @'
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; ~.; gc"
bm akapp!*PlatformNode*GetPropertyValue* ".echo AK_GETPROPERTYVALUE; ~.; k 25; gc"
bm akapp!*PlatformNode*Invoke* ".echo AK_INVOKE; ~.; gc"
bm akapp!*akapp*Handler*do_action* ".echo APP_DO_ACTION; ~.; k 30; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\ak-thread.cdb" $cmd
foreach ($op in 'name', 'invoke') {
    $p = Start-Probe $puia "hwnd=$w0 op=$op elem=0 elems=16"
    $dl = "$S\ak-thread-$op.log"; Remove-Item $dl -ErrorAction SilentlyContinue
    $c = Start-Cdb $ak.Id "$S\ak-thread.cdb" $dl "$S\akapp\target\release"
    $r = Invoke-Probe $p
    Start-Sleep -Milliseconds 300
    Stop-Cdb $c
    Stop-Probe $p
    Add-Content $Out "SERVING op=$op result=$r"
    Get-Content $dl | Where-Object { $_ -match '^(SRV_|AK_|APP_)|Id: |^[0-9a-f`]+ [0-9a-f`]+ +\S+!' } | Add-Content $Out
}
& $puia threads pid=$($ak.Id) *>> $Out
# Throughput, with the action counters read from window 0.
& $puia bench hwnd=$w0 label=akapp ops=name,invoke,fromhandle,rop k=name:64,invoke:64,fromhandle:32,rop:32 reps=20 elems=16 stats=$w0 *>> $Out
# A slow action (button 1, 200 ms) beside cheap calls into the same window and the other UI thread's window.
& $puia send hwnd=$w0 app=4 wp=200 lp=0 | Out-Null
foreach ($b in @(@($w0, 'name', 3), @($w0, 'invoke', 3), @($w1, 'name', 3))) {
    foreach ($xb in 0, 1) {
        & $puia pair label=akapp-slowinvoke-beside-$($b[1])-$(if ($b[0] -eq $w0) { 'same-window' } else { 'other-ui-thread' })-xb$xb hwnd_a=$w0 op_a=invoke elem_a=0 hwnd_b=$($b[0]) op_b=$($b[1]) elem_b=$($b[2]) iters_a=10 xb=$xb *>> $Out
    }
}
& $puia send hwnd=$w0 app=4 wp=0 lp=0 | Out-Null
# A busy UI thread (no call queued on it), and a slow action, each for 4 s, with probes.
$probes = @("probe=same-window-name:${w0}:name:3", "probe=same-window-invoke:${w0}:invoke:3", "probe=same-window-rop:${w0}:rop:3", "probe=same-window-new-connection:${w0}:fromhandle",
    "probe=other-ui-thread-name:${w1}:name:3", "probe=other-ui-thread-new-connection:${w1}:fromhandle", "xprobe=xproc-same-window-name:${w0}:name:3")
& $puia hung label=akapp-busy-ui-thread "block=busy:${w0}:4000" @probes observe_ms=6000 trials=2 *>> $Out
& $puia hung label=akapp-slow-action "block=slow:${w0}:4000:sleep" "stuck=${w0}:invoke:0" @probes observe_ms=6000 trials=2 *>> $Out
# Every stack while an Invoke into the window waits on its busy UI thread.
& "$PSScriptRoot\stacks.ps1" -Exe $puia -Cdb cdbX64.exe -Symbols "$PSScriptRoot\..\..\target\symbols" -Out "$S\akapp-stacks-busy-invoke.txt" -TargetPid $ak.Id -DelayMs 1000 -HungArgs @('label=akapp-busy-invoke', "block=busy:${w0}:4000", "probe=same-window-invoke:${w0}:invoke:3", 'observe_ms=6000', 'trials=1')
Get-Content "$S\akapp-stacks-busy-invoke.txt.hung.txt" | Add-Content $Out
Stop-Process -Id $ak.Id
