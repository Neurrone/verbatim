param([string]$Backend = 'uia')
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe and mockapp in mtarget\debug; logs go there.
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo "$S\mtarget\debug\mockapp.exe", "--fixture $PSScriptRoot\..\..\crates\mockapp\tests\fixtures\tree.json --backend $Backend --title puia-q3-$Backend"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$m = [System.Diagnostics.Process]::Start($psi)
$null = $m.StandardOutput.ReadLine()
$h = & $puia windows pid=$($m.Id) | Select-String "^top (\d+) .*title=puia-q3-$Backend" | ForEach-Object { $_.Matches[0].Groups[1].Value }
& "$PSScriptRoot\msaa-path.ps1" -TargetPid $m.Id -Hwnd $h -Tag "mockapp-$Backend"
$m.StandardInput.WriteLine('quit'); if (-not $m.WaitForExit(5000)) { $m.Kill() }
