//! The event-kind table: every kind of event a reader tells Core, declared
//! once with its teller, the interface event it comes from and the
//! properties cached with it (ADR 0003, "Properties, event kinds and
//! requests"; ADR 0003, "Who holds what": each kind of event has exactly
//! one teller, so no event reaches Core twice by two routes).

use serde::{Deserialize, Serialize};

use crate::property::PropertyId;

/// One kind of event, as a tell names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EventKind {
    /// An MSAA focus.
    MsaaFocus,
    /// A window came to the foreground.
    Foreground,
    /// Program Manager's or the taskbar's foreground event, which updates
    /// Core's foreground record and is no foreground entry.
    ShellForeground,
    /// A menu started.
    MenuStart,
    /// A menu ended.
    MenuEnd,
    /// A popup menu started.
    PopupStart,
    /// A popup menu ended.
    PopupEnd,
    /// The task switcher ended.
    SwitchEnd,
    /// The input desktop changed.
    DesktopSwitch,
    /// An MSAA alert.
    Alert,
    /// An object's name changed.
    NameChange,
    /// An object's value changed.
    ValueChange,
    /// An object's states changed.
    StateChange,
    /// An object's description changed.
    DescriptionChange,
    /// An object was selected.
    Selection,
    /// An object was added to a selection.
    SelectionAdd,
    /// An object was removed from a selection.
    SelectionRemove,
    /// The selection within a container changed.
    SelectionWithin,
    /// An object was shown.
    Show,
    /// An object was hidden.
    Hide,
    /// A window was destroyed.
    Destroy,
    /// The system caret moved.
    SystemCaret,
    /// A UI Automation focus.
    UiaFocus,
    /// A UI Automation menu opened.
    UiaMenuOpened,
    /// A UI Automation element was selected.
    UiaElementSelected,
    /// A UI Automation window opened, a toast candidate.
    UiaWindowOpened,
    /// A UI Automation notification.
    UiaNotification,
    /// A property of a followed UI Automation element changed.
    UiaPropertyChanged,
    /// The text of a followed UI Automation element changed.
    UiaTextChanged,
    /// The text selection of a followed UI Automation element changed: its
    /// caret moved.
    UiaTextSelectionChanged,
    /// The console's caret moved.
    ConsoleCaret,
    /// A region of the console's screen changed.
    ConsoleUpdateRegion,
    /// One character of the console's screen changed.
    ConsoleUpdateSimple,
    /// The console's screen scrolled.
    ConsoleUpdateScroll,
    /// The console's layout changed.
    ConsoleLayout,
    /// A change signal of the terminal `Follow` names: Windows Terminal's
    /// text-changed or caret event of the terminal's element, or one of the
    /// console host's five console events of its console window
    /// (`docs/design/readers-tell-core-asks-build3.md`, "The outpost: change
    /// tells").
    TerminalChanged,
}

impl EventKind {
    /// This kind's entry in the table.
    #[must_use]
    pub fn entry(self) -> &'static EventKindEntry {
        match self {
            Self::MsaaFocus => &MSAA_FOCUS,
            Self::Foreground => &FOREGROUND,
            Self::ShellForeground => &SHELL_FOREGROUND,
            Self::MenuStart => &MENU_START,
            Self::MenuEnd => &MENU_END,
            Self::PopupStart => &POPUP_START,
            Self::PopupEnd => &POPUP_END,
            Self::SwitchEnd => &SWITCH_END,
            Self::DesktopSwitch => &DESKTOP_SWITCH,
            Self::Alert => &ALERT,
            Self::NameChange => &NAME_CHANGE,
            Self::ValueChange => &VALUE_CHANGE,
            Self::StateChange => &STATE_CHANGE,
            Self::DescriptionChange => &DESCRIPTION_CHANGE,
            Self::Selection => &SELECTION,
            Self::SelectionAdd => &SELECTION_ADD,
            Self::SelectionRemove => &SELECTION_REMOVE,
            Self::SelectionWithin => &SELECTION_WITHIN,
            Self::Show => &SHOW,
            Self::Hide => &HIDE,
            Self::Destroy => &DESTROY,
            Self::SystemCaret => &SYSTEM_CARET,
            Self::UiaFocus => &UIA_FOCUS,
            Self::UiaMenuOpened => &UIA_MENU_OPENED,
            Self::UiaElementSelected => &UIA_ELEMENT_SELECTED,
            Self::UiaWindowOpened => &UIA_WINDOW_OPENED,
            Self::UiaNotification => &UIA_NOTIFICATION,
            Self::UiaPropertyChanged => &UIA_PROPERTY_CHANGED,
            Self::UiaTextChanged => &UIA_TEXT_CHANGED,
            Self::UiaTextSelectionChanged => &UIA_TEXT_SELECTION_CHANGED,
            Self::ConsoleCaret => &CONSOLE_CARET,
            Self::ConsoleUpdateRegion => &CONSOLE_UPDATE_REGION,
            Self::ConsoleUpdateSimple => &CONSOLE_UPDATE_SIMPLE,
            Self::ConsoleUpdateScroll => &CONSOLE_UPDATE_SCROLL,
            Self::ConsoleLayout => &CONSOLE_LAYOUT,
            Self::TerminalChanged => &TERMINAL_CHANGED,
        }
    }
}

