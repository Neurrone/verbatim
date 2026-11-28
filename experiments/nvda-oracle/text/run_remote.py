"""UIA remote operations against the stand-in text model.

NVDA builds remote operations as bytecode (UIAHandler._remoteOps) and runs them through
UIARemote.dll in the provider's process, which needs real UIA proxies. Its own LocalExecutor runs
the same bytecode in-process, instruction by instruction, against the client interfaces, so the
stand-ins can serve it. This compares the two heading paths UIAHandler.browseMode takes
(remote.findFirstHeadingInTextRange / collectAllHeadingsInTextRange when remote.isSupported(),
the iterUIARangeByUnit paragraph walk otherwise), and shows what UIAHandler.remote.initialize does.

Usage: python run_remote.py"""
import sys, os

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import boot  # noqa: F401
import standins as S
import textmodel as T
from comInterfaces import UIAutomationClient as UIA

import UIAHandler.remote as remote
from UIAHandler._remoteOps import operation
from UIAHandler._remoteOps.localExecute import LocalExecutor
from UIAHandler.utils import iterUIARangeByUnit
import winVersion

print("before initialize: isSupported =", remote.isSupported())
remote.initialize(True, None)
print("after the real initialize (it only compares the Windows version):", remote.isSupported(), "on", winVersion.getWinVer())

text = "Intro paragraph\rHeading one\rBody text here\rSub heading\rMore body\r\r"
runs = []
for label, sid in (("Heading one", UIA.StyleId_Heading1), ("Sub heading", UIA.StyleId_Heading2)):
	i = text.index(label)
	runs.append((i, i + len(label) + 1, {"StyleId": sid}))
model = T.TextModel(text, runs=runs, defaults={"StyleId": UIA.StyleId_Normal}, final_mark=True)
docRange = T.make_range(model, 0, model.N)

# LocalExecutor in place of RemoteExecutor: the same bytecode, run in-process on the stand-ins.
operation.Operation._executorClass = LocalExecutor

remoteAll = [(lvl, lab, S.lookup(r).start) for lvl, lab, r in remote.collectAllHeadingsInTextRange(docRange)]
first = remote.findFirstHeadingInTextRange(T.make_range(model, 0, model.N), None, False)
firstL2 = remote.findFirstHeadingInTextRange(T.make_range(model, 0, model.N), 2, False)


def fallback(wanted=None):
	out = []
	for sub in iterUIARangeByUnit(T.make_range(model, 0, model.N), UIA.TextUnit_Paragraph):
		v = sub.getAttributeValue(UIA.UIA_StyleIdAttributeId)
		if isinstance(v, int) and UIA.StyleId_Heading1 <= v <= UIA.StyleId_Heading9:
			lvl = v - UIA.StyleId_Heading1 + 1
			if not wanted or wanted == lvl:
				out.append((lvl, sub.GetText(-1), S.lookup(sub).start))
	return out


print("remote (local executor) all headings:", remoteAll)
print("fallback paragraph walk all headings:", fallback())
print("remote first heading from the start:", first and (first[0], first[1], S.lookup(first[2]).start))
print("remote first level-2 heading:", firstL2 and (firstL2[0], firstL2[1], S.lookup(firstL2[2]).start))
print("same headings and positions:", [(a, c) for a, b, c in remoteAll] == [(a, c) for a, b, c in fallback()])
print("=== misses", S.MISSES)
calls = {k: v for k, v in S.CALLS.items() if "TextRange" in k}
print("=== text range calls (both paths):", calls)
sys.stdout.flush()
os._exit(0)
