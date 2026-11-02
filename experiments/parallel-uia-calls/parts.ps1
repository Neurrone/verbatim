# Splits one name read into its parts on the synthetic provider, in each
# threading mode, at N = 1, 2, 4 and 8 client threads (puia parts), and
# with -Isolated adds single calls with their UTC times for a trace.
# PUIA_SCRATCH: the scratch folder with build\target\release\puia.exe; logs go there.
param([string[]]$Modes = @('sta', 'agile', 'nocom'), [string]$Out, [int]$Reps = 15, [int]$Isolated = 0, [string]$Threads = '1,2,4,8')
$S = $env:PUIA_SCRATCH
$puia = "$S\build\target\release\puia.exe"
Copy-Item $puia "$S\puia-serve.exe" -Force
Remove-Item $Out -ErrorAction SilentlyContinue
foreach ($mode in $Modes) {
    $log = "$S\parts-serve-$mode.txt"
    $plog = "$S\parts-provider-$mode.txt"
    $sv = Start-Process "$S\puia-serve.exe" -ArgumentList 'serve', "mode=$mode", 'layout=0,0,1', "log=$plog" -RedirectStandardOutput $log -PassThru -WindowStyle Hidden
    while (-not ((Test-Path $log) -and (Select-String -Path $log -Pattern '^ready' -Quiet) -and @(Select-String -Path $log -Pattern '^window').Count -eq 3)) { Start-Sleep -Milliseconds 50 }
    $h0 = (Select-String -Path $log -Pattern '^window 0 hwnd=(\d+)').Matches[0].Groups[1].Value
    & $puia parts label=serve-$mode hwnd=$h0 op=name k=64 threads=$Threads reps=$Reps serve_log="$plog" isolated=$Isolated *>> $Out
    Stop-Process -Id $sv.Id
}
