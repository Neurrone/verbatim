//! A dialog read once it is shown, by postponement
//! (`books/dev/src/explanation/design/focus.md`, "A dialog read once it is
//! shown"; Dickson, 2026-11-20: "For the dialog: ok"). A focus read asked
//! with the hidden check that meets a hidden dialog ends its entry, and
//! Core keeps one fact, the postponed dialog ([`Postponed`]), in its focus
//! record. The listener tells of a top-level window's show when that window
//! is in front; at that tell's turn, or at a foreground report naming the
//! dialog, the focus is read again. Typed text or a command pressed while
//! the dialog is postponed and in front reads the focus first, as it is.
//! A reader's focus executed, Core's own commit and the dialog's
//! destruction end the fact; the committed focus spoken again in place
//! (`Commit::InPlace`) leaves it.

use serde::{Deserialize, Serialize};
use verbatim_model::WindowHandle;
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{NodeWindow, PressAction, Reply};
use verbatim_model::tell::Event;

use super::retake::{CurrentFocus, Why};
use crate::entries::{Entries, front_fence, request_window, window_facts};
use crate::event_queue::{Cx, Outcome, Request, RequestWindow};
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

impl<T> Hidden<T> {
    /// `reply` as a focus read's answer: [`Reply::HiddenDialog`], or the
    /// read `parse` makes of any other reply, `None` for one it refuses.
    pub(crate) fn parse(reply: Reply, parse: impl FnOnce(Reply) -> Option<T>) -> Option<Self> {
        match reply {
            Reply::HiddenDialog(window) => Some(Self::Dialog(window)),
            reply => parse(reply).map(Self::Read),
        }
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

/// Asks the focus read `request` under `due_us`: the one place a hidden
/// dialog's answer is decided for every focus read. A hidden dialog, which
/// only a read made to check decodes, postpones it, keeping the older
/// fence when the same dialog is postponed already, and `None` is
/// returned: the caller's entry ends, nothing said.
pub(crate) async fn ask<R, T>(cx: &mut Cx<Entries>, request: R, due_us: u64) -> Option<Outcome<T>>
where
    R: Request<Reply = Hidden<T>>,
{
    let dialog = match cx.ask(request, due_us).await.split() {
        Ok(Hidden::Read(read)) => return Some(Outcome::Result(read)),
        Ok(Hidden::Dialog(dialog)) => dialog,
        Err(other) => return Some(other),
    };
    let fence = front_fence(cx);
    let mut facts = cx.facts_mut();
    let postponed = &mut facts.versions.postponed;
    if postponed.is_none_or(|kept| kept.dialog.handle != dialog.handle) {
        *postponed = Some(Postponed {
            dialog: RequestWindow::of_node(&dialog),
            fence,
        });
    }
    None
}

/// The entry a tell `tell` becomes when it is the show of a top-level
/// window of a class outside NVDA's show classes, which the listener tells
/// only while the window is in front: the read at its turn, which asks
/// nothing unless that window is the postponed dialog ([`shown_due`]).
pub(crate) fn of_show(tell: &Event) -> Option<CurrentFocus> {
    let facts = window_facts(tell)?;
    if tell.kind != EventKind::Show
        || facts.window.handle != facts.root.handle
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
/// postponed dialog is `window`, and it is in front.
pub(crate) fn shown_due(cx: &Cx<Entries>, window: WindowHandle) -> bool {
    let postponed = cx
        .facts()
        .versions
        .postponed
        .is_some_and(|postponed| postponed.dialog.handle == window);
    postponed && cx.foreground().is_some_and(|front| front.window == window)
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

/// A press other than a caret key, while the postponed dialog is in front:
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
    if !shown_due(cx, postponed.dialog.handle) {
        return;
    }
    let read = CurrentFocus {
        why: Why::Retake,
        window: postponed.dialog,
        trace_id: press.trace_id,
        fence: postponed.fence,
    };
    super::retake::read_current::<super::retake::AsItIs>(cx, &read).await;
}

#[cfg(test)]
#[path = "shown_tests.rs"]
mod tests;
