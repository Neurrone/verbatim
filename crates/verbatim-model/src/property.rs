//! The property table: every property a reader reads for Core, declared
//! once with its name, its type, how each interface reads it and its cost
//! (ADR 0003, "Properties, event kinds and requests").
//!
//! A read names a set of properties fixed at compile time: a struct that
//! [`property_set!`](crate::property_set) declares, with one typed field
//! per property of the set. The reader is sent the set's
//! [`PropertySet::PROPERTIES`], builds its cache request from them, and
//! answers [`PropertyValues`], which Core parses once, at the boundary,
//! into the set's struct ([`PropertySet::parse`]), so a rule reads only the
//! fields its read asked for and the compiler checks it. Extensions name
//! entries of the same table at run time ([`PropertyId::named`]) and read
//! [`PropertyValues`] as they are.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::request::HeldIds;
use crate::{CellPosition, Pid, Rect, Role, StateSet, TableCounts, WindowHandle};

/// One property of the table, as a request names it and an answer carries
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PropertyId {
    /// The accessible name.
    Name,
    /// The normalized role.
    Role,
    /// The value: a field's text, a slider's position, a combo box's
    /// selection.
    Value,
    /// The normalized states.
    States,
    /// The description, when it adds to the name.
    Description,
    /// The keyboard shortcut the object advertises.
    KeyboardShortcut,
    /// The one-based position in the containing set.
    PositionInSet,
    /// The size of the containing set.
    SetSize,
    /// The one-based nesting level.
    Level,
    /// The bounding rectangle in screen coordinates.
    Rect,
    /// The object's own class name: UI Automation's `ClassName`.
    ClassName,
    /// UI Automation's `AutomationId`.
    AutomationId,
    /// The window the object is, when it is one.
    NativeWindow,
    /// The process that serves the object.
    Process,
    /// UI Automation's runtime id.
    RuntimeId,
    /// Whether the object has the keyboard focus.
    HasKeyboardFocus,
    /// Whether the object has a text pattern.
    HasTextPattern,
    /// A range value's current value.
    RangeValue,
    /// Where the object is in a grid, when it is a grid's cell.
    Cell,
    /// A table's row and column counts.
    TableCounts,
    /// What an object's text says of it: its selected text, or else the
    /// line at its caret, or its first line when it has no caret, as
    /// NVDA's object speech reads an object's text content.
    TextSummary,
    /// Whether the object's value is its first selected item's name: a
    /// combo box without the `Value` pattern, or a looping selector, as
    /// NVDA's `ComboBoxWithoutValuePattern` reads it.
    ValueFromSelection,
    /// Whether the object reports suggestions opening and closing and they
    /// are open: absent when it reports none (neither editable text nor a
    /// search field); else whether its controller-for relation names any
    /// element, read live, as NVDA's `EditableTextWithSuggestions` reads
    /// `controllerFor` when it handles the change.
    Suggestions,
    /// Whether the object has UI Automation's `Value` pattern.
    HasValuePattern,
    /// The class name of the object's own window, or of its nearest
    /// ancestor's, as NVDA's `windowClassName` is: read from Windows with
    /// no call into the application.
    WindowClass,
    /// For a classic .NET Framework Windows Forms combo box (NVDA's
    /// `_NetFrameworkWinFormsComboBox`), the window of its drop-down list,
    /// by `GetComboBoxInfo`; absent for any other object.
    ComboBoxList,
    /// The automation id of the element's raw-view parent, which a UI
    /// Automation focus tell carries for the task switcher's item alone:
    /// no cache holds a parent's property, so the listener's check thread
    /// reads it with one call, as NVDA's File Explorer module reads the
    /// item's parent when it chooses the item's class
    /// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's
    /// pane").
    ParentAutomationId,
    /// How many children an expanded tree view item holds, which Core says
    /// as the focus newly expands, as NVDA's `SysTreeView32` item counts
    /// its children with the control's messages
    /// (`docs/nvda/speech.md`, "How many items an expanded tree view item
    /// holds").
    ChildCount,
    /// Whether the object, or an ancestor up to the desktop, has the
    /// focused state, by a walk of MSAA parents; absent for a UI
    /// Automation object, whose focus is read in full.
    FocusedWithin,
    /// The MSAA ids an object was reached at after NVDA's focus redirect,
    /// with the child id the reader holds it by; read with no call.
    MsaaAddress,
    /// Whether the object's own window, or its nearest ancestor's, is
    /// visible, read from Windows with no call into the application.
    WindowShown,
    /// Whether UI Automation counts the object both a control and a
    /// content element.
    IsContent,
}

impl PropertyId {
    /// This property's entry in the table.
    #[must_use]
    pub fn entry(self) -> &'static PropertyEntry {
        match self {
            Self::Name => &NAME,
            Self::Role => &ROLE,
            Self::Value => &VALUE,
            Self::States => &STATES,
            Self::Description => &DESCRIPTION,
            Self::KeyboardShortcut => &KEYBOARD_SHORTCUT,
            Self::PositionInSet => &POSITION_IN_SET,
            Self::SetSize => &SET_SIZE,
            Self::Level => &LEVEL,
            Self::Rect => &RECT,
            Self::ClassName => &CLASS_NAME,
            Self::AutomationId => &AUTOMATION_ID,
            Self::NativeWindow => &NATIVE_WINDOW,
            Self::Process => &PROCESS,
            Self::RuntimeId => &RUNTIME_ID,
            Self::HasKeyboardFocus => &HAS_KEYBOARD_FOCUS,
            Self::HasTextPattern => &HAS_TEXT_PATTERN,
            Self::RangeValue => &RANGE_VALUE,
            Self::Cell => &CELL,
            Self::TableCounts => &TABLE_COUNTS,
            Self::TextSummary => &TEXT_SUMMARY,
            Self::ValueFromSelection => &VALUE_FROM_SELECTION,
            Self::Suggestions => &SUGGESTIONS,
            Self::HasValuePattern => &HAS_VALUE_PATTERN,
            Self::WindowClass => &WINDOW_CLASS,
            Self::ComboBoxList => &COMBO_BOX_LIST,
            Self::ParentAutomationId => &PARENT_AUTOMATION_ID,
            Self::ChildCount => &CHILD_COUNT,
            Self::FocusedWithin => &FOCUSED_WITHIN,
            Self::MsaaAddress => &MSAA_ADDRESS,
            Self::WindowShown => &WINDOW_SHOWN,
            Self::IsContent => &IS_CONTENT,
        }
    }

    /// The property whose entry is named `name`, as an extension names it
    /// at run time; `None` for a name the table does not declare.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        PROPERTIES
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }
}

