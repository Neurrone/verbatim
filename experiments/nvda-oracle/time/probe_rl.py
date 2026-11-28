import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_ratelimit as R
def gated(burst):
    R.received.clear(); R.gate.update(armed=True, open=False, entered=False)
    R.send("window", R.NAME)
    while not R.gate["entered"]: R.REAL_SLEEP(0.001)
    for n,e in burst: R.send(n,e)
    R.gate["open"]=True
    R.REAL_SLEEP(0.3)
    return [(n,e) for n,e,_ in R.received]
print("calls", {k:v for k,v in R.H.S.CALLS.items() if 'Runtime' in k})
for b in ([("edit",R.SEL),("edit",R.SEL)], [("window",R.SEL),("edit",R.SEL)], [("edit",R.SEL),("window",R.SEL)], R.BURST):
    print(b, "->", gated(b))
print("errors", R.H.S.MISSES[-5:])
