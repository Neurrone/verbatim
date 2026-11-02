# Later calls beside a stuck one, all on the stuck call's client thread
# (mode=same) or each on a new thread (mode=new), relative to the release
# and to UIA's timeouts (puia abandon). The arms: the synthetic provider's
# STA with its UI thread busy 40 s, a provider that serves off the UI thread
# (nocom) with its UI thread busy and with its provider call itself slow,
# mockapp's hold released at 40 s, and a stuck call that is a pending
# connection (ElementFromHandle of the busy window), with later calls into
# the busy thread and, in sta-pending-connection-only, with none.
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
param([string]$Out, [string[]]$Arms = @('sta-busy', 'nocom-busy', 'nocom-slow', 'sta-pending-connection', 'sta-pending-connection-only', 'mockapp-hold'), [int]$Trials = 2)
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
Copy-Item $puia "$S\puia-serve.exe" -Force
foreach ($arm in $Arms) {
    $mode = if ($arm -like 'nocom*') { 'nocom' } else { 'sta' }
    $sv = $null
    if ($arm -ne 'mockapp-hold') {
        $log = "$S\abandon-serve-$arm.txt"
        $sv = Start-Process "$S\puia-serve.exe" -ArgumentList 'serve', "mode=$mode", 'layout=0,0,1', "log=$S\abandon-provlog.txt" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
        while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^ready' -Quiet) -and @(Select-String -Path $log -Pattern '^window').Count -eq 3)) { Start-Sleep -Milliseconds 50 }
        $h = @{}; Select-String -Path $log -Pattern '^window (\d) hwnd=(\d+)' | ForEach-Object { $h[$_.Matches[0].Groups[1].Value] = $_.Matches[0].Groups[2].Value }
        $w0 = $h['0']; $w1 = $h['1']; $w2 = $h['2']
        Get-Content $log | Add-Content $Out
        $calls = @("call=1000:same-window:${w0}:name:0", "call=3000:other-window-same-ui-thread:${w1}:name:0", "call=5000:other-ui-thread:${w2}:name:0",
            "call=8000:other-ui-thread-new-connection:${w2}:fromhandle", "call=25000:after-timeout-same-window:${w0}:name:0",
            "call=26000:after-timeout-other-ui-thread:${w2}:name:0", "call=41000:after-release-other-ui-thread:${w2}:name:0")
        $stuck = if ($arm -like 'sta-pending-connection*') { "stuck=${w0}:fromhandle" } else { "stuck=${w0}:name:0" }
        $block = if ($arm -eq 'nocom-slow') { "block=slow:${w0}:40000" } else { "block=busy:${w0}:40000" }
        if ($arm -eq 'sta-pending-connection-only') {
            # No later call goes to the busy thread: only the pending connection is there.
            $block = "block=busy:${w0}:12000"
            $calls = @("call=500:other-ui-thread:${w2}:name:0", "call=1000:other-ui-thread-rop:${w2}:rop:0", "call=1500:other-ui-thread-new-connection:${w2}:fromhandle",
                "call=3000:after-connection-timeout-other-ui-thread:${w2}:name:0", "call=13000:after-release-other-ui-thread:${w2}:name:0")
        }
        $extra = @()
    } else {
        $calls = @('call=1000:same-window-other-element:main:name:3', 'call=3000:same-window-rop:main:rop:3', 'call=5000:same-window-new-connection:main:fromhandle',
            'call=25000:after-timeout-other-element:main:name:3', 'call=41000:after-release-other-element:main:name:3')
        $stuck = 'stuck=main:name:1'
        $block = 'block=mock-hold:40000'
        $extra = @("mockapp=$S\mtarget\debug\mockapp.exe", "fixture=$PSScriptRoot\..\..\crates\mockapp\tests\fixtures\tree.json", 'title=puia-abandon')
    }
    foreach ($m in 'same', 'new') {
        $elems = if ($arm -eq 'mockapp-hold') { 16 } else { 0 }
        & $puia abandon label=$arm-$m mode=$m $block $stuck @calls @extra elems=$elems trials=$Trials tail_ms=3000 *>> $Out
    }
    if ($sv) { Stop-Process -Id $sv.Id }
}
