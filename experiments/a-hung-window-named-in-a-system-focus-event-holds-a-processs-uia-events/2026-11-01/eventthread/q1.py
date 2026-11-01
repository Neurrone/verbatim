"""Q1: a non-focus WinEvent naming a proxied (MSAA) window that is stalled or slow, in a process
holding a desktop-wide name-change registration with a cache request. Does B's event wait?

D: deskprop:base (root subtree, Name, NVDA's base cache) + name:B, Verbatim's client settings.
D0: name:B only (control process, no registration covering the proxied window).
Usage: q1.py <tag> <case> [reps]
cases: ctl_tree, stall_tree, stall_win, slow_msaa, slow_msaa_win  (see CASES below)
"""
import sys, os, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evc import *

tag, case = sys.argv[1], sys.argv[2]
reps = int(sys.argv[3]) if len(sys.argv) > 3 else 3
dspec = sys.argv[4] if len(sys.argv) > 4 else "deskprop:base"
open_out(f"q1_{tag}.out")
procs = []
try:
    TV = mockapp("q1TV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    M = mockapp("q1M", fixture="small.json", backend="msaa"); procs.append(M)
    B = mockapp("q1B"); procs.append(B)
    tree = (int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")) or int(tool("findchild", TV.hwnd, "SysTreeView32")))
    assert tree, "tree view not found"
    D = client("D", VERBATIM, *[s.replace("{TV}", str(TV.hwnd)).replace("{M}", str(M.hwnd)) for s in dspec.split("+")], f"name:{B.hwnd}"); procs.append(D)
    D0 = client("D0", VERBATIM, f"name:{B.hwnd}"); procs.append(D0)
    time.sleep(0.5)
    for r in range(reps):
        # Baseline for this rep.
        D.drain(); D0.drain()
        raised = B.send(f"set-name btn1 Base{r}"); B.wait("applied")
        t, _ = D.wait(f"ev D name:", pred=lambda l: f"Base{r}" in l); base = (t - raised) / 1000
        time.sleep(0.3)
        D.drain(); D0.drain()
        trig = now_us()
        if case == "ctl_tree":
            tool("winevent", tree, "800C", -4, 0)
            stall = 0
        elif case == "stall_tree":
            stall = 8000
            TV.send(f"stall {stall}"); TV.wait("stall started")
            trig = now_us(); tool("winevent", tree, "800C", -4, 0)
        elif case == "stall_win":
            stall = 8000
            M.send(f"stall {stall}"); M.wait("stall started")
            trig = now_us(); tool("winevent", M.hwnd, "800C", -4, 0)
        elif case in ("slow_msaa", "slow_msaa_win"):
            stall = 0
            M.send("slow 3000"); M.wait("applied")
            trig = now_us()
            if case == "slow_msaa":
                M.send(f"client-name Renamed{r}"); M.wait("applied")
            else:
                tool("winevent", M.hwnd, "800C", -4, 0)
        else:
            raise SystemExit(f"unknown case {case}")
        time.sleep(0.2)
        raised = B.send(f"set-name btn1 During{r}"); B.wait("applied")
        try:
            t, _ = D.wait("ev D name:", timeout=40, pred=lambda l: f"During{r}" in l)
            dd = f"{(t - raised) / 1000:.0f} ms"
        except TimeoutError:
            dd = "never (40 s)"
        try:
            t0, _ = D0.wait("ev D0 name:", timeout=5, pred=lambda l: f"During{r}" in l)
            d0 = f"{(t0 - raised) / 1000:.0f} ms"
        except TimeoutError:
            d0 = "never (5 s)"
        res(f"RESULT {tag} {case} rep {r}: D(base {base:.1f} ms) B's name after {dd}; D0 after {d0}; trigger {(raised - trig) / 1000:.0f} ms before B's raise")
        if stall:
            (TV if case == "stall_tree" else M).wait("stall ended", timeout=stall / 1000 + 10)
        if case.startswith("slow_msaa"):
            M.send("slow 0"); M.wait("applied")
        time.sleep(1.5)
        # Whatever D received meanwhile, for the record.
        for t, l in D.drain():
            log(f"late D: {l}")
finally:
    for p in reversed(procs):
        p.stop()
    save(f"q1_{tag}.log")


