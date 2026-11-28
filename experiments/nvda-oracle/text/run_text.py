"""Drive NVDA's UIA text path (review cursor by character, word and line; caret keys in an edit)
over a stand-in Windows 11 Notepad document backed by textmodel.TextModel, and compare with the
live capture target/captures-2026-11-20/s10-review-words-output/notepad.jsonl.

Usage: python run_text.py [--mixed]   (NVDA_LIB set to the installed NVDA's lib/x64 path)"""
import sys, os, threading, json

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401  (headless NVDA)
import standins as S
import textmodel as T

import config
import baseObject
import queueHandler
import api
import appModuleHandler
import speech
import speech.speech as sp
from speech.extensions import pre_speech, pre_speechQueued, speechCanceled
from comInterfaces import UIAutomationClient as UIA

PATCHES = []
config.conf["UIA"]["enhancedEventProcessing"] = "disabled"
PATCHES.append("config UIA.enhancedEventProcessing = disabled (native rate limiter sits before the injection point)")

PID, TID = 8120, 8124
DESKTOP, NOTEPAD, DOC = 0x10010, 0x370F0A, 0x2B0C1E
TITLE = "verbatim-e2e-review-words.txt - Notepad"
CAPTURE_TEXT = "Fruit   Color   Price\r\nApple   red     1.20\r\nFig     purple\r\nBanana  yellow  0.50\r\n"
# RichEdit keeps paragraphs ended by CR alone and always ends its story with one more CR.
TEXT = CAPTURE_TEXT.replace("\r\n", "\r") + "\r"

MIXED = "--mixed" in sys.argv
MSAA_FRAME = "--msaa-frame" in sys.argv
if MIXED:
	config.conf["documentFormatting"]["fontAttributeReporting"] = 1  # speech
	PATCHES.append("documentFormatting.fontAttributeReporting = speech (the mixed-attribute check only)")

defaults = {
	"Culture": 2057,  # en-GB, as the capture's NVDA log speaks
	"IsReadOnly": False,
	"StyleId": UIA.StyleId_Normal,
	"AnnotationTypes": (),
	"FontWeight": 400,
	"IsItalic": False,
	"UnderlineStyle": 0,
	"StrikethroughStyle": 0,
}
runs = []
if MIXED:
	# "red" in bold: the line "Apple   red     1.20" has mixed FontWeight.
	i = TEXT.index("red")
	runs = [(i, i + 3, {"FontWeight": 700})]
model = T.TextModel(TEXT, runs=runs, defaults=defaults, final_mark=True, caret=0)
pattern = S.register(T.TextPattern(model))

from standins import Node, Window, WinWorld, NS
import world_common as WC

doc = Node(
	"doc",
	WC.common(UIA.UIA_DocumentControlTypeId, "Text editor", "RichEditD2DPT", "Win32", hwnd=DOC, pid=PID,
		LocalizedControlType="document", RuntimeId=(42, DOC), IsTextPatternAvailable=True,
		ProviderDescription="[pid:%d,providerId:0x%X Main:Nonclient:Microsoft: Non-Client Proxy (unmanaged:uiautomationcore.dll); Main:Microsoft: RichEdit Proxy (unmanaged:msftedit.dll)]" % (PID, DOC)),
	patterns={UIA.UIA_TextPatternId: pattern, UIA.UIA_TextPattern2Id: pattern, UIA.UIA_TextEditPatternId: None,
		UIA.UIA_ValuePatternId: None, UIA.UIA_LegacyIAccessiblePatternId: None},
)
model.element = doc
notepad = Node(
	"notepad",
	WC.common(UIA.UIA_WindowControlTypeId, TITLE, "Notepad", "Win32", hwnd=NOTEPAD, pid=PID,
		LocalizedControlType="window", RuntimeId=(42, NOTEPAD)),
	patterns={UIA.UIA_TextPatternId: None},
	children=[doc],
)
root = Node(
	"root",
	WC.common(UIA.UIA_PaneControlTypeId, "Desktop 1", "#32769", "Win32", hwnd=DESKTOP, pid=600,
		LocalizedControlType="pane", RuntimeId=(42, DESKTOP)),
	patterns={UIA.UIA_TextPatternId: None},
	children=[notepad],
)
nodes = {n.key: n for n in (root, notepad, doc)}
ww = WinWorld([
	Window(DESKTOP, "#32769", "", 0, 600, 604),
	Window(NOTEPAD, "Notepad", TITLE, DESKTOP, PID, TID, uiaProvider=not MSAA_FRAME),
	Window(DOC, "RichEditD2DPT", "", NOTEPAD, PID, TID, style=0x56000000, uiaProvider=True),
], DESKTOP, NOTEPAD, DOC)
if not MSAA_FRAME:
	PATCHES.append("the Notepad frame answers UiaHasServerSideProvider (UIA-only run; --msaa-frame gives the frame no UIA provider)")

