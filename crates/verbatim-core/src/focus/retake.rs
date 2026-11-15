//! The reads of an application's current focus that Core makes itself
//! (design, "Focus and foreground"; Tables 3 and 18): the focus at
//! startup, asked at the listener's first foreground report and spoken as
//! a switch with no duplicate test; the re-take after the committed
//! focus's reader ends, or the listener ends, compared whole with the
//! focus Core holds and taken silently when it reads the same; and the
//! read after a window Core stopped waiting on as hung answers again,
//! judged as the re-take, a UI Automation runtime id equal to the
//! readers' last focus's being the duplicate test's answer.
//!
//! None of them sends the ancestors Core already knows.

use serde::{Deserialize, Serialize};
use verbatim_model::TraceId;
use verbatim_model::request::{FocusRead, KnownAncestor, Reply, Request as Wire};

use super::FOCUS_SET;
use super::execute::{Executed, Known, Origin, execute, retake};
use super::versions::{Committed, ReadBy};
use crate::entries::Entries;
use crate::event_queue::{
    Cx, FOCUS_DUE_US, Foreground, Instance, Outcome, Reader, Request, RequestWindow,
};
use crate::state::SrState;

/// Why Core reads an application's current focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Why {
    /// The focus at startup.
    Startup,
    /// The re-take after a reader's replacement.
    Retake,
    /// A window Core stopped waiting on as hung answers again.
    Recovered,
}

/// An entry reading an application's current focus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CurrentFocus {
    /// Why.
    pub(crate) why: Why,
    /// The window the read names, fixed when the entry is made.
    pub(crate) window: RequestWindow,
    /// The trace of the input that made it.
    pub(crate) trace_id: TraceId,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: crate::keys::entry::Fence,
}

/// The request for an application's current focus, of the outpost of the
/// window's application.
#[derive(Debug)]
pub(crate) struct CurrentFocusRead {
    /// The window it names.
    pub(crate) window: RequestWindow,
    /// The ancestors Core already knows.
    pub(crate) known: Vec<KnownAncestor>,
}

impl Request for CurrentFocusRead {
    type Reply = Box<FocusRead>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::CurrentFocus {
            set: FOCUS_SET.to_vec(),
            known: self.known.clone(),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        match reply {
            Reply::Focus(read) => Some(read),
            _ => None,
        }
    }
}

/// How a current-focus answer is named: by the MSAA ids it carries, else
/// by its runtime id, which the listener holds behind no token; `None`
/// for an answer with neither, a reader's fault.
pub(crate) fn read_by(read: &FocusRead) -> Option<ReadBy> {
    match (&read.msaa, &read.focus.runtime_id) {
        (Some(held), _) => Some(ReadBy::Msaa {
            event: Some(held.event),
            held_child: held.held_child,
        }),
        (None, Some(runtime_id)) => Some(ReadBy::Uia {
            runtime_id: runtime_id.clone(),
            token: None,
        }),
        (None, None) => None,
    }
}

/// The startup entry, at the listener's first foreground report
/// `foreground`: it asks the foreground application's outpost, the console
/// host for a console window as the report names it.
pub(crate) fn startup(state: &SrState, foreground: &Foreground) -> CurrentFocus {
    CurrentFocus {
        why: Why::Startup,
        window: foreground.window(),
        trace_id: state.queue_trace.0,
        fence: crate::keys::entry::Fence::default(),
    }
}

/// The entry after a window in the recovery set, `window`, answers again.
pub(crate) fn recovered(state: &SrState, window: &RequestWindow) -> CurrentFocus {
    CurrentFocus {
        why: Why::Recovered,
        window: *window,
        trace_id: state.queue_trace.0,
        fence: crate::keys::entry::Fence::default(),
    }
}

/// `instance` ended (Table 3, column F): the listener's tokens die with
/// it, and the committed focus's data is kept with its ids dead when its
/// outpost ended. The re-take is returned when one is due: for the
/// committed focus's outpost, of its application, naming the foreground
/// record's window when that application holds attention and the window
/// its read named otherwise; for the listener, once, of the application
/// holding attention. None for Core's own focus, which has no reader.
pub(crate) fn reader_ended(
    state: &mut SrState,
    instance: Instance,
    foreground: Option<Foreground>,
) -> Option<CurrentFocus> {
    let window = match instance.reader {
        Reader::Listener => {
            if let Some(last) = state.versions.last.as_mut()
                && let ReadBy::Uia { token, .. } = &mut last.read_by
            {
                *token = None;
            }
            match &state.versions.committed {
                Committed::Read(_) | Committed::Ended(_) => foreground?.window(),
                Committed::Nothing | Committed::Own { .. } => return None,
            }
        }
        Reader::Outpost(_) => {
            let outpost = verbatim_model::OutpostId(instance.generation);
            let super::versions::ReaderEnd::Retake = state.versions.outpost_ended(outpost) else {
                return None;
            };
            let kept = state.versions.committed_kept()?;
            match foreground {
                Some(foreground) if foreground.process == kept.source => foreground.window(),
                Some(_) | None => kept.window,
            }
        }
    };
    Some(CurrentFocus {
        why: Why::Retake,
        window,
        trace_id: state.queue_trace.0,
        fence: crate::keys::entry::Fence::default(),
    })
}

/// Whether the recovered read `read` is a duplicate: a UI Automation
/// focus whose runtime id is the readers' last focus's, since the read is
/// of the current keyboard focus.
fn recovered_duplicate(state: &SrState, read: &FocusRead) -> bool {
    match (
        state.versions.last.as_ref().map(|last| &last.read_by),
        &read.msaa,
        &read.focus.runtime_id,
    ) {
        (Some(ReadBy::Uia { runtime_id, .. }), None, Some(answered)) => runtime_id == answered,
        _ => false,
    }
}

/// The function of a current-focus entry. A failed read clears Core's
/// caret baseline.
pub(crate) async fn run(mut cx: Cx<Entries>, entry: CurrentFocus) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let request = CurrentFocusRead {
        window: entry.window,
        known: Vec::new(),
    };
    let Outcome::Result(read) = cx.ask(request, due_us).await else {
        cx.facts_mut().focus_read_failed();
        return;
    };
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = Origin::of_read(entry.trace_id, entry.window);
    let executed = {
        let mut facts = cx.facts_mut();
        match entry.why {
            Why::Startup => execute(&mut facts, &origin, &read, &Known::default(), read_by),
            Why::Recovered if recovered_duplicate(&facts, &read) => Executed::default(),
            Why::Retake | Why::Recovered => retake(&mut facts, &origin, &read, read_by).1,
        }
    };
    executed.deliver(&mut cx);
}

#[cfg(test)]
#[path = "retake_tests.rs"]
mod tests;
