"""An in-memory text model with UI Automation's unit and endpoint rules, and the COM stand-ins
(IUIAutomationTextPattern2, IUIAutomationTextRange3, the two arrays) that serve it to NVDA.

The model holds a string, attribute runs and the selection; every range NVDA holds is a pair of
offsets into it, so each answer comes from the model, never from a replayed per-call table.

UIA rules followed (Microsoft's ITextRangeProvider documentation):
- ExpandToEnclosingUnit normalises a range to the unit that holds its start; a start at the end
  of the text takes the last unit.
- Move collapses to the start, moves count unit boundaries (a start inside a unit moving
  backwards counts its own unit's start as one), then a non-degenerate range is expanded to the
  unit it landed on; a degenerate one stays degenerate. It returns the units actually moved.
- MoveEndpointByUnit moves one endpoint over unit boundaries, the text's end being a boundary;
  an endpoint crossing the other drags it along so start never passes end.
- MoveEndpointByRange does the same with a given position.
- CompareEndpoints gives the signed distance in characters.
Provider rules taken from the RichEdit (TOM) family, which Windows 11 Notepad uses, and switchable:
- final_mark: the text ends in a paragraph mark that an insertion point cannot pass, so Move of a
  degenerate range never reaches the end offset (TOM's rule for the story's final CR).
- a word is a run of word characters or of punctuation with its trailing spaces; a line break is
  a word of its own."""
import ctypes
import unicodedata

import comtypes
from comtypes import COMObject, IUnknown, hresult
from comtypes.automation import VARIANT
from comtypes.safearray import _midlSAFEARRAY

from comInterfaces import UIAutomationClient as UIA
import standins as S

CHARACTER, FORMAT, WORD, LINE, PARAGRAPH, PAGE, DOCUMENT = range(7)
UNITNAME = {0: "character", 1: "format", 2: "word", 3: "line", 4: "paragraph", 5: "page", 6: "document"}
ATTRNAME = {v: k[4:-11] for k, v in vars(UIA).items() if k.startswith("UIA_") and k.endswith("AttributeId")}
ATTRID = {n: i for i, n in ATTRNAME.items()}
BREAKS = "\r\n \u000b"


