"""Classic Excel: nvdaHelperRemote's nvdaInProcUtils_excel_getCellInfos (execInThread onto the "Excel" thread,
then Excel's object model through IDispatch), reached by NVDA's own in-process RPC, against the stand-in."""
import os, sys, ctypes
from ctypes import byref, c_long

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import inproc
from comtypes import BSTR
import disp
import excelmodel as X
from NVDAHelper.localLib import EXCEL_CELLINFO

SHEET = X.Sheet({
	(1, 1): {"text": "Region", "font": {"Bold": True}},
	(1, 2): {"text": "Sales", "font": {"Bold": True}},
	(2, 1): {"text": "North and the northern territories combined", "comment": "Includes the islands"},
	(2, 2): {"text": "1,200", "formula": "=SUM(C2:F2)", "locked": False},
	(3, 1): {"text": "South", "link": True, "validation": (3, "Pick", "Choose a region")},
	(3, 2): {"text": "900"},
}, protect=True)
hwnd, am = inproc.start("EXCEL7", "Book1", "excel", lambda: (lambda: X.Window(SHEET)))
L = inproc.NVDAHelper.localLib
bh = am.helperLocalBindingHandle
ALL = 0xFFFF


def infos(address, count, flags=ALL):
	arr = (EXCEL_CELLINFO * count)()
	n = c_long()
	r = L.nvdaInProcUtils_excel_getCellInfos(bh, hwnd, BSTR(address), flags, count, arr, byref(n))
	return r, [{k: getattr(arr[i], k) for k, _ in EXCEL_CELLINFO._fields_} for i in range(n.value)]


for addr, count in (("$A$1", 1), ("$A$2", 1), ("$B$2", 1), ("$A$3", 1), ("$A$1:$B$3", 6)):
	r, cells = infos(addr, count)
	print(f"getCellInfos({addr}, {count}) -> {r}")
	for c in cells:
		print("   ", c)
print("=== members called by the native code:", len(disp.CALLS))
for k, v in sorted(disp.CALLS.items()):
	print("  ", k, v)
print("=== misses", dict(disp.MISSES))
print("=== NVDA log (after boot)")
for line in inproc.boot.LOGLINES[4:20]:
	print("  ", line[:500])
sys.stdout.flush()
os._exit(0)
