<#
Runs end-to-end scenarios one at a time against the scratch measurement
build, for the focus pipeline rework's step 1 ("Measurement",
docs/design/focus-pipeline-rework.md). Scratch tooling, never merged.

It builds the scratch worktree's binaries into its own target directory,
starts a local agent from that build, and runs each scenario Runs times
through `cargo test -p verbatim-e2e --test e2e -- --ignored --exact`,
one test process at a time, with:

- VERBATIM_E2E_VERBATIM_EXE naming the scratch build's verbatim.exe, so the
  harness stages that build (and does not build the default target\debug,
  which a worktree built with CARGO_TARGET_DIR does not have);
- VERBATIM_E2E_RUST_LOG set to the harness's default filter widened to the
  patch's target, rework_measure=debug;
- VERBATIM_E2E_RECORD=0 unless -Record is given.

After each run it copies the scenario's artifacts folder,
<worktree>\target\e2e-artifacts\<scenario>, without the video, to
<OutRoot>\<Label>\<scenario>\<NN>, with the test's output as
cargo-test.log and result.txt (pass or fail, and the exit code). A failed
scenario is recorded and the batch goes on; the batch stops when the agent
has exited or no longer accepts a connection, saying so in
<OutRoot>\<Label>\stopped.txt. With -UntilHookIns, a scenario stops early
once its runs in this label hold that many outpost starts (the patch's
hook_in lines).

It takes over the desktop while it runs, as every local run does.

