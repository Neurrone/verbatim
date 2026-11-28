"""Word's UIA remote-operation extensions, implemented in NVDA's LocalExecutor.

NVDA's own UIAHandler.remote.msWord_* functions build their bytecode unchanged; IsExtensionSupported and
CallExtension, which NVDA's LocalExecutor leaves unimplemented, are given local bodies here that call a
stand-in of Word's provider-side extension objects over the ../text UIA text-range stand-ins, extended with
a sentence unit following the stand-in's Word sentence rules. Then NVDA's WordDocumentTextInfo sentence
methods (NVDAObjects/UIA/wordDocument.py) run over them, and the caret-move-by-sentence sequence."""
import os, sys, re

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "uiacopy"))
import boot  # noqa: F401
import standins as S
import textmodel as T
from comtypes import GUID
from comInterfaces import UIAutomationClient as UIA
import UIAHandler.remote as remote
from UIAHandler._remoteOps import operation, instructions
from UIAHandler._remoteOps.localExecute import LocalExecutor

SENTENCE = "sentence"
G_PATTERN = GUID("{93514122-FF04-4B2C-A4AD-4AB04587C129}")
G_ATTR = GUID("{081ACA91-32F2-46F0-9FB9-017038BC45F8}")
G_MOVE = GUID("{F39655AC-133A-435B-A318-C197F0D3D203}")
G_MOVE_EP = GUID("{368E89A2-1BC2-4402-8C58-33C63ECFFA3B}")
G_EXPAND = GUID("{98FE8B34-F317-459A-9627-21123EA95BEA}")
EXT_CALLS = []


class WordTextModel(T.TextModel):
	"""Adds Word's sentence unit (stand-in rule: a sentence ends after . ! or ? and any closing quotes or
	brackets, taking the spaces after it; a paragraph mark ends one too)."""

	def starts(self, unit):
		if unit != SENTENCE:
			return super().starts(unit)
		out = [0] if self.N else []
		for m in re.finditer(r"[.!?]+[\"')\]”]* +|\r", self.text):
			if m.end() < self.N:
				out.append(m.end())
		return sorted(set(out))


def rng(p):
	return S.lookup(p)


class ExtendedTextRangePattern:
	"""Stands for the object Word's provider returns for the extendedTextRangePattern extension."""

	supported = {G_ATTR, G_MOVE, G_MOVE_EP, G_EXPAND}

	def call(self, guid, args, regs):
		if guid == G_MOVE:
			r, count, out = args
			o = rng(regs[r.operandId])
			s, e, moved = o.m.move(o.start, o.end, SENTENCE, regs[count.operandId])
			regs[r.operandId] = T.make_range(o.m, s, e)
			regs[out.operandId] = moved
		elif guid == G_MOVE_EP:
			r, ep, count, out = args
			o = rng(regs[r.operandId])
			s, e = o.start, o.end
			if regs[ep.operandId] == UIA.TextPatternRangeEndpoint_Start:
				s, moved = o.m.move_endpoint(s, SENTENCE, regs[count.operandId])
				e = max(e, s)
			else:
				e, moved = o.m.move_endpoint(e, SENTENCE, regs[count.operandId])
				s = min(s, e)
			regs[r.operandId] = T.make_range(o.m, s, e)
			regs[out.operandId] = moved
		elif guid == G_EXPAND:
			(r,) = args
			o = rng(regs[r.operandId])
			pos = o.start if o.start < o.m.N else 0  # Word wraps to the first sentence at the very end (NVDA's comment)
			s, e = o.m.unit_at(SENTENCE, pos)
			regs[r.operandId] = T.make_range(o.m, s, e)
		elif guid == G_ATTR:
			r, attrId, out = args
			regs[out.operandId] = None
		EXT_CALLS.append(str(guid))


def target_of(v):
	if isinstance(v, ExtendedTextRangePattern):
		return v
	o = S.lookup(v)
	return o


def isExtSupported(self, registers):
	t = target_of(registers[self.target.operandId])
	g = registers[self.extensionId.operandId]
	if isinstance(t, ExtendedTextRangePattern):
		registers[self.result.operandId] = g in t.supported
	else:  # the document element
		registers[self.result.operandId] = g == G_PATTERN and getattr(t, "node", None) is not None and t.node.key == "wordDoc"
	EXT_CALLS.append(f"IsExtensionSupported {g} -> {registers[self.result.operandId]}")


