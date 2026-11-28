"""Feed a fixed fake object tree through NVDA's focus path; capture speech sequences."""
import sys, os

sys.argv = [sys.argv[0], "--more", "gui,brailleViewer,markdown,bleak,hwIo,bdDetect,braille,brailleInput,brailleTables,pyphen,nh3,comInterfaces,winBindings.oleacc"]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "oracle2.py")).read())

import controlTypes, api, eventHandler, queueHandler
import speech.speech as sp
from NVDAObjects import NVDAObject

spoken = []
sp.speak = lambda seq, symbolLevel=None, priority=None: spoken.append(list(seq))


import appModuleHandler, baseObject


class FakeApp(appModuleHandler.AppModule):
	def __init__(self, processID, appName):
		baseObject.ScriptableObject.__init__(self)
		self.processID = processID
		self.appName = appName
		self.processHandle = None
		self.helperLocalBindingHandle = None
		self._inprocRegistrationHandle = None


APP = FakeApp(40, "app")


class Fake(NVDAObject):
	"""A stand-in with fixed properties, as a recording would supply them."""

	processID = 40
	windowThreadID = 41
	appModule = APP
	treeInterceptor = None

	def __init__(self, key=None, role=None, name="", value=None, states=(), parent=None, description="", pos=None, children=()):
		super().__init__()
		self.key = key
		self.role = role
		self.name = name
		self.value = value
		self.states = set(states)
		self.parent = parent
		self.description = description
		self._pos = pos or {}
		self.firstChild = None

	def _get_positionInfo(self):
		return self._pos

	def _isEqual(self, other):
		return isinstance(other, Fake) and other.key == self.key

	def objectInForeground(self):
		return FOREGROUND


R, S = controlTypes.Role, controlTypes.State
desktop = Fake(key="desk", role=R.PANE, name="Desktop")
dialog = Fake(key="dlg", role=R.DIALOG, name="Save As", parent=desktop, description="")
group = Fake(key="grp", role=R.GROUPING, name="Options", parent=dialog)
check = Fake(key="chk", role=R.CHECKBOX, name="Hide extensions", states=[S.FOCUSABLE, S.FOCUSED, S.CHECKABLE, S.CHECKED], parent=group, pos={"indexInGroup": 2, "similarItemsInGroup": 5})
edit = Fake(key="edt", role=R.EDITABLETEXT, name="File name", value="report.txt", states=[S.FOCUSABLE, S.FOCUSED, S.EDITABLE], parent=dialog)
FOREGROUND = dialog

api.setDesktopObject(desktop)
api.setFocusObject(desktop)
api.setForegroundObject(desktop)


def run(label, fn):
	spoken.clear()
	fn()
	queueHandler.pumpAll()
	print(f"--- {label}")
	for s in spoken:
		print("   ", [str(x) if not isinstance(x, str) else x for x in s])


run("gainFocus edit in Save As dialog (foreground changes)", lambda: eventHandler.executeEvent("gainFocus", edit))
run("gainFocus checkbox in group (focusEntered group)", lambda: eventHandler.executeEvent("gainFocus", check))
check.states = check.states - {S.CHECKED}
run("stateChange on checkbox (unchecked)", lambda: eventHandler.executeEvent("stateChange", check))
run("getPropertiesSpeech direct", lambda: spoken.append(list(sp.getPropertiesSpeech(reason=controlTypes.OutputReason.FOCUS, role=R.BUTTON, name="OK", states={S.FOCUSABLE, S.PRESSED}))))

# Typing echo: NVDA's speakTypedCharacters with the default config.
run("focus edit, then typed characters h, i, space", lambda: [eventHandler.executeEvent("gainFocus", edit)] + [eventHandler.executeEvent("typedCharacter", edit, ch=c) for c in "hi "])

# Review cursor: NVDAObjectTextInfo over a fixed text, moved by line as review.py's scripts do.
import textInfos
from NVDAObjects import NVDAObjectTextInfo


class FakeDoc(Fake):
	TextInfo = NVDAObjectTextInfo

	def _get_basicText(self):
		return "first line\nsecond line\nthird"


doc = FakeDoc(key="doc", role=R.EDITABLETEXT, name="Notes", value="", parent=dialog)


def review():
	info = doc.makeTextInfo(textInfos.POSITION_FIRST)
	info.move(textInfos.UNIT_LINE, 1)
	info.expand(textInfos.UNIT_LINE)
	sp.speakTextInfo(info, unit=textInfos.UNIT_LINE, reason=controlTypes.OutputReason.CARET)
	c = info.copy()
	c.collapse()
	c.move(textInfos.UNIT_CHARACTER, 3)
	c.expand(textInfos.UNIT_CHARACTER)
	sp.speakTextInfo(c, unit=textInfos.UNIT_CHARACTER, reason=controlTypes.OutputReason.CARET)
	w = info.copy()
	w.collapse()
	w.expand(textInfos.UNIT_WORD)
	sp.speakTextInfo(w, unit=textInfos.UNIT_WORD, reason=controlTypes.OutputReason.CARET)


run("review: next line, a character, a word", review)

# Terminal output: the diff NVDA's LiveText applies between two snapshots.
import diffHandler

old = r"C:\> dir\nfile1\n"
new = r"C:\> dir\nfile1\nfile2\nC:\> "
print("--- dmp diff:", diffHandler.prefer_dmp().diff(new, old))
print("--- difflib diff:", diffHandler.prefer_difflib().diff(new, old))
