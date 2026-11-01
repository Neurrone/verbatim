"""Poll IsHungAppWindow and the ghost window for a top-level window by title, once a second."""
import ctypes, sys, time, datetime
from ctypes import wintypes
u = ctypes.windll.user32
title = sys.argv[1]; secs = int(sys.argv[2]) if len(sys.argv) > 2 else 30
hwnd = u.FindWindowW(None, title)
print("hwnd", hex(hwnd or 0), flush=True)
ghost = None
def ghosts():
    found = []
    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256); u.GetClassNameW(h, buf, 256)
        if buf.value == "Ghost" and u.IsWindowVisible(h):
            t = ctypes.create_unicode_buffer(256); u.GetWindowTextW(h, t, 256); found.append((hex(h), t.value))
        return True
    u.EnumWindows(cb, 0); return found
for _ in range(secs):
    print(datetime.datetime.now().strftime("%H:%M:%S"), "hung", u.IsHungAppWindow(hwnd), "fg", hex(u.GetForegroundWindow() or 0), "ghosts", ghosts(), flush=True)
    time.sleep(1)
