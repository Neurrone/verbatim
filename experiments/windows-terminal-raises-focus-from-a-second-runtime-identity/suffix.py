import re,sys
from datetime import datetime
A=re.compile(r"\x1b\[[0-9;]*m")
R=re.compile(r"UIA focus event received pid=(\d+) marker=(\d+) kind=Focus runtime_id=\[([^\]]*)\]")
S=re.compile(r"UIA focus sender runtime_id=\[(14,[^\]]*)\] class_name=(\S+) framework=(\S+)")
D=re.compile(r"UIA focus dropped pid=(\d+) marker=(\d+) reason=\"([^\"]*)\"")
t=lambda l: datetime.strptime(l[:26],"%Y-%m-%dT%H:%M:%S.%f").timestamp()
match=miss=0; cls={}; reasons={}
for f in sys.argv[1:]:
    L=[A.sub("",l) for l in open(f,encoding="utf-8",errors="replace")]
    ev=[(t(l),m.group(1),m.group(2),[int(x) for x in m.group(3).split(",")]) for l in L if (m:=R.search(l))]
    mk={}
    for (tt,p,k,r) in ev: mk[(p,k)]=r
    for l in L:
        if m:=S.search(l): cls[(m.group(2),m.group(3))]=cls.get((m.group(2),m.group(3)),0)+1
        if (m:=D.search(l)) and mk.get((m.group(1),m.group(2)),[0])[0]==14: reasons[m.group(3)]=reasons.get(m.group(3),0)+1
    for (tt,p,k,r) in ev:
        if r[0]!=14: continue
        near=[r2 for (t2,p2,k2,r2) in ev if p2==p and r2[0]==42 and len(r2)==4 and abs(t2-tt)<1.0]
        if any(r2[2:]==r[-2:] for r2 in near): match+=1
        else: miss+=1; print("no 42 twin within 1s", f[-30:], r, near)
print("14-id focuses with a 42 twin of equal tail within 1 s:",match,"without:",miss)
print("14 sender class/framework:",cls)
print("drop reasons of 14 focuses:",reasons)
