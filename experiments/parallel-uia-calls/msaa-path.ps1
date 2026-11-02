# One msaa call (accName and accChildCount on AccessibleObjectFromWindow(OBJID_CLIENT)),
# with the client's outgoing paths and the server's serving thread on breakpoints.
param([int]$TargetPid, [string]$Hwnd, [string]$Tag, [switch]$NoServer)
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$cli = @'
bm /a oleacc!*::get_accName ".echo CLI_OLEACC_NAME; k 20; gc"
bm /a oleacc!*::get_accChildCount ".echo CLI_OLEACC_COUNT; k 20; gc"
bp UIAutomationCore!AccessibleProxy::get_accName ".echo CLI_UIABRIDGE_NAME; k 20; gc"
bp UIAutomationCore!AccessibleProxy::get_accChildCount ".echo CLI_UIABRIDGE_COUNT; k 20; gc"
bp UIAutomationCore!ChannelBasedClientConnection::DoCallWithReturn ".echo CLI_UIA_REQUEST; k 20; gc"
bp combase!NdrExtpProxySendReceive ".echo CLI_COM_RPC; k 20; gc"
bp win32u!NtUserMessageCall ".echo CLI_WINDOW_MESSAGE; r rdx; k 20; gc"
.echo READY_MARK
g
'@
$srv = @'
bp UIAutomationCore!AccessibleProxy::get_accName ".echo SRV_UIABRIDGE_NAME; ~.; k 45; gc"
bp UIAutomationCore!AccessibleProxy::get_accChildCount ".echo SRV_UIABRIDGE_COUNT; ~.; k 45; gc"
bm /a oleacc!*::get_accName ".echo SRV_OLEACC_NAME; ~.; k 45; gc"
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_UIA_REQUEST; gc"
bp USER32!DefWindowProcW ".if ((@edx == 0xd) | (@edx == 0xe)) {.echo SRV_WM_GETTEXT_DEFWINDOWPROC; ~.; k 30}; gc"
bm /a mockapp!mockapp::msaa::handler::*get_accName ".echo SRV_MOCKAPP_NAME; ~.; k 45; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\q3-cli.cdb" $cli
Set-Content -Encoding ascii "$S\q3-srv.cdb" $srv
$p = Start-Probe $puia "hwnd=$Hwnd op=msaa elem=0 elems=0"
$lc = "$S\q3-$Tag-client.log"; $ls = "$S\q3-$Tag-server.log"
Remove-Item $lc, $ls -ErrorAction SilentlyContinue
if (-not $NoServer) { $cs = Start-Cdb $TargetPid "$S\q3-srv.cdb" $ls "$S\mtarget\debug" }
$cc = Start-Cdb $p.Id "$S\q3-cli.cdb" $lc
$r = Invoke-Probe $p
Start-Sleep -Milliseconds 300
Stop-Cdb $cc
if (-not $NoServer) { Stop-Cdb $cs }
Stop-Probe $p
"$Tag result=$r"
foreach ($f in @($lc) + $(if (-not $NoServer) { @($ls) } else { @() })) {
    $c = Select-String -Path $f -Pattern '^(CLI|SRV)_[A-Z_]+$' | ForEach-Object { $_.Line } | Group-Object | ForEach-Object { "$($_.Name)=$($_.Count)" }
    "$Tag $(Split-Path $f -Leaf): $($c -join ' ')"
}
