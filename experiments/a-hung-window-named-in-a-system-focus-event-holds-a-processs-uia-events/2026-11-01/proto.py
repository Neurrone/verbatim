"""Shared helpers for the option (c) prototype experiments (scratch)."""
import ctypes
import os
import queue
import subprocess
import sys
import threading
import time

ROOT = r"C:\Users\Dickson\Desktop\projects\verbatim"
BIN = os.path.join(ROOT, r"target\proto-hold\build\debug")
TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fp", "target", "debug", "focusproto.exe")
MOCKAPP = os.path.join(ROOT, "target", "debug", "mockapp.exe")
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
        flags = 0
        if name == "D" and os.environ.get("H7_NOWINDOW"):
            flags |= 0x08000000  # CREATE_NO_WINDOW, as the supervisor starts the listener
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, text=True, bufsize=1, creationflags=flags)
        if name == "D" and os.environ.get("H7_JOB"):
            k = ctypes.windll.kernel32
            k.CreateJobObjectW.restype = ctypes.c_void_p
            job = k.CreateJobObjectW(None, None)
            class BASIC(ctypes.Structure):
                _fields_ = [("a", ctypes.c_int64), ("b", ctypes.c_int64), ("LimitFlags", ctypes.c_uint32),
                            ("c", ctypes.c_size_t), ("d", ctypes.c_size_t), ("e", ctypes.c_uint32),
                            ("f", ctypes.c_size_t), ("g", ctypes.c_uint32), ("h", ctypes.c_uint32)]
            class EXT(ctypes.Structure):
                _fields_ = [("Basic", BASIC), ("Io", ctypes.c_uint64 * 6), ("ProcessMemoryLimit", ctypes.c_size_t),
                            ("JobMemoryLimit", ctypes.c_size_t), ("PeakProcessMemoryUsed", ctypes.c_size_t),
                            ("PeakJobMemoryUsed", ctypes.c_size_t)]
            info = EXT(); info.Basic.LimitFlags = 0x2000 | 0x100  # KILL_ON_JOB_CLOSE | PROCESS_MEMORY
            info.ProcessMemoryLimit = 200 * 1024 * 1024
            ok1 = k.SetInformationJobObject(ctypes.c_void_p(job), 9, ctypes.byref(info), ctypes.sizeof(info))
            k.OpenProcess.restype = ctypes.c_void_p
            h = k.OpenProcess(0x1F0FFF, False, self.p.pid)
            ok2 = k.AssignProcessToJobObject(ctypes.c_void_p(job), ctypes.c_void_p(h))
            self._job = job
            print(f"job set={ok1} assigned={ok2}", flush=True)
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


DESK = "protohold"


def ondesk(argv):
    """Real-desktop variant: runs argv on the user's own desktop."""
    return list(argv)


def tool(*args):
    out = subprocess.run(ondesk([TOOL, *map(str, args)]), capture_output=True, text=True, timeout=60)
    s = out.stdout.strip()
    log(f"[{now_us()}] tool {' '.join(map(str, args))} -> {s} {out.stderr.strip()}")
    return s


def mockapp(title, fixture="small.json", backend="uia", show=False):
    p = Proc(title, [MOCKAPP, "--fixture", os.path.join(FIXTURES, fixture), "--backend",
                     backend, "--title", title] + (["--show"] if show else []))
    p.pid = p.p.pid
    p.wait("ready")
    hwnd = int(tool("find", title))
    p.hwnd = hwnd
    return p


def client(tag, flags, *specs):
    c = Proc(tag, [TOOL, "client", tag, flags, *specs])
    c.pid = c.p.pid
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


CDB = r"C:\Program Files\WindowsApps\Microsoft.WinDbg_1.2610.1001.0_x64__8wekyb3d8bbwe\amd64\cdb.exe"
SYMS = r"srv*" + os.path.join(ROOT, r"target\symbols") + r"*https://msdl.microsoft.com/download/symbols"


def stacks(pid, label, out_dir, cmds="~*kn 60"):
    """Non-invasive attach: dumps every thread's stack, then detaches (the target resumes)."""
    t0 = now_us()
    r = subprocess.run([CDB, "-pv", "-p", str(pid), "-y", SYMS, "-c", f".lines -d; {cmds}; qd"],
                       capture_output=True, text=True, timeout=600)
    t1 = now_us()
    path = os.path.join(out_dir, f"{label}.stacks.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write(r.stdout)
    log(f"[{t0}] stacks {label} pid={pid} took {(t1 - t0) / 1000:.0f} ms -> {path}")
    return r.stdout