/// Who tells Core of a kind of event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Teller {
    /// The focus listener's desktop-wide `WinEvent` hook.
    ListenerHook,
    /// The focus listener's UI Automation focus handler.
    ListenerFocusHandler,
    /// The focus listener's UI Automation global event group.
    ListenerGlobalGroup,
    /// An outpost's UI Automation group on the objects Core names in
    /// `Follow`.
    OutpostFollowGroup,
    /// A console host's outpost, which keeps the console's five events.
    ConsoleHost,
    /// An outpost, of the terminal Core names in `Follow`.
    OutpostTerminal,
}

/// The interface event a kind comes from, by the number Windows gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A `WinEvent`, by its event constant.
    WinEvent(u32),
    /// A `WinEvent` of one object only, by its event constant and the
    /// object id it names: the system caret's location change.
    WinEventOfObject {
        /// The event constant.
        event: u32,
        /// The object id.
        object: i32,
    },
    /// A UI Automation event, by its event id.
    Uia(i32),
    /// Any change signal of a followed terminal: the console host's five
    /// console events, or Windows Terminal's text-changed and caret
    /// events of its element.
    TerminalSignals,
}

/// One kind's entry: everything about it, declared in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventKindEntry {
    /// The kind.
    pub kind: EventKind,
    /// Its name, as an extension names it.
    pub name: &'static str,
    /// Who tells it.
    pub teller: Teller,
    /// The interface event it comes from.
    pub source: Source,
    /// The properties its tell carries, read from the event's cache with
    /// no call. Empty for a `WinEvent`, which carries only ids.
    pub cached: &'static [PropertyId],
}

/// `WinEvent` constants, the numbers Windows' headers give them, so that
/// this crate needs no Windows bindings.
mod win_event {
    pub(super) const SYSTEM_ALERT: u32 = 0x0002;
    pub(super) const SYSTEM_FOREGROUND: u32 = 0x0003;
    pub(super) const SYSTEM_MENUSTART: u32 = 0x0004;
    pub(super) const SYSTEM_MENUEND: u32 = 0x0005;
    pub(super) const SYSTEM_MENUPOPUPSTART: u32 = 0x0006;
    pub(super) const SYSTEM_MENUPOPUPEND: u32 = 0x0007;
    pub(super) const SYSTEM_SWITCHEND: u32 = 0x0015;
    pub(super) const SYSTEM_DESKTOPSWITCH: u32 = 0x0020;
    pub(super) const CONSOLE_CARET: u32 = 0x4001;
    pub(super) const CONSOLE_UPDATE_REGION: u32 = 0x4002;
    pub(super) const CONSOLE_UPDATE_SIMPLE: u32 = 0x4003;
    pub(super) const CONSOLE_UPDATE_SCROLL: u32 = 0x4004;
    pub(super) const CONSOLE_LAYOUT: u32 = 0x4005;
    pub(super) const OBJECT_DESTROY: u32 = 0x8001;
    pub(super) const OBJECT_SHOW: u32 = 0x8002;
    pub(super) const OBJECT_HIDE: u32 = 0x8003;
    pub(super) const OBJECT_FOCUS: u32 = 0x8005;
    pub(super) const OBJECT_SELECTION: u32 = 0x8006;
    pub(super) const OBJECT_SELECTIONADD: u32 = 0x8007;
    pub(super) const OBJECT_SELECTIONREMOVE: u32 = 0x8008;
    pub(super) const OBJECT_SELECTIONWITHIN: u32 = 0x8009;
    pub(super) const OBJECT_STATECHANGE: u32 = 0x800A;
    pub(super) const OBJECT_LOCATIONCHANGE: u32 = 0x800B;
    pub(super) const OBJECT_NAMECHANGE: u32 = 0x800C;
    pub(super) const OBJECT_DESCRIPTIONCHANGE: u32 = 0x800D;
    pub(super) const OBJECT_VALUECHANGE: u32 = 0x800E;
    /// `OBJID_CARET`.
    pub(super) const CARET_OBJECT: i32 = -8;
}

