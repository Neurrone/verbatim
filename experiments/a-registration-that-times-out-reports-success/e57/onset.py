import re,sys
# summarize each e57 line: first heard event's raise offset, last unheard raise offset, late deliveries
for f in sys.argv[1:]:
    for line in open(f):
        if not line.startswith('e57'): print(f, line.strip()[:200]); continue
        evs=re.findall(r'ev(\d+) raised \+(\d+)( not heard;|((?: heard \+\d+ \(after \d+\);)+))',line)
        reg=re.search(r'registered (\S+) (\d+)',line).groups()
        heard=[(int(o),re.findall(r'after (\d+)',h)) for _,o,n,h in evs if n!=' not heard;']
        lost=[int(o) for _,o,n,h in evs if n==' not heard;']
        first=min([o for o,_ in heard],default=None)
        lastlost=max(lost,default=None)
        late=[ (o,a) for o,a in heard if any(int(x)>50 for x in a)]
        multi=[o for o,a in heard if len(a)>1]
        lostafter=[o for o in lost if first is not None and o>first]
        print(f"{f}: reg={reg[0]} {reg[1]}ms lost={len(lost)} last_lost=+{lastlost} first_heard=+{first} late={late} dup={multi} lost_after_first={lostafter}")