class TextModel:
	def __init__(self, text, runs=None, defaults=None, final_mark=True, caret=0):
		"""runs: list of (start, end, {attrName: value}) over defaults; an attribute absent from
		both is not supported (UIA's reserved not-supported value)."""
		self.text = text
		self.defaults = {ATTRID[k] if isinstance(k, str) else k: v for k, v in (defaults or {}).items()}
		self.runs = [(s, e, {ATTRID[k] if isinstance(k, str) else k: v for k, v in a.items()}) for s, e, a in (runs or [])]
		self.final_mark = final_mark
		self.sel = (caret, caret)
		self.element = None  # the Node whose text this is

	@property
	def N(self):
		return len(self.text)

	# -- attributes --
	def attr_at(self, i, aid):
		v = self.defaults.get(aid, S.NOT_SUPPORTED)
		for s, e, a in self.runs:
			if s <= i < e and aid in a:
				v = a[aid]
		return v

	def attr(self, start, end, aid):
		if start == end:
			return self.attr_at(min(start, max(self.N - 1, 0)), aid)
		vals = {repr(self.attr_at(i, aid)) for i in range(start, end)}
		if len(vals) > 1:
			return S.MIXED
		return self.attr_at(start, aid)

	def format_key(self, i):
		keys = set(self.defaults)
		for _s, _e, a in self.runs:
			keys |= set(a)
		return tuple((k, repr(self.attr_at(i, k))) for k in sorted(keys))

	# -- unit boundaries: the sorted unit starts, 0 first; the text's end closes the last unit --
	def _word_class(self, ch):
		if ch in BREAKS:
			return "break"
		if ch.isspace():
			return "space"
		if ch.isalnum() or ch == "_" or unicodedata.category(ch).startswith("M"):
			return "word"
		return "punct"

	def starts(self, unit):
		t, N = self.text, self.N
		if unit == CHARACTER:
			return list(range(N))
		if unit == DOCUMENT or unit == PAGE:
			return [0] if N else []
		if unit in (LINE, PARAGRAPH):
			out = [0] if N else []
			for i, ch in enumerate(t):
				if ch in BREAKS and i + 1 < N and not (ch == "\r" and t[i + 1] == "\n"):
					out.append(i + 1)
			return out
		if unit == FORMAT:
			out = []
			prev = None
			for i in range(N):
				k = self.format_key(i)
				if k != prev:
					out.append(i)
					prev = k
			return out
		if unit == WORD:
			# A word starts at the text's start, at a line break, after a line break, and where a run
			# of word characters or of punctuation begins; spaces trail the word before them.
			out = []
			prev = None
			for i, ch in enumerate(t):
				c = self._word_class(ch)
				crlf = ch == "\n" and prev == "break" and t[i - 1] == "\r"
				if not crlf and (prev is None or prev == "break" or c == "break" or (c in ("word", "punct") and c != prev)):
					out.append(i)
				prev = c
			return out
		raise ValueError(unit)

	def unit_at(self, unit, pos):
		"""The [start, end) of the unit holding pos; pos at the end takes the last unit."""
		st = self.starts(unit)
		if not st:
			return (0, 0)
		if pos >= self.N:
			pos = self.N - 1
		k = max(i for i, s in enumerate(st) if s <= pos)
		return st[k], (st[k + 1] if k + 1 < len(st) else self.N)

	def reachable_end(self):
		return self.N - 1 if (self.final_mark and self.N) else self.N

	def move(self, start, end, unit, count):
		"""ITextRangeProvider::Move."""
		degenerate = start == end
		st = self.starts(unit)
		limit = self.reachable_end() if degenerate else self.N
		pos, moved = start, 0
		if count > 0:
			while moved < count:
				nxt = [s for s in st if s > pos]
				# the end of the text counts as a boundary for a degenerate range when it is reachable
				cand = nxt[0] if nxt else (self.N if degenerate and self.N > pos else None)
				if cand is None or cand > limit:
					break
				pos, moved = cand, moved + 1
		elif count < 0:
			while moved < -count:
				prev = [s for s in st if s < pos]
				if not prev:
					break
				pos, moved = prev[-1], moved + 1
			moved = -moved
		if degenerate:
			return pos, pos, moved
		if moved == 0:
			return start, end, 0
		s, e = self.unit_at(unit, pos)
		return s, e, moved

	def move_endpoint(self, p, unit, count):
		st = self.starts(unit) + [self.N]
		moved = 0
		if count > 0:
			while moved < count:
				nxt = [s for s in st if s > p]
				if not nxt:
					break
				p, moved = nxt[0], moved + 1
		else:
			while moved < -count:
				prev = [s for s in st if s < p]
				if not prev:
					break
				p, moved = prev[-1], moved + 1
			moved = -moved
		return p, moved

	def expand(self, start, end, unit):
		return self.unit_at(unit, start)

	def lines(self):
		st = self.starts(LINE)
		return [(s, st[i + 1] if i + 1 < len(st) else self.N) for i, s in enumerate(st)]


def _ep(e):
	return "start" if e == UIA.TextPatternRangeEndpoint_Start else "end"


class ElementArray(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationElementArray]

	def __init__(self, elements):
		super().__init__()
		self.elements = elements

	def _get_Length(self):
		S.called("IUIAutomationElementArray.Length")
		return len(self.elements)

	def GetElement(self, index):
		S.called("IUIAutomationElementArray.GetElement")
		return self.elements[index]


class RangeArray(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextRangeArray]

	def __init__(self, ranges):
		super().__init__()
		self.ranges = ranges

	def _get_Length(self):
		S.called("IUIAutomationTextRangeArray.Length")
		return len(self.ranges)

	def GetElement(self, index):
		S.called("IUIAutomationTextRangeArray.GetElement")
		return self.ranges[index]


def make_range(model, start, end):
	return S.register(TextRange(model, start, end)).QueryInterface(UIA.IUIAutomationTextRange)