/// The type of a property's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueKind {
    /// A string.
    Text,
    /// A [`Role`].
    Role,
    /// A [`StateSet`].
    States,
    /// A count or a one-based position.
    Count,
    /// Yes or no.
    Flag,
    /// A [`Rect`].
    Rect,
    /// A [`WindowHandle`].
    Window,
    /// A [`Pid`].
    Process,
    /// A [`RuntimeId`].
    RuntimeId,
    /// A [`Number`].
    Number,
    /// A [`CellPosition`].
    Cell,
    /// A [`TableCounts`].
    TableCounts,
    /// A [`TextSummary`].
    TextSummary,
    /// A [`HeldIds`].
    MsaaAddress,
}

/// How much a property costs to read, which the bulk-read gate judges: a
/// read of many objects carries only cheap properties (architecture,
/// "Bulk reads carry only cheap properties").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Cost {
    /// Read among many as cheaply as alone.
    Cheap,
    /// Read only for an object read alone, since for some objects the
    /// application's own thread is asked for it.
    Alone,
}

/// How UI Automation reads a property: the properties cached for it, by
/// their UI Automation property ids, and how their values become the
/// property's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiaRead {
    /// The UI Automation property ids cached, in the order the conversion
    /// reads them.
    pub cached: &'static [i32],
    /// How the cached values become the property's value.
    pub conversion: UiaConversion,
}

/// How cached UI Automation values become a property's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiaConversion {
    /// The one string, absent when empty.
    Text,
    /// The first of the strings that is not empty.
    FirstText,
    /// The access key, else the accelerator key.
    Shortcut,
    /// The control type mapped to a [`Role`].
    ControlTypeToRole,
    /// The state properties and pattern availabilities mapped to a
    /// [`StateSet`].
    States,
    /// The one integer, absent when zero, as a count.
    Count,
    /// The one boolean.
    Flag,
    /// The bounding rectangle.
    Rect,
    /// The native window handle, absent when zero.
    Window,
    /// The process id.
    Process,
    /// The runtime id.
    RuntimeId,
    /// The one double.
    Number,
    /// The grid item's row and column, made one-based.
    Cell,
    /// The grid's row and column counts.
    TableCounts,
    /// Read through the text pattern when the cached availability says
    /// it has one, absent otherwise: the selection's text when it is not
    /// empty, else the line at the caret, else the first line.
    TextSummary,
    /// A combo box's control type with no `Value` pattern, or a list whose
    /// class name holds `LoopingSelector`.
    ValueFromSelection,
    /// Absent unless the text pattern is available or the automation id
    /// is `SearchTextBox` or `TextBox`; then whether the controller-for
    /// relation names any element.
    Suggestions,
    /// The class name of the native window handle's window, else of the
    /// nearest ancestor's window the reader knows, by `GetClassName`.
    NearestWindowClass,
    /// NVDA's test for a classic .NET Framework Windows Forms combo box:
    /// a combo box's control type, framework `WinForm`, its window's class
    /// normalized to `COMBOBOX`, a provider description naming
    /// `System.Windows.Forms, Version=4.0.0.0`, the `Value` pattern and no
    /// `Selection` pattern; then its window's `GetComboBoxInfo` list.
    WinFormsComboBoxList,
    /// The automation id of the element's raw-view parent, read by a step
    /// to the parent and its live `AutomationId`, absent when empty, never
    /// from the element's own cache.
    ParentAutomationId,
    /// Whether the native window handle's window, else the nearest
    /// ancestor's window the reader knows, is visible, by
    /// `IsWindowVisible`.
    NearestWindowShown,
    /// Whether both booleans, the control and the content element's, are
    /// true.
    ControlAndContent,
}

/// How MSAA reads a property: an `IAccessible` call, or a read of the
/// window the object is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MsaaRead {
    /// `accName`.
    Name,
    /// `accRole`, mapped to a [`Role`].
    Role,
    /// `accValue`.
    Value,
    /// `accState`, mapped to a [`StateSet`].
    State,
    /// `accDescription`.
    Description,
    /// `accKeyboardShortcut`.
    KeyboardShortcut,
    /// `accLocation`.
    Location,
    /// The window the object is in, from `WindowFromAccessibleObject`.
    Window,
    /// The process of the object's window.
    WindowProcess,
    /// The class name of the object's window.
    WindowClass,
    /// The text of an edit control's window, by its messages: the
    /// selection's text when it is not empty, else the line at the caret.
    TextSummary,
    /// A `SysTreeView32` item's children, counted by the control's
    /// messages from its first child (`TVGN_CHILD`) through each next
    /// sibling (`TVGN_NEXT`); absent for any other object.
    TreeViewChildCount,
    /// Whether the object or an ancestor up to the desktop has the focused
    /// state, by `accState` up the `accParent` chain.
    FocusedWithin,
    /// The ids the object was reached at, with the child id it is held by:
    /// no call.
    Address,
    /// Whether the object's window is visible, by `IsWindowVisible`.
    WindowShown,
}

/// How `IAccessible2` reads a property. Empty until `IAccessible2` is read
/// (milestone M7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ia2Read {}

/// One property's entry: everything about it, declared in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyEntry {
    /// The property.
    pub id: PropertyId,
    /// Its name, as an extension names it.
    pub name: &'static str,
    /// The type of its value.
    pub kind: ValueKind,
    /// How UI Automation reads it, `None` when it does not.
    pub uia: Option<UiaRead>,
    /// How MSAA reads it, `None` when it does not.
    pub msaa: Option<MsaaRead>,
    /// How `IAccessible2` reads it, `None` when it does not.
    pub ia2: Option<Ia2Read>,
    /// What it costs to read.
    pub cost: Cost,
    /// The setting that switches it, `None` for one every read that names
    /// it takes: a read leaves it out while its setting is off.
    pub detail: Option<NodeDetail>,
}

