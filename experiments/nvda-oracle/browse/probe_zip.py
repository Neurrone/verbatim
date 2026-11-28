import zipfile, comtypes.typeinfo as ti
z = zipfile.ZipFile(r"C:\Program Files\NVDA\library.zip")
names = [n for n in z.namelist() if "comInterfaces" in n or "IAccessible2" in n]
print(len(names)); print("\n".join(names[:60]))
for p in [r"C:\Program Files\Mozilla Firefox\AccessibleMarshal.dll"]:
    try:
        t = ti.LoadTypeLibEx(p); print(p, t.GetTypeInfoCount())
    except Exception as e: print(p, e)
