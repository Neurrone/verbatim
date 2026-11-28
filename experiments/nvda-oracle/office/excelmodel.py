"""A stand-in for the part of Excel's object model nvdaHelperRemote's excel.cpp reaches (Window.Application,
Application.Range(address), Range members, Font, Comment, Validation, EntireRow/EntireColumn, Worksheet,
Hyperlinks, Rows/Columns, _NewEnum). DISPIDs are Excel's, parsed from nvdaHelper/remote/excel/Constants.h."""
import re, ctypes
from comtypes import COMError, COMObject, hresult
from comtypes.automation import IEnumVARIANT, IDispatch, VARIANT
from disp import Disp, parse_constants

C = parse_constants(r"<repository>\nvda\nvdaHelper\remote\excel\Constants.h", "XL")
XLGeneralError = -2146827284  # 0x800A03EC


def mem(cls, kinds):
	return {n: (C[cls][n.upper()], k) for n, k in kinds.items()}


def col_letters(c):
	s = ""
	while c:
		c, r = divmod(c - 1, 26)
		s = chr(65 + r) + s
	return s


def parse_address(a):
	a = a.split("!")[-1].replace("$", "")
	parts = a.split(":")
	def one(p):
		m = re.fullmatch(r"([A-Z]+)(\d+)", p)
		c = 0
		for ch in m.group(1):
			c = c * 26 + ord(ch) - 64
		return int(m.group(2)), c
	r1, c1 = one(parts[0])
	r2, c2 = one(parts[-1])
	return r1, c1, r2, c2


class Sheet:
	def __init__(self, cells, protect=False, colWidth=48):
		self.cells = cells  # {(row, col): {"text":..., "formula":..., "comment":..., "font": {...}, "locked": bool, "validation": (type, title, msg)}}
		self.protect = protect
		self.colWidth = colWidth

	def cell(self, r, c):
		return self.cells.get((r, c), {})


class Enum(COMObject):
	_com_interfaces_ = [IEnumVARIANT]

	def __init__(self, items):
		super().__init__()
		self.items, self.i = items, 0

	def IEnumVARIANT_Next(self, this, celt, rgVar, pCeltFetched):
		n = 0
		while n < celt and self.i < len(self.items):
			rgVar[n].value = self.items[self.i].ptr()
			self.i += 1
			n += 1
		if pCeltFetched:
			pCeltFetched[0] = n
		return hresult.S_OK if n == celt else hresult.S_FALSE


class Font(Disp):
	MEMBERS = mem("FONT", {k: "prop" for k in ["Name", "Size", "Bold", "Italic", "Underline", "Strikethrough"]})

	def __init__(self, f):
		super().__init__()
		self.f = f

	def get_Name(self): return self.f.get("Name", "Calibri")
	def get_Size(self): return float(self.f.get("Size", 11.0))
	def get_Bold(self): return bool(self.f.get("Bold"))
	def get_Italic(self): return bool(self.f.get("Italic"))
	def get_Underline(self): return self.f.get("Underline", -4142)  # xlUnderlineStyleNone
	def get_Strikethrough(self): return bool(self.f.get("Strikethrough"))


class Comment(Disp):
	MEMBERS = mem("COMMENT", {"Text": "method"})

	def __init__(self, t):
		super().__init__()
		self.t = t

	def Text(self, *a):
		return self.t


class Validation(Disp):
	MEMBERS = mem("VALIDATION", {"Type": "prop", "InputTitle": "prop", "InputMessage": "prop"})

	def __init__(self, v):
		super().__init__()
		self.v = v

	def _need(self):
		if not self.v:
			raise COMError(XLGeneralError, "no validation", None)

	def get_Type(self): self._need(); return self.v[0]
	def get_InputTitle(self): self._need(); return self.v[1]
	def get_InputMessage(self): self._need(); return self.v[2]


class Line(Disp):
	"""A row or column (EntireRow / EntireColumn)."""
	def __init__(self, kind):
		super().__init__()
		self.MEMBERS = mem(kind, {"Summary": "prop", "ShowDetail": "prop", "OutlineLevel": "prop"})

	def get_Summary(self): return False
	def get_ShowDetail(self): return True
	def get_OutlineLevel(self): return 1


