import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import probe_rl as P
R = P.R
for b in ([("edit",R.SEL)]*3, [("edit",R.SEL),("window",R.SEL),("edit",R.SEL),("edit",R.NAME),("edit",R.SEL),("window",R.SEL)], [("edit",R.SEL)]*4 + [("window",R.SEL)]):
    print(len(b), b, "->", P.gated(b), flush=True)
print("survived")
