//! A UI Automation selection at its turn
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and
//! objects", its announcements).

use verbatim_model::batch::{MAX_DEPTH, MAX_NODES, Relation, View, Walk};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue, RuntimeId};
use verbatim_model::request::{NodeRead, Route, RouteEnd};
use verbatim_model::{Backend, NodeId, PropertyChange, Role, State, StateSet, WindowHandle};

use super::{Announcement, Kept, native, text};
use crate::batch::{Got, On};
use crate::changes::AtArrival;
use crate::entries::Entries;
use crate::entries::window_facts;
use crate::event_queue::{Cx, OTHER_DUE_US, Outcome, Reader, RequestWindow};
use crate::focus::FOCUS_SET;
use crate::focus::acceptance;
use crate::focus::execute::Known;
use crate::focus::uia_focus::walked;
use crate::focus::versions::Focus as CommittedFocus;

/// What the item's outpost found of a selected item, judged at its
/// decode.
#[derive(Debug)]
enum Item {
    /// The item, and whether the committed focus controls it or what it
    /// is inside of.
    Read(NodeRead, bool),
    /// The item, or the focus's controller-for relation, could not be
    /// read: nothing is said, as NVDA's COM error ends its
    /// `event_selection`.
    Gone,
}

/// The item the listener read by `route`, read by its outpost, with
/// whether the committed focus `focus` controls it or a subtree holding
/// it, found by its runtime id `runtime_id` (the request list's design,
/// section 6, `CompleteUia` for a selection's item).
async fn item(
    cx: &mut Cx<Entries>,
    (window, route, focus, runtime_id): (RequestWindow, Route, NodeId, &RuntimeId),
    due_us: u64,
) -> Outcome<Item> {
    let mut batch = cx.batch(|| ());
    let item = batch.read(
        On::route(route),
        &crate::details::node_set(&cx.facts(), FOCUS_SET),
    );
    let controlled = batch.related(focus, Relation::ControllerFor, &[]);
    let found = batch.descendants(
        controlled.every(),
        Walk {
            depth: MAX_DEPTH,
            view: View::Raw,
            matcher: Some(Matcher::Is(
                PropertyId::RuntimeId,
                PropertyValue::RuntimeId(runtime_id.clone()),
            )),
            prune: None,
            include_subject: true,
            limit: 1,
            nodes: MAX_NODES,
            set: Vec::new(),
        },
    );
    let request = batch
        .finish()
        .of(Reader::Outpost(window.process), window)
        .judged(move |answers| {
            let item = match answers.get(item) {
                Got::Done(item) => item.clone(),
                Got::NotThere(_) | Got::Failed => return Some(Item::Gone),
                Got::Refused(_) | Got::Skipped => return None,
            };
            match (answers.get(controlled), answers.get(found)) {
                (Got::Failed, _) => Some(Item::Gone),
                (_, Got::Done(found)) => Some(Item::Read(item, !found.nodes.is_empty())),
                // No controller-for relation: nothing to walk.
                (Got::Done(_) | Got::NotThere(_), Got::Skipped) => Some(Item::Read(item, false)),
                _ => None,
            }
        });
    cx.ask(request, due_us).await
}

/// The function of a UI Automation selection entry, its element `kept` by
/// the listener, the committed focus or not as it joined (`at_arrival`).
pub(crate) async fn run(
    mut cx: Cx<Entries>,
    announcement: Announcement,
    kept: Kept,
    at_arrival: AtArrival,
) {
    let tell = &announcement.tell;
    let Some(window) = announcement.window() else {
        return;
    };
    if !native(tell) {
        return;
    }
    let Some(committed) = cx.facts().versions.live().cloned() else {
        return;
    };
    let due_us = cx.due_in(OTHER_DUE_US);
    let list = if in_combo_box_list_window(tell, &committed) {
        combo_box_list(&mut cx, &committed, due_us).await
    } else {
        None
    };
    // The Windows Forms combo box's request takes its list's events
    // whatever the foreground.
    if acceptance::accepted(tell).is_err() && list.is_none() {
        return;
    }
    selected(
        &mut cx,
        &announcement,
        (&kept, at_arrival, window, &committed),
        due_us,
    )
    .await;
    if list == Some(window.handle) && cached_role(tell) == Some(Role::ListItem) {
        combo_box_value(&mut cx, tell.trace_id, &committed, due_us).await;
    }
}

