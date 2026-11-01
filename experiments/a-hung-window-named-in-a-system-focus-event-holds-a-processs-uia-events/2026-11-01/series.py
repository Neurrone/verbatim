"""M4.7 desktop series against a running Verbatim; key times go to series.keys.log."""
import sys, time, re
from keys import *
from keys import _key, fgdesc
log = open(os.path.join(SCRATCH, "series.keys.log"), "a")
def mark(s):
    line = f"{stamp()} MARK {s}"; print(line, flush=True); log.write(line + "\n"); log.flush()
def wait_log(name, pattern, after_size, timeout):
    path = os.path.join(SCRATCH, name + ".log"); end = time.time() + timeout
    while time.time() < end:
        with open(path) as f:
            f.seek(after_size); txt = f.read()
        if re.search(pattern, txt): return True
        time.sleep(0.05)
    return False
def size(name): return os.path.getsize(os.path.join(SCRATCH, name + ".log"))
def alt_tabs(n, label):
    t = stamp(); _key(0x12, False)
    for _ in range(n): _key(0x09, False); _key(0x09, True); time.sleep(0.05)
    _key(0x12, True)
    time.sleep(0.3)
    line = f"{t} KEY alt+tab*{n} {label} fg_after={fgdesc()}"; print(line, flush=True); log.write(line + "\n"); log.flush()
def arrows(n, gap=0.6):
    for _ in range(n): press("down", log=log); time.sleep(gap)
def notepad_home():
    press("esc", log=log, label="reset"); time.sleep(0.3); press("esc", log=log, label="reset"); time.sleep(0.3)
    press("ctrl", "home", log=log, label="reset"); time.sleep(0.8)

def run_hold(i):
    front("notes.txt", "Notepad"); time.sleep(0.5); notepad_home()
    s = size("slow-uia"); mock("slow-uia", "hold", log)
    mark(f"hold run {i}"); front("slow-uia")
    held = wait_log("slow-uia", r"held ", s, 5); mark(f"held seen={held}")
    time.sleep(0.5); alt_tabs(1, "to notepad"); time.sleep(1.5); arrows(4)
    s = size("slow-uia"); mock("slow-uia", "release", log); wait_log("slow-uia", r"applied|released", s, 10); time.sleep(1.5)

def run_hold2(i):
    front("", "CabinetWClass"); time.sleep(0.6)
    front("notes.txt", "Notepad"); time.sleep(0.5); notepad_home()
    s = size("slow-uia"); mock("slow-uia", "hold", log)
    mark(f"hold2 run {i}"); front("slow-uia")
    held = wait_log("slow-uia", r"held ", s, 5); mark(f"held seen={held}")
    time.sleep(0.5); alt_tabs(2, "to explorer"); time.sleep(1.5); arrows(2)
    alt_tabs(1, "to notepad"); time.sleep(1.5); arrows(2)
    s = size("slow-uia"); mock("slow-uia", "release", log); wait_log("slow-uia", r"applied", s, 10); time.sleep(1.5)

def run_busyfocus(i):
    import subprocess
    tv = find("hung-msaa")
    tree = int(subprocess.run([os.path.join(SCRATCH, "fp", "target", "debug", "focusproto.exe"), "findchild", str(tv), "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"], capture_output=True, text=True).stdout)
    front("notes.txt", "Notepad"); time.sleep(0.4); notepad_home()
    front("hung-msaa"); time.sleep(1.2)
    s = size("hung-msaa"); mock("hung-msaa", "stall 12000", log); wait_log("hung-msaa", "stall started", s, 5)
    mark(f"busyfocus run {i}"); time.sleep(0.1)
    subprocess.run([os.path.join(SCRATCH, "fp", "target", "debug", "focusproto.exe"), "winevent", str(tree), "8005", "-4", "0"])
    mark("focus winevent raised for the busy tree view")
    time.sleep(0.3); alt_tabs(1, "to notepad"); time.sleep(0.5); arrows(6, 0.6)
    wait_log("hung-msaa", r"stall ended", s, 20); time.sleep(1.5)

