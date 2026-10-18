"""P2c: proxied focus delivered to desktop-wide focus handlers: N processes, or N handlers in one."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
gap = float(sys.argv[1]); n = int(sys.argv[2]); layout = sys.argv[3]; tag = sys.argv[4]
procs = []
try:
    M = mockapp("protoM", backend="msaa"); procs.append(M)
    if layout == "procs":
        cs = [client(f"C{i}", "-", "desk:pid") for i in range(n)]
    else:
        cs = [client("C0", "-", *["desk:pid"] * n)]
    procs += cs
    time.sleep(0.5)
    sends = []
    for k in range(10):
        M.send(f"focus {['btn1', 'item2'][k % 2]}"); t, _ = M.wait("applied"); sends.append(t); time.sleep(gap)
    time.sleep(3)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"p2c_{tag}.out"))
