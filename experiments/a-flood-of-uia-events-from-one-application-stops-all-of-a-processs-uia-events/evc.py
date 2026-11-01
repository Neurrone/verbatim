"""Shared helpers for the M4.7 event-thread probes (scratch). Hidden windows only, no keys, no foreground."""
import ctypes, os, queue, subprocess, threading, time, re

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = r"C:\Users\Dickson\Desktop\projects\verbatim"
PROBE = os.path.join(HERE, "probe", "target", "release", "evprobe.exe")
MOCKAPP = os.path.join(ROOT, "target", "debug", "mockapp.exe")
FIXTURES = os.path.join(ROOT, r"crates\mockapp\tests\fixtures")
CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*" + os.path.join(ROOT, r"target\symbols") + r"*https://msdl.microsoft.com/download/symbols"
# Verbatim's client settings: connection 2 s, transaction 5 s, coalescing and recovery on.
VERBATIM = "cto=2000,tto=5000,coalesce=1,recovery=1"

_k32 = ctypes.windll.kernel32


def now_us():
    ft = ctypes.c_ulonglong()
    _k32.GetSystemTimePreciseAsFileTime(ctypes.byref(ft))
    return (ft.value - 116444736000000000) // 10


LOG = []
_OUT = [None]


def open_out(name):
    _OUT[0] = open(os.path.join(HERE, name), "a", encoding="utf-8")


def log(line, quiet=False):
    LOG.append(line)
    if not quiet:
        print(line, flush=True)


def res(line):
    log(line)
    if _OUT[0]:
        _OUT[0].write(line + "\n")
        _OUT[0].flush()


class Proc:
    def __init__(self, name, argv, quiet_prefix=None):
        self.name = name
        self.quiet_prefix = quiet_prefix
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.pid = self.p.pid
        self.q = queue.Queue()
        self.lines = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.rstrip("\n")
            rec = (now_us(), line)
            self.q.put(rec)
            self.lines.append(rec)
            log(f"[{rec[0]}] {self.name}: {line}", quiet=bool(self.quiet_prefix and line.startswith(self.quiet_prefix)))

    def send(self, line):
        t = now_us()
        log(f"[{t}] >{self.name}: {line}")
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        return t

    def wait(self, prefix, timeout=20.0, pred=None):
        end = time.time() + timeout
        while True:
            left = end - time.time()
            if left <= 0:
                raise TimeoutError(f"{self.name}: no line starting {prefix!r}")
            try:
                t, line = self.q.get(timeout=left)
            except queue.Empty:
                continue
            if line.startswith(prefix) and (pred is None or pred(line)):
                return t, line

    def drain(self):
        out = []
        while True:
            try:
                out.append(self.q.get_nowait())
            except queue.Empty:
                return out

    def stop(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
        except Exception:
            pass
        try:
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def tool(*args):
    out = subprocess.run([PROBE, *map(str, args)], capture_output=True, text=True, timeout=60)
    s = out.stdout.strip()
    log(f"[{now_us()}] tool {' '.join(map(str, args))} -> {s} {out.stderr.strip()}")
    return s


def mockapp(title, fixture="small.json", backend="uia"):
    p = Proc(title, [MOCKAPP, "--fixture", os.path.join(FIXTURES, fixture), "--backend", backend, "--title", title])
    p.wait("ready")
    p.hwnd = int(tool("find", title))
    assert p.hwnd, f"window {title} not found"
    return p


def client(tag, flags, *specs):
    c = Proc(tag, [PROBE, "client", tag, flags, *specs], quiet_prefix=None)
    c.wait("client-ready")
    return c


def stats(c):
    c.drain()
    c.send("stats")
    _, line = c.wait("stats")
    out = {}
    for part in line.split()[2:]:
        label, rest = part.split("=", 1)
        n, first, last = re.match(r"n(\d+):first(\d+):last(\d+)", rest).groups()
        out[label] = (int(n), int(first), int(last))
    return out


def cpu(c):
    c.send("cpu")
    _, line = c.wait("cpu")
    vals = {}
    for part in line.split()[2:]:
        k, v = part.split("=")
        vals[k] = float(v)
    return vals


def save(path):
    with open(os.path.join(HERE, path), "w", encoding="utf-8") as f:
        f.write("\n".join(LOG) + "\n")


def stacks(pid, label, cmds="~*kn 40"):
    """Non-invasive attach: dumps every thread's stack, then detaches."""
    t0 = now_us()
    r = subprocess.run([CDB, "-pv", "-p", str(pid), "-y", SYMS, "-c", f".lines -d; {cmds}; qd"],
                       capture_output=True, text=True, timeout=600)
    path = os.path.join(HERE, "stacks", f"{label}.txt")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(r.stdout)
    log(f"[{t0}] stacks {label} pid={pid} took {(now_us() - t0) / 1000:.0f} ms -> {path}")
    return r.stdout

