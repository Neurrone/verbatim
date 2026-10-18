import sys,re
# Summarize a --blocked report: one line per wait with time, thread, and key frames.
path=sys.argv[1]; pid=sys.argv[2] if len(sys.argv)>2 else None
t0=sys.argv[3] if len(sys.argv)>3 else None; t1=sys.argv[4] if len(sys.argv)>4 else None
keys=('uiautomationcore','UIAutomationCore','combase.dll!CoCreate','ModalLoop','Windows.UI.Xaml','RemoteOperation','NavigationHelper','EndpointState','GetMessage','DispatchMessage')
blocks=[];cur=None
for line in open(path,encoding='utf-8',errors='replace'):
    if not line.startswith(' ') and ' blocked ' in line:
        cur=[line.rstrip(),[],[]];blocks.append(cur);sec=1;continue
    if cur is None: continue
    s=line.strip()
    if s.startswith('where it waited'): sec=1;continue
    if s.startswith('where it was readied'): sec=2;continue
    cur[sec].append(s)
for h,w,r in blocks:
    m=re.search(r'from (\S+) to (\S+)',h)
    if pid and f' {pid} thread' not in h: continue
    if t0 and m and m.group(2)<t0: continue
    if t1 and m and m.group(1)>t1: continue
    print(h)
    uw=[f for f in w if '!' in f and not f.startswith(('ntoskrnl','ntdll','kernelbase','win32u','win32k'))]
    print('   W:',' < '.join(uw[:9]))
    ur=[f for f in r if '!' in f and not f.startswith(('ntoskrnl','ntdll','win32k'))]
    print('   R:',' < '.join(ur[:6]))
