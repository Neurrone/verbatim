"""NVDA's native UIA event limiter (nvdaHelper/local/UIAEventLimiter, in the installed
nvdaHelperLocal.dll) driven from the harness with stand-in elements, then the Python model of it
(ratelimit.py) compared with it on the same input.

Part one gates the DLL's flusher thread inside the first event it emits, sends a burst, opens the
gate, and records what reaches the inner handler. Part two sends the same burst with no gate,
20 times, to show the batch boundary is the native thread's timing."""
import os, sys, ctypes, threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim
import harness as H
import NVDAHelper
from comtypes import COMObject, IUnknown, POINTER, byref
from comInterfaces import UIAutomationClient as UIA
import ratelimit

dll = NVDAHelper.localLib.dll
REAL_SLEEP = sim.REAL_SLEEP
received = []
gate = {"armed": False, "open": False, "entered": False}


class Inner(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationEventHandler, UIA.IUIAutomationFocusChangedEventHandler,
		UIA.IUIAutomationPropertyChangedEventHandler]

	def IUIAutomationEventHandler_HandleAutomationEvent(self, sender, eventID):
		node = H.S.lookup(sender).node.key
		received.append((node, eventID, threading.get_ident() == MAIN))
		if gate["armed"] and not gate["entered"]:
			gate["entered"] = True
			while not gate["open"]:
				REAL_SLEEP(0.001)  # the native flusher thread is held here (real time: it is not ours)

	def IUIAutomationFocusChangedEventHandler_HandleFocusChangedEvent(self, sender):
		received.append((H.S.lookup(sender).node.key, "focus", threading.get_ident() == MAIN))

	def IUIAutomationPropertyChangedEventHandler_HandlePropertyChangedEvent(self, sender, propertyId, newValue):
		received.append((H.S.lookup(sender).node.key, f"prop {propertyId}", threading.get_ident() == MAIN))


MAIN = threading.get_ident()
inner = H.S.register(Inner())
limiter = POINTER(IUnknown)()
hr = dll.rateLimitedUIAEventHandler_create(inner.QueryInterface(IUnknown), byref(limiter))
assert hr == 0, hr
auto = limiter.QueryInterface(UIA.IUIAutomationEventHandler)
SEL, NAME = UIA.UIA_Text_TextSelectionChangedEventId, UIA.UIA_Text_TextChangedEventId
BURST = [("edit", SEL), ("window", SEL), ("edit", SEL), ("edit", NAME), ("window", SEL)]


def send(node, ev):
	auto.HandleAutomationEvent(H.element(node), ev)


def waitFor(n, limit=2.0):
	t = sim.REAL_PERF()
	while len(received) < n and sim.REAL_PERF() - t < limit:
		REAL_SLEEP(0.001)


# Part one: gated.
gate.update(armed=True)
send("window", NAME)
while not gate["entered"]:
	REAL_SLEEP(0.001)
for n, e in BURST:
	send(n, e)
gate["open"] = True
waitFor(1 + 3)
REAL_SLEEP(0.05)
print("gated, DLL delivered:", [(n, e) for n, e, _ in received])
print("   delivered on the caller's thread:", any(m for _, _, m in received))
model = ratelimit.Limiter()
model.queue(("window", NAME))
first = model.flushBatch()
for x in BURST:
	model.queue(x)
print("gated, model delivers:", first + model.flushBatch())

# Part two: ungated, 20 bursts.
gate.update(armed=False)
counts = {}
for i in range(20):
	received.clear()
	for n, e in BURST:
		send(n, e)
	waitFor(len(BURST), 0.2)
	REAL_SLEEP(0.02)
	k = tuple((n, e) for n, e, _ in received)
	counts[k] = counts.get(k, 0) + 1
print("ungated, 20 bursts, distinct deliveries:")
for k, c in counts.items():
	print(f"   {c:2d}x {list(k)}")
if __name__ == "__main__":
	dll.rateLimitedUIAEventHandler_terminate(limiter)
print("terminated")
