"""Display model: nvdaHelperRemote's GDI hooks (installed in this process only by initInprocManagerThreadIfNeeded)
record text an application thread draws with TextOutW, ExtTextOutW and DrawTextW into a never-shown window of
its own; NVDA's DisplayModelTextInfo, DisplayModelEditableText (caret) and DisplayModelLiveText (text change
notifications through nvdaControllerInternal_displayModelTextChangeNotify) read it."""
import os, sys, ctypes
from ctypes import byref, c_int
from ctypes import wintypes as w

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from helper import *  # noqa: F403
import helper as H
import config, api, textInfos, controlTypes, displayModel, NVDAHelper  # noqa: E401
import speech.speech as sp
from NVDAObjects.window import Window, DisplayModelEditableText, DisplayModelLiveText

gdi = ctypes.WinDLL("gdi32")
gdi.TextOutW.argtypes = [w.HDC, c_int, c_int, w.LPCWSTR, c_int]
gdi.ExtTextOutW.argtypes = [w.HDC, c_int, c_int, w.UINT, ctypes.POINTER(w.RECT), w.LPCWSTR, w.UINT, ctypes.c_void_p]
u32.DrawTextW.argtypes = [w.HDC, w.LPCWSTR, c_int, ctypes.POINTER(w.RECT), w.UINT]
u32.GetDC.restype = w.HDC
u32.GetDC.argtypes = [w.HWND]
u32.ReleaseDC.argtypes = [w.HWND, w.HDC]
u32.FillRect.argtypes = [w.HDC, ctypes.POINTER(w.RECT), w.HBRUSH]
gdi.GetStockObject.restype = w.HGDIOBJ
gdi.SelectObject.argtypes = [w.HDC, w.HGDIOBJ]
gdi.SelectObject.restype = w.HGDIOBJ
gdiCalls = []


@WNDPROC
def proc(h, m, wp, lp):
	return u32.DefWindowProcW(h, m, wp, lp)


def setup():
	assert u32.RegisterClassW(byref(WNDCLASSW(lpfnWndProc=proc, lpszClassName="OracleCanvas")))
	h = u32.CreateWindowExW(0, "OracleCanvas", "Canvas", 0x00CF0000, 200, 150, 500, 300, None, None, None, None)  # never shown
	return dict(top=h)


def draw(lines, caretAt=None):
	"""Clear the client area, then draw each (api, x, y, text) with real GDI on the application thread."""
	def run():
		hdc = u32.GetDC(st["top"])
		gdi.SelectObject(hdc, gdi.GetStockObject(17))  # DEFAULT_GUI_FONT
		r = w.RECT(0, 0, 480, 260)
		gdiCalls.append(("FillRect", u32.FillRect(hdc, byref(r), gdi.GetStockObject(0))))
		for api_, x, y, text in lines:
			if api_ == "TextOutW":
				res = gdi.TextOutW(hdc, x, y, text, len(text))
			elif api_ == "ExtTextOutW":
				rc = w.RECT(x, y, 480, y + 18)
				res = gdi.ExtTextOutW(hdc, x, y, 2, byref(rc), text, len(text), None)  # ETO_OPAQUE
			else:
				rc = w.RECT(x, y, 480, y + 18)
				res = u32.DrawTextW(hdc, text, -1, byref(rc), 0x20)  # DT_SINGLELINE
			gdiCalls.append((api_, text, res))
		u32.ReleaseDC(st["top"], hdc)
		if caretAt is not None:
			u32.CreateCaret(st["top"], None, 1, 14)
			u32.SetCaretPos(*caretAt)
	on_app(run)


