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
use verbatim_model::property::PropertyId;
use verbatim_model::request::{
    CandidateAnswer, FocusDepth, FocusRead, Front, HiddenRoot, KnownAncestor, Reply,
    Request as Wire,
};
use verbatim_model::{TextDetails, TraceId};

use super::FOCUS_SET;
use super::execute::{Executed, Known, Origin, execute, retake};
use super::shown::Hidden;
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
    /// The read after the user's input desktop returned.
    Returned,
    /// The show of a top-level window, read when it is the postponed
    /// dialog in front (`super::shown`).
    Shown,
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

/// How far a read of the current focus goes, as its request's type, with
/// how its answer is parsed at its decode.
pub(crate) trait FocusReadDepth: std::fmt::Debug + Unpin + 'static {
    /// The depth.
    const DEPTH: FocusDepth;
    /// Whether the outpost checks for a hidden dialog first, or reads as
    /// it is, at a press while a dialog is postponed (`super::shown`).
    const HIDDEN: HiddenRoot = HiddenRoot::Check;
    /// The answer, parsed.
    type Parsed: std::fmt::Debug + 'static;
    /// `answer` parsed, `None` for one that breaks the depth's rule.
    fn parse(answer: CandidateAnswer) -> Option<Self::Parsed>;
}

impl FocusReadDepth for super::msaa_run::FullIfFocused {
    const DEPTH: FocusDepth = FocusDepth::FullIfFocused;
    type Parsed = super::validity::Answered;
    fn parse(answer: CandidateAnswer) -> Option<Self::Parsed> {
        super::validity::Answered::parse(answer, &Self::DEPTH.into())
    }
}

/// A read in full answers the focus read, nothing else: the outpost answers
/// "not there" for a focus it cannot read.
impl FocusReadDepth for super::msaa_run::Full {
    const DEPTH: FocusDepth = FocusDepth::Full;
    type Parsed = Box<FocusRead>;
    fn parse(answer: CandidateAnswer) -> Option<Self::Parsed> {
        match super::validity::Answered::parse(answer, &Self::DEPTH.into())? {
            super::validity::Answered::Read { read, .. } => Some(read),
            super::validity::Answered::Unread(_) => None,
        }
    }
}

/// A read in full made as it is, the read at a press while a dialog is
/// postponed: a hidden dialog answered to it breaks its rule.
#[derive(Debug)]
pub(crate) struct AsItIs;

impl FocusReadDepth for AsItIs {
    const DEPTH: FocusDepth = FocusDepth::Full;
    const HIDDEN: HiddenRoot = HiddenRoot::ReadAnyway;
    type Parsed = Box<FocusRead>;
    fn parse(answer: CandidateAnswer) -> Option<Self::Parsed> {
        super::msaa_run::Full::parse(answer)
    }
}

/// An application's current focus as its read answers it: read to its
/// depth, or, when the shell answered the read unsent, its window hung or
/// no window in front, what the shell read in front, which the read's
/// entry judges (`super::plain::decide`).
#[derive(Debug)]
pub(crate) enum Focused<P> {
    /// The read, parsed to its depth.
    Read(P),
    /// This is in front.
    InFront(Front),
}

/// The request for an application's current focus, of the outpost of the
/// window's application, read to `D`.
#[derive(Debug)]
pub(crate) struct CurrentFocusRead<D: FocusReadDepth> {
    /// The window it names.
    pub(crate) window: RequestWindow,
    /// The ancestors Core already knows.
    pub(crate) known: Vec<KnownAncestor>,
    /// What the focus's caret read fetches beyond the text: its line's
    /// formatting.
    pub(crate) details: TextDetails,
    /// The properties of each node, chosen from the settings.
    pub(crate) set: Vec<PropertyId>,
    /// How far the read goes: in full, or, for the check after a menu end,
    /// NVDA's fake focus, in full only when focused.
    pub(crate) depth: std::marker::PhantomData<D>,
}

impl<D: FocusReadDepth> Request for CurrentFocusRead<D> {
    type Reply = Hidden<Focused<D::Parsed>>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::CurrentFocus {
            hidden: D::HIDDEN,
            set: self.set.clone(),
            known: self.known.clone(),
            details: self.details,
            depth: D::DEPTH,
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        if D::HIDDEN == HiddenRoot::ReadAnyway && matches!(reply, Reply::HiddenDialog(_)) {
            return None;
        }
        Hidden::parse(reply, |reply| match reply {
            Reply::Candidate(answer) => D::parse(answer).map(Focused::Read),
            _ => None,
        })
    }

