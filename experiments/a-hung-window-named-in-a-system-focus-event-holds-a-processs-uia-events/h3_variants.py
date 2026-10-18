"""H3: how long a hung proxied window holds a client process's UIA events, by client-side variant.

Usage: h3_variants.py <tag> <stall_ms> <winevent_at_ms> <flags> <spec>...
TV (comctl32 tree view, MSAA backend) stalls for stall_ms; winevent_at_ms into the stall a focus
WinEvent names its tree view; 200 ms later B (UIA) renames btn1. D is one client process with the
given flags and registrations ({B} is B's window). The hold is the name change's arrival in D minus
its raise. Writes h3_<tag>.out.
"""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag, stall, at, flags = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
procs = []
try:
    TV = mockapp("holdTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    B = mockapp("holdB"); procs.append(B)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")) \
        or int(tool("findchild", TV.hwnd, "SysTreeView32"))
    specs = [s.replace("{B}", str(B.hwnd)) for s in sys.argv[5:]]
    D = client("D", flags, *specs); procs.append(D)
    time.sleep(0.3)
    raised = B.send("set-name btn1 Base"); B.wait("applied")
    t, _ = D.wait("ev D name"); log(f"RESULT {tag} baseline name after {(t - raised) / 1000:.1f} ms")
    time.sleep(0.3)
    t_stall = TV.send(f"stall {stall}"); TV.wait("stall started")
    if at: time.sleep(at / 1000)
    for _ in range(int(os.environ.get("H3_REPEAT", "1"))):  # H3_REPEAT focus WinEvents in a row
        tool("winevent", tree, "8005", -4, 0)
    time.sleep(0.2)
    raised = B.send("set-name btn1 During"); B.wait("applied")
    try:
        t, _ = D.wait("ev D name", timeout=stall / 1000 + 25)
        log(f"RESULT {tag} held name after {(t - raised) / 1000:.0f} ms; "
            f"{(t - t_stall) / 1000:.0f} ms into the {stall} ms stall; WinEvent at {at} ms")
    except TimeoutError:
        log(f"RESULT {tag} name never arrived")
    TV.wait("stall ended", timeout=stall / 1000 + 10)
    time.sleep(0.5)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"h3_{tag}.out"))
