# Scratch out-of-context WinEvent hook for the focus pipeline rework's
# by-hand captures (docs/design/focus-pipeline-rework.md, "Order of work",
# step 1), never merged. Records every event from -EventMin to -EventMax
# (default: the one event -EventMin) for -Seconds, one line each to -Out:
# the time in microseconds since the Unix epoch, the event, the window, its
# class and InternalGetWindowText, the object and child ids, the raising
# process and thread, and whether the window is GetDesktopWindow(). With
# -DesktopOnly only events whose window is GetDesktopWindow() are written.
# For the desktop window it also reads its client object's MSAA accName.
# With -ProcessId the hook is scoped to that process.
param(
    [Parameter(Mandatory)] [string] $Out,
    [int] $EventMin = 0x8005,
    [int] $EventMax = 0,
    [int] $Seconds = 30,
    [int] $ProcessId = 0,
    [switch] $DesktopOnly
)
if ($EventMax -eq 0) { $EventMax = $EventMin }
Add-Type -AssemblyName Accessibility
Add-Type -ReferencedAssemblies Accessibility @"
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
public static class Hook {
    delegate void Proc(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time);
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, Proc proc, uint pid, uint tid, uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll")] static extern IntPtr GetDesktopWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll")] static extern uint GetProcessIdOfThread(IntPtr thread);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("user32.dll")] static extern int MsgWaitForMultipleObjects(uint n, IntPtr handles, bool all, uint ms, uint mask);
    [DllImport("user32.dll")] static extern bool PeekMessage(out MSG msg, IntPtr h, uint min, uint max, uint remove);
    [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG msg);
    [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG msg);
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public int x; public int y; }
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object acc);
    static string AccName(IntPtr h) {
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object o;
        if (AccessibleObjectFromWindow(h, 0xFFFFFFFC, ref iid, out o) != 0) return "<failed>";
        try { return ((Accessibility.IAccessible)o).get_accName(0); } catch (Exception e) { return "<" + e.GetType().Name + ">"; }
    }
    static StreamWriter writer;
    static bool desktopOnly;
    static IntPtr desktop;
    static Proc keep;
    static long Now() { return (DateTime.UtcNow - new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc)).Ticks / 10; }
    static string ProcessOfThread(uint tid) {
        IntPtr t = OpenThread(0x0800, false, tid);
        if (t == IntPtr.Zero) return "thread " + tid + " (process unknown)";
        uint pid = GetProcessIdOfThread(t);
        CloseHandle(t);
        string name = "?";
        try { name = Process.GetProcessById((int)pid).ProcessName; } catch { }
        return name + " (" + pid + ") thread " + tid;
    }
    static void Callback(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time) {
        bool isDesktop = hwnd == desktop;
        if (desktopOnly && !isDesktop) return;
        var cls = new StringBuilder(256);
        var text = new StringBuilder(512);
        GetClassName(hwnd, cls, 256);
        InternalGetWindowText(hwnd, text, 512);
        uint owner;
        GetWindowThreadProcessId(hwnd, out owner);
        writer.WriteLine(Now() + " event=0x" + ev.ToString("X4") + " hwnd=" + hwnd + " class=" + cls + " text='" + text + "' obj=" + obj + " child=" + child
            + " raised by " + ProcessOfThread(thread) + " window process " + owner + " desktop=" + isDesktop + " tick=" + time + (isDesktop ? " client accName='" + AccName(hwnd) + "'" : ""));
        writer.Flush();
    }
    public static void Run(string path, uint min, uint max, int seconds, uint pid, bool onlyDesktop) {
        writer = new StreamWriter(path, true);
        desktopOnly = onlyDesktop;
        desktop = GetDesktopWindow();
        keep = Callback;
        IntPtr h = SetWinEventHook(min, max, IntPtr.Zero, keep, pid, 0, 0x0002);
        writer.WriteLine(Now() + " hook " + h + " events 0x" + min.ToString("X4") + "-0x" + max.ToString("X4") + " process " + pid + " desktop window " + desktop);
        writer.Flush();
        var end = DateTime.UtcNow.AddSeconds(seconds);
        while (DateTime.UtcNow < end) {
            MsgWaitForMultipleObjects(0, IntPtr.Zero, false, 100, 0x04FF);
            MSG m;
            while (PeekMessage(out m, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref m); DispatchMessage(ref m); }
        }
        UnhookWinEvent(h);
        writer.WriteLine(Now() + " end");
        writer.Close();
    }
}
"@
[Hook]::Run($Out, [uint32]$EventMin, [uint32]$EventMax, $Seconds, [uint32]$ProcessId, $DesktopOnly.IsPresent)
