//! A dialog read once it is shown, by postponement
//! (`books/dev/src/explanation/design/focus.md`, "A dialog read once it is
//! shown"; Dickson, 2026-11-20: "For the dialog: ok"). A focus read asked
//! with the hidden check that meets a hidden dialog ends its entry, and
//! Core keeps one fact, the postponed dialog ([`Postponed`]), in its focus
//! record. The listener tells of a top-level window's show when that window
//! is in front, which the tell's own facts say; at that tell's turn, or at
//! a foreground report naming the dialog, the focus is read again. Typed
//! text or a command pressed while the dialog is postponed and the
//! foreground record names it reads the focus first, as it is.
//! A reader's focus executed, Core's own commit and the dialog's
//! destruction end the fact; the committed focus spoken again in place
//! (`Commit::InPlace`) leaves it.

use serde::{Deserialize, Serialize};
use verbatim_model::Role;
use verbatim_model::WindowHandle;
use verbatim_model::batch::Tested;
use verbatim_model::event_kind::EventKind;
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{NodeWindow, PressAction};
use verbatim_model::tell::Event;

use super::retake::{CurrentFocus, Why};
use crate::batch::Got;
use crate::entries::{Entries, front_fence, request_window, window_facts};
use crate::event_queue::{Cx, Outcome, RequestWindow};
use crate::keys::entry::{Fence, PressEntry};

/// A focus read's answer: the read, or the hidden dialog its window's
/// top-level window is, with nothing read.
#[derive(Debug)]
pub(crate) enum Hidden<T> {
    /// Read as asked.
    Read(T),
    /// The hidden dialog's window.
    Dialog(NodeWindow),
}

/// What the hidden-dialog check found of a top-level window.
pub(crate) enum Check {
    /// It is shown, or is no dialog: the read goes on.
    Shown,
    /// It is a hidden dialog.
    Dialog(NodeWindow),
    /// It is hidden and its role could not be read: the focus read is not
    /// there, as a failed acquire is (the request list's design, section
    /// 6, "Orders kept").
    Gone,
}

/// The hidden-dialog check (Dickson, 2026-11-19: a dialog's text is read
/// only once it is shown), a `Test` of a top-level window's own object
/// through MSAA: that it is hidden, two local calls that send the
/// application nothing, and only then that its client object's role is a
/// dialog's.
pub(crate) fn check() -> Matcher {
    Matcher::All(vec![
        Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
        Matcher::IsAnyOf(
            PropertyId::Role,
            [Role::Dialog, Role::Alert, Role::PropertyPage]
                .into_iter()
                .map(PropertyValue::Role)
                .collect(),
        ),
    ])
}

/// The hidden-dialog check's outcome, `got`, judged: a check not done, its
/// window gone or no window of the application's, is no hidden dialog;
/// `None` for an outcome no correct outpost gives.
pub(crate) fn judge(got: &Got<'_, Tested>) -> Option<Check> {
    match got {
        Got::Done(tested) if tested.matched => Some(Check::Dialog(tested.node.window)),
        Got::Done(tested) => {
            let value = |id| {
                tested
                    .node
                    .values
                    .answer(id)
                    .map(|answer| answer.value.as_ref())
            };
            let hidden = value(PropertyId::WindowShown) == Some(Some(&PropertyValue::Flag(false)));
            Some(if hidden && value(PropertyId::Role) == Some(None) {
                Check::Gone
            } else {
                Check::Shown
            })
        }
        Got::NotThere(_) | Got::Failed => Some(Check::Shown),
        Got::Refused(_) | Got::Skipped => None,
    }
}

/// The postponed dialog: the hidden dialog a focus read met, with the
/// speech fence of the entry that met it first, which the read at a press
/// is stamped with, so that the press's cut silences it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Postponed {
    /// The dialog's window.
    pub(crate) dialog: RequestWindow,
    /// The postponing entry's speech fence.
    pub(crate) fence: Fence,
}

/// A focus read's outcome, `outcome`: the one place a hidden dialog's
/// answer is decided for every focus read. A hidden dialog postpones it,
/// keeping the older fence when the same dialog is postponed already, and
/// `None` is returned: the caller's entry ends, nothing said.
pub(crate) fn postpone<T>(cx: &mut Cx<Entries>, outcome: Outcome<Hidden<T>>) -> Option<Outcome<T>> {
    match outcome.split() {
        Ok(Hidden::Read(read)) => Some(Outcome::Result(read)),
        Ok(Hidden::Dialog(dialog)) => {
            postpone_dialog(cx, &dialog);
            None
        }
        Err(other) => Some(other),
    }
}

