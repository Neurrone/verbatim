# Which thread of the application serves a UIA name read: breakpoints on the
# server's I/O thread entry and on the callback that runs the provider call.
param([int]$TargetPid, [string]$Hwnd, [string]$Tag, [string]$Op = 'name', [int]$Elem = 3, [int]$Elems = 16)
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$srv = @'
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_IO_ONDATA; ~.; k 10; gc"
bp UIAutomationCore!InvokeElementMethodOnCorrectContext_Callback ".echo SRV_PROVIDER_CONTEXT; ~.; k 40; gc"
bp UIAutomationCore!ComInvoker::CallTarget ".echo SRV_COMINVOKER; ~.; k 12; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\q1-srv.cdb" $srv
$p = Start-Probe $puia "hwnd=$Hwnd op=$Op elem=$Elem elems=$Elems"
$ls = "$S\q1-$Tag-$Op-server.log"
Remove-Item $ls -ErrorAction SilentlyContinue
$cs = Start-Cdb $TargetPid "$S\q1-srv.cdb" $ls
$r = Invoke-Probe $p
Start-Sleep -Milliseconds 300
Stop-Cdb $cs
Stop-Probe $p
"$Tag op=$Op result=$r"
$c = Select-String -Path $ls -Pattern '^SRV_[A-Z_]+$' | ForEach-Object { $_.Line } | Group-Object | ForEach-Object { "$($_.Name)=$($_.Count)" }
"$Tag server: $($c -join ' ')"
