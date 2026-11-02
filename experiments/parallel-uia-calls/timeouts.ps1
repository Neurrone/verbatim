# When a stuck call and a call queued behind it fail, for a set transaction
# (and connection) timeout, and what the provider receives after the release.
# mockapp (debug build) holds the first provider call (hold); a puia ops client
# starts call A, then call B 200 ms later on another thread, both name reads of
# one held element; mockapp releases at -ReleaseMs after A started. No debugger.
param([int]$Transaction = 2000, [int]$Connection = 0, [int]$ReleaseMs = 15000, [string]$Tag = 't2', [string]$OpB = 'cur_name', [switch]$Stacks, [switch]$ClientDebugger)
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$PSScriptRoot\ops.json`" --backend uia --title puia-to"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
while (($l = $m.StandardOutput.ReadLine()) -notlike 'ready*') { }
$m.StandardInput.WriteLine(''); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String '^top (\d+) .*title=puia-to' | ForEach-Object { $_.Matches[0].Groups[1].Value }
$psi2 = New-Object System.Diagnostics.ProcessStartInfo $puia, "ops hwnd=$h target=mock"
$psi2.UseShellExecute = $false; $psi2.RedirectStandardInput = $true; $psi2.RedirectStandardOutput = $true
$c = [System.Diagnostics.Process]::Start($psi2)
$null = $c.StandardOutput.ReadLine()
$c.StandardInput.WriteLine(''); $c.StandardInput.Flush(); $null = $c.StandardOutput.ReadLine()
function Cmd($line) { $c.StandardInput.WriteLine($line); $c.StandardInput.Flush(); $c.StandardOutput.ReadLine() }
$sw = [System.Diagnostics.Stopwatch]::StartNew()
function T { '{0,8:F1}' -f $sw.Elapsed.TotalMilliseconds }
"$Tag transaction=$Transaction connection=$Connection release=$ReleaseMs"
Cmd "transaction $Transaction"
if ($Connection -gt 0) { Cmd "connection $Connection" }
"before: $(Cmd 'hits')"
if ($ClientDebugger) {
    # Invasive, on the client only: the timeout each wait is given, with the debugger's clock.
    Set-Content -Encoding ascii "$S\to-cli.cdb" @'
bp UIAutomationCore!ChannelBasedClientConnection::GetTimeout "gu; .printf \"CLI_GETTIMEOUT ret=%d\\n\", @eax; .echotime; ~.; gc"
bp UIAutomationCore!ChannelBasedClientConnection::WaitForResponse ".printf \"CLI_WAITFORRESPONSE timeout=%d\\n\", @r8d; .echotime; ~.; gc"
bp KERNELBASE!WaitOnAddress ".printf \"CLI_WAITONADDRESS timeout=%d\\n\", @r9d; .echotime; ~.; k 3; gc"
bp UIAutomationCore!ChannelBasedClientConnection::SetResponseInfo ".echo CLI_SETRESPONSE; .echotime; ~.; gc"
.echo READY_MARK
g
'@
    $cc = Start-Cdb $c.Id "$S\to-cli.cdb" "$S\timeouts-$Tag-client.log"
}
$m.StandardInput.WriteLine('hold'); $m.StandardInput.Flush(); "$(T) mockapp: $($m.StandardOutput.ReadLine())"
$t0 = $sw.Elapsed.TotalMilliseconds
"$(T) $(Cmd 'async A cur_name')"
"$(T) mockapp: $($m.StandardOutput.ReadLine())"
Start-Sleep -Milliseconds 200
"$(T) $(Cmd "async B $OpB")"
if ($Stacks) {
    Start-Sleep -Milliseconds 1000
    # Every stack of both processes, non-invasive, while A and B wait.
    foreach ($p in @($c.Id, $m.Id)) {
        $log = "$S\timeouts-$Tag-stacks-$p.log"; Remove-Item $log -ErrorAction SilentlyContinue
        & cdbX64.exe -pv -p $p -y "srv*$PSScriptRoot\..\..\target\symbols*https://msdl.microsoft.com/download/symbols;$S\mtarget\debug;$S\build\target\release" -logo $log -c "~*k 30; qd" | Out-Null
    }
    "$(T) stacks taken"
}
# Read the async results while waiting for the release time.
$done = 0
$reader = $c.StandardOutput.ReadLineAsync()
while ($sw.Elapsed.TotalMilliseconds - $t0 -lt $ReleaseMs) {
    if ($reader.Wait(50)) { "$(T) $($reader.Result)"; if ($reader.Result -like 'async * took=*') { $done++ }; $reader = $c.StandardOutput.ReadLineAsync() }
}
$m.StandardInput.WriteLine('release'); $m.StandardInput.Flush(); "$(T) release sent; mockapp: $($m.StandardOutput.ReadLine())"
$deadline = $sw.Elapsed.TotalMilliseconds + 30000
while ($done -lt 2 -and $sw.Elapsed.TotalMilliseconds -lt $deadline) {
    if ($reader.Wait(50)) { "$(T) $($reader.Result)"; if ($reader.Result -like 'async * took=*') { $done++ }; $reader = $c.StandardOutput.ReadLineAsync() }
}
Start-Sleep -Milliseconds 2000
$c.StandardInput.WriteLine('hits'); $c.StandardInput.Flush()
$null = $reader.Wait(10000); "after: $($reader.Result)"
if ($ClientDebugger) { Stop-Cdb $cc }
$c.StandardInput.WriteLine('quit'); $c.StandardInput.Close(); if (-not $c.WaitForExit(5000)) { $c.Kill() }
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() }
