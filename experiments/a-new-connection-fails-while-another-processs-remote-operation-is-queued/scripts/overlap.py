"""For each run folder given, the timing of the listener's first remote
operation into the application against the outpost's first UIA call into
it, from the debug logs."""
import re
import sys
import glob
import os
from datetime import datetime, timedelta

ANSI = re.compile(r'\x1b\[[0-9;]*m')
TS = re.compile(r'^(\S+Z)')


def t(line):
    m = TS.match(line)
    return datetime.strptime(m.group(1)[:26], '%Y-%m-%dT%H:%M:%S.%f') if m else None


def lines(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        return [ANSI.sub('', l.rstrip()) for l in f]


image = sys.argv[1]
for run in sys.argv[2:]:
    outposts = glob.glob(os.path.join(run, 'outpost-*.log'))
    target = None
    for o in outposts:
        ls = lines(o)
        for l in ls:
            m = re.search(r'watching its application target_pid=(\d+) image="%s"' % image, l)
            if m:
                target = (o, ls, t(l), m.group(1))
                break
        if target:
            break
    if not target:
        for o in glob.glob(os.path.join(run, 'outpost-%s-*.log' % image)):
            ls = lines(o)
            first = next(t(l) for l in ls if t(l))
            target = (o, ls, first, o.rsplit('-', 1)[1][:-4])
    if not target:
        print(os.path.basename(run), 'no outpost for', image)
        continue
    o, ls, given, pid = target
    # The outpost's first UIA call: its first provider probe (start = end - elapsed).
    first_probe = None
    for l in ls:
        m = re.search(r'arbitration probed hwnd=(\d+) class="([^"]*)" is_uia=(\w+).*?elapsed_ms=(\d+)', l)
        if m and t(l) >= given and (m.group(3) == 'true' or int(m.group(4)) >= 2000):
            us = re.search(r'elapsed_us=(\d+)', l)
            el = int(us.group(1)) / 1000 if us else int(m.group(4))
            first_probe = (t(l), el, m.group(2), m.group(3))
            break
    # The listener's first remote operation in the application: from the
    # first sender line naming a window of it to the following ancestors line.
    ll = lines(os.path.join(run, 'listener.log'))
    rop = None
    start = None
    for i, l in enumerate(ll):
        if 'UIA focus sender' in l and t(l) and t(l) >= given - timedelta(seconds=2):
            start = t(l)
        if 'UIA focus ancestors read path="remote"' in l and start:
            rop = (start, t(l))
            break
    def ms(a, b):
        return (a - b).total_seconds() * 1000
    out = [os.path.basename(run)]
    if first_probe:
        end, el, cls, uia = first_probe
        pstart = end.timestamp() - el / 1000
        out.append('probe %s %s %.1f ms, starts %+.1f ms after the outpost was given' % (cls, uia, el, (pstart - given.timestamp()) * 1000))
        if rop:
            out.append('listener rop %.1f ms, from %+.1f to %+.1f ms relative to the probe start' % (
                ms(rop[1], rop[0]), (rop[0].timestamp() - pstart) * 1000, (rop[1].timestamp() - pstart) * 1000))
    print(' | '.join(out))
