//! Menus (design, "Focus and foreground"): the check after a menu or the
//! task switcher ends with no valid focus in its run, and the UI
//! Automation menu opening.
//!
//! The check: a menu end, popup end or switch end reaching the front when
//! the run before it executed no focus notes the committed focus, and
//! 50 ms later Core appends a check entry. At its turn, if the committed
//! focus is unchanged, Core asks the foreground application's outpost for
//! its focus, and judges an MSAA answer by NVDA's duplicate test and its
//! focused-state test (the object or an ancestor has the focused state);
//! a UI Automation answer by neither. NVDA:
//! `IAccessibleHandler/__init__.py` 976 to 1002, 760 to 782;
//! `NVDAObjects/IAccessible/__init__.py` 874 to 892.
//!
//! The UI Automation menu opening is dropped at its arrival while a
//! focus-kind entry that counts as pending, raised before it, waits or is
//! asked ahead of it (`UIAHandler/__init__.py` 772 to 777); on the
//! committed focus's runtime id the committed focus is executed again;
//! otherwise it is judged and read as a focus (`crate::focus::uia_focus`).

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::FocusRead;
use verbatim_model::{OutpostId, State, TraceId};

use super::backend::BackendCache;
use super::duplicate::msaa_duplicate;
use super::execute::{Known, Origin, execute};
use super::pending::counts_as_pending;
use super::retake::{CurrentFocusRead, read_by};
use super::versions::Committed;
use crate::entries::{Candidate, Entries, Entry};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader};
use crate::state::SrState;

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
}

/// Whether `kind` is an end whose check NVDA makes when no valid focus
/// came in its run.
fn ends(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::MenuEnd | EventKind::PopupEnd | EventKind::SwitchEnd
    )
}

/// The menu slot's entry `candidate`, judged at once with no request: an
/// end after a run that executed no focus appends the check 50 ms later. A menu or popup
/// start's own rule is not built in this step.
pub(crate) fn slot(cx: &mut Cx<Entries>, candidate: &Candidate, run_executed: bool) {
    if !ends(candidate.tell.kind) || run_executed {
        return;
    }
    let check = MenuCheck {
        committed: cx.facts().versions.committed.clone(),
        trace_id: candidate.tell.trace_id,
    };
    cx.append_after(CHECK_AFTER_US, Entry::MenuCheck(Box::new(check)));
}

/// Whether `read`'s object or one of its ancestors has the focused state.
fn focused_state(read: &FocusRead) -> bool {
    std::iter::once(&read.focus)
        .chain(read.ancestors.iter().map(|ancestor| &ancestor.node))
        .any(|node| {
            matches!(
                node.read.values.answer(PropertyId::States).and_then(|answer| answer.value.as_ref()),
                Some(PropertyValue::States(states)) if states.contains(State::Focused)
            )
        })
}

/// The function of a check entry.
pub(crate) async fn check(mut cx: Cx<Entries>, check: MenuCheck) {
    if cx.facts().versions.committed != check.committed {
        return;
    }
    let Some(foreground) = cx.foreground() else {
        return;
    };
    let window = foreground.window();
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(|instance| OutpostId(instance.generation));
    let known = Known::of(&cx.facts(), generation);
    let due_us = cx.due_in(FOCUS_DUE_US);
    let request = CurrentFocusRead {
        window,
        known: known.sent.clone(),
    };
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        cx.facts_mut().forget_caret();
        return;
    };
    if let Some(held) = &read.msaa
        && (msaa_duplicate(cx.facts().versions.last.as_ref(), &held.event) || !focused_state(&read))
    {
        return;
    }
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = Origin::of_read(check.trace_id, window);
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    executed.deliver(&mut cx);
}

/// Whether the UI Automation menu opening `arriving` is dropped at its
/// arrival: a focus-kind entry `ahead` of it, raised before it, counts as
/// pending on its tell's facts.
pub(crate) fn opening_dropped(
    facts: &SrState,
    ahead: &[&Entry],
    arriving: &Entry,
    backend: &BackendCache,
    now_us: u64,
) -> bool {
    let Entry::UiaFocus(opening) = arriving else {
        return false;
    };
    if opening.tell.kind != EventKind::UiaMenuOpened {
        return false;
    }
    let last = facts.versions.last.as_ref();
    ahead
        .iter()
        .flat_map(|entry| match entry {
            Entry::UiaFocus(focus) => vec![&focus.tell],
            Entry::Candidate(candidate) => vec![&candidate.tell],
            Entry::Run(candidates) => candidates.iter().map(|candidate| &candidate.tell).collect(),
            Entry::Menu(_)
            | Entry::MsaaChange(_)
            | Entry::UiaChange(_)
            | Entry::MenuCheck(_)
            | Entry::CurrentFocus(_)
            | Entry::ForegroundMarker { .. } => Vec::new(),
        })
        .any(|tell| {
            tell.at_us < opening.tell.at_us && counts_as_pending(tell, last, backend, now_us)
        })
}

#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
