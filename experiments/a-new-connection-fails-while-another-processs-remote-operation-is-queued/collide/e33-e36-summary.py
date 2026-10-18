"""Prints one line per trial of e33 to e36: the call under test, its time,
whether the element found carries the application's own provider (the
provider description names the application's executable or process as
Main, rather than only UIA's proxies), and when L's operation finished."""
import re
import sys
from pathlib import Path

for path in sys.argv[1:]:
    text = Path(path).read_text(encoding="utf-8", errors="replace")
    print(f"## {Path(path).stem}")
    for n, run in enumerate(text.split("=== run ")[1:], 1):
        busy = re.search(r"busy-begin|stall-begin", run)
        lop = re.search(r"done async-rop L start=\S+ ms=([\d.]+)", run)
        parts = [f"run {n}:"]
        for m in re.finditer(r"([BC]) \S+ done (\w+) [BC] start=\S+ ms=([\d.]+) (.*)", run):
            who, cmd, ms, rest = m.groups()
            if cmd in ("init", "target", "efh", "docchild", "holdfirstchild"):
                continue
            own = ""
            desc = re.search(r'desc="([^"]*)"', rest)
            if desc:
                mains = re.findall(r"Main(?:\(parent link\))?:[^;\[\]]*?\((?:un)?managed:([^)]+)\)", desc.group(1))
                own = "real" if any(m.lower() != "uiautomationcore.dll" for m in mains) else "proxy"
            short = re.sub(r' desc="[^"]*"', "", rest)
            short = re.sub(r"\(\d+\.\dms\)", "", short)[:150]
            parts.append(f"{who}.{cmd} {ms}ms [{own}] {short}")
        parts.append(f"L.rop {lop.group(1) if lop else 'none'}ms")
        print("  " + " ; ".join(parts))
