//! Rules for particular applications' windows, which Core applies until the
//! extension host runs application modules (decision D6): each is written
//! from its entry in the parity ledger, and none lives in the focus
//! listener or an outpost.

use verbatim_model::{NormalizedEvent, WindowFacts};

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

/// Whether `event`, in the window `window` describes, is a focus or a
/// foreground change on one of the shell's staging windows, which Core
/// drops whole: nothing is said for it, and the focus, attention and
/// speech stay as they were (`docs/parity/windows-and-processes.md`, "The
/// shell's staging windows", which records that speech is not cut there).
/// On Alt+Tab, File Explorer's staging window takes the focus with no name,
/// and was said as "pane" before the window switched to.
pub(crate) fn is_shell_staging_focus(
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> bool {
    matches!(event, NormalizedEvent::FocusChanged { .. })
        && window.is_some_and(|window| SHELL_STAGING_CLASSES.contains(&window.class.as_str()))
}
