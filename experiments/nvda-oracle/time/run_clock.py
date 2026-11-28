"""Clock replacements: each clock NVDA reads, core.callLater, threading.Timer and utils.debounce
fire at their simulated times; an hour of simulated time passes in milliseconds."""
import os, sys, time, threading
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sim
import harness as H
from sim import SIM
import core, queueHandler
from utils import blockUntilConditionMet as B
fired = []
note = lambda what: fired.append((round(SIM.now, 4), what))
w0 = sim.REAL_PERF()
core.callLater(250, note, "core.callLater 250 ms (runs via the event queue and the core pump)")
threading.Timer(0.3, note, ("threading.Timer 0.3 s",)).start()
core.callLater(3_600_000, note, "core.callLater 1 h")
def poller():
	start = SIM.now
	ok, _ = B.blockUntilConditionMet(lambda: SIM.now - start > 0.45, giveUpAfterSeconds=1.0)
	note(f"utils.blockUntilConditionMet met={ok}")
threading.Thread(target=poller, name="poller").start()
H.runUntil(3601)
print(*fired, sep="\n")
print("time.time", time.time(), "perf_counter", time.perf_counter(), "monotonic", time.monotonic())
print(f"wall: {(sim.REAL_PERF()-w0)*1000:.1f} ms for {SIM.now:.0f} simulated s; errors {SIM.errors}")
