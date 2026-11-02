# Threads of one client process against client processes of one thread
# each (xbench), on the synthetic provider in sta and nocom mode and on
# mockapp (debug build), for name reads and a 5 ms provider delay.
param([string]$Out)
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
foreach ($mode in 'sta', 'nocom') {
    $log = "$S\xb-serve-$mode-windows.txt"
    $p = Start-Process -FilePath $puia -ArgumentList 'serve', 'layout=0', "mode=$mode" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and ((Get-Content $log) -match '^ready'))) { Start-Sleep -Milliseconds 50 }
    $h = ((Get-Content $log) -match '^window 0 hwnd=(\d+)')[0] -replace '^window 0 hwnd=(\d+).*', '$1'
    & $puia xbench hwnd=$h op=name elems=0 k=256 reps=15 label=serve-$mode-name *>> $Out
    & $puia send hwnd=$h app=4 wp=5 lp=0 | Out-Null
    & $puia xbench hwnd=$h op=name elems=0 k=32 reps=10 label=serve-$mode-sleep5 *>> $Out
    Stop-Process -Id $p.Id
}
$psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$PSScriptRoot\..\..\crates\mockapp\tests\fixtures\tree.json`" --backend uia --title puia-xb"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
while (($l = $m.StandardOutput.ReadLine()) -notlike 'ready*') { }
$m.StandardInput.WriteLine(''); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String '^top (\d+) .*title=puia-xb' | ForEach-Object { $_.Matches[0].Groups[1].Value }
& $puia xbench hwnd=$h op=name elems=16 k=256 reps=15 label=mockapp-name *>> $Out
# Every provider call 5 ms late; a name read makes three (GetPropertyValue, HostRawElementProvider, FragmentRoot).
$m.StandardInput.WriteLine('slow 5'); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine()
& $puia xbench hwnd=$h op=name elems=16 k=32 reps=10 label=mockapp-slow5 *>> $Out
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() }
