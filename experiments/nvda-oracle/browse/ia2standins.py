"""In-process IAccessible2 stand-ins, shaped like Chromium's tree, for NVDA's gecko_ia2 vbuf backend.

Every node is a comtypes COMObject. Methods are low-level (take `this`, write out-pointers raw)
so their HRESULTs match what a browser returns (S_FALSE plus NULL for a missing name)."""
import os, sys, ctypes
from ctypes import POINTER, c_void_p, c_long, c_int, cast
from ctypes import wintypes as w

HERE = os.path.dirname(os.path.abspath(__file__))
GEN = os.path.join(HERE, "gen")
sys.path.insert(0, GEN)
import comtypes, comtypes.gen  # noqa: E401
comtypes.gen.__path__.insert(0, os.path.join(GEN, "comInterfaces"))
import comtypes.client
from comtypes import COMObject, GUID, IUnknown, COMMETHOD, HRESULT
from comtypes.automation import IDispatch

Acc = comtypes.client.GetModule("oleacc.dll")
import comInterfaces.IAccessible2Lib as IA2  # NVDA's own generated module, from its library.zip

S_OK, S_FALSE, E_INVALIDARG, E_NOTIMPL = 0, 1, -2147024809, -2147467263
_SysAllocString = ctypes.windll.oleaut32.SysAllocString
_SysAllocString.restype = c_void_p
_SysAllocString.argtypes = [w.LPCWSTR]
_CoTaskMemAlloc = ctypes.windll.ole32.CoTaskMemAlloc
_CoTaskMemAlloc.restype = c_void_p
_CoTaskMemAlloc.argtypes = [ctypes.c_size_t]

CALLS = {}  # method name -> count, to report the surface the backend touches


def _count(name):
	CALLS[name] = CALLS.get(name, 0) + 1


class IServiceProvider(IUnknown):
	_iid_ = GUID("{6D5140C1-7436-11CE-8034-00AA006009FA}")
	_methods_ = [COMMETHOD([], HRESULT, "QueryService",
		(["in"], POINTER(GUID), "guidService"), (["in"], POINTER(GUID), "riid"),
		(["out"], POINTER(c_void_p), "ppvObject"))]


def put_bstr(pp, s):
	if s is None:
		cast(pp, POINTER(c_void_p))[0] = None
		return S_FALSE
	cast(pp, POINTER(c_void_p))[0] = _SysAllocString(s)
	return S_OK


def put_long(p, v):
	cast(p, POINTER(c_long))[0] = v
	return S_OK


def raw_itf(com, itf):
	"""An AddRef'd raw pointer to `itf` on a stand-in, owned by the caller."""
	p = com.QueryInterface(itf)
	p.AddRef()
	return cast(p, c_void_p).value


def put_itf(pp, node, itf):
	if node is None:
		cast(pp, POINTER(c_void_p))[0] = None
		return S_FALSE
	cast(pp, POINTER(c_void_p))[0] = raw_itf(node.com, itf)
	return S_OK


ROLE_SYSTEM_GROUPING = 0x14
EMBED = "￼"


class World:
	def __init__(self, hwnd):
		self.hwnd = hwnd
		self.byId = {}
		self.nextId = -1
		self.root = None


WORLD = None


