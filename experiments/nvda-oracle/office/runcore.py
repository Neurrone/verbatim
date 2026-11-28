"""Real UI Automation core running a remote operation against an in-process stand-in provider.

The application thread owns a never-shown window whose WM_GETOBJECT(UiaRootObjectId) returns a Python
IRawElementProviderSimple that also implements ICoreAutomationRemoteOperationExtensionProvider, the
interface a provider such as Word's implements for custom extensions. NVDA's own RemoteExecutor (its
shipped UIARemote.dll over Windows' CoreAutomationRemoteOperation) runs NVDA-built bytecode that asks
IsExtensionSupported and CallExtension on the element. Nothing outside this process is touched."""
import os, sys, ctypes, threading, time
from ctypes import POINTER, byref, c_void_p, c_int, c_long, c_ulong, WINFUNCTYPE
from ctypes import wintypes as w

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401
import comtypes
from comtypes import GUID, COMObject, IUnknown, COMMETHOD, HRESULT, hresult
from comtypes.automation import VARIANT
from comInterfaces import UIAutomationClient as UIA
from UIAHandler._remoteOps import operation, remoteAPI
from UIAHandler._remoteOps.remoteTypes import RemoteExtensionTarget

LOG = []
u32 = ctypes.WinDLL("user32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32")
uiac = ctypes.WinDLL("UIAutomationCore")
WNDPROC = WINFUNCTYPE(ctypes.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
u32.DefWindowProcW.restype = ctypes.c_ssize_t
u32.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u32.CreateWindowExW.restype = w.HWND
u32.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, c_int, c_int, c_int, c_int, w.HWND, w.HMENU, w.HINSTANCE, c_void_p]
uiac.UiaReturnRawElementProvider.restype = ctypes.c_ssize_t
uiac.UiaReturnRawElementProvider.argtypes = [w.HWND, w.WPARAM, w.LPARAM, c_void_p]
uiac.UiaHostProviderFromHwnd.argtypes = [w.HWND, POINTER(POINTER(UIA.IRawElementProviderSimple))]


class IInspectable(IUnknown):
	_iid_ = GUID("{AF86E2E0-B12D-4C6A-9C5A-D7AA65101E90}")
	_methods_ = [
		COMMETHOD([], HRESULT, "GetIids", (["out"], POINTER(c_ulong), "iidCount"), (["out"], POINTER(POINTER(GUID)), "iids")),
		COMMETHOD([], HRESULT, "GetRuntimeClassName", (["out"], POINTER(c_void_p), "className")),
		COMMETHOD([], HRESULT, "GetTrustLevel", (["out"], POINTER(c_int), "trustLevel")),
	]


class ICoreAutomationRemoteOperationExtensionProvider(IInspectable):
	_iid_ = GUID("{88F53E67-DC69-553B-A0AA-70477E724DA8}")
	_methods_ = [
		COMMETHOD([], HRESULT, "CallExtension", (["in"], GUID, "extensionId"), (["in"], c_void_p, "context"),
			(["in"], c_ulong, "operandIdsSize"), (["in"], POINTER(c_int), "operandIds")),
		COMMETHOD([], HRESULT, "IsExtensionSupported", (["in"], GUID, "extensionId"), (["out"], POINTER(ctypes.c_bool), "result")),
	]


class ICoreAutomationConnectionBoundObjectProvider(IInspectable):
	_iid_ = GUID("{0620BB64-9616-5593-BE3A-EB8E6DAEB3FA}")
	_methods_ = [COMMETHOD(["propget"], HRESULT, "IsComThreadingRequired", (["out", "retval"], POINTER(ctypes.c_bool), "value"))]


G_PATTERN = GUID("{93514122-FF04-4B2C-A4AD-4AB04587C129}")
G_MOVE = GUID("{F39655AC-133A-435B-A318-C197F0D3D203}")
combase = ctypes.WinDLL("combase")
combase.WindowsCreateString.argtypes = [w.LPCWSTR, ctypes.c_uint32, POINTER(c_void_p)]
combase.RoGetActivationFactory.argtypes = [c_void_p, POINTER(GUID), POINTER(c_void_p)]


def vcall(p, slot, restype, *argtypes):
	vt = ctypes.cast(ctypes.cast(p, POINTER(c_void_p))[0], POINTER(c_void_p))
	return ctypes.WINFUNCTYPE(restype, c_void_p, *argtypes)(vt[slot])


