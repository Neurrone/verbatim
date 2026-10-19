"""Summarises boundary_collision output: per case, read path and arm, the
listener's read times and outcomes, O's operation outcomes, and the
parent's UiaRootObjectId WM_GETOBJECTs from the read's start on, as a
sequence of nonzero (N) and zero (Z) wParams, with the time the last
nonzero one was taken after the release."""

import re
import sys
from collections import defaultdict
from statistics import median


def parse(path):
    trials = []
    cur = None
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.startswith("=== "):
            case, rpath, *arm, n = line[4:].split("-")
            cur = {"case": case, "path": rpath, "arm": "-".join(arm), "n": int(n),
                   "getobj": [], "o": [], "evidence": []}
            trials.append(cur)
        elif line.startswith("listener read: "):
            m = re.match(r"listener read: ([\d.]+) ms; calls (.*?); (.*)", line)
            cur["read_ms"] = float(m.group(1))
            cur["calls"] = m.group(2)
            cur["outcome"] = re.sub(r"\(Some\(\"Mockapp Counts Fixture\"\), \d+\)", "(Some(\"Mockapp Counts Fixture\"), <child window>)", m.group(3))
        elif line.startswith("released at "):
            m = re.match(r"released at ([-\d.]+) ms .*\((.*)\)", line)
            cur["released_ms"] = float(m.group(1))
            cur["release_kind"] = m.group(2)
        elif line.startswith("O op done") or line.startswith("O plainread done"):
            cur["o"].append(line)
        elif line.startswith("P WM_GETOBJECT at "):
            m = re.match(r"P WM_GETOBJECT at ([-\d.]+) (\S+) hwnd=\S+ wparam=(0x[0-9a-f]+) lparam=(-?\d+)", line)
            cur["getobj"].append((float(m.group(1)), m.group(2), int(m.group(3), 16), int(m.group(4))))
        elif line.startswith("P hits"):
            cur["hits"] = line
        elif line.startswith("evidence: "):
            cur["evidence"].append(line)
    return trials


def stats(values):
    if not values:
        return "none"
    return f"{min(values):.1f} to {max(values):.1f} ms (median {median(values):.1f})"


def main(path):
    trials = parse(path)
    listener_base = None
    groups = defaultdict(list)
    for t in trials:
        groups[(t["case"], t["path"], t["arm"])].append(t)
    for key, ts in groups.items():
        case, rpath, arm = key
        print(f"== {case} / {rpath} / {arm}: {len(ts)} trials")
        print(f"   listener read: {stats([t['read_ms'] for t in ts])}")
        outcomes = defaultdict(int)
        for t in ts:
            outcomes[t["outcome"]] += 1
        for o, c in outcomes.items():
            print(f"   outcome x{c}: {o}")
        calls = defaultdict(int)
        for t in ts:
            calls[t["calls"]] += 1
        for o, c in calls.items():
            print(f"   calls x{c}: {o}")
        rel = [t["released_ms"] for t in ts if "released_ms" in t]
        if rel:
            kinds = defaultdict(int)
            for t in ts:
                kinds[t.get("release_kind")] += 1
            print(f"   released: {stats(rel)}; {dict(kinds)}")
        ops = [o for t in ts for o in t["o"] if o.startswith("O op done")]
        if ops:
            results = defaultdict(int)
            times = []
            for o in ops:
                m = re.match(r"O op done \d+ [\d.]+ (.*) \[at ([-\d.]+) ms", o)
                results[m.group(1)] += 1
                times.append(float(m.group(2)))
            print(f"   O op done at {stats(times)} rel read start; results {dict(results)}")
            gaps = [float(re.search(r"\[at ([-\d.]+) ms", o).group(1)) - t["released_ms"]
                    for t in ts for o in t["o"] if o.startswith("O op done")]
            print(f"   O op done after the release by {stats(gaps)}")
        # The parent's UiaRootObjectId messages taken from the read's start
        # on, as nonzero (N) or zero (Z) wParam, each with its time after
        # the release (or after the read's start where nothing was held).
        seqs = defaultdict(int)
        last_n = []
        for t in ts:
            base = t.get("released_ms", 0.0)
            msgs = [(at, w) for at, _, w, l in t["getobj"] if at >= 0 and l == -25]
            seqs[" ".join("N" if w else "Z" for _, w in msgs)] += 1
            ns = [at - base for at, w in msgs if w]
            if ns:
                last_n.append(ns[-1])
        for seq, c in seqs.items():
            print(f"   UiaRootObjectId messages taken by P x{c}: {seq or 'none'}")
        if last_n:
            print(f"   last nonzero one taken {stats(last_n)} after {'the release' if any('released_ms' in t for t in ts) else 'the read began'}")
        hits = defaultdict(int)
        for t in ts:
            hits[t.get("hits")] += 1
        for h, c in hits.items():
            print(f"   x{c} {h}")


if __name__ == "__main__":
    main(sys.argv[1])
