"""H6: what a short connection timeout costs: is a slow window's own focus event delivered once its
wait ends, or lost when the wait times out? M is a mockapp with the MSAA backend (its focus reaches a
desktop handler on this desktop, P2). D has a desktop focus handler with the flags' timeouts.
M raises focus while answering, then raises focus at the start of a 3 s stall (from inside it).
Usage: h6_lost.py <tag> <flags>"""
import time, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import *
HERE = os.path.dirname(os.path.abspath(__file__))
tag, flags = sys.argv[1], sys.argv[2]
procs = []
try:
    M = mockapp("holdM", backend="msaa"); procs.append(M)
    D = client("D", flags, "desk:min"); procs.append(D)
    time.sleep(0.3)
    t0 = M.send("focus item1"); M.wait("applied")
    try:
        t, l = D.wait("ev D desk", timeout=3); log(f"RESULT {tag} answering: focus after {(t - t0) / 1000:.0f} ms: {l}")
    except TimeoutError:
        log(f"RESULT {tag} answering: no focus event in 3 s")
    t_stall = M.send("stall 3000 0 focus item2"); M.wait("stall started")
    try:
        t, l = D.wait("ev D desk", timeout=8)
        log(f"RESULT {tag} raised inside a 3 s stall: focus {(t - t_stall) / 1000:.0f} ms into the stall: {l}")
    except TimeoutError:
        log(f"RESULT {tag} raised inside a 3 s stall: no focus event in 8 s")
    M.wait("stall ended", timeout=5)
    time.sleep(0.3)
    t0 = M.send("focus item1"); M.wait("applied")
    try:
        t, l = D.wait("ev D desk", timeout=3); log(f"RESULT {tag} answering again: focus after {(t - t0) / 1000:.0f} ms: {l}")
    except TimeoutError:
        log(f"RESULT {tag} answering again: no focus event in 3 s")
finally:
    for p in procs: p.stop()
    save(os.path.join(HERE, f"h6_{tag}.out"))