class TextRange(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextRange3, UIA.IUIAutomationTextRange2, UIA.IUIAutomationTextRange]

	def __init__(self, model, start, end):
		super().__init__()
		self.m, self.start, self.end = model, start, end

	def __repr__(self):
		return f"<range {self.start},{self.end} {self.m.text[self.start:self.end]!r}>"

	def _other(self, p):
		o = S.lookup(p)
		if not isinstance(o, TextRange) or o.m is not self.m:
			S.miss("text range from another text model")
			raise comtypes.ReturnHRESULT(hresult.E_INVALIDARG, "foreign range")
		return o

	def Clone(self):
		S.called("IUIAutomationTextRange.Clone")
		return make_range(self.m, self.start, self.end)

	def Compare(self, range):
		S.called("IUIAutomationTextRange.Compare")
		o = self._other(range)
		return (o.start, o.end) == (self.start, self.end)

	def CompareEndpoints(self, srcEndPoint, range, targetEndPoint):
		S.called("IUIAutomationTextRange.CompareEndpoints")
		o = self._other(range)
		a = self.start if srcEndPoint == UIA.TextPatternRangeEndpoint_Start else self.end
		b = o.start if targetEndPoint == UIA.TextPatternRangeEndpoint_Start else o.end
		return a - b

	def ExpandToEnclosingUnit(self, textUnit):
		S.called(f"IUIAutomationTextRange.ExpandToEnclosingUnit({UNITNAME[textUnit]})")
		self.start, self.end = self.m.expand(self.start, self.end, textUnit)

	def Move(self, unit, count):
		S.called(f"IUIAutomationTextRange.Move({UNITNAME[unit]})")
		self.start, self.end, moved = self.m.move(self.start, self.end, unit, count)
		return moved

	def MoveEndpointByUnit(self, endpoint, unit, count):
		S.called(f"IUIAutomationTextRange.MoveEndpointByUnit({UNITNAME[unit]})")
		if endpoint == UIA.TextPatternRangeEndpoint_Start:
			self.start, moved = self.m.move_endpoint(self.start, unit, count)
			self.end = max(self.end, self.start)
		else:
			self.end, moved = self.m.move_endpoint(self.end, unit, count)
			self.start = min(self.start, self.end)
		return moved

	def MoveEndpointByRange(self, srcEndPoint, range, targetEndPoint):
		S.called("IUIAutomationTextRange.MoveEndpointByRange")
		o = self._other(range)
		p = o.start if targetEndPoint == UIA.TextPatternRangeEndpoint_Start else o.end
		if srcEndPoint == UIA.TextPatternRangeEndpoint_Start:
			self.start = p
			self.end = max(self.end, p)
		else:
			self.end = p
			self.start = min(self.start, p)

	def GetText(self, maxLength):
		S.called("IUIAutomationTextRange.GetText")
		t = self.m.text[self.start:self.end]
		return t if maxLength < 0 else t[:maxLength]

	def GetAttributeValue(self, this, attr, retVal):
		S.called(f"IUIAutomationTextRange.GetAttributeValue({ATTRNAME.get(attr, attr)})")
		S.set_variant(retVal, self.m.attr(self.start, self.end, attr))
		return hresult.S_OK

	def GetAttributeValues(self, this, attributeIds, attributeIdCount, retVal):
		ids = [attributeIds[i] for i in range(attributeIdCount)]
		S.called("IUIAutomationTextRange3.GetAttributeValues(" + ",".join(sorted(ATTRNAME.get(i, str(i)) for i in ids)) + ")")
		vals = []
		for i in ids:
			v = self.m.attr(self.start, self.end, i)
			if v is S.NOT_SUPPORTED:
				v = S.NOT_SUPPORTED_PTR
			elif v is S.MIXED:
				v = S.MIXED_PTR
			vals.append(v)
		retVal[0] = vals  # comtypes builds the SAFEARRAY from the list
		return hresult.S_OK

	def GetBoundingRectangles(self, this, retVal):
		S.called("IUIAutomationTextRange.GetBoundingRectangles")
		rects = []
		for n, (s, e) in enumerate(self.m.lines()):
			a, b = max(s, self.start), min(e, self.end)
			if a < b or (self.start == self.end and s <= self.start < e):
				rects += [10.0 + 8.0 * (a - s), 100.0 + 20.0 * n, 8.0 * max(b - a, 1), 20.0]
		retVal[0] = rects
		return hresult.S_OK

	def _enclosing(self, cacheRequest):
		return S.make_element(self.m.element, S.lookup(cacheRequest) if cacheRequest else None).QueryInterface(
			UIA.IUIAutomationElement,
		)

	def GetEnclosingElement(self):
		S.called("IUIAutomationTextRange.GetEnclosingElement")
		return self._enclosing(None)

	def GetEnclosingElementBuildCache(self, cacheRequest):
		S.called("IUIAutomationTextRange3.GetEnclosingElementBuildCache")
		return self._enclosing(cacheRequest)

	def GetChildren(self):
		S.called("IUIAutomationTextRange.GetChildren")
		return S.register(ElementArray([])).QueryInterface(UIA.IUIAutomationElementArray)

	def GetChildrenBuildCache(self, cacheRequest):
		S.called("IUIAutomationTextRange3.GetChildrenBuildCache")
		return S.register(ElementArray([])).QueryInterface(UIA.IUIAutomationElementArray)

	def Select(self):
		S.called("IUIAutomationTextRange.Select")
		self.m.sel = (self.start, self.end)

	def ScrollIntoView(self, alignToTop):
		S.called("IUIAutomationTextRange.ScrollIntoView")

	def FindText(self, text, backward, ignoreCase):
		S.called("IUIAutomationTextRange.FindText")
		hay = self.m.text[self.start:self.end]
		h, n = (hay.lower(), text.lower()) if ignoreCase else (hay, text)
		i = h.rfind(n) if backward else h.find(n)
		if i < 0:
			return None
		return make_range(self.m, self.start + i, self.start + i + len(text))

	def ShowContextMenu(self):
		S.called("IUIAutomationTextRange2.ShowContextMenu")


