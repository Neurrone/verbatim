"""Prints one line per e37 trial, then counts per set and delay band:
the delay asked for and the actual one (the probe's start less the time
the console window was found), how long after launch the window was found
and whether it was visible then, and each call's time and answer. An
element from ElementFromHandleBuildCache is "real" when its provider
description names conhost.exe as Main, "proxy" when the only Main
provider is in uiautomationcore.dll."""
import re
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).parent


def us(t):
    h, m, s = t.split(":")
    return round((int(h) * 3600 + int(m) * 60 + float(s)) * 1e6)


def kind(desc):
    mains = re.findall(r"Main(?:\(parent link\))?:[^;\[\]]*?\((?:un)?managed:([^)]+)\)", desc)
    return "real" if any(m.lower() == "conhost.exe" for m in mains) else "proxy"


def band(d):
    if d >= 1000:
        return "2000"
    if d == 0:
        return "0"
    return f"{int(d // 25) * 25}-{int(d // 25) * 25 + 25}"


sets = ["e37-parallel", "e37-rawfirst", "e37-alone", "e37-parallel-control", "e37-rawfirst-control"]
for name in sets:
    text = (HERE / f"{name}.out").read_text(encoding="utf-8")
    print(f"## {name}")
    stats = defaultdict(lambda: defaultdict(list))
    for run in text.split("=== run ")[1:]:
        head = re.match(r"(\d+) delay=([\d.]+)", run)
        n, delay = head.group(1), float(head.group(2))
        found = re.search(r"run (\S+) hwnd \d+ found console .*after-launch-ms=([\d.]+) visible=(\w+)", run)
        t_found = us(found.group(1))
        parts = [f"run {n:>2} delay={delay:g}", f"found+{found.group(2)}ms vis={found.group(3)}"]
        first = None
        b = band(delay)
        for m in re.finditer(r"([URV]) \S+ done (\w+) [URV] start=(\S+) ms=([\d.]+) (.*)", run):
            who, cmd, start, ms, rest = m.groups()
            if cmd in ("init", "target"):
                continue
            if cmd in ("probe", "rawprobe") and first is None:
                first = (us(start) - t_found) / 1000
            if cmd == "efhbcd":
                ans = kind(rest) if rest.startswith("ok") else rest[:60]
            elif cmd == "probe":
                ans = rest.replace("result=", "")
            else:
                ans = rest.replace("sent=", "s").replace(" lresult=", "/")
            parts.append(f"{who}.{cmd} {ms}ms {ans}")
            stats[b][f"{who}.{cmd} {ans}"].append(float(ms))
        parts.insert(1, f"actual={first:.1f}" if first is not None else "actual=?")
        print("  " + " ; ".join(parts))
    print(f"### {name} counts by delay band (answer: count, min-max ms)")
    for b in sorted(stats, key=lambda k: (float(k.split("-")[0]), len(k))):
        cells = [f"{k}: {len(v)}, {min(v):.1f}-{max(v):.1f}" for k, v in sorted(stats[b].items())]
        print(f"  {b} ms: " + " ; ".join(cells))
