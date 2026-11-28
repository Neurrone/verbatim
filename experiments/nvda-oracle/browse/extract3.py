import os, sys
pkg = os.path.join(sys.argv[1], "comInterfaces")
sys.path.insert(0, sys.argv[1])
import comtypes.gen; comtypes.gen.__path__.insert(0, pkg)
import comInterfaces.IAccessible2Lib as IA2
print([k for k in dir(IA2) if k.startswith("IAccessible")])
print(IA2.IA2_ROLE_HEADING, IA2.IA2_ROLE_LANDMARK, IA2.IA2_ROLE_PARAGRAPH, IA2.IA2_ROLE_SECTION)
print([m for m in IA2.IAccessible2._methods_][:3])
