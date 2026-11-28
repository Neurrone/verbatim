"""Build ia2.tlb from the IAccessible2 IDL at NVDA's pinned commit (c9ae003d, nvda/.gitmodules
include/ia2; the submodule is not checked out, so the files were fetched into ia2-src/), merged as
nvda/nvdaHelper/ia2_sconscript does, compiled with midl from VS 2026's x64 tools, then wrapped by
comtypes.client.GetModule as nvda/source/comInterfaces_sconscript does (IAccessible2Lib.py).

Usage: python build_ia2.py"""
import os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "ia2-src")
OUT = os.path.join(HERE, "ia2-build")
os.makedirs(OUT, exist_ok=True)
FILES = """api_all_headers.idl IA2CommonTypes.idl AccessibleRelation.idl AccessibleAction.idl AccessibleRole.idl
AccessibleStates.idl Accessible2.idl Accessible2_2.idl AccessibleComponent.idl AccessibleValue.idl AccessibleText.idl
AccessibleText2.idl AccessibleEditableText.idl AccessibleHyperlink.idl AccessibleHypertext.idl AccessibleHypertext2.idl
AccessibleTable.idl AccessibleTable2.idl AccessibleTableCell.idl AccessibleImage.idl AccessibleEventID.idl
AccessibleApplication.idl AccessibleDocument.idl AccessibleTextSelectionContainer.idl IA2TypeLibrary.idl""".split()
RE_IDL_IMPORT = re.compile(r'import "[A-Z].*$', re.MULTILINE)
merged = os.path.join(OUT, "ia2.idl")
with open(merged, "w") as out:
	out.write(open(os.path.join(SRC, FILES[0])).read() + "\n")
	for f in FILES[1:]:
		out.write(RE_IDL_IMPORT.sub("", open(os.path.join(SRC, f)).read()) + "\n")

VCVARS = r"C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat"
bat = os.path.join(OUT, "midl.bat")
with open(bat, "w") as f:
	f.write(f'@call "{VCVARS}" >nul\r\ncd /d "{OUT}"\r\nmidl /nologo /tlb ia2.tlb /h ia2.h /iid ia2_i.c /proxy ia2_p.c /dlldata dlldata.c ia2.idl\r\n')
r = subprocess.run(["cmd", "/c", bat], capture_output=True, text=True)
print(r.stdout[-3000:], r.stderr[-3000:])
assert os.path.exists(os.path.join(OUT, "ia2.tlb")), "midl gave no type library"

# comtypes writes its wrappers into comtypes.gen; NVDA's build copies them to comInterfaces.
import comtypes.client

mod = comtypes.client.GetModule(os.path.join(OUT, "ia2.tlb"))
print("generated", mod.__name__, mod.__file__, len([n for n in dir(mod) if n.startswith("IAccessible")]), "IAccessible* names")
