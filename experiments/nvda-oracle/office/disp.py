"""A stand-in IDispatch with no type information, dispatched by DISPID (as nvdaHelperRemote's
_com_dispatch_raw_* calls do) and by name through GetIDsOfNames (as comtypes' dynamic Dispatch does
when GetTypeInfo fails). Each class lists its members once: name -> (dispid, kind, handler).

Every call is counted in CALLS as (class, member); a DISPID or name the class lacks is counted in MISSES."""
import ctypes, re, os
from collections import Counter
import comtypes
from comtypes import COMObject, COMError, hresult
from comtypes.automation import IDispatch, VARIANT, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPATCH_PROPERTYPUTREF

CALLS = Counter()
MISSES = Counter()
DISPID_PROPERTYPUT = -3
_REG = {}  # address of our IDispatch pointer -> object


def parse_constants(path, prefix):
	"""wdDISPID_RANGE_START = 3  ->  {'RANGE': {'START': 3}}"""
	out = {}
	for cls, mem, val in re.findall(prefix + r"DISPID_([A-Z]+)_([A-Z0-9_]+)\s*=\s*(-?\d+)", open(path).read()):
		out.setdefault(cls, {})[mem] = int(val)
	return out


class Disp(COMObject):
	_com_interfaces_ = [IDispatch]
	# Subclasses: MEMBERS = {"Start": (3, "prop"), "SetRange": (100, "method"), ...}; handler is get_<Name>/put_<Name>/<Name>.
	MEMBERS: dict = {}

	_idcache = {}

	def _byid(self):
		key = id(self.MEMBERS)
		m = Disp._idcache.get(key)
		if m is None:
			m = {d: (n, k) for n, (d, k) in self.MEMBERS.items()}
			Disp._idcache[key] = m
		return m

	def ptr(self):
		p = self.QueryInterface(IDispatch)
		_REG[ctypes.cast(p, ctypes.c_void_p).value] = self
		return p

	def IDispatch_GetIDsOfNames(self, this, riid, rgszNames, cNames, lcid, rgDispId):
		name = rgszNames[0]
		for n, (d, k) in self.MEMBERS.items():
			if n.lower() == name.lower():
				rgDispId[0] = d
				return hresult.S_OK
		MISSES[(type(self).__name__, "name:" + name)] += 1
		return hresult.DISP_E_UNKNOWNNAME

	def IDispatch_Invoke(self, this, dispid, riid, lcid, wFlags, pDispParams, pVarResult, pExcepInfo, puArgErr):
		cls = type(self).__name__
		ent = self._byid().get(dispid)
		if ent is None:
			MISSES[(cls, f"dispid:{dispid}:flags{wFlags}")] += 1
			return hresult.DISP_E_MEMBERNOTFOUND
		name, kind = ent
		dp = pDispParams[0]
		args = [_unwrap(dp.rgvarg[i]) for i in range(dp.cArgs)][::-1]
		named = [dp.rgdispidNamedArgs[i] for i in range(dp.cNamedArgs)]
		try:
			if wFlags & (DISPATCH_PROPERTYPUT | DISPATCH_PROPERTYPUTREF):
				CALLS[(cls, name + "=")] += 1
				fn = getattr(self, "put_" + name, None)
				if fn is None:
					MISSES[(cls, name + "= (no setter)")] += 1
					return hresult.DISP_E_MEMBERNOTFOUND
				fn(*args)
				return hresult.S_OK
			if kind == "method" and not (wFlags & DISPATCH_METHOD) and not args:
				# A method fetched as a property with no arguments: comtypes' dynamic Dispatch then calls it.
				return hresult.DISP_E_MEMBERNOTFOUND
			CALLS[(cls, name)] += 1
			fn = getattr(self, ("get_" + name) if kind == "prop" else name)
			r = fn(*args)
		except COMError as e:
			return e.hresult
		except AttributeError as e:
			MISSES[(cls, f"{name}: no handler")] += 1
			return hresult.DISP_E_MEMBERNOTFOUND
		except TypeError as e:
			MISSES[(cls, f"{name} args {args}: {e}")] += 1
			return hresult.DISP_E_BADPARAMCOUNT
		if pVarResult:
			_wrap(pVarResult, r)
		return hresult.S_OK


def _unwrap(v):
	val = v.value
	if isinstance(val, ctypes.POINTER(IDispatch)) or (hasattr(val, "_iid_") and val):
		try:
			p = val.QueryInterface(IDispatch)
			o = _REG.get(ctypes.cast(p, ctypes.c_void_p).value)
			if o is not None:
				return o
		except Exception:
			pass
	return val


def _wrap(pVarResult, r):
	if isinstance(r, Disp):
		pVarResult[0].value = r.ptr()
	elif r is None:
		pass
	else:
		pVarResult[0].value = r
