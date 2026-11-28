"""Stand-ins for the UI Automation client and the window functions, backed by a recorded world.

Each stand-in is a real in-process COM object (comtypes COMObject), so NVDA reaches it through
real vtables, comtypes' marshalling, its case-insensitive names, isinstance checks and HRESULTs.
Every member NVDA calls is logged; a member with no implementation, a property the world does not
hold, or a cached read of an uncached property is logged as a miss, never defaulted."""
import ctypes
import array
import inspect
import threading
from collections import OrderedDict

import comtypes
from comtypes import COMObject, COMError, IUnknown, hresult
from comtypes.automation import VARIANT
import comtypes._vtbl as _vtbl
from comtypes.safearray import _midlSAFEARRAY

from comInterfaces import UIAutomationClient as UIA

CALLS = OrderedDict()  # member -> count, in first-call order
MISSES = []
_KEEP = []  # stand-ins stay alive for the run
_lock = threading.Lock()


def hold(p):
	"""Keep a reference to an interface NVDA hands over, as UIA does."""
	if p:
		p.AddRef()
		_KEEP.append(p)
	return p


def called(member):
	with _lock:
		CALLS[member] = CALLS.get(member, 0) + 1


def miss(text):
	with _lock:
		if text not in MISSES:
			MISSES.append(text)


def _do_implement(interface_name, method_name):
	def _missing(*args):
		miss(f"COM {interface_name}.{method_name}: no stand-in")
		return hresult.E_NOTIMPL

	return _missing


_vtbl._do_implement = _do_implement

PROPNAME = {v: k[4:-10] for k, v in vars(UIA).items() if k.startswith("UIA_") and k.endswith("PropertyId")}
PATNAME = {v: k[4:-9] for k, v in vars(UIA).items() if k.startswith("UIA_") and k.endswith("PatternId")}
PROPID = {n: i for i, n in PROPNAME.items()}


class NS:
	"""A property the element does not support: Ex(ignoreDefault=True) gives UIA's
	reserved not-supported value; a plain read gives the default UIA substitutes."""

	def __init__(self, default):
		self.default = default


def addr(p):
	return ctypes.cast(p, ctypes.c_void_p).value


_REG = {}


def register(obj):
	# One COM reference held for the run: comtypes empties an object's pointers at refcount 0.
	_KEEP.append((obj, obj.QueryInterface(IUnknown)))
	for p in obj._com_pointers_.values():
		_REG[addr(p)] = obj
	return obj


def lookup(p):
	if not p:
		return None
	o = _REG.get(addr(p))
	if o is None:
		o = _REG.get(addr(p.QueryInterface(IUnknown)))
	return o


class Sentinel(COMObject):
	_com_interfaces_ = [IUnknown]


NOT_SUPPORTED = register(Sentinel())
MIXED = register(Sentinel())
NOT_SUPPORTED_PTR = NOT_SUPPORTED.QueryInterface(IUnknown)
MIXED_PTR = MIXED.QueryInterface(IUnknown)


def set_variant(pvar, v):
	if isinstance(v, Node):
		pvar[0].value = make_element(v, None).QueryInterface(IUnknown)
	elif isinstance(v, float) or (isinstance(v, tuple) and v and isinstance(v[0], float)):
		pvar[0].value = array.array("d", v) if isinstance(v, tuple) else v
	elif isinstance(v, tuple):
		pvar[0].value = array.array("i", v)
	elif v is NOT_SUPPORTED:
		pvar[0].value = NOT_SUPPORTED_PTR
	else:
		pvar[0].value = v


DEFAULT_PATTERNS = {}


class Node:
	"""One recorded element: its properties by id, its patterns, its place in the raw tree."""

	def __init__(self, key, props, patterns=None, children=()):
		self.key = key
		self.props = {PROPID[k] if isinstance(k, str) else k: v for k, v in props.items()}
		self.patterns = dict(patterns) if patterns is not None else dict(DEFAULT_PATTERNS)
		self.parent = None
		self.children = list(children)
		for c in self.children:
			c.parent = self

	def value(self, pid, ignoreDefault, how):
		if pid not in self.props:
			miss(f"world: {self.key} has no {PROPNAME.get(pid, pid)} ({how})")
			raise KeyError(pid)
		v = self.props[pid]
		if isinstance(v, NS):
			return NOT_SUPPORTED if ignoreDefault else v.default
		return v

	def __repr__(self):
		return f"<{self.key}>"


