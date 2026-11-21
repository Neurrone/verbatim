//! A UI Automation focus at the front (design, "Focus and foreground" and
//! "The focus answers"): judged on its tell's facts, then read in two
//! batches chained under one due time, the listener's `Read` and
//! `Ancestors` of the element it kept, of which Core makes the element's
//! route, and the application's outpost's `FocusRead` of the route,
//! answering the focus; then executed.

use std::iter;

use verbatim_model::batch::{AncestorsEnd, Ancestry, MAX_DEPTH, WindowOf};
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{
    FocusRead, KnownAncestor, NodeRead, NodeWindow, Route, RouteEnd, RouteStep,
};
use verbatim_model::tell::ElementToken;
use verbatim_model::{Backend, State};

use super::FOCUS_SET;
use super::execute::{Known, Origin, again, execute};
use super::retake::{Focused, read_by};
use super::shown::Check;
use super::validity::{self, How, Subject, Validity};
use super::versions::ReadBy;
use crate::batch::{Got, On};
use crate::entries::{Entries, UiaFocus, request_window};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, RequestWindow};

/// What the listener read of a kept element and its ancestors, as a route
/// the application's outpost finds its own copy by.
#[derive(Debug)]
pub(crate) struct Walked {
    /// The route, with what was read of each of its elements.
    pub(crate) route: Route,
    /// Its ancestors as read, nearest first.
    pub(crate) ancestors: Vec<NodeRead>,
}

/// The properties the listener reads of a kept element and each ancestor
/// for a route: `set`, and the runtime id, own window and whether it is
/// content that a route's step is made of.
fn route_set(set: &[PropertyId]) -> Vec<PropertyId> {
    let mut set = set.to_vec();
    for id in [
        PropertyId::RuntimeId,
        PropertyId::NativeWindow,
        PropertyId::IsContent,
    ] {
        if !set.contains(&id) {
            set.push(id);
        }
    }
    set
}

/// The route of a kept element read as `element`, with its ancestors as
/// `ancestry` read them, up to one of `known`; `None` for a read no
/// correct listener answers.
fn route_of(element: &NodeRead, ancestry: &Ancestry, known: &[KnownAncestor]) -> Option<Route> {
    let value = |read: &NodeRead, id| {
        read.values
            .answer(id)
            .and_then(|answer| answer.value.clone())
    };
    let step = |read: &NodeRead| {
        let Some(PropertyValue::RuntimeId(runtime_id)) = value(read, PropertyId::RuntimeId) else {
            return None;
        };
        Some(RouteStep {
            runtime_id,
            window: match value(read, PropertyId::NativeWindow) {
                Some(PropertyValue::Window(window)) => Some(window),
                _ => None,
            },
            content: value(read, PropertyId::IsContent) == Some(PropertyValue::Flag(true)),
        })
    };
    let end = match ancestry.end {
        AncestorsEnd::Met(place) => {
            RouteEnd::Known(known.get(usize::try_from(place).ok()?)?.clone())
        }
        AncestorsEnd::ProcessBoundary => RouteEnd::ProcessBoundary,
        AncestorsEnd::Top => RouteEnd::Root,
        AncestorsEnd::ContinueFrom(window) => RouteEnd::ContinueFrom(window),
        AncestorsEnd::PlainFrom(window) => RouteEnd::PlainFrom(window),
        // The listener's walk goes on by plain windows past its depth.
        AncestorsEnd::DepthLimit => return None,
    };
    let reads = || iter::once(element).chain(&ancestry.ancestors);
    Some(Route {
        window: element.window.handle,
        steps: reads().map(step).collect::<Option<_>>()?,
        end,
        read: reads().map(|read| read.values.clone()).collect(),
    })
}

/// Reads the element the listener of generation `listener` keeps behind
/// `token`, kept as read after it when `keep`, and its ancestors up to one
/// of `known`, by `set` with the description whatever the settings, as a
/// route; `window` its nearest window. The name read is the one read
/// afresh, and no focused state is added: a focus's caller makes both its
/// own ([`as_focus`]).
pub(crate) async fn walked(
    cx: &mut Cx<Entries>,
    (listener, token, keep): (u64, ElementToken, bool),
    (window, known): (RequestWindow, Vec<KnownAncestor>),
    due_us: u64,
) -> Outcome<Walked> {
    let set = route_set(&crate::details::description_kept(&cx.facts(), FOCUS_SET));
    let mut batch = cx.batch(|| ());
    let element = batch.read(On::kept(listener, token, keep), &set);
    let ancestry = batch.ancestors(
        On::kept(listener, token, keep),
        known.clone(),
        MAX_DEPTH,
        &set,
    );
    let request = batch
        .finish()
        .of(Reader::Listener, window)
        .judged(move |answers| {
            let (Got::Done(element), Got::Done(ancestry)) =
                (answers.get(element), answers.get(ancestry))
            else {
                return None;
            };
            Some(Walked {
                route: route_of(element, ancestry, &known)?,
                ancestors: ancestry.ancestors.clone(),
            })
        });
    cx.ask(request, due_us).await
}