Example:
  powershell -ExecutionPolicy Bypass -File batch.ps1 -Label repeats `
    -Scenarios notepad_editing,explorer_folder_window -Runs 5
#>
param(
    [Parameter(Mandatory = $true)][string[]]$Scenarios,
    [int]$Runs = 5,
    [string]$Label = (Get-Date -Format 'yyyyMMdd-HHmmss'),
    [string]$Worktree = 'C:\Users\Dickson\Desktop\projects\verbatim-wt-scratch',
    [string]$TargetDir = 'C:\Users\Dickson\Desktop\projects\verbatim-target-scratch',
    [string]$OutRoot = 'C:\Users\Dickson\Desktop\projects\rework-runs',
    [string]$WxDir = 'C:\Users\Dickson\Desktop\projects\verbatim\target\wxwidgets',
    [int]$Port = 44001,
    [int]$UntilHookIns = 0,
    [switch]$Record
)

$ErrorActionPreference = 'Stop'
# At most 20 runs of any scenario in a measurement (Dickson, 2026-10-29).
$Runs = [Math]::Min($Runs, 20)
$defaultLog = 'info,verbatim_outpost=debug,verbatim_uia::subscribe=debug,verbatim_uia::com=debug,verbatim_ia2::hook=debug'

$env:CARGO_TARGET_DIR = $TargetDir
$env:VERBATIM_WX_DIR = $WxDir
$env:VERBATIM_E2E_ENDPOINT = "127.0.0.1:$Port"
$env:VERBATIM_E2E_VERBATIM_EXE = Join-Path $TargetDir 'debug\verbatim.exe'
$env:VERBATIM_E2E_RUST_LOG = "$defaultLog,rework_measure=debug"
if ($Record) { Remove-Item Env:\VERBATIM_E2E_RECORD -ErrorAction SilentlyContinue } else { $env:VERBATIM_E2E_RECORD = '0' }

$batch = Join-Path $OutRoot $Label
New-Item -ItemType Directory -Force $batch | Out-Null
$log = Join-Path $batch 'batch.log'
function Say([string]$text) {
    $line = "$(Get-Date -Format 'HH:mm:ss') $text"
    Write-Output $line
    Add-Content -Path $log -Value $line -Encoding utf8
}

function Test-Agent([System.Diagnostics.Process]$agent) {
    if ($agent.HasExited) { return "the agent exited with code $($agent.ExitCode)" }
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1', $Port)
        return $null
    } catch {
        return "the agent accepts no connection on port ${Port}: $($_.Exception.Message)"
    } finally {
        $client.Close()
    }
}

Push-Location $Worktree
try {
    Say "building the scratch worktree $Worktree into $TargetDir"
    & cargo build -j 4 -p verbatim-app -p verbatim-outpost -p verbatim-synth-host -p mockapp -p verbatim-agent
    if ($LASTEXITCODE -ne 0) { throw "the build failed ($LASTEXITCODE)" }
    & cargo test -j 4 -p verbatim-e2e --test e2e --no-run
    if ($LASTEXITCODE -ne 0) { throw "the end-to-end suite's build failed ($LASTEXITCODE)" }

    $listening = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if ($listening) { throw "port $Port is already listening (process $($listening[0].OwningProcess)); stop that agent first" }
    $agentExe = Join-Path $TargetDir 'debug\verbatim-agent.exe'
    $agent = Start-Process -FilePath $agentExe -ArgumentList '--bind-address', '127.0.0.1', '--port', "$Port" `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $batch 'agent-stdout.log') `
        -RedirectStandardError (Join-Path $batch 'agent-stderr.log')
    $deadline = (Get-Date).AddSeconds(30)
    while ((Test-Agent $agent) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    $problem = Test-Agent $agent
    if ($problem) { throw "the agent did not start: $problem" }
    Say "agent $($agent.Id) listening on port $Port"

    try {
        foreach ($scenario in $Scenarios) {
            $folder = Join-Path $batch $scenario
            New-Item -ItemType Directory -Force $folder | Out-Null
            $existing = @(Get-ChildItem -Directory $folder | Where-Object { $_.Name -match '^\d+$' })
            $next = 1
            if ($existing.Count -gt 0) { $next = ($existing | ForEach-Object { [int]$_.Name } | Measure-Object -Maximum).Maximum + 1 }
            for ($run = 1; $run -le $Runs; $run++) {
                if ($UntilHookIns -gt 0) {
                    $hookIns = @(Get-ChildItem -Recurse -File -Path $folder -Filter 'outpost-*.log' -ErrorAction SilentlyContinue |
                        Select-String -SimpleMatch 'what="hook_in"' -ErrorAction SilentlyContinue).Count
                    if ($hookIns -ge $UntilHookIns) {
                        Say "$scenario has $hookIns outpost starts; stopping it"
                        break
                    }
                }
                $number = '{0:D2}' -f $next
                $next++
                $dest = Join-Path $folder $number
                New-Item -ItemType Directory -Force $dest | Out-Null
                Say "$scenario run $number starting"
                $output = Join-Path $dest 'cargo-test.log'
                $exitCode = 0
                $previous = $ErrorActionPreference
                $ErrorActionPreference = 'Continue'
                & cargo test -p verbatim-e2e --test e2e -- --ignored --exact $scenario --test-threads=1 *> $output
                $exitCode = $LASTEXITCODE
                $ErrorActionPreference = $previous
                $artifacts = Join-Path $Worktree "target\e2e-artifacts\$scenario"
                if (Test-Path $artifacts) {
                    Get-ChildItem -File $artifacts | Where-Object { $_.Extension -ne '.mp4' } |
                        Copy-Item -Destination $dest
                } else {
                    Say "$scenario run ${number}: no artifacts folder at $artifacts"
                }
                $result = if ($exitCode -eq 0) { 'pass' } else { 'fail' }
                Set-Content -Path (Join-Path $dest 'result.txt') -Value "$result exit=$exitCode" -Encoding utf8
                Say "$scenario run $number $result (exit $exitCode)"
                $problem = Test-Agent $agent
                if ($problem) {
                    Set-Content -Path (Join-Path $batch 'stopped.txt') -Value "stopped after $scenario run ${number}: $problem" -Encoding utf8
                    Say "stopping: $problem"
                    exit 2
                }
            }
        }
        Say 'batch done'
    } finally {
        if (-not $agent.HasExited) {
            Stop-Process -Id $agent.Id -Force
            Say "agent $($agent.Id) stopped"
        }
    }
} finally {
    Pop-Location
}
