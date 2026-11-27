//! The reads of an application's current focus that Core makes itself
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", the committed focus, and "Core's requests", the recovery set): the focus at
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
use verbatim_model::batch::WindowOf;
use verbatim_model::request::{FocusRead, Front, KnownAncestor, NodeWindow};
use verbatim_model::{Backend, TraceId};

use super::FOCUS_SET;
use super::execute::{Executed, Known, Origin, execute, retake};
use super::shown::Check;
use super::validity::{Answered, CandidateFacts, Unread};
use super::versions::{Committed, HeldBy, ReadBy};
use crate::batch::{Answers, Got, On, Ref, kind};
use crate::entries::Entries;
use crate::event_queue::{Cx, FOCUS_DUE_US, Foreground, Instance, Outcome, Reader, RequestWindow};
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

/// How far a read of an application's current focus goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Depth {
    /// In full, after the hidden-dialog check.
    Full,
    /// In full as it is, hidden or not: the read at a press while a dialog
    /// is postponed (`super::shown`).
    AsItIs,
    /// After the hidden-dialog check, its focused state, and in full only
    /// when focused: the check after a menu end, NVDA's fake focus.
    FullIfFocused,
}

/// An application's current focus as its read answers it: read to its
/// depth, or, when the shell answered the read unsent, its window hung or
/// no window in front, what the shell read in front, which the read's
/// entry judges (`super::plain::decide`).
#[derive(Debug)]
pub(crate) enum Focused<P> {
    /// The read, to its depth.
    Read(P),
    /// This is in front.
    InFront(Front),
}

/// What a read of the current focus found, judged at its decode, its
/// read made `T` by the depth's rule.
#[derive(Debug)]
enum Current<T> {
    /// The shell answered it unsent with what is in front.
    InFront(Front),
    /// The focused window's top-level window is a hidden dialog.
    Hidden(NodeWindow),
    /// No focus could be read.
    Gone,
    /// Read to its depth's rule.
    Read(T),
}

/// The steps of a current-focus batch.
struct Steps<B> {
    hidden: Option<Ref<B, kind::Test>>,
    facts: Option<Ref<B, kind::Test>>,
    read: Ref<B, kind::FocusRead>,
}

/// `answers` to a current-focus batch of `steps`, judged, a focus read
/// made `T` by `read` with its MSAA focused state when the batch read it,
/// and one found unfocused by `unfocused`: `None` for an answer no correct
/// outpost gives.
fn judge<B, T>(
    answers: &Answers<B>,
    steps: &Steps<B>,
    (read, unfocused): Rule<T>,
) -> Option<Current<T>> {
    if let Some(front) = answers.in_front() {
        return Some(Current::InFront(front.clone()));
    }
    if let Some(check) = steps.hidden {
        match super::shown::judge(&answers.get(check))? {
            Check::Dialog(window) => return Some(Current::Hidden(window)),
            Check::Gone => return Some(Current::Gone),
            Check::Shown => {}
        }
    }
    let focused = match steps.facts.map(|test| answers.get(test)) {
        None => None,
        Some(Got::Done(tested)) if tested.matched => return unfocused().map(Current::Read),
        Some(Got::Done(tested)) => super::msaa_run::focused_within(&tested.node.values),
        Some(Got::NotThere(_) | Got::Failed) => return Some(Current::Gone),
        Some(Got::Refused(_) | Got::Skipped) => return None,
    };
    match answers.get(steps.read) {
        Got::Done(focus) => Some(Current::Read(read(focused, Box::new(focus.clone())))),
        Got::NotThere(_) | Got::Failed => Some(Current::Gone),
        Got::Refused(_) | Got::Skipped => None,
    }
}

/// How a depth makes a current-focus read its answer: a read with its MSAA
/// focused state when read, and a focus found unfocused, `None` where the
/// depth reads no focused state.
type Rule<T> = (fn(Option<bool>, Box<FocusRead>) -> T, fn() -> Option<T>);

/// Reads the current focus of `window`'s application to `depth`, its
/// ancestors up to one of `known`, its answer made by `rule`: `None` when
/// a hidden dialog postponed it (`super::shown`). The window with the
/// keyboard focus, and its top-level window for the hidden-dialog check,
/// are the outpost's.
async fn current<T: std::fmt::Debug + 'static>(
    cx: &mut Cx<Entries>,
    window: RequestWindow,
    (known, depth, rule): (Vec<KnownAncestor>, Depth, Rule<T>),
    due_us: u64,
) -> Option<Outcome<Focused<T>>> {
    let mut batch = cx.batch(|| ());
    let hidden = (depth != Depth::AsItIs).then(|| {
        let window = On::window(WindowOf::Focused, Backend::Msaa, false);
        let check = batch.test(window, super::shown::check());
        batch.when(!check.matched());
        check
    });
    let facts = (depth == Depth::FullIfFocused).then(|| {
        let test = batch.test(On::focus(), super::msaa_run::unfocused());
        batch.when(!test.matched());
        test
    });
    let read = {
        let facts = cx.facts();
        batch.focus_read(
            On::focus(),
            known,
            &crate::details::focus_set(&facts, FOCUS_SET),
            crate::details::focus_details(&facts),
        )
    };
    let steps = Steps {
        hidden,
        facts,
        read,
    };
    let request = batch
        .finish()
        .of(Reader::Outpost(window.process), window)
        .judged(move |answers| judge(answers, &steps, rule));
    Some(match cx.ask(request, due_us).await.split() {
        Ok(Current::InFront(front)) => Outcome::Result(Focused::InFront(front)),
        Ok(Current::Hidden(dialog)) => {
            super::shown::postpone_dialog(cx, &dialog);
            return None;
        }
        Ok(Current::Gone) => Outcome::NotThere,
        Ok(Current::Read(read)) => Outcome::Result(Focused::Read(read)),
        Err(other) => other,
    })
}

