//! `Follow` (`docs/design/readers-tell-core-asks-build1.md`, "Changes on
//! objects" and Table 10): Core names the committed focus's node, its
//! event group and any extra object, or nothing, and the outpost keeps
//! that node's registration, adding it when absent, and removes every
//! other. The completion of a UI Automation focus adds the new node's
//! registration while keeping the one `Follow` named, so the old focus
//! stays followed until Core commits the new one.
//!
//! What `Follow` names is taken when the worker takes the request, before
//! the registering thread's add, and is the outpost's only record of which
//! terminal is followed. The registering thread makes every registration
//! (`verbatim_uia::Registration`); the worker hands it each move and waits
//! for it.

#![forbid(unsafe_code)]

use std::time::Instant;

use verbatim_model::NodeId;
use verbatim_model::request::{Follow, FollowGroup, Outcome, Reply};
use verbatim_uia::{Group, Scope};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

use super::read::Client;
use super::{Context, RequestContext};

/// What the newest `Follow` named, with the elements its registration was
/// given.
#[derive(Clone)]
pub(super) struct Named {
    /// The node followed.
    pub(super) node: NodeId,
    /// Its element.
    element: AgileReference<IUIAutomationElement>,
    /// The group its element was given.
    group: Group,
    /// The extra object followed for its value changes, if any.
    extra: Option<AgileReference<IUIAutomationElement>>,
    /// When the node is a terminal, whether its window is a console's.
    pub(super) terminal: Option<bool>,
}

/// The registration group `group` gives its element: a console host's
/// terminal takes the extended group without its text and caret events,
/// which its console's own events tell.
fn uia_group(group: FollowGroup) -> Group {
    match group {
        FollowGroup::Base => Group::Base,
        FollowGroup::Extended | FollowGroup::Terminal { console: false } => Group::Extended,
        FollowGroup::Terminal { console: true } => Group::ExtendedWithoutTextEvents,
    }
}

/// Runs `follow` on the worker: records what it names, moves the
/// registration on the registering thread and waits for the move. A node
/// the outpost no longer holds an element for is answered not there, with
/// every registration removed.
pub(super) fn follow(
    context: &RequestContext<'_>,
    client: &mut Client,
    follow: &Follow,
) -> Outcome {
    let Some(registration) = context.focus_events.get() else {
        tracing::debug!("Follow with no registration to move");
        return Outcome::Ended;
    };
    let named = follow.node.as_ref().and_then(|followed| {
        let element = context.store.element_of(followed.node)?.reference();
        // The extra object, the slider a focused thumb is in, is an
        // ancestor the completion minted, whose element is found again by
        // its route when the outpost keeps none.
        let extra = followed
            .extra
            .and_then(|extra| super::read::element_reference(context, client, extra));
        Some(Named {
            node: followed.node,
            element,
            group: uia_group(followed.group),
            extra,
            terminal: match followed.group {
                FollowGroup::Terminal { console } => Some(console),
                FollowGroup::Base | FollowGroup::Extended => None,
            },
        })
    });
    let not_there = follow.node.is_some() && named.is_none();
    if named
        .as_ref()
        .is_some_and(|named| named.terminal == Some(true))
    {
        context.hook_console();
    }
    context.named.lock().clone_from(&named);
    let started = Instant::now();
    let moved = named.as_ref().map(|named| (named.node, named.group));
    match named {
        Some(named) => registration.retarget_with(
            Scope::Elements(vec![named.element]),
            named.group,
            named
                .extra
                .into_iter()
                .map(|extra| (extra, Group::Value))
                .collect(),
        ),
        None => registration.retarget(Scope::Nothing),
    }
    registration.settle();
    log_move("Follow", moved, started);
    if not_there {
        Outcome::NotThere
    } else {
        Outcome::Result(Reply::Followed)
    }
}

/// Adds the registration of a focus's `element`, node `node`, keeping
/// what `Follow` named, and waits until it is in: the completion of a UI
/// Automation focus, which registers before it reads the node's states.
/// Core alone decides a node's group, so a node other than the one
/// `Follow` names is given the base group, and the node `Follow` names
/// keeps the group Core named, with its extra object: a repeated focus or
/// a re-take on a followed terminal leaves its text changes coming.
pub(super) fn add_focus(
    context: &Context,
    (node, element): (NodeId, AgileReference<IUIAutomationElement>),
) {
    let Some(registration) = context.focus_events.get() else {
        return;
    };
    let named = context.named.lock().clone();
    let (group, kept) = match named {
        Some(named) if named.node == node => (
            named.group,
            named
                .extra
                .map(|extra| (extra, Group::Value))
                .into_iter()
                .collect(),
        ),
        named => (
            Group::Base,
            named
                .into_iter()
                .flat_map(|named| {
                    std::iter::once((named.element, named.group))
                        .chain(named.extra.map(|extra| (extra, Group::Value)))
                })
                .collect(),
        ),
    };
    let started = Instant::now();
    registration.retarget_with(Scope::Elements(vec![element]), group, kept);
    registration.settle();
    log_move("a focus's completion", Some((node, group)), started);
}

/// Logs one move of the focus registration, made for `by` and settled,
/// with the node given its group, or `None` when the move names nothing,
/// and how long the worker waited for it from `started`: the add on the
/// new element, then the removals.
fn log_move(by: &str, moved: Option<(NodeId, Group)>, started: Instant) {
    tracing::debug!(
        by,
        ?moved,
        elapsed_us = started.elapsed().as_micros(),
        "the focus registration moved"
    );
}

/// Window `hwnd` was destroyed: when the node `Follow` names is in it, its
/// registration is dropped with the window's nodes, and nothing is named
/// (Table 10, column E).
pub(super) fn window_destroyed(context: &Context, hwnd: isize, gone: &[NodeId]) {
    let named = context.named.lock().clone();
    let Some(named) = named else {
        return;
    };
    let in_window = gone.contains(&named.node)
        || context.store.nearest_window_of(named.node) == Some(hwnd)
        || matches!(context.store.runtime_id_of(named.node).as_deref(), Some([42, own]) if isize::try_from(*own).ok() == Some(hwnd));
    if !in_window {
        return;
    }
    *context.named.lock() = None;
    if let Some(registration) = context.focus_events.get() {
        registration.retarget(Scope::Nothing);
    }
}

/// The terminal `Follow` names, with whether its window is a console's.
pub(super) fn terminal(context: &Context) -> Option<(NodeId, bool)> {
    context
        .named
        .lock()
        .as_ref()
        .and_then(|named| named.terminal.map(|console| (named.node, console)))
}

/// The node Core's newest `Follow` names: the committed focus's, when it
/// is this outpost's.
pub(super) fn node(context: &Context) -> Option<NodeId> {
    context.named.lock().as_ref().map(|named| named.node)
}
