"""Option (a), whole stack: NVDA's Python browse mode (IAccessibleHandler, chromium.Document, ChromeVBuf,
browseMode quick navigation, focus-mode switching) over NVDA's native gecko_ia2 backend and ia2LiveRegions,
all in this one process, on in-process IAccessible2 stand-ins. Prints what NVDA would speak.

Nothing outside this process is touched: no injection_initialize, no IAccessibleHandler.initialize (its global
winEvent hooks), no installIA2Support; the stand-in window is never shown."""
import os, sys, ctypes, time
from ctypes import POINTER, byref, c_void_p, c_int, c_long, WINFUNCTYPE
from ctypes import wintypes as w

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import bootb  # noqa: F401  headless NVDA
import ia2standins as S
from ia2standins import Node, IA2, Acc
import comtypes

import config, queueHandler, api, baseObject, appModuleHandler, NVDAHelper, IAccessibleHandler, winUser  # noqa: E401
import speech.speech as sp
import NVDAObjects.window

PATCHES = []
LIB = os.environ["NVDA_LIB"]
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u32 = ctypes.WinDLL("user32", use_last_error=True)
oleacc = ctypes.WinDLL("oleacc")
k32.LoadLibraryExW.restype = c_void_p
k32.LoadLibraryExW.argtypes = [w.LPCWSTR, c_void_p, w.DWORD]

config.conf["virtualBuffers"]["autoSayAllOnPageLoad"] = False
config.conf["virtualBuffers"]["passThroughAudioIndication"] = False
PATCHES.append("config: autoSayAllOnPageLoad off, passThroughAudioIndication off (speech instead of a sound)")
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: "chrome" + (".exe" if includeExt else "")
PATCHES.append("appModuleHandler.getAppNameFromProcessID: this process is named chrome")

spoken = []
sp.speak = lambda seq, symbolLevel=None, priority=None: spoken.append("  ".join(x for x in seq if isinstance(x, str)))

ROLE_DOCUMENT, ROLE_LINK, ROLE_PUSHBUTTON, ROLE_TEXT, ROLE_TABLE, ROLE_ROW, ROLE_CELL, ROLE_COLHDR = 0x0F, 0x1E, 0x2B, 0x2A, 0x18, 0x1C, 0x1D, 0x19
FOCUSABLE, READONLY, LINKED, FOCUSED = 0x100000, 0x40, 0x400000, 0x4
IA2_EDITABLE, IA2_SINGLE_LINE = 0x8, 0x2000
BLOCK = {"display": "block"}
INLINE = {"display": "inline"}


def build_page():
	label = Node(IA2.IA2_ROLE_LABEL, content=["Search"], attrs={"tag": "label", **INLINE})
	edit = Node(ROLE_TEXT, name="Search", states=FOCUSABLE, ia2states=IA2_EDITABLE | IA2_SINGLE_LINE,
		attrs={"tag": "input", "explicit-name": "true", "display": "inline-block", "text-input-type": "search"}, content=[""], labelledBy=label)
	link = Node(ROLE_LINK, name="docs", states=FOCUSABLE | LINKED, attrs={"tag": "a", **INLINE}, content=["docs"], actions=["jump"], value="https://example.test/docs")
	t = Node(ROLE_TABLE, attrs={"tag": "table", "display": "table"}, table=(2, 2))
	h1 = Node(ROLE_COLHDR, content=["Name"], attrs={"tag": "th", "display": "table-cell"}, cell=(0, 0, t))
	h2 = Node(ROLE_COLHDR, content=["Size"], attrs={"tag": "th", "display": "table-cell"}, cell=(0, 1, t))
	c1 = Node(ROLE_CELL, content=["a.txt"], attrs={"tag": "td", "display": "table-cell"}, cell=(1, 0, t))
	c2 = Node(ROLE_CELL, content=["3 KB"], attrs={"tag": "td", "display": "table-cell"}, cell=(1, 1, t))
	t.childList = [Node(ROLE_ROW, children=[h1, h2], attrs={"tag": "tr", "display": "table-row"}),
		Node(ROLE_ROW, children=[c1, c2], attrs={"tag": "tr", "display": "table-row"})]
	live = Node(IA2.IA2_ROLE_SECTION, attrs={"tag": "div", **BLOCK, "live": "polite", "container-live": "polite", "container-relevant": "additions text"}, content=[""])
	nodes = dict(link=link, edit=edit, table=t, live=live, label=label)
	doc = Node(ROLE_DOCUMENT, name="Test page", states=FOCUSABLE | READONLY | FOCUSED, attrs={"tag": "#document"}, content=[
		Node(IA2.IA2_ROLE_LANDMARK, attrs={"xml-roles": "navigation", "tag": "nav", **BLOCK}, content=[
			Node(IA2.IA2_ROLE_PARAGRAPH, attrs={"tag": "p", **BLOCK}, content=["Read the ", link, " first."]),
		]),
		heading := Node(IA2.IA2_ROLE_HEADING, name="Welcome", attrs={"tag": "h2", "level": "2", **BLOCK}, content=["Welcome"]),
		Node(IA2.IA2_ROLE_PARAGRAPH, attrs={"tag": "p", **BLOCK}, content=[label, " ", edit]),
		Node(ROLE_PUSHBUTTON, name="Go", states=FOCUSABLE, attrs={"tag": "button", "display": "inline-block"}, content=["Go"], actions=["press"]),
		t,
		live,
	])
	nodes.update(doc=doc, heading=heading)
	return nodes


