"""Counts Windows Terminal's second terminal identity in a listener log.

usage: python -I identity_judge.py <listener.log or dir of runs> [--verbose]

The log is split into segments, each starting at a foreground event of a
Windows Terminal window (a 14-id event up to 200 ms before a foreground
belongs to that foreground's segment): a "launch" segment is a window's
first foreground when it is the first window of its process run, "second"
a second window's first, "return" any later foreground of a known window.

Per segment kind:
- odd: segments with any event from a runtime id starting 14;
- 14 focus: focus events from such an id received;
- 14 accepted: those not dropped (no "UIA focus dropped" names the marker);
- extra: accepted terminal focuses in the segment beyond the first, which
  Verbatim announces again ("terminal" said more than once).
"""
import os
import re
import sys
from datetime import datetime

ANSI = re.compile(r"\x1b\[[0-9;]*m")
REC = re.compile(r"UIA focus event received pid=(\d+) marker=(\d+) kind=(\w+) runtime_id=\[([^\]]*)\]")
DROP = re.compile(r"UIA focus dropped pid=(\d+) marker=(\d+) reason=\"([^\"]*)\"")
FG = re.compile(r"WinEvent received kind=Foreground .*?hwnd=(\d+)")
CASCADIA = re.compile(r"UIA focus sender runtime_id=\[42, (\d+)\] class_name=Some\(\"CASCADIA_HOSTING_WINDOW_CLASS\"\)")
KINDS = ("launch", "second", "return")


def ts(line):
    return datetime.strptime(line[:26], "%Y-%m-%dT%H:%M:%S.%f").timestamp()


def analyse(path, verbose):
    lines = [ANSI.sub("", l) for l in open(path, encoding="utf-8", errors="replace")]
    cascadia = {int(m.group(1)) for l in lines if (m := CASCADIA.search(l))}
    events, drops, fgs = [], {}, []
    for l in lines:
        if m := REC.search(l):
            rid = [int(x) for x in m.group(4).split(",") if x.strip()]
            events.append((ts(l), m.group(1), m.group(2), m.group(3), rid))
        elif m := DROP.search(l):
            drops[(m.group(1), m.group(2))] = m.group(3)
        elif (m := FG.search(l)):
            fgs.append((ts(l), int(m.group(1))))
    # Segments: every foreground; Windows Terminal ones are judged. A WT
    # window's pid is learnt from its focus events: the pid of a [42, hwnd]
    # focus event.
    pid_of = {}
    for (_, pid, _, _, rid) in events:
        if len(rid) == 2 and rid[0] == 42 and rid[1] in cascadia:
            pid_of.setdefault(rid[1], pid)
    seen, windows_of_pid, segs = set(), {}, []
    for (t, h) in fgs:
        if h not in cascadia:
            segs.append((t, h, None))
            continue
        if h in seen:
            kind = "return"
        else:
            seen.add(h)
            pid = pid_of.get(h)
            n = windows_of_pid.get(pid, 0)
            windows_of_pid[pid] = n + 1
            kind = "launch" if n == 0 else "second"
        segs.append((t, h, kind))

    def segment(t):
        index = None
        for i, (st, _, _) in enumerate(segs):
            if st <= t + 0.2:
                index = i
        return index

    stats = {k: dict(n=0, odd=0, focus14=0, acc14=0, extra=0) for k in KINDS}
    per = {}
    for (t, pid, marker, kind, rid) in events:
        i = segment(t)
        if i is None or segs[i][2] is None:
            continue
        s = per.setdefault(i, dict(ev14=0, focus14=0, acc14=0, accepted=[]))
        is14 = bool(rid) and rid[0] == 14
        is42term = len(rid) == 4 and rid[0] == 42 and rid[2] == 4
        if is14:
            s["ev14"] += 1
        if kind != "Focus":
            continue
        s["focus14"] += is14
        if (pid, marker) in drops or not (is14 or is42term):
            continue
        s["acc14"] += is14
        s["accepted"].append(rid)
    for i, (t, h, kind) in enumerate(segs):
        if kind is None:
            continue
        s = per.get(i, dict(ev14=0, focus14=0, acc14=0, accepted=[]))
        st = stats[kind]
        st["n"] += 1
        st["odd"] += s["ev14"] > 0
        st["focus14"] += s["focus14"]
        st["acc14"] += s["acc14"]
        st["extra"] += max(0, len(s["accepted"]) - 1)
        if verbose or s["acc14"] or len(s["accepted"]) > 1:
            print(f"  {path[-60:]} segment {i} {kind} hwnd {h}: 14-id events {s['ev14']}, accepted {s['accepted']}")
    return stats


def main():
    target = sys.argv[1]
    verbose = "--verbose" in sys.argv
    paths = [target] if os.path.isfile(target) else [
        os.path.join(r, "listener.log") for r, _, f in sorted(os.walk(target)) if "listener.log" in f]
    tot = {k: dict(n=0, odd=0, focus14=0, acc14=0, extra=0) for k in KINDS}
    for p in paths:
        st = analyse(p, verbose)
        for k in KINDS:
            for f in tot[k]:
                tot[k][f] += st[k][f]
    for k in KINDS:
        t = tot[k]
        print(f"{k}: segments {t['n']}, odd {t['odd']}, 14-id focus receipts {t['focus14']}, "
              f"14-id focuses accepted {t['acc14']}, extra terminal announcements {t['extra']}")


main()
