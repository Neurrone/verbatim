//! The shell's tray icons and taskbar buttons, as the system tray list
//! dialog shows them: what Core asks File Explorer's outpost for and the
//! outpost answers (`docs/design/listener-without-connections.md`,
//! decision 9).

use serde::{Deserialize, Serialize};

/// Which shell surface to enumerate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellItemKind {
    /// The system tray: the notification area plus the overflow flyout
    /// window when it is open.
    SystemTray,
    /// The taskbar buttons: everything on the taskbar outside the
    /// notification area.
    Taskbar,
}

/// One enumerated tray icon or taskbar button. It carries no position: a
/// list read never asks for one, and a click asks the outpost where the
/// item is then, finding it again first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellItem {
    /// The item's accessible name, shown in the list dialog.
    pub name: String,
    /// The item's UIA runtime id, which identifies its element for as long
    /// as the element exists; empty when the shell gave none.
    pub runtime_id: Vec<i32>,
}
