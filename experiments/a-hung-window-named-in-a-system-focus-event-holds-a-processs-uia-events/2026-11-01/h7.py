"""H7 (real desktop): the hold a hung, marked-hung window puts on a process holding a UIA focus registration.

Usage: h7.py <tag> <mode> <stall_ms> <at_ms> [reps]
mode synth: at_ms into TV's stall, a focus WinEvent names TV's tree view (as h3).
mode alttab: TV is foreground when it stalls; at_ms into the stall a real Alt+Tab leaves it (to the
  next window in Alt+Tab order, Notepad when the series set it up).
Either way 200 ms later B renames btn1; D (desktop focus handler + B's name changes, connection
timeout 2000 ms like Verbatim) receives it; the hold is arrival minus raise. Also reports whether
Windows had marked TV hung at the moment.
"""
import sys, os, time, ctypes
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
import keys
tag, mode, stall, at = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
reps = int(sys.argv[5]) if len(sys.argv) > 5 else 1
u = ctypes.windll.user32
procs = []
out = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), f"h7_{tag}.out"), "a")
def res(s): log(s); out.write(s + "\n"); out.flush()
try:
    TV = mockapp("h7TV", fixture="tree_view.json", backend="msaa", show=True); procs.append(TV)
    B = mockapp("h7B"); procs.append(B)
    tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1")); assert tree, "tree view not found"
    D = client("D", os.environ.get("H7_FLAGS", "to=2000"), "desk:base", f"name:{B.hwnd}"); procs.append(D)
    time.sleep(0.5)
    for r in range(reps):
        if mode == "synth_bg":
            keys.front("h7TV"); time.sleep(0.4); keys.front("notes.txt", "Notepad"); time.sleep(1.0)
        elif mode == "alttab_to":
            keys.front("h7TV"); time.sleep(0.4); keys.front("notes.txt", "Notepad"); time.sleep(1.0)
        else:
            keys.front("notes.txt", "Notepad"); time.sleep(0.4); keys.front("h7TV"); time.sleep(1.0)
        D.drain()
        raised = B.send(f"set-name btn1 Base{r}"); B.wait("applied")
        t, _ = D.wait("ev D name"); base = (t - raised) / 1000
        TV.drain()
        t_stall = TV.send(f"stall {stall} 100 focus drafts" if mode == "selfbusy" else f"stall {stall}")
        TV.wait("stall started")
        time.sleep(at / 1000)
        hung = u.IsHungAppWindow(TV.hwnd)
        if mode == "selfbusy":
            pass
        elif mode in ("synth", "synth_bg"):
            tool("winevent", tree, "8005", -4, 0)
        else:
            keys.press("alt", "tab"); time.sleep(0.15); keys.press("down", label="after") if False else None
            log(f"after alt+tab fg={keys.fgdesc()}")
        time.sleep(0.2)
        D.drain(); raised = B.send(f"set-name btn1 During{r}"); B.wait("applied")
        try:
            t, _ = D.wait("ev D name", timeout=stall / 1000 + 25)
            res(f"RESULT {tag} rep {r}: hung_at_trigger={hung} baseline {base:.1f} ms; held name after {(t - raised) / 1000:.0f} ms; {(t - t_stall) / 1000:.0f} ms into the {stall} ms stall; trigger at {at} ms")
        except TimeoutError:
            res(f"RESULT {tag} rep {r}: hung_at_trigger={hung} name never arrived")
        TV.wait("stall ended", timeout=stall / 1000 + 10)
        time.sleep(1.5)
finally:
    for p in reversed(procs): p.stop()