class Element(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationElement9]

	def __init__(self, node, cache):
		super().__init__()
		self.node = node
		self.cache = cache  # (frozenset props, frozenset patterns) or None

	# -- property values --
	def _read(self, pid, ignoreDefault, cached, pvar):
		how = "cached" if cached else "current"
		called(f"IUIAutomationElement.Get{how.title()}PropertyValue[Ex]({PROPNAME.get(pid, pid)})")
		if cached and (self.cache is None or pid not in self.cache[0]):
			miss(f"uncached read: {self.node.key}.{PROPNAME.get(pid, pid)}")
			return hresult.E_INVALIDARG
		try:
			v = self.node.value(pid, ignoreDefault, how)
		except KeyError:
			return hresult.E_FAIL
		set_variant(pvar, v)
		return hresult.S_OK

	def GetCurrentPropertyValue(self, this, propertyId, retVal):
		return self._read(propertyId, False, False, retVal)

	def GetCurrentPropertyValueEx(self, this, propertyId, ignoreDefaultValue, retVal):
		return self._read(propertyId, bool(ignoreDefaultValue), False, retVal)

	def GetCachedPropertyValue(self, this, propertyId, retVal):
		return self._read(propertyId, False, True, retVal)

	def GetCachedPropertyValueEx(self, this, propertyId, ignoreDefaultValue, retVal):
		return self._read(propertyId, bool(ignoreDefaultValue), True, retVal)

	def __getattr__(self, name):
		# Current<Prop> and Cached<Prop> getters; comtypes falls back to these at call time.
		for prefix in ("Current", "Cached"):
			if name.startswith(prefix) and name[len(prefix):] in PROPID:
				prop = name[len(prefix):]
				pid = PROPID[prop]
				cached = prefix == "Cached"
				called(f"IUIAutomationElement.{name}")
				if cached and (self.cache is None or pid not in self.cache[0]):
					miss(f"uncached read: {self.node.key}.{prop}")
					raise comtypes.ReturnHRESULT(hresult.E_INVALIDARG, "not cached")
				try:
					v = self.node.value(pid, False, prefix.lower())
				except KeyError:
					raise comtypes.ReturnHRESULT(hresult.E_FAIL, "not in world")
				if prop == "BoundingRectangle":
					l, t, w, h = v
					return UIA.tagRECT(int(l), int(t), int(l + w), int(t + h))
				if isinstance(v, Node):
					return make_element(v, None).QueryInterface(UIA.IUIAutomationElement)
				return v
		raise AttributeError(name)

	def GetRuntimeId(self, this, runtimeId):
		called("IUIAutomationElement.GetRuntimeId")
		runtimeId[0] = _midlSAFEARRAY(ctypes.c_int).create(list(self.node.value(PROPID["RuntimeId"], False, "current")))
		return hresult.S_OK

	def BuildUpdatedCache(self, cacheRequest):
		called("IUIAutomationElement.BuildUpdatedCache")
		return make_element(self.node, lookup(cacheRequest)).QueryInterface(UIA.IUIAutomationElement)

	# -- patterns --
	def _pattern(self, pid, cached):
		how = "Cached" if cached else "Current"
		called(f"IUIAutomationElement.Get{how}Pattern({PATNAME.get(pid, pid)})")
		if cached and (self.cache is None or pid not in self.cache[1]):
			miss(f"uncached pattern: {self.node.key}.{PATNAME.get(pid, pid)}")
			raise comtypes.ReturnHRESULT(hresult.E_INVALIDARG, "not cached")
		if pid not in self.node.patterns:
			miss(f"world: {self.node.key} has no answer for pattern {PATNAME.get(pid, pid)}")
			raise comtypes.ReturnHRESULT(hresult.E_FAIL, "not in world")
		p = self.node.patterns[pid]
		if p is None:
			return None
		miss(f"pattern object asked for: {self.node.key}.{PATNAME.get(pid, pid)} (no stand-in)")
		raise comtypes.ReturnHRESULT(hresult.E_NOTIMPL, "pattern stand-in")

	def GetCurrentPattern(self, patternId):
		return self._pattern(patternId, False)

	def GetCachedPattern(self, patternId):
		return self._pattern(patternId, True)


