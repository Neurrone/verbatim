"""P4: a hung proxied window named in a focus WinEvent, against outpost-like processes (own desktop).

TV: mockapp's comctl32 tree view (MSAA backend), stalled 8 s. B: a UIA mockapp.
O_A: group-scoped focus on A only.  O_B: group-scoped focus on B, and B's name changes.
N_B: no focus registration at all, only B's name changes.  D: a desktop-wide focus handler.
"""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag = sys.argv[1]
procs = []
try:
    TV = mockapp("protoTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    A = mockapp("protoA"); procs.append(A)
    B = mockapp("protoB"); procs.append(B)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))         or int(tool("findchild", TV.hwnd, "SysTreeView32"))
    assert tree, "no tree view window"
    log(f"tree view hwnd {tree}")
    cs = [client("O_A", "-", f"group:{A.hwnd}:pid"),
          client("O_B", "-", f"group:{B.hwnd}:pid", f"name:{B.hwnd}"),
          client("N_B", "-", f"name:{B.hwnd}"),
          client("D", "-", "desk:pid")]
    procs += cs
    time.sleep(0.5)
    log("=== baseline: B focus and name change, TV answering")
    B.send("focus btn1"); B.wait("applied"); time.sleep(0.2)
    B.send("set-name btn1 Base"); B.wait("applied"); time.sleep(0.5)
    A.send("focus item1"); A.wait("applied"); time.sleep(0.5)
    log("=== TV stalled 8 s, then a focus WinEvent naming its tree view")
    TV.send("stall 8000"); TV.wait("stall started")
    tool("winevent", tree, "8005", -4, 0)
    time.sleep(0.2)
    log("=== B raises focus and a name change; A raises focus")
    B.send("focus item2"); B.wait("applied")
    B.send("set-name btn1 During"); B.wait("applied")
    A.send("focus slider1"); A.wait("applied")
    TV.wait("stall ended", timeout=15)
    time.sleep(1.5)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"p4_{tag}.out"))
