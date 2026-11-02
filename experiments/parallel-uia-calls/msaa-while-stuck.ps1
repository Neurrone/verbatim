# While a UIA name read sleeps on the synthetic provider's UIA I/O thread (nocom), or
# its UI thread is busy (sta), does an MSAA call into the same process answer?
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
$out = "$S\q4-msaa-while-io-thread-stuck.txt"
Remove-Item $out -ErrorAction SilentlyContinue
foreach ($arm in @('nocom:window:slow', 'nocom:bridge:slow', 'sta:bridge:slow', 'sta:bridge:busy')) {
    $mode, $msaa, $block = $arm.Split(':')
    $log = "$S\q4-serve-$mode-$msaa.txt"
    $sv = Start-Process $puia -ArgumentList 'serve', "mode=$mode", 'layout=0,0,1', "msaa=$msaa" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and @(Select-String -Path $log -Pattern '^window').Count -eq 3 -and (Select-String -Path $log -Pattern '^ready' -Quiet))) { Start-Sleep -Milliseconds 50 }
    $h = @{}; Select-String -Path $log -Pattern '^window (\d) hwnd=(\d+)' | ForEach-Object { $h[$_.Matches[0].Groups[1].Value] = $_.Matches[0].Groups[2].Value }
    Add-Content $out "ARM mode=$mode msaa=$msaa block=$block"
    Get-Content $log | Add-Content $out
    $b = if ($block -eq 'slow') { "block=slow:$($h['0']):4000:sleep" } else { "block=busy:$($h['0']):4000" }
    $stuck = if ($block -eq "slow") { "stuck=$($h["0"]):name:0" } else { "nostuck=1" }
    & $puia hung label="$mode-$msaa-$block" $b $stuck "probe=same-thread-window-msaa:$($h['1']):msaanew" "probe=other-thread-window-msaa:$($h['2']):msaanew" "probe=same-thread-window-uia-name:$($h['1']):name:0" "probe=other-thread-window-uia-name:$($h['2']):name:0" "xprobe=xproc-same-thread-window-msaa:$($h['1']):msaanew" elems=0 observe_ms=4000 trials=2 *>> $out
    Stop-Process -Id $sv.Id
}
Select-String -Path $out -Pattern '^(ARM|TRIAL|RESULT)' | ForEach-Object { $_.Line }
