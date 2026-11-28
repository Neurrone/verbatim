"""A hung application and NVDA's watchdog (watchdog.py, unchanged) on simulated time.

The kernel objects the watchdog uses (its waitable timer, the cancel-call event) and CoCancelCall
are simulated; watchdog.initialize, alive, asleep, the watcher thread, _shouldRecoverAfterMinTimeout,
waitForFreezeRecovery and _recoverAttempt are NVDA's. The application hangs from the moment a
down arrow is sent; the caret script's first text-pattern call then blocks.

Cases: the application answers after 3 s; it stays hung 20 s (NVDA cancels at its 10 s limit);
and it stays hung but the foreground moves to another application's window (NVDA's 0.5 s path)."""
import os, sys, ctypes, types, logging

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim
import harness as H
from sim import SIM
import boot
import textstandins as T

import winBindings.kernel32 as k32
import winBindings.ole32 as ole32
import winKernel, NVDAHelper, watchdog, core, config, winUser, api

# -- simulated kernel objects --
OBJ = {}


class Obj:
	def __init__(self, kind, manual=True):
		self.kind, self.manual, self.state, self.due = kind, manual, False, None

	def signaled(self):
		if self.due is not None and SIM.now >= self.due:
			self.state, self.due = True, None
		return self.state


def handle(kind):
	h = 0x7000000 + len(OBJ) * 4
	OBJ[h] = Obj(kind)
	return h


def _val(h):
	return h.value if hasattr(h, "value") else h


def setWaitableTimer(h, pDue, period, routine, arg, resume):
	o = OBJ[_val(h)]
	due = pDue._obj.value
	o.state = False
	o.due = SIM.now + (-due) / 1e7 if due < 0 else 0.0  # negative is relative, in 100 ns
	return 1


def cancelWaitableTimer(h):
	OBJ[_val(h)].due = None  # the signaled state is kept, as Windows does
	return 1


def setEvent(h):
	OBJ[_val(h)].state = True
	return 1


def resetEvent(h):
	o = OBJ.get(_val(h))
	if o:
		o.state = False
	return 1


_plainWait = winKernel.waitForSingleObject


def waitForSingleObject(h, ms):
	o = OBJ.get(_val(h))
	if o is None:
		return _plainWait(h, ms)  # a process handle: alive (WAIT_TIMEOUT), as before
	if o.signaled() or ms == 0:
		return 0 if o.signaled() else 0x102
	SIM.block(o.signaled, None if ms == winKernel.INFINITE else SIM.now + ms / 1000)
	return 0 if o.signaled() else 0x102


SIM.wakeHooks.append(lambda: min((o.due for o in OBJ.values() if o.due is not None), default=None))
CANCELS = []


def coCancelCall(threadId, timeout):
	CANCELS.append(round(SIM.now, 3))
	if T.CALL["ident"] == threadId:
		T.CALL["cancelled"] = True
		return 0
	return -2147418097  # CO_E_CANCEL_DISABLED-like: nothing outstanding to cancel (0x8001000F RPC_E_CALL_COMPLETE)


k32.CreateWaitableTimer = lambda *a: handle("timer")
k32.SetWaitableTimer = setWaitableTimer
k32.CancelWaitableTimer = cancelWaitableTimer
k32.SetEvent = setEvent
k32.ResetEvent = resetEvent
k32.SetUnhandledExceptionFilter = lambda f: None
ole32.CoEnableCallCancellation = ole32.CoDisableCallCancellation = lambda r: 0
ole32.CoCancelCall = coCancelCall
winKernel.waitForSingleObject = waitForSingleObject
watchdog._coreDeadTimer = handle("timer")
CANCEL_EVENT = handle("event")

NVDAHelper._setDllFuncPointer = lambda *a: None


class _HANDLE:
	@staticmethod
	def in_dll(dll, name):
		assert name == "cancelCallEvent"
		return types.SimpleNamespace(value=CANCEL_EVENT)


class _Ctypes(types.ModuleType):
	def __getattr__(self, n):
		return getattr(ctypes, n)


