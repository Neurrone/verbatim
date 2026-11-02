# The handles UIA's channel uses, in mockapp and in a puia ops client: cdb on
# both before the client connects, with breakpoints on the file, pipe and ALPC
# calls that print the handle's type and name (!handle). Nothing is written to
# the channel but UIA's own traffic: a connection, a name read and an event.
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture `"$PSScriptRoot\ops.json`" --backend uia --title puia-tr"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
while (($l = $m.StandardOutput.ReadLine()) -notlike 'ready*') { }
$m.StandardInput.WriteLine(''); $m.StandardInput.Flush(); $null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String '^top (\d+) .*title=puia-tr' | ForEach-Object { $_.Matches[0].Groups[1].Value }
$psi2 = New-Object System.Diagnostics.ProcessStartInfo $puia, "ops hwnd=$h target=mock wait=1"
$psi2.UseShellExecute = $false; $psi2.RedirectStandardInput = $true; $psi2.RedirectStandardOutput = $true
$c = [System.Diagnostics.Process]::Start($psi2)
$c.StandardOutput.ReadLine()
"mockapp pid=$($m.Id) client pid=$($c.Id)"
$common = @'
bp KERNELBASE!CreateNamedPipeW ".echo X_CREATENAMEDPIPE; du @rcx; ~.; k 14; gc"
bp KERNELBASE!CreateFileW ".echo X_CREATEFILE; du @rcx; ~.; k 14; gc"
bp KERNELBASE!ConnectNamedPipe ".printf \"X_CONNECTNAMEDPIPE h=%p\\n\", @rcx; ~.; k 10; !handle @rcx f; gc"
bp KERNELBASE!WriteFile "r $t1=@$t1+1; .printf \"X_WRITEFILE h=%p len=%d\\n\", @rcx, @r8d; ~.; .if (@$t1<=4) {k 12; !handle @rcx f}; gc"
bp KERNELBASE!ReadFile "r $t2=@$t2+1; .printf \"X_READFILE h=%p len=%d\\n\", @rcx, @r8d; ~.; .if (@$t2<=4) {k 12; !handle @rcx f}; gc"
bp KERNELBASE!WaitForMultipleObjectsEx "r $t4=@$t4+1; .if (@$t4<=12) {.printf \"X_WAITMULTIPLE count=%d timeout=%d\\n\", @ecx, @r9d; ~.; k 6; .for (r $t5=0; @$t5<@rcx; r $t5=@$t5+1) {!handle poi(@rdx+@$t5*8) f}}; gc"
bp ntdll!NtAlpcSendWaitReceivePort "r $t3=@$t3+1; .if (@$t3<=3) {.printf \"X_ALPC h=%p\\n\", @rcx; ~.; k 12; !handle @rcx f} .else {.echo X_ALPC}; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\tr.cdb" $common
$ls = "$S\transport-server.log"; $lc = "$S\transport-client.log"
Remove-Item $ls, $lc -ErrorAction SilentlyContinue
$cs = Start-Cdb $m.Id "$S\tr.cdb" $ls "$S\mtarget\debug"
$cc = Start-Cdb $c.Id "$S\tr.cdb" $lc "$S\build\target\release"
foreach ($op in 'connect', 'cur_name', 'add_auto', 'mock: select item1', 'remove_auto') {
    if ($op -like 'mock:*') { $m.StandardInput.WriteLine($op.Substring(5).Trim()); $m.StandardInput.Flush(); "mock -> $($m.StandardOutput.ReadLine())"; Start-Sleep -Milliseconds 1500; continue }
    if ($op -ne 'connect') { "[$(Get-Date -Format HH:mm:ss.fff)] $op"; $c.StandardInput.WriteLine($op) } else { $c.StandardInput.WriteLine('') }
    $c.StandardInput.Flush()
    while ($true) { $l = $c.StandardOutput.ReadLine(); $l; if ($l -like 'op *' -or $l -like 'ready*') { break } }
}
Start-Sleep -Milliseconds 800
Stop-Cdb $cc; Stop-Cdb $cs
$c.StandardInput.WriteLine('quit'); $c.StandardInput.Close(); if (-not $c.WaitForExit(5000)) { $c.Kill() }
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() }
