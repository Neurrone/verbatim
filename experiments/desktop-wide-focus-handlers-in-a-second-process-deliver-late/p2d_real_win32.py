"""P2d: focus WinEvents named on a real Win32 window (charmap) reach N desktop-wide focus handlers."""
import time, sys, os, subprocess
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
n = int(sys.argv[1]); layout = sys.argv[2]; tag = sys.argv[3]
procs = []
try:
    cm = Proc("charmap", ondesk([r"C:\Windows\System32\charmap.exe"])); procs.append(cm)
    _, pl = cm.wait("ondesk-child-pid"); cm.pid = int(pl.split()[1])
    h = 0
    for _ in range(100):
        h = int(tool("find", "Character Map"))
        if h: break
        time.sleep(0.1)
    if layout == "procs":
        cs = [client(f"C{i}", "-", "desk:pid") for i in range(n)]
    else:
        cs = [client("C0", "-", *["desk:pid"] * n)]
    procs += cs
    time.sleep(0.5)
    for k in range(10):
        tool("winevent", h, "8005", -4, 0)   # EVENT_OBJECT_FOCUS, OBJID_CLIENT, CHILDID_SELF
        time.sleep(0.15)
    time.sleep(3)
finally:
    for c in procs[1:]: c.stop()
    subprocess.run(["taskkill", "/PID", str(cm.pid), "/F"], capture_output=True)
    cm.p.wait(timeout=5)
    save(os.path.join(HERE, f"p2d_{tag}.out"))
