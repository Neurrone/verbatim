"""Shared helpers for the option (c) prototype experiments (scratch)."""
import ctypes
import os
import queue
import subprocess
import sys
import threading
import time

ROOT = r"C:\Users\Dickson\Desktop\projects\verbatim"
BIN = os.path.join(ROOT, r"target\proto-c\debug")
TOOL = os.path.join(BIN, "focusproto.exe")
MOCKAPP = os.path.join(BIN, "mockapp.exe")
FIXTURES = os.path.join(ROOT, r"crates\mockapp\tests\fixtures")

_k32 = ctypes.windll.kernel32
_u32 = ctypes.windll.user32


def now_us():
    ft = ctypes.c_ulonglong()
    _k32.GetSystemTimePreciseAsFileTime(ctypes.byref(ft))
    return (ft.value - 116444736000000000) // 10


LOG = []


def log(line):
    LOG.append(line)
    print(line, flush=True)


class Proc:
    def __init__(self, name, argv):
        self.name = name
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.q = queue.Queue()
        self.lines = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.rstrip("\n")
            rec = (now_us(), line)
            self.lines.append(rec)
            self.q.put(rec)
            log(f"[{rec[0]}] {self.name}: {line}")

    def send(self, line):
        t = now_us()
        log(f"[{t}] >{self.name}: {line}")
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        return t

    def wait(self, prefix, timeout=20.0):
        end = time.time() + timeout
        while True:
            left = end - time.time()
            if left <= 0:
                raise TimeoutError(f"{self.name}: no line starting {prefix!r}")
            try:
                t, line = self.q.get(timeout=left)
            except queue.Empty:
                continue
            if line.startswith(prefix):
                return t, line

    def drain(self):
        while True:
            try:
                self.q.get_nowait()
            except queue.Empty:
                return

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


DESK = "protoc"


def ondesk(argv):
    """Runs argv on the experiments' own desktop, never the user's."""
    return [TOOL, "ondesk", DESK, *argv]


def tool(*args):
    out = subprocess.run(ondesk([TOOL, *map(str, args)]), capture_output=True, text=True, timeout=60)
    s = out.stdout.strip()
    log(f"[{now_us()}] tool {' '.join(map(str, args))} -> {s} {out.stderr.strip()}")
    return s


def mockapp(title, fixture="small.json", backend="uia"):
    p = Proc(title, ondesk([MOCKAPP, "--fixture", os.path.join(FIXTURES, fixture), "--backend",
                            backend, "--title", title]))
    _, pl = p.wait("ondesk-child-pid")
    p.pid = int(pl.split()[1])
    p.wait("ready")
    hwnd = int(tool("find", title))
    p.hwnd = hwnd
    return p


def client(tag, flags, *specs):
    c = Proc(tag, ondesk([TOOL, "client", tag, flags, *specs]))
    _, pl = c.wait("ondesk-child-pid")
    c.pid = int(pl.split()[1])
    c.wait("client-ready")
    return c


WM_HITS_READ = 0x8000 + 2
WM_HITS_RESET = 0x8000 + 3
METHODS = ["WM_GETOBJECT", "ProviderOptions", "GetPatternProvider", "GetPropertyValue",
           "HostRawElementProvider", "Navigate", "GetRuntimeId", "BoundingRectangle",
           "GetEmbeddedFragmentRoots", "SetFocus", "FragmentRoot", "ElementProviderFromPoint"]

_u32.SendMessageW.restype = ctypes.c_ssize_t
_u32.SendMessageW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]


def hits_reset(hwnd):
    tool("send", hwnd, WM_HITS_RESET, 0)


def hits(hwnd, n=13):
    out = {}
    for i in range(n):
        v = int(tool("send", hwnd, WM_HITS_READ, i))
        if v:
            out[METHODS[i] if i < len(METHODS) else f"m{i}"] = v
    return out


def save(path):
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(LOG) + "\n")
