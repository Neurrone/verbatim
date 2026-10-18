"""Condense a --timeline report: each entry whose stacks hold one of the
patterns, with its header and the frames that match."""
import sys
import re

path = sys.argv[1]
patterns = sys.argv[2:] or ['NavigationHelper', 'EvaluateOnCorrectThread', 'EndpointState', 'RemoteOperation', 'CoCreateInstance', 'HasServerSideProvider', 'ChannelBased', 'WM_GETOBJECT', 'SendWmGetObject', 'HandleWMGetObject', 'UiaReturnRawElementProvider']
entry = None


def flush(e):
    if not e:
        return
    hits = [f for f in e[1] if any(p in f for p in patterns)]
    if hits:
        head = re.sub(r'windowsterminal.exe \d+ ', '', e[0])
        print(head)
        seen = []
        for f in hits:
            if f not in seen:
                seen.append(f)
        print('      ' + ' < '.join(seen[:8]))


for line in open(path, encoding='utf-8'):
    if not line.startswith(' '):
        flush(entry)
        entry = [line.rstrip(), []]
        side = 'S:'
    elif line.strip().startswith('where it waited'):
        side = 'W:'
    elif line.strip().startswith('where it was readied'):
        side = 'R:'
    else:
        entry[1].append(side + line.strip())
flush(entry)