_elements = []


def make_element(node, cacheRequest):
	cache = None
	if cacheRequest is not None:
		cache = (frozenset(cacheRequest.props), frozenset(cacheRequest.patterns))
		if cacheRequest.TreeScope != UIA.TreeScope_Element:
			miss(f"cache request with tree scope {cacheRequest.TreeScope} (children not modelled)")
	return register(Element(node, cache))


class CacheRequest(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationCacheRequest]

	def __init__(self, props=(), patterns=(), scope=None, filt=None, mode=None):
		super().__init__()
		self.props = set(props)
		self.patterns = set(patterns)
		self.TreeScope = UIA.TreeScope_Element if scope is None else scope
		self.TreeFilter = filt
		self.AutomationElementMode = UIA.AutomationElementMode_Full if mode is None else mode

	def AddProperty(self, propertyId):
		called("IUIAutomationCacheRequest.AddProperty")
		self.props.add(propertyId)

	def AddPattern(self, patternId):
		called("IUIAutomationCacheRequest.AddPattern")
		self.patterns.add(patternId)

	def Clone(self):
		called("IUIAutomationCacheRequest.Clone")
		return register(
			CacheRequest(self.props, self.patterns, self.TreeScope, self.TreeFilter, self.AutomationElementMode),
		).QueryInterface(UIA.IUIAutomationCacheRequest)


# -- conditions --
class PropertyCondition(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationPropertyCondition]

	def __init__(self, pid, value, flags=0):
		super().__init__()
		self.PropertyId, self.PropertyValue, self.PropertyConditionFlags = pid, value, flags

	def match(self, node):
		try:
			v = node.value(self.PropertyId, False, "condition")
		except KeyError:
			return False
		if isinstance(self.PropertyValue, str) and isinstance(v, str) and self.PropertyConditionFlags & 1:
			return v.lower() == self.PropertyValue.lower()
		return v == self.PropertyValue


class NotCondition(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationNotCondition]

	def __init__(self, child):
		super().__init__()
		self.child = child

	def match(self, node):
		return not self.child.match(node)


class BoolCondition(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationBoolCondition]

	def __init__(self, value):
		super().__init__()
		self.BooleanValue = value

	def match(self, node):
		return self.BooleanValue


class JunctionCondition(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationAndCondition]

	def __init__(self, children, isAnd):
		super().__init__()
		self.kids, self.isAnd = children, isAnd

	def match(self, node):
		results = [c.match(node) for c in self.kids]
		return all(results) if self.isAnd else any(results)


class OrCondition(JunctionCondition):
	_com_interfaces_ = [UIA.IUIAutomationOrCondition]


def cond_ptr(c):
	return register(c).QueryInterface(UIA.IUIAutomationCondition)


