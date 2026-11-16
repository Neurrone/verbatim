//! A focus tell judged at a moment with no read, from the facts it
//! carries: NVDA's focus overrides, and whether it counts as pending
//! (design, "Terms", "A focus is pending", and "Focus and foreground").
//! NVDA counts as pending only the focuses queued after these judgements,
//! and every judgement is made on the tell's facts, so this is exact.

use verbatim_model::Role;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::MsaaIds;
use verbatim_model::tell::{Event, EventObject, Read, TellWindow};

use super::acceptance;
use super::backend::{BackendCache, Judged, JudgedWindow, Judgement, WindowKey, by_class};
use super::duplicate::{msaa_duplicate, uia_duplicate};
use super::versions::LastFocus;

/// File Explorer, by its executable name.
const FILE_EXPLORER: &str = "explorer";

/// What an override makes of a UI Automation focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Override {
    /// No override applies.
    None,
    /// Refused at its tell.
    Refused,
    /// Let through whatever its keyboard-focus answer, and redirected at
    /// its read to its nearest popup menu ancestor within four parents.
    NetUiMenuItem,
}

fn text(values: &PropertyValues, id: PropertyId) -> Option<&str> {
    match values.answer(id).and_then(|answer| answer.value.as_ref()) {
        Some(PropertyValue::Text(text)) => Some(text),
        _ => None,
    }
}

fn role(values: &PropertyValues) -> Option<Role> {
    match values
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
}

/// NVDA's focus overrides for a UI Automation focus `tell`, from its
/// cached automation id, class name and role and its nearest window's
/// class: the console window's own element, Excel's `EXCEL6` pane and an
/// unnamed first `NetUITWMenuItem`; and File Explorer's task switcher
/// frame, by the tell's application name.
pub(crate) fn focus_override(tell: &Event) -> Override {
    let values = &tell.cached;
    let window_class = match &tell.window {
        TellWindow::Found(facts) => Some(facts.window.class.as_str()),
        TellWindow::NoWindow | TellWindow::Failed => None,
    };
    let automation_id = text(values, PropertyId::AutomationId);
    let class_name = text(values, PropertyId::ClassName);
    let role = role(values);
    if automation_id == Some("Console Window") && window_class == Some("ConsoleWindowClass") {
        return Override::Refused;
    }
    if window_class == Some("EXCEL6") && role == Some(Role::Pane) {
        return Override::Refused;
    }
    if tell.application == FILE_EXPLORER
        && matches!(
            (class_name, role),
            (Some("MultitaskingViewFrame"), Some(Role::Window))
                | (
                    Some("Windows.UI.Input.InputSite.WindowClass"),
                    Some(Role::Pane)
                )
        )
    {
        return Override::Refused;
    }
    let unnamed = text(values, PropertyId::Name).is_none_or(str::is_empty);
    let first = tell
        .checked
        .as_ref()
        .and_then(|checked| checked.previous_sibling)
        == Some(Read::Answered(false));
    if class_name == Some("NetUITWMenuItem") && role == Some(Role::MenuItem) && unnamed && first {
        return Override::NetUiMenuItem;
    }
    Override::None
}

/// The window `tell` is judged by, as the backend cache judges it.
fn judged_window(tell: &Event) -> Option<JudgedWindow> {
    let TellWindow::Found(facts) = &tell.window else {
        return None;
    };
    Some(JudgedWindow {
        key: WindowKey {
            handle: facts.window.handle.0,
            thread: facts.window.thread,
        },
        by_class: by_class(&facts.window.class, &facts.root.class),
    })
}

/// Whether the focus-kind `tell` counts as pending at `now_us`: judged
/// with no read, its acceptance passes, its verdict does not drop it, an
/// override does not refuse it, a UI Automation focus's keyboard-focus
/// answer let it through, and it is not a duplicate of the readers' last
/// focus. A UI Automation focus whose walk found no window is dropped, as
/// NVDA's native-element test drops it.
pub(crate) fn counts_as_pending(
    tell: &Event,
    last: Option<&LastFocus>,
    backend: &BackendCache,
    now_us: u64,
) -> bool {
    let uia = matches!(tell.kind, EventKind::UiaFocus | EventKind::UiaMenuOpened);
    let msaa = matches!(tell.kind, EventKind::MsaaFocus | EventKind::Foreground);
    if !uia && !msaa {
        return false;
    }
    let facts = match &tell.window {
        TellWindow::Found(facts) => Some(facts.as_ref()),
        TellWindow::NoWindow | TellWindow::Failed if uia => return false,
        TellWindow::NoWindow | TellWindow::Failed => None,
    };
    if let Some(window) = judged_window(tell) {
        let judged = if uia { Judged::Uia } else { Judged::Msaa };
        if backend.judge(judged, &window, tell.at_us, now_us) == Judgement::Drop {
            return false;
        }
    }
    let overridden = if uia {
        focus_override(tell)
    } else {
        Override::None
    };
    if overridden == Override::Refused {
        return false;
    }
    if acceptance::accept(tell.kind, facts, false).is_err() {
        return false;
    }
    match &tell.object {
        EventObject::Uia { runtime_id, .. } => {
            let focused = tell
                .checked
                .as_ref()
                .and_then(|checked| checked.keyboard_focus);
            let through =
                overridden == Override::NetUiMenuItem || focused != Some(Read::Answered(false));
            through && !uia_duplicate(last, runtime_id, tell.checked.as_ref())
        }
        EventObject::Msaa {
            window,
            object,
            child,
        } => {
            tell.kind == EventKind::Foreground
                || !msaa_duplicate(
                    last,
                    &MsaaIds {
                        window: *window,
                        object: *object,
                        child: *child,
                    },
                )
        }
        // A focus is never told by an outpost's `Follow` group.
        EventObject::Followed { .. } => false,
    }
}

#[cfg(test)]
#[path = "pending_tests.rs"]
mod tests;
