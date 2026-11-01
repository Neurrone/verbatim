"""Which state spares Verbatim's running listener from the focus-tracking hold?

For a target A (mockapp tree view, msaa), hidden and never focused, or shown and focused once under Verbatim
(which also gives it an outpost): A stalls 9 s, a focus WinEvent names its tree view, 200 ms later
- listener: hidden UIA mockapp B raises a notification; hold = its arrival in the running listener's log;
- stand-in: B renames btn1; hold = its arrival in a fresh stand-in client (Verbatim's client settings).
"""
import sys, os, time, re, datetime
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from proto import client, tool, mockapp, now_us
import keys
LOG = r"C:\Users\Dickson\Desktop\projects\verbatim\target\debug\logs\9392\listener.log"
ANSI = re.compile(r"\x1b\[[0-9;]*m")

def log_us(line):
    ts = ANSI.sub("", line).split()[0]
    d = datetime.datetime.strptime(ts[:26], "%Y-%m-%dT%H:%M:%S.%f").replace(tzinfo=datetime.timezone.utc)
    return int(d.timestamp() * 1_000_000)

def wait_listener_notification(after_us, timeout):
    end = time.time() + timeout
    while time.time() < end:
        with open(LOG, encoding="utf-8", errors="replace") as f:
            for line in f:
                if 'event="notification"' in line and "UIA event received" in line:
                    t = log_us(line)
                    if t > after_us:
                        return t
        time.sleep(0.1)
    return None

def tree_of(A):
    return int(tool("findchild", A.hwnd, "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"))

def listener_hold(A, B, tree, label):
    raised = B.send("notify base"); B.wait("applied")
    t = wait_listener_notification(raised, 5); base = (t - raised) / 1000 if t else None
    A.drain(); A.send("stall 9000"); A.wait("stall started"); time.sleep(0.1)
    tool("winevent", tree, "8005", -4, 0); time.sleep(0.2)
    raised = B.send("notify during"); B.wait("applied")
    t = wait_listener_notification(raised, 12)
    print(f"RESULT {label} listener: baseline {base} ms, during the stall {((t - raised) / 1000) if t else 'never'} ms", flush=True)
    A.wait("stall ended", timeout=15); time.sleep(1.5)

def standin_hold(A, B, tree, label):
    D = client("D", "cto=2000,tto=5000,coalesce=1,recovery=1,vbmap", "desk:base", f"name:{B.hwnd}")
    time.sleep(0.8)
    D.drain(); raised = B.send("set-name btn1 Base"); B.wait("applied"); t, _ = D.wait("ev D name"); base = (t - raised) / 1000
    A.drain(); A.send("stall 9000"); A.wait("stall started"); time.sleep(0.1)
    tool("winevent", tree, "8005", -4, 0); time.sleep(0.2)
    D.drain(); raised = B.send("set-name btn1 During"); B.wait("applied")
    try:
        t, _ = D.wait("ev D name", timeout=15); held = f"{(t - raised) / 1000:.0f}"
    except TimeoutError:
        held = "never"
    print(f"RESULT {label} stand-in: baseline {base:.1f} ms, during the stall {held} ms", flush=True)
    D.stop(); A.wait("stall ended", timeout=15); time.sleep(1.5)

def main():
    global B, A, which, tree
    B = mockapp("stB", show=True)
    time.sleep(1.0); keys.front("notes.txt", "Notepad"); time.sleep(1.5)
    which = sys.argv[1]
    class Ext:
        """The already-running hung-msaa, driven through its controller's command file."""
        def __init__(self, title):
            self.hwnd = keys.find(title); self.title = title
        def drain(self): pass
        def send(self, cmd):
            self._size = os.path.getsize(os.path.join(keys.SCRATCH, self.title + ".log")); keys.mock(self.title, cmd)
        def wait(self, text, timeout=15):
            end = time.time() + timeout
            while time.time() < end:
                with open(os.path.join(keys.SCRATCH, self.title + ".log")) as f:
                    f.seek(self._size)
                    if text in f.read(): return
                time.sleep(0.05)
            raise TimeoutError(text)
        def stop(self): pass
    if which == "hungmsaa":
        A = Ext("hung-msaa")
    elif which == "hidden":
        A = mockapp("stAh", fixture="tree_view.json", backend="msaa")
    else:
        A = mockapp("stAs", fixture="tree_view.json", backend="msaa", show=True)
        time.sleep(0.5); keys.front("stAs"); time.sleep(2.5)  # focused under Verbatim: read, spoken, outpost started
        keys.front("notes.txt", "Notepad"); time.sleep(1.5)
    tree = tree_of(A)
    for i in range(3):
        listener_hold(A, B, tree, which)
    for i in range(0 if which == "hungmsaa" else 3):
        standin_hold(A, B, tree, which)
    A.stop(); B.stop()

if __name__ == '__main__':
    main()
