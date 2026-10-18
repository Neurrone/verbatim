"""P3: how long an event's sender stays usable when a process keeps it (own desktop)."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
procs = []
try:
    A = mockapp("protoA"); procs.append(A)
    # S stands for a spare: it keeps every sender it receives for an application it does not serve.
    S = client("S", "hold", f"group:{A.hwnd}:base"); procs.append(S)
    time.sleep(0.3)
    for n in ["btn1", "item1"]:
        A.send(f"focus {n}"); A.wait("applied"); time.sleep(0.3)
    for age in [0, 30, 120]:
        time.sleep(max(0, age - 0))
        log(f"=== read at about {age} s")
        S.send("held"); S.wait("held"); time.sleep(0.5)
    log("=== focus moved on (slider1), then read")
    A.send("focus slider1"); A.wait("applied"); time.sleep(0.3)
    S.send("held"); S.wait("held"); time.sleep(0.5)
    log("=== btn1's runtime id given to item2 (take-runtime-id), then read")
    A.send("take-runtime-id item2 btn1"); A.wait("applied"); time.sleep(0.3)
    S.send("held"); S.wait("held"); time.sleep(0.5)
    log("=== A exits, then read")
    A.stop(); procs.remove(A); time.sleep(0.5)
    S.send("held"); S.wait("held"); time.sleep(1.0)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, "p3_held.out"))