/// A node detail a setting switches: a read leaves out the properties of
/// one whose setting is off, so its call is not made
/// (`docs/design/readers-tell-core-asks-build2.md`, "Rules, each with its
/// reason").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeDetail {
    /// The description.
    Description,
    /// The keyboard shortcut.
    KeyboardShortcut,
    /// The position in the set and the set's size.
    Position,
    /// The nesting level.
    Level,
    /// A table cell's row and column.
    Cell,
}

impl NodeDetail {
    /// Every detail, in declaration order.
    pub const ALL: [Self; 5] = [
        Self::Description,
        Self::KeyboardShortcut,
        Self::Position,
        Self::Level,
        Self::Cell,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::Description => 1 << 0,
            Self::KeyboardShortcut => 1 << 1,
            Self::Position => 1 << 2,
            Self::Level => 1 << 3,
            Self::Cell => 1 << 4,
        }
    }
}

/// A set of [`NodeDetail`]s, stored as a bitmask: the details the settings
/// ask for, from which Core builds a read's property set
/// ([`NodeDetailSet::choose`]), and the details a read's property set
/// names ([`NodeDetailSet::named_by`]), which the reader fetches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeDetailSet(u8);

impl NodeDetailSet {
    /// No detail.
    pub const NONE: Self = Self(0);
    /// Every detail.
    pub const ALL: Self = Self::of(&NodeDetail::ALL);

    /// The set of `details`.
    #[must_use]
    pub const fn of(mut details: &[NodeDetail]) -> Self {
        let mut bits = 0;
        while let [first, rest @ ..] = details {
            bits |= first.bit();
            details = rest;
        }
        Self(bits)
    }

    /// Whether `detail` is in the set.
    #[must_use]
    pub const fn contains(self, detail: NodeDetail) -> bool {
        self.0 & detail.bit() != 0
    }

    /// This set without `detail`.
    #[must_use]
    pub const fn without(self, detail: NodeDetail) -> Self {
        Self(self.0 & !detail.bit())
    }

    /// The details `set` names: each whose properties it holds one of.
    #[must_use]
    pub fn named_by(set: &[PropertyId]) -> Self {
        Self(
            set.iter()
                .filter_map(|id| id.entry().detail)
                .fold(0, |bits, detail| bits | detail.bit()),
        )
    }

    /// `base` less the properties of the details not in this set: the
    /// property set a read takes under the settings this set holds.
    #[must_use]
    pub fn choose(self, base: &[PropertyId]) -> Vec<PropertyId> {
        base.iter()
            .copied()
            .filter(|id| id.entry().detail.is_none_or(|detail| self.contains(detail)))
            .collect()
    }
}

/// UI Automation property ids, the numbers UI Automation's headers give
/// them, so that this crate needs no Windows bindings.
mod uia {
    pub(super) const RUNTIME_ID: i32 = 30000;
    pub(super) const BOUNDING_RECTANGLE: i32 = 30001;
    pub(super) const PROCESS_ID: i32 = 30002;
    pub(super) const CONTROL_TYPE: i32 = 30003;
    pub(super) const NAME: i32 = 30005;
    pub(super) const ACCELERATOR_KEY: i32 = 30006;
    pub(super) const ACCESS_KEY: i32 = 30007;
    pub(super) const HAS_KEYBOARD_FOCUS: i32 = 30008;
    pub(super) const IS_KEYBOARD_FOCUSABLE: i32 = 30009;
    pub(super) const IS_ENABLED: i32 = 30010;
    pub(super) const AUTOMATION_ID: i32 = 30011;
    pub(super) const CLASS_NAME: i32 = 30012;
    pub(super) const HELP_TEXT: i32 = 30013;
    pub(super) const IS_CONTROL_ELEMENT: i32 = 30016;
    pub(super) const IS_CONTENT_ELEMENT: i32 = 30017;
    pub(super) const IS_PASSWORD: i32 = 30019;
    pub(super) const NATIVE_WINDOW_HANDLE: i32 = 30020;
    pub(super) const IS_REQUIRED_FOR_FORM: i32 = 30025;
    pub(super) const IS_TEXT_PATTERN_AVAILABLE: i32 = 30040;
    pub(super) const IS_VALUE_PATTERN_AVAILABLE: i32 = 30043;
    pub(super) const VALUE_VALUE: i32 = 30045;
    pub(super) const VALUE_IS_READ_ONLY: i32 = 30046;
    pub(super) const RANGE_VALUE_VALUE: i32 = 30047;
    pub(super) const GRID_ROW_COUNT: i32 = 30062;
    pub(super) const GRID_COLUMN_COUNT: i32 = 30063;
    pub(super) const GRID_ITEM_ROW: i32 = 30064;
    pub(super) const GRID_ITEM_COLUMN: i32 = 30065;
    pub(super) const EXPAND_COLLAPSE_STATE: i32 = 30070;
    pub(super) const SELECTION_ITEM_IS_SELECTED: i32 = 30079;
    pub(super) const TOGGLE_STATE: i32 = 30086;
    pub(super) const FRAMEWORK_ID: i32 = 30024;
    pub(super) const IS_SELECTION_PATTERN_AVAILABLE: i32 = 30037;
    pub(super) const IS_DATA_VALID_FOR_FORM: i32 = 30103;
    pub(super) const CONTROLLER_FOR: i32 = 30104;
    pub(super) const PROVIDER_DESCRIPTION: i32 = 30107;
    pub(super) const POSITION_IN_SET: i32 = 30152;
    pub(super) const SIZE_OF_SET: i32 = 30153;
    pub(super) const LEVEL: i32 = 30154;
    pub(super) const FULL_DESCRIPTION: i32 = 30159;
}