/// The selection's own handling (`event_selection`): the state change of
/// the committed focus or a kept ancestor, an item the focus controls
/// spoken as a focus, or the value change of the combo box that is the
/// item's parent.
async fn selected(
    cx: &mut Cx<Entries>,
    announcement: &Announcement,
    (kept, at_arrival, window, committed): (
        &super::Kept,
        AtArrival,
        RequestWindow,
        &CommittedFocus,
    ),
    due_us: u64,
) {
    let tell = &announcement.tell;
    let trace_id = tell.trace_id;
    // A selection of the committed focus or a kept ancestor is its state
    // change, judged on the states its tell carries, when its element was
    // on the line at its arrival.
    if let Some(node) = crate::changes::node_of(committed, &kept.runtime_id) {
        if at_arrival == AtArrival::OffLine {
            return;
        }
        let Some(states) = cached_states(tell) else {
            return;
        };
        crate::changes::speak_change(cx, trace_id, node, &PropertyChange::States(states), None);
        return;
    }
    let walk = Known::of(&cx.facts(), None).sent;
    let kept_element = (kept.listener, kept.token, false);
    let Outcome::Result(walked) = walked(cx, kept_element, (window, walk), due_us).await else {
        return;
    };
    let generation = cx
        .instance(Reader::Outpost(window.process))
        .map(verbatim_model::request::Instance::outpost_id);
    let known = Known::of(&cx.facts(), generation);
    let met = match &walked.route.end {
        RouteEnd::Known(met) => match known.sent.iter().position(|known| known == met) {
            Some(place) => Some(place),
            // The committed focus changed between the two reads.
            None => return,
        },
        _ => None,
    };
    // The item's parent is the committed focus: the listener's walk up met
    // the focus itself, the first of what Core knows, before any other
    // ancestor.
    let parent_is_focus =
        walked.ancestors.is_empty() && met.and_then(|met| known.places.get(met)) == Some(&0);
    let focus = committed.context.snapshot.id;
    let asked = (window, walked.route, focus, &kept.runtime_id);
    let Outcome::Result(Item::Read(item, controlled)) = item(cx, asked, due_us).await else {
        return;
    };
    // The focus committed meanwhile is no longer the one asked about.
    if cx
        .facts()
        .versions
        .live()
        .is_none_or(|now| now.context.snapshot.id != focus)
    {
        return;
    }
    if controlled {
        crate::changes::controlled_selection(cx, trace_id, focus, (item, Backend::Uia));
    } else if parent_is_focus && !focused(&item) && read_role(&item) == Some(Role::ListItem) {
        if committed.kept.value_from_selection {
            combo_box_value(cx, trace_id, committed, due_us).await;
        } else {
            core_window_value(cx, trace_id, committed, due_us).await;
        }
    }
}
/// Whether `tell`, a selection, is from a `ComboLBox` window of the
/// committed focus's process while the focus is a combo box: the events a
/// Windows Forms combo box asks for, Core asking the focus whether it is
/// one ([`combo_box_list`]).
fn in_combo_box_list_window(
    tell: &verbatim_model::tell::Event,
    committed: &CommittedFocus,
) -> bool {
    committed.context.snapshot.role == Role::ComboBox
        && tell.process == committed.context.source
        && window_facts(tell).is_some_and(|facts| facts.window.class == "ComboLBox")
}

