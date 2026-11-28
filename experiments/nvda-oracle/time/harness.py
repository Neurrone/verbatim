"""Headless NVDA on simulated time: an edit-field world, NVDA's own core pump, its script path,
and a UIA client thread that raises recorded events at recorded simulated times.

Import, then use `H.at(t, action)` to schedule world actions, `H.key(name)` to press a key the way
NVDA's keyboard hook would, and `H.runUntil(t)` to be NVDA's main loop until simulated time t."""
import sys, os, ast, heapq, threading, logging, types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import boot  # noqa: F401  (headless NVDA, simulated time installed first)
import sim
from sim import SIM
import standins as S
import textstandins as T

import config
import api
import appModuleHandler
import core
import queueHandler
import speech.speech as sp
from comInterfaces import UIAutomationClient as UIA
from world_explorer import common, STATE_DEFAULTS  # noqa: F401  (the same property set)

PATCHES = []
config.conf["UIA"]["enhancedEventProcessing"] = "disabled"
PATCHES.append("config UIA.enhancedEventProcessing = disabled (the native limiter is modelled in ratelimit.py)")

# -- the world: a WPF editor window holding one edit --
PID, TID = 5100, 5104
DESKTOP, TOP = 0x10010, 0x4C0A10
S.DEFAULT_PATTERNS.update({UIA.UIA_TextPatternId: None})
DOC = T.Doc("first line\nsecond line\nthird line\n", caret=0)


def build():
	edit = S.Node("edit", common("edit", UIA.UIA_EditControlTypeId, "Body", "TextBox", "WPF",
		LocalizedControlType="edit", RuntimeId=(7, PID, 0x31), AutomationId="Body",
		IsTextPatternAvailable=True, IsValuePatternAvailable=True, ValueIsReadOnly=False,
		ValueValue=DOC.text, pid=PID))
	edit.patterns[UIA.UIA_TextPatternId] = S.register(T.TextPattern(edit, DOC))
	window = S.Node("window", common("window", UIA.UIA_WindowControlTypeId, "Notes", "Window", "WPF",
		hwnd=TOP, LocalizedControlType="window", RuntimeId=(42, TOP), pid=PID), children=[edit])
	root = S.Node("root", common("root", UIA.UIA_PaneControlTypeId, "Desktop 1", "#32769", "Win32", hwnd=DESKTOP,
		pid=600, LocalizedControlType="pane", RuntimeId=(42, DESKTOP)), children=[window])
	windows = [
		S.Window(DESKTOP, "#32769", "", 0, 600, 604),
		S.Window(TOP, "HwndWrapper[Notes;;1]", "Notes", DESKTOP, PID, TID, uiaProvider=True),
	]
	nodes = {n.key: n for n in (root, window, edit)}
	return root, nodes, S.WinWorld(windows, DESKTOP, TOP, TOP)


root, nodes, ww = build()
world = types.SimpleNamespace(root=root, focused=nodes["window"])
world.byHwnd = {n.props[S.PROPID["NativeWindowHandle"]]: n for n in nodes.values() if n.props[S.PROPID["NativeWindowHandle"]]}
HUNG = {}  # hwnd -> simulated time its thread stopped answering messages
S.install_window_standins(ww)
import winUser  # noqa: E402

# IsHungAppWindow: Windows answers true once the window's thread has not taken a message for 5 s.
winUser.isHungAppWindow = lambda h: h in HUNG and SIM.now - HUNG[h] >= 5.0
PATCHES.append("winUser.isHungAppWindow: true 5 s (simulated) after the world marks the window's thread blocked")
_getKeyState = {}
winUser.getKeyState = lambda vk: _getKeyState.get(vk, 0)
PATCHES.append("winUser.getKeyState: no key held (the keyboard hook's state is not modelled)")
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: {PID: "notes", 600: "csrss"}[pid] + (".exe" if includeExt else "")
appModuleHandler.getProcessHandleFromProcessId = lambda pid, fallBackToTopLevelWindowEnumeration=True: 0x9000 + pid

automation = S.register(S.Automation(world))
import UIAHandler  # noqa: E402

UIAHandler.CoCreateInstance = lambda clsid, interface=None, clsctx=None: automation.QueryInterface(interface)
UIAHandler.UIARemote.initialize = lambda *a: S.called("UIARemote.initialize (skipped)")
UIAHandler.initialize()
handler = UIAHandler.handler
assert handler.clientObject, "MTA thread init"

import NVDAObjects.window  # noqa: E402

desk = NVDAObjects.window.Window(windowHandle=ww.desktop)
for f in (api.setDesktopObject, api.setForegroundObject, api.setFocusObject, api.setNavigatorObject):
	f(desk)