/// What a completion of a focus's route found.
#[derive(Debug)]
enum Completed {
    /// Its window's top-level window is a hidden dialog.
    Hidden(NodeWindow),
    /// Not there: a hidden window whose role could not be read, or the
    /// element not found.
    Gone,
    /// The focus, read.
    Read(Box<FocusRead>),
}

/// Completes the focus whose route the listener read, `route`, by its
/// application's outpost: the hidden-dialog check on its window's
/// top-level window, then the focus read of the route unless it matched
/// (the request list's design, section 6, `CompleteUia`). `None` when a
/// hidden dialog postponed it.
async fn complete(
    cx: &mut Cx<Entries>,
    (window, route, known): (RequestWindow, Route, &Known),
    due_us: u64,
) -> Option<Outcome<Box<FocusRead>>> {
    let mut batch = cx.batch(|| ());
    let top = On::window(WindowOf::TopLevelOf(route.window), Backend::Msaa, false);
    let check = batch.test(top, super::shown::check());
    batch.when(!check.matched());
    let read = {
        let facts = cx.facts();
        batch.focus_read(
            On::route(route),
            known.sent.clone(),
            &crate::details::node_set(&facts, FOCUS_SET),
            crate::details::focus_details(&facts),
        )
    };
    let request = batch
        .finish()
        .of(Reader::Outpost(window.process), window)
        .judged(move |answers| {
            match super::shown::judge(&answers.get(check))? {
                Check::Dialog(window) => return Some(Completed::Hidden(window)),
                // A hidden window whose role could not be read: the read
                // that ran after it is not there (the design's section 6,
                // "Orders kept").
                Check::Gone => return Some(Completed::Gone),
                Check::Shown => {}
            }
            match answers.get(read) {
                Got::Done(read) => Some(Completed::Read(Box::new(read.clone()))),
                Got::NotThere(_) | Got::Failed => Some(Completed::Gone),
                Got::Refused(_) | Got::Skipped => None,
            }
        });
    Some(match cx.ask(request, due_us).await.split() {
        Ok(Completed::Hidden(dialog)) => {
            super::shown::postpone_dialog(cx, &dialog);
            return None;
        }
        Ok(Completed::Gone) => Outcome::NotThere,
        Ok(Completed::Read(read)) => Outcome::Result(read),
        Err(other) => other,
    })
}

/// `values`, a focus's element as the listener read it, as a focus
/// answers it: the name its event's cache had, `cached`
/// ([`with_cached_name`]), and the focused state.
fn as_focus(values: &mut PropertyValues, cached: &PropertyValues) {
    with_cached_name(values, cached);
    for answer in &mut values.0 {
        if let (PropertyId::States, Some(PropertyValue::States(states))) =
            (answer.id, &mut answer.value)
        {
            *states = states.with(State::Focused);
        }
    }
}

/// `values`, an element as the listener read it, with the name its event's
/// cache had, `cached`, in place of the name read afresh, as NVDA's object
/// for the event reads it.
pub(crate) fn with_cached_name(values: &mut PropertyValues, cached: &PropertyValues) {
    for answer in &mut values.0 {
        if answer.id == PropertyId::Name {
            answer.value = cached
                .answer(PropertyId::Name)
                .and_then(|cached| cached.value.clone());
        }
    }
}

