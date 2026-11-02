# Which thread of Windows Terminal runs a text call into its terminal
# control: breakpoints on the server's I/O thread taking the request, on
# UIA's cross-apartment call, and on system XAML's text range wrapper
# (Windows.UI.Xaml), which forwards to the control's own provider, with
# the thread and its stack. A probe in its own process makes the call.
param([int]$TargetPid, [string]$Hwnd, [string[]]$Ops = @('text', 'name'), [int]$Elem = 10)
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe; logs go there.
$S = $env:PUIA_SCRATCH
Import-Module "$PSScriptRoot\dbg.psm1" -Force
$puia = "$S\build\target\release\puia.exe"
# The terminal control's text pattern, not the tab title's (a TextBlock, found first).
$env:PUIA_TEXT_CLASS = 'TermControl'
$cmd = @'
bp UIAutomationCore!ChannelBasedServerConnection::OnData ".echo SRV_ONDATA; ~.; gc"
bp UIAutomationCore!ComInvoker::CallTarget ".echo SRV_COMINVOKER; ~.; k 14; gc"
bp UIAutomationCore!InvokeElementMethodOnCorrectContext_Callback ".echo SRV_ELEMENT_METHOD_CONTEXT; ~.; gc"
bp Windows_UI_Xaml!CUIATextRangeProviderWrapper::GetText ".echo XAML_RANGE_GETTEXT; ~.; k 45; gc"
bp Windows_UI_Xaml!CManagedTextRangeProvider::GetText ".echo XAML_MANAGED_GETTEXT; ~.; k 8; gc"
bm Microsoft_Terminal_Control!Microsoft::Console::Types::UiaTextRangeBase::GetText ".echo TERMINAL_UIATEXTRANGEBASE_GETTEXT; ~.; k 12; gc"
bm Windows_UI_Xaml!CUIAWrapper::GetPropertyValue ".echo XAML_GETPROPERTYVALUE; ~.; k 30; gc"
.echo READY_MARK
g
'@
Set-Content -Encoding ascii "$S\wt-thread.cdb" $cmd
"window thread: $(& $puia windows pid=$TargetPid | Select-String "^top $Hwnd ")"
foreach ($op in $Ops) {
    $p = Start-Probe $puia "hwnd=$Hwnd op=$op elem=$Elem elems=16"
    $dl = "$S\wt-thread-$op.log"; Remove-Item $dl -ErrorAction SilentlyContinue
    $c = Start-Cdb $TargetPid "$S\wt-thread.cdb" $dl
    $r = Invoke-Probe $p
    Start-Sleep -Milliseconds 300
    Stop-Cdb $c
    Stop-Probe $p
    "SERVING op=$op result=$r"
    Get-Content $dl | Where-Object { $_ -match '^(SRV_|XAML_|TERMINAL_)|Id: |^[0-9a-f`]+ [0-9a-f`]+ +\S+!|^\(Inline' }
}