def box_int(v):
	"""Windows.Foundation.PropertyValue.CreateInt32(v), an IInspectable boxing an int."""
	hs = c_void_p()
	name = "Windows.Foundation.PropertyValue"
	combase.WindowsCreateString(name, len(name), byref(hs))
	fac = c_void_p()
	assert combase.RoGetActivationFactory(hs, byref(GUID("{629BDBC8-D932-4FF4-96B9-8D96C5C1E858}")), byref(fac)) == 0
	out = c_void_p()
	assert vcall(fac, 10, HRESULT, c_int, POINTER(c_void_p))(fac, v, byref(out)) == 0  # CreateInt32
	return out


def unbox_int(insp):
	"""IReference<INT32>::get_Value."""
	ref = c_void_p()
	iid = GUID("{548CEFBD-BC8A-5FA0-8DF2-957440FC8BF4}")  # IReference<INT32>
	assert vcall(insp, 0, HRESULT, POINTER(GUID), POINTER(c_void_p))(insp, byref(iid), byref(ref)) == 0
	v = c_int()
	assert vcall(ref, 6, HRESULT, POINTER(c_int))(ref, byref(v)) == 0
	return v.value


class Target(COMObject):
	"""Stands for the extension target an extension returns (Word's extendedTextRangePattern object)."""
	_com_interfaces_ = [ICoreAutomationConnectionBoundObjectProvider, ICoreAutomationRemoteOperationExtensionProvider]

	def ICoreAutomationConnectionBoundObjectProvider__get_IsComThreadingRequired(self, this, value):
		LOG.append("target IsComThreadingRequired")
		value[0] = False
		return hresult.S_OK

	def IInspectable_GetIids(self, this, count, iids):
		return hresult.E_NOTIMPL

	def IInspectable_GetRuntimeClassName(self, this, name):
		return hresult.E_NOTIMPL

	def IInspectable_GetTrustLevel(self, this, level):
		level[0] = 0
		return hresult.S_OK

	def ICoreAutomationRemoteOperationExtensionProvider_IsExtensionSupported(self, this, extensionId, result):
		LOG.append(f"target IsExtensionSupported({extensionId})")
		result[0] = extensionId == G_MOVE
		return hresult.S_OK

	def ICoreAutomationRemoteOperationExtensionProvider_CallExtension(self, this, extensionId, context, n, ids):
		ids = [ids[i] for i in range(n)]
		get = vcall(context, 6, HRESULT, c_int, POINTER(c_void_p))
		setop = vcall(context, 7, HRESULT, c_int, c_void_p)
		cnt = c_void_p()
		get(context, ids[0], byref(cnt))
		count = unbox_int(cnt)
		r = setop(context, ids[1], box_int(count * 10))
		LOG.append(f"target CallExtension({extensionId}, operandIds={ids}): read count {count}, wrote {count * 10}, SetOperand hr={r}")
		return hresult.S_OK


class Provider(COMObject):
	_com_interfaces_ = [UIA.IRawElementProviderSimple, ICoreAutomationRemoteOperationExtensionProvider]

	def __init__(self, hwnd):
		super().__init__()
		self.hwnd = hwnd

	def IRawElementProviderSimple__get_ProviderOptions(self, this, pRetVal):
		pRetVal[0] = UIA.ProviderOptions_ServerSideProvider | UIA.ProviderOptions_UseComThreading
		return hresult.S_OK

	def IRawElementProviderSimple_GetPatternProvider(self, this, patternId, pRetVal):
		pRetVal[0] = None
		return hresult.S_OK

	def IRawElementProviderSimple_GetPropertyValue(self, this, propertyId, pRetVal):
		v = {UIA.UIA_NamePropertyId: "Stand-in document", UIA.UIA_ClassNamePropertyId: "_WwG",
			UIA.UIA_ControlTypePropertyId: UIA.UIA_DocumentControlTypeId}.get(propertyId)
		if v is not None:
			pRetVal[0].value = v
		return hresult.S_OK

	def IRawElementProviderSimple__get_HostRawElementProvider(self, this, pRetVal):
		p = POINTER(UIA.IRawElementProviderSimple)()
		uiac.UiaHostProviderFromHwnd(self.hwnd, byref(p))
		pRetVal[0] = p
		return hresult.S_OK

	def IInspectable_GetIids(self, this, count, iids):
		return hresult.E_NOTIMPL

	def IInspectable_GetRuntimeClassName(self, this, name):
		return hresult.E_NOTIMPL

	def IInspectable_GetTrustLevel(self, this, level):
		level[0] = 0
		return hresult.S_OK

	def ICoreAutomationRemoteOperationExtensionProvider_IsExtensionSupported(self, this, extensionId, result):
		LOG.append(f"provider IsExtensionSupported({extensionId}) on thread {k32.GetCurrentThreadId()}")
		result[0] = extensionId == G_PATTERN
		return hresult.S_OK

	def ICoreAutomationRemoteOperationExtensionProvider_CallExtension(self, this, extensionId, context, n, ids):
		ids = [ids[i] for i in range(n)]
		try:
			st.setdefault("targets", []).append(Target())
			tp = st["targets"][-1].QueryInterface(IInspectable)
			try:
				r = vcall(context, 7, HRESULT, c_int, c_void_p)(context, ids[0], ctypes.cast(tp, c_void_p))
			except OSError as e1:
				LOG.append(f"provider SetOperand(target) failed: {e1}; trying SetOperand2 with the extension provider IID")
				tp2 = st["targets"][-1].QueryInterface(ICoreAutomationConnectionBoundObjectProvider)
				r = vcall(context, 8, HRESULT, c_int, c_void_p, GUID)(context, ids[0], ctypes.cast(tp2, c_void_p), ICoreAutomationConnectionBoundObjectProvider._iid_)
		except Exception:
			import traceback
			LOG.append("provider CallExtension raised: " + traceback.format_exc()[-600:])
			return hresult.E_FAIL
		LOG.append(f"provider CallExtension({extensionId}, operandIds={ids}): SetOperand(target) hr={r}")
		return hresult.S_OK


