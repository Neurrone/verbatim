"""Option (a): NVDA's shipped gecko_ia2 vbuf backend, run in this process (no injection anywhere else),
built from in-process IAccessible2 stand-ins, read back through NVDA's own VBuf_* RPC client stubs.

Only this process is touched: initInprocManagerThreadIfNeeded hooks winEvents for this process id only;
the global injection_initialize is never called. The RPC binding is to this process's own dynamic endpoint."""
import os, sys, ctypes, threading, time
from ctypes import POINTER, byref, c_void_p, c_int, c_long, c_bool, c_ulonglong, c_wchar_p, WINFUNCTYPE
from ctypes import wintypes as w

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import ia2standins as S
from ia2standins import Node, IA2, Acc
import comtypes, logging
logging.basicConfig(level=logging.WARNING)
from comtypes import BSTR

LIB = os.environ["NVDA_LIB"]
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u32 = ctypes.WinDLL("user32", use_last_error=True)
oleacc = ctypes.WinDLL("oleacc")
rpcrt4 = ctypes.WinDLL("rpcrt4")
k32.LoadLibraryExW.restype = c_void_p
k32.LoadLibraryExW.argtypes = [w.LPCWSTR, c_void_p, w.DWORD]

# Roles and states (MSAA / IA2 constants)
ROLE_DOCUMENT, ROLE_LINK, ROLE_PUSHBUTTON, ROLE_TEXT, ROLE_TABLE, ROLE_ROW, ROLE_CELL, ROLE_COLHDR, ROLE_STATIC = 0x0F, 0x1E, 0x2B, 0x2A, 0x18, 0x1C, 0x1D, 0x19, 0x29
FOCUSABLE, READONLY, LINKED, FOCUSED = 0x100000, 0x40, 0x400000, 0x4
IA2_EDITABLE, IA2_SINGLE_LINE = 0x8, 0x2000


def build_page():
	label = Node(IA2.IA2_ROLE_LABEL, content=["Search"], attrs={"tag": "label", "display": "inline"})
	edit = Node(ROLE_TEXT, name="Search", states=FOCUSABLE, ia2states=IA2_EDITABLE | IA2_SINGLE_LINE,
		attrs={"tag": "input", "explicit-name": "true", "display": "inline-block"}, content=[""], labelledBy=label)
	link = Node(ROLE_LINK, name="docs", states=FOCUSABLE | LINKED, attrs={"tag": "a", "display": "inline"},
		content=["docs"], actions=["jump"], value="https://example.test/docs")
	t = Node(ROLE_TABLE, attrs={"tag": "table", "display": "table"}, table=(2, 2))
	h1 = Node(ROLE_COLHDR, content=["Name"], attrs={"tag": "th"}, cell=(0, 0, t))
	h2 = Node(ROLE_COLHDR, content=["Size"], attrs={"tag": "th"}, cell=(0, 1, t))
	c1 = Node(ROLE_CELL, content=["a.txt"], attrs={"tag": "td"}, cell=(1, 0, t))
	c2 = Node(ROLE_CELL, content=["3 KB"], attrs={"tag": "td"}, cell=(1, 1, t))
	r1 = Node(ROLE_ROW, children=[h1, h2], attrs={"tag": "tr"})
	r2 = Node(ROLE_ROW, children=[c1, c2], attrs={"tag": "tr"})
	t.childList = [r1, r2]
	doc = Node(ROLE_DOCUMENT, name="Test page", states=FOCUSABLE | READONLY | FOCUSED, attrs={"tag": "#document"}, content=[
		Node(IA2.IA2_ROLE_LANDMARK, attrs={"xml-roles": "navigation", "tag": "nav"}, content=[
			Node(IA2.IA2_ROLE_PARAGRAPH, attrs={"tag": "p"}, content=["Read the ", link, " first."]),
		]),
		Node(IA2.IA2_ROLE_HEADING, name="Welcome", attrs={"tag": "h2", "level": "2"}, content=["Welcome"]),
		Node(IA2.IA2_ROLE_PARAGRAPH, attrs={"tag": "p"}, content=[label, " ", edit]),
		Node(ROLE_PUSHBUTTON, name="Go", states=FOCUSABLE, attrs={"tag": "button", "display": "inline-block"}, content=["Go"], actions=["press"]),
		t,
	])
	return doc


