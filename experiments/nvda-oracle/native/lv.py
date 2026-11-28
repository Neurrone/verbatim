"""List view: a real, hidden SysListView32 (comctl32 v6 by activation context) owned by an application thread in
this process; NVDA's real IAccessible/sysListView32 classes read it through nvdaHelperRemote's in-process RPC
(nvdaInProcUtils_sysListView32_*) and, for comparison, through the out-of-process LVM_* path."""
import os, sys, ctypes, collections
from ctypes import byref, c_int, c_void_p
from ctypes import wintypes as w

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from helper import *  # noqa: F403
import helper as H
import config, api, IAccessibleHandler, NVDAHelper, eventHandler  # noqa: E401
import NVDAObjects.IAccessible.sysListView32 as slv

LVM_FIRST = 0x1000
LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETITEMTEXTW, LVM_SETITEMSTATE = LVM_FIRST + 97, LVM_FIRST + 77, LVM_FIRST + 116, LVM_FIRST + 43
LVM_SETCOLUMNORDERARRAY, LVM_SETEXTENDEDLISTVIEWSTYLE = LVM_FIRST + 58, LVM_FIRST + 54
LVM_INSERTGROUP, LVM_ENABLEGROUPVIEW, LVM_SETITEMW = LVM_FIRST + 145, LVM_FIRST + 157, LVM_FIRST + 76


class LVCOLUMNW(ctypes.Structure):
	_fields_ = [("mask", w.UINT), ("fmt", c_int), ("cx", c_int), ("pszText", w.LPWSTR), ("cchTextMax", c_int),
		("iSubItem", c_int), ("iImage", c_int), ("iOrder", c_int), ("cxMin", c_int), ("cxDefault", c_int), ("cxIdeal", c_int)]


class LVITEMW(ctypes.Structure):
	_fields_ = [("mask", w.UINT), ("iItem", c_int), ("iSubItem", c_int), ("state", w.UINT), ("stateMask", w.UINT),
		("pszText", w.LPWSTR), ("cchTextMax", c_int), ("iImage", c_int), ("lParam", w.LPARAM), ("iIndent", c_int),
		("iGroupId", c_int), ("cColumns", w.UINT), ("puColumns", c_void_p), ("piColFmt", c_void_p), ("iGroup", c_int)]


class ACTCTXW(ctypes.Structure):
	_fields_ = [("cbSize", w.ULONG), ("dwFlags", w.DWORD), ("lpSource", w.LPCWSTR), ("wProcessorArchitecture", w.USHORT),
		("wLangId", w.USHORT), ("lpAssemblyDirectory", w.LPCWSTR), ("lpResourceName", w.LPCWSTR),
		("lpApplicationName", w.LPCWSTR), ("hModule", w.HMODULE)]


MANIFEST = os.path.join(HERE, "cc6.manifest")
with open(MANIFEST, "w") as f:
	f.write('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">'
		'<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" '
		'processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>')
k32.CreateActCtxW.restype = w.HANDLE
k32.ActivateActCtx.argtypes = [w.HANDLE, ctypes.POINTER(ctypes.c_size_t)]
COLS = [("Name", 120), ("Size", 80), ("Type", 100)]
ROWS = [("a.txt", "3 KB", "Text Document"), ("b.png", "120 KB", "PNG image"), ("notes.md", "1 KB", "Markdown")]
ORDER = (c_int * 3)(0, 2, 1)  # the user dragged Type before Size


@WNDPROC
def hostproc(h, m, wp, lp):
	return u32.DefWindowProcW(h, m, wp, lp)


def setup():
	ctx = ACTCTXW(cbSize=ctypes.sizeof(ACTCTXW), lpSource=MANIFEST)
	hctx = k32.CreateActCtxW(byref(ctx))
	assert hctx and hctx != ctypes.c_void_p(-1).value, ("CreateActCtx", ctypes.get_last_error())
	cookie = ctypes.c_size_t()
	assert k32.ActivateActCtx(hctx, byref(cookie)), ctypes.get_last_error()
	ctypes.WinDLL("comctl32").InitCommonControls()
	assert u32.RegisterClassW(byref(WNDCLASSW(lpfnWndProc=hostproc, lpszClassName="OracleHost")))
	top = u32.CreateWindowExW(0, "OracleHost", "Files", 0x00CF0000, 0, 0, 600, 400, None, None, None, None)  # never shown
	lv = u32.CreateWindowExW(0, "SysListView32", "", 0x50000000 | 0x1 | 0x8, 0, 0, 580, 380, top, None, None, None)
	for i, (t, cx) in enumerate(COLS):
		col = LVCOLUMNW(mask=1 | 2 | 4 | 8, cx=cx, pszText=t, iSubItem=i)
		u32.SendMessageW(lv, LVM_INSERTCOLUMNW, i, ctypes.addressof(col))
	for r, row in enumerate(ROWS):
		it = LVITEMW(mask=1, iItem=r, iSubItem=0, pszText=row[0])
		u32.SendMessageW(lv, LVM_INSERTITEMW, 0, ctypes.addressof(it))
		for c in (1, 2):
			it = LVITEMW(iSubItem=c, pszText=row[c])
			u32.SendMessageW(lv, LVM_SETITEMTEXTW, r, ctypes.addressof(it))
	u32.SendMessageW(lv, LVM_SETCOLUMNORDERARRAY, 3, ctypes.addressof(ORDER))
	u32.SetFocus(lv)
	return dict(top=top, lv=lv, focusOnLv=u32.GetFocus() == lv)


