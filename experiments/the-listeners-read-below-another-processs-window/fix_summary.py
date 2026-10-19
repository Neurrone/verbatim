import re,sys,statistics as st
from collections import defaultdict
g=defaultdict(list)
for f in sys.argv[1:]:
    cur=None
    for l in open(f):
        l=l.rstrip()
        if l.startswith('=== '):
            k=l[4:].rsplit('-',1)[0]; cur={'f':f,'during':0,'after':0,'outpost':None,'chain':None}; g[(f,k)].append(cur)
        elif l.startswith('listener read:'):
            m=re.match(r'listener read: ([\d.]+) ms; calls (.*?); (.*)',l); cur['t']=float(m[1]); cur['calls']=m[2]; cur['chain']=m[3]
        elif l.startswith('outpost window'):
            cur['outpost']=float(re.search(r'took ([\d.]+)',l)[1])
        elif l.startswith('released at'):
            cur['rel']=float(re.search(r'at ([\d.-]+) ms',l)[1])
        elif l.startswith('P WM_GETOBJECT at'):
            t=float(l.split()[3])
            if t>=0:
                if t<=cur['t']: cur['during']+=1
                else: cur['after']+=1
for (f,k),v in g.items():
    ts=[x['t'] for x in v]
    s=f"{f} {k}: n={len(v)} read {min(ts):.1f}-{max(ts):.1f} (median {st.median(ts):.1f}) ms; P msgs during read {sorted(set(x['during'] for x in v))}; after read {sorted(set(x['after'] for x in v))}; chains {set(x['chain'] for x in v)}; calls {set(x['calls'] for x in v)}"
    if any('rel' in x for x in v): s+=f"; released {min(x['rel'] for x in v):.1f}-{max(x['rel'] for x in v):.1f}"
    o=[x['outpost'] for x in v if x['outpost'] is not None]
    if o: s+=f"; outpost window read {min(o):.1f}-{max(o):.1f} (median {st.median(o):.1f}) ms"
    print(s)
