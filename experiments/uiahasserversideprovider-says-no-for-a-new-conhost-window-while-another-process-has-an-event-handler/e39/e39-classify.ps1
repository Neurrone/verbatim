# e39 experiment 3: opens windows of several frameworks, then has one fresh
# collide client run, on every visible top-level window of those processes
# and every descendant window, UiaHasServerSideProvider, the raw probe and
# the lookup (`classify`), and closes what it opened. Windows Terminal is
# left out: a new terminal window would be hosted by the user's running
# terminal process. Output: e39-classify.out.
$ErrorActionPreference = 'Stop'
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
$here = $PSScriptRoot
$exe = 'C:\Users\Dickson\Desktop\projects\verbatim\target\collide\debug\collide.exe'
$edgeData = Join-Path $env:TEMP 'e39-edge-profile'
$before = @{}
Get-Process | ForEach-Object { $before[$_.Id] = $true }

$started = @()
$started += Start-Process notepad.exe -PassThru
$started += Start-Process winver.exe -PassThru
$started += Start-Process rundll32.exe -ArgumentList 'shell32.dll,Control_RunDLL main.cpl' -PassThru
$started += Start-Process conhost.exe -ArgumentList 'cmd.exe /d /k title e39-console' -PassThru
$started += Start-Process powershell.exe -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $here 'e39-winforms.ps1') -PassThru
$started += Start-Process 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe' -ArgumentList "--user-data-dir=$edgeData", '--no-first-run', '--new-window', 'data:text/html,<title>e39-edge</title><h1>Heading</h1><input value=text><button>Press</button>' -PassThru
Start-Process explorer.exe -ArgumentList 'C:\Windows\Web'
Start-Process 'ms-settings:'
Start-Sleep -Seconds 6

$new = Get-Process | Where-Object { -not $before.ContainsKey($_.Id) -and $_.MainWindowHandle -ne 0 }
$explorer = Get-Process explorer
$frame = Get-Process ApplicationFrameHost -ErrorAction SilentlyContinue | Select-Object -First 1
$pids = @($new | ForEach-Object { $_.Id }) + @($explorer | ForEach-Object { $_.Id }) + @($frame | ForEach-Object { $_.Id })
$pids = $pids | Where-Object { $_ -ne 10412 } | Sort-Object -Unique
"pids: " + (($new | ForEach-Object { "$($_.Id)=$($_.ProcessName)" }) -join ' ') + " explorer=$($explorer.Id -join ',') frame=$($frame.Id)" | Out-File -Encoding utf8 (Join-Path $here 'e39-classify.out')
"init`nclassify $($pids -join ' ')`nquit`n" | & $exe client 0 K | Out-File -Append -Encoding utf8 (Join-Path $here 'e39-classify.out')

# Close what was opened: the Explorer window by its folder, the rest by process.
(New-Object -ComObject Shell.Application).Windows() | Where-Object { $_.LocationName -eq 'Web' } | ForEach-Object { $_.Quit() }
@($started) + @($new) | Where-Object { $_.ProcessName -ne 'explorer' } | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch {} }
Get-Process msedge -ErrorAction SilentlyContinue | Where-Object { -not $before.ContainsKey($_.Id) } | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch {} }
Get-Process SystemSettings, notepad, winver, rundll32 -ErrorAction SilentlyContinue | Where-Object { -not $before.ContainsKey($_.Id) } | ForEach-Object { try { Stop-Process -Id $_.Id -Force } catch {} }