watchdog.ctypes = _Ctypes("ctypes")
watchdog.ctypes.wintypes = types.SimpleNamespace(HANDLE=_HANDLE, LARGE_INTEGER=ctypes.wintypes.LARGE_INTEGER)
PATCHES = [
	"winBindings.kernel32 CreateWaitableTimer/SetWaitableTimer/CancelWaitableTimer/SetEvent/ResetEvent: simulated objects",
	"winBindings.kernel32.SetUnhandledExceptionFilter: no crash handler",
	"winBindings.ole32 CoEnableCallCancellation/CoDisableCallCancellation: no-op; CoCancelCall: cancels the stand-in call outstanding on that thread",
	"winKernel.waitForSingleObject: simulated wait on simulated objects",
	"watchdog._coreDeadTimer: replaced after import (it is created at import, before any replacement could reach kernel32)",
	"watchdog.ctypes: a proxy whose wintypes.HANDLE.in_dll gives the simulated cancel event, not the DLL global",
	"NVDAHelper._setDllFuncPointer: no-op (the native SendMessage cancel callback is not installed)",
]

watchdog.initialize()
config.conf["debugLog"]["UIA"] = True
H.logDebug(True)

# A second application, for the foreground-moves case.
OTHER = 0x5D0010
H.ww.w[OTHER] = H.S.Window(OTHER, "Notepad", "Untitled - Notepad", H.DESKTOP, 6100, 6104)


def case(name, t, hangFor, foregroundMovesAt=None, windowEvents=()):
	mark = len(boot.LOGLINES)
	spokenMark = len(H.SPOKEN)
	T.CALL["log"].clear()
	CANCELS.clear()
	H.DOC.caret = 0

	def press():
		T.HANG.update({"from": SIM.now, "until": SIM.now + hangFor})
		H.HUNG[H.TOP] = SIM.now
		H.key("downArrow")

	H.at(t, press)
	if foregroundMovesAt is not None:
		def move():
			H.ww.foreground = H.ww.focusHwnd = OTHER
		H.at(t + foregroundMovesAt, move)
	for dt in windowEvents:
		H.at(t + dt, lambda: H.raiseEvent("window", H.UIA.UIA_Text_TextSelectionChangedEventId))
	H.runUntil(t + hangFor + 2)
	H.HUNG.pop(H.TOP, None)
	H.ww.foreground = H.ww.focusHwnd = H.TOP
	print(f"## {name}: key at {t} s, application hung {hangFor} s")
	for l in T.CALL["log"]:
		print("   call:", l)
	print("   CoCancelCall at:", CANCELS[:3], f"... ({len(CANCELS)} calls)" if len(CANCELS) > 3 else "")
	for l in boot.LOGLINES[mark:]:
		first = l.splitlines()[0]
		if any(w in first for w in ("Caret ", "freeze", "Freeze", "frozen", "not responding", "Potential", "Recovered", "Listing stacks")) or first.startswith(("ERROR", "WARNING")):
			print("   log:", first[:170])
			if first.startswith("ERROR") and os.environ.get("TB"):
				print(l[-1500:])
	print("   speech:", H.SPOKEN[spokenMark:])
	print("   watchdog.isAttemptingRecovery at end:", watchdog.isAttemptingRecovery)


H.at(0.0, lambda: H.raiseFocus("edit"))
H.runUntil(0.5)
w0 = sim.REAL_PERF()
case("answers after 3 s", 1.0, 3.0, windowEvents=(2.0,))
case("hung 20 s", 10.0, 20.0, windowEvents=(3.0, 8.0))
case("hung 20 s, foreground moves to Notepad at 0.2 s", 40.0, 20.0, foregroundMovesAt=0.2)
wall = sim.REAL_PERF() - w0
print(f"=== {SIM.now:.1f} simulated seconds in {wall:.2f} s real; thread errors: {SIM.errors}")
print("=== threads:", [x.name for x in SIM.all])
print("=== patches", *PATCHES, sep="\n   ")
