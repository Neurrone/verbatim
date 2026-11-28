"""Thread order: NVDA's LiveText monitor thread (NVDAObjects.behaviors.LiveText._monitor, unchanged)
against an application writing to a terminal, under fifo order and under seeds.

The application writes "4" then "2" after a prompt at the same simulated instant, each write
followed by a text-change notification (LiveText.event_textChange, which NVDA documents as safe
from any thread). Whether the monitor reads between the writes is a thread-order choice; NVDA
drops a lone new character as a typed one, so the order decides what is spoken."""
import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim
import harness as H
from sim import SIM
import diffHandler
from NVDAObjects.behaviors import LiveText
from NVDAObjects import NVDAObject


class Screen:
	text = ""


class Live(LiveText, NVDAObject):
	"""A terminal's live text as the application leaves it: _getText reads the world's screen."""

	processID = H.PID
	windowHandle = H.TOP

	def _get_diffAlgo(self):
		return diffHandler.prefer_difflib()

	def _getText(self):
		H.S.called("terminal.getText")
		return Screen.text


def trial(t, stabilize):
	Screen.text = "C:\\> "
	obj = Live()
	obj.initOverlayClass()  # LiveText is a base here, not an overlay, so NVDA would not call it
	obj.STABILIZE_DELAY = stabilize
	obj.startMonitoring()
	H.runUntil(t - 0.5)

	def write(s):
		def act():
			Screen.text += s
			obj.event_textChange()
		return act

	H.at(t, write("4"))
	H.at(t, write("2"))
	mark = len(H.SPOKEN)
	H.runUntil(t + 0.5)
	obj.stopMonitoring()
	H.runUntil(t + 0.6)
	return [s for _, s in H.SPOKEN[mark:]]


H.S.PREEMPT_AT_CALLS = bool(os.environ.get("PREEMPT"))
results = {}
t = 1.0
for stabilize in (0, 0.03):
	for seed in [None] + list(range(1, 13)):
		if seed is None:
			SIM.policy = "fifo"
		else:
			SIM.seed(seed)
		c0 = SIM.choices
		spoken = trial(t, stabilize)
		t += 1.0
		key = (stabilize, "fifo" if seed is None else f"seed {seed}")
		results[key] = (spoken, SIM.choices - c0)

for (stab, who), (spoken, ch) in results.items():
	print(f"STABILIZE_DELAY={stab:<5} {who:8} seed-decided picks={ch:2d} speech={spoken}")
print("thread errors:", SIM.errors)
print("threads:", [x.name for x in SIM.all])