class Node:
	"""content: list of str and Node (hypertext), or None for a node without IAccessibleText."""

	def __init__(self, role, name=None, states=0, ia2states=0, attrs=None, content=None, children=None,
			value=None, desc=None, actions=(), msaaRole=None, labelledBy=None, table=None, cell=None):
		self.role = role
		self.msaaRole = msaaRole if msaaRole is not None else (role if role < 0x400 else ROLE_SYSTEM_GROUPING)
		self.name, self.value, self.desc = name, value, desc
		self.states, self.ia2states = states, ia2states
		self.attrs = dict(attrs or {})
		self.content = content
		self.childList = children
		self.actions = list(actions)
		self.labelledBy = labelledBy
		self.table = table  # (rows, cols) for a table
		self.cell = cell  # (row, col, tableNode) for a cell
		self.parent = None
		self.uid = None
		self.com = None

	@property
	def children(self):
		if self.content is not None:
			return [c for c in self.content if isinstance(c, Node)]
		return list(self.childList or [])

	@property
	def text(self):
		if self.content is None:
			return None
		return "".join(EMBED if isinstance(c, Node) else c for c in self.content)

	def embedOffset(self, child):
		off = 0
		for c in self.parent.content:
			if c is child:
				return off
			off += 1 if isinstance(c, Node) else len(c)
		raise LookupError

	def attach(self, world, parent=None):
		self.parent = parent
		self.uid = world.nextId
		world.nextId -= 1
		world.byId[self.uid] = self
		itfs = [IA2.IAccessible2_2, IA2.IAccessibleHyperlink, IServiceProvider, IA2.IAccessibleApplication]
		if self.content is not None:
			itfs.insert(1, IA2.IAccessibleHypertext)
		if self.table:
			itfs.append(IA2.IAccessibleTable2)
		if self.cell:
			itfs.append(IA2.IAccessibleTableCell)
		key = tuple(i.__name__ for i in itfs)
		cls = _CLASSES.get(key)
		if cls is None:
			cls = _CLASSES[key] = type("NodeCOM_" + "_".join(k[11:] or k for k in key), (NodeCOM,), {"_com_interfaces_": itfs})
		self.com = cls(self, world)
		# The world owns its objects, as a browser does: comtypes tears a COMObject's vtables down when its
		# refcount reaches zero, so without this hold the first client Release would kill the node.
		self._hold = self.com.QueryInterface(IUnknown)
		for c in self.children:
			c.attach(world, self)


_CLASSES = {}
_SERVICES = {str(Acc.IAccessible._iid_), str(IA2.IAccessible2._iid_), str(IA2.IAccessibleApplication._iid_)}


def _esc(s):
	return s.replace("\\", "\\\\").replace(":", "\\:").replace(";", "\\;").replace(",", "\\,").replace("=", "\\=")


