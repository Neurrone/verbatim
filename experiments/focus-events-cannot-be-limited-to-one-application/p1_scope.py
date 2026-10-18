"""P1: which registrations receive which focus events (own desktop, no keyboard focus)."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *

procs = []
try:
    A = mockapp("protoA"); procs.append(A)
    B = mockapp("protoB"); procs.append(B)
    M = mockapp("protoM", backend="msaa"); procs.append(M)
    clients = [
        client("D", "-", "desk:min"),
        client("RG", "-", "rootgroup:min"),
        client("GA", "-", f"group:{A.hwnd}:min"),
        client("GB", "-", f"group:{B.hwnd}:min"),
        client("GM", "-", f"group:{M.hwnd}:min"),
        client("X", "-", f"direct:{A.hwnd}:min"),
    ]
    procs += clients
    time.sleep(0.5)
    log("=== A raises three focus events")
    for n in ["btn1", "item1", "slider1"]:
        A.send(f"focus {n}"); A.wait("applied"); time.sleep(0.4)
    log("=== B raises two")
    for n in ["btn1", "item2"]:
        B.send(f"focus {n}"); B.wait("applied"); time.sleep(0.4)
    log("=== M (MSAA backend, UIA's proxy) raises two")
    for n in ["btn1", "item2"]:
        M.send(f"focus {n}"); M.wait("applied"); time.sleep(0.6)
    time.sleep(0.5)
    log("=== done")
finally:
    for p in procs:
        p.stop()
    save(os.path.join(os.path.dirname(os.path.abspath(__file__)), "p1_scope.out"))
