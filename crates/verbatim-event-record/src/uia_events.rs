//! What the recorder registers with UI Automation, and each event as a
//! line. Every callback only reads the element's cache: no cross-process
//! call, and no call into a provider that has gone.

use std::sync::Arc;

use serde_json::{Value, json};
use verbatim_uia::{
    Cached, FOCUS_PROPERTIES, Part, RecordedEvent, Subscription, variant_f64, variant_i32,
    variant_optional_bool, variant_string,
};
use verbatim_win32::window::{focus_window, top_level_of};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VT_BOOL, VT_BSTR, VT_EMPTY, VT_I4, VT_R8, VT_UNKNOWN,
};
use windows::Win32::UI::Accessibility::{
    StructureChangeType, StructureChangeType_ChildAdded, StructureChangeType_ChildRemoved,
    StructureChangeType_ChildrenBulkAdded, StructureChangeType_ChildrenBulkRemoved,
    StructureChangeType_ChildrenInvalidated, StructureChangeType_ChildrenReordered,
    UIA_AsyncContentLoadedEventId, UIA_ControllerForPropertyId, UIA_Drag_DragCancelEventId,
    UIA_Drag_DragCompleteEventId, UIA_Drag_DragStartEventId, UIA_DropTarget_DragEnterEventId,
    UIA_DropTarget_DragLeaveEventId, UIA_DropTarget_DroppedEventId, UIA_EVENT_ID,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_HelpTextPropertyId,
    UIA_IsEnabledPropertyId, UIA_ItemStatusPropertyId, UIA_LayoutInvalidatedEventId,
    UIA_LiveRegionChangedEventId, UIA_MenuClosedEventId, UIA_MenuOpenedEventId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_RangeValueValuePropertyId,
    UIA_SelectionItem_ElementAddedToSelectionEventId,
    UIA_SelectionItem_ElementRemovedFromSelectionEventId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SystemAlertEventId, UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId, UIA_ToggleToggleStatePropertyId,
    UIA_ToolTipClosedEventId, UIA_ToolTipOpenedEventId, UIA_ValueValuePropertyId,
    UIA_Window_WindowClosedEventId, UIA_Window_WindowOpenedEventId,
};

use crate::behind::Focus;
use crate::line::EventData;
use crate::target::Target;
use crate::writer::{Arrivals, EventArrival, Message};

/// The events registered desktop-wide beside focus, so that a new window's
/// first event is not missed: only the target's are written.
const DESKTOP_EVENTS: [UIA_EVENT_ID; 3] = [
    UIA_MenuOpenedEventId,
    UIA_Window_WindowOpenedEventId,
    UIA_ToolTipOpenedEventId,
];

/// The automation events registered on the target's windows.
const TARGET_EVENTS: [UIA_EVENT_ID; 18] = [
    UIA_SelectionItem_ElementSelectedEventId,
    UIA_SelectionItem_ElementAddedToSelectionEventId,
    UIA_SelectionItem_ElementRemovedFromSelectionEventId,
    UIA_MenuClosedEventId,
    UIA_ToolTipClosedEventId,
    UIA_Window_WindowClosedEventId,
    UIA_LiveRegionChangedEventId,
    UIA_SystemAlertEventId,
    UIA_LayoutInvalidatedEventId,
    UIA_Drag_DragStartEventId,
    UIA_Drag_DragCancelEventId,
    UIA_Drag_DragCompleteEventId,
    UIA_DropTarget_DragEnterEventId,
    UIA_DropTarget_DragLeaveEventId,
    UIA_DropTarget_DroppedEventId,
    UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
    UIA_AsyncContentLoadedEventId,
];

/// The properties recorded on the target's windows, beyond
/// [`FOCUS_PROPERTIES`].
const EXTRA_PROPERTIES: [UIA_PROPERTY_ID; 2] = [
    UIA_ItemStatusPropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
];

/// What every UI Automation callback shares.
#[derive(Clone)]
pub(crate) struct UiaEvents {
    arrivals: Arc<Arrivals>,
    target: Target,
}

impl UiaEvents {
    /// The recording's callbacks, for `target`.
    pub(crate) fn new(arrivals: Arc<Arrivals>, target: Target) -> Self {
        Self { arrivals, target }
    }

    /// Whether the element is the target's: its process, or in window
    /// mode the root of its own window, or of the keyboard focus window
    /// for an element with none. Local, from the cache and `GetAncestor`.
    fn in_target(&self, fields: &RecordedEvent) -> bool {
        match self.target {
            Target::Process(pid) => fields.process_id == Some(pid),
            Target::Window { hwnd, .. } => {
                let own = if fields.hwnd == 0 {
                    focus_window().unwrap_or(0)
                } else {
                    fields.hwnd
                };
                top_level_of(own) == hwnd
            }
        }
    }

