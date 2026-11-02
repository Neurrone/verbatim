$script:Sym = "srv*$PSScriptRoot\..\..\target\symbols*https://msdl.microsoft.com/download/symbols"
Add-Type -Namespace W -Name K -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError=true)] public static extern System.IntPtr OpenProcess(uint a, bool i, int pid);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool DebugBreakProcess(System.IntPtr h);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr h);
'@

# Invasive attach with breakpoints; the commands end in g so the target runs on.
function Start-Cdb([int]$TargetPid, [string]$CmdFile, [string]$Log, [string]$ExtraSym = '') {
    $y = $script:Sym; if ($ExtraSym) { $y = "$y;$ExtraSym" }
    $psi = New-Object System.Diagnostics.ProcessStartInfo 'cdbX64.exe'
    $psi.Arguments = "-p $TargetPid -y `"$y`" -logo `"$Log`" -cf `"$CmdFile`""
    $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $null = $p.StandardOutput.ReadToEndAsync(); $null = $p.StandardError.ReadToEndAsync()
    # Wait until the target runs again: the log shows the last command's g.
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        if ((Test-Path $Log) -and (Select-String -Path $Log -Pattern 'READY_MARK' -Quiet)) { break }
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Milliseconds 200
    [pscustomobject]@{ Proc = $p; Target = $TargetPid; Log = $Log }
}

# Breaks in with DebugBreakProcess and detaches, leaving the target running.
function Stop-Cdb($c, [string]$Final = '') {
    $h = [W.K]::OpenProcess(0x1F0FFF, $false, $c.Target)
    [W.K]::DebugBreakProcess($h) | Out-Null
    [W.K]::CloseHandle($h) | Out-Null
    Start-Sleep -Milliseconds 300
    
    # The first line carries the writer's byte order mark, which cdb rejects.
    $c.Proc.StandardInput.WriteLine('')
    $c.Proc.StandardInput.WriteLine('qd')
    $c.Proc.StandardInput.Flush()
    if (-not $c.Proc.WaitForExit(30000)) { Write-Output "cdb did not exit"; $c.Proc.Kill() }
}

# A puia probe in its own process: makes one call at start, then one per Invoke-Probe.
function Start-Probe([string]$Exe, [string]$ProbeArgs) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo $Exe, "probe $ProbeArgs"
    $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $l = $p.StandardOutput.ReadLine()
    if ($l -notlike 'ready*') { throw "probe: $l" }
    $p
}
function Invoke-Probe($p) { $p.StandardInput.WriteLine('go'); $p.StandardInput.Flush(); $p.StandardOutput.ReadLine() }
function Stop-Probe($p) { $p.StandardInput.Close(); if (-not $p.WaitForExit(5000)) { $p.Kill() } }

Export-ModuleMember -Function *