st = {}


@WNDPROC
def wndproc(hwnd, msg, wp, lp):
	if msg == 0x3D and ctypes.c_long(lp & 0xFFFFFFFF).value == -25:  # WM_GETOBJECT, UiaRootObjectId
		LOG.append("WM_GETOBJECT UiaRootObjectId")
		p = st["provider"].QueryInterface(UIA.IRawElementProviderSimple)
		return uiac.UiaReturnRawElementProvider(hwnd, wp, lp, ctypes.cast(p, c_void_p))
	return u32.DefWindowProcW(hwnd, msg, wp, lp)


class WNDCLASSW(ctypes.Structure):
	_fields_ = [("style", w.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", c_int), ("cbWndExtra", c_int),
		("hInstance", w.HINSTANCE), ("hIcon", w.HICON), ("hCursor", c_void_p), ("hbrBackground", w.HBRUSH),
		("lpszMenuName", w.LPCWSTR), ("lpszClassName", w.LPCWSTR)]


def app():
	comtypes.CoInitialize()
	wc = WNDCLASSW(lpfnWndProc=wndproc, lpszClassName="_WwG")
	u32.RegisterClassW(byref(wc))
	h = u32.CreateWindowExW(0, "_WwG", "Stand-in", 0, 0, 0, 400, 300, None, None, None, None)
	st["hwnd"] = h
	st["provider"] = Provider(h)
	st["ready"].set()
	msg = w.MSG()
	while u32.GetMessageW(byref(msg), None, 0, 0) > 0:
		u32.TranslateMessage(byref(msg))
		u32.DispatchMessageW(byref(msg))


st["ready"] = threading.Event()
threading.Thread(target=app, daemon=True).start()
st["ready"].wait(5)
client = comtypes.CoCreateInstance(UIA.CUIAutomation8._reg_clsid_, interface=UIA.IUIAutomation, clsctx=comtypes.CLSCTX_INPROC_SERVER)
el = client.ElementFromHandle(st["hwnd"])
print("element:", el.CurrentName, el.CurrentClassName, "main thread", k32.GetCurrentThreadId())

op = operation.Operation()


@op.buildFunction
def code(ra: remoteAPI.RemoteAPI):
	e = ra.newElement(el)
	sup = e.isExtensionSupported(G_PATTERN)
	other = e.isExtensionSupported(GUID("{F39655AC-133A-435B-A318-C197F0D3D203}"))
	res = ra.newVariant()
	e.callExtension(G_PATTERN, res)
	tgt = res.asType(RemoteExtensionTarget)
	moveSup = tgt.isExtensionSupported(G_MOVE)
	count = ra.newInt(3)
	moved = ra.newInt(0)
	tgt.callExtension(G_MOVE, count, moved)
	ra.Return(sup, other, res.isNull(), moveSup, moved)


t0 = time.perf_counter()
try:
	print("remote operation result (pattern supported, other supported, result is null, target supports move, moved):", op.execute(), f"in {1000 * (time.perf_counter() - t0):.1f} ms")
except Exception as ex:
	import traceback
	traceback.print_exc(file=sys.stdout)
print("=== provider log:")
for l in LOG:
	print("  ", l)
sys.stdout.flush()
os._exit(0)
