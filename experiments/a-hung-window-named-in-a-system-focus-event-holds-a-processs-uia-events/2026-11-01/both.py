"""One hold, two probes: B's notification (another application) and Notepad's menu focus after Alt+F."""
import sys, os, time, re
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import states as st
from proto import mockapp, tool, now_us
import keys
B = mockapp("stB", show=True); time.sleep(1.0)
keys.front("notes.txt", "Notepad"); time.sleep(1.5)
A = mockapp("stA2", fixture="tree_view.json", backend="msaa")
tree = st.tree_of(A)
def listener_events_after(t_us):
    out = []
    with open(st.LOG, encoding="utf-8", errors="replace") as f:
        for line in f:
            if "UIA event received" in line:
                t = st.log_us(line)
                if t > t_us:
                    m = re.search(r'event="([a-z ]+)".*runtime_id=(\[[^\]]*\])', st.ANSI.sub("", line))
                    out.append((t, m.group(1), m.group(2)))
    return out
for r in range(3):
    keys.front("notes.txt", "Notepad"); time.sleep(0.8)
    A.drain(); A.send("stall 9000"); A.wait("stall started"); time.sleep(0.1)
    t0 = now_us(); tool("winevent", tree, "8005", -4, 0); time.sleep(0.2)
    tn = B.send("notify during"); B.wait("applied")
    time.sleep(0.6); tk = now_us(); keys.press("alt", "f")
    time.sleep(0.8); keys.press("esc"); time.sleep(0.25); keys.press("esc")
    A.wait("stall ended", timeout=15); time.sleep(1.0)
    evs = listener_events_after(t0)
    notif = next(((t - tn) / 1000 for t, k, _ in evs if k == "notification"), None)
    menu = next(((t - tk) / 1000 for t, k, _ in evs if k == "focus" and t > tk), None)
    print(f"RESULT run {r}: B's notification after {notif} ms; Notepad menu focus after Alt+F {menu} ms", flush=True)
A.stop(); B.stop()
