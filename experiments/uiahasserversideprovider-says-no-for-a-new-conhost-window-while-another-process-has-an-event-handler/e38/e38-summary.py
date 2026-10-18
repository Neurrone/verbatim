"""Prints one line per e38 trial, then counts per set: the delay asked for
and the actual one (the first probe's start less the time the last
console window was found), when that window became visible and the
foreground window, each call's time and answer (U and R as in e37; L's
provider probe, console check and remote focus reads of the window), and
an "ODD" mark on any trial with a "no", a proxy, a raw answer other than
0x10000, a call over 1000 ms, or a call with no answer before the trial
ended (NO-ANSWER-IN-TRIAL; the console is closed 1.5 s after U's lookup). Usage: e38-summary.py <set>..."""
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


for name in sys.argv[1:]:
    text = (HERE / f"{name}.out").read_text(encoding="utf-8")
    print(f"## {name}")
    stats = defaultdict(list)
    odd = 0
    trials = 0
    for run in text.split("=== run ")[1:]:
        trials += 1
        head = re.match(r"(\d+) delay=([\d.]+)", run)
        n, delay = head.group(1), float(head.group(2))
        founds = list(re.finditer(r"run (\S+) hwnd (\d+) found console .*after-launch-ms=([\d.]+) visible=(\w+)", run))
        if not founds or "console closed" not in run:
            print(f"  run {n} incomplete")
            continue
        found = founds[-1]
        t_found, hwnd = us(found.group(1)), found.group(2)
        after = run[found.end():]
        vis = re.search(r"console-visible after-found-ms=([\d.]+)", after)
        fg = re.search(r"console-foreground after-found-ms=([\d.]+)", after)
        parts = [f"run {n:>2} delay={delay:g}", f"found+{found.group(3)}ms",
                 f"vis+{vis.group(1) if vis else '-'} fg+{fg.group(1) if fg else '-'}"]
        first = None
        bad = False
        for m in re.finditer(r"([URV]) \S+ done (\w+)(?: \d+)? [URV] start=(\S+) ms=([\d.]+) (.*)", after):
            who, cmd, start, ms, rest = m.groups()
            if cmd in ("init", "target"):
                continue
            if cmd in ("probe", "rawprobe") and first is None:
                first = (us(start) - t_found) / 1000
            if cmd == "efhbcd":
                ans = kind(rest) if rest.startswith("ok") else rest[:60]
                bad |= ans != "real"
            elif cmd == "probe":
                ans = rest.replace("result=", "")
                bad |= ans != "true"
            elif cmd == "rawprobe":
                ans = rest.replace("sent=", "s").replace(" lresult=", "/")
                bad |= ans != "s1/0x10000"
            else:
                ans = rest.split()[0] if rest else ""
            bad |= float(ms) > 1000
            parts.append(f"{who}.{cmd} {ms}ms {ans}")
            stats[f"{who}.{cmd} {ans}"].append(float(ms))
        sent = re.findall(r"run send ([URV]) (probe|rawprobe|efhbcd)\b", after)
        done = re.findall(r"([URV]) \S+ done (probe|rawprobe|efhbcd) ", after)
        for who, cmd in sent:
            if (who, cmd) not in done:
                parts.append(f"{who}.{cmd} NO-ANSWER-IN-TRIAL")
                stats[f"{who}.{cmd} no answer within the trial"].append(0.0)
                bad = True
        lp = [m for m in re.finditer(r" L done lprobe start=(\S+) ms=([\d.]+) hwnd=(\d+) result=(\w+)", after) if m.group(3) == hwnd]
        for m in lp:
            parts.append(f"L.probe@{(us(m.group(1)) - t_found) / 1000:.0f} {m.group(2)}ms {m.group(4)}")
            stats[f"L.probe {m.group(4)}"].append(float(m.group(2)))
            bad |= m.group(4) != "true" or float(m.group(2)) > 1000
        reads = [m for m in re.finditer(r" L done lread start=(\S+) ms=([\d.]+) queued_ms=[\d.]+ rid=\[42, (\d+)[^\]]*\] (\S+)", after) if m.group(3) == hwnd]
        if reads:
            first_read = (us(reads[0].group(1)) - t_found) / 1000
            parts.append(f"L.reads {len(reads)} first@{first_read:.0f} max {max(float(r.group(2)) for r in reads):.1f}ms {','.join(sorted(set(r.group(4) for r in reads)))}")
            for r in reads:
                stats[f"L.read {r.group(4)}"].append(float(r.group(2)))
        elif "listen L" in run:
            parts.append("L.reads 0")
        parts.insert(1, f"actual={first:.1f}" if first is not None else "actual=?")
        if bad:
            odd += 1
            parts.append("ODD")
        if "timed out" in run or "run timeout" in run:
            parts.append("TIMEOUT")
        print("  " + " ; ".join(parts))
    print(f"### {name}: {trials} trials, {odd} odd; answer: count, min-max ms")
    for k, v in sorted(stats.items()):
        print(f"  {k}: {len(v)}, {min(v):.1f}-{max(v):.1f}")