const NAME: PropertyEntry = PropertyEntry {
    id: PropertyId::Name,
    name: "name",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::NAME],
        conversion: UiaConversion::Text,
    }),
    msaa: Some(MsaaRead::Name),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const ROLE: PropertyEntry = PropertyEntry {
    id: PropertyId::Role,
    name: "role",
    kind: ValueKind::Role,
    uia: Some(UiaRead {
        cached: &[uia::CONTROL_TYPE],
        conversion: UiaConversion::ControlTypeToRole,
    }),
    msaa: Some(MsaaRead::Role),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const VALUE: PropertyEntry = PropertyEntry {
    id: PropertyId::Value,
    name: "value",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::VALUE_VALUE],
        conversion: UiaConversion::Text,
    }),
    msaa: Some(MsaaRead::Value),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const STATES: PropertyEntry = PropertyEntry {
    id: PropertyId::States,
    name: "states",
    kind: ValueKind::States,
    uia: Some(UiaRead {
        cached: &[
            uia::HAS_KEYBOARD_FOCUS,
            uia::IS_KEYBOARD_FOCUSABLE,
            uia::IS_ENABLED,
            uia::TOGGLE_STATE,
            uia::EXPAND_COLLAPSE_STATE,
            uia::SELECTION_ITEM_IS_SELECTED,
            uia::IS_PASSWORD,
            uia::IS_REQUIRED_FOR_FORM,
            uia::IS_DATA_VALID_FOR_FORM,
            uia::VALUE_IS_READ_ONLY,
        ],
        conversion: UiaConversion::States,
    }),
    msaa: Some(MsaaRead::State),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const DESCRIPTION: PropertyEntry = PropertyEntry {
    id: PropertyId::Description,
    name: "description",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::FULL_DESCRIPTION, uia::HELP_TEXT],
        conversion: UiaConversion::FirstText,
    }),
    msaa: Some(MsaaRead::Description),
    ia2: None,
    cost: Cost::Cheap,
    detail: Some(NodeDetail::Description),
};

const KEYBOARD_SHORTCUT: PropertyEntry = PropertyEntry {
    id: PropertyId::KeyboardShortcut,
    name: "keyboard shortcut",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::ACCESS_KEY, uia::ACCELERATOR_KEY],
        conversion: UiaConversion::Shortcut,
    }),
    msaa: Some(MsaaRead::KeyboardShortcut),
    ia2: None,
    cost: Cost::Cheap,
    detail: Some(NodeDetail::KeyboardShortcut),
};

