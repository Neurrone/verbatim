"""P5: UiaAddEvent (the core API) for focus on one window's node and on the root (own desktop)."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
procs = []
try:
    A = mockapp("protoA"); procs.append(A)
    M = mockapp("protoM", backend="msaa"); procs.append(M)
    for tag, target in [("CA", A.hwnd), ("CM", M.hwnd), ("CR", "root")]:
        c = Proc(tag, ondesk([TOOL, "coreevent", tag, str(target)])); procs.append(c); c.wait("client-ready")
    time.sleep(0.5)
    log("=== A raises two UIA focus events")
    for n in ["btn1", "item1"]:
        A.send(f"focus {n}"); A.wait("applied"); time.sleep(0.4)
    log("=== M raises two MSAA focus events")
    for n in ["btn1", "item2"]:
        M.send(f"focus {n}"); M.wait("applied"); time.sleep(0.6)
    time.sleep(0.5)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"p5_core{sys.argv[1]}.out"))