    /// Writes one UI Automation event as a line.
    fn record(
        &self,
        kind: &'static str,
        fields: &RecordedEvent,
        (property, value): (Option<String>, Option<Value>),
        focus: Option<Focus>,
    ) {
        let in_target = self.in_target(fields);
        let window = if fields.hwnd == 0 {
            focus_window()
        } else {
            Some(fields.hwnd)
        };
        let data = EventData {
            kind,
            in_target,
            pid: fields.process_id,
            window,
            runtime_id: fields.runtime_id.clone(),
            name: fields.name.clone(),
            control_type: fields.control_type.map(control_type_name),
            class: fields.class_name.clone(),
            automation_id: fields.automation_id.clone(),
            framework: fields.framework_id.clone(),
            property,
            value,
            is_focus: Some(fields.has_focus),
            event: None,
            object: None,
            child: None,
        };
        self.arrivals.arrive(Message::Event(Box::new(EventArrival {
            data,
            focus,
            raised_ms_ago: None,
        })));
    }
}

/// Registers desktop-wide: the focus handler, written for every process,
/// and the three events that start a new window, written only for the
/// target's.
pub(crate) fn desktop_parts(events: &UiaEvents) -> Vec<Part> {
    let focus_events = events.clone();
    let opened_events = events.clone();
    vec![
        Part::shaped::<RecordedEvent>(Subscription::Focus {
            callback: Arc::new(move |element: &Cached<RecordedEvent>| {
                let fields = element.read();
                let focus = fields
                    .process_id
                    .filter(|_| fields.hwnd != 0)
                    .map(|process| Focus::Uia {
                        process,
                        window: fields.hwnd,
                    });
                focus_events.record("uia_focus", &fields, (None, None), focus);
            }),
        }),
        Part::shaped::<RecordedEvent>(Subscription::Events {
            events: DESKTOP_EVENTS.to_vec(),
            callback: Arc::new(move |element: &Cached<RecordedEvent>, event| {
                let fields = element.read();
                if opened_events.in_target(&fields) {
                    opened_events.record(
                        "uia_event",
                        &fields,
                        (Some(event_name(event).to_owned()), None),
                        None,
                    );
                }
            }),
        }),
    ]
}

/// Registers on the target's windows: everything recorded of them but
/// focus and the three desktop-wide events.
pub(crate) fn target_parts(events: &UiaEvents) -> Vec<Part> {
    let properties: Vec<UIA_PROPERTY_ID> = FOCUS_PROPERTIES
        .iter()
        .chain(&EXTRA_PROPERTIES)
        .copied()
        .collect();
    let [
        for_properties,
        for_structure,
        for_events,
        for_notifications,
        for_position,
    ] = [events, events, events, events, events].map(Clone::clone);
    vec![
        Part::shaped::<RecordedEvent>(Subscription::PropertyValues {
            properties,
            callback: Arc::new(
                move |element: &Cached<RecordedEvent>, property, value: &VARIANT| {
                    for_properties.record(
                        "uia_property",
                        &element.read(),
                        (Some(property_name(property)), value_of(value)),
                        None,
                    );
                },
            ),
        }),
        Part::shaped::<RecordedEvent>(Subscription::Structure {
            callback: Arc::new(
                move |element: &Cached<RecordedEvent>, change: StructureChangeType| {
                    for_structure.record(
                        "uia_structure",
                        &element.read(),
                        (None, Some(json!(structure_name(change)))),
                        None,
                    );
                },
            ),
        }),
        Part::shaped::<RecordedEvent>(Subscription::Events {
            events: TARGET_EVENTS.to_vec(),
            callback: Arc::new(move |element: &Cached<RecordedEvent>, event| {
                for_events.record(
                    "uia_event",
                    &element.read(),
                    (Some(event_name(event).to_owned()), None),
                    None,
                );
            }),
        }),
        Part::shaped::<RecordedEvent>(Subscription::Notifications {
            callback: Arc::new(
                move |element: &Cached<RecordedEvent>,
                      _kind,
                      _processing,
                      display: Option<String>,
                      activity: Option<String>| {
                    for_notifications.record(
                        "uia_event",
                        &element.read(),
                        (
                            Some("Notification".to_owned()),
                            display.or(activity).map(Value::from),
                        ),
                        None,
                    );
                },
            ),
        }),
        Part::shaped::<RecordedEvent>(Subscription::ActiveTextPosition {
            callback: Arc::new(move |element: &Cached<RecordedEvent>, _range| {
                for_position.record(
                    "uia_event",
                    &element.read(),
                    (Some("ActiveTextPositionChanged".to_owned()), None),
                    None,
                );
            }),
        }),
    ]
}

/// A property change's new value, null for an element-valued property.
fn value_of(value: &VARIANT) -> Option<Value> {
    let vt: VARENUM = value.vt();
    if vt == VT_EMPTY || vt == VT_UNKNOWN {
        None
    } else if vt == VT_BOOL {
        variant_optional_bool(value).map(Value::from)
    } else if vt == VT_I4 {
        variant_i32(value).map(Value::from)
    } else if vt == VT_R8 {
        variant_f64(value).map(Value::from)
    } else if vt == VT_BSTR {
        Some(Value::from(variant_string(value).unwrap_or_default()))
    } else {
        variant_string(value).map(Value::from)
    }
}

