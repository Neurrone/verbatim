"""Q2 follow-up: after a heavy flood, how long until the client delivers again, and what its worker does.
Usage: q2long.py <tag> <layout one|two> <coalesce> <kind> <count> <interval> <watch_s> [stacks]
"""
import sys, os, time, re
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evc import *

tag, layout, co, kind, count, interval, watch = sys.argv[1:8]
want_stacks = len(sys.argv) > 8
watch = float(watch)
open_out(f"q2long_{tag}.out")
flags = f"cto=2000,tto=5000,coalesce={co},recovery=1"
procs = []
try:
    A = mockapp("qlA", fixture="text.json"); procs.append(A)
    B = mockapp("qlB"); procs.append(B)
    if layout == "one":
        D = client("D", flags, f"qname:{A.hwnd}", f"qtext:{A.hwnd}", f"name:{B.hwnd}"); procs.append(D); DA = DB = D
    else:
        DA = client("DA", flags, f"qname:{A.hwnd}", f"qtext:{A.hwnd}"); procs.append(DA)
        DB = client("DB", flags, f"name:{B.hwnd}"); procs.append(DB)
    time.sleep(0.5)
    B.send("set-name btn1 Warm"); B.wait("applied"); time.sleep(0.3)
    t0 = A.send(f"flood {count} {interval} {kind}")
    t_ack, _ = A.wait("applied", timeout=120)
    res(f"{tag}: flood of {count} at {interval} ms raised in {(t_ack - t0) / 1e6:.2f} s")
    probes = {}
    k = 0
    end = time.time() + watch
    next_report = time.time()
    dumped = False
    nb = len(DB.lines)
    while time.time() < end:
        k += 1
        probes[k] = B.send(f"set-name btn1 L{k}"); B.wait("applied")
        time.sleep(0.25)
        if time.time() >= next_report:
            st = stats(DA)
            n = sum(v[0] for lab, v in st.items() if lab.startswith("q"))
            c = cpu(DA)
            wcpu = sum(x for kk, x in c.items() if kk.startswith("worker") and not kk.endswith("_cycles"))
            got = sorted(int(m.group(1)) for _, l in DB.lines[nb:] for m in [re.search(r'name="L(\d+)"', l)] if m)
            res(f"{tag}: +{(now_us() - t_ack) / 1e6:.1f} s after the flood: A's events received {n}; worker CPU {wcpu:.0f} ms; B probes sent {k}, delivered {len(got)}")
            next_report = time.time() + 2
            if want_stacks and not dumped and (now_us() - t_ack) > 2e6:
                stacks(DA.pid, f"q2long_{tag}")
                dumped = True
    arrived = {}
    for _, l in DB.lines[nb:]:
        m = re.search(r' t=(\d+) .*name="L(\d+)"', l)
        if m:
            arrived[int(m.group(2))] = int(m.group(1))
    first = min(arrived) if arrived else None
    if first:
        res(f"{tag}: first B probe delivered L{first}, raised {(probes[first] - t_ack) / 1e6:.1f} s after the flood, delay {(arrived[first] - probes[first]) / 1000:.0f} ms; delivered {len(arrived)} of {k}")
    else:
        res(f"{tag}: no B probe delivered in {watch} s")
finally:
    for p in reversed(procs):
        p.stop()
    save(f"q2long_{tag}.log")
