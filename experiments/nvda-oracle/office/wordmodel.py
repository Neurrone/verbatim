"""A stand-in for Word's object model (Window, Application, Document, Selection, Range and the objects
nvdaHelperRemote's winword.cpp and NVDA's Python reach from them), over a small document model.

DISPIDs are Word's, from nvdaHelper/remote/WinWord/Constants.h (parsed, not copied). Members Python
reaches by name but the native code never calls get a stand-in DISPID of 5000 and up.

Text units are the stand-in's rules, written to approximate Word's; which of them Word must be
recorded for is listed in the report: words (Word's word includes its trailing spaces, punctuation
is its own word), lines (here: wrap at WIDTH characters after a space), sentences (here: up to and
including the spaces after . ! or ?, or the paragraph mark)."""
import re, os
from comtypes import COMError
from comtypes.automation import IDispatch
from disp import Disp, parse_constants

C = parse_constants(r"<repository>\nvda\nvdaHelper\remote\WinWord\Constants.h", "wd")
wdCharacter, wdWord, wdSentence, wdParagraph, wdLine, wdStory, wdCharacterFormatting = 1, 2, 3, 4, 5, 6, 13
wdUndefined = 9999999
E_NOMEMBER = -2146822347  # 0x800A1735 "The requested member of the collection does not exist"
_extra = [5000]


def members(cls, kinds):
	"""kinds: {Name: kind} with kind 'prop' or 'method'; DISPID from Constants.h class `cls`, else a stand-in one."""
	out = {}
	for name, kind in kinds.items():
		d = None
		for c in cls:
			d = C.get(c, {}).get(name.upper())
			if d is not None:
				break
		if d is None:
			_extra[0] += 1
			d = _extra[0]
		out[name] = (d, kind)
	return out


class Doc:
	def __init__(self, paragraphs, width=40):
		"""paragraphs: [(style name, [(text, font dict), ...]), ...]"""
		self.width = width
		self.text = ""
		self.paras = []  # (start, end, style)
		self.runs = []  # (start, end, font)
		for style, runs in paragraphs:
			s = len(self.text)
			for t, f in runs:
				self.runs.append((len(self.text), len(self.text) + len(t), f))
				self.text += t
			self.runs.append((len(self.text), len(self.text) + 1, runs[-1][1]))
			self.text += "\r"
			self.paras.append((s, len(self.text), style))
		self.N = len(self.text)
		self.selection = None
		self.screenUpdating = True

	# Unit boundaries: a sorted list of unit starts, plus N.
	def bounds(self, unit):
		t = self.text
		if unit == wdCharacter:
			b = list(range(self.N))
		elif unit == wdWord:
			b = [m.start() for m in re.finditer(r"\w+ *|[^\w\s\r] *|\r| +", t)]
		elif unit == wdSentence:
			b = [0]
			for m in re.finditer(r"[.!?]+[\"')\]]* +|\r", t):
				b.append(m.end())
		elif unit == wdParagraph:
			b = [p[0] for p in self.paras]
		elif unit == wdLine:
			b = []
			for s, e, _ in self.paras:
				b.append(s)
				pos = s
				while e - pos > self.width:
					cut = t.rfind(" ", pos, pos + self.width)
					cut = cut + 1 if cut >= pos else pos + self.width
					b.append(cut)
					pos = cut
		elif unit == wdStory:
			b = [0]
		elif unit == wdCharacterFormatting:
			b = []
			last = None
			for s, e, f in self.runs:
				if f != last:
					b.append(s)
				last = f
		else:
			raise COMError(-2147024809, f"unit {unit}", None)
		b = sorted(set(x for x in b if x < self.N))
		return b + [self.N]

	def enclosing(self, unit, pos):
		b = self.bounds(unit)
		pos = min(max(pos, 0), self.N - 1)
		for i in range(len(b) - 1):
			if b[i] <= pos < b[i + 1]:
				return b[i], b[i + 1]
		return b[-2], b[-1]

	def font_at(self, s, e, key, default=0):
		vals = {r[2].get(key, default) for r in self.runs if r[0] < max(e, s + 1) and r[1] > s}
		return vals.pop() if len(vals) == 1 else wdUndefined

	def paras_in(self, s, e):
		return [p for p in self.paras if p[0] < max(e, s + 1) and p[1] > s]

	def line_number(self, pos):
		b = self.bounds(wdLine)
		return sum(1 for x in b[:-1] if x <= pos)


