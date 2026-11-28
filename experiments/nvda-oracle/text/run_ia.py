"""Drive NVDA's IAccessibleHandler path (winEvent at winEventCallback, orderedWinEventLimiter,
pumpAll, accessibleObjectFromEvent) over stand-in MSAA objects for the Run dialog, and compare with
target/captures-2026-11-20/s9-run-dialog-tab-output/run-tab-1.jsonl (Tab from "Open" to OK).

Usage: python run_ia.py   (NVDA_LIB set to the installed NVDA's lib/x64 path)"""
import sys, os, threading, json, ctypes

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401
import standins as S
import msaa as M
from msaa import Acc

import config, baseObject, queueHandler, api, appModuleHandler, speech, winUser, watchdog, winKernel
from speech.extensions import pre_speechQueued
from comInterfaces import UIAutomationClient as UIA

PATCHES = []
config.conf["UIA"]["enhancedEventProcessing"] = "disabled"
PID, TID = 6032, 6036  # the capture's NVDA log names rundll32 as process 6032
DESKTOP, DIALOG, ICON, TEXT, LABEL, COMBO, EDIT, OK, CANCEL, BROWSE = (
	0x10010, 0x5A0B2C, 0x2C0B30, 0x2C0B32, 0x2C0B34, 0x2C0B36, 0x2C0B38, 0x2C0B3A, 0x2C0B3C, 0x2C0B3E)
CHILD = 0x50000000  # WS_CHILD | WS_VISIBLE
ww = S.WinWorld([
	S.Window(DESKTOP, "#32769", "", 0, 600, 604),
	S.Window(DIALOG, "#32770", "Run", DESKTOP, PID, TID, style=0x94C808CC),
	S.Window(ICON, "Static", "", DIALOG, PID, TID, style=CHILD | 0x3, controlID=12297),
	S.Window(TEXT, "Static", "Type the name of a program, folder, document, or Internet resource, and Windows will open it for you.", DIALOG, PID, TID, style=CHILD, controlID=12289),
	S.Window(LABEL, "Static", "&Open:", DIALOG, PID, TID, style=CHILD, controlID=12305),
	S.Window(COMBO, "ComboBox", "", DIALOG, PID, TID, style=CHILD | 0x00010000 | 0x42, controlID=12298),
	S.Window(EDIT, "Edit", "notepad", COMBO, PID, TID, style=CHILD | 0x80, controlID=1001),
	S.Window(OK, "Button", "OK", DIALOG, PID, TID, style=CHILD | 0x00010000 | 0x1, controlID=1),
	S.Window(CANCEL, "Button", "Cancel", DIALOG, PID, TID, style=CHILD | 0x00010000, controlID=2),
	S.Window(BROWSE, "Button", "&Browse...", DIALOG, PID, TID, style=CHILD | 0x00010000, controlID=12288),
], DESKTOP, DIALOG, EDIT)

# The MSAA world: what Windows' oleacc proxies give for each window (written by hand, see report).
mw = M.MsaaWorld()
_, deskC = mw.window("desktop", DESKTOP, None, Acc("desktop", 0, 0, "client", name="Desktop", states=("focusable",)), wname="Desktop")
_, dlgC = mw.window("dialog", DIALOG, deskC, Acc("dialog", 0, 0, "dialog", name="Run", states=("focusable",)), wname="Run")
mw.window("icon", ICON, dlgC, Acc("icon", 0, 0, "graphic", states=("readonly",)))
mw.window("text", TEXT, dlgC, Acc("text", 0, 0, "statictext", name=ww.w[TEXT].text, states=("readonly",)), wname=ww.w[TEXT].text)
mw.window("label", LABEL, dlgC, Acc("label", 0, 0, "statictext", name="Open:", shortcut="Alt+o", states=("readonly",)), wname="Open:")
_, comboC = mw.window("combo", COMBO, dlgC, Acc("combo", 0, 0, "combobox", name="Open:", value="notepad", shortcut="Alt+o", states=("focusable", "collapsed")))
_, editC = mw.window("edit", EDIT, comboC, Acc("edit", 0, 0, "text", name="Open:", value="notepad", shortcut="Alt+o", states=("focusable", "focused")), wname="notepad")
_, okC = mw.window("ok", OK, dlgC, Acc("ok", 0, 0, "pushbutton", name="OK", defaultAction="Press", states=("focusable", "default")), wname="OK")
mw.window("cancel", CANCEL, dlgC, Acc("cancel", 0, 0, "pushbutton", name="Cancel", defaultAction="Press", states=("focusable",)), wname="Cancel")
mw.window("browse", BROWSE, dlgC, Acc("browse", 0, 0, "pushbutton", name="Browse...", shortcut="Alt+b", defaultAction="Press", states=("focusable",)), wname="Browse...")

