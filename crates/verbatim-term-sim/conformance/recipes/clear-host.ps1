# Windows PowerShell 5.1's Clear-Host after more than three screens of
# output, for the conformance corpus's "clear-host" recording: what `cls`
# writes to a terminal to erase its screen and its history, and a line
# printed after it.
foreach ($n in 1..100) { "before $n" }
Start-Sleep -Milliseconds 300
Clear-Host
Start-Sleep -Milliseconds 300
'after the clear'