/// UI Automation event ids, the numbers UI Automation's headers give them.
mod uia_event {
    pub(super) const MENU_OPENED: i32 = 20003;
    pub(super) const PROPERTY_CHANGED: i32 = 20004;
    pub(super) const FOCUS_CHANGED: i32 = 20005;
    pub(super) const ELEMENT_SELECTED: i32 = 20012;
    pub(super) const TEXT_SELECTION_CHANGED: i32 = 20014;
    pub(super) const TEXT_CHANGED: i32 = 20015;
    pub(super) const WINDOW_OPENED: i32 = 20016;
    pub(super) const NOTIFICATION: i32 = 20035;
}

/// What a UI Automation focus or menu opening carries: what Core's
/// native-element test, its overrides and its rules for an application's
/// elements read. The parent's automation id is the check thread's read,
/// present only for the task switcher's item.
const UIA_FOCUS_CACHED: &[PropertyId] = &[
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::ClassName,
    PropertyId::AutomationId,
    PropertyId::NativeWindow,
    PropertyId::Process,
    PropertyId::HasTextPattern,
    PropertyId::ParentAutomationId,
];

/// Declares one kind.
const fn kind(
    kind: EventKind,
    name: &'static str,
    teller: Teller,
    source: Source,
    cached: &'static [PropertyId],
) -> EventKindEntry {
    EventKindEntry {
        kind,
        name,
        teller,
        source,
        cached,
    }
}

/// The listener's `WinEvent` kinds carry ids only.
const fn hooked(event_kind: EventKind, name: &'static str, event: u32) -> EventKindEntry {
    kind(
        event_kind,
        name,
        Teller::ListenerHook,
        Source::WinEvent(event),
        &[],
    )
}

/// The console host's five events carry ids only.
const fn console(event_kind: EventKind, name: &'static str, event: u32) -> EventKindEntry {
    kind(
        event_kind,
        name,
        Teller::ConsoleHost,
        Source::WinEvent(event),
        &[],
    )
}

const MSAA_FOCUS: EventKindEntry = hooked(EventKind::MsaaFocus, "focus", win_event::OBJECT_FOCUS);

const FOREGROUND: EventKindEntry = hooked(
    EventKind::Foreground,
    "foreground",
    win_event::SYSTEM_FOREGROUND,
);

const SHELL_FOREGROUND: EventKindEntry = hooked(
    EventKind::ShellForeground,
    "shell foreground",
    win_event::SYSTEM_FOREGROUND,
);

const MENU_START: EventKindEntry = hooked(
    EventKind::MenuStart,
    "menu start",
    win_event::SYSTEM_MENUSTART,
);

const MENU_END: EventKindEntry = hooked(EventKind::MenuEnd, "menu end", win_event::SYSTEM_MENUEND);

const POPUP_START: EventKindEntry = hooked(
    EventKind::PopupStart,
    "popup start",
    win_event::SYSTEM_MENUPOPUPSTART,
);

const POPUP_END: EventKindEntry = hooked(
    EventKind::PopupEnd,
    "popup end",
    win_event::SYSTEM_MENUPOPUPEND,
);

const SWITCH_END: EventKindEntry = hooked(
    EventKind::SwitchEnd,
    "switch end",
    win_event::SYSTEM_SWITCHEND,
);

const DESKTOP_SWITCH: EventKindEntry = hooked(
    EventKind::DesktopSwitch,
    "desktop switch",
    win_event::SYSTEM_DESKTOPSWITCH,
);

const ALERT: EventKindEntry = hooked(EventKind::Alert, "alert", win_event::SYSTEM_ALERT);

const NAME_CHANGE: EventKindEntry = hooked(
    EventKind::NameChange,
    "name change",
    win_event::OBJECT_NAMECHANGE,
);

const VALUE_CHANGE: EventKindEntry = hooked(
    EventKind::ValueChange,
    "value change",
    win_event::OBJECT_VALUECHANGE,
);

const STATE_CHANGE: EventKindEntry = hooked(
    EventKind::StateChange,
    "state change",
    win_event::OBJECT_STATECHANGE,
);

const DESCRIPTION_CHANGE: EventKindEntry = hooked(
    EventKind::DescriptionChange,
    "description change",
    win_event::OBJECT_DESCRIPTIONCHANGE,
);

const SELECTION: EventKindEntry = hooked(
    EventKind::Selection,
    "selection",
    win_event::OBJECT_SELECTION,
);

