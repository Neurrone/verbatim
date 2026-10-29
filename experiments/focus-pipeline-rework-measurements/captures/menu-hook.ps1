# Scratch out-of-context WinEvent hook for the focus pipeline rework's
# capture of a classic Win32 context menu (docs/design/focus-pipeline-rework.md,
# difference 17), never merged. One thread holds two hooks, one for
# EVENT_SYSTEM_FOREGROUND to EVENT_SYSTEM_MENUPOPUPEND (0x0003 to 0x0007) and
# one for EVENT_OBJECT_FOCUS (0x8005), scoped to -ProcessId (0 for every
# process), so their callbacks arrive in the order the events were queued.
# For -Seconds it writes one line per event to -Out: the time in
# microseconds since the Unix epoch from GetSystemTimePreciseAsFileTime,
# the event's own time stamp (dwmsEventTime, milliseconds), the event, the
# window, its class and InternalGetWindowText, and the object and child
# ids.
param(
    [Parameter(Mandatory)] [string] $Out,
    [int] $Seconds = 30,
    [int] $ProcessId = 0
)
Add-Type @"
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
public static class MenuHook {
    delegate void Proc(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time);
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, Proc proc, uint pid, uint tid, uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long t);
    [DllImport("user32.dll")] static extern int MsgWaitForMultipleObjects(uint n, IntPtr handles, bool all, uint ms, uint mask);
    [DllImport("user32.dll")] static extern bool PeekMessage(out MSG msg, IntPtr h, uint min, uint max, uint remove);
    [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG msg);
    [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG msg);
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public int x; public int y; }
    static StreamWriter writer;
    static Proc keep;
    static long Now() { long t; GetSystemTimePreciseAsFileTime(out t); return (t - 116444736000000000L) / 10; }
    static string Name(uint ev) {
        switch (ev) {
            case 0x0003: return "foreground";
            case 0x0004: return "menustart";
            case 0x0005: return "menuend";
            case 0x0006: return "popupstart";
            case 0x0007: return "popupend";
            case 0x8005: return "focus";
            default: return "0x" + ev.ToString("X4");
        }
    }
    static void Callback(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time) {
        long now = Now();
        var cls = new StringBuilder(256); var text = new StringBuilder(512);
        GetClassName(hwnd, cls, 256); InternalGetWindowText(hwnd, text, 512);
        writer.WriteLine(now + " tick=" + time + " " + Name(ev) + " hwnd=" + hwnd + " class=" + cls + " text='" + text + "' obj=" + obj + " child=" + child + " thread=" + thread);
        writer.Flush();
    }
    public static void Run(string path, int seconds, uint pid) {
        writer = new StreamWriter(path, true);
        keep = Callback;
        IntPtr a = SetWinEventHook(0x0003, 0x0007, IntPtr.Zero, keep, pid, 0, 0x0002);
        IntPtr b = SetWinEventHook(0x8005, 0x8005, IntPtr.Zero, keep, pid, 0, 0x0002);
        writer.WriteLine(Now() + " hooks " + a + " " + b + " process " + pid);
        writer.Flush();
        var end = DateTime.UtcNow.AddSeconds(seconds);
        while (DateTime.UtcNow < end) {
            MsgWaitForMultipleObjects(0, IntPtr.Zero, false, 100, 0x04FF);
            MSG m;
            while (PeekMessage(out m, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref m); DispatchMessage(ref m); }
        }
        UnhookWinEvent(a); UnhookWinEvent(b);
        writer.WriteLine(Now() + " end");
        writer.Close();
    }
}
"@
[MenuHook]::Run($Out, $Seconds, [uint32]$ProcessId)
