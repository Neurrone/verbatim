# A slow call A into one window while a cheap call B goes to another
# element of the same window (so the same UI thread), from another thread
# of this process and from another process: A a FindAll with a cache
# request (many round trips), one long remote operation, or one read of the
# whole document's text; B a name read or a remote operation reading it.
# Usage: run-pairs.ps1 -Exe <puia.exe> -Out <folder> -Label <name> -Hwnd <window> [-As findall,ropslow,textall]
param([string]$Exe, [string]$Out, [string]$Label, [string]$Hwnd, [string[]]$As = @('findall', 'ropslow', 'textall'))
New-Item -ItemType Directory -Force $Out | Out-Null
$f = Join-Path $Out "$Label-pairs.txt"
foreach ($opa in $As) {
    foreach ($opb in 'name', 'rop') {
        foreach ($xb in 0, 1) {
            & $Exe pair label=$Label-$opa-beside-$opb-xb$xb hwnd_a=$Hwnd op_a=$opa elem_a=3 hwnd_b=$Hwnd op_b=$opb elem_b=5 iters_a=10 xb=$xb *>> $f
        }
    }
}
