import subprocess, time, sys, os, ctypes
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from proto import mockapp, tool, client, Proc
CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*C:\Users\Dickson\Desktop\projects\verbatim\target\symbols*https://msdl.microsoft.com/download/symbols"
here = os.path.dirname(os.path.abspath(__file__))
TV = mockapp("focTV", fixture="tree_view.json", backend="msaa")
tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))
D = client("D", "to=2000", "desk:base")
out = open(os.path.join(here, "run_focus2.txt"), "w")
dbg = subprocess.Popen([CDB, "-pd", "-p", str(D.pid), "-y", SYMS, "-cf", os.path.join(here, "focus.cdb")],
                       stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT, text=True)
time.sleep(12)
u = ctypes.windll.user32
print(f"tree hwnd {tree:#x}", flush=True)
TV.send("stall 7000"); TV.wait("stall started")
tool("winevent", tree, "8005", -4, 0)
time.sleep(8)
buf = ctypes.create_unicode_buffer(256)
for line in open(os.path.join(here, "run_focus2.txt"), errors="replace"):
    if "msg=c" in line:
        m = int(line.split("msg=")[1].split()[0], 16)
        u.GetClipboardFormatNameW(m, buf, 256); print("registered", hex(m), "=", buf.value); break
dbg.stdin.write("qd\n"); dbg.stdin.flush()
try: dbg.wait(timeout=20)
except Exception: dbg.kill()
D.stop(); TV.stop()