WNDPROC = WINFUNCTYPE(c_ssize := ctypes.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
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


WM_GETOBJECT, OBJID_CLIENT = 0x3D, -4
EVENT_OBJECT_FOCUS = 0x8005
state = {}
ready = threading.Event()


def render_thread():
	comtypes.CoInitialize()

	@WNDPROC
	def wndproc(hwnd, msg, wp, lp):
		if msg == WM_GETOBJECT and ctypes.c_long(lp & 0xFFFFFFFF).value == OBJID_CLIENT and "doc" in state:
			S._count("WM_GETOBJECT")
			pacc = state["doc"].com.QueryInterface(Acc.IAccessible)
			return oleacc.LresultFromObject(byref(Acc.IAccessible._iid_), wp, ctypes.cast(pacc, c_void_p))
		return u32.DefWindowProcW(hwnd, msg, wp, lp)

	state["wndproc"] = wndproc
	wc = WNDCLASSW(lpfnWndProc=wndproc, lpszClassName="Chrome_RenderWidgetHostHWND")
	assert u32.RegisterClassW(byref(wc)), ctypes.get_last_error()
	HWND_MESSAGE = w.HWND(-3)
	hwnd = u32.CreateWindowExW(0, "Chrome_RenderWidgetHostHWND", "stand-in", 0, 0, 0, 0, 0, HWND_MESSAGE, None, None, None)
	assert hwnd
	world = S.World(hwnd)
	S.WORLD = world
	doc = build_page()
	doc.attach(world)
	state.update(hwnd=hwnd, doc=doc, world=world, tid=k32.GetCurrentThreadId())
	# An in-context winEvent on this thread makes nvdaHelperRemote hook it (inproc_winEventCallback), which
	# is what lets execInThread run the backend's render here.
	u32.NotifyWinEvent(EVENT_OBJECT_FOCUS, hwnd, OBJID_CLIENT, doc.uid)
	ready.set()
	msg = w.MSG()
	while u32.GetMessageW(byref(msg), None, 0, 0) > 0:
		if msg.message == 0x8000 + 1:  # WM_APP+1: run a queued callable on this thread
			state.pop("call")()
			continue
		u32.TranslateMessage(byref(msg))
		u32.DispatchMessageW(byref(msg))


def on_render_thread(fn):
	done = threading.Event()
	box = {}

	def run():
		box["r"] = fn()
		done.set()

	state["call"] = run
	u32.PostThreadMessageW(state["tid"], 0x8000 + 1, 0, 0)
	done.wait(10)
	return box.get("r")


def self_binding():
	"""A binding handle to this process's own ncalrpc endpoint, which nvdaHelperRemote's rpcSrv registered."""
	vec = c_void_p()
	assert rpcrt4.RpcServerInqBindings(byref(vec)) == 0
	count = ctypes.cast(vec, POINTER(w.ULONG))[0]
	handles = ctypes.cast(vec.value + ctypes.sizeof(c_void_p), POINTER(c_void_p))
	for i in range(count):
		sb = c_wchar_p()
		rpcrt4.RpcBindingToStringBindingW(c_void_p(handles[i]), byref(sb))
		s = sb.value
		if s.startswith("ncalrpc"):
			h = c_void_p()
			assert rpcrt4.RpcBindingFromStringBindingW(c_wchar_p(s), byref(h)) == 0
			return h, s
	raise RuntimeError("no ncalrpc binding")


REG = {}
NOTES = []
_KEEP = []


def wire_controller(local):
	"""Do what NVDAHelper.initialize does for its own process: point nvdaHelperLocal's controller callbacks at
	Python, start its NvdaCtlr server, and take the inproc registration the remote sends back."""
	FT = ctypes.WINFUNCTYPE

	@FT(c_long, c_wchar_p)
	def requestRegistration(uuid):
		local.createRemoteBindingHandle.restype = c_void_p
		local.createRemoteBindingHandle.argtypes = [c_wchar_p]
		REG["uuid"] = uuid
		REG["binding"] = local.createRemoteBindingHandle(uuid)
		h = c_void_p()
		local.nvdaInProcUtils_registerNVDAProcess.argtypes = [c_void_p, POINTER(c_void_p)]
		REG["reg"] = local.nvdaInProcUtils_registerNVDAProcess(REG["binding"], byref(h))
		REG["regHandle"] = h
		return 0

	@FT(c_long, c_long, c_long, c_wchar_p)
	def logMessage(level, pid, msg):
		NOTES.append(f"log {level}: {msg.strip()[:300]}")
		return 0

	@FT(c_long, c_int, c_int)
	def vbufChangeNotify(docHandle, ID):
		NOTES.append(f"vbufChangeNotify {docHandle} {ID}")
		return 0

	@FT(c_long, c_wchar_p, c_wchar_p)
	def reportLiveRegion(text, politeness):
		NOTES.append(f"reportLiveRegion {text!r} {politeness}")
		return 0

	def noop(n):
		@FT(c_long, *([c_void_p] * n))
		def f(*a):
			return 0
		return f

	table = {
		"_nvdaControllerInternal_requestRegistration": requestRegistration,
		"_nvdaControllerInternal_logMessage": logMessage,
		"_nvdaControllerInternal_vbufChangeNotify": vbufChangeNotify,
		"_nvdaControllerInternal_reportLiveRegion": reportLiveRegion,
	}
	for name, n in [("displayModelTextChangeNotify", 5), ("inputLangChangeNotify", 3), ("inputCompositionUpdate", 4),
			("inputCandidateListUpdate", 3), ("IMEOpenStatusUpdate", 1), ("inputConversionModeUpdate", 3),
			("typedCharacterNotify", 1), ("installAddonPackageFromPath", 1), ("handleRemoteURL", 1),
			("drawFocusRectNotify", 5), ("openConfigDirectory", 0)]:
		table["_nvdaControllerInternal_" + name] = noop(n)
	for name, n in [("brailleMessage", 1), ("cancelSpeech", 0), ("speakText", 1), ("speakSsml", 4), ("isSpeaking", 0)]:
		table["_nvdaController_" + name] = noop(n)
	for name, f in table.items():
		_KEEP.append(f)
		ctypes.cast(getattr(local, name), POINTER(c_void_p))[0] = ctypes.cast(f, c_void_p).value
	local.nvdaHelperLocal_initialize(False)


def main():
	local = ctypes.CDLL(os.path.join(LIB, "nvdaHelperLocal.dll"))
	wire_controller(local)
	hRemote = k32.LoadLibraryExW(os.path.join(LIB, "nvdaHelperRemote.dll"), None, 8)  # LOAD_WITH_ALTERED_SEARCH_PATH
	assert hRemote, ctypes.get_last_error()
	remote = ctypes.CDLL("nvdaHelperRemote", handle=hRemote)
	print("initInprocManagerThreadIfNeeded:", remote.initInprocManagerThreadIfNeeded())
	print("registration:", REG)
	t = threading.Thread(target=render_thread, daemon=True)
	t.start()
	ready.wait(10)
	binding = c_void_p(REG["binding"])
	local.VBuf_createBuffer.restype = c_void_p
	local.VBuf_createBuffer.argtypes = [c_void_p, c_int, c_int, c_wchar_p]
	local.VBuf_getTextLength.restype = c_int
	local.VBuf_getTextLength.argtypes = [c_void_p]
	getTextInRange = ctypes.CFUNCTYPE(c_int, c_void_p, c_int, c_int, POINTER(BSTR), c_int)(
		("VBuf_getTextInRange", local), ((1,), (1,), (1,), (2,), (1,)))
	local.VBuf_getLineOffsets.restype = c_int
	local.VBuf_getLineOffsets.argtypes = [c_void_p, c_int, c_int, c_bool, POINTER(c_int), POINTER(c_int)]
	local.VBuf_findNodeByAttributes.restype = c_int
	local.VBuf_findNodeByAttributes.argtypes = [c_void_p, c_int, c_int, c_wchar_p, c_wchar_p, POINTER(c_int), POINTER(c_int), POINTER(c_ulonglong)]
	hwnd, doc = state["hwnd"], state["doc"]
	t0 = time.perf_counter()
	buf = local.VBuf_createBuffer(binding, hwnd, doc.uid, "gecko_ia2")
	t1 = time.perf_counter()
	print(f"createBuffer -> {buf!r} in {1000 * (t1 - t0):.1f} ms")
	if not buf:
		return
	n = local.VBuf_getTextLength(buf)
	print("text length:", n)
	print("plain:", repr(getTextInRange(buf, 0, n, 0)))
	markup = getTextInRange(buf, 0, n, 1)
	open(os.path.join(HERE, "expA_markup.xml"), "w", encoding="utf-8").write(markup)
	print("markup chars:", len(markup))
	# Lines, as NVDA's VirtualBufferTextInfo._getLineOffsets asks (maxLineLength 100, screen layout on)
	off, lines = 0, []
	while off < n:
		s, e = c_int(), c_int()
		local.VBuf_getLineOffsets(buf, off, 100, True, byref(s), byref(e))
		lines.append((s.value, e.value, getTextInRange(buf, s.value, e.value, 0)))
		off = max(e.value, off + 1)
	for l in lines:
		print("line", l)
	# Quick navigation: headings and links, the way browseMode's _iterNodesByType builds its query
	# NVDA's own query builder, executed from the vendored source (virtualBuffers/__init__.py lines 42-94).
	src = open(r"<repository>\nvda\source\virtualBuffers\__init__.py", encoding="utf-8").read().splitlines()
	ns = {}
	exec("\n".join(src[41:94]), ns)
	word = ns["VBufStorage_findMatch_word"]
	for label, query in [
		("heading", {"IAccessible::role": [IA2.IA2_ROLE_HEADING]}),
		("link", {"IAccessible::role": [0x1E], "IAccessible::state_%d" % LINKED: [1]}),
		("landmark", [{"IAccessible::role": [IA2.IA2_ROLE_LANDMARK]}, {"IAccessible2::attribute_xml-roles": [word("navigation"), word("main")]}]),
		("table", {"IAccessible::role": [0x18], "table-layout": [None]}),
		("edit", [{"IAccessible::role": [ROLE_TEXT], "IAccessible::state_%d" % READONLY: [None]}, {"IAccessible2::state_%d" % IA2_EDITABLE: [1]}]),
	]:
		attrs, regexp = ns["_prepareForFindByAttributes"](query)
		s, e, node = c_int(), c_int(), c_ulonglong()
		ok = local.VBuf_findNodeByAttributes(buf, 0, 0, attrs, regexp, byref(s), byref(e), byref(node))  # 0 = forward
		print("find", label, ok, (s.value, e.value), repr(getTextInRange(buf, s.value, e.value, 0)) if ok else "")
	# Incremental update: change the heading's text and fire the events Chromium fires
	heading = [c for c in doc.children if c.role == IA2.IA2_ROLE_HEADING][0]

	def mutate():
		heading.content = ["Welcome back"]
		heading.name = "Welcome back"
		u32.NotifyWinEvent(IA2.IA2_EVENT_TEXT_INSERTED, state["hwnd"], OBJID_CLIENT, heading.uid)
		u32.NotifyWinEvent(0x800C, state["hwnd"], OBJID_CLIENT, heading.uid)  # EVENT_OBJECT_NAMECHANGE

	on_render_thread(mutate)
	# The backend re-renders on its own 100 ms timer on the render thread; wait for the length to change.
	deadline = time.perf_counter() + 3
	while local.VBuf_getTextLength(buf) == n and time.perf_counter() < deadline:
		time.sleep(0.02)
	n2 = local.VBuf_getTextLength(buf)
	print("after update, length", n2, repr(getTextInRange(buf, 0, n2, 0)))
	print("surface:", sorted(S.CALLS.items()))
	print("notes:", *NOTES[:40], sep="\n  ")


if __name__ == "__main__":
	main()