const SELECTION_ADD: EventKindEntry = hooked(
    EventKind::SelectionAdd,
    "selection add",
    win_event::OBJECT_SELECTIONADD,
);

const SELECTION_REMOVE: EventKindEntry = hooked(
    EventKind::SelectionRemove,
    "selection remove",
    win_event::OBJECT_SELECTIONREMOVE,
);

const SELECTION_WITHIN: EventKindEntry = hooked(
    EventKind::SelectionWithin,
    "selection within",
    win_event::OBJECT_SELECTIONWITHIN,
);

const SHOW: EventKindEntry = hooked(EventKind::Show, "show", win_event::OBJECT_SHOW);

const HIDE: EventKindEntry = hooked(EventKind::Hide, "hide", win_event::OBJECT_HIDE);

const DESTROY: EventKindEntry = hooked(EventKind::Destroy, "destroy", win_event::OBJECT_DESTROY);

const SYSTEM_CARET: EventKindEntry = kind(
    EventKind::SystemCaret,
    "system caret",
    Teller::ListenerHook,
    Source::WinEventOfObject {
        event: win_event::OBJECT_LOCATIONCHANGE,
        object: win_event::CARET_OBJECT,
    },
    &[],
);

const UIA_FOCUS: EventKindEntry = kind(
    EventKind::UiaFocus,
    "UI Automation focus",
    Teller::ListenerFocusHandler,
    Source::Uia(uia_event::FOCUS_CHANGED),
    UIA_FOCUS_CACHED,
);

const UIA_MENU_OPENED: EventKindEntry = kind(
    EventKind::UiaMenuOpened,
    "UI Automation menu opened",
    Teller::ListenerGlobalGroup,
    Source::Uia(uia_event::MENU_OPENED),
    UIA_FOCUS_CACHED,
);

const UIA_ELEMENT_SELECTED: EventKindEntry = kind(
    EventKind::UiaElementSelected,
    "UI Automation element selected",
    Teller::ListenerGlobalGroup,
    Source::Uia(uia_event::ELEMENT_SELECTED),
    &[
        PropertyId::Name,
        PropertyId::Role,
        PropertyId::States,
        PropertyId::ClassName,
        PropertyId::NativeWindow,
        PropertyId::Process,
    ],
);

const UIA_WINDOW_OPENED: EventKindEntry = kind(
    EventKind::UiaWindowOpened,
    "UI Automation window opened",
    Teller::ListenerGlobalGroup,
    Source::Uia(uia_event::WINDOW_OPENED),
    &[PropertyId::ClassName, PropertyId::NativeWindow],
);

const UIA_NOTIFICATION: EventKindEntry = kind(
    EventKind::UiaNotification,
    "UI Automation notification",
    Teller::ListenerGlobalGroup,
    Source::Uia(uia_event::NOTIFICATION),
    &[
        PropertyId::ClassName,
        PropertyId::AutomationId,
        PropertyId::NativeWindow,
        PropertyId::Process,
    ],
);

const UIA_PROPERTY_CHANGED: EventKindEntry = kind(
    EventKind::UiaPropertyChanged,
    "UI Automation property changed",
    Teller::OutpostFollowGroup,
    Source::Uia(uia_event::PROPERTY_CHANGED),
    &[PropertyId::HasTextPattern],
);

const UIA_TEXT_CHANGED: EventKindEntry = kind(
    EventKind::UiaTextChanged,
    "UI Automation text changed",
    Teller::OutpostFollowGroup,
    Source::Uia(uia_event::TEXT_CHANGED),
    &[],
);

const UIA_TEXT_SELECTION_CHANGED: EventKindEntry = kind(
    EventKind::UiaTextSelectionChanged,
    "UI Automation text selection changed",
    Teller::OutpostFollowGroup,
    Source::Uia(uia_event::TEXT_SELECTION_CHANGED),
    &[],
);

const CONSOLE_CARET: EventKindEntry = console(
    EventKind::ConsoleCaret,
    "console caret",
    win_event::CONSOLE_CARET,
);

const CONSOLE_UPDATE_REGION: EventKindEntry = console(
    EventKind::ConsoleUpdateRegion,
    "console update region",
    win_event::CONSOLE_UPDATE_REGION,
);

const CONSOLE_UPDATE_SIMPLE: EventKindEntry = console(
    EventKind::ConsoleUpdateSimple,
    "console update simple",
    win_event::CONSOLE_UPDATE_SIMPLE,
);

const CONSOLE_UPDATE_SCROLL: EventKindEntry = console(
    EventKind::ConsoleUpdateScroll,
    "console update scroll",
    win_event::CONSOLE_UPDATE_SCROLL,
);

