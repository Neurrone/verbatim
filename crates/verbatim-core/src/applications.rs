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

/// File Explorer, by its application name ([`WindowFacts::application`]).
const FILE_EXPLORER: &str = "explorer";

/// Whether a UI Automation focus on `node`, with the window facts
/// `window`, is the task switcher's frame, whose focus is refused whole: it
/// is dropped before it moves the focus or the navigator or cuts speech
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
/// It is when `node` was read through UI Automation, `window` names File
/// Explorer as its application, and `node`'s own UI Automation class and
/// role are `MultitaskingViewFrame` and window (Windows 10) or
/// `Windows.UI.Input.InputSite.WindowClass` and pane (Windows 11). The
/// class of the window the focus is in is not matched: a pane of another
/// class in an `InputSite` window, or a pane of that class in another
/// application, such as Windows Terminal's, is taken as any focus, and so
/// are the switcher's list items. Core applies it to a focus event and a
/// menu opening, never to a foreground change.
pub(crate) fn refuses_focus(window: Option<&WindowFacts>, node: &NodeSnapshot) -> bool {
    node.backend == Backend::Uia
        && window.is_some_and(|window| window.application == FILE_EXPLORER)
        && matches!(
            (node.details.class_name.as_deref(), node.role),
            (Some("MultitaskingViewFrame"), Role::Window)
                | (Some("Windows.UI.Input.InputSite.WindowClass"), Role::Pane)
        )
}