class Count(Disp):
	def __init__(self, n, cls, items=None):
		super().__init__()
		self.n, self.items = n, items or []
		self.MEMBERS = {"Count": (C[cls]["COUNT"], "prop"), **({"Item": (C[cls]["ITEM"], "method")} if "ITEM" in C[cls] else {})}

	def get_Count(self): return self.n
	def Item(self, i): return self.items[i - 1]


class Worksheet(Disp):
	MEMBERS = mem("WORKSHEET", {"ProtectContents": "prop"})

	def __init__(self, sheet):
		super().__init__()
		self.sheet = sheet

	def get_ProtectContents(self): return self.sheet.protect


class XRange(Disp):
	MEMBERS = mem("RANGE", {k: "prop" for k in ["_NewEnum", "Text", "Formula", "Formula_Local", "Row", "Column", "MergeArea", "Next", "Width",
		"ShrinkToFit", "WrapText", "Hyperlinks", "EntireRow", "EntireColumn", "Rows", "Columns", "Font", "Comment", "Validation",
		"HasFormula", "Locked", "Worksheet"]} | {"Address": "prop", "Item": "method"})

	def __init__(self, sheet, r1, c1, r2=None, c2=None):
		super().__init__()
		self.sheet, self.r1, self.c1 = sheet, r1, c1
		self.r2, self.c2 = r2 or r1, c2 or c1

	def _c(self): return self.sheet.cell(self.r1, self.c1)
	def get__NewEnum(self):
		e = Enum([XRange(self.sheet, r, c) for r in range(self.r1, self.r2 + 1) for c in range(self.c1, self.c2 + 1)])
		return e.QueryInterface(IEnumVARIANT)
	def get_Text(self): return self._c().get("text", "")
	def get_Formula(self): return self._c().get("formula", self._c().get("text", ""))
	get_Formula_Local = get_Formula
	def get_HasFormula(self): return "formula" in self._c()
	def get_Row(self): return self.r1
	def get_Column(self): return self.c1
	def get_MergeArea(self): return XRange(self.sheet, *self._c().get("merge", (self.r1, self.c1, self.r1, self.c1)))
	def get_Next(self): return XRange(self.sheet, self.r1, self.c2 + 1)
	def get_Width(self): return self.sheet.colWidth * (self.c2 - self.c1 + 1)
	def get_ShrinkToFit(self): return False
	def get_WrapText(self): return False
	def get_Hyperlinks(self): return Count(1 if self._c().get("link") else 0, "HYPERLINKS")
	def get_EntireRow(self): return Line("ROW")
	def get_EntireColumn(self): return Line("COLUMN")
	def get_Rows(self): return Count(self.r2 - self.r1 + 1, "ROWS")
	def get_Columns(self):
		return Count(self.c2 - self.c1 + 1, "COLUMNS", [XRange(self.sheet, self.r1, c, self.r2, c) for c in range(self.c1, self.c2 + 1)])
	def get_Font(self): return Font(self._c().get("font", {}))
	def get_Comment(self):
		t = self._c().get("comment")
		return Comment(t) if t else None
	def get_Validation(self): return Validation(self._c().get("validation"))
	def get_Locked(self): return self._c().get("locked", True)
	def get_Worksheet(self): return Worksheet(self.sheet)
	def get_Address(self, rowAbs=True, colAbs=True, style=1, external=False, *rest):
		a = f"${col_letters(self.c1)}${self.r1}"
		if (self.r2, self.c2) != (self.r1, self.c1):
			a += f":${col_letters(self.c2)}${self.r2}"
		return ("[Book1]Sheet1!" if external else "") + a
	def Item(self, r, c=1): return XRange(self.sheet, self.r1 + r - 1, self.c1 + c - 1)


class Application(Disp):
	MEMBERS = mem("APPLICATION", {"Range": "prop"})

	def __init__(self, sheet):
		super().__init__()
		self.sheet = sheet

	def get_Range(self, address, *rest):
		return XRange(self.sheet, *parse_address(address))


class Window(Disp):
	MEMBERS = mem("WINDOW", {"Application": "prop"})

	def __init__(self, sheet):
		super().__init__()
		self.sheet = sheet

	def get_Application(self): return Application(self.sheet)