/// Reads the current focus of `window`'s application with its focused
/// state, in full only when focused, its ancestors up to one of `known`:
/// the check after a menu end ([`current`]).
pub(crate) async fn current_if_focused(
    cx: &mut Cx<Entries>,
    window: RequestWindow,
    known: Vec<KnownAncestor>,
    due_us: u64,
) -> Option<Outcome<Focused<Answered>>> {
    let read = |focused, read| Answered::Read {
        facts: CandidateFacts::Found {
            focused,
            role: None,
        },
        read,
    };
    let unfocused = || Some(Answered::Unread(Unread::NotFocused));
    current(
        cx,
        window,
        (known, Depth::FullIfFocused, (read, unfocused)),
        due_us,
    )
    .await
}

/// Reads the current focus of `window`'s application in full, checking
/// for a hidden dialog or as it is by `depth`, its ancestors up to one of
/// `known` ([`current`]).
pub(crate) async fn current_full(
    cx: &mut Cx<Entries>,
    window: RequestWindow,
    (known, depth): (Vec<KnownAncestor>, Depth),
    due_us: u64,
) -> Option<Outcome<Focused<Box<FocusRead>>>> {
    let read = |_, read| read;
    current(cx, window, (known, depth, (read, || None)), due_us).await
}

/// How a current-focus answer is named: by the MSAA ids it carries, else
/// by its runtime id, which the listener holds behind no token; `None`
/// for an answer with neither, a reader's fault.
pub(crate) fn read_by(read: &FocusRead) -> Option<HeldBy> {
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
/// host for a console window as the report names it; none when no process
/// could be read as serving the window, since no reader can be asked.
pub(crate) fn startup(state: &SrState, foreground: &Foreground) -> Option<CurrentFocus> {
    Some(CurrentFocus {
        why: Why::Startup,
        window: foreground.window()?,
        trace_id: state.queue_trace.0,
        fence: crate::keys::entry::Fence::default(),
    })
}

/// The read after the user's input desktop returned, of the application
/// whose window `window` is in front
/// (`books/dev/src/reference/crates/verbatim-core.md`, "The secure desktop and the lock state").
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

/// `instance` ended (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", the committed focus): the listener's tokens die with
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
                // A window in front no process is read as serving: no
                // reader is asked.
                Committed::Read(_) | Committed::Ended(_) => foreground?.window()?,
                Committed::Nothing | Committed::Own => return None,
            }
        }
        Reader::Outpost(_) => {
            let outpost = instance.outpost_id();
            let super::versions::ReaderEnd::Retake = state.versions.outpost_ended(outpost) else {
                return None;
            };
            let focus = state.versions.focus()?;
            match foreground.and_then(|foreground| foreground.window()) {
                Some(front) if front.process == focus.context.source => front,
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
/// while the window it names is the postponed dialog.
pub(crate) async fn run(mut cx: Cx<Entries>, entry: CurrentFocus) {
    if entry.why == Why::Shown && !super::shown::shown_due(&cx, entry.window.handle) {
        return;
    }
    read_current(&mut cx, &entry, Depth::Full).await;
}

/// Reads the current focus `read` names, in full, checking for a hidden
/// dialog or reading as it is by `depth`, and judges its answer by its
/// reason: the one place that decides a current-focus read's outcomes. Its
/// speech carries `read`'s fence. A failed read clears Core's caret
/// baseline.
pub(crate) async fn read_current(cx: &mut Cx<Entries>, read: &CurrentFocus, depth: Depth) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let Some(outcome) = current_full(cx, read.window, (Vec::new(), depth), due_us).await else {
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
            super::validity::failed_read(&mut cx.facts_mut(), &outcome);
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
    let now = cx.now();
    let executed = {
        let mut facts = cx.facts_mut();
        match read.why {
            Why::Startup | Why::Returned => execute(
                &mut facts,
                &origin,
                &answer,
                &Known::default(),
                (&read_by, now),
            ),
            Why::Recovered if duplicate => Executed::default(),
            Why::Retake | Why::Recovered | Why::Shown => {
                retake(&mut facts, &origin, &answer, (&read_by, now))
            }
        }
    };
    executed.deliver(cx, read.fence);
}

#[cfg(test)]
#[path = "retake_tests.rs"]
mod tests;