# -- tree walker --
class TreeWalker(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationTreeWalker]

	def __init__(self, world, condition, name):
		super().__init__()
		self.world, self.cond, self.name = world, condition, name

	def inView(self, node):
		return node is self.world.root or self.cond.match(node)

	def viewParent(self, node):
		p = node.parent
		while p is not None and not self.inView(p):
			p = p.parent
		return p

	def viewChildren(self, node):
		out = []
		for c in node.children:
			if self.inView(c):
				out.append(c)
			else:
				out.extend(self.viewChildren(c))
		return out

	def _out(self, node, cacheRequest):
		if node is None:
			return None
		return make_element(node, lookup(cacheRequest) if cacheRequest else None).QueryInterface(
			UIA.IUIAutomationElement,
		)

	def _sib(self, node, step):
		p = self.viewParent(node)
		if p is None:
			return None
		sibs = self.viewChildren(p)
		i = sibs.index(node) + step
		return sibs[i] if 0 <= i < len(sibs) else None

	def GetParentElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).GetParentElementBuildCache")
		return self._out(self.viewParent(lookup(element).node), cacheRequest)

	def GetParentElement(self, element):
		called(f"IUIAutomationTreeWalker({self.name}).GetParentElement")
		return self._out(self.viewParent(lookup(element).node), None)

	def GetFirstChildElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).GetFirstChildElementBuildCache")
		k = self.viewChildren(lookup(element).node)
		return self._out(k[0] if k else None, cacheRequest)

	def GetLastChildElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).GetLastChildElementBuildCache")
		k = self.viewChildren(lookup(element).node)
		return self._out(k[-1] if k else None, cacheRequest)

	def GetNextSiblingElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).GetNextSiblingElementBuildCache")
		return self._out(self._sib(lookup(element).node, 1), cacheRequest)

	def GetPreviousSiblingElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).GetPreviousSiblingElementBuildCache")
		return self._out(self._sib(lookup(element).node, -1), cacheRequest)

	def NormalizeElementBuildCache(self, element, cacheRequest):
		called(f"IUIAutomationTreeWalker({self.name}).NormalizeElementBuildCache")
		n = lookup(element).node
		while n is not None and not self.inView(n):
			n = n.parent
		return self._out(n, cacheRequest)


class ProxyFactoryMapping(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationProxyFactoryMapping]
	count = 0


class EventHandlerGroup(COMObject):
	_com_interfaces_ = [UIA.IUIAutomationEventHandlerGroup]

	def __init__(self):
		super().__init__()
		self.handlers = []

	def AddActiveTextPositionChangedEventHandler(self, scope, cacheRequest, handler):
		called("IUIAutomationEventHandlerGroup.AddActiveTextPositionChangedEventHandler")
		hold(handler)

	def AddAutomationEventHandler(self, eventId, scope, cacheRequest, handler):
		called("IUIAutomationEventHandlerGroup.AddAutomationEventHandler")
		hold(handler)

	def AddNotificationEventHandler(self, scope, cacheRequest, handler):
		called("IUIAutomationEventHandlerGroup.AddNotificationEventHandler")
		hold(handler)

	def AddPropertyChangedEventHandler(self, scope, cacheRequest, handler, propertyArray, propertyCount):
		called("IUIAutomationEventHandlerGroup.AddPropertyChangedEventHandler")
		hold(handler)