class NodeCOM(COMObject):
	def __init__(self, node, world):
		super().__init__()
		self.n = node
		self.w = world

	# IAccessible
	def _get_accParent(self, this, pp):
		_count("accParent")
		if self.n.parent is None:
			# As Chromium: the document's parent is the system's window object for its HWND (OBJID_WINDOW),
			# which carries IOleWindow, so oleacc can map any descendant back to the window.
			p = c_void_p()
			hr = ctypes.oledll.oleacc.AccessibleObjectFromWindow(c_void_p(self.w.hwnd), ctypes.c_long(0), ctypes.byref(IDispatch._iid_), ctypes.byref(p))
			cast(pp, POINTER(c_void_p))[0] = p.value
			return S_OK if p.value else S_FALSE
		return put_itf(pp, self.n.parent, IDispatch)

	def _get_accChildCount(self, this, p):
		_count("accChildCount")
		return put_long(p, len(self.n.children))

	def _get_accChild(self, this, varChild, pp):
		_count("accChild")
		cid = varChild.value
		if cid == 0:
			target = self.n
		elif cid > 0:
			kids = self.n.children
			target = kids[cid - 1] if cid <= len(kids) else None
		else:
			target = self.w.byId.get(cid)
		if os.environ.get("IA2_TRACE"):
			print("accChild", self.n.uid, varChild.vt, cid, "->", target and target.uid, flush=True)
		if target is None:
			cast(pp, POINTER(c_void_p))[0] = None
			return E_INVALIDARG
		return put_itf(pp, target, IDispatch)

	def _get_accName(self, this, varChild, p):
		_count("accName")
		return put_bstr(p, self.n.name)

	def _get_accValue(self, this, varChild, p):
		_count("accValue")
		return put_bstr(p, self.n.value)

	def _get_accDescription(self, this, varChild, p):
		_count("accDescription")
		return put_bstr(p, self.n.desc)

	def _get_accRole(self, this, varChild, pv):
		_count("accRole")
		pv[0].value = self.n.msaaRole
		return S_OK

	def _get_accState(self, this, varChild, pv):
		_count("accState")
		pv[0].value = self.n.states
		return S_OK

	def _get_accKeyboardShortcut(self, this, varChild, p):
		_count("accKeyboardShortcut")
		return put_bstr(p, None)

	def _get_accSelection(self, this, pv):
		_count("accSelection")
		return S_FALSE

	def accLocation(self, this, l, t, wd, h, varChild):
		_count("accLocation")
		for i, p in enumerate((l, t, wd, h)):
			cast(p, POINTER(c_long))[0] = (10, 10, 100, 20)[i]
		return S_OK

	# IAccessible2
	def role(self, this, p):
		_count("IA2.role")
		return put_long(p, self.n.role)

	def _get_states(self, this, p):
		_count("IA2.states")
		return put_long(p, self.n.ia2states)

	def IAccessible2__get_attributes(self, this, p):
		_count("IA2.attributes")
		return put_bstr(p, "".join(f"{_esc(k)}:{_esc(v)};" for k, v in self.n.attrs.items()))

	def _get_uniqueID(self, this, p):
		_count("IA2.uniqueID")
		return put_long(p, self.n.uid)

	def _get_windowHandle(self, this, p):
		_count("IA2.windowHandle")
		cast(p, POINTER(c_void_p))[0] = self.w.hwnd
		return S_OK

	def _get_indexInParent(self, this, p):
		_count("IA2.indexInParent")
		return put_long(p, self.n.parent.children.index(self.n) if self.n.parent else -1)

	def _get_nRelations(self, this, p):
		_count("IA2.nRelations")
		return put_long(p, 0)

	def _get_relationTargetsOfType(self, this, typ, maxTargets, ppTargets, pn):
		_count("IA2_2.relationTargetsOfType:" + str(typ))
		targets = []
		if typ == "labelledBy" and self.n.labelledBy is not None:
			targets = [self.n.labelledBy]
		if not targets:
			cast(ppTargets, POINTER(c_void_p))[0] = None
			put_long(pn, 0)
			return S_FALSE
		arr = _CoTaskMemAlloc(ctypes.sizeof(c_void_p) * len(targets))
		a = cast(arr, POINTER(c_void_p))
		for i, t in enumerate(targets):
			a[i] = raw_itf(t.com, IUnknown)
		cast(ppTargets, POINTER(c_void_p))[0] = arr
		return put_long(pn, len(targets))

	# IAccessibleText (only on nodes with content)
	def _get_nCharacters(self, this, p):
		_count("Text.nCharacters")
		return put_long(p, len(self.n.text))

	def _get_text(self, this, start, end, p):
		_count("Text.text")
		t = self.n.text
		if os.environ.get("IA2_TRACE"):
			print("get_text", self.n.uid, start, end, repr(t), flush=True)
		if end == -1:  # IA2_TEXT_OFFSET_LENGTH
			end = len(t)
		if not (0 <= start <= end <= len(t)):
			return E_INVALIDARG
		return put_bstr(p, t[start:end]) if end > start else put_bstr(p, "")

	def IAccessibleText__get_attributes(self, this, offset, pStart, pEnd, pAttrs):
		_count("Text.attributes")
		put_long(pStart, 0)
		put_long(pEnd, len(self.n.text))
		return put_bstr(pAttrs, "font-family:sans-serif;font-size:16pt;")

	def _get_newText(self, this, pSeg):
		"""IA2TextSegment {BSTR text; long start; long end}: the last insertion, set by the world when it mutates."""
		_count("Text.newText")
		ins = getattr(self.n, "lastInsert", None)
		if ins is None:
			return E_NOTIMPL
		text, start = ins
		cast(pSeg, POINTER(c_void_p))[0] = _SysAllocString(text)
		seg = cast(cast(pSeg, c_void_p).value + ctypes.sizeof(c_void_p), POINTER(c_long))
		seg[0], seg[1] = start, start + len(text)
		return S_OK

	def _get_caretOffset(self, this, p):
		_count("Text.caretOffset")
		put_long(p, -1)
		return S_FALSE

	# IAccessibleHypertext
	def _get_nHyperlinks(self, this, p):
		_count("Hypertext.nHyperlinks")
		return put_long(p, len(self.n.children))

	def _get_hyperlink(self, this, index, pp):
		_count("Hypertext.hyperlink")
		kids = self.n.children
		if not 0 <= index < len(kids):
			cast(pp, POINTER(c_void_p))[0] = None
			return E_INVALIDARG
		return put_itf(pp, kids[index], IA2.IAccessibleHyperlink)

	# IAccessibleHyperlink / IAccessibleAction
	def _get_startIndex(self, this, p):
		_count("Hyperlink.startIndex")
		return put_long(p, self.n.embedOffset(self.n))

	def _get_endIndex(self, this, p):
		_count("Hyperlink.endIndex")
		return put_long(p, self.n.embedOffset(self.n) + 1)

	def nActions(self, this, p):
		_count("Action.nActions")
		return put_long(p, len(self.n.actions))

	def IAccessibleAction__get_name(self, this, i, p):
		_count("Action.name")
		return put_bstr(p, self.n.actions[i])

	# IServiceProvider / IAccessibleApplication
	def QueryService(self, this, guidService, riid, ppv):
		svc = str(guidService[0])
		_count("QueryService " + svc)
		# As Chromium: only the IAccessible / IAccessible2 / IAccessibleApplication services answer.
		if svc not in _SERVICES:
			cast(ppv, POINTER(c_void_p))[0] = None
			return -2147467262  # E_NOINTERFACE
		return self.IUnknown_QueryInterface(None, riid, ppv)

	def _get_toolkitName(self, this, p):
		_count("App.toolkitName")
		return put_bstr(p, "Chrome")

	# IAccessibleTable2 / IAccessibleTableCell
	def _get_nRows(self, this, p):
		_count("Table2.nRows")
		return put_long(p, self.n.table[0])

	def _get_nColumns(self, this, p):
		_count("Table2.nColumns")
		return put_long(p, self.n.table[1])

	def _get_rowColumnExtents(self, this, pr, pc, prs, pcs, psel):
		_count("Cell.rowColumnExtents")
		r, c, _t = self.n.cell
		put_long(pr, r)
		put_long(pc, c)
		put_long(prs, 1)
		put_long(pcs, 1)
		cast(psel, POINTER(ctypes.c_short))[0] = 0
		return S_OK

	def _get_table(self, this, pp):
		_count("Cell.table")
		return put_itf(pp, self.n.cell[2], IUnknown)

	def _headers(self, axis, ppCells, pn):
		r, c, t = self.n.cell
		hdrs = []
		for row in t.children:
			for cell in row.children:
				cr, cc, _ = cell.cell
				if axis == "col" and cell.role == 0x19 and cc == c and cr != r:  # ROLE_SYSTEM_COLUMNHEADER
					hdrs.append(cell)
		if not hdrs:
			cast(ppCells, POINTER(c_void_p))[0] = None
			put_long(pn, 0)
			return S_OK
		arr = _CoTaskMemAlloc(ctypes.sizeof(c_void_p) * len(hdrs))
		a = cast(arr, POINTER(c_void_p))
		for i, h in enumerate(hdrs):
			a[i] = raw_itf(h.com, IUnknown)
		cast(ppCells, POINTER(c_void_p))[0] = arr
		return put_long(pn, len(hdrs))

	def _get_columnHeaderCells(self, this, ppCells, pn):
		_count("Cell.columnHeaderCells")
		return self._headers("col", ppCells, pn)

	def _get_rowHeaderCells(self, this, ppCells, pn):
		_count("Cell.rowHeaderCells")
		cast(ppCells, POINTER(c_void_p))[0] = None
		return put_long(pn, 0)
