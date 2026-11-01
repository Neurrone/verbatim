"""Q3: the focus-tracking hold for a window that answers messages but whose provider is slow.

D: desk:base (desktop focus handler) + name:B, Verbatim's client settings. D0: name:B only.
Usage: q3.py <tag> <case> [reps] [slow_ms]
cases: ctl_uia_we, slow_uia_we, slow_uia_focus, slow_msaa_we, slow_msaa_focus
  *_we: a synthetic EVENT_OBJECT_FOCUS naming the window's client (as h3 did);
  *_focus: mockapp's own `focus btn1` (UIA: UiaRaiseAutomationEvent; MSAA: NotifyWinEvent).
"""
import sys, os, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evc import *

tag, case = sys.argv[1], sys.argv[2]
reps = int(sys.argv[3]) if len(sys.argv) > 3 else 3
slow = int(sys.argv[4]) if len(sys.argv) > 4 else 3000
open_out(f"q3_{tag}.out")
procs = []
try:
    U = mockapp("q3U", fixture="small.json", backend="uia"); procs.append(U)
    M = mockapp("q3M", fixture="small.json", backend="msaa"); procs.append(M)
    B = mockapp("q3B"); procs.append(B)
    D = client("D", VERBATIM, "desk:base", f"name:{B.hwnd}"); procs.append(D)
    D0 = client("D0", VERBATIM, f"name:{B.hwnd}"); procs.append(D0)
    time.sleep(0.5)
    target = U if "_uia_" in case else M
    for r in range(reps):
        D.drain(); D0.drain()
        raised = B.send(f"set-name btn1 Base{r}"); B.wait("applied")
        t, _ = D.wait("ev D name:", pred=lambda l: f"Base{r}" in l); base = (t - raised) / 1000
        time.sleep(0.3)
        if case.startswith("slow"):
            target.send(f"slow {slow}"); target.wait("applied")
        D.drain(); D0.drain()
        trig = now_us()
        if case.endswith("_we"):
            tool("winevent", target.hwnd, "8005", -4, 0)
        else:
            target.send(f"focus {'btn1' if r % 2 == 0 else 'slider1'}"); t_ap, _ = target.wait("applied", timeout=90); log(f"focus raise took {(t_ap - trig) / 1000:.0f} ms")
        time.sleep(0.2)
        raised = B.send(f"set-name btn1 During{r}"); B.wait("applied")
        try:
            t, _ = D.wait("ev D name:", timeout=60, pred=lambda l: f"During{r}" in l)
            dd = f"{(t - raised) / 1000:.0f} ms"
        except TimeoutError:
            dd = "never (60 s)"
        try:
            t0, _ = D0.wait("ev D0 name:", timeout=5, pred=lambda l: f"During{r}" in l)
            d0 = f"{(t0 - raised) / 1000:.0f} ms"
        except TimeoutError:
            d0 = "never (5 s)"
        focus = [l for _, l in D.drain() if l.startswith("ev D desk")]
        res(f"RESULT {tag} {case} slow={slow} rep {r}: D(base {base:.1f} ms) B's name after {dd}; D0 after {d0}; trigger {(raised - trig) / 1000:.0f} ms before; focus events so far {focus}")
        if case.startswith("slow"):
            target.send("slow 0"); target.wait("applied", timeout=90)
        time.sleep(2.0)
        for t, l in D.drain():
            log(f"late D: {l}")
finally:
    for p in reversed(procs):
        p.stop()
    save(f"q3_{tag}.log")

