//! The shell's tray icons and taskbar buttons, for Core's system tray list
//! dialog: the answers to [`Query::ShellItems`](verbatim_model::protocol::Query::ShellItems)
//! and [`Query::ShellItemPosition`](verbatim_model::protocol::Query::ShellItemPosition),
//! read by File Explorer's outpost within its own application, so Core
//! makes no UIA connection into the shell's windows
//! (`docs/design/listener-without-connections.md`, decision 9).
//!
//! The notification area is the `TrayNotifyWnd` child of `Shell_TrayWnd`,
//! with the overflow flyout window when it is visible; the taskbar is
//! `Shell_TrayWnd` with the notification area's subtree left out. Each
//! window is walked in the control view, each element read as
//! `verbatim_uia::ShellButton`, and every named, on-screen button becomes
//! a [`ShellItem`]. The walk reads whether each button is off the screen
//! but never where it is (`docs/architecture.md`, "Bulk reads carry only
//! cheap properties"): a click asks where the chosen item is, which the
//! outpost answers by walking the same surface again and reading
//! `verbatim_uia::Place` for each button whose runtime id or name is the
//! chosen item's, normally one, so a click lands where the button is then.
//! Which of them is the item, if any, Core decides
//! (`verbatim_gui`'s `refind`). A window the outpost's application
//! does not serve is not read: the outpost connects only to its own
//! application's windows (section 4.3 of the same design). `Shell_TrayWnd`
//! and `TrayNotifyWnd` are read through UIA whatever their provider, so
//! their lookup takes no proxy rule (`Uia::any_window_element`); the
//! overflow flyout's window has a provider of its own, so its lookup takes
//! the rule (`Uia::window_element`), and a lookup refused, as one of a
//! shell busy past the connection timeout is, reads none of its items.
//!
//! The walk has a time budget, [`WALK_BUDGET`], asked before each of its
//! calls after the first, a candidate's position read included: once it
//! has passed, the walk makes no more and the answer holds the items read
//! by then, rather than the worker's deadline for the query abandoning the
//! walk with none.

use std::cell::Cell;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationTreeWalker, UIA_ButtonControlTypeId};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, IsWindowVisible};
use windows::core::{HSTRING, PCWSTR};

use verbatim_model::{ShellCandidate, ShellCandidates, ShellItem, ShellItemKind};
use verbatim_uia::{Cached, Direction, Place, Request, ShellButton, Uia, element_is_gone};

use super::Context;
use super::read::{Client, ReadError, serves};
use crate::arbitration::time_limit;

/// Depth cap for the control-view walk; the shell trees of interest are
/// shallow, so anything deeper is runaway recursion, not real items.
const MAX_DEPTH: u32 = 16;

/// Total node cap across one walk, bounding the worst case against an
/// adversarially deep or wide provider.
const MAX_NODES: usize = 1024;

/// How long the walk may take before it makes no more calls and answers
/// the items read by then. With one call under way when it passes, which
/// UIA's transaction timeout ends after 5 seconds, a walk still answers
/// within the worker's deadline for the query
/// ([`WALK_DEADLINE`](verbatim_model::protocol::WALK_DEADLINE), 5 seconds), which
/// Core outlasts, unless that call takes longer than 3 seconds.
const WALK_BUDGET: Duration = Duration::from_secs(2);

/// The items of `kind`, read in the outpost's application.
pub(super) fn shell_items(
    context: &Context,
    client: &mut Client,
    kind: ShellItemKind,
) -> Result<Vec<ShellItem>, ReadError> {
    let uia = client
        .uia()
        .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
    let clock = context.clock();
    let out_of_time = time_limit(&clock, WALK_BUDGET);
    Ok(walk_surface(context, uia, kind, &out_of_time)?
        .found
        .into_iter()
        .map(|found| found.item)
        .collect())
}

