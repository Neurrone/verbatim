//! The shell's tray icons and taskbar buttons, as the system tray list
//! dialog shows them: what Core asks File Explorer's outpost for and the
//! outpost answers (`docs/design/listener-without-connections.md`,
//! decision 9).

use serde::{Deserialize, Serialize};

use crate::Rect;

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
/// list read never asks for one, and a click asks the outpost for the
/// buttons that may be the item then ([`ShellCandidates`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellItem {
    /// The item's accessible name, shown in the list dialog.
    pub name: String,
    /// The item's UIA runtime id, which identifies its element for as long
    /// as the element exists; empty when the shell gave none.
    pub runtime_id: Vec<i32>,
}

/// One button a click's position query found that may be the item chosen:
/// one whose runtime id or name is the chosen item's, with its position as
/// it was read for the click.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCandidate {
    /// The button, as a list would show it.
    pub item: ShellItem,
    /// Whether it is off the screen now.
    pub offscreen: bool,
    /// Its screen rectangle now, `None` when the shell gave none.
    pub rect: Option<Rect>,
}

/// The answer to a click's position query: the buttons of the surface that
/// may be the item chosen, in the shell's order, for Core to decide which
/// one to click, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCandidates {
    /// Every button found whose runtime id or name is the chosen item's,
    /// except one gone before its position was read.
    pub candidates: Vec<ShellCandidate>,
    /// Whether every button of the surface was read and every candidate's
    /// position with it: false when the walk's time budget or node cap cut
    /// it short, or a step or a window's lookup failed, so a button with
    /// the chosen name may be missing.
    pub complete: bool,
}
