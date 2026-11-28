"""IME: nvdaHelperRemote's IMM32 path (ime.cpp, its WH_CALLWNDPROC and WH_GETMESSAGE hooks on the application
thread) driven by real WM_IME_* messages sent to a never-shown window of this process, with nvdaHelperRemote's own
IMM32 imports redirected to a stand-in input context (no IME is installed or activated; no keys are sent).
Every nvdaControllerInternal_* call the remote makes is recorded at the controller boundary, then replayed straight
into NVDA's Python handlers in a second pass to compare speech."""
import os, sys, ctypes, struct
from ctypes import byref, c_void_p, WINFUNCTYPE
from ctypes import wintypes as w

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from helper import *  # noqa: F403
import helper as H
import config, api, NVDAHelper  # noqa: E401
from NVDAObjects.window import Window

WM_IME_STARTCOMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_COMPOSITION, WM_IME_NOTIFY = 0x10D, 0x10E, 0x10F, 0x282
IMN_CHANGECANDIDATE, IMN_CLOSECANDIDATE, IMN_OPENCANDIDATE, IMN_SETCONVERSIONMODE, IMN_SETOPENSTATUS = 3, 4, 5, 6, 8
GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR = 0x8, 0x80, 0x800
IME = dict(comp="", cursor=0, result="", cands=[], sel=0, open=False, conv=0)
immCalls = []


@WNDPROC
def proc(h, m, wp, lp):
	if m in (WM_IME_STARTCOMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_COMPOSITION, WM_IME_NOTIFY):
		return 0  # the application handles IME messages itself; the stand-in context has no real IME window
	return u32.DefWindowProcW(h, m, wp, lp)


def setup():
	assert u32.RegisterClassW(byref(WNDCLASSW(lpfnWndProc=proc, lpszClassName="OracleImeEdit")))
	h = u32.CreateWindowExW(0, "OracleImeEdit", "Note", 0x00CF0000, 0, 0, 400, 200, None, None, None, None)  # never shown
	u32.SetFocus(h)
	return dict(top=h)


# Stand-in IMM32, for nvdaHelperRemote's imports only.
def _str_into(s, buf, ln):
	data = s.encode("utf-16-le")
	if buf and ln:
		ctypes.memmove(buf, data, min(len(data), ln))
	return len(data)


@WINFUNCTYPE(ctypes.c_long, c_void_p, w.DWORD, c_void_p, w.DWORD)
def ImmGetCompositionStringW(himc, index, buf, ln):
	immCalls.append(("ImmGetCompositionStringW", hex(index)))
	if index == GCS_COMPSTR:
		return _str_into(IME["comp"], buf, ln)
	if index == GCS_RESULTSTR:
		return _str_into(IME["result"], buf, ln)
	if index == GCS_CURSORPOS:
		return IME["cursor"]
	return 0


def _candlist():
	strs = [s.encode("utf-16-le") + b"\0\0" for s in IME["cands"]]
	n = len(strs)
	head = 6 * 4 + 4 * n
	offs, pos = [], head
	for s in strs:
		offs.append(pos)
		pos += len(s)
	return struct.pack("<6I", pos, 1, n, IME["sel"], 0, min(n, 9)) + struct.pack(f"<{n}I", *offs) + b"".join(strs)


@WINFUNCTYPE(w.DWORD, c_void_p, ctypes.POINTER(w.DWORD))
def ImmGetCandidateListCountW(himc, count):
	immCalls.append(("ImmGetCandidateListCountW",))
	count[0] = 1 if IME["cands"] else 0
	return len(_candlist()) if IME["cands"] else 0


@WINFUNCTYPE(w.DWORD, c_void_p, w.DWORD, c_void_p, w.DWORD)
def ImmGetCandidateListW(himc, index, buf, ln):
	immCalls.append(("ImmGetCandidateListW", ln))
	data = _candlist()
	if buf and ln:
		ctypes.memmove(buf, data, min(ln, len(data)))
	return len(data)


@WINFUNCTYPE(w.BOOL, c_void_p)
def ImmGetOpenStatus(himc):
	immCalls.append(("ImmGetOpenStatus",))
	return IME["open"]


@WINFUNCTYPE(w.BOOL, c_void_p, ctypes.POINTER(w.DWORD), ctypes.POINTER(w.DWORD))
def ImmGetConversionStatus(himc, conv, sentence):
	immCalls.append(("ImmGetConversionStatus",))
	if conv:
		conv[0] = IME["conv"]
	return 1


@WINFUNCTYPE(w.DWORD, c_void_p, w.DWORD)
def ImmGetProperty(hkl, idx):
	return 0xFFFFFFFF


@WINFUNCTYPE(w.UINT, c_void_p, c_void_p, w.UINT)
def ImmGetIMEFileNameW(hkl, buf, ln):
	return _str_into("IMJP15.IME\0", buf, ln * 2) // 2 - 1


JA_HKL = 0x04110411


@WINFUNCTYPE(c_void_p, w.DWORD)
def GetKeyboardLayout(tid):
	return JA_HKL


start_app(setup)
make_visible([st["top"]])
init_remote()
remote = loaded_modules("nvdaHelperRemote.dll")[0]
for name, cb in [("ImmGetCompositionStringW", ImmGetCompositionStringW), ("ImmGetCandidateListCountW", ImmGetCandidateListCountW),
		("ImmGetCandidateListW", ImmGetCandidateListW), ("ImmGetOpenStatus", ImmGetOpenStatus),
		("ImmGetConversionStatus", ImmGetConversionStatus), ("ImmGetProperty", ImmGetProperty), ("ImmGetIMEFileNameW", ImmGetIMEFileNameW)]:
	hook_import(remote, b"IMM32.dll", name.encode(), cb)