class Automation(COMObject):
	_com_interfaces_ = [UIA.IUIAutomation6]

	def __init__(self, world):
		super().__init__()
		self.world = world
		self.focusCacheRequest = None
		self.focusHandler = None
		self.CoalesceEvents = 0
		self.ConnectionRecoveryBehavior = 0
		self.groupRegistrations = []
		self.trueCond = register(BoolCondition(True))
		self.rawWalker = register(TreeWalker(world, self.trueCond, "raw"))
		self.controlCond = register(PropertyCondition(UIA.UIA_IsControlElementPropertyId, True))
		self.controlWalker = register(TreeWalker(world, self.controlCond, "control"))

	def _el(self, node, cacheRequest):
		return make_element(node, lookup(cacheRequest) if cacheRequest else None).QueryInterface(
			UIA.IUIAutomationElement,
		)

	def _get_ProxyFactoryMapping(self):
		called("IUIAutomation.ProxyFactoryMapping")
		return register(ProxyFactoryMapping()).QueryInterface(UIA.IUIAutomationProxyFactoryMapping)

	def _get_RawViewWalker(self):
		called("IUIAutomation.RawViewWalker")
		return self.rawWalker.QueryInterface(UIA.IUIAutomationTreeWalker)

	def _get_ControlViewWalker(self):
		called("IUIAutomation.ControlViewWalker")
		return self.controlWalker.QueryInterface(UIA.IUIAutomationTreeWalker)

	def _get_ReservedNotSupportedValue(self):
		called("IUIAutomation.ReservedNotSupportedValue")
		return NOT_SUPPORTED_PTR

	def _get_ReservedMixedAttributeValue(self):
		called("IUIAutomation.ReservedMixedAttributeValue")
		return MIXED_PTR

	def CreateCacheRequest(self):
		called("IUIAutomation.CreateCacheRequest")
		return register(CacheRequest()).QueryInterface(UIA.IUIAutomationCacheRequest)

	def CreatePropertyCondition(self, this, propertyId, value, newCondition):
		called("IUIAutomation.CreatePropertyCondition")
		newCondition[0] = cond_ptr(PropertyCondition(propertyId, value.value))
		return hresult.S_OK

	def CreateNotCondition(self, condition):
		called("IUIAutomation.CreateNotCondition")
		return cond_ptr(NotCondition(lookup(condition)))

	def CreateTrueCondition(self):
		called("IUIAutomation.CreateTrueCondition")
		return self.trueCond.QueryInterface(UIA.IUIAutomationCondition)

	def CreateTreeWalker(self, pCondition):
		called("IUIAutomation.CreateTreeWalker")
		return register(TreeWalker(self.world, lookup(pCondition), "custom")).QueryInterface(
			UIA.IUIAutomationTreeWalker,
		)

	def GetRootElementBuildCache(self, cacheRequest):
		called("IUIAutomation.GetRootElementBuildCache")
		return self._el(self.world.root, cacheRequest)

	def GetFocusedElement(self):
		called("IUIAutomation.GetFocusedElement")
		return self._el(self.world.focused, None)

	def GetFocusedElementBuildCache(self, cacheRequest):
		called("IUIAutomation.GetFocusedElementBuildCache")
		return self._el(self.world.focused, cacheRequest)

	def ElementFromHandleBuildCache(self, hwnd, cacheRequest):
		called("IUIAutomation.ElementFromHandleBuildCache")
		return self._el(self.world.byHwnd[hwnd], cacheRequest)

	def CompareElements(self, el1, el2):
		called("IUIAutomation.CompareElements")
		return lookup(el1).node is lookup(el2).node

	def AddFocusChangedEventHandler(self, cacheRequest, handler):
		called("IUIAutomation.AddFocusChangedEventHandler")
		self.focusCacheRequest = lookup(cacheRequest)
		self.focusHandler = hold(handler)

	def CreateEventHandlerGroup(self):
		called("IUIAutomation6.CreateEventHandlerGroup")
		return register(EventHandlerGroup()).QueryInterface(UIA.IUIAutomationEventHandlerGroup)

	def AddEventHandlerGroup(self, element, handlerGroup):
		called("IUIAutomation6.AddEventHandlerGroup")
		self.groupRegistrations.append(lookup(element).node.key)

	def RemoveEventHandlerGroup(self, element, handlerGroup):
		called("IUIAutomation6.RemoveEventHandlerGroup")
		self.groupRegistrations.remove(lookup(element).node.key)

	def RemoveAllEventHandlers(self):
		called("IUIAutomation.RemoveAllEventHandlers")

	def IntSafeArrayToNativeArray(self, this, intArray, array, arrayCount):
		called("IUIAutomation.IntSafeArrayToNativeArray")
		arrayCount[0] = 0
		return hresult.S_OK


# -- window functions --
class Window:
	def __init__(self, hwnd, cls, text="", parent=0, pid=0, tid=0, style=0x16CF0000, exstyle=0, controlID=0, uiaProvider=False):
		self.__dict__.update(locals())
		del self.__dict__["self"]


class WinWorld:
	def __init__(self, windows, desktop, foreground, focusHwnd):
		self.w = {x.hwnd: x for x in windows}
		self.desktop, self.foreground, self.focusHwnd = desktop, foreground, focusHwnd


