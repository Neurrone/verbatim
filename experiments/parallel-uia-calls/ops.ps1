# Requests and provider calls per client operation: puia ops (ops.rs) against
# mockapp (debug build, ops.json) or the synthetic provider, with cdb on both
# processes. The server log is split at mockapp's hit-counter resets (FENCE),
# the client log at the client's PUIA_OP markers; ops-count.ps1 counts them.
# A script line "mock: <command>" goes to mockapp's stdin instead.
param([string]$Target = 'mock', [string]$Mode = 'sta', [string]$Tag = 'mock', [string[]]$Ops, [string]$MockArgs = '', [switch]$NoDebugger)
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
if ($Target -eq 'mock') {
    $psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$PSScriptRoot\ops.json`" --backend uia --title puia-ops --show $MockArgs"
    $title = 'puia-ops'
} else {
    $psi = New-Object System.Diagnostics.ProcessStartInfo $puia, "serve layout=0 mode=$Mode"
    $title = 'puia-serve-0'
}
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
while (($l = $m.StandardOutput.ReadLine()) -notlike 'ready*') { }
# The writer's first line carries a byte order mark: send a line that only carries it.
if ($Target -eq 'mock') { $m.StandardInput.WriteLine(''); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine() }
$h = & $puia windows pid=$($m.Id) | Select-String "^top (\d+) .*title=$title" | ForEach-Object { $_.Matches[0].Groups[1].Value }
"target pid=$($m.Id) hwnd=$h"
$psi2 = New-Object System.Diagnostics.ProcessStartInfo $puia, "ops hwnd=$h target=$Target"
$psi2.UseShellExecute = $false; $psi2.RedirectStandardInput = $true; $psi2.RedirectStandardOutput = $true
$c = [System.Diagnostics.Process]::Start($psi2)
$r = $c.StandardOutput.ReadLine(); $r
if ($r -notlike 'ready*') { $m.Kill(); throw 'ops client did not start' }
$c.StandardInput.WriteLine(''); $c.StandardInput.Flush(); $null = $c.StandardOutput.ReadLine()
$srv = @'
bp mockapp!mockapp::hits::reset ".echo FENCE; gc"
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; gc"
bp UIAutomationCore!ChannelBasedServerConnection::ProcessOneMessage ".echo SRV_MSG; gc"
bp UIAutomationCore!ChannelBasedServerConnection::WriteResponseToChannel ".echo SRV_RESP; gc"
bp UIAutomationCore!ProcessIncomingBatchedMessages ".echo SRV_BATCH; gc"
bp UIAutomationCore!ReleaseCollection::DispatchReleases ".echo SRV_RELEASE; gc"
bp UIAutomationCore!ChannelBasedServerConnection::SendEventData ".echo SRV_SENDEVENT; ~.; k 30; gc"
bp UIAutomationCore!UiaRaiseAutomationEvent ".echo SRV_RAISE; ~.; gc"
bp UIAutomationCore!UiaRaiseAutomationPropertyChangedEvent ".echo SRV_RAISEPROP; ~.; gc"
.echo READY_MARK
g
'@
if ($Target -ne 'mock') {
    # The synthetic provider has no hit counters: count its COM entry points.
    $srv = $srv.Replace('bp mockapp!mockapp::hits::reset ".echo FENCE; gc"', @'
bm puia!RNvNtCs*_4puia5serve7wndproc ".if (@edx == 0x8003) {.echo FENCE}; gc"
bm /a puia!*IRawElementProviderSimple_Vtbl*new* ".printf \"PROV %y\\n\", @rip; gc"
bm /a puia!*IInvokeProvider_Vtbl*new* ".printf \"PROV %y\\n\", @rip; gc"
'@)
}
$cli = @'
bp UIAutomationCore!ChannelBasedClientConnection::DoCallWithReturn ".echo CLI_CALL; k 10; gc"
bp UIAutomationCore!ChannelBasedClientConnection::WaitForResponse ".printf \"CLI_WAIT timeout=%d\\n\", @r8d; gc"
bp UIAutomationCore!ChannelBasedClientConnection::OnData ".echo CLI_ONDATA; ~.; gc"
bm puia!*Handle*Event*Impl* ".echo CLI_HANDLER; ~.; k 40; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\ops-srv.cdb" $srv
Set-Content -Encoding ascii "$S\ops-cli.cdb" $cli
$ls = "$S\ops-$Tag-server.log"; $lc = "$S\ops-$Tag-client.log"
Remove-Item $ls, $lc -ErrorAction SilentlyContinue
if (-not $NoDebugger) {
    $cs = Start-Cdb $m.Id "$S\ops-srv.cdb" $ls "$S\mtarget\debug;$S\build\target\release"
    $cc = Start-Cdb $c.Id "$S\ops-cli.cdb" $lc "$S\build\target\release"
}
foreach ($op in $Ops) {
    if ($op -like 'mock:*') {
        $m.StandardInput.WriteLine($op.Substring(5).Trim()); $m.StandardInput.Flush()
        "mock $($op.Substring(5).Trim()) -> $($m.StandardOutput.ReadLine())"
        Start-Sleep -Milliseconds 800
        continue
    }
    if ($op -like 'sleep:*') { Start-Sleep -Milliseconds ([int]$op.Substring(6)); continue }
    $c.StandardInput.WriteLine($op); $c.StandardInput.Flush()
    while ($true) {
        $l = $c.StandardOutput.ReadLine()
        $l
        if ($l -like 'op *' -or $l -like 'hits*' -or $l -like 'set *' -or $l -like 'async * launched') { break }
    }
}
# Let late lines (events, async calls) arrive.
Start-Sleep -Milliseconds 800
if (-not $NoDebugger) { Stop-Cdb $cc; Stop-Cdb $cs }
$c.StandardInput.WriteLine('quit'); $c.StandardInput.Close()
while (($l = $c.StandardOutput.ReadLine()) -ne $null) { $l }
if (-not $c.WaitForExit(5000)) { $c.Kill() }
if ($Target -eq 'mock') { $m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() } } else { $m.Kill() }