def focus_item(i):
	def run():
		it = LVITEMW(stateMask=3, state=0)
		u32.SendMessageW(st["lv"], LVM_SETITEMSTATE, ctypes.c_size_t(-1).value, ctypes.addressof(it))
		it = LVITEMW(stateMask=3, state=3)
		u32.SendMessageW(st["lv"], LVM_SETITEMSTATE, i, ctypes.addressof(it))
	on_app(run)
	IAccessibleHandler.processFocusWinEvent(st["lv"], -4, i + 1, force=True)


H.PATCHES.append("focus delivered with force=True: the hidden window is never foreground, so the item lacks FOCUSED (redirecting GetForegroundWindow/GetGUIThreadInfo in comctl32 and oleacc did not change that)")
calls = collections.Counter()
for n in [n for n in dir(NVDAHelper.localLib) if n.startswith("nvdaInProcUtils_sysListView32")]:
	f = getattr(NVDAHelper.localLib, n)
	setattr(NVDAHelper.localLib, n, (lambda f, n: lambda *a: (calls.update([n]), f(*a))[1])(f, n))

start_app(setup)
print("comctl32 v6 view:", u32.SendMessageW(st["lv"], LVM_FIRST + 143, 0, 0), "app thread has focus on list:", st["focusOnLv"])
make_visible([st["top"], st["lv"]])
_fg = ctypes.WINFUNCTYPE(w.HWND)(lambda: st["top"])
for _m in loaded_modules("comctl32.dll") + loaded_modules("oleacc.dll"):
	hook_import(_m, b"USER32.dll", b"GetForegroundWindow", _fg)
u32.GetGUIThreadInfo.argtypes = [w.DWORD, c_void_p]
_gti = ctypes.WINFUNCTYPE(w.BOOL, w.DWORD, c_void_p)(lambda tid, p: u32.GetGUIThreadInfo(tid or st["tid"], p))
for _m in loaded_modules("comctl32.dll") + loaded_modules("oleacc.dll"):
	hook_import(_m, b"USER32.dll", b"GetGUIThreadInfo", _gti)
print("hooks:", H.PATCHES[-4:])
winUser.getForegroundWindow = lambda: st["top"]
init_remote()
if os.environ.get("ATTACH", "1") == "1":
	print("AttachThreadInput(NVDA main thread, app thread):", u32.AttachThreadInput(k32.GetCurrentThreadId(), st["tid"], True), "GetFocus now:", u32.GetFocus() == st["lv"])
	H.PATCHES.append("AttachThreadInput: NVDA's main thread shares the app thread's input state, so GetFocus answers the list (both threads are in this process)")
# An in-context winEvent on the application thread: the remote's per-thread hooks install there and the RPC
# server registers with NVDA (nvdaControllerInternal_requestRegistration).
on_app(lambda: u32.NotifyWinEvent(0x8005, st["lv"], -4, 0))
pump(0.3)
set_desktop()
am = appModuleHandler.getAppModuleFromProcessID(os.getpid())
print("appModule:", am.appName, "in-process binding:", bool(am.helperLocalBindingHandle))

results = {}


def step(label, act, seconds=0.5):
	spoken.clear()
	act()
	pump(seconds, lambda: bool(spoken))
	pump(0.15)
	results[label] = list(spoken)


import logging
from logHandler import log as nvdaLog
config.conf["debugLog"]["MSAA"] = True
nvdaLog.setLevel(logging.DEBUG)
mark = len(boot.LOGLINES)
step("focus item 1", lambda: focus_item(0), 2)
nvdaLog.setLevel(logging.WARNING)
print("focus log:", *[l[:400] for l in boot.LOGLINES[mark:mark+30]], sep="\n   ")
import NVDAObjects.IAccessible as NIA
_o = NIA.getNVDAObjectFromEvent(st["lv"], -4, 1)
print("diag item raw state:", hex(_o.IAccessibleObject.accState(1)), "class of IAccessible:", type(_o.IAccessibleObject).__name__)
_p = _o.parent
while _p:
	try:
		print("   ancestor", type(_p).__name__, _p.windowClassName, _p.role, sorted(str(x) for x in _p.states))
	except Exception as e:
		print("   ancestor err", e)
	_p = _p.parent
fo = api.getFocusObject()
print("focus:", type(fo).__mro__[0].__name__, [c.__name__ for c in type(fo).__mro__[1:5]])
print("in-process calls so far:", dict(calls))
step("down arrow to item 2", lambda: focus_item(1))
fo = api.getFocusObject()
print("item 2 columns via NVDA (presentation order):", [(fo._getColumnHeader(c), fo._getColumnContent(c)) for c in range(1, fo.childCount + 1)])
print("item 2 rowNumber/positionInfo:", fo.positionInfo)
config.conf["documentFormatting"]["reportTableHeaders"] = 0
step("item 3 with table headers off", lambda: focus_item(2))
config.conf["documentFormatting"]["reportTableHeaders"] = 1
in_calls = dict(calls)
# Out-of-process path: the same objects with no binding handle (LVM_* via VirtualAllocEx/ReadProcessMemory).
am.helperLocalBindingHandle, saved = None, am.helperLocalBindingHandle
calls.clear()
step("out-of-process path, focus item 1", lambda: focus_item(0))
print("in-process calls on the out-of-process path:", dict(calls))
am.helperLocalBindingHandle = saved
print("=== speech")
for k, v in results.items():
	print(f"{k}: {v}")
print("in-process call counts (first three steps):", in_calls)
print("=== patches")
for p in H.PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES[:20]:
	if "espeak" in line or "VoiceSpeechDict" in line or "setSynth" in line:
		continue
	print("  ", line[:800])
on_app(lambda: u32.DestroyWindow(st["top"]))