/// The name of a UI Automation control type id, or its number for one
/// this table has not heard of.
fn control_type_name(id: i32) -> String {
    const NAMES: [&str; 41] = [
        "Button",
        "Calendar",
        "CheckBox",
        "ComboBox",
        "Edit",
        "Hyperlink",
        "Image",
        "ListItem",
        "List",
        "Menu",
        "MenuBar",
        "MenuItem",
        "ProgressBar",
        "RadioButton",
        "ScrollBar",
        "Slider",
        "Spinner",
        "StatusBar",
        "Tab",
        "TabItem",
        "Text",
        "ToolBar",
        "ToolTip",
        "Tree",
        "TreeItem",
        "Custom",
        "Group",
        "Thumb",
        "DataGrid",
        "DataItem",
        "Document",
        "SplitButton",
        "Window",
        "Pane",
        "Header",
        "HeaderItem",
        "Table",
        "TitleBar",
        "Separator",
        "SemanticZoom",
        "AppBar",
    ];
    usize::try_from(id - 50000)
        .ok()
        .and_then(|index| NAMES.get(index))
        .map_or_else(|| id.to_string(), |name| (*name).to_owned())
}

/// The name a line gives property `id`, its number for one not in the table.
fn property_name(id: i32) -> String {
    let property = UIA_PROPERTY_ID(id);
    let name = if property == UIA_NamePropertyId {
        "Name"
    } else if property == UIA_ValueValuePropertyId {
        "Value"
    } else if property == UIA_RangeValueValuePropertyId {
        "RangeValue"
    } else if property == UIA_ToggleToggleStatePropertyId {
        "ToggleState"
    } else if property == UIA_IsEnabledPropertyId {
        "IsEnabled"
    } else if property == UIA_ExpandCollapseExpandCollapseStatePropertyId {
        "ExpandCollapseState"
    } else if property == UIA_HelpTextPropertyId {
        "HelpText"
    } else if property == UIA_ControllerForPropertyId {
        "ControllerFor"
    } else if property == UIA_ItemStatusPropertyId {
        "ItemStatus"
    } else if property == UIA_SelectionItemIsSelectedPropertyId {
        "IsSelected"
    } else {
        return id.to_string();
    };
    name.to_owned()
}

/// The name a line gives automation event `id`.
fn event_name(id: i32) -> &'static str {
    const NAMES: [(UIA_EVENT_ID, &str); 21] = [
        (UIA_MenuOpenedEventId, "MenuOpened"),
        (UIA_MenuClosedEventId, "MenuClosed"),
        (UIA_Window_WindowOpenedEventId, "WindowOpened"),
        (UIA_Window_WindowClosedEventId, "WindowClosed"),
        (UIA_ToolTipOpenedEventId, "ToolTipOpened"),
        (UIA_ToolTipClosedEventId, "ToolTipClosed"),
        (UIA_SelectionItem_ElementSelectedEventId, "ElementSelected"),
        (
            UIA_SelectionItem_ElementAddedToSelectionEventId,
            "ElementAddedToSelection",
        ),
        (
            UIA_SelectionItem_ElementRemovedFromSelectionEventId,
            "ElementRemovedFromSelection",
        ),
        (UIA_LiveRegionChangedEventId, "LiveRegionChanged"),
        (UIA_SystemAlertEventId, "SystemAlert"),
        (UIA_LayoutInvalidatedEventId, "LayoutInvalidated"),
        (UIA_Drag_DragStartEventId, "DragStart"),
        (UIA_Drag_DragCancelEventId, "DragCancel"),
        (UIA_Drag_DragCompleteEventId, "DragComplete"),
        (UIA_DropTarget_DragEnterEventId, "DropTargetDragEnter"),
        (UIA_DropTarget_DragLeaveEventId, "DropTargetDragLeave"),
        (UIA_DropTarget_DroppedEventId, "DropTargetDropped"),
        (UIA_Text_TextChangedEventId, "TextChanged"),
        (UIA_Text_TextSelectionChangedEventId, "TextSelectionChanged"),
        (UIA_AsyncContentLoadedEventId, "AsyncContentLoaded"),
    ];
    NAMES
        .iter()
        .find(|(event, _)| event.0 == id)
        .map_or("Unknown", |(_, name)| name)
}

/// A structure change's name.
fn structure_name(change: StructureChangeType) -> &'static str {
    if change == StructureChangeType_ChildAdded {
        "ChildAdded"
    } else if change == StructureChangeType_ChildRemoved {
        "ChildRemoved"
    } else if change == StructureChangeType_ChildrenInvalidated {
        "ChildrenInvalidated"
    } else if change == StructureChangeType_ChildrenBulkAdded {
        "ChildrenBulkAdded"
    } else if change == StructureChangeType_ChildrenBulkRemoved {
        "ChildrenBulkRemoved"
    } else if change == StructureChangeType_ChildrenReordered {
        "ChildrenReordered"
    } else {
        "Unknown"
    }
}
