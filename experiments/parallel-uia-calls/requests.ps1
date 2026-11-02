# Counts what one call costs on each side: client round trips and server
# requests, batches and provider calls, for findall, name and rop on mockapp.
param([string[]]$Ops = @('findall', 'name', 'rop'), [string]$Fixture = "$PSScriptRoot\..\..\crates\mockapp\tests\fixtures\tree.json", [string]$Tag = 'tree', [int]$Calls = 1)
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$mock = "$S\mtarget\debug\mockapp.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo $mock, "--fixture `"$Fixture`" --backend uia --title puia-q2"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
$null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String '^top (\d+) .*title=puia-q2' | ForEach-Object { $_.Matches[0].Groups[1].Value }
"mockapp pid=$($m.Id) hwnd=$h"
$srv = @'
bm /a mockapp!mockapp::uia::handler::*GetPropertyValue ".echo PROV_PROP; gc"
bm /a mockapp!mockapp::uia::handler::*Navigate "r $t2=@$t2+1; .echo PROV_NAV; .if (@$t2==10) {.echo NAV10; ~.; k 40} .else {k 6}; gc"
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; gc"
bp UIAutomationCore!ChannelBasedServerConnection::OnDataIdle ".echo SRV_ONDATAIDLE; gc"
bp UIAutomationCore!ProcessIncomingBatchedMessages ".echo SRV_BATCH; ~.; k 25; gc"
bp UIAutomationCore!ChannelBasedServerConnection::ProcessOneMessage ".echo SRV_MSG; k 8; gc"
bp UIAutomationCore!ChannelBasedServerConnection::WriteResponseToChannel ".echo SRV_RESP; gc"
bp UIAutomationCore!ReleaseCollection::DispatchReleases ".echo SRV_RELEASE; gc"
.echo READY_MARK
g
'@
$cli = @'
bp UIAutomationCore!ChannelBasedClientConnection::DoCallWithReturn "r $t0=@$t0+1; .echo CLI_CALL; .if (@$t0<=2) {~.; k 25} .else {k 8}; gc"
bp UIAutomationCore!ChannelBasedClientConnection::WaitForResponse ".echo CLI_WAIT; gc"
bp UIAutomationCore!ChannelBasedClientConnection::StartNewRequest ".echo CLI_START; gc"
bp UIAutomationCore!ChannelBasedClientConnection::EndRequest ".echo CLI_END; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\q2-srv.cdb" $srv
Set-Content -Encoding ascii "$S\q2-cli.cdb" $cli
foreach ($op in $Ops) {
    $p = Start-Probe $puia "hwnd=$h op=$op elem=3 elems=16"
    $ls = "$S\q2-$Tag-$op-server.log"; $lc = "$S\q2-$Tag-$op-client.log"
    Remove-Item $ls, $lc -ErrorAction SilentlyContinue
    $cs = Start-Cdb $m.Id "$S\q2-srv.cdb" $ls "$S\mtarget\debug"
    $cc = Start-Cdb $p.Id "$S\q2-cli.cdb" $lc
    $r = @(); for ($i = 0; $i -lt $Calls; $i++) { $r += Invoke-Probe $p }
    Start-Sleep -Milliseconds 500
    Stop-Cdb $cc
    Stop-Cdb $cs
    Stop-Probe $p
    $cnt = { param($f, $k) @(Select-String -Path $f -Pattern "^$k$").Count }
    "$Tag op=$op calls=$Calls result=$($r -join ' | ') client_DoCallWithReturn=$(& $cnt $lc 'CLI_CALL') client_WaitForResponse=$(& $cnt $lc 'CLI_WAIT') client_StartNewRequest=$(& $cnt $lc 'CLI_START') client_EndRequest=$(& $cnt $lc 'CLI_END') server_OnData=$(& $cnt $ls 'SRV_ONDATA') server_OnDataIdle=$(& $cnt $ls 'SRV_ONDATAIDLE') server_batches=$(& $cnt $ls 'SRV_BATCH') server_messages=$(& $cnt $ls 'SRV_MSG') server_responses=$(& $cnt $ls 'SRV_RESP') server_release_dispatches=$(& $cnt $ls 'SRV_RELEASE') provider_navigate=$(& $cnt $ls 'PROV_NAV') provider_getproperty=$(& $cnt $ls 'PROV_PROP')"
}
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() }
