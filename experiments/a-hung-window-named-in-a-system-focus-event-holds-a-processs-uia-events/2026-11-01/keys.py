"""Shared helpers: real key input with wall-clock stamps, foreground by title, mockapp commands."""
import ctypes, time, datetime, os
from ctypes import wintypes
u = ctypes.windll.user32
SCRATCH = os.path.dirname(os.path.abspath(__file__))
VK = {"f": 0x46, "esc": 0x1B, "alt": 0x12, "tab": 0x09, "down": 0x28, "up": 0x26, "ctrl": 0x11, "shift": 0x10, "home": 0x24}
EXT = {0x28, 0x26, 0x24}
class KI(ctypes.Structure):
    _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t)]
class INP(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("ki", KI), ("pad", ctypes.c_byte * 32)]
    _fields_ = [("type", wintypes.DWORD), ("u", _U)]
def _key(vk, up):
    i = INP(); i.type = 1; i.u.ki.wVk = vk
    i.u.ki.dwFlags = (2 if up else 0) | (1 if vk in EXT else 0)
    u.SendInput(1, ctypes.byref(i), ctypes.sizeof(INP))
def stamp():
    return datetime.datetime.now().strftime("%H:%M:%S.%f")[:-3]
def press(*names, log=None, label=None):
    vks = [VK[n] for n in names]
    t = stamp()
    for v in vks: _key(v, False)
    for v in reversed(vks): _key(v, True)
    line = f"{t} KEY {'+'.join(names)} {label or ''} fg={fgdesc()}"
    print(line, flush=True)
    if log: log.write(line + "\n"); log.flush()
    return t
def find(title_part, cls=None):
    found = []
    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        t = ctypes.create_unicode_buffer(256); u.GetWindowTextW(h, t, 256)
        c = ctypes.create_unicode_buffer(256); u.GetClassNameW(h, c, 256)
        if cls and c.value != cls: return True
        if title_part in t.value and u.IsWindowVisible(h): found.append(h)
        return True
    u.EnumWindows(cb, 0); return found[0] if found else None
def front(title_part, cls=None):
    h = find(title_part, cls)
    for attempt in range(5):
        # Alt held lets SetForegroundWindow through; the unassigned key 0x88 in between stops
        # the Alt release from opening a menu bar.
        _key(0x12, False); _key(0x88, False); _key(0x88, True)
        u.SetForegroundWindow(h)
        _key(0x12, True)
        time.sleep(0.15)
        if u.GetForegroundWindow() == h: break
    else:
        print(f"{stamp()} FRONT FAILED {title_part} fg={fgdesc()}", flush=True)
    time.sleep(0.3)
    return u.GetForegroundWindow() == h
def mock(name, cmd, log=None):
    with open(os.path.join(SCRATCH, name + ".cmd"), "a") as f: f.write(cmd + "\n")
    line = f"{stamp()} CMD {name} {cmd}"
    print(line, flush=True)
    if log: log.write(line + "\n"); log.flush()

def fgdesc():
    h = u.GetForegroundWindow()
    t = ctypes.create_unicode_buffer(256); u.GetWindowTextW(h, t, 256)
    return repr(t.value[:30])
