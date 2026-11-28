"""Headless NVDA bootstrap after nvda/tests/unit/__init__.py, with stubs for what needs a build or a desktop.
Nothing is written under nvda/ (bytecode off, config path in this folder)."""
import sys, os, types, importlib.abc, importlib.machinery, builtins, logging, traceback
from unittest import mock

sys.dont_write_bytecode = True
SRC = r"<repository>\nvda\source"
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, SRC)

# Modules replaced wholesale: native DLLs not built in this checkout, wx GUI, braille tables, audio output.
STUBS = {
	"wx",  # GUI toolkit, not installed
	"louis",  # liblouis braille, built by scons
	"serial",  # pyserial, braille hardware
}
if len(sys.argv) > 1 and sys.argv[1] == "--more":
	STUBS |= set(sys.argv[2].split(","))


class StubModule(types.ModuleType):
	def __getattr__(self, name):
		if name.startswith("__") and name.endswith("__"):
			raise AttributeError(name)
		if name[:1].isupper() and not name.isupper():
			# A class name: give a real class, so NVDA code can subclass it.
			m = type(name, (), {
				"__init__": lambda self, *a, **k: None,
				"__getattr__": lambda self, n: mock.MagicMock(name=n),
			})
		else:
			m = mock.MagicMock(name=f"{self.__name__}.{name}")
		setattr(self, name, m)
		return m


class StubFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
	def find_spec(self, fullname, path, target=None):
		if fullname in STUBS or fullname.split(".")[0] in {s for s in STUBS if "." not in s}:
			return importlib.machinery.ModuleSpec(fullname, self, is_package=True)
		return None

	def create_module(self, spec):
		m = StubModule(spec.name)
		m.__path__ = []
		return m

	def exec_module(self, module):
		pass


sys.meta_path.insert(0, StubFinder())

builtins._ = lambda s: s
builtins.pgettext = lambda c, s: s
builtins.ngettext = lambda a, b, n: a if n == 1 else b
builtins.npgettext = lambda c, a, b, n: a if n == 1 else b

import globalVars
import NVDAState
if os.environ.get("NVDA_LIB"):
	NVDAState._ReadPaths.coreArchLibPath = property(lambda self: os.environ["NVDA_LIB"])
else:
	STUBS |= {"NVDAHelper.localLib", "NVDAHelper.localWin10", "wasapi"}

globalVars.appDir = SRC
globalVars.appArgs.configPath = os.path.join(HERE, "userconfig")
globalVars.appArgs.disableAddons = True
os.makedirs(globalVars.appArgs.configPath, exist_ok=True)

from logHandler import log

log.addHandler(logging.NullHandler())
log.setLevel(100)

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
import types as _T

_IA = _T.ModuleType("NVDAObjects.IAccessible")  # needs generated comInterfaces; only isinstance checks reach it here
_IA.IAccessible = type("IAccessible", (), {})
sys.modules["NVDAObjects.IAccessible"] = _IA
print("bootstrap OK; stubs:", sorted(STUBS))
