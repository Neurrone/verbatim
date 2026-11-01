import re, sys, os
def t(h): a,b,c=h.split(":"); return int(a)*3600+int(b)*60+float(c)
q={}; a={}
for l in open(os.environ.get("WATCH","watch2.log"),encoding="utf-8",errors="replace"):
    m=re.match(r"speech (u\d+) \(trace \d+\) queued \S+T(\S+) \(\+(-?\d+) ms after event\): (.*)",l)
    if m: q[m[1]]=(t(m[2]),m[4])
    m=re.match(r"speech (u\d+) audio started \S+T(\S+)",l)
    if m: a[m[1]]=t(m[2])
ev=[(t(l.split()[0])+8*3600, l.split(None,1)[1].strip()) for l in open("lev.txt")]
since=t(sys.argv[1])
lines=[l for l in open("series.keys.log").read().splitlines() if t(l.split()[0])>=since]
for l in lines:
    lt=t(l.split()[0])
    if "focus winevent raised" in l:
        w=lt; nxt=[(et,r) for et,r in ev if et>w][:2]
        print(l[13:], "| listener's next UIA events:", ", ".join(f"+{(et-w)*1000:.0f} ms {r[:22]}" for et,r in nxt))
    elif "MARK" in l: print(l[13:])
    elif "alt+f" in l:
        played=[(a[u],txt) for u,(qt,txt) in q.items() if lt<=qt<lt+5 and u in a]
        played.sort()
        s=f"+{(played[0][0]-lt)*1000:.0f} ms: {played[0][1][:40]}" if played else "SILENT within 5 s"
        print("   ", l[13:52], "-> first audio", s)