    fn decode_in_front(front: Front) -> Option<Self::Reply> {
        Some(Hidden::Read(Focused::InFront(front)))
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

/// The read after the user's input desktop returned, of the application
/// whose window `window` is in front (Table 8, column G).
pub(crate) fn returned(state: &SrState, window: RequestWindow) -> CurrentFocus {
    CurrentFocus {
        why: Why::Returned,
        window,
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
                // A plain focus has no reader that ended; asking hungwin
                // again would only find it hung once more.
                Committed::Read(focus) | Committed::Ended(focus)
                    if focus.kept.read_by == ReadBy::Plain =>
                {
                    return None;
                }
                Committed::Read(_) | Committed::Ended(_) => foreground?.window(),
                Committed::Nothing | Committed::Own => return None,
            }
        }
        Reader::Outpost(_) => {
            let outpost = instance.outpost_id();
            let super::versions::ReaderEnd::Retake = state.versions.outpost_ended(outpost) else {
                return None;
            };
            let focus = state.versions.focus()?;
            match foreground {
                Some(foreground) if foreground.process == focus.context.source => {
                    foreground.window()
                }
                Some(_) | None => focus.kept.asked,
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

/// The function of a current-focus entry. The show's entry reads only
/// while the postponed dialog it names is in front.
pub(crate) async fn run(mut cx: Cx<Entries>, entry: CurrentFocus) {
    if entry.why == Why::Shown && !super::shown::shown_due(&cx, entry.window.handle) {
        return;
    }
    read_current::<super::msaa_run::Full>(&mut cx, &entry).await;
}

/// Reads the current focus `read` names, in full, checking for a hidden
/// dialog or reading as it is by `D`, and judges its answer by its reason:
/// the one place that decides a current-focus read's outcomes. Its speech
/// carries `read`'s fence. A failed read clears Core's caret baseline.
pub(crate) async fn read_current<D: FocusReadDepth<Parsed = Box<FocusRead>>>(
    cx: &mut Cx<Entries>,
    read: &CurrentFocus,
) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let request = CurrentFocusRead::<D> {
        window: read.window,
        known: Vec::new(),
        details: crate::details::focus_details(&cx.facts()),
        set: crate::details::focus_set(&cx.facts(), FOCUS_SET),
        depth: std::marker::PhantomData,
    };
    let Some(outcome) = super::shown::ask(cx, request, due_us).await else {
        return;
    };
    let answer = match outcome {
        Outcome::Result(Focused::Read(answer)) => answer,
        Outcome::Result(Focused::InFront(front)) => {
            let hung = match read.why {
                Why::Startup => super::plain::HungRead::Startup,
                Why::Retake | Why::Shown => super::plain::HungRead::Retake,
                Why::Recovered => super::plain::HungRead::Recovered,
                Why::Returned => super::plain::HungRead::Returned,
            };
            super::plain::decide(cx, hung, read.window, &front, (read.trace_id, read.fence));
            return;
        }
        outcome => {
            // A re-take answered not there, or whose reader ended, tried
            // the focus and found it gone: the readers' last focus's fence
            // ends.
            if matches!(outcome, Outcome::NotThere | Outcome::ReaderEnded) {
                cx.facts_mut().versions.tried_and_refused();
            }
            cx.facts_mut().focus_read_failed();
            return;
        }
    };
    let Some(read_by) = read_by(&answer) else {
        return;
    };
    let duplicate = read.why == Why::Recovered
        && super::validity::at(
            cx,
            super::validity::Subject::Recovered(&answer),
            None,
            false,
        ) == super::validity::Validity::Duplicate;
    let origin = Origin::of_read(read.trace_id, read.window, read.fence);
    let executed = {
        let mut facts = cx.facts_mut();
        match read.why {
            Why::Startup | Why::Returned => {
                execute(&mut facts, &origin, &answer, &Known::default(), read_by)
            }
            Why::Recovered if duplicate => Executed::default(),
            Why::Retake | Why::Recovered | Why::Shown => {
                retake(&mut facts, &origin, &answer, read_by)
            }
        }
    };
    executed.deliver(cx, read.fence);
}

#[cfg(test)]
#[path = "retake_tests.rs"]
mod tests;
