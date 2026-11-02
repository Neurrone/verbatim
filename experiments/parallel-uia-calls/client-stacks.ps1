# Where the calling threads of one loaded client process are: a puia worker
# with 8 threads making name reads into the synthetic provider back to back,
# and 5 non-invasive snapshots of every client stack (cdb -pv, ~*k), 1 s apart.
param([string]$Mode = 'sta', [string]$Out)
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
$log = "$S\cs-serve-$Mode-windows.txt"
$sv = Start-Process -FilePath $puia -ArgumentList 'serve', 'layout=0', "mode=$Mode" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
while (-not ((Test-Path $log) -and ((Get-Content $log) -match '^ready'))) { Start-Sleep -Milliseconds 50 }
$h = ((Get-Content $log) -match '^window 0 hwnd=(\d+)')[0] -replace '^window 0 hwnd=(\d+).*', '$1'
$psi = New-Object System.Diagnostics.ProcessStartInfo $puia, "worker hwnd=$h op=name threads=8 elems=0"
$psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
$w = [System.Diagnostics.Process]::Start($psi)
$null = $w.StandardOutput.ReadLine()
$w.StandardInput.WriteLine('go 400000'); $w.StandardInput.Flush()
Start-Sleep -Milliseconds 1000
$sym = "srv*$PSScriptRoot\..\..\target\symbols*https://msdl.microsoft.com/download/symbols;$S\build\target\release"
for ($i = 1; $i -le 5; $i++) {
    $l = "$S\cs-$Mode-$i.log"; Remove-Item $l -ErrorAction SilentlyContinue
    & cdbX64.exe -pv -p $w.Id -y $sym -logo $l -c "~*k 25; qd" | Out-Null
    Get-Content $l | Add-Content $Out
    Start-Sleep -Milliseconds 1000
}
$w.Kill(); Stop-Process -Id $sv.Id