const CONSOLE_LAYOUT: EventKindEntry = console(
    EventKind::ConsoleLayout,
    "console layout",
    win_event::CONSOLE_LAYOUT,
);

const TERMINAL_CHANGED: EventKindEntry = kind(
    EventKind::TerminalChanged,
    "terminal changed",
    Teller::OutpostTerminal,
    Source::TerminalSignals,
    &[],
);

/// The whole table, in [`EventKind`] order.
pub const EVENT_KINDS: &[EventKindEntry] = &[
    MSAA_FOCUS,
    FOREGROUND,
    SHELL_FOREGROUND,
    MENU_START,
    MENU_END,
    POPUP_START,
    POPUP_END,
    SWITCH_END,
    DESKTOP_SWITCH,
    ALERT,
    NAME_CHANGE,
    VALUE_CHANGE,
    STATE_CHANGE,
    DESCRIPTION_CHANGE,
    SELECTION,
    SELECTION_ADD,
    SELECTION_REMOVE,
    SELECTION_WITHIN,
    SHOW,
    HIDE,
    DESTROY,
    SYSTEM_CARET,
    UIA_FOCUS,
    UIA_MENU_OPENED,
    UIA_ELEMENT_SELECTED,
    UIA_WINDOW_OPENED,
    UIA_NOTIFICATION,
    UIA_PROPERTY_CHANGED,
    UIA_TEXT_CHANGED,
    UIA_TEXT_SELECTION_CHANGED,
    CONSOLE_CARET,
    CONSOLE_UPDATE_REGION,
    CONSOLE_UPDATE_SIMPLE,
    CONSOLE_UPDATE_SCROLL,
    CONSOLE_LAYOUT,
    TERMINAL_CHANGED,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind, listed by hand, so that a kind added to the enum and
    /// not to the table fails here.
    const ALL: [EventKind; 36] = [
        EventKind::MsaaFocus,
        EventKind::Foreground,
        EventKind::ShellForeground,
        EventKind::MenuStart,
        EventKind::MenuEnd,
        EventKind::PopupStart,
        EventKind::PopupEnd,
        EventKind::SwitchEnd,
        EventKind::DesktopSwitch,
        EventKind::Alert,
        EventKind::NameChange,
        EventKind::ValueChange,
        EventKind::StateChange,
        EventKind::DescriptionChange,
        EventKind::Selection,
        EventKind::SelectionAdd,
        EventKind::SelectionRemove,
        EventKind::SelectionWithin,
        EventKind::Show,
        EventKind::Hide,
        EventKind::Destroy,
        EventKind::SystemCaret,
        EventKind::UiaFocus,
        EventKind::UiaMenuOpened,
        EventKind::UiaElementSelected,
        EventKind::UiaWindowOpened,
        EventKind::UiaNotification,
        EventKind::UiaPropertyChanged,
        EventKind::UiaTextChanged,
        EventKind::UiaTextSelectionChanged,
        EventKind::ConsoleCaret,
        EventKind::ConsoleUpdateRegion,
        EventKind::ConsoleUpdateSimple,
        EventKind::ConsoleUpdateScroll,
        EventKind::ConsoleLayout,
        EventKind::TerminalChanged,
    ];

    #[test]
    fn every_kind_is_declared_once_in_order() {
        assert_eq!(EVENT_KINDS.len(), ALL.len());
        for (kind, entry) in ALL.iter().zip(EVENT_KINDS) {
            assert_eq!(entry.kind, *kind);
            assert_eq!(kind.entry(), entry);
            assert_eq!(
                EVENT_KINDS
                    .iter()
                    .filter(|other| other.name == entry.name)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn each_interface_event_has_one_teller() {
        for entry in EVENT_KINDS {
            for other in EVENT_KINDS {
                if entry.source == other.source {
                    assert_eq!(
                        entry.teller, other.teller,
                        "{} and {}",
                        entry.name, other.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_win_event_carries_no_cached_property() {
        for entry in EVENT_KINDS {
            if matches!(
                entry.source,
                Source::WinEvent(_) | Source::WinEventOfObject { .. }
            ) {
                assert!(entry.cached.is_empty(), "{}", entry.name);
            }
        }
    }

    #[test]
    fn a_uia_focus_carries_what_the_overrides_read() {
        let cached = EventKind::UiaFocus.entry().cached;
        assert!(cached.contains(&PropertyId::AutomationId));
        assert!(cached.contains(&PropertyId::ClassName));
        assert!(cached.contains(&PropertyId::Role));
    }
}
