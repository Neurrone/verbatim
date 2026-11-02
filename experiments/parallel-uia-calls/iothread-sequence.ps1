# The order in which UIA's I/O thread in the synthetic provider takes
# requests: a name read into window 0 sleeps 1 second in the provider,
# and 50 and 100 ms after it starts two more name reads, into window 1 (the
# same UI thread) and window 2 (the other UI thread), start on threads of
# their own (puia abandon, mode=new). cdb (invasive) marks each request
# the server's I/O thread takes (ChannelBasedServerConnection::OnData), each
# provider entry, each cross-apartment call (ComInvoker::CallTarget) and
# each response written, with the thread and the debugger's clock.
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe; logs go there.
param([string]$Mode = 'nocom')
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
Copy-Item $puia "$S\puia-serve.exe" -Force
$log = "$S\q2seq-serve-$Mode.txt"
$t = Start-Process "$S\puia-serve.exe" -ArgumentList 'serve', "mode=$Mode", 'layout=0,0,1', "log=$S\q2seq-provlog.txt" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^ready' -Quiet) -and @(Select-String -Path $log -Pattern '^window').Count -eq 3)) { Start-Sleep -Milliseconds 50 }
$h = @{}; Select-String -Path $log -Pattern '^window (\d) hwnd=(\d+)' | ForEach-Object { $h[$_.Matches[0].Groups[1].Value] = $_.Matches[0].Groups[2].Value }
Get-Content $log
$cmd = @'
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; .echotime; ~.; gc"
bp UIAutomationCore!ComInvoker::CallTarget ".echo SRV_COMINVOKER_CALLTARGET; .echotime; ~.; gc"
bp UIAutomationCore!ChannelBasedServerConnection::WriteResponseToChannel ".echo SRV_WRITE_RESPONSE; .echotime; ~.; gc"
bm puia_serve!*serve9name_read* ".echo PROV_NAME_READ; .echotime; ~.; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\q2seq.cdb" $cmd
$dl = "$S\q2seq-$Mode.log"
Remove-Item $dl -ErrorAction SilentlyContinue
$c = Start-Cdb $t.Id "$S\q2seq.cdb" $dl "$S;$S\build\target\release"
& $puia abandon label=sequence-$Mode mode=new "block=slow:$($h['0']):1000" "stuck=$($h['0']):name:0" "call=50:other-window-same-ui-thread:$($h['1']):name:0" "call=100:other-ui-thread:$($h['2']):name:0" elems=0 trials=1 tail_ms=200
Stop-Cdb $c
Stop-Process -Id $t.Id