const POSITION_IN_SET: PropertyEntry = PropertyEntry {
    id: PropertyId::PositionInSet,
    name: "position in set",
    kind: ValueKind::Count,
    uia: Some(UiaRead {
        cached: &[uia::POSITION_IN_SET],
        conversion: UiaConversion::Count,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: Some(NodeDetail::Position),
};

const SET_SIZE: PropertyEntry = PropertyEntry {
    id: PropertyId::SetSize,
    name: "set size",
    kind: ValueKind::Count,
    uia: Some(UiaRead {
        cached: &[uia::SIZE_OF_SET],
        conversion: UiaConversion::Count,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: Some(NodeDetail::Position),
};

const LEVEL: PropertyEntry = PropertyEntry {
    id: PropertyId::Level,
    name: "level",
    kind: ValueKind::Count,
    uia: Some(UiaRead {
        cached: &[uia::LEVEL],
        conversion: UiaConversion::Count,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: Some(NodeDetail::Level),
};

const RECT: PropertyEntry = PropertyEntry {
    id: PropertyId::Rect,
    name: "rectangle",
    kind: ValueKind::Rect,
    uia: Some(UiaRead {
        cached: &[uia::BOUNDING_RECTANGLE],
        conversion: UiaConversion::Rect,
    }),
    msaa: Some(MsaaRead::Location),
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const CLASS_NAME: PropertyEntry = PropertyEntry {
    id: PropertyId::ClassName,
    name: "class name",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::CLASS_NAME],
        conversion: UiaConversion::Text,
    }),
    msaa: Some(MsaaRead::WindowClass),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const AUTOMATION_ID: PropertyEntry = PropertyEntry {
    id: PropertyId::AutomationId,
    name: "automation id",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::AUTOMATION_ID],
        conversion: UiaConversion::Text,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const NATIVE_WINDOW: PropertyEntry = PropertyEntry {
    id: PropertyId::NativeWindow,
    name: "window",
    kind: ValueKind::Window,
    uia: Some(UiaRead {
        cached: &[uia::NATIVE_WINDOW_HANDLE],
        conversion: UiaConversion::Window,
    }),
    msaa: Some(MsaaRead::Window),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const PROCESS: PropertyEntry = PropertyEntry {
    id: PropertyId::Process,
    name: "process",
    kind: ValueKind::Process,
    uia: Some(UiaRead {
        cached: &[uia::PROCESS_ID],
        conversion: UiaConversion::Process,
    }),
    msaa: Some(MsaaRead::WindowProcess),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const RUNTIME_ID: PropertyEntry = PropertyEntry {
    id: PropertyId::RuntimeId,
    name: "runtime id",
    kind: ValueKind::RuntimeId,
    uia: Some(UiaRead {
        cached: &[uia::RUNTIME_ID],
        conversion: UiaConversion::RuntimeId,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const HAS_KEYBOARD_FOCUS: PropertyEntry = PropertyEntry {
    id: PropertyId::HasKeyboardFocus,
    name: "has keyboard focus",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[uia::HAS_KEYBOARD_FOCUS],
        conversion: UiaConversion::Flag,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const HAS_TEXT_PATTERN: PropertyEntry = PropertyEntry {
    id: PropertyId::HasTextPattern,
    name: "has text pattern",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[uia::IS_TEXT_PATTERN_AVAILABLE],
        conversion: UiaConversion::Flag,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const RANGE_VALUE: PropertyEntry = PropertyEntry {
    id: PropertyId::RangeValue,
    name: "range value",
    kind: ValueKind::Number,
    uia: Some(UiaRead {
        cached: &[uia::RANGE_VALUE_VALUE],
        conversion: UiaConversion::Number,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const CELL: PropertyEntry = PropertyEntry {
    id: PropertyId::Cell,
    name: "cell",
    kind: ValueKind::Cell,
    uia: Some(UiaRead {
        cached: &[uia::GRID_ITEM_ROW, uia::GRID_ITEM_COLUMN],
        conversion: UiaConversion::Cell,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Alone,
    detail: Some(NodeDetail::Cell),
};

const TABLE_COUNTS: PropertyEntry = PropertyEntry {
    id: PropertyId::TableCounts,
    name: "table counts",
    kind: ValueKind::TableCounts,
    uia: Some(UiaRead {
        cached: &[uia::GRID_ROW_COUNT, uia::GRID_COLUMN_COUNT],
        conversion: UiaConversion::TableCounts,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const TEXT_SUMMARY: PropertyEntry = PropertyEntry {
    id: PropertyId::TextSummary,
    name: "text summary",
    kind: ValueKind::TextSummary,
    uia: Some(UiaRead {
        cached: &[uia::IS_TEXT_PATTERN_AVAILABLE],
        conversion: UiaConversion::TextSummary,
    }),
    msaa: Some(MsaaRead::TextSummary),
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const VALUE_FROM_SELECTION: PropertyEntry = PropertyEntry {
    id: PropertyId::ValueFromSelection,
    name: "value from selection",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[
            uia::CONTROL_TYPE,
            uia::CLASS_NAME,
            uia::IS_VALUE_PATTERN_AVAILABLE,
        ],
        conversion: UiaConversion::ValueFromSelection,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const SUGGESTIONS: PropertyEntry = PropertyEntry {
    id: PropertyId::Suggestions,
    name: "suggestions",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[
            uia::IS_TEXT_PATTERN_AVAILABLE,
            uia::AUTOMATION_ID,
            uia::CONTROLLER_FOR,
        ],
        conversion: UiaConversion::Suggestions,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const HAS_VALUE_PATTERN: PropertyEntry = PropertyEntry {
    id: PropertyId::HasValuePattern,
    name: "has value pattern",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[uia::IS_VALUE_PATTERN_AVAILABLE],
        conversion: UiaConversion::Flag,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const WINDOW_CLASS: PropertyEntry = PropertyEntry {
    id: PropertyId::WindowClass,
    name: "window class",
    kind: ValueKind::Text,
    uia: Some(UiaRead {
        cached: &[uia::NATIVE_WINDOW_HANDLE],
        conversion: UiaConversion::NearestWindowClass,
    }),
    msaa: Some(MsaaRead::WindowClass),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const COMBO_BOX_LIST: PropertyEntry = PropertyEntry {
    id: PropertyId::ComboBoxList,
    name: "combo box list",
    kind: ValueKind::Window,
    uia: Some(UiaRead {
        cached: &[
            uia::CONTROL_TYPE,
            uia::FRAMEWORK_ID,
            uia::NATIVE_WINDOW_HANDLE,
            uia::PROVIDER_DESCRIPTION,
            uia::IS_VALUE_PATTERN_AVAILABLE,
            uia::IS_SELECTION_PATTERN_AVAILABLE,
        ],
        conversion: UiaConversion::WinFormsComboBoxList,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const PARENT_AUTOMATION_ID: PropertyEntry = PropertyEntry {
    id: PropertyId::ParentAutomationId,
    name: "parent automation id",
    kind: ValueKind::Text,
    // Read by a call to the parent, never from the element's cache.
    uia: Some(UiaRead {
        cached: &[uia::AUTOMATION_ID],
        conversion: UiaConversion::ParentAutomationId,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const CHILD_COUNT: PropertyEntry = PropertyEntry {
    id: PropertyId::ChildCount,
    name: "child count",
    kind: ValueKind::Count,
    uia: None,
    msaa: Some(MsaaRead::TreeViewChildCount),
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const FOCUSED_WITHIN: PropertyEntry = PropertyEntry {
    id: PropertyId::FocusedWithin,
    name: "focused within",
    kind: ValueKind::Flag,
    uia: None,
    msaa: Some(MsaaRead::FocusedWithin),
    ia2: None,
    cost: Cost::Alone,
    detail: None,
};

const MSAA_ADDRESS: PropertyEntry = PropertyEntry {
    id: PropertyId::MsaaAddress,
    name: "msaa address",
    kind: ValueKind::MsaaAddress,
    uia: None,
    msaa: Some(MsaaRead::Address),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const WINDOW_SHOWN: PropertyEntry = PropertyEntry {
    id: PropertyId::WindowShown,
    name: "window shown",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[uia::NATIVE_WINDOW_HANDLE],
        conversion: UiaConversion::NearestWindowShown,
    }),
    msaa: Some(MsaaRead::WindowShown),
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

const IS_CONTENT: PropertyEntry = PropertyEntry {
    id: PropertyId::IsContent,
    name: "is content",
    kind: ValueKind::Flag,
    uia: Some(UiaRead {
        cached: &[uia::IS_CONTROL_ELEMENT, uia::IS_CONTENT_ELEMENT],
        conversion: UiaConversion::ControlAndContent,
    }),
    msaa: None,
    ia2: None,
    cost: Cost::Cheap,
    detail: None,
};

/// The whole table, in [`PropertyId`] order.
pub const PROPERTIES: &[PropertyEntry] = &[
    NAME,
    ROLE,
    VALUE,
    STATES,
    DESCRIPTION,
    KEYBOARD_SHORTCUT,
    POSITION_IN_SET,
    SET_SIZE,
    LEVEL,
    RECT,
    CLASS_NAME,
    AUTOMATION_ID,
    NATIVE_WINDOW,
    PROCESS,
    RUNTIME_ID,
    HAS_KEYBOARD_FOCUS,
    HAS_TEXT_PATTERN,
    RANGE_VALUE,
    CELL,
    TABLE_COUNTS,
    TEXT_SUMMARY,
    VALUE_FROM_SELECTION,
    SUGGESTIONS,
    HAS_VALUE_PATTERN,
    WINDOW_CLASS,
    COMBO_BOX_LIST,
    PARENT_AUTOMATION_ID,
    CHILD_COUNT,
    FOCUSED_WITHIN,
    MSAA_ADDRESS,
    WINDOW_SHOWN,
    IS_CONTENT,
];

/// A UI Automation runtime id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeId(pub Vec<i32>);

/// A number a property reads as a double, such as a range value. Two
/// numbers are equal when their bits are, so that a value compares with
/// itself.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(pub f64);

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Number {}

/// One property's value, as an answer carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyValue {
    /// A string.
    Text(String),
    /// A role.
    Role(Role),
    /// States.
    States(StateSet),
    /// A count or one-based position.
    Count(u32),
    /// Yes or no.
    Flag(bool),
    /// A rectangle.
    Rect(Rect),
    /// A window.
    Window(WindowHandle),
    /// A process.
    Process(Pid),
    /// A runtime id.
    RuntimeId(RuntimeId),
    /// A number.
    Number(Number),
    /// A grid cell's place.
    Cell(CellPosition),
    /// A table's counts.
    TableCounts(TableCounts),
    /// What an object's text says of it.
    TextSummary(TextSummary),
    /// The MSAA ids an object was reached at, and the child id it is held
    /// by.
    MsaaAddress(HeldIds),
}

/// What an object's text says of it, as NVDA's object speech reads it
/// (`docs/parity/focus-announcement.md`): its selected text when the
/// selection is not empty, else one line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSummary {
    /// The selected text.
    Selected(String),
    /// The line at the caret, or the first line when the text has no
    /// caret.
    Line(String),
}

impl PropertyValue {
    /// The type of this value.
    #[must_use]
    pub const fn kind(&self) -> ValueKind {
        match self {
            Self::Text(_) => ValueKind::Text,
            Self::Role(_) => ValueKind::Role,
            Self::States(_) => ValueKind::States,
            Self::Count(_) => ValueKind::Count,
            Self::Flag(_) => ValueKind::Flag,
            Self::Rect(_) => ValueKind::Rect,
            Self::Window(_) => ValueKind::Window,
            Self::Process(_) => ValueKind::Process,
            Self::RuntimeId(_) => ValueKind::RuntimeId,
            Self::Number(_) => ValueKind::Number,
            Self::Cell(_) => ValueKind::Cell,
            Self::TableCounts(_) => ValueKind::TableCounts,
            Self::TextSummary(_) => ValueKind::TextSummary,
            Self::MsaaAddress(_) => ValueKind::MsaaAddress,
        }
    }
}

/// One property as an answer gives it: its value, or `None` when the
/// object has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyAnswer {
    /// The property.
    pub id: PropertyId,
    /// Its value, `None` when the object has none.
    pub value: Option<PropertyValue>,
}

/// The properties a reader read, as they travel: each property the read
/// named, once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PropertyValues(pub Vec<PropertyAnswer>);

impl PropertyValues {
    /// The answer for `id`, `None` when the read did not name it.
    #[must_use]
    pub fn answer(&self, id: PropertyId) -> Option<&PropertyAnswer> {
        self.0.iter().find(|answer| answer.id == id)
    }
}

/// Why [`PropertyValues`] are not the answer to a set's read: the reader
/// broke the protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetMismatch {
    /// A property of the set has no answer.
    Missing(PropertyId),
    /// A property has more than one answer.
    Repeated(PropertyId),
    /// A property the set does not name was answered.
    Unasked(PropertyId),
    /// A property's value is not of its declared type.
    WrongKind {
        /// The property.
        id: PropertyId,
        /// The type its value had.
        found: ValueKind,
    },
}

impl fmt::Display for SetMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(id) => write!(formatter, "no answer for {}", id.entry().name),
            Self::Repeated(id) => write!(formatter, "two answers for {}", id.entry().name),
            Self::Unasked(id) => write!(formatter, "an answer for {}, not asked", id.entry().name),
            Self::WrongKind { id, found } => write!(
                formatter,
                "{} answered as {found:?}, declared {:?}",
                id.entry().name,
                id.entry().kind
            ),
        }
    }
}

impl std::error::Error for SetMismatch {}

/// One property as a type, so that a set's field has the property's value
/// type. Implemented by the markers in [`props`].
pub trait Property {
    /// The property.
    const ID: PropertyId;
    /// The type of its value.
    type Value;
    /// `value` as this property's value, `None` when it is of another
    /// type.
    fn take(value: PropertyValue) -> Option<Self::Value>;
    /// This property's value as it travels.
    fn give(value: Self::Value) -> PropertyValue;
}

/// A set of properties fixed at compile time, declared with
/// [`property_set!`](crate::property_set): a struct with one field per
/// property, each `None` when the object has no value for it.
pub trait PropertySet: Sized {
    /// The set's properties, in the order of its fields.
    const PROPERTIES: &'static [PropertyId];

    /// The answer to a read of this set, parsed once: every property of
    /// the set answered exactly once, with a value of its type, and
    /// nothing else.
    ///
    /// # Errors
    ///
    /// [`SetMismatch`] when `values` are not that answer.
    fn parse(values: PropertyValues) -> Result<Self, SetMismatch>;

    /// The set's values as they travel.
    fn into_values(self) -> PropertyValues;
}

/// Checks that `values` answer exactly `properties`, each once and of its
/// declared type, and returns them in the order of `properties`.
///
/// # Errors
///
/// [`SetMismatch`] when they do not.
pub fn answers_in_order(
    properties: &[PropertyId],
    values: PropertyValues,
) -> Result<Vec<Option<PropertyValue>>, SetMismatch> {
    let mut slots: Vec<Option<Option<PropertyValue>>> = vec![None; properties.len()];
    for answer in values.0 {
        let place = properties
            .iter()
            .position(|id| *id == answer.id)
            .ok_or(SetMismatch::Unasked(answer.id))?;
        if let Some(value) = &answer.value {
            let declared = answer.id.entry().kind;
            if value.kind() != declared {
                return Err(SetMismatch::WrongKind {
                    id: answer.id,
                    found: value.kind(),
                });
            }
        }
        let slot = slots
            .get_mut(place)
            .ok_or(SetMismatch::Unasked(answer.id))?;
        if slot.is_some() {
            return Err(SetMismatch::Repeated(answer.id));
        }
        *slot = Some(answer.value);
    }
    properties
        .iter()
        .zip(slots)
        .map(|(id, slot)| slot.ok_or(SetMismatch::Missing(*id)))
        .collect()
}

/// The properties as types, one marker per entry of the table.
pub mod props {
    use super::{Number, Property, PropertyId, PropertyValue};
    use crate::{CellPosition, Pid, StateSet, WindowHandle};

    macro_rules! marker {
        ($(#[$doc:meta])* $marker:ident, $value:ty, $variant:ident) => {
            $(#[$doc])*
            #[derive(Clone, Copy, Debug)]
            pub struct $marker;

            impl Property for $marker {
                const ID: PropertyId = PropertyId::$marker;
                type Value = $value;

                fn take(value: PropertyValue) -> Option<$value> {
                    match value {
                        PropertyValue::$variant(value) => Some(value),
                        _ => None,
                    }
                }

                fn give(value: $value) -> PropertyValue {
                    PropertyValue::$variant(value)
                }
            }
        };
    }

    marker!(
        /// [`PropertyId::Name`].
        Name, String, Text
    );
    marker!(
        /// [`PropertyId::Role`].
        Role, crate::Role, Role
    );
    marker!(
        /// [`PropertyId::Value`].
        Value, String, Text
    );
    marker!(
        /// [`PropertyId::States`].
        States, StateSet, States
    );
    marker!(
        /// [`PropertyId::Description`].
        Description, String, Text
    );
    marker!(
        /// [`PropertyId::KeyboardShortcut`].
        KeyboardShortcut, String, Text
    );
    marker!(
        /// [`PropertyId::PositionInSet`].
        PositionInSet, u32, Count
    );
    marker!(
        /// [`PropertyId::SetSize`].
        SetSize, u32, Count
    );
    marker!(
        /// [`PropertyId::Level`].
        Level, u32, Count
    );
    marker!(
        /// [`PropertyId::Rect`].
        Rect, crate::Rect, Rect
    );
    marker!(
        /// [`PropertyId::ClassName`].
        ClassName, String, Text
    );
    marker!(
        /// [`PropertyId::AutomationId`].
        AutomationId, String, Text
    );
    marker!(
        /// [`PropertyId::NativeWindow`].
        NativeWindow, WindowHandle, Window
    );
    marker!(
        /// [`PropertyId::Process`].
        Process, Pid, Process
    );
    marker!(
        /// [`PropertyId::RuntimeId`].
        RuntimeId, super::RuntimeId, RuntimeId
    );
    marker!(
        /// [`PropertyId::HasKeyboardFocus`].
        HasKeyboardFocus, bool, Flag
    );
    marker!(
        /// [`PropertyId::HasTextPattern`].
        HasTextPattern, bool, Flag
    );
    marker!(
        /// [`PropertyId::RangeValue`].
        RangeValue, Number, Number
    );
    marker!(
        /// [`PropertyId::Cell`].
        Cell, CellPosition, Cell
    );
    marker!(
        /// [`PropertyId::TableCounts`].
        TableCounts, crate::TableCounts, TableCounts
    );
    marker!(
        /// [`PropertyId::TextSummary`].
        TextSummary, super::TextSummary, TextSummary
    );
    marker!(
        /// [`PropertyId::ValueFromSelection`].
        ValueFromSelection, bool, Flag
    );
    marker!(
        /// [`PropertyId::FocusedWithin`].
        FocusedWithin, bool, Flag
    );
    marker!(
        /// [`PropertyId::MsaaAddress`].
        MsaaAddress, crate::request::HeldIds, MsaaAddress
    );
    marker!(
        /// [`PropertyId::WindowShown`].
        WindowShown, bool, Flag
    );
    marker!(
        /// [`PropertyId::IsContent`].
        IsContent, bool, Flag
    );
}

/// Declares a set of properties fixed at compile time: a struct with one
/// field per property, typed by the property's value, and its
/// [`PropertySet`] implementation.
///
/// ```
/// use verbatim_model::property::{PropertySet, props};
///
/// verbatim_model::property_set! {
///     /// What a rule reads.
///     pub struct Named {
///         name: props::Name,
///         role: props::Role,
///     }
/// }
///
/// assert_eq!(Named::PROPERTIES.len(), 2);
/// ```
#[macro_export]
macro_rules! property_set {
    (
        $(#[$doc:meta])*
        $vis:vis struct $set:ident {
            $( $(#[$field_doc:meta])* $field:ident : $property:ty ),+ $(,)?
        }
    ) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq)]
        $vis struct $set {
            $(
                $(#[$field_doc])*
                pub $field: ::core::option::Option<
                    <$property as $crate::property::Property>::Value
                >,
            )+
        }

        impl $crate::property::PropertySet for $set {
            const PROPERTIES: &'static [$crate::property::PropertyId] = &[
                $( <$property as $crate::property::Property>::ID ),+
            ];

            fn parse(
                values: $crate::property::PropertyValues,
            ) -> ::core::result::Result<Self, $crate::property::SetMismatch> {
                let mut answers =
                    $crate::property::answers_in_order(Self::PROPERTIES, values)?.into_iter();
                ::core::result::Result::Ok(Self {
                    $(
                        $field: match answers.next() {
                            ::core::option::Option::Some(::core::option::Option::Some(value)) => {
                                let found = value.kind();
                                ::core::option::Option::Some(
                                    <$property as $crate::property::Property>::take(value)
                                        .ok_or($crate::property::SetMismatch::WrongKind {
                                            id: <$property as $crate::property::Property>::ID,
                                            found,
                                        })?,
                                )
                            }
                            ::core::option::Option::Some(::core::option::Option::None) => {
                                ::core::option::Option::None
                            }
                            ::core::option::Option::None => {
                                return ::core::result::Result::Err(
                                    $crate::property::SetMismatch::Missing(
                                        <$property as $crate::property::Property>::ID,
                                    ),
                                );
                            }
                        },
                    )+
                })
            }

            fn into_values(self) -> $crate::property::PropertyValues {
                $crate::property::PropertyValues(::std::vec![
                    $(
                        $crate::property::PropertyAnswer {
                            id: <$property as $crate::property::Property>::ID,
                            value: self.$field.map(
                                <$property as $crate::property::Property>::give,
                            ),
                        }
                    ),+
                ])
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::property_set! {
        /// A set for the tests.
        struct Focus {
            name: props::Name,
            role: props::Role,
            states: props::States,
            window: props::NativeWindow,
        }
    }

    fn answer(id: PropertyId, value: Option<PropertyValue>) -> PropertyAnswer {
        PropertyAnswer { id, value }
    }

    #[test]
    fn every_property_has_one_entry_in_the_table_in_order() {
        for (place, entry) in PROPERTIES.iter().enumerate() {
            assert_eq!(entry.id.entry(), entry);
            assert_eq!(
                PROPERTIES.iter().position(|other| other.id == entry.id),
                Some(place)
            );
            assert_eq!(
                PROPERTIES
                    .iter()
                    .filter(|other| other.name == entry.name)
                    .count(),
                1
            );
        }
        assert_eq!(PROPERTIES.len(), 32);
    }

    #[test]
    fn every_property_is_read_by_some_interface() {
        for entry in PROPERTIES {
            assert!(
                entry.uia.is_some() || entry.msaa.is_some(),
                "{} is read by no interface",
                entry.name
            );
            if let Some(read) = entry.uia {
                assert!(!read.cached.is_empty(), "{} caches nothing", entry.name);
            }
        }
    }

    #[test]
    fn an_extension_names_a_property_by_its_entrys_name() {
        assert_eq!(
            PropertyId::named("automation id"),
            Some(PropertyId::AutomationId)
        );
        assert_eq!(PropertyId::named("colour"), None);
    }

    #[test]
    fn a_set_lists_its_properties_in_field_order() {
        assert_eq!(
            Focus::PROPERTIES,
            [
                PropertyId::Name,
                PropertyId::Role,
                PropertyId::States,
                PropertyId::NativeWindow
            ]
        );
    }

    #[test]
    fn a_sets_answer_parses_into_its_fields_in_any_order() {
        let values = PropertyValues(vec![
            answer(
                PropertyId::States,
                Some(PropertyValue::States(StateSet::new())),
            ),
            answer(
                PropertyId::Name,
                Some(PropertyValue::Text("Save".to_owned())),
            ),
            answer(PropertyId::NativeWindow, None),
            answer(PropertyId::Role, Some(PropertyValue::Role(Role::Button))),
        ]);
        let focus = Focus::parse(values).unwrap();
        assert_eq!(
            focus,
            Focus {
                name: Some("Save".to_owned()),
                role: Some(Role::Button),
                states: Some(StateSet::new()),
                window: None,
            }
        );
    }

    #[test]
    fn a_set_goes_out_and_back_unchanged() {
        let focus = Focus {
            name: None,
            role: Some(Role::Button),
            states: Some(StateSet::new()),
            window: Some(WindowHandle(7)),
        };
        assert_eq!(Focus::parse(focus.clone().into_values()), Ok(focus));
    }

    #[test]
    fn an_answer_missing_a_property_does_not_parse() {
        let values = PropertyValues(vec![
            answer(PropertyId::Name, None),
            answer(PropertyId::Role, None),
            answer(PropertyId::States, None),
        ]);
        assert_eq!(
            Focus::parse(values),
            Err(SetMismatch::Missing(PropertyId::NativeWindow))
        );
    }

    #[test]
    fn an_answer_naming_a_property_twice_does_not_parse() {
        let values = PropertyValues(vec![
            answer(PropertyId::Name, None),
            answer(PropertyId::Name, None),
        ]);
        assert_eq!(
            Focus::parse(values),
            Err(SetMismatch::Repeated(PropertyId::Name))
        );
    }

    #[test]
    fn an_answer_with_a_property_not_asked_does_not_parse() {
        let values = PropertyValues(vec![answer(PropertyId::Level, None)]);
        assert_eq!(
            Focus::parse(values),
            Err(SetMismatch::Unasked(PropertyId::Level))
        );
    }

    #[test]
    fn an_answer_with_a_value_of_another_type_does_not_parse() {
        let values = PropertyValues(vec![
            answer(PropertyId::Name, Some(PropertyValue::Count(3))),
            answer(PropertyId::Role, None),
            answer(PropertyId::States, None),
            answer(PropertyId::NativeWindow, None),
        ]);
        assert_eq!(
            Focus::parse(values),
            Err(SetMismatch::WrongKind {
                id: PropertyId::Name,
                found: ValueKind::Count
            })
        );
    }

    #[test]
    fn values_travel_as_json_and_back() {
        let values = Focus {
            name: Some("Save".to_owned()),
            role: None,
            states: None,
            window: Some(WindowHandle(9)),
        }
        .into_values();
        let json = serde_json::to_string(&values).unwrap();
        assert_eq!(
            serde_json::from_str::<PropertyValues>(&json).unwrap(),
            values
        );
    }

    #[test]
    fn a_number_equals_itself() {
        assert_eq!(Number(0.5), Number(0.5));
        assert_ne!(Number(0.5), Number(0.25));
    }

    /// A property set names each detail one of whose properties it holds,
    /// and the settings' details choose a set that keeps every property
    /// no setting switches.
    #[test]
    fn node_details_are_named_by_a_set_and_choose_one() {
        assert_eq!(
            NodeDetailSet::named_by(&[PropertyId::Name, PropertyId::SetSize, PropertyId::Cell]),
            NodeDetailSet::of(&[NodeDetail::Position, NodeDetail::Cell])
        );
        assert_eq!(
            NodeDetailSet::named_by(&[PropertyId::Name, PropertyId::Value]),
            NodeDetailSet::NONE
        );
        let base = [
            PropertyId::Name,
            PropertyId::Description,
            PropertyId::PositionInSet,
            PropertyId::SetSize,
            PropertyId::Level,
        ];
        assert_eq!(
            NodeDetailSet::of(&[NodeDetail::Level]).choose(&base),
            [PropertyId::Name, PropertyId::Level]
        );
        assert_eq!(NodeDetailSet::ALL.choose(&base), base);
        assert_eq!(
            NodeDetailSet::named_by(&NodeDetailSet::ALL.choose(&base)),
            NodeDetailSet::of(&[
                NodeDetail::Description,
                NodeDetail::Position,
                NodeDetail::Level
            ])
        );
    }
}
