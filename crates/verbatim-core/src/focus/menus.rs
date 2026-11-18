//! Menus (design, "Focus and foreground"): the check after a menu or the
//! task switcher ends with no valid focus in its run, and the UI
//! Automation menu opening.
//!
//! The check: a menu end, popup end or switch end reaching the front when
//! the run before it executed no focus notes the committed focus, and
//! 50 ms later Core appends a check entry. At its turn, if the committed
//! focus is unchanged, Core asks the foreground application's outpost for
//! its focus with the focused state as a fact, which the outpost reads of
//! an MSAA focus over every ancestor up to the desktop, reading the focus
//! in full only when focused; Core judges the answer by the one
//! definition's menu-check rows (`super::validity`): NVDA's duplicate test
//! and the focused state for an MSAA answer, neither for a UI Automation
//! one. NVDA: `IAccessibleHandler/__init__.py` 976 to 1002, 760 to
//! 782; `NVDAObjects/IAccessible/__init__.py` 874 to 892.
//!
//! The UI Automation menu opening is dropped at its arrival while a
//! focus-kind entry that counts as pending by the one definition, raised
//! before it, waits or is asked ahead of it (`UIAHandler/__init__.py` 772 to 777); on the
//! committed focus's runtime id the committed focus is executed again;
//! otherwise it is judged and read as a focus (`crate::focus::uia_focus`).

use serde::{Deserialize, Serialize};
use verbatim_model::TraceId;
use verbatim_model::event_kind::EventKind;

use super::FOCUS_SET;
use super::backend::BackendCache;
use super::execute::{Origin, execute};
use super::msaa_run::{RunRequest, known_for};
use super::retake::{CurrentFocusRead, read_by};
use super::validity::{self, Pending, Purpose, Subject, Validity, View};
use super::versions::{Committed, ReadBy};
use crate::entries::{Candidate, Entries, Entry};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome};
use crate::state::SrState;
use verbatim_model::request::Depth;

/// How long after a menu or switcher end Core appends its check: NVDA's
/// 50 ms (`IAccessibleHandler/__init__.py` 976 to 991).
pub(crate) const CHECK_AFTER_US: u64 = 50_000;

/// The check entry: the committed focus as the end noted it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MenuCheck {
    /// The committed focus when the end reached the front.
    pub(crate) committed: Committed,
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
    let committed = cx.facts().versions.committed.clone();
    if cx.timer_due_near(
        CHECK_AFTER_US,
        crate::event_queue::PUMP_WINDOW_US,
        |entry| matches!(entry, Entry::MenuCheck(due) if due.committed == committed),
    ) {
        return;
    }
    let check = MenuCheck {
        committed,
        trace_id: candidate.tell.trace_id,
        fence: crate::keys::entry::Fence::default(),
    };
    cx.append_after(CHECK_AFTER_US, Entry::MenuCheck(Box::new(check)));
}

/// The menu-start rule (design, "Focus and foreground"; NVDA:
/// `IAccessibleHandler/__init__.py` 942 to 973, 1082 to 1103), as the one
/// definition judges it: a menu or popup start is refused when the run
/// before it just executed a focus and the readers' last focus is an MSAA
/// menu popup or menu item, since the focus is already on the menu.
/// Otherwise, passing acceptance and the verdict, its object is read in
/// full as a focus with neither the duplicate test nor the focused-state
/// test; it executes as a focus when it is a menu popup, a start on
/// anything else saying nothing.
async fn start(cx: &mut Cx<Entries>, candidate: &Candidate, run_executed: bool) {
    let judged = |cx: &Cx<Entries>, facts| {
        validity::at(
            cx,
            Subject::Msaa(candidate),
            facts,
            Purpose::Turn,
            run_executed,
        )
    };
    match judged(cx, candidate.confirmation.facts()) {
        Validity::Refused(reason) => {
            if reason.tried() {
                cx.facts_mut().versions.tried_and_refused();
            }
            return;
        }
        Validity::Duplicate => return,
        Validity::Valid(_) | Validity::NeedsRead(_) => {}
    }
    let known = known_for(cx, candidate.tell.process);
    let request = RunRequest::of(cx, candidate, (Depth::Full, None), &known);
    let due_us = cx.due_in(FOCUS_DUE_US);
    let Outcome::Result(answered) = cx.ask(request, due_us).await else {
        cx.facts_mut().focus_read_failed();
        return;
    };
    match judged(cx, Some(&answered.facts)) {
        Validity::Valid(_) => {}
        Validity::Refused(reason) => {
            if reason.tried() {
                cx.facts_mut().versions.tried_and_refused();
            }
            return;
        }
        Validity::Duplicate | Validity::NeedsRead(_) => return,
    }
    let Some(read) = answered.read.as_deref() else {
        // A valid answer with no full read: the outpost's fault.
        return;
    };
    let Some(held) = read.msaa else {
        return;
    };
    let read_by = ReadBy::Msaa {
        event: Some(held.event),
        held_child: held.held_child,
    };
    let origin = Origin::of_tell(&candidate.tell, candidate.window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, read, &known, read_by);
    executed.deliver(cx);
}

/// The function of a check entry: the foreground application's focus read
/// with its focused state as a fact, in full only when focused, then
/// judged by the one definition's menu-check rows.
pub(crate) async fn check(mut cx: Cx<Entries>, check: MenuCheck) {
    if cx.facts().versions.committed != check.committed {
        return;
    }
    let Some(foreground) = cx.foreground() else {
        return;
    };
    let window = foreground.window();
    let known = known_for(&cx, window.process);
    let due_us = cx.due_in(FOCUS_DUE_US);
    let request = CurrentFocusRead {
        window,
        known: known.sent.clone(),
        details: crate::details::focus_details(&cx.facts()),
        set: crate::details::node_set(&cx.facts(), FOCUS_SET),
        depth: validity::depth(Subject::MenuCheck(None)),
    };
    let Outcome::Result(answered) = cx.ask(request, due_us).await else {
        cx.facts_mut().focus_read_failed();
        return;
    };
    let ids = answered
        .read
        .as_ref()
        .and_then(|read| read.msaa)
        .map(|held| held.event);
    match validity::at(
        &cx,
        Subject::MenuCheck(ids.as_ref()),
        Some(&answered.facts),
        Purpose::Turn,
        false,
    ) {
        Validity::Valid(_) => {}
        Validity::Refused(reason) => {
            if reason.tried() {
                cx.facts_mut().versions.tried_and_refused();
            }
            return;
        }
        Validity::Duplicate | Validity::NeedsRead(_) => return,
    }
    let Some(read) = answered.read else {
        // A valid answer with no full read: the outpost's fault.
        return;
    };
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = Origin::of_read(check.trace_id, window);
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    executed.deliver(&mut cx);
}

/// Whether the UI Automation menu opening `arriving` is dropped at its
/// arrival: a focus-kind entry `ahead` of it, raised before it, counts as
/// pending by the one definition. An unconfirmed MSAA focus does not count
/// and is not read, as NVDA's events not yet pumped are not pending.
pub(crate) fn opening_dropped(
    facts: &SrState,
    ahead: &[&Entry],
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
    let counts = |entry: &Entry| {
        validity::judge_alone(entry, &view).is_some_and(|(answer, trace, at_us)| {
            at_us < opening.tell.at_us && validity::pending(answer, trace) == Pending::Counts
        })
    };
    ahead.iter().any(|entry| match entry {
        Entry::Run(candidates) => candidates
            .iter()
            .any(|candidate| counts(&Entry::Candidate(Box::new(candidate.clone())))),
        other => counts(other),
    })
}

#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
