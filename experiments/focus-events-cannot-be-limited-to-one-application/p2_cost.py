"""P2: what one focus event costs with N registered client processes (own desktop)."""
import time, sys, os, statistics, re
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *

HERE = os.path.dirname(os.path.abspath(__file__))
NAMES = {0: "WM_GETOBJECT", 1: "ProviderOptions", 2: "GetPatternProvider", 3: "GetPropertyValue",
         4: "HostRawElementProvider", 5: "Navigate", 6: "GetRuntimeId", 7: "BoundingRectangle",
         10: "FragmentRoot", 12: "GetFocus", 33: "accParent", 34: "accChildCount", 35: "accChild",
         36: "accName", 37: "accValue", 38: "accDescription", 39: "accRole", 40: "accState",
         41: "accHelp", 43: "accKeyboardShortcut", 44: "accFocus", 45: "accSelection",
         46: "accDefaultAction", 48: "accLocation", 49: "accNavigate", 50: "accHitTest"}
K = 10
RESULTS = []


def cpu_of(pid):
    s = tool("cpu", pid)
    m = re.search(r"user=([\d.]+) kernel=([\d.]+)", s)
    return float(m.group(1)) + float(m.group(2))


def run(app, kind, cache, n, nodes):
    clients = []
    for i in range(n):
        spec = f"desk:{cache}" if kind == "desk" else f"group:{app.hwnd}:{cache}"
        clients.append(client(f"C{i}", "quiet", spec))
    time.sleep(0.4)
    tool("hits", app.hwnd, 1)
    app_cpu0 = cpu_of(app.pid)
    cl_cpu0 = [cpu_of(c.pid) for c in clients]
    sends = []
    for k in range(K):
        t = app.send(f"focus {nodes[k % 2]}")
        app.wait("applied")
        sends.append(t)
        time.sleep(0.15)
    time.sleep(0.5)
    app_cpu1 = cpu_of(app.pid)
    cl_cpu1 = [cpu_of(c.pid) for c in clients]
    h = tool("hits", app.hwnd, 1)
    counts = {}
    for part in h.split():
        i, v = part.split(":")
        counts[NAMES.get(int(i), f"m{i}")] = int(v)
    total = sum(counts.values())
    lats, got = [], []
    for c in clients:
        c.drain()
        c.send("times")
        _, line = c.wait("times")
        ts = [int(x.split(":")[0]) for x in line.split()[2:]]
        got.append(len(ts))
        for s, t in zip(sends, ts):
            lats.append((t - s) / 1000)
    for c in clients:
        c.stop()
    res = dict(app=app.name, kind=kind, cache=cache, n=n, events=K, received=got,
               provider_calls_per_event=round(total / K, 1),
               calls={k: round(v / K, 1) for k, v in sorted(counts.items(), key=lambda x: -x[1])},
               app_cpu_ms_per_event=round((app_cpu1 - app_cpu0) / K, 3),
               client_cpu_ms_per_event=[round((b - a) / K, 3) for a, b in zip(cl_cpu0, cl_cpu1)],
               latency_ms_p50=round(statistics.median(lats), 2) if lats else None,
               latency_ms_max=round(max(lats), 2) if lats else None)
    RESULTS.append(res)
    log("RESULT " + repr(res))


procs = []
try:
    M = mockapp("protoM", backend="msaa"); procs.append(M)
    A = mockapp("protoA"); procs.append(A)
    for cache in ["none", "pid", "base"]:
        for n in [0, 1, 2, 4, 8]:
            run(M, "desk", cache, n, ["btn1", "item2"])
    for cache in ["none", "pid", "base"]:
        for n in [0, 1, 2, 4, 8]:
            run(A, "group", cache, n, ["btn1", "item2"])
finally:
    for p in procs:
        p.stop()
    save(os.path.join(HERE, "p2_cost.out"))
    with open(os.path.join(HERE, "p2_cost.summary.txt"), "w") as f:
        for r in RESULTS:
            f.write(repr(r) + "\n")
