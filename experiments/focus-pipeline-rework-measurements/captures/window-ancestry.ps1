# Scratch reader for the by-hand captures, never merged: for window -Window,
# prints it and each GetAncestor(GA_PARENT) step to the desktop, each with
# its class, InternalGetWindowText, and its client object's (OBJID_CLIENT)
# MSAA role and name.
param([Parameter(Mandatory)] [long] $Window)
Add-Type -AssemblyName Accessibility
Add-Type -ReferencedAssemblies Accessibility @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class Anc {
    [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object acc);
    [DllImport("oleacc.dll", CharSet = CharSet.Unicode)] static extern uint GetRoleText(uint role, StringBuilder s, uint n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int InternalGetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    public static string Line(IntPtr h) {
        var c = new StringBuilder(256); var t = new StringBuilder(256);
        GetClassName(h, c, 256); InternalGetWindowText(h, t, 256);
        string role = "?", name = "?";
        Guid iid = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71"); object o;
        if (AccessibleObjectFromWindow(h, 0xFFFFFFFC, ref iid, out o) == 0) {
            var a = (Accessibility.IAccessible)o;
            try { object r = a.get_accRole(0); if (r is int) { var s = new StringBuilder(128); GetRoleText((uint)(int)r, s, 128); role = s.ToString(); } } catch { }
            try { name = a.get_accName(0); } catch { }
        }
        return h + " class=" + c + " text='" + t + "' client role=" + role + " name='" + name + "'";
    }
    public static IntPtr Parent(IntPtr h) { return GetAncestor(h, 1); }
}
"@
$h = [IntPtr]$Window
while ($h -ne [IntPtr]::Zero) { [Anc]::Line($h); $p = [Anc]::Parent($h); if ($p -eq $h) { break }; $h = $p }
