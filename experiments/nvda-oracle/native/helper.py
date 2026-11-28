"""Shared harness: headless NVDA (boot.py), NVDAHelper wired as NVDAHelper.initialize does but with no
injection_initialize, and nvdaHelperRemote loaded into this process with only initInprocManagerThreadIfNeeded
(after ../browse/expB.py). An "application thread" owns hidden windows and pumps messages, as an app's UI thread would."""
import os, sys, ctypes, time, threading
from ctypes import byref, c_void_p, c_int, WINFUNCTYPE
from ctypes import wintypes as w

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401
import config, queueHandler, api, baseObject, appModuleHandler, NVDAHelper, winUser  # noqa: E401
import speech.speech as sp
import NVDAObjects.window

LIB = os.environ["NVDA_LIB"]
PATCHES = []
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u32 = ctypes.WinDLL("user32", use_last_error=True)
k32.LoadLibraryExW.restype = c_void_p
k32.LoadLibraryExW.argtypes = [w.LPCWSTR, c_void_p, w.DWORD]
u32.CreateWindowExW.restype = w.HWND
u32.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, c_int, c_int, c_int, c_int, w.HWND, w.HMENU, w.HINSTANCE, c_void_p]
u32.SendMessageW.restype = ctypes.c_ssize_t
u32.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u32.DefWindowProcW.restype = ctypes.c_ssize_t
u32.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u32.NotifyWinEvent.argtypes = [w.DWORD, w.HWND, ctypes.c_long, ctypes.c_long]
WNDPROC = WINFUNCTYPE(ctypes.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)


class WNDCLASSW(ctypes.Structure):
	_fields_ = [("style", w.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", c_int), ("cbWndExtra", c_int),
		("hInstance", w.HINSTANCE), ("hIcon", w.HICON), ("hCursor", c_void_p), ("hbrBackground", w.HBRUSH),
		("lpszMenuName", w.LPCWSTR), ("lpszClassName", w.LPCWSTR)]


APPNAME = os.environ.get("ORACLE_APP", "testapp")
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: APPNAME + (".exe" if includeExt else "")
PATCHES.append(f"appModuleHandler.getAppNameFromProcessID: this process is named {APPNAME}")

spoken = []
sp.speak = lambda seq, symbolLevel=None, priority=None: spoken.append("  ".join(str(x) for x in seq if isinstance(x, str)))

controllerCalls = []
NAMES = ["nvdaController_speakText", "nvdaController_speakSsml", "nvdaController_cancelSpeech", "nvdaController_brailleMessage",
	"nvdaController_isSpeaking", "nvdaControllerInternal_requestRegistration", "nvdaControllerInternal_reportLiveRegion",
	"nvdaControllerInternal_inputLangChangeNotify", "nvdaControllerInternal_typedCharacterNotify",
	"nvdaControllerInternal_displayModelTextChangeNotify", "nvdaControllerInternal_logMessage",
	"nvdaControllerInternal_inputCompositionUpdate", "nvdaControllerInternal_inputCandidateListUpdate",
	"nvdaControllerInternal_IMEOpenStatusUpdate", "nvdaControllerInternal_inputConversionModeUpdate",
	"nvdaControllerInternal_vbufChangeNotify", "nvdaControllerInternal_installAddonPackageFromPath",
	"nvdaControllerInternal_drawFocusRectNotify", "nvdaControllerInternal_openConfigDirectory", "nvdaControllerInternal_handleRemoteURL"]


for name in NAMES:
	NVDAHelper._setDllFuncPointer(NVDAHelper.localLib.dll, f"_{name}", getattr(NVDAHelper, name))
NVDAHelper.localLib.nvdaHelperLocal_initialize(False)
hRemote = k32.LoadLibraryExW(os.path.join(LIB, "nvdaHelperRemote.dll"), None, 8)
NVDAHelper._remoteLib = ctypes.CDLL("nvdaHelperRemote", handle=hRemote)
PATCHES.append("NVDAHelper: callbacks wired as initialize does, nvdaHelperLocal_initialize; remote loaded, no injection_initialize")

WM_RUN = 0x8000 + 1
st = {}
_ready = threading.Event()


def app_thread(setup):
	comtypes_init()
	r = setup()
	st.update(r)
	st["tid"] = k32.GetCurrentThreadId()
	_ready.set()
	msg = w.MSG()
	while u32.GetMessageW(byref(msg), None, 0, 0) > 0:
		if msg.message == WM_RUN and not msg.hWnd:
			st.pop("run")()
			continue
		u32.TranslateMessage(byref(msg))
		u32.DispatchMessageW(byref(msg))


def comtypes_init():
	import comtypes
	comtypes.CoInitialize()


def start_app(setup):
	threading.Thread(target=app_thread, args=(setup,), daemon=True).start()
	assert _ready.wait(10)
	return st


def pump(seconds=0.3, until=None):
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


def on_app(fn):
	done = threading.Event()
	box = {}

	def run():
		try:
			box["r"] = fn()
		except Exception as e:  # noqa: BLE001
			box["e"] = e
		finally:
			done.set()

	st["run"] = run
	u32.PostThreadMessageW(st["tid"], WM_RUN, 0, 0)
	pump(10, done.is_set)
	if "e" in box:
		raise box["e"]
	return box.get("r")


def init_remote():
	r = NVDAHelper._remoteLib.initInprocManagerThreadIfNeeded()
	PATCHES.append(f"initInprocManagerThreadIfNeeded (this process only): {r}")
	return r


def set_desktop():
	desktop = NVDAObjects.window.Window(windowHandle=u32.GetDesktopWindow())
	api.setDesktopObject(desktop)
	api.setForegroundObject(desktop)
	api.setFocusObject(desktop)
	api.setNavigatorObject(desktop)


def make_visible(hwnds):
	vis = set(hwnds)
	winUser.isWindowVisible = lambda h: True if h in vis else bool(u32.IsWindowVisible(h))
	PATCHES.append("winUser.isWindowVisible: the test windows count as visible (they are never shown)")


def loaded_modules(basename):
	psapi = ctypes.WinDLL("psapi")
	k32.GetCurrentProcess.restype = w.HANDLE
	psapi.EnumProcessModules.argtypes = [w.HANDLE, ctypes.POINTER(c_void_p), w.DWORD, ctypes.POINTER(w.DWORD)]
	k32.GetModuleFileNameW.argtypes = [c_void_p, w.LPWSTR, w.DWORD]
	arr = (c_void_p * 1024)()
	need = w.DWORD()
	psapi.EnumProcessModules(k32.GetCurrentProcess(), arr, ctypes.sizeof(arr), byref(need))
	out = []
	for h in arr[: need.value // ctypes.sizeof(c_void_p)]:
		buf = ctypes.create_unicode_buffer(520)
		k32.GetModuleFileNameW(c_void_p(h), buf, 520)
		if os.path.basename(buf.value).lower() == basename.lower():
			out.append(buf.value)
	return out


_hookKeep = []


def hook_import(targetPath, importDll, func, cb):
	"""Redirect one import of one module in this process (NVDA's own dllImportTableHooks)."""
	f = NVDAHelper.localLib.dll.dllImportTableHooks_hookSingle
	f.restype = c_void_p
	f.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, c_void_p]
	_hookKeep.append(cb)
	r = f(targetPath.encode("mbcs"), importDll, func, ctypes.cast(cb, c_void_p))
	PATCHES.append(f"{os.path.basename(targetPath)} ({targetPath[-60:]}) import of {func.decode()} redirected: {'ok' if r else 'FAILED'}")
	return r