/// The buttons of `kind` that may be `chosen`, an item the user picked
/// from an earlier list, now: the surface walked again, and every button
/// whose runtime id or name is `chosen`'s ([`is_candidate`]) read for its
/// place, one read each. A candidate gone before its read is left out; one
/// whose read fails otherwise is left out and makes the answer incomplete;
/// the budget passing before a read leaves that candidate and the rest out
/// and the answer incomplete.
pub(super) fn shell_item_position(
    context: &Context,
    client: &mut Client,
    kind: ShellItemKind,
    chosen: &ShellItem,
) -> Result<ShellCandidates, ReadError> {
    let uia = client
        .uia()
        .ok_or_else(|| ReadError::Failed("could not create a UIA client".to_owned()))?;
    let place = uia
        .request_fixed::<Place>()
        .map_err(|error| ReadError::Failed(format!("could not build a cache request: {error}")))?;
    let clock = context.clock();
    let out_of_time = time_limit(&clock, WALK_BUDGET);
    let walked = walk_surface(context, uia, kind, &out_of_time)?;
    let mut complete = walked.complete;
    let mut candidates = Vec::new();
    for found in walked
        .found
        .iter()
        .filter(|found| is_candidate(chosen, &found.item))
    {
        if out_of_time() {
            tracing::debug!(
                name = chosen.name,
                "the shell walk ran out of time before a candidate's position was read"
            );
            complete = false;
            break;
        }
        match found.element.live().refresh(&place) {
            Ok(element) => {
                let place = element.read();
                candidates.push(ShellCandidate {
                    item: found.item.clone(),
                    offscreen: place.offscreen,
                    rect: place.rect,
                });
            }
            Err(error) if element_is_gone(&error) => {}
            Err(error) => {
                // One button's failed read leaves the others to decide on;
                // the answer says it is incomplete, so a name alone is
                // not trusted to pick the button.
                tracing::warn!(
                    %error,
                    name = found.item.name,
                    "could not read a shell item's position; it is left out"
                );
                complete = false;
            }
        }
    }
    Ok(ShellCandidates {
        candidates,
        complete,
    })
}

/// Whether the walked button `item` may be `chosen`: its runtime id is
/// `chosen`'s, when `chosen` has one, or its name is.
fn is_candidate(chosen: &ShellItem, item: &ShellItem) -> bool {
    (!chosen.runtime_id.is_empty() && item.runtime_id == chosen.runtime_id)
        || item.name == chosen.name
}

/// One walked button: the item it becomes, and its element, for its
/// position when it is the one clicked.
struct Found {
    item: ShellItem,
    element: Cached<ShellButton>,
}

/// One walk of a surface.
struct Walked {
    /// The buttons listed, in the shell's order.
    found: Vec<Found>,
    /// Whether every button was read: false when the time budget or the
    /// node cap cut the walk short, or a step or a window's lookup failed.
    complete: bool,
}

/// The buttons of `kind`, walked in the outpost's application under the
/// budget `out_of_time` keeps.
fn walk_surface(
    context: &Context,
    uia: &Uia,
    kind: ShellItemKind,
    out_of_time: &dyn Fn() -> bool,
) -> Result<Walked, ReadError> {
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
        out_of_time,
        cut_short: Cell::new(false),
    };
    let mut items = Vec::new();
    match kind {
        ShellItemKind::SystemTray => {
            let notify = notification_area(shell).ok_or_else(|| {
                ReadError::Failed("the taskbar's notification area window was not found".to_owned())
            })?;
            walk.collect_from_window(notify, None, Lookup::AnyProvider, &mut items);
            if let Some(overflow) = visible_overflow_window() {
                walk.collect_from_window(overflow, None, Lookup::ProxyRule, &mut items);
            }
        }
        ShellItemKind::Taskbar => {
            // The notification area lives inside Shell_TrayWnd; excluding
            // its subtree is what makes this the taskbar list rather than
            // both lists at once.
            walk.collect_from_window(
                shell,
                notification_area(shell),
                Lookup::AnyProvider,
                &mut items,
            );
        }
    }
    let complete = !walk.cut_short.get();
    if !complete {
        tracing::debug!(
            ?kind,
            items = items.len(),
            "the shell walk was cut short; the list is partial"
        );
    }
    Ok(Walked {
        found: items,
        complete,
    })
}

/// How a shell window's element is looked up.
#[derive(Clone, Copy)]
enum Lookup {
    /// Whatever serves it, UIA's own proxies included
    /// (`Uia::any_window_element`): `Shell_TrayWnd` and `TrayNotifyWnd`.
    AnyProvider,
    /// Under the proxy rule (`Uia::window_element`): the overflow flyout's
    /// window.
    ProxyRule,
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
    /// Whether the walk's [`WALK_BUDGET`] has passed, asked before each
    /// call after the first.
    out_of_time: &'a dyn Fn() -> bool,
    /// Whether a button may have been left unread: set when the budget or
    /// the node cap stopped the walk, or a step or a window's lookup
    /// failed. The depth cap and the excluded subtree do not set it.
    cut_short: Cell<bool>,
}

