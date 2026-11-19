//! A dialog read once it is shown (Dickson, 2026-11-19;
//! `books/dev/src/explanation/design/focus.md`, "A dialog read once it is
//! shown"). A dialog hidden when its focus or foreground is read gives no
//! text, since its static children are hidden too, so a focus read whose
//! window's top-level window is a hidden dialog is answered with that
//! window alone, and nothing more is read. The entry then asks the listener
//! to tell it when the window is shown, and reads the same focus again from
//! its start, under the same due time: the dialog's name, text, ancestors
//! and field are all read once it is shown.
//!
//! The wait ends early when the user is elsewhere, a condition on the queue
//! rather than an event: it holds whenever the input that makes it true
//! arrived, so it stops a wait from beginning as well as ending one
//! ([`ending`]). Nothing is then spoken for the hidden dialog, and the
//! entries behind are judged as they came. A key pressed while the dialog
//! is hidden ends the wait too, but does not skip the dialog: the focus is
//! read at once, hidden, and committed before the key is handled, so a
//! quick key skips the dialog's text and not the dialog.
//!
//! No focus or foreground event is ignored for being hidden, as NVDA ignores
//! none (`docs/parity/focus-context.md`, "A dialog's text"): only the read
//! is postponed, then judged by the same rules.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::WindowHandle;
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{CancelReason, HiddenRoot, NodeWindow, Reply, Request as Wire};

use super::acceptance;
use super::retake::{CurrentFocus, Why};
use super::validity::{self, Subject, Validity, View};
use crate::entries::{Entries, Entry, request_window};
use crate::event_queue::{Cx, Foreground, Item, Outcome, Reader, Refused, Request, RequestWindow};
use crate::focus::backend::BackendCache;
use crate::state::SrState;

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

/// The listener's request to be told when a hidden dialog's window is
/// shown.
#[derive(Debug)]
pub(crate) struct AwaitShown {
    /// The dialog's window.
    pub(crate) window: RequestWindow,
}

impl Request for AwaitShown {
    type Reply = ();

    fn reader(&self) -> Reader {
        Reader::Listener
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::AwaitShown {
            window: self.window.handle,
        }
    }

    fn decode(reply: Reply) -> Option<()> {
        matches!(reply, Reply::Shown).then_some(())
    }
}

/// Whether `request` is a wait for a window's show, which stops by its own
/// rule ([`stops`]) rather than by the foreground's tree and owner chain.
pub(crate) fn waits(request: &Wire) -> bool {
    matches!(request, Wire::AwaitShown { .. })
}

/// What ends a wait for a hidden dialog's show, besides its show and its
/// due time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Ending {
    /// The user is elsewhere: nothing is spoken for the dialog.
    Left,
    /// A key was pressed: the focus is read at once, hidden, and committed
    /// before the key is handled.
    Key,
}

/// The ending condition for the hidden dialog `dialog`, a pure function of
/// the foreground record `foreground`, the entries waiting behind the
/// front, `waiting`, and Core's facts `view` (Dickson, 2026-11-19, his
/// option C and the keypress). It is the user being elsewhere when any of
/// these holds:
///
/// - the foreground record names a window other than `dialog` (a
///   top-level window's root is itself), owner and owned windows included;
/// - a menu or popup start waits for a window whose top-level window is not
///   `dialog`, accepted, from the application holding attention;
/// - an MSAA or UI Automation focus waits for a window whose top-level
///   window is not `dialog`, that the one definition's rows needing no read
///   accept: valid, or needing only its read ([`focus_ends`]).
///
/// Otherwise a key waiting, any key, ends it as [`Ending::Key`].
pub(crate) fn ending(
    dialog: WindowHandle,
    waiting: &VecDeque<Item<Entry>>,
    view: &View<'_>,
    foreground: Option<&Foreground>,
) -> Option<Ending> {
    let elsewhere = foreground.is_some_and(|foreground| foreground.window != dialog)
        || entries(waiting)
            .any(|entry| menu_ends(entry, dialog, foreground) || focus_ends(entry, dialog, view));
    if elsewhere {
        return Some(Ending::Left);
    }
    entries(waiting)
        .any(|entry| matches!(entry, Entry::Press(_)))
        .then_some(Ending::Key)
}

/// Every entry of `waiting`, in arrival order, a run's candidates and the
/// menu slot's entry included.
fn entries(waiting: &VecDeque<Item<Entry>>) -> impl Iterator<Item = &Entry> {
    waiting.iter().flat_map(|item| match item {
        Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_ref(entry),
        Item::Run(entries) => entries.as_slice(),
    })
}

