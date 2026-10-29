# Scratch reader for the by-hand captures (docs/design/focus-pipeline-
# rework.md, "Order of work", step 1, difference 21), never merged. For each
# visible top-level window of process -ProcessId whose class is
# -DropDownClass (a combo box's drop-down list, ComboLBox), prints the
# window's owner, parent and root owner, then walks its MSAA accParent chain
# from its client object (OBJID_CLIENT) to the end, each entry's role, name
# and WindowFromAccessibleObject, and its UIA raw-view parent chain
# (System.Windows.Automation, TreeWalker.RawViewWalker) from
# AutomationElement.FromHandle to the root, each entry's control type, name,
# class and native window handle.
param(
    [Parameter(Mandatory)] [int] $ProcessId,
    [string] $DropDownClass = 'ComboLBox'
)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName Accessibility
Add-Type -ReferencedAssemblies Accessibility @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text; using Accessibility;
public static class Acc {
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object acc);
    [DllImport("oleacc.dll")] static extern int WindowFromAccessibleObject(IAccessible acc, out IntPtr h);
    [DllImport("oleacc.dll", CharSet = CharSet.Unicode)] static extern uint GetRoleText(uint role, StringBuilder s, uint n);
    public delegate bool P(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(P p, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll")] static extern IntPtr GetParent(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
    public static string Describe(IntPtr h) {
        var c = new StringBuilder(256); var t = new StringBuilder(256);
        GetClassName(h, c, 256); InternalGetWindowText(h, t, 256);
        return h + " (" + c + " '" + t + "')";
    }
    public static List<IntPtr> Find(uint pid, string cls) {
        var o = new List<IntPtr>();
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && c.ToString() == cls) o.Add(h); return true; }, IntPtr.Zero);
        return o;
    }
    public static string Windows(IntPtr h) {
        return "owner " + Describe(GetWindow(h, 4)) + "; GetParent " + Describe(GetParent(h)) + "; GA_PARENT " + Describe(GetAncestor(h, 1))
            + "; GA_ROOTOWNER " + Describe(GetAncestor(h, 3)) + "; desktop " + Describe(GetDesktopWindow());
    }
    public static List<string> Chain(IntPtr h) {
        var lines = new List<string>();
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71");
        object o;
        int hr = AccessibleObjectFromWindow(h, 0xFFFFFFFC, ref iid, out o);
        if (hr != 0) { lines.Add("AccessibleObjectFromWindow failed 0x" + hr.ToString("X8")); return lines; }
        var acc = (IAccessible)o;
        for (int depth = 0; acc != null && depth < 20; depth++) {
            string name = null, role = "?"; IntPtr w = IntPtr.Zero;
            try { name = acc.get_accName(0); } catch (Exception e) { name = "<" + e.GetType().Name + ">"; }
            try { object r = acc.get_accRole(0); if (r is int) { var s = new StringBuilder(128); GetRoleText((uint)(int)r, s, 128); role = s.ToString(); } else role = "" + r; } catch { }
            WindowFromAccessibleObject(acc, out w);
            lines.Add(depth + ": role=" + role + " name='" + name + "' window " + Describe(w));
            object p = null;
            try { p = acc.accParent; } catch (Exception e) { lines.Add("accParent failed: " + e.Message); }
            acc = p as IAccessible;
        }
        lines.Add("end of accParent chain");
        return lines;
    }
}
"@
foreach ($h in [Acc]::Find([uint32]$ProcessId, $DropDownClass)) {
    "drop-down window $([Acc]::Describe($h))"
    "  $([Acc]::Windows($h))"
    "  MSAA accParent chain from OBJID_CLIENT:"
    [Acc]::Chain($h) | ForEach-Object { "    $_" }
    "  UIA raw-view parent chain:"
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $element = [System.Windows.Automation.AutomationElement]::FromHandle($h)
    $depth = 0
    while ($null -ne $element -and $depth -lt 20) {
        $c = $element.Current
        "    ${depth}: $($c.ControlType.ProgrammaticName) name='$($c.Name)' class=$($c.ClassName) window $($c.NativeWindowHandle) ($([Acc]::Describe([IntPtr]$c.NativeWindowHandle)))"
        $element = $walker.GetParent($element)
        $depth++
    }
    "    end of raw-view chain"
}