# The MSAA side: the desktop always (NVDA's desktop object is IAccessible), the frame with --msaa-frame.
import msaa as M

mw = M.MsaaWorld()
_, deskC = mw.window("desktop", DESKTOP, None, M.Acc("desktop", 0, 0, "client", name="Desktop", states=("focusable",)), wname="Desktop")
if MSAA_FRAME:
	_, frameC = mw.window("frame", NOTEPAD, deskC, M.Acc("frame", 0, 0, "client", name=TITLE, states=("focusable",)), wname=TITLE)


class World:
	pass


world = World()
world.root = root
world.focused = notepad
world.byHwnd = {DESKTOP: root, NOTEPAD: notepad, DOC: doc}

import winKernel
_lcid = winKernel.LCIDToLocaleName
S.install_window_standins(ww)
winKernel.LCIDToLocaleName = _lcid  # a pure table lookup in NVDA's own process
M.install_oleacc_standins(mw)
import winBindings.user32 as user32
user32._GhostWindowFromHungWindow = lambda h: 0
PATCHES.append("winKernel.LCIDToLocaleName: the real call (a locale table lookup, no application state)")
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: {PID: "notepad", 600: "csrss"}[pid] + (".exe" if includeExt else "")
appModuleHandler.getProcessHandleFromProcessId = lambda pid, fallBackToTopLevelWindowEnumeration=True: 0x9000 + pid
PATCHES.append("appModuleHandler.getAppNameFromProcessID / getProcessHandleFromProcessId: from the world's process table")

automation = S.register(S.Automation(world))
import UIAHandler

UIAHandler.CoCreateInstance = lambda clsid, interface=None, clsctx=None: automation.QueryInterface(interface)
if "--remote-init" not in sys.argv:
	UIAHandler.UIARemote.initialize = lambda *a: S.called("UIARemote.initialize (skipped)")
	PATCHES.append("UIAHandler.UIARemote.initialize: skipped (remote operations need the real UIA core)")
else:
	PATCHES.append("UIAHandler.UIARemote.initialize: the real one (it only checks the Windows version)")
UIAHandler.initialize()
handler = UIAHandler.handler

import NVDAObjects.window

desktopObject = NVDAObjects.window.Window(windowHandle=DESKTOP)
api.setDesktopObject(desktopObject)
api.setForegroundObject(desktopObject)
api.setFocusObject(desktopObject)
api.setNavigatorObject(desktopObject)

spoken = []


def _onSpeech(speechSequence, **kw):
	spoken.append(" ".join(x for x in speechSequence if isinstance(x, str)).strip())


pre_speech.register(_onSpeech)
queued = []


def _onQueued(speechSequence, **kw):
	# The capture's add-on before 2026-11-21: pre_speechQueued, string items joined with no separator.
	queued.append("".join(x for x in speechSequence if isinstance(x, str)).strip())


pre_speechQueued.register(_onQueued)
synthText = []
import synthDriverHandler
from speech.commands import IndexCommand

_synth = synthDriverHandler.getSynth()


def _synthSpeak(seq):
	"""After symbol processing: what the synthesizer is given. Reports every index and done, as a
	real synthesizer does, so the speech manager moves to the next utterance."""
	synthText.append("".join(x for x in seq if isinstance(x, str)))
	for x in seq:
		if isinstance(x, IndexCommand):
			synthDriverHandler.synthIndexReached.notify(synth=_synth, index=x.index)
	synthDriverHandler.synthDoneSpeaking.notify(synth=_synth)