class TextPattern(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTextPattern2, UIA.IUIAutomationTextPattern]

	def __init__(self, model, supportedSelection=UIA.SupportedTextSelection_Single):
		super().__init__()
		self.m = model
		self.supportedSelection = supportedSelection

	def _get_DocumentRange(self):
		S.called("IUIAutomationTextPattern.DocumentRange")
		return make_range(self.m, 0, self.m.N)

	def _get_SupportedTextSelection(self):
		S.called("IUIAutomationTextPattern.SupportedTextSelection")
		return self.supportedSelection

	def GetSelection(self):
		S.called("IUIAutomationTextPattern.GetSelection")
		s, e = self.m.sel
		return S.register(RangeArray([make_range(self.m, s, e)])).QueryInterface(UIA.IUIAutomationTextRangeArray)

	def GetVisibleRanges(self):
		S.called("IUIAutomationTextPattern.GetVisibleRanges")
		return S.register(RangeArray([make_range(self.m, 0, self.m.N)])).QueryInterface(UIA.IUIAutomationTextRangeArray)

	def RangeFromChild(self, child):
		S.called("IUIAutomationTextPattern.RangeFromChild")
		node = S.lookup(child).node
		if node is self.m.element:
			S.miss("RangeFromChild with the document itself (UIA: E_INVALIDARG)")
			raise comtypes.ReturnHRESULT(hresult.E_INVALIDARG, "not a child")
		S.miss(f"RangeFromChild({node.key}): no embedded children modelled")
		return None

	def RangeFromPoint(self, pt):
		S.called("IUIAutomationTextPattern.RangeFromPoint")
		S.miss("RangeFromPoint: not modelled")
		raise comtypes.ReturnHRESULT(hresult.E_NOTIMPL, "point")

	def GetCaretRange(self):
		S.called("IUIAutomationTextPattern2.GetCaretRange")
		s, e = self.m.sel
		return True, make_range(self.m, e, e)

	def RangeFromAnnotation(self, annotation):
		S.called("IUIAutomationTextPattern2.RangeFromAnnotation")
		S.miss("RangeFromAnnotation: not modelled")
		raise comtypes.ReturnHRESULT(hresult.E_NOTIMPL, "annotation")
