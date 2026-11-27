//! Menus (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects"): the check after a menu or the
//! task switcher ends with no valid focus in its run, and the UI
//! Automation menu opening.
//!
//! The check: a menu end, popup end or switch end reaching the front when
//! the run before it executed no focus notes the committed focus, and
//! 50 ms later Core appends a check entry, the wake that brings it
//! carrying what Windows names in front then, read live by Core's shell
//! ([`due`]), as NVDA's timer asks Windows for the foreground window where
//! Core's record waits for the listener's report (Dickson, 2026-11-26). At
//! its turn, if the committed focus is unchanged, Core asks the outpost of
//! that window's application for
//! its focus with the focused state as a fact, which the outpost reads of
//! an MSAA focus over every ancestor up to the desktop, reading the focus
//! in full only when focused; Core judges the answer by the one
//! definition's menu-check rows (`super::validity`): NVDA's duplicate test
//! and the focused state for an MSAA answer, neither for a UI Automation
//! one. NVDA: `IAccessibleHandler/__init__.py` 976 to 1002, 760 to
//! 782; `NVDAObjects/IAccessible/__init__.py` 874 to 892;
//! `NVDAObjects/window/__init__.py` 162 to 181.
//!
//! The UI Automation menu opening is dropped at its arrival while a
//! focus-kind entry that counts as pending by the one definition, raised
//! before it, waits or is asked ahead of it (`UIAHandler/__init__.py` 772 to 777); on the
//! committed focus's runtime id the committed focus is executed again;
//! otherwise it is judged and read as a focus (`crate::focus::uia_focus`).

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::LiveFront;
use verbatim_model::{NodeId, TraceId};

use super::backend::BackendCache;
use super::execute::{Origin, execute};
use super::msaa_run::{known_for, read_full};
use super::plain::HungRead;
use super::retake::{Focused, current_if_focused, read_by};
use super::validity::{self, Answered, Pending, Settled, Subject, Validity, View, Within};
use crate::entries::{Candidate, Entries, Entry};
use crate::event_queue::{Cx, FOCUS_DUE_US, Item, Outcome, RequestWindow};
use crate::state::SrState;
use std::collections::VecDeque;

/// How long after a menu or switcher end Core appends its check: NVDA's
/// 50 ms (`IAccessibleHandler/__init__.py` 976 to 991).
pub(crate) const CHECK_AFTER_US: u64 = 50_000;

/// Which end made a check: NVDA's fake focus after a menu or popup end,
/// which makes a plain window of a ghost in front, or the task switcher's
/// end, which is left as it was (`super::plain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CheckedEnd {
    /// A menu end or a popup end.
    MenuOrPopup,
    /// The task switcher's end.
    Switch,
}

/// What is in front for a check: the check's one lifecycle, waiting for
/// its time and then holding what the wake that brought it carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum InFront {
    /// The check waits for its time.
    Waiting,
    /// Windows named this window in front when the check came due.
    Window(RequestWindow),
    /// Windows named no window in front.
    Nothing,
    /// The window in front could not be read: no reader can be asked.
    Gone,
}

/// The check entry: the committed focus's node as the end noted it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MenuCheck {
    /// What is in front, read live when the check comes due ([`due`]).
    pub(crate) in_front: InFront,
    /// The committed focus's node when the end reached the front, `None`
    /// with nothing committed or an own focus: the check compares the
    /// node, not NVDA's object identity (`docs/parity/focus-events.md`).
    pub(crate) committed: Option<NodeId>,
    /// The end that made it; ends of one pump that share a check keep the
    /// first one's.
    pub(crate) end: CheckedEnd,
    /// The trace of the end's tell.
    pub(crate) trace_id: TraceId,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: crate::keys::entry::Fence,
}

/// Whether `kind` is an end whose check NVDA makes when no valid focus
/// came in its run.
fn ends(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::MenuEnd | EventKind::PopupEnd | EventKind::SwitchEnd
    )
}

