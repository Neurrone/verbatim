# Summarizes e32-warm-up.out: per variant, the first probe of S's window
# and the lookup after it, and the warm-up's own probe, in ms.
import re, statistics, sys
seqs = {}
for line in open(sys.argv[1]):
    m = re.match(r"\s*([CXN])(\d+) \S+ done (probe|efh) \S+ start=\S+ ms=([\d.]+) (.*)", line)
    if m:
        seqs.setdefault(m.group(1), {}).setdefault(m.group(2), []).append((m.group(3), float(m.group(4)), m.group(5).strip()))
names = {"C": "cold", "X": "warm-other", "N": "warm-own"}
def stats(xs):
    xs = sorted(xs)
    return f"min {xs[0]}, median {statistics.median(xs)}, max {xs[-1]}"
for v in "CXN":
    runs = seqs.get(v, {})
    target = [calls[-2:] for calls in runs.values()]
    print(f"{names[v]}: n={len(runs)}; first target probe {stats([t[0][1] for t in target])}; "
          f"target lookup {stats([t[1][1] for t in target])}; answers {sorted(set(t[0][2] for t in target) | set(t[1][2] for t in target))}")
    warm = [calls[0][1] for calls in runs.values() if len(calls) == 4]
    if warm:
        print(f"   the warm-up's own probe: {stats(warm)}")