class Coll(Disp):
	"""A Word collection: Count and Item(i), 1-based."""
	def __init__(self, items, countId):
		super().__init__()
		self.items = items
		self.MEMBERS = {"Count": (countId, "prop"), "Item": (0, "method")}

	def get_Count(self):
		return len(self.items)

	def Item(self, i):
		if not 1 <= i <= len(self.items):
			raise COMError(E_NOMEMBER, "no such member", None)
		x = self.items[i - 1]
		return x() if callable(x) else x


class Style(Disp):
	MEMBERS = members(["STYLE"], {"NameLocal": "prop", "Parent": "prop"})

	def __init__(self, doc, name):
		super().__init__()
		self.doc, self.name = doc, name

	def get_NameLocal(self):
		return self.name

	def get_Parent(self):
		return Document(self.doc)


class Document(Disp):
	MEMBERS = members(["DOCUMENT"], {"Range": "method", "Styles": "prop", "Content": "prop"})

	def __init__(self, doc):
		super().__init__()
		self.doc = doc

	def get_Styles(self):
		c = Coll([], 2)
		names = {-1: "Normal", **{-2 - i: f"Heading {i + 1}" for i in range(9)}}

		def item(i):
			if i not in names:
				raise COMError(E_NOMEMBER, "no such style", None)
			return Style(self.doc, names[i])
		c.Item = item
		return c

	def Range(self, s=0, e=None):
		return WRange(self.doc, s, self.doc.N if e is None else e)

	def get_Content(self):
		return WRange(self.doc, 0, self.doc.N)


class Font(Disp):
	MEMBERS = members(["FONT"], {k: "prop" for k in ["Name", "Size", "Bold", "Italic", "Underline", "Strikethrough", "DoubleStrikeThrough", "Hidden", "Color", "Superscript", "Subscript"]})

	def __init__(self, doc, s, e):
		super().__init__()
		self.doc, self.s, self.e = doc, s, e

	def __getattr__(self, name):
		if name.startswith("get_"):
			key = name[4:]
			default = {"Name": "Calibri", "Size": 11.0, "Color": -16777216}.get(key, 0)
			return lambda: self.doc.font_at(self.s, self.e, key, default)
		raise AttributeError(name)


class ParagraphFormat(Disp):
	MEMBERS = members(["PARAGRAPHFORMAT"], {k: "prop" for k in ["Alignment", "RightIndent", "LeftIndent", "FirstLineIndent", "LineSpacing", "LineSpacingRule"]})

	def get_Alignment(self):
		return 0

	def get_RightIndent(self):
		return 0.0

	get_LeftIndent = get_FirstLineIndent = get_RightIndent

	def get_LineSpacing(self):
		return 12.0

	def get_LineSpacingRule(self):
		return 0


class Paragraph(Disp):
	MEMBERS = members(["PARAGRAPH"], {"Range": "prop", "Style": "prop", "OutlineLevel": "prop", "Collapsed_State": "prop"})

	def __init__(self, doc, p):
		super().__init__()
		self.doc, self.p = doc, p

	def get_Range(self):
		return WRange(self.doc, self.p[0], self.p[1])

	def get_Style(self):
		return Style(self.doc, self.p[2])

	def get_OutlineLevel(self):
		m = re.match(r"Heading (\d)", self.p[2])
		return int(m.group(1)) if m else 10

	def get_Collapsed_State(self):
		return False


class ListFormat(Disp):
	MEMBERS = members(["LISTFORMAT"], {"ListString": "prop"})

	def get_ListString(self):
		return ""


