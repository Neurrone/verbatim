"""H4: does Windows consider mockapp's stalled tree view hung (IsHungAppWindow), second by second?"""
import time, sys, os, ctypes
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
u = ctypes.windll.user32
u.IsHungAppWindow.argtypes = [ctypes.c_void_p]
procs = []
try:
    TV = mockapp("holdTV", fixture="tree_view.json", backend="msaa"); procs.append(TV)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))
    t0 = TV.send("stall 9000"); TV.wait("stall started")
    if len(sys.argv) > 1 and sys.argv[1] == "post":
        u.PostMessageW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
        log(f"RESULT posted WM_NULL to the tree view: {u.PostMessageW(tree, 0, 0, 0)}")
    for i in range(10):
        log(f"RESULT {(now_us() - t0) / 1e6:.1f}s IsHungAppWindow(top)={u.IsHungAppWindow(TV.hwnd)} (tree)={u.IsHungAppWindow(tree)}")
        time.sleep(1)
    TV.wait("stall ended", timeout=5)
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"h4_ishung_{sys.argv[1] if len(sys.argv) > 1 else 'plain'}.out"))
