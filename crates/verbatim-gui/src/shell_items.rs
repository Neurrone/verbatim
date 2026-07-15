//! Enumerating system tray icons and taskbar buttons over UIA, for the
//! systrayList replica dialog (roadmap, milestone M3).
//!
//! Enumeration goes through the existing `verbatim-uia` client over the
//! shell windows, not the NVDA add-on's per-Windows-build window-class
//! walks: [`request_shell_items`] resolves the notification area (a
//! `TrayNotifyWnd` child of `Shell_TrayWnd`, plus the overflow flyout
//! window when it is visible) or the taskbar (`Shell_TrayWnd` with the
//! notification area's subtree excluded), walks the control view with the
//! base cache request extended by the bounding rectangle, and collects
//! every named, on-screen button as a [`ShellItem`] of name and screen
//! rectangle.
//!
//! Threading. Cross-process UIA calls must never run on the GUI thread or
//! any input or speech thread, so the query runs on a short-lived worker
//! thread spawned per request. A hung shell must not hang Verbatim: a
//! sibling guard thread waits on the worker with a deadline and abandons it
//! on expiry (a blocked cross-process COM call cannot be safely cancelled —
//! the same reasoning as `verbatim-outpost`'s query pool, kept local and
//! simple here because this is a one-shot query, not an outpost). Either
//! way the outcome is handed to the GUI thread through wxDragon's
//! call-after queue, the same channel [`GuiHandle`](crate::GuiHandle)
//! posts commands through.

use std::thread;
use std::time::Duration;

use crossbeam_channel::{RecvTimeoutError, bounded};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationTreeWalker,
    UIA_BoundingRectanglePropertyId, UIA_ButtonControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, IsWindowVisible};
use windows::core::{HSTRING, PCWSTR};

use verbatim_model::Rect;
use verbatim_uia::Uia;

/// Which shell surface to enumerate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellItemKind {
    /// The system tray: the notification area plus the overflow flyout
    /// window when it is open.
    SystemTray,
    /// The taskbar buttons: everything on the taskbar outside the
    /// notification area.
    Taskbar,
}

/// One enumerated tray icon or taskbar button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellItem {
    /// The item's accessible name, shown in the list dialog.
    pub name: String,
    /// The item's screen rectangle; click actions target its center.
    pub rect: Rect,
}

/// How long the guard thread waits for the enumeration worker before
/// abandoning it. Generous relative to the outpost's 300 millisecond
/// per-call deadline because one enumeration is many cross-process calls,
/// but short enough that a hung shell answers a gesture with a logged
/// failure rather than a dialog that arrives half a minute late.
const ENUMERATION_DEADLINE: Duration = Duration::from_secs(3);

/// Depth cap for the control-view walk; the shell trees of interest are
/// shallow, so anything deeper is runaway recursion, not real items.
const MAX_DEPTH: u32 = 16;

/// Total node cap across one walk, bounding the worst case against an
/// adversarially deep or wide provider.
const MAX_NODES: usize = 1024;

/// Enumerates `kind` on a worker thread and hands the outcome to the GUI
/// thread through wxDragon's call-after queue.
///
/// `present` runs on the GUI thread with `Some(items)` on success (possibly
/// empty) or `None` when enumeration failed or exceeded its deadline (both
/// already logged); it is invoked exactly once. Callable from any thread.
pub fn request_shell_items<F>(kind: ShellItemKind, present: F)
where
    F: FnOnce(Option<Vec<ShellItem>>) + Send + 'static,
{
    let guard = thread::Builder::new()
        .name("verbatim-shell-guard".to_owned())
        .spawn(move || {
            let outcome = enumerate_with_deadline(kind);
            wxdragon::call_after(Box::new(move || present(outcome)));
            wxdragon::wake_up_idle();
        });
    if let Err(error) = guard {
        tracing::warn!(%error, "could not spawn the shell enumeration guard thread");
    }
}

/// Runs [`enumerate`] on its own worker thread and waits up to
/// [`ENUMERATION_DEADLINE`] for the answer. On expiry the worker is
/// abandoned — it may complete later, but its send lands in a dropped
/// channel and the thread exits.
fn enumerate_with_deadline(kind: ShellItemKind) -> Option<Vec<ShellItem>> {
    let (result_tx, result_rx) = bounded(1);
    let worker = thread::Builder::new()
        .name("verbatim-shell-enum".to_owned())
        .spawn(move || {
            let _ = result_tx.send(enumerate(kind));
        });
    if let Err(error) = worker {
        tracing::warn!(%error, "could not spawn the shell enumeration worker thread");
        return None;
    }
    match result_rx.recv_timeout(ENUMERATION_DEADLINE) {
        Ok(items) => items,
        Err(RecvTimeoutError::Timeout) => {
            tracing::warn!(
                ?kind,
                deadline_ms = ENUMERATION_DEADLINE.as_millis(),
                "shell enumeration exceeded its deadline; worker abandoned"
            );
            None
        }
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

/// The whole enumeration, run on the worker thread: create a UIA client,
/// resolve the shell windows for `kind`, and collect their buttons.
fn enumerate(kind: ShellItemKind) -> Option<Vec<ShellItem>> {
    let uia = match Uia::new() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "shell enumeration could not create a UIA client");
            return None;
        }
    };
    let cache = match extended_cache_request(&uia) {
        Ok(cache) => cache,
        Err(error) => {
            tracing::warn!(%error, "shell enumeration could not build its cache request");
            return None;
        }
    };
    // SAFETY: the client is a live IUIAutomation on this thread.
    let walker = match unsafe { uia.client().ControlViewWalker() } {
        Ok(walker) => walker,
        Err(error) => {
            tracing::warn!(%error, "shell enumeration could not create a tree walker");
            return None;
        }
    };

    let mut items = Vec::new();
    match kind {
        ShellItemKind::SystemTray => {
            let Some(notify) = notification_area() else {
                tracing::warn!("the taskbar's notification area window was not found");
                return None;
            };
            collect_from_window(&uia, &walker, &cache, notify, None, &mut items);
            if let Some(overflow) = visible_overflow_window() {
                collect_from_window(&uia, &walker, &cache, overflow, None, &mut items);
            }
        }
        ShellItemKind::Taskbar => {
            let Some(shell) = find_top_level("Shell_TrayWnd") else {
                tracing::warn!("the taskbar window was not found");
                return None;
            };
            // The notification area lives inside Shell_TrayWnd; excluding
            // its subtree is what makes this the taskbar list rather than
            // both lists at once.
            collect_from_window(
                &uia,
                &walker,
                &cache,
                shell,
                notification_area(),
                &mut items,
            );
        }
    }
    Some(items)
}

