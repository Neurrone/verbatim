"""Q2: floods. App A floods name or text changes (mockapp `flood`); app B renames btn1 every 100 ms.
B's event delay at the client holding B's registration, and the UIA worker thread's CPU per event.

Usage: q2.py <tag> <layout one|two> <coalesce 0|1> <kind name|text> <count> <interval_ms> [...more count/interval pairs]
layout one: D holds qname:A, qtext:A and name:B. layout two: DA holds A's, DB holds B's.
"""
import sys, os, time, re, statistics
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evc import *

tag, layout, coalesce, kind = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
pairs = [(int(sys.argv[i]), sys.argv[i + 1]) for i in range(5, len(sys.argv), 2)]
open_out(f"q2_{tag}.out")
flags = f"cto=2000,tto=5000,coalesce={coalesce},recovery=1"
procs = []


def worker_sum(c):
    v = cpu(c)
    ms = sum(x for k, x in v.items() if k.startswith("worker") and (k.endswith("_user") or k.endswith("_kernel")))
    cyc = sum(x for k, x in v.items() if k.endswith("_cycles"))
    workers = sorted({k.split("_")[0] for k in v if k.startswith("worker")})
    return ms, cyc, v["proc_user"] + v["proc_kernel"], workers


try:
    A = mockapp("q2A", fixture="text.json"); procs.append(A)
    B = mockapp("q2B"); procs.append(B)
    if layout == "one":
        D = client("D", flags, f"qname:{A.hwnd}", f"qtext:{A.hwnd}", f"name:{B.hwnd}"); procs.append(D)
        DA, DB = D, D
    else:
        DA = client("DA", flags, f"qname:{A.hwnd}", f"qtext:{A.hwnd}"); procs.append(DA)
        DB = client("DB", flags, f"name:{B.hwnd}"); procs.append(DB)
    time.sleep(0.5)
    # Warm-up: one event of each so every worker thread has run a handler once.
    A.send("flood 1 0 name"); A.wait("applied"); A.send("flood 1 0 text"); A.wait("applied")
    B.send("set-name btn1 Warm"); B.wait("applied"); time.sleep(0.5)
    for count, interval in pairs:
        for c in {id(DA): DA, id(DB): DB}.values():
            c.send("reset"); c.wait("reset")
        a_ms0, a_cyc0, a_proc0, wa = worker_sum(DA)
        b_ms0, b_cyc0, b_proc0, wb = worker_sum(DB) if DB is not DA else (0, 0, 0, [])
        nb_lines = len(DB.lines)
        A.drain()
        t_start = A.send(f"flood {count} {interval} {kind}")
        probes = {}
        k = 0
        t_ack = None
        last_n, last_change = -1, time.time()
        while True:
            k += 1
            probes[k] = B.send(f"set-name btn1 P{k}"); B.wait("applied")
            if t_ack is None:
                for t, l in A.drain():
                    if l.startswith("applied"):
                        t_ack = t
                    elif l.startswith("rejected"):
                        raise SystemExit(l)
            time.sleep(0.1)
            if t_ack is not None:
                st = stats(DA)
                n = sum(v[0] for lab, v in st.items() if lab.startswith("q"))
                if n != last_n:
                    last_n, last_change = n, time.time()
                elif time.time() - last_change > 1.0:
                    break
            if time.time() - t_start / 1e6 > 120:
                break
        time.sleep(0.3)
        st = stats(DA)
        recv = sum(v[0] for lab, v in st.items() if lab.startswith("q"))
        a_last = max((v[2] for lab, v in st.items() if lab.startswith("q")), default=0)
        a_ms1, a_cyc1, a_proc1, wa1 = worker_sum(DA)
        if DB is not DA:
            b_ms1, b_cyc1, b_proc1, wb1 = worker_sum(DB)
        arrived = {}
        for _, l in DB.lines[nb_lines:]:
            m = re.search(r" t=(\d+) .*name=\"P(\d+)\"", l)
            if m:
                arrived[int(m.group(2))] = int(m.group(1))
        during, after, lost = [], [], 0
        for i, raised in probes.items():
            if i not in arrived:
                lost += 1
                continue
            d = (arrived[i] - raised) / 1000
            (during if t_ack is None or raised <= t_ack else after).append(d)
        dur = (t_ack - t_start) / 1e6 if t_ack else float("nan")
        f = lambda xs: (f"n={len(xs)} median={statistics.median(xs):.1f} max={max(xs):.0f}" if xs else "n=0")
        res(f"RESULT {tag} layout={layout} coalesce={coalesce} kind={kind} count={count} interval={interval}: "
            f"raised in {dur:.2f} s ({count / dur:.0f}/s); A's events received {recv} ({recv / count:.0%}), last {(a_last - t_ack) / 1000 if a_last and t_ack else float('nan'):.0f} ms after the flood's end; "
            f"B's delay during flood {f(during)}; after it {f(after)}; B's lost {lost}; "
            f"A-client worker CPU {a_ms1 - a_ms0:.0f} ms, {(a_cyc1 - a_cyc0) / max(recv, 1) / 1e3:.0f} kcycles/event, {(a_ms1 - a_ms0) * 1000 / max(recv, 1):.1f} us/event; A-client process CPU {a_proc1 - a_proc0:.0f} ms; workers {wa1}"
            + (f"; B-client worker CPU {b_ms1 - b_ms0:.0f} ms" if DB is not DA else ""))
        time.sleep(1.0)
finally:
    for p in reversed(procs):
        p.stop()
    save(f"q2_{tag}.log")
