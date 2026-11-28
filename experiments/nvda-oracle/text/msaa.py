"""Stand-in MSAA objects (IAccessible, IAccIdentity) and NVDA's oleacc module calls, backed by a
recorded world of windows: each window has a WINDOW object (OBJID_WINDOW) and a CLIENT object
(OBJID_CLIENT) as Windows' oleacc proxies give, with the proxies' parent rules:
the CLIENT's parent is its own WINDOW object, a WINDOW's parent is the parent window's CLIENT.
Every member NVDA calls is logged; anything the world does not hold is a miss, never a default."""
import ctypes
import struct

import comtypes
from comtypes import COMObject, IUnknown, hresult
from comtypes.automation import VARIANT, IDispatch

from comInterfaces import Accessibility as IA
import standins as S

OBJID_WINDOW, OBJID_CLIENT = 0, -4
ROLE = {
	"window": 9, "client": 10, "dialog": 18, "statictext": 41, "text": 42, "pushbutton": 43, "combobox": 46,
	"graphic": 40, "list": 33,
}
STATE = {
	"unavailable": 0x1, "selected": 0x2, "focused": 0x4, "pressed": 0x8, "checked": 0x10, "readonly": 0x40,
	"default": 0x100, "collapsed": 0x400, "invisible": 0x8000, "focusable": 0x100000, "selectable": 0x200000,
}
_ole32 = ctypes.oledll.ole32
_ole32.CoTaskMemAlloc.restype = ctypes.c_void_p


class Acc(COMObject):
	"""One MSAA object: its properties as oleacc would return them for child id 0."""

	_com_interfaces_ = [IA.IAccessible, IA.IAccIdentity]

	def __init__(self, key, hwnd, objid, role, name=None, value=None, description=None, states=(), shortcut=None,
			defaultAction=None, help=None, location=(0, 0, 0, 0)):
		super().__init__()
		self.key, self.hwnd, self.objid = key, hwnd, objid
		self.props = dict(role=ROLE[role] if isinstance(role, str) else role, name=name, value=value,
			description=description, shortcut=shortcut, defaultAction=defaultAction, help=help, location=location)
		self.states = set(states)
		self.parent = None
		self.children = []

	def __repr__(self):
		return f"<acc {self.key}>"

	def ptr(self, iface=IA.IAccessible):
		return self.QueryInterface(iface)

	def _child0(self, varChild, member):
		c = varChild.value if isinstance(varChild, VARIANT) else varChild
		S.called(f"IAccessible.{member}")
		if c not in (0, None):
			S.miss(f"{self.key}.{member}({c}): simple children not modelled")
			raise comtypes.ReturnHRESULT(hresult.E_INVALIDARG, "child")

	def _str(self, this, varChild, out, prop, member):
		self._child0(varChild, member)
		v = self.props[prop]
		if v is None:
			return hresult.S_FALSE  # oleacc proxies give S_FALSE and a NULL string for an absent property
		out[0] = v
		return hresult.S_OK

	def _get_accName(self, this, varChild, pszName):
		return self._str(this, varChild, pszName, "name", "accName")

	def _get_accValue(self, this, varChild, pszValue):
		return self._str(this, varChild, pszValue, "value", "accValue")

	def _get_accDescription(self, this, varChild, pszDescription):
		return self._str(this, varChild, pszDescription, "description", "accDescription")

	def _get_accKeyboardShortcut(self, this, varChild, psz):
		return self._str(this, varChild, psz, "shortcut", "accKeyboardShortcut")

	def _get_accDefaultAction(self, this, varChild, psz):
		return self._str(this, varChild, psz, "defaultAction", "accDefaultAction")

	def _get_accHelp(self, this, varChild, psz):
		return self._str(this, varChild, psz, "help", "accHelp")

	def _get_accRole(self, this, varChild, pvarRole):
		self._child0(varChild, "accRole")
		pvarRole[0].value = self.props["role"]
		return hresult.S_OK

	def _get_accState(self, this, varChild, pvarState):
		self._child0(varChild, "accState")
		v = 0
		for s in self.states:
			v |= STATE[s]
		pvarState[0].value = v
		return hresult.S_OK

	def _get_accParent(self, this, ppdispParent):
		S.called("IAccessible.accParent")
		if self.parent is None:
			return hresult.S_FALSE
		ppdispParent[0] = self.parent.ptr(IDispatch)
		return hresult.S_OK

	def _get_accChildCount(self, this, pcount):
		S.called("IAccessible.accChildCount")
		pcount[0] = len(self.children)
		return hresult.S_OK

	def _get_accChild(self, this, varChild, ppdispChild):
		S.called("IAccessible.accChild")
		S.miss(f"{self.key}.accChild({varChild.value}): not modelled")
		return hresult.E_NOTIMPL

	def _get_accFocus(self, this, pvarChild):
		S.called("IAccessible.accFocus")
		# The proxies answer with the focused child window's object, or nothing.
		for c in self.children:
			if "focused" in c.states:
				pvarChild[0].value = c.ptr(IDispatch)
				return hresult.S_OK
		if "focused" in self.states:
			pvarChild[0].value = 0  # CHILDID_SELF
			return hresult.S_OK
		pvarChild[0].vt = 0  # VT_EMPTY
		return hresult.S_FALSE

	def accLocation(self, this, pxLeft, pyTop, pcxWidth, pcyHeight, varChild):
		self._child0(varChild, "accLocation")
		pxLeft[0], pyTop[0], pcxWidth[0], pcyHeight[0] = self.props["location"]
		return hresult.S_OK

	def accNavigate(self, this, navDir, varStart, pvarEndUpAt):
		S.called(f"IAccessible.accNavigate({navDir})")
		S.miss(f"{self.key}.accNavigate({navDir}): not modelled")
		return hresult.E_NOTIMPL

	def GetIdentityString(self, this, dwIDChild, ppIDString, pdwIDStringLen):
		"""The proxies' window identity: flags, window, object id, child id (four 32-bit fields)."""
		S.called("IAccIdentity.GetIdentityString")
		raw = struct.pack("IIiI", 0, self.hwnd, self.objid, dwIDChild)
		p = _ole32.CoTaskMemAlloc(len(raw))
		ctypes.memmove(p, raw, len(raw))
		ppIDString[0] = ctypes.cast(p, type(ppIDString[0]))
		pdwIDStringLen[0] = len(raw)
		return hresult.S_OK