/// The base cache request extended with the bounding rectangle, which the
/// base set does not carry (events never need it; this dialog does).
fn extended_cache_request(uia: &Uia) -> windows::core::Result<IUIAutomationCacheRequest> {
    let cache = uia.base_cache_request()?;
    // SAFETY: adding a property to a freshly built, not-yet-used cache
    // request; the property id is a valid UIA constant.
    unsafe { cache.AddProperty(UIA_BoundingRectanglePropertyId) }?;
    Ok(cache)
}

/// The notification area window: the `TrayNotifyWnd` child of
/// `Shell_TrayWnd`, present on Windows 10 and 11 alike.
fn notification_area() -> Option<HWND> {
    let shell = find_top_level("Shell_TrayWnd")?;
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
/// skipped — their items report stale rectangles.
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

/// Collects every qualifying button under `hwnd` into `items`, skipping the
/// subtree rooted at the window `exclude` (when given).
fn collect_from_window(
    uia: &Uia,
    walker: &IUIAutomationTreeWalker,
    cache: &IUIAutomationCacheRequest,
    hwnd: HWND,
    exclude: Option<HWND>,
    items: &mut Vec<ShellItem>,
) {
    let root = match uia.element_from_handle(hwnd.0 as isize, cache) {
        Ok(element) => element,
        Err(error) => {
            tracing::warn!(%error, "shell window has no UIA element");
            return;
        }
    };
    let mut visited = 0usize;
    collect_recursive(walker, cache, &root, exclude, 0, &mut visited, items);
}

/// Recursive worker for [`collect_from_window`]: control-view children via
/// `GetFirstChildElementBuildCache` and `GetNextSiblingElementBuildCache`,
/// the same shape as `Uia::walk_tree`. Buttons are collected and not
/// descended into; the excluded window's subtree is skipped wholesale.
fn collect_recursive(
    walker: &IUIAutomationTreeWalker,
    cache: &IUIAutomationCacheRequest,
    element: &IUIAutomationElement,
    exclude: Option<HWND>,
    depth: u32,
    visited: &mut usize,
    items: &mut Vec<ShellItem>,
) {
    if let Some(exclude) = exclude
        // SAFETY: reading a cached property of a live element.
        && unsafe { element.CachedNativeWindowHandle() }.is_ok_and(|hwnd| hwnd == exclude)
    {
        return;
    }

    // SAFETY: reading cached properties of a live element built with a
    // cache request carrying all of them.
    let control_type = unsafe { element.CachedControlType() }.unwrap_or_default();
    if control_type == UIA_ButtonControlTypeId {
        // SAFETY: as above; every property read here was prefetched.
        let (name, offscreen, rect) = unsafe {
            (
                element.CachedName().ok().map(|name| name.to_string()),
                element
                    .CachedIsOffscreen()
                    .is_ok_and(windows::core::BOOL::as_bool),
                element.CachedBoundingRectangle().ok().map(|rect| Rect {
                    left: rect.left,
                    top: rect.top,
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                }),
            )
        };
        if let Some(item) = item_from_parts(name, offscreen, rect) {
            items.push(item);
        }
        // Tray and taskbar buttons carry no nested items worth walking.
        return;
    }

    if depth >= MAX_DEPTH {
        return;
    }
    // SAFETY: walker, element, and cache are live and from this thread's
    // client; an Err from the walker means "no child / no sibling", the
    // same convention Uia::walk_tree relies on.
    let mut next = unsafe { walker.GetFirstChildElementBuildCache(element, cache) }.ok();
    while let Some(current) = next {
        *visited += 1;
        if *visited >= MAX_NODES {
            tracing::debug!("shell enumeration hit its node cap; results may be partial");
            return;
        }
        collect_recursive(walker, cache, &current, exclude, depth + 1, visited, items);
        // SAFETY: as above.
        next = unsafe { walker.GetNextSiblingElementBuildCache(&current, cache) }.ok();
    }
}

/// The pure filter deciding whether one walked button becomes a
/// [`ShellItem`]: it must have a non-blank name (the list shows nothing
/// else), be on screen, and have a non-empty rectangle (click actions
/// target its center, so a degenerate rectangle is unusable).
fn item_from_parts(name: Option<String>, offscreen: bool, rect: Option<Rect>) -> Option<ShellItem> {
    if offscreen {
        return None;
    }
    let name = name.filter(|name| !name.trim().is_empty())?;
    let rect = rect.filter(|rect| rect.width > 0 && rect.height > 0)?;
    Some(ShellItem { name, rect })
}

/// The screen point click actions target: the center of `rect`.
#[must_use]
pub fn center_of(rect: Rect) -> (i32, i32) {
    (rect.left + rect.width / 2, rect.top + rect.height / 2)
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