S.install_window_standins(ww)
M.install_oleacc_standins(mw)
import winBindings.user32 as user32

user32._GhostWindowFromHungWindow = lambda h: 0
PATCHES.append("user32._GhostWindowFromHungWindow: no ghost window (the application is not hung)")
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: {PID: "rundll32", 600: "csrss"}[pid] + (".exe" if includeExt else "")
appModuleHandler.getProcessHandleFromProcessId = lambda pid, fallBackToTopLevelWindowEnumeration=True: 0x9000 + pid

# The Edit window's text, answered to the messages NVDA's window.edit.Edit sends.
edit_sel = [0, 7]
_send = watchdog.cancellableSendMessage


def editMessages(h, msg, wp, lp, flags=0, timeout=60000):
	if h == EDIT:
		t = ww.w[EDIT].text
		if msg == winUser.EM_GETSEL:
			wp._obj.value, lp._obj.value = edit_sel
			return edit_sel[0] | (edit_sel[1] << 16)
		if msg == winUser.EM_LINEFROMCHAR:
			return 0
		if msg == winUser.EM_LINEINDEX:
			return 0
		if msg == winUser.EM_LINELENGTH:
			return len(t)
		if msg == winUser.EM_GETLINECOUNT:
			return 1
	return _send(h, msg, wp, lp, flags, timeout)


watchdog.cancellableSendMessage = editMessages

# The UIA client, for UIAHandler.isUIAWindow (every window here answers UiaHasServerSideProvider False).
class World:
	pass


import world_common as WC

uworld = World()
uworld.root = S.Node("root", WC.common(UIA.UIA_PaneControlTypeId, "Desktop 1", "#32769", "Win32", hwnd=DESKTOP, pid=600,
	LocalizedControlType="pane", RuntimeId=(42, DESKTOP)), patterns={UIA.UIA_TextPatternId: None})
uworld.focused = uworld.root
uworld.byHwnd = {DESKTOP: uworld.root}
automation = S.register(S.Automation(uworld))
import UIAHandler

UIAHandler.CoCreateInstance = lambda clsid, interface=None, clsctx=None: automation.QueryInterface(interface)
UIAHandler.UIARemote.initialize = lambda *a: S.called("UIARemote.initialize (skipped)")
UIAHandler.initialize()
handler = UIAHandler.handler

import core

core.requestPump = lambda immediate=False: S.called(f"core.requestPump(immediate={immediate})")
PATCHES.append("core.requestPump: recorded; the oracle pumps IAccessibleHandler itself (no wx main loop)")
import IAccessibleHandler
from IAccessibleHandler import internalWinEventHandler as IWE

# Not IAccessibleHandler.initialize(): it hooks real winEvents and creates CAccPropServices; the oracle
# injects at winEventCallback, and getIAccIdentity then parses identity strings itself.
PATCHES.append("IAccessibleHandler.initialize not run (it sets real winEvent hooks); events enter at winEventCallback")
import NVDAObjects.window