def callExt(self, registers):
	t = target_of(registers[self.target.operandId])
	g = registers[self.extensionId.operandId]
	if isinstance(t, ExtendedTextRangePattern):
		t.call(g, self.arguments, registers)
	elif g == G_PATTERN:
		(out,) = self.arguments
		registers[out.operandId] = ExtendedTextRangePattern()
		EXT_CALLS.append("CallExtension extendedTextRangePattern")


instructions.IsExtensionSupported.localExecute = isExtSupported
instructions.CallExtension.localExecute = callExt
operation.Operation._executorClass = LocalExecutor
remote._isSupported = True
PATCHES = [
	"instructions.IsExtensionSupported/CallExtension.localExecute: local bodies calling the Word extension stand-in",
	"operation.Operation._executorClass = LocalExecutor",
	"UIAHandler.remote._isSupported = True (initialize would set it on Windows 11)",
]

text = "Mr. Smith arrived at 3 p.m. on Monday. \"Is it done?\" he asked. Yes!\rNext paragraph here. Last one\r"
model = WordTextModel(text, final_mark=True)
doc = S.Node("wordDoc", {"ClassName": "_WwG", "ControlType": UIA.UIA_DocumentControlTypeId})
docEl = S.make_element(doc, None).QueryInterface(UIA.IUIAutomationElement)


def show(r):
	o = rng(r)
	return (o.start, o.end, o.m.text[o.start:o.end])


print("sentence starts (stand-in rule):", model.starts(SENTENCE))
caret = T.make_range(model, 0, 0)
print("moveBySentence(caret 0, +1):", (lambda x: (show(x[0]), x[1]))(remote.msWord_textRange_moveBySentence(docEl, caret, 1)))
print("caret range after (unchanged expected):", show(caret))
print("moveBySentence(caret 0, +3):", (lambda x: (show(x[0]), x[1]))(remote.msWord_textRange_moveBySentence(docEl, caret, 3)))
print("moveBySentence(caret 0, -1):", (lambda x: (show(x[0]), x[1]))(remote.msWord_textRange_moveBySentence(docEl, caret, -1)))
print("expandToEnclosingSentence(caret 45):", show(remote.msWord_textRange_expandToEnclosingSentence(docEl, T.make_range(model, 45, 45))))
print("expandToEnclosingSentence(caret at end, guarded by NVDA):", show(remote.msWord_textRange_expandToEnclosingSentence(docEl, T.make_range(model, model.N, model.N))))
print("moveEndpointBySentence(end, +1) on range 0..0:", (lambda x: (show(x[0]), x[1]))(remote.msWord_textRange_moveEndpointBySentence(docEl, T.make_range(model, 0, 0), UIA.TextPatternRangeEndpoint_End, 1)))
other = S.make_element(S.Node("notWord", {"ClassName": "Edit"}), None).QueryInterface(UIA.IUIAutomationElement)
print("moveBySentence on an element without the extension:", remote.msWord_textRange_moveBySentence(other, caret, 1))

# NVDA's WordDocumentTextInfo sentence methods, on an object holding only what they read.
from NVDAObjects.UIA.wordDocument import WordDocumentTextInfo as WTI


class Holder:
	pass


def ti(s, e):
	h = Holder()
	h.obj = Holder()
	h.obj.UIAElement = docEl
	h._rangeObj = T.make_range(model, s, e)
	return h


print("=== NVDA's caret-move-by-sentence sequence (move, then expand; the expanded text is what is spoken)")
pos = 0
for n in range(6):
	h = ti(pos, pos)
	moved = WTI._moveBySentenceRemote(h, 1)
	pos = rng(h._rangeObj).start
	WTI._expandToSentenceRemote(h)
	print(f"  next sentence {n + 1}: moved {moved}, caret {pos}, speaks {show(h._rangeObj)[2]!r}")
for n in range(3):
	h = ti(pos, pos)
	moved = WTI._moveBySentenceRemote(h, -1)
	pos = rng(h._rangeObj).start
	WTI._expandToSentenceRemote(h)
	print(f"  previous sentence {n + 1}: moved {moved}, caret {pos}, speaks {show(h._rangeObj)[2]!r}")
print("=== extension calls:", len(EXT_CALLS))
print("=== patches:", PATCHES)
print("=== misses:", S.MISSES[:10])
print("=== log:", [l[:300] for l in boot.LOGLINES[4:12]])
sys.stdout.flush()
os._exit(0)