class WRange(Disp):
	KINDS = {
		"Start": "prop", "End": "prop", "Text": "prop", "StoryType": "prop", "Application": "prop", "Duplicate": "prop",
		"SetRange": "method", "Collapse": "method", "Move": "method", "MoveEnd": "method", "MoveStart": "method",
		"Expand": "method", "StartOf": "method", "EndOf": "method", "Select": "method", "InRange": "method", "IsEqual": "method",
		"Information": "method", "Font": "prop", "Paragraphs": "prop", "ParagraphFormat": "prop", "Style": "prop",
		"ListFormat": "prop", "LanguageID": "prop", "HighlightColorIndex": "prop", "InlineShapes": "prop",
		"SpellingErrors": "prop", "Fields": "prop", "FormFields": "prop", "ContentControls": "prop", "Tables": "prop",
		"Cells": "prop", "Comments": "prop", "Revisions": "prop", "Footnotes": "prop", "Endnotes": "prop",
		"Hyperlinks": "prop", "Sections": "prop", "PageSetup": "prop", "Document": "prop", "Bookmarks": "prop",
	}
	MEMBERS = members(["RANGE"], KINDS)

	def __init__(self, doc, s, e):
		super().__init__()
		self.doc, self.s, self.e = doc, s, e

	def _set(self, s, e):
		self.s, self.e = min(max(s, 0), self.doc.N), min(max(e, 0), self.doc.N)
		if self.e < self.s:
			self.e = self.s

	def get_Start(self):
		return self.s

	def put_Start(self, v):
		self._set(v, max(v, self.e))

	def get_End(self):
		return self.e

	def put_End(self, v):
		self._set(min(self.s, v), v)

	def get_Text(self):
		return self.doc.text[self.s:self.e]

	def get_StoryType(self):
		return 1

	def get_Application(self):
		return Application(self.doc)

	def get_Document(self):
		return Document(self.doc)

	def get_Duplicate(self):
		return WRange(self.doc, self.s, self.e)

	def SetRange(self, s, e):
		self._set(min(s, e), max(s, e))

	def Collapse(self, direction=1):
		if direction == 1:
			self.e = self.s
		else:
			self.s = self.e

	def _next(self, unit, pos):
		return next((x for x in self.doc.bounds(unit) if x > pos), self.doc.N)

	def _prev(self, unit, pos):
		return max((x for x in self.doc.bounds(unit) if x < pos), default=0)

	def Move(self, unit=wdCharacter, count=1):
		moved = 0
		lastIP = self.doc.N - 1  # the insertion point cannot pass the final paragraph mark
		if count > 0:
			pos = self.e
			if self.e > self.s:
				moved = 1
				count -= 1
			while count > 0 and pos < lastIP:
				pos = min(self._next(unit, pos), lastIP)
				moved += 1
				count -= 1
		elif count < 0:
			pos = self.s
			if self.e > self.s:
				moved = -1
				count += 1
			while count < 0 and pos > 0:
				pos = self._prev(unit, pos)
				moved -= 1
				count += 1
		else:
			return 0
		self.s = self.e = pos
		return moved

	def MoveEnd(self, unit=wdCharacter, count=1):
		moved = 0
		pos = self.e
		while count > 0 and pos < self.doc.N:
			pos = self._next(unit, pos)
			moved += 1
			count -= 1
		while count < 0 and pos > 0:
			pos = self._prev(unit, pos)
			moved -= 1
			count += 1
		self.e = pos
		if self.s > self.e:
			self.s = self.e
		return moved

	def MoveStart(self, unit=wdCharacter, count=1):
		moved = 0
		pos = self.s
		while count > 0 and pos < self.doc.N:
			pos = self._next(unit, pos)
			moved += 1
			count -= 1
		while count < 0 and pos > 0:
			pos = self._prev(unit, pos)
			moved -= 1
			count += 1
		self.s = pos
		if self.e < self.s:
			self.e = self.s
		return moved

	def Expand(self, unit=wdWord):
		s, _ = self.doc.enclosing(unit, self.s)
		_, e = self.doc.enclosing(unit, max(self.e - 1, self.s))
		added = (self.s - s) + (e - self.e)
		self.s, self.e = s, e
		return added

	def StartOf(self, unit=wdWord, extend=0):
		s, _ = self.doc.enclosing(unit, self.s)
		moved = s - self.s
		self.s = s
		if not extend:
			self.e = s
		return moved

	def EndOf(self, unit=wdWord, extend=0):
		_, e = self.doc.enclosing(unit, self.e - 1 if self.e > self.s else self.e)
		moved = e - self.e
		self.e = e
		if not extend:
			self.s = e
		return moved

	def Select(self):
		self.doc.selection._set(self.s, self.e)

	def InRange(self, other):
		return other.s <= self.s and self.e <= other.e

	def IsEqual(self, other):
		return (self.s, self.e) == (other.s, other.e)

	def Information(self, kind):
		return {1: 1, 2: 1, 3: 1, 10: self.doc.line_number(self.s), 12: False, 5: 72.0}.get(kind, 0)

	def get_Font(self):
		return Font(self.doc, self.s, self.e)

	def get_Paragraphs(self):
		return Coll([lambda p=p: Paragraph(self.doc, p) for p in self.doc.paras_in(self.s, self.e)], 2)

	def get_ParagraphFormat(self):
		return ParagraphFormat()

	def get_Style(self):
		ps = self.doc.paras_in(self.s, self.e)
		return Style(self.doc, ps[0][2]) if len(ps) == 1 else None

	def get_ListFormat(self):
		return ListFormat()

	def get_LanguageID(self):
		return 1033

	def get_HighlightColorIndex(self):
		return 0

	def _empty(countId=1):
		return lambda self: Coll([], countId)

	get_InlineShapes = _empty(C["INLINESHAPES"]["COUNT"])
	get_SpellingErrors = _empty(C["SPELLINGERRORS"]["COUNT"])
	get_Fields = _empty(C["FIELDS"]["COUNT"])
	get_FormFields = get_ContentControls = get_Tables = get_Cells = _empty(2)
	get_Comments = _empty(C["COMMENTS"]["COUNT"])
	get_Revisions = _empty(C["REVISIONS"]["COUNT"])
	get_Footnotes = _empty(C["FOOTNOTES"]["COUNT"])
	get_Endnotes = _empty(C["ENDNOTES"]["COUNT"])
	get_Hyperlinks = _empty(C["HYPERLINKS"]["COUNT"])
	get_Bookmarks = _empty(2)


