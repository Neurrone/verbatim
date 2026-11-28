"""Property defaults shared by the worlds in this folder (after ../uia/world_explorer.py)."""
from standins import NS
from comInterfaces import UIAutomationClient as UIA

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
	"BoundingRectangle": (100.0, 100.0, 400.0, 300.0),
	"LiveSetting": 0,
}


def common(ct, name, cls, fw, hwnd=0, pid=0, **extra):
	p = {
		"ControlType": ct,
		"Name": name,
		"ClassName": cls,
		"FrameworkId": fw,
		"AutomationId": extra.pop("AutomationId", ""),
		"NativeWindowHandle": hwnd,
		"ProcessId": pid,
		"ProviderDescription": extra.pop("ProviderDescription", "[pid:%d,providerId:0x0 Main:Unidentified Provider]" % pid),
		"IsTextPatternAvailable": extra.pop("IsTextPatternAvailable", False),
		"IsContentElement": True,
		"IsControlElement": True,
		"LocalizedControlType": extra.pop("LocalizedControlType", ""),
		"HasKeyboardFocus": False,
		"RuntimeId": extra.pop("RuntimeId"),
	}
	p.update(STATE_DEFAULTS)
	p.update(extra)
	return p
