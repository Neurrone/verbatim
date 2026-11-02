# One real application: throughput, a slow call beside a cheap one, and its
# UI thread suspended with plain calls and remote operations beside it.
# Usage: run-real.ps1 -Exe <puia.exe> -Out <folder> -Label <name> -Hwnd <window>
#   [-Other <a window of the same process on another UI thread>] [-Ops <bench ops>] [-NoSuspend]
param([string]$Exe, [string]$Out, [string]$Label, [string]$Hwnd, [string]$Other = '',
    [string]$Ops = 'name,findall,fromhandle,text,msaa,rop', [switch]$NoSuspend)
New-Item -ItemType Directory -Force $Out | Out-Null
$f = Join-Path $Out "$Label.txt"
& $Exe bench label=$Label hwnd=$Hwnd ops=$Ops k=name:64,findall:8,fromhandle:32,text:32,msaa:64,rop:32 reps=20 *> $f
& $Exe pair label=$Label-findall-beside-name hwnd_a=$Hwnd op_a=findall hwnd_b=$Hwnd op_b=name elem_b=3 iters_a=10 *>> $f
& $Exe pair label=$Label-findall-beside-rop hwnd_a=$Hwnd op_a=findall hwnd_b=$Hwnd op_b=rop elem_b=3 iters_a=10 *>> $f
if ($NoSuspend) { return }
$probes = @("probe=other-element:${Hwnd}:name:3", "probe=other-element-rop:${Hwnd}:rop:3", "probe=same-window-new-connection:${Hwnd}:fromhandle",
    "xprobe=other-element:${Hwnd}:name:3", "xprobe=other-element-rop:${Hwnd}:rop:3")
if ($Other) {
    $probes += @("probe=other-thread-window:${Other}:name:3", "probe=other-thread-window-rop:${Other}:rop:3",
        "probe=other-thread-window-new-connection:${Other}:fromhandle", "xprobe=other-thread-window:${Other}:name:3")
}
& $Exe hung label=$Label-suspended-name "block=suspend:$Hwnd" "stuck=${Hwnd}:name:1" @probes observe_ms=4000 trials=2 *>> $f
& $Exe hung label=$Label-suspended-rop "block=suspend:$Hwnd" "stuck=${Hwnd}:rop:1" @probes observe_ms=4000 trials=2 *>> $f
