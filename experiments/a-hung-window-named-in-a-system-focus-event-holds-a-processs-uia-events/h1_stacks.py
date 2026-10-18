"""H1: every thread's stack in the receiving process while a hung proxied window holds its UIA events.

TV: mockapp's comctl32 tree view (MSAA backend), stalled 8 s. B: a UIA mockapp.
D: one client process with a desktop focus handler and B's name changes.
During the stall, a focus WinEvent names the tree view; 200 ms later B renames btn1.
D's stacks are dumped non-invasively before the stall and several times during it.
"""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag = sys.argv[1]
flags = sys.argv[2] if len(sys.argv) > 2 else "-"
OUT = os.path.join(HERE, f"h1_{tag}")
os.makedirs(OUT, exist_ok=True)
procs = []
try:
    TV = mockapp("holdTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    B = mockapp("holdB"); procs.append(B)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")) \
        or int(tool("findchild", TV.hwnd, "SysTreeView32"))
    assert tree, "no tree view window"
    log(f"tree view hwnd {tree}, TV pid {TV.pid}")
    D = client("D", flags, "desk:pid", f"name:{B.hwnd}"); procs.append(D)
    stacks(D.pid, "0_idle", OUT)
    B.send("set-name btn1 Base"); B.wait("applied"); D.wait("ev D name"); time.sleep(0.3)
    log("=== TV stalled 8 s, then a focus WinEvent naming its tree view")
    t_stall = TV.send("stall 8000"); TV.wait("stall started")
    tool("winevent", tree, "8005", -4, 0)
    time.sleep(0.2)
    B.send("set-name btn1 During"); B.wait("applied")
    for label, at in [("1_held_0.6s", 0.6), ("2_held_2.0s", 2.0), ("3_held_3.3s", 3.3), ("4_after_5.0s", 5.0)]:
        delay = t_stall / 1e6 + at - now_us() / 1e6
        if delay > 0: time.sleep(delay)
        stacks(D.pid, label, OUT)
    TV.wait("stall ended", timeout=15)
    time.sleep(1.0)
finally:
    for p in procs: p.stop()
    save(os.path.join(OUT, "run.out"))
