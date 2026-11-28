import os, sys
sys.path.insert(0, sys.argv[1])
import comtypes.gen; comtypes.gen.__path__.insert(0, os.path.join(sys.argv[1], "comInterfaces"))
import comInterfaces.IAccessible2Lib as IA2
import comtypes.client
Acc = comtypes.client.GetModule("oleacc.dll")
for itf in [Acc.IAccessible, IA2.IAccessible2, IA2.IAccessible2_2, IA2.IAccessibleText, IA2.IAccessibleHypertext, IA2.IAccessibleHyperlink, IA2.IAccessibleAction, IA2.IAccessibleApplication, IA2.IAccessibleTable2, IA2.IAccessibleTableCell]:
    print(itf.__name__, [ (m.name, [f[0] for f in (m.paramflags or ())]) for m in itf._methods_])
print([k for k in dir(IA2) if k.startswith("IA2_ROLE")][:80])
