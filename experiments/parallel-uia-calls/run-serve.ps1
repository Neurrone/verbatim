# Runs the synthetic provider matrix: for each threading mode, the
# throughput bench at three provider delays and the stuck-call arms.
# Usage: run-serve.ps1 -Exe <puia.exe> -ServeExe <copy of puia.exe> -Out <folder>
param([string]$Exe, [string]$ServeExe, [string]$Out)
New-Item -ItemType Directory -Force $Out | Out-Null

function Start-Serve($mode) {
    $log = Join-Path $Out "serve-$mode-windows.txt"
    $p = Start-Process -FilePath $ServeExe -ArgumentList "serve", "layout=0,0,1", "mode=$mode" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and ((Get-Content $log) -match '^window 2'))) { Start-Sleep -Milliseconds 50 }
    Start-Sleep -Milliseconds 200
    $h = @{}
    foreach ($l in Get-Content $log) { if ($l -match '^window (\d) hwnd=(\d+)') { $h[[int]$Matches[1]] = $Matches[2] } }
    return @{ Process = $p; H = $h }
}

foreach ($mode in 'sta', 'agile', 'nocom') {
    $s = Start-Serve $mode
    $w0 = $s.H[0]; $w1 = $s.H[1]; $w2 = $s.H[2]
    $f = Join-Path $Out "serve-$mode-bench.txt"
    foreach ($arm in @(@{ms = 0; lp = 0; n = 'none'}, @{ms = 5; lp = 0; n = 'sleep5'}, @{ms = 5; lp = 1; n = 'pump5'})) {
        & $Exe send hwnd=$w0 app=4 wp=$($arm.ms) lp=$($arm.lp) | Out-Null
        & $Exe bench hwnd=$w0 label=serve-$mode-$($arm.n) ops=name elems=0 k=name:32 reps=15 stats=$w0 *>> $f
    }
    & $Exe send hwnd=$w0 app=4 wp=0 lp=0 | Out-Null
    $f = Join-Path $Out "serve-$mode-hung.txt"
    $probes = @("probe=same-window:${w0}:name", "probe=other-window-same-thread:${w1}:name", "probe=other-ui-thread:${w2}:name", "probe=other-ui-thread-new-connection:${w2}:fromhandle")
    & $Exe hung label=serve-$mode-busy "block=busy:${w0}:3000" "stuck=${w0}:name" @probes elems=0 observe_ms=6000 trials=3 *>> $f
    & $Exe hung label=serve-$mode-slow-sleep "block=slow:${w0}:3000:sleep" "stuck=${w0}:name" @probes elems=0 observe_ms=6000 trials=3 *>> $f
    & $Exe hung label=serve-$mode-slow-pump "block=slow:${w0}:3000:pump" "stuck=${w0}:name" @probes elems=0 observe_ms=6000 trials=3 *>> $f
    Stop-Process -Id $s.Process.Id -Force
    Start-Sleep -Milliseconds 300
}