_synth.speak = _synthSpeak
_synth.languageIsSupported = lambda lang: True
from speech.commands import CharacterModeCommand, LangChangeCommand, BreakCommand, PitchCommand, RateCommand, VolumeCommand, PhonemeCommand
# The capture's synthesizer is eSpeak NG: its command set decides how the speech manager splits and joins a sequence.
_synth.supportedCommands = {IndexCommand, CharacterModeCommand, LangChangeCommand, BreakCommand, PitchCommand, RateCommand, VolumeCommand, PhonemeCommand}
_synth.supportedNotifications = {synthDriverHandler.synthIndexReached, synthDriverHandler.synthDoneSpeaking}
PATCHES.append("recording synthesizer: eSpeak NG's supportedCommands and notifications")
PATCHES.append("recording synthesizer: supports every language, as the capture's eSpeak NG does English")


def drain_mta():
	done = threading.Event()
	handler.MTAThreadQueue.put_nowait(done.set)
	done.wait(5)


def core_cycle():
	for _ in range(3):
		queueHandler.pumpAll()
		drain_mta()
	baseObject.AutoPropertyObject.invalidateCaches()


def focus(key, hwnd):
	old = world.focused
	old.props[S.PROPID["HasKeyboardFocus"]] = False
	new = nodes[key]
	new.props[S.PROPID["HasKeyboardFocus"]] = True
	world.focused = new
	ww.focusHwnd = hwnd
	sender = S.make_element(new, automation.focusCacheRequest).QueryInterface(UIA.IUIAutomationElement)
	spoken.clear()
	queued.clear()
	handler.IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent(sender)
	core_cycle()
	return [x for x in queued if x]


# -- keys: what the application does with each caret key --
def line_of(pos):
	for n, (s, e) in enumerate(model.lines()):
		if s <= pos < e or (pos == model.N and e == model.N):
			return n, s, e


def app_key(name):
	s, e = model.sel
	pos = e
	last = model.reachable_end()
	if name == "control+home":
		pos = 0
	elif name == "control+end":
		pos = last
	elif name in ("downarrow", "uparrow"):
		lines = model.lines()
		n, ls, le = line_of(pos)
		col = pos - ls
		m = n + (1 if name == "downarrow" else -1)
		if 0 <= m < len(lines):
			ms, me = lines[m]
			pos = min(ms + col, me - 1 if me > ms else ms)
			pos = min(pos, last)
	else:
		raise ValueError(name)
	model.sel = (pos, pos)


class Gesture:
	"""A key NVDA's script sends on to the application."""

	def __init__(self, name):
		self.name = name
		self.wasInSayAll = False
		self.script = None
		self.isModifier = False

	def send(self):
		S.called(f"gesture.send({self.name})")
		app_key(self.name)

	def __repr__(self):
		return self.name


import globalCommands

CARET_SCRIPTS = {"control+home": "caret_moveByLine", "control+end": "caret_moveByLine", "downarrow": "caret_moveByLine", "uparrow": "caret_moveByLine"}
REVIEW_SCRIPTS = {
	"numpad5": "review_currentWord", "numpad6": "review_nextWord", "numpad4": "review_previousWord",
	"numpad1": "review_previousCharacter", "numpad2": "review_currentCharacter", "numpad3": "review_nextCharacter",
	"numpad7": "review_previousLine", "numpad8": "review_currentLine", "numpad9": "review_nextLine",
	"shift+numpad1": "review_startOfLine", "shift+numpad3": "review_endOfLine",
	"shift+numpad7": "review_top", "shift+numpad9": "review_bottom",
}


def key(name):
	speech.cancelSpeech()
	spoken.clear()
	queued.clear()
	g = Gesture(name)
	if name in CARET_SCRIPTS:
		obj = api.getFocusObject()
		getattr(obj, "script_" + CARET_SCRIPTS[name])(g)
	else:
		getattr(globalCommands.commands, "script_" + REVIEW_SCRIPTS[name])(g)
	core_cycle()
	return [x for x in queued if x]


