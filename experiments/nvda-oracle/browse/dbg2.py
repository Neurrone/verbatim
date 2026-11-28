import sys, os, ctypes
from ctypes import byref, c_void_p, c_long, POINTER
from ctypes import wintypes as w
sys.path.insert(0, sys.argv[1])
import expA as E   # reuses window class / wndproc helpers
import ia2standins as S
from comtypes.automation import VARIANT, IDispatch
import comtypes
u32 = E.u32
@E.WNDPROC
def wndproc(hwnd, msg, wp, lp):
    if msg == E.WM_GETOBJECT and ctypes.c_long(lp & 0xFFFFFFFF).value == E.OBJID_CLIENT:
        pacc = E.state["doc"].com.QueryInterface(S.Acc.IAccessible)
        return E.oleacc.LresultFromObject(byref(S.Acc.IAccessible._iid_), wp, ctypes.cast(pacc, c_void_p))
    return u32.DefWindowProcW(hwnd, msg, wp, lp)
wc = E.WNDCLASSW(lpfnWndProc=wndproc, lpszClassName="T2")
u32.RegisterClassW(byref(wc))
hwnd = u32.CreateWindowExW(0, "T2", "t", 0x80000000, 0,0,10,10, None, None, None, None)
world = S.World(hwnd); doc = E.build_page(); doc.attach(world); E.state["doc"] = doc
oa = ctypes.oledll.oleacc
for uid in (-1, -5):
    p = POINTER(S.Acc.IAccessible)(); v = VARIANT()
    oa.AccessibleObjectFromEvent(w.HWND(hwnd), c_long(-4), c_long(uid), byref(p), byref(v))
    print("AOFE", uid, "->", v.vt, v.value)
# direct call on doc through comtypes client
d = doc.com.QueryInterface(S.Acc.IAccessible)
c = d.accChild(-5)
print("direct accChild(-5):", c)
