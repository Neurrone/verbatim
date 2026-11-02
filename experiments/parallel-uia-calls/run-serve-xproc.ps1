# The stuck-call arms again, with the same probes also made from client
# processes of their own, to tell a queue in the client process from one in
# the application. Usage as run-serve.ps1.
param([string]$Exe, [string]$ServeExe, [string]$Out, [string[]]$Modes = @('sta', 'nocom'))
New-Item -ItemType Directory -Force $Out | Out-Null
foreach ($mode in $Modes) {
    $log = Join-Path $Out "serve-$mode-xproc-windows.txt"
    $p = Start-Process -FilePath $ServeExe -ArgumentList "serve", "layout=0,0,1", "mode=$mode" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and ((Get-Content $log) -match '^window 2'))) { Start-Sleep -Milliseconds 50 }
    Start-Sleep -Milliseconds 200
    $h = @{}
    foreach ($l in Get-Content $log) { if ($l -match '^window (\d) hwnd=(\d+)') { $h[[int]$Matches[1]] = $Matches[2] } }
    $w0 = $h[0]; $w1 = $h[1]; $w2 = $h[2]
    $f = Join-Path $Out "serve-$mode-xproc.txt"
    $probes = @("probe=other-window-same-thread:${w1}:name", "probe=other-ui-thread:${w2}:name", "probe=other-ui-thread-new-connection:${w2}:fromhandle",
        "xprobe=same-window:${w0}:name", "xprobe=other-window-same-thread:${w1}:name", "xprobe=other-ui-thread:${w2}:name", "xprobe=other-ui-thread-new-connection:${w2}:fromhandle")
    & $Exe hung label=serve-$mode-busy-x "block=busy:${w0}:3000" "stuck=${w0}:name" @probes elems=0 observe_ms=6000 trials=3 *>> $f
    & $Exe hung label=serve-$mode-slow-sleep-x "block=slow:${w0}:3000:sleep" "stuck=${w0}:name" @probes elems=0 observe_ms=6000 trials=3 *>> $f
    Stop-Process -Id $p.Id -Force
    Start-Sleep -Milliseconds 300
}
