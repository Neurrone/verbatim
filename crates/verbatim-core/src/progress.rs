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
//!   A bar has one read out at a time: a change of a bar whose read is
//!   out marks it, and the read's ending, whatever it is, reads the bar
//!   again when it was marked, so its newest value is still heard, as
//!   NVDA's limiter keeps one waiting event per object and its handler
//!   reads one at a time (`docs/parity/backends.md`, "MSAA winevent flood
//!   control"). A flood of one bar's changes so keeps one read out, where
//!   one read per change left thousands out, each step's work growing with
//!   them (Claude, 2026-11-21).
//! - A UI Automation bar's change carries its percentage and place from
//!   its event's cache, so it asks nothing: it plays at its turn when the
//!   element is native by Core's backend cache and in the foreground or
//!   with the setting on.

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{RuntimeId, props};
use verbatim_model::request::{MsaaIds, ObjectRef, Purpose, ReadObject, Reply, Request as Wire};
use verbatim_model::tell::{Event, Payload, Relation};
use verbatim_model::{Effect, Percent, Role, State};

use crate::changes::MsaaChange;
use crate::entries::{Entries, request_window, window_facts};
use crate::event_queue::{Cx, Outcome, Reader, RequestWindow};
use crate::focus::acceptance::{self, Acceptance};
use crate::focus::backend::{Judged, Judgement};
use crate::focus::versions::ReadBy;
use crate::state::SrState;

verbatim_model::property_set! {
    /// What a classic bar's read answers.
    struct BarAnswer {
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

/// A classic bar's key among the reads out: its window, object id and
/// child id.
pub(crate) type BarKey = (u64, i32, i32);

/// The key of the bar `ids` names.
fn key(ids: &MsaaIds) -> BarKey {
    (ids.window.0, ids.object, ids.child)
}

/// A classic bar's read out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BarRead {
    /// Its read is out under `token`, and no change came since it was
    /// sent.
    Reading {
        /// The read's token.
        token: u64,
    },
    /// Its read is out under `token`, and a newer change came, naming
    /// `window`: its ending reads the bar again.
    ReadingNewer {
        /// The read's token.
        token: u64,
        /// The window the newest change's request names.
        window: RequestWindow,
    },
}

impl BarRead {
    /// The token of its read out.
    fn token(self) -> u64 {
        match self {
            Self::Reading { token } | Self::ReadingNewer { token, .. } => token,
        }
    }
}

/// Wants a read of the bar `ids` names, in `window`, under a new token,
/// and records it as out.
fn read(facts: &mut SrState, ids: MsaaIds, window: RequestWindow) {
    let token = facts.wants.token();
    facts
        .bar_reads
        .insert(key(&ids), BarRead::Reading { token });
    let read = ReadObject {
        object: ObjectRef::Msaa {
            window: ids.window,
            object: ids.object,
            child: ids.child,
        },
        set: <BarAnswer as verbatim_model::property::PropertySet>::PROPERTIES.to_vec(),
        relation: None,
        descendants: false,
        purpose: Purpose::Other,
    };
    facts.wants.want(
        Some(token),
        (Reader::Listener, window),
        Wire::ReadObject(read).into(),
    );
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
/// focus, its object is read of the listener at once, outside the queue,
/// unless the bar's read is out, which it marks to be read again.
pub(crate) fn msaa_changed(cx: &mut Cx<Entries>, change: &MsaaChange) {
    if change.tell.kind != EventKind::ValueChange {
        return;
    }
    {
        let mut facts = cx.facts_mut();
        if !heard(&facts, &change.tell) || on_last_focus(&facts, change) {
            return;
        }
        if let Some(out) = facts.bar_reads.get_mut(&key(&change.ids)) {
            *out = BarRead::ReadingNewer {
                token: out.token(),
                window: change.window,
            };
            return;
        }
        read(&mut facts, change.ids, change.window);
    }
    cx.send_wanted();
}

/// Whether `token` is a bar read's, as its ending arrives: the bar's read
/// out ends, and a bar changed since it was sent is read again.
pub(crate) fn ended(facts: &mut SrState, token: u64) -> bool {
    let Some((&bar, &out)) = facts.bar_reads.iter().find(|(_, out)| out.token() == token) else {
        return false;
    };
    facts.bar_reads.remove(&bar);
    if let BarRead::ReadingNewer { window, .. } = out {
        let (window_handle, object, child) = bar;
        let ids = MsaaIds {
            window: verbatim_model::WindowHandle(window_handle),
            object,
            child,
        };
        read(facts, ids, window);
    }
    true
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
    let Ok(bar) = <BarAnswer as verbatim_model::property::PropertySet>::parse(read.object.values)
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
