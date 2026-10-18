"""Summarizes e39 experiment 2 sets: per trial, the probe (if any) and the
lookup, their answers and times, and their sum; per set, counts by
outcome and the distribution (min, median, 90th percentile, max) of the
probe, the lookup and the sum. A trial is "correct" when the probe (if
any) says yes and the lookup returns conhost's own element (kind=native).
Usage: e39-summary.py <set>..."""
import re
import statistics
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).parent


def us(t):
    h, m, s = t.split(":")
    return round((int(h) * 3600 + int(m) * 60 + float(s)) * 1e6)


def dist(values):
    if not values:
        return "-"
    v = sorted(values)
    p90 = v[min(len(v) - 1, round(0.9 * (len(v) - 1)))]
    return f"n={len(v)} min {v[0]:.1f} median {statistics.median(v):.1f} p90 {p90:.1f} max {v[-1]:.1f}"


for name in sys.argv[1:]:
    text = (HERE / f"{name}.out").read_text(encoding="utf-8")
    print(f"## {name}")
    outcomes = Counter()
    probes, lookups, sums, actual = [], [], [], []
    for run in text.split("=== run ")[1:]:
        n = run.split()[0]
        found = re.search(r"run (\S+) hwnd (\d+) found console", run)
        if not found:
            print(f"  run {n} incomplete")
            continue
        t0 = us(found.group(1))
        probe = re.search(r"V \S+ done (probe|rawprobe2) V start=(\S+) ms=([\d.]+) (.*)", run)
        look = re.search(r"V \S+ done lookup V start=(\S+) ms=([\d.]+) (\S+)(?: kind=(\w+))?", run)
        if not look:
            print(f"  run {n} no lookup answer")
            outcomes["no lookup answer"] += 1
            continue
        first_start = us(probe.group(2)) if probe else us(look.group(1))
        actual.append((first_start - t0) / 1000)
        parts = [f"run {n:>2} at {(first_start - t0) / 1000:.1f}"]
        verdict = "yes"
        pms = 0.0
        if probe:
            pms = float(probe.group(3))
            answer = probe.group(4)
            if probe.group(1) == "probe":
                verdict = "yes" if answer == "result=true" else "no"
                parts.append(f"probe {pms}ms {answer}")
            else:
                m = re.match(r"sent=(\d+) lresult=(\S+) err=(\d+)", answer)
                verdict = "yes" if m.group(1) == "1" and m.group(2) != "0x0" else "no"
                parts.append(f"raw {pms}ms sent={m.group(1)} lresult={m.group(2)} err={m.group(3)}")
            probes.append(pms)
        lms = float(look.group(2))
        kind = look.group(4) if look.group(3) == "ok" else look.group(3)
        lookups.append(lms)
        sums.append(pms + lms)
        parts.append(f"lookup {lms}ms {kind}")
        parts.append(f"sum {pms + lms:.1f}ms")
        outcome = f"probe {verdict}, lookup {kind}" if probe else f"lookup {kind}"
        outcomes[outcome] += 1
        mark = "" if verdict == "yes" and kind == "native" and pms + lms < 1000 else " ; ODD"
        print("  " + " ; ".join(parts) + mark)
    print(f"### {name}: outcomes")
    for k, v in sorted(outcomes.items()):
        print(f"  {k}: {v}")
    print(f"  first call's start after the window was found: {dist(actual)}")
    print(f"  probe ms: {dist(probes)}")
    print(f"  lookup ms: {dist(lookups)}")
    print(f"  sum ms: {dist(sums)}")
