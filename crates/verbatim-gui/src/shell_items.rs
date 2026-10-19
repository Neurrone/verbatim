//! The system tray icons and taskbar buttons the systrayList replica
//! dialog lists (roadmap, milestone M3), and how an item chosen from an
//! earlier list is found again and clicked.
//!
//! The GUI makes no UIA connection into the shell's windows: it asks its
//! [`ShellItemsHost`], which the app implements by sending File Explorer's
//! outpost a query; the outpost reads the items within its own application
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

/// What enumerates the shell's items for the GUI: the app, which asks the
/// outpost of the process that owns the taskbar.
pub trait ShellItemsHost: Send + Sync {
    /// Asks for `kind`'s items and returns at once. `deliver` is called
    /// exactly once with the outcome when this returns true; false means
    /// the request could not be made (logged), and `deliver` is never
    /// called.
    fn request(&self, kind: ShellItemKind, deliver: DeliverShellItems) -> bool;
}

/// The item in a fresh enumeration, `fresh`, that is `chosen`, an item the
/// user picked from an earlier one, wherever it is now: the item with the
/// same runtime id, or else the only item with the same name. `None` when
/// it is gone, or when its name alone cannot tell it from another item's,
/// so a click never lands on an icon that merely took its place.
#[must_use]
pub fn refind<'a>(chosen: &ShellItem, fresh: &'a [ShellItem]) -> Option<&'a ShellItem> {
    if !chosen.runtime_id.is_empty()
        && let Some(item) = fresh
            .iter()
            .find(|item| item.runtime_id == chosen.runtime_id)
    {
        return Some(item);
    }
    let mut named = fresh.iter().filter(|item| item.name == chosen.name);
    match (named.next(), named.next()) {
        (Some(item), None) => Some(item),
        _ => None,
    }
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

    fn item(name: &str, left: i32, runtime_id: &[i32]) -> ShellItem {
        ShellItem {
            name: name.into(),
            rect: Rect {
                left,
                top: 0,
                width: 24,
                height: 24,
            },
            runtime_id: runtime_id.to_vec(),
        }
    }

    #[test]
    fn a_moved_item_is_found_by_its_runtime_id() {
        let chosen = item("Volume", 0, &[42, 1]);
        let fresh = [item("Network", 0, &[42, 2]), item("Volume", 24, &[42, 1])];
        assert_eq!(refind(&chosen, &fresh), Some(&fresh[1]));
    }

    #[test]
    fn an_item_with_a_new_runtime_id_is_found_by_its_unique_name() {
        let chosen = item("Volume", 0, &[42, 1]);
        let fresh = [item("Network", 0, &[42, 2]), item("Volume", 24, &[42, 7])];
        assert_eq!(refind(&chosen, &fresh), Some(&fresh[1]));
    }

    #[test]
    fn a_gone_or_ambiguous_item_is_not_found() {
        let chosen = item("Volume", 0, &[42, 1]);
        assert_eq!(refind(&chosen, &[item("Network", 0, &[42, 1, 9])]), None);
        let twins = [item("Volume", 0, &[42, 3]), item("Volume", 24, &[42, 4])];
        assert_eq!(
            refind(&chosen, &twins),
            None,
            "two items of that name, neither of them the one chosen"
        );
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
