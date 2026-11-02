# One search into a blocked mockapp (debug build, small.json) from E-TT's client
# (puia ett, src/ops.rs): with -Block hold, the next provider call is held
# until -ReleaseMs after the search starts; with -Block stall, the window
# thread takes no message for -ReleaseMs. -Op ett_find is E-TT's
# FindFirstBuildCache, stall_find the removed stalled-thread test's FindFirst.
# -Conn0 is the connection timeout the client is made with, -Conn1 the one set
# before the root is fetched, -Connection the one set just before the search.
# -GoDelay waits between the root's fetch and the block. -ProviderDebugger
# attaches cdb to mockapp before the client starts; -ClientDebugger attaches
# cdb to the client after it fetched the root. No debugger by default.
param([string]$Block = 'hold', [string]$Op = 'ett_find', [int]$Transaction = 1000, [int]$Connection = 1000,
      [int]$Recovery = 1, [int]$Conn0 = 10000, [int]$Conn1 = 0, [int]$ReleaseMs = 16000, [int]$GoDelay = 0,
      [string]$Tag = 'search', [switch]$ProviderDebugger, [switch]$ClientDebugger)
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$fixture = "$PSScriptRoot\..\..\crates\mockapp\tests\fixtures\small.json"
$sym = "$S\mtarget\debug;$S\build\target\release"
# The provider: each request the I/O thread reads, the stub that runs it, each
# response, each window message and each provider call, with the debugger's clock.
Set-Content -Encoding ascii "$S\search-srv.cdb" @'
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; .echotime; gc"
bp UIAutomationCore!ChannelBasedServerConnection::ProcessOneMessage ".echo SRV_MSG; gc"
bp UIAutomationCore!ChannelBasedServerConnection::WriteResponseToChannel ".echo SRV_RESP; .echotime; gc"
bp UIAutomationCore!ProcessIncomingBatchedMessages ".echo SRV_BATCH; gc"
bp UIAutomationCore!ReleaseCollection::DispatchReleases ".echo SRV_RELEASE; gc"
bm /a UIAutomationCore!*Stub::Incoming_* ".printf \"SRV_STUB %y\\n\", @rip; .echotime; gc"
bp mockapp!mockapp::window::wnd_proc ".printf \"WND msg=%x wp=%p lp=%p\\n\", @edx, @r8, @r9; .echotime; gc"
bp mockapp!mockapp::hits::hit_get_object ".printf \"GETOBJECT wp=%p lp=%d\\n\", @rcx, @edx; .echotime; gc"
bp mockapp!mockapp::hits::hit_title_bar_info ".echo TITLEBARINFO; .echotime; gc"
bm mockapp!mockapp::hits::hit ".printf \"HIT %d\\n\", @cl; gc"
.echo READY_MARK
g
'@
# The client: each request with the frames that send it, each SendMessageTimeoutW
# with its message and timeout, and the timeout each wait reads.
Set-Content -Encoding ascii "$S\search-cli.cdb" @'
bp UIAutomationCore!ChannelBasedClientConnection::DoCallWithReturn ".echo CLI_CALL; .echotime; k 14; gc"
bp UIAutomationCore!ChannelBasedClientConnection::WaitForResponse ".printf \"CLI_WAIT timeout=%d\\n\", @r8d; gc"
bp USER32!SendMessageTimeoutW ".printf \"CLI_SMTO hwnd=%p msg=%x wp=%p lp=%p flags=%x timeout=%d\\n\", @rcx, @edx, @r8, @r9, dwo(@rsp+28), dwo(@rsp+30); .echotime; k 8; gc"
bp UIAutomationCore!ChannelBasedClientConnection::GetTimeout "gu; .printf \"CLI_GETTIMEOUT ret=%d\\n\", @eax; gc"
.echo READY_MARK
g
'@
$psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$fixture`" --backend uia --title puia-search"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
while (($l = $m.StandardOutput.ReadLine()) -notlike 'ready*') { }
$m.StandardInput.WriteLine(''); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String '^top (\d+) .*title=puia-search' | ForEach-Object { $_.Matches[0].Groups[1].Value }
if ($ProviderDebugger) { $cs = Start-Cdb $m.Id "$S\search-srv.cdb" "$S\search-$Tag-server.log" $sym; 'provider debugger attached before the client' }
$psi2 = New-Object System.Diagnostics.ProcessStartInfo $puia, "ett hwnd=$h trans=$Transaction conn=$Connection recovery=$Recovery conn0=$Conn0 conn1=$Conn1 op=$Op"
$psi2.UseShellExecute = $false; $psi2.RedirectStandardInput = $true; $psi2.RedirectStandardOutput = $true
$c = [System.Diagnostics.Process]::Start($psi2)
$r = $c.StandardOutput.ReadLine()
"$Tag block=$Block op=$Op transaction=$Transaction connection=$Connection recovery=$Recovery conn0=$Conn0 conn1=$Conn1 release=$ReleaseMs mockapp=$($m.Id) client=$($c.Id) $r"
# The first line carries the writer's byte order mark; the client waits for go.
$c.StandardInput.WriteLine(''); $c.StandardInput.Flush()
$sw = [System.Diagnostics.Stopwatch]::StartNew()
function T { '{0,8:F1}' -f $sw.Elapsed.TotalMilliseconds }
if ($ClientDebugger) { $cc = Start-Cdb $c.Id "$S\search-cli.cdb" "$S\search-$Tag-client.log" $sym; "$(T) client debugger attached" }
$script:mr = $m.StandardOutput.ReadLineAsync()
$script:cr = $c.StandardOutput.ReadLineAsync()
$script:done = 0
function Pump([double]$until, [int]$need) {
    while ($sw.Elapsed.TotalMilliseconds -lt $until) {
        if ($script:mr -and $script:mr.Wait(5)) { if ($null -eq $script:mr.Result) { $script:mr = $null } else { "$(T) mockapp: $($script:mr.Result)"; $script:mr = $m.StandardOutput.ReadLineAsync() } }
        if ($script:cr -and $script:cr.Wait(5)) { if ($null -eq $script:cr.Result) { $script:cr = $null } else { "$(T) $($script:cr.Result)"; if ($script:cr.Result -like 'ett *') { $script:done++ }; $script:cr = $c.StandardOutput.ReadLineAsync() } }
        if (-not $script:mr -and -not $script:cr) { Start-Sleep -Milliseconds 20 }
        if ($need -gt 0 -and $script:done -ge $need) { break }
    }
}
if ($GoDelay -gt 0) { Start-Sleep -Milliseconds $GoDelay; "$(T) waited $GoDelay ms before the block" }
if ($Block -eq 'hold') { $m.StandardInput.WriteLine('hold') } else { $m.StandardInput.WriteLine("stall $ReleaseMs") }
$m.StandardInput.Flush()
Pump ($sw.Elapsed.TotalMilliseconds + 300) 0
$t0 = $sw.Elapsed.TotalMilliseconds
"$(T) go at $((Get-Date).ToString('HH:mm:ss.fff'))"
$c.StandardInput.WriteLine('go'); $c.StandardInput.Flush()
Pump ($t0 + $ReleaseMs) 0
if ($Block -eq 'hold') { $m.StandardInput.WriteLine('release'); $m.StandardInput.Flush(); "$(T) release sent at $((Get-Date).ToString('HH:mm:ss.fff'))" }
Pump ($sw.Elapsed.TotalMilliseconds + 30000) 1
Pump ($sw.Elapsed.TotalMilliseconds + 2500) 0
if ($ClientDebugger) { Stop-Cdb $cc; "$(T) client debugger detached" }
if ($ProviderDebugger) { Stop-Cdb $cs; "$(T) provider debugger detached" }
$c.StandardInput.WriteLine('quit'); $c.StandardInput.Close(); if (-not $c.WaitForExit(5000)) { $c.Kill(); 'client killed' }
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill(); 'mockapp killed' }