class MsaaWorld:
	def __init__(self):
		self.objs = {}  # (hwnd, objid) -> Acc
		self.byKey = {}

	def add(self, acc):
		S.register(acc)
		self.objs[(acc.hwnd, acc.objid)] = acc
		self.byKey[acc.key] = acc
		return acc

	def window(self, key, hwnd, parentClient, client, wname=None, location=(0, 0, 0, 0), wstates=("focusable",)):
		"""A window's two proxy objects: WINDOW (named by the window text) holding CLIENT."""
		w = self.add(Acc(key + ".window", hwnd, OBJID_WINDOW, "window", name=wname, states=wstates, location=location))
		client.hwnd, client.objid = hwnd, OBJID_CLIENT
		self.add(client)
		client.parent = w
		w.children = [client]
		w.parent = parentClient
		if parentClient is not None:
			parentClient.children.append(w)
		return w, client


def install_oleacc_standins(mw):
	import oleacc

	def fromEvent(hwnd, objectID, childID):
		S.called("oleacc.AccessibleObjectFromEvent")
		a = mw.objs.get((hwnd, objectID))
		if a is None:
			S.miss(f"msaa world: no object for window {hwnd:#x} object id {objectID}")
			raise OSError("no object")
		if childID:
			S.miss(f"msaa world: simple child {childID} of {a.key}")
		return a.ptr(), childID

	def fromWindow(hwnd, objectID, interface=IA.IAccessible):
		S.called("oleacc.AccessibleObjectFromWindow")
		a = mw.objs.get((hwnd, objectID))
		if a is None:
			S.miss(f"msaa world: no object for window {hwnd:#x} object id {objectID}")
			raise OSError("no object")
		return a.ptr(interface)

	def windowFrom(pacc):
		S.called("oleacc.WindowFromAccessibleObject")
		a = S.lookup(pacc)
		return a.hwnd

	def children(pacc, start, count):
		S.called("oleacc.AccessibleChildren")
		a = S.lookup(pacc)
		out = []
		for c in a.children[start:start + count]:
			v = VARIANT()
			v.value = c.ptr(IDispatch)
			out.append(v.value)  # as oleacc.AccessibleChildren unpacks its VARIANT array
		return out

	impl = {
		"AccessibleObjectFromEvent": fromEvent,
		"AccessibleObjectFromEvent_safe": lambda h, o, c, timeout=2: fromEvent(h, o, c),
		"AccessibleObjectFromWindow": fromWindow,
		"AccessibleObjectFromWindow_safe": lambda h, o, interface=IA.IAccessible, timeout=2: fromWindow(h, o, interface),
		"WindowFromAccessibleObject": windowFrom,
		"AccessibleChildren": children,
		"GetProcessHandleFromHwnd": lambda h: 0x9000,
	}
	# Role and state names are oleacc.dll's own localised table in NVDA's process: real calls.
	OWN = {"GetRoleText", "GetStateText"}
	import inspect

	for name, f in list(vars(oleacc).items()):
		if inspect.isfunction(f) and f.__module__ == "oleacc" and name not in OWN:
			if name in impl:
				g = impl[name]
			else:
				def g(*a, _n=name, **k):
					S.miss(f"oleacc.{_n}: no stand-in")
					raise OSError(f"stand-in missing: oleacc.{_n}")
			setattr(oleacc, name, g)
