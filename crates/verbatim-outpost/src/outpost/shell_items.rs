//! The shell's tray icons and taskbar buttons, for Core's system tray list
//! dialog: the answer to [`Query::ShellItems`](crate::protocol::Query::ShellItems),
//! read by File Explorer's outpost within its own application, so Core
//! makes no UIA connection into the shell's windows
//! (`docs/design/listener-without-connections.md`, decision 9).
//!
//! The notification area is the `TrayNotifyWnd` child of `Shell_TrayWnd`,
//! with the overflow flyout window when it is visible; the taskbar is
//! `Shell_TrayWnd` with the notification area's subtree left out. Each
//! window is walked in the control view, each element read as
//! `verbatim_uia::ShellButton`, and every named, on-screen button with a
//! rectangle becomes a [`ShellItem`]. A window the outpost's application
//! does not serve is not read: the outpost connects only to its own
//! application's windows (section 4.3 of the same design). The shell
//! windows are read through UIA whatever their provider, so the lookup
//! takes no proxy rule (`Uia::any_window_element`). The worker's deadline
//! for the query bounds the walk.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationTreeWalker, UIA_ButtonControlTypeId};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, IsWindowVisible};
use windows::core::{HSTRING, PCWSTR};

use verbatim_model::{Rect, ShellItem, ShellItemKind};
use verbatim_uia::{Cached, Direction, Request, ShellButton, Uia};

use super::Context;
use super::read::{Client, ReadError, serves};

/// Depth cap for the control-view walk; the shell trees of interest are
/// shallow, so anything deeper is runaway recursion, not real items.
const MAX_DEPTH: u32 = 16;

/// Total node cap across one walk, bounding the worst case against an
/// adversarially deep or wide provider.
const MAX_NODES: usize = 1024;

/// The items of `kind`, read in the outpost's application.
pub(super) fn shell_items(
    context: &Context,
    client: &mut Client,
    kind: ShellItemKind,
) -> Result<Vec<ShellItem>, ReadError> {
    let uia = client
        .uia()
        .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
    let request = uia
        .request_fixed::<ShellButton>()
        .map_err(|error| ReadError::Failed(format!("could not build a cache request: {error}")))?;
    let walker = uia
        .control_view_walker()
        .map_err(|error| ReadError::Failed(format!("could not create a tree walker: {error}")))?;
    let shell = find_top_level("Shell_TrayWnd")
        .ok_or_else(|| ReadError::Failed("the taskbar window was not found".to_owned()))?;
    if !serves(context, shell.0 as isize) {
        return Err(ReadError::Failed(
            "the taskbar window is another application's".to_owned(),
        ));
    }
    let walk = Walk {
        context,
        uia,
        walker: &walker,
        request: &request,
    };
    let mut items = Vec::new();
    match kind {
        ShellItemKind::SystemTray => {
            let notify = notification_area(shell).ok_or_else(|| {
                ReadError::Failed("the taskbar's notification area window was not found".to_owned())
            })?;
            walk.collect_from_window(notify, None, &mut items);
            if let Some(overflow) = visible_overflow_window() {
                walk.collect_from_window(overflow, None, &mut items);
            }
        }
        ShellItemKind::Taskbar => {
            // The notification area lives inside Shell_TrayWnd; excluding
            // its subtree is what makes this the taskbar list rather than
            // both lists at once.
            walk.collect_from_window(shell, notification_area(shell), &mut items);
        }
    }
    Ok(items)
}

/// The notification area window: the `TrayNotifyWnd` child of the taskbar
/// window `shell`, present on Windows 10 and 11 alike.
fn notification_area(shell: HWND) -> Option<HWND> {
    // SAFETY: FindWindowExW reads window state only; a stale parent handle
    // yields an error, not a fault.
    unsafe {
        FindWindowExW(
            Some(shell),
            None,
            &HSTRING::from("TrayNotifyWnd"),
            PCWSTR::null(),
        )
    }
    .ok()
    .filter(|hwnd| !hwnd.is_invalid())
}

/// The tray overflow flyout window, only while it is actually open:
/// Windows 11 names its class `TopLevelWindowForOverflowXamlIsland`,
/// Windows 10 `NotifyIconOverflowWindow`. Hidden overflow windows are
/// skipped, since their items report stale rectangles.
fn visible_overflow_window() -> Option<HWND> {
    [
        "TopLevelWindowForOverflowXamlIsland",
        "NotifyIconOverflowWindow",
    ]
    .into_iter()
    .filter_map(find_top_level)
    // SAFETY: IsWindowVisible reads window state only.
    .find(|&hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
}

/// Finds a top-level window by class name.
fn find_top_level(class: &str) -> Option<HWND> {
    // SAFETY: FindWindowW reads window state only.
    unsafe { FindWindowW(&HSTRING::from(class), PCWSTR::null()) }
        .ok()
        .filter(|hwnd| !hwnd.is_invalid())
}

/// What one enumeration walks with.
struct Walk<'a> {
    context: &'a Context,
    uia: &'a Uia,
    walker: &'a IUIAutomationTreeWalker,
    request: &'a Request<ShellButton>,
}

