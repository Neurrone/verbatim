"""Tabulates e39-classify.out: one line per window with its process,
class, visibility, the three verdicts and their times (probe/raw/lookup
ms), and the lookup's main provider. The lookup's verdict is "native"
when a main provider's module is anything but UIAutomationCore.dll (the
tool's own kind= field mislabels nested descriptions and is ignored).
DISAGREE marks a window where the three verdicts differ."""
import re
from collections import Counter
lines = open('e39-classify.out', encoding='utf-8-sig').read().splitlines()
names = dict(re.findall(r"(\d+)=(\w+)", lines[0]))
for e in lines[0].split("explorer=")[1].split()[0].split(','):
    names.setdefault(e, 'explorer')
names.setdefault(lines[0].split("frame=")[1].strip(), 'ApplicationFrameHost')
dis = Counter(); n = 0
for l in lines:
    m = re.search(r"classify hwnd=(\d+) top=(\d+) pid=(\d+) class=\"(.*?)\" vis=(\w+) probe=(\w+) \(([\d.]+)ms\) raw\[sent=(\d+) lresult=(\S+) err=(\d+) lparam=\S+\] \(([\d.]+)ms\) lookup=(\w+) \(([\d.]+)ms\) (.*)", l)
    if not m:
        continue
    n += 1
    hwnd, top, pid, cls, vis, probe, pms, sent, lr, err, rms, _, lms, detail = m.groups()
    mains = re.findall(r"Main(?:\(parent link\))?:([^;\]\[]*?\((?:un)?managed:([^)]+)\))", detail)
    if detail.startswith("err"):
        kind = "error"
    else:
        kind = "native" if any(mod.lower() != "uiautomationcore.dll" for _, mod in mains) else "proxy"
    v = (probe == "true", sent == "1" and lr == "0x10000", kind == "native")
    flag = ""
    if len(set(v)) > 1:
        flag = " DISAGREE"
        dis[(v, lr)] += 1
    print(f"{names.get(pid, pid)} {'top' if hwnd == top else 'child'} {cls} vis={vis} probe={probe} raw={sent}/{lr} lookup={kind} ({pms}/{rms}/{lms} ms) main={'|'.join(x.strip() for x, _ in mains)}{flag}")
print(f"windows {n}; disagreements (probe, raw==0x10000, lookup native), raw lresult: {dict(dis)}")