start_app(setup)
make_visible([st["top"]])
init_remote()
on_app(lambda: u32.NotifyWinEvent(0x8005, st["top"], -4, 0))  # in-context winEvent: per-thread hooks and RPC registration
pump(0.3)
set_desktop()
displayModel.initialize()
am = appModuleHandler.getAppModuleFromProcessID(os.getpid())
print("appModule:", am.appName, "in-process binding:", bool(am.helperLocalBindingHandle))
SCREEN = [("TextOutW", 4, 4, "File  Edit  View  Help"), ("ExtTextOutW", 4, 40, "Name: Report.txt"),
	("DrawTextW", 4, 60, "Size: 3 KB"), ("TextOutW", 4, 120, "Status: ready")]
# Caret after "Report" on the ExtTextOutW line (client coordinates).
draw(SCREEN)
obj = Window(windowHandle=st["top"])
print("window location (screen):", obj.location)
ti = displayModel.DisplayModelTextInfo(obj, textInfos.POSITION_ALL)
print("DisplayModelTextInfo text:", repr(ti.text))
lines = []
t = displayModel.DisplayModelTextInfo(obj, textInfos.POSITION_FIRST)
for _i in range(8):
	c = t.copy()
	c.expand(textInfos.UNIT_LINE)
	lines.append(c.text)
	if t.move(textInfos.UNIT_LINE, 1) == 0:
		break
print("lines:", lines)
w0 = displayModel.DisplayModelTextInfo(obj, textInfos.POSITION_FIRST)
w0.move(textInfos.UNIT_LINE, 1)
w0.expand(textInfos.UNIT_WORD)
print("first word of line 2:", repr(w0.text), "rect:", w0.boundingRects[:1])
spoken.clear()
li = displayModel.DisplayModelTextInfo(obj, textInfos.POSITION_FIRST)
li.move(textInfos.UNIT_LINE, 2)
li.expand(textInfos.UNIT_LINE)
sp.speakTextInfo(li, unit=textInfos.UNIT_LINE, reason=controlTypes.OutputReason.CARET)
print("speakTextInfo line 3:", spoken)

# Caret: DisplayModelEditableText finds the caret rectangle with GetGUIThreadInfo (displayModel_getCaretRect, in-process).
draw(SCREEN, caretAt=(4 + 0, 40))
ed = DisplayModelEditableText(windowHandle=st["top"])
try:
	c = ed.makeTextInfo(textInfos.POSITION_CARET)
	c.expand(textInfos.UNIT_LINE)
	print("caret line (DisplayModelEditableText):", repr(c.text))
	c = ed.makeTextInfo(textInfos.POSITION_CARET)
	c.expand(textInfos.UNIT_CHARACTER)
	print("caret character:", repr(c.text))
except Exception as e:  # noqa: BLE001
	print("caret read failed:", type(e).__name__, e)
on_app(lambda: u32.DestroyCaret())

# Live text: a new line is drawn; the remote's 50 ms timer reports the changed rectangle to NVDA.
notes = []
orig = NVDAHelper.nvdaControllerInternal_displayModelTextChangeNotify
_tn = displayModel.textChangeNotify
displayModel.textChangeNotify = lambda *a: (notes.append(a), _tn(*a))
live = DisplayModelLiveText(windowHandle=st["top"])
live.initOverlayClass()  # LiveText's, as NVDA's dynamic class construction would run it
live.startMonitoring()
pump(0.5)
spoken.clear()
draw(SCREEN + [("TextOutW", 4, 140, "Copied 3 files")])
pump(1.5, lambda: bool(spoken))
pump(0.3)
print("text change notifications (hwnd, l, t, r, b):", notes[:3], "count", len(notes))
print("live text speech after a new line is drawn:", spoken)
live.stopMonitoring()
print("GDI calls (api, text, result):", gdiCalls[:5])
print("=== patches")
for p in H.PATCHES:
	print("  ", p)
print("=== NVDA log (warnings and above)")
for line in boot.LOGLINES:
	if any(s in line for s in ("espeak", "VoiceSpeechDict", "setSynth", "6364", "Access is denied", "No window handle")):
		continue
	print("  ", line[:600])
on_app(lambda: u32.DestroyWindow(st["top"]))
