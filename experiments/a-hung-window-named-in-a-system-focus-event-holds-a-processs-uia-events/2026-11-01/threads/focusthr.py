import ctypes, os, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__)); S = os.path.dirname(here); sys.path.insert(0, S)
from proto import client, mockapp, tool
import keys
CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*C:\Users\Dickson\Desktop\projects\verbatim\target\symbols*https://msdl.microsoft.com/download/symbols"
B = mockapp("ftB"); A = mockapp("ftA", fixture="tree_view.json", backend="msaa")
tree = int(tool("findchild", A.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))
D = client("D", "cto=2000,tto=5000,coalesce=1,recovery=1,vbmap", "desk:base", f"name:{B.hwnd}")
outp = os.path.join(here, "focusthr2.txt"); out = open(outp, "w")
dbg = subprocess.Popen([CDB, "-pd", "-p", str(D.pid), "-y", SYMS, "-cf", os.path.join(here, "focusthr.cdb")], stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT, text=True)
for _ in range(240):
    time.sleep(0.5)
    if "ATTACHED" in open(outp, errors="replace").read(): break
keys.front("notes.txt", "Notepad"); time.sleep(0.8)
A.drain(); A.send("stall 9000"); A.wait("stall started"); time.sleep(0.1)
print("winevent", keys.stamp(), flush=True); tool("winevent", tree, "8005", -4, 0); time.sleep(0.2)
print("rename B", keys.stamp(), flush=True); B.send("set-name btn1 During"); B.wait("applied")
time.sleep(0.6); keys.press("alt", "f"); time.sleep(0.8); print("down", keys.stamp(), flush=True); keys.press("down"); time.sleep(0.8); keys.press("esc"); time.sleep(0.25); keys.press("esc")
A.wait("stall ended", timeout=15); time.sleep(1.0)
k = ctypes.windll.kernel32; k.OpenProcess.restype = ctypes.c_void_p
h = k.OpenProcess(0x1F0FFF, False, D.pid)
for attempt in range(3):
    k.DebugBreakProcess(ctypes.c_void_p(h)); time.sleep(1.0)
    if attempt == 0: dbg.stdin.write("bc *\nqd\n"); dbg.stdin.flush()
    try: dbg.wait(timeout=20); print("detached", flush=True); break
    except subprocess.TimeoutExpired: pass
else: print("DEBUGGER DID NOT DETACH", flush=True)
D.stop(); A.stop(); B.stop()
