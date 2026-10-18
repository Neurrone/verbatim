"""H2: an invasive trace of D during the hold: each SendMessageTimeoutW (message, flags, timeout,
result), and the thread and stack that deliver the held name event. Setup as H1.
Usage: h2_trace.py <tag> [flags] [stall_ms] [spec...]; specs as h3 ({B} is B's window)."""
import time, sys, os, ctypes
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag = sys.argv[1]
flags = sys.argv[2] if len(sys.argv) > 2 else "-"
stall = int(sys.argv[3]) if len(sys.argv) > 3 else 8000
OUT = os.path.join(HERE, f"h2_{tag}")
os.makedirs(OUT, exist_ok=True)
procs = []
dbg = None
try:
    TV = mockapp("holdTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    B = mockapp("holdB"); procs.append(B)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")) \
        or int(tool("findchild", TV.hwnd, "SysTreeView32"))
    log(f"tree view hwnd {tree:#x}, TV pid {TV.pid}, B hwnd {B.hwnd:#x}")
    specs = [x.replace("{B}", str(B.hwnd)) for x in sys.argv[4:]] or ["desk:pid", f"name:{B.hwnd}"]
    D = client("D", flags, *specs); procs.append(D)
    dbg = Proc("cdb", [CDB, "-pd", "-p", str(D.pid), "-y", SYMS, "-cf", os.path.join(HERE, "h2_trace.cdb")])
    dbg.wait("ATTACHED", timeout=600)
    time.sleep(1.0)
    B.send("set-name btn1 Base"); B.wait("applied"); D.wait("ev D name"); time.sleep(0.5)
    log("=== TV stalled, then a focus WinEvent naming its tree view")
    TV.send(f"stall {stall}"); TV.wait("stall started")
    tool("winevent", tree, "8005", -4, 0)
    time.sleep(0.2)
    B.send("set-name btn1 During"); B.wait("applied")
    D.wait("ev D name", timeout=stall / 1000 + 10)
    TV.wait("stall ended", timeout=stall / 1000 + 10)
    time.sleep(1.0)
finally:
    for p in procs: p.stop()
    if dbg: dbg.p.kill()
    save(os.path.join(OUT, "run.out"))
