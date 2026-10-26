"""Summarise a conhost-caret-probe output: each signal's latency after each press.

Usage: python analyse.py <out.tsv>
"""
import sys
from collections import defaultdict

NAMES = {
    0x4001: "EVENT_CONSOLE_CARET", 0x4002: "EVENT_CONSOLE_UPDATE_REGION",
    0x4003: "EVENT_CONSOLE_UPDATE_SIMPLE", 0x4004: "EVENT_CONSOLE_UPDATE_SCROLL",
    0x4005: "EVENT_CONSOLE_LAYOUT", 0x4006: "EVENT_CONSOLE_START_APPLICATION",
    0x4007: "EVENT_CONSOLE_END_APPLICATION", 0x800B: "EVENT_OBJECT_LOCATIONCHANGE",
    0x800E: "EVENT_OBJECT_VALUECHANGE", 0x8014: "EVENT_OBJECT_TEXTSELECTIONCHANGED",
    0x8005: "EVENT_OBJECT_FOCUS", 0x800C: "EVENT_OBJECT_NAMECHANGE",
    0x8002: "EVENT_OBJECT_SHOW", 0x8003: "EVENT_OBJECT_HIDE",
}
UIA = {"20014": "UIA TextSelectionChanged", "20015": "UIA TextChanged",
       "20005": "UIA focus changed", "20035": "UIA Notification",
       "20036": "UIA ActiveTextPositionChanged", "20032": "UIA TextEdit_TextChanged"}


def pct(values, p):
    values = sorted(values)
    if not values:
        return float("nan")
    k = (len(values) - 1) * p / 100
    lo = int(k)
    hi = min(lo + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (k - lo)


def main(path):
    presses, records, header, moves = [], [], "", []
    section = None
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.startswith("# "):
            header = line
        elif line.startswith("## presses"):
            section = "p"
        elif line.startswith("## records"):
            section = "r"
        elif section == "p":
            i, at, key, before, after, moved_ms = line.split("\t")
            presses.append((float(at) / 1000, key, before, after))
            moves.append(float(moved_ms))
        elif section == "r":
            parts = line.split("\t")
            records.append((float(parts[0]) / 1000, parts[1:]))
    window = header.split("window=")[1].split()[0]
    print(header)

    def kind(fields):
        src = fields[0]
        if src == "win":
            ev = int(fields[1], 16)
            hwnd = fields[2].split("=")[1]
            obj = fields[3].split("=")[1]
            where = "console" if hwnd == window else f"hwnd {hwnd}"
            if (0x100 <= ev <= 0x1FF or ev == 0x8014) and int(obj) > 0:
                obj = "n"
            return f"{NAMES.get(ev, hex(ev))} obj={obj} ({where})"
        if src in ("uia", "uiafocus"):
            return f"{UIA.get(fields[1], 'UIA ' + fields[1])} {fields[2]}"
        if src == "uiaprop":
            return f"UIA property {fields[1]} {fields[2]}"
        return None

    idle = [r for r in records if r[1][0] == "mark"]
    idle_start = next(r[0] for r in idle if r[1][1] == "idle-start")
    idle_end = next(r[0] for r in idle if r[1][1] == "idle-end")
    idle_counts = defaultdict(int)
    for at, fields in records:
        k = kind(fields)
        if k and idle_start <= at < idle_end:
            idle_counts[k] += 1
    print(f"idle {idle_end - idle_start:.0f} ms:", dict(idle_counts) or "nothing")

    moved = [p for p in presses if p[2] != p[3]]
    print(f"presses {len(presses)}, moved {len(moved)}")
    seen = [m for m in moves if m >= 0]
    print(f"caret move seen by spin reads in {len(seen)}/{len(moves)}: ms after key "
          f"P50 {pct(seen, 50):.2f} P95 {pct(seen, 95):.2f} max {max(seen):.2f} min {min(seen):.2f}")
    latencies = defaultdict(list)
    counts = defaultdict(list)
    for n, (at, key, before, after) in enumerate(presses):
        end = presses[n + 1][0] if n + 1 < len(presses) else at + 10_000
        seen = {}
        per = defaultdict(int)
        for t, fields in records:
            if at <= t < end:
                k = kind(fields)
                if k:
                    per[k] += 1
                    seen.setdefault(k, t - at)
        for k, v in seen.items():
            latencies[k].append(v)
            counts[k].append(per[k])
    for k in sorted(latencies, key=lambda k: pct(latencies[k], 50)):
        v = latencies[k]
        print(f"{k}: in {len(v)}/{len(presses)} presses; first after key ms "
              f"P50 {pct(v, 50):.1f} P95 {pct(v, 95):.1f} max {max(v):.1f} min {min(v):.1f}; "
              f"per press {min(counts[k])}-{max(counts[k])}")
    caret = [r[0] for r in records if r[1][0] == "win" and r[1][1] == "0x4001"
             and r[1][2].split("=")[1] == window]
    gaps = [b - a for a, b in zip(caret, caret[1:])]
    print("EVENT_CONSOLE_CARET gaps ms, first 40:", " ".join(f"{g:.0f}" for g in gaps[:40]))


main(sys.argv[1])
