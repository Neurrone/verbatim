"""A UIA text pattern stand-in just rich enough for NVDA's caret scripts on a plain edit:
IUIAutomationTextPattern, IUIAutomationTextRangeArray, IUIAutomationTextRange (not TextRange2/3,
so NVDA takes its documented fallbacks), and an empty IUIAutomationElementArray for GetChildren.

The document is the edit node's `doc` (a Doc: text and caret), read live, so a recorded caret move
is a change to the Doc. A stand-in COM call into the application can hang: while the world's
`HANG` interval holds, a call blocks in simulated time until the interval ends or until the
harness's CoCancelCall cancels it, when it fails with RPC_E_CALL_CANCELED, as a
cross-process COM call cancelled by CoCancelCall does."""
import re
import comtypes
from comtypes import COMObject, IUnknown, hresult
from comInterfaces import UIAutomationClient as UIA
import standins as S
import sim

RPC_E_CALL_CANCELED = -2147418110  # 0x80010002
HANG = {"from": None, "until": None}  # the application's UI thread takes no call in [from, until)
CALL = {"member": None, "ident": None, "cancelled": False, "log": []}


def app_call(member):
	"""A call into the application: instant unless the world says the application is hung now."""
	import threading
	S.called(f"IUIAutomationText.{member}")
	until = HANG["until"]
	if until is None or not (HANG["from"] <= sim.SIM.now < until):
		return
	CALL.update(member=member, ident=threading.get_ident(), cancelled=False)
	start = sim.SIM.now
	CALL["log"].append(f"{start:.3f}s {member}: the application is hung until {until} s")
	sim.SIM.block(lambda: CALL["cancelled"], until)
	CALL["ident"] = None
	if CALL["cancelled"]:
		CALL["log"].append(f"{sim.SIM.now:.3f}s {member}: cancelled after {sim.SIM.now - start:.3f} s, fails RPC_E_CALL_CANCELED")
		raise comtypes.ReturnHRESULT(RPC_E_CALL_CANCELED, "call cancelled")
	CALL["log"].append(f"{sim.SIM.now:.3f}s {member}: answered after {sim.SIM.now - start:.3f} s")


class Doc:
	def __init__(self, text, caret=0):
		self.text, self.caret = text, caret

	def boundaries(self, unit):
		n = len(self.text)
		if unit == UIA.TextUnit_Character:
			return list(range(n + 1))
		if unit == UIA.TextUnit_Word:
			b = {m.start() for m in re.finditer(r"\S+\s*|\s+", self.text)}
		elif unit in (UIA.TextUnit_Line, UIA.TextUnit_Paragraph):
			b = {m.end() for m in re.finditer("\n", self.text)}
		else:  # format, page, document: one run
			b = set()
		return sorted(b | {0, n})


class Range(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextRange]

	def __init__(self, pattern, start, end):
		super().__init__()
		self.pattern, self.doc = pattern, pattern.doc
		self.s, self.e = start, end

	def _new(self, s, e):
		return S.register(Range(self.pattern, s, e)).QueryInterface(UIA.IUIAutomationTextRange)

	def Clone(self):
		app_call("Clone")
		return self._new(self.s, self.e)

	def Compare(self, range):
		app_call("Compare")
		o = S.lookup(range)
		return (self.s, self.e) == (o.s, o.e)

	def _ep(self, which):
		return self.s if which == UIA.TextPatternRangeEndpoint_Start else self.e

	def _setEp(self, which, v):
		if which == UIA.TextPatternRangeEndpoint_Start:
			self.s = v
			self.e = max(self.e, v)
		else:
			self.e = v
			self.s = min(self.s, v)

	def CompareEndpoints(self, srcEndPoint, range, targetEndPoint):
		app_call("CompareEndpoints")
		return self._ep(srcEndPoint) - S.lookup(range)._ep(targetEndPoint)

	def _unitAt(self, unit, p):
		b = self.doc.boundaries(unit)
		n = len(self.doc.text)
		if p >= n and n:
			p = n - 1
		start = max(x for x in b if x <= p)
		later = [x for x in b if x > start]
		return start, (later[0] if later else n)

	def ExpandToEnclosingUnit(self, TextUnit):
		app_call("ExpandToEnclosingUnit")
		self.s, self.e = self._unitAt(TextUnit, self.s)

	def _step(self, unit, p, count):
		b = self.doc.boundaries(unit)
		moved = 0
		while moved != count:
			if count > 0:
				nxt = [x for x in b if x > p]
			else:
				nxt = [x for x in b if x < p]
			if not nxt:
				break
			p = nxt[0] if count > 0 else nxt[-1]
			moved += 1 if count > 0 else -1
		return p, moved

	def Move(self, unit, count):
		app_call("Move")
		degenerate = self.s == self.e
		start = self._unitAt(unit, self.s)[0] if not degenerate else self.s
		p, moved = self._step(unit, start, count)
		if degenerate:
			self.s = self.e = p
		else:
			if p >= len(self.doc.text) and count > 0:
				return 0
			self.s, self.e = self._unitAt(unit, p)
		return moved

	def MoveEndpointByUnit(self, endpoint, unit, count):
		app_call("MoveEndpointByUnit")
		p, moved = self._step(unit, self._ep(endpoint), count)
		self._setEp(endpoint, p)
		return moved

	def MoveEndpointByRange(self, srcEndPoint, range, targetEndPoint):
		app_call("MoveEndpointByRange")
		self._setEp(srcEndPoint, S.lookup(range)._ep(targetEndPoint))

	def GetText(self, maxLength):
		app_call("GetText")
		t = self.doc.text[self.s:self.e]
		return t if maxLength < 0 else t[:maxLength]

	def GetAttributeValue(self, this, attr, value):
		app_call("GetAttributeValue")
		value[0].value = S.NOT_SUPPORTED_PTR
		return hresult.S_OK

	def GetEnclosingElement(self):
		app_call("GetEnclosingElement")
		return S.make_element(self.pattern.node, None).QueryInterface(UIA.IUIAutomationElement)

	def GetChildren(self):
		app_call("GetChildren")
		return S.register(ElementArray([])).QueryInterface(UIA.IUIAutomationElementArray)

	def Select(self):
		app_call("Select")
		self.doc.caret = self.s


class ElementArray(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationElementArray]

	def __init__(self, items):
		super().__init__()
		self.items = items

	def _get_Length(self):
		return len(self.items)

	def GetElement(self, index):
		return S.make_element(self.items[index], None).QueryInterface(UIA.IUIAutomationElement)


class RangeArray(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextRangeArray]

	def __init__(self, ranges):
		super().__init__()
		self.ranges = ranges

	def _get_Length(self):
		return len(self.ranges)

	def GetElement(self, index):
		return self.ranges[index].QueryInterface(UIA.IUIAutomationTextRange)


class TextPattern(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextPattern]

	def __init__(self, node, doc):
		super().__init__()
		self.node, self.doc = node, doc

	def GetSelection(self):
		app_call("GetSelection")
		c = self.doc.caret
		r = S.register(Range(self, c, c))
		return S.register(RangeArray([r])).QueryInterface(UIA.IUIAutomationTextRangeArray)

	def _get_DocumentRange(self):
		app_call("DocumentRange")
		return S.register(Range(self, 0, len(self.doc.text))).QueryInterface(UIA.IUIAutomationTextRange)

	def _get_SupportedTextSelection(self):
		return UIA.SupportedTextSelection_Single
