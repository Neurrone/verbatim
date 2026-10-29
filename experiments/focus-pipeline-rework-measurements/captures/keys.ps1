# Scratch key presser for the by-hand captures, never merged: presses each
# step with SendInput, for keys that move the foreground off the capture's
# window (a new virtual desktop, Task View), which cargo xtask nvda capture
# refuses to press. A step is a plus-joined combination of key names
# (win, ctrl, shift, alt, tab, enter, escape, left, right, up, down, f2,
# home, end, apps, space, d, f4, or one letter or digit), "sleep:<ms>", "type:<text>",
# "focus" (prints the UIA focused element), "seek:<name>" (presses Down
# until the focused element has that name, then Enter), or "touch:<path>". Each step is
# logged with the time in microseconds since the Unix epoch.
param([Parameter(ValueFromRemainingArguments)] [string[]] $Steps)
Add-Type -AssemblyName UIAutomationClient
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Keys {
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort vk; public ushort scan; public uint flags; public uint time; public IntPtr extra; }
    [StructLayout(LayoutKind.Explicit, Size = 40)] struct INPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    static INPUT Key(ushort vk, bool up, bool unicode, ushort ch) {
        var i = new INPUT(); i.type = 1; i.ki.vk = unicode ? (ushort)0 : vk; i.ki.scan = unicode ? ch : (ushort)0;
        uint f = 0; if (up) f |= 2; if (unicode) f |= 4;
        if (!unicode && (vk == 0x25 || vk == 0x26 || vk == 0x27 || vk == 0x28 || vk == 0x24 || vk == 0x23 || vk == 0x5B || vk == 0x5D)) f |= 1;
        i.ki.flags = f; return i;
    }
    public static uint Combo(ushort[] vks) {
        var list = new System.Collections.Generic.List<INPUT>();
        foreach (var vk in vks) list.Add(Key(vk, false, false, 0));
        for (int k = vks.Length - 1; k >= 0; k--) list.Add(Key(vks[k], true, false, 0));
        return SendInput((uint)list.Count, list.ToArray(), Marshal.SizeOf(typeof(INPUT)));
    }
    public static uint Type(string s) {
        var list = new System.Collections.Generic.List<INPUT>();
        foreach (char c in s) { list.Add(Key(0, false, true, c)); list.Add(Key(0, true, true, c)); }
        return SendInput((uint)list.Count, list.ToArray(), Marshal.SizeOf(typeof(INPUT)));
    }
}
"@
$names = @{ win = 0x5B; ctrl = 0x11; shift = 0x10; alt = 0x12; tab = 0x09; enter = 0x0D; escape = 0x1B; left = 0x25; up = 0x26; right = 0x27; down = 0x28; f2 = 0x71; f4 = 0x73; home = 0x24; end = 0x23; apps = 0x5D; space = 0x20 }
function Now { [long](([DateTime]::UtcNow.Ticks - 621355968000000000) / 10) }
foreach ($step in $Steps) {
    if ($step -like 'sleep:*') { Start-Sleep -Milliseconds ([int]$step.Substring(6)); continue }
    if ($step -like 'touch:*') { [IO.File]::WriteAllText($step.Substring(6), ''); "$(Now) touch $($step.Substring(6))"; continue }
    if ($step -like 'type:*') { [void][Keys]::Type($step.Substring(5)); "$(Now) type $($step.Substring(5))"; continue }
    if ($step -like 'seek:*') {
        # Presses Down until the UIA focus is named the text, then Enter.
        $want = $step.Substring(5)
        for ($i = 0; $i -lt 12; $i++) {
            $f = [System.Windows.Automation.AutomationElement]::FocusedElement
            $name = if ($null -ne $f) { $f.Current.Name } else { '' }
            "$(Now) seek: focus '$name'"
            if ($name -eq $want) { [void][Keys]::Combo([uint16[]]@(0x0D)); "$(Now) key enter on '$want'"; break }
            [void][Keys]::Combo([uint16[]]@(0x28)); Start-Sleep -Milliseconds 400
        }
        continue
    }
    if ($step -eq 'focus') {
        $f = [System.Windows.Automation.AutomationElement]::FocusedElement
        if ($null -eq $f) { "$(Now) focus: none" } else { $c = $f.Current; "$(Now) focus: $($c.ControlType.ProgrammaticName) name='$($c.Name)' class=$($c.ClassName) window $($c.NativeWindowHandle) process $($c.ProcessId)" }
        continue
    }
    $vks = foreach ($part in $step.Split('+')) {
        if ($names.ContainsKey($part)) { [uint16]$names[$part] } elseif ($part.Length -eq 1) { [uint16][char]$part.ToUpper() } else { throw "unknown key $part" }
    }
    $sent = [Keys]::Combo([uint16[]]@($vks))
    "$(Now) key $step (sent $sent inputs)"
}