/// The function of the menu slot's entry `candidate`: a menu or popup
/// start by the menu-start rule ([`start`]); an end after a run that
/// executed no focus appends the check 50 ms later, with no request.
pub(crate) async fn slot(mut cx: Cx<Entries>, candidate: Candidate, run_executed: bool) {
    if matches!(
        candidate.tell.kind,
        EventKind::MenuStart | EventKind::PopupStart
    ) {
        start(&mut cx, &candidate, run_executed).await;
        return;
    }
    if !ends(candidate.tell.kind) || run_executed {
        return;
    }
    // Ends of one pump share one check: NVDA's limiter keeps one menu
    // event of a flush (`orderedWinEventLimiter.py` 77, 93 to 95), so its
    // pump fakes the focus once (`IAccessibleHandler/__init__.py` 1084,
    // 1093 to 1103). Core has no pump; a check already due within its
    // window that noted the same committed focus stands for this one
    // (`event_queue::PUMP_WINDOW_US`). One that noted another focus will
    // end unasked, as a focus committed between the two ends put this end
    // in a later pump of NVDA's.
    let committed = cx.facts().versions.committed_node();
    if cx.timer_due_near(
        CHECK_AFTER_US,
        crate::event_queue::PUMP_WINDOW_US,
        |entry| matches!(entry, Entry::MenuCheck(due) if due.committed == committed),
    ) {
        return;
    }
    let end = match candidate.tell.kind {
        EventKind::SwitchEnd => CheckedEnd::Switch,
        _ => CheckedEnd::MenuOrPopup,
    };
    let check = MenuCheck {
        in_front: InFront::Waiting,
        committed,
        end,
        trace_id: candidate.tell.trace_id,
        fence: crate::keys::entry::Fence::default(),
    };
    cx.append_after(CHECK_AFTER_US, Entry::MenuCheck(Box::new(check)));
}

/// The menu-start rule (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects"; NVDA:
/// `IAccessibleHandler/__init__.py` 942 to 973, 1082 to 1103), as the one
/// definition judges it: a menu or popup start is refused when the run
/// before it just executed a focus and the readers' last focus is an MSAA
/// menu popup or menu item, since the focus is already on the menu.
/// Otherwise, passing acceptance and the verdict, its object is read in
/// full as a focus with neither the duplicate test nor the focused-state
/// test; it executes as a focus when it is a menu popup, a start on
/// anything else saying nothing.
async fn start(cx: &mut Cx<Entries>, candidate: &Candidate, run_executed: bool) {
    let judged =
        |cx: &Cx<Entries>, facts| validity::at(cx, Subject::Msaa(candidate), facts, run_executed);
    match judged(cx, candidate.confirmation.facts()) {
        Validity::Refused(reason) => {
            validity::refused(&mut cx.facts_mut(), reason);
            return;
        }
        Validity::Duplicate => return,
        Validity::Valid(_) | Validity::NeedsRead => {}
    }
    let known = known_for(cx, candidate.tell.process);
    let due_us = cx.due_in(FOCUS_DUE_US);
    let Some(answered) = read_full(cx, candidate, (&known, due_us)).await else {
        return;
    };
    let (facts, read) = match answered {
        Answered::Unread(unread) => {
            validity::refused(&mut cx.facts_mut(), unread.reason());
            return;
        }
        Answered::Read { facts, read } => (facts, read),
    };
    match validity::read_at(cx, Subject::Msaa(candidate), &facts, run_executed) {
        Settled::Valid(_) => {}
        Settled::Refused(reason) => {
            validity::refused(&mut cx.facts_mut(), reason);
            return;
        }
        Settled::Duplicate => return,
    }
    super::msaa_run::executed(cx, candidate, &read, &known);
}

/// The wake at `at_us` carries `in_front`, what Core's shell read of
/// Windows in front as it made the wake: every check whose time has come
/// takes it, before the wake's step joins the check to the queue. The
/// shell reads whenever a check waits (`SrState::menu_check_waits`), so a
/// check never joins without it.
pub(crate) fn due(state: &mut SrState, at_us: u64, in_front: &LiveFront) {
    let read = match in_front {
        LiveFront::Window(front) => InFront::Window(RequestWindow {
            handle: front.window,
            top_level: front.window,
            root_owner: front.root_owner,
            process: front.process,
            thread: front.thread,
            // The foreground window is top-level: its own root.
            by_class: verbatim_model::class::by_class(&front.class, &front.class),
        }),
        LiveFront::Nothing => InFront::Nothing,
        LiveFront::Gone => InFront::Gone,
    };
    for entry in state.queue.later_due_mut(at_us) {
        if let Entry::MenuCheck(check) = entry {
            check.in_front = read;
        }
    }
}

