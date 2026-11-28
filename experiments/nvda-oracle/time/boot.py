"""Headless NVDA bootstrap, copied from ../uia/boot.py; changed: wx is the simulated loop from sim.py,
installed with simulated time and threads before NVDA is imported.

Original notes: headless NVDA bootstrap for the UIA-layer oracle.

After ../oracle2.py, with three changes: NVDA's own comtypes monkey patches are applied first,
as nvda.pyw does; comInterfaces.UIAutomationClient and comInterfaces.Accessibility are the
comtypes modules generated from the system's UIAutomationCore.dll and oleacc.dll (NVDA's build
generates the same with comtypes.client.GetModule); and nothing UIA-related is stubbed.
Nothing is written under nvda/ (bytecode off, config path in the scratchpad)."""
import sys, os, types, importlib.abc, importlib.machinery, builtins, logging
from unittest import mock

sys.dont_write_bytecode = True
SRC = r"<repository>\nvda\source"
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, SRC)

# Modules replaced wholesale: GUI, braille, hardware, and COM libraries off this path.
STUBS = {
	"louis", "cryptography", "cffi", "pycaw", "serial", "markdown", "nh3", "bleak", "pyphen",  # third-party, not installed
	"gui", "brailleViewer", "screenCurtain", "_magnifier", "hwIo", "bdDetect", "braille", "brailleInput", "brailleTables",  # GUI and braille output
	"comInterfaces.IAccessible2Lib", "comInterfaces.tom", "comInterfaces.SpeechLib",
}


class StubModule(types.ModuleType):
	def __getattr__(self, name):
		if name.startswith("__") and name.endswith("__"):
			raise AttributeError(name)
		if name[:1].isupper() and not name.isupper():
			m = type(name, (), {
				"__init__": lambda self, *a, **k: None,
				"__getattr__": lambda self, n: mock.MagicMock(name=n),
			})
		else:
			m = mock.MagicMock(name=f"{self.__name__}.{name}")
		setattr(self, name, m)
		return m


def _stubbed(fullname):
	return fullname.startswith("wx.") or any(fullname == s or fullname.startswith(s + ".") for s in STUBS)


class StubFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
	def find_spec(self, fullname, path, target=None):
		if _stubbed(fullname):
			return importlib.machinery.ModuleSpec(fullname, self, is_package=True)
		return None

	def create_module(self, spec):
		m = StubModule(spec.name)
		m.__path__ = []
		return m

	def exec_module(self, module):
		pass


sys.meta_path.insert(0, StubFinder())

# Simulated time, threads and wx before any NVDA module binds a clock or a primitive.
import sim
WX_MODULE = sim.install(StubModule)

builtins._ = lambda s: s
builtins.pgettext = lambda c, s: s
builtins.ngettext = lambda a, b, n: a if n == 1 else b
builtins.npgettext = lambda c, a, b, n: a if n == 1 else b

import globalVars
import NVDAState
NVDAState._ReadPaths.coreArchLibPath = property(lambda self: os.environ["NVDA_LIB"])

globalVars.appDir = SRC
globalVars.appArgs.configPath = os.path.join(HERE, "userconfig")
globalVars.appArgs.disableAddons = True
os.makedirs(globalVars.appArgs.configPath, exist_ok=True)
# NVDA's comtypes patches, before any comtypes import (nvda.pyw does this first).
import monkeyPatches.comtypesMonkeyPatches as _cmp
_cmp.applyMonkeyPatches()
# The patched COM pointer finalizer imports garbageHandler, which needs config imported first.
from logHandler import log
import config, garbageHandler  # noqa: E401

import comtypes.client
_UIA = comtypes.client.GetModule("UIAutomationCore.dll")
_ACC = comtypes.client.GetModule("oleacc.dll")
import comInterfaces
sys.modules["comInterfaces.UIAutomationClient"] = _UIA
sys.modules["comInterfaces.Accessibility"] = _ACC
comInterfaces.UIAutomationClient = _UIA
comInterfaces.Accessibility = _ACC
# Import order as NVDA's own startup reaches it: nvwave before speech, else a circular import fails.
pass


from logHandler import log

LOGLINES = []


class _Collect(logging.Handler):
	def emit(self, record):
		msg = f"{record.levelname} {record.name}: {record.getMessage()}"
		if record.exc_info:
			import traceback
			msg += "\n" + "".join(traceback.format_exception(*record.exc_info))[-1500:]
		LOGLINES.append(msg)


log.addHandler(_Collect())
log.setLevel(logging.DEBUG if os.environ.get("ORACLE_DEBUG") else logging.WARNING)

import config

config.initialize()
import languageHandler

languageHandler.setLanguage("en")
import appModuleHandler

appModuleHandler.initialize()
import vision

vision.initialize()
import characterProcessing

characterProcessing.initialize()
import speech

speech.initialize()
import speechDictHandler, synthDriverHandler

speechDictHandler.initialize()
assert synthDriverHandler.setSynth("silence")

# NVDAObjects.IAccessible needs IAccessible2Lib, built from IDL by NVDA's build.
# Only class identity reaches it on this path (isinstance checks, explorer's imports).
_IA = types.ModuleType("NVDAObjects.IAccessible")
_IA.IAccessible = type("IAccessible", (), {})
_IA.List = type("List", (), {})
sys.modules["NVDAObjects.IAccessible"] = _IA
import NVDAObjects
NVDAObjects.IAccessible = _IA
