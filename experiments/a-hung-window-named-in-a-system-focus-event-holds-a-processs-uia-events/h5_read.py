"""H5: is the connection timeout one value per process? Client D's main client object sets the
flags' timeouts; its specs may make other client objects with their own. With no WinEvent raised,
D reads the stalled tree view (ElementFromHandle + CurrentName) on its main client object.
Usage: h5_read.py <tag> <flags> <spec>..."""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag, flags = sys.argv[1], sys.argv[2]
procs = []
try:
    TV = mockapp("holdTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))
    D = client("D", flags, *sys.argv[3:]); procs.append(D)
    D.send(f"read {tree}"); _, l = D.wait("readdone"); log(f"RESULT {tag} answering: {l}")
    TV.send("stall 15000"); TV.wait("stall started")
    D.send(f"read {tree}"); _, l = D.wait("readdone", timeout=30); log(f"RESULT {tag} stalled: {l}")
    TV.wait("stall ended", timeout=30)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"h5_{tag}.out"))
