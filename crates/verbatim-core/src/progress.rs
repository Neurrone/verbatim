//! Progress bars as Core's asks (ADR 0003, "Progress bars";
//! `docs/parity/changes-and-selection.md`, "Progress bars"): a bar's change
//! is a tell that joins the event queue and is judged at its turn, by its
//! tell's own relation to the foreground window and by the setting that
//! reports background progress bars.
//!
//! A bar is in the foreground when its tell's window is the foreground
//! window or beneath it, the relation the listener read with the
//! foreground window live, as NVDA asks Windows for it as the event is
//! handled; Core's own foreground record, which lags behind the
//! listener's foreground hold, is not consulted (Dickson, 2026-11-21).
//!
//! - A classic bar's value change, an MSAA value change read through MSAA,
//!   in the foreground or with the setting on, is read by an ordinary
//!   object read the listener serves (role, value, states and place), sent
//!   at once outside the queue as Core's facts' own request, so no key
//!   waits behind it; its answer plays the tone of a progress bar on
//!   screen whose value is a number. The read has Core's ordinary due
//!   time and endings, the hang check's "not responding" included
//!   (Dickson, 2026-11-21). A value change on the readers' last focus,
//!   whose role is known and is no progress bar, is not read: the
//!   focused-value path speaks it, as NVDA redirects that event to its
//!   focus object (Claude, 2026-11-21).
//! - A UI Automation bar's change carries its percentage and place from
//!   its event's cache, so it asks nothing: it plays at its turn when the
//!   element is native by Core's backend cache and in the foreground or
//!   with the setting on.

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{RuntimeId, props};
use verbatim_model::request::{ObjectRef, Purpose, ReadObject, Reply, Request as Wire};
use verbatim_model::tell::{Event, Payload, Relation};
use verbatim_model::{Effect, Percent, Role, State};

use crate::changes::MsaaChange;
use crate::entries::{Entries, request_window, window_facts};
use crate::event_queue::{Cx, Outcome, Reader};
use crate::focus::acceptance::{self, Acceptance};
use crate::focus::backend::{Judged, Judgement};
use crate::focus::versions::ReadBy;
use crate::state::SrState;

verbatim_model::property_set! {
    /// What a classic bar's read answers.
    struct BarRead {
        role: props::Role,
        value: props::Value,
        states: props::States,
        rect: props::Rect,
    }
}

/// A UI Automation progress bar's change, parsed: the tell, its element's
/// runtime id and the percentage and place its tell carries, with its
/// acceptance judged as it arrived.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProgressTell {
    /// The tell.
    pub(crate) tell: Event,
    /// The element's runtime id, which keys the merge of a flood.
    pub(crate) runtime_id: RuntimeId,
    /// Its percentage.
    pub(crate) percent: Percent,
    /// The centre of its rectangle.
    pub(crate) place: (i32, i32),
    /// Its acceptance, judged as its entry is made.
    pub(crate) accepted: Acceptance,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: crate::keys::entry::Fence,
}

impl ProgressTell {
    /// The progress tell `tell` names on `runtime_id`, with its acceptance
    /// judged as it arrives, with background progress bars reported as
    /// `background` says; `None` for one without its percentage and place,
    /// a reader's fault.
    pub(crate) fn of_tell(tell: Event, runtime_id: RuntimeId, background: bool) -> Option<Self> {
        let Payload::Progress { percent, place } = tell.payload else {
            return None;
        };
        let accepted = acceptance::accept(EventKind::UiaProgress, window_facts(&tell), background);
        Some(Self {
            tell,
            runtime_id,
            percent,
            place,
            accepted,
            fence: crate::keys::entry::Fence::default(),
        })
    }

    /// Whether `arriving` replaces it as it waits: the same element of the
    /// same process, whose runtime ids are unique only within their host.
    pub(crate) fn merges(&self, arriving: &Self) -> bool {
        (&self.runtime_id, self.tell.process) == (&arriving.runtime_id, arriving.tell.process)
    }
}

/// Whether `tell`'s window is the foreground window or beneath it, by the
/// relation its reader read with the foreground window live, NVDA's
/// `isInForeground`.
fn in_foreground(tell: &Event) -> bool {
    window_facts(tell).is_some_and(|facts| facts.window_to_foreground != Relation::Outside)
}