/// Postpones the hidden dialog `dialog` a focus read met, keeping the
/// older fence when it is postponed already; a dialog no process is read
/// as serving is not, since no read of it could be asked.
pub(crate) fn postpone_dialog(cx: &mut Cx<Entries>, dialog: &NodeWindow) {
    let Some(window) = RequestWindow::of_node(dialog) else {
        return;
    };
    let fence = front_fence(cx);
    let mut facts = cx.facts_mut();
    let postponed = &mut facts.versions.postponed;
    if postponed.is_none_or(|kept| kept.dialog.handle != dialog.handle) {
        *postponed = Some(Postponed {
            dialog: window,
            fence,
        });
    }
}

/// The entry a tell `tell` becomes when it is the show of a top-level
/// window of a class outside NVDA's show classes that its own facts name
/// in front, read by the listener as it placed the tell: the read at its
/// turn, which asks nothing unless that window is the postponed dialog
/// ([`shown_due`]); the foreground record is not read
/// (`books/dev/src/explanation/design/focus.md`, "A dialog read once it is
/// shown").
pub(crate) fn of_show(tell: &Event) -> Option<CurrentFocus> {
    let facts = window_facts(tell)?;
    if tell.kind != EventKind::Show
        || facts.window.handle != facts.root.handle
        || facts.foreground != facts.window.handle
        || super::acceptance::show_class(&facts.window.class)
    {
        return None;
    }
    Some(CurrentFocus {
        why: Why::Shown,
        window: request_window(tell)?,
        trace_id: tell.trace_id,
        fence: Fence::default(),
    })
}

/// Whether the show's entry for `window` reads the focus at its turn: the
/// postponed dialog is `window`. What made the entry said the window is in
/// front: the show's tell by its own facts ([`of_show`]), or the foreground
/// report whose marker joined it ([`marker`]).
pub(crate) fn shown_due(cx: &Cx<Entries>, window: WindowHandle) -> bool {
    cx.facts()
        .versions
        .postponed
        .is_some_and(|postponed| postponed.dialog.handle == window)
}

/// The foreground marker for `window` at its turn: when `window` is the
/// postponed dialog, the show's entry joins, judged as the show's tell.
pub(crate) fn marker(cx: &mut Cx<Entries>, window: WindowHandle) {
    let Some(postponed) = cx
        .facts()
        .versions
        .postponed
        .filter(|postponed| postponed.dialog.handle == window)
    else {
        return;
    };
    let read = CurrentFocus {
        why: Why::Shown,
        window: postponed.dialog,
        trace_id: cx.facts().queue_trace.0,
        fence: Fence::default(),
    };
    cx.join(crate::entries::Entry::CurrentFocus(Box::new(read)));
}

/// A press other than a caret key, while the foreground record names the
/// postponed dialog. A key carries no window facts of its own, and the
/// keyboard hook's view of the foreground is made from that record. Then
/// the focus is read first, as it is, stamped with the postponing entry's
/// fence and committed, so that the press is judged against it and its
/// cut silences what the read says. A caret key, whose action was bound at
/// its press, reads nothing, and the show says the dialog, as NVDA's
/// captures of Right Arrow do. Any other outcome leaves the fact as it was.
pub(crate) async fn before_press(cx: &mut Cx<Entries>, press: &PressEntry) {
    // A caret key leaves the dialog for its show, as NVDA's captures of
    // caret keys do; a key passed to the application arrives as a caret key
    // of the motion `Other` and reads the dialog first, as every other key.
    let judges = !matches!(
        press.press.action,
        PressAction::CaretKey { key, .. } if key.motion != verbatim_model::CaretMotion::Other
    );
    let Some(postponed) = cx.facts().versions.postponed.filter(|_| judges) else {
        return;
    };
    let dialog = postponed.dialog.handle;
    if cx.foreground().is_none_or(|front| front.window != dialog) {
        return;
    }
    let read = CurrentFocus {
        why: Why::Retake,
        window: postponed.dialog,
        trace_id: press.trace_id,
        fence: postponed.fence,
    };
    super::retake::read_current(cx, &read, super::retake::Depth::AsItIs).await;
}

#[cfg(test)]
#[path = "shown_tests.rs"]
mod tests;
