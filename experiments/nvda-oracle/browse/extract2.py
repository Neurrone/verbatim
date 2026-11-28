import zipfile, os, sys
pkg = os.path.join(sys.argv[1], "comInterfaces")
os.makedirs(pkg, exist_ok=True)
open(os.path.join(pkg,"__init__.py"),"w").write("")
z = zipfile.ZipFile(r"C:\Program Files\NVDA\library.zip")
for n in z.namelist():
    if n.startswith("comInterfaces/") and n != "comInterfaces/__init__.pyc":
        open(os.path.join(pkg, os.path.basename(n)),"wb").write(z.read(n))
sys.path.insert(0, sys.argv[1])
import comInterfaces.IAccessible2Lib as IA2
print([k for k in dir(IA2) if k.startswith("IAccessible")])
print(IA2.IA2_ROLE_HEADING, IA2.IA2_ROLE_LANDMARK, IA2.IA2_ROLE_PARAGRAPH, IA2.IA2_ROLE_SECTION)
