import sys, re, collections
for tag in sys.argv[1:]:
    lines = open(f"p2d_{tag}.out").read().splitlines()
    sends = [int(re.search(r"winevent sent t=(\d+)", l).group(1)) for l in lines if "winevent sent" in l]
    per = collections.defaultdict(list)
    for l in lines:
        m = re.search(r"\] (C\d+): ev \S+ focus t=(\d+)", l)
        if m: per[m.group(1)].append(int(m.group(2)))
    out = []
    for c, ts in sorted(per.items()):
        lats = [round((t - max([s for s in sends if s <= t] or [0])) / 1000, 1) for t in ts]
        out.append(f"{c}:{len(ts)}ev after_send_ms={lats}")
    print(tag, "sends", len(sends), " | ".join(out))
