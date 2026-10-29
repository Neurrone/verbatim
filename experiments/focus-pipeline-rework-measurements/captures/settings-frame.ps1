# Scratch reader for the Settings frame capture of the focus pipeline
# rework (docs/design/focus-pipeline-rework.md, "Order of work", the
# readers' walks), never merged. For the foreground window and each
# window from the UIA focus's window up to it, prints the class, the
# serving process, InternalGetWindowText, and the MSAA name and role of
# its OBJID_WINDOW and OBJID_CLIENT objects; then the UIA raw-view
# ancestry of the focused element, each with its control type, name,
# process and window handle.
Add-Type -AssemblyName Accessibility
Add-Type -AssemblyName UIAutomationClient
Add-Type -ReferencedAssemblies Accessibility @"
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
public static class Frame {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object acc);
    [DllImport("oleacc.dll", CharSet = CharSet.Unicode)] static extern uint GetRoleText(uint role, StringBuilder s, uint n);
    static string Msaa(IntPtr h, uint id) {
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object o;
        int hr = AccessibleObjectFromWindow(h, id, ref iid, out o);
        if (hr != 0) return "<failed 0x" + hr.ToString("X8") + ">";
        var a = (Accessibility.IAccessible)o;
        string name, role;
        try { name = "'" + a.get_accName(0) + "'"; } catch (Exception e) { name = "<" + e.GetType().Name + ">"; }
        try {
            object r = a.get_accRole(0);
            if (r is int) { var s = new StringBuilder(128); GetRoleText((uint)(int)r, s, 128); role = (int)r + " (" + s + ")"; } else role = "'" + r + "'";
        } catch (Exception e) { role = "<" + e.GetType().Name + ">"; }
        return "name=" + name + " role=" + role;
    }
    public static string Describe(IntPtr h) {
        var cls = new StringBuilder(256); var text = new StringBuilder(512);
        GetClassName(h, cls, 256); InternalGetWindowText(h, text, 512);
        uint pid; GetWindowThreadProcessId(h, out pid);
        string proc = "?"; try { proc = Process.GetProcessById((int)pid).ProcessName; } catch { }
        return "window " + h + " class=" + cls + " process=" + proc + " (" + pid + ") InternalGetWindowText='" + text + "'\n"
            + "    OBJID_WINDOW " + Msaa(h, 0) + "\n    OBJID_CLIENT " + Msaa(h, 0xFFFFFFFC);
    }
}
"@
$fg = [Frame]::GetForegroundWindow()
"foreground:"
[Frame]::Describe($fg)
$focus = [System.Windows.Automation.AutomationElement]::FocusedElement
$h = [IntPtr]$focus.Current.NativeWindowHandle
if ($h -eq [IntPtr]::Zero) {
    $w = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $e = $focus
    while ($null -ne $e -and $e.Current.NativeWindowHandle -eq 0) { $e = $w.GetParent($e) }
    if ($null -ne $e) { $h = [IntPtr]$e.Current.NativeWindowHandle }
}
"windows from the focus's nearest window up to the desktop:"
$desktop = [Frame]::GetDesktopWindow()
while ($h -ne [IntPtr]::Zero -and $h -ne $desktop) { [Frame]::Describe($h); $h = [Frame]::GetAncestor($h, 1) }
"UIA raw-view ancestry of the focus:"
$w = [System.Windows.Automation.TreeWalker]::RawViewWalker
$e = $focus
while ($null -ne $e) {
    $c = $e.Current
    "  $($c.ControlType.ProgrammaticName) name='$($c.Name)' class=$($c.ClassName) process $($c.ProcessId) window $($c.NativeWindowHandle)"
    $e = $w.GetParent($e)
}
