"""Q1/Q3 thread naming: thread dumps of D (non-invasive cdb) during a hold.
Usage: q1stacks.py <case> [dspec]
cases: stall_tree (TV stalled, NAMECHANGE on its tree view), slow_msaa (M slow 3000, client-name),
       q3_slow_msaa_we (M slow 3000, FOCUS WinEvent on M; D holds desk:base), q3_slow_uia_we (U slow 3000)
"""
import sys, os, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evc import *

case = sys.argv[1]
dspec = sys.argv[2] if len(sys.argv) > 2 else ("desk:base" if case.startswith("q3") else "deskprop:base")
procs = []
try:
    TV = mockapp("qsTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    M = mockapp("qsM", fixture="small.json", backend="msaa"); procs.append(M)
    U = mockapp("qsU", fixture="small.json", backend="uia"); procs.append(U)
    B = mockapp("qsB"); procs.append(B)
    tree = (int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")) or int(tool("findchild", TV.hwnd, "SysTreeView32")))
    D = client("D", VERBATIM, *[s.replace("{TV}", str(TV.hwnd)).replace("{M}", str(M.hwnd)).replace("{U}", str(U.hwnd)) for s in dspec.split("+")], f"name:{B.hwnd}"); procs.append(D)
    time.sleep(0.5)
    raised = B.send("set-name btn1 Base"); B.wait("applied"); D.wait("ev D name:")
    log(f"D pid {D.pid}; cpu {cpu(D)}")
    if case == "stall_tree":
        TV.send("stall 8000"); TV.wait("stall started"); tool("winevent", tree, "800C", -4, 0)
    elif case == "slow_msaa":
        M.send("slow 3000"); M.wait("applied"); M.send("client-name Renamed"); M.wait("applied")
    elif case == "q3_slow_msaa_we":
        M.send("slow 3000"); M.wait("applied"); tool("winevent", M.hwnd, "8005", -4, 0)
    elif case == "q3_slow_uia_we":
        U.send("slow 3000"); U.wait("applied"); tool("winevent", U.hwnd, "8005", -4, 0)
    trig = now_us()
    time.sleep(0.2)
    raised = B.send("set-name btn1 During"); B.wait("applied")
    time.sleep(0.8)
    stacks(D.pid, f"{case}_1s")
    stacks((U if "uia" in case else (TV if "tree" in case else M)).pid, f"{case}_target_1s")
    time.sleep(1.5)
    stacks(D.pid, f"{case}_3s")
    t, _ = D.wait("ev D name:", timeout=60, pred=lambda l: "During" in l)
    log(f"RESULT {case}: B's name after {(t - raised) / 1000:.0f} ms (stack dumps included, each pauses D)")
    M.send("slow 0"); U.send("slow 0")
    time.sleep(1)
finally:
    for p in reversed(procs):
        p.stop()
    save(f"q1stacks_{case}.log")



