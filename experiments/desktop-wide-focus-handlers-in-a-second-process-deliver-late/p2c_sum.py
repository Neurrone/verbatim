import sys, re, collections
for tag in sys.argv[1:]:
    lines = open(f"p2c_{tag}.out").read().splitlines()
    sends = [int(re.match(r"\[(\d+)\]", l).group(1)) for l in lines if "protoM: applied" in l]
    per = collections.defaultdict(list)
    for l in lines:
        m = re.search(r"\] (C\d+): ev \S+ (\S+) focus t=(\d+)", l)
        if m: per[(m.group(1), m.group(2) + str(len(per)))] if False else per[m.group(1)].append(int(m.group(3)))
    out = []
    for c, ts in sorted(per.items()):
        out.append(f"{c}:{len(ts)}ev max_after_prev_send={max(round((t - max([s for s in sends if s <= t] or [0])) / 1000) for t in ts)}ms")
    print(tag, "sends", len(sends), " ".join(out))
