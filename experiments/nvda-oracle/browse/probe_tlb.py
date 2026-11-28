import sys, os
os.environ.setdefault("COMTYPES_CACHE","")
import comtypes.client, comtypes.typeinfo as ti
lib = os.environ["NVDA_LIB"]
for d in ["IAccessible2proxy.dll","ISimpleDOM.dll"]:
    p = os.path.join(lib,d)
    try:
        t = ti.LoadTypeLibEx(p)
        print(d, "typelib:", t.GetDocumentation(-1)[0], t.GetTypeInfoCount())
    except Exception as e:
        print(d, "no typelib", e)
import ctypes, ctypes.wintypes as w
# exports of nvdaHelperRemote / Local
import pefile
