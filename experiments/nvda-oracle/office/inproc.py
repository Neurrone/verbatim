"""Headless NVDA plus NVDA's shipped nvdaHelperRemote.dll loaded into this process, wired as ../browse/expB.py
does (callbacks, nvdaHelperLocal_initialize, initInprocManagerThreadIfNeeded, no injection_initialize, no
global hooks). An "application thread" owns a never-shown window of the given class whose WM_GETOBJECT
answers OBJID_NATIVEOM with a stand-in IDispatch, as Word's and Excel's document windows do.

The remote's per-thread hooks (winword's WH_CALLWNDPROC hook, execInThread) are installed on the
application thread by an in-context winEvent raised there."""
import os, sys, ctypes, time, threading
from ctypes import POINTER, byref, c_void_p, c_int, c_long, WINFUNCTYPE
from ctypes import wintypes as w

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401
import comtypes
from comtypes.automation import IDispatch
import NVDAHelper, queueHandler, baseObject, appModuleHandler  # noqa: E401

PATCHES = []
LIB = os.environ["NVDA_LIB"]
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u32 = ctypes.WinDLL("user32", use_last_error=True)
oleacc = ctypes.WinDLL("oleacc")
k32.LoadLibraryExW.restype = c_void_p
k32.LoadLibraryExW.argtypes = [w.LPCWSTR, c_void_p, w.DWORD]
WNDPROC = WINFUNCTYPE(ctypes.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
u32.DefWindowProcW.restype = ctypes.c_ssize_t
u32.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u32.CreateWindowExW.restype = w.HWND
u32.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, c_int, c_int, c_int, c_int, w.HWND, w.HMENU, w.HINSTANCE, c_void_p]
oleacc.LresultFromObject.restype = ctypes.c_ssize_t
oleacc.LresultFromObject.argtypes = [POINTER(comtypes.GUID), w.WPARAM, c_void_p]
u32.NotifyWinEvent.argtypes = [w.DWORD, w.HWND, c_long, c_long]
WM_GETOBJECT, OBJID_NATIVEOM, OBJID_CLIENT, EVENT_OBJECT_FOCUS = 0x3D, -16, -4, 0x8005
WM_RUN = 0x8000 + 1


class WNDCLASSW(ctypes.Structure):
	_fields_ = [("style", w.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", c_int), ("cbWndExtra", c_int),
		("hInstance", w.HINSTANCE), ("hIcon", w.HICON), ("hCursor", c_void_p), ("hbrBackground", w.HBRUSH),
		("lpszMenuName", w.LPCWSTR), ("lpszClassName", w.LPCWSTR)]


st = {"nativeom": 0}


@WNDPROC
def wndproc(hwnd, msg, wp, lp):
	if msg == WM_GETOBJECT and ctypes.c_long(lp & 0xFFFFFFFF).value == OBJID_NATIVEOM and "om" in st:
		st["nativeom"] += 1
		p = st["om"]().ptr()
		return oleacc.LresultFromObject(byref(IDispatch._iid_), wp, ctypes.cast(p, c_void_p))
	return u32.DefWindowProcW(hwnd, msg, wp, lp)


def pump(seconds=0.2, until=None):
	end = time.perf_counter() + seconds
	msg = w.MSG()
	while time.perf_counter() < end:
		while u32.PeekMessageW(byref(msg), None, 0, 0, 1):
			u32.TranslateMessage(byref(msg))
			u32.DispatchMessageW(byref(msg))
		queueHandler.pumpAll()
		baseObject.AutoPropertyObject.invalidateCaches()
		if until and until():
			return True
		time.sleep(0.005)
	return False


def _app_thread(className, title, makeWorld, ready):
	comtypes.CoInitialize()
	wc = WNDCLASSW(lpfnWndProc=wndproc, lpszClassName=className)
	assert u32.RegisterClassW(byref(wc))
	h = u32.CreateWindowExW(0, className, title, 0x40000000 if False else 0, 0, 0, 800, 600, None, None, None, None)
	st.update(hwnd=h, tid=k32.GetCurrentThreadId())
	st["om"] = makeWorld()
	ready.set()
	msg = w.MSG()
	while u32.GetMessageW(byref(msg), None, 0, 0) > 0:
		if msg.message == WM_RUN and not msg.hWnd:
			st.pop("run")()
			continue
		u32.TranslateMessage(byref(msg))
		u32.DispatchMessageW(byref(msg))


def on_app(fn):
	done = threading.Event()
	box = {}

	def run():
		try:
			box["r"] = fn()
		except Exception as e:
			box["e"] = e
		finally:
			done.set()
	st["run"] = run
	u32.PostThreadMessageW(st["tid"], WM_RUN, 0, 0)
	pump(10, done.is_set)
	if "e" in box:
		raise box["e"]
	return box.get("r")


def start(className, title, appName, makeWorld):
	"""makeWorld runs on the application thread and returns a callable giving the OBJID_NATIVEOM object."""
	appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: appName + (".exe" if includeExt else "")
	PATCHES.append(f"appModuleHandler.getAppNameFromProcessID: this process is named {appName}")
	NAMES = [n for n in dir(NVDAHelper) if n.startswith("nvdaController")]
	for name in NAMES:
		try:
			NVDAHelper._setDllFuncPointer(NVDAHelper.localLib.dll, f"_{name}", getattr(NVDAHelper, name))
		except Exception:
			pass
	NVDAHelper.localLib.nvdaHelperLocal_initialize(False)
	hRemote = k32.LoadLibraryExW(os.path.join(LIB, "nvdaHelperRemote.dll"), None, 8)
	NVDAHelper._remoteLib = ctypes.CDLL("nvdaHelperRemote", handle=hRemote)
	PATCHES.append("NVDAHelper: callbacks wired and nvdaHelperLocal_initialize; nvdaHelperRemote loaded, only initInprocManagerThreadIfNeeded called")
	ready = threading.Event()
	threading.Thread(target=_app_thread, args=(className, title, makeWorld, ready), daemon=True).start()
	assert ready.wait(10)
	print("initInprocManagerThreadIfNeeded:", NVDAHelper._remoteLib.initInprocManagerThreadIfNeeded())
	on_app(lambda: u32.NotifyWinEvent(EVENT_OBJECT_FOCUS, st["hwnd"], OBJID_CLIENT, 0))
	pump(0.3)
	am = appModuleHandler.getAppModuleFromProcessID(os.getpid())
	print("appModule:", am.appName, type(am).__module__, "binding:", bool(am.helperLocalBindingHandle))
	return st["hwnd"], am
