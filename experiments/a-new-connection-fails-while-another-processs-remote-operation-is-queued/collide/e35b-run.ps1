# Runs an e35b script once per fresh conhost console (cmd.exe as client):
# starts the console, finds conhost's window thread, runs, closes it.
param([string]$Script, [int]$Count = 5)
Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class E35b { delegate bool P(IntPtr h, IntPtr l);
[DllImport("user32.dll")] static extern bool EnumThreadWindows(int tid, P cb, IntPtr l);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
public static bool OwnsConsole(int tid) { bool found = false; EnumThreadWindows(tid, (h,l) => { var b = new StringBuilder(64); GetClassName(h,b,64); if (b.ToString() == "ConsoleWindowClass") found = true; return true; }, IntPtr.Zero); return found; } }
'@
$exe = 'C:\Users\Dickson\Desktop\projects\verbatim\target\collide\debug\collide.exe'
$out = [IO.Path]::ChangeExtension($Script, '.out')
Set-Content -Path $out -Value $null
for ($n = 1; $n -le $Count; $n++) {
    $title = "collide-fresh-$n-$PID"
    $console = Start-Process conhost.exe -ArgumentList "cmd.exe /k title $title" -PassThru
    $tid = $null
    for ($i = 0; $i -lt 50 -and -not $tid; $i++) {
        Start-Sleep -Milliseconds 100
        $tid = ((Get-Process -Id $console.Id).Threads | Where-Object { [E35b]::OwnsConsole($_.Id) } | Select-Object -First 1).Id
    }
    Start-Sleep -Milliseconds 500  # the title command runs after the window exists
    $env:CONSOLE_TITLE = $title; $env:CONSOLE_TID = $tid
    Add-Content -Path $out -Value "=== run $n conhost pid=$($console.Id) window-thread=$tid"
    & $exe run $Script 2>&1 | Add-Content -Path $out
    Get-CimInstance Win32_Process -Filter "ParentProcessId=$($console.Id)" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    Stop-Process -Id $console.Id -Force -ErrorAction SilentlyContinue
}
