# Scratch timer for the focus pipeline rework's check of the desktop
# window's name (docs/design/focus-pipeline-rework.md, difference 23),
# never merged. -Count times, -IntervalMs apart, it reads the MSAA accName
# of GetDesktopWindow()'s client object (AccessibleObjectFromWindow with
# OBJID_CLIENT, then accName), each read on a fresh object, and writes one
# line per read: the time in microseconds since the Unix epoch, the
# foreground window with its class, title and IsHungAppWindow, the time
# AccessibleObjectFromWindow took and the time accName took, both in
# microseconds, and the name.
param([int] $Count = 20, [int] $IntervalMs = 250)
Add-Type -AssemblyName Accessibility
Add-Type -ReferencedAssemblies Accessibility @"
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
public static class DesktopName {
    [DllImport("user32.dll")] static extern IntPtr GetDesktopWindow();
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool IsHungAppWindow(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long t);
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object acc);
    static long Now() { long t; GetSystemTimePreciseAsFileTime(out t); return (t - 116444736000000000L) / 10; }
    static double Us(long ticks) { return ticks * 1000000.0 / Stopwatch.Frequency; }
    public static string Read() {
        IntPtr fg = GetForegroundWindow();
        var cls = new StringBuilder(256); var text = new StringBuilder(512);
        GetClassName(fg, cls, 256); InternalGetWindowText(fg, text, 512);
        bool hung = IsHungAppWindow(fg);
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object o;
        long t0 = Stopwatch.GetTimestamp();
        int hr = AccessibleObjectFromWindow(GetDesktopWindow(), 0xFFFFFFFC, ref iid, out o);
        long t1 = Stopwatch.GetTimestamp();
        string name;
        if (hr != 0) name = "<failed 0x" + hr.ToString("X8") + ">";
        else { try { name = "'" + ((Accessibility.IAccessible)o).get_accName(0) + "'"; } catch (Exception e) { name = "<" + e.GetType().Name + ">"; } }
        long t2 = Stopwatch.GetTimestamp();
        if (o != null) Marshal.ReleaseComObject(o);
        return Now() + " foreground " + fg + " class=" + cls + " text='" + text + "' hung=" + hung
            + " AccessibleObjectFromWindow " + Us(t1 - t0).ToString("F1") + " us accName " + Us(t2 - t1).ToString("F1") + " us name=" + name;
    }
}
"@
for ($i = 0; $i -lt $Count; $i++) { [DesktopName]::Read(); Start-Sleep -Milliseconds $IntervalMs }