WNDPROC = WINFUNCTYPE(ctypes.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
u32.DefWindowProcW.restype = ctypes.c_ssize_t
u32.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u32.CreateWindowExW.restype = w.HWND
u32.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, c_int, c_int, c_int, c_int, w.HWND, w.HMENU, w.HINSTANCE, c_void_p]
oleacc.LresultFromObject.restype = ctypes.c_ssize_t
oleacc.LresultFromObject.argtypes = [POINTER(comtypes.GUID), w.WPARAM, c_void_p]
u32.NotifyWinEvent.argtypes = [w.DWORD, w.HWND, c_long, c_long]


class WNDCLASSW(ctypes.Structure):
	_fields_ = [("style", w.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", c_int), ("cbWndExtra", c_int),
		("hInstance", w.HINSTANCE), ("hIcon", w.HICON), ("hCursor", c_void_p), ("hbrBackground", w.HBRUSH),
		("lpszMenuName", w.LPCWSTR), ("lpszClassName", w.LPCWSTR)]


WM_GETOBJECT, OBJID_CLIENT, EVENT_OBJECT_FOCUS = 0x3D, -4, 0x8005
st = {}


@WNDPROC
def wndproc(hwnd, msg, wp, lp):
	if msg == WM_GETOBJECT and ctypes.c_long(lp & 0xFFFFFFFF).value == OBJID_CLIENT and "nodes" in st:
		S._count("WM_GETOBJECT")
		pacc = st["nodes"]["doc"].com.QueryInterface(Acc.IAccessible)
		return oleacc.LresultFromObject(byref(Acc.IAccessible._iid_), wp, ctypes.cast(pacc, c_void_p))
	return u32.DefWindowProcW(hwnd, msg, wp, lp)


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


# NVDAHelper, as NVDAHelper.initialize does it but without injection_initialize (global hooks) or installIA2Support.
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
PATCHES.append("NVDAHelper: initialize's callback wiring and nvdaHelperLocal_initialize; remote loaded and only initInprocManagerThreadIfNeeded called")

# The "browser UI thread": a never-shown top-level window with Chromium's class name, and the stand-ins, in their
# own STA with a message loop, as the browser's own thread would be. NVDA's main thread reaches them through COM
# marshaling (IA2 proxies registered by installIA2Support, in this process only), and nvdaHelperRemote's
# execInThread runs the backend's render here. With the world on NVDA's main thread instead, NVDA's synchronous
# in-process RPC (findContentDescendant) deadlocks against execInThread.
import threading
WM_RUN = 0x8000 + 1
_ready = threading.Event()


def browser_thread():
	comtypes.CoInitialize()
	wc = WNDCLASSW(lpfnWndProc=wndproc, lpszClassName="Chrome_RenderWidgetHostHWND")
	assert u32.RegisterClassW(byref(wc))
	h = u32.CreateWindowExW(0x80, "Chrome_RenderWidgetHostHWND", "Test page", 0x80000000, 0, 0, 800, 600, None, None, None, None)
	wd = S.World(h)
	S.WORLD = wd
	ns = build_page()
	ns["doc"].attach(wd)
	st.update(nodes=ns, world=wd, hwnd=h, tid=k32.GetCurrentThreadId())
	_ready.set()
	msg = w.MSG()
	while u32.GetMessageW(byref(msg), None, 0, 0) > 0:
		if msg.message == WM_RUN and not msg.hWnd:
			fn = st.pop("run")
			fn()
			continue
		u32.TranslateMessage(byref(msg))
		u32.DispatchMessageW(byref(msg))


def on_browser(fn):
	"""Run fn on the browser thread, pumping NVDA's side while waiting (COM calls may come back)."""
	done = threading.Event()
	box = {}

	def run():
		try:
			box["r"] = fn()
		finally:
			done.set()

	st["run"] = run
	u32.PostThreadMessageW(st["tid"], WM_RUN, 0, 0)
	pump(5, done.is_set)
	return box.get("r")


if os.environ.get("BROWSER_THREAD"):
	threading.Thread(target=browser_thread, daemon=True).start()
	_ready.wait(10)
	print("IA2 support on NVDA's main thread:", NVDAHelper._remoteLib.installIA2Support())
	PATCHES.append("installIA2Support on the main thread, as NVDAHelper.initialize does (IA2 proxies, this process only)")
else:
	# The world on NVDA's main thread: no COM marshaling. NVDA's synchronous in-process calls that the remote
	# runs on the window's thread (nvdaInProcUtils_*, through execInThread) are made from a helper thread while
	# this thread pumps messages, as the browser's own UI thread would serve them.
	wc = WNDCLASSW(lpfnWndProc=wndproc, lpszClassName="Chrome_RenderWidgetHostHWND")
	assert u32.RegisterClassW(byref(wc))
	h = u32.CreateWindowExW(0x80, "Chrome_RenderWidgetHostHWND", "Test page", 0x80000000, 0, 0, 800, 600, None, None, None, None)
	wd = S.World(h)
	S.WORLD = wd
	ns = build_page()
	ns["doc"].attach(wd)
	st.update(nodes=ns, world=wd, hwnd=h)

	def on_browser(fn):  # noqa: F811
		return fn()

	def _offThread(f):
		def call(*a):
			box = {}
			t = threading.Thread(target=lambda: box.__setitem__("r", f(*a)), daemon=True)
			t.start()
			msg = w.MSG()
			while t.is_alive():
				while u32.PeekMessageW(byref(msg), None, 0, 0, 1):
					u32.TranslateMessage(byref(msg))
					u32.DispatchMessageW(byref(msg))
				t.join(0.002)
			return box.get("r")
		return call

	for _n in [n for n in dir(NVDAHelper.localLib) if n.startswith("nvdaInProcUtils_") and "register" not in n.lower()]:
		setattr(NVDAHelper.localLib, _n, _offThread(getattr(NVDAHelper.localLib, _n)))
	PATCHES.append("NVDAHelper.localLib.nvdaInProcUtils_*: called from a helper thread while the main (window) thread pumps")
hwnd, world, nodes = st["hwnd"], st["world"], st["nodes"]

# ia2LiveRegions only reports for a visible foreground window; answer those two questions for this window, in nvdaHelperRemote only.
HOOKFN = WINFUNCTYPE(w.BOOL, w.HWND)
GETFG = WINFUNCTYPE(w.HWND)
isVisibleHook = HOOKFN(lambda h: 1 if h == hwnd else u32.IsWindowVisible(h))
fgHook = GETFG(lambda: hwnd)
NVDAHelper.localLib.dll.dllImportTableHooks_hookSingle.restype = c_void_p
NVDAHelper.localLib.dll.dllImportTableHooks_hookSingle.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, c_void_p]
for fn, cb in [(b"IsWindowVisible", isVisibleHook), (b"GetForegroundWindow", fgHook)]:
	r = NVDAHelper.localLib.dll.dllImportTableHooks_hookSingle(b"nvdaHelperRemote.dll", b"USER32.dll", fn, ctypes.cast(cb, c_void_p))
	PATCHES.append(f"nvdaHelperRemote import of {fn.decode()} redirected to the stand-in window: {'ok' if r else 'FAILED'}")