/// The window of the committed focus's drop-down list when the focus is a
/// classic .NET Framework Windows Forms combo box, read by its outpost
/// (`ComboBoxList`); `None` for any other focus, or when it is not
/// answered.
async fn combo_box_list(
    cx: &mut Cx<Entries>,
    combo_box: &CommittedFocus,
    due_us: u64,
) -> Option<WindowHandle> {
    let asked = combo_box.kept.asked;
    let read = (asked.process, asked, combo_box.context.snapshot.id);
    let Outcome::Result(values) =
        crate::navigator::read_values(cx, read, &[PropertyId::ComboBoxList], due_us).await
    else {
        return None;
    };
    match values
        .answer(PropertyId::ComboBoxList)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Window(list)) => Some(*list),
        _ => None,
    }
}

/// The value change of `combo_box`, the committed focus, whose selected
/// item changed, when it has the `Value` pattern and its window's class
/// begins with `Windows.UI.Core` (NVDA issue 6337): the three read in one
/// request, its value spoken as a value change.
async fn core_window_value(
    cx: &mut Cx<Entries>,
    trace_id: verbatim_model::TraceId,
    combo_box: &CommittedFocus,
    due_us: u64,
) {
    let asked = combo_box.kept.asked;
    let read = (asked.process, asked, combo_box.context.snapshot.id);
    let Outcome::Result(values) = crate::navigator::read_values(
        cx,
        read,
        &[
            PropertyId::HasValuePattern,
            PropertyId::WindowClass,
            PropertyId::Value,
        ],
        due_us,
    )
    .await
    else {
        return;
    };
    let value_of = |id| values.answer(id).and_then(|answer| answer.value.as_ref());
    let has_value = matches!(
        value_of(PropertyId::HasValuePattern),
        Some(PropertyValue::Flag(true))
    );
    let core_window =
        text(value_of(PropertyId::WindowClass)).is_some_and(|class| class.starts_with(CORE_WINDOW));
    if !(has_value && core_window) {
        return;
    }
    let value = text(value_of(PropertyId::Value)).map(str::to_owned);
    crate::changes::speak_change(
        cx,
        trace_id,
        combo_box.context.snapshot.id,
        &PropertyChange::Value(value),
        None,
    );
}

/// The class prefix of a Windows Runtime core window.
const CORE_WINDOW: &str = "Windows.UI.Core";

/// The value change of `combo_box`, the committed focus, whose selected
/// item changed: its value read live and spoken as a value change.
async fn combo_box_value(
    cx: &mut Cx<Entries>,
    trace_id: verbatim_model::TraceId,
    combo_box: &CommittedFocus,
    due_us: u64,
) {
    let asked = combo_box.kept.asked;
    let read = (asked.process, asked, combo_box.context.snapshot.id);
    let Outcome::Result(values) =
        crate::navigator::read_values(cx, read, &[PropertyId::Value], due_us).await
    else {
        return;
    };
    let value = text(
        values
            .answer(PropertyId::Value)
            .and_then(|answer| answer.value.as_ref()),
    )
    .map(str::to_owned);
    crate::changes::speak_change(
        cx,
        trace_id,
        combo_box.context.snapshot.id,
        &PropertyChange::Value(value),
        None,
    );
}

/// The role a selection's tell carries from its event's cache.
fn cached_role(tell: &verbatim_model::tell::Event) -> Option<Role> {
    match tell
        .cached
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
}

/// The item's role, as read.
fn read_role(item: &NodeRead) -> Option<Role> {
    match item
        .values
        .answer(PropertyId::Role)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Role(role)) => Some(*role),
        _ => None,
    }
}

/// The states a selection's tell carries from its event's cache.
fn cached_states(tell: &verbatim_model::tell::Event) -> Option<StateSet> {
    match tell
        .cached
        .answer(PropertyId::States)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(verbatim_model::property::PropertyValue::States(states)) => Some(*states),
        _ => None,
    }
}

/// Whether the item has the keyboard focus, as read.
fn focused(item: &NodeRead) -> bool {
    matches!(
        item.values
            .answer(PropertyId::States)
            .and_then(|answer| answer.value.as_ref()),
        Some(verbatim_model::property::PropertyValue::States(states)) if states.contains(State::Focused)
    )
}