def install_window_standins(ww):
	import winUser, winKernel, watchdog, appModuleHandler
	import winBindings.user32 as user32
	import winBindings.uiAutomationCore as uiaCore

	def W(h):
		if h not in ww.w:
			miss(f"window world: no window {h:#x}")
			raise LookupError(h)
		return ww.w[h]

	def getAncestor(h, flags):
		w = W(h)
		if flags == winUser.GA_PARENT:
			return w.parent
		if flags in (winUser.GA_ROOT, winUser.GA_ROOTOWNER):
			while w.parent and w.parent != ww.desktop:
				w = W(w.parent)
			return w.hwnd
		miss(f"getAncestor flags {flags}")
		return 0

	def isDescendantWindow(parent, child):
		while child:
			if child == parent:
				return True
			child = W(child).parent
		return False

	def getGUIThreadInfo(tid):
		gi = user32.GUITHREADINFO()
		fg = W(ww.foreground)
		if tid in (0, fg.tid):
			gi.hwndActive = ww.foreground
			gi.hwndFocus = ww.focusHwnd
		return gi

	impl = {
		"getClassName": lambda h: W(h).cls,
		"getWindowThreadProcessID": lambda h: (W(h).pid, W(h).tid),
		"getAncestor": getAncestor,
		"getForegroundWindow": lambda: ww.foreground,
		"getDesktopWindow": lambda: ww.desktop,
		"getWindowText": lambda h: W(h).text,
		"getWindowStyle": lambda h: W(h).style,
		"getExtendedWindowStyle": lambda h: W(h).exstyle,
		"getControlID": lambda h: W(h).controlID,
		"isWindow": lambda h: h in ww.w,
		"isWindowVisible": lambda h: bool(W(h).style & winUser.WS_VISIBLE),
		"isHungAppWindow": lambda h: False,
		"isDescendantWindow": isDescendantWindow,
		"getGUIThreadInfo": getGUIThreadInfo,
		"getParent": lambda h: W(h).parent,
	}

	def wrap(modname, name, f):
		def standin(*a, **k):
			called(f"{modname}.{name}")
			return f(*a, **k)

		return standin

	def raiser(modname, name):
		def standin(*a, **k):
			miss(f"{modname}.{name}{a}: no stand-in")
			raise RuntimeError(f"stand-in missing: {modname}.{name}")

		return standin

	for name, f in list(vars(winUser).items()):
		if inspect.isfunction(f) and f.__module__ == "winUser":
			setattr(winUser, name, wrap("winUser", name, impl[name]) if name in impl else raiser("winUser", name))
	# Calls about NVDA's own process and thread (its desktop, its session) reach the real system,
	# logged apart; every other user32 call is about the recorded application and must be a stand-in.
	OWN = {"GetThreadDesktop", "GetUserObjectInformation", "GetUserObjectInformationW", "CloseDesktop", "OpenInputDesktop"}

	def passthrough(name, f):
		def standin(*a, **k):
			called(f"user32.{name} (NVDA's own state, real call)")
			return f(*a, **k)

		return standin

	for name, f in list(vars(user32).items()):
		if isinstance(f, ctypes._CFuncPtr):
			setattr(user32, name, passthrough(name, f) if name in OWN else raiser("user32", name))

	kimpl = {"waitForSingleObject": lambda h, t: 0x102}  # WAIT_TIMEOUT: the process lives
	for name, f in list(vars(winKernel).items()):
		if inspect.isfunction(f) and f.__module__ == "winKernel":
			setattr(winKernel, name, wrap("winKernel", name, kimpl[name]) if name in kimpl else raiser("winKernel", name))

	uiaCore.UiaHasServerSideProvider = wrap("uiAutomationCore", "UiaHasServerSideProvider", lambda h: W(h).uiaProvider)

	def sendMessage(h, msg, wp, lp, flags=0, timeout=60000):
		if msg == winUser.WM_GETTEXTLENGTH:
			return len(W(h).text)
		if msg == winUser.WM_GETTEXT:
			ctypes.memmove(lp, ctypes.create_unicode_buffer(W(h).text), (len(W(h).text) + 1) * 2)
			return len(W(h).text)
		miss(f"watchdog.cancellableSendMessage msg {msg:#x}")
		raise RuntimeError("message")

	watchdog.cancellableSendMessage = wrap("watchdog", "cancellableSendMessage", sendMessage)
