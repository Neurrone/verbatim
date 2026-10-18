"""Summarises e58's outputs: per trial, for each handler, the last event
lost and the first heard (offsets in ms from the stall's end), any lost
after the first heard, and the slowest delivery; for the removal cases,
the removal's result and how long it took, and the handler's release.
usage: python summarise.py out/*.out"""
import re
import sys

for path in sys.argv[1:]:
    print(path)
    for line in open(path, encoding='utf-8'):
        line = line.strip()
        if not line.startswith('e58 '):
            print('  ', line[:200])
            continue
        out = []
        m = re.search(r'registered (\S+ \d+)', line)
        out.append(f'registered {m.group(1)}')
        m = re.search(r'advises \[([^\]]*)\]', line)
        if m:
            out.append(f'advises [{m.group(1)}]')
        m = re.search(r'first call, sent message or advise after the backlog: ([^;]*)', line)
        if m:
            out.append(f'first activity after backlog {m.group(1)}')
        m = re.search(r'removal at \+(\d+) from the stall\'s end: (\S+) (\d+); handler ([^;]*)', line)
        if m:
            out.append(f'removal at +{m.group(1)}: {m.group(2)} in {m.group(3)} ms; handler {m.group(4)}')
        for label, lost, heard in re.findall(r'(\w): lost \[([^\]]*)\] heard \[([^\]]*)\]', line):
            lost_at = [int(x.split('@')[1]) for x in lost.split()]
            heard_items = [re.match(r'\S+@(\d+)\+(-?\d+)', x) for x in heard.split()]
            heard_at = [int(h.group(1)) for h in heard_items]
            delays = [int(h.group(2)) for h in heard_items]
            first = min(heard_at) if heard_at else None
            after = [x for x in lost_at if first is not None and x > first]
            out.append(f'{label}: {len(lost_at)} lost (last at {max(lost_at) if lost_at else None}), '
                       f'{len(heard_at)} heard (first at {first}, slowest +{max(delays) if delays else None})'
                       + (f', lost after first heard {after}' if after else ''))
        print('   ' + '; '.join(out))
