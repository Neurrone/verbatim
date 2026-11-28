"""The caret watch (editableText.EditableText._hasCaretMoved) on simulated time.

Down arrow goes through NVDA's script path; the application moves the caret 40 ms (simulated)
after the key is sent, and raises TextSelectionChanged. Then a second down arrow the application
ignores: NVDA's watch must time out after caretMoveTimeoutMs, as live."""
import os, sys, time

t0 = time.perf_counter()
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim
REAL = sim.REAL_PERF
if os.environ.get("SEED"):
	sim.SIM.seed(int(os.environ["SEED"]))
import harness as H
H.S.PREEMPT_AT_CALLS = bool(os.environ.get("PREEMPT"))
from sim import SIM
import boot

tBoot = REAL() - t0
DELAY = float(os.environ.get("CARET_DELAY", "0.040"))


def react(gesture):
	if REACT[0]:
		off = REACT.pop(0)
		H.at(SIM.now + DELAY, lambda: H.moveCaret(off))
	else:
		REACT.pop(0)


REACT = []
H.ON_SEND.append(react)
H.at(0.0, lambda: H.raiseFocus("edit"))
H.runUntil(0.5)
focusSpeech = list(H.SPOKEN)

H.logDebug(True)
mark = len(boot.LOGLINES)
w0 = REAL()
REACT.append(11)  # the application moves the caret to "second line"
H.at(1.0, lambda: H.key("downArrow"))
H.runUntil(2.0)
REACT.append(None)  # the application does not move the caret
H.at(3.0, lambda: H.key("downArrow"))
H.runUntil(4.0)
wall = REAL() - w0
H.logDebug(False)

print("=== speech (simulated seconds, text)")
for t, s in H.SPOKEN:
	print(f"  {t:8.4f}  {s!r}")
print("=== keys sent to the application:", H.SENT)
print("=== NVDA's caret-watch log")
for l in boot.LOGLINES[mark:]:
	if "aret" in l or "lapsed" in l:
		print("  ", l.splitlines()[0][:200])
print("=== misses")
for m in H.S.MISSES:
	print("  ", m)
print(f"=== wall clock: boot {tBoot:.2f} s; 3.5 simulated seconds of run in {wall*1000:.1f} ms real")
print("=== picks with several runnable threads:", SIM.contended, "decided by the seed:", SIM.choices)
import hashlib
print("=== trace hash:", hashlib.md5(repr(SIM.trace).encode()).hexdigest())
print("=== scheduler switches:", len(SIM.trace), "threads:", [t.name for t in SIM.all])
print("=== thread errors", *SIM.errors, sep="\n")
print("=== warnings")
for l in boot.LOGLINES:
	if l.startswith(("WARNING", "ERROR", "CRITICAL")):
		print("  ", l[:1500])