def run_altf(i):
    import subprocess
    fp = os.path.join(SCRATCH, "fp", "target", "debug", "focusproto.exe")
    tv = find("hung-msaa")
    tree = int(subprocess.run([fp, "findchild", str(tv), "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"], capture_output=True, text=True).stdout)
    front("notes.txt", "Notepad"); time.sleep(0.4); notepad_home()
    front("hung-msaa"); time.sleep(1.2)
    s = size("hung-msaa"); mock("hung-msaa", "stall 12000", log); wait_log("hung-msaa", "stall started", s, 5)
    mark(f"altf run {i}"); time.sleep(0.1)
    t0 = time.time()
    subprocess.run([fp, "winevent", str(tree), "8005", "-4", "0"])
    mark("focus winevent raised for the busy tree view")
    def at(sec): time.sleep(max(0, t0 + sec - time.time()))
    at(0.4); alt_tabs(1, "to notepad")
    at(1.0); press("alt", "f", log=log, label="in hold (1.0 s)")
    at(1.8); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    at(2.5); press("alt", "f", log=log, label="in hold (2.5 s)")
    at(3.3); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    at(6.0); press("alt", "f", log=log, label="after hold (6 s, control)")
    at(6.8); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    wait_log("hung-msaa", r"stall ended", s, 20); time.sleep(1.5)

def run_altf_bg(i):
    import subprocess
    fp = os.path.join(SCRATCH, "fp", "target", "debug", "focusproto.exe")
    tv = find("hung-msaa")
    tree = int(subprocess.run([fp, "findchild", str(tv), "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"], capture_output=True, text=True).stdout)
    front("notes.txt", "Notepad"); time.sleep(0.4); notepad_home(); time.sleep(0.5)
    s = size("hung-msaa"); mock("hung-msaa", "stall 12000", log); wait_log("hung-msaa", "stall started", s, 5)
    mark(f"altf_bg run {i}"); time.sleep(0.1)
    t0 = time.time()
    subprocess.run([fp, "winevent", str(tree), "8005", "-4", "0"])
    mark("focus winevent raised for the busy tree view")
    def at(sec): time.sleep(max(0, t0 + sec - time.time()))
    at(1.0); press("alt", "f", log=log, label="in hold (1.0 s)")
    at(1.8); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    at(2.5); press("alt", "f", log=log, label="in hold (2.5 s)")
    at(3.3); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    at(6.0); press("alt", "f", log=log, label="after hold (6 s, control)")
    at(6.8); press("esc", log=log, label="close"); time.sleep(0.25); press("esc", log=log, label="close")
    wait_log("hung-msaa", r"stall ended", s, 20); time.sleep(1.5)

def run_slow(i, ms=1000):
    front("notes.txt", "Notepad"); time.sleep(0.5); notepad_home()
    mock("slow-uia", f"slow {ms}", log); time.sleep(0.2)
    mark(f"slow run {i}"); front("slow-uia"); time.sleep(0.5)
    alt_tabs(1, "to notepad"); time.sleep(1.5); arrows(4)
    s = size("slow-uia"); mock("slow-uia", "slow 0", log)
    ok = wait_log("slow-uia", r"applied", s, 60); mark(f"slow 0 applied={ok}"); time.sleep(1.5)

def run_hung(i):
    front("", "CabinetWClass"); time.sleep(0.6); front("notes.txt", "Notepad"); time.sleep(0.4); notepad_home()
    front("hung-msaa"); time.sleep(1.2)
    s = size("hung-msaa"); mock("hung-msaa", "stall 16000", log); mark(f"hung run {i}")
    time.sleep(6.5); mark(f"hung={u.IsHungAppWindow(find('hung-msaa'))} fg_class={cls(u.GetForegroundWindow())}")
    alt_tabs(1, "to notepad"); time.sleep(1.5); arrows(3)
    alt_tabs(2, "to explorer over hung"); time.sleep(1.5); arrows(2)
    wait_log("hung-msaa", r"stall ended", s, 20); time.sleep(1.5)

def run_background(i):
    front("notes.txt", "Notepad"); time.sleep(0.5); notepad_home()
    s = size("hung-msaa"); mock("hung-msaa", "stall 14000", log); mark(f"background run {i}")
    time.sleep(6.0); arrows(6, 0.8)
    wait_log("hung-msaa", r"stall ended", s, 20); time.sleep(1)

def cls(h):
    c = ctypes.create_unicode_buffer(64); u.GetClassNameW(h, c, 64); return c.value

which = sys.argv[1]; n = int(sys.argv[2])
fn = {"altf_bg": run_altf_bg, "altf": run_altf, "busyfocus": run_busyfocus, "hold": run_hold, "hold2": run_hold2, "slow": run_slow, "hung": run_hung, "background": run_background}[which]
for i in range(1, n + 1): fn(i)
mark(f"done {which}")