# -- NVDA's core pump: the class core.main defines, taken from core.py's own source unchanged --
def _extract_core_pump():
	src = open(core.__file__, encoding="utf-8").read()
	main = next(n for n in ast.parse(src).body if isinstance(n, ast.FunctionDef) and n.name == "main")
	cls = next(n for n in ast.walk(main) if isinstance(n, ast.ClassDef) and n.name == "CorePump")
	mod = ast.Module(body=[cls], type_ignores=[])
	import wx, watchdog, baseObject, vision, braille

	quiet = types.SimpleNamespace(pumpAll=lambda: None)
	ns = dict(vars(core))
	ns.update(wx=wx, watchdog=watchdog, baseObject=baseObject, queueHandler=queueHandler, vision=vision,
		braille=braille, touchHandler=types.SimpleNamespace(handler=None), JABHandler=quiet,
		IAccessibleHandler=quiet, mouseHandler=quiet, sessionTracking=quiet)
	exec(compile(mod, core.__file__, "exec"), ns)
	return ns["CorePump"]


core._pump = _extract_core_pump()()
PATCHES.append("core._pump: core.main's CorePump class compiled from core.py's source (unchanged) on the simulated wx.Timer; "
	"JAB, IAccessible, mouse and session pumps empty (no such sources in the world)")

import inputCore, keyboardHandler  # noqa: E402

inputCore.initialize()
import globalCommands  # noqa: E402,F401  (scriptHandler imports it on the first key; surface import errors here)
SENT = []


def _send(self):
	SENT.append((round(SIM.now, 4), self.displayName))
	for f in list(ON_SEND):
		f(self)


ON_SEND = []
keyboardHandler.KeyboardInputGesture.send = _send
PATCHES.append("KeyboardInputGesture.send: records the key and lets the world react (no SendInput)")

SPOKEN = []


def _speak(seq, symbolLevel=None, priority=None):
	SPOKEN.append((round(SIM.now, 4), "  ".join(x for x in seq if isinstance(x, str))))


sp.speak = _speak
import speech  # noqa: E402

speech.speak = _speak


# -- the UIA client thread: delivers the recorded world's events at their simulated times --
class Client:
	def __init__(self):
		self.heap, self.seq = [], 0
		# A blocked waiter whose wake time changes after it blocked: the scheduler asks for it.
		SIM.wakeHooks.append(lambda: self.heap[0][0] if self.heap else None)
		self.thread = threading.Thread(name="uia-client", target=self.run, daemon=True)
		self.thread.start()

	def at(self, t, action):
		self.seq += 1
		heapq.heappush(self.heap, (t, self.seq, action))

	def run(self):
		import comtypes
		comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
		while True:
			if self.heap and self.heap[0][0] <= SIM.now:
				_, _, action = heapq.heappop(self.heap)
				action()
				# A real client thread can be preempted after any event: let the scheduler choose.
				SIM.yield_()
				continue
			SIM.block(lambda: bool(self.heap) and self.heap[0][0] <= SIM.now,
				self.heap[0][0] if self.heap else None)


client = Client()
at = client.at


def element(key):
	return S.make_element(nodes[key], automation.focusCacheRequest).QueryInterface(UIA.IUIAutomationElement)


def raiseFocus(key):
	old = world.focused
	old.props[S.PROPID["HasKeyboardFocus"]] = False
	world.focused = nodes[key]
	nodes[key].props[S.PROPID["HasKeyboardFocus"]] = True
	handler.IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent(element(key))


def raiseEvent(key, eventId):
	handler.IUIAutomationEventHandler_HandleAutomationEvent(element(key), eventId)


def moveCaret(offset, key="edit"):
	"""The application moves its caret, then raises TextSelectionChanged."""
	DOC.caret = offset
	raiseEvent(key, UIA.UIA_Text_TextSelectionChangedEventId)


VK = {"downArrow": (0x28, 0x50, True), "upArrow": (0x26, 0x48, True)}


def key(name):
	"""A key arrives as from NVDA's keyboard hook: the gesture goes to inputCore.manager.executeGesture."""
	vk, scan, ext = VK[name]
	g = keyboardHandler.KeyboardInputGesture(set(), vk, scan, ext)
	try:
		inputCore.manager.executeGesture(g)
	except inputCore.NoInputGestureAction:
		g.send()


def runUntil(t):
	sim.WX.runUntil(t)


def logDebug(on):
	from logHandler import log
	log.setLevel(logging.DEBUG if on else logging.WARNING)
