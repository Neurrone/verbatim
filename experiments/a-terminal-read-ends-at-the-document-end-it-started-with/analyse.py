"""Summarises one run of footer-omission-probe: python -I analyse.py runs/<run>.tsv"""

import statistics
import sys


def pct(values, p):
    if not values:
        return "n/a"
    values = sorted(values)
    return f"{values[min(len(values) - 1, int(len(values) * p))]:.2f}"


def main():
    path = sys.argv[1]
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    print(lines[0])
    header = lines[1].split("\t")
    rows = [dict(zip(header, line.split("\t"))) for line in lines[2:] if "\terror" not in line]
    errors = sum(1 for line in lines[2:] if "\terror" in line)
    if errors:
        print(f"errors: {errors}")
    for phase in ("before", "flood", "after"):
        group = [r for r in rows if r["phase"] == phase]
        if not group:
            continue
        cut = [r for r in group if r["cut"] == "1"]
        moved = [r for r in group if r["end_moved"] != "0"]
        retried = [r for r in group if r["retry_cut"] != "-1"]
        print(f"{phase}: {len(group)} reads")
        print(f"  cut (the snapshot's last row with text is not the footer): {len(cut)}")
        print(f"  document's end moved during the read: {len(moved)}")
        print(f"  cut with the end moved: {sum(1 for r in cut if r['end_moved'] != '0')}")
        print(f"  cut with the end not moved: {sum(1 for r in cut if r['end_moved'] == '0')}")
        print(f"  end moved, not cut: {sum(1 for r in moved if r['cut'] == '0')}")
        print(
            f"  retried to the moved end: {len(retried)}, still cut {sum(1 for r in retried if r['retry_cut'] == '1')},"
            f" end moved again {sum(1 for r in retried if r['retry_moved'] != '0')}"
        )
        print(f"  screen's last row is the footer: {sum(1 for r in group if r['screen_footer'] == '1')}")
        print(f"  console window moved during the read: {sum(1 for r in group if r['window_moved'] != '0')}")
        caret_rows = sorted({r["cursor_row"] for r in group}, key=int)
        print(f"  caret rows (0 is the screen's top): {', '.join(caret_rows[:8])}{' ...' if len(caret_rows) > 8 else ''}; screen height {group[0]['window_height']}")
        caret_bottom = sum(1 for r in group if int(r["cursor_row"]) == int(r["window_height"]) - 1)
        print(f"  caret on the screen's last row: {caret_bottom}")
        read_ms = [float(r["read_ms"]) for r in group]
        print(f"  read ms: P50 {pct(read_ms, 0.5)}, P95 {pct(read_ms, 0.95)}, max {max(read_ms):.2f}")
        for name, sub in (("cut", cut), ("not cut", [r for r in group if r["cut"] == "0"])):
            since = [float(r["since_event_ms"]) for r in sub if float(r["since_event_ms"]) >= 0]
            during = sum(1 for r in sub if r["events_during"] != "0")
            if sub:
                print(
                    f"  {name}: console update event during the read in {during} of {len(sub)};"
                    f" ms since the last before it P50 {pct(since, 0.5)}, P95 {pct(since, 0.95)}"
                )
        flood = [int(r["flood_n"]) for r in group]
        if phase == "flood" and len(group) > 1:
            t = [float(r["t_ms"]) for r in group]
            print(f"  flood lines written while reading: {flood[0]} to {flood[-1]} over {t[-1] - t[0]:.0f} ms")
        if phase == "after":
            print(f"  footer text: {statistics.mode(r['doc_last'] for r in group)!r}")


main()
