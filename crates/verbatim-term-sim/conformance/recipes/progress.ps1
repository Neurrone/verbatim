# Progress as programs draw it, for the conformance corpus's "progress"
# recording: a carriage-return bar, several rows rewritten above the cursor
# as docker pull draws its layers, a spinner with a message, and an elapsed
# timer ticking once a second.
$e = [char]27
$o = [Console]::Out
$o.Write("downloading`r`n")
for ($p = 0; $p -le 100; $p += 10) {
    $o.Write("`r[" + ('#' * ($p / 10)) + (' ' * (10 - $p / 10)) + "] $p%")
    Start-Sleep -Milliseconds 120
}
$o.Write("`r`n")
$o.Write("layer1: waiting`r`nlayer2: waiting`r`nlayer3: waiting`r`n")
foreach ($step in 1..5) {
    foreach ($layer in 1..3) {
        $up = 4 - $layer
        $o.Write("$e[$($up)A`r$e[2Klayer$($layer): $($step * 20)%$e[$($up)B`r")
        Start-Sleep -Milliseconds 60
    }
}
$o.Write("$e[3A`r$e[2Klayer1: Pull complete$e[3B`r")
$frames = [char[]]'|/-\'
foreach ($tick in 0..11) {
    $o.Write("`r$e[2K$($frames[$tick % 4]) Working... $([int]($tick / 4))s")
    Start-Sleep -Milliseconds 250
}
$o.Write("`r$e[2KDone in 3s`r`n")
foreach ($second in 1..3) {
    $o.Write("`rElapsed $($second)s")
    Start-Sleep -Milliseconds 1000
}
$o.Write("`r`n")
