"""Turn a --timeline report into phases: each entry's state is the
outermost interesting frame of the stack it waited on (or was sampled in);
consecutive entries of one state are merged into a span."""
import sys
import re

path = sys.argv[1]
only = sys.argv[2] if len(sys.argv) > 2 else None
KEYS = [
    ('EnsureEndpointStateNotifierSinkRegistered', 'setup: EndpointStateNotifier CoCreateInstance (modal loop)'),
    ('NavigationHelperProxyOneCore::Initialize', 'setup: NavigationHelperProxyOneCore::Initialize'),
    ('RemoteOperation::EvaluateOnCorrectThread', 'remote operation (after setup)'),
    ('HydrateImportedElements', 'remote operation import'),
    ('HandleHookMessage', 'UIA hook message (cross-process call on UI thread)'),
    ('HwndInvoker', 'UIA HwndInvoker'),
    ('WM_GETOBJECT', 'WM_GETOBJECT'),
    ('UiaReturnRawElementProvider', 'UiaReturnRawElementProvider'),
    ('ComInvoker', 'UIA ComInvoker'),
    ('ThreadDispatch', 'COM call dispatch'),
    ('CreateLowBoxChannelBasedClientConnection', 'client: waiting for channel connection'),
    ('ChannelBasedClientConnection', 'client: channel connection'),
    ('UiaHasServerSideProvider', 'client: UiaHasServerSideProvider'),
    ('OverlappedIOManager::IoThreadProc', 'UIA IO thread'),
    ('GetMessageW', 'idle in GetMessage'),
]


def state(frames):
    # The outermost interesting frame wins for setup states, else innermost.
    for key, name in KEYS[:2]:
        if any(key in f for f in frames):
            return name
    for f in frames:
        for key, name in KEYS[2:]:
            if key in f:
                return name
    return 'other: ' + (next((f for f in frames if '!' in f and not f.startswith(('ntdll', 'kernelbase', 'win32u'))), '?'))[:80]


spans = []
entry = None
TIME = re.compile(r'(?:from|at) (\d\d:\d\d:\d\d\.\d+)(?: to (\d\d:\d\d:\d\d\.\d+))?')


def flush(e):
    if not e:
        return
    head, waited, sampled = e
    if only and f' thread {only} ' not in head.split(' blocked')[0].split(' sampled')[0] + ' ':
        return
    m = TIME.search(head)
    if not m:
        return
    start = m.group(1)
    end = m.group(2) or start
    st = state(sampled if 'sampled' in head else waited)
    if spans and spans[-1][2] == st:
        spans[-1][1] = end
        spans[-1][3] += 1
    else:
        spans.append([start, end, st, 1])


side = None
for line in open(path, encoding='utf-8'):
    if not line.startswith(' '):
        flush(entry)
        entry = [line.rstrip(), [], []]
        side = 'S'
    elif line.strip().startswith('where it waited'):
        side = 'W'
    elif line.strip().startswith('where it was readied'):
        side = 'R'
    elif side == 'W':
        entry[1].append(line.strip())
    elif side == 'S':
        entry[2].append(line.strip())
flush(entry)
for start, end, st, n in spans:
    print(f'{start} to {end} ({n}) {st}')
