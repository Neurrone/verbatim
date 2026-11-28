import zipfile, os, sys
gen = sys.argv[1]
os.makedirs(os.path.join(gen,"ia2gen"), exist_ok=True)
z = zipfile.ZipFile(r"C:\Program Files\NVDA\library.zip")
for n in ["comInterfaces/IAccessible2Lib.pyc","comInterfaces/_CE3F726E_D1D3_44FE_B995_FF1DB3B48B2B_0_1_3.pyc"]:
    open(os.path.join(gen,"ia2gen",os.path.basename(n)),"wb").write(z.read(n))
print(z.read("comInterfaces/_CE3F726E_D1D3_44FE_B995_FF1DB3B48B2B_0_1_3.pyc")[:4])
import importlib.util; print(importlib.util.MAGIC_NUMBER)
import comtypes; print(comtypes.__version__)
