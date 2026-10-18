"""Prints, for each trial of an e39 message-capture set, U's calls and the
WM_GETOBJECT messages conhost's hooks logged, in ms from the moment the
console window was found. Messages are labelled by wParam: U1 and U2 are
the cookies of U's probe and lookup (consecutive values, the probe's
first), Lx another process's nonzero cookie, 0 a zero wParam; the lParam
is shown as the object id with "s" for sign-extended to 64 bits or "z" for
zero-extended. A message with "call" but no "ret" is marked "(no ret)".
Usage: e39-msg-summary.py <set> [all|no]"""
import re
import sys
from pathlib import Path

HERE = Path(__file__).parent
name = sys.argv[1]
which = sys.argv[2] if len(sys.argv) > 2 else "no"


def us(t):
    h, m, s = t.split(":")
    return round((int(h) * 3600 + int(m) * 60 + float(s)) * 1e6)


OBJ = {0xffffffe7: "UiaRoot", 0xfffffffc: "OBJID_CLIENT", 0xfffffffb: "OBJID_VSCROLL", 0xfffffffa: "OBJID_HSCROLL",
       0xfffffffe: "OBJID_TITLEBAR", 0xfffffffd: "OBJID_MENU", 0xfffffff9: "OBJID_SIZEGRIP", 0: "OBJID_WINDOW"}
text = (HERE / f"{name}.out").read_text(encoding="utf-8")
counts = {"trials": 0, "no": 0, "probe-msgs-seen": 0}
for run in text.split("=== run ")[1:]:
    n = run.split()[0]
    found = re.search(r"run (\S+) hwnd (\d+) found console conhost-pid=(\d+)", run)
    if not found:
        continue
    counts["trials"] += 1
    t0, pid = us(found.group(1)), found.group(3)
    probe = re.search(r"U \S+ done probe U start=(\S+) ms=([\d.]+) result=(\w+)", run)
    look = re.search(r"U \S+ done lookup U start=(\S+) ms=([\d.]+) ok kind=(\w+)", run)
    hooked = re.search(r"run (\S+) hooked pid=" + pid, run)
    fg = re.search(r"console-foreground after-found-ms=([\d.]+)", run)
    vis = re.search(r"console-visible after-found-ms=([\d.]+)", run)
    is_no = probe and probe.group(3) == "false"
    counts["no"] += bool(is_no)
    if which == "no" and not is_no:
        continue
    print(f"run {n}: probe {probe.group(2)}ms {probe.group(3)} at {(us(probe.group(1)) - t0) / 1000:.1f}; "
          f"lookup {look.group(2) if look else '-'}ms {look.group(3) if look else '-'}; "
          f"hooked at {(us(hooked.group(1)) - t0) / 1000:.1f}; fg {fg.group(1) if fg else '-'} vis {vis.group(1) if vis else '-'}")
    log = HERE / "hooklogs" / f"{name}-{n}-{pid}.log"
    if not log.exists():
        print("  no hook log")
        continue
    lines = log.read_text(encoding="utf-8").splitlines()
    cookies = []
    events = []
    for line in lines:
        m = re.match(r"(\S+) tid=\d+ (call|ret)\s+hwnd=\d+ (?:class=\S+ )?msg=0x3d wparam=(0x[0-9a-f]+) lparam=(0x[0-9a-f]+)(?:.*lresult=(0x[0-9a-f]+))?", line)
        if not m:
            continue
        t, kind, wp, lp, lr = m.groups()
        wp, lp = int(wp, 16), int(lp, 16)
        if wp and lp == 0xffffffffffffffe7 and wp not in cookies:
            cookies.append(wp)
        events.append(((us(t) - t0) / 1000, kind, wp, lp, lr))
    ucookie = None
    for c in cookies:
        if c + 1 in cookies:
            ucookie = c
    label = {}
    for c in cookies:
        if c == ucookie:
            label[c] = "U1"
        elif ucookie is not None and c == ucookie + 1:
            label[c] = "U2"
        else:
            label[c] = f"L{len([k for k in label.values() if k.startswith('L')]) + 1}"
    firsts = [(t, label[wp]) for t, kind, wp, lp, lr in events if kind == "call" and wp in label]
    seen_labels = []
    for t, l in firsts:
        if l not in [x[1] for x in seen_labels]:
            seen_labels.append((t, l))
    first_ms = next((t for t, kind, *_ in events), None)
    order = " ".join(f"{l}@{t:.1f}" for t, l in seen_labels)
    u1ret = next((t for t, kind, wp, lp, lr in events if kind == "ret" and label.get(wp) == "U1"), None)
    zero_after = next((t for t, kind, wp, lp, lr in events if kind == "call" and wp == 0 and u1ret is not None and t > u1ret), None)
    gap = f"{zero_after - u1ret:.1f}" if zero_after is not None else "-"
    zeros_early = sum(1 for t, kind, wp, lp, lr in events if kind == "call" and wp == 0 and lp == 0xffffffffffffffe7 and t < 2000)
    zeros_late = sum(1 for t, kind, wp, lp, lr in events if kind == "call" and wp == 0 and lp == 0xffffffffffffffe7 and t >= 2000)
    print(f"  ZEROS result={probe.group(3)} hooked-early={first_ms is not None and (us(hooked.group(1)) - t0) / 1000 < first_ms and 'U1' in label.values()} before2s={zeros_early} after2s={zeros_late}")
    print(f"  GAP result={probe.group(3)} first-zero-wparam-after-U1-reply-ms={gap}")
    print(f"  ORDER result={probe.group(3)} hooked-before-first={first_ms is not None and (us(hooked.group(1)) - t0) / 1000 < first_ms} {order}")
    if which == "order":
        continue
    for t, kind, wp, lp, lr in events:
        obj = OBJ.get(lp & 0xffffffff, hex(lp & 0xffffffff))
        ext = "s" if lp >> 32 else "z"
        who = label.get(wp, "0" if wp == 0 else hex(wp))
        print(f"  {t:8.1f} {kind:4} w={who:<6} {obj}/{ext}" + (f" -> {lr}" if lr else ""))
print(counts)