impl Walk<'_> {
    /// Collects every qualifying button under `hwnd` into `items`, skipping
    /// the subtree rooted at the window `exclude` (when given). A window
    /// the outpost's application does not serve is not read.
    fn collect_from_window(&self, hwnd: HWND, exclude: Option<HWND>, items: &mut Vec<ShellItem>) {
        let window = hwnd.0 as isize;
        if !serves(self.context, window) {
            tracing::debug!(window, "a shell window of another application is not read");
            return;
        }
        let root = match self.uia.any_window_element(window, self.request) {
            Ok(element) => element,
            Err(error) => {
                tracing::warn!(%error, "shell window has no UIA element");
                return;
            }
        };
        let mut visited = 0usize;
        self.collect_recursive(&root, exclude, 0, &mut visited, items);
    }

    /// Recursive worker for [`collect_from_window`](Self::collect_from_window):
    /// control-view children via `GetFirstChildElementBuildCache` and
    /// `GetNextSiblingElementBuildCache`. Buttons are collected and not
    /// descended into; the excluded window's subtree is skipped wholesale.
    fn collect_recursive(
        &self,
        element: &Cached<ShellButton>,
        exclude: Option<HWND>,
        depth: u32,
        visited: &mut usize,
        items: &mut Vec<ShellItem>,
    ) {
        let button = element.read();
        if let Some(exclude) = exclude
            && button.hwnd == exclude.0 as isize
        {
            return;
        }
        if button.control_type == Some(UIA_ButtonControlTypeId.0) {
            if let Some(mut item) = item_from_parts(button.name, button.offscreen, button.rect) {
                // Local: UIA keeps the runtime id with the element.
                item.runtime_id = element.runtime_id();
                items.push(item);
            }
            // Tray and taskbar buttons carry no nested items worth walking.
            return;
        }
        if depth >= MAX_DEPTH {
            return;
        }
        // An error from the walker means "no child" or "no sibling".
        let mut next = element
            .live()
            .walk(self.walker, Direction::FirstChild, self.request)
            .ok();
        while let Some(current) = next {
            *visited += 1;
            if *visited >= MAX_NODES {
                tracing::debug!("shell enumeration hit its node cap; results may be partial");
                return;
            }
            self.collect_recursive(&current, exclude, depth + 1, visited, items);
            next = current
                .live()
                .walk(self.walker, Direction::NextSibling, self.request)
                .ok();
        }
    }
}

/// Whether one walked button becomes a [`ShellItem`]: it must have a
/// non-blank name (the list shows nothing else), be on screen, and have a
/// non-empty rectangle (click actions target its center, so a degenerate
/// rectangle is unusable).
fn item_from_parts(name: Option<String>, offscreen: bool, rect: Option<Rect>) -> Option<ShellItem> {
    if offscreen {
        return None;
    }
    let name = name.filter(|name| !name.trim().is_empty())?;
    let rect = rect.filter(|rect| rect.width > 0 && rect.height > 0)?;
    Some(ShellItem {
        name,
        rect,
        runtime_id: Vec::new(),
    })
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
    fn named_onscreen_button_becomes_an_item() {
        let item = item_from_parts(Some("Volume".into()), false, Some(rect(24, 24)));
        assert_eq!(
            item,
            Some(ShellItem {
                name: "Volume".into(),
                rect: rect(24, 24),
                runtime_id: Vec::new(),
            })
        );
    }

    #[test]
    fn unnamed_or_blank_named_buttons_are_dropped() {
        assert_eq!(item_from_parts(None, false, Some(rect(24, 24))), None);
        assert_eq!(
            item_from_parts(Some("   ".into()), false, Some(rect(24, 24))),
            None,
            "a whitespace-only name shows as an empty row and is dropped"
        );
    }

    #[test]
    fn offscreen_buttons_are_dropped() {
        assert_eq!(
            item_from_parts(Some("Clock".into()), true, Some(rect(24, 24))),
            None
        );
    }

    #[test]
    fn degenerate_rectangles_are_dropped() {
        assert_eq!(item_from_parts(Some("Clock".into()), false, None), None);
        assert_eq!(
            item_from_parts(Some("Clock".into()), false, Some(rect(0, 24))),
            None,
            "a zero-width rectangle has no clickable center"
        );
        assert_eq!(
            item_from_parts(Some("Clock".into()), false, Some(rect(24, 0))),
            None,
            "a zero-height rectangle has no clickable center"
        );
    }
}
