//! The system tray icons and taskbar buttons the systrayList replica
//! dialog lists (roadmap, milestone M3), and where an item chosen from an
//! earlier list is when it is clicked.
//!
//! The GUI makes no UIA connection into the shell's windows: it asks its
//! [`ShellItemsHost`], which the app implements by sending File Explorer's
//! outpost a query; the outpost reads the items, or the position of the
//! one item clicked, which no list carries, within its own application
//! (`docs/design/listener-without-connections.md`, decision 9) and the
//! answer comes back through the host's delivery callback, on another
//! thread, which the GUI uses to send it down the same channel
//! [`GuiHandle`](crate::GuiHandle) posts commands through. The GUI thread
//! never waits for the shell.

use verbatim_model::Rect;
pub use verbatim_model::{ShellItem, ShellItemKind};

/// Where an enumeration's outcome goes: `Some(items)` on success, possibly
/// empty, or `None` when it failed or passed its deadline (already logged).
/// Called once, on a thread that is not the GUI's.
pub type DeliverShellItems = Box<dyn FnOnce(Option<Vec<ShellItem>>) + Send>;

/// Where a position's outcome goes: `Some(rect)`, the chosen item's screen
/// rectangle now, or `None` when the item is gone, cannot be told apart by
/// its name, has no point to click, or the request failed or passed its
/// deadline (already logged). Called once, on a thread that is not the
/// GUI's.
pub type DeliverShellItemPosition = Box<dyn FnOnce(Option<Rect>) + Send>;

/// What the GUI asks its [`ShellItemsHost`] about one shell surface.
pub enum ShellAsk {
    /// The surface's items, for the list.
    Items(DeliverShellItems),
    /// Where an item chosen from an earlier list of the surface is now, for
    /// a click on it: the outpost finds it again by its runtime id, or else
    /// by a name no other item has.
    Position(ShellItem, DeliverShellItemPosition),
}

impl ShellAsk {
    /// Hands the asker the outcome of a request that got no answer.
    pub fn answer_nothing(self) {
        match self {
            Self::Items(deliver) => deliver(None),
            Self::Position(_, deliver) => deliver(None),
        }
    }
}

/// What reads the shell's items for the GUI: the app, which asks the
/// outpost of the process that owns the taskbar.
pub trait ShellItemsHost: Send + Sync {
    /// Asks `ask` of `kind`'s surface and returns at once. Its delivery
    /// callback is called exactly once with the outcome when this returns
    /// true; false means the request could not be made (logged), and the
    /// callback is never called.
    fn request(&self, kind: ShellItemKind, ask: ShellAsk) -> bool;
}

/// The screen point click actions target: the center of `rect`.
#[must_use]
pub fn center_of(rect: Rect) -> (i32, i32) {
    // Saturating, since the rectangle came from another process.
    (
        rect.left.saturating_add(rect.width / 2),
        rect.top.saturating_add(rect.height / 2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: i32, height: i32) -> Rect {
        Rect {
            left: 100,
            top: 200,
            width,
            height,
        }
    }

    #[test]
    fn center_is_the_rectangle_midpoint() {
        assert_eq!(center_of(rect(24, 32)), (112, 216));
        assert_eq!(
            center_of(Rect {
                left: -10,
                top: 5,
                width: 4,
                height: 3,
            }),
            (-8, 6),
            "integer division truncates toward zero, still inside the rectangle"
        );
    }
}
