import subprocess, time, sys, os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from proto import mockapp, tool
CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*C:\Users\Dickson\Desktop\projects\verbatim\target\symbols*https://msdl.microsoft.com/download/symbols"
FP = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "fp", "target", "debug", "focusproto.exe")
here = os.path.dirname(os.path.abspath(__file__))
B = mockapp("thrB")
out = open(os.path.join(here, "run_name.txt"), "w")
dbg = subprocess.Popen([CDB, "-y", SYMS, "-cf", os.path.join(here, "trace.cdb"), FP, "client", "D", "to=2000", f"name:{B.hwnd}"],
                       stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT, text=True)
time.sleep(15)
out.flush()
with open(os.path.join(here, "run_name.txt"), "a") as f: f.write("\n=== MARK raising name change now\n")
B.send("set-name btn1 Renamed"); B.wait("applied")
time.sleep(5)
with open(os.path.join(here, "run_name.txt"), "a") as f: f.write("\n=== MARK quitting\n")
try: dbg.communicate("quit\n", timeout=20)
except Exception: dbg.kill()
B.stop()