/// Whether a bar `tell` tells of is heard: in the foreground, or anywhere
/// with background progress bars reported, as the setting stands at its
/// turn.
fn heard(facts: &SrState, tell: &Event) -> bool {
    facts.settings.report_background_progress_bars || in_foreground(tell)
}

/// Whether `change` names the readers' last focus, whose role is known
/// and is no progress bar: its value change is the focused-value path's.
fn on_last_focus(facts: &SrState, change: &MsaaChange) -> bool {
    facts.versions.last.as_ref().is_some_and(|last| {
        matches!(&last.read_by, ReadBy::Msaa { event: Some(ids), .. } if *ids == change.ids)
            && !matches!(last.role, Role::ProgressBar | Role::Unknown)
    })
}

/// An MSAA value change at its turn, accepted and read through MSAA by
/// its window's verdict: when it is heard and not on the readers' last
/// focus, its object is read of the listener at once, outside the queue.
pub(crate) fn msaa_changed(cx: &mut Cx<Entries>, change: &MsaaChange) {
    if change.tell.kind != EventKind::ValueChange {
        return;
    }
    {
        let mut facts = cx.facts_mut();
        if !heard(&facts, &change.tell) || on_last_focus(&facts, change) {
            return;
        }
        let token = facts.wants.token();
        facts.bar_reads.insert(token);
        let read = ReadObject {
            object: ObjectRef::Msaa {
                window: change.ids.window,
                object: change.ids.object,
                child: change.ids.child,
            },
            set: <BarRead as verbatim_model::property::PropertySet>::PROPERTIES.to_vec(),
            relation: None,
            descendants: false,
            walk: None,
            purpose: Purpose::Other,
        };
        facts.wants.want(
            Some(token),
            (Reader::Listener, change.window),
            Wire::ReadObject(read),
        );
    }
    cx.send_wanted();
}

/// Whether `token` is a bar read's: taken out of the reads out, as its
/// ending arrives.
pub(crate) fn ended(facts: &mut SrState, token: u64) -> bool {
    facts.bar_reads.remove(&token)
}

/// A bar read ended with `outcome`: the tone of a progress bar on screen
/// whose value is a number, at the centre of its rectangle, (0, 0) when it
/// reports none, as NVDA places one; nothing for anything else.
pub(crate) fn answered(facts: &mut SrState, outcome: Outcome<Reply>) -> Vec<Effect> {
    let Outcome::Result(Reply::Object(read)) = outcome else {
        return Vec::new();
    };
    // An answer other than the set asked is the reader's fault, which says
    // nothing, as an undecodable answer to the queue's request does.
    let Ok(bar) = <BarRead as verbatim_model::property::PropertySet>::parse(read.object.values)
    else {
        return Vec::new();
    };
    let states = bar.states.unwrap_or_default();
    if bar.role != Some(Role::ProgressBar)
        || states.contains(State::Invisible)
        || states.contains(State::Offscreen)
    {
        return Vec::new();
    }
    let Some(percent) = bar.value.as_deref().and_then(Percent::parse) else {
        return Vec::new();
    };
    let place = bar.rect.map_or((0, 0), |rect| {
        (
            rect.left.saturating_add(rect.width / 2),
            rect.top.saturating_add(rect.height / 2),
        )
    });
    crate::reduce::indicate_progress(facts, place, percent)
}

/// The function of a UI Automation bar's entry: accepted, native by Core's
/// backend cache and heard, it plays its tone from its tell.
pub(crate) fn uia(cx: &mut Cx<Entries>, bar: &ProgressTell) {
    if bar.accepted.is_err() {
        return;
    }
    let Some(window) = request_window(&bar.tell) else {
        return;
    };
    if cx
        .backend()
        .judge(Judged::Uia, &window.judged(), bar.tell.at_us, cx.now_us())
        == Judgement::Drop
    {
        return;
    }
    let said = {
        let mut facts = cx.facts_mut();
        if !heard(&facts, &bar.tell) {
            return;
        }
        crate::reduce::indicate_progress(&mut facts, bar.place, bar.percent)
    };
    for effect in said {
        cx.emit(effect);
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