/// Whether `entry` is a menu or popup start outside `dialog`, accepted,
/// from the application holding attention: a background application's
/// does not end a wait.
fn menu_ends(entry: &Entry, dialog: WindowHandle, foreground: Option<&Foreground>) -> bool {
    let Entry::Menu(candidate) = entry else {
        return false;
    };
    matches!(
        candidate.tell.kind,
        EventKind::MenuStart | EventKind::PopupStart
    ) && candidate.window.top_level != dialog
        && acceptance::accept(candidate.tell.kind, candidate.facts(), false).is_ok()
        && foreground.is_some_and(|foreground| foreground.attends(&candidate.window))
}

/// Whether `entry` is a focus outside `dialog` that the rows of the one
/// definition needing no read accept, judged as its own turn would judge
/// it now: acceptance, the verdict and the native-element test, the
/// overrides and the duplicate test. A focus valid, or needing only the
/// read of its focused state, counts; a refused one, a duplicate and a UI
/// Automation focus with no window do not.
fn focus_ends(entry: &Entry, dialog: WindowHandle, view: &View<'_>) -> bool {
    let answer = match entry {
        Entry::Candidate(candidate) if candidate.tell.kind == EventKind::MsaaFocus => {
            if candidate.window.top_level == dialog {
                return false;
            }
            validity::judge(
                Subject::Msaa(candidate),
                view,
                candidate.confirmation.facts(),
            )
        }
        Entry::UiaFocus(focus) if focus.tell.kind == EventKind::UiaFocus => {
            if request_window(&focus.tell).is_none_or(|window| window.top_level == dialog) {
                return false;
            }
            validity::judge(Subject::Uia(focus), view, None)
        }
        _ => return false,
    };
    matches!(answer, Validity::Valid(_) | Validity::NeedsRead(_))
}

/// The front's request `request` stops now when it is a wait for a window's
/// show whose ending condition holds ([`ending`]), judged as it begins and
/// after every input while it waits: for the user being elsewhere, and
/// withdrawn for a key, whose read follows at once.
pub(crate) fn stops(
    facts: &SrState,
    request: &Wire,
    waiting: &VecDeque<Item<Entry>>,
    backend: &BackendCache,
    (now_us, foreground): (u64, Option<Foreground>),
) -> Option<CancelReason> {
    let Wire::AwaitShown { window } = request else {
        return None;
    };
    let view = View {
        versions: &facts.versions,
        in_front: foreground.map(|foreground| foreground.window),
        backend,
        now_us,
        run_before_valid: false,
    };
    match ending(*window, waiting, &view, foreground.as_ref())? {
        Ending::Left => Some(CancelReason::LeftHiddenDialog),
        Ending::Key => Some(CancelReason::Withdrawn),
    }
}

/// The oldest waiting focus that ends `dialog`'s wait ([`focus_ends`])
/// carries `dialog`, so that its run, should it execute nothing, reads the
/// focus again while the dialog is in front ([`after_ended_wait`]).
fn mark_ending_focus(cx: &mut Cx<Entries>, dialog: WindowHandle) {
    let place = validity::with_view(cx, false, |view| {
        cx.waiting().iter().enumerate().find_map(|(at, item)| {
            let found = match item {
                Item::One(entry) | Item::Menu { entry, .. } => {
                    focus_ends(entry, dialog, view).then_some(None)
                }
                Item::Run(entries) => entries
                    .iter()
                    .position(|entry| focus_ends(entry, dialog, view))
                    .map(Some),
            };
            found.map(|inner| (at, inner))
        })
    });
    let Some((at, inner)) = place else {
        return;
    };
    let mut waiting = cx.waiting_mut();
    let entry = match (waiting.get_mut(at), inner) {
        (Some(Item::One(entry) | Item::Menu { entry, .. }), None) => Some(entry),
        (Some(Item::Run(entries)), Some(inner)) => entries.get_mut(inner),
        _ => None,
    };
    if let Some(slot) = entry.and_then(ended_wait_mut)
        && slot.is_none()
    {
        *slot = Some(dialog);
    }
}

/// The hidden dialog `entry` ended the wait of, to change: a focus's.
fn ended_wait_mut(entry: &mut Entry) -> Option<&mut Option<WindowHandle>> {
    match entry {
        Entry::Candidate(candidate) => Some(&mut candidate.ended_wait),
        Entry::UiaFocus(focus) => Some(&mut focus.ended_wait),
        _ => None,
    }
}

/// `removed`, merged into the newer `into` for the same object, leaves it
/// the hidden dialog whose wait it ended, as the probe is kept through a
/// merge.
pub(crate) fn merged(removed: &Entry, into: &mut Entry) {
    let carried = match removed {
        Entry::Candidate(candidate) => candidate.ended_wait,
        Entry::UiaFocus(focus) => focus.ended_wait,
        _ => None,
    };
    if let (Some(dialog), Some(slot)) = (carried, ended_wait_mut(into))
        && slot.is_none()
    {
        *slot = Some(dialog);
    }
}

