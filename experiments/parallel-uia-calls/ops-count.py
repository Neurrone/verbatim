# Counts ops.ps1's logs per operation: python ops-count.py <client log> <server log> <ops output>
# Client: requests sent (ChannelBasedClientConnection::DoCallWithReturn) by the UIA
# function that sent them, those sent after the operation returned, and the
# replies and events its I/O thread read (ChannelBasedClientConnection::OnData).
# Server: requests read (ChannelBasedServerConnection::OnData), messages
# processed, responses written, release batches, events sent.
import re
import sys
from collections import Counter

cl, sv, out = (open(p, encoding='utf-8', errors='replace').read().splitlines() for p in sys.argv[1:4])

def segments(lines, start):
    segs, cur = [], None
    for l in lines:
        if start(l):
            cur = [l]
            segs.append(cur)
        elif cur is not None:
            cur.append(l)
    return segs

client = segments(cl, lambda l: l.startswith('PUIA_OP '))
server = segments(sv, lambda l: l.strip() == 'FENCE')
results = [l for l in out if l.startswith('op ')]
for i, seg in enumerate(client):
    name = seg[0][8:].lstrip('﻿?')
    senders, after, ended, ondata, waits = Counter(), 0, False, Counter(), Counter()
    j = 0
    while j < len(seg):
        l = seg[j]
        if l.startswith('PUIA_END'):
            ended = True
        if l.strip() == 'CLI_CALL':
            frames = [f for f in seg[j + 1:j + 14] if 'uiautomationcore!' in f.lower() or '!' in f]
            fn = '?'
            for f in frames[4:]:
                m = re.search(r'!(\S+?)(\+0x[0-9a-f]+)?$', f.strip())
                if m and 'DoCallWithReturn' not in m.group(1):
                    fn = m.group(1).replace('LocalUiaNodeProxy::RealCrossProcess_', '').replace('LocalUiaNodeProxy::', '')
                    break
            senders[fn] += 1
            if ended:
                after += 1
        if l.startswith('CLI_WAIT'):
            waits[l.split('=')[1]] += 1
        if l.strip() == 'CLI_ONDATA':
            t = next((x for x in seg[j + 1:j + 4] if 'Start:' in x), '')
            ondata['io' if 'StaticIoThreadProc' in t else t.strip()[:60]] += 1
        j += 1
    s = server[i] if i < len(server) else []
    c = Counter(x.strip() for x in s if x.strip().startswith(('SRV_', 'PROV')))
    prov = Counter(re.sub(r'.*new\d+([A-Za-z]+?)Nt.*', r'\1', x.strip()) for x in s if x.strip().startswith('PROV'))
    r = results[i] if i < len(results) else ''
    hits = r.split('provider', 1)[1].strip() if 'provider' in r else ''
    print(f"{name}: client_requests={sum(senders.values())} ({' '.join(f'{k}={v}' for k, v in senders.items())})"
          f" after_return={after} client_iothread_reads={sum(ondata.values())}{'' if set(ondata) <= {'io'} else ' ' + str(dict(ondata))}"
          f" wait_timeouts={dict(waits)} | server_read={c['SRV_ONDATA']} messages={c['SRV_MSG']} responses={c['SRV_RESP']}"
          f" release_batches={c['SRV_BATCH']} release_dispatches={c['SRV_RELEASE']} events_sent={c['SRV_SENDEVENT']}"
          f" raised={c['SRV_RAISE'] + c['SRV_RAISEPROP']} | provider {hits}{' '.join(f'{k}={v}' for k, v in prov.items())}")
