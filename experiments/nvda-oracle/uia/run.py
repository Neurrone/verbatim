"""Drive NVDA's UIA focus path from a recorded world and print the speech.

Usage: python run.py   (NVDA_LIB set to the installed NVDA's lib/x64 path)"""
import sys, os, threading, json

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import boot  # noqa: F401  (headless NVDA)
import standins as S
import world_explorer as W

import config
import baseObject
import queueHandler
import api
import appModuleHandler
import speech.speech as sp

PATCHES = []

# Enhanced event processing wraps NVDA's handler in a native rate limiter (nvdaHelperLocal) ahead of
# the handler entry this oracle injects at; off, so no native thread holds the stand-in client.
config.conf["UIA"]["enhancedEventProcessing"] = "disabled"
PATCHES.append("config UIA.enhancedEventProcessing = disabled (native rate limiter sits before the injection point)")

S.DEFAULT_PATTERNS.update(W.NO_PATTERNS)
root, nodes, ww = W.build()


class World:
	pass


world = World()
world.root = root
world.focused = nodes["cabinet"]
world.byHwnd = {n.props[S.PROPID["NativeWindowHandle"]]: n for n in nodes.values() if n.props[S.PROPID["NativeWindowHandle"]]}

S.install_window_standins(ww)
appModuleHandler.getAppNameFromProcessID = lambda pid, includeExt=False: W.PROCESSES[pid] + (".exe" if includeExt else "")
appModuleHandler.getProcessHandleFromProcessId = lambda pid, fallBackToTopLevelWindowEnumeration=True: 0x9000 + pid
PATCHES.append("appModuleHandler.getAppNameFromProcessID / getProcessHandleFromProcessId: from the world's process table")

automation = S.register(S.Automation(world))
import UIAHandler

UIAHandler.CoCreateInstance = lambda clsid, interface=None, clsctx=None: automation.QueryInterface(interface)
PATCHES.append("UIAHandler.CoCreateInstance: returns the stand-in IUIAutomation")
UIAHandler.UIARemote.initialize = lambda *a: S.called("UIARemote.initialize (skipped)")
PATCHES.append("UIAHandler.UIARemote.initialize: skipped (remote operations need the real UIA core)")

UIAHandler.initialize()
handler = UIAHandler.handler
assert handler.clientObject, "MTA thread init"

import NVDAObjects.window

desktopObject = NVDAObjects.window.Window(windowHandle=ww.desktop)
api.setDesktopObject(desktopObject)
api.setForegroundObject(desktopObject)
api.setFocusObject(desktopObject)
api.setNavigatorObject(desktopObject)

spoken = []
sp.speak = lambda seq, symbolLevel=None, priority=None: spoken.append(
	"  ".join(x for x in seq if isinstance(x, str)),
)


def drain_mta():
	done = threading.Event()
	handler.MTAThreadQueue.put_nowait(done.set)
	done.wait(5)


def core_cycle():
	queueHandler.pumpAll()
	drain_mta()
	queueHandler.pumpAll()
	baseObject.AutoPropertyObject.invalidateCaches()


def focus(key):
	"""The application moves its focus, then UIA raises the focus event with the registered cache."""
	old = world.focused
	old.props[S.PROPID["HasKeyboardFocus"]] = False
	new = nodes[key]
	new.props[S.PROPID["HasKeyboardFocus"]] = True
	world.focused = new
	ww.focusHwnd = W.BRIDGE
	sender = S.make_element(new, automation.focusCacheRequest).QueryInterface(UIAHandler.UIA.IUIAutomationElement)
	spoken.clear()
	handler.IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent(sender)
	core_cycle()
	return list(spoken)


def replay(key):
	"""The same element raises a second focus event, with no change in the world."""
	sender = S.make_element(nodes[key], automation.focusCacheRequest).QueryInterface(UIAHandler.UIA.IUIAutomationElement)
	spoken.clear()
	handler.IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent(sender)
	core_cycle()
	return list(spoken)


CAPTURE = {
	"moreOptions": ["More options  button"],
	"details": ["Details  button  Show or hide the details pane.  1 of 1"],
}

results = {}
results["search (setup, not compared)"] = focus("search")
results["moreOptions"] = focus("moreOptions")
results["details"] = focus("details")

import logging
from logHandler import log as nvdaLog

config.conf["debugLog"]["UIA"] = True
nvdaLog.setLevel(logging.DEBUG)
mark = len(boot.LOGLINES)
results["details again (duplicate)"] = replay("details")
dupLog = [l.splitlines()[0] for l in boot.LOGLINES[mark:] if "ocus" in l]
mark = len(boot.LOGLINES)
results["moreOptions late event, focus now on details"] = replay("moreOptions")
lateLog = [l.splitlines()[0] for l in boot.LOGLINES[mark:] if "ocus" in l]
nvdaLog.setLevel(logging.WARNING)
config.conf["debugLog"]["UIA"] = False
# Sensitivity: the same world with one recorded fact changed must change the speech.
nodes["moreOptions"].props[S.PROPID["ToggleToggleState"]] = UIAHandler.UIA.ToggleState_On
results["moreOptions with a toggle state On (sensitivity)"] = focus("moreOptions")

print("=== speech")
for k, v in results.items():
	mark = ""
	if k in CAPTURE:
		mark = "  SAME" if v == CAPTURE[k] else f"  DIFFERENT, capture: {CAPTURE[k]}"
	print(f"{k}: {v}{mark}")
print("=== UIA debug, duplicate:", *dupLog, sep="\n   ")
print("=== UIA debug, late event:", *lateLog, sep="\n   ")
print("=== misses")
for m in S.MISSES:
	print("  ", m)
print("=== members called")
for m, n in S.CALLS.items():
	print(f"   {n:4d} {m}")
print("=== patches")
for p in PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES:
	print("  ", line[:2500])
print("event handler groups registered on:", automation.groupRegistrations)