winUser.isWindowVisible = lambda h: True if h == hwnd else bool(u32.IsWindowVisible(h))
winUser.getForegroundWindow = lambda: hwnd
PATCHES.append("winUser.isWindowVisible / getForegroundWindow: the stand-in window is visible and foreground")

print("initInprocManagerThreadIfNeeded:", NVDAHelper._remoteLib.initInprocManagerThreadIfNeeded())
# Install the remote's per-thread hooks (and IA2 proxies) on the browser thread: an in-context winEvent there.
on_browser(lambda: u32.NotifyWinEvent(EVENT_OBJECT_FOCUS, hwnd, OBJID_CLIENT, nodes["doc"].uid))
pump(0.2)
desktop = NVDAObjects.window.Window(windowHandle=u32.GetDesktopWindow())
api.setDesktopObject(desktop)
api.setForegroundObject(desktop)
api.setFocusObject(desktop)
api.setNavigatorObject(desktop)
am = appModuleHandler.getAppModuleFromProcessID(os.getpid())
print("appModule:", am.appName, "binding:", bool(am.helperLocalBindingHandle))

results = {}


def step(label, action, until=None, seconds=0.6):
	spoken.clear()
	action()
	pump(seconds, until)
	results[label] = list(spoken)


def focus(key):
	def move():
		for n in world.byId.values():
			n.states &= ~FOCUSED
		nodes[key].states |= FOCUSED
		u32.NotifyWinEvent(EVENT_OBJECT_FOCUS, hwnd, OBJID_CLIENT, nodes[key].uid)  # in-context: the vbuf backend's forceUpdate

	on_browser(move)
	# NVDA's out-of-context winEvent hook is not installed (it would see the whole desktop); deliver the event as it would.
	IAccessibleHandler.processFocusWinEvent(hwnd, OBJID_CLIENT, nodes[key].uid)


