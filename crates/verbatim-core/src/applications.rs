//! Rules for particular applications' windows, which Core applies until the
//! extension host runs application modules (decision D6): each is written
//! from its entry in the parity ledger, and none lives in the focus
//! listener or an outpost.

use verbatim_model::{Backend, NodeSnapshot, Role, WindowFacts};

/// The classes of the shell's windows that take the focus or the foreground
/// only while a switch is staged: the Alt+Tab switcher's and Task View's
/// staging window, the Windows+X menu's host, and the desktop shell's
/// application manager window (`docs/parity/windows-and-processes.md`,
/// "The shell's staging windows").
const SHELL_STAGING_CLASSES: [&str; 3] = [
    "ForegroundStaging",
    "LauncherTipWnd",
    "ApplicationManager_DesktopShellWindow",
];

/// Whether `window` is one of the shell's staging windows. A focus or a
/// foreground change there is taken like any other, so the focus, the
/// navigator and the foreground move to it, and speech is cut when the
/// foreground window changed, but nothing is said for it (`docs/parity/windows-and-processes.md`, "The shell's
/// staging windows"). On Alt+Tab, File Explorer's staging window takes the
/// focus with no name, and was said as "pane" before the window switched
/// to.
pub(crate) fn is_shell_staging_window(window: Option<&WindowFacts>) -> bool {
    window.is_some_and(|window| SHELL_STAGING_CLASSES.contains(&window.class.as_str()))
}

/// The classes of Program Manager's and the taskbar's windows, whose
/// foreground changes NVDA never takes, so a focus entering them says no
/// title (`docs/parity/focus-context.md`).
const SHELL_FOREGROUND_CLASSES: [&str; 2] = ["Progman", "Shell_TrayWnd"];

/// Whether `class` is Program Manager's or the taskbar's window class.
pub(crate) fn is_shell_foreground_class(class: &str) -> bool {
    SHELL_FOREGROUND_CLASSES.contains(&class)
}

/// File Explorer, by its application name ([`WindowFacts::application`]).
const FILE_EXPLORER: &str = "explorer";

/// The automation id of the task switcher's list of windows, the parent of
/// each of its items, in Windows 10 from version 1809 and in Windows 11.
const TASK_SWITCHING_LIST: &str = "SwitchItemListControl";

/// The class of the Windows 10 task switcher's window before version 1809,
/// which its items are in.
const TASK_SWITCHING_FRAME: &str = "MultitaskingViewFrame";

/// Whether a UI Automation focus on `node`, with the window facts
/// `window`, is an item of the task switcher reached with Alt held, whose
/// containers are dropped: the focus is taken as having none, so none of
/// them is spoken and none is kept for the next focus to compare with
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's
/// pane"). It is when `node` was read through UI Automation as a list
/// item, `window` names File Explorer as its application, Alt was held
/// when the focus's window facts were read, and either `node`'s raw-view
/// parent has the automation id `SwitchItemListControl` (Windows 10 from
/// version 1809, Windows 11) or its window's class is
/// `MultitaskingViewFrame` (Windows 10 before). With Alt released, as in
/// the switcher that Control+Alt+Tab keeps open, the containers are
/// spoken as any focus's are.
pub(crate) fn drops_ancestry(window: Option<&WindowFacts>, node: &NodeSnapshot) -> bool {
    node.backend == Backend::Uia
        && node.role == Role::ListItem
        && window.is_some_and(|window| {
            window.application == FILE_EXPLORER
                && window.alt_held
                && (window.class == TASK_SWITCHING_FRAME
                    || node.details.parent_automation_id.as_deref() == Some(TASK_SWITCHING_LIST))
        })
}