class Selection(WRange):
	MEMBERS = {**WRange.MEMBERS, **members(["SELECTION"], {"Range": "prop", "StartIsActive": "prop", "SetRange": "method", "StartOf": "method", "EndOf": "method"})}

	def __init__(self, doc):
		super().__init__(doc, 0, 0)
		self.startIsActive = False
		doc.selection = self
		self._keep = self.QueryInterface(IDispatch)  # comtypes empties a COMObject at its last release; Word's selection lives on

	def get_Range(self):
		return WRange(self.doc, self.s, self.e)

	def get_StartIsActive(self):
		return self.startIsActive

	def put_StartIsActive(self, v):
		self.startIsActive = bool(v)


class Application(Disp):
	MEMBERS = members(["APPLICATION"], {"ScreenUpdating": "prop", "IsSandbox": "prop", "Selection": "prop", "ActiveDocument": "prop", "Version": "prop", "PixelsToPoints": "method"})

	def __init__(self, doc):
		super().__init__()
		self.doc = doc

	def get_ScreenUpdating(self):
		return self.doc.screenUpdating

	def put_ScreenUpdating(self, v):
		self.doc.screenUpdating = bool(v)
		self.doc.screenUpdatingLog = getattr(self.doc, "screenUpdatingLog", []) + [bool(v)]

	def get_IsSandbox(self):
		return False

	def get_Selection(self):
		return self.doc.selection

	def get_ActiveDocument(self):
		return Document(self.doc)

	def get_Version(self):
		return "16.0"

	def PixelsToPoints(self, v, vertical=False):
		return v * 0.75


class Window(Disp):
	MEMBERS = members(["WINDOW"], {"Application": "prop", "Selection": "prop", "Document": "prop"})

	def __init__(self, doc):
		super().__init__()
		self.doc = doc

	def get_Application(self):
		return Application(self.doc)

	def get_Selection(self):
		return self.doc.selection

	def get_Document(self):
		return Document(self.doc)
