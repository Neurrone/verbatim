# mockapp: throughput on both backends, and a held or stalled window thread
# with plain calls and remote operations on either side.
# Usage: run-mockapp.ps1 -Exe <puia.exe> -Mock <mockapp.exe> -Fixtures <crates\mockapp\tests\fixtures> -Out <folder>
param([string]$Exe, [string]$Mock, [string]$Fixtures, [string]$Out)
New-Item -ItemType Directory -Force $Out | Out-Null
$tree = Join-Path $Fixtures 'tree.json'
$text = Join-Path $Fixtures 'text.json'

$f = Join-Path $Out 'mockapp-bench.txt'
& $Exe bench label=mockapp-uia mockapp=$Mock fixture=$tree backend=uia ops=name,findall,fromhandle,rop k=name:64,findall:8,fromhandle:32,rop:32 reps=20 *> $f
& $Exe bench label=mockapp-uia-text mockapp=$Mock fixture=$text backend=uia ops=text k=text:32 reps=20 *>> $f
& $Exe bench label=mockapp-msaa mockapp=$Mock fixture=$tree backend=msaa ops=msaa elems=0 k=msaa:64 reps=20 *>> $f

$f = Join-Path $Out 'mockapp-hung.txt'
$common = @("mockapp=$Mock", "fixture=$tree", 'backend=uia', 'child=1', 'observe_ms=4000', 'trials=3')
$probes = @('probe=other-element:main:name:3', 'probe=other-element-rop:main:rop:3', 'probe=child-window-new-connection:child:fromhandle',
    'xprobe=other-element:main:name:3', 'xprobe=other-element-rop:main:rop:3')
& $Exe hung label=mockapp-hold-name title=puia-mock-1 block=mock-hold stuck=main:name:1 @common @probes *> $f
& $Exe hung label=mockapp-hold-rop title=puia-mock-2 block=mock-hold stuck=main:rop:1 @common @probes *>> $f
& $Exe hung label=mockapp-stall-rop title=puia-mock-3 block=mock-stall:3000 stuck=main:rop:1 @common @probes *>> $f
& $Exe hung label=mockapp-msaa-hold title=puia-mock-4 "mockapp=$Mock" "fixture=$tree" backend=msaa observe_ms=4000 trials=3 block=mock-hold stuck=main:msaa 'probe=same-object-msaa:main:msaa' 'probe=new-msaa-object:main:msaanew' 'xprobe=same-object-msaa:main:msaa' elems=0 *>> $f