/// The function of a check entry: the focus of the application Windows
/// named in front when the check came due, read with its focused state as
/// a fact, in full only when focused, then judged by the one definition's
/// menu-check rows. With no window in front the read names the foreground
/// record's window, and Core's shell answers it unsent with the desktop
/// (`super::plain::decide`); a window in front that could not be read ends
/// the check, as NVDA's fake focus does nothing with no focus object
/// (`IAccessibleHandler/__init__.py` 995 to 997).
pub(crate) async fn check(mut cx: Cx<Entries>, check: MenuCheck) {
    if cx.facts().versions.committed_node() != check.committed {
        return;
    }
    let window = match check.in_front {
        InFront::Window(window) => window,
        InFront::Nothing => {
            let Some(foreground) = cx.foreground() else {
                return;
            };
            let Some(window) = foreground.window() else {
                // No process could be read as serving the window in
                // front: no reader is asked, and the check ends as an
                // untried read.
                validity::untried_read(&mut cx.facts_mut());
                return;
            };
            window
        }
        // A check joins the queue only by a wake, which carries the read.
        InFront::Gone | InFront::Waiting => return,
    };
    let due_us = cx.due_in(FOCUS_DUE_US);
    let known = known_for(&cx, window.process);
    let Some(outcome) = current_if_focused(&mut cx, window, known.sent.clone(), due_us).await
    else {
        return;
    };
    let answered = match outcome {
        Outcome::Result(Focused::Read(answered)) => answered,
        Outcome::Result(Focused::InFront(front)) => {
            let read = match check.end {
                CheckedEnd::MenuOrPopup => HungRead::MenuCheck,
                CheckedEnd::Switch => HungRead::SwitchCheck,
            };
            super::plain::decide(&mut cx, read, window, &front, (check.trace_id, check.fence));
            return;
        }
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            return;
        }
    };
    let (facts, read) = match answered {
        Answered::Unread(unread) => {
            validity::refused(&mut cx.facts_mut(), unread.reason());
            return;
        }
        Answered::Read { facts, read } => (facts, read),
    };
    let ids = read.msaa.map(|held| held.event);
    match validity::read_at(&cx, Subject::MenuCheck(ids.as_ref()), &facts, false) {
        Settled::Valid(_) => {}
        Settled::Refused(reason) => {
            validity::refused(&mut cx.facts_mut(), reason);
            return;
        }
        Settled::Duplicate => return,
    }
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = Origin::of_read(check.trace_id, window, check.fence);
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    let fence = crate::entries::front_fence(&cx);
    executed.deliver(&mut cx, fence);
}

/// Whether the UI Automation menu opening `arriving` is dropped at its
/// arrival: a focus-kind entry ahead of it, the front `front` or one
/// waiting, raised before it, counts as pending by the one definition,
/// each run walked as a run ([`validity::pending_behind`]). An unconfirmed
/// MSAA focus does not count and is not read, as NVDA's events not yet
/// pumped are not pending.
pub(crate) fn opening_dropped(
    facts: &SrState,
    (front, waiting): (Option<&Entry>, &VecDeque<Item<Entry>>),
    arriving: &Entry,
    backend: &BackendCache,
    (now_us, foreground): (u64, Option<crate::event_queue::Foreground>),
) -> bool {
    let Entry::UiaFocus(opening) = arriving else {
        return false;
    };
    if opening.tell.kind != EventKind::UiaMenuOpened {
        return false;
    }
    let view = View {
        versions: &facts.versions,
        in_front: foreground.map(|foreground| foreground.window),
        backend,
        now_us,
        run_before_valid: false,
    };
    let within = Within {
        raised_before: Some(opening.tell.at_us),
        unconfirmed_before: None,
    };
    let ahead: VecDeque<Item<Entry>> = front
        .map(|front| match front {
            Entry::Run(candidates) => Item::Run(
                candidates
                    .iter()
                    .map(|candidate| Entry::Candidate(Box::new(candidate.clone())))
                    .collect(),
            ),
            other => Item::One(other.clone()),
        })
        .into_iter()
        .chain(waiting.iter().cloned())
        .collect();
    validity::pending_behind(&ahead, &view, within) == Pending::Counts
}

#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