impl Walk<'_> {
    /// Collects every qualifying button under `hwnd`, looked up as
    /// `lookup` says, into `items`, skipping the subtree rooted at the
    /// window `exclude` (when given). A window the outpost's application
    /// does not serve is not read.
    fn collect_from_window(
        &self,
        hwnd: HWND,
        exclude: Option<HWND>,
        lookup: Lookup,
        items: &mut Vec<Found>,
    ) {
        let window = hwnd.0 as isize;
        if !serves(self.context, window) {
            tracing::debug!(window, "a shell window of another application is not read");
            return;
        }
        if self.out_of_time_now() {
            return;
        }
        let root = match lookup {
            Lookup::AnyProvider => self.uia.any_window_element(window, self.request),
            Lookup::ProxyRule => self.uia.window_element(window, self.request),
        };
        let root = match root {
            Ok(element) => element,
            Err(error) => {
                tracing::warn!(%error, "shell window has no UIA element");
                self.cut_short.set(true);
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
        items: &mut Vec<Found>,
    ) {
        let button = element.read();
        if let Some(exclude) = exclude
            && button.hwnd == exclude.0 as isize
        {
            return;
        }
        if button.control_type == Some(UIA_ButtonControlTypeId.0) {
            if let Some(name) = listed_name(button.name, button.offscreen) {
                items.push(Found {
                    item: ShellItem {
                        name,
                        // Local: UIA keeps the runtime id with the element.
                        runtime_id: element.runtime_id(),
                    },
                    element: element.clone(),
                });
            }
            // Tray and taskbar buttons carry no nested items worth walking.
            return;
        }
        if depth >= MAX_DEPTH || self.out_of_time_now() {
            return;
        }
        let mut next = self.step(element.live().walk(
            self.walker,
            Direction::FirstChild,
            self.request,
        ));
        while let Some(current) = next {
            *visited += 1;
            if *visited >= MAX_NODES {
                tracing::debug!("shell enumeration hit its node cap; results may be partial");
                self.cut_short.set(true);
                return;
            }
            self.collect_recursive(&current, exclude, depth + 1, visited, items);
            if self.out_of_time_now() {
                return;
            }
            next = self.step(current.live().walk(
                self.walker,
                Direction::NextSibling,
                self.request,
            ));
        }
    }

    /// Whether the budget has passed, recording that the walk stops short.
    fn out_of_time_now(&self) -> bool {
        let out = (self.out_of_time)();
        if out {
            self.cut_short.set(true);
        }
        out
    }

    /// One step of the walk: the element reached, or none at the end of an
    /// element's children (the null element, an error carrying no failure
    /// code). A step that fails is read as no more items too, as NVDA reads
    /// a failed UIA step as no child, so the list keeps the items read by
    /// then; the failure is logged, naming its error, so it is not
    /// invisible, and leaves the walk incomplete.
    fn step(
        &self,
        found: windows::core::Result<Cached<ShellButton>>,
    ) -> Option<Cached<ShellButton>> {
        match found {
            Ok(element) => Some(element),
            Err(error) if error.code().is_ok() => None,
            Err(error) => {
                tracing::warn!(%error, "a shell walk step failed; its items there are not read");
                self.cut_short.set(true);
                None
            }
        }
    }
}

/// The name one walked button is listed under, if it is listed: it must
/// have a non-blank name (the list shows nothing else) and be on screen.
fn listed_name(name: Option<String>, offscreen: bool) -> Option<String> {
    if offscreen {
        return None;
    }
    name.filter(|name| !name.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, runtime_id: &[i32]) -> ShellItem {
        ShellItem {
            name: name.into(),
            runtime_id: runtime_id.to_vec(),
        }
    }

    #[test]
    fn a_named_onscreen_button_is_listed() {
        assert_eq!(
            listed_name(Some("Volume".into()), false),
            Some("Volume".to_owned())
        );
    }

    #[test]
    fn unnamed_or_blank_named_buttons_are_not_listed() {
        assert_eq!(listed_name(None, false), None);
        assert_eq!(
            listed_name(Some("   ".into()), false),
            None,
            "a whitespace-only name shows as an empty row and is dropped"
        );
    }

    #[test]
    fn offscreen_buttons_are_not_listed() {
        assert_eq!(listed_name(Some("Clock".into()), true), None);
    }

    #[test]
    fn a_button_with_the_chosen_runtime_id_or_name_is_a_candidate() {
        let chosen = item("Volume", &[42, 1]);
        assert!(is_candidate(&chosen, &item("Network", &[42, 1])));
        assert!(is_candidate(&chosen, &item("Volume", &[42, 7])));
        assert!(!is_candidate(&chosen, &item("Network", &[42, 1, 9])));
    }

    #[test]
    fn an_empty_runtime_id_matches_only_by_name() {
        let chosen = item("Volume", &[]);
        assert!(!is_candidate(&chosen, &item("Network", &[])));
        assert!(is_candidate(&chosen, &item("Volume", &[42, 7])));
    }
}