hook_import(remote, b"USER32.dll", b"GetKeyboardLayout", GetKeyboardLayout)

# Record the controller boundary: wrap NVDA's own callbacks (as NVDAHelper.initialize installs them).
boundary = []
_keep = []
for name in ["nvdaControllerInternal_inputCompositionUpdate", "nvdaControllerInternal_inputCandidateListUpdate",
		"nvdaControllerInternal_IMEOpenStatusUpdate", "nvdaControllerInternal_inputConversionModeUpdate",
		"nvdaControllerInternal_inputLangChangeNotify", "nvdaControllerInternal_typedCharacterNotify"]:
	orig = getattr(NVDAHelper, name)
	proto = WINFUNCTYPE(orig._restype_, *orig._argtypes_)
	wrapped = proto((lambda orig, name: lambda *a: (boundary.append((name, a)), orig(*a))[1])(orig, name))
	_keep.append(wrapped)
	NVDAHelper._setDllFuncPointer(NVDAHelper.localLib.dll, f"_{name}", wrapped)
H.PATCHES.append("controller callbacks for IME, language and typed characters wrapped to record their arguments, then call NVDA's own")

on_app(lambda: u32.NotifyWinEvent(0x8005, st["top"], -4, 0))  # in-context winEvent: per-thread hooks, TSF sink, RPC registration
pump(0.3)
set_desktop()
am = appModuleHandler.getAppModuleFromProcessID(os.getpid())
print("appModule:", am.appName, "in-process binding:", bool(am.helperLocalBindingHandle))
imm = ctypes.WinDLL("imm32")
imm.ImmGetContext.restype = c_void_p
imm.ImmGetContext.argtypes = [w.HWND]
print("real input context on the app thread:", on_app(lambda: bool(imm.ImmGetContext(st["top"]))))
focusObj = Window(windowHandle=st["top"])
api.setFocusObject(focusObj)
api.getDesktopObject().__class__.objectWithFocus = lambda self: focusObj
H.PATCHES.append("NVDA focus set to the test window; Desktop.objectWithFocus answers it (the hidden window is never the OS foreground)")
config.conf["inputComposition"]["autoReportAllCandidates"] = True


def send(msg, wp=0, lp=0):
	on_app(lambda: u32.SendMessageW(st["top"], msg, wp, lp))


SEQ = []


def act(label, fn):
	SEQ.append((label, fn))


def comp(s, cursor):
	def f():
		IME.update(comp=s, cursor=cursor)
		send(WM_IME_COMPOSITION, 0, GCS_COMPSTR | GCS_CURSORPOS)
	return f


def cands(lst, sel, msg):
	def f():
		IME.update(cands=lst, sel=sel)
		send(WM_IME_NOTIFY, msg, 0)
	return f


def end(result):
	def f():
		IME.update(comp="", result=result, cands=[])
		send(WM_IME_NOTIFY, IMN_CLOSECANDIDATE, 0)
		send(WM_IME_ENDCOMPOSITION)
	return f


act("IME closes (Japanese)", lambda: (IME.update(open=False), send(WM_IME_NOTIFY, IMN_SETOPENSTATUS)))
act("IME opens (Japanese)", lambda: (IME.update(open=True), send(WM_IME_NOTIFY, IMN_SETOPENSTATUS)))
act("conversion mode to hiragana", lambda: (IME.update(conv=9), send(WM_IME_NOTIFY, IMN_SETCONVERSIONMODE)))
act("type n", comp("ｎ", 1))
act("type i", comp("に", 1))
act("type h o n", comp("にほん", 3))
act("space: candidates open", cands(["日本", "二本", "にほん"], 0, IMN_OPENCANDIDATE))
act("space: next candidate", cands(["日本", "二本", "にほん"], 1, IMN_CHANGECANDIDATE))
act("enter: commit", end("二本"))

results = []
for label, fn in SEQ:
	spoken.clear()
	mark = len(boundary)
	fn()
	pump(0.4, lambda: bool(spoken))
	pump(0.2)
	results.append((label, list(spoken), boundary[mark:]))
print("=== live pass: real WM_IME_* through nvdaHelperRemote's hooks")
for label, sp_, b in results:
	print(f"{label}: speech {sp_}")
	for name, a in b:
		print(f"      boundary {name.replace('nvdaControllerInternal_', '')}{a}")

# Replay: the recorded boundary calls, straight into NVDA's callbacks, with the remote out of the path.
api.setFocusObject(focusObj)
import eventHandler
eventHandler.executeEvent("gainFocus", focusObj)
pump(0.3)
print("=== replay pass: the recorded calls into NVDA's controller callbacks")
same = True
for label, sp_, b in results:
	spoken.clear()
	for name, a in b:
		getattr(NVDAHelper, name)(*a)
	pump(0.4, lambda: bool(spoken))
	pump(0.2)
	print(f"{label}: speech {spoken}{'' if spoken == sp_ else '   DIFFERS from live'}")
	same = same and spoken == sp_
print("replay matches live:", same)
print("stand-in IMM32 calls:", len(immCalls), sorted({c[0] for c in immCalls}))
print("=== patches")
for p in H.PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES:
	if any(s in line for s in ("espeak", "VoiceSpeechDict", "setSynth", "6364", "Access is denied", "No window handle")):
		continue
	print("  ", line[:600])
on_app(lambda: u32.DestroyWindow(st["top"]))