desktopObject = NVDAObjects.window.Window(windowHandle=DESKTOP)
api.setDesktopObject(desktopObject)
api.setForegroundObject(desktopObject)
api.setFocusObject(desktopObject)
api.setNavigatorObject(desktopObject)

import synthDriverHandler
from speech.commands import IndexCommand

_synth = synthDriverHandler.getSynth()


def _synthSpeak(seq):
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
queued = []
def _onQueued(speechSequence, **kw):
	# The capture's add-on before 2026-11-21: pre_speechQueued, string items joined with no separator.
	queued.append("".join(x for x in speechSequence if isinstance(x, str)).strip())


pre_speechQueued.register(_onQueued)  # extension points hold handlers weakly: a named function


def drain_mta():
	done = threading.Event()
	handler.MTAThreadQueue.put_nowait(done.set)
	done.wait(5)


def winEvent(eventID, hwnd, objid=M.OBJID_CLIENT, child=0):
	"""Windows calls NVDA's hook; NVDA's core then pumps IAccessibleHandler and its queue."""
	IWE.winEventCallback(None, eventID, hwnd, objid, child, TID, 0)
	IAccessibleHandler.pumpAll()
	for _ in range(3):
		queueHandler.pumpAll()
		drain_mta()
	baseObject.AutoPropertyObject.invalidateCaches()


def capture(path):
	steps = []
	for line in open(path, encoding="utf-8"):
		line = line.strip()
		if line.startswith("{"):
			o = json.loads(line)
			if "label" in o:
				steps.append([o["label"], []])
			elif o.get("kind") == "speech":
				steps[-1][1].append(o["text"])
	return steps


CAP = capture(r"<repository>\target\captures-2026-11-20\s9-run-dialog-tab-output\run-tab-1.jsonl")
out = []
# The dialog opens: Windows fires foreground on it, then focus on the edit.
queued.clear()
winEvent(winUser.EVENT_SYSTEM_FOREGROUND, DIALOG)
winEvent(winUser.EVENT_OBJECT_FOCUS, EDIT)
out.append(("dialog shown (foreground, then focus on the edit)", list(queued), CAP[1][1]))
print("focus:", api.getFocusObject(), [c.__name__ for c in type(api.getFocusObject()).__mro__][:6])
# Tab: the edit loses focus, the OK button takes it.
editC.states.discard("focused")
okC.states.add("focused")
ww.focusHwnd = OK
queued.clear()
speech.cancelSpeech()
winEvent(winUser.EVENT_OBJECT_FOCUS, OK)
out.append(("tab", list(queued), CAP[2][1]))
print("focus:", api.getFocusObject(), [c.__name__ for c in type(api.getFocusObject()).__mro__][:6])
# Sensitivity: the same event with OK pressed-looking state changes; and a late focus event for the edit.
mark = len(queued)
winEvent(winUser.EVENT_OBJECT_FOCUS, OK)
out.append(("tab: duplicate focus event on OK", queued[mark:], []))
okC.states.discard("focused")
okC.props["name"] = "Run it"
mark = len(queued)
winEvent(winUser.EVENT_OBJECT_FOCUS, CANCEL)
out.append(("focus event on Cancel, which lacks the focused state (dropped by shouldAllowIAccessibleFocusEvent)", queued[mark:], []))

same = 0
print("=== speech (oracle vs capture)")
for label, got, expect in out:
	ok = got == expect
	same += ok
	print(f"{label}: {got}" + ("  SAME" if ok else f"  DIFFERENT, capture: {expect}"))
print(f"=== {same} of {len(out)} the same")
print("=== misses")
for m in S.MISSES:
	print("  ", m)
print("=== members called")
for m, n in S.CALLS.items():
	print(f"   {n:5d} {m}")
print("=== patches")
for p in PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES:
	if "setSynth failed" in line or "script decorator" in line or "VoiceSpeechDict" in line:
		continue
	print("  ", line[:1500])
sys.stdout.flush()
os._exit(0)