/// How a wait for a hidden dialog's show ended, for the read that met the
/// dialog.
enum Waited<T> {
    /// Read again from its start, checking for a hidden dialog or, after a
    /// key, reading as it is.
    Again(HiddenRoot),
    /// The read ends with this outcome.
    Ended(Outcome<T>),
}

/// Waits for `dialog`'s show under `due_us`. The ending condition is
/// judged by the queue as the request begins and after every input while
/// it waits ([`stops`]), so the outcome alone says which ending held, and
/// a function rebuilt from its checkpoint decides alike: the user
/// elsewhere ends the read, nothing spoken; a key, the request withdrawn,
/// reads it again at once, hidden. Shown, it is read again; any other
/// outcome of the wait is the read's: not there when the window was
/// destroyed or its application exited, not responding when it hung, the
/// due time, or the listener ended.
async fn wait<T>(cx: &mut Cx<Entries>, dialog: &NodeWindow, due_us: u64) -> Waited<T> {
    let request = AwaitShown {
        window: RequestWindow::of_node(dialog),
    };
    let outcome = cx.ask(request, due_us).await;
    match outcome {
        Outcome::Result(()) => Waited::Again(HiddenRoot::Check),
        Outcome::NoLongerAwaited(CancelReason::Withdrawn) => Waited::Again(HiddenRoot::ReadAnyway),
        Outcome::NoLongerAwaited(CancelReason::LeftHiddenDialog) => {
            mark_ending_focus(cx, dialog.handle);
            Waited::Ended(Outcome::NoLongerAwaited(CancelReason::LeftHiddenDialog))
        }
        outcome => Waited::Ended(unanswered(outcome)),
    }
}

/// An outcome without a result as one of another request's: the wait's,
/// as the read's.
fn unanswered<T>(outcome: Outcome<()>) -> Outcome<T> {
    match outcome {
        Outcome::Result(()) => Outcome::Refused(Refused::Undecodable),
        Outcome::Refused(refused) => Outcome::Refused(refused),
        Outcome::NotThere => Outcome::NotThere,
        Outcome::NotResponding => Outcome::NotResponding,
        Outcome::ReaderEnded => Outcome::ReaderEnded,
        Outcome::NoLongerAwaited(reason) => Outcome::NoLongerAwaited(reason),
    }
}

/// A focus read made by `make` under `due_us`, read again from its start
/// while it answers a hidden dialog that is then shown ([`wait`]): `make`
/// builds the request, checking for a hidden dialog or reading as it is,
/// with what its answer is judged against, made afresh for each read so
/// that nothing of an earlier one is kept. A hidden dialog answered to a
/// read made to read as it is breaks the request's rule: undecodable.
pub(crate) async fn read_shown<R, T, X>(
    cx: &mut Cx<Entries>,
    due_us: u64,
    mut make: impl FnMut(&Cx<Entries>, HiddenRoot) -> (R, X),
) -> Outcome<(T, X)>
where
    R: Request<Reply = Hidden<T>>,
{
    let mut hidden = HiddenRoot::Check;
    loop {
        let (request, with) = make(cx, hidden);
        let outcome = cx.ask(request, due_us).await;
        let dialog = match outcome {
            Outcome::Result(Hidden::Read(read)) => return Outcome::Result((read, with)),
            Outcome::Result(Hidden::Dialog(dialog)) if hidden == HiddenRoot::Check => dialog,
            Outcome::Result(Hidden::Dialog(_)) => return Outcome::Refused(Refused::Undecodable),
            Outcome::Refused(refused) => return Outcome::Refused(refused),
            Outcome::NotThere => return Outcome::NotThere,
            Outcome::NotResponding => return Outcome::NotResponding,
            Outcome::ReaderEnded => return Outcome::ReaderEnded,
            Outcome::NoLongerAwaited(reason) => return Outcome::NoLongerAwaited(reason),
        };
        match wait(cx, &dialog, due_us).await {
            Waited::Again(again) => hidden = again,
            Waited::Ended(outcome) => return outcome,
        }
    }
}

/// A focus that ended the wait for `dialog`, whose entry then executed
/// nothing (`executed` false), every candidate of its run refused by its
/// read or the rules: while the foreground record still names `dialog`,
/// the foreground window's current focus is read as a re-take, which waits
/// for the dialog's show if it must, and speaks the dialog with its text,
/// then the field.
pub(crate) fn after_ended_wait(cx: &mut Cx<Entries>, dialog: WindowHandle, executed: bool) {
    if executed {
        return;
    }
    let Some(foreground) = cx
        .foreground()
        .filter(|foreground| foreground.window == dialog)
    else {
        return;
    };
    let read = CurrentFocus {
        why: Why::Retake,
        window: foreground.window(),
        trace_id: cx.facts().queue_trace.0,
        fence: crate::keys::entry::Fence::default(),
    };
    cx.join(Entry::CurrentFocus(Box::new(read)));
}

#[cfg(test)]
#[path = "shown_tests.rs"]
mod tests;
