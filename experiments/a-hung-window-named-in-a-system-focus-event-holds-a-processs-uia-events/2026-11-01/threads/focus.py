import subprocess, time, sys, os, ctypes
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from proto import mockapp, tool, now_us
CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*C:\Users\Dickson\Desktop\projects\verbatim\target\symbols*https://msdl.microsoft.com/download/symbols"
here = os.path.dirname(os.path.abspath(__file__))
FP = os.path.join(os.path.dirname(here), "fp", "target", "debug", "focusproto.exe")
TV = mockapp("focTV", fixture="tree_view.json", backend="msaa")
tree = int(tool("findchild", TV.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))
out = open(os.path.join(here, "run_focus.txt"), "w")
dbg = subprocess.Popen([CDB, "-y", SYMS, "-cf", os.path.join(here, "focus.cdb"), FP, "client", "D", "to=2000", "desk:base"],
                       stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT, text=True)
time.sleep(15)
print("tree hwnd", hex(tree), "TV hwnd", hex(TV.hwnd), flush=True)
TV.send("stall 7000"); TV.wait("stall started")
t = tool("winevent", tree, "8005", -4, 0)
print("winevent", t, flush=True)
time.sleep(8)
try: dbg.communicate("quit\n", timeout=20)
except Exception: dbg.kill()
TV.stop()
