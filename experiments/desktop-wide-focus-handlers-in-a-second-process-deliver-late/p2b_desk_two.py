"""P2b: two and four desktop-wide focus handlers in separate processes, proxied focus, slow pace."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
gap = float(sys.argv[1]); n = int(sys.argv[2]); tag = sys.argv[3]
procs = []
try:
    M = mockapp("protoM", backend="msaa"); procs.append(M)
    cs = [client(f"C{i}", "-", "desk:pid") for i in range(n)]
    procs += cs
    time.sleep(0.5)
    for k in range(8):
        M.send(f"focus {['btn1', 'item2'][k % 2]}"); M.wait("applied"); time.sleep(gap)
    time.sleep(2.5)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"p2b_{tag}.out"))
