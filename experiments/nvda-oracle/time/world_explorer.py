"""The recorded world for the Explorer capture (target/captures-2026-11-20/s2-explorer-forms-output/
f5-address-tab.jsonl, steps 4 to 6): Tab from the search box to "More options", then to "Details".

Built by hand. From the capture: each element's name, role, description and position. Assumed
(no recording of this window exists): the window handles and classes, the element classes and
automation ids, the unnamed containers, and every value UIA would give by default."""
from standins import Node, NS, Window, WinWorld
from comInterfaces import UIAutomationClient as UIA

PID, TID = 4321, 4400
DESKTOP, CABINET, BRIDGE = 0x10010, 0x2A0B12, 0x3B0C20


def common(key, ct, name, cls, fw, hwnd=0, pid=PID, **extra):
	"""The properties NVDA's base cache request and its focus and state reads ask for."""
	p = {
		"ControlType": ct,
		"Name": name,
		"ClassName": cls,
		"FrameworkId": fw,
		"AutomationId": extra.pop("AutomationId", ""),
		"NativeWindowHandle": hwnd,
		"ProcessId": pid,
		"ProviderDescription": "[pid:%d,providerId:0x0 Main(parent link):Unidentified Provider (unmanaged:Microsoft.UI.Xaml.dll)]" % pid,
		"IsTextPatternAvailable": False,
		"IsContentElement": True,
		"IsControlElement": True,
		"LocalizedControlType": extra.pop("LocalizedControlType", ""),
		"HasKeyboardFocus": False,
		"RuntimeId": extra.pop("RuntimeId"),
	}
	p.update(STATE_DEFAULTS)
	p.update(extra)
	return p


# What UIA gives for these properties on an element that sets none of them; NS(x) marks a pattern
# property the element does not support, x being the default UIA substitutes on a plain read.
STATE_DEFAULTS = {
	"IsKeyboardFocusable": True,
	"IsPassword": False,
	"IsSelectionItemPatternAvailable": False,
	"IsValuePatternAvailable": False,
	"IsEnabled": True,
	"SelectionCanSelectMultiple": NS(False),
	"IsOffscreen": False,
	"IsDataValidForForm": True,
	"IsRequiredForForm": False,
	"ValueIsReadOnly": NS(True),
	"ExpandCollapseExpandCollapseState": NS(UIA.ExpandCollapseState_LeafNode),
	"ToggleToggleState": NS(UIA.ToggleState_Off),
	"AnnotationTypes": NS(()),
	"DragIsGrabbed": NS(False),
	"ValueValue": NS(""),
	"RangeValueValue": NS(0.0),
	"GridItemRow": NS(0),
	"GridItemColumn": NS(0),
	"GridItemRowSpan": NS(1),
	"GridItemColumnSpan": NS(1),
	"GridRowCount": NS(0),
	"GridColumnCount": NS(0),
	"TableItemRowHeaderItems": NS(None),
	"TableItemColumnHeaderItems": NS(None),
	"GridItemContainingGrid": NS(None),
	"IsDialog": False,
	"FullDescription": "",
	"HelpText": "",
	"AccessKey": "",
	"AcceleratorKey": "",
	"PositionInSet": 0,
	"SizeOfSet": 0,
	"Level": 0,
	"BoundingRectangle": (100.0, 100.0, 40.0, 30.0),
	"LiveSetting": 0,
}


# Patterns each element answers for: None is a recorded 'not supported'.
NO_PATTERNS = {UIA.UIA_TextPatternId: None}


def build():
	more = Node(
		"moreOptions",
		common(
			"moreOptions", UIA.UIA_ButtonControlTypeId, "More options", "AppBarButton", "XAML",
			LocalizedControlType="button", RuntimeId=(42, BRIDGE, 3, 101), AutomationId="MoreButton",
		),
	)
	details = Node(
		"details",
		common(
			"details", UIA.UIA_ButtonControlTypeId, "Details", "AppBarButton", "XAML",
			LocalizedControlType="button", RuntimeId=(42, BRIDGE, 3, 102), AutomationId="DetailsButton",
			HelpText="Show or hide the details pane.", PositionInSet=1, SizeOfSet=1,
		),
	)
	commandGroup = Node(
		"commandGroup",
		common("commandGroup", UIA.UIA_GroupControlTypeId, "", "CommandBar", "XAML",
			LocalizedControlType="group", RuntimeId=(42, BRIDGE, 3, 100)),
		children=[more, details],
	)
	search = Node(
		"search",
		common("search", UIA.UIA_EditControlTypeId, "Search verbatim-e2e-b9", "TextBox", "XAML",
			LocalizedControlType="edit", RuntimeId=(42, BRIDGE, 3, 51), AutomationId="SearchEditBox"),
	)
	headerGroup = Node(
		"headerGroup",
		common("headerGroup", UIA.UIA_GroupControlTypeId, "", "Grid", "XAML",
			LocalizedControlType="group", RuntimeId=(42, BRIDGE, 3, 50)),
		children=[search],
	)
	bridge = Node(
		"bridge",
		common("bridge", UIA.UIA_PaneControlTypeId, "", "Microsoft.UI.Content.DesktopChildSiteBridge", "Win32",
			hwnd=BRIDGE, LocalizedControlType="pane", RuntimeId=(42, BRIDGE)),
		children=[headerGroup, commandGroup],
	)
	cabinet = Node(
		"cabinet",
		common("cabinet", UIA.UIA_WindowControlTypeId, "verbatim-e2e-b9 - File Explorer", "CabinetWClass", "Win32",
			hwnd=CABINET, LocalizedControlType="window", RuntimeId=(42, CABINET)),
		children=[bridge],
	)
	root = Node(
		"root",
		common("root", UIA.UIA_PaneControlTypeId, "Desktop 1", "#32769", "Win32", hwnd=DESKTOP, pid=600,
			LocalizedControlType="pane", RuntimeId=(42, DESKTOP)),
		children=[cabinet],
	)
	windows = [
		Window(DESKTOP, "#32769", "", 0, 600, 604),
		Window(CABINET, "CabinetWClass", "verbatim-e2e-b9 - File Explorer", DESKTOP, PID, TID, uiaProvider=True),
		Window(BRIDGE, "Microsoft.UI.Content.DesktopChildSiteBridge", "", CABINET, PID, TID, style=0x56000000),
	]
	return root, {n.key: n for n in walk(root)}, WinWorld(windows, DESKTOP, CABINET, BRIDGE)


def walk(n):
	yield n
	for c in n.children:
		yield from walk(c)


PROCESSES = {PID: "explorer", 600: "csrss"}