/// The function of a UI Automation focus entry, or of a menu opening: a
/// menu opening on the committed focus executes it again; otherwise the
/// native-element test with the verdict, the overrides, acceptance, the
/// keyboard-focus answer and the duplicate test on its tell's facts; then
/// the two reads; then execution. Any outcome but a result ends it
/// unspoken and clears Core's caret baseline.
pub(crate) async fn run(mut cx: Cx<Entries>, focus: UiaFocus) {
    let cx = &mut cx;
    let focus = &focus;
    match validity::at(cx, Subject::Uia(focus), None, false) {
        Validity::Valid(How::Again) => {
            let said = again(&mut cx.facts_mut(), focus.tell.trace_id);
            for said in said {
                cx.emit(said);
            }
            return;
        }
        Validity::Valid(How::Read) => {}
        Validity::Duplicate | Validity::Refused(_) | Validity::NeedsRead => return,
    }
    // Valid, the focus's walk found its nearest window.
    let Some(window) = request_window(&focus.tell) else {
        return;
    };
    let due_us = cx.due_in(FOCUS_DUE_US);
    let walk = Known::of(&cx.facts(), None).sent;
    let kept = (focus.listener, focus.token, true);
    let mut route = match walked(cx, kept, (window, walk), due_us).await {
        Outcome::Result(walked) => walked.route,
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            return;
        }
    };
    if let Some(element) = route.read.first_mut() {
        as_focus(element, &focus.tell.cached);
    }
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(verbatim_model::request::Instance::outpost_id);
    let known = Known::of(&cx.facts(), generation);
    if let RouteEnd::Known(met) = &route.end
        && !known.sent.contains(met)
    {
        // The committed focus's line changed between the two reads (a
        // desktop switch committed the sleeping focus, or a focus Core
        // holds over it took its place), so the ancestor the walk met is
        // no longer in it and the chain cannot be completed against it.
        // The listener let the element go with its read, so the
        // application's focus is read afresh against the line as it now
        // is, as NVDA's walk compares with the focus line at the time.
        afresh(cx, focus, window, known, due_us).await;
        return;
    }
    // A hidden dialog postponed, answered before any registration: its
    // show reads the current focus.
    let Some(outcome) = complete(cx, (window, route, &known), due_us).await else {
        return;
    };
    let read = match outcome {
        Outcome::Result(read) => read,
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            // An outpost that ended took its registrations with it (Table
            // 10, column F).
            if !matches!(outcome, Outcome::ReaderEnded) {
                unexecuted(cx, window);
            }
            return;
        }
    };

    let read_by = ReadBy::Uia {
        runtime_id: focus.runtime_id.clone(),
        token: Some(focus.token),
    };
    super::narrow::judge_ahead(cx, &read);
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    if !executed.committed {
        unexecuted(cx, window);
    }
    let fence = crate::entries::front_fence(cx);
    executed.deliver(cx, fence);
}

/// The origin of `focus` read on `window`: its tell's, with Alt as Core's
/// key state had it at the tell's arrival.
fn origin(
    focus: &UiaFocus,
    window: RequestWindow,
    foreground: Option<crate::event_queue::Foreground>,
) -> Origin {
    let mut origin = Origin::of_tell(&focus.tell, window, foreground, focus.fence);
    if let Some(facts) = origin.window.as_mut() {
        facts.alt_held = focus.alt_down;
    }
    origin
}

/// The read of `focus` afresh, when the ancestor its listener read met
/// left the committed focus's line before the completion: the current
/// focus of `window`'s application, read by its outpost against the
/// ancestors Core now knows, `known`, then executed as the tell's focus.
/// Any outcome but a result ends it unspoken and clears Core's caret
/// baseline, as a failed read of the tell's element does.
async fn afresh(
    cx: &mut Cx<Entries>,
    focus: &UiaFocus,
    window: RequestWindow,
    known: Known,
    due_us: u64,
) {
    let depth = (known.sent.clone(), super::retake::Depth::Full);
    let Some(outcome) = super::retake::current_full(cx, window, depth, due_us).await else {
        return;
    };
    let read = match outcome {
        Outcome::Result(Focused::Read(read)) => read,
        Outcome::Result(Focused::InFront(front)) => {
            let trace = (focus.tell.trace_id, focus.fence);
            super::plain::decide(cx, super::plain::HungRead::Afresh, window, &front, trace);
            return;
        }
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            return;
        }
    };
    let Some(read_by) = read_by(&read) else {
        return;
    };
    let origin = origin(focus, window, cx.foreground());
    let executed = execute(&mut cx.facts_mut(), &origin, &read, &known, read_by);
    let fence = crate::entries::front_fence(cx);
    executed.deliver(cx, fence);
}

/// A completion the outpost of `window`'s application answered, or Core
/// stopped waiting for, ended without its focus executed: a `Follow`
/// naming what Core names there removes the completion's registration
/// (Table 10, column C).
fn unexecuted(cx: &mut Cx<Entries>, window: RequestWindow) {
    let request = cx.facts().follows.unexecuted(window.process, window);
    super::follow::send(cx, vec![request]);
}
