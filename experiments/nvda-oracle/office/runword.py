"""Classic Word: nvdaHelperRemote's three winword entry points (getTextInRange, expandToLine, moveByLine), reached
by NVDA's own in-process RPC from this process's NVDA, run against the Word object-model stand-in.
Then NVDA's Python WordDocumentTextInfo over them, if STAGE2 is set."""
import os, sys, ctypes
from ctypes import byref, c_int

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import inproc
from comtypes import BSTR
import disp
import wordmodel as W

B, I, U = {"Bold": -1}, {"Italic": -1}, {"Underline": 1}
P = {}
DOC = W.Doc([
	("Heading 1", [("Quarterly report", P)]),
	("Normal", [("Sales rose by ", P), ("twelve percent", B), (" this quarter. Costs fell. Did margins hold? ", P), ("Yes", I), (", they did.", P)]),
	("Heading 2", [("Outlook", P)]),
	("Normal", [("We expect growth to continue through the next two quarters, as orders remain strong.", P)]),
], width=40)
state = {}


def makeWorld():
	W.Selection(DOC)
	return lambda: W.Window(DOC)


hwnd, am = inproc.start("_WwG", "Microsoft Word Document", "winword", makeWorld)
L = inproc.NVDAHelper.localLib
bh = am.helperLocalBindingHandle


def text_in_range(s, e, flags):
	t = BSTR()
	r = L.nvdaInProcUtils_winword_getTextInRange(bh, hwnd, s, e, flags, byref(t))
	return r, t.value


def expand_to_line(off):
	a, b = c_int(), c_int()
	r = L.nvdaInProcUtils_winword_expandToLine(bh, hwnd, off, byref(a), byref(b))
	return r, a.value, b.value


def move_by_line(off, back):
	n = c_int()
	r = L.nvdaInProcUtils_winword_moveByLine(bh, hwnd, off, back, byref(n))
	return r, n.value


print("document text:", repr(DOC.text))
FONT_ATTRS, HEADINGS, STYLE, PAGE, LINENUM = 0x4, 0x2000, 0x20, 0x80, 0x100
DOC.selection._set(20, 20)
print("getTextInRange para 2, flags 0:", text_in_range(17, 40, 0))
print("getTextInRange para 1 with headings and font attributes:", text_in_range(0, 17, HEADINGS | FONT_ATTRS))
print("getTextInRange para 2 font attributes, style, page, line number:", text_in_range(17, DOC.paras[1][1], FONT_ATTRS | STYLE | PAGE | LINENUM))
print("selection after getTextInRange (unchanged expected):", (DOC.selection.s, DOC.selection.e))
for off in (0, 20, 60, 100):
	print(f"expandToLine({off}):", expand_to_line(off), "line text:", repr(DOC.text[slice(*expand_to_line(off)[1:])]))
print("selection after expandToLine:", (DOC.selection.s, DOC.selection.e), "screenUpdating toggles:", getattr(DOC, "screenUpdatingLog", None))
for off, back in ((20, 0), (60, 0), (60, 1), (0, 1)):
	print(f"moveByLine({off}, back={back}):", move_by_line(off, back))
print("WM_GETOBJECT OBJID_NATIVEOM answered:", inproc.st["nativeom"])
print("=== members called by the native code:", len(disp.CALLS))
for k, v in sorted(disp.CALLS.items()):
	print("  ", k, v)
print("=== misses", dict(disp.MISSES))
print("=== NVDA log")
for line in inproc.boot.LOGLINES[:20]:
	print("  ", line[:600])
sys.stdout.flush()
if not os.environ.get("STAGE2"):
	os._exit(0)

# ---- Stage 2: NVDA's Python Word support (NVDAObjects.window.winword) over the same stand-in.
import textInfos, controlTypes, config, watchdog
import speech.speech as sp
import NVDAObjects.window.winword as ww
spoken = []
sp.speak = lambda seq, symbolLevel=None, priority=None: spoken.append("  ".join(x for x in seq if isinstance(x, str)))
watchdog.cancellableExecute = lambda f, *a, **k: f(*a, **k)
inproc.PATCHES.append("watchdog.cancellableExecute: a direct call (no watchdog thread)")
disp.CALLS.clear()
import NVDAObjects.IAccessible as NIA
obj = NIA.getNVDAObjectFromEvent(hwnd, -4, 0)
print("IAccessible object for the window:", type(obj).__mro__[:3], obj.windowClassName if obj else None)
if not isinstance(obj, ww.WordDocument):
	ww.WordDocument.TextInfo = ww.WordDocumentTextInfo
	obj = ww.WordDocument(windowHandle=hwnd)
	inproc.PATCHES.append("window-level WordDocument given WordDocumentTextInfo (the IAccessible overlay was not chosen)")
print("NVDA object:", type(obj).__name__, "documentWindowHandle == hwnd:", obj.documentWindowHandle == hwnd)


def say(label, info, unit):
	spoken.clear()
	sp.speakTextInfo(info, unit=unit, reason=controlTypes.OutputReason.CARET)
	print(f"{label}: {spoken}")


def caret(pos):
	DOC.selection._set(pos, pos)
	return obj.makeTextInfo(textInfos.POSITION_CARET)


for pos in (0, 20, 60):
	i = caret(pos)
	i.expand(textInfos.UNIT_LINE)
	say(f"line at caret {pos}", i, textInfos.UNIT_LINE)
i = caret(20)
for n in range(4):
	r = i.move(textInfos.UNIT_LINE, 1)
	j = i.copy()
	j.expand(textInfos.UNIT_LINE)
	say(f"next line {n + 1} (moved {r})", j, textInfos.UNIT_LINE)
i = caret(20)
for n in range(4):
	j = i.copy()
	j.expand(textInfos.UNIT_SENTENCE)
	say(f"sentence {n + 1}", j, textInfos.UNIT_SENTENCE)
	r = i.move(textInfos.UNIT_SENTENCE, 1)
i = caret(20)
i.expand(textInfos.UNIT_WORD)
say("word at 20", i, textInfos.UNIT_WORD)
i = caret(0)
i.expand(textInfos.UNIT_PARAGRAPH)
say("paragraph at 0", i, textInfos.UNIT_PARAGRAPH)
print("=== stage 2 members called:", len(disp.CALLS))
for k, v in sorted(disp.CALLS.items()):
	print("  ", k, v)
print("=== misses", dict(disp.MISSES))
print("=== patches", inproc.PATCHES)
print("=== NVDA log (from stage 2)")
for line in inproc.boot.LOGLINES[5:30]:
	print("  ", line[:900])
sys.stdout.flush()
os._exit(0)
