param([string]$Scenario, [int]$From, [int]$To, [string]$Tree, [switch]$Trace, [string]$Out = 'lc')
# Runs one scenario repeatedly, optionally inside a WPR trace; keeps every
# run's logs, and a trace only when a provider probe took 2 s or more.
$ErrorActionPreference = 'Continue'
$root = 'C:\Users\Dickson\Desktop\projects\verbatim'
$out = "<scratch folder>\$Out"  # redacted: a session's scratch folder
New-Item -ItemType Directory -Force $out | Out-Null
$env:VERBATIM_E2E_ENDPOINT = '127.0.0.1:44001'
$env:VERBATIM_E2E_VERBATIM_EXE = "$root\target\debug\verbatim.exe"
$env:CARGO_TARGET_DIR = "$root\target"
$env:VERBATIM_WX_DIR = "$root\target\wxwidgets"
$xt = "$root\target\debug\xtask.exe"
Set-Location $Tree
foreach ($i in $From..$To) {
  if ($Trace) { & $xt trace start *> $null }
  $r = cargo test -q -p verbatim-e2e --test e2e -- --ignored --exact $Scenario --test-threads=1 2>&1 | Out-String
  $etl = $null
  if ($Trace) { & $xt trace stop *> $null; $etl = (Get-Content C:\ProgramData\Verbatim\Traces\last-trace.txt -Raw).Trim() }
  $ok = $r -match 'test result: ok'
  $dir = "$Tree\target\e2e-artifacts\$Scenario"
  $slow = Select-String -Path "$dir\outpost-*.log", "$dir\listener.log" -Pattern 'arbitration probed.*elapsed_ms(\x1b\[[0-9;]*m|=)*[2-9]\d\d\d' | Select-Object -First 1
  $first = (Import-Csv "$dir\latency.csv" -ErrorAction SilentlyContinue | Select-Object -First 1).utterance
  $tag = if ($slow) { 'SLOW' } else { 'fast' }
  $keep = "$out\$Scenario-$i"
  New-Item -ItemType Directory -Force $keep | Out-Null
  Get-ChildItem $dir -File | Where-Object { $_.Extension -notin '.mp4', '.wav' } | Copy-Item -Destination $keep
  $r | Set-Content "$keep\test-output.txt"
  if ($etl -and -not $slow) { Remove-Item $etl -ErrorAction SilentlyContinue; $etl = 'deleted' }
  "$Scenario $i ok=$ok probe=$tag first=""$first"" etl=$etl" | Tee-Object -Append -FilePath "$out\runs.txt"
}