# -- the capture --
CAP = os.path.join(r"<repository>\target\captures-2026-11-20\s10-review-words-output", "notepad.jsonl")
steps = []
for line in open(CAP, encoding="utf-8"):
	line = line.strip()
	if not line.startswith("{"):
		continue
	o = json.loads(line)
	if "label" in o:
		steps.append([o["label"], []])
	elif o.get("kind") == "speech":
		steps[-1][1].append(o["text"])

out = []
if MSAA_FRAME:
	# Windows fires foreground on the MSAA frame (IAccessibleHandler), then UIA focus on the document.
	import core, IAccessibleHandler, winUser as WU
	from IAccessibleHandler import internalWinEventHandler as IWE

	core.requestPump = lambda immediate=False: S.called(f"core.requestPump(immediate={immediate})")
	PATCHES.append("core.requestPump: recorded; the oracle pumps IAccessibleHandler itself")
	queued.clear()
	IWE.winEventCallback(None, WU.EVENT_SYSTEM_FOREGROUND, NOTEPAD, 0, 0, TID, 0)
	IAccessibleHandler.pumpAll()
	core_cycle()
	fg = [x for x in queued if x]
	print("foreground object:", api.getForegroundObject(), type(api.getForegroundObject()).__mro__[:3])
	r = focus("doc", DOC)
	out.append(("foreground on the MSAA frame, then UIA focus on the document", fg + r, steps[1][1]))
	anc = [type(o).__name__ + ":" + str(o.name) for o in api.getFocusAncestors()]
	print("focus ancestors:", anc)
else:
	r = focus("notepad", NOTEPAD)
	out.append(("focus Notepad window (setup)", r, None))
	r = focus("doc", DOC)
	out.append(("focus document", r, steps[1][1][1:]))  # the capture's step 1: foreground title, then the focus
print("focused object classes:", [c.__name__ for c in type(api.getFocusObject()).__mro__][:8])
for label, expect in steps[2:]:
	got = key(label)
	out.append((label, got, expect))

# Beyond the capture: the expectation is what NVDA's scripts say they do (globalCommands review_*,
# editableText caret_moveByLine), written before the run; marked "rule" in the output.
RULE_STEPS = [
	("control+home", ["Fruit   Color   Price"]),
	("numpad3", ["r"]),
	("numpad3", ["u"]),
	("numpad1", ["r"]),
	("numpad2", ["r"]),
	("numpad9", ["Apple   red 1.20"]),
	("numpad8", ["Apple   red 1.20"]),
	("shift+numpad3", ["carriage return"]),
	("uparrow", ["Fruit   Color   Price"]),  # the caret cannot rise from the first line: NVDA re-reads it after its timeout
	("downarrow", ["Apple   red 1.20"]),
	("downarrow", ["Fig purple"]),
	("shift+numpad7", ["Fruit   Color   Price"]),
	("numpad7", ["Top", "Fruit   Color   Price"]),
]
if MIXED:
	RULE_STEPS = [("control+home", ["Fruit   Color   Price"]), ("downarrow", ["Apple   bold red not bold 1.20"]), ("numpad5", ["Apple"]), ("numpad6", ["bold red"])]
for label, expect in RULE_STEPS:
	out.append((label + " (rule)", key(label), expect))

same = 0
print("=== speech (oracle vs capture)")
for label, got, expect in out:
	mark = ""
	if expect is not None:
		ok = got == expect
		same += ok
		mark = "  SAME" if ok else f"  DIFFERENT, capture: {expect}"
	print(f"{label}: {got}{mark}")
print(f"=== {same} of {sum(1 for x in out if x[2] is not None)} steps the same")
print("=== synthesizer text after symbol processing (last 6)")
for t in synthText[-6:]:
	print("  ", repr(t))
print("=== misses")
for m in S.MISSES:
	print("  ", m)
print("=== members called")
for m, n in S.CALLS.items():
	if "TextRange" in m or "TextPattern" in m or "Array" in m or "gesture" in m:
		print(f"   {n:5d} {m}")
print("=== patches")
for p in PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES:
	if "setSynth failed" in line or "script decorator" in line or "VoiceSpeechDict" in line:
		continue  # boot noise: no real synthesizer loads here; gui stub decorators
	print("  ", line[:2000])
sys.stdout.flush()
os._exit(0)