t0 = time.perf_counter()
step("focus document (buffer loads)", lambda: focus("doc"),
	until=lambda: getattr(api.getFocusObject().treeInterceptor, "isReady", False) and spoken, seconds=5)
print(f"load and first speech after {1000 * (time.perf_counter() - t0):.0f} ms")
ti = api.getFocusObject().treeInterceptor
print("focus:", api.getFocusObject(), "treeInterceptor:", type(ti).__name__ if ti else None)
if ti and ti.isReady:
	for name in ["nextHeading", "nextLink", "nextTable", "nextLandmark", "nextEdit", "previousHeading"]:
		step(f"quick nav {name}", lambda name=name: getattr(ti, "script_" + name)(None))
	import textInfos, controlTypes  # noqa: E401

	def sayLine():
		info = ti.makeTextInfo(textInfos.POSITION_CARET)
		info.expand(textInfos.UNIT_LINE)
		sp.speakTextInfo(info, unit=textInfos.UNIT_LINE, reason=controlTypes.OutputReason.CARET)

	step("say line at caret", sayLine)
	import logging
	from logHandler import log as nvdaLog
	config.conf["debugLog"]["MSAA"] = True
	nvdaLog.setLevel(logging.DEBUG)
	mark = len(bootb.LOGLINES)
	step("browser focuses the edit field (focus mode switch)", lambda: focus("edit"))
	focusLog = [l.splitlines()[0][:300] for l in bootb.LOGLINES[mark:]]
	nvdaLog.setLevel(logging.WARNING)
	config.conf["debugLog"]["MSAA"] = False
	print("passThrough after edit focus:", ti.passThrough, "focus now:", api.getFocusObject())
	print("focus log:", *focusLog[:25], sep="\n   ")
	before = dict(S.CALLS)

	def liveChange():
		lr = nodes["live"]
		lr.content = ["Saved 3 files"]
		lr.lastInsert = ("Saved 3 files", 0)
		u32.NotifyWinEvent(IA2.IA2_EVENT_TEXT_INSERTED, hwnd, OBJID_CLIENT, lr.uid)

	step("live region text inserted", lambda: on_browser(liveChange))
	print("calls during live change:", {k: v - before.get(k, 0) for k, v in S.CALLS.items() if v != before.get(k, 0)})
	step("buffer after the update (vbufChangeNotify)", lambda: None, seconds=0.4)
	print("buffer text now:", repr(ti.makeTextInfo(textInfos.POSITION_ALL).text))


import traceback
try:
	from comtypes.automation import IDispatch as _ID
	for key in ("doc", "heading"):
		c = nodes[key].com
		print("diag", key, type(c).__name__, "refcnt", c._refcnt.value, "ptrs", len(c._com_pointers_), str(_ID._iid_) in {str(k) for k in c._com_pointers_})
	try:
		print("diag raw_itf heading:", S.raw_itf(nodes["heading"].com, _ID))
		print("diag accChild direct:", nodes["doc"].com.QueryInterface(Acc.IAccessible).accChild(nodes["heading"].uid))
	except Exception:
		traceback.print_exc(file=sys.stdout)
	import oleacc as _oa
	for key in ("doc", "heading", "edit"):
		pacc, cid = _oa.AccessibleObjectFromEvent(hwnd, OBJID_CLIENT, nodes[key].uid)
		ia2 = IAccessibleHandler.normalizeIAccessible(pacc, cid)
		print("diag AOFE", key, nodes[key].uid, "-> childID", cid, "uniqueID", getattr(ia2, "uniqueID", None))
	o = NVDAObjects.IAccessible.getNVDAObjectFromEvent(hwnd, OBJID_CLIENT, nodes["edit"].uid)
	print("diag edit object:", type(o).__mro__[:4], "IA2:", type(o.IAccessibleObject), "states:", o.states, "role:", o.role)
except Exception:
	traceback.print_exc(file=sys.stdout)
print("=== speech")
for k, v in results.items():
	print(f"{k}: {v}")
print("=== patches")
for p in PATCHES:
	print("  ", p)
print("=== stand-in members called")
print("  ", sorted(S.CALLS.items()))
print("=== NVDA log (warnings and above)")
for line in bootb.LOGLINES[:30]:
	print("  ", line[:1500])

